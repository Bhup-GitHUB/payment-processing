pub mod redis_kv;

use std::collections::HashMap;

use async_trait::async_trait;

#[async_trait]
pub trait KeyValue: Send + Sync + 'static {
    async fn store(&self, key: &str, field: &str, value: &str) -> anyhow::Result<()>;
    async fn load(&self, key: &str) -> anyhow::Result<HashMap<String, String>>;
    async fn delete(&self, key: &str, fields: &[&str]) -> anyhow::Result<()>;
}
