# MiniQuantDesk V4 — Operator Ledger

Update this after audits, proof runs, patch landings, or major repo changes.

## Current repo posture

- **Branch / state:** update before each serious work session
- **Last known clean full proof:** update with commit hash and date
- **Last known major failures:** update as needed
- **Current high-priority domain:** MAIN remaining-work closure
- **Current active patch:** operator-maintained
- **Current blocked patch:** operator-maintained
- **Current parked verification list location:** operator-maintained

## Canonical truth reminders

- **Canonical engine:** `MAIN`
- **Non-canonical engine:** `EXP` research-only unless explicitly promoted
- **DB-authoritative areas:** wherever readiness lock/scorecard require DB-backed truth
- **Mounted but not fully wired risk:** some surfaces may exist before full authority exists
- **Areas requiring fail-closed behavior:** operator truth surfaces, restart/control semantics, suppressions/summary/config-diff style truth

## Repo Reality Ledger

### Readiness state
- **Mechanical proof posture:** strong when canonical committed-state proof is green
- **Open caution:** a source-level truth concern is not erased by broad proof alone if the concern remains disputed
- **Working stance:** readiness requires both proof and honest source semantics

### Completion state
- **Strong:** core platform infrastructure, truth-conscious design, patch discipline
- **Partial:** strategy registry truth, suppressions truth, restart/mode transitions, canonical OMS/metrics surfaces
- **Deferred / not yet part of completion claims:** broader asset-class expansion, EXP promotion, full multi-account architecture

### Maintainability state
- **Current strain:** sink files in daemon state/routes, runtime orchestrator, DB layer, GUI system APIs
- **Refactor principle:** semantics first, decomposition later, one patch at a time

## Trading Viability Ledger

### Strategy / alpha state
- still open
- no durable post-friction edge should be claimed without hard evidence

### Research → production state
- still open
- artifact chain and promotion semantics remain important closure work

### Data / tradability state
- partial and bounded
- do not treat current data posture as broad multi-asset production readiness

### Economic viability state
- still open
- economics and deployment/tradability gate remain unfinished

## Live Ops Ledger

### Operator workflow state
- partial
- truthfulness matters more than surface completeness

### Crash / restart / recovery state
- partial
- restart and mode-transition closure remain open

### Observability / alerting state
- partial
- dashboards and runbooks still need strengthening

### Broker / feed / reconcile failure-handling state
- stronger than many other areas, but not a substitute for true ops maturity

### Runbook quality state
- still open

## Patch Ledger

_Last documented full proof snapshot: 2026-04-14, commit 236118b656984f4132539a3dfb99a4d2c3d0bd10 (17/17 lanes PASSED; full DB-backed institutional proof). Current HEAD: a8905aa. HEAD is 14 commits post-snapshot; no updated full proof run is documented against current HEAD._

### Open / active MAIN patches

_No MAIN patches currently outstanding following LEDGER-RECLASS-01 reclassification (2026-04-15)._

**Conditional only:** RESEARCH-NON-EQ-01 (open only if non-equity canonical surfaces are mounted)

**Non-blocking follow-up only:** MT-07 (decomposition; does not block any MAIN lane)

### Closed — patch-local re-audit confirmed (2026-04-15)

Implementation and targeted scenario proof exist in committed HEAD. Closure is patch-local only: scenario files and real implementation are committed; no new full-harness proof run has been documented against current HEAD. HEAD is post-snapshot relative to 236118b (2026-04-14).

| Patch | Proof artifacts (committed HEAD) | Scope note |
|---|---|---|
| **RUNTIME-LONGRUN-01** | `mqk-daemon/tests/scenario_runtime_longrun_01.rs` (LR-01..LR-06, 6 pure in-process); `mqk-runtime/tests/scenario_runtime_longrun_01.rs` (LR-RT-01..LR-RT-03, 3 DB-backed; skip without `MQK_DATABASE_URL`) | Repeated-cycle runtime ingest / cursor / idempotency invariants; CI-11 guard passed. |
| **CTRL-AUTH-01** | `mqk-daemon/tests/scenario_ctrl_auth_01.rs` (CA-01..CE-03, 21 pure in-process) | Control-plane authority consistency: arm/disarm surfaces, kill-switch semantics, auth fail-closed, idempotency, no stale routes. |
| **DATA-INTEGRITY-01** | `mqk-testkit/tests/scenario_data_integrity_01.rs` (DI-01..DI-04, 4 pure); `mqk-testkit/tests/scenario_data_integrity_01_db.rs` (DB-DI-01..DB-DI-03, 3 DB-backed; skip without `MQK_DATABASE_URL`) | Multi-cycle idempotency, duplicate-event convergence, reconcile halt semantics, D2 crash-recovery alignment. |
| **EXEC-PROTECT-01** | `mqk-testkit/tests/scenario_exec_protect_01.rs` (EP-UNSAFE..EP-STALE, 11 pure in-process) | Unified execution-protection gate ordering for submit, cancel, and replace; closes GAP-A/B/C left by prior per-operation coverage. |
| **CORP-ACT-01** | `mqk-testkit/tests/scenario_corp_act_01.rs` (CA-01..CA-04, 4 pure in-process) | **Scope: backtest/accounting seam only.** Split-adjustment economic neutrality, `ForbidPeriods` boundary halt, explicit drift visibility. Live OMS corp-action path is not wired; B7 separately proves operator surface honesty (`corp_actions_screening = "not_wired"`). |

### Closed — committed-state code/test proof verified (2026-04-12)

**Maintainability series:**
- **MT-01** — `routes/execution_order_analysis.rs` extracted A5/outbox handlers from `execution.rs`; `routes/system_artifact.rs` extracted artifact/parity/topology handlers from `system.rs`; `mod` declarations + router imports updated in `routes.rs`; workspace compiles clean
- **MT-02** — `orchestrator/dispatch.rs` extracted Phase-1 dispatch helpers (`dispatch_submit_claimed_outbox_row`, `dispatch_cancel_claimed_outbox_row`) from `orchestrator.rs`; `mod dispatch;` registered; workspace compiles clean
- **MT-03** — `mqk-db/src/inbox.rs` extracted from `orders.rs`; `lib.rs` re-exports `inbox::*`; GUI route layer already modular (14 `routes/` modules in HEAD); workspace compiles clean

**Initial audit batch (IR/CC/TV/LO series):**
- **IR-01** — `scenario_operator_audit_ir01.rs` (3 pure tests P1–P3); DB-backed proofs `ir01_control_arm_no_run_no_synthetic_run_created`, `ir01_control_disarm_no_run_no_synthetic_run_created`, `ir01_control_arm_with_real_run_writes_audit_event` in `scenario_daemon_runtime_lifecycle.rs`
- **IR-02** — `scenario_operator_audit_ir02.rs`
- **CC-01** — `StrategySummaryResponse` in `api_types.rs`; daemon strategy summary route
- **CC-02** — `sys_strategy_suppressions` migration; fleet enable/disable scenarios
- **CC-03** — `ModeChangeGuidanceResponse`; `scenario_mode_transition_cc03a/b/c.rs`
- **CC-04** — `OmsOverviewResponse`; `scenario_oms_overview_cc04.rs`
- **CC-05** — `MetricsDashboardResponse`; `scenario_metrics_dashboards_cc05.rs`
- **TV-01** — `contracts.py` (`PromotedArtifactManifest`); `signal_pack/promote.py`; `test_artifact_contract.py`
- **TV-02** — `deployment/gate.py`; `test_deployability_gate.py`
- **TV-03** — `deployment/parity.py`; `test_parity_evidence.py`
- **TV-04** — `capital_policy` module; five `scenario_capital_policy_tv04*.rs` files
- **LO-01** — `docs/runbooks/operator_workflows.md` (full 9-section runbook)
- **LO-02** — `docs/runbooks/stressed_recovery_proof_matrix.md`; `scenario_stressed_recovery_lo02.rs`
- **LO-03** — `docs/runbooks/live_shadow_operational_proof.md`; `scenario_live_shadow_preflight_lo03.rs`
- **DOC-01** — AI workflow docs Batch 1A partial reconciliation 2026-04-11; superseded by DOC-TRUTH-FINAL-01
- **DOC-TRUTH-FINAL-01** — Four ai_workflow + readiness docs reconciled to committed proof state a0b017d4; stale commit refs, false closure claims, remaining-work truth corrected (2026-04-12)

**Subsequently closed (scenario-file proof in committed repo, verified 2026-04-11):**
- **CC-01B** — `scenario_strategy_summary_registry.rs`
- **CC-02D** — `scenario_fleet_enable_disable_interaction_cc02d.rs`
- **CC-06** — `scenario_alerts_events_cc06.rs`
- **TV-01A–D** — `scenario_artifact_schema_lock_tv01a.rs`; `scenario_artifact_intake_tv01b.rs`; `scenario_artifact_provenance_tv01cd.rs`
- **TV-02A/B/C** (runtime deployability gate) — `scenario_artifact_deployability_tv02.rs`
- **TV-03** (daemon proof) — `scenario_parity_evidence_tv03.rs`
- **TV-03C** — `scenario_parity_evidence_tv03c.rs`
- **TV-04C–F** — `scenario_capital_policy_tv04c/d/e/f.rs`
- **TV-EXEC-01 / TV-EXEC-01B** — `scenario_fill_quality_tv_exec01.rs`; `scenario_fill_quality_orchestrator_tv_exec01b.rs`
- **A3/A4, A5A–A5E** — `scenario_a3_a4_operator_surfaces.rs`; `scenario_order_timeline_a5a.rs` through `scenario_order_causality_a5e.rs`
- **B1A–B1C** — `scenario_native_strategy_bootstrap_daemon_b1a.rs`; `_loop_dispatch_b1b.rs`; `_bridge_b1c.rs`
- **B2A–B3** — `scenario_native_strategy_registry_b2a.rs`; `scenario_strategy_fleet_control_truth_b2b_b3.rs`
- **B4–B6** — `scenario_protection_status_b4.rs`; `scenario_native_strategy_b5_short_guard.rs`; `_b6_budget_gate.rs`
- **B7–B8** — `scenario_corp_actions_b7.rs`; `scenario_asset_class_scope_b8.rs`
- **LO-02B–E** — `scenario_shadow_recovery_lo02b.rs` through `scenario_kill_switch_persistence_lo02e.rs`
- **LO-03A–G** — `scenario_live_shadow_operator_lo03ab.rs` through `scenario_audit_ops_lo03g.rs`
- **BRK-00R-04/05, BRK-07R, BRK-08R, BRK-00R-06** — ws continuity/transport/cursor/gap/proof bundle scenarios
- **BRK-09R** — `scenario_reconcile_start_gate_brk09r.rs`
- **C1–C4, LT-01** — `scenario_live_trust_c1.rs` through `scenario_live_trust_chain_lt01.rs`
- **RTS-07 / RSK-07** — `scenario_combined_paper_gate_rts07_rsk07.rs`
- **PTA-01/02** — `scenario_canonical_paper_path_pta01.rs`; `scenario_paper_survivability_pta02.rs`
- **OPS-08/09, EXEC-06, JOUR-01** — `scenario_paper_supervision_ops08_exec06.rs`; `scenario_paper_journal_jour01_ops09.rs`
- **OPS-NOTIFY-01, DIS-01/02** — `scenario_notify_ops01.rs`; `scenario_discord_dis01_dis02.rs`
- **AC-01, AUTON-01, AUTON-TRUTH-01, AH-01** — `scenario_autonomous_*.rs` series
- **OC-01/02** — `scenario_ops_control_oc01_oc02.rs`
- **RT-01** — `scenario_route_contract_rt01.rs`
- **EXEC-02** — `scenario_replace_cancel_chains_exec02.rs` (7 tests); `0035_oms_order_lifecycle_events.sql`; `order_lifecycle.rs`; orchestrator Phase 3b lifecycle hook

### Closed — doc/workflow only (no runtime proof required)

Taxonomy: **DOC/WORKFLOW-ONLY** — documentation or workflow-only change; no scenario file expected or applicable. **POST-SNAPSHOT** if committed after the last documented harness run (236118b656984f4132539a3dfb99a4d2c3d0bd10, 2026-04-14; see `.proof/proof_snapshot.json`).

| Patch | Label | Note |
|---|---|---|
| **AUTON-10** | DOC/WORKFLOW-ONLY · POST-SNAPSHOT | `MASTER_COMMAND_BRIEF.md` posture correction; doc-only; committed 2026-04-14 (post-snapshot per `.proof/proof_snapshot.json`). No runtime proof required. |

### Disputed / needs targeted re-audit
Use this section when a broad green proof exists but a source-level concern may still survive.

### Superseded
Use this section for findings retired by newer proof or newer source inspection.

## Evidence Ledger

- **Latest repo snapshot:** committed HEAD a8905aa; last documented clean proof snapshot commit 236118b656984f4132539a3dfb99a4d2c3d0bd10 (2026-04-14); HEAD is 14 commits post-snapshot
- **Latest full proof transcript:** `.proof/full_repo_proof_output.txt` — commit 236118b656984f4132539a3dfb99a4d2c3d0bd10 (2026-04-14; 17/17 lanes PASSED; `audit_profile=full_db_backed_institutional_proof_audit`; `workspace_state=committed_repo_state`)
- **Machine-readable proof provenance:** `.proof/proof_snapshot.json` (`schema_version=proof-snapshot-v1`); structured extraction of the snapshot above; no updated run documented for HEAD
- **Latest readiness lock:** `docs/INSTITUTIONAL_READINESS_LOCK.md`
- **Latest scorecard:** `docs/INSTITUTIONAL_SCORECARD.md`
- **Latest carried-forward audits:** update list/date
- **Latest EXP isolation policy:** update if changed

## EXP isolation reminders

- EXP is research-only by default.
- EXP must not widen MAIN readiness or proof burden.
- EXP may share platform primitives but not operational truth.
- EXP work should stay out of daemon truth, GUI truth, canonical metrics surfaces, and readiness claims.

## Working notes for the next prompt

- **Smallest useful context bundle:** operator-maintained
- **Files to inspect first:** operator-maintained
- **Tempting but probably unnecessary files:** operator-maintained
- **Exact question to ask next:** operator-maintained
