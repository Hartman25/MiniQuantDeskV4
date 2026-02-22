# Secrets and Config Management (V4)

Secrets are environment-only; never stored in YAML/DB/manifests/logs.
Separate keys for:
- data vs broker
- paper vs live
- MAIN vs EXP (preferred)

Rotation:
DISARM -> update env -> restart -> reconcile CLEAN -> ARM
