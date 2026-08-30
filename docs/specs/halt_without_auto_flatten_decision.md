# Decision Record: HALT / DISARM Does Not Auto-Flatten

**Recorded by:** `MASTER-LEDGER-CURRENT-TRUTH-CLOSURE-01` (L1), 2026-08-30
**Baseline:** `main` @ `70ed507acfe02ef860b8378b9e5eddb25a36065d`
**Status:** DOCUMENTS EXISTING BEHAVIOR — no runtime behavior was changed to produce this record.

## Decision

Halting the runtime (kill-switch trip, operator halt, `Stop-Process`, deadman-file
deletion) never automatically submits a flatten (position-close) order in the
Paper/Live daemon. Flattening open positions is always a separate, explicit,
operator-confirmed action, and that action is itself **unavailable while the run
is halted or not armed** — an operator must investigate, restart, and re-arm
before flatten becomes available again.

## Evidence

### HALT / DISARM behavior (does not flatten)

- `docs/runbooks/operator_control_surface.md` §5 ("Emergency Abort Rules"):
  the halt command is `POST /api/v1/ops/action {action_key: "kill-switch"}` (or
  killing the daemon process directly). The documented post-halt sequence is:
  do NOT re-arm immediately → capture evidence → inspect halt reason via
  `GET /api/v1/audit/operator-actions` → follow halt recovery → restart cleanly
  → re-arm. No step submits an order.
- The daemon's live kill-switch state (`kill_switch_active`) is a simple boolean
  gate tracked directly in `mqk-daemon` (`api_types.rs` and consuming routes);
  tripping it blocks further order submission, it does not itself generate one.

### Explicit flatten path (separate, gated action)

- `docs/runbooks/operator_control_surface.md` §4 ("Safe Flatten Instructions"):
  "Flatten is the **only** operator action that submits a paper order." It
  requires `flatten_available=true` and empty `flatten_blockers`, confirmed
  market hours, and `live_routing_enabled=false` — checked *before* the
  operator issues `POST /api/v1/ops/action {action_key: "flatten-paper-positions"}`.
- The flatten-blocker table in that same section makes the ordering explicit:
  `arm_state != armed` → *"Not armed → Arm first, then flatten"*; and
  `runtime_status != running` → *"Runtime not active → Start runtime, then
  flatten"*. I.e. flatten is unavailable during a halt and only becomes
  available again after the operator has deliberately restored the run to
  armed/running.

### The `mqk-risk` crate's `RiskAction::FlattenAndHalt` is not wired into the live daemon

`mqk-risk::evaluate()` can return a `RiskAction::FlattenAndHalt` verdict (e.g.
for the `MissingProtectiveStop` kill-switch type, gated by
`RiskConfig::missing_protective_stop_flattens`), proven by
`core-rs/crates/mqk-risk/tests/scenario_auto_flatten_on_critical_event.rs`.
However:

- `mqk-daemon` does not depend on the `mqk-risk` crate at all (no `mqk-risk`
  entry in `core-rs/crates/mqk-daemon/Cargo.toml`, no `mqk_risk::`/`RiskAction`
  reference anywhere under `core-rs/crates/mqk-daemon/src/`).
- The only consumer of `RiskAction::FlattenAndHalt` that turns it into an
  actual position close is `mqk-backtest/src/engine.rs` — the research
  *simulation* engine, where "flattening" a simulated position has no
  real-world broker consequence.
- `mqk-testkit`'s own risk-gate test (`scenario_risk_engine_blocks_submit.rs`)
  documents `FlattenAndHalt` as treated identically to `Reject`/`Halt` for
  submission-blocking purposes — i.e. as a gate decision, not an executed
  action, in any context where it is actually consumed as a gate.

**Conclusion:** the enum name `FlattenAndHalt` describes an intended decision
inside the standalone `mqk-risk` crate and its backtest-simulation consumer.
It is not currently wired into the Paper/Live daemon's own halt path, so it
does not contradict the decision above — no live/paper kill-switch trip,
including a `MissingProtectiveStop` one, currently submits a flatten order
automatically. If `mqk-risk::evaluate()` is ever wired into the daemon in the
future, this decision record's claim would need to be re-verified against
that new call site.

## Rationale

Auto-submitting a broker/paper order as a side effect of a safety halt is
higher-risk than the halt itself: it fires exactly when the system has just
demonstrated something is not behaving as expected, is capital-affecting, and
cannot be undone. The accepted design instead treats halt as strictly
"stop and do not act further" and treats flatten as a distinct, always-manual,
precondition-gated action an operator takes only after evaluating the
halt reason.

## Non-goals of this record

This record does not change behavior. It does not evaluate whether
`RiskAction::FlattenAndHalt` *should* eventually be wired into the daemon —
that is a separate, unstarted design question, not addressed here.
