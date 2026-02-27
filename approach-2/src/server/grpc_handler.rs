use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use futures_util::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, info};

use crate::auth::{AuthContext, AuthService};
use crate::config::{ClientConfig, KeepaliveConfig};
use crate::error::PropellerError;
use crate::metrics::Metrics;
use crate::push::models::{
    Device, GetActiveDevicesRequest, SendEventToClientDeviceRequest, SendEventToClientRequest,
    SendEventToTopicRequest,
};
use crate::push::service::PushService;

pub mod proto {
    tonic::include_proto!("push.v1");
}

use proto::push_service_server::PushService as PushServiceTrait;
use proto::{
    ChannelRequest, ChannelResponse, ConnectAck, GetClientActiveDevicesRequest,
    GetClientActiveDevicesResponse, ResponseStatus, SendEventToClientChannelRequest,
    SendEventToClientChannelResponse, SendEventToClientDeviceChannelRequest,
    SendEventToClientDeviceChannelResponse, SendEventToTopicRequest as ProtoSendEventToTopicRequest,
    SendEventToTopicResponse, SendEventToTopicsRequest, SendEventToTopicsResponse,
};

pub struct GrpcPushHandler {
    push_service: Arc<PushService>,
    auth: Arc<AuthService>,
    client_config: ClientConfig,
    keepalive: KeepaliveConfig,
    metrics: Arc<Metrics>,
}

impl GrpcPushHandler {
    pub fn new(
        push_service: Arc<PushService>,
        auth: Arc<AuthService>,
        client_config: ClientConfig,
        keepalive: KeepaliveConfig,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self {
            push_service,
            auth,
            client_config,
            keepalive,
            metrics,
        }
    }

    fn extract_device(&self, metadata: &tonic::metadata::MetadataMap) -> Result<Option<Device>, Status> {
        if !self.client_config.enable_device_support {
            return Ok(None);
        }

        let device_id = metadata
            .get(&self.client_config.device_header)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();
        if device_id.is_empty() {
            return Ok(None);
        }

        let mut attrs = HashMap::new();
        for key in &self.client_config.device_attribute_headers {
            if let Some(val) = metadata.get(key).and_then(|v| v.to_str().ok()) {
                attrs.insert(key.clone(), val.to_string());
            }
        }

        Ok(Some(Device {
            id: device_id,
            attributes: attrs,
        }))
    }

    fn auth_client(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<(AuthContext, bool), Status> {
        self.auth
            .authenticate_grpc(metadata, &self.client_config)
            .map_err(Status::from)
    }

    fn authorize_publisher(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<AuthContext, Status> {
        self.auth
            .authorize_api_key_grpc(metadata)
            .map_err(Status::from)?;
        let (ctx, _) = self.auth_client(metadata)?;
        Ok(ctx)
    }

    fn ok_status() -> ResponseStatus {
        ResponseStatus {
            success: true,
            error_code: String::new(),
            message: HashMap::new(),
            error_type: String::new(),
        }
    }

    fn err_status(err: &str) -> ResponseStatus {
        let mut msg = HashMap::new();
        msg.insert("message".to_string(), err.to_string());
        ResponseStatus {
            success: false,
            error_code: String::new(),
            message: msg,
            error_type: String::new(),
        }
    }

    fn extract_event(event: &Option<proto::Event>) -> (Vec<u8>, String) {
        match event {
            Some(e) => (e.data.clone(), e.name.clone()),
            None => (Vec::new(), String::new()),
        }
    }
}

type ChannelStream = Pin<Box<dyn Stream<Item = Result<ChannelResponse, Status>> + Send>>;

#[tonic::async_trait]
impl PushServiceTrait for GrpcPushHandler {
    type ChannelStream = ChannelStream;

    async fn channel(
        &self,
        request: Request<Streaming<ChannelRequest>>,
    ) -> Result<Response<Self::ChannelStream>, Status> {
        let metadata = request.metadata().clone();
        let (auth, legacy_mode) = match self.auth_client(&metadata) {
            Ok(v) => {
                self.metrics.auth_success_total.fetch_add(1, Ordering::Relaxed);
                if v.1 {
                    self.metrics
                        .legacy_header_auth_total
                        .fetch_add(1, Ordering::Relaxed);
                }
                v
            }
            Err(status) => {
                self.metrics.auth_failure_total.fetch_add(1, Ordering::Relaxed);
                return Err(status);
            }
        };
        let device = self.extract_device(&metadata)?;
        let mut inbound = request.into_inner();

        if legacy_mode {
            info!(client_id = auth.client_id, "grpc channel using legacy header auth");
        }
        info!(tenant_id = auth.tenant_id, client_id = auth.client_id, "client connecting");

        let mut sub = self
            .push_service
            .subscribe_client(&auth.tenant_id, &auth.client_id, device.as_ref())
            .await
            .map_err(Status::from)?;

        let (resp_tx, resp_rx) = mpsc::channel(128);

        let ack = ChannelResponse {
            response: Some(proto::channel_response::Response::ConnectAck(ConnectAck {
                status: Some(Self::ok_status()),
            })),
        };
        if resp_tx.send(Ok(ack)).await.is_err() {
            return Err(Status::internal("failed to send connect ack"));
        }

        let push_service = self.push_service.clone();
        let metrics = self.metrics.clone();
        let keepalive = self.keepalive.clone();
        let tenant_owned = auth.tenant_id.clone();
        let client_id_owned = auth.client_id.clone();
        let device_owned = device.clone();

        tokio::spawn(async move {
            let mut keepalive_tick = tokio::time::interval(Duration::from_secs(1));
            keepalive_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut last_inbound_at = Instant::now();
            loop {
                tokio::select! {
                    client_msg = inbound.message() => {
                        match client_msg {
                            Ok(Some(req)) => {
                                last_inbound_at = Instant::now();
                                handle_client_request(&push_service, &tenant_owned, &sub, &resp_tx, req, &metrics).await;
                            }
                            Ok(None) => {
                                debug!(client_id = client_id_owned, "stream closed");
                                break;
                            }
                            Err(e) => {
                                debug!(client_id = client_id_owned, error = %e, "stream error");
                                break;
                            }
                        }
                    }
                    event = sub.event_rx.recv() => {
                        match event {
                            Some(topic_event) => {
                                let response = ChannelResponse {
                                    response: Some(proto::channel_response::Response::ChannelEvent(
                                        proto::ChannelEvent {
                                            event: Some(proto::Event {
                                                name: String::new(),
                                                format_type: 0,
                                                data: topic_event.event,
                                            }),
                                            topic: topic_event.topic,
                                        },
                                    )),
                                };
                                metrics.messages_delivered.fetch_add(1, Ordering::Relaxed);
                                if resp_tx.send(Ok(response)).await.is_err() {
                                    break;
                                }
                            }
                            None => break,
                        }
                    }
                    _ = keepalive_tick.tick() => {
                        if last_inbound_at.elapsed() > Duration::from_secs(keepalive.grpc_stream_idle_timeout_secs) {
                            metrics.grpc_keepalive_timeout_total.fetch_add(1, Ordering::Relaxed);
                            let _ = resp_tx.send(Err(Status::deadline_exceeded("channel keepalive timeout"))).await;
                            break;
                        }
                    }
                }
            }

            push_service
                .unsubscribe_client(&tenant_owned, &client_id_owned, &sub, device_owned.as_ref())
                .await;
        });

        let output_stream = ReceiverStream::new(resp_rx);
        Ok(Response::new(Box::pin(output_stream) as Self::ChannelStream))
    }

    async fn send_event_to_client_channel(
        &self,
        request: Request<SendEventToClientChannelRequest>,
    ) -> Result<Response<SendEventToClientChannelResponse>, Status> {
        let publisher = self.authorize_publisher(request.metadata())?;
        let req = request.into_inner();
        let (event_bytes, event_name) = Self::extract_event(&req.event);

        self.push_service
            .publish_to_client(SendEventToClientRequest {
                tenant_id: publisher.tenant_id,
                client_id: req.client_id,
                event_name,
                event: event_bytes,
            })
            .await
            .map_err(Status::from)?;

        Ok(Response::new(SendEventToClientChannelResponse {
            status: Some(Self::ok_status()),
        }))
    }

    async fn send_event_to_client_device_channel(
        &self,
        request: Request<SendEventToClientDeviceChannelRequest>,
    ) -> Result<Response<SendEventToClientDeviceChannelResponse>, Status> {
        let publisher = self.authorize_publisher(request.metadata())?;
        let req = request.into_inner();
        let (event_bytes, event_name) = Self::extract_event(&req.event);

        self.push_service
            .publish_to_client_device(SendEventToClientDeviceRequest {
                tenant_id: publisher.tenant_id,
                client_id: req.client_id,
                device_id: req.device_id,
                event_name,
                event: event_bytes,
            })
            .await
            .map_err(Status::from)?;

        Ok(Response::new(SendEventToClientDeviceChannelResponse {
            status: Some(Self::ok_status()),
        }))
    }

    async fn send_event_to_topic(
        &self,
        request: Request<ProtoSendEventToTopicRequest>,
    ) -> Result<Response<SendEventToTopicResponse>, Status> {
        let publisher = self.authorize_publisher(request.metadata())?;
        let req = request.into_inner();
        let (event_bytes, event_name) = Self::extract_event(&req.event);

        self.push_service
            .publish_to_topic(SendEventToTopicRequest {
                tenant_id: publisher.tenant_id,
                topic: req.topic,
                event_name,
                event: event_bytes,
            })
            .await
            .map_err(Status::from)?;

        Ok(Response::new(SendEventToTopicResponse {
            status: Some(Self::ok_status()),
        }))
    }

    async fn send_event_to_topics(
        &self,
        request: Request<SendEventToTopicsRequest>,
    ) -> Result<Response<SendEventToTopicsResponse>, Status> {
        let publisher = self.authorize_publisher(request.metadata())?;
        let req = request.into_inner();

        let topic_requests: Vec<SendEventToTopicRequest> = req
            .requests
            .into_iter()
            .map(|r| {
                let (event_bytes, event_name) = Self::extract_event(&r.event);
                SendEventToTopicRequest {
                    tenant_id: publisher.tenant_id.clone(),
                    topic: r.topic,
                    event_name,
                    event: event_bytes,
                }
            })
            .collect();

        self.push_service
            .publish_to_topics(topic_requests)
            .await
            .map_err(Status::from)?;

        Ok(Response::new(SendEventToTopicsResponse {
            status: Some(Self::ok_status()),
        }))
    }

    async fn get_client_active_devices(
        &self,
        request: Request<GetClientActiveDevicesRequest>,
    ) -> Result<Response<GetClientActiveDevicesResponse>, Status> {
        let (viewer, _) = self.auth_client(request.metadata())?;
        let req = request.into_inner();
        let client_id = if req.client_id.is_empty() {
            viewer.client_id.clone()
        } else if req.client_id != viewer.client_id {
            return Err(Status::from(PropellerError::PermissionDenied(
                "client id mismatch".into(),
            )));
        } else {
            req.client_id
        };

        let devices = self
            .push_service
            .get_active_devices(GetActiveDevicesRequest {
                tenant_id: viewer.tenant_id,
                client_id,
            })
            .await
            .map_err(Status::from)?;

        let is_online = !devices.is_empty();
        let proto_devices: Vec<proto::Device> = devices
            .into_iter()
            .map(|d| proto::Device {
                id: d.id,
                attributes: d.attributes,
            })
            .collect();

        Ok(Response::new(GetClientActiveDevicesResponse {
            status: Some(Self::ok_status()),
            is_client_online: is_online,
            devices: proto_devices,
        }))
    }
}

async fn handle_client_request(
    push_service: &Arc<PushService>,
    tenant_id: &str,
    sub: &crate::broker::Subscription,
    resp_tx: &mpsc::Sender<Result<ChannelResponse, Status>>,
    req: ChannelRequest,
    metrics: &Arc<Metrics>,
) {
    use proto::channel_request::Request as Req;

    let request = match req.request {
        Some(r) => r,
        None => return,
    };

    match request {
        Req::TopicSubscriptionRequest(topic_req) => {
            let topic = topic_req.topic;
            let status = match push_service.topic_subscribe(tenant_id, &topic, sub).await {
                Ok(()) => {
                    metrics.topic_subscribe_total.fetch_add(1, Ordering::Relaxed);
                    GrpcPushHandler::ok_status()
                }
                Err(e) => GrpcPushHandler::err_status(&e.to_string()),
            };
            let resp = ChannelResponse {
                response: Some(proto::channel_response::Response::TopicSubscriptionRequestAck(
                    proto::TopicSubscriptionRequestAck {
                        topic,
                        status: Some(status),
                    },
                )),
            };
            let _ = resp_tx.send(Ok(resp)).await;
        }

        Req::TopicUnsubscriptionRequest(unsub_req) => {
            let topic = unsub_req.topic;
            let status = match push_service.topic_unsubscribe(tenant_id, &topic, sub).await {
                Ok(()) => {
                    metrics
                        .topic_unsubscribe_total
                        .fetch_add(1, Ordering::Relaxed);
                    GrpcPushHandler::ok_status()
                }
                Err(e) => {
                    metrics
                        .topic_unsubscribe_error_total
                        .fetch_add(1, Ordering::Relaxed);
                    GrpcPushHandler::err_status(&e.to_string())
                }
            };
            let resp = ChannelResponse {
                response: Some(proto::channel_response::Response::TopicUnsubscriptionRequestAck(
                    proto::TopicUnsubscriptionRequestAck {
                        topic,
                        status: Some(status),
                    },
                )),
            };
            let _ = resp_tx.send(Ok(resp)).await;
        }

        Req::ChannelEvent(_) => {
            debug!("received channel event from client");
        }

        Req::ChannelEventAck(_) => {
            debug!("received event ack from client");
        }
    }
}
