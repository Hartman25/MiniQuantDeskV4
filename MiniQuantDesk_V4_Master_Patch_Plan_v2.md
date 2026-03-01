# MiniQuantDesk V4 --- Master Patch Plan (Source of Truth)

This is the single authoritative patch tracker for **everything** in the
repo. It is designed for **one patch at a time** execution with manual
verification after each patch.

**Rules** - ONE patch per cycle. No bundling. - No refactors outside
patch scope. - No randomness or wall-clock time in decision logic. - No
hidden I/O. - All public API changes must be versioned. - Each patch
must end with a green build + relevant tests.

**Status Values** - TODO / IN-PROGRESS / BLOCKED / DONE / DEFERRED

------------------------------------------------------------------------

## Current Institutional State (implemented code, current zip)

-   Safety score: **64/100**
-   Primary blockers:
    -   RNG in production (`Uuid::new_v4`)
    -   Wall-clock enforcement (`Utc::now` in deadman)
    -   DB implicit timestamps (`default now()`)
    -   Control-plane security (no auth)
    -   Outbox-first not mechanically enforced (FK TODO in migration)

------------------------------------------------------------------------

# Phase 0 --- Baseline Controls (Meta)

## P0-1 --- Patch Discipline + CI Gate

**Goal:** Make it impossible to merge "unsafe" patterns silently.\
**Scope:** repo-wide meta checks.\
**Work:** - Add denylist checks (grep-based) for: - `Uuid::new_v4()` in
non-test - `Utc::now()` in enforcement modules - `default now()` in
migrations (if policy says explicit timestamps only) - Ensure CI runs: -
`cargo fmt --check` -
`cargo clippy --workspace --all-targets -- -D warnings` -
`cargo test --workspace` **Files:** (new) `.github/workflows/ci.yml` or
equivalent; (new) scripts/guards\
**Tests:** CI green.\
**Status:** TODO

------------------------------------------------------------------------

# Phase 1 --- Determinism Hardening (CRITICAL FOUNDATION)

## D1-1 --- Replace RNG Run IDs (daemon + cli)

**Goal:** Zero RNG run identity.\
**Work:** - Replace `Uuid::new_v4()` run id generation with
deterministic run_id derivation. - Derivation inputs:
`engine_id + policy_hash + asof_utc + universe_hash` (exact contract to
be defined). **Targets:** - `core-rs/crates/mqk-daemon/src/routes.rs` -
`core-rs/crates/mqk-cli/src/commands/run.rs` **Tests:** daemon + cli
tests green.\
**Status:** TODO

## D1-2 --- Replace RNG Audit Event IDs (audit)

**Goal:** Audit correlation without RNG dependency in core invariants.\
**Work:** - Decide: audit event ids may be random **only** if audit is
explicitly "ops metadata" (not used for determinism). - If not allowed:
derive event_id from `(prev_hash + payload_hash + seq)`
deterministically. **Targets:** - `core-rs/crates/mqk-audit/src/lib.rs`
**Tests:** audit tests green.\
**Status:** TODO

## D1-3 --- Remove Wall-Clock from Enforcement (deadman)

**Goal:** Deadman decisions do not depend on wall clock.\
**Work:** - Introduce injected `TimeSource` or explicit "tick counter /
monotonic timestamp" abstraction. - Replace `Utc::now()` usage in
enforcement logic. **Targets:** - `core-rs/crates/mqk-db/src/lib.rs`
(`deadman_expired` / enforcement call site) **Tests:** db scenario tests
green.\
**Status:** TODO

## D1-4 --- Remove DB `default now()` For Semantics-Bearing Columns

**Goal:** No hidden timestamps that affect logic/audit.\
**Work:** - Remove `default now()` where appropriate. - Require explicit
timestamps. - Add migrations to backfill / enforce NOT NULL if needed.
**Targets:** - `core-rs/crates/mqk-db/migrations/*.sql` **Tests:**
migrations apply; db tests pass.\
**Status:** TODO

------------------------------------------------------------------------

# Phase 2 --- Execution Boundary Hardening (PRIORITY)

## EB-1 --- Prove Non-bypassable Broker Submit Gate

**Goal:** All broker submissions go through `BrokerGateway` and always
enforce gates.\
**Work:** - Confirm `OrderRouter` remains `pub(crate)` and requires
private invoke token. - Add targeted tests to prove no bypass path.
**Targets:** - `core-rs/crates/mqk-execution/src/gateway.rs` -
`core-rs/crates/mqk-execution/src/order_router.rs` - tests:
`core-rs/crates/mqk-execution/tests/*` or
`core-rs/crates/mqk-testkit/tests/*` **Status:** TODO

## EB-2 --- Cancel/Replace Must Be Provenance-Checked

**Goal:** No cancel/replace of unknown/unowned orders.\
**Work:** - Require internal intent id; resolve to broker id via
mapping; refuse if missing. **Targets:** -
`mqk-execution/src/gateway.rs` - `mqk-db` mapping read API (minimal)
**Status:** TODO

## EB-3 --- Outbox-first Enforcement at Runtime Boundary

**Goal:** Submit requires claimed outbox record; no "free-form" submit.\
**Work:** - Tighten `OutboxClaimToken` contract and payload usage.
**Targets:** - `mqk-execution`, `mqk-db` (claim types) **Status:** TODO

## EB-4 --- Broker Map FK (Schema) to Enforce Outbox-first

**Goal:** Make it impossible to create mappings that did not originate
in outbox.\
**Work:** - Implement FK referenced by TODO in
`0010_idempotency_constraints.sql`. **Targets:** -
`core-rs/crates/mqk-db/migrations/0010_idempotency_constraints.sql`
**Status:** TODO

## EB-5 --- Execution Crash Windows (at least 2 scenarios)

**Goal:** No double submit across crash boundaries.\
**Work:** - Add scenarios: crash after sent before ack; crash after ack
before persist. **Targets:** - `core-rs/crates/mqk-testkit/tests/*`
**Status:** TODO

------------------------------------------------------------------------

# Phase 3 --- Reconcile Authority Wiring

## R3-1 --- Periodic Reconcile Tick Enforcement

**Goal:** Drift triggers `HaltAndDisarm` and is acted on.\
**Targets:** runtime wiring (daemon/runner)\
**Status:** TODO

## R3-2 --- Fill Ordering Policy + Tests

**Goal:** Deterministic fill application ordering.\
**Status:** TODO

------------------------------------------------------------------------

# Phase 4 --- Portfolio / Risk Math Integrity

## M4-1 --- Fixed-Point Money Type

**Goal:** Remove f64 from money/exposure where it can affect decisions.\
**Status:** TODO

## M4-2 --- Conservation Invariants

**Goal:** Cash + positions + lots conservation mechanically tested.\
**Status:** TODO

## M4-3 --- Corporate Actions Layer

**Goal:** Split/dividend adjustments (at minimum: split).\
**Status:** DEFERRED

------------------------------------------------------------------------

# Phase 5 --- Backtest + Promotion Integrity

## B5-1 --- Lookahead Bias Proof Harness

**Goal:** Detect future-bar leakage deterministically.\
**Status:** TODO

## B5-2 --- Backtest/Live Semantics Alignment

**Goal:** Promotion based on comparable execution model.\
**Status:** TODO

## B5-3 --- Mandatory Stress Battery Gate

**Goal:** Promotion requires stress scenarios pass.\
**Status:** PARTIAL (promotion has stress gate, wiring TBD)

------------------------------------------------------------------------

# Phase 6 --- Audit Anchoring + Forensics

## A6-1 --- Audit Chain Anchoring

**Goal:** Tampering detectable via external anchor/signature.\
**Status:** TODO

## A6-2 --- Log Durability Policy

**Goal:** Explicit flush/rotation policy.\
**Status:** TODO

------------------------------------------------------------------------

# Phase 7 --- Control Plane Security

## S7-1 --- Token Auth Middleware

**Goal:** No unauthenticated operator actions.\
**Status:** TODO

## S7-2 --- Loopback-only Default Bind

**Goal:** Prevent accidental network exposure.\
**Status:** TODO

## S7-3 --- Disable Snapshot Inject in Release Builds

**Goal:** Dev-only endpoint cannot be enabled accidentally in prod.\
**Status:** TODO

------------------------------------------------------------------------

# Phase 8 --- GUI (Operator Console) --- Deferred Until Safety Base

## G8-1 --- Trading Tab Structured Tables

**Status:** DEFERRED

## G8-2 --- Execution Control Panel (arm/halt/run start/stop)

**Status:** DEFERRED (requires Phase 7 auth)

------------------------------------------------------------------------

# Patch Execution Template (copy/paste)

\## Patch `<ID>`{=html} ---
```{=html}
<Title>
```
**Status:** TODO\
**Goal:**\
**Non-goals:**\
**Files:**\
**Implementation Steps:**\
**Tests to Run:**

``` bash
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

**Acceptance Criteria:**\
**Notes / Follow-ups:**

------------------------------------------------------------------------

End of Master Patch Plan.

------------------------------------------------------------------------

# Phase 9 --- System Invariant & Chaos Test Battery (Institutional Proof Layer)

## I9-1 --- Capital Conservation Invariant Suite

**Goal:** Prove cash + positions + realized/unrealized PnL are conserved
under all event sequences.\
**Work:** - Add invariant assertions after every OMS + portfolio
apply. - Scenario coverage: - partial fills - late fills after cancel -
replace reject then fill - duplicate broker events **Targets:** -
mqk-portfolio - mqk-execution (OMS integration) - mqk-testkit/tests/\*
**Status:** TODO

## I9-2 --- Duplicate / Out-of-Order Event Harness

**Goal:** System state is identical regardless of duplicate or
re-ordered broker events.\
**Work:** - Inject: - duplicate ACK - duplicate FILL - FILL before ACK -
cancel-ack after full fill - Assert final state identical to canonical
ordering. **Status:** TODO

## I9-3 --- Full Crash Matrix

**Goal:** No double-submit, no double-apply, no state divergence across
restarts.\
**Work:** Simulate crash at: - after outbox CLAIM - after broker SUBMIT
before SENT mark - after SENT before ACK - after ACK before
broker_map_upsert - after inbox receive before portfolio apply
**Status:** TODO

## I9-4 --- Deterministic Replay Proof

**Goal:** Given identical inputs + broker event log, replay yields
byte-identical portfolio + audit state.\
**Status:** TODO

------------------------------------------------------------------------

# Phase 10 --- Real Broker Adapter + Reconcile Closure

## B10-1 --- Real Broker Adapter Contract Definition

**Goal:** Explicitly document and codify broker guarantees: -
idempotency key behavior - event ordering guarantees - fill semantics -
partial fill behavior - cancel/replace guarantees **Status:** TODO

## B10-2 --- Live Adapter Integration Test Harness

**Goal:** Adapter integration tested in sandbox mode with: - duplicate
responses - delayed responses - partial network failures **Status:**
TODO

## B10-3 --- Reconcile Authority Closure

**Goal:** Prove reconcile_tick drives runtime behavior: - drift → halt -
halt persists disarm - restart defaults to disarmed if last drifted
**Status:** TODO

## B10-4 --- Reconcile Drift Tolerance Policy

**Goal:** Explicit thresholds for acceptable drift (rounding, latency).\
**Status:** TODO

------------------------------------------------------------------------

# Institutional Grade Definition (Internal Standard)

MiniQuantDesk V4 may be declared **Institutional Grade** only when:

1.  All Phases 0--10 are DONE.
2.  Deterministic replay proof passes.
3.  Full crash matrix suite passes.
4.  Reconcile drift is authoritative and enforced.
5.  Control plane requires authentication.
6.  No RNG or wall-clock time affects capital decisions.
7.  Outbox-first is enforced both in code and schema.
8.  Capital conservation invariants hold across all scenarios.

Until all eight are satisfied, system is classified as: - HARDENED
EXPERIMENTAL (Phases 0--4 complete) - PRE-INSTITUTIONAL (Phases 0--8
complete) - INSTITUTIONAL (Phases 0--10 complete + invariant proof suite
green)

------------------------------------------------------------------------
