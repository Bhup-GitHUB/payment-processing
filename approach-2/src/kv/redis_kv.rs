use std::collections::HashMap;

use anyhow::Context;
use async_trait::async_trait;
use redis::aio::MultiplexedConnection;
use redis::AsyncCommands;

use super::KeyValue;

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
