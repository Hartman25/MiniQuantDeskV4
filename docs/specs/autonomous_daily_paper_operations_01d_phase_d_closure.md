# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D — Phase D Closure

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-INTEGRATED-LIFECYCLE-PROOF-AND-PHASE-D-CLOSURE`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Starting HEAD for D4: `98d6d1d82d7ddc0439498480b5d179eba2533d50` ("fix: close completed-bar
task supervision gaps" — D3 accepted). This document and the D4 code/test/ledger
changes described below are committed together in one commit on top of that HEAD;
see `MiniQuantDesk_Master_Patch_Ledger_v2.md`'s Bundle 3 entry for the exact
resulting commit hash.

Status: D4 implementation complete, **awaiting ChatGPT and operator acceptance**.
This document records what D4 built and proved; it is not itself an acceptance
record, and it does not close Phase D or Bundle 3 in the ledger.

## D4 repair layer: AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-EVALUATION-LINEAGE-AND-AUTONOMOUS-PREOPEN-CLOSURE-01

A second patch, layered on top of the D4 commit above (starting HEAD
`8b8d388c7e2fdca7c850ecb436c2ebce4f329382`, "fix: close autonomous phase D
integration"), closes the final gaps in D4's own dispatch-completion and
preopen proofs. See §12 below for full detail. Status: **implementation
complete, awaiting independent acceptance** — Phase D, Bundle 3, and this
repair itself remain OPEN in the ledger; nothing here is closed or accepted
by this document.

## 1. What Phase D covers

Phase D (D1–D4) is the production wiring, task supervision, and integrated-proof
layer built on top of Phase B's durable daily-operation coordination
(`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01B-*`) and Phase C's completed-bar driver
(`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01C-*`):

- **D1** — typed coordinator retry/reason-code policy
  (`autonomous_retry_policy.rs`), replacing string-matched classification.
- **D2** — the durable session coordinator itself
  (`autonomous_daily_coordinator.rs`: `tick_autonomous_daily_coordinator`),
  driving create/recover, preopen/open dispatch, canonical start, running
  reconciliation, recovery scheduling, session close, and stop retry — all
  through the existing canonical `start_execution_runtime` /
  `stop_execution_runtime` gate chains, never a parallel start/stop path.
- **D3** — the supervised completed-bar task
  (`autonomous_completed_bar_task.rs`): exactly-one task ownership, a
  monitor-wrapper supervisor core with bounded restart (3 restarts, 30s/60s/120s
  delays, 4 worker generations), durable permanent-failure degradation with a
  sticky operator-visible overlay, and cutover in `main.rs` from the legacy
  blind-timer ticker (`autonomous_bar_ticker.rs`, retained for compatibility
  tests only) to the supervised task.
- **D4 (this patch)** — closes the one confirmed cross-cutting defect left in
  the D1–D3 foundation (the completed-bar dispatch-ownership race, §3 below),
  adds a deterministic concurrency proof for it, and adds one integrated
  end-to-end scenario test driving a synthetic Paper+Alpaca day through the
  real production seams from D2/D3 together for the first time.

## 2. Production task graph (`main.rs`)

```text
main()
  ├── spawn_alpaca_paper_ws_task            (WS transport; no watchdog — pre-existing D-series gap, unchanged)
  ├── spawn_autonomous_session_controller   (durable coordinator poll loop; session_controller.rs
  │                                          -> run_durable_session_controller_tick
  │                                          -> tick_autonomous_daily_coordinator, unchanged by D4)
  ├── spawn_autonomous_completed_bar_driver_task   (D3; exactly one; see §5)
  └── graceful shutdown (Ctrl-C):
        cancel_and_wait_completed_bar_task_for_shutdown()   -- awaited first
        stop_for_shutdown()                                 -- execution runtime stop, second
```

`state::autonomous_bar_ticker` (the legacy blind-timer ticker) is never spawned
in production; it remains in source only for its own compatibility test
(`scenario_autonomous_completed_bar_task_01.rs`'s `i01`). D4 does not change
this wiring — it was already correct as of D3 and is re-verified by the new
Phase D closure guard (§8).

## 3. Dispatch-ownership race — audit, fix, and proof

### 3.1 Confirmed race (D4.1)

Before D4, `autonomous_completed_bar_driver::claim_and_dispatch_observed_bar`
(the function that runs once a fresh durable bar-dispatch claim is created)
dispatched a claimed bar by:

```rust
state.deposit_strategy_bar_input(StrategyBarInput { .. }).await;   // (1) deposit
state.tick_strategy_dispatch_for_symbol(symbol, timeframe).await   // (2) take + dispatch
```

`AppState::pending_strategy_bar_input` is a single, account-wide,
**destructive** mailbox. Every ordinary execution-loop tick also drains it
unconditionally, every ~1s, via
`tick_strategy_dispatch_multi_symbol`/`tick_strategy_dispatch`
(`loop_runner.rs`) — regardless of whether the completed-bar driver just
deposited into it. Because the completed-bar driver's claim and its own
dispatch of that claim were two separate `.await` points on the *same* shared
mailbox, a concurrent execution-loop tick could interleave between them:

```text
completed-bar driver: claim_autonomous_daily_bar_dispatch  -> Claimed
completed-bar driver: deposit_strategy_bar_input(bar)
                                                    execution loop tick: tick_strategy_dispatch_multi_symbol
                                                      -> takes the just-deposited bar, dispatches it
completed-bar driver: tick_strategy_dispatch_for_symbol -> takes None (mailbox already drained)
completed-bar driver: fail_autonomous_daily_bar_dispatch("... no result despite a fresh claim ...")
```

The claim is recorded **failed** even though a real strategy evaluation
occurred — for the execution loop's caller, against the wrong claim. This is
confirmed live-code, not theoretical: the execution loop's mailbox drain
(`loop_runner.rs:620-622`) runs unconditionally on every orchestrator tick with
no coordination with the completed-bar task at all.

Manual signal route (`routes/strategy.rs`) and the legacy ticker
(`autonomous_bar_ticker.rs`) are the mailbox's other producers; the ordinary
execution loop is its only consumer besides the (pre-D4) completed-bar driver.
Neither of those two producers is part of the race — only the completed-bar
driver's own deposit-then-immediately-consume sequence created the window.

### 3.2 Fix (D4.2)

`claim_and_dispatch_observed_bar` now calls the canonical exact-input dispatch
seam directly, never touching the shared mailbox:

```rust
let bar = StrategyBarInput { .. };
state.record_exact_bar_input_ts(bar_end_ts);   // B3 telemetry parity, see below
let dispatch_result = state
    .dispatch_native_strategy_for_symbol_with_bar(symbol, timeframe, bar)
    .await;
```

`dispatch_native_strategy_for_symbol_with_bar` is the same canonical native
strategy dispatch implementation `tick_strategy_dispatch_for_symbol` already
delegated to (DB-backed bar-context load, staleness gate, promotion/risk path,
decision journal — all unchanged) — it is simply invoked with this claim's own
`StrategyBarInput` value directly instead of round-tripping it through the
account-wide mailbox. This makes the claim's own evaluation **structurally**
independent of mailbox state: there is no longer any window in which a
concurrent execution-loop tick can consume the completed-bar driver's input.

Two small preserving changes accompanied the fix:

- **B3 telemetry parity**: `deposit_strategy_bar_input` was the sole writer of
  `last_bar_input_ts` (surfaced by `/api/v1/strategy/summary`). Since the
  claim path no longer calls it, `AppState::record_exact_bar_input_ts` records
  the same telemetry explicitly, so that operator surface does not go stale
  for completed-bar-driven dispatches.
- **D4.4 test hook**: a `None`-by-default rendezvous hook
  (`AutonomousCompletedBarPostClaimTestHook`) was added to `AppState`, checked
  once per `RunningDispatch` claim immediately after the claim and before the
  dispatch call. In production this is one uncontended `Option` check under an
  async mutex and never blocks; it exists solely so the concurrency proof
  (§3.3) can deterministically pause the completed-bar path at that exact
  point.

Execution-loop and manual-route dispatch (`tick_strategy_dispatch_multi_symbol`,
`tick_strategy_dispatch`, `deposit_strategy_bar_input`) are byte-for-byte
unchanged — they still read/write the mailbox exactly as before.

### 3.3 Claim completion truth (D4.3)

Unchanged in shape, now fed from the exact-input call instead of the mailbox
take:

```text
claim + exact-input dispatch returns Some(result)  -> complete_autonomous_daily_bar_dispatch -> DispatchCompleted
claim + exact-input dispatch returns None          -> fail_autonomous_daily_bar_dispatch      -> DispatchClaimUnresolved{FAILED}
claim already AlreadyCompleted                     -> AlreadyDispatched, zero dispatch call
claim Unresolved (prior failed/unknown)             -> DispatchClaimUnresolved, zero auto-redispatch
```

Both `complete_autonomous_daily_bar_dispatch` and
`fail_autonomous_daily_bar_dispatch` are `?`-propagated `PgPool` calls
unchanged from before D4 — a DB write failure on either path already
propagates as an `Err` out of `tick_autonomous_completed_bar_driver`, which the
production adapter and task record as a tick error (fail-closed; no retry
loop), not a silently-assumed completion.

### 3.4 Concurrency proof (D4.4)

`tests/scenario_autonomous_daily_phase_d_integration_01.rs`:

- `phase_d_concurrency_forward_ordering_execution_loop_cannot_steal_claimed_bar`
  — pauses the completed-bar path (via the D4.4 hook) immediately after its
  fresh claim and before the exact-input dispatch call, drives a concurrent
  execution-loop-style mailbox dispatch to completion via `tokio::join!`, then
  resumes the completed-bar path. Proves: exactly one evaluation for the
  claimed bar, the claim completes, the concurrent execution-loop tick
  independently and successfully dispatches its own (decoy) mailbox-deposited
  bar, the mailbox ends empty, no operation degradation, and a repeat tick is
  `AlreadyDispatched` with no second evaluation.
- `phase_d_concurrency_reverse_ordering_execution_loop_first_is_also_safe` —
  the inverse ordering: the execution-loop tick runs to completion first,
  fully draining the mailbox, before the completed-bar claim is even created.
  Proves the same claim-completion and no-degradation invariants hold
  regardless of ordering.

Both are deterministic: the synchronization point is an explicit
`tokio::sync::Notify` rendezvous (production no-op otherwise), never a sleep
used as assertion authority.

## 4. Integrated lifecycle proof (D4.5)

`phase_d_full_day_lifecycle` (same file) drives one isolated Paper+Alpaca
synthetic trading day through the real production seams, reusing the exact
real-start fixture pattern already proven by
`scenario_autonomous_paper_day_lifecycle_auton12.rs`'s AL-03 (synthetic
instrument/provider registry files, seeded canonical `md_bars`, a loopback
in-process mock Alpaca REST server, a registered `intraday_scalper` strategy, a
live broker cursor) — extended to additionally drive the completed-bar
production adapter and a real coordinator recovery cycle on top of that real
start, which AL-03 itself explicitly does not attempt.

Proven, in order, against a real isolated-DB fixture (no real provider,
broker, or network call):

1. **Preopen**: a durable operation is created; before any bar exists, the
   strict readiness gate correctly refuses (a genuine, pre-existing production
   behavior for a bar-less preopen instant — not weakened or changed by this
   patch); zero dispatch claim exists; zero strategy evaluation occurs.
   `select_driver_mode_for_state` is proven directly (pure, DB-independent) to
   map every pre-running state to `PrepareDataOnly` and `running` to
   `RunningDispatch`.
2. **Open and start**: the coordinator's canonical start binds an exact local
   `run_id`; the durable operation reaches `running` with a matching `run_id`;
   local runtime ownership binds to that exact `run_id`.
3. **Running dispatch**: a fresh claim is created and dispatches the exact
   expected bar exactly once (`DispatchCompleted`); the repeat tick is
   `AlreadyDispatched` with zero second evaluation.
4. **Runtime interruption and recovery**: the run is marked terminal without a
   halt (simulating a crash); a fresh `AppState` (simulating a restarted
   process, zero local ownership) observes it via `handle_running` and
   schedules `recovery_retrying` with a bounded future `next_retry_utc`; once
   due, the coordinator's canonical recovery start binds an exact
   **replacement** `run_id` (proven distinct from the original) and the
   operation returns to `running`; the already-dispatched bar is proven to
   remain `AlreadyDispatched` after recovery (zero reevaluation).
5. **Session close**: the coordinator's close tick stops the matching runtime
   canonically (`RuntimeStopped`); the operation reaches `stopping` with
   `stopped_at_utc` set; a subsequent completed-bar tick against the stopping
   state selects no automated driver invocation (`ModeNotApplicable`); zero
   orders were ever submitted across the whole synthetic day (`oms_outbox`
   count 0 for both run ids).

Task-supervision liveness/shutdown and permanent-failure truth are proven in
three separate, lighter, fully DB-backed fixtures in the same file
(`phase_d_task_liveness_then_shutdown_blocks_further_ticks`,
`phase_d_task_permanent_failure_degrades_operation_once_and_stays_visible`,
`phase_d_spawn_seam_starts_at_most_one_completed_bar_task`), reusing the
existing D3 supervisor machinery unchanged — D4 did not modify supervision,
restart-policy, or shutdown-ordering code, only the dispatch-ownership seam
inside one worker tick.

## 5. Restart policy (unchanged from D3)

`AutonomousCompletedBarRestartPolicy::production()`: 3 restarts, delays
30s/60s/120s. The final failing attempt is never counted as a successful
restart — 4 worker generations, `restart_count == 3` at exhaustion, then the
outer monitor applies durable permanent-failure degradation exactly once and
sends exactly one critical notification. Not touched by D4.

## 6. Runtime recovery behavior

Recovery is entirely coordinator-owned (`handle_running` /
`attempt_canonical_start` in `autonomous_daily_coordinator.rs`), unchanged by
D4: a terminal run without a halt schedules `recovery_retrying` with a bounded
future retry; once due, `attempt_canonical_start` re-enters the exact same
canonical `start_execution_runtime` gate chain a first start uses, binding a
new `run_id` only on success. D4 does not add a second start path, weaken any
gate, or change retry timing. What D4 newly proves (§4 point 4) is that this
existing recovery path composes correctly with the completed-bar driver: an
already-dispatched bar's durable claim survives a recovery cycle unchanged and
is never reevaluated.

## 7. Session-close and shutdown ordering

- **Session-close** (`handle_session_close`/`handle_stopping`/`retry_stop`,
  unchanged by D4): stops only a locally-owned runtime whose `run_id` matches
  the durable operation's own `run_id`; an operator-managed or mismatched
  runtime is never touched. The completed-bar production adapter's own mode
  selection (`select_driver_mode_for_state`) returns `None` for `stopping` and
  every other non-preparing/non-running state, so no automated driver
  invocation happens once the operation has left `running`.
- **Shutdown** (`main.rs`, unchanged by D4):
  `cancel_and_wait_completed_bar_task_for_shutdown()` is awaited strictly
  before `stop_for_shutdown()`. The completed-bar task's cancellation is a
  bounded wait-then-abort sequence (20s graceful, 5s abort-wait) that always
  ends with the spawn claim released and no in-flight or future tick possible
  before the runtime shutdown path runs. D4's new
  `phase_d_task_liveness_then_shutdown_blocks_further_ticks` test proves this
  with a live, ticking supervised task rather than only a synthetic exit.

## 8. Durable/operator truth ownership

Unchanged by D4, and re-verified (not re-implemented) as part of this patch's
review:

- The durable `sys_autonomous_daily_operations` row/state machine is the sole
  daily-operation lifecycle authority. `session_controller.rs`'s own
  projections and the completed-bar task's process-local liveness truth are
  both strictly subordinate — neither can become lifecycle authority, and
  neither can hide the other's truth. Specifically:
  `AppState::autonomous_session_truth()` already overlays a sticky
  `CompletedBarDriverExited` truth over whatever the session-controller's own
  tick most recently set, and only clears when a *newer* task generation
  reports non-`Failed` liveness (D3's `k06`, re-exercised end-to-end by D4's
  `phase_d_task_permanent_failure_degrades_operation_once_and_stays_visible`).
- No new API route is added by D4 (none was authorized). Existing status/
  readiness route conversions (`routes/autonomous_paper_status.rs`,
  `routes/system.rs`'s `autonomous_readiness`) are untouched by this patch's
  diff and were not re-derived — D4's change surface is limited to one
  dispatch call site inside the completed-bar driver, plus the new test-only
  hook and telemetry helper on `AppState`.

## 9. Test matrix

New: `tests/scenario_autonomous_daily_phase_d_integration_01.rs` — 6 DB-backed
tests (`--include-ignored`), all passing against the isolated port-5434 test
database:

```text
phase_d_concurrency_forward_ordering_execution_loop_cannot_steal_claimed_bar
phase_d_concurrency_reverse_ordering_execution_loop_first_is_also_safe
phase_d_task_liveness_then_shutdown_blocks_further_ticks
phase_d_task_permanent_failure_degrades_operation_once_and_stays_visible
phase_d_spawn_seam_starts_at_most_one_completed_bar_task
phase_d_full_day_lifecycle
```

Regressions re-run clean against the same isolated DB (see the commit's
pre-commit report for exact pass counts): `scenario_autonomous_completed_bar_task_01`,
`scenario_autonomous_completed_bar_driver_01`,
`scenario_autonomous_daily_session_coordinator_01`,
`scenario_autonomous_paper_day_lifecycle_auton12` (AL-01/02/03, including
after this patch's new file has run against the same shared adapter-id slot —
proving no cross-file interference under the mandated one-binary-at-a-time
execution discipline), `scenario_daemon_runtime_lifecycle`,
`scenario_multi_symbol_dispatch_loop_01`, `scenario_per_symbol_bar_window_01`,
`scenario_signal_evaluation_journal_auton_no_signal_obs_01`,
`scenario_autonomous_daily_coordinator_policy_01`,
`scenario_autonomous_daily_operation_identity_01`,
`scenario_daily_data_readiness_start_gate_01`,
`scenario_native_strategy_bootstrap_daemon_b1b`,
`scenario_autonomous_gate_parity_auton11`,
`scenario_autonomous_daily_operation_lifecycle_01` (mqk-db),
`scenario_autonomous_daily_operation_store_01` (mqk-db),
`scenario_autonomous_daily_operation_data_evidence_01` (mqk-db), and the
legacy-ticker unit group (`state::autonomous_bar_ticker::tests::`).

## 10. Known limitations (not closed by this patch)

- **WS transport and reconcile-tick tasks still have no supervisor/watchdog**
  (pre-existing D-series gap, documented since Phase A; out of D4 scope).
- **No durable daily outcome/no-trade classification** (`completed` /
  `completed_no_trade` / `completed_with_activity`) exists yet — Phase E's
  job, explicitly not started here.
- **No read-only daily-operation API or GUI panel** — Phase E/F, explicitly
  not started here.
- **Single-symbol only** — multi-symbol autonomous rollout is out of scope for
  this bundle entirely.
- **No portfolio/P&L durability** — Bundle 4's job.
- **10–20-session unattended soak has not been run.** This patch's proof is a
  synthetic, isolated-DB, single-day integration test, not a live or extended
  soak.
- **Live capital is not ready** and is not touched by this bundle.

## 11. Explicit Phase E boundary

This patch does not begin, and its tests do not exercise, finalize, or assert:

- durable daily outcome / no-trade / completed-with-activity classification;
- any new read-only API route or GUI surface;
- multi-symbol autonomous rollout;
- new strategies, or any change to strategy or risk mathematics or sizing
  policy;
- live-capital enablement.

Phase E (durable daily outcome/no-trade classification and read-only API) is
the next authorized phase, and only after independent ChatGPT/operator
acceptance of this D4 patch.

## 12. AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-EVALUATION-LINEAGE-AND-AUTONOMOUS-PREOPEN-CLOSURE-01

Starting HEAD: `8b8d388c7e2fdca7c850ecb436c2ebce4f329382` ("fix: close
autonomous phase D integration"). Status: implementation complete, awaiting
independent acceptance. This repair closes five confirmed gaps in D4's own
dispatch-completion truth and preopen proof; it does not close Phase D,
Bundle 3, or itself.

### 12.1 Evaluation-lineage binding (REPAIRs 1–4)

Before this repair, `claim_and_dispatch_observed_bar`'s completion path had
two defects, both silent:

1. It called `complete_autonomous_daily_bar_dispatch(..., None)` on every
   success — the durable claim never recorded which
   `strategy_signal_evaluations` row actually proved the dispatch, even
   though the journal writer (`AppState::record_signal_evaluation`) always
   computes a deterministic identity for the row it writes.
2. It never inspected the completion write's own `Result<bool>` — a
   `Ok(false)` (guarded on `status = 'claimed'`, so it never should have
   fired for a fresh claim, but a race or a store error meant it went
   uninvestigated either way) or an `Err` was both treated as unconditional
   success by the trailing `?` and `Ok(DispatchCompleted)`.

A callback result from the canonical strategy dispatch call was therefore
sufficient, by itself, to report `DispatchCompleted` — even when no durable
evidence backed it (e.g. the strategy callback ran through the fallback
stub path rather than the DB-backed path that writes the journal row, or a
real config-drift between the assignment's configured strategy and the
bootstrap's actually-active strategy).

Fixes, all in `autonomous_completed_bar_driver.rs`:

- **Identity helper** (`AppState::derive_strategy_signal_evaluation_id`,
  `state.rs`): the exact pre-existing seed format
  (`mqk.signal-evaluation.v1|{run_id-or-none}|{strategy_id}|{symbol}|{timeframe}|{now_tick}`)
  extracted into one pure function. Both `record_signal_evaluation` and
  `claim_and_dispatch_observed_bar` call it — never a second,
  independently-derived algorithm.
- **Exact evaluation confirmation**: after the strategy dispatch call
  returns `Some(result)`, the claim path derives its own expected
  evaluation id from the claim's verified `operation.run_id`, bound
  `strategy_id`, normalized `symbol`/`timeframe`, and exact
  `StrategyBarInput.now_tick`; fetches that row via the new
  `mqk_db::fetch_strategy_signal_evaluation(pool, evaluation_id)` read-only
  helper; and requires `run_id`/`strategy_id`/`symbol`/`timeframe`/
  `decision_stage == "strategy_evaluated"` to all agree. A mismatch or
  absence returns the new
  `AutonomousCompletedBarDriverOutcome::DispatchEvaluationEvidenceMissing`
  outcome and marks the claim `failed` — never completed on an unconfirmed
  callback result alone.
- **Persisted evaluation id**: `complete_autonomous_daily_bar_dispatch` is
  now always called with `Some(expected_evaluation_id)`, never `None`.
- **Completion outcome honored**: the completion call's `Result<bool>` is
  captured (`let completion_result = ...`) and matched explicitly.
  `Ok(true)` → `DispatchCompleted`. `Ok(false)` or `Err` both route through
  one shared `reconfirm_dispatch_completion_or_fail_closed` re-read: only an
  exact `completed` claim row carrying the exact expected evaluation id is
  accepted as committed truth (a write that landed but whose acknowledgement
  was lost); anything else — still `claimed`, a different evaluation id, or
  the re-read itself failing — returns the new
  `AutonomousCompletedBarDriverOutcome::DispatchCompletionUnconfirmed`
  outcome. The strategy evaluation is never automatically rerun on any of
  these paths (the claim's own `claimed`/`failed`/`uncertain` status
  continues to block automatic redispatch exactly as before this repair).
- **Test-only commit-uncertainty seam**: `AppState::completed_bar_completion_fault_test_hook`
  (`false` in production) lets a test make `claim_and_dispatch_observed_bar`
  skip the real completion write and take the `Err` branch, so the
  re-read-on-store-error path is provable deterministically without a real
  DB fault.

Proof: `scenario_autonomous_daily_phase_d_integration_01.rs`'s
`phase_d_missing_evaluation_evidence_fails_closed_never_completes_claim` and
`phase_d_completion_store_error_reconfirms_via_authoritative_re_read_and_fails_closed`,
plus `mqk-db`'s `scenario_autonomous_daily_operation_data_evidence_01.rs`'s
`exact_evaluation_row_lookup_and_completed_claim_stores_evaluation_id`. One
pre-existing test (`scenario_autonomous_completed_bar_driver_01.rs`'s
`repair10_17_restart_does_not_redispatch_completed_exact_bar`) had asserted
the old `evaluation_id: None` bug as correct behavior; its assertion is
updated to expect the now-persisted id.

### 12.2 Concurrency decoy bar-identity correction (REPAIR 5)

`phase_d_concurrency_forward_ordering_execution_loop_cannot_steal_claimed_bar`
and its reverse-ordering sibling previously deposited their "unrelated
execution-loop" decoy bar with the *same* `end_ts` as the completed-bar
claim's own expected bar — `now_tick` differed (`999` vs. the claim's own
tick), which already kept the two evaluations' identities distinct, but the
decoy's own bar identity was not actually distinct, weakening the proof's
own narrative. Both tests now seed a genuinely separate, independently
observable prior bar (`decoy_ts = expected_ts - 300`) via a new
`seed_light_bars` helper (accepts a slice, unlike the single-bar
`seed_light_bar` it replaces, which wiped on every call and so could not
build up a multi-bar window). Both tests additionally now assert the
claimed bar's evaluation id is durably stored, that its exact evaluation row
exists exactly once, and that the repeat tick reports that same stored id.

### 12.3 Real autonomous preopen proof (REPAIR 6)

`phase_d_full_day_lifecycle` previously called `apply_transition` to
manually clear `manual_intervention_required` at preopen — a genuine
readiness block caused by the fixture leaving `md_bars` empty at that point,
worked around rather than resolved. The fix seeds the *correct* preopen-
relevant bar window before any tick, rather than skipping the block:

- The completed-bar driver's own readiness evaluation is keyed on whatever
  `now_utc` its caller passes — the preopen tick passes `preopen_now`, not
  the later `pd_now()` used for the running/dispatch phase.
- At a genuine preopen instant (before the current session's own grid has
  closed any bar), `expected_intraday_end_ts_window` spills entirely into
  the *previous* trading session's own tail grid (its own documented
  behavior) — a different, earlier bar window than `pd_expected_bar_window`
  (today's window).
- A new `pd_preopen_expected_bar_window(preopen_now)` helper computes that
  exact tail window; its bars are seeded *before* the preopen tick. The
  later today-dated window continues to be seeded immediately before the
  "open and start" phase, exactly as before — seeding it any earlier would
  make the readiness gate see bars provenanced in the future relative to
  `preopen_now` (a real `latest_bar_future` block, confirmed empirically
  while building this fix), which is not a shortcut to avoid but a correct
  gate this patch must not weaken.

With the correct window seeded, the preopen tick resolves to `preparing_data`
(no blocker) instead of `manual_intervention_required`, and
`apply_transition` is no longer called anywhere in the happy path. The test
now asserts, exactly: the coordinator preopen tick never reaches `Started`;
zero dispatch claim and zero strategy evaluation exist before start; the
real production completed-bar adapter (`tick_autonomous_completed_bar_driver_from_state`)
selects `PrepareDataOnly` and returns exactly
`BarObserved { bar_end_ts: preopen_expected_ts }`; the operation row never
enters `manual_intervention_required`; the exact preopen bar is durably
recorded as observed; and zero provider calls were made (the bar was
already local).

### 12.4 Supervised task under an injected clock (REPAIR 7)

`m01_task_level_prepare_to_running_exactly_once` (in
`scenario_autonomous_completed_bar_task_01.rs`) already proved the
production adapter's `PrepareDataOnly` → `RunningDispatch` transition
thoroughly, but by calling `tick_autonomous_completed_bar_driver_from_state`
directly — it never exercised the supervised task (spawn/supervisor/
cancellation) at all, which the mission's own review flagged as
insufficient on its own.

A narrow test-only clock seam was added around the existing production task
tick: `AppState::completed_bar_task_clock_override` (`None` in production,
never installed there) and its accessors
`set_completed_bar_task_clock_override_for_test` /
`completed_bar_task_tick_clock`. `autonomous_completed_bar_task::run_one_production_tick`
now resolves `now_utc` via `state.completed_bar_task_tick_clock().await`
instead of calling `Utc::now()` directly; that method returns the installed
override if present, else `Utc::now()` — production behavior is unchanged
because the seam is never installed in production code.

The new test `n01_supervised_task_drives_real_adapter_under_injected_clock`
reuses m01's exact real fixture (registries, seeded bars, mock Alpaca
server, real coordinator start) but drives every tick through the real
`spawn_autonomous_completed_bar_driver_task` (the same task ownership,
supervisor, and cancellation production uses) under the injected clock, and
proves: the live supervised task invokes the real production adapter in
`PrepareDataOnly` under a controlled preopen instant with zero claims/
evaluations; after a real coordinator start, advancing the injected clock
into the dispatch window makes the same live task invoke the real adapter
in `RunningDispatch` and complete the dispatch exactly once; and
`cancel_and_wait_completed_bar_task_for_shutdown` cancels and awaits that
same task, after which no further tick occurs.

### 12.5 Scope boundary

Not touched by this repair: strategy/risk mathematics, multi-symbol
rollout, any API route or GUI surface, Phase E daily-outcome/no-trade
classification, the legacy ticker, migrations, or any provider/broker/
network/order path. `scripts/guards/validate_autonomous_daily_paper_operations_01d4_evaluation_lineage_and_autonomous_preopen_closure_01.ps1`
is the source-aware static guard for this repair specifically; the original
D3/D4 guard (`validate_autonomous_daily_paper_operations_01d_phase_d_closure.ps1`)
remains unmodified and continues to pass, since D4.2's dispatch-ownership
fix it validates is untouched by this repair.
