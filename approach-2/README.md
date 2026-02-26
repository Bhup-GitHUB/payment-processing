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
Use `grpcurl` or any gRPC client. Pass `x-client-id` and `x-device-id` as metadata headers when opening a `Channel` stream.

### Connect a client (WebSocket)
```bash
websocat ws://localhost:8080/ws/connect -H "x-client-id: user_123" -H "x-device-id: iphone-15"
```

### Send an event from your backend
```bash
grpcurl -plaintext -d '{
  "client_id": "user_123",
  "event": {
    "name": "payment.confirmed",
    "data": "eyJvcmRlcl9pZCI6ICIxMjMifQ=="
  }
}' localhost:50051 push.v1.PushService/SendEventToClientChannel
```

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
