# MiniQuantDeskV4 — Patch-by-Patch Implementation Plan (Capital-Safety First)

This document converts the consolidated audit output into an execution-ready patch plan.
**Rule:** one patch at a time. Each patch has a hard **Definition of Done (DoD)**.

---

## Global rules (apply to every patch)

- **One patch only** per PR/branch. No drive-by refactors.
- **Fail-closed** for anything “live”: default to **HALT/DISARM**.
- Every patch must include:
  - Scenario test(s) proving the patch invariant.
  - `cargo fmt`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
- If a patch adds DB behavior: migration + scenario test on a clean DB.
- If a patch touches execution: enforce **single choke-point** semantics (or move closer to it).

### Branch naming convention
Use:
- `claude/patch-L1-chokepoint`
- `claude/patch-L2-outbox-idempotency`
- `claude/patch-B1-no-lookahead`
(etc.)

### Standard verification commands
Run from `core-rs/`:
```bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

---

# LIVE TRADING SAFETY HARDENING PATCHES (L1–L10)

## PATCH L1 — Single Submission Choke-Point (No Gate Bypass)
**Addresses:** CRITICAL-1  
**Goal:** There exists exactly one code path that can result in broker submit/cancel/replace, and it always enforces: **integrity armed + risk OK + reconcile OK**.

**Likely files**
- `core-rs/crates/mqk-daemon/src/routes.rs`
- `core-rs/crates/mqk-execution/src/order_router.rs`
- `core-rs/crates/mqk-risk/*` (gate surface)
- `core-rs/crates/mqk-reconcile/*` (gate surface)

**Invariant**
- No other module can submit to broker without passing the choke-point gate.

**Definition of Done (DoD)**
- [ ] A single “gateway” function/API exists and is the only place broker actions can be invoked.
- [ ] All daemon routes that start/stop/halt/arm/disarm route through the gateway.
- [ ] Any legacy direct call paths are removed or made private and unreachable.
- [ ] New scenario test proves bypass is impossible (compile-time visibility OR runtime refusal):
  - e.g. `mqk-testkit/tests/scenario_only_gateway_can_submit.rs`
- [ ] Workspace: fmt + clippy(-D warnings) + tests all green.

---

## PATCH L2 — Outbox-First + Stable client_order_id Idempotency
**Addresses:** CRITICAL-3  
**Goal:** Broker submission is safe under network ambiguity and retries.

**Likely files**
- `core-rs/crates/mqk-db/src/lib.rs` (outbox helpers)
- `core-rs/crates/mqk-execution/src/order_router.rs`
- `core-rs/crates/mqk-broker-paper/*`

**Invariant**
- No broker submit occurs unless an outbox row exists first.
- Retries reuse the same **client_order_id** derived from a stable intent ID.

**Definition of Done (DoD)**
- [ ] Submitting an intent always creates an outbox row *before* broker submit.
- [ ] A deterministic idempotency key (intent_id → client_order_id) is used end-to-end.
- [ ] A retry path reuses the same client_order_id and does not create duplicates.
- [ ] Scenario tests:
  - `mqk-db/tests/scenario_outbox_first_enforced.rs`
  - `mqk-testkit/tests/scenario_retry_reuses_client_order_id.rs`
- [ ] Workspace green (fmt/clippy/tests).

---

## PATCH L3 — Outbox Dispatcher Claim/Lock Semantics (No Double Dispatch)
**Addresses:** HIGH-3  
**Goal:** Make dispatch concurrency-safe.

**Likely files**
- `core-rs/crates/mqk-db/src/lib.rs` (claim query/lock)
- Runtime loop module (daemon/orchestrator execution loop)

**Invariant**
- At most one dispatcher can claim a given outbox row at a time.
- Claimed rows deterministically progress to SUBMITTED/FAILED or revert.

**Definition of Done (DoD)**
- [ ] Add a DB claim API using `FOR UPDATE SKIP LOCKED` (or equivalent safe locking).
- [ ] Status transitions exist and are enforced: CREATED → CLAIMED → SUBMITTED/FAILED.
- [ ] Scenario test simulates two dispatchers; only one submits:
  - `mqk-db/tests/scenario_outbox_claim_lock_prevents_double_dispatch.rs`
- [ ] Workspace green.

---

## PATCH L4 — Minimal OMS State Machine v1 (Capital-Safety Events Only)
**Addresses:** CRITICAL-2  
**Goal:** Define deterministic behavior for partial fills, late fills, cancel-reject, replace semantics.

**Likely files**
- `core-rs/crates/mqk-execution/src/oms/state_machine.rs` (new)
- `core-rs/crates/mqk-db/*` (persisted order state, if required)

**Invariant**
- OMS transitions are explicit and deterministic; cancel is a request, not a terminal state.
- Event replays are idempotent (same event twice causes no double effect).

**Definition of Done (DoD)**
- [ ] Introduce explicit order states and event types.
- [ ] Implement legal transitions; illegal transitions halt/flag deterministically.
- [ ] Add tests for:
  - cancel-reject handling
  - partial fill then late fill
  - replace semantics (request vs. broker ack)
  - idempotent replay of the same event
  - e.g. `mqk-testkit/tests/scenario_oms_cancel_reject_handled.rs`
  - e.g. `mqk-testkit/tests/scenario_oms_partial_fill_then_late_fill.rs`
- [ ] Workspace green.

---

## PATCH L5 — Inbox Dedupe → Apply Gating (Atomic Insert→Apply)
**Addresses:** CRITICAL-4  
**Goal:** Fills apply to portfolio exactly once.

**Likely files**
- `core-rs/crates/mqk-db/src/lib.rs` (inbox insert)
- `core-rs/crates/mqk-portfolio/*` (apply path)
- Runtime fill handler

**Invariant**
- Apply occurs only if inbox insert is first-time (unique constraint).
- Duplicate/out-of-order events do not double-apply exposure/PnL.

**Definition of Done (DoD)**
- [ ] Inbox has DB uniqueness keyed by broker fill ID (or stable derived key).
- [ ] Apply path is gated: apply only if insert succeeded (not “already existed”).
- [ ] Scenario tests:
  - `mqk-db/tests/scenario_inbox_insert_then_apply_is_atomic.rs`
  - `mqk-testkit/tests/scenario_duplicate_fill_not_applied_twice.rs`
- [ ] Workspace green.

---

## PATCH L6 — Reconcile Hard Gate on Arm/Start + Periodic Reconcile Tick (Hard Halt)
**Addresses:** HIGH-1, HIGH-4  
**Goal:** Reconcile is mandatory for live arm/start and continuously enforced.

**Likely files**
- `core-rs/crates/mqk-reconcile/src/engine.rs`
- `core-rs/crates/mqk-daemon/src/routes.rs`
- Runtime loop scheduler

**Invariant**
- Arm/start impossible unless reconcile = CLEAN.
- Drift forces HALT + persistent DISARM.

**Definition of Done (DoD)**
- [ ] Live arm/start routes call reconcile and enforce CLEAN required.
- [ ] Periodic reconcile tick exists in live runtime loop.
- [ ] Drift triggers HALT + persistent DISARM (survives restart).
- [ ] Scenario tests:
  - `mqk-testkit/tests/scenario_reconcile_blocks_arm_and_start.rs`
  - `mqk-testkit/tests/scenario_periodic_reconcile_drift_halts.rs`
- [ ] Workspace green.

---

## PATCH L7 — Sticky DISARM + Deadman Across Restarts (Fail-Closed Boot)
**Addresses:** CRITICAL-5  
**Goal:** Restart cannot bypass disarm/deadman; boot defaults to DISARMED until explicit re-arm.

**Likely files**
- `core-rs/crates/mqk-integrity/src/engine.rs`
- `core-rs/crates/mqk-db/migrations/*` (if persistence missing)
- `core-rs/crates/mqk-daemon/src/state.rs`

**Invariant**
- Disarmed state is persisted and sticky across restarts.
- Deadman halt/disarm cannot be cleared by restart.

**Definition of Done (DoD)**
- [ ] Persistence exists for disarm (DB column/event stream) and is loaded on boot.
- [ ] On boot: system is DISARMED unless explicitly armed after passing checks.
- [ ] Deadman/Integrity disarm is sticky across restart.
- [ ] Scenario tests:
  - `mqk-testkit/tests/scenario_restart_defaults_to_disarmed.rs`
  - `mqk-testkit/tests/scenario_deadman_sticky_across_restart.rs`
- [ ] Workspace green.

---

## PATCH L8 — Snapshot Freshness + Monotonicity Enforcement
**Addresses:** HIGH-2  
**Goal:** Stale broker snapshots cannot drive sizing or mask drift.

**Likely files**
- `core-rs/crates/mqk-reconcile/src/snapshot_adapter.rs`
- Broker snapshot fetch path
- Risk sizing inputs

**Invariant**
- Snapshots include freshness metadata (timestamp/sequence).
- Non-monotonic or stale snapshots block execution.

**Definition of Done (DoD)**
- [ ] Snapshot structs include required freshness fields.
- [ ] Runtime tracks last accepted snapshot “watermark” and rejects older ones.
- [ ] Scenario test:
  - `mqk-reconcile/tests/scenario_snapshot_monotonicity_enforced.rs`
- [ ] Workspace green.

---

## PATCH L9 — Remove Floats From Execution Routing + Fix ID Mapping
**Addresses:** CRITICAL-6  
**Goal:** Remove nondeterministic float drift and incorrect cancel/replace targeting.

**Likely files**
- `core-rs/crates/mqk-execution/src/types.rs`
- `core-rs/crates/mqk-execution/src/order_router.rs`
- Broker adapter response types

**Invariant**
- Prices are integer micros in execution decisions.
- broker_order_id comes from broker ack and is persisted/mapped correctly.
- Replace/cancel always target the correct broker order.

**Definition of Done (DoD)**
- [ ] All execution routing uses integer micros (no f64 in decision surface).
- [ ] Mapping exists: internal intent/order IDs ↔ broker_order_id (from ack).
- [ ] Scenario tests:
  - `mqk-testkit/tests/scenario_replace_cancel_correct_order_targeted.rs`
  - Unit tests for deterministic price serialization/conversion
- [ ] Workspace green.

---

## PATCH L10 — Exposure Sanity Clamps (Last Line of Defense)
**Goal:** Even with bad upstream inputs, you don’t leak risk.

**Likely files**
- `core-rs/crates/mqk-risk/src/engine.rs`

**Invariant**
- NaN/overflow/negative sizes → HALT.
- Hard exposure caps enforced under all conditions.

**Definition of Done (DoD)**
- [ ] Checked arithmetic on notional/exposure computations.
- [ ] Explicit failure reasons and deterministic HALT path.
- [ ] Scenario tests:
  - `mqk-risk/tests/scenario_overflow_or_nan_halts.rs`
  - `mqk-risk/tests/scenario_negative_qty_halts.rs`
- [ ] Workspace green.

---

# BACKTEST TRUST HARDENING PATCHES (B1–B6)

## PATCH B1 — No Lookahead + Same-Bar Ambiguity Defaults (Conservative)
**Goal:** Prevent false confidence from lookahead fills.

**Likely files**
- `core-rs/crates/mqk-backtest/src/engine.rs`

**Invariant**
- Default policy forbids same-bar lookahead fills (unless explicitly configured).
- Ambiguity resolves worst-case by default.

**Definition of Done (DoD)**
- [ ] Default fill policy blocks same-bar lookahead.
- [ ] Ambiguity worst-case enforcement is covered by tests and cannot be silently disabled.
- [ ] Scenario tests:
  - `mqk-backtest/tests/scenario_no_same_bar_fill_by_default.rs`
  - `mqk-backtest/tests/scenario_ambiguity_worst_case_enforced.rs`
- [ ] Workspace green.

---

## PATCH B2 — Partial-Fill Stress Suite Mandatory For Promotion
**Goal:** Promotion requires surviving adversarial fill conditions.

**Invariant**
- Promotion requires passing partial fills + cancel/replace stress suite.

**Definition of Done (DoD)**
- [ ] Stress profile exists (partial fills, cancel/replace edge cases).
- [ ] Promotion fails if stress suite not run or not passed.
- [ ] Tests:
  - `mqk-promotion/tests/scenario_promotion_requires_partial_fill_stress.rs`
- [ ] Workspace green.

---

## PATCH B3 — Calendar/Sessions/Holidays Module (Session-Aware Gaps/Staleness)
**Invariant**
- Gap detection and staleness are session-aware and do not false-positive on holidays.

**Definition of Done (DoD)**
- [ ] Deterministic session calendar module exists (minimal v1 acceptable).
- [ ] DQ gate uses calendar for gap/stale logic.
- [ ] Scenario tests cover holiday/weekend gaps correctly.
- [ ] Workspace green.

---

## PATCH B4 — Corporate Actions Policy (Implement OR Guard/Forbid)
**Invariant**
- Either adjustments are applied deterministically OR backtests forbid affected assets/periods.

**Definition of Done (DoD)**
- [ ] Explicit policy exists (adjust vs forbid) and is enforced.
- [ ] Tests prove enforcement.
- [ ] Workspace green.

---

## PATCH B5 — Slippage Realism v1 (Deterministic & Conservative)
**Invariant**
- Slippage depends on a deterministic volatility proxy; applied conservatively.

**Definition of Done (DoD)**
- [ ] Slippage model uses deterministic volatility proxy (ATR/spread proxy).
- [ ] Tests show slippage worsens equity deterministically.
- [ ] Workspace green.

---

## PATCH B6 — Golden Artifacts: Hash-Lock + Immutability Gate
**Invariant**
- Promotion accepts only artifacts with validated manifest + hash chain.

**Definition of Done (DoD)**
- [ ] Artifacts include manifest with stable hashes.
- [ ] Promotion verifies manifest/hash-lock before scoring.
- [ ] Tests reject partial/corrupted/unlocked artifacts.
- [ ] Workspace green.

---

# Live Readiness Checklist (Must all be YES before real capital)

## Execution + idempotency
- [ ] Only one code path can submit broker orders (choke-point enforced)
- [ ] Broker submissions use stable client_order_id and are idempotent across retries
- [ ] Outbox written before broker submit, always
- [ ] Outbox dispatcher uses claim/lock semantics (no double dispatch)

## Risk + integrity enforcement
- [ ] Risk gate enforced at choke-point and cannot be bypassed
- [ ] Integrity disarm is persistent and sticky across restarts
- [ ] Deadman halt cannot be bypassed by restart

## Reconcile correctness
- [ ] Reconcile blocks live arm/start if any mismatch exists
- [ ] Reconcile runs periodically; drift forces HALT + persistent disarm
- [ ] Snapshot freshness monotonicity enforced

## Inbox/portfolio correctness
- [ ] Inbox dedupe keyed by broker fill ID (or stable derived key) with DB uniqueness
- [ ] Portfolio applies a fill only if inbox insert succeeded (idempotent apply proven)
- [ ] Out-of-order partial fills handled deterministically or force HALT
