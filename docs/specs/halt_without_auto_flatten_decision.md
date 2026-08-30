# Decision Record: HALT / DISARM Does Not Auto-Flatten

**Recorded by:** `MASTER-LEDGER-CURRENT-TRUTH-CLOSURE-01` (L1), 2026-08-30
**Corrected by:** `FULL-SYSTEM-COMPLETION-SITUATIONAL-AUDIT-01` (L2), 2026-08-30 — the `mqk-risk`
wiring claim below was found incomplete during L2 research and is corrected in place
(the decision itself is unchanged; see the "Correction" note in that section).
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

### The `mqk-risk` crate's `RiskAction::FlattenAndHalt` reaches the live gate, but only as a deny — never as an executed order

**Correction (`FULL-SYSTEM-COMPLETION-SITUATIONAL-AUDIT-01`, L2, 2026-08-30):**
the paragraph originally here claimed `mqk-daemon` does not depend on
`mqk-risk` "at all." That was incomplete: `mqk-daemon` depends on
`mqk-runtime` (`mqk-daemon/Cargo.toml`), and `mqk-runtime` depends on
`mqk-risk` directly. `mqk-runtime/src/runtime_risk.rs`'s `RuntimeRiskGate`
wraps `mqk_risk::evaluate()` and implements `mqk_execution::gateway::RiskGate`,
which is wired into the live per-order gate pipeline
(`mqk-execution/src/gateway.rs::enforce_gates`, invoked via
`mqk-daemon/src/state/orchestrator_build.rs`) as the middle of three gates
checked on every order submission: `IntegrityGate::is_armed()` →
`RiskGate::evaluate_gate_for_request()` → `ReconcileGate::is_clean()`. So
`mqk_risk::evaluate()` — and by extension a `FlattenAndHalt` verdict from it
— genuinely does reach live/paper order submission. The decision below is
otherwise unchanged, because of what happens to that verdict once it arrives:

- `runtime_risk_decision_to_execution_decision`
  (`mqk-runtime/src/runtime_risk.rs`, the sole conversion from `mqk_risk`'s
  `RiskAction` to the gateway's `RiskDecision`) maps **every** `RiskAction`
  variant other than `Allow` — `Reject`, `Halt`, and `FlattenAndHalt` alike —
  to the same `RiskDecision::Deny(...)`. The match arm carries no
  `FlattenAndHalt`-specific case; the original `RiskAction` variant is not
  even preserved in the resulting `RiskDenial`. There is no code path from
  here that submits, enqueues, or otherwise executes a flatten order — a
  `FlattenAndHalt` verdict has exactly the same practical effect on the
  gateway as a plain `Reject`: the pending order is denied.
- The only mechanism that lets a *reducing* order (a real flatten) through
  this same gate is `RiskRequestContext.is_risk_reducing`, which the
  *caller* sets when it already intends to submit a risk-reducing order —
  the gate does not initiate this itself; it only permits an
  already-risk-reducing request through where a non-reducing one would be
  denied.
- `mqk-testkit`'s own risk-gate test (`scenario_risk_engine_blocks_submit.rs`)
  and `mqk-runtime`'s own gate tests
  (`evaluate_gate_for_request_denies_non_reducing_order_when_halted`,
  `evaluate_gate_for_request_allows_verified_flatten_when_halted`) confirm
  exactly this shape: deny by default, allow only a caller-verified
  risk-reducing request.

**Conclusion (revised, same practical outcome as before, more strongly
evidenced):** `mqk_risk::evaluate()`'s `FlattenAndHalt` decision is real
production code reachable from the live gate — the earlier "not wired at
all" framing was wrong — but the daemon's own conversion of that decision
into a gateway `Deny` means it still functions purely as "block this
order," never as "submit a flatten order automatically." No live/paper
kill-switch trip, including a `MissingProtectiveStop` one that produces
`FlattenAndHalt`, currently submits a flatten order automatically. The
enum name remains a latent misnomer for what the daemon actually does with
it today.

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
