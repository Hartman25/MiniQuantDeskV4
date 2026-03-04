MiniQuantDesk – Hot Restart (Scaffold Design)

Goal
- Provide an operator-controlled "hot restart" that is safe under crash/restart, retries, and broker event duplication.
- GUI can request restart and display runtime status.
- Runtime restarts FAIL-CLOSED (disarmed) by default. No auto-resume until you later add explicit gates.

Core Ideas
1) Leader lease (DB)
- Exactly one runtime instance is allowed to be "leader" at a time.
- Lease has expiry; leader must renew heartbeat.
- If lease lost => runtime disarms immediately.

2) Epoch
- Each successful leadership acquisition increments epoch.
- Outbound actions (outbox claim/dispatch, broker event apply) can be tagged/gated by epoch.
- GUI surfaces current epoch so you can see which instance is live.

3) Control plane endpoints (daemon)
- POST /control/disarm
- POST /control/arm
- POST /control/restart (request restart id; actual process restart is performed by supervisor/systemd/service manager)
- GET  /control/status (epoch, leader_id, lease expiry, armed/disarmed, last reconcile, etc.)

Phased rollout
Phase 1 (Safe Restart)
- Restart requests disarm first.
- On process boot: disarmed; acquire lease; run recovery; remain disarmed until manual arm.

Phase 2 (Warm Resume)
- Add explicit gates: reconcile OK, no drift, outbox healthy, risk checks pass.
- Only then allow "restart + resume".

What this scaffold provides
- SQL migration for runtime_leader lease table.
- Rust modules (db lease helper, daemon routes, runtime control plane stubs).
- GUI panel + API client stubs.
- Patch notes on where to wire these in later.
