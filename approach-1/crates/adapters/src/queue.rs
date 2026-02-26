use anyhow::Result;
use redis::streams::{StreamReadOptions, StreamReadReply};

#[derive(Clone)]
pub struct WebhookQueue {
    client: redis::Client,
    pub stream: String,
}

impl WebhookQueue {
    pub fn new(redis_url: &str, stream: &str) -> Result<Self> {
        Ok(Self {
            client: redis::Client::open(redis_url)?,
            stream: stream.to_string(),
        })
    }

    pub async fn enqueue(&self, payload: &str) -> Result<()> {
        let mut connection = self.client.get_multiplexed_tokio_connection().await?;
        let _: String = redis::cmd("XADD")
            .arg(&self.stream)
            .arg("*")
            .arg("payload")
            .arg(payload)
            .query_async(&mut connection)
            .await?;
        Ok(())
    }

    pub async fn read_after(&self, last_id: &str, count: usize) -> Result<Vec<(String, String)>> {
        let mut connection = self.client.get_multiplexed_tokio_connection().await?;
        let opts = StreamReadOptions::default().block(1000).count(count);
        let reply: StreamReadReply = connection
            .xread_options(&[&self.stream], &[last_id], &opts)
            .await?;

        let mut out = Vec::new();
        for key in reply.keys {
            for id in key.ids {
                if let Some(payload) = id.map.get("payload") {
                    out.push((id.id, payload.to_string()));
                }
            }
        }
        Ok(out)
    }
}
