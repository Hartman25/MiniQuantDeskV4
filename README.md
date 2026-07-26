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
(`scenario_autonomous_daily_operation_api_01.rs`, now 50 tests: 19 non-DB
router-level tests, run every time, plus 31 DB-backed proofs) proves
truth-state vocabulary, terminal/nonterminal projection honoring the
activity-count outcome, lineage-scoped counts, history ordering/limit-
clamping, summary-block fail-soft behavior, downstream count-read-failure
truth-state demotion, and zero operation/run/outbox/inbox/claim/evaluation
side effects from either route — all 50 pass. A second follow-on
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-EXACT-MARKET-DATE-PARSER-REPAIR-02
closed the one remaining E4 validation defect: the explicit `market_date`
query branch parsed with `NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")`,
which silently accepted whitespace-padded forms (`" 2026-07-20"`,
`"2026-07-20 "`) even though the frozen route contract requires the exact
`YYYY-MM-DD` lexical form with no normalization. A new pure helper,
`parse_exact_market_date`, replaces it: exact 10-byte length, dash bytes at
positions 4/7, ASCII digits everywhere else, then `chrono` parsing followed
by a canonical `format("%Y-%m-%d")` round-trip check against the raw input —
rejecting whitespace, non-zero-padded fields, sign prefixes, trailing
characters, and Unicode digit lookalikes. The fixed bounded invalid-request
message is unchanged. **E4 (plus both repairs and their test suites) is now accepted.**
On top of that accepted foundation, the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-CLOSURE
patch adds one new integrated scenario test file
(`scenario_autonomous_daily_phase_e_closure_01.rs`, 6 tests, all passing)
proving six end-to-end proofs against the real, isolated test database and
the real production coordinator/finalizer/API seams (fake notifier
instrumentation only, no real provider/broker/Discord/network call): a
clean no-trade day's full pipeline plus replay; an activity day whose
full-lineage counts correctly include an earlier, non-current run's fill and
order evidence; an evidence-blocker notify-once/silent-replay/recovery
cycle; restart safety after a durable stop, after a terminal commit, and
after an evidence blocker (each step using a brand-new `AppState`, this
crate's established restart-proof convention); the E4 routes' full
before/after read-only guarantee across all five GET endpoints; and the
frozen E4 fail-soft truth vocabulary (`not_found`/`backend_unavailable`/
`query_failed`/an invalid-lineage evidence gap/the exact malformed-
`market_date` 400). No production Rust behavior was added or changed by
E5 — every seam it exercises was already accepted by E1–E4. **E5 (plus this
closure) is now accepted — Phase E is accepted complete in full.** On top of
that, `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-
PROJECTION` adds the first Phase F GUI surface: a read-only `Daily
Operations` operator screen consuming the accepted E4 API verbatim (§ below).
A follow-on repair, `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-RUNTIME-SHAPE-
AND-HISTORY-BLOCKER-REPAIR-01`, hardens the GUI mapper's runtime shape
validation (a malformed HTTP 200 body — e.g. `active` with a missing
`operation`, or history with a missing/invalid `rows` array — now fails
closed to `endpoint_unavailable` instead of rendering as false-authoritative
truth) and renders every history row's `evidence_blockers`, which the
original F1 pass omitted. **F1 (plus this repair) is now accepted.** On top
of the accepted F1 head,
`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F2-OPERATOR-RUNBOOK-CORRECTION` corrects
the canonical operator runbook (`docs/runbooks/autonomous_paper_ops.md`) to
cover the durable daily-operation lifecycle: a safety-boundary section, the
five authoritative read-only routes and their full truth-state/finalization-
status/evidence vocabulary, a before-session checklist, during-session
supervision guidance, bounded recovery procedures (never a manual
finalization command or DB rewrite), stop/emergency posture, end-of-day
evidence capture, and restart distinctions. Documentation and validation
only — no daemon, GUI, or migration file is touched. A follow-on repair,
`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-F2-F3-G-FINAL-OPERATIONAL-SAFETY-REPAIR`
(below), corrected supervised-only runbook language and WS-gap restart
guidance found by independent acceptance review. **F2 (plus this repair) is
now accepted.**

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
- Phase E4 (the strictly read-only daily-operation API projection: `GET /api/v1/autonomous/daily-operation[s]`, full-run-lineage activity counts, and the additive `daily_operation` summary block on readiness/paper-status/preflight), plus its READ-TRUTH-AND-EVIDENCE-STATE-REPAIR-01 repair (terminal evidence state now honors the activity-count outcome instead of always reporting `complete`; a downstream count-read failure now demotes the top-level `truth_state` to `query_failed`; generic administrative `completed` no longer reports evidence as complete; the malformed-`market_date` 400 response no longer echoes raw input) and its EXACT-MARKET-DATE-PARSER-REPAIR-02 repair (the explicit `market_date` query branch now uses an exact lexical parser instead of `.trim()`-then-parse, rejecting whitespace, non-zero-padded fields, sign prefixes, trailing characters, and Unicode digit lookalikes), is **accepted complete**
- Phase E5 (the integrated Phase E closure proof: one new scenario test file proving a clean no-trade day, a two-run full-lineage activity day, an evidence-blocker notify-once/silent-replay/recovery cycle, restart safety across a durable stop/terminal commit/evidence blocker, the E4 routes' full read-only guarantee, and the frozen E4 fail-soft truth vocabulary, all against the real test database and the real production coordinator/finalizer/API seams, zero production Rust change) is **accepted complete — Phase E is accepted complete in full**
- Phase F1 (the read-only GUI daily-operation truth projection: strict TypeScript response types mirroring the accepted E4 API, both canonical routes wired into the existing operator-model polling cycle, a dedicated `Daily Operations` operator screen with no mutation controls, null-count-vs-zero handling, and a screen-local source-authority helper), plus its RUNTIME-SHAPE-AND-HISTORY-BLOCKER-REPAIR-01 repair (complete runtime shape validation of both mapper functions — a malformed HTTP 200 body now fails closed to `endpoint_unavailable` instead of rendering false-authoritative truth — plus history-row `evidence_blockers` rendering, which the original F1 pass omitted), is **accepted complete**
- Phase F2 (operator runbook correction: `docs/runbooks/autonomous_paper_ops.md` updated in place with a safety-boundary section, the five authoritative read-only routes and their full vocabulary, before-session checklist, during-session supervision, bounded recovery procedures, stop/emergency posture, end-of-day evidence capture, and restart distinctions — documentation and validation only), plus its final operational-safety repair, is **accepted complete**
- Bundle 3 (D1 through Phase G, including the final guard-and-evidence-integrity repair) is **accepted complete** (independent ChatGPT/operator acceptance received); Bundle 4 (`DURABLE-PAPER-PORTFOLIO-AND-PNL-01-COMBINED`, durable paper portfolio/P&L truth) is closure-implementation complete, awaiting final ChatGPT/operator acceptance, built on Bundle 3's now-accepted foundation
- paper+paper is not treated as an authoritative execution path
- backtest deployment through the daemon is intentionally refused fail-closed
- live-shadow and live-capital remain outside the current operational finish line

### Current readiness boundary

Use these labels precisely:

| Mode | Current posture | Meaning |
|---|---|---|
| **Supervised Paper + Alpaca** | Available for controlled validation | Credible current path after a clean proof run, valid env, Alpaca paper auth, and active operator supervision. |
| **Autonomous Paper + Alpaca** | Pre-soak hardening — Bundle 3 accepted, Bundle 4 awaiting acceptance | Bundle 3 (`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`, D1 through Phase G) is **ACCEPTED — COMPLETE** (independent ChatGPT/operator acceptance received). Bundle 4 (`DURABLE-PAPER-PORTFOLIO-AND-PNL-01-COMBINED`, durable paper portfolio/fill-accounting/P&L truth, B4-0 through B4-G), built on top of Bundle 3's now-accepted implementation, is **closure-implementation complete, awaiting final ChatGPT/operator acceptance** before an unattended autonomous soak may begin. Controlled autonomous Paper + Alpaca operation under active operator supervision remains the current target — not unattended soak. |
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

**Accepted complete**
(AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-PROJECTION,
plus its READ-TRUTH-AND-EVIDENCE-STATE-REPAIR-01 and EXACT-MARKET-DATE-PARSER-REPAIR-02 repairs):

- exactly two strictly read-only public routes: `GET /api/v1/autonomous/daily-operation[?market_date=]` (exact-slot lookup via `mqk_db::fetch_autonomous_daily_operation_for_slot`, default market date resolved via the same pure `resolve_autonomous_daily_session_plan_from_env` the coordinator uses) and `GET /api/v1/autonomous/daily-operations[?limit=]` (via `mqk_db::list_recent_autonomous_daily_operations`, limit clamped `[1,100]`) — no mutating method mounted on either
- one shared pure projection function used by both routes and every summary block — terminal `outcome_class`/`outcome_reason_code`/`finalized_at_utc` are read verbatim from the already-durable row (never a classifier rerun); nonterminal rows always project `null` for those three fields; the terminal branch's `evidence_state` now honors the activity-count outcome instead of always reporting `complete`
- the full `active`/`not_found`/`backend_unavailable`/`query_failed` truth-state vocabulary (plus `invalid_request` for a malformed `market_date`, the only non-200 case); a downstream count-read failure now demotes the top-level `truth_state` to `query_failed` on every surface
- full-run-lineage activity counts (`strategy_evaluation_count`/`order_activity_count`/`fill_count`) via `mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage` plus one new narrow, unbounded `mqk_db::count_strategy_signal_evaluations_for_runs` helper (no migration) — an unreadable/contradictory lineage or a downstream read failure always yields `null` counts, never a false zero
- an additive `daily_operation` summary block on `AutonomousPaperReadinessResponse`/`AutonomousPaperStatusResponse`/`PreflightStatusResponse`, supplied by every response-construction branch in all three handlers, computed by a function that structurally cannot change its caller's HTTP status or any other field on a daily-operation DB failure
- an exact lexical `market_date` parser (`parse_exact_market_date`) rejecting whitespace, non-zero-padded fields, sign prefixes, trailing characters, and Unicode digit lookalikes, with a fixed bounded invalid-request message that never echoes raw caller input
- a scenario test file (`scenario_autonomous_daily_operation_api_01.rs`, 50 tests: 19 non-DB router-level/structural tests plus 31 DB-backed proofs of truth-state vocabulary, terminal/nonterminal projection, lineage-scoped counts, history ordering/limit-clamping, summary-block fail-soft behavior, downstream count-read-failure truth-state demotion, and zero operation/run/outbox/inbox/claim/evaluation side effects) — all 50 pass
- **no GUI surface was added** — that remains Phase F's job

**Accepted complete**
(AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-AND-CLOSURE
— Phase E is accepted complete in full):

- one new integrated scenario test file (`scenario_autonomous_daily_phase_e_closure_01.rs`, 6 tests, all passing) proving six end-to-end proofs against the real, isolated test database and the real production coordinator/finalizer/API seams together in one place, for the first time: a clean no-trade day's full durable pipeline plus replay/typed-projection/read-model checks; a two-run activity day whose full-lineage counts correctly include an earlier, non-current run's fill and order-ack evidence; an evidence-blocker notify-once/silent-replay/repair/recovery-through-`stopping`/terminal cycle; restart safety after a durable stop, after a terminal commit, and after an evidence blocker (each step via a brand-new `AppState`, this crate's established restart-proof convention); the E4 routes' full read-only guarantee across all five GET endpoints (before/after snapshot of state/version/lifecycle-event/coverage-event/run/claim/evaluation/outbox/inbox counts); and the frozen E4 fail-soft truth vocabulary exercised together (`not_found`/`backend_unavailable`/`query_failed`/an invalid-lineage evidence gap/the exact malformed-`market_date` 400)
- one new closure specification (`docs/specs/autonomous_daily_paper_operations_01e_phase_e_closure.md`) and one new closure guard (`scripts/guards/validate_autonomous_daily_paper_operations_01e_phase_e_closure.ps1`) that invokes every E1–E4 guard and adds source-aware Phase-E-specific checks of its own
- **zero production Rust behavior added or changed** — every seam E5 exercises was already accepted by E1–E4
- **no GUI surface was added** — that remains Phase F's job
- a follow-on E5 deterministic proof and closure-guard repair: every `tokio::time::sleep` is removed from the closure test in favor of a `PeAlertRecorder`/`wait_for_alert_count` helper driven by a `tokio::sync::watch` completion signal (bounded by a deadlock-protection-only timeout, never a delay that makes an assertion pass); `PeSnapshot`/`pe_snapshot` now derive the operation's full validated run lineage (never a caller-supplied single `run_id`) and additionally record global totals across `runs`/`sys_autonomous_daily_bar_dispatches`/`strategy_signal_evaluations`/`oms_outbox`/`oms_inbox`/`sys_autonomous_daily_operation_events`/`sys_autonomous_session_events`, closing the gap where an unrelated new-identity row could escape an operation-scoped-only snapshot; the read-only guarantee proof now runs against a genuine two-run lineage; and the closure guard's production-Rust/migration/GUI checks now inspect the committed `11664945e90a582e6984f0eab66cf89690120769..HEAD` patch range (previously only the working tree) in addition to the staged/unstaged working tree — **zero production Rust, migration, or GUI change**

**Accepted complete** (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-PROJECTION,
plus its RUNTIME-SHAPE-AND-HISTORY-BLOCKER-REPAIR-01 repair):

- strict GUI-side TypeScript response types (`AutonomousDailyOperationApiRow`/`AutonomousDailyOperationSurface`/`AutonomousDailyOperationsSurface`) mirroring the accepted E4 daemon API shapes verbatim, plus a GUI-only `transport_state` (`available`/`endpoint_unavailable`) layered in front of the daemon's own `truth_state` so a network/HTTP failure can never masquerade as the daemon's authoritative `not_found`
- both canonical E4 routes (`GET /api/v1/autonomous/daily-operation`, `GET /api/v1/autonomous/daily-operations?limit=20`) wired into the existing tracked probe assembly in `fetchOperatorModel` — no second polling timer, no mock-data fallback, `SystemModel` gained exactly two new read-only fields (`autonomousDailyOperation`/`autonomousDailyOperations`)
- a dedicated `Daily Operations` operator screen (`AutonomousDailyOperationsScreen.tsx`, screen key `dailyOperations`, registered in the operator monitor group next to Dashboard/Session/Ops and in the left rail) rendering current-operation truth (finalization status, outcome class/reason, evidence state/blockers, bar and activity counters, identity, timestamps) and recent history in daemon-preserved order — **zero action, mutation, or control element** (no button, form, input, `onClick`, `postJson`, or `invokeOperatorAction` reference anywhere in the screen)
- a pure `formatDailyOperationCount` helper so the three full-run-lineage activity counts render `null` as an explicit "Unavailable" marker and a real `0` as `"0"` — never a false zero
- a dedicated `classifyDailyOperationsSourceAuthority` helper (distinct from the existing coarse per-panel `classifyPanelSources`) implementing the required section-level availability matrix: both surfaces available → both sections render; one surface unavailable → only that section shows an unavailable notice; both unavailable → one screen-level fail-closed notice
- a focused GUI test matrix (4 new test files, 20+ assertions) proving both routes are requested, every truth-state distinction is preserved, null-vs-zero count rendering, history-order preservation, screen registration/reachability, the absence of any mutation control, and that the fallback/unavailable mapping never fabricates healthy truth — `npm test` (850 tests) and `npm run build` both pass
- **zero daemon route, response field, classifier, finalizer, coordinator, notification, coverage, lineage, or migration change** — F1 touches no production Rust file
- **F1 (plus its repair) is now accepted**

Implemented on the local `main` worktree
(AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F2-OPERATOR-RUNBOOK-CORRECTION),
but not yet independently accepted:

- `docs/runbooks/autonomous_paper_ops.md` (the existing canonical autonomous-paper runbook) updated in place — no competing duplicate runbook created
- a new `## 0. Safety boundary` section (Paper + Alpaca only, single-symbol long-only US equity/ETF, active operator supervision required, unattended soak not started, live capital not ready) plus prerequisites and an operating-vs-test-vs-reality-test database port table (operating `5432`; isolated `cargo test` DB `5434`; manual proof DB `55432`; reality-test DB `5440` — the latter three explicitly excluded from operating-database use)
- a new `# Part 2 — Daily-Operation Lifecycle Truth` section documenting the start-of-day sequence, the five authoritative read-only routes and their full truth-state/finalization-status/evidence vocabulary (`not_found` is explicitly not a backend failure; null counts are explicitly unavailable, not zero; generic `completed` is explicitly not automatic no-trade/activity proof), a before-session checklist, during-session supervision guidance, bounded recovery procedures that never invent a manual finalization command or a database row rewrite, stop/emergency posture, end-of-day evidence capture, and restart distinctions (before finalization / after a terminal commit / after an evidence blocker)
- a new F2 spec doc and a new source-aware F2 guard that re-invokes the F1 guard and proves the runbook content above
- **documentation and validation only — no daemon, GUI, or migration file is touched**
- F2, plus its final operational-safety repair, **is now accepted — F2: ACCEPTED — COMPLETE**

On top of F2,
`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F3-SUPERVISED-SOAK-EVIDENCE-PREPARATION`
prepares read-only evidence-capture tooling for **future** supervised Paper +
Alpaca sessions — it does not perform, start, count, or claim an unattended
soak:

- `scripts/soak/capture_autonomous_paper_session_evidence.ps1` — GET-only,
  fail-closed to local daemon hosts only (`127.0.0.1`/`localhost`/`::1`),
  never touches `.env.local` or any credential, never calls a mutating or
  lifecycle route; supports `-ValidateOnly` (no daemon call, no file write)
  and `-FixturePath` (local-fixture-only) safe modes
- `scripts/soak/validate_autonomous_paper_session_evidence.ps1` — validates
  a captured manifest: valid JSON, known schema version, every required
  field present, a valid `capture_phase`, `deployment_mode == paper` and
  `live_routing_enabled == false` when observable, truth-state vocabulary
  preserved, null counts never coerced to zero, a ten-pattern secret scan,
  and every daemon-sourced field either present or explicitly recorded in
  `missing_endpoints`
- `scripts/soak/templates/autonomous_paper_session_manifest.template.json`
  (`schema_version: "autonomous-paper-soak-evidence-v1"`) and
  `scripts/soak/supervised_session_evidence_checklist.md`
- one narrow new `.gitignore` rule (`smoke_logs/autonomous_paper_soak/`) —
  the tool's default, ignored output location; **no generated evidence is
  staged or committed by this patch**
- **F3 is final-repair-implementation complete, awaiting ChatGPT and
  operator acceptance** (its own evidence-validator integrity repair is
  described below)

On top of F3, `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01G-BUNDLE-3-FINAL-CLOSURE-AUDIT`
audits D1 through F3 against the committed repository state (specs, guards,
tests, ledger — not session memory) and answers the mission's thirteen
required closure questions, all resolving to a supported "yes" (see
`docs/specs/autonomous_daily_paper_operations_01g_bundle_3_final_closure.md`
§2). The full regression matrix (14 named scenario binaries, all passing)
plus the `scenario_autonomous_completed_bar_driver_01` baseline (47 passed,
9 identical pre-existing failures, 0 new Bundle 3 failures) was re-run
clean. `cargo check` on `mqk-db`/`mqk-runtime`/`mqk-daemon` is clean;
`git diff --check` and `git diff --cached --check` are clean.

Independent acceptance review of this F2/F3/G session then found genuine
defects: F2's runbook contradicted its own supervised-only safety boundary
and gave an unproven WS-gap restart procedure, and F3's capture/validator
tooling accepted an unvalidated daemon URL, unbounded error text, and an
unobserved/absent safety-identity as non-violations.
`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-F2-F3-G-FINAL-OPERATIONAL-SAFETY-REPAIR`
closes all of these in one commit — supervised-only runbook language,
source-proven WS-gap restart guidance, a legacy-tooling boundary on the
older soak harness, strict daemon-URL validation, pre-write secret
rejection, bounded capture-error records, a validator that requires
paper + alpaca + operator-supervised + observed-false live-routing, executable
local-fixture tests (16/16 passing), and hardened F2/F3/G guards — with no
production Rust, daemon API, GUI behavior, migration, or real external call.
**F2 is now accepted; F3 and Phase G are each repair-implementation
complete, awaiting final ChatGPT and operator acceptance.**

Independent proof-integrity review of that repair then found the E2A–E4
guards' own "Phase E: ACCEPTED"/"E5: ACCEPTED" forbidden-claim entries had
gone stale the moment Phase E was genuinely accepted (each guard was
permanently failing against the truthful, required README status line), the
Phase G guard only ever transitively re-ran F1→F2→F3 and never actually
invoked a single D-guard or E-guard directly, the Phase G committed-range
proof used a permanently-widening `4b6eec72..HEAD` window instead of a fixed
range, and the F3 evidence validator accepted several coercible/unsafe
shapes (`operator_supervised` coercing from `1`, `deployment_mode` accepting
a case- or whitespace-mismatched string, `live_routing_enabled` accepting a
non-Boolean, `repository_commit` accepting any nonempty string including raw
Git error text, and a `Double`/negative count silently passing as valid).
`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-BUNDLE-3-FINAL-GUARD-AND-EVIDENCE-
INTEGRITY-REPAIR` closes all of these in one further commit: the stale
forbidden-claim entries are retired from the E2A/E2B/E3/E4/E5 guards (every
genuine implementation invariant is preserved); the Phase G guard now
explicitly invokes the full D1/D4/E1–E5/F1–F3 guard matrix plus
`check_unsafe_patterns.ps1` by exact filename; the Bundle 3 committed-range
proof is now a fixed, self-locating range bounded by the exact commit
subject `fix: close bundle 3 proof and evidence gaps` (never `..HEAD`); the
F3 capture script now captures `repository_commit` via
`git rev-parse --verify HEAD` with explicit `$LASTEXITCODE` handling and a
40-hex-SHA shape check, never merging Git's stderr into the captured value;
and the F3 validator now enforces exact-type, exact-value safety-identity
checks (`deployment_mode`/`adapter_id` via ordinal string equality after a
`[string]` type check, `operator_supervised`/`live_routing_enabled` via an
exact `System.Boolean` type check), a strict integer-or-null count helper
(rejecting `Double`/`Decimal`/negative/numeric-string counts and validating
every retained `query_failed` row, never skipping it), a bounded
`capture_errors` shape check, and the `daemon_base_url` path check. The
local-fixture suite grew from 16 to 35 scenarios (19 new focused rejection
proofs). No production Rust, daemon API, migration, GUI behavior, broker,
provider, trading, order, or live-capital change.
**Bundle 3 (D1 through Phase G, including the final guard-and-evidence-
integrity repair) has now received independent ChatGPT/operator
acceptance — BUNDLE 3: ACCEPTED — COMPLETE.** This repository does not mark
Bundle 3 "closed" (a distinct, narrower claim this repo has not made); the
guard `validate_autonomous_daily_paper_operations_01g_bundle_3_final_closure.ps1`
now requires this accepted status rather than forbidding it.

```text
D1–D4: ACCEPTED — COMPLETE
PHASE D: ACCEPTED — COMPLETE

E1–E5: ACCEPTED — COMPLETE
PHASE E: ACCEPTED — COMPLETE

F1: ACCEPTED — COMPLETE
F2: ACCEPTED — COMPLETE
F3: ACCEPTED — COMPLETE
PHASE F: ACCEPTED — COMPLETE
PHASE G: ACCEPTED — COMPLETE

BUNDLE 3: ACCEPTED — COMPLETE
```

### Bundle 4: durable paper portfolio and P&L truth

`DURABLE-PAPER-PORTFOLIO-AND-PNL-01-COMBINED` closes the durable,
restart-surviving portfolio and P&L truth gap for paper + Alpaca,
single-symbol long-only US equity/ETF, supervised, on top of Bundle 3's
implementation (D1 through Phase G, reused unchanged; Bundle 3 itself is
now **ACCEPTED — COMPLETE**, not advanced or affected further by Bundle 4).
Eight phases (B4-0 stabilizes a
time-independent completed-bar fixture; B4-A is a current-truth audit and
binding contract; B4-B adds the durable schema/store — migration `0053`,
`sys_paper_portfolio_snapshots`/`_positions` and
`sys_paper_portfolio_accounting_state`; B4-C wires authoritative snapshot
persistence into the run-start/periodic/terminal-expiry call sites; B4-D
adds durable FIFO fill accounting reusing `recover_oms_and_portfolio`'s
duplicate-fill guard; B4-E adds three read-only durable routes and wires
`paper-lifecycle`'s real portfolio/P&L truth states; B4-F adds the operator
GUI section, runbook sections, and soak-evidence coverage; B4-G is the
integrated closure proof and final audit) are each implemented and proven
against the isolated port-5434 test database with real production seams —
no synthetic broker events, no fabricated fills/marks/P&L, fail-closed on
incomplete fill history.

The Bundle 4 final closure review found six correctness/closure defects
(cross-run snapshot/accounting contamination, same-watermark accounting
staleness, unconfirmed-snapshot accounting, one-directional completeness,
leaked/collapsed API errors, and a Bundle-3-guard live canary that could
never pass once Bundle 4 existed) — all six are closed by
`DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR`
(one commit; see the ledger and the per-phase spec addenda for detail).

A further final coherence and closure-proof pass,
`DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-COHERENCE-AND-ACCEPTANCE-PROOF`,
closes the remaining source-proven coherence gaps: transactional
source-snapshot integrity and deterministic snapshot-authority ordering on
every accounting write (mqk-db); one shared provenance classifier used
identically by durable-summary and paper-lifecycle so a stale accounting
row can never be reported active beside a newer snapshot on either route
(mqk-daemon); and closed GUI truth-state vocabularies, non-finite/non-
integral numeric rejection, state invariants, and snapshot-id (not just
run-id) cross-response consistency (mqk-gui). See the ledger for full
per-phase detail.

```text
B4-0: ACCEPTED — COMPLETE
B4-A: ACCEPTED — COMPLETE
B4-B–B4-G: FINAL REPAIR AND CLOSURE PROOF COMPLETE — AWAITING CHATGPT/OPERATOR ACCEPTANCE

BUNDLE 4:
FINAL REPAIR AND CLOSURE PROOF COMPLETE —
AWAITING FINAL CHATGPT AND OPERATOR ACCEPTANCE

BUNDLE 5: NOT STARTED
MULTI-SYMBOL AUTONOMOUS: NOT ENABLED
UNATTENDED 10–20-SESSION SOAK: NOT STARTED
LIVE CAPITAL: NOT READY
```

Bundle 4 is **not** marked accepted or closed in this repository. Still
required before Bundle 4 closes:

1. independent ChatGPT/operator acceptance of the final repair and
   coherence proof (B4-B through B4-G)

### What Bundle 3 completion unlocks

Bundle 3 is accepted complete; the intended result is a daemon that can remain running across
supported NYSE sessions and autonomously:

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

- Bundle 3 (`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`, D1 through Phase G including the final guard-and-evidence-integrity repair) is **accepted complete** (independent ChatGPT/operator acceptance received)
- Bundle 4 (`DURABLE-PAPER-PORTFOLIO-AND-PNL-01-COMBINED`, durable paper cash/positions/lots/cost-basis/P&L truth) is **closure-implementation complete (B4-0 through B4-G), awaiting final ChatGPT/operator acceptance** — required before trusting the accounting of any extended autonomous soak, not merely a nice-to-have
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

The long-only single-symbol Paper + Alpaca lane is the current finish line. Bundle 3 itself is now
accepted complete, but the autonomous MVP should not be called closed until its remaining market
evidence gates (real-fill/reconcile/Discord evidence, the autonomous-paper soak) are complete too:

| Item | Status |
|---|---|
| BUNDLE-3-AUTONOMOUS-DAILY-OPS | Accepted complete (D1–D4, Phase E1–E5, Phase F1–F3, Phase G, and the final guard-and-evidence-integrity repair) |
| PAPER-TRADE-LIFECYCLE-01 | Open — market-hours paper smoke with real fills |
| RECONCILE-AFTER-REAL-FILL-01 | Open — reconcile pass after a real paper fill |
| DISCORD-TRADE-LIFECYCLE-REAL-01 | Open — Discord notification evidence from a real cycle |
| AUTONOMOUS-PAPER-SOAK | Not started — target roughly 10–20 clean sessions after Bundle 4 closure |
| DURABLE-PAPER-PORTFOLIO-PNL | Closure-implementation complete (B4-0–B4-G), awaiting final ChatGPT/operator acceptance |
| PAPER-SMOKE-EVIDENCE-REVIEW-02 | Closed — review tool exists; future smoke evidence still requires review |

These include both code gates and operational evidence gates.

The 2026-06-15 no-trade smoke remains useful historical evidence, but it does not close the
current completed-bar task, real order/fill lifecycle, reconcile-after-fill, Discord real-cycle,
or repeated autonomous-session proof.

### Evidence capture workflow

Now that Bundle 3 is accepted, once configured and inside the session window, the durable
autonomous controller should own starting the paper run. Evidence remains captured using the read-only
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

1. Bundle 3 autonomous daily paper operations — **accepted complete**
2. close Bundle 4 durable paper portfolio and P&L truth (closure-implementation complete, awaiting final ChatGPT/operator acceptance)
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
