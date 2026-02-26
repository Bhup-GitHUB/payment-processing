use anyhow::Result;
use domain::models::{GatewayWebhookEvent, OrderStatus, PaymentMethod, PaymentStatus};
use sqlx::{Pool, Postgres};
use uuid::Uuid;

#[derive(Clone)]
pub struct Repository {
    pub db: Pool<Postgres>,
}

impl Repository {
    pub fn new(db: Pool<Postgres>) -> Self {
        Self { db }
    }

    pub async fn create_order(&self, user_id: &str, amount: i64, currency: &str) -> Result<Uuid> {
        let order_id = Uuid::new_v4();
        sqlx::query(
            "INSERT INTO orders (id, user_id, amount, currency, status) VALUES ($1, $2, $3, $4, 'PENDING')",
        )
        .bind(order_id)
        .bind(user_id)
        .bind(amount)
        .bind(currency)
        .execute(&self.db)
        .await?;
        Ok(order_id)
    }

    pub async fn create_payment(
        &self,
        order_id: Uuid,
        gateway: &str,
        method: &PaymentMethod,
        checkout_url: &str,
    ) -> Result<Uuid> {
        let payment_id = Uuid::new_v4();
        let method_str = match method {
            PaymentMethod::Upi => "UPI",
            PaymentMethod::Card => "CARD",
        };
        sqlx::query(
            "INSERT INTO payments (id, order_id, gateway, method, checkout_url, status) VALUES ($1, $2, $3, $4, $5, 'PENDING')",
        )
        .bind(payment_id)
        .bind(order_id)
        .bind(gateway)
        .bind(method_str)
        .bind(checkout_url)
        .execute(&self.db)
        .await?;
        Ok(payment_id)
    }

    pub async fn mark_payment_success(&self, payment_id: Uuid, order_id: Uuid) -> Result<()> {
        let mut tx = self.db.begin().await?;
        sqlx::query("UPDATE payments SET status = 'SUCCESS', updated_at = now() WHERE id = $1")
            .bind(payment_id)
            .execute(&mut *tx)
            .await?;
        sqlx::query("UPDATE orders SET status = 'PAID', updated_at = now() WHERE id = $1")
            .bind(order_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(())
    }

    pub async fn log_transaction(&self, event: &GatewayWebhookEvent) -> Result<()> {
        let status = match event.status {
            PaymentStatus::Pending => "PENDING",
            PaymentStatus::Success => "SUCCESS",
            PaymentStatus::Failed => "FAILED",
        };
        sqlx::query("INSERT INTO transactions (id, payment_id, gateway, gateway_txn_id, webhook_event_id, status, raw_payload) VALUES ($1, $2, $3, $4, $5, $6, $7) ON CONFLICT (gateway, webhook_event_id) DO NOTHING")
            .bind(Uuid::new_v4())
            .bind(event.payment_id)
            .bind(&event.gateway)
            .bind(&event.gateway_txn_id)
            .bind(&event.event_id)
            .bind(status)
            .bind(serde_json::to_string(event)?)
            .execute(&self.db)
            .await?;
        Ok(())
    }

    pub async fn pending_payments(&self) -> Result<Vec<(Uuid, Uuid, String)>> {
        let rows = sqlx::query_as::<_, (Uuid, Uuid, String)>(
            "SELECT id, order_id, gateway FROM payments WHERE status = 'PENDING' ORDER BY created_at ASC LIMIT 100",
        )
        .fetch_all(&self.db)
        .await?;
        Ok(rows)
    }

    pub async fn insert_settlement(
        &self,
        payment_id: Uuid,
        gateway: &str,
        amount: i64,
    ) -> Result<()> {
        sqlx::query("INSERT INTO settlements (id, payment_id, gateway, settlement_ref, amount, status) VALUES ($1, $2, $3, $4, $5, 'MATCHED') ON CONFLICT DO NOTHING")
            .bind(Uuid::new_v4())
            .bind(payment_id)
            .bind(gateway)
            .bind(format!("settle-{}", payment_id))
            .bind(amount)
            .execute(&self.db)
            .await?;
        Ok(())
    }
}
