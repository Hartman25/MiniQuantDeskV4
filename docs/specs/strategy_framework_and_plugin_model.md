# Strategy Framework and Plugin Model (V4)

Strategies output TARGET POSITIONS; core converts to orders.

Contract:
- on_bar -> SignalIntents (target_qty/target_notional)
- optional on_fill/on_timer intents

Context provides bounded recent bars window; no DB/broker access.

Shadow mode:
- strategies run but cannot trade; emits SHADOW intents for parity checks.

Determinism required (event stream + config + seed).
