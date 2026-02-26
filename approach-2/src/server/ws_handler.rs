use std::collections::HashMap;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket};
use axum::extract::{State, WebSocketUpgrade};
use axum::http::HeaderMap;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use tracing::{debug, error, info};

use crate::config::ClientConfig;
use crate::push::models::Device;
use crate::push::service::PushService;

pub struct WsState {
    pub push_service: Arc<PushService>,
    pub client_config: ClientConfig,
}

/// Axum handler that upgrades HTTP to WebSocket at /ws/connect.
/// Extracts client/device identification from HTTP headers (same as
/// Propeller's websocket_handler.go), then runs the same event loop.
pub async fn ws_connect(
    State(state): State<Arc<WsState>>,
    ws: WebSocketUpgrade,
    headers: HeaderMap,
) -> impl IntoResponse {
    let client_config = state.client_config.clone();

    // extract client id from headers
    let client_id = headers
        .get(&client_config.header)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();

    let device = if client_config.enable_device_support {
        let device_id = headers
            .get(&client_config.device_header)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string();

        let mut attrs = HashMap::new();
        for header_key in &client_config.device_attribute_headers {
            if let Some(val) = headers.get(header_key).and_then(|v| v.to_str().ok()) {
                attrs.insert(header_key.clone(), val.to_string());
            }
        }

        if !device_id.is_empty() {
            Some(Device {
                id: device_id,
                attributes: attrs,
            })
        } else {
            None
        }
    } else {
        None
    };

    ws.on_upgrade(move |socket| handle_ws(state, socket, client_id, device))
}

async fn handle_ws(
    state: Arc<WsState>,
    socket: WebSocket,
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
        .subscribe_client(&client_id, device.as_ref())
        .await
    {
        Ok(s) => s,
        Err(e) => {
            error!(client_id = client_id, error = %e, "failed to subscribe ws client");
            return;
        }
    };

    let (mut ws_tx, mut ws_rx) = socket.split();

    // send connect ack
    let ack = serde_json::json!({
        "type": "connect_ack",
        "status": { "success": true }
    });
    if ws_tx
        .send(Message::Text(ack.to_string().into()))
        .await
        .is_err()
    {
        return;
    }

    // event loop: read from websocket and from broker subscription
    loop {
        tokio::select! {
            ws_msg = ws_rx.next() => {
                match ws_msg {
                    Some(Ok(Message::Text(text))) => {
                        debug!(client_id = client_id, "received ws message: {}", text);
                        // could parse and handle topic subscribe/unsubscribe here
                    }
                    Some(Ok(Message::Close(_))) | None => {
                        debug!(client_id = client_id, "ws client disconnected");
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
                        if ws_tx
                            .send(Message::Text(payload.to_string().into()))
                            .await
                            .is_err()
                        {
                            break;
                        }
                    }
                    None => {
                        break;
                    }
                }
            }
        }
    }

    state
        .push_service
        .unsubscribe_client(&client_id, &sub, device.as_ref())
        .await;
}
