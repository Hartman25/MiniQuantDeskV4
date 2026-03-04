Hot Restart Scaffold – What to do next (one patch at a time)

This zip adds NEW FILES only. Nothing is wired in.

Recommended patch order:
1) DB-LEASE-1
   - Apply migration: runtime_leader_lease table
   - Implement lease functions in mqk-db and tests (single-flight acquisition, expiry, renewal)

2) DAEMON-CTRL-1
   - Add /control/status endpoint; read-only, no side effects
   - Add /control/disarm (writes disarm state)

3) RUNTIME-CTRL-1
   - Runtime uses lease on startup; disarms if lease lost
   - Runtime publishes status snapshot (epoch, leader_id)

4) RESTART-1
   - /control/restart writes restart request record
   - Supervisor watches DB or file trigger and restarts process (out of scope here)

5) GUI-CTRL-1
   - Add RuntimeControlPanel; display status and call disarm/restart/arm

Safety default:
- Never auto-arm after restart until you have recovery + reconcile + idempotency tests that prove safe.
