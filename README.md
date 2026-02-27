# Payment Processing Prototype — Two Approaches

This repo contains two separate prototypes under:

- `approach-1/`: payment backend + embedded realtime updates
- `approach-2/`: “Propeller”-style realtime gateway service (standalone)

They solve different problems and can be used independently or together.

---

## Approach 1: Payment backend with realtime inside the API

**Folder:** `approach-1/`

**What it is**
- A Rust payment-processing prototype that models a Zomato-style backend flow:
  - order creation
  - payment initiation with gateway routing
  - webhook ingestion
  - idempotent async processing (worker)
  - reconciliation loop
  - realtime confirmation events to clients

**Core components**
- `apps/api`: HTTP API, webhook receiver, WebSocket endpoint
- `apps/worker`: webhook processor + reconciliation scheduler
- Postgres: source of truth for orders/payments
- Redis:
  - Streams: webhook queue (`stream:webhooks`)
  - Pub/Sub: event fan-out channel(s)

**Realtime model (high level)**
- Clients connect to the payment API WebSocket endpoint.
- When webhooks arrive, the API enqueues them and can also push events to connected clients.

**When Approach 1 is a better fit**
- You want an end-to-end payment backend prototype (DB + worker + webhooks).
- You’re early-stage and prefer fewer moving parts (realtime “good enough” inside API).
- You don’t need sophisticated client/device routing, presence, or multi-device semantics yet.

**Trade-offs**
- Realtime is coupled to the payment API lifecycle and scaling characteristics.
- Harder to evolve realtime into a dedicated subsystem without refactoring.

---

## Approach 2: “Propeller” realtime gateway alongside payment services

**Folder:** `approach-2/`

**What it is**
- A standalone Rust service responsible for persistent client connections and event delivery.
- Inspired by CRED’s “Propeller” pattern: keep realtime connections out of core business services.

**Interfaces**
- WebSocket server for clients (HTTP/WS)
- gRPC server for backends/workers to push events into client channels

**State + routing**
- Redis Pub/Sub for message routing.
- Redis KV (hashes) for storing client/device state.
- Client identity is carried via headers (e.g., `x-client-id`), with optional device support.

**When Approach 2 is a better fit**
- You already have multiple backends/workers that need to push events to clients.
- You need targeted routing (per-client/per-device), device management, and clearer semantics.
- You expect a high number of concurrent connections and want to scale realtime separately.
- You want a cleaner boundary: business services publish events; realtime service delivers them.

**Trade-offs**
- More operational complexity (another service to deploy, observe, and scale).
- Requires defining an event contract and integration points (gRPC/Redis topics, auth headers).

---

## Which is “better” for what use case?

**Pick Approach 1 when**
- The goal is to build/validate payment flows end-to-end quickly.
- You want to demo webhook → worker → DB state changes, and realtime is secondary.
- Team size and ops maturity are small; simplicity is a priority.

**Pick Approach 2 when**
- Realtime delivery is a core capability (many connections, multi-device, routing guarantees).
- You anticipate multiple services publishing events (payments, refunds, delivery, offers, etc.).
- You want independent scaling and a clearer separation of concerns.

**Use both together when**
- `approach-1` owns payment business logic, state, and webhook processing.
- `approach-2` owns client connectivity and delivery.
- `approach-1` (API/worker) publishes “payment status changed” events to `approach-2`
  (via gRPC or Redis Pub/Sub), and clients listen only to `approach-2`.

---

## How to run

Each approach has its own `README.md` with exact run steps:

- `approach-1/README.md`
- `approach-2/README.md`

