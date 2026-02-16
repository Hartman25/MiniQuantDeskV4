# Arming Model Spec (V4)

Default posture: DISARMED. System must prove safety before trading.

States (PAPER/LIVE):
- DISARMED
- ARMING_PENDING
- ARMED
- DISARMING

## DISARMED
Broker/data/reconcile ok. No broker actions except reconcile repairs.

## ARMING_PENDING
Pre-arm checks:
- clean reconcile
- risk limits present
- config hash pinned
- kill switches enabled
Then require exact operator confirmation:
`ARM LIVE <last4> <daily_loss_limit>`

## ARMED
OMS outbox actions allowed. Kill switches continuously enforced.

## DISARMING
HALT_NEW; cancel non-protective orders; optional FLATTEN on critical; return DISARMED.

Kill switches:
- stale data, reject storm, desync, drawdown, missing stop, PDT guard

Deadman file (Windows safety):
runtime/ARMED.flag deletion triggers DISARM.
