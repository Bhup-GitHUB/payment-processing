use std::collections::HashMap;
use std::pin::Pin;
use std::sync::Arc;

use futures_util::Stream;
use tokio::sync::mpsc;
use tokio_stream::wrappers::ReceiverStream;
use tonic::{Request, Response, Status, Streaming};
use tracing::{debug, info};

use crate::config::ClientConfig;
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
    client_config: ClientConfig,
}

impl GrpcPushHandler {
    pub fn new(push_service: Arc<PushService>, client_config: ClientConfig) -> Self {
        Self {
            push_service,
            client_config,
        }
    }

    fn extract_client_device(
        &self,
        metadata: &tonic::metadata::MetadataMap,
    ) -> Result<(String, Option<Device>), Status> {
        let client_id = metadata
            .get(&self.client_config.header)
            .and_then(|v| v.to_str().ok())
            .map(|s| s.to_string())
            .ok_or_else(|| Status::invalid_argument("missing client header"))?;

        let device = if self.client_config.enable_device_support {
            let device_id = metadata
                .get(&self.client_config.device_header)
                .and_then(|v| v.to_str().ok())
                .map(|s| s.to_string())
                .ok_or_else(|| Status::invalid_argument("missing device header"))?;

            let mut attrs = HashMap::new();
            for header_key in &self.client_config.device_attribute_headers {
                if let Some(val) = metadata.get(header_key).and_then(|v| v.to_str().ok()) {
                    attrs.insert(header_key.clone(), val.to_string());
                }
            }

            Some(Device {
                id: device_id,
                attributes: attrs,
            })
        } else {
            None
        };

        Ok((client_id, device))
    }

    fn ok_status() -> ResponseStatus {
        ResponseStatus {
            success: true,
            error_code: String::new(),
            message: HashMap::new(),
            error_type: String::new(),
        }
    }
}

type ChannelStream = Pin<Box<dyn Stream<Item = Result<ChannelResponse, Status>> + Send>>;

#[tonic::async_trait]
impl PushServiceTrait for GrpcPushHandler {
    type ChannelStream = ChannelStream;

    /// Bidirectional streaming channel. This is the main entry point for
    /// frontend clients. Equivalent to Propeller's Channel() in grpc_handler.go.
    async fn channel(
        &self,
        request: Request<Streaming<ChannelRequest>>,
    ) -> Result<Response<Self::ChannelStream>, Status> {
        let metadata = request.metadata().clone();
        let (client_id, device) = self.extract_client_device(&metadata)?;
        let mut inbound = request.into_inner();

        info!(client_id = client_id, "client connecting");

        let mut sub = self
            .push_service
            .subscribe_client(&client_id, device.as_ref())
            .await
            .map_err(Status::from)?;

        // channel to send responses back to the client
        let (resp_tx, resp_rx) = mpsc::channel(128);

        // send connect ack
        let ack = ChannelResponse {
            response: Some(proto::channel_response::Response::ConnectAck(ConnectAck {
                status: Some(Self::ok_status()),
            })),
        };
        if resp_tx.send(Ok(ack)).await.is_err() {
            return Err(Status::internal("failed to send connect ack"));
        }

        let push_service = self.push_service.clone();
        let client_id_owned = client_id.clone();
        let device_owned = device.clone();

        // spawn main event loop
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    // incoming message from the client
                    client_msg = inbound.message() => {
                        match client_msg {
                            Ok(Some(req)) => {
                                handle_client_request(
                                    &push_service,
                                    &sub,
                                    &resp_tx,
                                    req,
                                ).await;
                            }
                            Ok(None) => {
                                // client closed the stream
                                debug!(client_id = client_id_owned, "client stream closed");
                                break;
                            }
                            Err(e) => {
                                debug!(client_id = client_id_owned, error = %e, "client stream error");
                                break;
                            }
                        }
                    }

                    // event from the broker subscription
                    event = sub.event_rx.recv() => {
                        match event {
                            Some(topic_event) => {
                                let response = ChannelResponse {
                                    response: Some(
                                        proto::channel_response::Response::ChannelEvent(
                                            proto::ChannelEvent {
                                                event: Some(proto::Event {
                                                    name: String::new(),
                                                    format_type: 0,
                                                    data: topic_event.event,
                                                }),
                                                topic: topic_event.topic.clone(),
                                            },
                                        ),
                                    ),
                                };
                                if resp_tx.send(Ok(response)).await.is_err() {
                                    break;
                                }
                                debug!(
                                    client_id = client_id_owned,
                                    topic = topic_event.topic,
                                    "forwarded event to client"
                                );
                            }
                            None => {
                                debug!(client_id = client_id_owned, "subscription channel closed");
                                break;
                            }
                        }
                    }
                }
            }

            // cleanup on disconnect
            push_service
                .unsubscribe_client(&client_id_owned, &sub, device_owned.as_ref())
                .await;
        });

        let output_stream = ReceiverStream::new(resp_rx);
        Ok(Response::new(Box::pin(output_stream) as Self::ChannelStream))
    }

    async fn send_event_to_client_channel(
        &self,
        request: Request<SendEventToClientChannelRequest>,
    ) -> Result<Response<SendEventToClientChannelResponse>, Status> {
        let req = request.into_inner();
        let event_bytes = req
            .event
            .as_ref()
            .map(|e| e.data.clone())
            .unwrap_or_default();
        let event_name = req
            .event
            .as_ref()
            .map(|e| e.name.clone())
            .unwrap_or_default();

        self.push_service
            .publish_to_client(SendEventToClientRequest {
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
        let req = request.into_inner();
        let event_bytes = req
            .event
            .as_ref()
            .map(|e| e.data.clone())
            .unwrap_or_default();
        let event_name = req
            .event
            .as_ref()
            .map(|e| e.name.clone())
            .unwrap_or_default();

        self.push_service
            .publish_to_client_device(SendEventToClientDeviceRequest {
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
        let req = request.into_inner();
        let event_bytes = req
            .event
            .as_ref()
            .map(|e| e.data.clone())
            .unwrap_or_default();
        let event_name = req
            .event
            .as_ref()
            .map(|e| e.name.clone())
            .unwrap_or_default();

        self.push_service
            .publish_to_topic(SendEventToTopicRequest {
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
        let req = request.into_inner();

        let topic_requests: Vec<SendEventToTopicRequest> = req
            .requests
            .into_iter()
            .map(|r| {
                let event_bytes = r.event.as_ref().map(|e| e.data.clone()).unwrap_or_default();
                let event_name = r
                    .event
                    .as_ref()
                    .map(|e| e.name.clone())
                    .unwrap_or_default();
                SendEventToTopicRequest {
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
        let req = request.into_inner();

        let devices = self
            .push_service
            .get_active_devices(GetActiveDevicesRequest {
                client_id: req.client_id,
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

/// Handles incoming requests from the client on the bidirectional stream.
async fn handle_client_request(
    push_service: &Arc<PushService>,
    sub: &crate::broker::Subscription,
    resp_tx: &mpsc::Sender<Result<ChannelResponse, Status>>,
    req: ChannelRequest,
) {
    use proto::channel_request::Request as Req;

    let request = match req.request {
        Some(r) => r,
        None => return,
    };

    match request {
        Req::TopicSubscriptionRequest(topic_req) => {
            let topic = topic_req.topic;
            let result = push_service.topic_subscribe(&topic, sub).await;

            let status = match result {
                Ok(()) => GrpcPushHandler::ok_status(),
                Err(e) => {
                    let mut msg = HashMap::new();
                    msg.insert("message".to_string(), e.to_string());
                    ResponseStatus {
                        success: false,
                        error_code: String::new(),
                        message: msg,
                        error_type: String::new(),
                    }
                }
            };

            let resp = ChannelResponse {
                response: Some(
                    proto::channel_response::Response::TopicSubscriptionRequestAck(
                        proto::TopicSubscriptionRequestAck {
                            topic,
                            status: Some(status),
                        },
                    ),
                ),
            };
            let _ = resp_tx.send(Ok(resp)).await;
        }

        Req::TopicUnsubscriptionRequest(unsub_req) => {
            let topic = unsub_req.topic;
            let result = push_service.topic_unsubscribe(&topic, sub).await;

            let status = match result {
                Ok(()) => GrpcPushHandler::ok_status(),
                Err(e) => {
                    let mut msg = HashMap::new();
                    msg.insert("message".to_string(), e.to_string());
                    ResponseStatus {
                        success: false,
                        error_code: String::new(),
                        message: msg,
                        error_type: String::new(),
                    }
                }
            };

            let resp = ChannelResponse {
                response: Some(
                    proto::channel_response::Response::TopicUnsubscriptionRequestAck(
                        proto::TopicUnsubscriptionRequestAck {
                            topic,
                            status: Some(status),
                        },
                    ),
                ),
            };
            let _ = resp_tx.send(Ok(resp)).await;
        }

        Req::ChannelEvent(_event) => {
            // client-to-server events can be handled here if needed
            debug!("received channel event from client");
        }

        Req::ChannelEventAck(_ack) => {
            debug!("received event ack from client");
        }
    }
}
