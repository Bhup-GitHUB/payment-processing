use std::sync::Arc;

use anyhow::Context;
use tracing::{debug, error, info, warn};

use crate::broker::{PubSub, Subscription};
use crate::config::ClientConfig;
use crate::error::PropellerError;
use crate::kv::KeyValue;

use super::models::{
    Device, GetActiveDevicesRequest, SendEventToClientDeviceRequest, SendEventToClientRequest,
    SendEventToTopicRequest,
};

/// Core push service. Owns the broker and kv store references,
/// and implements all the publish/subscribe/device-management logic.
pub struct PushService {
    pubsub: Arc<dyn PubSub>,
    kv: Arc<dyn KeyValue>,
    config: ClientConfig,
}

impl PushService {
    pub fn new(pubsub: Arc<dyn PubSub>, kv: Arc<dyn KeyValue>, config: ClientConfig) -> Self {
        Self { pubsub, kv, config }
    }

    /// Subscribe a client to their personal channel on the broker.
    /// If device support is enabled, also subscribes to the device-specific
    /// channel and stores device attributes in the KV store.
    pub async fn subscribe_client(
        &self,
        client_id: &str,
        device: Option<&Device>,
    ) -> Result<Subscription, PropellerError> {
        if client_id.is_empty() {
            return Err(PropellerError::InvalidArgument(
                "client id is required".into(),
            ));
        }

        let mut channels = vec![client_id.to_string()];

        if self.config.enable_device_support {
            if let Some(dev) = device {
                if dev.id.is_empty() {
                    return Err(PropellerError::InvalidArgument(
                        "device id is required when device support is enabled".into(),
                    ));
                }
                let device_channel = format!("{}--{}", client_id, dev.id);
                channels.push(device_channel);
            }
        }

        let sub = self
            .pubsub
            .subscribe(&channels)
            .await
            .map_err(|e| PropellerError::Internal(e.to_string()))?;

        // store device attributes in kv if device support is on
        if self.config.enable_device_support {
            if let Some(dev) = device {
                let attrs_json = serde_json::to_string(&dev.attributes)
                    .map_err(|e| PropellerError::Internal(e.to_string()))?;
                if let Err(e) = self.kv.store(client_id, &dev.id, &attrs_json).await {
                    error!(
                        client_id = client_id,
                        device_id = dev.id,
                        "failed to store device attrs: {}",
                        e
                    );
                }
            }
        }

        info!(client_id = client_id, "client subscribed");
        Ok(sub)
    }

    /// Unsubscribe a client and clean up device state.
    pub async fn unsubscribe_client(
        &self,
        client_id: &str,
        sub: &Subscription,
        device: Option<&Device>,
    ) {
        if self.config.enable_device_support {
            if let Some(dev) = device {
                if let Err(e) = self.kv.delete(client_id, &[dev.id.as_str()]).await {
                    error!(
                        client_id = client_id,
                        device_id = dev.id,
                        "failed to delete device entry: {}",
                        e
                    );
                }
            }
        }

        if let Err(e) = self.pubsub.unsubscribe(sub.id).await {
            error!(
                client_id = client_id,
                "failed to unsubscribe from broker: {}", e
            );
        }

        info!(client_id = client_id, "client unsubscribed");
    }

    /// Publish an event to a specific client.
    pub async fn publish_to_client(
        &self,
        req: SendEventToClientRequest,
    ) -> Result<(), PropellerError> {
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

        debug!(
            client_id = req.client_id,
            event = req.event_name,
            "published to client"
        );
        Ok(())
    }

    /// Publish an event to a specific device of a client.
    pub async fn publish_to_client_device(
        &self,
        req: SendEventToClientDeviceRequest,
    ) -> Result<(), PropellerError> {
        if !self.config.enable_device_support {
            return Err(PropellerError::FailedPrecondition(
                "device support is disabled".into(),
            ));
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

        debug!(
            client_id = req.client_id,
            device_id = req.device_id,
            event = req.event_name,
            "published to client device"
        );
        Ok(())
    }

    /// Publish an event to a topic.
    pub async fn publish_to_topic(
        &self,
        req: SendEventToTopicRequest,
    ) -> Result<(), PropellerError> {
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

        debug!(topic = req.topic, event = req.event_name, "published to topic");
        Ok(())
    }

    /// Publish events to multiple topics.
    pub async fn publish_to_topics(
        &self,
        requests: Vec<SendEventToTopicRequest>,
    ) -> Result<(), PropellerError> {
        let pairs: Vec<(&str, &[u8])> = requests
            .iter()
            .map(|r| (r.topic.as_str(), r.event.as_slice()))
            .collect();

        self.pubsub
            .publish_bulk(&pairs)
            .await
            .map_err(|e| PropellerError::Broker(e.to_string()))?;

        Ok(())
    }

    /// Subscribe to a custom topic on an existing client subscription.
    pub async fn topic_subscribe(
        &self,
        topic: &str,
        sub: &Subscription,
    ) -> Result<(), PropellerError> {
        if topic.is_empty() {
            return Err(PropellerError::InvalidArgument("topic is empty".into()));
        }

        self.pubsub
            .add_subscription(topic, sub.id)
            .await
            .map_err(|e| PropellerError::Internal(e.to_string()))?;

        info!(topic = topic, sub_id = %sub.id, "subscribed to topic");
        Ok(())
    }

    /// Unsubscribe from a custom topic.
    pub async fn topic_unsubscribe(
        &self,
        topic: &str,
        sub: &Subscription,
    ) -> Result<(), PropellerError> {
        if topic.is_empty() {
            return Err(PropellerError::InvalidArgument("topic is empty".into()));
        }

        self.pubsub
            .remove_subscription(topic, sub.id)
            .await
            .map_err(|e| PropellerError::Internal(e.to_string()))?;

        debug!(topic = topic, sub_id = %sub.id, "unsubscribed from topic");
        Ok(())
    }

    /// Get all active devices for a client. Loads from KV store, then
    /// could optionally validate each device is still connected via a
    /// pub/sub ping (simplified here compared to Propeller's full
    /// validation flow).
    pub async fn get_active_devices(
        &self,
        req: GetActiveDevicesRequest,
    ) -> Result<Vec<Device>, PropellerError> {
        if !self.config.enable_device_support {
            return Err(PropellerError::FailedPrecondition(
                "device support is disabled".into(),
            ));
        }
        if req.client_id.is_empty() {
            return Err(PropellerError::InvalidArgument("client id is empty".into()));
        }

        let entries = self
            .kv
            .load(&req.client_id)
            .await
            .map_err(|e| PropellerError::Internal(e.to_string()))?;

        let mut devices = Vec::new();
        for (device_id, attrs_json) in entries {
            let attrs: std::collections::HashMap<String, String> =
                serde_json::from_str(&attrs_json).unwrap_or_default();
            devices.push(Device {
                id: device_id,
                attributes: attrs,
            });
        }

        debug!(
            client_id = req.client_id,
            count = devices.len(),
            "loaded active devices"
        );
        Ok(devices)
    }
}
