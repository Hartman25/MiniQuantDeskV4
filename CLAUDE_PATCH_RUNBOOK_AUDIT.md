# CLAUDE_PATCH_RUNBOOK_AUDIT.md
MiniQuantDesk V4 — Audit Hardening Runbook (Repo-Evidence Derived)

**Purpose:** Execute a safety hardening program for MiniQuantDesk V4 derived from *code and structural evidence in the repo*, not from tracker claims or docs.  
This is capital-protection work. Assume real money deployment. Fail closed.

**Primary integrations (current):**
- **Market data:** TwelveData (primary)
- **Broker:** Alpaca (paper + live)
- **Alerts/C2/ops:** Discord webhooks (multiple channels)

**Design constraint:** Leave explicit extension points for additional market data providers later (e.g., FMP), without requiring architectural rewrites.

---

## 0. Non‑Negotiables

### Claude global hard rules (every patch)
1) **ONE PATCH ONLY** per response. Do not start the next patch.  
2) **NO DIFFS.** Output the **FULL UPDATED CONTENT** of every file you change (entire file).  
3) Keep changes **minimal**, **additive**, and **strictly scoped** to the current patch goal.  
4) **No unrelated refactors.** No renames. No reformatting unrelated files.  
5) **Do not rewrite migration history.** Never edit migrations that are already applied/“DONE.”  
6) **Never print secrets.** Never paste `.env` values. Only refer to env var **NAMES**.  
7) If ambiguous: **fail closed**, add **TODOs** with exact file paths + rationale.  
8) End every response with the exact stop line:

**`STOP — PATCH <ID> COMPLETE. Awaiting test results.`**

---

## 1. Gate Checklist (run after every patch)

From `core-rs/`:

```powershell
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

If patch touches CLI or daemon:

```powershell
cargo run -p mqk-cli -- --help
cargo run -p mqk-daemon -- --help
```

If patch touches DB schema/state machine behavior and you have `MQK_DATABASE_URL` configured:

```powershell
cargo test -p mqk-db
```

**No green = no next patch.**  
If gates fail, Claude fixes only what’s required within the current patch scope.

---

## 2. Stop Protocol (operator control)

- Claude implements one patch and stops.
- You run gates locally.
- You reply with either:
  - “PATCH <ID> gates are green. Proceed to PATCH <NEXT>.”  
  - or paste failing output and demand “Fix ONLY within PATCH <ID> scope.”

---

# PHASE 0 — SECRETS, ENV, AND WEBHOOK ROUTING (FOUNDATIONAL)

## PATCH S1 — Secrets & Webhook Routing (dotenv + env-name config + fail-closed enforcement)

### Problem
V1–V3 used `.env.local` (dotenv) for broker/data/discord credentials. In V4, secrets loading and usage are not clearly centralized or mechanically enforced across entrypoints and modes.

### Goal
Implement a clean, Rust-native secret handling layer that:
- Loads `.env.local` in dev (dotenv behavior) but does not require it in prod
- Stores only **env var names** in YAML/config (never secret values)
- Resolves secret values via `std::env::var` at runtime into a `ResolvedSecrets` struct
- Enforces mode-aware requirements:
  - **LIVE** fails closed if required broker + webhook + provider keys are missing
  - **PAPER** fails closed if paper broker keys required are missing
  - **BACKTEST** does not require broker keys (webhook optional)
- Supports **multiple Discord webhooks** to separate channels:
  - paper, live, backtest, alerts, heartbeat, C2, and future channels
- Leaves extension points for **future data providers** (e.g., FMP) without rewrites

### Evidence targets (where to wire)
- Entry points for dotenv bootstrap: `mqk-cli`, `mqk-daemon` (and any other binaries)
- Config schema / loader: `mqk-config`
- Provider construction: `mqk-md` (TwelveData is primary now; reserve interface for future providers)
- Broker construction: execution/broker adapter wiring (Alpaca paper/live)
- Discord notifier/router module (centralized; no scattered `env::var` calls)

### Required outcomes (mechanical)
- `.env.local` is **gitignored** and an `.env.local.example` exists (no real keys)
- Config contains **env var NAMES** (e.g., `ALPACA_PAPER_KEY`) not the values
- Runtime builds `ResolvedSecrets` once and passes it to constructors
- No logs print secret values
- LIVE start/arm fails closed if secrets missing

### Success criteria
- A minimal integration test or scenario proves:
  - missing env vars in LIVE mode → hard failure
  - configured env var names appear in canonical config JSON (names only)
  - secret values never appear in config hash / artifacts

**Fix category:** Infra / Config / Runtime / Alerts / Data / Broker

---

# PHASE 1 — CRITICAL EXECUTION SAFETY (CAPITAL BLOCKERS)

## PATCH A1 — Eliminate broker bypass (hard choke-point)

### Problem
Broker submission interfaces are publicly accessible and can bypass gating and outbox discipline.

### Evidence
- `core-rs/crates/mqk-execution/src/lib.rs` (public re-exports)
- `core-rs/crates/mqk-execution/src/order_router.rs` (`pub trait BrokerAdapter`)

### Required outcome
- Exactly one mechanical choke-point can reach broker submit/cancel/replace.
- External crates cannot call submit/cancel/replace directly.
- Compile-time restriction preferred (visibility/sealing), runtime checks secondary.

### Success criteria
- Attempting direct submit outside the choke-point fails at compile time.

**Fix category:** Execution

---

## PATCH A2 — Make gates non-forgeable (remove caller-supplied GateVerdicts)

### Problem
Safety gates are caller-provided booleans, trivially forgeable.

### Evidence
- `core-rs/crates/mqk-execution/src/gateway.rs` (`GateVerdicts` fields are public)

### Required outcome
- Gate checks are derived from real engine state (integrity/risk/reconcile) or persisted run state.
- No user-supplied “verdict struct” can claim gates are clean.

### Success criteria
- Gateway/submit cannot be called with a “fake” gate verdict.

**Fix category:** Execution / Integrity / Risk / Reconcile

---

## PATCH A3 — Outbox-first must be enforced at the execution boundary

### Problem
Execution submit can be performed without a persisted outbox record.

### Evidence
- DB outbox helpers exist, but execution entrypoint accepts a raw request.

### Required outcome
- Broker submit requires a persisted outbox row (or equivalent durable intent record).
- Dispatch uses DB claim/lock semantics; retries are idempotent.

### Success criteria
- There is no code path to broker submit that does not originate from outbox.

**Fix category:** Execution / Infra

---

## PATCH A4 — Persist intent↔broker_order_id mapping (crash-safe cancel/replace)

### Problem
Order identity mapping is in-memory; crash loses control of live orders.

### Evidence
- `core-rs/crates/mqk-execution/src/id_map.rs` (`HashMap` only)

### Required outcome
- Persist mapping (intent_id/client_order_id ↔ broker_order_id) in DB.
- Restart can resume cancel/replace safely.

### Success criteria
- After simulated restart, cancel/replace can locate the right broker order id.

**Fix category:** Execution / DB

---

## PATCH A5 — Strict numeric normalization (reject NaN/Inf in release)

### Problem
Release builds can accept NaN/Inf and cast into garbage micros.

### Evidence
- `core-rs/crates/mqk-execution/src/prices.rs` uses `debug_assert!(is_finite)`

### Required outcome
- Runtime validation: reject non-finite, reject out-of-range, reject negative where invalid.
- Errors are propagated and logged safely (no sensitive values).

### Success criteria
- Unit test: NaN/Inf inputs fail deterministically in release mode.

**Fix category:** Execution

---

# PHASE 2 — RECONCILE & ARMING SAFETY

## PATCH B1 — Arming must not trust a forgeable audit string for reconcile cleanliness

### Problem
Arming preflight trusts an `audit_events` string `"CLEAN"` as proof reconcile ran.

### Evidence
- `core-rs/crates/mqk-db/src/lib.rs::arm_preflight()`

### Required outcome
- Arming runs reconcile (or verifies a signed/hash-locked reconcile artifact tied to current snapshot watermark).
- Never accept “clean” based on a mutable DB string alone.

### Success criteria
- A forged audit event cannot satisfy arming without actual reconcile evidence.

**Fix category:** Reconcile / DB / Runtime

---

## PATCH B2 — Enforce snapshot monotonicity inside reconcile (not optional)

### Problem
Watermark helper exists but reconcile can be invoked without enforcing monotonic snapshots.

### Evidence
- `core-rs/crates/mqk-reconcile/src/watermark.rs` exists
- `core-rs/crates/mqk-reconcile/src/engine.rs` does not force it

### Required outcome
- Reconcile rejects stale/non-monotonic snapshots by default.

### Success criteria
- Test: older snapshot after newer one is rejected.

**Fix category:** Reconcile

---

## PATCH B3 — Reconcile must be periodic + enforced in runtime

### Problem
Nothing proves reconcile runs periodically or blocks execution when stale.

### Required outcome
- Runtime orchestration schedules reconcile and blocks submits when reconcile is stale or dirty.
- Reconcile freshness bound is explicit (configurable) and fail-closed.

### Success criteria
- A stale reconcile watermark prevents broker dispatch.

**Fix category:** Runtime / Reconcile

---

# PHASE 3 — RESTART / FAIL‑CLOSED SAFETY

## PATCH C1 — Sticky DISARM persisted + loaded at daemon boot

### Problem
Sticky DISARM semantics exist in parts but are not proven wired into daemon boot.

### Evidence
- `mqk-db` has load/persist helpers
- `mqk-integrity` has fail-closed boot logic
- daemon appears to hold integrity state in memory

### Required outcome
- Daemon boot loads persisted arm/disarm state.
- Persisted “armed” state never auto-enables execution after crash.

### Success criteria
- Restart while armed results in disarmed state until explicit operator action.

**Fix category:** Integrity / Runtime / DB

---

## PATCH C2 — Deadman must be sticky across restart (fail closed)

### Problem
Time-based deadman logic can be accidentally cleared by restart unless persisted.

### Required outcome
- Deadman violation state persists.
- Restart does not reset to OK without explicit recovery action.

### Success criteria
- A deadman-triggered stop remains in effect after restart.

**Fix category:** Runtime / Integrity / DB

---

# PHASE 4 — DB STATE MACHINE & TRANSACTIONAL INVARIANTS

## PATCH D1 — Add DB CHECK constraints for enumerated text states

### Problem
Critical state columns are plain `text` with comments; typos corrupt logic.

### Evidence
- Outbox status is `text` with comment-based states in migrations

### Required outcome
- CHECK constraints for allowed values on:
  - outbox status
  - run lifecycle status
  - any other enum-like text columns

### Success criteria
- Invalid status writes fail at DB level.

**Fix category:** DB / Infra

---

## PATCH D2 — Make inbox “insert → apply” transactionally real

### Problem
Tests simulate atomicity with counters; portfolio/exposure apply is not guaranteed atomic.

### Required outcome
- Broker event insert and state apply are:
  - done in one DB transaction, or
  - made idempotent with strong dedupe + deterministic replay on recovery

### Success criteria
- Duplicate fills cannot double-apply PnL/exposure even under crash/restart.

**Fix category:** DB / Execution / Portfolio

---

## PATCH D3 — Enforce idempotency keys at schema level for submit/replace/cancel intents

### Problem
Outbox has a unique idempotency key, but full identity invariants are not guaranteed for all intent types.

### Required outcome
- Add schema constraints / uniqueness for:
  - intent_id
  - client_order_id
  - broker_order_id
  - broker_message_id (inbox)
  - and stable mapping between them where required

### Success criteria
- Retry cannot generate duplicate effective orders without failing dedupe.

**Fix category:** DB / Execution

---

# PHASE 5 — INTEGRITY ENGINE CORRECTIONS

## PATCH E1 — Holiday-aware gap detection (not just weekends)

### Problem
Daily gap detection ignores market holidays, causing false alarms or missed gaps.

### Evidence
- TODOs in market data / md module regarding 1D gap detection

### Required outcome
- Introduce market calendar abstraction:
  - minimal US equities calendar initially
  - explicit extension point for other exchanges

### Success criteria
- 1D gap logic behaves correctly on common US market holidays.

**Fix category:** Integrity / Market calendar

---

## PATCH E2 — Integrity DISARM must block execution end-to-end

### Problem
Integrity can disarm, but execution can still submit if bypasses/gates exist.

### Required outcome
- DISARM is enforced at the broker choke-point.
- No submit when disarmed, regardless of caller.

### Success criteria
- Simulated stale feed → disarm → submit attempt fails.

**Fix category:** Integrity / Execution

---

# PHASE 6 — BACKTEST & PROMOTION ANTI‑LIE

## PATCH F1 — Disallow negative slippage / favorable fills

### Problem
Backtest config can make fills systematically favorable.

### Evidence
- Stress profile allows negative slippage bps

### Required outcome
- Validate config: negative slippage rejected.
- If stress knobs exist, they must be conservative-only.

### Success criteria
- Attempting negative slippage fails fast.

**Fix category:** Backtest / Promotion

---

## PATCH F2 — Make defaults conservative (integrity on, corporate actions safe)

### Problem
Defaults disable integrity and allow corporate actions.

### Required outcome
- Conservative defaults for any “run in anger” mode.
- Keep `test_defaults()` clearly labeled and not used for real evaluation.

### Success criteria
- CLI backtest without explicit config uses conservative defaults.

**Fix category:** Backtest

---

## PATCH F3 — Promotion evaluator must fail‑closed on NaN metrics

### Problem
NaN comparisons collapse to “Equal,” corrupting ranking.

### Required outcome
- Any NaN in key metrics fails evaluation or fails promotion outright.

### Success criteria
- Test: NaN metric cannot be promoted.

**Fix category:** Promotion

---

# PHASE 7 — TEST DISCIPLINE (STOP LYING TO YOURSELF)

## PATCH G1 — Eliminate silent skips of DB scenario tests

### Problem
Safety tests skip if `MQK_DATABASE_URL` is absent; green is meaningless.

### Required outcome
- CI/test mode either:
  - requires DB and fails if not configured, or
  - uses a testcontainer/spin-up Postgres strategy
- No silent “SKIP” for safety scenarios

### Success criteria
- Running `cargo test -p mqk-db` without DB config fails explicitly with instructions.

**Fix category:** Infra / Tests

---

## PATCH G2 — Replace placeholder testkit parity with real parity assertions

### Problem
Testkit placeholders claim parity but aren’t real.

### Required outcome
- Deterministic replay/backtest parity harness that compares artifacts/hashes/events.

### Success criteria
- Real parity test fails on divergence and reports mismatched artifacts.

**Fix category:** Tests / Backtest parity

---

# PHASE 8 — RUNTIME & OPERATOR GUARDRAILS

## PATCH H1 — Prove daemon cannot submit without clean gates

### Problem
Even if routes call gateway, the gateway must be mechanically enforced.

### Required outcome
- REST routes cannot trigger broker dispatch unless:
  - integrity armed (real)
  - risk allowed (real)
  - reconcile clean (real)
  - outbox-first satisfied
- No bypass via alternate endpoints.

### Success criteria
- Negative scenario tests for daemon endpoints fail closed.

**Fix category:** Runtime / Execution

---

## PATCH H2 — Operator confirmation + state proof bundle for dangerous actions

### Problem
Arm/start/stop may be callable without proof of state.

### Required outcome
- CLI/daemon require explicit confirmation for LIVE
- Require proof bundle:
  - config_hash
  - reconcile watermark
  - integrity state
  - risk state
  - secrets presence (names only)

### Success criteria
- LIVE start cannot occur without explicit confirmation and proof checks.

**Fix category:** CLI / Ops safety

---

# 3. Patch Count and Recommended Order

Total audit-derived patches: **23**

Recommended order:
1) S1
2) A1
3) A2
4) A3
5) A5
6) A4
7) B1
8) B2
9) B3
10) C1
11) C2
12) D1
13) D3
14) D2
15) E2
16) E1
17) F1
18) F3
19) F2
20) G1
21) G2
22) H1
23) H2

---

# 4. TwelveData-first with provider extension points

**Requirement:** TwelveData is primary now, Alpaca is broker. Future providers (FMP, etc.) should be add-on adapters.

Minimum structure expectation:
- `MarketDataProvider` trait/interface in `mqk-md`
- `TwelveDataProvider` implements it
- Future: `FmpProvider` implements it
- Provider selection configured via config (provider name + env var names), resolved via **PATCH S1**
- Provider keys must never enter config hashing or artifacts (names only).

---

# 5. Deployment Readiness Standard (blunt)

Not allocator-grade until:
- No broker bypass exists
- Gates cannot be forged
- Reconcile cannot be spoofed
- DISARM/deadman are sticky across restart
- Outbox/inbox are transactional and deduped
- Backtest can’t be configured to lie
- DB tests never silently skip
- Daemon/CLI require proof bundle for LIVE
