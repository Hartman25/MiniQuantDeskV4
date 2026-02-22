# MiniQuantDesk V4 -- Spine-to-Edges File Map & Gap-Fill Plan (Deep Version)

Purpose: Provide a practical, exhaustive map of the codebase from the
architectural spine outward, including an ordered list of individual
files and a targeted list of missing modules/files to fill known gaps.
This is meant to be the working blueprint to finish the program.

## 0. Repository Roots

-   This repo includes:

```{=html}
<!-- -->
```
-   Docs/specs/runbooks/audits (design intent + operational rules)

-   Config YAML profiles (defaults, environments, engines, risk
    profiles)

-   Rust workspace (core-rs) with multiple crates

-   GUI (Tauri + React) under core-rs/mqk-gui

-   Test fixtures under tests/fixtures (bars, broker snapshots, fills)

## 1. Spine Definition

Spine = the minimal inner layer that must remain stable:
schemas/contracts → config → DB → audit/artifacts. Everything else
depends on this.

## 2. Workspace Map (core-rs)

Workspace root: core-rs/Cargo.toml

-   Topological crate order (approx. spine → outward):

```{=html}
<!-- -->
```
-   mqk-schemas (mqk-schemas 0.0.1)

-   mqk-config (mqk-config 0.0.1)

-   mqk-db (mqk-db 0.0.1)

-   mqk-audit (mqk-audit 0.0.1)

-   mqk-artifacts (mqk-artifacts 0.0.1)

-   mqk-execution (mqk-execution 0.1.0)

-   mqk-portfolio (mqk-portfolio 0.1.0)

-   mqk-risk (mqk-risk 0.1.0)

-   mqk-integrity (mqk-integrity 0.1.0)

-   mqk-reconcile (mqk-reconcile 0.1.0)

-   mqk-md (mqk-md {\'workspace\': True})

-   mqk-strategy (mqk-strategy 0.1.0)

-   mqk-isolation (mqk-isolation 0.1.0)

-   mqk-broker-paper (mqk-broker-paper 0.0.1)

-   mqk-backtest (mqk-backtest 0.1.0)

-   mqk-testkit (mqk-testkit 0.0.1)

-   mqk-promotion (mqk-promotion 0.1.0)

-   mqk-cli (mqk-cli 0.0.1)

-   mqk-daemon (mqk-daemon {\'workspace\': True})

## 3. Crate Dependency Table

  ------------------------------------------------------------------------
  Crate                 Workspace deps        Role (in one line)
  --------------------- --------------------- ----------------------------
  mqk-schemas           (none)                Canonical shared types +
                                              envelopes

  mqk-config            (none)                Layered config + hashing +
                                              secrets rules

  mqk-db                (none)                Postgres schema +
                                              migrations + run lifecycle +
                                              md ingestion

  mqk-audit             (none)                Hash chain audit logging

  mqk-artifacts         (none)                Deterministic run
                                              artifacts + manifests

  mqk-execution         (none)                Intent/order translation
                                              primitives

  mqk-portfolio         (none)                Positions, FIFO, exposure,
                                              metrics

  mqk-risk              (none)                Limits, kill switches, PDT,
                                              enforcement

  mqk-integrity         (none)                Feed quality → disarm/halt
                                              signals

  mqk-reconcile         (none)                Broker drift detection +
                                              gating

  mqk-md                (none)                Market data layer
                                              (provider/normalization)

  mqk-strategy          mqk-execution         Strategy host boundary

  mqk-isolation         mqk-portfolio         Cross-engine state bleed
                                              prevention

  mqk-broker-paper      mqk-reconcile         Paper broker adapter (stub)

  mqk-backtest          mqk-execution,        Deterministic simulation
                        mqk-integrity,        engine
                        mqk-isolation,        
                        mqk-portfolio,        
                        mqk-risk,             
                        mqk-strategy          

  mqk-testkit           mqk-artifacts,        End-to-end harness +
                        mqk-audit,            fixtures + orchestration
                        mqk-backtest,         
                        mqk-broker-paper,     
                        mqk-config, mqk-db,   
                        mqk-execution,        
                        mqk-integrity,        
                        mqk-portfolio,        
                        mqk-reconcile,        
                        mqk-risk,             
                        mqk-schemas,          
                        mqk-strategy          

  mqk-promotion         mqk-backtest,         Strategy evaluation +
                        mqk-portfolio         ranking

  mqk-cli               mqk-artifacts,        Operator CLI
                        mqk-audit,            (arming/run/backtest/etc.)
                        mqk-config, mqk-db,   
                        mqk-md, mqk-testkit   

  mqk-daemon            mqk-audit, mqk-db,    Control plane runtime
                        mqk-integrity,        (HTTP/SSE)
                        mqk-testkit           
  ------------------------------------------------------------------------

## 4. Spine-to-Edges Full File List

This is the exhaustive file listing ordered from spine outward. Paths
are relative to repo root.

### 4.1 Root-level files

-   README.md

-   README_TECHNICAL.md

-   PATCH_TRACKER.md

-   patch_tracker_updated.md

### 4.2 docs/

-   docs/audits/2026-02-17_repo_audit_and_patch_tracker.md

-   docs/audits/delta_tracking.csv

-   docs/runbooks/common_failure_modes.md

-   docs/runbooks/db_migration_safety.md

-   docs/specs/arming_model.md

-   docs/specs/backtest_policy.md

-   docs/specs/broker_adapter_contract.md

-   docs/specs/config_layering_and_hashing.md

-   docs/specs/data_pipeline_and_integrity.md

-   docs/specs/event_taxonomy.md

-   docs/specs/execution_model.md

-   docs/specs/kill_switches_and_limits.md

-   docs/specs/portfolio_and_accounting_model.md

-   docs/specs/reconciliation.md

-   docs/specs/run_artifacts_and_reproducibility.md

-   docs/specs/secrets_and_config_management.md

-   docs/specs/strategy_evaluation_and_ranking.md

-   docs/specs/strategy_framework_and_plugin_model.md

-   docs/specs/testing_fixtures_and_harness.md

-   docs/specs/testing_strategy.md

### 4.3 config/

-   config/defaults/base.yaml

-   config/engines/experimental.yaml

-   config/engines/main.yaml

-   config/environments/linux-prod.yaml

-   config/environments/windows-dev.yaml

-   config/risk_profiles/experimental_tier_B.yaml

-   config/risk_profiles/tier_A\_consistent.yaml

-   config/stress_profiles/latency_high.yaml

-   config/stress_profiles/slippage_x2.yaml

### 4.4 runtime/

-   runtime/payload_start.json

### 4.5 tests/fixtures (external fixtures)

-   tests/fixtures/bars/bars_1h_chop_meanrevert.csv

-   tests/fixtures/bars/bars_1h_gap_down_stop.csv

-   tests/fixtures/bars/bars_1h_missing_bar_gap.csv

-   tests/fixtures/bars/bars_1h_outlier_spike.csv

-   tests/fixtures/bars/bars_1h_trend_up.csv

-   tests/fixtures/broker/broker_snapshot_clean.json

-   tests/fixtures/broker/broker_snapshot_desync_unknown_order.json

-   tests/fixtures/fills/fills_duplicates.jsonl

### 4.6 core-rs workspace

-   core-rs/Cargo.toml

-   core-rs/Cargo.lock

### 4.7 Crates (ordered)

#### mqk-schemas

-   core-rs/crates/mqk-schemas/Cargo.toml

-   core-rs/crates/mqk-schemas/src/lib.rs

#### mqk-config

-   core-rs/crates/mqk-config/Cargo.toml

-   core-rs/crates/mqk-config/src/consumption.rs

-   core-rs/crates/mqk-config/src/lib.rs

-   core-rs/crates/mqk-config/tests/scenario_config_hash_stable.rs

-   core-rs/crates/mqk-config/tests/scenario_secrets_excluded.rs

-   core-rs/crates/mqk-config/tests/scenario_unused_keys_warn_or_fail.rs

#### mqk-db

-   core-rs/crates/mqk-db/Cargo.toml

-   core-rs/crates/mqk-db/migrations/0001_init.sql

-   core-rs/crates/mqk-db/migrations/0002_run_lifecycle.sql

-   core-rs/crates/mqk-db/migrations/0003_backtest_schema.sql

-   core-rs/crates/mqk-db/migrations/0004_md_quality_reports.sql

-   core-rs/crates/mqk-db/src/lib.rs

-   core-rs/crates/mqk-db/src/md.rs

-   core-rs/crates/mqk-db/tests/scenario_arm_preflight_blocks_zero_risk_limits.rs

-   core-rs/crates/mqk-db/tests/scenario_arm_preflight_requires_reconcile.rs

-   core-rs/crates/mqk-db/tests/scenario_backtest_schema_tables_exist.rs

-   core-rs/crates/mqk-db/tests/scenario_deadman_enforces_halt.rs

-   core-rs/crates/mqk-db/tests/scenario_inbox_dedupe_prevents_double_fill.rs

-   core-rs/crates/mqk-db/tests/scenario_md_ingest_csv.rs

-   core-rs/crates/mqk-db/tests/scenario_migrate_idempotent_on_clean_db.rs

-   core-rs/crates/mqk-db/tests/scenario_outbox_idempotency_prevents_double_submit.rs

-   core-rs/crates/mqk-db/tests/scenario_recovery_query_returns_pending_outbox.rs

-   core-rs/crates/mqk-db/tests/scenario_run_lifecycle_enforced.rs

#### mqk-audit

-   core-rs/crates/mqk-audit/Cargo.toml

-   core-rs/crates/mqk-audit/src/lib.rs

-   core-rs/crates/mqk-audit/tests/scenario_hash_chain_tamper_detected.rs

#### mqk-artifacts

-   core-rs/crates/mqk-artifacts/Cargo.toml

-   core-rs/crates/mqk-artifacts/src/lib.rs

#### mqk-execution

-   core-rs/crates/mqk-execution/Cargo.toml

-   core-rs/crates/mqk-execution/src/engine.rs

-   core-rs/crates/mqk-execution/src/lib.rs

-   core-rs/crates/mqk-execution/src/types.rs

-   core-rs/crates/mqk-execution/tests/scenario_target_to_intent.rs

#### mqk-portfolio

-   core-rs/crates/mqk-portfolio/Cargo.toml

-   core-rs/crates/mqk-portfolio/src/accounting.rs

-   core-rs/crates/mqk-portfolio/src/lib.rs

-   core-rs/crates/mqk-portfolio/src/metrics.rs

-   core-rs/crates/mqk-portfolio/src/types.rs

-   core-rs/crates/mqk-portfolio/tests/scenario_exposure_enforcement_multi_symbol.rs

-   core-rs/crates/mqk-portfolio/tests/scenario_pnl_partial_fills_fifo.rs

-   core-rs/crates/mqk-portfolio/tests/scenario_position_flatten_fifo.rs

#### mqk-risk

-   core-rs/crates/mqk-risk/Cargo.toml

-   core-rs/crates/mqk-risk/src/engine.rs

-   core-rs/crates/mqk-risk/src/lib.rs

-   core-rs/crates/mqk-risk/src/types.rs

-   core-rs/crates/mqk-risk/tests/scenario_auto_flatten_on_critical_event.rs

-   core-rs/crates/mqk-risk/tests/scenario_forced_halt_on_threshold_breach.rs

-   core-rs/crates/mqk-risk/tests/scenario_new_order_rejection_after_limit.rs

-   core-rs/crates/mqk-risk/tests/scenario_pdt_auto_mode_enforcement.rs

#### mqk-integrity

-   core-rs/crates/mqk-integrity/Cargo.toml

-   core-rs/crates/mqk-integrity/src/engine.rs

-   core-rs/crates/mqk-integrity/src/lib.rs

-   core-rs/crates/mqk-integrity/src/types.rs

-   core-rs/crates/mqk-integrity/tests/scenario_disarm_propagates_to_risk.rs

-   core-rs/crates/mqk-integrity/tests/scenario_feed_disagreement_halt.rs

-   core-rs/crates/mqk-integrity/tests/scenario_gap_fail.rs

-   core-rs/crates/mqk-integrity/tests/scenario_incomplete_bar_rejection.rs

-   core-rs/crates/mqk-integrity/tests/scenario_stale_feed_disarm.rs

#### mqk-reconcile

-   core-rs/crates/mqk-reconcile/Cargo.toml

-   core-rs/crates/mqk-reconcile/src/engine.rs

-   core-rs/crates/mqk-reconcile/src/lib.rs

-   core-rs/crates/mqk-reconcile/src/types.rs

-   core-rs/crates/mqk-reconcile/tests/scenario_drift_detection.rs

-   core-rs/crates/mqk-reconcile/tests/scenario_reconcile_gate_blocks_live_arm.rs

-   core-rs/crates/mqk-reconcile/tests/scenario_reconcile_required_before_live.rs

-   core-rs/crates/mqk-reconcile/tests/scenario_unknown_order_detection.rs

#### mqk-md

-   core-rs/crates/mqk-md/Cargo.toml

-   core-rs/crates/mqk-md/src/lib.rs

#### mqk-strategy

-   core-rs/crates/mqk-strategy/Cargo.toml

-   core-rs/crates/mqk-strategy/src/host.rs

-   core-rs/crates/mqk-strategy/src/lib.rs

-   core-rs/crates/mqk-strategy/src/types.rs

-   core-rs/crates/mqk-strategy/tests/scenario_multi_strategy_rejection.rs

-   core-rs/crates/mqk-strategy/tests/scenario_shadow_mode_does_not_execute.rs

-   core-rs/crates/mqk-strategy/tests/scenario_timeframe_mismatch_rejection.rs

#### mqk-isolation

-   core-rs/crates/mqk-isolation/Cargo.toml

-   core-rs/crates/mqk-isolation/src/lib.rs

-   core-rs/crates/mqk-isolation/tests/scenario_cross_engine_state_bleed_prevented.rs

#### mqk-broker-paper

-   core-rs/crates/mqk-broker-paper/Cargo.toml

-   core-rs/crates/mqk-broker-paper/src/lib.rs

-   core-rs/crates/mqk-broker-paper/src/types.rs

#### mqk-backtest

-   core-rs/crates/mqk-backtest/Cargo.toml

-   core-rs/crates/mqk-backtest/src/engine.rs

-   core-rs/crates/mqk-backtest/src/lib.rs

-   core-rs/crates/mqk-backtest/src/types.rs

-   core-rs/crates/mqk-backtest/tests/scenario_allocation_cap_enforced.rs

-   core-rs/crates/mqk-backtest/tests/scenario_ambiguity_worst_case_enforced.rs

-   core-rs/crates/mqk-backtest/tests/scenario_replay_determinism.rs

-   core-rs/crates/mqk-backtest/tests/scenario_stale_data_stops_execution.rs

-   core-rs/crates/mqk-backtest/tests/scenario_stress_impact_measurable.rs

#### mqk-testkit

-   core-rs/crates/mqk-testkit/Cargo.toml

-   core-rs/crates/mqk-testkit/src/lib.rs

-   core-rs/crates/mqk-testkit/src/orchestrator.rs

-   core-rs/crates/mqk-testkit/src/paper_broker.rs

-   core-rs/crates/mqk-testkit/src/recovery.rs

-   core-rs/crates/mqk-testkit/tests/scenario_clean_reconcile_required_before_live_arm.rs

-   core-rs/crates/mqk-testkit/tests/scenario_cli_run_start_creates_artifacts.rs

-   core-rs/crates/mqk-testkit/tests/scenario_crash_recovery_no_double_order.rs

-   core-rs/crates/mqk-testkit/tests/scenario_entry_places_protective_stop.rs

-   core-rs/crates/mqk-testkit/tests/scenario_orchestrator_blocks_on_integrity_disarm.rs

-   core-rs/crates/mqk-testkit/tests/scenario_orchestrator_end_to_end_green.rs

-   core-rs/crates/mqk-testkit/tests/scenario_replay_determinism_matches_artifacts.rs

-   core-rs/crates/mqk-testkit/tests/scenario_run_artifacts_manifest_created.rs

#### mqk-promotion

-   core-rs/crates/mqk-promotion/Cargo.toml

-   core-rs/crates/mqk-promotion/src/evaluator.rs

-   core-rs/crates/mqk-promotion/src/lib.rs

-   core-rs/crates/mqk-promotion/src/types.rs

-   core-rs/crates/mqk-promotion/tests/scenario_backtest_to_promotion_pipeline.rs

-   core-rs/crates/mqk-promotion/tests/scenario_fail_below_threshold.rs

-   core-rs/crates/mqk-promotion/tests/scenario_pass_above_threshold.rs

-   core-rs/crates/mqk-promotion/tests/scenario_tie_break_correctness.rs

#### mqk-cli

-   core-rs/crates/mqk-cli/Cargo.toml

-   core-rs/crates/mqk-cli/src/main.rs

-   core-rs/crates/mqk-cli/tests/scenario_cli_arm_requires_confirmation.rs

-   core-rs/crates/mqk-cli/tests/scenario_cli_db_migrate_requires_yes_when_live_active.rs

-   core-rs/crates/mqk-cli/tests/scenario_cli_halt_stops_execution.rs

#### mqk-daemon

-   core-rs/crates/mqk-daemon/Cargo.toml

-   core-rs/crates/mqk-daemon/src/main.rs

### 4.8 GUI (Tauri + React)

-   core-rs/mqk-gui/.gitignore

-   core-rs/mqk-gui/README.md

-   core-rs/mqk-gui/index.html

-   core-rs/mqk-gui/package-lock.json

-   core-rs/mqk-gui/package.json

-   core-rs/mqk-gui/public/tauri.svg

-   core-rs/mqk-gui/public/vite.svg

-   core-rs/mqk-gui/src/App.css

-   core-rs/mqk-gui/src/App.tsx

-   core-rs/mqk-gui/src/assets/react.svg

-   core-rs/mqk-gui/src/main.tsx

-   core-rs/mqk-gui/src/vite-env.d.ts

-   core-rs/mqk-gui/src-tauri/.gitignore

-   core-rs/mqk-gui/src-tauri/Cargo.lock

-   core-rs/mqk-gui/src-tauri/Cargo.toml

-   core-rs/mqk-gui/src-tauri/build.rs

-   core-rs/mqk-gui/src-tauri/capabilities/default.json

-   core-rs/mqk-gui/src-tauri/gen/schemas/acl-manifests.json

-   core-rs/mqk-gui/src-tauri/gen/schemas/capabilities.json

-   core-rs/mqk-gui/src-tauri/gen/schemas/desktop-schema.json

-   core-rs/mqk-gui/src-tauri/gen/schemas/windows-schema.json

-   core-rs/mqk-gui/src-tauri/icons/128x128.png

-   core-rs/mqk-gui/src-tauri/icons/128x128@2x.png

-   core-rs/mqk-gui/src-tauri/icons/32x32.png

-   core-rs/mqk-gui/src-tauri/icons/Square107x107Logo.png

-   core-rs/mqk-gui/src-tauri/icons/Square142x142Logo.png

-   core-rs/mqk-gui/src-tauri/icons/Square150x150Logo.png

-   core-rs/mqk-gui/src-tauri/icons/Square284x284Logo.png

-   core-rs/mqk-gui/src-tauri/icons/Square30x30Logo.png

-   core-rs/mqk-gui/src-tauri/icons/Square310x310Logo.png

-   core-rs/mqk-gui/src-tauri/icons/Square44x44Logo.png

-   core-rs/mqk-gui/src-tauri/icons/Square71x71Logo.png

-   core-rs/mqk-gui/src-tauri/icons/Square89x89Logo.png

-   core-rs/mqk-gui/src-tauri/icons/StoreLogo.png

-   core-rs/mqk-gui/src-tauri/icons/icon.icns

-   core-rs/mqk-gui/src-tauri/icons/icon.ico

-   core-rs/mqk-gui/src-tauri/icons/icon.png

-   core-rs/mqk-gui/src-tauri/src/lib.rs

-   core-rs/mqk-gui/src-tauri/src/main.rs

-   core-rs/mqk-gui/src-tauri/tauri.conf.json

-   core-rs/mqk-gui/tsconfig.json

-   core-rs/mqk-gui/tsconfig.node.json

-   core-rs/mqk-gui/vite.config.ts

## 5. Gap Analysis (Targeted Missing Modules/Files)

These are proposed additions to make the architecture explicit and
reduce coupling. They are not random --- they reflect boundaries implied
by docs/specs and by the existing scenario tests.

### mqk-md -- missing modules to add

-   core-rs/crates/mqk-md/src/provider.rs

```{=html}
<!-- -->
```
-   Provider trait(s) for historical/live bar sources (TwelveData now,
    others later).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-md/src/normalizer.rs

```{=html}
<!-- -->
```
-   Canonical OHLCV normalization into md_bars (micros, UTC epoch,
    completeness).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-md/src/quality.rs

```{=html}
<!-- -->
```
-   Data Quality Gate report builder (duplicates, gaps, monotonicity,
    missing, outliers).

### mqk-daemon -- missing modules to add

-   core-rs/crates/mqk-daemon/src/routes.rs

```{=html}
<!-- -->
```
-   Axum routes split from main; REST endpoints + SSE stream wiring.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-daemon/src/state.rs

```{=html}
<!-- -->
```
-   Runtime state container: handles run lifecycle status + shared
    resources.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-daemon/src/api_types.rs

```{=html}
<!-- -->
```
-   Request/response structs for endpoints (v1).

### mqk-cli -- missing modules to add

-   core-rs/crates/mqk-cli/src/commands/mod.rs

```{=html}
<!-- -->
```
-   Command modules split out of main.rs to reduce fragility.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-cli/src/commands/backtest.rs

```{=html}
<!-- -->
```
-   Backtest command module (parsing, dispatch).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-cli/src/commands/run.rs

```{=html}
<!-- -->
```
-   Run command module (start/stop/halt/arm/disarm).

### mqk-execution -- missing modules to add

-   core-rs/crates/mqk-execution/src/order_router.rs

```{=html}
<!-- -->
```
-   Intent → broker routing boundary (single place that touches broker
    adapter).

### mqk-reconcile -- missing modules to add

-   core-rs/crates/mqk-reconcile/src/snapshot_adapter.rs

```{=html}
<!-- -->
```
-   Deserialize broker snapshots + normalize to internal types.

### mqk-risk -- missing modules to add

-   core-rs/crates/mqk-risk/src/pdt.rs

```{=html}
<!-- -->
```
-   Explicit PDT enforcement policy and helpers (separate from generic
    limits).

### mqk-portfolio -- missing modules to add

-   core-rs/crates/mqk-portfolio/src/ledger.rs

```{=html}
<!-- -->
```
-   Ledger abstraction to make FIFO and PnL rules explicit/isolated.

### mqk-strategy -- missing modules to add

-   core-rs/crates/mqk-strategy/src/plugin_registry.rs

```{=html}
<!-- -->
```
-   Registry of available strategies + metadata; supports plugin model
    later.

## 6. Manual Build Order (Inside-Out)

1.  Step 1: Keep spine green (schemas/config/db/audit/artifacts) -- no
    feature work until check/fmt/clippy/test are green.

2.  Step 2: Finish market-data boundaries (mqk-md) -- provider +
    normalization + DQ gate. This feeds both backtest and live.

3.  Step 3: Integrity → Risk propagation: prove stale/disarm blocks
    execution submission paths end-to-end.

4.  Step 4: Broker boundary + reconcile: paper broker contract +
    reconcile engine gates arm/run.

5.  Step 5: Execution + portfolio correctness: intents, fills, ledger
    invariants, exposure enforcement.

6.  Step 6: Backtest end-to-end (CLI + engine + artifacts): backtest
    should produce deterministic artifacts and match replay determinism
    tests.

7.  Step 7: Promotion pipeline: backtest outputs feed evaluator; enforce
    thresholds and tie-break tests.

8.  Step 8: CLI/Daemon/GUI operational shell: only after core engines
    are deterministic and proven by scenario tests.

## 7. Notes on How to Use This Document

Use the file list as your execution plan. When you open a patch, pick
the smallest missing boundary file(s) and wire it in with minimal
surface-area changes. Your gate is always: fmt → clippy -D warnings →
test.

## 8. Institutional-Grade Gaps (What's Missing for a Capital Allocator)

Below are the major missing capabilities that typically separate a
hobby/solo quant platform from an institutional-style capital allocator.
These are grouped by function and expressed as concrete modules/files to
add (or boundaries to harden) using the same format as earlier gap
sections.

### 8.1 Portfolio Construction & Allocation

-   core-rs/crates/mqk-portfolio/src/allocator.rs

```{=html}
<!-- -->
```
-   Portfolio construction: target weights, constraints, and allocation
    decisions (separate from execution).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-portfolio/src/constraints.rs

```{=html}
<!-- -->
```
-   Constraint system: max position %, sector caps, leverage,
    concentration, liquidity filters.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-portfolio/src/attribution.rs

```{=html}
<!-- -->
```
-   Performance attribution: PnL decomposition by
    symbol/strategy/factor; ties to artifacts/reporting.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-portfolio/src/benchmarks.rs

```{=html}
<!-- -->
```
-   Benchmark definitions and relative risk metrics (tracking error,
    active risk).

### 8.2 Risk Modeling (Beyond Hard Limits)

-   core-rs/crates/mqk-risk/src/var.rs

```{=html}
<!-- -->
```
-   VaR / CVaR estimators (parametric + historical) with configurable
    horizons and confidence levels.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-risk/src/stress.rs

```{=html}
<!-- -->
```
-   Stress testing harness (scenario shocks, correlation breaks,
    volatility regime shifts).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-risk/src/exposure.rs

```{=html}
<!-- -->
```
-   Factor/sector/asset-class exposure aggregation (ties to symbol
    classification tables).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-risk/src/liquidity.rs

```{=html}
<!-- -->
```
-   Liquidity + capacity model (ADV participation caps, spread
    assumptions).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-risk/src/limits_engine.rs

```{=html}
<!-- -->
```
-   Unified risk decision engine: consumes integrity signals, portfolio
    state, reconcile state, and config.

### 8.3 Pre-Trade / Post-Trade Compliance

-   core-rs/crates/mqk-risk/src/compliance/mod.rs

```{=html}
<!-- -->
```
-   Compliance module boundary (rules engine).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-risk/src/compliance/pre_trade.rs

```{=html}
<!-- -->
```
-   Pre-trade checks: restricted list, max order size, price bands,
    short-sale constraints, PDT gating.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-risk/src/compliance/post_trade.rs

```{=html}
<!-- -->
```
-   Post-trade checks: trade surveillance flags, wash-sale heuristics
    (if applicable), anomaly detection.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-db/migrations/00xx_compliance_tables.sql

```{=html}
<!-- -->
```
-   DB tables for compliance rules, restricted lists, and
    violations/alerts history.

### 8.4 Institutional OMS & Execution Quality

-   core-rs/crates/mqk-execution/src/oms/mod.rs

```{=html}
<!-- -->
```
-   OMS boundary: order intents, state machine, retries, idempotency,
    and broker acknowledgements.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-execution/src/oms/state_machine.rs

```{=html}
<!-- -->
```
-   Order state machine with deterministic transitions
    (Submitted/Ack/Partial/Filled/Canceled/Rejected).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-execution/src/tca.rs

```{=html}
<!-- -->
```
-   Transaction Cost Analysis: slippage, spread, implementation
    shortfall, fill quality metrics.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-execution/src/slippage_model.rs

```{=html}
<!-- -->
```
-   Pluggable slippage/latency model used by backtest and live
    estimation.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-execution/src/smart_router.rs

```{=html}
<!-- -->
```
-   Smart routing abstraction (even if only one broker today) to keep
    execution policy explicit.

### 8.5 Market Data Ops (Lineage, Corporate Actions, Reference Data)

-   core-rs/crates/mqk-md/src/lineage.rs

```{=html}
<!-- -->
```
-   Data lineage: source, fetch timestamps, checksum, normalization
    version, quality gate results.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-md/src/corporate_actions.rs

```{=html}
<!-- -->
```
-   Corporate actions ingestion + adjustment logic (splits/dividends)
    for clean backtests.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-db/migrations/00xx_reference_data.sql

```{=html}
<!-- -->
```
-   Reference data tables: exchanges, calendars, trading sessions,
    symbol metadata, corporate actions.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-md/src/calendar.rs

```{=html}
<!-- -->
```
-   Trading calendar utilities (sessions/holidays) shared by backtest
    and live.

### 8.6 Backoffice: Accounting, Reconciliation, and Statements

-   core-rs/crates/mqk-portfolio/src/accounting.rs

```{=html}
<!-- -->
```
-   Accounting rules: realized/unrealized PnL, fees, dividends, borrow
    costs (if relevant).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-reconcile/src/custody_recon.rs

```{=html}
<!-- -->
```
-   Custodian/broker statement reconciliation (positions, cash, fills)
    with discrepancy reports.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-db/migrations/00xx_accounting_tables.sql

```{=html}
<!-- -->
```
-   DB tables for cash ledger, fees, dividends, borrow, and statement
    snapshots.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-artifacts/src/reports/mod.rs

```{=html}
<!-- -->
```
-   Report generator: daily/monthly portfolio reports; ties to
    artifacts + audit chain.

### 8.7 Observability, Operations, and Runbooks

-   core-rs/crates/mqk-daemon/src/metrics.rs

```{=html}
<!-- -->
```
-   Prometheus-style metrics (or structured metrics sink): orders,
    latencies, DQ stats, risk events.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-daemon/src/health.rs

```{=html}
<!-- -->
```
-   Health endpoints: readiness/liveness and dependency checks (DB,
    provider, broker).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-daemon/src/logging.rs

```{=html}
<!-- -->
```
-   Structured logging setup (trace IDs, run IDs, component tags).

```{=html}
<!-- -->
```
-   runtime/docker/Dockerfile.core

```{=html}
<!-- -->
```
-   Containerization for repeatable deploys (even local).

```{=html}
<!-- -->
```
-   runtime/docker/docker-compose.yml

```{=html}
<!-- -->
```
-   Local orchestration: Postgres + core daemon + optional
    Grafana/Prometheus stack.

```{=html}
<!-- -->
```
-   docs/runbooks/incident_response.md

```{=html}
<!-- -->
```
-   Operational runbook: alerts, halts, incident triage, recovery steps.

### 8.8 Security & Governance

-   core-rs/crates/mqk-config/src/secrets.rs

```{=html}
<!-- -->
```
-   Formal secrets policy enforcement (env var names only, redaction,
    denylist).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-daemon/src/auth.rs

```{=html}
<!-- -->
```
-   Auth boundary for control plane actions (even local token auth).

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-db/migrations/00xx_authz.sql

```{=html}
<!-- -->
```
-   DB tables for users/roles/tokens (if multi-operator).

```{=html}
<!-- -->
```
-   docs/security/threat_model.md

```{=html}
<!-- -->
```
-   Threat model + hardening checklist (keys, logs, data exfiltration,
    supply chain).

### 8.9 CI/CD & Deterministic Build Hygiene

-   .github/workflows/ci.yml

```{=html}
<!-- -->
```
-   CI pipeline: fmt, clippy -D warnings, tests, migrations check,
    artifacts build.

```{=html}
<!-- -->
```
-   docs/dev/build_matrix.md

```{=html}
<!-- -->
```
-   Document supported toolchains, features, and deterministic build
    expectations.

```{=html}
<!-- -->
```
-   core-rs/crates/mqk-testkit/src/golden.rs

```{=html}
<!-- -->
```
-   Golden artifact tests (backtest outputs must match expected hashes).

## 9. Reality Check: Minimum Set to Call It 'Institutional-Like'

If you want the smallest defensible subset that feels like an
institutional allocator (not a toy), prioritize the following before
expanding feature surface area:

-   Deterministic backtest + artifact hashing + golden tests (prove
    repeatability).

-   Unified risk decision engine that blocks execution paths
    (integrity + reconcile + limits).

-   Robust OMS state machine + idempotent broker boundary + drift
    reconciliation.

-   Market data lineage + corporate action handling (or explicit
    'no-adjust' policy with tests).

-   Daily reporting: positions, PnL, exposures, and exception alerts
    with audit trail.

-   Operational runbooks + metrics/health endpoints + CI gating
    (fmt/clippy/test).
