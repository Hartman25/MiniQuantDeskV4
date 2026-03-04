Alpaca Adapter Wiring Notes (scaffold)

This crate is intentionally NOT wired into the runtime yet.

Minimum wiring steps (future patches):
1) Add crate to core-rs workspace Cargo.toml members.
2) Define/implement the BrokerGateway trait in mqk-execution for AlpacaGateway.
3) Persist mapping table in DB:
   - internal_order_id (UUID)
   - idempotency_key (unique)
   - alpaca_order_id (unique once known)
   - last_known_status
4) Implement event ingestion:
   - REST snapshot poll (truth)
   - websocket stream (fast path)
   - dedupe keys for fills/order updates
5) Reconcile loop:
   - compare ledger vs broker snapshot
   - on divergence -> HaltAndDisarm
6) Tests:
   - recorded fixtures for duplicate/out-of-order events
   - crash window tests (submit before ack; ack before db commit; etc.)

Do NOT auto-arm after restart until reconcile passes.
