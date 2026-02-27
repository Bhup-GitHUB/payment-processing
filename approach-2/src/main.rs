mod auth;
mod broker;
mod config;
mod error;
mod kv;
mod metrics;
mod push;
mod server;

use std::sync::Arc;

use axum::routing::get;
use axum::{Json, Router};
use tokio::signal;
use tonic::transport::Server as TonicServer;
use tracing::info;

use broker::redis_pubsub::RedisPubSub;
use auth::AuthService;
use config::AppConfig;
use kv::redis_kv::RedisKV;
use metrics::Metrics;
use push::service::PushService;
use server::grpc_handler::proto::push_service_server::PushServiceServer;
use server::grpc_handler::GrpcPushHandler;
use server::ws_handler::{ws_connect, WsState};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "propeller=debug,info".into()),
        )
        .init();

    let cfg = AppConfig::load("config/propeller")?;
    info!(service = cfg.service.name, "starting propeller");

    let pubsub = RedisPubSub::new(&cfg.broker.address).await?;
    let pubsub: Arc<dyn broker::PubSub> = Arc::new(pubsub);

    let redis_client = redis::Client::open(cfg.broker.address.as_str())?;
    let redis_conn = redis_client.get_multiplexed_async_connection().await?;
    let kv: Arc<dyn kv::KeyValue> = Arc::new(RedisKV::new(redis_conn));

    let metrics = Arc::new(Metrics::new());
    let auth = Arc::new(AuthService::new(cfg.auth.clone()));

    let push_service = Arc::new(PushService::new(
        pubsub.clone(),
        kv.clone(),
        cfg.client.clone(),
        metrics.clone(),
    ));

    let grpc_handler = GrpcPushHandler::new(
        push_service.clone(),
        auth.clone(),
        cfg.client.clone(),
        metrics.clone(),
    );
    let grpc_addr = cfg.grpc.address.parse()?;

    let grpc_server = TonicServer::builder()
        .add_service(PushServiceServer::new(grpc_handler))
        .serve(grpc_addr);

    info!(address = %cfg.grpc.address, "grpc server starting");

    let ws_state = Arc::new(WsState {
        push_service: push_service.clone(),
        auth: auth.clone(),
        client_config: cfg.client.clone(),
        metrics: metrics.clone(),
    });

    let metrics_ref = metrics.clone();
    let http_app = Router::new()
        .route("/ws/connect", get(ws_connect))
        .route("/health", get(|| async { "ok" }))
        .route("/metrics", get(move || {
            let m = metrics_ref.clone();
            let pubsub = pubsub.clone();
            async move {
                let mut snapshot = m.snapshot();
                snapshot.active_topic_listeners = pubsub.active_topic_listeners();
                Json(snapshot)
            }
        }))
        .with_state(ws_state);

    let http_addr: std::net::SocketAddr = cfg.http.address.parse()?;
    let http_listener = tokio::net::TcpListener::bind(http_addr).await?;

    info!(address = %cfg.http.address, "http/ws server starting");

    tokio::select! {
        result = grpc_server => {
            if let Err(e) = result {
                tracing::error!("grpc server error: {}", e);
            }
        }
        result = axum::serve(http_listener, http_app) => {
            if let Err(e) = result {
                tracing::error!("http server error: {}", e);
            }
        }
        _ = signal::ctrl_c() => {
            info!("received shutdown signal");
        }
    }

    info!("propeller stopped");
    Ok(())
}
