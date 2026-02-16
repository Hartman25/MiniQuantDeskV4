# Broker Adapter Contract (V4)

Broker adapters implement:
submit/cancel/replace + fetch snapshots (orders/fills/positions/account).

Must set client_order_id; fills must have unique broker_fill_id.

Idempotency safe:
duplicates tolerated; outbox/inbox enforce uniqueness.

SimBroker must follow same contract for parity backtests.
