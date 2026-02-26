pub mod redis_pubsub;

use async_trait::async_trait;
use tokio::sync::mpsc;
use uuid::Uuid;

/// A single event received from a broker subscription.
#[derive(Debug, Clone)]
pub struct TopicEvent {
    pub topic: String,
    pub event: Vec<u8>,
}

/// Handle returned when subscribing. The receiver end gets TopicEvents
/// pushed by the broker whenever a message arrives on any subscribed channel.
pub struct Subscription {
    pub id: Uuid,
    pub event_rx: mpsc::UnboundedReceiver<TopicEvent>,
    pub(crate) event_tx: mpsc::UnboundedSender<TopicEvent>,
}

impl Subscription {
    pub fn new() -> Self {
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        Self {
            id: Uuid::new_v4(),
            event_rx,
            event_tx,
        }
    }
}

/// Abstraction over the message broker. Redis today, NATS tomorrow.
#[async_trait]
pub trait PubSub: Send + Sync + 'static {
    /// Publish raw bytes to a channel.
    async fn publish(&self, channel: &str, data: &[u8]) -> anyhow::Result<()>;

    /// Publish to multiple channels.
    async fn publish_bulk(&self, requests: &[(&str, &[u8])]) -> anyhow::Result<()>;

    /// Subscribe to one or more channels. Returns a Subscription whose
    /// event_rx will receive TopicEvents as they arrive.
    async fn subscribe(&self, channels: &[String]) -> anyhow::Result<Subscription>;

    /// Add a channel to an existing subscription (dynamic topic subscribe).
    async fn add_subscription(&self, channel: &str, sub_id: Uuid) -> anyhow::Result<()>;

    /// Remove a channel from an existing subscription (dynamic topic unsubscribe).
    async fn remove_subscription(&self, channel: &str, sub_id: Uuid) -> anyhow::Result<()>;

    /// Tear down a subscription entirely.
    async fn unsubscribe(&self, sub_id: Uuid) -> anyhow::Result<()>;
}
