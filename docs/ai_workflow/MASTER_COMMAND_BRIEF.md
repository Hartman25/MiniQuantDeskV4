# MiniQuantDesk V4 — Master Command Brief

Keep this file compact and current. This is the top-level command map to give an AI before non-trivial repo work.

## Repo identity

- **Project name:** MiniQuantDesk V4
- **Current stage:** strong partial platform with active closure work still open
- **Primary purpose:** build an institutional-style, deterministic trading and research platform with truthful operator surfaces, durable artifacts, controlled execution, and auditable proof
- **What this system is not:** not yet a complete trading business, not yet a proven alpha engine, not yet a mature live-ops platform, not yet a multi-asset production stack

## Canonical system boundary

- **Canonical engine:** `MAIN`
- **Non-canonical / experimental areas:** `EXP` research-side lanes only
- **Live-authoritative areas:** canonical MAIN execution/runtime/control truth only
- **Mounted but not always fully wired areas:** some operator surfaces may be mounted before their final authoritative backend is complete; treat them with suspicion until proven
- **Areas that must remain fail-closed:** daemon/operator truth surfaces, restart/control semantics, suppressions/summary/config-diff style truth, any area where unavailable truth could be mistaken for authoritative empty truth

## Major domains

### 1. Core Rust platform
- **Purpose:** canonical runtime, daemon, DB, execution, backtest, GUI support surfaces
- **Owning paths:** `core-rs/crates/*`
- **Operator relevance:** high
- **Truth sources:** code, DB-backed behavior, scenario proof, readiness docs

### 2. Research Python layer
- **Purpose:** research workflows, experiments, artifacts, supporting evaluation
- **Owning path:** `research-py/`
- **Operator relevance:** medium for research, low for canonical operator truth
- **Truth sources:** code, experiment manifests, research-side tests; not canonical ops truth

### 3. Readiness authority
- **Purpose:** define readiness standard and scoring
- **Owning docs:** `docs/INSTITUTIONAL_READINESS_LOCK.md`, `docs/INSTITUTIONAL_SCORECARD.md`
- **Operator relevance:** very high for readiness judgments
- **Truth sources:** these docs plus committed-state proof

### 4. Patch planning / workflow
- **Purpose:** organize closure work and AI/operator process
- **Owning docs:** remaining-work patch plan, operator ledger, AI workflow pack
- **Operator relevance:** high for execution sequencing
- **Truth sources:** current operator-maintained docs only

## High-level architecture

- Rust core provides canonical daemon/runtime/db/execution/backtest surfaces.
- Python research layer provides non-canonical research workflows and EXP-side experimentation.
- DB-backed truth is preferred where readiness rules require it.
- Mounted surfaces must not imply authoritative truth unless backed by the right source.
- Canonical proof matters more than optimistic implementation claims.
- EXP may share foundations but must not widen MAIN operational truth.

## Non-negotiable invariants

- no fabricated truth
- no optimistic defaults on operator surfaces
- DB-backed truth where required by readiness rules
- deterministic behavior where expected
- explicit truth-state distinction between unavailable, empty, and present
- fail closed when authority is unavailable
- MAIN and EXP must remain distinct in operational meaning

## Current project posture snapshot

### Readiness
- Full committed-state proof transcript exists and is valuable.
- Readiness is still not automatically “closed forever” if a source-level truth concern remains disputed.

### Completion
- Infrastructure is strong.
- Platform completion is still partial.

### Trading viability
- Alpha, economics, and business realism are still open questions.

### Live ops
- Autonomous intraday paper trading (Paper + Alpaca session controller) is the canonical MAIN path, proven for unattended intraday operation (AUTON-01, AC-01; see `autonomous_paper_ops.md`).
- Unattended live-capital use is not yet proven or claimed.

### Maintainability
- Several sink files remain too large and need staged decomposition later.

## Current MAIN remaining-work list

_Prior reconciliation snapshot: 2026-04-12, commit a0b017d4 (17/17 lanes passed). Current HEAD: 6bd208d (2026-04-14). No updated full proof run is documented against current HEAD; the prior snapshot remains the latest documented harness result._
_Verification basis: scenario test files, production source modules, runbook files present in HEAD, and confirmed proof harness result. Scenario-file presence is necessary but not sufficient — the harness result is required for closure._

### Closure taxonomy key

| Label | Meaning |
|---|---|
| **HARNESS-BACKED** | Code + scenario test(s) committed; documented full harness pass explicitly covers the patch |
| **SCENARIO-PRESENT** | Scenario file in committed HEAD; last documented harness pass (a0b017d4, 2026-04-12) covers it; not re-proven on any commit after that snapshot |
| **POST-SNAPSHOT** | Code or doc committed after the last documented harness run (a0b017d4); harness status against current HEAD is not yet re-documented |
| **DOC/WORKFLOW-ONLY** | Documentation or workflow-only change; no runtime proof required or applicable |

### Open — genuinely remaining

| Patch | Description |
|---|---|
| **RUNTIME-LONGRUN-01** | long-run runtime durability |
| **CTRL-AUTH-01** | operator auth controls |
| **DATA-INTEGRITY-01** | data integrity enforcement |
| **EXEC-PROTECT-01** | execution protection controls |
| **CORP-ACT-01** | corporate actions handling |

**Conditional only (not blocking main lane):**
- **RESEARCH-NON-EQ-01** — open only if non-equity research surfaces are mounted as canonical truth

**Non-blocking follow-up only:**
- **MT-07** — decomposition/refactor; does not block any MAIN lane

---

### Closed — committed-state code/test proof verified

| Patch | Verification evidence |
|---|---|
| **MT-01** | `routes/execution_order_analysis.rs` extracted A5/outbox handlers (1122 lines) from `execution.rs`; `routes/system_artifact.rs` extracted artifact/parity/topology handlers (380 lines) from `system.rs`; `mod` declarations + router imports updated in `routes.rs`; `cargo check -p mqk-daemon` + clippy zero warnings; 96 lib tests + 72 scenario_daemon_routes + 23 contract gate tests all pass |
| **MT-02** | `orchestrator/dispatch.rs` extracted Phase-1 dispatch helpers (`dispatch_submit_claimed_outbox_row`, `dispatch_cancel_claimed_outbox_row`) from `orchestrator.rs`; `mod dispatch;` registered; unused imports removed; `cargo check --workspace` + clippy zero warnings |
| **MT-03** | `mqk-db/src/inbox.rs` extracted from `orders.rs`; `lib.rs` re-exports `inbox::*`; GUI route layer already modular (14 `routes/` modules in HEAD); `cargo check --workspace` clean |
| **IR-01** | `scenario_operator_audit_ir01.rs` (3 pure in-process tests P1–P3); DB-backed proof (`ir01_control_arm_no_run_no_synthetic_run_created`, `ir01_control_disarm_no_run_no_synthetic_run_created`, `ir01_control_arm_with_real_run_writes_audit_event`) in `scenario_daemon_runtime_lifecycle.rs` |
| **IR-02** | `scenario_operator_audit_ir02.rs` |
| **CC-01** | `StrategySummaryResponse` in `api_types.rs`; daemon strategy summary route |
| **CC-02** | `sys_strategy_suppressions` migration; fleet enable/disable scenario |
| **CC-03** | `ModeChangeGuidanceResponse` in `api_types.rs`; `scenario_mode_transition_cc03a.rs`, `scenario_restart_intent_cc03b.rs`, `scenario_restart_workflow_cc03c.rs` |
| **CC-04** | `OmsOverviewResponse` in `api_types.rs`; `scenario_oms_overview_cc04.rs` |
| **CC-05** | `MetricsDashboardResponse` in `api_types.rs`; `scenario_metrics_dashboards_cc05.rs` |
| **TV-01** | `contracts.py` (`PromotedArtifactManifest`); `signal_pack/promote.py`; `test_artifact_contract.py` |
| **TV-02** | `research-py/src/mqk_research/deployment/gate.py`; `test_deployability_gate.py` |
| **TV-03** | `research-py/src/mqk_research/deployment/parity.py`; `test_parity_evidence.py` |
| **TV-04** | `capital_policy` module in daemon; `scenario_capital_policy_tv04*.rs` (five scenario files) |
| **LO-01** | `docs/runbooks/operator_workflows.md` (full runbook, 9+ sections) |
| **LO-02** | `docs/runbooks/stressed_recovery_proof_matrix.md`; `scenario_stressed_recovery_lo02.rs` |
| **LO-03** | `docs/runbooks/live_shadow_operational_proof.md`; `scenario_live_shadow_preflight_lo03.rs` |
| **DOC-01** | AI workflow docs Batch 1A partial reconciliation 2026-04-11; full doc-truth reconciliation completed by DOC-TRUTH-FINAL-01 |
| **DOC-TRUTH-FINAL-01** | Four ai_workflow + readiness docs reconciled to committed proof state a0b017d4; stale commit refs, false closure claims, and remaining-work truth corrected (2026-04-12) |

### Additionally closed — scenario-file proof in committed HEAD (2026-04-12)

_(Patches not in the initial audit table above; confirmed by scenario-file presence in committed repo AND full proof harness pass (commit a0b017d4, all 17 lanes passed). Scenario-file presence alone is not closure proof — the harness result is required. Not exhaustive — additional infrastructure patches predating this tracking format exist in scenario files.)_

| Patch group | Verification evidence |
|---|---|
| **CC-01B** (strategy summary registry) | `scenario_strategy_summary_registry.rs` |
| **CC-02D** (fleet enable/disable interaction) | `scenario_fleet_enable_disable_interaction_cc02d.rs` |
| **CC-06** (alerts / events routes) | `scenario_alerts_events_cc06.rs` |
| **TV-01A–D** (artifact integrity chain extensions) | `scenario_artifact_schema_lock_tv01a.rs`; `scenario_artifact_intake_tv01b.rs`; `scenario_artifact_provenance_tv01cd.rs` |
| **TV-02A/B/C** (runtime deployability gate) | `scenario_artifact_deployability_tv02.rs` |
| **TV-03** (daemon-side parity proof) | `scenario_parity_evidence_tv03.rs` |
| **TV-03C** (artifact-id cross-validation) | `scenario_parity_evidence_tv03c.rs` |
| **TV-04C–F** (capital policy extensions) | `scenario_capital_policy_tv04c.rs`; `scenario_capital_policy_tv04d.rs`; `scenario_capital_policy_tv04e.rs`; `scenario_capital_policy_tv04f.rs` |
| **TV-EXEC-01 / TV-EXEC-01B** (fill quality telemetry) | `scenario_fill_quality_tv_exec01.rs`; `scenario_fill_quality_orchestrator_tv_exec01b.rs` |
| **A3/A4** (operator surfaces) | `scenario_a3_a4_operator_surfaces.rs` |
| **A5A–A5E** (execution/OMS surface series) | `scenario_order_timeline_a5a.rs`; `scenario_order_trace_a5b.rs`; `scenario_order_replay_a5c.rs`; `scenario_order_chart_a5d.rs`; `scenario_order_causality_a5e.rs` |
| **B1A–B1C** (native strategy bootstrap/bridge) | `scenario_native_strategy_bootstrap_daemon_b1a.rs`; `scenario_native_strategy_loop_dispatch_b1b.rs`; `scenario_native_strategy_bridge_b1c.rs` |
| **B2A–B3** (strategy registry / fleet control) | `scenario_native_strategy_registry_b2a.rs`; `scenario_strategy_fleet_control_truth_b2b_b3.rs` |
| **B4–B6** (protection / short guard / budget gate) | `scenario_protection_status_b4.rs`; `scenario_native_strategy_b5_short_guard.rs`; `scenario_native_strategy_b6_budget_gate.rs` |
| **B7–B8** (corp-actions / asset-class scope) | `scenario_corp_actions_b7.rs`; `scenario_asset_class_scope_b8.rs` |
| **LO-02B–E** (stressed recovery extensions) | `scenario_shadow_recovery_lo02b.rs`; `scenario_continuity_restart_coherence_lo02c.rs`; `scenario_restart_quarantine_lo02d.rs`; `scenario_kill_switch_persistence_lo02e.rs` |
| **LO-03A–G** (live shadow operator extensions) | `scenario_live_shadow_operator_lo03ab.rs`; `scenario_live_shadow_restart_lo03c.rs`; `scenario_live_capital_lo03de.rs`; `scenario_shadow_to_live_cutover_lo03f.rs`; `scenario_audit_ops_lo03g.rs` |
| **BRK-00R-04/05, BRK-07R, BRK-08R, BRK-00R-06** (WS transport/cursor series) | `scenario_ws_continuity_gate_brk00r04.rs`; `scenario_alpaca_paper_ws_transport_brk00r05.rs`; `scenario_ws_cursor_durability_brk07r.rs`; `scenario_ws_gap_recovery_brk08r.rs`; `scenario_paper_alpaca_proof_bundle_brk00r06.rs` |
| **BRK-09R** (reconcile start gate) | `scenario_reconcile_start_gate_brk09r.rs` |
| **C1–C4, LT-01** (live trust series) | `scenario_live_trust_c1.rs`; `scenario_preflight_live_trust_c2.rs`; `scenario_mode_change_guidance_c3.rs`; `scenario_session_live_trust_c4.rs`; `scenario_live_trust_chain_lt01.rs` |
| **RTS-07 / RSK-07** (strategy-to-intent contract) | `scenario_combined_paper_gate_rts07_rsk07.rs` |
| **PTA-01/02** (canonical paper path) | `scenario_canonical_paper_path_pta01.rs`; `scenario_paper_survivability_pta02.rs` |
| **OPS-08/09, EXEC-06, JOUR-01** (paper operations) | `scenario_paper_supervision_ops08_exec06.rs`; `scenario_paper_journal_jour01_ops09.rs` |
| **OPS-NOTIFY-01, DIS-01/02** (notification proofs) | `scenario_notify_ops01.rs`; `scenario_discord_dis01_dis02.rs` |
| **AC-01, AUTON-01, AUTON-TRUTH-01, AH-01** (autonomous paper series) | `scenario_autonomous_calendar_ac01.rs`; `scenario_autonomous_paper_day_auton01.rs`; `scenario_autonomous_readiness_auton_truth01.rs`; `scenario_auton_hist_durability_ah01.rs` |
| **OC-01/02** (ops control) | `scenario_ops_control_oc01_oc02.rs` |
| **RT-01** (route contract) | `scenario_route_contract_rt01.rs` |
| **EXEC-02** (replace/cancel lineage) | `scenario_replace_cancel_chains_exec02.rs` (7 tests E02-P01–P05+D01–D02); `0035_oms_order_lifecycle_events.sql`; `mqk-db/src/order_lifecycle.rs`; `orchestrator/lifecycle_events.rs`; Phase 3b hook |

### Closed — doc/workflow only (no runtime proof required)

_(No scenario file expected or applicable. Status is HARNESS-BACKED only in the sense that it is committed to HEAD; the doc change itself is the full deliverable.)_

| Patch | Label | Note |
|---|---|---|
| **AUTON-10** | DOC/WORKFLOW-ONLY · POST-SNAPSHOT | `MASTER_COMMAND_BRIEF.md` "Live ops" posture corrected: autonomous intraday paper path classified as canonical MAIN; unattended live-capital caution preserved. Committed 2026-04-14, after last documented harness run. No runtime proof required. |

## Default instructions for any serious AI task

- Start narrow.
- Identify the active audit axis or patch objective.
- Use subsystem brief + patch packet + minimal file bundle.
- Keep MAIN and EXP separate.
- Do not claim closure beyond the evidence.
