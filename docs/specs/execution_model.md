# Execution Model Spec (V4)

Pluggable deterministic fill models for parity backtests.

Supports (v0):
- MARKET, LIMIT, STOP, STOP_LIMIT
- Brackets as entry + protective stop (TP optional future)
- Trailing stop via cancel/replace on bar close (never loosen)

Core assumptions:
- Swing default: market fills at next bar open ± slippage
- Same-bar ambiguity resolved by CONSERVATIVE_WORST_CASE for promotion

Parameters:
- spread bps, slippage bps, volatility multiplier, caps
- latency submit/cancel/replace (event-time)
- optional partial fills (stress)

Determinism required: same data+config+seed => identical outputs.
