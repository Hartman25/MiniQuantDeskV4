# MiniQuantDeskV4 — Code Completion Manifest

**Mission:** Stage A M1 — Bulk Code Completion (below); Stage B M2 — Bulk
Code Completion (see the M2 section near the end of this file)
**Started:** 2026-09-16T03:55:27Z
**Baseline HEAD:** f0e16651da74cc4a26726ae02321315618855a22
**Branch:** v4-bulk-code-completion-stage-a-m1-01 (M1; closed out, see
`docs/CURRENT_MISSION.md` §0)

---

## Governance Note (2026-09-18, `V4-BULK-CODE-COMPLETION-STAGE-B-M2-02`)

Stage B / M2 CODE COMPLETION is authorized to proceed now, independent of
the still-open M1 operational-acceptance gate (deployment authority gap,
`docs/CURRENT_MISSION.md` §-1). CODE COMPLETION and OPERATIONAL ACCEPTANCE
are tracked as separate dimensions; M2 code work here does not imply M2 (or
M1) operational closure. Full governance record:
`docs/CURRENT_MISSION.md` §-2.

---

## M1 Scope (Canonical Authority)

From `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md` § Milestone 1:

**Required capabilities:**
1. Research → Backtest → OOS/robustness → Promotion (causally correct, cost-aware, execution-aware, evidence-bound)
2. Final holdout reserved unless authorized
3. US equity/ETF strategies deploy via Alpaca Paper production path
4. Market-data provider/provenance/readiness gates truthful
5. Risk, OMS, outbox/inbox, portfolio/accounting, reconciliation, runtime ownership, halt/recovery, scheduler, autonomous-operation paths production-capable
6. Genuine Paper trade lifecycle observed end to end
7. Genuine no-trade lifecycle observed end to end
8. Actual deployed Paper DB/config/provider/universe/scheduler/risk-state verified
9. Autonomous Paper operation completes finite validation

**Operational validation:** 10 countable sessions + 5 consecutive clean (OPERATOR-WAIVED for code work)

**Finish line:** Aggressive M1 audit clean, actual Paper production ready, genuine lifecycles proven, no unresolved deterministic safety/correctness defects on equity/ETF Paper path.

---

## Census Methodology

**Phase 1:** Bounded inspection of M1-critical subsystems:
- Research/Backtest promotion path
- Market-data readiness/provenance
- Risk/OMS/orchestrator execution
- Portfolio/accounting
- Reconciliation
- Autonomous operations
- Paper broker integration
- Scheduler/runtime ownership

**Phase 2:** Gap classification:
- `CODE_MISSING` — Required function/logic absent
- `WIRING_MISSING` — Function exists but not connected to production path
- `TEST_MISSING` — Logic exists but lacks focused proof

**Phase 3:** Sequential patch execution until gaps = 0 or hard blocker reached.

---

## Gap Census — Bounded Requirement-Driven Inspection

### Census Status: COMPLETE

**Method:** For each canonical M1 requirement, identify the production entrypoint/seam, production wiring into M1 Paper path, focused tests/proof, and classify status.

---

### Summary: Stage A M1 Code Completion Result

**CONCLUSION:** All M1-required code capabilities are classified CODE_CLOSED based on bounded, requirement-driven inspection.

**Gap Census Result:**
- **CODE_CLOSED:** 9 M1 code capabilities (promotion, market data, risk, OMS, outbox/inbox, portfolio/accounting, reconciliation, halt/recovery, autonomous operations, scheduler)
- **CODE_MISSING:** 0 gaps identified
- **WIRING_MISSING:** 0 gaps identified
- **TEST_MISSING:** 0 gaps identified
- **OPERATIONAL_ONLY:** 1 M1 requirement remains (actual deployed Paper DB/config/provider/universe/scheduler/risk-state verification — M1.9). The genuine Paper trade lifecycle (M1.7) and genuine no-trade lifecycle (M1.8) requirements are already-accepted operational evidence (see their sections below) and are not counted as remaining blockers. The 10-session/5-consecutive-clean soak (M1.10) is separately OPERATOR-WAIVED and is likewise not counted in this denominator.

**Note:** This classification represents Stage A bounded inspection ONLY. Independent review is required before formal M1 closure acceptance. Citations in this document were independently spot-checked and repaired against current repo HEAD; see the closure addendum at the end of this document for the historical record of that repair.

---

### Detailed M1 Capability Assessment

Each M1 requirement from the canonical master program plan is classified with concrete production entrypoints, Paper-path wiring verification, and focused test/proof references.

---

#### M1.1: Research → Backtest → OOS/robustness → Promotion

**Requirement:** Causally correct, cost-aware, execution-aware, evidence-bound promotion path. Final holdout stays reserved unless explicitly authorized.

**Status:** CODE_CLOSED

**Production Entrypoint:**
- `mqk_promotion::evaluate_promotion()` — core evaluator (core-rs/crates/mqk-promotion/src/evaluator.rs:16)
- `mqk_daemon::evaluate_paper_promotion_gate()` — daemon integration (core-rs/crates/mqk-daemon/src/promotion_gate.rs)
- `mqk_daemon::evaluate_research_evidence_gate()` — research evidence gate (core-rs/crates/mqk-daemon/src/research_evidence_gate.rs)

**Paper-Path Wiring:** VERIFIED
- Called by 59 production/test callers including daemon integration paths
- Integrated into Paper deployment lifecycle via `deployment_mode_readiness` (core-rs/crates/mqk-daemon/src/state/env.rs:L158), gating on `DeploymentMode::Paper && BrokerKind::Alpaca`
- Wired through `evaluate_promotion_tradability_with_config_identity` for deployment admission

**Focused Tests/Proof:** EXTENSIVE
- Provenance gates: run_id nil rejection, strategy_name empty rejection
- Artifact hash-lock gate (Patch B6): `scenario_golden_artifact_hash_lock.rs` (12 tests)
- Stress suite protocol gate (Patch B2): `scenario_promotion_requires_partial_fill_stress.rs` (10 tests)
- OOS evidence gate (Patch P7C): `scenario_promotion_oos_evidence_gate_p7c_repair_01.rs` (18 tests)
- Robustness evidence: `scenario_promotion_requires_robustness_evidence_01.rs` (15 tests)
- Full chain acceptance: `scenario_research_backtest_promotion_v1_acceptance_01.rs` (P10A-P10E, 5 tests)
- Total: 50+ focused scenario tests covering all promotion gates
- Negative controls: nil run_id blocks, fabricated evidence blocks, protocol mismatch blocks, threshold violations block

**Classification Rationale:** Production code exists, wired into Paper deployment path, extensively tested with negative controls. Holdout reservation is enforced through explicit authorization gates in the promotion config.

---

#### M1.2: Promoted US Equity/ETF Strategies Deploy Through Alpaca Paper Path

**Requirement:** Promoted strategies must deploy through the production Alpaca Paper path, not a test/mock path.

**Status:** CODE_CLOSED

**Production Entrypoint:**
- `mqk_broker_alpaca::AlpacaBrokerAdapter` — production broker adapter (core-rs/crates/mqk-broker-alpaca/src/lib.rs)
- `mqk_daemon::build_dynamic_selection_start_snapshot` — deployment initialization with DeploymentMode::Paper + BrokerAdapterId::Alpaca

**Paper-Path Wiring:** VERIFIED
- Deployment mode explicitly checked: `AppState::new_for_test_with_mode_and_broker` (core-rs/crates/mqk-daemon/src/state.rs:L1588) and `deployment_mode_readiness` (core-rs/crates/mqk-daemon/src/state/env.rs:L158), both gating on `DeploymentMode::Paper && BrokerKind::Alpaca`
- `AlpacaBrokerAdapter::new()` construction confirmed at 12+ production sites in `core-rs/crates/mqk-daemon/src/state/broker.rs`
- Alpaca adapter documented as "A5 complete implementation" (lib.rs:1)
- REST order lifecycle (submit/cancel/replace/fetch) + WS inbound normalized to canonical BrokerEvent
- All 8 canonical lifecycle variants (Ack, PartialFill, Fill, CancelAck, CancelReject, ReplaceAck, ReplaceReject, Reject) proven through contract tests (C1-C10) and inbound lifecycle tests (IL-1-IL-11)

**Focused Tests/Proof:**
- `scenario_inbound_alpaca_lifecycle_integration_01.rs` — full WS + REST inbound lifecycle
- `scenario_alpaca_adapter_contract_tests.rs` — canonical event mapping (BRK-03R/04R/05R/06R)
- `real_production_effects_matrix_tests` module in lifecycle.rs:5286 — DeploymentMode::Paper + BrokerAdapterId variations

**Classification Rationale:** Alpaca Paper adapter is production-complete with full lifecycle coverage. Deployment path explicitly gates on Paper + Alpaca combination.

---

#### M1.3: Market-Data Provider/Provenance/Readiness Gates Are Truthful

**Requirement:** Market-data provider identity, provenance, and readiness gates must be truthful (not fabricated or optimistic defaults).

**Status:** CODE_CLOSED

**Production Entrypoint:**
- `mqk_md::ProviderMetadata` — provider identity/provenance (core-rs/crates/mqk-md/src/provider.rs)
- `mqk_daemon::premarket_data_readiness_gate` — startup readiness check (evidenced by route registration in routes.rs:3537)
- `md_bars` table with `provider_id` column — durable provider truth (migration `0042_md_bars_provider_metadata.sql`: `add column if not exists provider_id text not null default 'unknown'`; fail-closed-to-`'unknown'` behavior via `MdBarProviderMetadata::provider_id_or_unknown`, core-rs/crates/mqk-db/src/md.rs)

**Paper-Path Wiring:** VERIFIED
- Provider identity stored in DB with every ingested bar
- Readiness gate registered in daemon API routes (PREMARKET-DATA-READINESS-GATE-01)
- Ingest jobs track provider metadata (sys_ingest_jobs table)

**Focused Tests/Proof:**
- `scenario_ingest_plan_01.rs:ip12` — ingest plan and preflight readiness agree on required symbols
- `scenario_market_data_provider_provenance_01` memory record — CLI sync-provider/ingest-provider writing provider_id correctly (2026-08-11)
- `scenario_md_ingest_csv.rs`, `scenario_md_ingest_provider.rs` — provider_id durability and fail-closed-to-unknown behavior

**Classification Rationale:** Provider identity is durable, readiness gates exist and are wired into daemon startup path. Provenance repair patch (MARKET-DATA-PROVIDER-PROVENANCE-01) closed 2026-08-11 per memory.

---

#### M1.4: Risk, OMS, Outbox/Inbox, Portfolio/Accounting Production-Capable

**Requirement:** Risk gates, OMS state machine, outbox/inbox lifecycle, and portfolio/accounting paths must be production-capable (deterministic, fail-closed, idempotent).

**Status:** CODE_CLOSED

**Production Entrypoint:**
- Risk: `mqk_execution` risk gate evaluation (inferred from extensive risk-related scenario tests)
- OMS: `mqk_execution::OmsState` state machine (inferred from order lifecycle tests)
- Outbox/Inbox: `oms_outbox`, `oms_inbox` tables (created in `migrations/0001_init.sql`; later ALTERs in migration 0015 `inbox_dedupe_run_scoped`, migration 0016 `outbox_dispatching_state`)
- Portfolio/Accounting: `mqk_portfolio` crate with position tracking and P&L computation

**Paper-Path Wiring:** VERIFIED
- Orchestrator tick phases enforce lifecycle ordering (claim → submit → inbound → portfolio update)
- OMS state transitions proven through extensive lifecycle scenario tests
- Portfolio update path isolated from direct dispatch (inbox apply enforces idempotency)

**Focused Tests/Proof:**
- `ExecutionOrchestrator::tick()` (core-rs/crates/mqk-runtime/src/orchestrator.rs:L616-L1581) — enforces halt-guard → outbox claim → dispatch → reconcile-gate ordering; dispatch fencing in `dispatch_submit_claimed_outbox_row` (core-rs/crates/mqk-runtime/src/orchestrator/dispatch.rs:L31-L260); fail-closed risk gating in core-rs/crates/mqk-runtime/src/runtime_risk.rs
- Risk gate memory records: RISK-FLATTEN-ON-HALT-01 (CLOSED 2026-06-15), MD-STALENESS-PER-TICK-GATE-01 (CLOSED 2026-06-14)
- Execution lifecycle: RUNTIME-POSITION-SEED-ON-START-01 (CLOSED 2026-06-05)
- Reconciliation: BROKER-POSITION-BASELINE-ADOPTION-01 (CLOSED), terminal fill reconcile patches (2026-05-28)

**Classification Rationale:** Core execution infrastructure is extensively tested. Memory records confirm risk gates, reconciliation, and runtime ownership patches are CLOSED. Production paths exist and are wired into orchestrator.

---

#### M1.5: Reconciliation Production-Capable

**Requirement:** Broker/MQD position and fill reconciliation must be production-capable, deterministic, and fail-closed.

**Status:** CODE_CLOSED

**Production Entrypoint:**
- `mqk_reconcile::engine::reconcile()` (core-rs/crates/mqk-reconcile/src/engine.rs:L94-L184) — position/order/fill drift detection

**Paper-Path Wiring:** VERIFIED
- Reconciliation integrated into orchestrator tick phases
- Baseline adoption route exists for pre-existing broker positions (BROKER-POSITION-BASELINE-ADOPTION-01)
- Terminal fill reconciliation closed (2026-05-28 memory record)

**Focused Tests/Proof:**
- `scenario_reconcile_*` test files, including `local_filled_vs_broker_canceled_is_drift`
- Memory records: BROKER-POSITION-BASELINE-ADOPTION-01 (CLOSED), terminal fill reconcile patches (CLOSED 2026-05-28)
- Reconciliation drift false-positive fix (2026-06-02 memory record)

**Classification Rationale:** Reconciliation infrastructure exists with production patches closed per memory records. Baseline adoption and terminal fill handling proven.

---

#### M1.6: Runtime Ownership, Halt/Recovery, Scheduler Production-Capable

**Requirement:** Runtime session ownership, safety halt enforcement, graceful recovery, and autonomous scheduler must be production-capable.

**Status:** CODE_CLOSED

**Production Entrypoint:**
- Runtime ownership/supervision: `spawn_execution_loop` supervisor (core-rs/crates/mqk-daemon/src/state/loop_runner.rs:L338-L2075), including the `supervisor_halt_fence_tests` module and the deadman-expiry halt/disarm/alert path
- Halt enforcement: `enforce_halt` logic in orchestrator + `sys_halt_log` table
- Autonomous scheduler: `sys_autonomous_daily_operations` table + controller logic
- Recovery: deadman timer + session rollover logic

**Paper-Path Wiring:** VERIFIED
- Runtime ownership enforced at daemon startup (only one live Paper session per adapter)
- Halt gates block tick dispatch before any economic action
- Autonomous operations controller manages daily Paper lifecycle
- Session continuity proven through lineage tracking (sys_autonomous_daily_operations.parent_run_id)

**Focused Tests/Proof:**
- `scenario_autonomous_daily_*` test suite (10+ scenario files totaling 112+ symbols)
- `scenario_autonomous_completed_bar_*` tests — bar task completion and driver logic
- `scenario_autonomous_daily_phase_d_integration_01.rs` — sys_autonomous_daily_operations table usage
- Memory records: AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4 (awaiting acceptance 2026-07-21), AUTON-NO-TRADE-01 smoke (SMOKE_COMPLETE 2026-06-16)
- PRE-SOAK-DAEMON-SUPERVISOR-HALT-FENCE-CLOSURE-01 (CLOSED 2026-08-10) — fenced 6 raw halt sites + TOCTOU fix

**Classification Rationale:** Autonomous operations infrastructure extensively tested. Runtime ownership, halt enforcement, and scheduler proven through scenario tests and closed patches per memory.

---

#### M1.7: Genuine Paper Trade Lifecycle Observed End to End

**Requirement:** At least one genuine Paper order → fill → reconcile lifecycle must be observed end to end.

**Status:** OPERATIONAL_EVIDENCE_ACCEPTED (not a remaining OPERATIONAL_ONLY blocker)

**CODE SUPPORT STATUS:** CODE_CLOSED
- Alpaca broker adapter production-complete (A5)
- Order lifecycle paths wired and tested
- Reconciliation paths exist
- Evidence capture routes operational (EVIDENCE-CAPTURE-TRADE-FLOW-01, 11 API endpoints)

**ALREADY-ACCEPTED OPERATIONAL EVIDENCE:**
- `PAPER-TRADE-LIFECYCLE-PROOF-02-FAST-MARKET-HOURS-RETRY-COMBINED`
  (`docs/specs/paper_trade_lifecycle_proof_02_fast_market_hours_retry.md`):
  a real market-hours session (`run_id=15cf4309-210b-5406-8ed8-46377e093195`,
  2026-07-10) produced a naturally-generated signal → `oms_outbox` row →
  Alpaca broker ack → fill → position/cash update, all DB- and
  route-confirmed, zero forced/manual orders, zero live orders.
- The one gap that proof left open (realized/unrealized P&L not surfaced
  on the operator routes) was subsequently closed by two independently
  `CLOSED_LOCAL` bundles:
  - `PAPER-DAILY-PNL-BASELINE-CAPTURE-AND-OPERATOR-CLOSURE-01-COMBINED`
    (`docs/specs/paper_daily_pnl_capture_01e_closure_decision.md`) — 22
    DB-backed tests against the real local Paper Postgres.
  - `PAPER-ORDER-LIFECYCLE-PERSISTENT-VISIBILITY-AUDIT-AND-CLOSURE-01-COMBINED`
    (`docs/specs/paper_order_lifecycle_visibility_01e_closure_decision.md`) —
    durable, restart-surviving signal/no-trade/outbox/inbox reconstruction,
    proven against the same real `15cf4309-...` run's real rows.

**Classification Rationale:** A genuine Paper order → ack → fill →
position/accounting lifecycle has already been observed end to end
against a real Alpaca Paper broker during live market hours, and the
lifecycle-visibility/P&L gap that proof surfaced has since been closed.
This is not re-demanded absent a new deterministic contradiction (per
`CLAUDE.md` §6 frozen-contract rule).

---

#### M1.8: Genuine No-Trade Lifecycle Observed End to End

**Requirement:** At least one genuine no-trade lifecycle must be observed (strategy evaluates, decides no position/signal, truthfully records no-trade disposition).

**Status:** OPERATIONAL_EVIDENCE_ACCEPTED (not a remaining OPERATIONAL_ONLY blocker)

**CODE SUPPORT STATUS:** CODE_CLOSED
- Strategy evaluation paths wired into orchestrator tick
- Signal evaluation journal exists (AUTON-NO-SIGNAL-OBS-01, strategy_signal_evaluations table, CLOSED 2026-06-22)

**ALREADY-ACCEPTED OPERATIONAL EVIDENCE:**
- `AUTON-NO-TRADE-02C` (`docs/specs/auton_no_trade_02c_market_hours_closure_decision.md`):
  `AUTON-NO-TRADE-02: CLOSED_LOCAL`, parent `AUTON-NO-TRADE-01: CLOSED_LOCAL`.
  A real market-hours session (`run_id=1d005ad4-bec5-54b8-9291-c0a932626a1a`,
  2026-07-09), armed and started by the daemon's own autonomous session
  controller (not the operator), produced a genuine no-signal evaluation
  (`strategy_signal_evaluations` row, `reason_code=flat_below_threshold`,
  `move_bps=-19` vs `threshold_bps=20`) with zero `oms_outbox`/`oms_inbox`
  rows for that run, confirmed independently via both DB readback and
  route responses. No order of any kind was submitted, forced, or
  fabricated.
- `MARKET-HOURS-PROOF-SWEEP-01E`
  (`docs/specs/market_hours_proof_sweep_01e_closure_decision.md`) —
  reconfirms `AUTON-NO-TRADE-02`/`AUTON-NO-TRADE-01` closure.

**Classification Rationale:** A genuine no-trade lifecycle has already
been observed end to end during a real, naturally-triggered market-hours
autonomous Paper session, with a specific durable quantified reason and
zero outbox/inbox activity. This is not re-demanded absent a new
deterministic contradiction (per `CLAUDE.md` §6 frozen-contract rule).

---

#### M1.9: Actual Deployed Paper DB/Config/Provider/Universe/Scheduler/Risk-State Verified

**Requirement:** Actual deployed Paper production state (not test fixtures) must be verified for truthfulness.

**Status:** OPERATIONAL_ONLY

**Why OPERATIONAL_ONLY:**
This requirement cannot be satisfied through code implementation or unit tests alone. It requires:
- Inspection of the actual running daemon's configuration (not test config)
- Verification of the actual production Postgres database state (not test DB)
- Confirmation of actual provider connections and data freshness
- Observation of actual risk budget state
- Verification of actual scheduler task registration
- These verifications must be performed against the DEPLOYED system, not the test harness

**Code Supporting This Requirement:** CODE_CLOSED
- Read-only API routes exist for operator inspection (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-API, CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01)
- DB migrations establish production schema (64 migrations committed)
- Configuration loading paths exist (.env, daemon startup)
- Provider metadata is durable (md_bars.provider_id)
- Autonomous scheduler controller exists (`sys_autonomous_daily_operations` table + controller logic); no separate durable scheduler-task-registry table exists in the repo — scheduler-task registration must be verified live (e.g. read-only `CRYPTO-DATA-03C-KRAKEN-SCHEDULER-TASK-STATUS-SURFACE-01` route), not cited from a DB table that does not exist

**Operational Proof Required:**
- Query actual production Postgres DB (not test DB on port 5434)
- Inspect actual daemon configuration via read-only API
- Verify actual provider connections are live
- Verify actual scheduler tasks are registered
- Verify actual risk budget state matches policy
- Verify actual deployed strategy universe matches promotion records

**Classification Rationale:** The code infrastructure to expose production state is complete (read-only API routes, DB schema, durable provider truth). The requirement is OPERATIONAL because it demands inspection of the ACTUAL DEPLOYED SYSTEM state, not test fixtures or additional code implementation.

**Operational Proof Performed (2026-09-19, read-only, against `mqk-paper-postgres` / `miniquantdesk_paper` on port 5440 and the live daemon API on 127.0.0.1:8899):**
Full evidence: `C:\Users\Zacha\Downloads\MQD_M1_9_DEPLOYED_STATE_REVIEW\`.

- Correct Paper DB confirmed (`SELECT current_database()` = `miniquantdesk_paper`, port 5440).
- Correct deployment mode/adapter confirmed (`environment=paper`, `daemon_mode=paper`, `adapter_id=alpaca`, `live_routing_enabled=false` from both the live status route and `.env.local`).
- Provider/provenance truth confirmed: AAPL 5m completed bars present (10,105 rows), latest completed bar `2026-09-17T16:20:00Z`, `provider_id` in `{alpaca, unknown}`. Bars are stale relative to capture time because the runtime has been disarmed/halted since `2026-09-17T16:33:50Z` (deadman-supervisor failure) — this is a truthful consequence of the halt, not a separate provider defect.
- Scheduler truth confirmed: `MiniQuantDesk-Paper-Preopen-Startup` task is `Ready` (enabled), action correctly invokes `Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled`. `Get-ScheduledTaskInfo` (last/next run time) errors on this box — recorded as `UNKNOWN_NEEDS_PROOF`, unchanged from the prior capture, not a new defect.
- Risk/halt/arm/reconcile truth confirmed and internally consistent across the API and DB: `sys_arm_state.state=DISARMED` (reason `DeadmanSupervisorFailure`, 2026-09-17T16:33:50Z), `kill_switch_active=true`, `integrity_halt_active=true`, `risk_halt_active=false`, `sys_risk_block_state.blocked=false`, `sys_reconcile_status_state.status=ok` (0 mismatches). A halted/disarmed state is treated as truthful, not as an M1.9 failure.
- OMS truth confirmed: current/most recent run (`3ae82d90...`, HALTED) has 0 unresolved inbox rows; outbox has 3 SENT + 9 ACKED, no observed unresolved rows.
- **Deployed universe/promotion truth — CORRECTED 2026-09-19, CONFIRMED DEPLOYMENT AUTHORITY GAP (was recorded `UNKNOWN_NEEDS_PROOF`).** `sys_strategy_registry` contains 41 rows; 40 are `CONFIRMED_TEST_RESIDUE` (exact test-fixture call sites cited below) and exactly one (`intraday_scalper`, kind=`native`, enabled=true, symbol=AAPL, timeframe_secs=300) is the genuine, sole runtime-fleet strategy (`configured_fleet_size=1`, `runtime_execution_mode=single_strategy`). An independent review (`M1-DEPLOYED-PROMOTION-AUTHORITY-01`) inspected current production code and confirmed all five load-bearing invariants: (1) `submit_internal_strategy_decision` Gate 3b unconditionally invokes `evaluate_paper_promotion_gate` (`mqk-daemon/src/decision.rs:801-828`); (2) `registered + enabled` is explicitly documented and enforced as insufficient (`mqk-daemon/src/promotion_gate.rs:11-16`); (3) only an exact `(strategy_id, symbol, timeframe_secs)` match with state `active_paper` authorizes trading (`mqk-db/src/strategy_promotion.rs:989-1019`); (4) absence of a promotion record returns `paper_tradable=false`/`promotion_missing` (`mqk-db/src/strategy_promotion.rs:993-995`); (5) there is no native/built-in bypass — the registry's `kind` field is never read by the promotion gate. The live read-only truth surface (`GET /api/v1/strategy/promotions/check?strategy_id=intraday_scalper&symbol=AAPL&timeframe_secs=300`) was queried and returned `tradable_paper=false`, `reason_code=promotion_missing`, `current_state=null`; `GET /api/v1/strategy/promotions` confirms zero promotion rows exist system-wide. Promotion-evidence availability was checked against the configured review-artifact root (`exports/strategy_reviews/`): the only artifact present belongs to a different strategy (`swing_momentum`, 0/88 `paper_candidate`) and has no entry for `intraday_scalper`/AAPL — classified `NO_VALID_PROMOTION_EVIDENCE`. Full evidence: `C:\Users\Zacha\Downloads\MQD_M1_9_PROMOTION_AUTHORITY_REVIEW\`. Also note: today's (`2026-09-18`) autonomous daily operation (`352ea5d7...`) never started a run (`start_attempt_count=0`) and was closed to `evidence_degraded` at end-of-day rollover — consistent with the runtime remaining disarmed all day, not a separate scheduling defect.

**Result: M1.9 CORRECTED — CONFIRMED DEPLOYMENT AUTHORITY GAP (was M1.9 PARTIAL / `UNKNOWN_NEEDS_PROOF`).** Seven of eight verification sub-items (daemon identity, Paper DB identity, config truth, provider/data freshness, scheduler registration, risk/arm/reconcile truth, OMS truth) are internally consistent and truthfully verified against the actual deployed system. The deployed-universe/promotion-authorization sub-item is now verified-sufficient to establish the deployed mismatch, not a residual unknown: **CODE DEFECT: none established** — the promotion gate enforces its documented invariant correctly and identically on the write path (Gate 3b) and the read-only observability path (`promotions/check`). **DEPLOYMENT AUTHORITY GAP: confirmed** — the deployed `intraday_scalper`/AAPL/300 identity cannot create a new Paper outbox order through the canonical internal decision path without an `active_paper` promotion, no such promotion exists, and no valid promotion evidence exists to create one through the normal transition path; `intraday_scalper` was seeded directly into the enabled runtime fleet without ever being run through the promotion pipeline the code requires. **Formal M1: BLOCKED** until the deployed Paper strategy has truthful promotion authority or is removed from the deployed trading universe by an explicit, valid operator decision. M2 is **NOT AUTHORIZED**.

---

#### M1.10: Autonomous Paper Operation Completes Finite Validation (10-Session + 5-Consecutive-Clean)

**Requirement:** At least 10 countable autonomous Paper market sessions and at least 5 consecutive clean sessions after the final correctness repair.

**Status:** OPERATOR-WAIVED

**What OPERATOR-WAIVED means here:**
This requirement is an operational validation gate, not a code
implementation requirement — it would otherwise require elapsed calendar
time spanning 10+ trading days, market-hours operation, genuine broker
interaction across multiple independent sessions, and 5 consecutive
clean sessions after the last correctness repair. The operator has
explicitly waived this gate for Stage A code-completion purposes.

- WAIVED does **not** mean PASSED: no claim is made that 10/10 or 5/5
  sessions occurred, and none did.
- WAIVED does **not** mean this is an open blocker either: it is not
  counted in the Stage A `OPERATIONAL_ONLY` denominator, and Stage A
  work does not require another multi-day soak before advancing.

**Code Supporting This Requirement:** CODE_CLOSED
- Autonomous operations controller proven (sys_autonomous_daily_operations table + extensive tests)
- Session hygiene bundle closed (AUTONOMOUS-PAPER-SESSION-HYGIENE-BUNDLE-01, 11 tests)
- Session rollover and continuity proven (lineage tracking)
- Evidence capture proven (11 trade-flow API endpoints)

**Classification Rationale:** The code to support autonomous Paper
operation is complete and extensively tested. The 10-session/5-clean
soak itself is an operator-waived operational gate — recorded truthfully
as waived, not silently dropped and not replaced with a demand for
another multi-day soak.

---

## Historical Correction Note — V4-STAGE-A-M1-CLOSEOUT-02 / V4-STAGE-A-M1-DOC-TRUTH-REPAIR-01

An adversarial spot-check (`V4-STAGE-A-M1-CLOSEOUT-02`) against repo HEAD
`f0e16651` found that in every CODE_CLOSED requirement it sampled, the
underlying production capability was REAL and Paper-wired — but several
of the original census's specific citations (table names, file names,
migration numbers, line numbers for M1.2, M1.3, M1.4, M1.5, M1.6, M1.9)
did not match the repo and appeared to have been reconstructed from
memory/summary text rather than direct inspection. `V4-STAGE-A-M1-DOC-TRUTH-REPAIR-01`
subsequently rewrote the affected body sections above to cite the
verified current locations directly, so this addendum no longer needs to
carry the corrections separately — see each section's citations for the
current verified truth.

None of these corrections changed any classification: no CODE_MISSING,
WIRING_MISSING, or TEST_MISSING gap was found in the items sampled.
Items not independently re-verified in that bounded pass should still be
treated as citation-unverified until independently checked.

---

## M2 Code Completion Manifest — Stage B (`V4-BULK-CODE-COMPLETION-STAGE-B-M2-02`)

**Mission:** Stage B M2 — Bulk Code Completion
**Started:** 2026-09-18
**Baseline HEAD:** 28fff6952a65791cd489b88d3673561ab8c12a03
**Branch:** v4-bulk-code-completion-stage-b-m2-01

### CORRECTION (`V4-STAGE-B-M2-REPAIR-01`, 2026-09-18) — the 9/9 CODE_CLOSED conclusion below was REJECTED by independent review

The census below (original commit `2b520aa0`) is preserved verbatim as audit
history. It contained a real defect: it verified that Bundle 6
(`runtime_strategy_conflict.rs`) is production-wired and correctly resolves
conflicting same-symbol decisions **when given synthetic/hand-constructed
same-symbol inputs**, but never checked whether the actual upstream
production producer can ever hand Bundle 6 two genuine, independently
dispatched, economically-active strategy decisions for the *same* symbol in
one cycle. It cannot, today:

- `mqk-portfolio/src/dynamic_selection.rs:29` — the dynamic-selection plan
  model is explicitly documented and enforced: "Exactly one selected
  candidate per symbol, or none — never more."
- `mqk-strategy/src/host.rs` (`StrategyHostError::MultiStrategyNotAllowed`)
  — a `StrategyHost` instance refuses a second strategy registration.
  `mqk-strategy/tests/scenario_parallel_long_short_strategy_01.rs::p11_strategy_host_enforces_single_strategy`
  is a committed, passing test that exists specifically to document and
  prove this limit, asserting `MultiStrategyNotAllowed` and stating in its
  own doc comment: "True concurrent long+short dispatch on the same symbol
  requires a separate patch: `MULTI-STRATEGY-RUNTIME-DISPATCH-01`."
- `mqk-strategy/src/engines/mod.rs:71-75` — the short-side strategy variant
  is registered in the plugin catalog but documented as selectable only
  one-at-a-time via `MQK_STRATEGY_IDS`, with the same
  `MULTI-STRATEGY-RUNTIME-DISPATCH-01` forward reference.
- `state/autonomous_completed_bar_driver.rs:329-333`
  (`resolve_single_effective_binding`) — the actual authoritative
  autonomous completed-bar production driver requires "exactly one
  configured assignment that matches the resolved binding's symbol/strategy
  exactly — ... a same-strategy/different-symbol or any other multi-symbol
  assignment is unsupported, never silently narrowed to 'the first
  assignment'" (fails closed, but confirms multi-symbol autonomous
  completed-bar dispatch does not exist in the authoritative driver). The
  original census's citation of `pending_strategy_bar_input.lock().await.take()`
  in the ordinary execution-loop tick was real code, but not the
  authoritative autonomous completed-bar driver this requirement needs.

**Corrected classifications** (see the "Requirement-by-Requirement
Evidence" sections below, which are corrected in place rather than
duplicated):

- **M2.1** (multiple strategy instances and symbols operate simultaneously):
  `CODE_CLOSED` → **`PARTIAL / WIRING_MISSING`**. Multiple *symbols* with
  distinct single strategies operate simultaneously (host-pool evidence
  from the original census stands for that sub-case). Multiple
  *economically-active strategies on the same symbol* do not — that
  sub-case is blocked by the same-symbol producer gap below.
- **M2.3** (same-symbol competing strategy intentions resolve through
  explicit portfolio/execution authority): `CODE_CLOSED` → **`WIRING_MISSING`**.
  Bundle 6 (the resolver) is real and correctly tested against synthetic
  same-symbol inputs; the producer wiring that could ever hand it two
  genuine same-symbol candidates does not exist yet.
- **M2.7** (concurrent completed bars cannot be lost or double-consumed):
  `CODE_CLOSED` → **`WIRING_MISSING`**. The durable per-assignment claim
  identity foundation (`sys_autonomous_daily_bar_dispatches`, keyed by
  `(operation_id, local_symbol, timeframe, bar_end_ts)`, migration
  `0050_autonomous_daily_bar_dispatches.sql`) exists, but the authoritative
  driver (`resolve_single_effective_binding`) only ever resolves exactly
  one binding, not one per configured multi-symbol assignment.

All other requirements (M2.2, M2.4, M2.5, M2.6, M2.8, M2.9, and the five
additional invariants) are **not** rejected by this review and retain their
original `CODE_CLOSED` classification and evidence below — none of that
evidence depended on the same-symbol-producer or multi-symbol-autonomous-bar
gaps.

**M2 CODE COMPLETION is NOT CLOSED.** `docs/CURRENT_MISSION.md` §-2 records
the corrected overall status. See the repair patches (R1/R2/R3/R4) tracked
under `V4-STAGE-B-M2-REPAIR-01` for the remaining work.

### M2 Scope (Canonical Authority)

From `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md` § Milestone 2
("Concurrent Multi-Strategy / Multi-Symbol Engine"), required capabilities:

1. multiple strategy instances and symbols operate simultaneously
2. per-strategy/per-symbol state cannot contaminate another
3. same-symbol competing strategy intentions resolve through explicit
   portfolio/execution authority
4. strategies propose targets/opportunities rather than owning the account
5. portfolio-level capital allocation and risk aggregation span strategies
6. retries/restarts cannot create duplicate dispatches/orders
7. concurrent completed bars cannot be lost or double-consumed
8. conflict resolution/order is deterministic where economics depend on it
9. failures/panics are isolated according to explicit policy

### Census Result

**CONCLUSION:** Bounded, requirement-driven inspection found that every
M2-required capability is **already CODE_CLOSED** through prior, already-
committed patches (`MULTI-SYMBOL-DISPATCH-LOOP-01`,
`MULTI-SYMBOL-CAPITAL-CAPS-01`, `DYNAMIC-STRATEGY-SYMBOL-SELECTION-01`
Phases 2-7B / `PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE`,
`MULTI-STRATEGY-CONFLICT-POLICY-01` (Bundle 6),
`RUNTIME-OPPORTUNITY-ALLOCATION-01` (Bundle 5),
`MULTI-STRATEGY-RUNTIME-DRY-RUN-01`,
`A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01`,
`STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02`). No `CODE_MISSING`,
`WIRING_MISSING`, or `TEST_MISSING` gap was found against the canonical M2
requirement list above. **No new production/test code was implemented by
this controller** — per the controller's own instruction ("If M2 code is
already complete: prove it with load-bearing references/tests and do not
invent changes"), none was needed.

`CODE_CLOSED: 9/9`. `CODE_MISSING: 0`. `WIRING_MISSING: 0`.
`TEST_MISSING: 0`. `SPEC_DECISION_REQUIRED: 0`. `BLOCKED_DEPENDENCY: 0`.

This is a bounded requirement-driven census (mirrors the Stage A M1
methodology), not an exhaustive audit of every file in the concurrency
seam. Items not independently re-verified here should be treated as
citation-unverified until independently checked, per the Stage A
historical-correction precedent above.

### Requirement-by-Requirement Evidence

#### M2.1 — Multiple strategy instances and symbols operate simultaneously

**Status:** PARTIAL / WIRING_MISSING (corrected by `V4-STAGE-B-M2-REPAIR-01`; was CODE_CLOSED)

**Correction:** the evidence below is real and stands for the "multiple
*symbols*, each with a distinct single strategy" sub-case. It does **not**
prove "multiple economically-active *strategies* on the same symbol" —
`StrategyHostError::MultiStrategyNotAllowed` and `dynamic_selection.rs`'s
"exactly one selected candidate per symbol, or none" both block that
sub-case today. See the manifest-level correction note above and R1 in
`docs/CURRENT_MISSION.md` for the remaining work.

**Production seam:** `DynamicSelectionHostPool` (`core-rs/crates/mqk-daemon/src/dynamic_selection_host_pool.rs:121-140`) — `BTreeMap<HostPoolKey, StrategyHost>`, one isolated `StrategyHost` instance per selected `(symbol, strategy_id, timeframe_secs)` binding. Built by `RuntimeStrategyDispatchAuthority::DynamicPaperEnforced` before the Phase 7A start barrier releases and moved into the execution loop (`state/loop_runner.rs`).

**Wiring:** `state/lifecycle.rs:668` calls `dynamic_selection_start_gate::evaluate_dynamic_selection_start_gate` from the real start path (`start_execution_runtime`); the resulting authority/host pool is dispatched every tick in `state/loop_runner.rs:1169-1250` (`tick_strategy_dispatch_selected_hosts_with_bar_facts`), branching per-binding, not per-run. The legacy `Legacy { assignments }` variant already dispatches multiple `(symbol, strategy_id)` pairs per tick via `tick_strategy_dispatch_multi_symbol_with_bar_facts` (`state.rs:4287-4299`, `MULTI-SYMBOL-DISPATCH-LOOP-01`).

**Test proof (this session, green):** `dynamic_selection_host_pool::tests::two_symbols_get_independent_isolated_hosts`, `same_strategy_independently_instantiated_for_two_symbols`, `same_symbol_strategy_different_timeframe_is_not_treated_as_duplicate_key` — 10/10 passed.

#### M2.2 — Per-strategy/per-symbol state cannot contaminate another

**Status:** CODE_CLOSED

**Production seam:** Each `HostPoolKey` gets its own `StrategyHost` (no shared mutable strategy state across bindings). Cross-binding provenance is independently re-validated in `state/loop_runner.rs` (`dynamic_selection_envelope_ok`) before submission, keyed by exact `(run_id, plan_id, symbol, strategy_id, timeframe_secs)`.

**Test proof (this session, green, real negative controls):** `state::loop_runner::phase7b_provenance_tests::swapped_provenance_between_two_decisions_fails_closed` (swaps two real bindings' provenance and proves both are rejected), `provenance_naming_a_binding_that_does_not_exist_fails_closed`, `mutated_symbol_fails_closed`, `mutated_strategy_id_fails_closed`, `mutated_timeframe_secs_fails_closed`, `mutated_plan_id_fails_closed` — 13/13 passed.

#### M2.3 — Same-symbol competing strategy intentions resolve through explicit portfolio/execution authority

**Status:** WIRING_MISSING (corrected by `V4-STAGE-B-M2-REPAIR-01`; was CODE_CLOSED)

**Correction:** Bundle 6 below is real, production-wired, and correctly
resolves conflicting same-symbol inputs — but only inputs it is actually
given. No current production producer can hand it two genuine,
independently-dispatched, economically-active same-symbol strategy
decisions in one cycle: `dynamic_selection.rs` selects at most one
candidate per symbol, and `StrategyHost` refuses a second concurrent
strategy registration
(`scenario_parallel_long_short_strategy_01.rs::p11_strategy_host_enforces_single_strategy`,
asserting `MultiStrategyNotAllowed`). Remaining work:
`MULTI-STRATEGY-RUNTIME-DISPATCH-01` (tracked as R1).

**Production seam:** `runtime_strategy_conflict.rs` (`MULTI-STRATEGY-CONFLICT-POLICY-01` Bundle 6) — `apply_conflict_policy`/`gather_and_resolve`, wired into `state/loop_runner.rs` immediately before Bundle 5 (opportunity allocation). Closed-vocabulary, fail-closed mode resolution (`off`/`shadow`/`paper_enforced`) mirroring `dynamic_selection_mode.rs`'s pattern.

**Test proof (this session, green):** `runtime_strategy_conflict::tests::paper_enforced_emits_at_most_one_decision_per_symbol`, `unbound_sell_is_refused_and_never_reaches_downstream`, `paper_enforced_never_resurrects_a_refused_increase`, `unrelated_symbol_unaffected_by_a_refused_conflict`, `shadow_mode_returns_exact_original_vector_in_exact_original_order` — 25/25 passed.

#### M2.4 — Strategies propose targets/opportunities rather than owning the account

**Status:** CODE_CLOSED

**Production seam:** Native `on_bar` output (`TargetPosition`) is a proposal only: it passes through symbol-match guard (`AppState::retain_targets_matching_symbol`), per-symbol position caps (`MULTI-SYMBOL-CAPITAL-CAPS-01`), Bundle 6 conflict policy, and Bundle 5 opportunity allocation (`runtime_opportunity_allocation::gather_and_apply`, which delegates to the pure `mqk_portfolio::compute_allocation_cycle` allocator — a real production caller, not a dormant library) before any decision reaches `submit_internal_strategy_decision`.

**Test proof (this session, green):** `runtime_opportunity_allocation::tests::symbol_not_in_opportunity_set_is_refused_not_fabricated`, `paper_enforced_drops_decision_when_no_capital_available` — 35/35 passed in module.

#### M2.5 — Portfolio-level capital allocation and risk aggregation span strategies

**Status:** CODE_CLOSED

**Production seam:** `mqk_portfolio::compute_allocation_cycle` (`AllocationCycleContext`/`AllocationCandidateInput`/`AllocationCycleResult`), called from `runtime_opportunity_allocation.rs:82-85,721` (`gather_and_apply`), itself called once per tick across every symbol dispatched that cycle (decisions collected across all symbols before any submission — `state/loop_runner.rs:1302-1317`).

**Test proof (this session, green):** `runtime_opportunity_allocation::tests` — 35/35 passed, including `same_economic_cycle_replayed_on_a_later_tick_yields_the_same_cycle_id`.

#### M2.6 — Retries/restarts cannot create duplicate dispatches/orders

**Status:** CODE_CLOSED

**Production seam:** Reuses the M1-accepted outbox claim/idempotency seam (`outbox_claim_batch`, single atomic operation, DB rules unchanged) plus `STRATEGY-DECISION-ECONOMIC-IDEMPOTENCY-02` decision-id derivation, which is per-symbol/per-strategy salted (not a shared wall-clock value — see `state/loop_runner.rs:1251-1267` comment on `now_micros` being excluded from Bundle 5/6 identity). No new seam invented; existing M1 durable-write invariants (`db_rules.md`) apply unchanged per-symbol.

#### M2.7 — Concurrent completed bars cannot be lost or double-consumed

**Status:** WIRING_MISSING (corrected by `V4-STAGE-B-M2-REPAIR-01`; was CODE_CLOSED)

**Correction:** the `pending_strategy_bar_input.lock().await.take()` seam
cited below is real, but it is the ordinary execution-loop tick's shared
trigger, not the authoritative autonomous completed-bar production driver.
That driver, `resolve_single_effective_binding`
(`state/autonomous_completed_bar_driver.rs:329-333`), requires "exactly one
configured assignment" and explicitly documents that "a
same-strategy/different-symbol or any other multi-symbol assignment is
unsupported" (fails closed, not silently narrowed — a correctly fail-closed
gap, not a correctness defect, but still a gap against M2.7's multi-symbol
requirement). The durable claim-identity foundation this needs already
exists (`sys_autonomous_daily_bar_dispatches`, keyed by `(operation_id,
local_symbol, timeframe, bar_end_ts)`, migration
`0050_autonomous_daily_bar_dispatches.sql`) and must be reused, not
replaced. Remaining work: extend the driver to resolve one binding per
configured multi-symbol assignment (tracked as R2).

**Production seam (real, but not sufficient alone — see correction above):**
`pending_strategy_bar_input.lock().await.take()` (`state.rs:4248,4295`) — a single-consumer `Option::take()` per completed-bar trigger, shared once per tick across every dispatched symbol/binding; each symbol's actual OHLC window is then independently loaded from `md_bars` per `(symbol, timeframe)` inside `dispatch_native_strategy_for_symbol_with_bar[_and_facts]`, so the shared trigger cannot be double-consumed across ticks while per-symbol data stays independent. This covers the ordinary execution-loop path only, not the autonomous daily-operation completed-bar driver.

#### M2.8 — Conflict resolution/order is deterministic where economics depend on it

**Status:** CODE_CLOSED

**Production seam:** `dynamic_selection_host_pool.rs` uses `BTreeMap` (deterministic key ordering, not hash-map iteration order); `runtime_strategy_conflict.rs`'s `compute_conflict_cycle_id` binds cycle identity to every result-affecting fact (mode, timeframe, bar provenance, order semantics) so two structurally-different evaluations never collide, and two identical replays always agree.

**Test proof (this session, green):** `dynamic_selection_host_pool::tests::input_order_does_not_change_the_resulting_pool_keys`, `runtime_strategy_conflict::tests::different_symbol_set_changes_cycle_id`, `different_bar_changes_cycle_id`, `different_strategy_changes_cycle_id`, `different_current_position_changes_cycle_id`, `shadow_versus_paper_enforced_changes_plan_id` — all passed (counted in the 10/10 and 25/25 above).

#### M2.9 — Failures/panics are isolated according to explicit policy

**Status:** CODE_CLOSED

**Production seam:** `A1-MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01` — a panic inside the real `Strategy::on_bar` callback is caught narrowly at the one seam that invokes it (`invoke_native_strategy_host_on_bar`), never around infrastructure (DB load, journal writes, which still unwind normally on a genuine infra panic). The explicit, documented policy is to quarantine the affected shared host (`Failed`) for the rest of the run rather than permit sibling continuation against possibly-corrupted state — a deliberate fail-closed choice, not silent contamination. The `DynamicPaperEnforced` selected-host path applies the same narrow containment per selected host.

**Test proof (this session, green, real DB — `postgres://postgres:postgres@127.0.0.1:5434/mqk_test`):** `state::phase7b_selected_host_dispatch_tests::a1_t7_selected_host_panic_is_contained_and_halts_whole_tick`, `a1_r7_real_on_bar_panic_persists_durable_fault_evidence_via_legacy_path` — both inject a real panic (`A1_TEST_INJECTED_PANIC`, `A1_REAL_ON_BAR_PANIC_LEGACY_DB_TEST`), both pass.

### Additional invariants inspected (not separate M2 numbered requirements, but explicitly required by the controller mission)

- **Dry-run strategies remain incapable of economic submission:** `state/dry_run_strategy.rs` — structurally impossible to submit (no `PgPool`/`AppState`/broker handle in any function signature in the module; `DryRunStrategyDiagnostic.submitted` is always `false`). Also proves same-tick multi-strategy evaluation (`drs07_multiple_ids_evaluated_independently_same_window`). Test proof: 7/7 passed.
- **No silent loss of assignments over the configured cap:** `MultiSymbolConfigError::ConcurrentLimitExceeded` / `HardCeilingExceeded` fail closed rather than truncating (`state/multi_symbol_config.rs:171-176,298-310`). Test proof: `multi_symbol_config::frozen_fleet_raw_inputs_tests` — 3/3 passed.
- **Unknown strategy/symbol/timeframe fail closed:** `dynamic_selection_host_pool::tests::unknown_strategy_id_is_refused`, `wrong_timeframe_for_a_real_strategy_is_refused`, `duplicate_key_is_refused` — all passed (counted above).
- **Promotion refusal remains enforced per strategy identity (no dynamic-selection bypass of M1's promotion gate):** `dynamic_selection_plan_builder.rs:329,344` calls `promotion_evidence_validation::validate_active_paper_candidate` — every dynamic-selection candidate is still gated on `active_paper` promotion identity before it can be selected; no separate authorization path exists.
- **Mode default and live-lock:** `MQK_DYNAMIC_STRATEGY_SYMBOL_SELECTION_MODE` defaults to `Off` (unset/unrecognized never fabricates a mode) and is hard-locked to `Off` outside `deployment_mode=paper && adapter=alpaca` (`dynamic_selection_mode.rs`). Test proof: `dynamic_selection_mode::tests` — 14/14 passed, plus `dynamic_selection_start_gate::tests` — 28/28 passed.

### Test Summary (this session, all green, no code changed)

```text
dynamic_selection_host_pool::           10 passed
runtime_strategy_conflict::             25 passed
state::dry_run_strategy::                7 passed
state::loop_runner::phase7b_provenance_tests::  13 passed
state::multi_symbol_config::             3 passed
runtime_opportunity_allocation::        35 passed
dynamic_selection_start_gate::          28 passed
dynamic_selection_mode::                14 passed
state::phase7b_selected_host_dispatch_tests::a1_t7_...   1 passed (real DB)
state::phase7b_selected_host_dispatch_tests::a1_r7_...   1 passed (real DB)
---------------------------------------------------------------
TOTAL                                  137 passed, 0 failed
```

Run against `postgres://postgres:postgres@127.0.0.1:5434/mqk_test` (`mqk-test-postgres`) where DB-backed; no migration-checksum drift encountered this session (the drift noted in `docs/CURRENT_MISSION.md` §6 as of the prior checkpoint did not reproduce here). Full workspace/GUI/research-py suites were not run (no crate outside `mqk-daemon` was touched; per the controller's efficiency contract).

### M2 Status (ORIGINAL — SUPERSEDED, see correction note above)

~~**M2 CODE COMPLETION: CODE_CLOSED** (9/9 canonical requirements, bounded census, no gaps found, no new code needed).~~

~~**M2 OPERATIONAL ACCEPTANCE: NOT CLAIMED BY THIS CONTROLLER.**~~

**This conclusion was REJECTED by independent review (`V4-STAGE-B-M2-REPAIR-01`).**
Corrected status: `CODE_CLOSED 6/9` (M2.2, M2.4, M2.5, M2.6, M2.8, M2.9),
`WIRING_MISSING 2/9` (M2.3, M2.7), `PARTIAL/WIRING_MISSING 1/9` (M2.1). See
`docs/CURRENT_MISSION.md` §-2 for the current authoritative M2 status and
remaining-work tracking (R1-R4).

---

## Stage B M2 Repair Findings (`V4-STAGE-B-M2-REPAIR-01`, 2026-09-18)

Baseline: `27a1733c` (after PATCH R0). Working through R1-R4 in mission order.

### R1 — `MULTI-STRATEGY-RUNTIME-DISPATCH-01`: STOPPED — `SPEC_DECISION_REQUIRED`

**Investigation performed (no code changed):**

- `mqk-portfolio/src/dynamic_selection.rs:29` — the Bundle 7 pure selection
  model is explicitly frozen: "Exactly one selected candidate per symbol, or
  none — never more." This is the actual production seam that would need to
  hand Bundle 6 two same-symbol candidates; it structurally cannot.
- `docs/specs/multi_strategy_conflict_policy_01a_current_truth_and_contract.md`
  Q4 (Bundle 6's own committed design doc) states directly: "Building real
  multi-strategy competition requires per-`(symbol, strategy_id)` host
  instantiation — that is Bundle 7's dynamic strategy-symbol selection work,
  explicitly deferred... Bundle 6 must be provably correct against the
  *possibility* of multiple same-symbol candidates without requiring the
  runtime to actually produce them today." Bundle 6 was deliberately scoped
  to prove conflict-resolution *correctness*, not to be fed genuine
  same-symbol candidates — that production step was explicitly punted
  forward, undated, unnamed beyond the `MULTI-STRATEGY-RUNTIME-DISPATCH-01`
  forward reference.
- `docs/specs/dynamic_strategy_symbol_selection_01f_closure.md` (Bundle 7's
  own closure doc) confirms the same deferral and does not name any
  follow-up design for multi-strategy-per-symbol economic authority.
- No file anywhere in the repo matching `*MULTI-STRATEGY-RUNTIME-DISPATCH*`
  exists as a design doc — the forward reference in
  `mqk-strategy/src/engines/mod.rs:75` and
  `scenario_parallel_long_short_strategy_01.rs:15` is aspirational, not a
  committed design.

**The exact unresolved choice:** how should the set of strategies
simultaneously economically-active on the *same* symbol be configured and
authorized? None of the following is answered by any committed contract:

- (a) Extend `MQK_STRATEGY_IDS` (today: only its first entry is used,
  `state/multi_symbol_config.rs:92-95,509-518`) so every entry becomes an
  independently economically-active strategy applied to every configured
  symbol?
- (b) Extend the watchlist-v2 artifact's `strategy_assignments` from
  `symbol -> one strategy_id` to `symbol -> Vec<strategy_id>`, so multi-
  strategy authorization is per-symbol explicit, not fleet-wide?
- (c) Relax `dynamic_selection.rs`'s frozen "exactly one selected candidate
  per symbol" invariant to a ranked top-N under a new explicit mode, still
  gated by `active_paper` promotion per candidate?
- (d) Something else the operator has in mind that isn't derivable from
  existing code/docs.

Each choice has different economic, promotion-authority, and capital-caps
consequences (`MULTI-SYMBOL-CAPITAL-CAPS-01`'s `per_symbol_max_position_qty`
is currently scoped per-symbol, not per-`(symbol,strategy)` — a second
economically-active strategy on the same symbol would need its own capital
budget carved out of the same per-symbol cap, which is itself a policy
choice). Per `CLAUDE.md` §19 ("do not invent trading economics") and this
mission's own explicit instruction ("If the existing configuration contract
cannot unambiguously define which strategies are economically active...
STOP this patch as `SPEC_DECISION_REQUIRED` with the exact unresolved
choice"), this patch stops here. **No code was written for R1.**

### R2 — Multi-symbol completed-bar driver: STOPPED — `SPEC_DECISION_REQUIRED`

**Investigation performed (no code changed):**

- `state/autonomous_completed_bar_driver.rs:334-381`
  (`resolve_single_effective_binding`) confirmed as the authoritative
  driver: requires `assignment_config.symbols.len() == 1`, fails closed
  (`MultiSymbolAssignmentNotExactlyBound`) otherwise — never silently
  narrows to "the first assignment." The durable claim-identity foundation
  (`sys_autonomous_daily_bar_dispatches`, keyed by `(operation_id,
  local_symbol, timeframe, bar_end_ts)`, migration
  `0050_autonomous_daily_bar_dispatches.sql`) exists and should be reused
  unchanged — this part of R2 is genuinely a mechanical extension.
- However, `tick_autonomous_completed_bar_driver`'s single
  `AutonomousCompletedBarDriverOutcome` return value flows into
  `state/autonomous_daily_coordinator.rs::apply_completed_bar_driver_outcome`,
  which classifies it into `NoDurableEffect` or `Critical{...}` and, for
  `Critical`, durably transitions the **one** `AutonomousDailyOperationRecord.state`
  column (e.g. `running -> controller_degraded`) — an operation-level, not
  symbol-level, state machine (`ALL_OPERATION_STATES`,
  `mqk-db/src/autonomous_daily_operation.rs:45-62`).
- **Concrete, code-proven conflict with R2's own requirements #3/#7:**
  `state/autonomous_completed_bar_task.rs::select_driver_mode_for_state`
  (lines 154-164) returns `Some(...)` (permits automated driver invocation
  at all) **only** for `STATE_RUNNING` and the five pre-runtime states —
  every other state, explicitly including `controller_degraded` and
  `evidence_degraded`, returns `None`: "no automated driver invocation is
  legal for this state (recovery/stopping/manual/degraded/terminal/unknown)."
  Concretely: if binding MSFT hits a genuine non-remediable blocker (e.g.
  `unsupported_timeframe`) and the existing, unmodified
  `apply_completed_bar_driver_outcome` degrades the operation to
  `controller_degraded` (today's behavior, correct for the single-binding
  case), then on the *very next tick* the entire completed-bar task stops
  being invoked for **every** binding, including a healthy AAPL binding
  that was dispatching correctly. This is not a hypothetical risk — it is
  the literal, current, tested behavior of `select_driver_mode_for_state`,
  and it directly violates R2's own requirement #3 ("a claimed bar for
  symbol A cannot suppress symbol B") and #7 ("one symbol lacking a new bar
  must not cause another symbol's real new bar to be lost").

**The exact unresolved choice:** how does per-operation durable state
(`running`/`controller_degraded`/`evidence_degraded`/`manual_intervention_required`)
aggregate across N simultaneously-configured symbol/timeframe bindings when
they can independently succeed, transiently wait, or hit a non-remediable
blocker in the same tick? Candidate resolutions, none currently authorized
by any committed contract:

- (a) Keep operation-level state coarse (today's shape, unchanged): any one
  binding's `Critical` outcome degrades the *whole* operation for *all*
  bindings — simplest, zero new schema, but knowingly violates R2 #3/#7 as
  demonstrated above the moment more than one symbol is configured.
- (b) Introduce genuine per-binding fault state (e.g. a status column on
  `sys_autonomous_daily_bar_dispatches` or a new durable table), and change
  `select_driver_mode_for_state`/the coordinator so operation-level
  `controller_degraded` reflects "no configured binding can make progress"
  rather than "at least one binding failed" — satisfies #3/#7 but is a real
  schema + state-machine design change to an already-audited, heavily
  fail-closed-tested subsystem (Phase D/D3, `REPAIR 4` durable-degrade,
  critical-notification paths), not a mechanical extension.
- (c) Some other operator-directed policy (e.g. only certain fault classes
  are per-binding-isolable; evidence/dispatch-claim-integrity faults still
  degrade globally since they indicate possible corruption, while
  readiness/binding-config faults are per-binding-isolable).

This is a genuine operational-safety design decision for a live-money-
adjacent autonomous system (it determines when the whole autonomous
operation halts vs. continues degraded-for-one-symbol), not a matter of
code effort. Writing an unwired "resolve multiple bindings" helper without
resolving this would not move any M2.7 sub-requirement to `CODE_CLOSED` — it
would just be more `WIRING_MISSING` surface, repeating exactly the mistake
this repair mission exists to fix. **No code was written for R2.**

### R3 — Retry/restart load-bearing proof: `CLOSED`

Ran the existing focused scenario in full against the real test Postgres
(`postgres://postgres:postgres@127.0.0.1:5434/mqk_test`):

```text
cargo test -p mqk-daemon --test scenario_strategy_decision_idempotency_01
running 16 tests ... test result: ok. 16 passed; 0 failed
```

Includes the DB-backed restart/replay proofs
(`d05_same_bar_reevaluated_twice_submits_exactly_one_outbox_row`,
`d06_restart_recomputes_identical_decision_id_and_resubmit_is_a_noop`,
`c2_partial_fill_reevaluation_creates_zero_additional_order`,
`c4_restart_replay_with_partial_fill_creates_zero_additional_order`) and the
multi-symbol identity negative control
(`c10_multi_symbol_one_symbols_current_never_affects_another`). No code
changed — all 16 already passed. **M2.6 now has direct load-bearing proof
run this session**, upgrading it from "cited seam" to "directly verified,"
still `CODE_CLOSED`.

### R4 — Integrated M2 finish-line scenario: `BLOCKED`

Cannot be honestly built. The canonical finish-line combination this
scenario must prove ("same-symbol conflicting strategy signals" and
"multiple strategies × multiple symbols" operating through the *real*
production seam) does not yet exist as a production path — R1 and R2 are
both `SPEC_DECISION_REQUIRED`, not implemented. A test that reimplemented
the missing production logic itself would violate this patch's own
instruction ("must not reimplement the production logic inside the test")
and would prove nothing about the real system. R4 is blocked on R1 and R2.

### Corrected M2 Status (after R0-R4)

```text
M2.1  PARTIAL / WIRING_MISSING   (blocked on R1's SPEC_DECISION_REQUIRED)
M2.2  CODE_CLOSED                (unaffected by this repair)
M2.3  WIRING_MISSING             (blocked on R1's SPEC_DECISION_REQUIRED)
M2.4  CODE_CLOSED                (unaffected by this repair)
M2.5  CODE_CLOSED                (unaffected by this repair)
M2.6  CODE_CLOSED                (upgraded: direct 16/16 DB-backed proof run this session, R3)
M2.7  WIRING_MISSING             (blocked on R2's SPEC_DECISION_REQUIRED)
M2.8  CODE_CLOSED                (unaffected by this repair)
M2.9  CODE_CLOSED                (unaffected by this repair)

CODE_CLOSED: 6/9   WIRING_MISSING: 2/9   PARTIAL/WIRING_MISSING: 1/9
SPEC_DECISION_REQUIRED: 2 (R1, R2 — both block further M2 code progress
                            on M2.1/M2.3/M2.7 until resolved)
```

**M2 CODE COMPLETION: NOT CLOSED.** Two genuine operator decisions are
required before the remaining M2 code can be safely implemented — see R1
and R2 above for the exact unresolved choices and their options. **M2
operational acceptance remains unclaimed** (unaffected either way). M1
operational blocker, `intraday_scalper` rejection, and alpha-discovery
deferral are unchanged by this repair.

---

## Stage B M2 Completion After Operator Decisions (`V4-STAGE-B-M2-REPAIR-02`, 2026-09-18)

Baseline: `0ba64172` (end of `V4-STAGE-B-M2-REPAIR-01`). The operator resolved
both `SPEC_DECISION_REQUIRED` items with frozen decisions; this controller
implemented against them.

### R1A — FROZEN: `docs/specs/multi_strategy_runtime_dispatch_01a_frozen_contract.md`

Documents the operator's R1 decision (explicit per-symbol multi-strategy
authorization via a new, additive `watchlist-v3` schema) in full: JSON
shape, validation contract, `MAX_STRATEGIES_PER_SYMBOL` bound, identity
tuple, independent per-binding promotion/readiness requirements,
deterministic ordering, reuse of the existing host-pool/Bundle-6/Bundle-5
pipeline, account/symbol (not per-strategy) capital authority, and no
dry-run/Live authority change. Commit `7f78cc0d`.

### R1B — `MULTI-STRATEGY-RUNTIME-DISPATCH-01`: CODE_CLOSED (dispatch/conflict/config pipeline); WIRING_MISSING (live daemon activation)

Implemented and tested (commit `a00f9238`, 69 tests green: 9 new R1B proofs
+ 43 unchanged v1/v2 regression + 17 new v3 schema tests):

- `watchlist_intake.rs`: `WATCHLIST_SCHEMA_VERSION_V3`, `LoadedWatchlistArtifactV3`,
  `evaluate_watchlist_intake_v3` — a wholly separate, additive evaluation
  path. v1/v2 parsing is unchanged byte-for-byte (regression-proven).
- `multi_strategy_runtime_dispatch.rs` (new): resolves every `(symbol,
  strategy_id)` pair a v3 artifact authorizes **independently** through the
  real Bundle 7 evidence gate (`evaluate_candidate` +
  `compute_dynamic_selection_plan`, called once per binding so ranking
  never applies — a promoted sibling structurally cannot authorize an
  unpromoted one), excludes dry-run identities before any I/O, and builds
  the real `DynamicSelectionHostPool` + `RuntimeStrategyDispatchAuthority::
  DynamicPaperEnforced` — the exact same per-tick dispatch/conflict/
  allocation pipeline Bundle 7 already uses, reused verbatim, not
  reimplemented.
- All 10 of R1B's mission-required focused proofs pass, each calling a real
  production function: two real strategies on one symbol both genuinely
  `on_bar`-dispatch with distinct identity
  (`r1b_two_strategies_same_symbol_both_dispatch_with_distinct_identity`,
  real DB-backed); both original same-symbol proposals reach Bundle 6 and
  resolve to the deterministic risk-reducing survivor regardless of input
  order (`r1b_both_real_strategy_proposals_reach_bundle6_and_resolve_
  deterministically`, `r1b_reversed_real_strategy_input_order_yields_
  same_result`); a promoted sibling never authorizes an unpromoted one
  (`r1b_06_promoted_sibling_never_authorizes_an_unpromoted_one`); same-symbol
  sibling provenance swap fails closed
  (`r1b_same_symbol_sibling_provenance_swap_fails_closed`); dry-run identity
  never appears in the authorized set
  (`r1b_08_dry_run_strategy_id_never_appears_in_authorized_bindings`); no
  cross-symbol/cross-strategy contamination
  (`r1b_10_multi_symbol_multi_strategy_no_cross_contamination`); existing
  single-strategy configuration is provably unaffected (43/43 unmodified
  v1/v2 tests still pass).

**What remains WIRING_MISSING:** activation in the live daemon start
sequence. `state/lifecycle.rs::build_dynamic_selection_start_snapshot` (the
one call site that constructs the run's `RuntimeStrategyDispatchAuthority`)
returns `DynamicSelectionRuntimeState`, a type whose fields
(`disposition: DynamicSelectionStartGateDisposition`, `plan:
Option<DynamicSelectionPlan>`, etc.) are Bundle-7-plan-shaped and feed a
durable evidence-persistence path (`dynamic_selection_evidence_writer`,
`sys_dynamic_selection_plans`) built specifically for Bundle 7's ranking
plan. Splicing v3 in at this exact call site would require either
fabricating a synthetic Bundle-7-shaped plan/evidence for a mechanism that
isn't Bundle 7 (semantically dishonest evidence), or extending that
durable evidence/status surface to understand a second plan kind — a new
observability surface R1A's own frozen contract §9 explicitly placed out of
scope for R1B ("Not in scope for R1B: GUI surfaces, new API routes"). This
is a genuine, narrowly-scoped follow-up integration patch, not a design
question — the dispatch mechanism itself is proven correct; only its
activation switch and observable evidence shape remain to be built.

### R2A — durable per-binding driver state: CODE_CLOSED

Implemented and tested (commit `9fd7df12`, 8/8 DB-backed tests green,
migration 0071). See `docs/V4_CODE_COMPLETION_MANIFEST.md`'s R2A commit
message and the migration's own header for full detail: additive
`sys_autonomous_daily_binding_state` table, closed active/locally_blocked
vocabulary with a closed binding-local reason-code set mirroring the R2
operator decision's own enumerated categories, `mark_autonomous_daily_
binding_active`/`_locally_blocked` (idempotent upserts) and `fetch_
autonomous_daily_binding_states`. Proven: schema/constraint enforcement,
restart persistence (fresh-pool read), cross-binding isolation (blocking
one binding never mutates a sibling, including two strategies on the same
symbol).

### R2B/R2C — multi-binding completed-bar driver + aggregate operation state: NOT IMPLEMENTED — WIRING_MISSING

Investigated in full; not implemented this session. `resolve_single_
effective_binding` (`state/autonomous_completed_bar_driver.rs:334-381`),
`tick_autonomous_completed_bar_driver` (897-994), its production task
adapter (`autonomous_completed_bar_task.rs`, ~900 lines, including
`select_driver_mode_for_state`), and the coordinator's outcome-to-state
aggregation (`autonomous_daily_coordinator.rs::apply_completed_bar_driver_
outcome`/`classify_completed_bar_driver_outcome`) together form roughly
2,600 lines of already-deeply-audited, safety-critical autonomous-operation
machinery — the actual live trading heartbeat for the deployed system. A
correct rewrite requires: (1) resolving every configured binding instead of
exactly one; (2) reusing `sys_autonomous_daily_bar_dispatches`'s existing
per-bar claim identity unchanged per binding; (3) writing R2A's new
per-binding state on every binding-local outcome; (4) reclassifying
`apply_completed_bar_driver_outcome` so a binding-local fault (per R2's
frozen enumeration) updates only that binding's row and leaves
`sys_autonomous_daily_operations.state` untouched, while a global-critical
fault (evidence-lineage corruption, dispatch-claim ambiguity, runtime-
ownership/leadership failure, etc.) still degrades the whole operation
exactly as today; and (5) proving, with real DB-backed negative controls,
that a binding-local block on one symbol never suppresses another symbol's
genuine progress on the same or a later tick. Given the safety-critical
nature of this exact code path (CLAUDE.md's correctness-first priority, and
this repair mission's own origin — a prior session's rushed, insufficiently-
verified claim of closure on this same subsystem class), attempting this
rewrite in the time remaining in this session risked exactly the kind of
under-verified change this mission exists to prevent. R2A's durable
foundation is real and ready for R2B/R2C to build on in a focused follow-up
patch with its own dedicated verification budget.

### R3 — retry/restart idempotency: reconfirmed, still CODE_CLOSED

Rerun after R1B/R2A (neither touched this scenario's dependencies): `cargo
test -p mqk-daemon --test scenario_strategy_decision_idempotency_01` — 16/16
passed, unchanged, no code modified.

### R4 — integrated M2 finish-line scenario: still BLOCKED

Blocked on R1B's live-activation wiring and R2B/R2C, neither of which exist
yet as production paths. Building it anyway would require reimplementing
the missing production logic inside the test itself, which the mission's
own instructions forbid and which would prove nothing about the real
system (see the original `08_integrated_m2_proof.txt` reasoning, unchanged).

### Corrected M2 Status (final, this session)

```text
M2.1  WIRING_MISSING   dispatch/conflict pipeline CODE_CLOSED (R1B); live activation not wired
M2.2  CODE_CLOSED      unaffected; reinforced by R1B's same-symbol provenance-swap proof
M2.3  WIRING_MISSING   dispatch/conflict pipeline CODE_CLOSED (R1B); live activation not wired
M2.4  CODE_CLOSED      unaffected
M2.5  CODE_CLOSED      unaffected
M2.6  CODE_CLOSED      unaffected; direct proof reconfirmed this session (R3)
M2.7  WIRING_MISSING   durable per-binding schema CODE_CLOSED (R2A); driver/aggregation not wired (R2B/R2C)
M2.8  CODE_CLOSED      unaffected
M2.9  CODE_CLOSED      unaffected

CODE_CLOSED: 6/9   WIRING_MISSING: 3/9 (M2.1, M2.3, M2.7 — each has a real,
                    tested underlying capability; each is missing only its
                    final production-activation splice)
SPEC_DECISION_REQUIRED: 0 (both R1 and R2 resolved by the operator this session)
```

**M2 CODE COMPLETION: NOT CLOSED**, per this controller's own instruction
("Only claim M2 CODE_CLOSED if the real production integrated proof
passes" — R4 did not run). Real, meaningful progress was made this
session: both `SPEC_DECISION_REQUIRED` blockers are resolved with frozen,
documented decisions; the same-symbol multi-strategy dispatch pipeline is
implemented and proven correct at the production-function level (R1B); the
durable per-binding state foundation for multi-symbol completed-bar
dispatch is implemented and proven (R2A). What remains for full M2 closure
is narrowly scoped and precisely identified: R1B's live-daemon-start-
sequence splice (needs its own evidence/status design, explicitly deferred
by R1A), and R2B/R2C's driver/coordinator rewrite (needs its own dedicated
verification budget given its safety-critical nature). **M2 operational
acceptance remains unclaimed.** M1 operational blocker (deployment
authority gap), the `intraday_scalper` promotion rejection, and alpha-
discovery deferral are all unchanged.

### R5 — `V4-STAGE-B-M2-C1-C3-FINAL-01`: C1/C2/C3 live-activation splice closed

Superseded status above (R1B/R2A era): M2.1/M2.3's "live activation not
wired" gap was closed by a follow-on controller
(`V4-STAGE-B-M2-C1-C3-REPAIR-04`, commits `97a9af5c`/`b0e957b5`/`ac7b400d`)
and this session (`0af79883`/`420c0690`) — see
`docs/CURRENT_MISSION.md` §-6 for the full record. A subsequent independent
review found two deterministic manifest/gitattributes governance defects in
this same commit range (migration 0073 omitted from `manifest.json` and
`.gitattributes`, both CI-breaking) plus a scanner_rank narrowing-cast risk
disposed as already-safe-and-proven; see `docs/CURRENT_MISSION.md` §-6a
(`V4-STAGE-B-M2-C1-C3-CORRECTION-02`) for the full record. Summary:

- `state/lifecycle.rs`'s real `start_execution_runtime` routes a configured,
  approved `watchlist-v3` artifact under `PaperEnforced` to
  `build_explicit_multi_strategy_start_snapshot`: canonical v3 config
  resolution -> real per-binding `evaluate_candidate` DB evidence gate ->
  durable authority build+persist (`sys_explicit_multi_strategy_authority`
  + `_bindings`, migrations 0072/0073) -> true read-side validation
  (`explicit_multi_strategy_evidence_validator.rs`, recomputes `authority_id`
  from durable stored facts, never trusts the stored column) -> isolated
  `StrategyHost` pool construction, strictly in that order (host pool is
  never built before validation passes).
- The STRATEGY-DORMANCY-01 gate (native `MQK_STRATEGY_IDS` bootstrap
  required for Paper+Alpaca autonomous operation) is bypassed only for an
  approved v3 artifact under `paper_enforced` — this session closed the one
  remaining caller (`autonomous_runtime_context::resolve_autonomous_runtime_context_from_fleet`,
  used by the daily coordinator/completed-bar-task/operator-retry paths)
  that still carried the pre-bypass interpretation.
- Read-only status (`routes/dynamic_selection_evidence.rs`) truthfully
  distinguishes `explicit_watchlist_v3_multi_strategy` from
  `bundle7_dynamic_selection`/`legacy` and never fabricates a Bundle-7
  `committed_plan_id` for the explicit-v3 mechanism.
- Known, deliberately unchanged: `autonomous_completed_bar_driver.rs`'s
  single-effective-binding restriction (R2B/R2C) is not on this path (it
  drives a separate, legacy single-binding autonomous dispatch mechanism);
  still requires its own dedicated verification budget as a future patch.

```text
M2.1  CODE_CLOSED      dispatch/conflict pipeline + live activation both closed
M2.2  CODE_CLOSED      unaffected
M2.3  CODE_CLOSED      dispatch/conflict pipeline + live activation both closed
M2.4  CODE_CLOSED      unaffected
M2.5  CODE_CLOSED      unaffected
M2.6  CODE_CLOSED      unaffected
M2.7  WIRING_MISSING   durable per-binding schema CODE_CLOSED (R2A); driver/aggregation still not wired (R2B/R2C)
M2.8  CODE_CLOSED      unaffected
M2.9  CODE_CLOSED      unaffected

CODE_CLOSED: 8/9   WIRING_MISSING: 1/9 (M2.7 — R2B/R2C, out of scope for this controller)
```

**M2 CODE COMPLETION: still NOT CLOSED** (M2.7's driver/aggregation rewrite
remains). C1/C2/C3 (the watchlist-v3 canonical authority / durable
authority identity / real daemon-start activation chain this controller was
scoped to) are each independently `CODE_CLOSED` pending independent review.
M2 overall (C4/C5/C6, i.e. R2B/R2C and the integrated M2 finish-line proof)
remains open. M1 operational status, `intraday_scalper` promotion
rejection, and alpha-discovery deferral are unchanged. No Paper/Live/
runtime state modified; no push; no M3.
