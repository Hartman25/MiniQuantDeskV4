# Reconciliation Spec (V4)

Continuous broker reconciliation + invariant checks.
Reconcile allowed even when DISARMED. Trading requires CLEAN reconcile.

## 1) Inputs
Broker:
- open orders, recent fills, positions, account snapshot
Internal:
- OMS orders/states, fills, fill-driven ledger positions, protective stop invariant

## 2) Phases
0) snapshot capture (hash raw payloads)
1) normalize (symbology -> instrument_id)
2) diff (unknown broker order, missing internal, state mismatch, fill mismatch, position mismatch, missing stop)
3) classify (INFO/WARN/CRITICAL)
4) action (INFO log, WARN halt_new, CRITICAL disarm)
5) limited repairs (only if provably ours via client_order_id prefix)
6) verify + finalize (CLEAN/WARN/CRITICAL)

## 3) Protective stop invariant
Open position must have venue-resting protective stop.
Missing stop => CRITICAL:
- attempt place/repair
- if cannot confirm: FLATTEN (configurable) + DISARM

## 4) Engine scoping
Reconcile is engine-scoped via client_order_id prefix (MAIN_/EXP_).
Unknown orders CRITICAL only inside namespace.

## 5) Required tests
- unknown broker order => CRITICAL + DISARM
- fill mismatch => CRITICAL
- position mismatch => CRITICAL
- missing stop => CRITICAL
- engine-scoped logic correct
