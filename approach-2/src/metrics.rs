use std::sync::atomic::{AtomicU64, Ordering};

pub struct Metrics {
    pub connections: AtomicU64,
    pub messages_published: AtomicU64,
    pub messages_delivered: AtomicU64,
    pub auth_success_total: AtomicU64,
    pub auth_failure_total: AtomicU64,
    pub legacy_header_auth_total: AtomicU64,
    pub topic_subscribe_total: AtomicU64,
    pub topic_unsubscribe_total: AtomicU64,
    pub topic_unsubscribe_error_total: AtomicU64,
    pub active_topic_listeners: AtomicU64,
    pub ws_ping_sent_total: AtomicU64,
    pub ws_pong_received_total: AtomicU64,
    pub ws_keepalive_timeout_total: AtomicU64,
    pub grpc_keepalive_timeout_total: AtomicU64,
}

impl Metrics {
    pub fn new() -> Self {
        Self {
            connections: AtomicU64::new(0),
            messages_published: AtomicU64::new(0),
            messages_delivered: AtomicU64::new(0),
            auth_success_total: AtomicU64::new(0),
            auth_failure_total: AtomicU64::new(0),
            legacy_header_auth_total: AtomicU64::new(0),
            topic_subscribe_total: AtomicU64::new(0),
            topic_unsubscribe_total: AtomicU64::new(0),
            topic_unsubscribe_error_total: AtomicU64::new(0),
            active_topic_listeners: AtomicU64::new(0),
            ws_ping_sent_total: AtomicU64::new(0),
            ws_pong_received_total: AtomicU64::new(0),
            ws_keepalive_timeout_total: AtomicU64::new(0),
            grpc_keepalive_timeout_total: AtomicU64::new(0),
        }
    }

    pub fn snapshot(&self) -> MetricsSnapshot {
        MetricsSnapshot {
            connections: self.connections.load(Ordering::Relaxed),
            messages_published: self.messages_published.load(Ordering::Relaxed),
            messages_delivered: self.messages_delivered.load(Ordering::Relaxed),
            auth_success_total: self.auth_success_total.load(Ordering::Relaxed),
            auth_failure_total: self.auth_failure_total.load(Ordering::Relaxed),
            legacy_header_auth_total: self.legacy_header_auth_total.load(Ordering::Relaxed),
            topic_subscribe_total: self.topic_subscribe_total.load(Ordering::Relaxed),
            topic_unsubscribe_total: self.topic_unsubscribe_total.load(Ordering::Relaxed),
            topic_unsubscribe_error_total: self.topic_unsubscribe_error_total.load(Ordering::Relaxed),
            active_topic_listeners: self.active_topic_listeners.load(Ordering::Relaxed),
            ws_ping_sent_total: self.ws_ping_sent_total.load(Ordering::Relaxed),
            ws_pong_received_total: self.ws_pong_received_total.load(Ordering::Relaxed),
            ws_keepalive_timeout_total: self.ws_keepalive_timeout_total.load(Ordering::Relaxed),
            grpc_keepalive_timeout_total: self.grpc_keepalive_timeout_total.load(Ordering::Relaxed),
        }
    }
}

#[derive(serde::Serialize)]
pub struct MetricsSnapshot {
    pub connections: u64,
    pub messages_published: u64,
    pub messages_delivered: u64,
    pub auth_success_total: u64,
    pub auth_failure_total: u64,
    pub legacy_header_auth_total: u64,
    pub topic_subscribe_total: u64,
    pub topic_unsubscribe_total: u64,
    pub topic_unsubscribe_error_total: u64,
    pub active_topic_listeners: u64,
    pub ws_ping_sent_total: u64,
    pub ws_pong_received_total: u64,
    pub ws_keepalive_timeout_total: u64,
    pub grpc_keepalive_timeout_total: u64,
}
