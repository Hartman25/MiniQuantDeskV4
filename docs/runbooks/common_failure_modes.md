# Runbooks — Common Failure Modes (V4)

Stale data:
- verify vendor connectivity + clock
- restart feed
- reconcile CLEAN
- arm

Reject storm:
- inspect reject reasons
- fix risk/account issues
- reconcile CLEAN
- arm

Broker desync:
- DISARM
- inspect broker snapshot
- close/cancel safely
- reconcile CLEAN
- arm

Missing protective stop:
- attempt stop placement
- if cannot confirm: flatten + disarm

Safe restart:
DISARM -> stop -> start DISARMED -> reconcile CLEAN -> ARM
