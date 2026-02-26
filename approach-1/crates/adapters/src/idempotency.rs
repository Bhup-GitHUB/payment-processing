use anyhow::Result;
use redis::AsyncCommands;

#[derive(Clone)]
pub struct IdempotencyStore {
    client: redis::Client,
}

impl IdempotencyStore {
    pub fn new(redis_url: &str) -> Result<Self> {
        let client = redis::Client::open(redis_url)?;
        Ok(Self { client })
    }

    pub async fn mark_if_absent(&self, key: &str, ttl_seconds: u64) -> Result<bool> {
        let mut connection = self.client.get_multiplexed_tokio_connection().await?;
        let was_set: bool = connection.set_nx(key, "1").await?;
        if was_set {
            let _: () = connection.expire(key, ttl_seconds as i64).await?;
        }
        Ok(was_set)
    }
}
