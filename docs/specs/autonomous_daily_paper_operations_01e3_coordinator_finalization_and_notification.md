# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3 — Coordinator Finalization Integration and Notification

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-COORDINATOR-FINALIZATION-INTEGRATION-AND-NOTIFICATION`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Phase: Phase E3 — coordinator finalization integration and outcome notification.

Starting HEAD: `1739864fcd27f6e29a995029ad758736df6974c4` (`fix: harden autonomous outcome
terminal truth` — the accepted E2B commit).

Status: **IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE.** This document
records what E3 built and proved; it is not itself an acceptance record, and it does not close
Phase E or Bundle 3.

## 0. Accepted foundation (recorded, not re-litigated)

```text
D1–D4: ACCEPTED — COMPLETE
PHASE D: ACCEPTED — COMPLETE
E1: ACCEPTED — COMPLETE
E2A: ACCEPTED — COMPLETE
E2B: ACCEPTED — COMPLETE
E3: IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE
PHASE E: OPEN
BUNDLE 3: OPEN
```

E3 is built entirely on top of E2B's accepted strict outcome classifier and finalization CAS
(`core-rs/crates/mqk-daemon/src/state/autonomous_daily_outcome.rs`,
`docs/specs/autonomous_daily_paper_operations_01e2b_outcome_classifier_and_finalization.md`) and the
E1 outcome contract's §12 notification contract
(`docs/specs/autonomous_daily_paper_operations_01e_outcome_truth_contract.md`). Neither the
classifier's evidence precedence, terminal reason codes, expected-bar derivation, coverage-anchor/
run-lineage authority, nor the finalization CAS itself is reopened or redesigned by this patch.

## 1. Scope

E3 implements exactly the production integration layer the E1 contract's §13 E3 decomposition and
the E2B module's own boundary (§14) authorize:

1. invoking the accepted E2B finalizer from the durable daily coordinator once stop completion is
   durably proven;
2. supplying E2B's process-local runtime-stop context and current policy inputs from the existing
   authoritative `AppState`/config seams `ensure_coverage_authority` already uses;
3. routing post-stop `evidence_degraded` operations through E2B's recovery path;
4. projecting every E2B result into a bounded typed coordinator outcome;
5. sending exactly one outcome notification for a newly finalized operation, and exactly one warning
   notification for a newly applied finalization evidence blocker;
6. proving restart safety from durable stop through finalization.

**No new API route, no GUI surface, and no schema change are introduced.** `outcome`/
`finalized_at_utc`/the two `evidence_degraded` edges were all already implemented by E2B; E3 adds no
new DB write primitive of its own beyond the one narrow blocker-persistence seam named in §4 below.

## 2. Finalization entry condition (E1 §3.2)

The coordinator invokes E2B's finalizer only when durable truth proves: `state` is `stopping` or
`stop_retrying`, or `state` is `evidence_degraded` with `stopped_at_utc` already present; and no
matching locally-owned runtime is active. The matching-local-runtime fact is derived exactly as E1
§3.2 condition 4 requires — `operation.run_id` compared against
`AppState::locally_owned_run_id()` — never `locally_started`, task liveness, a process-local bar
counter, or GUI state:

```rust
// core-rs/crates/mqk-daemon/src/state/autonomous_daily_coordinator.rs
async fn matching_local_runtime_active(
    state: &Arc<AppState>,
    operation: &AutonomousDailyOperationRecord,
) -> bool {
    match operation.run_id {
        Some(expected) => state.locally_owned_run_id().await == Some(expected),
        None => false,
    }
}
```

The tick that first records `stopped_at_utc` (inside `retry_stop`'s successful
`stop_execution_runtime()` branch, or the past-close first-observation branch in
`tick_autonomous_daily_coordinator`) continues to return the existing `RuntimeStopped`/
`AwaitingOutcomeFinalization` outcome unchanged — it never also attempts finalization on that same
tick, preserving the existing one-effect-per-tick structure. No coordinator code path issues a second
stop call merely to reach finalization.

## 3. Coordinator routing

`handle_stopping`'s top-of-function gate — previously `if operation.stopped_at_utc.is_some() {
return Ok(AwaitingOutcomeFinalization) }`, a permanently-repeating no-op once stop completed — now
routes into the new `handle_outcome_finalization` helper instead:

```text
stopping/stop_retrying + stopped_at_utc NULL     -> existing handle_stopping/retry_stop behavior (unchanged)
stopping/stop_retrying + stopped_at_utc present  -> handle_outcome_finalization (E2B finalization)
evidence_degraded + stopped_at_utc present       -> handle_outcome_finalization (E2B recovery/replay)
evidence_degraded + stopped_at_utc NULL          -> existing mid-run degraded/manual projection (unchanged)
completed/completed_no_trade/completed_with_activity -> read-only projection, no classifier re-run, no DB call
```

Because `handle_stopping` is the single call site shared by `dispatch_by_state`'s ordinary per-tick
routing **and** `reconcile_existing_operation_against_relevant_lookup`'s fallback-lookup routing
(itself reached only via `resolve_or_degrade_on_resolution_failure`/
`resolve_or_reconcile_on_nontrading_day`), this one change closes the finalization gap for every
production entry path named in the mission's §E3.11 audit list simultaneously. The parallel
`evidence_degraded` case required one additional, symmetric fix: both `dispatch_by_state`'s combined
`STATE_CONTROLLER_DEGRADED | STATE_EVIDENCE_DEGRADED` arm and
`reconcile_existing_operation_against_relevant_lookup`'s equivalent combined arm were split so that
`evidence_degraded` with a durable `stopped_at_utc` routes into `handle_outcome_finalization` rather
than into the generic, unrelated manual/controller-degraded blocker-refresh path — a real defect
found only by running the new test suite (§10 below) and confirmed by fresh source review of both
functions: without this fix, a subsequent resolution-failure tick would silently overwrite E2B's own
durable `unknown_*` reason with an unrelated resolution-failure reason on every tick, producing a
spurious repeated "newly applied" write and a spurious repeated warning notification. `handle_stopping`
and `dispatch_by_state`'s new `completed*` read-only arm never call the classifier a second time for
an already-terminal row (no DB round trip beyond the row already fetched that tick).

`handle_outcome_finalization` (`autonomous_daily_coordinator.rs`) is invoked at most once per tick, and:

1. computes `context.matching_local_runtime_active` (§2);
2. resolves current policy inputs (§4);
3. calls `state::autonomous_daily_outcome::classify_and_finalize_autonomous_daily_operation` once;
4. projects the result into a bounded typed coordinator outcome (§5).

E2B's own production entry point already internally distinguishes the `stopping`/`stop_retrying`
(finalize-or-degrade) and `evidence_degraded` (recover-or-refresh) starting states via one shared
`run_gather_classify_and_persist` pipeline reached from `classify_and_finalize_autonomous_daily_operation`
— confirmed by direct source read before this patch relied on it. E3 therefore needs exactly **one**
coordinator call site, not two: the "E2B finalization helper" and "E2B recovery/classification
helper" the mission names are the same function, correctly dispatching internally on the freshly
re-fetched operation's own state.

## 4. Current policy-input source (E3.3)

`AutonomousDailyFinalizationPolicyInputs` is resolved fresh, once per finalization attempt, from
exactly the same production authorities `ensure_coverage_authority` already threads through
`resolve_current_coverage_policy_inputs`/`construct_coverage_bound_detail` — no second environment
parser, no duplicate assignment algorithm, no duplicate runtime-binding algorithm, no duplicate
coverage construction, no cached process-local policy, no provider network call:

```text
config           = crate::state::build_multi_symbol_runtime_config_from_env()
runtime_context  = resolve_autonomous_runtime_context(state).await
readiness_context = crate::daily_data_readiness::load_readiness_context_from_env()

calendar_provider = readiness_context.calendar_provider.as_ref()
binding           = &runtime_context.effective_runtime_binding
strategy_registry = &readiness_context.strategy_registry
```

This is the identical three-call composition `tick_autonomous_daily_coordinator`/
`ensure_coverage_authority` already perform for coverage-anchor binding — E3 does not introduce a
fourth. `load_readiness_context_from_env` has no failure mode (it is infallible, matching its
existing use inside `ensure_coverage_authority`); the two calls that can fail are `config` and
`runtime_context`.

**Resolution-failure seam (E3.4).** When `config` or `runtime_context` resolution itself fails —
before evidence gathering can even begin, since there is no real `MultiSymbolRuntimeConfig`/
`EffectiveRuntimeBinding` to construct `AutonomousDailyFinalizationPolicyInputs` from —
`handle_outcome_finalization` persists `unknown_assignment_identity_unavailable` via one
narrowly-scoped, `pub` wrapper added to E2B's own module:

```rust
// core-rs/crates/mqk-daemon/src/state/autonomous_daily_outcome.rs
pub async fn persist_autonomous_daily_finalization_blocker(
    pool: &PgPool,
    operation: &AutonomousDailyOperationRecord,
    now_utc: DateTime<Utc>,
    reason: AutonomousDailyUnknownReason,
    context: AutonomousDailyFinalizationContext,
) -> anyhow::Result<AutonomousDailyFinalizationOutcome> {
    if !finalization_blocker_persistence_eligible(operation, &context) {
        return Ok(AutonomousDailyFinalizationOutcome::NotEligible);
    }
    apply_evidence_degraded_blocker(pool, operation, reason, now_utc,
        AutonomousDailyFinalizationEffectSeam::default()).await
}
```

This reuses the existing private `apply_evidence_degraded_blocker` verbatim — the exact same CAS/
same-state-refresh/authoritative-re-read machinery every other `evidence_degraded` blocker in E2B
already uses (it already branches correctly on `operation.state == STATE_EVIDENCE_DEGRADED` vs.
`stopping`/`stop_retrying`, so this seam behaves identically regardless of which finalization-eligible
state the operation is currently in). E2B remains the sole owner of blocker CAS, signature,
authoritative re-read, and replay semantics; the coordinator creates no second blocker writer and no
parallel classifier. No raw config, SQL, path, environment, or credential text ever enters this
seam's durable fields — only the closed `AutonomousDailyUnknownReason` code. A DB failure while
loading or persisting still returns the existing `DatabaseUnavailable` result via the same
`reread_confirm_evidence_degraded` path every other ambiguous write in E2B already uses.

### 4a. AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-GATE-REPAIR-01

**Confirmed defect (closed by this repair).** As originally implemented, `handle_outcome_finalization`
computed `context.matching_local_runtime_active` but never consulted it before calling
`build_multi_symbol_runtime_config_from_env`/`resolve_autonomous_runtime_context`/
`persist_autonomous_daily_finalization_blocker` in its two resolution-failure branches, and the
original `persist_autonomous_daily_finalization_blocker` performed no eligibility check of its own —
it delegated straight to `apply_evidence_degraded_blocker`. A `stopping`/`stop_retrying` operation
with `stopped_at_utc` present whose matching local runtime was still active, observed on a tick where
current policy/config/runtime-context resolution also failed, could therefore be incorrectly written
to `evidence_degraded` and warned about, even though E1 contract §3.2 condition 4 requires "no
matching locally-owned runtime is active" as a precondition for finalization of any kind — including
the evidence-blocker write, not merely the terminal CAS.

**Repair, two layers of defense:**

1. **Coordinator early gate.** `handle_outcome_finalization` now returns
   `AutonomousDailyCoordinatorTickOutcome::AwaitingOutcomeFinalization` immediately after computing
   `context`, strictly before `build_multi_symbol_runtime_config_from_env`,
   `resolve_autonomous_runtime_context`, `persist_autonomous_daily_finalization_blocker`, or
   `classify_and_finalize_autonomous_daily_operation` are ever called, whenever
   `context.matching_local_runtime_active` is `true`. Zero DB writes, zero notifications — identical
   truth to what E2B's own `check_finalization_eligibility`/`evidence_degraded` gate already produces
   on the successful-policy-resolution path, just reached without attempting policy resolution first.
2. **E2B wrapper hardening.** `persist_autonomous_daily_finalization_blocker` now requires the caller
   to supply `AutonomousDailyFinalizationContext` and gates on a new private
   `finalization_blocker_persistence_eligible` check — refusing (returning `NotEligible`, zero DB
   calls) whenever `context.matching_local_runtime_active` is `true`, `operation.stopped_at_utc` is
   absent, or `operation.state` is not `stopping`, `stop_retrying`, or an already `evidence_degraded`
   post-stop row. This is deliberate defense-in-depth: the seam must never become a second way to
   bypass finalization eligibility even if a future caller forgets the coordinator-level check. Both
   coordinator call sites now thread `context` through.

The pre-existing, still-valid policy-failure behavior (`stopped_at_utc` present, no matching local
runtime, policy resolution unavailable → `unknown_assignment_identity_unavailable` →
`evidence_degraded`, with exact-replay `newly_applied=false` and zero duplicate notification) is
unchanged and re-proven by `ci_16_17`/`store_47`.

**New proof:**

- `ci_03b_matching_local_runtime_blocks_policy_failure_without_write_or_notification`
  (`scenario_autonomous_daily_outcome_coordinator_integration_01.rs`) drives the real coordinator/
  session-controller integration path end-to-end and proves zero state/version/reason/signature/
  outcome change, zero lifecycle-event delta, and zero notification.
- `store_59_persist_finalization_blocker_refuses_when_matching_runtime_active`
  (`scenario_autonomous_daily_outcome_classifier_and_finalization_01.rs`) proves the hardened wrapper
  directly returns `NotEligible` with zero DB writes when supplied a matching-runtime-active context,
  and that the legitimate case is unaffected.
- `scripts/guards/validate_autonomous_daily_paper_operations_01e3_coordinator_finalization_and_notification.ps1`
  checks `[17]`–`[20]` statically enforce both defense layers and the presence of both new tests by
  exact name; both checks fail against the pre-repair commit and pass after this repair.

## 5. Coordinator outcome model (E3.5)

`AutonomousDailyCoordinatorTickOutcome` gains six new variants (`autonomous_daily_coordinator.rs`),
each carrying only bounded typed facts (operation/run identity, terminal state, terminal/unknown
reason code, `newly_applied`) — never a raw `anyhow` error, SQL text, connection string, filesystem
path, provider payload, or panic string:

```text
OutcomeFinalized { operation_id, run_id, outcome_reason_code }
OutcomeAlreadyFinalized { state, outcome_reason_code }
OutcomeEvidenceDegraded { operation_id, run_id, reason_code, newly_applied }
OutcomeRecoveredToStopping
OutcomeFinalizationDatabaseUnavailable
OutcomeFinalizationConflict
```

`project_finalization_outcome` maps every one of E2B's seven `AutonomousDailyFinalizationOutcome`
variants (`NotEligible`, `AlreadyFinalized`, `Finalized`, `EvidenceDegraded`, `RecoveredToStopping`,
`DatabaseUnavailable`, `Conflict`) onto exactly one coordinator variant — `NotEligible` maps to the
existing `AwaitingOutcomeFinalization` (a matching local runtime this tick, or the residual
`RuntimeStopUnproven` case, is honestly "still awaiting," not a new fact). `OutcomeAlreadyFinalized`
additionally covers the generic administrative `completed` state (read directly from
`dispatch_by_state`'s new `completed*` arm, never via the classifier) with `outcome_reason_code:
None` distinguishing it from a complete automatic terminal row.

`session_controller.rs`'s `log_coordinator_outcome` match is not a wildcard — every new variant
required its own explicit arm, so the compiler itself enforces the exhaustiveness the mission
requires; no new variant can silently fall through undiscarded.

## 6. Newly-applied authority (E3.6)

Notification dedup authority must come from durable CAS behavior, never process-local memory.
`AutonomousDailyFinalizationOutcome::EvidenceDegraded` gained one new field:

```rust
EvidenceDegraded {
    reason_code: AutonomousDailyUnknownReason,
    record: AutonomousDailyOperationRecord,
    newly_applied: bool,
}
```

Threaded through exactly the seams the mission names: `TransitionOutcome::Applied` (a fresh
`stopping`/`stop_retrying -> evidence_degraded` transition) → `true`; `TransitionOutcome::AlreadyApplied`
(an exact-reason replay) → `false`; `RefreshOutcome::Applied` (a same-state refresh whose reason/
signature genuinely changed) → `true`; `RefreshOutcome::AlreadyApplied` (an unchanged same-state
refresh) → `false`; the authoritative re-read fallback after an ambiguous write
(`reread_confirm_evidence_degraded`) → always `false`, since that path can never prove *this*
invocation was the one that newly advanced durable truth. `AutonomousDailyTerminalReason`'s own
`Finalized`/`AlreadyFinalized` distinction already carried this exact semantic with no change needed
(a fresh CAS `Applied` — or a confirmed-via-re-read commit — is `Finalized`; anything already-terminal
is `AlreadyFinalized`). No notification-dedup table, migration, or new `AppState` field was added —
`newly_applied` is carried entirely in the return-value chain from the DB write outward.

## 7. Terminal outcome notification (E3.7)

`log_coordinator_outcome`'s new `Outcome::OutcomeFinalized` arm sends exactly one notification via
the existing `discord_notifier.notify_run_status` dispatcher (the same one `Outcome::RuntimeStopped`
already uses), with the stable, documented event string `autonomous.daily_operation.outcome`. The
payload's `note` field carries `operation_id`/`outcome` in bounded text form; `run_id` is included
when durably available. Dedup is structural: this variant is produced only for E2B's `Finalized`
result (a fresh CAS apply, or a commit confirmed via authoritative re-read after an ambiguous write)
— never for `AlreadyFinalized`, which maps to the separate, never-notifying `OutcomeAlreadyFinalized`
variant. A same-tick replay, a later tick observing the same terminal row, and a restart all
therefore observe zero duplicate notifications by construction, proven end-to-end in §10.

A notifier delivery failure (a true no-op `DiscordNotifier`, or an unreachable webhook) never rolls
back, rewrites, or downgrades the already-committed terminal row — the DB write and the notification
send are two independent steps in `log_coordinator_outcome`, and the DB write already committed
before `log_coordinator_outcome` is ever invoked (it observes the tick's own return value). No real
Discord/network call is made anywhere in this patch's own tests.

## 8. Evidence-degraded warning (E3.8)

`log_coordinator_outcome`'s new `Outcome::OutcomeEvidenceDegraded` arm sends exactly one
`severity: "warning"` notification via the existing `discord_notifier.notify_critical_alert`
dispatcher (the same one `Outcome::ManualInterventionRequired` already uses at `severity: "critical"`),
with `alert_class: "autonomous.daily_operation.evidence_degraded"`, gated on `newly_applied` exactly
like the existing `ManualInterventionRequired` pattern (REPAIR 11 of the D2 lifecycle closure). A
new transition into `evidence_degraded`, or a durably changed blocker (different reason/signature),
produces `newly_applied: true` → one warning. An exact blocker replay produces `newly_applied: false`
→ zero additional warnings. A `DatabaseUnavailable` result without a confirmed blocker write produces
neither `OutcomeEvidenceDegraded` nor any notification — it maps to the separate, never-notifying
`OutcomeFinalizationDatabaseUnavailable` variant. This patch does not classify an evidence gap as
`severity: "critical"` — the E1 contract does not require it, and existing precedent
(`ManualInterventionRequired`) reserves `critical` for operator-blocking conditions.

## 9. Result projection (E3.9)

`log_coordinator_outcome` projects every new outcome onto the existing `AutonomousSessionTruth`
compatibility surface without introducing a new variant — `StoppedAtBoundary { detail }` (already
used for "stopped and awaiting end-of-day outcome finalization") is reused, with `detail` updated to
name the specific fact (`"operation finalized: outcome=... operation_id=..."`,
`"operation already finalized: state=... outcome=..."`, `"...evidence recovered; re-attempt
pending"`), and `StartRefused { detail }` (already used for `ManualInterventionRequired`) is reused
for `OutcomeEvidenceDegraded`. `OutcomeFinalizationDatabaseUnavailable`/`OutcomeFinalizationConflict`
leave the last-set truth unchanged rather than fabricating a new label for a rare, transient
condition — source review confirmed no existing variant honestly represents "backend unavailable"
without materially misrepresenting the fact, and per the mission's own instruction this alone does
not authorize widening `AutonomousSessionTruth`, so no new variant was added; the coordinator's own
typed `AutonomousDailyCoordinatorTickOutcome` (§5) is the durable, precise record of what happened,
independent of the compatibility-surface projection. No E4 API response field, route, or GUI
presentation is introduced by this projection.

## 10. Restart and replay proof (E3.10)

Proven end-to-end against the real, isolated port-5434 test database in
`core-rs/crates/mqk-daemon/tests/scenario_autonomous_daily_outcome_coordinator_integration_01.rs`,
driving the real production `run_durable_session_controller_tick` seam (the same function
`spawn_autonomous_session_controller`'s production poll loop calls — coordinator tick plus bounded
logging/notification together, exactly as production behaves):

1. **Stop-before-finalization restart** (`ci_09_10`): a durably stopped operation is finalized by a
   brand-new `Arc<AppState>` (no in-memory continuity with whatever process originally stopped it),
   then a second brand-new `AppState` observes the row read-only. Exactly one notification across
   both restarts.
2. **Restart after terminal commit** (`ci_09_10`, `ci_22`): a fresh process's first tick against an
   already-`completed_no_trade` row performs zero writes and sends zero notifications.
3. **Evidence blocker recovery** (`ci_14_15`): a stopped operation with one corrupted dispatch claim
   degrades to `evidence_degraded` with one warning; the same corrupted state replayed produces no
   duplicate warning (proven structurally by `ci_11_12`/`ci_16_17`'s replay assertions); the claim is
   repaired, the operation takes `evidence_degraded -> stopping` (no direct completion, `newly_applied`
   dedup unaffected), and only the *following* tick reaches terminal completion, with exactly one
   terminal notification in addition to the earlier warning.

No test uses a sleep-based assertion for authority — every dedup/newly-applied assertion is proven
against durable DB truth (`sys_autonomous_daily_operation_events` row-count deltas, `state`/
`state_reason_code`/`state_version` equality across ticks), with `tokio::time::sleep` used only to
allow the loopback HTTP sink's already-`.await`ed POST to be observed by the test's own assertion,
never as a substitute for a correctness proof.

## 11. Database-unavailable behavior

Unchanged from E2B (§9 of the E2B doc, §9 of the E1 contract): complete outage (the operation row
itself cannot be loaded) → `DatabaseUnavailable`, zero write attempts, zero notification, retried on
a future tick. Partial read failure (operation row loaded, a later evidence query fails) → a
best-effort `evidence_degraded` blocker write, confirmed by re-read → `EvidenceDegraded` (one
warning if newly applied); if that write itself fails or cannot be confirmed → `DatabaseUnavailable`,
never a fabricated "blocker written" claim. E3 introduces no new database-failure behavior of its
own beyond routing E2B's existing typed results into `OutcomeFinalizationDatabaseUnavailable`
(never-notifying).

## 12. Notification-failure behavior

A `DiscordNotifier` delivery failure (webhook unreachable, non-2xx response, timeout) is caught,
logged as `warn!`, and swallowed inside `DiscordNotifier::notify_run_status`/`notify_critical_alert`
themselves (pre-existing behavior, unchanged by this patch) — it never propagates back into
`log_coordinator_outcome`, and it can never affect the already-committed DB row `log_coordinator_outcome`
is merely reporting on. `ci_22_notification_noop_does_not_alter_terminal_db_truth` proves this
end-to-end with a true no-op notifier (`DiscordNotifier::noop()`, never a webhook URL at all).

## 13. Test matrix

File: `core-rs/crates/mqk-daemon/tests/scenario_autonomous_daily_outcome_coordinator_integration_01.rs`
(15 tests as originally accepted: 14 `#[ignore]` DB-backed `#[tokio::test]` integration tests
— including the dedicated typed-outcome-projection test, itself DB-backed and `#[ignore]` — plus
one non-DB, non-`#[ignore]` source-level guard-redundant `#[test]` unit test, `ci_24`. This
corrects the original count of "16: 14 DB-backed + one typed-outcome-projection + one non-DB unit
test" — the typed-outcome-projection test was miscounted as a separate category when it is in fact
one of the 14 DB-backed `#[ignore]` tests. AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-
POLICY-FAILURE-GATE-REPAIR-01 adds one further DB-backed `#[ignore]` test, `ci_03b`, bringing the
file to 16 tests total: 15 DB-backed `#[ignore]` `#[tokio::test]` plus the one non-DB `#[test]`.
Run with `--include-ignored --test-threads=1`, real port-5434 test DB, in-process loopback Discord
webhook sink, no real network call. All 16 pass.

```text
ci_01  clean no-trade operation finalizes through a real coordinator tick, one notification
ci_02  fill-evidence operation finalizes as activity_fill_confirmed
ci_03  a bound run_id with no local runtime never blocks finalization (§2's false side, proven
       end-to-end; the true/blocking side is proven at the pure-function level by E2B's own
       accepted eligibility_* tests, regression-run by this bundle -- see the file's own header
       comment for the documented reason a live-execution-loop integration proof was not built)
ci_03b AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-GATE-REPAIR-01: a
       matching local runtime blocks finalization even when current policy/config/runtime-context
       resolution itself also fails this tick -- the confirmed defect this repair closes. Injects
       a locally-owned execution loop bound to the operation's own durable run_id
       (`AppState::inject_running_loop_for_test`), clears strategy env so policy resolution
       genuinely fails, and proves zero state/version/reason/signature/outcome change, zero
       lifecycle-event delta, zero notification, through both a direct typed coordinator-tick call
       and the full notifying session-controller integration path
ci_04  stopped_at_utc = NULL preserves stop/retry handling; E2B is never invoked
ci_05_06  stop-completion tick defers; the next tick finalizes exactly once (one new lifecycle
          event, one notification total)
ci_07_08  a repeated terminal tick writes and notifies nothing
ci_09_10  restart before and after finalization is exactly once (see §10)
ci_11_12  a newly applied evidence blocker (missing coverage anchor) warns once; an exact replay
          warns zero additional times
ci_14_15  evidence repair recovers to stopping, not direct completion; the following tick performs
          terminal completion (see §10)
ci_16_17  a policy-resolution failure persists unknown_assignment_identity_unavailable; a replay
          writes and notifies zero additional times
ci_21  a generic administrative completed row is read-only
ci_22  notifier absence never alters durable terminal DB truth
ci_24  source-level: the completed-bar production adapter never references the finalizer
       (redundant with the new guard's own check, kept as a fast non-DB regression trip-wire)
ci_25  finalization remains reachable through the relevant-existing-operation fallback lookup after
       a resolution-failure tick -- a stopped operation is never abandoned
ci_typed_outcome_projection_...  tick_autonomous_daily_coordinator's own typed return value,
       called directly (not through the notifying wrapper), projects a clean finalization into
       exactly OutcomeFinalized -- proving the bounded typed projection in isolation
```

**Not separately proven as a dedicated numbered scenario** (noted, not fabricated, per the mission's
own allowance): a literal complete-DB-outage proof and a literal partial-evidence-read-failure-with-
effect-seam proof at the coordinator level were not built — both require injecting the
`AutonomousDailyFinalizationEffectSeam`/killing the connection pool mid-tick, and the production
`classify_and_finalize_autonomous_daily_operation` entry point the coordinator calls intentionally
never exposes that seam (only the E2B test-support entry point does). Both facts are already
exhaustively proven at the E2B store level (`store_46`, `store_48`, `store_49`, regression-run
clean by this patch, §14 below) and structurally unreachable for a coordinator-level injection
without adding a forbidden test-only seam to production code. A literal live-concurrent-writer
`Conflict` proof (distinct from the read-only `completed*`/`ci_21` proof already covering "no
repeated notification for an already-terminal row") was likewise not built, for the same reason
E2A's own concurrency proof required a dedicated `tokio::join!`/`Notify` rendezvous hook — building
an equivalent for this patch was judged out of proportion to what `store_53`–`store_58` already
prove at the CAS level.

## 14. Regressions

Re-run clean against the same isolated test DB, one binary at a time, `--include-ignored
--test-threads=1`:

```text
scenario_autonomous_daily_outcome_classifier_and_finalization_01   67/67 (66/66 as originally
                                                                     accepted; AUTONOMOUS-DAILY-
                                                                     PAPER-OPERATIONS-01E3-MATCHING-
                                                                     RUNTIME-POLICY-FAILURE-GATE-
                                                                     REPAIR-01 adds store_59)
scenario_autonomous_daily_session_coordinator_01                   48/48
scenario_autonomous_daily_phase_d_integration_01                    8/8
scenario_autonomous_daily_coverage_anchor_and_run_lineage_01        41/41
scenario_autonomous_completed_bar_task_01                           49/49
scenario_autonomous_paper_day_lifecycle_auton12                      3/3
scenario_signal_evaluation_journal_auton_no_signal_obs_01            7/7 (paper-DB-scoped cases
                                                                      self-skip without a paper DB
                                                                      env var -- unaffected by E3)
```

AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-POLICY-FAILURE-GATE-REPAIR-01 additionally
re-ran `scenario_autonomous_daily_outcome_coordinator_integration_01` clean (16/16, up from 15/15 as
originally accepted — see §13) and `scenario_autonomous_daily_session_coordinator_01`/
`scenario_autonomous_daily_phase_d_integration_01` clean at their existing 48/48 and 8/8 totals,
confirming the repair introduces no regression outside the two files it touches.

The E2B test binary required one mechanical, non-semantic change: three existing `EvidenceDegraded {
reason_code, record }` match patterns updated to `{ reason_code, record, newly_applied: _ }` for the
new field (§6) — no assertion's meaning changed. `scenario_autonomous_completed_bar_driver_01`'s
known baseline (47 passed, 9 pre-existing failures, confirmed unrelated by E2A's own audit) was not
re-run — E3 touches no production seam in that file's driver path.

## 15. Known limitations

- The matching-local-runtime **blocking** case (§2, §13) is proven at the pure-function level by
  E2B's own accepted tests, and — as of AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E3-MATCHING-RUNTIME-
  POLICY-FAILURE-GATE-REPAIR-01's `ci_03b` (§4a) — at the coordinator integration level too, using
  the existing `AppState::inject_running_loop_for_test` seam (already relied on by every other
  AppState-owned-runtime test in this crate) rather than a full mock-Alpaca-backed live execution
  loop. No new production test surface was added to reach this proof.
- A literal complete-DB-outage and partial-evidence-read-failure proof at the coordinator level, and
  a literal live-concurrent-writer `Conflict` proof, were not built (§13) — both are already
  exhaustively proven at the E2B store level and re-run clean by this patch's own regression pass.
- `AutonomousDailyCoordinatorTickOutcome::OutcomeFinalizationDatabaseUnavailable`/
  `OutcomeFinalizationConflict` leave `AutonomousSessionTruth` unchanged rather than introducing a
  new variant (§9) — an operator reading only that compatibility surface during a transient DB
  outage sees the prior tick's last-known truth, not an explicit "finalization pending" label; the
  coordinator's own typed outcome (visible in structured logs) is the precise record.
- This patch does not implement §11's read-only API contract or any GUI surface — that is E4/Phase F,
  not started here.

## 16. E4 boundary

E3 implements no API route, no GUI surface, no pagination, no read-model projection beyond the
existing `AutonomousSessionTruth` compatibility surface (§9). E4 (not started, not authorized here)
implements the frozen §11 read-only `GET /api/v1/autonomous/daily-operation[s]` routes and the
additive summary-block extensions to the existing readiness/paper-status/preflight routes, strictly
read-only, consuming only the already-existing `fetch_autonomous_daily_operation_by_id`/`_for_slot`/
`list_recent_autonomous_daily_operations` functions — no new evidence computation in the route
handler itself. Phase E remains open; Bundle 3 remains open. The 10–20-session unattended soak does
not begin until Phase E/Bundle 3 close in full, per the standing Phase F/G boundary (`01a` spec
§17/§19, reaffirmed by every prior phase document in this bundle).
