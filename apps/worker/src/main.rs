use adapters::idempotency::IdempotencyStore;
use adapters::queue::WebhookQueue;
use adapters::realtime::RealtimePublisher;
use adapters::repository::Repository;
use anyhow::{Context, Result};
use chrono::Utc;
use domain::models::{GatewayWebhookEvent, PaymentStatus};
use sqlx::{Pool, Postgres};
use std::env;
use std::time::Duration;
use tokio::time::sleep;
use tracing::{error, info};

#[tokio::main]
async fn main() -> Result<()> {
    let database_url = env::var("DATABASE_URL").context("DATABASE_URL missing")?;
    let redis_url = env::var("REDIS_URL").unwrap_or_else(|_| "redis://127.0.0.1:6379".to_string());

    let db = Pool::<Postgres>::connect(&database_url).await?;
    let repo = Repository::new(db);
    let queue = WebhookQueue::new(&redis_url, "stream:webhooks")?;
    let idempotency = IdempotencyStore::new(&redis_url)?;
    let realtime = RealtimePublisher::new(&redis_url)?;

    let processor = tokio::spawn(process_webhooks(
        repo.clone(),
        queue.clone(),
        idempotency,
        realtime,
    ));
    let reconciler = tokio::spawn(run_reconciliation(repo));

    let _ = tokio::join!(processor, reconciler);
    Ok(())
}

async fn process_webhooks(
    repo: Repository,
    queue: WebhookQueue,
    idempotency: IdempotencyStore,
    realtime: RealtimePublisher,
) {
    let mut last_id = "0-0".to_string();

    loop {
        match queue.read_after(&last_id, 50).await {
            Ok(events) => {
                for (stream_id, payload) in events {
                    if let Err(err) = handle_event(&repo, &idempotency, &realtime, &payload).await {
                        error!("failed to process webhook: {err}");
                    }
                    last_id = stream_id;
                }
            }
            Err(err) => error!("queue read failed: {err}"),
        }

        sleep(Duration::from_millis(250)).await;
    }
}

async fn handle_event(
    repo: &Repository,
    idempotency: &IdempotencyStore,
    realtime: &RealtimePublisher,
    payload: &str,
) -> Result<()> {
    let event: GatewayWebhookEvent = serde_json::from_str(payload)?;
    let idem_key = format!("idemp:webhook:{}:{}", event.gateway, event.event_id);

    let should_process = idempotency.mark_if_absent(&idem_key, 3600).await?;
    if !should_process {
        info!(event_id = %event.event_id, "duplicate webhook skipped");
        return Ok(());
    }

    repo.log_transaction(&event).await?;
    if matches!(event.status, PaymentStatus::Success) {
        repo.mark_payment_success(event.payment_id, event.order_id)
            .await?;
        realtime.publish_payment_confirmed(payload).await?;
    }

    Ok(())
}

async fn run_reconciliation(repo: Repository) {
    loop {
        if let Err(err) = reconcile_once(&repo).await {
            error!("reconciliation failed: {err}");
        }
        sleep(Duration::from_secs(300)).await;
    }
}

async fn reconcile_once(repo: &Repository) -> Result<()> {
    let pending = repo.pending_payments().await?;
    for (payment_id, order_id, gateway) in pending {
        repo.mark_payment_success(payment_id, order_id).await?;
        repo.insert_settlement(payment_id, &gateway, 100).await?;
        info!(
            payment_id = %payment_id,
            reconciled_at = %Utc::now(),
            "reconciliation marked payment successful"
        );
    }
    Ok(())
}
