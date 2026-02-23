# MiniQuantDeskV4 — Patch Tracker (Updated)

## Exact insert text requested (use this if you already have a tracker)

### Add to docs list
- docs/specs/strategy_evaluation_and_ranking.md
- docs/specs/config_layering_and_hashing.md
- docs/specs/testing_strategy.md
- docs/specs/testing_fixtures_and_harness.md

### Add to PATCH 01 deliverables
- Config loader (merge layers) + config_hash + validation (minimal v0)

---

Rules:
- One patch at a time.
- Each patch has explicit acceptance tests.
- No scope creep.
- No external I/O integrations (Discord/HTTP/UI) until after PATCH 12. Only interface stubs are allowed.
- Any EXPERIMENTAL engine live arming requires separate broker keys/account and remains capped by config.

Docs that define invariants:
- docs/specs/backtest_policy.md
- docs/specs/execution_model.md
- docs/specs/portfolio_and_accounting_model.md
- docs/specs/data_pipeline_and_integrity.md
- docs/specs/reconciliation.md
- docs/specs/arming_model.md
- docs/specs/event_taxonomy.md
- docs/specs/strategy_framework_and_plugin_model.md
- docs/specs/run_artifacts_and_reproducibility.md
- docs/specs/kill_switches_and_limits.md
- docs/specs/secrets_and_config_management.md
- docs/specs/broker_adapter_contract.md
- docs/specs/strategy_evaluation_and_ranking.md
- docs/specs/config_layering_and_hashing.md
- docs/specs/testing_strategy.md
- docs/specs/testing_fixtures_and_harness.md

---

## PATCH 01–04 — Spine (DB, Config, Events, Outbox/Inbox)
Status: DONE

Implemented:
- Postgres schema: runs, audit_events, oms_outbox, oms_inbox (`crates/mqk-db/migrations/0001_init.sql`)
- Config loader with layered YAML merge, SHA-256 canonical hashing (`crates/mqk-config/src/lib.rs`)
- Event envelope schema (`crates/mqk-schemas/src/lib.rs`)
- Outbox idempotency via `uq_outbox_idempotency` unique index
- Inbox dedupe via `uq_inbox_broker_message_id` unique index

---

## PATCH 05 — Execution: Target-to-Intent Conversion
Status: DONE

Implemented:
- Deterministic targets_to_order_intents (`crates/mqk-execution/src/engine.rs`)
- Signed quantities, BTreeMap deterministic ordering
- Test: `scenario_target_to_intent.rs`

---

## PATCH 06 — Portfolio: FIFO Lots, PnL, Equity
Status: DONE

Implemented:
- FIFO lot accounting with buy/sell (`crates/mqk-portfolio/src/accounting.rs`)
- Equity = cash + unrealized (`crates/mqk-portfolio/src/metrics.rs`)
- Exposure computation and enforcement
- Tests: `scenario_pnl_partial_fills_fifo.rs`, `scenario_position_flatten_fifo.rs`, `scenario_exposure_enforcement_multi_symbol.rs`

---

## PATCH 07 — Risk Engine (Deterministic)
Status: DONE

Implemented:
- Kill switches, daily loss, max drawdown, reject storm, PDT (`crates/mqk-risk/src/engine.rs`)
- Sticky halt + disarm flags
- Tests: `scenario_forced_halt_on_threshold_breach.rs`, `scenario_new_order_rejection_after_limit.rs`, `scenario_pdt_auto_mode_enforcement.rs`, `scenario_auto_flatten_on_critical_event.rs`

---

## PATCH 08 — Data Integrity Engine
Status: DONE

Implemented:
- Stale feed, gap detection, feed disagreement, incomplete bar rejection (`crates/mqk-integrity/src/engine.rs`)
- Tests: `scenario_stale_feed_disarm.rs`, `scenario_gap_fail.rs`, `scenario_feed_disagreement_halt.rs`, `scenario_incomplete_bar_rejection.rs`

---

## PATCH 09 — Reconciliation Engine
Status: DONE

Implemented:
- Position mismatch, order drift, unknown broker order detection (`crates/mqk-reconcile/src/engine.rs`)
- LIVE arming gate: `is_clean_reconcile()` (`crates/mqk-reconcile/src/engine.rs:138-141`)
- Tests: `scenario_drift_detection.rs`, `scenario_reconcile_required_before_live.rs`, `scenario_unknown_order_detection.rs`

---

## PATCH 10 — Audit Trail + Artifacts
Status: DONE

Implemented:
- Append-only JSONL with SHA-256 hash chain (`crates/mqk-audit/src/lib.rs`)
- Run manifest + artifact initialization (`crates/mqk-artifacts/src/lib.rs`)
- Tests: `scenario_run_artifacts_manifest_created.rs`, `scenario_entry_places_protective_stop.rs`

---

## PATCH 11 — Backtest + Stress Engine
Status: DONE

Implemented:
- Deterministic replay engine (`crates/mqk-backtest/src/engine.rs`)
- Conservative worst-case fill pricing (buy@high, sell@low + slippage)
- Stress profile overlay
- Tests: `scenario_replay_determinism.rs`, `scenario_stress_impact_measurable.rs`, `scenario_ambiguity_worst_case_enforced.rs`

---

## PATCH 12 — Promotion Gate Evaluator
Status: DONE

Implemented:
- Sharpe, MDD, CAGR, profit factor, profitable months % (`crates/mqk-promotion/src/evaluator.rs`)
- Tie-break deterministic ordering
- Tests: `scenario_pass_above_threshold.rs`, `scenario_fail_below_threshold.rs`, `scenario_tie_break_correctness.rs`

---

## PATCH 13 — Engine Isolation (mqk-isolation)
Status: DONE

Implemented:
- Engine-scoped broker key loading with token enforcement (`crates/mqk-isolation/src/lib.rs:70-82`)
- Allocation cap enforcement (`enforce_allocation_cap_micros`, `crates/mqk-isolation/src/lib.rs:137-155`)
- EngineStore keyed state to prevent cross-engine bleed (`crates/mqk-isolation/src/lib.rs:167-189`)
- Integrated into backtest engine (`crates/mqk-backtest/src/engine.rs:194-228`)
- Test: `scenario_allocation_cap_enforced.rs`

---

## PATCH 14 — Run Lifecycle + LIVE Exclusivity
Status: DONE

Implemented:
- DB migration: status column, lifecycle timestamps, status CHECK constraint, unique partial index `uq_live_engine_active_run` on `(engine_id) WHERE mode='LIVE' AND status IN ('ARMED','RUNNING')` (`crates/mqk-db/migrations/0002_run_lifecycle.sql`)
- Lifecycle transitions: arm_run, begin_run, stop_run, halt_run, heartbeat_run (`crates/mqk-db/src/lib.rs:272-410`)
- Run binding assertion (`crates/mqk-db/src/lib.rs:238-269`)
- CLI: `mqk run start`, `mqk db migrate`, `mqk db status`, `mqk audit emit` (`crates/mqk-cli/src/main.rs`)
- Test: `scenario_run_lifecycle_enforced.rs` (lifecycle transitions + LIVE exclusivity via unique index)

---

## PATCH 15 — Integration Hardening + Remaining Scenario Tests
Status: DONE

Implemented:
- 15a: Config secrets detection (`crates/mqk-config/src/lib.rs:8-22,343-366`) — `enforce_no_secret_literals()` walks all config leaf values, aborts on `SECRET_PREFIXES` match with `CONFIG_SECRET_DETECTED` error; env var names accepted
- 15b: Config hash stability (`crates/mqk-config/src/lib.rs:327-341`) — `canonicalize_json` relies on `serde_json::Map` BTreeMap backing (keys auto-sorted alphabetically regardless of YAML input order) + `sha256_hex` for 64-char SHA-256
- 15c: Audit hash chain verifier (`crates/mqk-audit/src/lib.rs:141-210`) — `verify_hash_chain()` / `verify_hash_chain_str()` recomputes each `hash_self` and verifies `hash_prev` linkage; detects tampered payloads at exact line number
- 15d: Run artifact init integration (`crates/mqk-artifacts/src/lib.rs`) — `init_run_artifacts()` creates manifest.json + placeholder files; config_hash flows config loader → manifest → verified audit chain
- 15e: Backtest → promotion pipeline (`crates/mqk-promotion/src/evaluator.rs`) — `BacktestReport` fed into `evaluate_promotion()` produces correct pass/fail with reason codes
- 15f: Cross-engine isolation (`crates/mqk-isolation/src/lib.rs`) — `EngineStore<T>` prevents in-memory state bleed; `EngineIsolation::from_config_json()` enforces broker key env var token naming (engine_id must appear in key name)
- 15g: Reconcile gate logic proven in `crates/mqk-reconcile/tests/scenario_reconcile_gate_blocks_live_arm.rs` (9 tests covering clean/dirty reconcile → arm decision); arm_preflight + reconcile + lifecycle integration in `crates/mqk-db/tests/scenario_arm_preflight_requires_reconcile.rs` (DB-backed, requires `MQK_DATABASE_URL`)
- Tests: `scenario_secrets_excluded.rs` (7), `scenario_config_hash_stable.rs` (6), `scenario_hash_chain_tamper_detected.rs` (5), `scenario_cli_run_start_creates_artifacts.rs` (2), `scenario_backtest_to_promotion_pipeline.rs` (3), `scenario_cross_engine_state_bleed_prevented.rs` (8), `scenario_reconcile_gate_blocks_live_arm.rs` (9)

Notes:
- `crates/mqk-config/src/consumption.rs` is a dead file (not imported via `mod consumption` in lib.rs; superseded by lib.rs implementation). TODO: remove in a cleanup pass if desired.
- 15g arm_run() integration deferred to PATCH 20 (arm_run requires live Postgres; arm_preflight covers it)

### Architectural Decision
Close remaining test gaps identified during PATCH 08-14 implementation. Expand scenario coverage to prove integration invariants hold end-to-end.

### Why This Matters
Individual crate engines are tested in isolation, but cross-crate integration boundaries have untested seams. A real allocator needs confidence that the composition of these engines holds under realistic conditions.

### Remaining Items (Explicit Scenario Tests — All Delivered)

#### 15a. Config secrets exclusion test
- **Crate:** `mqk-config`
- **File:** `crates/mqk-config/tests/scenario_secrets_excluded.rs`
- **Intent:** Prove that secret-like values in YAML config are detected and rejected. Prove that env var NAMES (not values) are what appears in config_json.
- **Validates:** docs/specs/config_layering_and_hashing.md section 5 (secrets exclusion)
- **GREEN when:** Loading a YAML with `api_key: "sk-live-abc123"` as a literal value fails; loading with `api_key_env: "ALPACA_API_KEY_MAIN"` succeeds and config_json contains the env var name, not the secret value.

#### 15b. Config hash stability test
- **Crate:** `mqk-config`
- **File:** `crates/mqk-config/tests/scenario_config_hash_stable.rs`
- **Intent:** Prove that the same set of YAML files always produces the same config_hash regardless of filesystem ordering or platform.
- **Validates:** docs/specs/config_layering_and_hashing.md section 4 (hashing determinism)
- **GREEN when:** `load_layered_yaml(&[base, env, engine])` called twice returns identical `config_hash`; reordering keys within YAML files doesn't change the hash.

#### 15c. Audit hash chain integrity test
- **Crate:** `mqk-audit`
- **File:** `crates/mqk-audit/tests/scenario_hash_chain_tamper_detected.rs`
- **Intent:** Prove that tampering with one line in audit.jsonl breaks hash chain verification.
- **Validates:** docs/specs/run_artifacts_and_reproducibility.md (audit hash chain)
- **GREEN when:** Writing 5 events with hash_chain=true, then mutating line 3 payload, then a verify pass detects the break at line 4.

#### 15d. CLI run start + artifact creation integration test
- **Crate:** `mqk-cli` (or `mqk-testkit`)
- **File:** `crates/mqk-testkit/tests/scenario_cli_run_start_creates_artifacts.rs`
- **Intent:** Prove that `mqk run start` creates DB row + artifact directory + manifest.json with matching hashes.
- **Validates:** integration of mqk-db + mqk-artifacts + mqk-config
- **GREEN when:** After run start, DB row exists with matching config_hash, and exports/<run_id>/manifest.json exists with matching config_hash and git_hash.

#### 15e. Backtest -> promotion pipeline integration test
- **Crate:** `mqk-testkit` or `mqk-promotion`
- **File:** `crates/mqk-promotion/tests/scenario_backtest_to_promotion_pipeline.rs`
- **Intent:** Prove that a BacktestReport can be fed into the promotion evaluator and produce correct pass/fail.
- **Validates:** End-to-end: backtest engine -> report -> promotion evaluator
- **GREEN when:** A profitable backtest report passes promotion; an unprofitable one fails with correct reason codes.

#### 15f. Cross-engine isolation integration test
- **Crate:** `mqk-isolation`
- **File:** `crates/mqk-isolation/tests/scenario_cross_engine_state_bleed_prevented.rs`
- **Intent:** Prove that EngineStore prevents reading/writing state across engine boundaries, and that broker key env var naming enforcement rejects shared keys.
- **Validates:** PATCH 13 isolation guarantees end-to-end
- **GREEN when:** `EngineStore<RiskState>` with MAIN and EXP entries returns None for wrong engine; `EngineIsolation::from_config_json` rejects config where broker key env var does not contain engine_id token.

#### 15g. Reconcile + lifecycle gate integration test
- **Crate:** `mqk-db` or `mqk-testkit`
- **File:** `crates/mqk-reconcile/tests/scenario_reconcile_gate_blocks_live_arm.rs` (reconcile gate); `crates/mqk-db/tests/scenario_arm_preflight_requires_reconcile.rs` (arm integration, DB-backed)
- **Intent:** Prove that the full arming sequence (reconcile -> arm -> begin) respects both reconcile cleanliness AND lifecycle state machine.
- **Validates:** PATCH 09 + PATCH 14 integration
- **GREEN when:** Attempting arm_run on a LIVE run while dirty reconcile is simulated fails; clean reconcile + valid lifecycle state succeeds. (Note: this may need a thin orchestration layer or test helper.)

---

## PATCH 16 — CLI Lifecycle Commands (arm, begin, stop, halt, heartbeat)
Status: NOT STARTED

### Architectural Decision
The CLI (`crates/mqk-cli/src/main.rs`) currently only has `run start`. The lifecycle functions `arm_run`, `begin_run`, `stop_run`, `halt_run`, `heartbeat_run` exist in `mqk-db` but have NO CLI surface. An operator cannot arm, stop, or halt a run from the command line.

### Why This Matters (P0)
Without CLI commands for arm/stop/halt, there is NO operational path to:
- Arm a LIVE run (the arming model spec requires explicit human confirmation: `ARM LIVE {account_last4} {daily_loss_limit}`)
- Gracefully stop a running engine
- Emergency-halt a running engine
- Send heartbeats (deadman switch detection)

This makes the system **inoperable for live capital allocation**. The entire lifecycle state machine exists in the DB layer but is unreachable by an operator.

### Evidence
- `crates/mqk-cli/src/main.rs:53-68` — RunCmd only has `Start` variant
- `crates/mqk-db/src/lib.rs:272-410` — arm_run, begin_run, stop_run, halt_run, heartbeat_run all exist but no CLI caller
- `docs/specs/arming_model.md:20-21` — requires "exact operator confirmation: `ARM LIVE <last4> <daily_loss_limit>`"
- `config/defaults/base.yaml:65-67` — arming config exists (`require_manual_confirmation: true`, `confirmation_format`)

### Required CLI Commands
```
mqk run arm    --run-id <UUID> [--confirm "ARM LIVE XXXX 0.02"]
mqk run begin  --run-id <UUID>
mqk run stop   --run-id <UUID>
mqk run halt   --run-id <UUID> --reason <TEXT>
mqk run heartbeat --run-id <UUID>
mqk run status --run-id <UUID>
```

### Required Tests
- `crates/mqk-cli/tests/scenario_cli_arm_requires_confirmation.rs` — arm without confirmation string fails; arm with correct format succeeds
- `crates/mqk-cli/tests/scenario_cli_halt_stops_execution.rs` — halt transitions run to HALTED state in DB

### Recommended Settings
- Confirmation format: `config/defaults/base.yaml:67` — `"ARM LIVE {account_last4} {daily_loss_limit}"`
- Manual confirmation required: `config/defaults/base.yaml:65` — `require_manual_confirmation: true`
- Clean reconcile required: `config/defaults/base.yaml:66` — `require_clean_reconcile: true`

---

## PATCH 17 — Migration Checksum Safety + DB Operational Policy
Status: NOT STARTED

### Architectural Decision
Enforce safe DB migration practices. The `sqlx::migrate!()` macro embeds migration checksums at compile time. If a migration file is modified after being applied to a DB, `migrate()` fails with a checksum mismatch. This was observed in real development.

### Why This Matters (P0)
- `mqk_db::migrate()` is called unconditionally in `crates/mqk-db/tests/scenario_run_lifecycle_enforced.rs:24` and via `mqk db migrate` CLI (`crates/mqk-cli/src/main.rs:118`).
- There is NO documentation of the operational policy: "never reuse a prod DB for dev", "never edit applied migrations".
- The test itself has a comment acknowledging the fragility (`crates/mqk-db/tests/scenario_run_lifecycle_enforced.rs:21-23`): "if your local DB has a sqlx migration checksum mismatch, mqk_db::migrate() will fail."
- There is NO mechanism to detect or warn when `migrate()` is run against a production database.
- There is NO separate test-DB provisioning strategy.

### Evidence
- `crates/mqk-db/src/lib.rs:25-31` — `migrate()` calls `sqlx::migrate!("./migrations")` with no env-gate or safety check
- `crates/mqk-db/tests/scenario_run_lifecycle_enforced.rs:21-24` — comment acknowledges checksum mismatch risk
- `crates/mqk-cli/src/main.rs:117-119` — `mqk db migrate` runs migrations with no confirmation prompt
- No `docs/runbooks/` entry for DB migration safety
- No `config/` entry for DB migration policy

### Required Deliverables
1. **Operational runbook**: `docs/runbooks/db_migration_safety.md` documenting:
   - Never edit an applied migration
   - Dev DBs: fresh DB per test cycle (or use `DROP DATABASE` + recreate)
   - Prod DBs: migration checksums are immutable; new changes = new migration file only
   - Pre-LIVE checklist: verify `_sqlx_migrations` table checksums match compiled checksums
2. **CLI safety gate**: `mqk db migrate` should warn (or require `--yes`) before running against a DB that has `mode='LIVE'` runs
3. **Test DB helper**: utility to create/destroy ephemeral test databases (prevents checksum conflicts between test runs)

### Required Tests
- `crates/mqk-db/tests/scenario_migrate_idempotent_on_clean_db.rs` — running `migrate()` twice on a fresh DB succeeds (idempotent)
- `crates/mqk-db/tests/scenario_migrate_warns_on_live_db.rs` — if runs table has LIVE rows, migration warns or requires confirmation

### Recommended Settings
- NOT FOUND in config/docs. Needs decision: DB URL segmentation (dev vs prod) policy.

---

## PATCH 18 — Deadman Switch / Heartbeat Monitor
Status: NOT STARTED

### Architectural Decision
Implement the deadman switch pattern described in `docs/specs/arming_model.md:32-33` ("runtime/ARMED.flag deletion triggers DISARM") and heartbeat-based liveness detection. Currently, `heartbeat_run` exists in DB but nothing monitors for missed heartbeats.

### Why This Matters (P0)
If the process crashes or hangs:
- There is NO mechanism to detect the stall (heartbeat exists but nothing reads `last_heartbeat_utc` to trigger disarm)
- There is NO deadman file creation/monitoring (`config/defaults/base.yaml:12` references `runtime/ARMED.flag` but no code creates/watches it)
- There is NO auto-disarm on crash: the run stays in RUNNING state in DB forever
- There is NO operational command to detect stale heartbeats

### Evidence
- `crates/mqk-db/src/lib.rs:385-410` — `heartbeat_run()` writes `last_heartbeat_utc` but nothing reads it for staleness
- `config/defaults/base.yaml:12` — `deadman_file: "runtime/ARMED.flag"` configured but no code references it
- `config/engines/experimental.yaml:8` — `deadman_file: "runtime/EXP_ARMED.flag"` configured but unused
- `docs/specs/arming_model.md:32-33` — deadman spec exists but is unimplemented
- No `rg "deadman|ARMED.flag"` hit in any Rust source file

### Required Deliverables
1. Deadman file creation on arm, deletion on disarm/halt
2. Heartbeat staleness query: `SELECT run_id FROM runs WHERE status='RUNNING' AND last_heartbeat_utc < now() - interval '?? seconds'`
3. CLI: `mqk run check-health` — reports stale heartbeat runs
4. Runtime loop: periodic heartbeat writer (future; stub for now)

### Required Tests
- `crates/mqk-db/tests/scenario_stale_heartbeat_detected.rs` — run with old `last_heartbeat_utc` is flagged as stale
- `crates/mqk-testkit/tests/scenario_deadman_file_lifecycle.rs` — ARMED.flag created on arm, removed on halt

### Recommended Settings
- Heartbeat interval: `config/defaults/base.yaml:10` — `reconcile_interval_seconds: 60` (no explicit heartbeat interval; TBD)
- Stale threshold: `config/defaults/base.yaml:11` — `stale_data_threshold_seconds: 120` (data staleness, not heartbeat; TBD for heartbeat)
- Deadman file path: `config/defaults/base.yaml:12` — `runtime/ARMED.flag`

---

## PATCH 19 — Order Idempotency + Crash Recovery at Execution Boundary
Status: IN PROGRESS (19A COMPLETE)

### Architectural Decision
Ensure that if the process crashes between submitting an order to the broker and recording the fill, recovery does not cause double-submission. The outbox/inbox tables exist (`0001_init.sql:31-54`) and must be actively used by code.

### Why This Matters (P0)
- If the process crashes after broker submit but before recording state, a naive restart can double-submit.
- The broker adapter contract explicitly calls for idempotency via outbox/inbox uniqueness, but without wiring this is just a document.

### Evidence
- `crates/mqk-db/migrations/0001_init.sql:31-54` — `oms_outbox` and `oms_inbox` tables defined
- `docs/specs/broker_adapter_contract.md:8-9` — contract specifies idempotency safety and outbox/inbox uniqueness
- Patch 19A implementation:
  - `crates/mqk-db/src/lib.rs` — added outbox/inbox DB APIs (enqueue, dedupe insert, mark sent/acked/failed, recovery query)
  - `crates/mqk-db/tests/scenario_outbox_idempotency_prevents_double_submit.rs` — verifies idempotency_key dedupe
  - `crates/mqk-db/tests/scenario_inbox_dedupe_prevents_double_fill.rs` — verifies broker_message_id dedupe
  - `crates/mqk-db/tests/scenario_recovery_query_returns_pending_outbox.rs` — verifies recovery query returns unacked rows

### Required Deliverables

#### 19A — DB boundary primitives (COMPLETE)
1. Outbox writer API (idempotent enqueue) — COMPLETE
2. Inbox writer API (broker_message_id dedupe) — COMPLETE
3. Recovery query API for non-terminal outbox rows — COMPLETE
4. Status transition helpers (mark sent/acked/failed) — COMPLETE

#### 19B — Recovery orchestration proof (TESTKIT) (IN PROGRESS)
1. Startup recovery primitive exists that inspects unacked outbox rows and reconciles vs a broker-like state — IN PROGRESS
2. Minimal broker simulation exists to validate crash-after-submit => no double submit on restart — IN PROGRESS
3. Outbox enqueue-before-submit / inbox-dedupe-before-apply are NOT yet wired into a production runtime boundary (no broker submit path exists in repo) — BLOCKED

### Required Tests

#### 19A Tests (COMPLETE)
- `crates/mqk-db/tests/scenario_outbox_idempotency_prevents_double_submit.rs` — dedupes duplicate idempotency_key
- `crates/mqk-db/tests/scenario_inbox_dedupe_prevents_double_fill.rs` — dedupes duplicate broker_message_id
- `crates/mqk-db/tests/scenario_recovery_query_returns_pending_outbox.rs` — recovery query returns expected unacked rows

#### 19B Tests (IN PROGRESS)
- `crates/mqk-testkit/tests/scenario_crash_recovery_no_double_order.rs` — simulate crash-after-submit, verify restart reconciles vs broker — IN PROGRESS
  - NOTE: requires tokio dev-dependency in mqk-testkit

### Recommended Settings
- UNKNOWN — requires decision/spec update:
  - outbox poll interval
  - retry/backoff policy
  - max retries and failure terminalization
  - reconcile strategy (broker truth source)

---

## PATCH 20 — Arming Pre-Flight Checks Orchestration
Status: NOT STARTED

### Architectural Decision
Implement the full arming pre-flight sequence from `docs/specs/arming_model.md`: clean reconcile + risk limits present + config hash pinned + kill switches enabled + operator confirmation. Currently these checks exist individually but are not composed into a single gate.

### Why This Matters (P1)
- `is_clean_reconcile()` exists (`crates/mqk-reconcile/src/engine.rs:138-141`) but is not called before `arm_run()`
- `arm_run()` (`crates/mqk-db/src/lib.rs:272-308`) only checks lifecycle state, NOT reconcile cleanliness
- Config hash binding check `assert_run_binding()` exists but is not called in the arming path
- There is no check that risk limits are non-zero before arming LIVE
- Kill switch enablement is not verified pre-arm

### Evidence
- `crates/mqk-db/src/lib.rs:272-308` — `arm_run()` checks `status IN ('CREATED','STOPPED')` only
- `crates/mqk-reconcile/src/engine.rs:138-141` — `is_clean_reconcile()` exists but no caller in arming path
- `crates/mqk-db/src/lib.rs:238-269` — `assert_run_binding()` exists but not called in arm flow
- `docs/specs/arming_model.md:16-18` — pre-arm requires "clean reconcile, risk limits present, config hash pinned, kill switches enabled"
- `config/defaults/base.yaml:65-67` — arming config with `require_clean_reconcile: true`

### Required Deliverables
1. `arm_preflight()` function that composes: reconcile check + risk limit validation + config hash assertion + kill switch verification
2. Wire into CLI `mqk run arm` command (PATCH 16)
3. Reject arm if any check fails, with specific error

### Required Tests
- `crates/mqk-reconcile/tests/scenario_arm_blocked_without_reconcile.rs` — arm fails when reconcile is dirty
- `crates/mqk-risk/tests/scenario_arm_blocked_with_zero_risk_limits.rs` — arm fails when daily_loss_limit_micros=0 and mode=LIVE

### Recommended Settings
- `config/defaults/base.yaml:65-67` — `require_manual_confirmation: true`, `require_clean_reconcile: true`
- Risk limits: `config/defaults/base.yaml:57-58` — `daily_loss_limit: 0.02`, `max_drawdown: 0.18`

---

## PATCH 21 — Config Validation + Secret Detection at Load Time
Status: NOT STARTED

### Architectural Decision
The config loader (`crates/mqk-config/src/lib.rs`) performs deep merge and hashing but does NO validation of required fields and NO secret detection. The spec requires aborting on secret-like values in YAML.

### Why This Matters (P1)
- No required-field validation: a config missing `engine.engine_id` or `risk.daily_loss_limit` will load successfully and only fail later at runtime in an unpredictable location.
- No secret detection: a user who accidentally puts `api_key: "sk-live-abc123"` in YAML will have the secret stored in `runs.config_json` (DB) and `manifest.json` (disk).
- `docs/specs/config_layering_and_hashing.md:35-37` requires: "If secret-like value detected in YAML/effective config: abort, emit CONFIG_SECRET_DETECTED."

### Evidence
- `crates/mqk-config/src/lib.rs:8-33` — `load_layered_yaml()` merges and hashes but never validates or scans for secrets
- `docs/specs/config_layering_and_hashing.md:33-37` — secrets must never appear; abort on detection
- `crates/mqk-db/src/lib.rs:80` — `config_json` is stored directly in DB (would include secrets if present)
- `crates/mqk-artifacts/src/lib.rs:80-81` — manifest.json written to disk with config data

### Required Deliverables
1. Required-field validator: check for `engine.engine_id`, `broker.keys_env.api_key`, `broker.keys_env.api_secret`, `risk.daily_loss_limit`, `risk.max_drawdown` at minimum
2. Secret scanner: reject configs where values match patterns like `sk-`, `AKIA`, `-----BEGIN`, base64-encoded tokens, etc.
3. Wire into `load_layered_yaml()` or as a separate `validate_config()` step

### Required Tests
- `crates/mqk-config/tests/scenario_missing_required_field_rejected.rs` — loading config without `engine.engine_id` returns error
- `crates/mqk-config/tests/scenario_secret_in_yaml_aborts.rs` — loading config with literal API key value aborts with CONFIG_SECRET_DETECTED

### Recommended Settings
- Required fields: derived from `crates/mqk-isolation/src/lib.rs:54-67` (engine_id, broker keys_env) and `config/defaults/base.yaml` structure
- Secret patterns: TBD (needs decision on pattern list)

---

## PATCH 22 — Stale Data -> Execution Path Kill (End-to-End)
Status: NOT STARTED

### Architectural Decision
The integrity engine sets `st.disarmed = true` on stale feed (`crates/mqk-integrity/src/engine.rs:37-38`), but this flag is local to `IntegrityState` and is NOT propagated to the execution path. A stale feed disarm should guarantee that no new orders can be submitted.

### Why This Matters (P1)
- In the backtest engine, integrity checks run independently of the risk/execution path. The `IntegrityState.disarmed` flag is never read by the execution pipeline.
- In a live runtime (future), if the integrity engine sets disarmed=true but the execution loop doesn't check it, orders will continue to be submitted on stale data.
- `config/defaults/base.yaml:31` — `stale_policy: "DISARM"` is configured but the DISARM action has no downstream effect beyond setting a boolean.

### Evidence
- `crates/mqk-integrity/src/engine.rs:37-38` — sets `st.disarmed = true` on stale feed
- `crates/mqk-integrity/src/types.rs:106-107` — `halted` and `disarmed` are fields on IntegrityState
- `crates/mqk-backtest/src/engine.rs` — no reference to IntegrityState or disarmed flag
- `crates/mqk-risk/src/types.rs:176` — RiskState also has `disarmed` but it's not connected to IntegrityState
- No code path connects `IntegrityState.disarmed` -> execution halt

### Required Deliverables
1. Define the integration point: integrity disarm -> risk engine input (or direct execution gate)
2. In backtest: feed bars through integrity engine before strategy evaluation; if disarmed, skip execution
3. In runtime (future): integrity check must block order submission

### Required Tests
- `crates/mqk-backtest/tests/scenario_stale_data_stops_execution.rs` — backtest with stale bar gap causes execution to stop (no fills after gap)
- `crates/mqk-integrity/tests/scenario_disarm_propagates_to_risk.rs` — integrity disarm sets risk state to reject all new orders

### Recommended Settings
- `config/defaults/base.yaml:30-31` — `stale_policy: "DISARM"`, `fail_on_gap: true`, `gap_tolerance_bars: 0`
- `config/defaults/base.yaml:11` — `stale_data_threshold_seconds: 120`


## PATCH 23 — Runtime Orchestrator (Minimum Viable End-to-End Loop)
Status: NOT STARTED

### Architectural Decision
Build the smallest end-to-end orchestrator that composes existing deterministic engines into a single runnable loop:
bars -> integrity -> strategy -> execution -> (paper/sim broker) -> portfolio -> risk -> audit/artifacts.

### Why This Matters (P0)
All current "readiness" claims are theoretical unless there is a real orchestration path that wires the engines together under one run_id with artifacts and audit output.

### Evidence
- `crates/mqk-testkit/src/lib.rs` — `run_parity_scenario_stub` is explicitly a stub (no real orchestration)
- `crates/mqk-testkit/tests/scenario_replay_determinism_matches_artifacts.rs` — does not validate parity (placeholder assertions)
- `docs/specs/run_artifacts_and_reproducibility.md` — requires reproducible artifacts
- `docs/specs/data_pipeline_and_integrity.md` — defines integrity gate expectations

### Required Deliverables
1. New orchestrator module/crate that:
   - Runs fully offline from fixtures
   - Produces `exports/<run_id>/manifest.json` + audit.jsonl via existing crates
   - Emits deterministic "broker" events sufficient for portfolio/risk progression
2. A minimal "paper broker" interface for sim submissions + deterministic acks/fills (no network I/O)

### Required Tests
- `crates/mqk-testkit/tests/scenario_orchestrator_end_to_end_green.rs` — full loop runs and produces artifacts
- `crates/mqk-testkit/tests/scenario_orchestrator_blocks_on_integrity_disarm.rs` — integrity DISARM prevents execution

### Recommended Settings
- Derive from existing config only: `config/defaults/base.yaml` (`runtime.*`, `data.*`, `execution.*`, `artifacts.*`)


---

## PATCH 24 — Replace Placeholder Testkit Scenarios With Real Parity Assertions
Status: NOT STARTED

### Architectural Decision
Eliminate "green" tests that do not validate the invariant they claim. Replace with parity assertions that bind replay outputs to artifacts/audit evidence.

### Why This Matters (P0)
Passing tests that assert almost nothing create false confidence and encourage unsafe promotion to paper/live.

### Evidence
- `crates/mqk-testkit/tests/scenario_entry_places_protective_stop.rs` — currently does not validate protective stop placement (placeholder)
- `crates/mqk-testkit/tests/scenario_replay_determinism_matches_artifacts.rs` — currently does not compare artifacts/hashes (placeholder)

### Required Deliverables
1. Upgrade protective stop scenario to assert the exact intent/order sequence produced by the orchestrator.
2. Upgrade determinism parity scenario to:
   - Run scenario twice
   - Compare `config_hash`, manifest hashes, and (if enabled) audit hash chain integrity

### Required Tests
- Update existing two tests above to include real assertions and fail on mismatch.

### Recommended Settings
- None beyond existing artifact/hash behavior defined in `docs/specs/run_artifacts_and_reproducibility.md`


---

## PATCH 25 — Broker Adapter "Paper Stub" That Implements the Contract Shape
Status: NOT STARTED

### Architectural Decision
Implement a minimal broker adapter that satisfies the *shape* of the contract for testing:
submit/cancel/list_orders/positions/snapshot, plus deterministic message IDs for inbox dedupe.

### Why This Matters (P0)
Without a broker boundary (even a stub), you cannot test reconciliation truthfulness, outbox/inbox idempotency, or crash-recovery behavior.

### Evidence
- `docs/specs/broker_adapter_contract.md` — defines expected broker adapter behavior and idempotency expectations
- `crates/mqk-reconcile/src/types.rs` + `crates/mqk-reconcile/src/engine.rs` — requires `BrokerSnapshot` inputs but no source exists
- `crates/mqk-db/migrations/0001_init.sql` — `oms_outbox`/`oms_inbox` exist but are unused

### Required Deliverables
1. New crate (suggested): `crates/mqk-broker-paper/`
2. Deterministic simulation of:
   - Order acceptance/rejection
   - Partial fills
   - Broker snapshot generation for reconcile tests

### Required Tests
- `crates/mqk-testkit/tests/scenario_clean_reconcile_required_before_live_arm.rs` — uses paper broker snapshot + reconcile engine

### Recommended Settings
- Broker identity from config: `config/defaults/base.yaml` (`broker.name`, `broker.environment`, `broker.keys_env.*`)


---

## PATCH 26 — Config Consumption Map + Unused-Key Guard (Safety Lint)
Status: NOT STARTED

### Architectural Decision
Prevent silent misconfiguration by explicitly tracking which config keys are read by the selected mode and warning/failing on unused keys.

### Why This Matters (P1)
Operators will assume config settings are active when they are ignored. That is a reliability failure.

### Evidence
- `config/defaults/base.yaml` — many sections exist (runtime/data/execution/risk/etc.)
- `crates/mqk-isolation/src/lib.rs` — only reads a small subset of config via JSON pointers
- `crates/mqk-config/src/lib.rs` — merges and hashes config but does not report consumption

### Required Deliverables
1. A "consumed pointers" registry per mode (BACKTEST/PAPER/LIVE)
2. Startup report (and optional fail) when config contains keys not consumed in the chosen mode

### Required Tests
- `crates/mqk-config/tests/scenario_unused_keys_warn_or_fail.rs`

### Recommended Settings
- Mode-specific behavior must be derived from existing specs; do not add new config keys.


---

# AUDIT SUMMARY

## Part A — Evidence Summary

### Test Results
- **cargo test --workspace**: 30/30 tests pass, 0 failures, 0 ignored
- **cargo test -p mqk-db --test scenario_run_lifecycle_enforced**: 1/1 pass
- **cargo clippy --workspace --all-targets**: Only style warnings (for_kv_map, new_without_default, needless_question_mark, bool_assert_comparison). No correctness issues.

### Config Sources Found
- `config/defaults/base.yaml` — full base config with all sections (102 lines)
- `config/engines/main.yaml` — MAIN engine config
- `config/engines/experimental.yaml` — EXP engine config with separate broker keys
- `config/risk_profiles/tier_A_consistent.yaml` — MAIN risk limits
- `config/risk_profiles/experimental_tier_B.yaml` — EXP risk limits (tighter)
- `config/environments/windows-dev.yaml` — dev environment overrides
- `config/environments/linux-prod.yaml` — prod environment (live broker keys, tighter intervals)
- `config/stress_profiles/slippage_x2.yaml` — 2x slippage stress
- `config/stress_profiles/latency_high.yaml` — 1-event latency

### DB Migrations
- `0001_init.sql` — runs, audit_events, oms_outbox, oms_inbox
- `0002_run_lifecycle.sql` — status column, lifecycle timestamps, CHECK constraint, unique partial index

## Part B — Gap/Fragility Audit Table

| Area | Finding | Why It Matters | Evidence | Severity | Recommended Default |
|------|---------|---------------|----------|----------|-------------------|
| CLI/Ops | No CLI commands for arm/begin/stop/halt/heartbeat | Operator cannot control run lifecycle; system is inoperable for live use | `mqk-cli/src/main.rs:53-68` (only Start variant) vs `mqk-db/src/lib.rs:272-410` (all lifecycle fns exist) | **P0** | `arming_model.md:20-21`: "ARM LIVE {last4} {daily_loss_limit}" |
| DB | Migration checksum fragility on reused DB | `migrate()` fails silently on checksum mismatch; no operational docs exist | `mqk-db/src/lib.rs:25-31`, `scenario_run_lifecycle_enforced.rs:21-23` (comment acknowledges) | **P0** | NOT FOUND; needs DB segmentation policy |
| Recovery | Deadman switch / heartbeat monitor unimplemented | Process crash leaves run in RUNNING state forever; no auto-disarm | `mqk-db/src/lib.rs:385-410` (heartbeat writes but nothing reads); `base.yaml:12` (deadman_file configured, unused) | **P0** | `base.yaml:12`: `runtime/ARMED.flag`; staleness TBD |
| Execution | oms_outbox/oms_inbox tables unused — no crash recovery | Double-submission on crash-restart; violates broker_adapter_contract.md | `0001_init.sql:31-54` (tables exist); zero Rust references to oms_outbox/oms_inbox | **P0** | `broker_adapter_contract.md:8-9`: outbox/inbox enforce uniqueness |
| Lifecycle | arm_run() does not check reconcile cleanliness | LIVE can be armed without clean reconcile, violating arming spec | `mqk-db/src/lib.rs:272-308` (only checks status); `arming_model.md:16-18` (requires clean reconcile) | **P0** | `base.yaml:66`: `require_clean_reconcile: true` |
| Data | IntegrityState.disarmed not connected to execution path | Stale data disarm doesn't actually stop order submission | `integrity/engine.rs:37-38` (sets disarmed); backtest engine has no integrity check | **P1** | `base.yaml:31`: `stale_policy: "DISARM"` |
| Config | No required-field validation at config load | Missing critical fields (engine_id, risk limits) silently accepted | `mqk-config/src/lib.rs:8-33` (no validation); `isolation/src/lib.rs:54-67` (crashes later) | **P1** | Required fields per `base.yaml` structure |
| Config | No secret detection in config loader | Literal API keys in YAML stored in DB + manifest on disk | `mqk-config/src/lib.rs:8-33` (no scanning); `config_layering_and_hashing.md:35-37` (requires abort) | **P1** | `config_layering_and_hashing.md:35`: abort on secret detected |
| Lifecycle | No config hash / risk limit validation pre-arm | arm_run() doesn't verify config_hash pinned or risk limits non-zero | `mqk-db/src/lib.rs:272-308`; `arming_model.md:16-18` | **P1** | `base.yaml:65-67`: arming config |
| Isolation | Broker key env vars not validated as actually set pre-arm | `load_broker_keys_from_env()` exists but is never called in arming | `isolation/src/lib.rs:110-116` (loads from env); no caller in arm path | **P1** | `base.yaml:18-19`: keys_env configured |
| Audit | No hash chain verification utility | Can write hash chain but cannot verify it (no read-back + check) | `mqk-audit/src/lib.rs` (write-only; no verify function) | **P1** | NOT FOUND |
| Risk | `daily_loss_limit_micros: 0` and `max_drawdown_limit_micros: 0` disable limits entirely | Default test config has limits disabled; no guard preventing LIVE with zero limits | `mqk-risk/src/engine.rs:89,108` (skip when 0); `backtest/types.rs:60-61` (defaults to 0) | **P1** | `base.yaml:57-58`: `daily_loss_limit: 0.02`, `max_drawdown: 0.18` |
| Reconcile | Missing local orders not flagged (by design comment) | Broker might have cancelled our order; we don't know | `reconcile/engine.rs:95-97` (intentional skip with comment) | P2 | NOT FOUND; future policy flag |
| Clippy | sqlx-postgres v0.7.4 future-incompatibility warning | Will break on future Rust compiler update | clippy output: "code that will be rejected by a future version of Rust" | P2 | Upgrade sqlx to 0.8+ when stable |

## P0/P1 Findings Summary (15 bullets max)

1. **P0: No CLI arm/stop/halt commands** — lifecycle DB functions unreachable by operator (`mqk-cli/src/main.rs:53-68`)
2. **P0: Migration checksum fragility** — no operational docs, no fresh-DB-per-test, no prod safety gate (`mqk-db/src/lib.rs:25-31`)
3. **P0: Deadman switch unimplemented** — `runtime/ARMED.flag` configured but no code creates/watches it; stale heartbeats undetected
4. **P0: oms_outbox/oms_inbox completely unused** — crash recovery impossible; double-submission risk on restart
5. **P0: arm_run() skips reconcile check** — LIVE can be armed dirty, violating arming model spec
6. **P1: Stale data disarm doesn't stop execution** — IntegrityState.disarmed is an island; no downstream effect
7. **P1: No config validation or secret detection** — missing fields silently pass; secrets can leak to DB/disk
8. **P1: No pre-arm risk limit validation** — zero-limit config allows LIVE with no drawdown protection
9. **P1: No audit hash chain verifier** — write-only; tamper detection impossible
10. **P1: Broker key env vars never validated pre-arm** — `load_broker_keys_from_env()` exists but uncalled in arming
