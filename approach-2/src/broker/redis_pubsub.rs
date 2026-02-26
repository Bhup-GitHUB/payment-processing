use std::sync::Arc;

use anyhow::Context;
use async_trait::async_trait;
use dashmap::DashMap;
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;
use tokio::sync::mpsc;
use tracing::{debug, info};
use uuid::Uuid;

use super::{PubSub, Subscription, TopicEvent};

struct ActiveSubscription {
    event_tx: mpsc::UnboundedSender<TopicEvent>,
    control_tx: mpsc::UnboundedSender<ControlMessage>,
}

enum ControlMessage {
    Shutdown,
}

pub struct RedisPubSub {
    conn: MultiplexedConnection,
    client: redis::Client,
    subscriptions: Arc<DashMap<Uuid, ActiveSubscription>>,
}

impl RedisPubSub {
    pub async fn new(redis_url: &str) -> anyhow::Result<Self> {
        let client = redis::Client::open(redis_url)
            .context("failed to create redis client")?;
        let conn = client
            .get_multiplexed_async_connection()
            .await
            .context("failed to connect to redis")?;

        info!("connected to redis at {}", redis_url);

        Ok(Self {
            conn,
            client,
            subscriptions: Arc::new(DashMap::new()),
        })
    }
}

#[async_trait]
impl PubSub for RedisPubSub {
    async fn publish(&self, channel: &str, data: &[u8]) -> anyhow::Result<()> {
        let mut conn = self.conn.clone();
        conn.publish::<_, _, ()>(channel, data).await?;
        debug!(channel = channel, "published message");
        Ok(())
    }

    async fn publish_bulk(&self, requests: &[(&str, &[u8])]) -> anyhow::Result<()> {
        let mut conn = self.conn.clone();
        for (channel, data) in requests {
            conn.publish::<_, _, ()>(*channel, *data).await?;
        }
        Ok(())
    }

    async fn subscribe(&self, channels: &[String]) -> anyhow::Result<Subscription> {
        let sub = Subscription::new();
        let sub_id = sub.id;
        let event_tx = sub.event_tx.clone();

        let (control_tx, mut control_rx) = mpsc::unbounded_channel();

        self.subscriptions.insert(sub_id, ActiveSubscription {
            event_tx: event_tx.clone(),
            control_tx,
        });

        let mut pubsub_conn = self.client
            .get_async_pubsub()
            .await
            .context("failed to get pubsub connection")?;

        for ch in channels {
            pubsub_conn.subscribe(ch).await?;
        }

        tokio::spawn(async move {
            let mut msg_stream = pubsub_conn.into_on_message();

            loop {
                tokio::select! {
                    msg = futures_util::StreamExt::next(&mut msg_stream) => {
                        match msg {
                            Some(msg) => {
                                let channel: String = msg.get_channel_name().to_string();
                                let payload: Vec<u8> = redis::from_redis_value(
                                    &msg.get_payload::<redis::Value>().unwrap_or(redis::Value::Nil)
                                ).unwrap_or_default();

                                let event = TopicEvent { topic: channel, event: payload };
                                if event_tx.send(event).is_err() {
                                    return;
                                }
                            }
                            None => return,
                        }
                    }
                    ctrl = control_rx.recv() => {
                        match ctrl {
                            Some(ControlMessage::Shutdown) | None => return,
                        }
                    }
                }
            }
        });

        Ok(sub)
    }

    async fn add_subscription(&self, channel: &str, sub_id: Uuid) -> anyhow::Result<()> {
        let entry = self.subscriptions.get(&sub_id)
            .ok_or_else(|| anyhow::anyhow!("subscription {} not found", sub_id))?;

        let event_tx = entry.event_tx.clone();

        let mut pubsub_conn = self.client
            .get_async_pubsub()
            .await
            .context("failed to get pubsub connection")?;

        let channel_owned = channel.to_string();
        pubsub_conn.subscribe(&channel_owned).await?;

        tokio::spawn(async move {
            let mut msg_stream = pubsub_conn.into_on_message();
            while let Some(msg) = futures_util::StreamExt::next(&mut msg_stream).await {
                let ch: String = msg.get_channel_name().to_string();
                let payload: Vec<u8> = redis::from_redis_value(
                    &msg.get_payload::<redis::Value>().unwrap_or(redis::Value::Nil)
                ).unwrap_or_default();

                if event_tx.send(TopicEvent { topic: ch, event: payload }).is_err() {
                    return;
                }
            }
        });

        Ok(())
    }

    async fn remove_subscription(&self, _channel: &str, _sub_id: Uuid) -> anyhow::Result<()> {
        Ok(())
    }

    async fn unsubscribe(&self, sub_id: Uuid) -> anyhow::Result<()> {
        if let Some((_, entry)) = self.subscriptions.remove(&sub_id) {
            let _ = entry.control_tx.send(ControlMessage::Shutdown);
        }
        Ok(())
    }
}
