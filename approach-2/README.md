# Propeller (Rust)

A Rust implementation of a real-time bidirectional communication platform, inspired by CRED's Propeller.

Sits alongside your payment backends and handles all persistent client connections, event routing, and device management.

## Stack
- Rust (Tokio, tonic for gRPC, axum for HTTP/WebSocket)
- Redis (Pub/Sub for message routing, Hashes for device state)
- Protobuf for the service contract

## Run

1. Start Redis:
   ```bash
   docker-compose up -d
   ```

2. Build and run:
   ```bash
   cargo run
   ```

   gRPC server starts on `:50051`, HTTP/WS server on `:8080`.

## Usage

### Connect a client (gRPC)
Use `grpcurl` or any gRPC client. Send an `authorization` bearer token with claims:
- `sub` or `client_id`
- `tenant_id` (configurable via `auth.tenant_claim`)

During migration, `x-client-id` still works when `auth.legacy_header_mode=true`.

### Connect a client (WebSocket)
```bash
websocat ws://localhost:8080/ws/connect \
  -H "Authorization: Bearer <jwt>" \
  -H "x-device-id: iphone-15"
```

### Send an event from your backend
```bash
grpcurl -plaintext -d '{
  "client_id": "user_123",
  "event": {
    "name": "payment.confirmed",
    "data": "eyJvcmRlcl9pZCI6ICIxMjMifQ=="
  }
}' \
  -H "authorization: Bearer <publisher-jwt>" \
  -H "x-api-key: dev-publisher-key" \
  localhost:50051 push.v1.PushService/SendEventToClientChannel
```

## Auth and Tenancy

- Auth is configured in `config/propeller.toml` under `[auth]`.
- Tenant is derived from JWT claim (`tenant_id` by default).
- Channels are tenant-scoped internally:
  - `tenant:{tenant}:client:{client}`
  - `tenant:{tenant}:client:{client}:device:{device}`
  - `tenant:{tenant}:topic:{topic}`
- `legacy_header_mode=true` allows temporary header-only auth fallback. Track and remove this mode after migration.

## Keepalive

- gRPC transport keepalive uses `[grpc].ping_interval_secs` and `[grpc].ping_timeout_secs`.
- WebSocket and gRPC stream-idle enforcement use `[keepalive]`:
  - `ws_ping_interval_secs`
  - `ws_pong_timeout_secs`
  - `grpc_stream_idle_timeout_secs`
- WS clients must reply to Ping frames with Pong.
- gRPC channel clients must send periodic inbound frames to stay active.

## Architecture

```
Frontend (Mobile/Web)
    |
    | gRPC bidi stream / WebSocket
    v
+-------------------+
|   Propeller        |    <-- this service
|  (Rust/Tokio)      |
+-------------------+
    |         ^
    | sub     | pub
    v         |
+-------------------+
|   Redis Pub/Sub    |
+-------------------+
          ^
          | publish event
          |
+-------------------+
|  Your Payment      |
|  Worker / Backend  |
+-------------------+
```
