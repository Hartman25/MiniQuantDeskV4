<p align="center">
  <img src="assets/logo/Veritas Ledger.png" alt="Veritas Ledger" width="520">
</p>

<p align="center">
  <strong>Deterministic, risk-first execution and capital allocation framework</strong><br/>
  Rust core • explicit lifecycle • DB-backed safety • scenario-tested proof lanes
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-stable-orange?logo=rust" />
  <img src="https://img.shields.io/badge/Execution-deterministic-purple" />
  <img src="https://img.shields.io/badge/Proof-DB--backed-blue" />
  <img src="https://img.shields.io/badge/Status-autonomous%20paper%20hardening%20%7C%20live%20not%20ready-orange" />
</p>

## Overview

Veritas Ledger is a structured quantitative trading platform built around one principle:

> **Capital protection is a systems problem.**

This repo is not a signal toy and not a broker-click wrapper.
It is a deterministic execution spine designed to enforce explicit lifecycle control, durable state, fail-closed behavior, restart discipline, and truthful operator surfaces under hostile assumptions.

It is built for:

- traders who want institutional structure instead of ad hoc scripts
- developers building serious trading infrastructure
- systematic workflows that need deterministic replay, bounded state transitions, and durable auditability

The system is engineered assuming that:

- market data can be stale, missing, or internally inconsistent
- broker events can drift, duplicate, gap, or arrive out of order
- orders can partially fill at the worst possible boundary
- processes can restart during submit, ack, or fill windows
- humans can misconfigure the control plane

Safety is enforced architecturally, not socially.

## What the repo is today

MiniQuantDeskV4 has real institutional bones and a materially stronger proof posture than scaffold-stage trading repos.

**Repository snapshot used for this update (2026-07-20):** local `main` at
`3591064a805efc82b3f6468e1de0fe06ea028471`
(`docs: require coverage authority before bar processing`), plus independent
ChatGPT/operator acceptance of D4
(integrated preopen-to-shutdown lifecycle proof and completed-bar
dispatch-ownership race closure) and its evaluation-lineage repair
(durable claim-to-evaluation binding, corrected fixtures, injected-clock
supervised-task proof) together — **Phase D is now accepted complete in
full** — plus independent acceptance of the four-times-corrected
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-DURABLE-OUTCOME-AUTHORITY-AND-EVIDENCE-CONTRACT
(Phase E1: a read-only architecture audit producing the binding
durable-outcome/no-trade contract for Phase E) — **E1 is now accepted
complete** — plus the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-FOUNDATION
patch on top of it: the first Phase E runtime code — a durable,
operation-scoped `autonomous_daily_coverage_bound` evidence event, its
canonical construction/write/re-read/replay/conflict contract, the
coordinator's ensure-authority seam, the completed-bar adapter's mandatory
per-tick authority/mid-day-drift gate, and a raw full-run-lineage read/
validate helper — followed by the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-AUTHORITY-ENVELOPE-GATE-ORDERING-AND-CONCURRENCY-CLOSURE
repair on top of that: a complete durable event-envelope validator (id,
event_type, source, `run_id IS NULL`, `resume_source IS NULL` — not merely
the id and JSON detail), a duplicate-JSON-key-rejecting typed parser, the
adapter's authority gate reordered strictly before any assignment/identity
resolution (a missing anchor is now a quiet no-op even under a locally
malformed environment), and a live `tokio::join!`-driven coordinator/adapter
concurrency proof — followed by the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-SAME-INSTANT-CONCURRENCY-AND-SIDE-EFFECT-PROOF-01
final proof repair on top of that: the closure repair's own concurrency test
drove the coordinator and adapter tasks at two *independently timestamped*
ticks rather than one shared logical instant, so it never actually proved the
adapter observes the coordinator's own newly-created operation as of the
coordinator's own `now_utc`. The same test is rewritten in place to use one
shared `now_utc`, a full durable before/after snapshot (state, state_version,
run_id, bar/claim/lifecycle-event/coverage-event/evaluation/decision counts)
proving zero side effects while the coordinator is paused, a
deliberately-invalid-registry-path proof that the adapter never touches the
instrument registry before its authority gate resolves, and a
release-then-normal-progression proof. No production coordinator/state file
was touched by this repair. **E2A (plus both repairs) is now accepted
complete.** On top of that accepted foundation, the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-FINALIZATION-CAS
patch implements the strict evidence classifier and durable finalization CAS:
a pure global-precedence classifier consuming E2A's coverage-anchor and
run-lineage authorities (never re-deriving them), the terminal finalization
CAS (`outcome`/`finalized_at_utc` set atomically with the terminal state
transition, generic `completed` and `no_trade_reason` both structurally
unreachable), the two new `stopping`/`stop_retrying -> evidence_degraded`
legal edges, and the commit-uncertainty-safe database-failure contract —
followed by the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-UNCERTAINTY-CLOSURE
repair on top of that: a single shared pure validator now enforces the exact
authorized terminal state/outcome pairing (a cross-paired combination, e.g.
`completed_no_trade` with `activity_fill_confirmed`, was previously accepted
by the finalization CAS's legality check); `AlreadyApplied` replay now
requires complete durable terminal truth (`finalized_at_utc` present, no
residual `state_reason_code`/`state_blocker_signature`), not merely a
matching state/outcome; the high-level entry point distinguishes a
manual/administrative generic `completed` row (read-only, never rewritten)
from a malformed automatic terminal row (`Conflict`, never accepted as
truth); the coverage-missing precedence no longer special-cases an empty run
lineage (always `unknown_incomplete_bar_coverage`); and the commit-
uncertainty re-read now requires `state_version` to have strictly advanced
past this attempt's own expected version, proven end-to-end against a real
database through a narrow injected effect seam (no mocked successful write)
covering commit-acknowledgment-loss, genuine CAS staleness, and a genuine
conflicting concurrent writer, plus a real partial-evidence-read-failure
proof distinct from the prior mislabeled identity-unavailable test.
**E2B is now accepted complete.** On top of that accepted foundation, the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-INTEGRATION-AND-NOTIFICATION
patch wires the accepted E2B finalizer into the durable daily coordinator's
`handle_stopping`/`dispatch_by_state` routing (invoked at most once per
eligible tick, gated on a durable `stopped_at_utc` and the real
`AppState::locally_owned_run_id()`-vs-`operation.run_id` matching-runtime
fact — never a process-local counter), resolves E2B's current policy inputs
from the same production `build_multi_symbol_runtime_config_from_env`/
`resolve_autonomous_runtime_context`/readiness-context seams
`ensure_coverage_authority` already uses, routes post-stop
`evidence_degraded` operations through E2B's existing recovery edge, projects
every E2B result into six new bounded typed coordinator outcomes, and sends
exactly one outcome notification per newly finalized operation plus exactly
one warning per newly applied finalization evidence blocker — both gated on
durable CAS-derived facts, never process-local memory. A real defect found
only by running the new integration test suite against a live coordinator
tick was fixed along the way: the coordinator's resolution-failure fallback
path could silently overwrite E2B's own durable evidence-degraded reason
with an unrelated one on every subsequent tick; it now routes back into the
same finalization seam instead. A new scenario test file
(`scenario_autonomous_daily_outcome_coordinator_integration_01.rs`, 15
tests as originally accepted — 14 DB-backed `#[ignore]` integration tests
plus one non-DB source-level unit test — all real-DB-backed against the real
production coordinator/finalizer seams with a loopback Discord sink, no
real network call) proves clean no-trade and fill-confirmed finalization,
notification dedup, restart safety, evidence-degraded recovery, and the
resolution-failure fallback path — all 15 pass.

A follow-on repair, AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-
POLICY-FAILURE-GATE-REPAIR-01, closed a second confirmed defect: E3's own
resolution-failure branches in `handle_outcome_finalization` computed the
matching-local-runtime fact but never consulted it before persisting an
`evidence_degraded` blocker, so a matching local runtime could still be
incorrectly overridden whenever current policy/config resolution also failed
the same tick. The coordinator now returns `AwaitingOutcomeFinalization`
before any policy resolution or blocker persistence is attempted whenever a
matching local runtime is active, and the policy-failure blocker-persistence
wrapper (`persist_autonomous_daily_finalization_blocker`) independently
refuses to write (returns `NotEligible`) under the same condition, as
defense-in-depth. Two new tests
(`ci_03b_matching_local_runtime_blocks_policy_failure_without_write_or_notification`
in the E3 coordinator test file, bringing it to 16 tests, all passing; and
`store_59_persist_finalization_blocker_refuses_when_matching_runtime_active`
in the E2B classifier/finalization test file) prove zero write and zero
notification for this case end-to-end. **E3 is now accepted** (plus this
repair, both accepted together). On top of that accepted foundation, the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-PROJECTION
patch adds exactly two strictly read-only routes —
`GET /api/v1/autonomous/daily-operation[?market_date=]` and
`GET /api/v1/autonomous/daily-operations[?limit=]` — projecting already-durable
outcome truth (never rerunning the classifier or finalizer), full-run-lineage
activity counts (strategy evaluations, order activity, fills — never a false
zero on an unreadable lineage), and one shared pure projection reused by a
new additive `daily_operation` summary block on the readiness, paper-status,
and preflight responses. A follow-on
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-TRUTH-AND-EVIDENCE-STATE-REPAIR-01
repair closed five read-truth defects independent review found in that
implementation: the terminal projection could report `evidence_state =
"complete"` without consulting the activity-count outcome at all; a
downstream count-read failure still left the top-level `truth_state` at
`"active"` on the single/history/summary surfaces; generic administrative
`completed` was given the same evidence-complete treatment as the two
automatic classifier terminal states; the malformed-`market_date` 400
response echoed the raw, unbounded caller-controlled query value; and none
of the above had test or guard coverage. The scenario test file
(`scenario_autonomous_daily_operation_api_01.rs`, now 43 tests: 12 non-DB
router-level tests plus one non-DB structural scan, run every time, plus 30
DB-backed proofs) proves truth-state vocabulary, terminal/nonterminal
projection honoring the activity-count outcome, lineage-scoped counts,
history ordering/limit-clamping, summary-block fail-soft behavior, downstream
count-read-failure truth-state demotion, and zero operation/run/outbox/
inbox/claim/evaluation side effects from either route — all 43 pass. **E4
(plus this repair and its test suite) is implementation complete, awaiting
ChatGPT and operator acceptance.** No GUI surface exists yet — that is Phase
F, not started here.

The strongest current operational route is:

- **deployment mode:** `paper`
- **adapter:** `alpaca`
- **current supported operating lane:** long-only, single-symbol US equity/ETF paper trading
- **operator surface:** daemon + Vite GUI
- **proof posture:** targeted DB-backed scenario tests, repo guards, GUI/daemon contract checks, and the repo-native full proof runner

What that means in plain English:

- the canonical **Paper + Alpaca** path is the only route being advanced toward daily autonomous operation
- strategy promotion and daily-data readiness now fail closed before paper order-producing logic can proceed
- durable daily-operation identity, retry, recovery, and stop authority are implemented
- production `main.rs` starts the supervised completed-bar task instead of the legacy blind-timer ticker; the legacy ticker (`state::autonomous_bar_ticker`) remains in source for compatibility tests only and is never spawned in production
- D3 (completed-bar task terminal supervision, durable critical-outcome handling, task-level exactly-once proof) is accepted complete
- D4 (integrated preopen-to-shutdown lifecycle proof, plus closing a confirmed completed-bar dispatch-ownership race against the ordinary execution loop) is **accepted complete**
- a follow-on D4 repair (evaluation-lineage binding: a completed dispatch claim now durably records and confirms the exact strategy-evaluation row that proves it ran, never `None`; the completion write's outcome is honored instead of ignored; the concurrency proof's decoy fixture and the full-day preopen fixture were both corrected; a supervised-task proof under an injected clock was added) is **accepted complete** — **Phase D is accepted complete in full**
- Phase E1 (a read-only architecture audit producing the binding durable daily outcome/no-trade contract for Phase E, four-times-corrected) is **accepted complete**
- Phase E2A (the durable coverage-anchor and run-lineage evidence foundation: the `autonomous_daily_coverage_bound` event, the coordinator ensure-authority seam, the completed-bar adapter's mandatory authority/mid-day-drift gate, and the raw run-lineage read/validate helper), plus its AUTHORITY-ENVELOPE-GATE-ORDERING-AND-CONCURRENCY-CLOSURE and SAME-INSTANT-CONCURRENCY-AND-SIDE-EFFECT-PROOF-01 repairs, is **accepted complete**
- Phase E2B (the strict evidence classifier consuming E2A's authorities, the terminal finalization CAS, the two new `evidence_degraded` post-stop edges, and the commit-uncertainty/database-failure contract) is **accepted complete**
- Phase E3 (coordinator finalization integration: wiring the accepted E2B finalizer into the durable daily coordinator's routing, current policy-input resolution, evidence-degraded recovery routing, six new bounded typed coordinator outcomes, and the E1 §12 outcome/evidence-degraded-warning notifications) is **accepted complete**
- Phase E4 (the strictly read-only daily-operation API projection: `GET /api/v1/autonomous/daily-operation[s]`, full-run-lineage activity counts, and the additive `daily_operation` summary block on readiness/paper-status/preflight), plus its READ-TRUTH-AND-EVIDENCE-STATE-REPAIR-01 repair (terminal evidence state now honors the activity-count outcome instead of always reporting `complete`; a downstream count-read failure now demotes the top-level `truth_state` to `query_failed`; generic administrative `completed` no longer reports evidence as complete; the malformed-`market_date` 400 response no longer echoes raw input), is implementation complete, awaiting ChatGPT and operator acceptance; no GUI surface exists yet
- Bundle 3 is still **open** — E5 integrated Phase E closure, GUI/runbook/soak preparation, and closure audit all remain
- paper+paper is not treated as an authoritative execution path
- backtest deployment through the daemon is intentionally refused fail-closed
- live-shadow and live-capital remain outside the current operational finish line

### Current readiness boundary

Use these labels precisely:

| Mode | Current posture | Meaning |
|---|---|---|
| **Supervised Paper + Alpaca** | Available for controlled validation | Credible current path after a clean proof run, valid env, Alpaca paper auth, and active operator supervision. |
| **Autonomous Paper + Alpaca** | Pre-soak hardening — Bundle 3 open | The durable daily controller, completed-bar production cutover, and the D4 integrated lifecycle/dispatch-ownership proof are implemented and **accepted** (Phase D accepted complete in full); the E1 durable-outcome contract, the E2A durable evidence foundation, the E2B strict classifier/finalization CAS, and the E3 coordinator finalization integration are **accepted**; Bundle 3 still requires the E4 read-only API's own acceptance (implementation complete, awaiting ChatGPT and operator acceptance) and E5 integrated Phase E closure, operator/runbook preparation, and final closure before it should begin an unattended autonomous soak. Controlled autonomous Paper + Alpaca operation under active operator supervision is the current Bundle 3 target — not unattended soak. |
| **Live / live-capital** | Not ready | Typed support and gates exist, but this repo must not be treated as safe for unattended live trading. |

### Current Bundle 3 position

`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED` is the active bundle.

Already implemented and **accepted** (D1–D4, Phase D accepted complete in full):

- authoritative session planning and durable daily-operation identity
- restart-safe current-state and append-only transition evidence
- typed start/recovery/stop retry behavior
- canonical start through `AppState::start_execution_runtime`
- exact completed-bar detection and durable exactly-once dispatch claims
- safe recovery and nontrading-day reconciliation
- durable blocker signatures and operator-facing lifecycle truth
- the production cutover from the legacy bar ticker to the supervised completed-bar task, with bounded restart, durable permanent-failure degradation, and sticky operator-visible task-failure truth
- closed a confirmed completed-bar dispatch-ownership race: the completed-bar driver's durable claim dispatches through the exact-input strategy-dispatch seam directly instead of round-tripping through the shared account-wide pending-bar mailbox the ordinary execution loop also drains every tick, so a concurrent execution-loop tick can no longer cause a real evaluation to be recorded as a failed claim, plus a deterministic concurrency proof for that fix (both interleaving orderings) and one integrated scenario test driving a synthetic Paper+Alpaca day through preopen, canonical start, running dispatch, runtime interruption/recovery, session close, and shutdown together
- a completed dispatch claim durably stores and confirms the exact `strategy_signal_evaluations` row that proves it ran (a shared deterministic identity helper, never a second algorithm); the completion write's `Ok(false)`/`Err` outcomes are honored via one authoritative re-read instead of being silently treated as success; the full-day lifecycle test's preopen phase resolves through real production readiness truth instead of a manual unstick workaround; a supervised-task proof under an injected clock
- the four-times-corrected Phase E1 durable-outcome/no-trade contract audit (outcome authority, finalization eligibility, terminal-state semantics, activity/no-trade evidence hierarchies, an `unknown_insufficient_evidence` representation requiring no schema migration, evidence-conflict precedence, a restart/idempotency contract, a bounded reason-code matrix, a read-only API contract, a notification contract, and the E2A/E2B decomposition) — **E1 is accepted complete**; no Phase E runtime code was written by E1 itself
- Phase E2A, plus its AUTHORITY-ENVELOPE-GATE-ORDERING-AND-CONCURRENCY-CLOSURE and SAME-INSTANT-CONCURRENCY-AND-SIDE-EFFECT-PROOF-01 repairs (the typed, schema-versioned `autonomous_daily_coverage_bound` payload model/parser/semantic-equality comparison; canonical side-effect-free first/final dispatchable-bar construction; a complete durable event-envelope validator; a duplicate-JSON-key-rejecting typed parser; the exact write/re-read/idempotent-replay/conflict authority contract; the coordinator's ensure-authority seam with a pristine-bind/prior-activity fail-closed split; the completed-bar adapter's two-stage authority gate and mid-day drift check; a raw run-lineage read/validate helper; and a live, same-instant `tokio::join!`-driven coordinator/adapter concurrency proof) — **E2A is accepted complete**; no outcome classifier and no finalization behavior were written by E2A itself

**Accepted complete**
(AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-FINALIZATION-CAS):

- a durable evidence snapshot model plus an async evidence-gathering pass (`gather_autonomous_daily_outcome_evidence`) that performs every database read for one classification attempt up front — the coverage anchor via E2A's own `check_coverage_authority`, the full run lineage via E2A's own validated-lineage helper, the exact expected dispatch-bar set derived purely from the immutable anchor, every expected bar's durable dispatch claim and evaluation row, and every `oms_outbox`/`oms_inbox` row across the complete validated run lineage (two new narrow, unbounded, any-status/any-event-kind read helpers)
- a pure global-precedence classifier (`classify_autonomous_daily_outcome`) applying the E1 contract's exact ten-step order over that snapshot with zero I/O and zero access to any process-local diagnostic counter — structurally impossible, not merely avoided by convention
- four closed terminal reason codes (`activity_fill_confirmed`, `activity_order_submitted`, `activity_decision_accepted`, `no_trade_strategy_evaluated_no_signal`) and eight closed nonterminal `unknown_*` reason codes; generic `completed` is not a representable classifier output at all
- the terminal finalization CAS (`mqk_db::finalize_autonomous_daily_operation`): sets `state`/`outcome`/`finalized_at_utc` atomically in the same `UPDATE`, clears stale blocker/retry evidence, rejects generic `completed` and any outcome outside the closed four-code set before touching SQL, and never writes the retired `no_trade_reason` column — no migration
- two new legal transition edges (`stopping`/`stop_retrying -> evidence_degraded`, per the E1 contract §3.3), the durable evidence-degraded blocker write reusing the existing D1 blocker-signature mechanism verbatim, and the pre-existing `evidence_degraded -> stopping` edge as the sole recovery path (never finalizing directly from `evidence_degraded`)
- a commit-uncertainty-safe write discipline for every CAS (finalization and blocker alike): an ambiguous write result always triggers one authoritative re-read before any success is claimed, exactly mirroring D4's dispatch-completion confirmation pattern; complete database outage performs zero write attempts, and a partial evidence-read failure's best-effort blocker write is only ever claimed durable after a confirming re-read
- a high-level `classify_and_finalize_autonomous_daily_operation` entry point, callable from a production tick since E3 wired it in (below)
- a new scenario test file (`scenario_autonomous_daily_outcome_classifier_and_finalization_01.rs`, 66 tests: 26 pure classifier scenarios, 4 pure eligibility proofs, 33 DB-backed finalization/blocker CAS store proofs, and 3 DB-backed integrated end-to-end proofs including a full unresolved-claim → degrade → repair → recover → finalize round trip) — all 66 pass

**Accepted complete**
(AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-INTEGRATION-AND-NOTIFICATION):

- `handle_stopping`'s stop-completion no-op now routes into a new `handle_outcome_finalization` seam once `stopped_at_utc` is durable — reached from both `dispatch_by_state`'s ordinary per-tick routing and `reconcile_existing_operation_against_relevant_lookup`'s fallback-lookup routing, so a stopped operation is never abandoned regardless of which path finds it
- the matching-local-runtime fact (E1 §3.2 condition 4) is computed from `AppState::locally_owned_run_id()` compared against `operation.run_id` — never a process-local counter
- E2B's current policy inputs are resolved fresh, once per attempt, from the exact same `build_multi_symbol_runtime_config_from_env`/`resolve_autonomous_runtime_context`/readiness-context seams `ensure_coverage_authority` already uses — no second parser, no cached policy; a resolution failure persists `unknown_assignment_identity_unavailable` through one new narrow `pub` wrapper (`persist_autonomous_daily_finalization_blocker`) around E2B's own existing blocker-CAS machinery
- post-stop `evidence_degraded` operations route into the same finalization seam for recovery-or-replay — a real defect found only by running the new integration suite against a live coordinator tick was fixed along the way: the pre-existing resolution-failure fallback path could silently overwrite E2B's own durable evidence-degraded reason with an unrelated one on every subsequent tick
- `AutonomousDailyFinalizationOutcome::EvidenceDegraded` gained a `newly_applied: bool` field, threaded through every CAS branch (a fresh transition/refresh is `true`; an exact replay or an ambiguous-write re-read is always `false`) — the sole, durable-CAS-derived dedup authority for the new warning notification
- six new bounded typed `AutonomousDailyCoordinatorTickOutcome` variants project every one of E2B's seven results; `session_controller.rs`'s `log_coordinator_outcome` gained an explicit (non-wildcard) arm for each, sending exactly one outcome notification for a newly finalized operation and exactly one warning for a newly applied evidence blocker — both gated on durable facts, never process-local memory; a database-unavailable or conflicting-terminal-truth result never notifies
- a new scenario test file (`scenario_autonomous_daily_outcome_coordinator_integration_01.rs`, 16 tests, all real-DB-backed against the real production `run_durable_session_controller_tick` seam with a loopback Discord sink, no real network call) proving clean no-trade/fill-confirmed finalization, exactly-once finalization and notification, restart safety, evidence-degraded recovery, and the resolution-failure fallback path — all 16 pass
- no API route and no GUI surface were added by E3 itself — E4 (below) adds the read-only API; GUI remains Phase F's job

Implemented on the local `main` worktree
(AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-PROJECTION),
but not yet independently accepted:

- exactly two strictly read-only public routes: `GET /api/v1/autonomous/daily-operation[?market_date=]` (exact-slot lookup via `mqk_db::fetch_autonomous_daily_operation_for_slot`, default market date resolved via the same pure `resolve_autonomous_daily_session_plan_from_env` the coordinator uses) and `GET /api/v1/autonomous/daily-operations[?limit=]` (via `mqk_db::list_recent_autonomous_daily_operations`, limit clamped `[1,100]`) — no mutating method mounted on either
- one shared pure projection function used by both routes and every summary block — terminal `outcome_class`/`outcome_reason_code`/`finalized_at_utc` are read verbatim from the already-durable row (never a classifier rerun); nonterminal rows always project `null` for those three fields
- the full `active`/`not_found`/`backend_unavailable`/`query_failed` truth-state vocabulary (plus `invalid_request` for a malformed `market_date`, the only non-200 case)
- full-run-lineage activity counts (`strategy_evaluation_count`/`order_activity_count`/`fill_count`) via `mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage` plus one new narrow, unbounded `mqk_db::count_strategy_signal_evaluations_for_runs` helper (no migration) — an unreadable/contradictory lineage or a downstream read failure always yields `null` counts, never a false zero
- an additive `daily_operation` summary block on `AutonomousPaperReadinessResponse`/`AutonomousPaperStatusResponse`/`PreflightStatusResponse`, supplied by every response-construction branch in all three handlers, computed by a function that structurally cannot change its caller's HTTP status or any other field on a daily-operation DB failure
- a new scenario test file (`scenario_autonomous_daily_operation_api_01.rs`, 37 tests: 11 non-DB router-level/structural tests plus 26 DB-backed proofs of truth-state vocabulary, terminal/nonterminal projection, lineage-scoped counts, history ordering/limit-clamping, summary-block fail-soft behavior, and zero operation/run/outbox/inbox/claim/evaluation side effects) — all 37 pass
- **no GUI surface was added** — that remains Phase F's job

Still required before Bundle 3 closes:

1. independent ChatGPT/operator acceptance of the E4 read-only daily-operation API
2. implement E5 integrated Phase E proof and closure
3. finish Phase F GUI, runbook, and soak-evidence preparation
4. complete Phase G closure audit and ledger reconciliation

### What Bundle 3 completion unlocks

After Bundle 3 is closed, the intended result is a daemon that can remain running across supported
NYSE sessions and autonomously:

- prepare and verify daily market data
- create or recover one durable daily paper operation
- arm and start only through the canonical safety gates
- process only the exact expected completed bar
- dispatch that bar at most once
- retry transient failures with bounded backoff
- recover safely after a daemon/runtime interruption
- stop the matching owned runtime at the session boundary
- record a durable daily activity or no-trade outcome
- expose truthful operator status and evidence

That is the point where the project can begin a **supervised autonomous Paper + Alpaca soak**.
It is not permission to use live capital. The planned operating sequence is to collect roughly
10–20 clean autonomous paper sessions, review real order/fill/reconcile/notification evidence,
and complete durable paper portfolio/P&L truth before broader rollout.

**Historical smoke status (2026-06-15):** the earlier AAPL/5m smoke proved readiness and a durable
no-trade reason for `intraday_scalper`; it did not close real order/fill lifecycle, reconcile after
a real fill, Discord trade-lifecycle evidence, or repeated autonomous cycles.

**Historical full-proof status (2026-06-01):** `full_repo_proof.ps1 -ProofProfile full -LowMemory`
passed 18/18 lanes at that snapshot. Current `main` has advanced materially since then; this README
does not claim that the 18/18 transcript independently re-proves commit `7cff4592`.

The proof harness can prove a locked repository scope. It does **not** prove profitability,
broker correctness, or live-capital readiness.

## Architecture

<p align="center">
  <img src="assets/diagrams/architecture.svg" alt="MiniQuantDeskV4 architecture" width="960" />
</p>

### High-level flow

Market data / broker snapshots / research artifacts  
→ canonical ingest + quality gates  
→ deterministic backtest / replay / promotion evidence  
→ integrity + risk gates  
→ execution boundary  
→ durable outbox / broker / durable inbox / OMS  
→ portfolio + reconcile  
→ operator control plane (CLI / daemon / GUI)

### Load-bearing subsystems

| Layer | Purpose |
|---|---|
| **Market data ingest** | Canonical `md_bars` ingest, provider/CSV support, and quality reporting. |
| **Backtest / replay** | Deterministic replay with conservative semantics and promotion-oriented evidence paths. |
| **DB + lifecycle enforcement** | Durable run state, outbox/inbox truth, broker mapping, and lifecycle constraints. |
| **Integrity + risk gates** | Stale feed, gap, disagreement, limits, halt, and risk-cap enforcement before execution. |
| **Execution boundary** | Intent-to-order constraint enforcement, OMS transitions, cancel/replace discipline. |
| **Reconcile** | Snapshot normalization, drift detection, and start/arm gating tied to durable truth. |
| **Control plane** | CLI, HTTP daemon, GUI, audit/event surfaces, and restart-intent operator workflows. |

## Core characteristics

| Property | Description |
|---|---|
| **Deterministic** | Same inputs should produce the same replay, artifacts, and constrained execution decisions. |
| **Risk-first** | Integrity and risk gates sit in front of the execution boundary, not behind it. |
| **Lifecycle-controlled** | Runs move through explicit status transitions instead of ad hoc process state. |
| **OMS-governed** | Order lifecycle transitions are constrained by an explicit state machine. |
| **DB-enforced where it matters** | Durable outbox/inbox, lifecycle, broker identity mapping, cursor state, and operator truth are persisted where the readiness bar requires it. |
| **Scenario-tested** | Reliability work is backed by adversarial scenario tests and proof lanes, not comments or happy-path demos. |
| **Fail-closed** | Missing authority, invalid mode/adapter combinations, and unsafe control-plane actions are refused rather than guessed. |
| **Operator-honest** | Daemon and GUI are being hardened as truth surfaces, not decorative dashboards. |

## Repository structure

```text
core-rs/
  crates/
    mqk-config
    mqk-db
    mqk-audit
    mqk-artifacts
    mqk-cli
    mqk-testkit
    mqk-execution
    mqk-portfolio
    mqk-risk
    mqk-integrity
    mqk-reconcile
    mqk-strategy
    mqk-backtest
    mqk-promotion
    mqk-broker-paper
    mqk-broker-alpaca
    mqk-daemon
    mqk-runtime
    mqk-md
    mqk-isolation
    mqk-schemas

  mqk-gui/

research-py/
config/
scripts/
docs/
assets/
```

Rust is the authoritative execution and control layer.
Python research is optional and is intended to emit deterministic artifacts that the Rust spine can consume.

Operationally, `MAIN` is the canonical engine.
`EXP` is a research-side experimental sandbox and is not part of readiness or operator-truth claims unless explicitly promoted.

## What is strong right now

### Core platform

- deterministic Rust workspace with explicit execution boundaries
- DB-backed lifecycle and execution-path safety model
- authoritative local proof runner: `full_repo_proof.ps1`
- repo-native DB proof harness and mandatory DB matrix
- scenario-driven reliability validation across runtime, execution, DB, broker, and daemon surfaces
- guard rails for unsafe patterns, ignored-proof hygiene, migration governance, workspace dependency inheritance, and GUI/daemon contract drift

### Market data and readiness

- canonical `md_bars` ingest
- CSV and provider ingestion paths
- incremental provider sync support
- data-quality reporting artifacts
- stale / gap / incomplete-bar handling in the integrity path
- one shared daily-data-readiness evaluator used by start/preflight/operator surfaces
- explicit provider-call authorization; trusted local exact bars remain usable with provider calls disabled

### Backtesting and promotion

- deterministic replay
- conservative ambiguity handling
- promotion-facing infrastructure and artifact checks
- durable strategy-promotion authority for paper order-producing paths
- parity and provenance work is materially stronger than earlier scaffolds

### Execution core

- explicit OMS order state machine
- durable outbox-first submission flow
- durable inbox event ingestion
- idempotent broker-event handling
- broker/internal order identity mapping
- partial-fill-aware cancel / replace handling
- restart and crash-window proof coverage

### Autonomous daily control

- deterministic daily-operation identity and durable state/version authority
- append-only transition evidence
- typed retry, recovery, and stop handling
- nontrading-day recovery and durable ownership checks
- exact completed-bar observation and durable exactly-once dispatch claims, dispatched through the same exact-input strategy-dispatch seam the execution loop uses — no shared-mailbox race with the ordinary execution loop's own per-tick dispatch
- production cutover away from the blind legacy ticker (legacy ticker retained in source for compatibility tests only, never spawned in production)
- fail-closed runtime ownership and blocker-signature handling

### Risk, integrity, and reconcile

- allocation / exposure boundary checks
- stale feed and disagreement controls
- deadman-style enforcement paths
- reconcile normalization and mismatch detection
- arming preflight tied to durable truth
- autonomous paper gating tied to session truth and WS continuity for the canonical Paper + Alpaca route

### Control plane

- CLI workflows for DB, market data, runs, and backtests
- HTTP daemon with readiness, preflight, control, audit, and event surfaces
- persisted restart-intent workflow for admissible mode changes
- Vite/React GUI operator console with a CI-enforced daemon contract gate
- optional Windows desktop bootstrap scripts for a stricter desktop operator path

## What is still partial

Be honest about the open edges.

- Bundle 3 remains open; Phase D (D1–D4, integrated lifecycle proof and dispatch-ownership race closure plus the evaluation-lineage repair) is accepted complete in full; the Phase E1 contract audit (the binding durable outcome/no-trade contract, four-times-corrected), Phase E2A (the durable coverage-anchor/run-lineage evidence foundation, plus its repairs), Phase E2B (the strict outcome classifier and finalization CAS), and Phase E3 (durable daily outcome/no-trade classification runtime coordinator integration and notification) are all **accepted complete**
- the Phase E4 read-only daily-operation API (`GET /api/v1/autonomous/daily-operation[s]` plus the additive readiness/paper-status/preflight summary block) is implementation complete and is the immediate item awaiting independent ChatGPT/operator acceptance; E5 integrated closure, final GUI/runbook/soak preparation (Phase F), and closure audit (Phase G) remain
- Bundle 4 durable paper cash/positions/lots/cost basis/P&L truth has not started — this is required before trusting the accounting of any extended autonomous soak, not merely a nice-to-have
- the current autonomous lane is long-only and single-symbol; multi-symbol rollout is deferred until after the soak
- real paper order/fill/reconcile/Discord evidence is still incomplete
- research → deployability → runtime artifact closure is not fully complete
- live-shadow and live-capital typed support are not the same thing as proven safe live operation
- Alpaca WebSocket gap recovery is still not a complete lifecycle replay story for every non-fill event
- Alpaca REST fill recovery must remain treated carefully until pagination and high-volume recovery are proven end to end
- shadow/live parity evidence is not yet fully surfaced and enforced end to end
- some deeper GUI detail surfaces are intentionally deferred or unmounted rather than faked
- desktop bootstrap exists, but the primary documented operator path remains daemon + browser GUI

## Open autonomous-paper proof items

The long-only single-symbol Paper + Alpaca lane is the current finish line, but the autonomous MVP
should not be called closed until Bundle 3 and its market evidence gates are complete:

| Item | Status |
|---|---|
| BUNDLE-3-AUTONOMOUS-DAILY-OPS | Open — Phase D accepted complete; Phase E1 contract (four-times-corrected) accepted complete; Phase E2A durable evidence foundation, plus its authority-envelope/gate-ordering/concurrency closure repairs, accepted complete; Phase E2B strict outcome classifier and finalization CAS accepted complete; Phase E3 coordinator finalization integration and notification accepted complete; Phase E4 read-only daily-operation API implementation complete, awaiting acceptance; E5 closure runtime, GUI/runbook/soak prep, and closure remain |
| PAPER-TRADE-LIFECYCLE-01 | Open — market-hours paper smoke with real fills |
| RECONCILE-AFTER-REAL-FILL-01 | Open — reconcile pass after a real paper fill |
| DISCORD-TRADE-LIFECYCLE-REAL-01 | Open — Discord notification evidence from a real cycle |
| AUTONOMOUS-PAPER-SOAK | Not started — target roughly 10–20 clean sessions after Bundle 3 closure |
| DURABLE-PAPER-PORTFOLIO-PNL | Open — Bundle 4 |
| PAPER-SMOKE-EVIDENCE-REVIEW-02 | Closed — review tool exists; future smoke evidence still requires review |

These include both code gates and operational evidence gates.

The 2026-06-15 no-trade smoke remains useful historical evidence, but it does not close the
current completed-bar task, real order/fill lifecycle, reconcile-after-fill, Discord real-cycle,
or repeated autonomous-session proof.

### Evidence capture workflow

When Bundle 3 is closed, configured, and inside the session window, the durable autonomous
controller should own starting the paper run. Evidence remains captured using the read-only
`scripts/windows/Capture-PaperSmokeEvidence.ps1` workflow and reviewed with
`scripts/windows/Review-PaperSmokeEvidence.ps1 -Latest -WriteSummary`. The full workflow is
documented in `docs/runbooks/paper_smoke_evidence_pack.md`.

`scripts/windows/Run-AAPL5mMarketSmoke.ps1` remains an optional AAPL/5m helper path. It is not
the sole source of truth; captured daemon/API/DB evidence plus the review script is the evidence
boundary.

Live trading remains locked until repeated clean paper evidence and all later live-capital gates
are satisfied.

## Local setup model

The repo now has a more explicit local Docker/DB split than older docs suggested.

### Runtime/operator DB

For real local daemon, GUI, and autonomous paper work, use a **runtime DB** that matches your local env configuration.

The repo ships `.env.local.example` as the starting point for this workflow.
It defines a default runtime URL of:

```text
MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5432/mqk_dev
```

Many local workflows keep separate runtime, proof, and reality-test DBs.
That separation is healthy.

### Proof DB

For proof work, use a **disposable proof DB** instead of reusing your runtime DB.
The isolated example below binds Postgres to `55432` specifically to avoid collisions with a normal local runtime DB on `5432`.

### Env-file workflow

`mqk-cli` and `mqk-daemon` will auto-load `.env.local` from the **current working directory**.

Practical implication:

- if you launch from the repo root, a repo-root `.env.local` is picked up automatically
- if you launch from `core-rs/`, place a copy at `core-rs/.env.local` or export the env vars in your shell

The Windows desktop launcher and the autonomous reality-test script already look for both repo-root and `core-rs` env files.

## Verification model

This repo does not rely on a single `cargo test` story.

Command verification note for this README refresh:

- `full_repo_proof.ps1` exists at repo root and accepts `-ProofProfile local`, `-ProofProfile full`, `-ProofProfile exploratory`, and optional `-LowMemory`.
- `core-rs/Cargo.toml` is the current workspace manifest.
- `mqk-daemon` is a workspace package with a binary named `mqk-daemon`.
- `core-rs/mqk-gui/package.json` includes `dev`, `test`, and `build` scripts.
- `core-rs/mqk-gui/vite.config.ts` pins the browser dev server to port `1420`, not Vite's usual `5173`.


### Authoritative local proof runner

- `full_repo_proof.ps1 -ProofProfile local` runs the non-DB local lane set
- `full_repo_proof.ps1 -ProofProfile full` runs the DB-backed proof path and requires `MQK_DATABASE_URL`
- `-LowMemory` can be added to any proof profile and reproduces the proven Windows low-memory posture

### Main proof and guard lanes

- **workspace lane** — `fmt`, `clippy`, and broad workspace tests
- **daemon proof lanes** — route truth, token auth, runtime lifecycle, fail-closed boot, and deadman behavior
- **broker lane** — Alpaca adapter contract and inbound lifecycle mapping proof
- **runtime lane** — lifecycle continuity and runtime proof surfaces
- **DB proof lane** — migrations, lifecycle constraints, outbox/inbox durability, restart quarantine, deadman, and broker-map enforcement
- **GUI contract lane** — GUI truth tests, GUI build, and daemon/GUI contract drift checks
- **guard lanes** — unsafe patterns, ignored-proof hygiene, migration governance, and workspace dependency inheritance
- **Windows low-memory parity** — proof posture for the actual operator OS class

That DB-backed lane remains the load-bearing proof surface for the most important durability claims.

### CI vs operator-class local proof boundary (CI-DB-01)

CI runs five jobs on every push:

- **gui-contract** — GUI truth tests, build gate, daemon/GUI contract gate (ubuntu)
- **guards** — safety pattern guards (ubuntu)
- **rust** — fmt + clippy + workspace tests with ephemeral Postgres (ubuntu)
- **db-proof** — DB proof bootstrap and targeted DB-backed safety proof lanes (ubuntu)
- **windows** — fmt + clippy + workspace tests on windows-latest; no Postgres available on GitHub Actions Windows runners, so **DB-backed lanes do not run in CI Windows**

The Windows CI job proves the build is correct on the operator OS class. It does NOT run the full operator-class DB proof. DB-backed proof in CI is run on ubuntu only.

The full operator-class proof — Windows platform + DB-backed lanes together — requires a local run:

```powershell
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full
```

Release and readiness claims require a clean transcript from this local full proof run (or an equivalent operator-class DB proof). CI passing alone does not substitute for it.

## Quick start

### 1. Clone

```powershell
git clone <your-repo-url>
cd MiniQuantDeskV4
```

### 2. Create your local env file

```powershell
Copy-Item .env.local.example .env.local
```

Fill in the values you actually use for local runtime work.
At minimum, that usually means:

- `MQK_DATABASE_URL`
- `MQK_OPERATOR_TOKEN`
- `MQK_DAEMON_DEPLOYMENT_MODE`
- `MQK_DAEMON_ADAPTER_ID`
- `ALPACA_API_KEY_PAPER`
- `ALPACA_API_SECRET_PAPER`

### 3. Start a local runtime DB

Match this to your `.env.local`.
If you keep the example runtime DB URL from `.env.local.example`, a compatible local Postgres looks like this:

```powershell
docker run --name mqk-postgres-dev `
  --restart unless-stopped `
  -e POSTGRES_USER=postgres `
  -e POSTGRES_PASSWORD=postgres `
  -e POSTGRES_DB=mqk_dev `
  -p 5432:5432 `
  -d postgres:16

# If the container already exists, use this instead:
# docker start mqk-postgres-dev

docker exec mqk-postgres-dev pg_isready -U postgres -d mqk_dev
```

### 4. Start a separate local proof DB

```powershell
docker run --name mqk-postgres-proof `
  --restart unless-stopped `
  -e POSTGRES_USER=mqk `
  -e POSTGRES_PASSWORD=mqk `
  -e POSTGRES_DB=mqk_test `
  -p 55432:5432 `
  -d postgres:16

# If the container already exists, use this instead:
# docker start mqk-postgres-proof

docker exec mqk-postgres-proof pg_isready -U mqk -d mqk_test
```

### 5. Run the canonical proof path

```powershell
# Non-DB proof
.\full_repo_proof.ps1 -ProofProfile local

# Full DB-backed proof against the isolated proof DB
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full

# Same full proof using the Windows low-memory posture
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full -LowMemory
```

### 6. Run the daemon from repo root

Running from repo root lets `mqk-daemon` auto-load repo-root `.env.local`.

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --bin mqk-daemon
```

### 7. Run the GUI

```powershell
cd core-rs\mqk-gui
npm ci
npm run dev
```

Open:

- GUI: `http://127.0.0.1:1420`
- Daemon: `http://127.0.0.1:8899`

The GUI defaults to the daemon URL `http://127.0.0.1:8899`. You can override it with `VITE_MQK_DAEMON_URL` or through the GUI's saved daemon URL setting.

## Design philosophy

> **Returns are a strategy problem. Blow-ups are a systems problem.**

Veritas Ledger is engineered primarily to address the second.

## Scope and non-goals

### Within scope

- deterministic backtest replay
- explicit lifecycle enforcement
- durable execution-path truth
- idempotent broker-event handling
- operator/control-plane hardening
- scenario-based reliability validation

### Not promised by this repo

- profitability
- broker correctness
- exchange correctness
- host-level security
- fully hardened secret management
- safe unattended live deployment without stronger parity evidence, deeper runbooks, and additional controls

## Roadmap

Immediate operational sequence:

1. close Bundle 3 autonomous daily paper operations
2. build Bundle 4 durable paper portfolio and P&L truth
3. run and review roughly 10–20 autonomous Paper + Alpaca sessions
4. close real-fill, reconcile, Discord, restart, and repeated-cycle evidence gates
5. only then consider broader symbol coverage or later live-shadow preparation

Items beyond the current long-only single-symbol finish line:

- multi-symbol universe support
- additional strategies only after promotion evidence and operational stability
- full data-ingestion expansion (additional providers, bar types, tick data)
- multi-asset expansion
- trade journal and forensic review surface
- regime attribution and strategy-decay detection
- GUI reskin and multi-monitor operator polish
- live-capital readiness lock, gated on repeated clean paper evidence and all live-capital gates

## Read next

- `README_TECHNICAL.md` — practical setup, proof commands, daemon/GUI startup, and operator boundaries
- `docs/runbooks/autonomous_paper_ops.md` — canonical autonomous paper operations
- `docs/runbooks/operator_workflows.md` — operator control-plane workflows
- `docs/runbooks/live_shadow_operational_proof.md` — current live-shadow proof posture
- `docs/INSTITUTIONAL_READINESS_LOCK.md` — readiness lock and guardrail context
- `docs/INSTITUTIONAL_SCORECARD.md` — scorecard context

## Snapshot and secret hygiene

Never include a real `.env.local`, API keys, operator tokens, Discord webhooks, or broker secrets in repo snapshots, support zips, or AI handoff bundles. `.env.local.example` is safe to share because it contains names/placeholders only; `.env.local` is not safe to share.

If a support snapshot ever included real credentials, rotate them before running broker-connected sessions again.

## Disclaimer

This repository is an engineering framework for systematic capital allocation research and operator-controlled execution. It is not investment advice and should not be treated as a promise of profitability or safe unattended live trading.
