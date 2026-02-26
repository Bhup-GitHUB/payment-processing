use anyhow::{anyhow, Result};
use async_trait::async_trait;
use chrono::{Duration, Utc};
use domain::models::{GatewayWebhookEvent, PaymentMethod, PaymentStatus};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct CheckoutSession {
    pub gateway: String,
    pub checkout_url: String,
    pub expires_at: chrono::DateTime<Utc>,
}

#[async_trait]
pub trait PaymentGateway: Send + Sync {
    fn name(&self) -> &'static str;
    async fn create_checkout(
        &self,
        order_id: Uuid,
        payment_id: Uuid,
        method: &PaymentMethod,
    ) -> Result<CheckoutSession>;
    async fn verify_webhook_signature(&self, event: &GatewayWebhookEvent, secret: &str) -> bool;
    async fn fetch_payment_status(&self, _payment_id: Uuid) -> Result<PaymentStatus>;
}

#[derive(Debug, Clone)]
pub struct MockGateway {
    pub gateway_name: &'static str,
    pub should_fail: bool,
}

#[async_trait]
impl PaymentGateway for MockGateway {
    fn name(&self) -> &'static str {
        self.gateway_name
    }

    async fn create_checkout(
        &self,
        order_id: Uuid,
        payment_id: Uuid,
        _method: &PaymentMethod,
    ) -> Result<CheckoutSession> {
        if self.should_fail {
            return Err(anyhow!("{} unavailable", self.gateway_name));
        }

        Ok(CheckoutSession {
            gateway: self.gateway_name.to_string(),
            checkout_url: format!(
                "https://mock-{}.example/checkout?order_id={}&payment_id={}",
                self.gateway_name.to_lowercase(),
                order_id,
                payment_id
            ),
            expires_at: Utc::now() + Duration::minutes(15),
        })
    }

    async fn verify_webhook_signature(&self, event: &GatewayWebhookEvent, secret: &str) -> bool {
        event.signature == format!("{}:{}", self.gateway_name, secret)
    }

    async fn fetch_payment_status(&self, _payment_id: Uuid) -> Result<PaymentStatus> {
        Ok(PaymentStatus::Success)
    }
}

pub struct SmartRouter {
    gateways: Vec<Box<dyn PaymentGateway>>,
}

impl SmartRouter {
    pub fn new(gateways: Vec<Box<dyn PaymentGateway>>) -> Self {
        Self { gateways }
    }

    pub async fn pick_checkout(
        &self,
        order_id: Uuid,
        payment_id: Uuid,
        method: &PaymentMethod,
    ) -> Result<CheckoutSession> {
        let mut last_error = None;
        for gateway in &self.gateways {
            match gateway.create_checkout(order_id, payment_id, method).await {
                Ok(session) => return Ok(session),
                Err(err) => {
                    last_error = Some(anyhow!("{} failed: {err}", gateway.name()));
                }
            }
        }

        Err(last_error.unwrap_or_else(|| anyhow!("no gateway configured")))
    }
}
