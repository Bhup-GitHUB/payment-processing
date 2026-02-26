use std::sync::atomic::{AtomicU64, Ordering};

pub struct Metrics {
    pub connections: AtomicU64,
    pub messages_published: AtomicU64,
    pub messages_delivered: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            connections: AtomicU64::new(0),
            messages_published: AtomicU64::new(0),
            messages_delivered: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            connections: self.connections.load(Ordering::Relaxed),
            messages_published: self.messages_published.load(Ordering::Relaxed),
            messages_delivered: self.messages_delivered.load(Ordering::Relaxed),
        }
    }
}

#[derive(serde::Serialize)]
pub struct MetricsSnapshot {
    pub connections: u64,
    pub messages_published: u64,
    pub messages_delivered: u64,
}
