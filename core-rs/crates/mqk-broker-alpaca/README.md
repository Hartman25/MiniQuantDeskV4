mqk-broker-alpaca

Implements `mqk-execution::BrokerAdapter` for Alpaca Markets via real HTTP and WebSocket calls.

## What is implemented

- `submit_order`  — POST /v2/orders (real reqwest blocking call; AmbiguousSubmit on non-connect errors)
- `cancel_order`  — DELETE /v2/orders/{id}
- `replace_order` — GET /v2/orders/{id} for filled_qty, then PATCH; fail-closed if filled_qty malformed
- `fetch_events`  — GET /v2/account/activities polling (FILL/PARTIAL_FILL only; cursor = last activity id)

WebSocket transport (`alpaca_ws_transport.rs` in mqk-daemon): connects, authenticates, subscribes,
marks continuity Live on confirmation, and routes frames through `process_ws_inbound_batch`.
Reconnects with GapDetected on disconnect.  Spawned at daemon boot for paper+alpaca deployments.

## Credentials

Selected by `DeploymentMode` at daemon startup:

- Paper: `ALPACA_API_KEY_PAPER` / `ALPACA_API_SECRET_PAPER` / `ALPACA_PAPER_BASE_URL`
- Live:  `ALPACA_API_KEY_LIVE`  / `ALPACA_API_SECRET_LIVE`

See `.env.local.example` for canonical names.

## Scope

Paper-trading path (Paper + Alpaca adapter) is proven and operator-truthful.
Live-capital deployment is a separate gate chain and is not the scope of this crate.
