pub mod redis_pubsub;

use async_trait::async_trait;
use tokio::sync::mpsc;
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct TopicEvent {
    pub topic: String,
    pub event: Vec<u8>,
}

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

#[async_trait]
pub trait PubSub: Send + Sync + 'static {
    async fn publish(&self, channel: &str, data: &[u8]) -> anyhow::Result<()>;
    async fn publish_bulk(&self, requests: &[(&str, &[u8])]) -> anyhow::Result<()>;
    async fn subscribe(&self, channels: &[String]) -> anyhow::Result<Subscription>;
    async fn add_subscription(&self, channel: &str, sub_id: Uuid) -> anyhow::Result<()>;
    async fn remove_subscription(&self, channel: &str, sub_id: Uuid) -> anyhow::Result<()>;
    async fn unsubscribe(&self, sub_id: Uuid) -> anyhow::Result<()>;
    fn active_topic_listeners(&self) -> u64;
}
