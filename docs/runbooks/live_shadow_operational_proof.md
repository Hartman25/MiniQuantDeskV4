# Live-Shadow / Live-Capital End-to-End Operational Proof Path — MiniQuantDesk V4 (LO-03)

## Purpose

This document defines the explicit, repeatable end-to-end operational proof path for
live-shadow and live-capital posture in MiniQuantDesk V4.

It ties together the artifact chain (TV-01/02/03), the daemon control surface,
and the shadow-mode execution gate into a single auditable proof sequence.

**What this proof path establishes:**
- The candidate artifact was promoted through the canonical research pipeline (TV-01).
- The artifact passed the explicit deployability gate (TV-02).
- Parity evidence was recorded and trust gaps were stated (TV-03).
- The daemon starts fail-closed and requires explicit operator preconditions before running.
- Shadow mode does not execute live capital (mqk-strategy gate).
- The reconcile gate blocks arming until positions are clean (mqk-reconcile gate).
- Each daemon precondition failure produces an explicit, inspectable error (not a crash).

**What this proof path does NOT establish:**
- This proof path does NOT prove edge, profitability, or expected return.
- It does NOT prove the strategy is profitable under live conditions.
- It does NOT prove live execution is fully trusted.
- `live_trust_complete=False` remains the correct state after this proof.
- Broad institutional trust requires the full scorecard, not this proof path alone.

---

## Proof Sequence

The proof sequence is divided into three legs:

1. **Research artifact leg** — proves the artifact chain (TV-01/02/03).
2. **Daemon control leg** — proves the daemon preconditions and gates.
3. **Execution gate leg** — proves shadow-mode and reconcile-gate behavior.

Each leg has an explicit executable reference.  None of the legs alone is
sufficient; all three must be clear before declaring the pre-deployment
posture is honest.

---

## Leg 1: Research Artifact Leg (TV-01/02/03)

**What it proves:**
- A candidate signal_pack can be promoted with a canonical, deterministic artifact_id.
- The deployability gate evaluates the artifact and produces a stable pass/fail result.
- A parity evidence manifest records what shadow evidence exists and what trust gaps remain.

**Executable references:**

| Test file | What it proves |
|---|---|
| `research-py/tests/test_artifact_contract.py` | TV-01: canonical promoted manifest, stable artifact_id, producer→consumer agreement |
| `research-py/tests/test_deployability_gate.py` | TV-02: gate evaluates deterministically, each check independently provable, round-trip stable |
| `research-py/tests/test_parity_evidence.py` | TV-03: parity evidence chains TV-01/TV-02, live_trust_complete always false, trust gaps explicit |

**How to run:**
```
cd research-py
python -m pytest tests/test_artifact_contract.py tests/test_deployability_gate.py tests/test_parity_evidence.py -v
```

**Expected result:** All tests pass (74 tests as of LO-03 landing).

**What to check in artifact files before proceeding:**
- `promoted_manifest.json` — `schema_version=promoted-v1`, `stage=promoted`, all required_files present.
- `deployability_gate.json` — `passed=true`, all four checks pass.
- `parity_evidence.json` — `gate_passed=true`, `live_trust_complete=false`, `live_trust_gaps` non-empty.

If `deployability_gate.json` shows `passed=false`, do not proceed to live or shadow.
Review the failing checks and fix the artifact before re-evaluating.

---

## Leg 2: Daemon Control Leg

**What it proves:**
- The daemon boots fail-closed (disarmed, idle).
- Operator preconditions are enforced: token required, DB required, arm required.
- Mode-change is guided, not silent.
- Recovery states after restart are explicit and honest.

**Executable references:**

| Test file | What it proves |
|---|---|
| `crates/mqk-daemon/tests/scenario_stressed_recovery_lo02.rs` | LO-02 matrix: SR-01..SR-08 in-process recovery behaviors |
| `crates/mqk-daemon/tests/scenario_live_shadow_preflight_lo03.rs` | LO-03 preflight: daemon precondition chain for live-shadow/live-capital |
| `crates/mqk-daemon/tests/scenario_daemon_boot_is_fail_closed.rs` | Boot: disarmed at start, token gate, arm-then-DB gate |
| `crates/mqk-daemon/tests/scenario_daemon_runtime_lifecycle.rs` | DB-backed: durable halt, deadman, restart recovery |

**How to run (in-process, no DB required):**
```
cd core-rs
cargo test -p mqk-daemon scenario_stressed_recovery_lo02
cargo test -p mqk-daemon scenario_live_shadow_preflight_lo03
cargo test -p mqk-daemon scenario_daemon_boot_is_fail_closed
```

**How to run (DB-backed, requires MQK_DATABASE_URL):**
```
MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
  cargo test -p mqk-daemon scenario_daemon_runtime_lifecycle -- --include-ignored
```

**What to confirm:**
- Fresh boot: `GET /v1/health` → ok=true, `GET /v1/status` → state=idle, integrity_armed=false.
- Without DB: `POST /v1/run/start` → 503, not crash.
- Without arm: `POST /v1/run/start` → 403 gate=integrity_armed, not crash.
- Mode-change attempt: → 409 + guidance body with preconditions and steps.

---

## Leg 3: Execution Gate Leg

**What it proves:**
- Shadow mode does not execute live capital.
- Reconcile gate blocks arming when positions are dirty.

**Executable references:**

| Test file | What it proves |
|---|---|
| `crates/mqk-strategy/tests/scenario_shadow_mode_does_not_execute.rs` | Shadow mode sets IntentMode::Shadow and should_execute() returns false |
| `crates/mqk-reconcile/tests/scenario_reconcile_gate_blocks_live_arm.rs` | Dirty reconcile → is_clean_reconcile() returns false → gate blocks |
| `crates/mqk-reconcile/tests/scenario_reconcile_required_before_live.rs` | Clean reconcile allows arming; dirty reconcile blocks |

**How to run:**
```
cd core-rs
cargo test -p mqk-strategy scenario_shadow_mode_does_not_execute
cargo test -p mqk-reconcile
```

**What to confirm:**
- Shadow mode: `r.intents.mode == IntentMode::Shadow`, `r.intents.should_execute() == false`.
- Dirty reconcile (position mismatch): `is_clean_reconcile() == false`.
- Clean reconcile (positions match): `is_clean_reconcile() == true`.

---

## Pre-Deployment Checklist (Summary)

Use this checklist before transitioning to live-shadow or live-capital posture.

### Artifact chain (TV-01/02/03)
- [ ] `promoted_manifest.json` exists and schema_version=promoted-v1
- [ ] `deployability_gate.json` shows passed=true with all four checks passing
- [ ] `parity_evidence.json` exists with gate_passed=true and live_trust_gaps documented
- [ ] All TV research-py tests pass (74 tests)

### Daemon preconditions
- [ ] `GET /v1/health` returns ok=true
- [ ] `GET /api/v1/system/status` shows no warnings, db_status not unavailable
- [ ] `GET /api/v1/reconcile/status` is clean
- [ ] `MQK_OPERATOR_TOKEN` is set (operator routes require auth)
- [ ] `MQK_DATABASE_URL` is set (run/start requires DB)
- [ ] All LO-02 in-process recovery tests pass
- [ ] All LO-03 preflight tests pass

### Execution gates
- [ ] Shadow mode test passes (strategy does not execute in shadow)
- [ ] Reconcile gate test passes (dirty positions block arming)

### Operator step to start a live-shadow run
Once all of the above are confirmed:
1. Arm: `POST /v1/integrity/arm`
2. Start: `POST /v1/run/start`
3. Verify: `GET /v1/status` → state=running, `GET /api/v1/system/status` → runtime_status=running
4. Observe: monitor `deadman_status` stays healthy; review `alpaca_ws_continuity`.

---

## Remaining Trust Gaps (Always State These Explicitly)

The following gaps are not resolved by this proof path.  They are surfaced
explicitly in `parity_evidence.json` and must remain in this document.

1. TV-02 gate evaluates historical metrics only; no live fill data verified.
2. No shadow-mode execution against live broker has been run for any specific artifact.
3. Live slippage and market-impact costs are not modelled in backtest metrics.
4. Broker execution latency and partial-fill behavior not proven for any specific artifact.
5. LO-03 operator proof is complete; live deployment authorization review remains
   an operator responsibility and is not automatically granted by this proof path.

**`live_trust_complete` remains False.**
It becomes True only when operator authorization is explicitly granted after
reviewing all of the above gaps with actual live-shadow execution evidence.

---

## Proof Repeatability

This proof path is repeatable because:
- All artifact chain tests are deterministic (content-addressed IDs, injected timestamps).
- All daemon control tests are in-process and do not require external state.
- All execution gate tests are pure (no broker, no DB, no network).
- The exact test files and commands are named above.

An auditor can reproduce this proof path from scratch by:
1. Checking out the repo.
2. Running the three test groups listed in each leg.
3. Inspecting artifact files in a promoted artifact directory.
4. Comparing results against this document.
