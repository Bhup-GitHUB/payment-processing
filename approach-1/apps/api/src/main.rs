use adapters::gateway::{MockGateway, PaymentGateway, SmartRouter};
use adapters::queue::WebhookQueue;
use adapters::repository::Repository;
use anyhow::{Context, Result};
use axum::extract::{ws::WebSocketUpgrade, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use domain::models::{GatewayWebhookEvent, PaymentMethod};
use futures_util::StreamExt;
use redis::Client;
use serde::{Deserialize, Serialize};
use sqlx::{Pool, Postgres};
use std::collections::HashMap;
use std::env;
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::sync::broadcast;
use tracing::{error, info};
use uuid::Uuid;

#[derive(Clone)]
struct AppState {
    repo: Repository,
    queue: WebhookQueue,
    router: Arc<SmartRouter>,
    ws_tx: broadcast::Sender<String>,
}

#[derive(Debug, Deserialize)]
struct CreateOrderRequest {
    user_id: String,
    amount: i64,
    currency: String,
}

#[derive(Debug, Serialize)]
struct CreateOrderResponse {
    order_id: Uuid,
    status: &'static str,
}

#[derive(Debug, Deserialize)]
struct InitiatePaymentRequest {
    order_id: Uuid,
    payment_method: PaymentMethod,
}

#[derive(Debug, Serialize)]
struct InitiatePaymentResponse {
    payment_id: Uuid,
    gateway: String,
    checkout_url: String,
    expires_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Deserialize)]
struct WsQuery {
    user_id: String,
}

#[tokio::main]
async fn main() -> Result<()> {
    let database_url = env::var("DATABASE_URL").context("DATABASE_URL missing")?;
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());
    let api_addr = env::var("API_ADDR").unwrap_or_else(|_| "0.0.0.0:8080".to_string());

    let db = Pool::<Postgres>::connect(&database_url).await?;
    let repo = Repository::new(db);
    let queue = WebhookQueue::new(&redis_url, "stream:webhooks")?;
    let redis_client = Client::open(redis_url)?;

    let gateways: Vec<Box<dyn PaymentGateway>> = vec![
        Box::new(MockGateway {
            gateway_name: "Razorpay",
            should_fail: false,
        }),
        Box::new(MockGateway {
            gateway_name: "Cashfree",
            should_fail: false,
        }),
        Box::new(MockGateway {
            gateway_name: "Juspay",
            should_fail: false,
        }),
    ];

    let router = Arc::new(SmartRouter::new(gateways));
    let (tx, _) = broadcast::channel(1024);

    let state = AppState {
        repo,
        queue,
        router,
        ws_tx: tx,
    };
    tokio::spawn(forward_pubsub_to_ws(redis_client, state.ws_tx.clone()));

    let app = Router::new()
        .route("/healthz", get(healthz))
        .route("/orders", post(create_order))
        .route("/orders/:order_id", get(get_order))
        .route("/payments/initiate", post(initiate_payment))
        .route("/webhooks/:gateway", post(receive_webhook))
        .route("/ws", get(ws_handler))
        .with_state(state);

    let addr: SocketAddr = api_addr.parse()?;
    info!(%addr, "payment api listening");
    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
}

async fn create_order(
    State(state): State<AppState>,
    Json(req): Json<CreateOrderRequest>,
) -> Result<Json<CreateOrderResponse>, ApiError> {
    let order_id = state
        .repo
        .create_order(&req.user_id, req.amount, &req.currency)
        .await
        .map_err(ApiError::internal)?;

    Ok(Json(CreateOrderResponse {
        order_id,
        status: "PENDING",
    }))
}

async fn initiate_payment(
    State(state): State<AppState>,
    Json(req): Json<InitiatePaymentRequest>,
) -> Result<Json<InitiatePaymentResponse>, ApiError> {
    let payment_id = Uuid::new_v4();
    let session = state
        .router
        .pick_checkout(req.order_id, payment_id, &req.payment_method)
        .await
        .map_err(ApiError::internal)?;

    let created_payment_id = state
        .repo
        .create_payment(
            req.order_id,
            &session.gateway,
            &req.payment_method,
            &session.checkout_url,
        )
        .await
        .map_err(ApiError::internal)?;

    Ok(Json(InitiatePaymentResponse {
        payment_id: created_payment_id,
        gateway: session.gateway,
        checkout_url: session.checkout_url,
        expires_at: session.expires_at,
    }))
}

async fn receive_webhook(
    State(state): State<AppState>,
    Path(gateway): Path<String>,
    Json(mut event): Json<GatewayWebhookEvent>,
) -> Result<StatusCode, ApiError> {
    event.gateway = gateway;
    let payload = serde_json::to_string(&event).map_err(ApiError::internal)?;
    state
        .queue
        .enqueue(&payload)
        .await
        .map_err(ApiError::internal)?;
    let _ = state.ws_tx.send(payload);
    Ok(StatusCode::ACCEPTED)
}

async fn get_order(
    State(state): State<AppState>,
    Path(order_id): Path<Uuid>,
) -> Result<Json<serde_json::Value>, ApiError> {
    let row = sqlx::query_as::<_, (Uuid, String)>("SELECT id, status FROM orders WHERE id = $1")
        .bind(order_id)
        .fetch_optional(&state.repo.db)
        .await
        .map_err(ApiError::internal)?;

    match row {
        Some((id, status)) => Ok(Json(
            serde_json::json!({ "order_id": id, "status": status }),
        )),
        None => Err(ApiError::not_found("order not found")),
    }
}

async fn ws_handler(
    ws: WebSocketUpgrade,
    Query(params): Query<WsQuery>,
    State(state): State<AppState>,
) -> Response {
    ws.on_upgrade(move |socket| ws_stream(socket, params.user_id, state.ws_tx.subscribe()))
}

async fn ws_stream(
    mut socket: axum::extract::ws::WebSocket,
    _user_id: String,
    mut rx: broadcast::Receiver<String>,
) {
    use axum::extract::ws::Message;
    while let Ok(msg) = rx.recv().await {
        if socket.send(Message::Text(msg.into())).await.is_err() {
            break;
        }
    }
}

async fn forward_pubsub_to_ws(redis_client: Client, ws_tx: broadcast::Sender<String>) {
    let mut pubsub = match redis_client.get_async_pubsub().await {
        Ok(pubsub) => pubsub,
        Err(err) => {
            error!("failed creating redis pubsub client: {err}");
            return;
        }
    };

    if let Err(err) = pubsub.subscribe("payments.confirmed").await {
        error!("failed subscribing to payments.confirmed: {err}");
        return;
    }

    let mut stream = pubsub.on_message();
    while let Some(message) = stream.next().await {
        let payload: String = message.get_payload().unwrap_or_default();
        let _ = ws_tx.send(payload);
    }
}

struct ApiError {
    code: StatusCode,
    message: String,
}

impl ApiError {
    fn internal(err: impl std::fmt::Display) -> Self {
        error!("internal error: {err}");
        Self {
            code: StatusCode::INTERNAL_SERVER_ERROR,
            message: "internal server error".to_string(),
        }
    }

    fn not_found(message: &str) -> Self {
        Self {
            code: StatusCode::NOT_FOUND,
            message: message.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.code, Json(HashMap::from([("error", self.message)]))).into_response()
    }
}
