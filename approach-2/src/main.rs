mod broker;
mod config;
mod error;
mod kv;
mod push;
mod server;

use std::sync::Arc;

use axum::routing::get;
use axum::Router;
use tokio::signal;
use tonic::transport::Server as TonicServer;
use tracing::info;

use broker::redis_pubsub::RedisPubSub;
use config::AppConfig;
use kv::redis_kv::RedisKV;
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

    // connect to redis
    let pubsub = RedisPubSub::new(&cfg.broker.address).await?;
    let pubsub: Arc<dyn broker::PubSub> = Arc::new(pubsub);

    let redis_client = redis::Client::open(cfg.broker.address.as_str())?;
    let redis_conn = redis_client.get_multiplexed_async_connection().await?;
    let kv: Arc<dyn kv::KeyValue> = Arc::new(RedisKV::new(redis_conn));

    let push_service = Arc::new(PushService::new(
        pubsub.clone(),
        kv.clone(),
        cfg.client.clone(),
    ));

    // grpc server
    let grpc_handler = GrpcPushHandler::new(push_service.clone(), cfg.client.clone());
    let grpc_addr = cfg.grpc.address.parse()?;

    let grpc_server = TonicServer::builder()
        .add_service(PushServiceServer::new(grpc_handler))
        .serve(grpc_addr);

    info!(address = %cfg.grpc.address, "grpc server starting");

    // http/ws server
    let ws_state = Arc::new(WsState {
        push_service: push_service.clone(),
        client_config: cfg.client.clone(),
    });

    let http_app = Router::new()
        .route("/ws/connect", get(ws_connect))
        .route("/health", get(|| async { "ok" }))
        .with_state(ws_state);

    let http_addr: std::net::SocketAddr = cfg.http.address.parse()?;
    let http_listener = tokio::net::TcpListener::bind(http_addr).await?;

    info!(address = %cfg.http.address, "http/ws server starting");

    // run both servers concurrently, shut down on ctrl-c
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
