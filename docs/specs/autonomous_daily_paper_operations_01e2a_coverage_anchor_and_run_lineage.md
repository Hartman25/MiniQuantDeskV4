# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A — Coverage Anchor and Run Lineage Foundation

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-FOUNDATION`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Phase: Phase E2A — durable coverage-anchor and run-lineage evidence foundation.

Starting HEAD: `3591064a805efc82b3f6468e1de0fe06ea028471` (`docs: require coverage authority before
bar processing`) — the accepted E1 contract commit. This document and the E2A code/test/ledger changes
described below are committed together in one commit on top of that HEAD; see
`MiniQuantDesk_Master_Patch_Ledger_v2.md`'s Bundle 3 entry for the exact resulting commit hash.

**Closure repair** (`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-AUTHORITY-ENVELOPE-GATE-ORDERING-AND-
CONCURRENCY-CLOSURE`, applied on top of the original E2A implementation commit `6c77b6f1de3f7fd1075
fa8a7e3410b593d3d3253`, "daemon: bind autonomous coverage authority"): closes six defects left in the
original implementation below, found by a fresh, targeted re-read of `autonomous_daily_coverage_authority.rs`
and `autonomous_completed_bar_task.rs`: (1) every authority read now validates the row's complete
envelope (`id`/`event_type`/`source`/`run_id IS NULL`/`resume_source IS NULL`) via one shared
`validate_coverage_authority_envelope` helper, not merely the deterministic id plus a matching JSON
detail payload (§2.1, §4, new); (2) the JSON parser now decodes directly into a typed,
`#[serde(deny_unknown_fields)]` wire struct instead of a `serde_json::Value` object map, so a literal
duplicate JSON key is rejected by serde's own derived `visit_map` rather than silently collapsed to its
last occurrence — the original §2.2 claim that duplicate-key detection was unnecessary because
`serde_json` "deduplicates... before this code ever sees it" is retracted as a source-proven defect
(§2.2, corrected); (3) the read/parse/policy-comparison responsibilities are split into a Stage A
(authority presence and envelope, requiring no assignment/runtime/policy resolution of any kind) and a
Stage B (semantic comparison against the one value Stage A already loaded), never re-read or re-parsed
through a second algorithm (§4, new); (4) the completed-bar adapter's gate is reordered so Stage A runs
strictly before any assignment/runtime-binding resolution is even attempted — the original ordering
resolved assignment/runtime/policy configuration (and durably applied an `IdentityUnresolved` blocker
on failure) *before* ever checking whether an authority existed at all, so a newly-visible, unanchored
operation racing against a locally malformed environment could receive a lifecycle mutation the mission
requires stay a quiet no-op (§6, corrected); (5) `write_and_confirm_coverage_authority`'s re-read now
goes through the same Stage A envelope validator, so a row with any single tampered envelope field is
rejected without ever overwriting the original (§5, new); (6) the coordinator/adapter inter-task race
is now proven by a live `tokio::sync::Notify`-based rendezvous hook (`AutonomousCoverageAuthorityPreBindTestHook`,
mirroring the existing D4.4 `AutonomousCompletedBarPostClaimTestHook` pattern) driving the real
coordinator tick and the real production adapter tick concurrently via `tokio::join!` — §10's and §12's
prior "known limitation" (a deterministic sequential reconstruction standing in for a literal
concurrent-task proof) is resolved and retracted below. This closure repair does not redo the original
E2A audit except where a repair above required a fresh source read (recorded inline); it is not itself
an acceptance record, and it does not close Phase E or Bundle 3.

**Final proof repair** (`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-SAME-INSTANT-CONCURRENCY-AND-SIDE-
EFFECT-PROOF-01`, applied on top of the closure repair commit `98d6d1d8` / `6c77b6f1`): closes the one
remaining proof gap the closure repair's own §12 left implicit but did not actually deliver —
`f04_live_coordinator_and_adapter_interleaving_proves_zero_side_effects_while_paused` used two
*independently timestamped* ticks (`coordinator_now=12:00Z`, `adapter_now=13:05Z`), not one shared
logical instant, so it never proved the adapter observes the coordinator's *own* newly-created
operation as of the *same* `now_utc` the coordinator itself just used. `f04` is rewritten in place
(same test, not a new one) to: (1) tick both tasks at one shared `shared_now_utc=13:05Z`, inside
`[preopen_start_utc, effective_operation_open_utc)` — simultaneously a valid `PrepareDataOnly` instant
for the coordinator's own `initial_state_for_plan` and inside the adapter's own relevant-operation
lookup window; (2) capture a full durable before/after snapshot (state, `state_version`, `run_id`,
bar/claim/lifecycle-event/coverage-event/strategy-evaluation/decision counts) while task A is paused,
strictly before and after task B's own tick, proving zero delta from task B; (3) run task B's
paused-window tick under a deliberately invalid instrument-registry path, proving Stage A's
`coverage_authority_not_bound` outcome — never `RegistryUnavailable`/`IdentityUnresolved` — is reached
without the registry (or any provider client it could gate) ever being touched; (4) prove a later real
adapter tick, under a freshly-constructed validly-configured state, proceeds through the ordinary
eligible mode path (`DriverOutcome`) once released. Because a real coordinator dispatch through
`handle_preparing_data`'s readiness evaluation is now unavoidable at a shared post-preopen instant
(unlike this file's other coordinator-only fixtures, which stop before `preopen_start_utc`), `f04` also
gained a real 5-bar prior-session readiness fixture (provider registry + instrument registry + explicit
historical `ingested_at`, since `md_bars.ingested_at` is a real-wall-clock DB default the production
future-skew ceiling caps at 60s) and switched its strategy fixture from `swing_momentum` (native
timeframe 1D, silently mismatched against this file's `5m` assignment fixture) to `intraday_scalper`
(native timeframe 5m) — the mismatch was latent in every other test in this file, which never actually
reached the readiness evaluation that would have exposed it. §10/§11/§12 below are corrected in place.

Status: implementation complete, **awaiting ChatGPT and operator acceptance**. This document records
what E2A (both repairs) built and proved; it is not itself an acceptance record, and it does not close
Phase E or Bundle 3.

## 1. Scope

E2A implements exactly the durable evidence foundation the accepted E1 contract
(`docs/specs/autonomous_daily_paper_operations_01e_outcome_truth_contract.md`, §6a/§6b/§13)
authorizes:

1. an immutable, operation-scoped coverage-event model and parser;
2. a deterministic semantic comparison excluding changing metadata (the event row's own `ts_utc`);
3. an exact write/re-read/idempotent/conflict authority helper;
4. a coordinator ensure-authority seam;
5. a completed-bar adapter authority and mid-day compatibility gate;
6. a pristine-legacy-row binding rule;
7. a prior-activity missing-authority fail-closed rule;
8. a raw ordered full-run-lineage helper;
9. a restart and inter-task concurrency proof;
10. a mid-day policy-drift proof.

**No outcome classifier and no finalization behavior are implemented.** `outcome` and
`finalized_at_utc` remain unwritten by any production code path. No `completed*` transition is taken
by this patch. No new API route or GUI surface exists. No migration was required.

## 2. Coverage event schema

Store: the existing `sys_autonomous_session_events` table (migration `0032`), unchanged. New
`event_type` value only.

```text
id            : autonomous_daily_coverage_bound:{operation_id}
event_type    : autonomous_daily_coverage_bound
run_id        : NULL   (operation-scoped, not run-scoped)
source        : mqk-daemon.autonomous_daily_coordinator
ts_utc        : the bind instant — metadata only, excluded from semantic equality
detail        : the JSON payload below, serialized as text
```

The `(id)` primary key is the immutability mechanism: `ON CONFLICT (id) DO NOTHING` guarantees at
most one row per operation can ever exist, independent of any application-level convention.

### 2.1 Typed payload (`CoverageBoundDetail`)

Implemented in
`core-rs/crates/mqk-daemon/src/state/autonomous_daily_coverage_authority.rs`.

```text
schema_version                      i64, closed set, currently 1
operation_id                        UUID
market_date                         "YYYY-MM-DD"
deployment_mode                     e.g. "PAPER"
adapter_id

first_dispatchable_bar_end_ts       i64 (positive, canonical grid end_ts label)
final_dispatchable_bar_end_ts       i64 (positive, >= first)

local_symbol                        normalized (trimmed, uppercased)
timeframe                           normalized (Timeframe::as_str() canonical form)
timeframe_secs                      i64 (positive)
required_history_bars               i64 (positive)
effective_grace_seconds             i64 (nonnegative)

session_plan_identity               non-blank
assignment_identity                 non-blank
runtime_binding_identity            non-blank

exchange_session_open_utc           RFC3339
exchange_session_close_utc          RFC3339, > open
effective_operation_open_utc        RFC3339
effective_operation_close_utc       RFC3339, > open
```

Deliberately excludes `bound_at_utc`/any caller-`now` field. `#[derive(PartialEq)]` over exactly
these fields *is* the semantic-equality comparison the stable-replay rule requires — no separate
comparator function exists to drift out of sync with the struct definition.

### 2.2 Parsing (fail-closed)

`parse_coverage_bound_detail` requires the JSON object to have exactly the expected key set (no
fewer, no extra), every field to decode to its expected type, `schema_version == 1`, and every
semantic invariant above (non-blank identities, positive timeframe/history, nonnegative grace,
positive/ordered bar timestamps, ordered session boundaries) to hold. Any violation — malformed
JSON, a missing field, a wrong-typed field, an unknown schema version, or a semantic-invariant
violation — returns a typed `CoverageParseError`, never a partially-trusted value.

**Corrected by the closure repair.** The original claim above — that a duplicate key is harmlessly
deduplicated by `serde_json::Value` before this code ever sees it — is retracted: a `serde_json::Value`
object map's `.len()`/key-presence check cannot prove a duplicate key was *absent*, only that the
resulting map has the expected key set, which is exactly what a duplicated-then-collapsed key also
produces. `parse_coverage_bound_detail` now decodes directly into a typed wire struct
(`CoverageBoundDetailWire`, `#[derive(Deserialize)]`, `#[serde(deny_unknown_fields)]`) instead of a
`serde_json::Value` map. Serde's own derived `Visitor::visit_map` tracks each field in an `Option<T>`
and returns a `duplicate_field` error the second time any key is seen, before the value is ever
converted — this is standard `serde`/`serde_derive` struct-deserialization behavior, not a
custom detection this parser has to implement. `deny_unknown_fields` rejects any key outside the closed
set, and every field is non-`Option`, so a missing key remains a hard error exactly as before.

## 3. Canonical coverage construction

`construct_coverage_bound_detail` (pure, side-effect-free) reuses only
`daily_data_readiness::expected_intraday_end_ts_window` and
`daily_data_readiness::intraday_grid_starts` — no second calendar, timeframe, grace, or
completed-bar algorithm.

### 3.1 First dispatchable bar

The final element of `expected_intraday_end_ts_window(calendar_provider, schedule,
effective_operation_open_utc.timestamp(), timeframe_secs, effective_grace_seconds,
required_history_bars)`, where `schedule` is reconstructed directly from the operation's own
persisted exchange-calendar columns (`exchange_session_open_utc`/`_close_utc`/`exchange_is_early_close`/
`previous_trading_date`) — never a fresh env-driven calendar resolution. At an ordinary, on-time
market open this window has not yet observed any current-session bar, so its spillover branch fires
and the final element (hence the first dispatchable bar) is the **previous** trading session's own
final grid identity. Earlier elements of the same window are strategy-history context, never a
separate dispatch obligation. `started_at_utc` never enters this computation — the constructor has no
such parameter at all.

### 3.2 Final dispatchable bar

The last current-session grid identity (`intraday_grid_starts(exchange_session_open_utc,
exchange_session_close_utc, timeframe_secs)`) whose own expectation instant (`slot_start +
timeframe_secs + effective_grace_seconds`) is strictly greater than the first bar's own expectation
instant and strictly less than `effective_operation_close_utc`. When no such slot exists, the final
bar equals the first bar. The strict (`<`, not `<=`) upper bound means a bar whose expectation instant
lands exactly at `effective_operation_close_utc` is correctly excluded — proven in
`a01_ordinary_open_spills_into_previous_session_and_excludes_close_boundary_bar`, where the session's
own literal last grid slot is excluded for exactly this reason.

## 4. Semantic replay identity

The write/re-read/replay/conflict contract (`write_and_confirm_coverage_authority`,
`check_coverage_authority`) never trusts a write call's own `Ok(())` return. Every path re-reads the
row by its exact id and compares the parsed payload against the freshly-recomputed one:

```text
missing row after write attempt          -> Unreadable (retry later, never assumed persisted)
row present, unparseable                 -> Unreadable
row present, operation_id mismatch       -> Unreadable / Invalid
row present, payload == fresh (by #[derive(PartialEq)])
                                          -> Bound / Compatible (idempotent success)
row present, payload != fresh            -> Conflict (original never overwritten —
                                             the store has no UPDATE path at all)
```

Because `ts_utc` is excluded from the comparison, a tick recomputing the identical policy hours after
the original bind still reports an exact match — proven in
`d02_exact_replay_under_later_caller_time_is_idempotent_and_ts_unchanged`.

## 5. Coordinator ensure-authority flow

`ensure_coverage_authority` (`state/autonomous_daily_coordinator.rs`) runs immediately after
`create_or_recover` resolves the operation and strictly before any state-handler dispatch — for both
newly created and recovered operations, on every coordinator tick:

```text
resolve current policy (timeframe/history/grace from assignment + strategy registry)
  -> fail  => degrade (see §7), stop this tick
  -> ok    => construct fresh payload
              -> fail => degrade, stop this tick
              -> ok   => check existing coverage-bound event
                         NotBound       -> pristine? bind and continue : degrade (§7)
                         Compatible     -> continue to dispatch_by_state
                         Unreadable/
                         Invalid/
                         Conflict       -> degrade (§7), stop this tick
```

Close priority is preserved: at or after `effective_operation_close_utc`, canonical close/stop
reconciliation (`handle_session_close`) always takes precedence over a fresh coverage blocker — a
coverage-authority problem must never strand a runtime past close (proven in
`e04_close_priority_stops_runtime_even_with_a_coverage_conflict`).

## 6. Adapter prerequisite flow

**Corrected by the closure repair.** The original ordering resolved assignment/runtime-binding/policy
configuration — and durably applied an `IdentityUnresolved` blocker on any failure there — *before*
ever checking whether an authority existed for the operation at all. That meant a newly-visible,
not-yet-anchored operation racing against a locally malformed environment (a missing/invalid
`MQK_STRATEGY_*` env var, an unresolvable runtime binding) could receive a real lifecycle mutation the
mission requires stay a quiet no-op regardless of local configuration health. `tick_autonomous_completed_bar_driver_from_state`
(`state/autonomous_completed_bar_task.rs`) now enforces a two-stage ordering instead:

```text
fetch relevant operation
  -> select_driver_mode_for_state (state-only, cheap, non-authority-bearing short-circuit)
  -> Stage A: check_coverage_authority_envelope (exact-id lookup, envelope validation,
       duplicate-safe parse, payload operation-id verification -- read-only, never a write,
       requires no assignment/runtime-binding/policy resolution of any kind)
       NotBound    -> CoverageAuthorityUnavailable{coverage_authority_not_bound}, no mutation,
                       zero attempt to resolve assignment/runtime configuration
       Unreadable  -> CoverageAuthorityUnavailable{coverage_authority_unreadable}, no mutation
       Invalid     -> CoverageAuthorityUnavailable{coverage_authority_invalid}, no mutation
       Present     -> only then: resolve assignment/runtime-binding/policy metadata
                       (existing IdentityUnresolved blocker behavior preserved unchanged for
                       failures here, now reachable only once a real, correctly shaped
                       authority has already been proven present)
  -> Stage B: semantic comparison of the Stage A authority value against the freshly
       constructed payload (never re-read or re-parsed through a second algorithm)
       mismatch    -> CoverageAuthorityUnavailable{coverage_authority_conflict}, no mutation
       match       -> only then: load_driver_instruments, provider_id resolution,
                       readiness evaluator / provider resolver construction, driver invocation
```

No provider client is constructed, no provider call is made, no bar is observed, no dispatch claim is
made, and no strategy evaluation occurs before both stages resolve clean. The adapter never mutates
operation lifecycle state for any of the four coverage-authority reason codes — durable fail-closed
projection remains coordinator-owned, matching the dual-enforcement design (a concurrent adapter tick
and a coordinator tick are independently scheduled with no synchronization between them).

## 7. Legacy / prior-activity behavior

`check_operation_pristine` requires **all** of: `run_id IS NULL`, `started_at_utc IS NULL`,
`bars_observed = 0`, `bars_dispatched = 0`, `last_completed_bar_ts IS NULL`,
`last_dispatched_bar_ts IS NULL`, zero `sys_autonomous_daily_bar_dispatches` rows
(`mqk_db::count_autonomous_daily_bar_dispatch_claims`), and an empty validated run lineage (§8). A DB
read failure anywhere in this check is treated the same as `HasActivity` — never optimistically
`Pristine`.

```text
Pristine + missing anchor          -> coordinator binds a fresh anchor, tick proceeds
HasActivity (or unreadable evidence) + missing anchor
                                    -> coverage_authority_missing_after_activity
                                       running            -> evidence_degraded
                                       other nonterminal   -> manual_intervention_required
                                       already blocked/degraded -> same-state refresh
                                       terminal            -> no mutation
```

The anchor is never fabricated retroactively for a row with any activity signal. Reuses the existing
D1 typed blocker-signature mechanism (`AutonomousCoordinatorReason::UnclassifiedFailClosed`,
`blocker_signature`, `apply_manual_if_changed`) verbatim — no new lifecycle state, no new transition
edge.

## 8. Mid-day drift behavior

`resolve_current_coverage_policy_inputs` resolves `timeframe_secs`/`required_history_bars`/
`effective_grace_seconds`/`local_symbol`/`timeframe` from the assignment's own configured timeframe
and the strategy registry's data requirements, on every tick, independently on both the coordinator
and adapter sides. It deliberately does **not** cross-check
`EffectiveRuntimeBinding::effective_runtime_timeframe_secs` against the assignment's configured
timeframe — that fact is already `daily_data_readiness::evaluate_assignment`'s own
`runtime_strategy_timeframe_mismatch` readiness blocker, reported by the driver itself as a typed
`BindingBlocked`/`ReadinessBlocked` outcome; duplicating the check here would abort coverage
construction before the driver ever gets to report its own outcome for the same fact.

Any of the compared fields (`operation_id`, `market_date`, `deployment_mode`, `adapter_id`,
`local_symbol`, `timeframe`, `timeframe_secs`, `required_history_bars`, `effective_grace_seconds`,
`session_plan_identity`, `assignment_identity`, `runtime_binding_identity`, exchange/effective session
boundaries, first/final dispatchable-bar identity) disagreeing with the already-bound anchor produces
`coverage_authority_conflict` — no driver invocation, no provider call, no bar observation, no claim,
no evaluation. Proven for both a `PrepareDataOnly`-eligible state and a `RunningDispatch`-eligible
state (`g01`/`g02` in the test matrix below), using a changed `MQK_DATA_READINESS_GRACE_SECS` as the
concrete drift.

## 9. Run-lineage query and validator

```sql
select transition_seq, run_id
from sys_autonomous_daily_operation_events
where operation_id = $1
  and to_state = 'running'
  and run_id is not null
order by transition_seq asc
```

No `DISTINCT`, no `LIMIT` — `fetch_autonomous_daily_operation_running_transitions_raw`
(`mqk-db/src/autonomous_daily_operation.rs`) is a dedicated, narrow read, never routed through
`list_autonomous_daily_operation_events`'s bounded/ordered API-facing list.

`validate_autonomous_daily_operation_run_lineage` (pure Rust) requires: `transition_seq` strictly
increasing; each `run_id` appearing exactly once; the operation's current `run_id` column equal to the
lineage's final entry whenever non-`NULL`; an empty lineage legal only when the current `run_id` is
`NULL`. Any violation fails closed to a typed `RunLineageValidationError` (`DuplicateRunId`,
`NonMonotonicSequence`, `CurrentRunMismatch`) — never silently deduplicated or re-sorted.
`fetch_and_validate_autonomous_daily_operation_run_lineage` composes the read and the validation in
one call, the single site both the coordinator's pristine check and (in a future E2B) the classifier
use.

## 10. Restart and inter-task concurrency proof

Restart reconstruction is proven structurally: the coverage anchor and the run lineage are both
computed from durable, persisted facts only (the operation row's own columns, the coverage-bound
event row, and the raw transition-event rows) — a fresh process reading only these sources recomputes
byte-identical results, proven by `e01_coordinator_binds_pristine_anchor_and_replays_on_second_tick`'s
two independent ticks and `h01_initial_and_recovery_run_lineage_read_and_validated`'s two-run recovery
cycle.

The inter-task race (a concurrent completed-bar tick observing a newly-visible, not-yet-anchored
operation) is proven two ways. First, by direct construction of the worst case: the operation row is
made durable via a direct DB call with zero coverage anchor (exactly the row shape `create_or_recover`
alone produces before the coordinator's own tick reaches its ensure-authority write point), then the
real production adapter is ticked against it
(`f01_adapter_returns_not_bound_with_zero_side_effects_for_a_newly_visible_unanchored_operation`).
Second — by a live `tokio::sync::Notify`-based rendezvous hook
(`AutonomousCoverageAuthorityPreBindTestHook`, mirroring the existing D4.4
`AutonomousCompletedBarPostClaimTestHook` pattern) installed on the coordinator's own tick, firing
immediately after `create_or_recover` commits the operation row and before `ensure_coverage_authority`
begins — by `f04_live_coordinator_and_adapter_interleaving_proves_zero_side_effects_while_paused`, whose
final proof repair (see the preamble above) makes this a genuine same-instant proof:

- **One shared logical instant** (`shared_now_utc=13:05Z`, inside `[preopen_start_utc=13:00Z,
  effective_operation_open_utc=13:30Z)`) drives both the coordinator's own tick and the concurrent
  adapter tick — the adapter genuinely finds the exact operation row the coordinator just created, at
  the coordinator's own `now_utc`, not a separately-timestamped tick.
- **Genuinely separate tasks, lost-notification hazard structurally excluded**: task A (coordinator)
  and task B (adapter) are two independent futures polled via `tokio::join!` (not `tokio::spawn` —
  `Notify::notify_waiters` silently drops a notification with no registered waiter, and two
  independently OS-scheduled `tokio::spawn` tasks racing to register-vs-notify have no other ordering
  guarantee; `join!`'s poll-task-A-then-task-B-on-every-wake ordering, combined with task A's own
  `create_or_recover` DB `.await` before it can reach `notify_waiters()`, guarantees task B's first
  poll — which does nothing but register on `operation_visible` — runs strictly before task A could
  possibly notify it; the symmetric argument holds for task A's own `proceed.notified()` registration,
  issued with no intervening await immediately after `operation_visible.notify_waiters()`).
- **Full durable before/after snapshot** (not just state/bars/claims): `state`, `state_version`,
  `run_id`, `bars_observed`, `bars_dispatched`, `last_completed_bar_ts`, `last_dispatched_bar_ts`,
  operation lifecycle-event count, coverage-event count, dispatch-claim count, and
  `strategy_signal_evaluations`-scoped evaluation/decision counts, captured by task B itself
  immediately before and after its own tick while task A remains paused — every field proven identical
  across the delta. `oms_outbox`/`oms_inbox` are proven zero structurally (both tables carry `run_id
  uuid not null references runs`; `run_id.is_none()` is asserted in both snapshots) rather than by a
  raw table count, which would only measure unrelated concurrently-running tests' shared-table state.
- **Provider/client/driver non-access proof**: task B's paused-window tick runs under a deliberately
  invalid/sentinel instrument-registry path. Since Stage A (`check_coverage_authority_envelope`) runs
  strictly before any assignment/runtime-binding resolution or registry load, an outcome of the quiet
  `coverage_authority_not_bound` path — never `RegistryUnavailable`/`IdentityUnresolved` — is itself the
  proof that the registry, and any provider client construction it could gate, was never reached.
- **Release and normal progression**: once released, task A durably binds the authority; a later real
  adapter tick, under a freshly-constructed validly-configured state (the "restore valid local
  configuration" step) and the real 5-bar prior-session readiness fixture, proceeds to
  `DriverOutcome` — the ordinary eligible mode path, not merely a tolerated blocked one.

Together these prove the identical safety invariant the mission requires — the adapter never invokes
the driver, never constructs a provider client, and never observes/claims/evaluates a bar for an
unanchored operation — as a literal, same-instant, genuinely concurrent-task proof.

## 11. Test matrix

File: `core-rs/crates/mqk-daemon/tests/scenario_autonomous_daily_coverage_anchor_and_run_lineage_01.rs`
(41 tests, unchanged in count by the final proof repair — `f04` was rewritten in place, not added —
`--include-ignored --test-threads=1` against the isolated port-5434 test database; groups A/B/C/I are
pure, no DB required):

```text
Group A (construction, pure):       a01-a05
Group B (parse/serialize/equality): b01-b06 (b06 added: duplicate-JSON-key rejection)
Group C (run-lineage validator, pure): c01-c08
Group D (durable write/replay/conflict): d01-d09 (d05-d09 added: independent
                                          envelope-field tamper cases -- event_type,
                                          source, run_id, resume_source, detail.operation_id)
Group E (coordinator ensure-authority): e01-e04
Group F (adapter authority gate + concurrency proof): f01-f04 (f04 rewritten by the final
                                          proof repair: one shared logical instant, full
                                          before/after durable snapshot, malformed-registry
                                          provider-non-access proof, release+progression proof)
Group G (mid-day drift): g01-g02
Group H (run-lineage helper, DB-backed): h01-h02
Group I (policy-resolution error taxonomy, pure): i01
```

Regressions re-run clean against the same isolated DB (`--include-ignored --test-threads=1`, one
binary at a time): `scenario_autonomous_completed_bar_task_01` (49/49, including new
`bind_coverage_authority_for_test`-mediated fixtures for the four tests whose operations are
constructed directly rather than through the coordinator, plus a closure-repair fixture correction to
`l04_identity_unresolved_degrades_durably_through_the_adapter`: since Stage A now runs before
assignment resolution, exercising the `IdentityUnresolved` path requires binding a real authority first
under a resolvable assignment, then clearing the assignment env before the tick under test — the
production assertion is unchanged), `scenario_autonomous_daily_session_coordinator_01`
(48/48), `scenario_autonomous_daily_phase_d_integration_01` (8/8, including
`phase_d_full_day_lifecycle`'s real coordinator+adapter integration), `scenario_daily_data_readiness_start_gate_01`
(20/20, untouched by this patch), `scenario_autonomous_daily_operation_store_01` (mqk-db, 26/26),
`scenario_autonomous_daily_operation_lifecycle_01` (mqk-db, 36/36), `scenario_autonomous_daily_operation_data_evidence_01`
(mqk-db, 9/9).

`scenario_autonomous_completed_bar_driver_01` has 9 failures (`DispatchClaimUnresolved{status:"failed"}`
instead of `DispatchCompleted`) that are **confirmed pre-existing and unrelated to this patch**: the
file is untouched by either E2A repair (`git diff --stat` against both `3591064a` and the current HEAD
is empty), so the identical 9 tests failing identically is structural, not merely observed. 47/56 pass
against current HEAD, `3591064a`, the original E2A commit, and its closure repair alike.

## 12. Known limitations

- **Resolved by the closure repair, then corrected by the final proof repair (§10).** The inter-task
  concurrency proof was previously a deterministic sequential reconstruction of the worst-case ordering
  only. The closure repair added a live `tokio::join!`/`Notify`-synchronized interleaving
  (`AutonomousCoverageAuthorityPreBindTestHook`, mirroring D4.4's
  `AutonomousCompletedBarPostClaimTestHook` pattern) but drove the two tasks at two *independently
  timestamped* ticks rather than one shared logical instant — a gap this document's §12 did not itself
  identify at the time. The final proof repair corrects this: `f04` now drives both tasks at one shared
  `now_utc`, adds the full before/after durable snapshot, and adds the malformed-registry
  provider-non-access proof (§10).
- `scenario_autonomous_completed_bar_driver_01`'s 9 pre-existing failures are not fixed by this patch
  (out of scope — confirmed unrelated via baseline reproduction, §11).
- No new API route or GUI surface exists to read the coverage anchor or run lineage — E4's job.
- The exact SQL joins a future E2B classifier will use to scope activity/no-trade evidence to the
  full run lineage are not specified here — only the lineage-read/validate authority itself.
- `no_trade_all_signals_blocked` remains deferred per the accepted E1 contract (no durable
  `sys_risk_denial_events` correlation exists) — unaffected by, and out of scope for, E2A.

## 13. E2B boundary

E2A implements no outcome classifier and no finalization behavior. E2B (not started here, not
authorized until E2A is independently accepted) implements: the pure evidence-gathering reads (§1,
§5, §6 of the E1 contract) consuming E2A's coverage-anchor and run-lineage authorities rather than
recomputing them; the pure classification function (§8/§10 of the E1 contract); the finalization CAS
write path (`outcome`/`finalized_at_utc`, atomic with the terminal state transition); the two new
`stopping`/`stop_retrying -> evidence_degraded` legal-transition edges the E1 contract's §3.3
authorizes; and the §9 database-failure write contract (complete-outage vs. partial-read-failure,
authoritative re-read before any success claim). None of this exists yet. No coordinator call site
invokes a classifier. No `completed*` transition is reachable through any production code path added
by E2A.
