# MiniQuantDesk V4 — Foundation Hardening Patch Tracker (FOUNDATION-GREEN)

**Scope:** Foundation only. No GUI, no strategy features, no research/backtest upgrades unless required for authoritative execution closure.

**Definition of Done (“FOUNDATIONALLY HARDENED”):**
- Single authoritative execution closure exists AND is used by daemon/CLI.
- Mechanically non-bypassable submit path (capability token from DB claim + gates + outbox-first + schema constraints).
- Determinism enforcement-grade (no RNG/wall clock/default now() in semantics-bearing paths; stable ordering; fixed rounding rules).
- Invariant authority (assert after every apply; violations halt/disarm and persist).
- Crash safety matrix proven (no double-submit, no double-apply, restart convergence).
- Deterministic replay proof (same inputs + broker event log → byte-identical portfolio + audit chain).

---

## Legend
- **Status:** `MISSING` / `PARTIAL` / `PRESENT-BUT-BYPASSABLE` / `DONE`
- **Severity:** `CRITICAL` / `HIGH` / `MEDIUM`
- **Acceptance:** must be objectively testable. Include exact test command(s).
- **Stop rule:** one patch at a time. Do not start the next patch until the acceptance criteria for the current patch is green.

---

## 0) Current Critical Blockers (must go to zero)

### RISK-001 — Forgeable OutboxClaimToken in production
- **Severity:** CRITICAL
- **Exploit:** Any crate can call `mqk_db::OutboxClaimToken::for_test()` in non-test builds → submit without real DB claim.
- **Fix Patch:** **FD-1**
- **Exit Criteria:** impossible to construct claim token outside `mqk-db` (except tests/testkit feature that is off in prod).

### RISK-002 — No authoritative execution closure used by daemon/CLI
- **Severity:** CRITICAL
- **Exploit:** Daemon/CLI can run without `mqk-runtime::ExecutionOrchestrator::tick`.
- **Fix Patch:** **FD-2**
- **Exit Criteria:** daemon/CLI tick loop calls runtime orchestrator; no other broker submit path exists.

### Schema gap — outbox-first not enforced via FK / constraints
- **Severity:** HIGH
- **Fix Patch:** **EB-4**
- **Exit Criteria:** DB rejects broker mappings without outbox provenance.

### Determinism blockers — wall clock + default now() + ordering + f64 decisions
- **Severity:** HIGH
- **Fix Patches:** **D1-4**, **D1-3**, **R3-2**, **M4-1**
- **Exit Criteria:** enforcement paths are inject-time only; schema has no default-now on semantics; stable ordering; fixed-point/rounding rules for gating.

---

## 1) Patch Queue (STRICT DEPENDENCY ORDER)

> Note: IDs match the master plan where possible; FD-* are foundation-specific where the plan is missing a mechanical closure.

---

### Patch P0-1 — Determinism & Safety Guardrails in CI (denylist)
- **ID:** P0-1
- **Status:** MISSING
- **Severity:** HIGH
- **Goal:** Add repo guard script + CI step that fails if forbidden patterns appear in non-test enforcement code.
- **Non-goals:** Any functional changes.
- **Files (expected):**
  - `.github/workflows/ci.yml`
  - `scripts/guards/denylist.sh` (or ps1)
  - `scripts/guards/denylist_rules.txt`
- **Acceptance Criteria:**
  - CI fails on new usage of: `Utc::now`, `SystemTime::now`, `timestamp_millis`, `Uuid::new_v4`, `rand::`, DB migrations containing `default now()` for semantics-bearing columns.
  - Exemptions allowed only under `#[cfg(test)]`, `tests/`, or a `testkit` feature.
- **Tests/Commands:**
  - `cargo fmt --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`
  - `./scripts/guards/denylist.sh`

---

### Patch FD-1 — Make OutboxClaimToken Unforgeable in Production (capability hardening)
- **ID:** FD-1
- **Status:** PRESENT-BUT-BYPASSABLE
- **Severity:** CRITICAL
- **Goal:** Claim tokens can only be created by DB claim code paths; no public constructors in prod.
- **Non-goals:** No changes to outbox semantics beyond token construction + visibility.
- **Files (expected):**
  - `core-rs/crates/mqk-db/src/lib.rs`
  - `core-rs/crates/mqk-db/src/outbox.rs` (if token lives there)
  - `core-rs/crates/mqk-testkit/*` (if test-only construction is needed)
- **Must change:**
  - `OutboxClaimToken::for_test` → gated behind `#[cfg(test)]` OR feature `"testkit"` that is **never enabled** in prod CI.
  - Ensure `OutboxClaimToken` cannot be instantiated via `pub struct` fields.
- **Acceptance Criteria:**
  - A non-test crate cannot compile code that constructs `OutboxClaimToken` without calling DB claim.
  - Add compile-fail test or a small “prover” crate under `tests/` that tries to call constructors and must fail.
- **Tests/Commands:**
  - `cargo test -p mqk-db`
  - `cargo test -p mqk-testkit`
  - Add: `cargo test --workspace`

---

### Patch EB-4 — Schema: Enforce Outbox-First Provenance via FK/Constraints
- **ID:** EB-4 (Master Plan)
- **Status:** MISSING
- **Severity:** HIGH
- **Goal:** DB prevents broker map rows without matching outbox idempotency key.
- **Non-goals:** No schema redesign.
- **Files (expected):**
  - `core-rs/crates/mqk-db/migrations/0010_idempotency_constraints.sql` (or follow-up migration `0011_*`)
- **Acceptance Criteria:**
  - Inserting broker map row referencing a non-existent outbox idempotency key fails.
  - Migration is idempotent and safe.
- **Tests/Commands:**
  - `cargo test -p mqk-db --test scenario_broker_map_upsert_is_idempotent`
  - Add a new DB scenario test: `scenario_broker_map_requires_outbox_fk.rs`

---

### Patch D1-4 — Remove DB `default now()` from semantics-bearing columns
- **ID:** D1-4 (Master Plan)
- **Status:** PARTIAL
- **Severity:** HIGH
- **Goal:** Every semantics-bearing timestamp must be explicitly supplied.
- **Non-goals:** No removal of timestamps.
- **Files (expected):**
  - `core-rs/crates/mqk-db/migrations/0001_init.sql`
  - `core-rs/crates/mqk-db/migrations/0006_arm_state.sql`
  - `core-rs/crates/mqk-db/migrations/0007_broker_order_map.sql`
  - `core-rs/crates/mqk-db/migrations/0008_reconcile_checkpoint.sql`
  - Any other migration using `default now()` for semantics-bearing columns
- **Acceptance Criteria:**
  - No `default now()` remains on: outbox/inbox/run status/lifecycle/audit chain/portfolio state.
  - All insert/update code paths pass explicit `now_utc` arguments.
- **Tests/Commands:**
  - `cargo test -p mqk-db`
  - `cargo test --workspace`

---

### Patch D1-3 — Remove wall-clock usage from enforcement decisions
- **ID:** D1-3 (Master Plan)
- **Status:** PARTIAL
- **Severity:** HIGH
- **Goal:** No `Utc::now()` (or equivalent) in enforcement/decisions. Use injected time source.
- **Non-goals:** You may keep timestamps as **data**, but never for logic decisions unless injected.
- **Files (expected):**
  - `core-rs/crates/mqk-audit/src/lib.rs`
  - `core-rs/crates/mqk-artifacts/src/lib.rs`
  - `core-rs/crates/mqk-daemon/src/state.rs`
  - `core-rs/crates/mqk-cli/src/commands/*`
- **Acceptance Criteria:**
  - Grep/guard: no `Utc::now` in non-test paths.
  - All decision points accept `now_utc` from caller.
- **Tests/Commands:**
  - `cargo test --workspace`
  - `./scripts/guards/denylist.sh`

---

### Patch FD-2 — Wire the SINGLE Authoritative Runtime Boundary into daemon/CLI
- **ID:** FD-2
- **Status:** MISSING
- **Severity:** CRITICAL
- **Goal:** `mqk-runtime::ExecutionOrchestrator::tick` is the only execution closure path used by daemon/CLI.
- **Non-goals:** No GUI; no HTTP “nice” routes except control-plane essentials.
- **Files (expected):**
  - `core-rs/crates/mqk-runtime/src/orchestrator.rs`
  - `core-rs/crates/mqk-daemon/src/main.rs`
  - `core-rs/crates/mqk-daemon/src/routes.rs` (remove in-memory placeholders; only control-plane)
  - `core-rs/crates/mqk-cli/src/commands/run.rs` (production run loop uses runtime)
- **Acceptance Criteria:**
  - `rg "ExecutionOrchestrator::tick"` shows daemon + CLI usage.
  - `rg "submit_order\("` shows no submit path outside runtime tick → gateway submit.
  - Daemon can run a deterministic tick loop in paper mode using DB outbox/inbox.
- **Tests/Commands:**
  - `cargo test -p mqk-daemon`
  - `cargo test -p mqk-runtime`
  - Add a `mqk-testkit` scenario: `scenario_daemon_tick_submits_via_outbox_claim.rs`

---

### Patch EB-1 — Prove Non-bypassable Submit Gate (lifecycle + reconcile + risk)
- **ID:** EB-1 (Master Plan)
- **Status:** PARTIAL
- **Severity:** CRITICAL
- **Goal:** Submits are mechanically impossible unless: (1) claimed outbox token, (2) lifecycle ARMED/RUNNING, (3) reconcile-clean, (4) risk gate pass.
- **Non-goals:** No strategy changes.
- **Files (expected):**
  - `core-rs/crates/mqk-execution/src/gateway.rs`
  - `core-rs/crates/mqk-execution/src/order_router.rs`
  - `core-rs/crates/mqk-db/src/*` (for run state queries)
- **Acceptance Criteria:**
  - Add tests proving each gate blocks submit:
    - `scenario_submit_refused_when_not_armed`
    - `scenario_submit_refused_when_reconcile_dirty`
    - `scenario_submit_refused_when_risk_rejects`
    - `scenario_submit_requires_real_claim_token` (compile-time or runtime)
- **Tests/Commands:**
  - `cargo test -p mqk-execution`
  - `cargo test -p mqk-testkit`

---

### Patch I9-1 — Invariant Authority after every apply + persistent halt/disarm
- **ID:** I9-1 (Master Plan)
- **Status:** MISSING
- **Severity:** CRITICAL
- **Goal:** After every OMS+portfolio apply in runtime tick, invariants are checked; violations persist HALT + DISARM and stop future submits.
- **Non-goals:** No fancy reporting.
- **Files (expected):**
  - `core-rs/crates/mqk-runtime/src/orchestrator.rs`
  - `core-rs/crates/mqk-db/src/*` (run status updates)
- **Acceptance Criteria:**
  - On invariant failure: DB run status becomes HALTED, system disarmed, and tick refuses further submit.
  - Add scenario test: `scenario_invariant_violation_halts_and_persists`.
- **Tests/Commands:**
  - `cargo test -p mqk-testkit --test scenario_invariant_violation_halts_and_persists`

---

### Patch I9-2 — Duplicate/out-of-order broker events proven through the real runtime boundary
- **ID:** I9-2 (Master Plan)
- **Status:** MISSING
- **Severity:** HIGH
- **Goal:** Duplicates/reordering do not change final state (OMS + portfolio).
- **Non-goals:** No new broker adapters.
- **Files (expected):**
  - `core-rs/crates/mqk-testkit/tests/*`
- **Acceptance Criteria:** Add scenarios (all must pass):
  - `scenario_duplicate_ack_no_effect`
  - `scenario_duplicate_fill_no_effect`
  - `scenario_fill_before_ack_stable`
  - `scenario_cancel_ack_after_fill_stable`
  - `scenario_replace_reject_then_fill_stable`
- **Tests/Commands:**
  - `cargo test -p mqk-testkit --test scenario_*`

---

### Patch I9-3 — Crash Matrix Bound to Runtime Orchestrator
- **ID:** I9-3 (Master Plan)
- **Status:** PARTIAL
- **Severity:** HIGH
- **Goal:** Prove: no double-submit, no double-apply, no divergence after restart across crash windows.
- **Non-goals:** No performance work.
- **Files (expected):**
  - `core-rs/crates/mqk-testkit/tests/scenario_execution_crash_windows_eb5.rs` (extend)
  - `core-rs/crates/mqk-runtime/src/orchestrator.rs` (if recovery hooks needed)
- **Acceptance Criteria:** Existing crash-window tests cover:
  - Crash after claim before submit
  - Crash after submit before mark_sent
  - Crash after mark_sent before ack ingest
  - Crash after inbox insert before apply
  - Crash after apply before mark_applied
  - Restart yields identical outcomes; no double-submit at broker mock.
- **Tests/Commands:**
  - `cargo test -p mqk-testkit --test scenario_execution_crash_windows_eb5`

---

### Patch I9-4 — Deterministic Replay Proof (byte-identical state + audit chain)
- **ID:** I9-4 (Master Plan)
- **Status:** MISSING
- **Severity:** HIGH
- **Goal:** Same inputs + broker event log → byte-identical portfolio state and audit chain.
- **Non-goals:** No new serialization formats.
- **Files (expected):**
  - `core-rs/crates/mqk-testkit/tests/scenario_replay_determinism_matches_artifacts.rs` (harden)
  - `core-rs/crates/mqk-audit/src/*`
  - `core-rs/crates/mqk-artifacts/src/*`
- **Acceptance Criteria:**
  - Two independent runs over same inbox log produce identical:
    - persisted portfolio snapshot bytes
    - audit chain hash
    - artifacts manifest hash
- **Tests/Commands:**
  - `cargo test -p mqk-testkit --test scenario_replay_determinism_matches_artifacts`

---

### Patch R3-2 — Stable ordering for fills / deterministic apply order
- **ID:** R3-2 (Master Plan)
- **Status:** PARTIAL
- **Severity:** MEDIUM → HIGH if it gates decisions
- **Goal:** Enforce stable ordering for any list iteration that affects state transitions / sums.
- **Non-goals:** No broad refactors.
- **Files (expected):**
  - `core-rs/crates/mqk-runtime/src/orchestrator.rs`
  - `core-rs/crates/mqk-portfolio/src/*` (if HashMap affects sums)
- **Acceptance Criteria:**
  - All event application uses a deterministic sort key (e.g., broker_msg_id, exchange_ts, sequence).
  - No HashMap iteration affects gating sums without explicit ordering/rounding.
- **Tests/Commands:**
  - Add a determinism test: apply same set of fills in permuted order → identical final state.

---

### Patch M4-1 — Fixed-point / explicit rounding for money/exposure/risk gating
- **ID:** M4-1 (Master Plan)
- **Status:** MISSING/PARTIAL
- **Severity:** HIGH if any f64 gates execution
- **Goal:** Remove raw f64 from money/exposure/risk gates OR enforce explicit rounding rules.
- **Non-goals:** Don’t rewrite the entire portfolio engine; target gates first.
- **Files (expected):**
  - `core-rs/crates/mqk-portfolio/src/allocator.rs`
  - `core-rs/crates/mqk-portfolio/src/constraints.rs`
  - `core-rs/crates/mqk-execution/src/*` (risk gates)
- **Acceptance Criteria:**
  - All “permit/deny execution” thresholds use fixed-point or deterministic rounding rules.
  - Unit tests cover rounding boundaries.
- **Tests/Commands:**
  - `cargo test -p mqk-portfolio`
  - `cargo test -p mqk-execution`

---

## 2) “Green” Audit Run Checklist (run this after each patch)
- `cargo fmt`
- `cargo clippy --workspace --all-targets -- -D warnings`
- `cargo test --workspace`
- `cargo test -p mqk-db`
- `cargo test -p mqk-testkit`
- (If daemon wired) `cargo test -p mqk-daemon`
- `./scripts/guards/denylist.sh`

---

## 3) Notes / Guardrails for Patch Execution
- Every patch must include at least one new test proving the new mechanical guarantee.
- Any “gate” must be bound to DB state (run/lifecycle/reconcile) to be authoritative.
- If a change widens public visibility or adds a new submit path: **reject**.

---

## 4) Optional / Nice-to-Haves (only after foundation is green)
- S7-* control-plane auth & loopback-only HTTP.
- Better telemetry / metrics.
- More broker adapters.
- GUI.
