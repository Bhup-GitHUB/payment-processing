# Rust Payment Processing Prototype

Prototype backend for a Zomato-style payment architecture: order creation, payment routing, async webhook processing, idempotency, reconciliation worker, and realtime payment confirmation events.

## Stack
- Rust (`axum`, `tokio`, `sqlx`)
- PostgreSQL
- Redis (Streams + Pub/Sub)

## Services
- `apps/api`: HTTP API + webhook receiver + WebSocket endpoint
- `apps/worker`: webhook processor + reconciliation scheduler

## Run
1. Start infra:
   ```bash
   docker-compose up -d
   ```
2. Set env:
   ```bash
   cp .env.example .env
   export $(cat .env | xargs)
   ```
3. Run migration:
   ```bash
   ./scripts/run-migrations.sh
   ```
4. Start API:
   ```bash
   cargo run -p payment-api
   ```
5. Start worker:
   ```bash
   cargo run -p payment-worker
   ```

## API Flow
1. Create order:
   ```bash
   curl -sX POST localhost:8080/orders -H 'Content-Type: application/json' -d '{"user_id":"u1","amount":45000,"currency":"INR"}'
   ```
2. Initiate payment:
   ```bash
   curl -sX POST localhost:8080/payments/initiate -H 'Content-Type: application/json' -d '{"order_id":"<ORDER_ID>","payment_method":"UPI"}'
   ```
3. Send gateway webhook:
   ```bash
   curl -sX POST localhost:8080/webhooks/Razorpay -H 'Content-Type: application/json' -d '{
     "event_id":"evt_1",
     "gateway":"Razorpay",
     "order_id":"<ORDER_ID>",
     "payment_id":"<PAYMENT_ID>",
     "gateway_txn_id":"txn_1",
     "status":"SUCCESS",
     "amount":45000,
     "signature":"Razorpay:dev-secret",
     "occurred_at":"2026-02-21T00:00:00Z"
   }'
   ```
4. Read order status:
   ```bash
   curl -s localhost:8080/orders/<ORDER_ID>
   ```

## Diagram Mapping
- Order Service: `/orders`
- Payment Service Core + Smart Router: `/payments/initiate` + `SmartRouter`
- Webhook Receiver: `/webhooks/{gateway}`
- Webhook Queue: Redis Stream `stream:webhooks`
- Webhook Processor + Idempotency: `apps/worker` + Redis key `idemp:webhook:*`
- Reconciliation Engine: worker task every 5 minutes
- Realtime Notification: Redis Pub/Sub channel `payments.confirmed` + `/ws`
