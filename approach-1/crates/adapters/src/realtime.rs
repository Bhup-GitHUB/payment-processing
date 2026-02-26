use anyhow::Result;
use redis::AsyncCommands;

#[derive(Clone)]
pub struct RealtimePublisher {
    client: redis::Client,
}

impl RealtimePublisher {
    pub fn new(redis_url: &str) -> Result<Self> {
        Ok(Self {
            client: redis::Client::open(redis_url)?,
        })
    }

    pub async fn publish_payment_confirmed(&self, payload: &str) -> Result<()> {
        let mut connection = self.client.get_multiplexed_tokio_connection().await?;
        let _: i64 = connection.publish("payments.confirmed", payload).await?;
        Ok(())
    }
}
