# MiniQuantDeskV4 — Technical README

This is the hands-on setup, proof, and operator guide for MiniQuantDeskV4.

## What this document is for

Use this file for:

- local setup
- env-file workflow
- proof and verification commands
- DB proof execution
- daemon and GUI startup
- CLI usage
- current deployment boundaries
- operator workflow reality

Use the root `README.md` for the high-level system story.

## Current proved posture

**Repository snapshot used for this update (2026-07-20):** local `main` at
`3591064a805efc82b3f6468e1de0fe06ea028471`
(`docs: require coverage authority before bar processing`), plus independent
ChatGPT/operator acceptance of D4 and its evaluation-lineage repair together
— **Phase D is accepted complete in full** — plus independent acceptance of
the four-times-corrected
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-DURABLE-OUTCOME-AUTHORITY-AND-EVIDENCE-CONTRACT
(Phase E1: the read-only architecture audit producing the binding
durable-outcome/no-trade contract for Phase E) — **E1 is accepted complete**
— plus the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-FOUNDATION
patch on top of it: the first Phase E runtime code. E2A implements the
durable, operation-scoped `autonomous_daily_coverage_bound` evidence event
(typed model, canonical construction, write/re-read/replay/conflict
contract), the coordinator's ensure-authority seam, the completed-bar
adapter's mandatory per-tick authority/mid-day-drift gate, and a raw
full-run-lineage read/validate helper — plus, on top of that, the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-AUTHORITY-ENVELOPE-GATE-ORDERING-AND-CONCURRENCY-CLOSURE
repair: a complete durable event-envelope validator (id, event_type, source,
`run_id IS NULL`, `resume_source IS NULL`), a duplicate-JSON-key-rejecting
typed parser, the adapter's authority gate reordered strictly before any
assignment/identity resolution so a missing anchor stays a quiet no-op even
under a locally malformed environment, and a live `tokio::join!`-driven
coordinator/adapter concurrency proof — plus, on top of that, the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-SAME-INSTANT-CONCURRENCY-AND-SIDE-EFFECT-PROOF-01
final proof repair: the closure repair's concurrency test drove the
coordinator and adapter at two independently timestamped ticks rather than
one shared logical instant, so it never proved the adapter observes the
coordinator's own newly-created operation as of the coordinator's own
`now_utc`. The same test (`f04`, rewritten in place) now drives both tasks at
one shared `now_utc`, captures a full durable before/after snapshot (state,
`state_version`, `run_id`, bar/claim/lifecycle-event/coverage-event/
evaluation/decision counts) proving zero side effects from the adapter while
the coordinator remains paused, proves the adapter never touches a
deliberately-invalid instrument-registry path before its authority gate
resolves, and proves release-then-normal-progression to `DriverOutcome`. No
production coordinator/state file was touched by this repair — **E2A (plus
both repairs) is accepted complete.** On top of that accepted foundation, the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-FINALIZATION-CAS
patch implements the strict evidence classifier and durable finalization CAS
(`core-rs/crates/mqk-daemon/src/state/autonomous_daily_outcome.rs`): a durable
evidence-snapshot model plus an async gathering pass consuming E2A's
coverage-anchor/run-lineage authorities without re-deriving them; a pure,
zero-I/O global-precedence classifier structurally incapable of reading any
process-local diagnostic counter; the terminal finalization CAS
(`mqk_db::finalize_autonomous_daily_operation`, `outcome`/`finalized_at_utc`
set atomically, generic `completed` and `no_trade_reason` both structurally
unreachable); two new `stopping`/`stop_retrying -> evidence_degraded` legal
edges; and a commit-uncertainty/database-failure contract mirroring D4's
authoritative-re-read discipline — plus, on top of that, the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-TERMINAL-TRUTH-PRECEDENCE-AND-UNCERTAINTY-CLOSURE
repair: `mqk_db::is_valid_terminal_state_outcome_pair` is the single shared
pure validator for the four authorized state/outcome pairs (closing a defect
where a cross-paired combination such as `completed_no_trade` +
`activity_fill_confirmed` previously passed the finalization CAS's legality
check); `mqk_db::is_complete_automatic_terminal_truth` gates both the store's
`AlreadyApplied` replay and the daemon's high-level already-terminal handling
(a matching state/outcome alone is no longer sufficient — `finalized_at_utc`
must be present and `state_reason_code`/`state_blocker_signature` both null);
the high-level entry point now distinguishes manual/administrative generic
`completed` (read-only `AlreadyFinalized`) from a malformed automatic
terminal row (`Conflict`); the classifier's coverage-missing precedence no
longer special-cases an empty run lineage (always
`unknown_incomplete_bar_coverage`, per the corrected identity -> lineage ->
coverage -> expected-bar -> claims -> evaluations precedence order); and the
commit-uncertainty re-read now also requires `state_version` to have
strictly advanced past the original expected version. All three commit-
uncertainty scenarios (commit-applied-acknowledgment-lost, genuine CAS
staleness, a genuine conflicting concurrent writer) and the real partial-
evidence-read-failure scenario are proven end-to-end against a real database
through a narrow `AutonomousDailyFinalizationEffectSeam` (default no-op in
production; never a mocked successful write). **E2B is accepted complete.**
On top of that accepted foundation, the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-INTEGRATION-AND-NOTIFICATION
patch wires the accepted E2B finalizer into the durable daily coordinator:
`handle_stopping`'s stop-completion no-op now routes into a new
`handle_outcome_finalization` seam once `stopped_at_utc` is durable (reached
from both `dispatch_by_state`'s ordinary routing and
`reconcile_existing_operation_against_relevant_lookup`'s fallback-lookup
routing, so a stopped operation is never abandoned); the matching-local-
runtime fact is computed from `AppState::locally_owned_run_id()` compared
against `operation.run_id`; E2B's current policy inputs are resolved fresh
from the exact same seams `ensure_coverage_authority` already uses; post-stop
`evidence_degraded` operations route into the same seam for recovery-or-
replay (a real defect — the pre-existing resolution-failure fallback path
could silently overwrite E2B's own durable evidence-degraded reason with an
unrelated one on every subsequent tick — was found only by running the new
integration suite and fixed); `AutonomousDailyFinalizationOutcome::EvidenceDegraded`
gained a durable-CAS-derived `newly_applied: bool` field; and
`session_controller.rs`'s `log_coordinator_outcome` sends exactly one outcome
notification per newly finalized operation and exactly one warning per newly
applied evidence blocker, both gated on durable facts, never process-local
memory. A new scenario test file
(`scenario_autonomous_daily_outcome_coordinator_integration_01.rs`, 15 tests
as originally accepted — 14 DB-backed `#[ignore]` integration tests plus one
non-DB source-level unit test, all real-DB-backed against the real
production coordinator/finalizer seams with a loopback Discord sink, no real
network call) proves clean finalization, exactly-once finalization/
notification, restart safety, evidence-degraded recovery, and the
resolution-failure fallback path — all 15 pass.

AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-GATE-
REPAIR-01 then closed a second confirmed defect: `handle_outcome_finalization`
computed the matching-local-runtime fact but never consulted it before
persisting an `evidence_degraded` blocker in its own resolution-failure
branches, so a matching local runtime could be incorrectly overridden
whenever current policy/config resolution also failed that tick. The
coordinator now returns `AwaitingOutcomeFinalization` before any policy
resolution or blocker persistence is attempted whenever a matching local
runtime is active, and `persist_autonomous_daily_finalization_blocker` now
requires the caller's `AutonomousDailyFinalizationContext` and independently
refuses to write (`NotEligible`) under the same condition, as
defense-in-depth. Two new tests
(`ci_03b_matching_local_runtime_blocks_policy_failure_without_write_or_notification`,
bringing the E3 coordinator test file to 16 tests, all passing; and
`store_59_persist_finalization_blocker_refuses_when_matching_runtime_active`
in the E2B classifier/finalization test file, bringing it to 67 tests, all
passing) prove zero write and zero notification end-to-end. **E3 is now
accepted** (plus this repair, both accepted together).

On top of that accepted foundation,
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-PROJECTION
adds exactly two strictly read-only routes,
`GET /api/v1/autonomous/daily-operation[?market_date=]` and
`GET /api/v1/autonomous/daily-operations[?limit=]`, one shared pure
projection (terminal fields read verbatim from the durable row, never a
classifier rerun), full-run-lineage activity counts via
`mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage` plus one
new narrow `mqk_db::count_strategy_signal_evaluations_for_runs` helper (no
migration), and an additive `daily_operation` summary block on
readiness/paper-status/preflight. A follow-on
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-TRUTH-AND-EVIDENCE-STATE-REPAIR-01
repair closed five source-proven read-truth defects: the terminal projection
reported `evidence_state = "complete"` regardless of whether the activity
counts could actually be gathered; a downstream count-read failure left the
single/history/summary responses' top-level `truth_state` at `"active"`;
generic administrative `completed` received the same evidence-complete
treatment as the two automatic classifier terminal states; the malformed-
`market_date` 400 response echoed the raw, unbounded caller-controlled
query value; and none of this had test or guard coverage. The repair adds a
shared `response_truth_state_for_counts` mapping (reused by all three
response-construction sites) and a narrow, always-`false`-in-production
test-only override for deterministically exercising a downstream database
failure. The scenario test file (`scenario_autonomous_daily_operation_api_01.rs`,
now 50 tests) proves the full truth-state vocabulary, terminal/nonterminal
projection honoring the activity-count outcome, lineage-scoped counts,
history ordering/limit-clamping, summary-block fail-soft behavior,
downstream count-read-failure truth-state demotion, and zero operation/run/
outbox/inbox/claim/evaluation side effects — all 50 pass. A second follow-on
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-EXACT-MARKET-DATE-PARSER-REPAIR-02
closed the one remaining E4 validation defect: the explicit `market_date`
query branch parsed with `NaiveDate::parse_from_str(raw.trim(), "%Y-%m-%d")`,
silently accepting whitespace-normalized forms the frozen route contract's
exact `YYYY-MM-DD` lexical form does not authorize. A new pure helper,
`parse_exact_market_date`, replaces it with an exact byte-length/dash-
position/ASCII-digit check followed by `chrono` parsing and a canonical
`format("%Y-%m-%d")` round-trip check against the raw input, rejecting
whitespace, non-zero-padded fields, sign prefixes, trailing characters, and
Unicode digit lookalikes; no normalization step remains anywhere in the
route. **E4 (plus both repairs) is now accepted.** On top of that accepted
foundation, AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E5-INTEGRATED-PHASE-E-PROOF-
AND-CLOSURE adds one new integrated scenario test file
(`scenario_autonomous_daily_phase_e_closure_01.rs`, 6 tests, all passing)
proving a clean no-trade day, a two-run full-lineage activity day, an
evidence-blocker notify-once/replay/recovery cycle, restart safety across a
durable stop/terminal commit/evidence blocker, the E4 routes' full read-only
guarantee, and the frozen E4 fail-soft truth vocabulary, all against the
real, isolated test database and the real production coordinator/finalizer/
API seams (fake notifier instrumentation only). Zero production Rust
behavior changed. A follow-on E5 deterministic proof and closure-guard
repair then closed four proof defects in that patch: every
`tokio::time::sleep` fixed delay is replaced by a `PeAlertRecorder`/
`wait_for_alert_count` helper whose completion signal is a
`tokio::sync::watch` channel (a bounded `tokio::time::timeout` remains only
as deadlock/failure protection, never as what makes an assertion pass);
`PeSnapshot`/`pe_snapshot` now derive the operation's full validated run
lineage instead of accepting a caller-supplied single `run_id`, and
additionally record global totals across `runs`,
`sys_autonomous_daily_bar_dispatches`, `strategy_signal_evaluations`,
`oms_outbox`, `oms_inbox`, `sys_autonomous_daily_operation_events`, and
`sys_autonomous_session_events` so an unrelated new-identity row cannot
escape an operation-scoped-only snapshot; the API read-only-guarantee proof
now runs against a genuine two-run lineage; and the closure guard's
production-Rust/migration/GUI checks now read the committed
`11664945e90a582e6984f0eab66cf89690120769..HEAD` patch range (previously
only the working tree was inspected, which sees nothing once E5's own work
is committed) in addition to the staged/unstaged working tree. Zero
production Rust, migration, or GUI change. **E5 (plus this closure and its
repair) is now accepted — Phase E is accepted complete in full.** On top of
that, `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-
PROJECTION` adds the first Phase F GUI surface: a read-only `Daily
Operations` operator screen consuming the accepted E4 API verbatim, with no
mutation control and no reinterpretation of daemon truth (§ "Active Bundle 3
boundary" below). A follow-on repair,
`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-RUNTIME-SHAPE-AND-HISTORY-BLOCKER-
REPAIR-01`, closed two confirmed defects: the GUI mapper functions now
perform complete runtime shape validation (a malformed HTTP 200 body — e.g.
`active` truth_state with a missing `operation`, or a history response with
a missing/invalid `rows` array — now fails closed to `endpoint_unavailable`
instead of rendering as false-authoritative truth, and `active` + null
operation can no longer be confused with the daemon's own authoritative
`not_found`), and the history table now renders every row's
`evidence_blockers`, which the original F1 pass omitted (only the
current-operation panel rendered blockers). **F1 is implementation complete,
awaiting ChatGPT and operator acceptance.**

The strongest current operational route is:

- `paper` deployment mode
- `alpaca` adapter
- long-only, single-symbol US equity/ETF lane
- daemon + Vite GUI operator path
- DB-backed targeted scenarios and repository guards as the load-bearing development proof
- `full_repo_proof.ps1` as the final locked-snapshot proof runner

The historical 2026-06-01 full DB-backed low-memory proof passed 18/18 lanes, but the repository has
advanced materially since that snapshot; this README does not represent that historical 18/18
transcript as a fresh full-repo proof of the current commit.

### Active Bundle 3 boundary

`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED` remains open.

Current local `main` contains, accepted (D1–D4, Phase D accepted complete in full):

- durable daily-operation identity, state/version CAS, and append-only transitions
- canonical session boundaries and nontrading-day reconciliation
- typed start, recovery, and stop retries
- exact completed-bar observation and durable dispatch claims
- production `main.rs` cutover from the legacy blind ticker to the supervised completed-bar task
  (legacy ticker retained in source for compatibility tests only, never spawned in production)
- retained supervisor ownership and shutdown wait behavior
- full restart-budget exhaustion proof (bounded to 3 restarts / 4 worker generations)
- durable operation degradation when the task permanently fails
- sticky operator failure truth (survives session-controller's own Running-style projections)
- complete typed classification of non-recoverable driver/setup outcomes
- task-level PrepareDataOnly → RunningDispatch → exactly-once proof
- closure of a confirmed completed-bar dispatch-ownership race: the completed-bar driver's durable
  claim previously deposited into the same shared, account-wide `pending_strategy_bar_input`
  mailbox the ordinary execution loop drains every tick, then immediately re-took it — a concurrent
  execution-loop tick could steal the deposited bar first, causing the claim to be recorded failed
  despite a real evaluation having occurred. The claim now dispatches directly through
  `AppState::dispatch_native_strategy_for_symbol_with_bar` (the same canonical exact-input dispatch
  implementation, just called with the claim's own bar value) and never touches the mailbox;
  execution-loop and manual-signal-route dispatch are unchanged. A deterministic concurrency proof
  (both interleaving orderings) and one integrated scenario test driving a synthetic Paper+Alpaca
  day through preopen, canonical start, running dispatch, runtime interruption/recovery, session
  close, and shutdown together accompany this fix.
- a shared deterministic identity helper (`AppState::derive_strategy_signal_evaluation_id`) used by
  both the signal-evaluation journal writer and the completed-bar claim path — never a second,
  independently-derived algorithm; the claim path durably confirms the exact
  `strategy_signal_evaluations` row before completing a claim; the completion write's `Result<bool>`
  is captured and matched explicitly, routing `Ok(false)`/`Err` through one authoritative re-read;
  the full-day lifecycle test's preopen phase resolves through real production readiness truth with
  zero manual unstick; a supervised-task proof under an injected clock

Accepted (Phase E1, four-times-corrected):

- the read-only architecture audit
  (`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-DURABLE-OUTCOME-AUTHORITY-AND-EVIDENCE-CONTRACT`,
  `docs/specs/autonomous_daily_paper_operations_01e_outcome_truth_contract.md`) producing the
  binding contract for Phase E's durable daily outcome/no-trade classification: which durable store
  is outcome authority (`sys_autonomous_daily_operations`, already Phase-B-built and unused by any
  production writer today), exactly when an operation becomes finalization-eligible
  (`stopped_at_utc IS NOT NULL` plus no locally-owned runtime/completed-bar-task activity remaining
  — `postclose_finalize_utc` is a stop-retry escalation deadline, not a finalization gate), the
  activity and no-trade evidence hierarchies, an `unknown_insufficient_evidence` representation that
  reuses the existing nonterminal `evidence_degraded` state (no migration required), evidence-
  conflict precedence, a restart/idempotency contract reusing the existing CAS transition machinery,
  a bounded reason-code matrix, a read-only API contract for two net-new
  `GET /api/v1/autonomous/daily-operation[s]` routes, a notification contract, the corrected
  operation-scoped `autonomous_daily_coverage_bound` coverage-anchor seam, the raw full-run-lineage
  read/validate contract, and the **E2A/E2B** implementation decomposition, across four correction
  passes that closed source-proven defects found by fresh, targeted re-reads of the driver/coordinator
  source
- **no Phase E runtime code was written by E1 itself** — E1 is documentation/guard-only; the classifier,
  coordinator wiring, API routes, and durable coverage-anchor/run-lineage foundation remained
  E2A/E2B/E3/E4's job

Accepted (Phase E2A, plus its AUTHORITY-ENVELOPE-GATE-ORDERING-AND-CONCURRENCY-CLOSURE and
SAME-INSTANT-CONCURRENCY-AND-SIDE-EFFECT-PROOF-01 repairs):

- the typed, schema-versioned `CoverageBoundDetail` payload
  (`core-rs/crates/mqk-daemon/src/state/autonomous_daily_coverage_authority.rs`), a duplicate-key-
  rejecting typed-wire-struct parser (`CoverageBoundDetailWire`, `#[serde(deny_unknown_fields)]` —
  decodes directly into a typed struct rather than a `serde_json::Value` object map, so serde's own
  derived `visit_map` rejects a literal duplicate field the moment it is seen a second time; rejects
  missing/wrong-type/unknown-field/unknown-schema-version payloads), and `#[derive(PartialEq)]`-based
  semantic equality over every immutable field — the payload deliberately excludes `bound_at_utc`; the
  bind instant is the event row's own `ts_utc` column, metadata only
- `validate_coverage_authority_envelope`: the complete durable event-envelope validator every authority
  read now uses — verifies the row's exact deterministic `id`, `event_type ==
  autonomous_daily_coverage_bound`, `source == mqk-daemon.autonomous_daily_coordinator`, `run_id IS
  NULL`, and `resume_source IS NULL`, never merely the id and a matching JSON detail payload
- `construct_coverage_bound_detail`, a pure, side-effect-free constructor reusing only
  `daily_data_readiness::expected_intraday_end_ts_window`/`intraday_grid_starts` — no second
  calendar, timeframe, grace, or completed-bar algorithm. The first dispatchable bar is the final
  element of the expected window evaluated at `effective_operation_open_utc` (may spill into the
  previous session at an ordinary open); the final dispatchable bar is the last current-session grid
  identity whose expectation instant is strictly greater than the first bar's own and strictly less
  than `effective_operation_close_utc` (a close-boundary bar is excluded), or the first bar itself
  when none qualifies
- `check_coverage_authority_envelope` (Stage A: exact-id fetch, complete envelope validation,
  duplicate-safe parse, payload `operation_id` verification — requires no assignment/runtime-binding/
  policy resolution of any kind) composed with `write_and_confirm_coverage_authority` /
  `check_coverage_authority` (Stage B: semantic comparison against the one already-loaded authority
  value, never re-read or re-parsed through a second algorithm): the exact write/re-read/idempotent-
  replay/conflict contract over the existing `sys_autonomous_session_events` store (`ON CONFLICT (id)
  DO NOTHING`, id = `autonomous_daily_coverage_bound:{operation_id}`) — a write error is never trusted
  without a confirming authoritative re-read through the same envelope validator, and a row with any
  single tampered envelope field (`event_type`/`source`/`run_id`/`resume_source`) or a tampered
  `detail.operation_id` is rejected without ever overwriting the original row
- the coordinator's `ensure_coverage_authority` seam
  (`state/autonomous_daily_coordinator.rs`), run immediately after `create_or_recover` and before any
  state-handler dispatch, for both newly created and recovered operations: a pristine operation (zero
  `run_id`/`started_at_utc`/bars/claims/running-lineage, via `check_operation_pristine`) may bind a
  fresh anchor; any operation with prior activity and no anchor fails closed to
  `coverage_authority_missing_after_activity` (`running` degrades to `evidence_degraded`; other
  nonterminal states degrade to `manual_intervention_required`), reusing the existing D1
  blocker-signature mechanism — never a retroactively fabricated anchor. Close priority is preserved:
  at or after `effective_operation_close_utc`, canonical close/stop reconciliation always takes
  precedence over a fresh coverage blocker
- the completed-bar production adapter's mandatory per-tick authority gate
  (`state/autonomous_completed_bar_task.rs`), corrected to a two-stage order: after operation fetch and
  the cheap state-only mode short-circuit, Stage A (`check_coverage_authority_envelope`) runs strictly
  *before* any assignment/runtime-binding resolution is even attempted — a missing, unreadable, or
  envelope-invalid authority returns `CoverageAuthorityUnavailable` with zero lifecycle mutation and
  zero attempt to resolve local assignment/runtime configuration at all, so this stays true even when
  that local environment/configuration is itself malformed. Only once Stage A proves a real,
  correctly-shaped authority exists does the adapter resolve its current policy, construct the fresh
  payload, and semantically compare it (Stage B) against the one authority value Stage A already
  loaded — strictly before `load_driver_instruments` or any provider/registry object is built. The
  `CoverageAuthorityUnavailable { operation_id, reason_code }` outcome variant carries the four closed
  reason codes (`coverage_authority_not_bound` / `_unreadable` / `_invalid` / `_conflict`); a missing
  anchor is a quiet, no-mutation no-op, while every other case refuses the driver without the adapter
  itself mutating lifecycle state — durable fail-closed projection remains coordinator-owned. Once
  Stage A has proven the authority present and valid, the existing source-aligned
  `IdentityUnresolved`/blocker behavior for assignment/runtime/policy failures is preserved unchanged
- mid-day coverage-policy drift: the adapter's `resolve_current_coverage_policy_inputs` resolves
  `timeframe_secs`/`required_history_bars`/`effective_grace_seconds` from the assignment's own
  configured timeframe and the strategy registry's data requirements on every tick (deliberately
  ignoring `EffectiveRuntimeBinding::effective_runtime_timeframe_secs` for this purpose, so this
  resolver never preempts the driver's own separate `runtime_strategy_timeframe_mismatch` readiness
  blocker with a competing check); any field disagreeing with the bound anchor is
  `coverage_authority_conflict`, proven for both `PrepareDataOnly`- and `RunningDispatch`-eligible
  states
- `mqk_db::fetch_autonomous_daily_operation_running_transitions_raw` /
  `validate_autonomous_daily_operation_run_lineage` /
  `fetch_and_validate_autonomous_daily_operation_run_lineage`: a raw, unbounded `(transition_seq,
  run_id)` read (`to_state = 'running'`, never `SELECT DISTINCT`, never the general-purpose 100-row
  API list cap) plus Rust-side strict-monotonicity/uniqueness/current-run-match validation, proven
  against a real two-run recovery cycle and a 150-row fixture
- `mqk_db::fetch_autonomous_session_event_by_id` (exact primary-key read) and
  `mqk_db::count_autonomous_daily_bar_dispatch_claims` — the two narrow `mqk-db` read helpers this
  foundation needed; no migration
- a live, deterministic coordinator/adapter concurrency proof
  (`AutonomousCoverageAuthorityPreBindTestHook`, `state/autonomous_daily_coordinator.rs`, mirroring the
  existing D4.4 `AutonomousCompletedBarPostClaimTestHook` pattern): a `tokio::sync::Notify`-based
  rendezvous pauses the real coordinator tick immediately after `create_or_recover` commits the
  operation row and before `ensure_coverage_authority` begins; production never installs the hook (one
  uncontended async mutex lock per tick). The scenario test drives the real coordinator tick and the
  real production adapter tick concurrently via `tokio::join!`, proving the adapter observes
  `coverage_authority_not_bound` with zero lifecycle mutation, zero claims, and zero bar observations
  while the coordinator is paused, then proves a normal eligible tick proceeds once released
- `tests/scenario_autonomous_daily_coverage_anchor_and_run_lineage_01.rs` (41 tests): construction
  bounds (ordinary-open spillover, close-boundary exclusion, no-later-bar-qualifies), serialize/parse
  round-trip and tamper cases including duplicate-JSON-key rejection, semantic-equality field
  sensitivity, pure run-lineage validation, the durable write/replay/conflict contract including five
  independent envelope-field tamper cases, the coordinator's pristine-bind and prior-activity
  fail-closed paths (plus close priority), the adapter's corrected two-stage authority gate including a
  deterministic zero-side-effect proof for a newly-visible not-yet-anchored operation, a
  proceeds-once-bound proof, and the live `tokio::join!` concurrency proof, mid-day drift for both
  eligible modes, and the DB-backed run-lineage read/validate helper
- **no outcome classifier and no finalization behavior were written by E2A itself** —
  `outcome`/`finalized_at_utc` remained unwritten by any production code path at E2A's own
  acceptance; no API route; no GUI change; no migration; no `is_legal_operation_transition` graph
  change from E2A itself

**Accepted complete**
(AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2B-STRICT-OUTCOME-CLASSIFIER-AND-FINALIZATION-CAS):

- `core-rs/crates/mqk-daemon/src/state/autonomous_daily_outcome.rs` (new): an
  `AutonomousDailyOutcomeEvidenceSnapshot` model plus
  `gather_autonomous_daily_outcome_evidence` (async, DB-read-only) — performs every database read
  for one classification attempt up front: the coverage anchor via E2A's own
  `check_coverage_authority`, the full run lineage via E2A's own validated-lineage helper, the exact
  expected dispatch-bar set (`derive_expected_bar_set`, pure, reuses only
  `daily_data_readiness::intraday_grid_starts`), every expected bar's durable dispatch claim and
  evaluation row (raw, unvalidated — every cross-check happens in the pure classifier), and every
  `oms_outbox`/`oms_inbox` row across the complete validated run lineage via two new narrow,
  unbounded, any-status/any-event-kind read helpers (`mqk_db::outbox_load_all_for_run`,
  `mqk_db::inbox_load_all_for_run`)
- `classify_autonomous_daily_outcome` (pure `fn`, zero I/O): applies the E1 contract's exact
  ten-step global precedence order (identity → lineage → coverage anchor → expected-bar
  coverage/aggregate consistency → zero unresolved claims → exact evaluation evidence → activity
  tier → order-evidence conflict → no-trade) over an already-gathered snapshot. No process-local
  diagnostic field (`AppState`, `bar_tick_dispatch_count`, `last_bar_signal_qty`) is reachable from
  the snapshot type at all — structurally impossible, not merely avoided by convention. A missing
  durable coverage anchor for an operation whose full run lineage is empty (never reached `running`)
  routes to `unknown_missing_evaluation_evidence`; the same missing anchor for an operation that did
  run routes to `unknown_incomplete_bar_coverage` — resolving an apparent tension between two E1
  contract statements by keying on lineage emptiness, documented and tested
  (`docs/specs/autonomous_daily_paper_operations_01e2b_outcome_classifier_and_finalization.md` §6)
- four closed terminal reason codes (`activity_fill_confirmed`, `activity_order_submitted`,
  `activity_decision_accepted`, `no_trade_strategy_evaluated_no_signal`) and eight closed
  nonterminal `unknown_*` reason codes; generic `completed` is not a representable variant of
  `AutonomousDailyOutcomeClassification` at all — structurally, not by runtime check
- `mqk_db::finalize_autonomous_daily_operation` (new,
  `core-rs/crates/mqk-db/src/autonomous_daily_operation.rs`): the terminal finalization CAS — sets
  `state`/`outcome`/`finalized_at_utc` atomically in one `UPDATE`, clears
  `state_reason_code`/`state_blocker_signature`/`next_retry_utc`/`last_error`, rejects generic
  `completed` and any outcome outside the closed four-code set before touching SQL, and never writes
  the retired `no_trade_reason` column; typed `Applied`/`AlreadyApplied`/`ConflictingTerminalTruth`/
  `StaleState`/`NotFound`/`IllegalTarget` outcomes — no migration
- two new legal transition edges (`stopping`/`stop_retrying -> evidence_degraded`, per the E1
  contract §3.3, `is_legal_operation_transition`), the evidence-degraded blocker write reusing the
  existing D1 blocker-signature mechanism verbatim
  (`AutonomousCoordinatorReason::UnclassifiedFailClosed`, zero changes to
  `autonomous_retry_policy.rs`), and the pre-existing `evidence_degraded -> stopping` edge as the
  sole recovery path — never finalizing directly from `evidence_degraded`
- a commit-uncertainty-safe write discipline for every CAS (finalization and blocker alike),
  mirroring D4's `reconfirm_dispatch_completion_or_fail_closed` pattern: an ambiguous write result
  always triggers one authoritative re-read before any success is claimed; complete database outage
  performs zero write attempts; a partial evidence-read failure's best-effort blocker write is only
  ever claimed durable after a confirming re-read
- a high-level `classify_and_finalize_autonomous_daily_operation` entry point taking a narrow,
  source-aligned `AutonomousDailyFinalizationPolicyInputs` (calendar provider, config, binding,
  strategy registry — mirroring exactly what the coordinator's own `ensure_coverage_authority`
  already threads through) instead of a full `AppState` — at E2B's own acceptance point, not yet
  called from any production tick; E3 (below) wires it in
- `tests/scenario_autonomous_daily_outcome_classifier_and_finalization_01.rs` (66 tests): 26 pure
  classifier scenarios and 4 pure eligibility proofs (no DB), 33 DB-backed finalization/blocker CAS
  store proofs, and 3 DB-backed integrated end-to-end proofs (clean no-trade → `completed_no_trade`;
  clean fill → `completed_with_activity`; unresolved claim → `evidence_degraded` → repair →
  `evidence_degraded -> stopping` → later invocation → `completed_no_trade`) — all 66 pass (the
  `newly_applied` field E3 added to `EvidenceDegraded`, below, required three mechanical match-pattern
  updates with no change in assertion meaning)
- at E2B's own acceptance point: **no coordinator invocation, no API route, and no GUI surface were
  added** — E3 (below) is exactly that coordinator invocation; E4 remains the API/GUI job

**Accepted complete**
(AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-INTEGRATION-AND-NOTIFICATION):

- `handle_stopping`'s stop-completion no-op (previously `if operation.stopped_at_utc.is_some() {
  return Ok(AwaitingOutcomeFinalization) }`, a permanently-repeating no-op once stop completed) now
  routes into a new `handle_outcome_finalization` helper. Because `handle_stopping` is the single call
  site shared by `dispatch_by_state`'s ordinary per-tick routing **and**
  `reconcile_existing_operation_against_relevant_lookup`'s fallback-lookup routing (itself reached
  only via `resolve_or_degrade_on_resolution_failure`/`resolve_or_reconcile_on_nontrading_day`), this
  one change closes the finalization gap for every production entry path simultaneously. The parallel
  `evidence_degraded` case required a symmetric split in both `dispatch_by_state`'s and
  `reconcile_existing_operation_against_relevant_lookup`'s combined `CONTROLLER_DEGRADED`/
  `EVIDENCE_DEGRADED` arms — a real defect found only by running the new integration suite against a
  live coordinator tick: without it, a subsequent resolution-failure tick would silently overwrite
  E2B's own durable `unknown_*` reason with an unrelated one on every tick, producing a spurious
  repeated "newly applied" write and a spurious repeated warning
- the matching-local-runtime fact (E1 §3.2 condition 4) is computed by a new
  `matching_local_runtime_active` helper from `AppState::locally_owned_run_id()` compared against
  `operation.run_id` — never `locally_started`, task liveness, a process-local bar counter, or GUI
  state
- `handle_outcome_finalization` resolves `AutonomousDailyFinalizationPolicyInputs` fresh, once per
  attempt, from the exact same `build_multi_symbol_runtime_config_from_env`/
  `resolve_autonomous_runtime_context`/`daily_data_readiness::load_readiness_context_from_env` seams
  `ensure_coverage_authority` already threads through `resolve_current_coverage_policy_inputs` — no
  second environment parser, no duplicate runtime-binding algorithm, no cached process-local policy.
  When `config`/`runtime_context` resolution itself fails (no real
  `MultiSymbolRuntimeConfig`/`EffectiveRuntimeBinding` exists to construct the policy inputs from at
  all), a new narrow `pub` wrapper in E2B's own module,
  `persist_autonomous_daily_finalization_blocker`, persists `unknown_assignment_identity_unavailable`
  by delegating verbatim to the existing private `apply_evidence_degraded_blocker` — E2B remains the
  sole owner of blocker CAS/signature/re-read/replay semantics; the coordinator creates no second
  blocker writer
- post-stop `evidence_degraded` operations (`stopped_at_utc` present) route into the same
  `handle_outcome_finalization` seam for recovery-or-replay; E2B's own production entry point already
  internally dispatches correctly on the freshly re-fetched operation's own state (`stopping`/
  `stop_retrying` → finalize-or-degrade, `evidence_degraded` → recover-or-refresh via one shared
  `run_gather_classify_and_persist` pipeline), so exactly one coordinator call site is needed, not two
- `AutonomousDailyFinalizationOutcome::EvidenceDegraded` gained one new field, `newly_applied: bool`,
  threaded through every CAS branch: `TransitionOutcome::Applied`/`RefreshOutcome::Applied` (a fresh
  transition or a genuinely changed same-state refresh) → `true`; `AlreadyApplied` (an exact-reason
  replay) → `false`; the authoritative re-read fallback after an ambiguous write → always `false`,
  since that path can never prove *this* invocation newly advanced durable truth — the sole,
  durable-CAS-derived dedup authority for the new warning notification, never process-local memory
- `AutonomousDailyCoordinatorTickOutcome` gained six new bounded typed variants
  (`OutcomeFinalized`/`OutcomeAlreadyFinalized`/`OutcomeEvidenceDegraded`/`OutcomeRecoveredToStopping`/
  `OutcomeFinalizationDatabaseUnavailable`/`OutcomeFinalizationConflict`), each carrying only bounded
  typed facts (operation/run identity, terminal/unknown reason code, `newly_applied`) — never a raw
  `anyhow` error, SQL text, connection string, filesystem path, provider payload, or panic string;
  `project_finalization_outcome` maps every one of E2B's seven results onto exactly one of them
- `session_controller.rs`'s `log_coordinator_outcome` match is not a wildcard — every new variant
  required its own explicit arm. `OutcomeFinalized` sends exactly one `discord_notifier.notify_run_status`
  notification (event `autonomous.daily_operation.outcome`) gated structurally on E2B's `Finalized`
  result only (never `AlreadyFinalized`); `OutcomeEvidenceDegraded` sends exactly one
  `discord_notifier.notify_critical_alert` warning (`severity: "warning"`, alert_class
  `autonomous.daily_operation.evidence_degraded`) gated on `newly_applied`, mirroring the existing
  `ManualInterventionRequired` REPAIR-11 pattern; `OutcomeAlreadyFinalized`/
  `OutcomeFinalizationDatabaseUnavailable`/`OutcomeFinalizationConflict` never notify. A notifier
  delivery failure (a no-op notifier, or an unreachable webhook) never rolls back, rewrites, or
  downgrades the already-committed terminal row — the DB write and the notification send are two
  independent steps, and the DB write already committed before `log_coordinator_outcome` is ever
  invoked
- `tests/scenario_autonomous_daily_outcome_coordinator_integration_01.rs` (15 tests as originally
  accepted, all real-DB-backed against the real production `run_durable_session_controller_tick` seam
  — the same coordinator-tick-plus-notification seam production's poll loop calls — with an
  in-process loopback Discord webhook sink, no real network call anywhere): clean no-trade and
  fill-confirmed finalization; a bound `run_id` with no local runtime never blocks finalization;
  `stopped_at_utc = NULL` never invokes E2B; exactly-once finalization and notification across
  repeats and restarts; evidence-degraded warning dedup (newly-applied vs. exact-reason replay);
  evidence repair recovers to `stopping`, not direct completion; a policy-resolution failure persists
  `unknown_assignment_identity_unavailable` with the same replay dedup; a generic `completed` row and
  a database-unavailable/conflict result never notify; a notifier no-op never alters durable terminal
  truth; the completed-bar production adapter never references the finalizer (redundant with the new
  guard's own check); finalization remains reachable through the resolution-failure fallback lookup —
  all 15 pass. A literal complete-DB-outage/partial-evidence-read-failure/live-concurrent-writer proof
  at the coordinator level remains covered instead by E2B's own accepted pure/store-level tests rather
  than fabricated at this level — see the E3 spec doc §13/§15 for the full reasoning.
  AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-GATE-REPAIR-01 adds a 16th
  test, `ci_03b_matching_local_runtime_blocks_policy_failure_without_write_or_notification`, proving
  at the coordinator integration level (via the existing `AppState::inject_running_loop_for_test`
  seam, not a full live execution loop) that a matching local runtime blocks finalization even when
  current policy/config resolution also fails the same tick — all 16 pass
- **no API route and no GUI surface were added** — those remain E4's job

After D4 and its evaluation-lineage repair (Phase D, accepted complete in full), the
four-times-corrected Phase E1 contract, Phase E2A (plus both repairs), Phase E2B (strict
classifier and finalization CAS, plus its terminal-truth-precedence repair), Phase E3 (coordinator
finalization integration and notification, plus its matching-runtime-policy-failure-gate repair),
Phase E4 (the read-only daily-operation API, plus both repairs), and Phase E5 (the integrated Phase
E closure proof) are all accepted — Phase E is accepted complete in full. Bundle 3 still requires
Phase F1's (the read-only GUI daily-operation truth projection) independent acceptance, then F2
operator runbook correction, F3 supervised soak-evidence preparation, and Phase G final closure.

### Operational meaning

Completion of Bundle 3 is the boundary for beginning a **supervised autonomous paper soak**.
It is not a live-capital authorization and it is not the end of the operational roadmap.

The intended post-Bundle-3 sequence is:

1. run final targeted and full locked-snapshot proof
2. start with operator-watched Paper + Alpaca sessions
3. collect roughly 10–20 clean autonomous sessions
4. close real-fill, reconcile, Discord, restart, and repeated-cycle evidence
5. complete Bundle 4 durable paper portfolio and P&L truth before treating accounting as restart-safe

This is a materially stronger operator posture than early scaffold state, but it is still **not**
a safe-live-capital blanket claim.

## Core principles

- **Deterministic inputs and outputs**
- **Explicit run lifecycle**
- **Integrity and risk gates before execution**
- **OMS-controlled order lifecycle**
- **Durable outbox / inbox truth**
- **Scenario-driven reliability validation**
- **Fail-closed operator posture where truth is missing**

## Repository structure

- `core-rs/` — authoritative Rust workspace
  - `crates/`
    - `mqk-config` — layered config loading and config-hash support
    - `mqk-db` — persistence, outbox/inbox, run lifecycle, broker mapping, proof-backed DB contracts
    - `mqk-audit` — audit and structured event support
    - `mqk-artifacts` — run artifact initialization and report writing
    - `mqk-cli` — CLI entrypoint
    - `mqk-execution` — broker gateway, order router, OMS state machine
    - `mqk-portfolio` — fill application and position/accounting behavior
    - `mqk-risk` — execution-boundary risk controls
    - `mqk-integrity` — stale/gap/disagreement controls
    - `mqk-reconcile` — broker snapshot normalization and mismatch handling
    - `mqk-strategy` — strategy interface layer
    - `mqk-backtest` — deterministic backtest engine
    - `mqk-promotion` — promotion/evaluation layer
    - `mqk-broker-paper` — deterministic paper broker adapter
    - `mqk-broker-alpaca` — Alpaca adapter under hardening
    - `mqk-daemon` — HTTP control plane
    - `mqk-runtime` — authoritative execution path
    - `mqk-testkit` — scenario-driven reliability harness
    - `mqk-md` — historical/provider market-data support
    - `mqk-isolation` — cross-engine isolation and anti-state-bleed support
    - `mqk-schemas` — shared schema contracts
  - `mqk-gui/` — Vite/React operator console
- `research-py/` — optional Python research CLI
- `config/` — layered config sets
- `scripts/` — repo-native helper and proof scripts
- `docs/` — specs, checklists, runbooks, audits
- `assets/` — branding and diagrams

Operationally, `MAIN` is the canonical engine.
`EXP` is a research-side experimental sandbox and should not be treated as readiness truth unless explicitly promoted.

## Local env-file workflow

The repo ships `.env.local.example` as the canonical local starting point.
It states that `.env.local` is loaded automatically by both `mqk-cli` and `mqk-daemon`.
That is true **when the file is in the current working directory used to launch them**.

### Practical rule

- launch from the **repo root** if you want a repo-root `.env.local` to auto-load
- if you launch from `core-rs/`, place a copy at `core-rs/.env.local` or export the needed env vars manually

This matters because many older command examples start with `cd core-rs`, while the snapshot keeps `.env.local.example` at repo root.

### Recommended local pattern

1. Copy the template:

```powershell
Copy-Item .env.local.example .env.local
```

2. Fill in the values you actually use.

3. For daemon and CLI runs that should auto-load repo-root `.env.local`, use repo-root launches such as:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- --help
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon
```

### What the local env file usually owns

At minimum, local runtime work normally needs:

- `MQK_DATABASE_URL`
- `MQK_OPERATOR_TOKEN`
- `MQK_DAEMON_DEPLOYMENT_MODE`
- `MQK_DAEMON_ADAPTER_ID`
- `ALPACA_API_KEY_PAPER`
- `ALPACA_API_SECRET_PAPER`

Optional but common entries include session-window overrides, Discord webhooks, and artifact/capital policy paths.

## Proof DB vs runtime DB

This repo now has a clearer local DB split than older docs suggested.
Do not collapse these into one mental model.

### Runtime/operator DB

Use a **runtime DB** for actual daemon, GUI, and autonomous paper work.
The template `.env.local.example` currently uses this runtime default:

```text
MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5432/mqk_dev
```

If you keep that default, a compatible local Postgres looks like this:

```powershell
docker run --name mqk-postgres-dev `
  -e POSTGRES_USER=postgres `
  -e POSTGRES_PASSWORD=postgres `
  -e POSTGRES_DB=mqk_dev `
  -p 5432:5432 `
  -d postgres:16
```

You can use a different runtime DB layout.
What matters is that your daemon and CLI point to the URL you actually configured.

### Proof DB

Use a **separate disposable proof DB** for proof work.
The recommended isolated manual example binds to `55432` specifically to avoid collisions with a normal runtime DB on `5432`.

```powershell
docker run --name mqk-postgres-proof `
  -e POSTGRES_USER=mqk `
  -e POSTGRES_PASSWORD=mqk `
  -e POSTGRES_DB=mqk_test `
  -p 55432:5432 `
  -d postgres:16
```

Sanity-check it:

```powershell
docker exec mqk-postgres-proof pg_isready -U mqk -d mqk_test
docker exec mqk-postgres-proof psql -U mqk -d mqk_test -c "select current_user, current_database();"
```

### DB proof bootstrap default

`scripts/db_proof_bootstrap.sh --start-postgres` has its own default local Docker path.
It starts or reuses a Postgres 16 container on **5432** and defaults to:

```text
postgres://mqk:mqk@127.0.0.1:5432/mqk_test
```

That is fine for quick proof work, but it is a different path from the isolated manual `55432` example above.

### Reality-test DB path

The snapshot also includes a committed autonomous paper reality-test PowerShell script at repo root:

- `autonomous_reality_test_paper.ps1.ps1`

That script intentionally uses its **own isolated Docker default path**:

- container: `mqk-reality-postgres`
- host port: `5440`
- DB user/password: `mqk` / `mqk`
- DB name: `mqk_v4`

That separation is deliberate.
Treat reality-test DB state as a different lane from both everyday runtime ops and proof DB work.

### Verify ports before trusting any default above

The ports above (`5432`, `55432`, `5440`) are *defaults*, not guarantees. On a machine that already
has long-running containers for other purposes — e.g. a persistent live-trading or paper-trading
Postgres container — one or more of those ports may already be bound to something you must not touch.
Before starting a new container or pointing `MQK_DATABASE_URL` at one of these defaults, run
`docker ps` and check the `PORTS` column for what is *actually* listening, not just what a doc or
script assumes. If a default port is already taken by something other than a disposable proof/test
container, pick a free port explicitly (check with `docker ps` first — don't just reuse a port a
container on this machine has used before) rather than colliding — and double-check with
`docker exec <container> psql -U <user> -c "select 1"` that the container you think you are talking
to is the one actually answering on that port; a stale host-side port forward (observed once on this
repo, recreating a container on a host port it had previously used) can otherwise make a correct
password look like authentication failure from outside the container, even though the same password
works fine via `docker exec` or Docker's internal network. If that happens, recreating the same
container on a *different* host port is the fastest fix — cheaper than re-debugging credentials.

## Prerequisites

### Core workspace

- Rust stable toolchain
- Docker

### GUI

- Node.js + npm

### Windows-specific

- Git Bash is useful because the repo-native DB proof harness is a shell script
- PowerShell is fine for Rust, Docker, daemon, GUI, and the root proof runner
- optional desktop bootstrap scripts exist under `scripts/windows/`, but the primary documented path remains daemon + browser GUI unless you have validated the desktop shell locally

## Database and proof model

### Canonical local proof harness

`full_repo_proof.ps1` at repo root is the authoritative local proof runner.
It runs the required lanes in sequence and writes a structured summary to `.proof/full_repo_proof_output.txt`.

```powershell
# Non-DB local proof
.\full_repo_proof.ps1 -ProofProfile local

# Low-memory Windows posture
.\full_repo_proof.ps1 -ProofProfile local -LowMemory

# Full DB-backed institutional proof against the isolated manual proof DB
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full

# Full DB-backed proof using the proven Windows low-memory profile
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full -LowMemory
```

When `-LowMemory` is active, the harness sets or preserves the proven Windows posture:

- `CARGO_BUILD_JOBS=1`
- `CARGO_INCREMENTAL=0`
- `RUSTFLAGS=-C debuginfo=0`
- all test lanes run with `--test-threads=1`

Use that profile on Windows hosts where linker or codegen parallelism causes OOM pressure.

### Repo-native DB proof bootstrap

`scripts/db_proof_bootstrap.sh` is the underlying DB proof harness invoked by `full_repo_proof.ps1` and by CI `db-proof`.

From repo root on Windows:

```powershell
& "C:\Program Files\Git\bin\bash.exe" -lc './scripts/db_proof_bootstrap.sh'
```

Or, to let the script start its own default `5432` proof DB container:

```powershell
& "C:\Program Files\Git\bin\bash.exe" -lc './scripts/db_proof_bootstrap.sh --start-postgres'
```

Or, to point it at the isolated manual proof DB on `55432`:

```powershell
& "C:\Program Files\Git\bin\bash.exe" -lc 'export MQK_DATABASE_URL="postgres://mqk:mqk@127.0.0.1:55432/mqk_test"; export DATABASE_URL="$MQK_DATABASE_URL"; ./scripts/db_proof_bootstrap.sh 2>&1 | tee db-proof.log'
```

What this proves:

- migration manifest and replay safety
- inbox dedupe and apply-fence behavior
- outbox idempotency, claim, and recovery behavior
- restart quarantine behavior
- runtime lease behavior
- deadman and runtime lifecycle behavior
- arm-preflight and DB constraint behavior
- market-data provider ingest and incremental sync DB behavior

Prefer running it through `full_repo_proof.ps1 -ProofProfile full` so the full lane set stays bundled.

### Local DB helpers

Also present in `scripts/`:

- `reset-mqk-testdb.ps1` — reset the local proof DB
- `psql-local.ps1` — interactive psql shortcut

Deprecated wrappers such as `test-all.ps1`, `test-db.ps1`, and `ci_gate.ps1` should not be used for operator validation.
The canonical local proof entrypoint is `full_repo_proof.ps1`.

## Core verification commands

All Rust commands below assume you are in `core-rs/`.

### Formatting, lint, and broad tests

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### GUI contract gate

```powershell
cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate
cargo test -p mqk-daemon --test scenario_route_contract_rt01
```

### GUI local truth checks

From `core-rs/mqk-gui/`:

```powershell
npm ci
npm run test
npm run build
```

### Focused execution, runtime, and broker checks

```powershell
cargo test -p mqk-execution --features testkit
cargo test -p mqk-broker-paper
cargo test -p mqk-broker-alpaca
cargo test -p mqk-runtime
cargo test -p mqk-testkit
```

### Workspace build

```powershell
cargo build --workspace
```

## Current deployment reality

This section is intentionally blunt.

### Valid daemon combinations today

Valid mode + adapter combinations with `deployment_start_allowed: true` include:

- `paper` mode + `alpaca` adapter — canonical honest paper path
- `live-shadow` mode + `alpaca` adapter — typed support, no current capital authority
- `live-capital` mode + `alpaca` adapter — typed support with additional gates, not operationally authorized here

### Fail-closed combinations

- `paper` mode + `paper` adapter — refused; not a valid start-authoritative daemon combination
- `live-shadow` or `live-capital` with `paper` adapter — refused
- any unrecognized adapter ID — refused
- `backtest` deployment in daemon runtime — unconditionally refused

### Strongest current operational path

The strongest current daemon path is canonical **Paper + Alpaca**.

Its current source-grounded capabilities include:

- durable daily-operation state in Postgres
- canonical NYSE-session planning and persisted operation boundaries
- strict daily-data-readiness gating before runtime start
- strategy-promotion gating before paper order-producing logic
- WS continuity and reconcile truth gates
- bounded typed start/recovery/stop behavior
- completed-bar-driven production task instead of the legacy blind timer
- durable observation and exactly-once dispatch-claim infrastructure, dispatched through the same
  exact-input strategy-dispatch seam the execution loop uses (D4)
- existing readiness, preflight, events, and alert surfaces

The path is still in **pre-soak hardening** because Bundle 3 is open. Phase D (D1–D4) is accepted
complete in full; the Phase E1 contract audit (four-times-corrected) is **accepted complete**; Phase
E2A (durable coverage-anchor/run-lineage evidence foundation), plus both repairs, is **accepted
complete**; Phase E2B (strict outcome classifier and finalization CAS) is **accepted complete**; Phase
E3 (coordinator finalization integration and notification) is **accepted complete**; Phase E4 (the
read-only daily-operation API, plus both repairs) is **accepted complete**; Phase E5 (the integrated
Phase E closure proof) is **accepted complete — Phase E is accepted complete in full**. Phase F1
(the read-only GUI daily-operation truth projection) is implementation complete, awaiting
acceptance. Do not label the current `main` head as a finished autonomous-paper MVP until F1's
acceptance and the later F2/F3/G phases are independently accepted.

### What is expected after Bundle 3

After Bundle 3 closes, the daemon should be able to remain running across supported NYSE sessions
and manage the daily Paper + Alpaca lifecycle without a person manually starting and stopping each
run.

The first deployment posture should still be:

```text
autonomous PAPER
+ operator watched
+ evidence captured
+ live capital disabled
```

Use the first sessions as a controlled soak. The project plan is roughly 10–20 clean sessions before
broader rollout. Bundle 4 then adds trustworthy durable paper cash, positions, lots, cost basis, and
daily realized/unrealized P&L across restarts.

### Completed-bar task configuration

Current D3 source adds:

```text
MQK_AUTONOMOUS_COMPLETED_BAR_TICK_SECS
```

Behavior:

- absent or blank: 15-second default
- allowed range: 1–300 seconds
- invalid, zero, negative, or out-of-range: task startup fails closed
- Tokio missed ticks are skipped rather than replayed in a burst

Provider network calls remain separately authorized by both:

```text
MQK_AUTONOMOUS_DATA_REFRESH_ENABLED=true
MQK_ALLOW_PROVIDER_API_CALLS=true
```

The completed-bar task may still use an exact trusted local bar when provider calls are disabled.
Do not enable provider calls merely to make startup succeed.

### Important vocabulary mismatch

- daemon deployment labels use `paper`, `live-shadow`, `live-capital`, and `backtest`
- `mqk run start` still uses the older run/config vocabulary: `BACKTEST | PAPER | LIVE`
- do not assume CLI `LIVE` maps one-to-one to daemon `live-shadow` versus `live-capital`

### Default bind posture

- default bind: `127.0.0.1:8899`
- non-loopback bind requires explicit opt-in through environment configuration

### Operator auth posture

If `MQK_OPERATOR_TOKEN` is not configured, privileged routes fail closed.

### Control-plane mode transitions

Mode transitions are restart-based, not hot-swapped.

Current truthful operator workflow:

- `change-system-mode` remains a guidance/compatibility path that returns `409`
- canonical operator actions include persisted restart-intent workflow through `/api/v1/ops/action`
- `request-mode-change` can persist a restart intent when the transition is admissible-with-restart
- `cancel-mode-transition` can cancel a pending durable restart intent
- the action catalog exposes restart workflows instead of pretending hot mode changes are authoritative

## CLI entry point

The CLI binary is `mqk`.

From repo root:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- --help
```

## CLI common operations

### DB status and migrations

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- db status
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- db migrate
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- db migrate --yes
```

Authoritative migration source:

- `core-rs/crates/mqk-db/migrations/`

Any tracked SQL file under another `/migrations/` path is rejected by migration governance guards.

### Config hash

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- config-hash config/defaults/base.yaml config/environments/windows-dev.yaml config/engines/main.yaml
```

### Market data — CSV ingest

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- md ingest-csv --path "<PATH_TO_CSV>" --timeframe "1D" --source "csv"
```

### Market data — provider ingest

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- md ingest-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D" `
  --start "2000-01-01" `
  --end "2026-01-01"
```

### Market data — incremental sync

First run, when no bars exist yet:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D" `
  --full-start "2020-01-01"
```

Subsequent incremental runs:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D"
```

Override end date or overlap:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY" `
  --timeframe "1D" `
  --end "2026-03-01" `
  --overlap-days 10
```

Notes:

- default overlap is 5 calendar days for `1D`, 2 days for `5m`, and 1 day for `1m`
- `--end` defaults to today for this operator-facing command
- `sync-provider` and `ingest-provider` share the same ingest path
- ingest ID is deterministic for identical inputs
- research and backtest paths should read from `md_bars` rather than calling providers directly

## Deterministic backtests

### Backtest from CSV

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- backtest csv `
  --bars "<PATH_TO_BARS_CSV>" `
  --timeframe-secs 60 `
  --initial-cash-micros 100000000000 `
  --integrity-enabled true `
  --integrity-stale-threshold-ticks 120 `
  --integrity-gap-tolerance-bars 0
```

Cash fields are integer micros. For a $100,000 backtest, enter `100000000000`; `100000` means $0.10, not $100,000. Using `100000` can make otherwise valid AAPL orders reject for insufficient cash. This applies to the GUI backtest form as well as CLI `--initial-cash-micros`.

Optional artifact output:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- backtest csv `
  --bars "<PATH_TO_BARS_CSV>" `
  --out-dir "runs/backtests"
```

### Backtest from Postgres `md_bars`

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- backtest db `
  --timeframe "1D" `
  --start-end-ts 946684800 `
  --end-end-ts 1704067200 `
  --symbols "SPY,QQQ"
```

Notes:

- `start_end_ts` and `end_end_ts` are epoch seconds over the `end_ts` bar range
- the backtest engine is deterministic, but promotion-grade provenance and realism are still being hardened

## Run lifecycle

Typical flow:

### Create a run

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run start `
  --engine "MAIN" `
  --mode "PAPER" `
  --config "config/defaults/base.yaml" `
  --config "config/environments/windows-dev.yaml" `
  --config "config/engines/main.yaml"
```

### Arm

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run arm --run-id "<RUN_ID>"
```

### Begin

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run begin --run-id "<RUN_ID>"
```

### Heartbeat

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run heartbeat --run-id "<RUN_ID>"
```

### Stop

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run stop --run-id "<RUN_ID>"
```

### Halt

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run halt --run-id "<RUN_ID>" --reason "manual halt"
```

### Status

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run status --run-id "<RUN_ID>"
```

### Deadman check

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run deadman-check --run-id "<RUN_ID>" --ttl-seconds 60
```

### Deadman enforce

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run deadman-enforce --run-id "<RUN_ID>" --ttl-seconds 60
```

Other helpers exist:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run --help
```

## Daemon

### Preferred local daemon launch

From repo root, with repo-root `.env.local` already configured:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon
```

Default local URL:

- `http://127.0.0.1:8899`

### Manual override example

If you prefer to launch from `core-rs/` instead, export env vars manually or keep a `core-rs/.env.local` copy.

```powershell
cd core-rs
$env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5432/mqk_dev"
$env:MQK_OPERATOR_TOKEN = "dev-local-operator-token"
$env:MQK_DAEMON_DEPLOYMENT_MODE = "paper"
$env:MQK_DAEMON_ADAPTER_ID = "alpaca"
$env:ALPACA_API_KEY_PAPER = "<your-paper-key>"
$env:ALPACA_API_SECRET_PAPER = "<your-paper-secret>"
cargo run -p mqk-daemon
```

Optional autonomous-operation variables:

```powershell
# Completed-bar worker cadence. Default 15; valid range 1-300 seconds.
$env:MQK_AUTONOMOUS_COMPLETED_BAR_TICK_SECS = "15"

# Only set both to true when real provider latest-bar calls are intentionally authorized.
$env:MQK_AUTONOMOUS_DATA_REFRESH_ENABLED = "true"
$env:MQK_ALLOW_PROVIDER_API_CALLS = "true"
```

Optional session override variables:

```powershell
$env:MQK_SESSION_START_HH_MM = "14:30"
$env:MQK_SESSION_STOP_HH_MM = "21:00"
```

Use session overrides only when you explicitly intend to replace the default NYSE regular-session
authority for a controlled test. Provider authorization is not required when the exact canonical
bar is already available locally.

### Useful daemon surfaces for the canonical paper path

- `GET /api/v1/system/status`
- `GET /api/v1/system/preflight`
- `GET /api/v1/autonomous/readiness`
- `GET /api/v1/alerts/active`
- `GET /api/v1/events/feed`
- `GET /api/v1/ops/catalog`
- `POST /api/v1/ops/action`

### Paper smoke review caveat

Current `scripts/windows/Review-PaperSmokeEvidence.ps1` derives `runtime_halted=true` by scanning captured `events_feed.json` rows for any `runtime_transition/HALTED` event. That check is not filtered by the current `run_id` or evidence window, so older HALTED events in the captured feed can set the flag. Treat `runtime_halted=true` as a review caveat and verify run_id/window context before using it as the current smoke verdict.

## GUI

Run from `core-rs/mqk-gui/`:

```powershell
npm ci
npm run build
npm run dev
```

Default dev URL:

- `http://127.0.0.1:1420`

Default daemon URL:

- `http://127.0.0.1:8899`

### Practical operator path

The practical repo-native operator flow today is still:

- run daemon
- run Vite GUI
- point the GUI at the daemon

### Optional Windows desktop bootstrap

An optional Windows desktop bootstrap exists under:

- `scripts/windows/Launch-VeritasLedger.ps1`
- `scripts/windows/Install-VeritasLedgerDesktopShortcut.ps1`

Intent of that path:

- desktop launcher verifies canonical local daemon identity before GUI open
- observe/attach and trade-ready launcher modes both exist
- desktop privileged actions are canonical-only, not legacy-fallback
- the launcher imports local env hints from repo-root and `core-rs` env files when present

Treat it as an operator convenience path that still requires local Windows validation on your machine.
The browser GUI + daemon path remains the primary documented workflow.

## One-shot local launch (two shells)

### Shell 1 — daemon

From repo root:

```powershell
cd C:\Users\<YOU>\Desktop\MiniQuantDeskV4
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon
```

### Shell 2 — GUI

```powershell
cd C:\Users\<YOU>\Desktop\MiniQuantDeskV4\core-rs\mqk-gui
npm ci
npm run dev
```

If you use `Start-Process`, keep the DB URL assignment quoted correctly inside the spawned command.

## Autonomous paper reality test

The repo includes a committed PowerShell reality-test harness at repo root:

- `autonomous_reality_test_paper.ps1.ps1`

Its job is different from normal proof or normal operator startup.
It unpacks a snapshot, provisions its own Docker Postgres container, launches the daemon, checks readiness, optionally injects a crash, and validates recovery behavior.

Default reality-test DB settings in the committed script:

- container: `mqk-reality-postgres`
- host port: `5440`
- DB name: `mqk_v4`

The script also looks for `.env.local` under both repo root and `core-rs/`.

Treat this as a dedicated reality-test lane, not your everyday operator startup path.

## Python research layer (optional)

From `research-py/`:

```powershell
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -U pip
.\.venv\Scripts\python.exe -m pip install -e .
.\.venv\Scripts\python.exe -m mqk_research.cli --help
```

This layer is intended to emit deterministic artifacts that the Rust stack can consume.

## CI overview

Current GitHub Actions coverage includes:

- **GUI contract lane** (`ubuntu-latest`)
  - GUI truth tests
  - GUI build
  - daemon/GUI contract gate

- **Safety guards** (`ubuntu-latest`)
  - unsafe-pattern checks
  - migration-governance checks
  - ignored-proof hygiene checks
  - workspace dependency inheritance guard

- **Rust lane** (`ubuntu-latest`, with Postgres service)
  - `cargo fmt --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`

- **DB proof lane** (`ubuntu-latest`, with Postgres service)
  - repo-native Postgres proof harness (`scripts/db_proof_bootstrap.sh`)

- **Windows platform lane** (`windows-latest`, no DB)
  - `cargo fmt --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace -- --test-threads=1`
  - `CARGO_BUILD_JOBS=1` + `CARGO_INCREMENTAL=0` + `RUSTFLAGS=-C debuginfo=0` reproduces the proven local `-LowMemory` posture

## Development discipline

This repo should be patched in small, test-backed units.

Recommended discipline:

1. change one invariant at a time
2. add or extend the scenario test that proves it
3. run targeted checks first
4. run broader checks after milestone patches
5. only commit once the patch and the directly affected surfaces are proven

## Current technical caveats

Be honest about these:

- Bundle 3 is not closed; Phase D (D1–D4, integrated lifecycle proof, dispatch-ownership race closure, and the evaluation-lineage repair) is accepted complete in full; the Phase E1 contract audit (the binding durable outcome/no-trade contract, four-times-corrected) is **accepted complete**; Phase E2A (durable coverage-anchor/run-lineage evidence foundation), plus both repairs, is **accepted complete**; Phase E2B (strict outcome classifier and finalization CAS, built on E2A's authorities) is **accepted complete**; Phase E3 (coordinator finalization integration and notification, built on E2B's classifier/CAS) is **accepted complete**; Phase E4 (the read-only daily-operation API, built on E3's accepted foundation, plus both repairs) is **accepted complete**; Phase E5 (the integrated Phase E closure proof, built on E1–E4's accepted foundation, zero production Rust change) is **accepted complete — Phase E is accepted complete in full**; Phase F1 (the read-only GUI daily-operation truth projection, built on E4's accepted API, zero daemon change) is implementation complete but awaiting independent ChatGPT/operator acceptance
- the current main branch should not begin an unattended soak until Phase F1's own independent acceptance and the later Bundle 3 phases (F2, F3, G) are accepted; controlled, operator-supervised autonomous Paper + Alpaca operation is the current Bundle 3 target, not unattended soak
- Bundle 4 durable paper cash/positions/lots/cost basis/P&L truth is still open
- real paper fill, reconcile-after-fill, Discord lifecycle, restart, and repeated-session evidence remain incomplete
- the daemon/operator plane is materially stronger, but some deeper GUI detail surfaces remain intentionally deferred or unmounted rather than faked
- the daemon has typed support for paper, live-shadow, and live-capital on Alpaca, but typed support is not the same thing as safe live operation
- the backtest system is strong, but still being hardened toward promotion-grade provenance and lifecycle realism
- shadow/live parity evidence is not yet strong enough for a safe unattended live claim
- scenario-tested does **not** mean profitable, broker-proof, or safe for live capital

## Reference docs

Useful repo docs:

- `docs/GUI_CONVERGENCE_CHECKLIST.md`
- `docs/ci/gui_daemon_contract_waivers.md`
- `docs/ci/dependency_governance.md`
- `docs/runbooks/operator_workflows.md`
- `docs/runbooks/autonomous_paper_ops.md`
- `docs/runbooks/live_shadow_operational_proof.md`
- `docs/runbooks/common_failure_modes.md`
- `docs/specs/`
- `docs/runbooks/`
- `docs/INSTITUTIONAL_READINESS_LOCK.md`
- `docs/INSTITUTIONAL_SCORECARD.md`
