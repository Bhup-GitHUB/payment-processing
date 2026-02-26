use std::sync::Arc;
use std::sync::atomic::Ordering;

use tracing::{debug, error, info};

use crate::broker::{PubSub, Subscription};
use crate::config::ClientConfig;
use crate::error::PropellerError;
use crate::kv::KeyValue;
use crate::metrics::Metrics;

use super::models::{
    Device, GetActiveDevicesRequest, SendEventToClientDeviceRequest, SendEventToClientRequest,
    SendEventToTopicRequest,
};

pub struct PushService {
    pubsub: Arc<dyn PubSub>,
    kv: Arc<dyn KeyValue>,
    config: ClientConfig,
    metrics: Arc<Metrics>,
}

impl PushService {
    pub fn new(
        pubsub: Arc<dyn PubSub>,
        kv: Arc<dyn KeyValue>,
        config: ClientConfig,
        metrics: Arc<Metrics>,
    ) -> Self {
        Self { pubsub, kv, config, metrics }
    }

    pub async fn subscribe_client(
        &self,
        client_id: &str,
        device: Option<&Device>,
    ) -> Result<Subscription, PropellerError> {
        if client_id.is_empty() {
            return Err(PropellerError::InvalidArgument("client id is required".into()));
        }

        let mut channels = vec![client_id.to_string()];

        if self.config.enable_device_support {
            if let Some(dev) = device {
                if dev.id.is_empty() {
                    return Err(PropellerError::InvalidArgument(
                        "device id is required when device support is enabled".into(),
                    ));
                }
                channels.push(format!("{}--{}", client_id, dev.id));
            }
        }

        let sub = self
            .pubsub
            .subscribe(&channels)
            .await
            .map_err(|e| PropellerError::Internal(e.to_string()))?;

        if self.config.enable_device_support {
            if let Some(dev) = device {
                let attrs_json = serde_json::to_string(&dev.attributes)
                    .map_err(|e| PropellerError::Internal(e.to_string()))?;
                if let Err(e) = self.kv.store(client_id, &dev.id, &attrs_json).await {
                    error!(client_id, device_id = dev.id, "failed to store device attrs: {}", e);
                }
            }
        }

        self.metrics.connections.fetch_add(1, Ordering::Relaxed);
        info!(client_id, "client subscribed");
        Ok(sub)
    }

    pub async fn unsubscribe_client(
        &self,
        client_id: &str,
        sub: &Subscription,
        device: Option<&Device>,
    ) {
        if self.config.enable_device_support {
            if let Some(dev) = device {
                if let Err(e) = self.kv.delete(client_id, &[dev.id.as_str()]).await {
                    error!(client_id, device_id = dev.id, "failed to delete device entry: {}", e);
                }
            }
        }

        if let Err(e) = self.pubsub.unsubscribe(sub.id).await {
            error!(client_id, "failed to unsubscribe: {}", e);
        }

        self.metrics.connections.fetch_sub(1, Ordering::Relaxed);
        info!(client_id, "client unsubscribed");
    }

    pub async fn publish_to_client(&self, req: SendEventToClientRequest) -> Result<(), PropellerError> {
        if req.client_id.is_empty() {
            return Err(PropellerError::InvalidArgument("client id is empty".into()));
        }
        if req.event.is_empty() {
            return Err(PropellerError::InvalidArgument("event is empty".into()));
        }

        self.pubsub
            .publish(&req.client_id, &req.event)
            .await
            .map_err(|e| PropellerError::Broker(e.to_string()))?;

        self.metrics.messages_published.fetch_add(1, Ordering::Relaxed);
        debug!(client_id = req.client_id, event = req.event_name, "published to client");
        Ok(())
    }

    pub async fn publish_to_client_device(&self, req: SendEventToClientDeviceRequest) -> Result<(), PropellerError> {
        if !self.config.enable_device_support {
            return Err(PropellerError::FailedPrecondition("device support is disabled".into()));
        }
        if req.client_id.is_empty() {
            return Err(PropellerError::InvalidArgument("client id is empty".into()));
        }
        if req.device_id.is_empty() {
            return Err(PropellerError::InvalidArgument("device id is empty".into()));
        }
        if req.event.is_empty() {
            return Err(PropellerError::InvalidArgument("event is empty".into()));
        }

        let channel = format!("{}--{}", req.client_id, req.device_id);
        self.pubsub
            .publish(&channel, &req.event)
            .await
            .map_err(|e| PropellerError::Broker(e.to_string()))?;

        self.metrics.messages_published.fetch_add(1, Ordering::Relaxed);
        debug!(client_id = req.client_id, device_id = req.device_id, "published to device");
        Ok(())
    }

    pub async fn publish_to_topic(&self, req: SendEventToTopicRequest) -> Result<(), PropellerError> {
        if req.topic.is_empty() {
            return Err(PropellerError::InvalidArgument("topic is empty".into()));
        }
        if req.event.is_empty() {
            return Err(PropellerError::InvalidArgument("event is empty".into()));
        }

        self.pubsub
            .publish(&req.topic, &req.event)
            .await
            .map_err(|e| PropellerError::Broker(e.to_string()))?;

        self.metrics.messages_published.fetch_add(1, Ordering::Relaxed);
        debug!(topic = req.topic, "published to topic");
        Ok(())
    }

    pub async fn publish_to_topics(&self, requests: Vec<SendEventToTopicRequest>) -> Result<(), PropellerError> {
        let pairs: Vec<(&str, &[u8])> = requests
            .iter()
            .map(|r| (r.topic.as_str(), r.event.as_slice()))
            .collect();

        self.pubsub
            .publish_bulk(&pairs)
            .await
            .map_err(|e| PropellerError::Broker(e.to_string()))?;

        self.metrics.messages_published.fetch_add(requests.len() as u64, Ordering::Relaxed);
        Ok(())
    }

    pub async fn topic_subscribe(&self, topic: &str, sub: &Subscription) -> Result<(), PropellerError> {
        if topic.is_empty() {
            return Err(PropellerError::InvalidArgument("topic is empty".into()));
        }

        self.pubsub
            .add_subscription(topic, sub.id)
            .await
            .map_err(|e| PropellerError::Internal(e.to_string()))?;

        info!(topic, sub_id = %sub.id, "subscribed to topic");
        Ok(())
    }

    pub async fn topic_unsubscribe(&self, topic: &str, sub: &Subscription) -> Result<(), PropellerError> {
        if topic.is_empty() {
            return Err(PropellerError::InvalidArgument("topic is empty".into()));
        }

        self.pubsub
            .remove_subscription(topic, sub.id)
            .await
            .map_err(|e| PropellerError::Internal(e.to_string()))?;

        debug!(topic, sub_id = %sub.id, "unsubscribed from topic");
        Ok(())
    }

    pub async fn get_active_devices(&self, req: GetActiveDevicesRequest) -> Result<Vec<Device>, PropellerError> {
        if !self.config.enable_device_support {
            return Err(PropellerError::FailedPrecondition("device support is disabled".into()));
        }
        if req.client_id.is_empty() {
            return Err(PropellerError::InvalidArgument("client id is empty".into()));
        }

        let entries = self
            .kv
            .load(&req.client_id)
            .await
            .map_err(|e| PropellerError::Internal(e.to_string()))?;

        let devices: Vec<Device> = entries
            .into_iter()
            .map(|(device_id, attrs_json)| {
                let attributes = serde_json::from_str(&attrs_json).unwrap_or_default();
                Device { id: device_id, attributes }
            })
            .collect();

        debug!(client_id = req.client_id, count = devices.len(), "loaded active devices");
        Ok(devices)
    }
}
