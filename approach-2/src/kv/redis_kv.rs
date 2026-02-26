use std::collections::HashMap;

use anyhow::Context;
use async_trait::async_trait;
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;
use tracing::debug;

use super::KeyValue;

/// Redis-backed KV store using hash maps.
/// Each client_id is a Redis hash key, device_id is the field,
/// and the value is a JSON-encoded map of device attributes.
pub struct RedisKV {
    conn: MultiplexedConnection,
}

impl RedisKV {
    pub fn new(conn: MultiplexedConnection) -> Self {
        Self { conn }
    }
}

#[async_trait]
impl KeyValue for RedisKV {
    async fn store(&self, key: &str, field: &str, value: &str) -> anyhow::Result<()> {
        let mut conn = self.conn.clone();
        conn.hset::<_, _, _, ()>(key, field, value)
            .await
            .context("redis hset failed")?;
        debug!(key = key, field = field, "stored device attrs");
        Ok(())
    }

    async fn load(&self, key: &str) -> anyhow::Result<HashMap<String, String>> {
        let mut conn = self.conn.clone();
        let result: HashMap<String, String> = conn.hgetall(key)
            .await
            .context("redis hgetall failed")?;
        Ok(result)
    }

    async fn delete(&self, key: &str, fields: &[&str]) -> anyhow::Result<()> {
        let mut conn = self.conn.clone();
        for field in fields {
            conn.hdel::<_, _, ()>(key, *field)
                .await
                .context("redis hdel failed")?;
        }
        Ok(())
    }
}
