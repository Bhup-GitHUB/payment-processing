use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::{Duration, Instant};

use axum::extract::ws::{CloseFrame, Message, WebSocket, close_code};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tracing::{debug, error, info};

use crate::auth::AuthService;
use crate::config::{ClientConfig, KeepaliveConfig};
use crate::metrics::Metrics;
use crate::push::models::Device;
use crate::push::service::PushService;

pub struct WsState {
    pub push_service: Arc<PushService>,
    pub auth: Arc<AuthService>,
    pub client_config: ClientConfig,
    pub keepalive: KeepaliveConfig,
    pub metrics: Arc<Metrics>,
}

pub async fn ws_connect(
    State(state): State<Arc<WsState>>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
) -> impl IntoResponse {
    let client_config = state.client_config.clone();
    let auth_result = state.auth.authenticate_http(&headers, &client_config);
    let (auth, legacy_mode) = match auth_result {
        Ok(value) => {
            state.metrics.auth_success_total.fetch_add(1, Ordering::Relaxed);
            if value.1 {
                state
                    .metrics
                    .legacy_header_auth_total
                    .fetch_add(1, Ordering::Relaxed);
            }
            value
        }
        Err(err) => {
            state.metrics.auth_failure_total.fetch_add(1, Ordering::Relaxed);
            error!(error = %err, "ws connection rejected by auth");
            return axum::http::StatusCode::UNAUTHORIZED.into_response();
        }
    };

    let device = if client_config.enable_device_support {
        let device_id = headers
            .get(&client_config.device_header)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let mut attrs = HashMap::new();
        for key in &client_config.device_attribute_headers {
            if let Some(val) = headers.get(key).and_then(|v| v.to_str().ok()) {
                attrs.insert(key.clone(), val.to_string());
            }
        }

        if !device_id.is_empty() {
            Some(Device { id: device_id, attributes: attrs })
        } else {
            None
        }
    } else {
        None
    };

    if legacy_mode {
        info!(client_id = auth.client_id, "ws using legacy header auth");
    }
    ws.on_upgrade(move |socket| handle_ws(state, socket, auth.tenant_id, auth.client_id, device))
}

async fn handle_ws(
    state: Arc<WsState>,
    socket: WebSocket,
    tenant_id: String,
    client_id: String,
    device: Option<Device>,
) {
    if client_id.is_empty() {
        error!("ws connection rejected: missing client id");
        return;
    }

    info!(client_id = client_id, "ws client connecting");

    let mut sub = match state
        .push_service
        .subscribe_client(&tenant_id, &client_id, device.as_ref())
        .await
    {
        Ok(s) => s,
        Err(e) => {
            error!(client_id = client_id, error = %e, "failed to subscribe ws client");
            return;
        }
    };

    let (mut ws_tx, mut ws_rx) = socket.split();
    let mut ping_interval = tokio::time::interval(Duration::from_secs(state.keepalive.ws_ping_interval_secs));
    ping_interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    let mut last_pong_at = Instant::now();

    let ack = serde_json::json!({ "type": "connect_ack", "status": { "success": true } });
    if ws_tx.send(Message::Text(ack.to_string().into())).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            ws_msg = ws_rx.next() => {
                match ws_msg {
                    Some(Ok(Message::Text(text))) => {
                        debug!(client_id = client_id, "ws message: {}", text);
                    }
                    Some(Ok(Message::Pong(_))) => {
                        last_pong_at = Instant::now();
                        state.metrics.ws_pong_received_total.fetch_add(1, Ordering::Relaxed);
                    }
                    Some(Ok(Message::Ping(payload))) => {
                        if ws_tx.send(Message::Pong(payload)).await.is_err() {
                            break;
                        }
                        last_pong_at = Instant::now();
                        state.metrics.ws_pong_received_total.fetch_add(1, Ordering::Relaxed);
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        debug!(client_id = client_id, "ws disconnected");
                        break;
                    }
                    Some(Err(e)) => {
                        debug!(client_id = client_id, error = %e, "ws error");
                        break;
                    }
                    _ => {}
                }
            }
            event = sub.event_rx.recv() => {
                match event {
                    Some(topic_event) => {
                        let payload = serde_json::json!({
                            "type": "event",
                            "topic": topic_event.topic,
                            "data": String::from_utf8_lossy(&topic_event.event),
                        });
                        state.metrics.messages_delivered.fetch_add(1, Ordering::Relaxed);
                        if ws_tx.send(Message::Text(payload.to_string().into())).await.is_err() {
                            break;
                        }
                    }
                    None => break,
                }
            }
            _ = ping_interval.tick() => {
                if last_pong_at.elapsed() > Duration::from_secs(state.keepalive.ws_pong_timeout_secs) {
                    state.metrics.ws_keepalive_timeout_total.fetch_add(1, Ordering::Relaxed);
                    let close = Message::Close(Some(CloseFrame {
                        code: close_code::POLICY,
                        reason: "keepalive timeout".into(),
                    }));
                    let _ = ws_tx.send(close).await;
                    break;
                }
                if ws_tx.send(Message::Ping(Vec::new().into())).await.is_err() {
                    break;
                }
                state.metrics.ws_ping_sent_total.fetch_add(1, Ordering::Relaxed);
            }
        }
    }

    state
        .push_service
        .unsubscribe_client(&tenant_id, &client_id, &sub, device.as_ref())
        .await;
}
