# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1 — Durable Outcome Authority and Evidence Contract

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-DURABLE-OUTCOME-AUTHORITY-AND-EVIDENCE-CONTRACT`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Phase: Phase E1 — durable daily outcome authority and evidence contract audit.
Scope: **read-only architecture audit plus documentation/guard patch.** No production code, test
code, or migration is added or modified by this patch. This document is the binding contract for
Phases E2–E5; it does not implement any part of them.

Starting HEAD: `544ec628708d0b8a5381aaaaef6c220af2f98253` ("fix: bind autonomous claims to
evaluation lineage").

**Correction pass 1** (`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-OUTCOME-CONTRACT-RECONCILIATION-01`,
applied on top of the original E1 audit commit, starting HEAD `33639b735810e0c84707483835a0f0bf8c09c8b0`
("docs: define durable autonomous daily outcome contract")): corrects eight source-proven defects
in the original E1 draft below — a false "no writer" claim for `evidence_degraded` (§7), an
unauthorized legal-transition gap for post-stop unresolved evidence (§3, §7), competing reason-code
authority between `outcome` and `state_reason_code` (§2), an overclaim about `sys_risk_denial_events`
correlation (§4–§6, §10), an under-specified bar-coverage requirement for `completed_no_trade`
(§6, §10, §13), a `no_trade_no_bar_expected` example that names an illegal state transition (§6),
and reconciles the now-stale D4 acceptance-prohibition guard
(`validate_autonomous_daily_paper_operations_01d4_evaluation_lineage_and_autonomous_preopen_closure_01.ps1`).
This correction does not redo the current-source audit in §1 except where a repair required a fresh
source read (recorded inline); it is not itself an acceptance record, and does not close E1, Phase
E, or Bundle 3.

**Correction pass 2** (`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-COVERAGE-ANCHOR-AND-RUN-LINEAGE-
RECONCILIATION-02`, applied on top of correction pass 1, starting HEAD
`0f87f521760b3a148c30263cc4decd24d2a48f7d` ("docs: reconcile autonomous outcome evidence contract")):
corrects ten further source-proven defects in §6 item 2's expected-bar-coverage derivation and in
the run-lineage/precedence/database-failure contract, found by a fresh, targeted re-read of
`daily_data_readiness.rs`, `autonomous_completed_bar_driver.rs`, `state/autonomous_daily_operation.rs`,
and the `mqk-db` autonomous-daily-operation read surface (recorded inline in the affected sections):
(1) the expected-bar lower bound must not be `operation.started_at_utc` — the accepted Phase D
production scenario proves `PrepareDataOnly` durably observes a bar that closes *before*
`started_at_utc`, which `RunningDispatch` then dispatches, so a `started_at_utc`-anchored lower
bound would silently exclude a bar production itself proves is expected (§6 item 2); (2) the
expected-bar upper bound must not be the inclusive `bar_end_ts <= effective_operation_close_utc` —
`tick_autonomous_completed_bar_driver` (`autonomous_completed_bar_driver.rs:910-914`) refuses all
processing once `now_utc >= operation.effective_operation_close_utc`, so a bar only becomes
processable if its expected-timestamp instant (`bar_end_ts + effective_grace_seconds`) falls
*strictly before* that boundary (§6 item 2); (3) the durable `daily_data_readiness_evaluated`
evidence payload (`daily_data_readiness.rs:1444-1483`), confirmed by direct source read, persists
only `applicability`/`start_allowed`/`top_level_blocker`/a bounded per-assignment
`readiness_state`/`blockers` list — it does **not** persist `expected_latest_bar_ts`,
`effective_grace_seconds`, the exact first/final dispatchable bar, or a coverage-policy identity,
so the exact expected-bar set cannot be reconstructed after restart from durable evidence alone
today (new §6a); (4) `sys_autonomous_daily_operation_events`
(`0048_autonomous_daily_operations.sql:171-200`) durably records every `(from_state, to_state,
run_id, transition_seq)` transition, but no existing `mqk-db` read helper aggregates the distinct
`run_id` values bound to one operation across recovery — `list_autonomous_daily_operation_events`
returns the raw bounded transition list only, confirmed by direct source read
(`mqk-db/src/autonomous_daily_operation.rs:1280-1298`) — so full run lineage is not reconstructible
today without new, narrowly-scoped composition (new §6b); (5) late-start/missed-bar coverage is
now defined fail-closed against the intended window, not the actual runtime's first tick (§6 item
2); (6) coverage across a recovery gap must remain continuous from the durable first coverage
anchor through the durable final coverage anchor, never silently narrowed to only the
post-recovery run's own window (new §6b); (7)/(8) the global evidence-integrity precedence order in
§8 is corrected — a confirmed fill no longer overrides an unresolved claim or incomplete lineage/
coverage unconditionally; both `completed_no_trade` and `completed_with_activity` require the full
evidence-integrity chain (DB/identity/lineage/coverage-anchor/coverage/zero-unresolved-claims) to
resolve before either terminal classification is reachable, resolving the contradiction between
§4's already-correct "evidence-complete proof" language and §8's prior "nothing below can override
this" language; (9) the database-failure contract is corrected to distinguish a classifier-level
`unknown_database_unavailable` result from a guaranteed durable blocker-write, and to forbid
claiming a blocker was persisted without an authoritative re-read (§9); (10) the single-E2
decomposition in §13 is replaced with the source-supported E2A/E2B split, since findings (3) and
(4) above are exactly the durable-evidence-foundation gap the original single-E2 decision (§13,
Correction 5 of pass 1) claimed did not exist. This correction does not redo the pass-1 audit
except where a repair above required a fresh source read (recorded inline); it is not itself an
acceptance record, and does not close E1, Phase E, or Bundle 3. No Rust file, test file, or
migration is added or modified by this correction.

## 0. Accepted foundation (recorded, not re-litigated)

```text
D1: ACCEPTED — COMPLETE
D2: ACCEPTED — COMPLETE
D3: ACCEPTED — COMPLETE
D4: ACCEPTED — COMPLETE
PHASE D: ACCEPTED — COMPLETE
BUNDLE 3: OPEN
```

Accepted Phase D production truth (per the bundle prompt, preserved verbatim as scope boundary):
durable daily-operation lifecycle authority; canonical start/recovery/stop ownership; nontrading-day
reconciliation; supervised completed-bar production task; legacy ticker not production-spawned;
exact completed-bar dispatch claims; single authoritative exact-input claim consumer; durable
strategy-evaluation lineage; real `PrepareDataOnly` preopen proof; running exactly-once dispatch
proof; bounded task restart and permanent-failure handling; awaited completed-bar-task shutdown.

This document does not reopen or redesign any of the above without a concrete source-proven
defect. None was found during this audit.

---

## 1. Current outcome surface inventory (E1.1)

Every field below was confirmed against source (`core-rs/crates/mqk-db/src/autonomous_daily_operation.rs`,
`core-rs/crates/mqk-db/migrations/0048–0052`, `core-rs/crates/mqk-db/src/strategy.rs`,
`core-rs/crates/mqk-db/src/orders.rs`, `core-rs/crates/mqk-db/src/inbox.rs`,
`core-rs/crates/mqk-db/src/fill_quality.rs`, `core-rs/crates/mqk-db/src/reconcile_state.rs`,
`core-rs/crates/mqk-daemon/src/state/autonomous_daily_coordinator.rs`,
`core-rs/crates/mqk-daemon/src/state/autonomous_completed_bar_driver.rs`,
`core-rs/crates/mqk-daemon/src/state/autonomous_completed_bar_task.rs`). No fact below is inferred
from a field name alone.

### 1.1 `sys_autonomous_daily_operations` (migration `0048`, boundary columns `0049`, stop/blocker
columns `0051`/`0052`) — the mutable current-state row, one per `(market_date, deployment_mode,
adapter_id)`

| Field | Write authority | Read authority | Durable | Identity key | Restart behavior | Proves activity? | Proves no-trade? | Known ambiguity |
|---|---|---|---|---|---|---|---|---|
| `state` | CAS-only via `transition_autonomous_daily_operation` / `transition_autonomous_daily_operation_to_running` / `refresh_autonomous_daily_operation_blocker` (`autonomous_daily_operation.rs:868,1063,1559`) — every write is `UPDATE ... WHERE operation_id=$ AND state=$expected AND state_version=$expected`, transactional with one matching event row | `fetch_autonomous_daily_operation_by_id`/`_for_slot`, `fetch_relevant_open_autonomous_daily_operation` | yes | `operation_id` (UUIDv5) | fully restart-safe; a fresh process re-reads this row, never trusts in-memory state | no (by itself) | no (by itself) | none — 16-value closed CHECK enum, matched 1:1 by Rust constants |
| `state_reason_code` / `state_blocker_signature` | same CAS path | same | yes | — | restart-safe | no | no | dedup authority is the *pair* `(state_reason_code, state_blocker_signature)`, never the reason code alone (migration `0052` comment) |
| `state_version` | incremented by exactly 1 per successful CAS transition | same | yes | monotonic per `operation_id` | restart-safe; is the optimistic-concurrency token | no | no | none |
| `run_id` | set once at canonical start / recovery start; nullable, no FK (matches `0032`/`0043` precedent) | same | yes | — | restart-safe | **necessary but not sufficient** — a bound `run_id` proves a runtime attempted to operate, not that it traded | no | linking `run_id` to `oms_outbox`/`oms_inbox` rows is required to prove real activity |
| `started_at_utc` / `stopped_at_utc` | `record_running_started` / `record_autonomous_runtime_stopped` — simple `coalesce(...)` UPDATEs, **idempotent, never rewind an existing value** (`autonomous_daily_operation.rs:2484-2513`) | same | yes | — | restart-safe (coalesce means a retried write after a crash is a no-op) | no | no | `stopped_at_utc` can be set from a `manual_intervention_required`/other non-`stopping` state too (§3.2) — it is not itself proof the operation ever ran |
| `bars_observed` / `last_completed_bar_ts` | `record_completed_bar_observed`, monotonic guarded UPDATE, refuses to rewind (`autonomous_daily_operation.rs:1849-1906`) | same | yes | `(operation_id)`, timestamp is bar `end_ts` | restart-safe | no (bar observation ≠ trading) | no | proves data pipeline liveness, not strategy or order activity |
| `bars_dispatched` / `last_dispatched_bar_ts` | `complete_autonomous_daily_bar_dispatch`, incremented only after a confirmed durable evaluation row (`autonomous_daily_operation.rs:2096-2136`) | same | yes | — | restart-safe | no (dispatch = evaluation attempted, not order placed) | no | this is evidence a **strategy evaluation** occurred, one level below order/fill evidence |
| `data_refresh_state`, `last_provider_poll_utc`, `provider_poll_attempt/success/failure_count` | simple (non-CAS) UPDATEs from the completed-bar driver | same | yes | — | restart-safe (durable counters, not process memory) | no | no | proves provider-poll liveness only |
| `finalized_at_utc` | **no production writer exists** — bound to `NULL` at row creation only | same | yes (column exists) | — | n/a | n/a | n/a | reserved by Phase B, unused until E2 |
| `outcome` | **no production writer exists** — bound to `NULL` at row creation only; **no CHECK constraint** enumerates legal values (unlike `state`) | same | yes (column exists) | — | n/a | n/a | n/a | this is the field E2 must become the sole writer of (§2, §9) |
| `no_trade_reason` | **no production writer exists** — bound to `NULL` at row creation only | same | yes (column exists) | — | n/a | n/a | n/a | see §2's disposition — this column is retired by this contract, never populated by E2+ |

### 1.2 `sys_autonomous_daily_operation_events` (migration `0048`) — append-only transition log

Write authority: exactly one row per `state_version` bump, inserted in the same transaction as the
current-state UPDATE (`autonomous_daily_operation.rs:852-936`). Read authority:
`list_autonomous_daily_operation_events`, `fetch_autonomous_daily_operation_event_at_sequence`
(exact-sequence lookup bypasses the 100-row list cap). Durable, keyed `(operation_id,
transition_seq)`. Restart-safe by construction (it is the transition history itself). Proves *how*
a state was reached, not activity/no-trade directly — it is the audit trail a Phase E1
finalization decision should be able to explain itself against, not itself an evidence source for
the classification.

### 1.3 `sys_autonomous_daily_bar_dispatches` (migration `0050`) — durable per-bar dispatch claim

Write authority: `claim_/complete_/fail_autonomous_daily_bar_dispatch`, keyed exactly on
`(operation_id, local_symbol, timeframe, bar_end_ts)`, race-safe (`INSERT ... ON CONFLICT DO
NOTHING`). Status ∈ `{claimed, completed, uncertain, failed}`. `evaluation_id` may be set only when
`status='completed'` (CHECK, `0050:70-71`) and is durably confirmed against
`strategy_signal_evaluations` before being written (D4 repair). Durable, restart-safe (a fresh
connection sees the claim; `unresolved_claim_visible_after_new_db_connection` proves this). Proves
a strategy evaluation was durably confirmed for that exact bar — **does not** prove an order was
submitted. Ambiguity: a non-`completed` status (`claimed`/`uncertain`/`failed`) means the prior
attempt's outcome is not provably resolved — this must never be silently treated as "no trade
happened" (§6, §7).

### 1.4 `strategy_signal_evaluations` (migration `0043`) — per dispatch-attempt evaluation evidence

Columns of record: `decision_stage` ∈ `{pre_dispatch_gate, strategy_evaluated}`,
`signal_generated: bool`, `signal_qty: Option<i64>` (signed sum of strategy target quantities —
this is the "nonzero target" evidence), `signal_side`, `bars_loaded`, `bar_context_source`,
`reason_code`/`reason`. Dedup key is `evaluation_id = uuidv5(run_id|strategy_id|symbol|timeframe|
now_tick)` — **an arbitrary in-process tick counter, not a bar timestamp** (0043 comment,
reconfirmed by 0050's own comment). Durable, restart-safe, `ON CONFLICT DO NOTHING`. Explicitly
documented as never implying an `oms_outbox`/order/fill row exists and never read by any gate,
decision, or dispatch path (`strategy.rs:397-398`) — **this table is evidence of intent-to-evaluate,
not of order submission.** `decision_stage='pre_dispatch_gate'` means a fail-closed market-data gate
refused dispatch before `on_bar` ever ran — this is a distinct, weaker signal than
`'strategy_evaluated'` and must not be conflated with it.

### 1.5 `autonomous_no_trade_diagnostics` (migration `0044`)

Per-minute-bucket diagnostic snapshot, deduplicated on `(reason_code, stage, minute_bucket)`,
written as a best-effort side effect of every `GET /api/v1/autonomous/readiness` call
(`AUTON-NO-TRADE-OFFHOURS-01B`, `state.rs:2590-2631`, `routes/system.rs:1095-1132`) — a genuine DB
write triggered by a GET request, non-fatal on failure, never surfaced to the caller. `paper_order_
attempted`/`live_order_attempted` are **always `false`** in every row this write path produces.
Durable, restart-safe, but **it is minute-resolution diagnostic telemetry, not an end-of-day outcome
classification** — there is no daily rollup, and off-hours rows include `run_id = NULL`. This
patch does not change or extend this deviation (§13's carried-forward rule).

### 1.6 `oms_outbox` (order intents/submissions)

Status machine (final CHECK, `orders.rs`/`0020`): `PENDING, CLAIMED, DISPATCHING, SENT, ACKED,
FAILED, AMBIGUOUS`. "An order was submitted" = a row with `status IN ('SENT','ACKED')` or
`dispatching_at_utc IS NOT NULL` (gateway `.submit()` was at least attempted). "Broker
acknowledged" = `status='ACKED'`, set only via `outbox_mark_acked`. `AMBIGUOUS` is a dedicated
status meaning "broker confirmed: outcome is unknown" (0020) — distinct from a crash-residue
`DISPATCHING` row; both are durable, restart-safe, and **must never be read as "no order was
submitted."** Exit from `AMBIGUOUS` requires an explicit operator/reconcile action
(`outbox_reset_ambiguous_to_pending`), never automatic reclassification.

### 1.7 `oms_inbox` (broker acks/fills/rejects)

`event_kind` ∈ `{ack, fill, partial_fill, cancel_ack, cancel_reject, replace_ack, replace_reject,
reject}`. "A fill occurred" = `event_kind IN ('fill','partial_fill')`;
`uq_inbox_run_order_single_fill` (0040) enforces at most one final fill row per
`(run_id, internal_order_id)`. `applied_at_utc IS NULL` marks a received-but-not-yet-portfolio-
applied row (crash-recovery marker) — durable, restart-safe. A genuine broker-level **rejection**
is durably recorded as `event_kind='reject'`, structurally distinct from a strategy "no signal"
row and from a pre-submission risk denial (§1.8) — real order activity occurred even though no
position resulted.

### 1.8 `sys_risk_denial_events` (migration `0026`)

Durably records pre-submission risk-gate denials — a decision that never reached `oms_outbox` at
all. Deterministic id `"{denied_at_utc_micros}:{rule_code}"`. This is structurally distinct from
both a broker rejection and a strategy no-signal evaluation: it proves the decision pipeline ran
and *chose* not to submit, for a documented rule reason.

### 1.9 `fill_quality_telemetry` (migration `0028`)

Derived, richer per-fill record (slippage, timing, `fill_kind ∈ {partial_fill, final_fill}`),
`provenance_ref` always `'oms_inbox:{broker_message_id}'` — it is not an independent evidence
source, it is a derived enrichment of `oms_inbox` fill rows.

### 1.10 Reconciliation truth

Two distinct durable stores: `sys_reconcile_checkpoint` (per-run `CLEAN`/`DIRTY` history, the sole
gate for arm-preflight) and `sys_reconcile_status_state` (singleton operator-visible posture,
`status ∈ {unknown, ok, dirty, stale}`). Neither is itself trade-activity evidence; both are
consulted only as a **precondition/guard**, never as a source of "activity happened" truth (§6).

### 1.11 Coordinator-side gap already documented in source

`autonomous_daily_coordinator.rs:27-29` states in its own module doc: *"never transitions to
`completed` / `completed_no_trade` / `completed_with_activity` (Phase E owns outcome
finalization)."* `handle_stopping` (`autonomous_daily_coordinator.rs:2776-2810`) returns
`AwaitingOutcomeFinalization` once `operation.stopped_at_utc.is_some()` — this is the coordinator's
existing, already-built handoff signal to whatever Phase E adds. Repo-wide grep confirms **zero**
production call sites transition an operation into any `completed*` state (the one match anywhere
in the repo is test code, `scenario_autonomous_daily_operation_store_01.rs:1451`, which exists only
to prove the DB layer *can* support the transition). The schema, the CAS transition function, and
the legal-transition graph edges from `stopping`/`stop_retrying` into all three `completed*` states
already exist (`is_legal_operation_transition`, `autonomous_daily_operation.rs:107-205`). **The
entire gap is: no caller decides which of the three terminal states applies, and no caller
populates `outcome`/`finalized_at_utc`.** This is precisely, and only, what Phase E2+ must add.

---

## 2. Authority decisions

- **Outcome authority (corrected — Correction 3)**: `sys_autonomous_daily_operations` is the sole
  durable outcome authority, split by exactly one authority per reason type — never two competing
  slots for the same fact:
  - **Terminal classification** (`completed_no_trade` / `completed_with_activity`): `outcome`
    (bounded ≤128 chars, currently unconstrained) is the single closed-set terminal reason code
    (§9), written atomically with `finalized_at_utc` in the same CAS transition that reaches the
    terminal state. The same code may also appear in that transition event's `reason_code` (§1.2)
    for audit lineage, but `outcome` is the read-model authority once finalized.
  - **Nonterminal insufficient-evidence classification** (the operation remains in, or transitions
    to, `evidence_degraded`): `state_reason_code` (already the existing CAS-transition reason-code
    field, §1.1) carries the closed-set `unknown_*` blocker code, paired with
    `state_blocker_signature` for dedup exactly as every other blocker in this table already works.
    `outcome` and `finalized_at_utc` both remain `NULL` while nonterminal — E2+ must never write an
    `unknown_*` value into `outcome`.
  - `no_trade_reason` remains retired per the rule below — unused by either authority.
- **`no_trade_reason` is retired, not reused.** It is a second, competing reason-code slot left over
  from the original Phase A schema draft, with no CHECK constraint and no established semantics
  distinguishing it from `outcome`. Per the D4 precedent of collapsing to "one shared identity
  helper... never a second, independently-derived algorithm," this contract establishes **one**
  reason-code authority (`outcome`) and forbids E2+ from writing `no_trade_reason`. It remains
  physically present (no migration needed to leave it null) and may be formally dropped by a future
  migration outside this bundle's scope; nothing in E2–E5 may treat it as authoritative or read it
  for any decision.
- **`finalized_at_utc`** is set exactly once, in the same transaction as the terminal CAS
  transition, to the wall-clock instant the finalization write commits (caller-supplied, per
  `db_rules.md` — never `DEFAULT now()`).
- **Generic `completed` disposition**: **prohibited as an output of the automatic E2 classifier.**
  The classifier's only two possible terminal outputs are `completed_no_trade` and
  `completed_with_activity`. Generic `completed` remains a legal DB state (the CHECK constraint and
  transition graph are unchanged — no migration needed) but is reserved for a future explicit
  manual-reconciliation/administrative-override path that is **out of scope for E2–E5**. This
  satisfies the required principle that `completed_no_trade` must never mean merely "no fill row
  was found" and that `completed_with_activity` must be evidence-backed: an ambiguous, unspecific
  `completed` is never something the automatic path is allowed to reach for.

---

## 3. Finalization eligibility (E1.2)

### 3.1 `postclose_finalize_utc` — exact meaning (source-resolved)

Confirmed by direct read of `handle_stopping` (`autonomous_daily_coordinator.rs:2776-2810`) and its
derivation (`state/autonomous_daily_operation.rs:366`, `AutonomousDailyPlanTiming::
production_default()`, `= effective_operation_close_utc + 15 minutes`): **it is a stop-retry
escalation deadline, not an earliest-allowed-finalization instant.** While `state ∈ {stopping,
stop_retrying}` and `now_utc < postclose_finalize_utc`, the coordinator keeps retrying the canonical
stop call. Once `now_utc >= postclose_finalize_utc` and the operation is still not durably stopped,
the coordinator gives up and escalates to `manual_intervention_required` with reason
`unresolved_stop_failure_at_postclose_finalize` — a terminal-for-automation, nonterminal-for-the-
state-machine outcome. It is **not** compared against `now_utc` anywhere else in the codebase, and
it is not read by anything finalization-shaped today because no finalization code exists yet.

**Binding rule for E2+**: `postclose_finalize_utc` is **not** used as a finalization-eligibility
gate. It already did its job during the stopping phase (bounding how long stop retries run before
escalating). Finalization eligibility is instead gated directly on `stopped_at_utc` (§3.2), which by
construction can only be set once the stop attempt has concluded (successfully or via the
`postclose_finalize_utc` escalation to `manual_intervention_required`, in which case finalization
is *not* eligible until that manual block clears — see §3.2 condition 2).

### 3.2 Binding eligibility rule

An operation is eligible for finalization if and only if **all** of the following hold:

1. **The durable operation row exists** (resolvable by `operation_id` or by the current day's
   `(market_date, deployment_mode, adapter_id)` slot).
2. **`state` is nonterminal** — not already `completed | completed_no_trade | completed_with_
   activity` (idempotent-replay handling is separate, §8) — **and not `manual_intervention_
   required`, `controller_degraded`, or `calendar_unavailable`.** These three states represent an
   unresolved operator-facing blocker; finalizing "through" them would let an automatic classifier
   silently paper over a condition CLAUDE.md's fail-closed principle requires a human to clear.
   E2's classifier only evaluates operations reachable from `stopping`/`stop_retrying` — the only
   two states the existing legal-transition graph already permits transitioning out of into a
   `completed*` state.
3. **`stopped_at_utc IS NOT NULL`** — the durable "runtime concluded" signal. Every current
   production call site that sets it (`autonomous_daily_coordinator.rs:417, 2677, 2764, 2877, 2901`)
   only fires after either the effective session has closed with nothing to stop, or a real
   `stop_execution_runtime()` call has returned `Ok`. This subsumes "effective session has closed"
   and "no unresolved stop retry remains": `record_autonomous_runtime_stopped` also clears
   `next_retry_utc` atomically (proven by `record_autonomous_runtime_stopped_clears_retry_state_
   atomically_and_is_idempotent`), so a nonterminal, un-stopped operation can never satisfy this
   condition while a retry is still pending.
4. **No locally-owned execution-loop runtime remains bound to this operation's `run_id`** —
   `AppState::locally_owned_run_id()` (`state.rs:3260-3269`) must not return a handle whose `run_id`
   matches `operation.run_id`, or `operation.run_id` is `None` (never started). This is
   process-local truth, valid only within the process attempting finalization; a freshly-restarted
   process trivially satisfies it (no `execution_loop` handle exists yet).
5. **The completed-bar task has quiesced with respect to this operation** — either it never ran
   against this operation (state never reached `preparing_data`/`running`), or
   `cancel_and_wait_completed_bar_task_for_shutdown()` has returned (its documented postcondition:
   "no completed-bar tick can begin, no tick remains in progress" —
   `autonomous_completed_bar_task.rs:1124-1129`), or the production adapter's own
   `select_driver_mode_for_state` already returns no mode for the operation's current state (true
   for every state reachable at condition 2 above, per the Phase D closure doc §7: driver mode
   selection returns `None` for `stopping` and every other non-preparing/non-running state). In
   practice this condition is **already implied** by conditions 2–3 together and requires no
   separate runtime check beyond what the coordinator already establishes before reaching
   `AwaitingOutcomeFinalization` — it is stated explicitly here because the mission requires it
   named, not because E2 needs new code to prove it.

**No coordinator-tick engine needs to change to compute this eligibility.** The coordinator's
existing `AwaitingOutcomeFinalization` outcome (`autonomous_daily_coordinator.rs:1604-1608, 2784,
418`) already fires at exactly the moment conditions 1–5 hold. E3's job is to make the coordinator,
upon observing that outcome, call the E2 classifier instead of merely logging it.

### 3.3 Finalization-time evidence gap → `evidence_degraded` (corrected — Correction 1/2)

§7 resolves the E1 draft's open question: `evidence_degraded` already has a confirmed production
writer (`apply_critical_completed_bar_blocker`, `autonomous_daily_coordinator.rs:1367-1395`, taking
the `running -> evidence_degraded` edge for `CompletedBarCriticalClass::Evidence` outcomes), and its
existing meaning — "durable evidence required for safe automatic operation is missing, contradictory,
corrupt, or unresolved" — is semantically compatible with a post-stop finalization evidence gap. The
existing legal-transition graph (`is_legal_operation_transition`,
`core-rs/crates/mqk-db/src/autonomous_daily_operation.rs:107-205`) does **not** currently permit
`stopping -> evidence_degraded` or `stop_retrying -> evidence_degraded` — only `running ->
evidence_degraded` exists today. This contract explicitly authorizes E2 to extend the Rust
`is_legal_operation_transition` graph with exactly those two new edges. **No migration is required**:
`evidence_degraded` is already a legal value in both `sys_autonomous_daily_operations`'s `state`
CHECK constraint and `sys_autonomous_daily_operation_events`'s `from_state`/`to_state` CHECK
constraints (migration `0048`, `autonomous_daily_operation.rs:142-150,187-207`) — the DB schema
places no restriction on which specific from→to pairs are legal; that is enforced entirely at the
Rust application layer, exactly the same seam D2 already extended five times without a migration
(§0 note; `autonomous_daily_operation.rs:93-106`).

**Exact finalization-time behavior**:

```text
state IN (stopping, stop_retrying)
  + stopped_at_utc IS NOT NULL
  + classification evidence incomplete (any §7 nonterminal trigger fires)
  -> evidence_degraded
     state_reason_code    = the exact matching unknown_* code (§7, §10)
     state_blocker_signature = canonical typed signature (same D1 blocker-signature
                                 mechanism apply_critical_completed_bar_blocker already
                                 uses today for the running -> evidence_degraded edge)
     outcome remains NULL
     finalized_at_utc remains NULL
```

**Recovery** (reuses the existing legal edge, adds nothing new): once evidence later becomes
sufficient (a delayed journal write lands, an operator manually reconciles, or a retried read
succeeds where a prior one failed), the classifier takes the already-legal
`evidence_degraded -> stopping` edge, and the finalization CAS is retried on a later tick. **No
new edge from `evidence_degraded` directly into any `completed*` state is added** — recovery must
route back through `stopping` and re-attempt the ordinary terminal CAS from there, unless a future
source review proves a direct edge is strictly safer and necessary (none was found by this audit).
A mid-run `evidence_degraded` row with `stopped_at_utc IS NULL` (the pre-existing `running ->
evidence_degraded` case) must never enter this finalization-recovery path — it is not
finalization-eligible under §3.2 condition 3 regardless, so no additional guard is required beyond
what §3.2 already establishes.

---

## 4. Terminal-state semantics (E1.3)

- **`completed_no_trade`**: reserved for a durable, evidence-complete proof that the strategy ran
  to conclusion for **every** expected completed bar across the operation's durably-anchored coverage
  window (§6's strict complete-coverage requirement and §6a's durable coverage-anchor precondition,
  corrected — Correction pass 2, Repairs 1/2/3/4) and chose not to trade. **Never** derived from
  "zero rows in `oms_inbox` with `event_kind='fill'`" alone — that fact is necessary but nowhere near
  sufficient (§6). Partial or unprovable bar coverage — including a recovery gap or a late-start gap
  (§6 item 2 steps 7–8) — is never silently treated as no-trade — it routes to nonterminal
  `unknown_incomplete_bar_coverage` (§7, §10).
- **`completed_with_activity`**: reserved for a durable, evidence-complete proof that the strategy
  or its downstream pipeline produced real, DB-observable order/fill/decision activity — never
  inferred from `AutonomousPaperReadinessResponse`'s process-local `bar_tick_dispatch_count` /
  `last_bar_signal_qty` fields (§5). "Evidence-complete" is binding, not aspirational (corrected —
  Correction pass 2, Repair 8): confirmed activity evidence alone does not exempt the operation from
  the same §8 steps 1–6 global evidence-integrity chain `completed_no_trade` requires — see §8.
- **Generic `completed`**: per §2, never chosen by the automatic classifier; reserved for a future
  out-of-scope manual path.
- **`unknown_insufficient_evidence`**: not a fourth terminal state (§7) — a `state_reason_code`
  applied while the operation remains (or moves to) the existing nonterminal `evidence_degraded`
  state (already a legal DB value; no migration required). See §7 for the full rationale.

---

## 5. Activity evidence hierarchy (E1.4)

Ordered from strongest to weakest signal, matching the real pipeline stages (never conflating a
completed *bar dispatch* with trading *activity* — a completed `sys_autonomous_daily_bar_dispatches`
row proves a strategy evaluation was confirmed, nothing about whether an order resulted). **All
`run_id` scoping below is against the operation's full run lineage (§6b), never only the operation's
current `run_id` column** — an earlier run's fill/order/decision evidence must never disappear
because a later recovery cycle overwrote the mutable `run_id`.

This hierarchy decides *which* terminal classification applies once §8 steps 1–6 (operation/DB
authority, identity, run lineage, coverage anchor, coverage completeness, zero unresolved claims)
have already resolved clean (corrected — Correction pass 2, Repair 8: pass 1 described tier 1 as
overriding "everything else," which contradicted §4's "evidence-complete proof" requirement for
`completed_with_activity`; see §8 for the corrected global order).

1. **Broker fill** — `oms_inbox` row(s) with `event_kind IN ('fill','partial_fill')` for a `run_id`
   in this operation's full run lineage. Strongest possible evidence *within this hierarchy*; once
   §8 steps 1–6 have resolved clean, its presence decides `completed_with_activity` unconditionally
   over every other tier below — but it does not itself resolve an unrelated evidence-integrity gap
   found at an earlier §8 step (§8 step 6's explicit example: confirmed fill + an unresolved claim
   for a *different* bar → `evidence_degraded`, not yet terminal).
2. **Broker acknowledgment / order reaching the broker** — `oms_outbox.status IN ('SENT','ACKED')`,
   or `dispatching_at_utc IS NOT NULL`, or an `oms_inbox` row with `event_kind IN ('ack',
   'cancel_ack','replace_ack','reject')` for a bound `run_id`. An order genuinely reached the
   broker even if later rejected or never filled — this is real operational activity, not a
   no-trade day.
3. **Accepted decision / outbox enqueue** — an `oms_outbox` row exists for this `run_id`, but its
   `status` never advanced past `PENDING`/`CLAIMED`/`FAILED` and never reached the broker. This is
   weaker "activity" — the pipeline decided to trade and enqueued the intent, but nothing external
   confirms it reached the broker. Classified `completed_with_activity` with reason
   `activity_decision_accepted` (a decision was made and durably recorded), distinct from the
   stronger `activity_order_submitted`/`activity_fill_confirmed` reasons.
4. **Risk/promotion-gate denial evidence (corrected — Correction 4, downgraded)** —
   `sys_risk_denial_events` (migration `0026`) is durable, but its columns are `id`,
   `denied_at_utc`, `rule`, `message`, `symbol`, `requested_qty`, `limit_qty`, `severity` — it
   carries **no** `operation_id`, `run_id`, `evaluation_id`, `strategy_id`, `timeframe`,
   `decision_id`, or `order_id` column (confirmed by direct read of
   `core-rs/crates/mqk-db/migrations/0026_risk_denial_events.sql:15-24`). A row in this table
   therefore **cannot be durably correlated** to a specific operation or strategy evaluation today —
   a nearby `symbol`/`requested_qty`/`denied_at_utc` match is coincidental proximity, not proof, and
   this contract forbids treating it as proof. §5 tier 4 as originally drafted (promoting a
   risk-denial match to no-trade evidence) is **removed**; a promotion denial for this operation's
   symbol is not currently distinguishable, by any durable join, from an unrelated denial for the
   same symbol on the same day.
5. **Strategy evaluation with nonzero target, no accepted decision, and no durably correlated order
   evidence** — a `strategy_signal_evaluations` row exists with `decision_stage='strategy_evaluated'`
   and `signal_generated=true`/`signal_qty != 0`, but no `oms_outbox` row exists. This is **not**
   classified as activity, and **not** classified as no-trade either — it is a genuine evidence gap
   between "the strategy wanted to trade" and "we can prove what happened next." Per Correction 4,
   this is now the **only** disposition for a nonzero uncovered signal (the risk-denial branch above
   is removed, not an alternative path). Routed to nonterminal `evidence_degraded` (§7) under reason
   `unknown_order_evidence_conflict`, never guessed in either direction. A nearby
   `sys_risk_denial_events` row with the same symbol/quantity/timestamp is **not** sufficient
   correlation to resolve this gap (§6, §9's deferred-work note).

Ordered/exclusive rule: evaluate top-down; the first matching tier wins. A day may have both fills
and unrelated no-signal evaluations for other bars — **once §8 steps 1–6 have resolved clean**, the
presence of *any* tier-1 or tier-2 evidence makes the whole operation `completed_with_activity`,
regardless of what any individual bar's evaluation otherwise showed. This tier-ordering rule governs
*which* terminal classification applies; it does not itself grant an exemption from the global
evidence-integrity chain in §8 — see §8 step 6 for the case where activity evidence and an unresolved
claim coexist.

---

## 6. No-trade evidence hierarchy (E1.5)

`completed_no_trade` requires **all** of the following to hold, durably:

1. `stopped_at_utc IS NOT NULL` (finalization-eligible, §3).
2. **Complete expected-bar coverage (corrected — Correction pass 2, Repairs 1/2/5/7, superseding
   pass 1's Correction 5 item 4 below)**. The original draft's "every existing dispatch row is
   `completed`" is necessary but not sufficient — it says nothing about bars that were expected but
   for which **no** dispatch row exists at all. The classifier must instead prove complete coverage
   using canonical session/timeframe truth, reusing only already-existing pure helpers and persisted
   columns (no new algorithm; §6a defines the one net-new durable field this composition still
   needs):
   1. Resolve the canonical single-symbol assignment and effective runtime binding for this
      operation (the same resolution `attempt_canonical_start`/`create_or_recover_autonomous_daily_
      operation` already performs, `autonomous_daily_coordinator.rs:370-372`).
   2. Derive `assignment_identity`/`runtime_binding_identity` from that resolution via the existing
      `derive_assignment_identity`/`derive_runtime_binding_identity` helpers
      (`state/autonomous_daily_operation.rs:506,537`) and require them to match the operation's own
      persisted `assignment_identity`/`runtime_binding_identity` columns **exactly**. A mismatch (or
      an inability to resolve the current assignment/binding at all) means the classifier cannot
      safely know which symbol/timeframe this operation actually ran — fail closed to
      `unknown_assignment_identity_unavailable` (§7), never guess which config applied.
   3. Use the accepted canonical session/calendar authority already bound to this operation
      (`session_open_utc`, `session_close_utc`, `effective_operation_close_utc` — all persisted
      per-operation columns, migrations `0048`/`0049`) together with the existing pure
      `daily_data_readiness::intraday_grid_starts(session_open_utc, session_close_utc,
      interval_secs)` helper — **not** `expected_intraday_end_ts_window` (that helper is
      `required_history_bars`-bounded for readiness-gate purposes, not a full-day coverage list).
   4. **Lower bound (corrected — Repair 1): never `operation.started_at_utc`.** The accepted Phase D
      production scenario (`phase_d_full_day_lifecycle`, §4 point 1 of the `01d` closure doc) proves
      the real production sequence is: `PrepareDataOnly` durably observes a bar via the preopen tail
      window (`pd_preopen_expected_bar_window`) whose `bar_end_ts` closes **before**
      `started_at_utc` is ever set; the canonical runtime then starts; `RunningDispatch` then
      dispatches that **same already-observed bar**, never a bar starting fresh from
      `started_at_utc`. A `started_at_utc`-anchored lower bound would therefore silently exclude a
      bar production itself proves is expected and dispatchable — exactly the kind of
      under-inclusive coverage window the mission forbids ("first dispatch row found," "current
      `last_completed_bar_ts`" are equally forbidden for the same reason: none of them can prove an
      *earlier* expected claim is not missing). The real lower bound is the first bar identity the
      production readiness/completed-bar authority itself would ever expect for this operation:
      `intraday_grid_starts(session_open_utc, session_close_utc, interval_secs)`'s first slot whose
      close instant already satisfies the grid's own expectation rule (item 5 below) at
      `preopen_start_utc` or later — i.e. the first grid slot for which
      `slot_start + interval_secs + effective_grace_seconds <= <the first instant the operation's
      own preopen tick could have run>`, mirroring exactly what
      `expected_intraday_end_ts_window`'s spillover branch and `pd_preopen_expected_bar_window`
      already compute for the preopen instant in production. If no such slot exists (the operation's
      own session grid produces nothing expected before the operation starts), the lower bound is
      simply the grid's first in-session slot — the ordinary case for a normal-hours start.
   5. **Upper bound (corrected — Repair 2): never the inclusive `bar_end_ts <=
      effective_operation_close_utc`.** `tick_autonomous_completed_bar_driver`
      (`autonomous_completed_bar_driver.rs:910-914`, confirmed by direct source read) refuses *all*
      processing — preopen observation and running dispatch alike — once `input.now_utc >=
      operation.effective_operation_close_utc`. A bar only ever becomes genuinely dispatchable in
      production if some valid `now_utc < effective_operation_close_utc` exists at which the bar is
      already expected under the grid's own rule (`slot_start + interval_secs +
      effective_grace_seconds <= now_utc`, `daily_data_readiness.rs:1082`). The latest such `now_utc`
      approaches `effective_operation_close_utc` from below, so the exact final-dispatchable-bar
      condition is:
      ```text
      slot_start + interval_secs + effective_grace_seconds < effective_operation_close_utc
      ```
      strictly less than, never less-than-or-equal — a close bar whose expectation instant lands
      exactly at or after `effective_operation_close_utc` is a bar production itself can never
      process, and the contract must not require its coverage as a precondition for
      `completed_no_trade`.
   6. **Coverage window (corrected — Repair 1/2 combined)**: the expected completed-bar end-
      timestamps are the subset of the session's full grid (`intraday_grid_starts`) whose slots
      satisfy both bound 4 and bound 5 above, **not** `[operation.started_at_utc,
      min(operation.stopped_at_utc, operation.effective_operation_close_utc)]` as pass 1 specified.
      If the operation never legally reaches a state where any grid slot could satisfy bound 4 (e.g.
      it reached `stopping` via a pre-running-state edge — `preflight_blocked -> stopping` and
      similar — before any preopen tick or running tick ever ran), the expected-bar set is empty by
      construction. This is **not** classified `completed_no_trade` under any reason code (pass 1's
      Correction 6 already removed `no_trade_no_bar_expected` for exactly this reason — no durable
      evidence proves "zero bars were ever applicable" as opposed to "the operation was blocked
      before it could find out"); it routes to `unknown_missing_evaluation_evidence` (§7).
   7. **Coverage across recovery (corrected — Repair 7, new)**: a runtime interruption and
      subsequent recovery (§6b) does not reset or re-anchor the coverage window. The intended
      coverage window remains continuous from the durable first coverage anchor (bound 4) through
      the durable final coverage anchor (bound 5) regardless of how many `run_id`s the operation
      bound in between. A bar whose expectation instant fell inside a runtime-interruption gap, with
      no completed claim or evaluation durably recorded for it under *any* run in the operation's
      full lineage (§6b), is **not** silently excluded from the expected set and is **not** silently
      treated as no-trade — it is a genuine missing-coverage fact, routed through item 9 below
      exactly like any other missing expected bar. A different rule (treating a recovery-gap bar as
      out-of-scope) requires explicit source proof that skipping bars during a recovery gap is the
      intended trading-strategy contract; no such proof exists today.
   8. **Late start (corrected — Repair 5, new)**: a runtime that starts after its intended effective
      open is not excused from bars that should have closed during the intended window before the
      delayed start. The intended window is bound 4 above — derived from the operation/session
      contract's own preopen/grid timing, never merely "whatever the actual runtime's first tick
      happened to observe." A late start that leaves an earlier intended bar with no completed claim
      or evaluation is exactly the same missing-coverage fact as item 7's recovery gap, and is
      resolved the same way (item 9).
   9. Require every expected bar identity `(local_symbol, timeframe, bar_end_ts)` — across the full
      run lineage of §6b, not merely the operation's current `run_id` — to have: a completed
      `sys_autonomous_daily_bar_dispatches` row; a non-null `evaluation_id` on that row; and a
      matching durable `strategy_signal_evaluations` row confirmed by the same exact-lookup pattern
      D4 already uses (`mqk_db::fetch_strategy_signal_evaluation`,
      `autonomous_completed_bar_driver.rs`, §12.1 of the `01d` closure doc).
   10. **Zero** missing expected bar identities (across recovery gaps and late-start gaps alike, per
       items 7–8).
   11. **Zero** extra, future, or inconsistent claims for this `operation_id` outside the expected
       set (a claim with a `bar_end_ts` the grid does not produce is itself a contradiction, not
       evidence to ignore).
   12. Aggregate counters must agree with the per-bar rows: `bars_observed`/`bars_dispatched` equal
       the proven-covered count, and `last_completed_bar_ts`/`last_dispatched_bar_ts` equal the final
       expected identity's `bar_end_ts`.
   13. **Durable coverage-anchor precondition (new, §6a)**: items 1–12 above are only trustworthy if
       the exact coverage policy that produced bounds 4/5 for *this operation, as it actually ran*
       can itself be durably proven — not merely recomputed from the current process's mutable
       environment configuration, which may have changed since the operation started. §6a defines
       this gap exactly. **No durable proof of the original coverage policy → no
       `completed_no_trade` finalization**, regardless of how clean items 1–12 otherwise appear.

   Any missing, unprovable, or contradictory expected-bar identity — including a `claimed`/
   `uncertain`/`failed` dispatch row anywhere in the expected set, a recovery-gap bar (item 7), or a
   late-start gap bar (item 8) — produces the nonterminal `unknown_incomplete_bar_coverage` (§7,
   §10), never a silent no-trade classification. This supersedes and retires the E1 draft's narrower
   `unknown_insufficient_bar_evidence` code (which only checked `bars_observed=0`): a day with some,
   but not all, expected bars proven is exactly as unproven as a day with zero, and both must use the
   same one authority — no free-form SQL join is specified here beyond what is stated; the exact
   query shape remains an E2B implementation detail (§13/§14).
3. At least one durable `strategy_signal_evaluations` row exists with `decision_stage=
   'strategy_evaluated'` for this operation's `run_id` (i.e., the strategy genuinely ran at least
   once). Given item 2's complete-coverage proof, this is implied whenever the expected-bar set is
   nonempty and fully covered — restated here because it remains the deciding fact when the expected
   set is legitimately empty (item 2's `started_at_utc IS NULL` case): zero evaluations then means
   zero proof of anything, routed to `unknown_missing_evaluation_evidence`, never assumed no-trade.
4. **Every** `strategy_signal_evaluations` row with `decision_stage='strategy_evaluated'` for this
   `run_id` has `signal_generated=false` or `signal_qty=0` (corrected — Correction 4: the original
   draft's second branch, "or a matching risk-denial row proves denial," is removed — see §5 tier 4
   and §9's deferred-work note. A nonzero signal with no matching `oms_outbox` row is **never**
   `completed_no_trade` under any circumstance; it is always routed to nonterminal
   `unknown_order_evidence_conflict`, regardless of any coincidentally nearby `sys_risk_denial_
   events` row).
5. Zero `oms_outbox` rows exist for this `run_id`.
6. Zero `oms_inbox` rows with `event_kind IN ('fill','partial_fill','ack','reject','cancel_ack',
   'replace_ack')` exist for this `run_id`.
7. `already_at_target` — the "strategy evaluated and concluded no position change was needed" case
   — is only ever a sub-classification of item 4 above (a `strategy_signal_evaluations` row proving
   the evaluation ran and concluded flat), never inferred from process-local target state.

**Reason families** (bounded, source-supported; corrected — Corrections 4/6):
- `no_trade_strategy_evaluated_no_signal` — items 1–7 hold, and every evaluated bar's signal was
  flat/zero.
- ~~`no_trade_all_signals_blocked`~~ — **removed from the initial E2 reason set.** The original
  draft's premise (a matching `sys_risk_denial_events` row proves a specific nonzero signal was
  denied) is not currently provable — §5 tier 4 confirms the table has no durable correlation column
  to any operation, run, evaluation, or strategy. Deferred per §9's binding disposition; not
  authorized for E2/E2's classifier to implement from the current schema under any name.
- `no_trade_no_bar_expected` — **removed from the initial E2 reason set (corrected — Correction 6).**
  The E1 draft's own example named an illegal state transition: `calendar_unavailable` has no legal
  edge to `stopping` in the existing transition graph (`is_legal_operation_transition`,
  `autonomous_daily_operation.rs:195-198`: `calendar_unavailable` only transitions to
  `awaiting_preopen` or `manual_intervention_required`) — an operation that is calendar-unavailable
  for the whole day is therefore never finalization-eligible under §3.2 and never reaches this
  classifier at all; it remains operator-blocked, which is the correct, already-existing disposition
  and requires no new reason code. The only *other* pre-running states with a legal edge to
  `stopping` (`awaiting_preopen`, `preparing_data`, `awaiting_open`, `preflight_blocked`,
  `start_retrying`) each represent a real readiness/config blocker, not a durable proof that "zero
  bars were ever applicable" for calendar reasons — using any of them as `no_trade_no_bar_expected`
  evidence would be exactly the kind of guessed no-trade reason §7's frozen Phase A rule forbids. No
  source-provable path to this reason code was found during this audit. If a future audit locates
  one, it must be re-added by name with its exact evidence proof, not reused implicitly.

Process-local values that **must never** be used as no-trade authority (per the mission's explicit
prohibition, cross-checked against source): `AppState`'s in-memory `bar_tick_dispatch_count`,
`last_bar_signal_qty` (both live only in `AutonomousPaperReadinessResponse`, never persisted
per-day), any completed-bar-task liveness cache, and any strategy diagnostic cache. All of these are
diagnostic/process-local by construction and carry no restart-safe authority.

---

## 6a. Durable coverage-anchor audit and future authority (new — Correction pass 2, Repairs 3/4)

**Exact existing durable evidence, confirmed by direct source read.** The only durable payload
produced before a start attempt is `build_pre_start_evidence_detail`
(`daily_data_readiness.rs:1448-1483`), persisted as the `daily_data_readiness_evaluated`
`sys_autonomous_session_events` row:

```text
schema_version, evaluation_id, evaluated_at_utc, binding_scope, applicability,
start_allowed, top_level_blocker, assignment_count,
assignments[].{assignment_symbol, assignment_timeframe, readiness_state, blockers}
```

This **does not** include `expected_latest_bar_ts`, `effective_grace_seconds`, the exact first or
final dispatchable bar identity, or any coverage-policy identity. The companion
`daily_data_readiness_run_linked` event (`daily_data_readiness.rs:1516-1539`) carries only
`evaluation_id`/`run_id`/`linked_at_utc` — no coverage fields either. The durable
`sys_autonomous_daily_operations` row itself (§1.1) persists `session_open_utc`,
`session_close_utc`, `effective_operation_close_utc`, `preopen_start_utc` — the session-plan
boundaries — but **not** the timeframe interval, the configured/effective grace seconds, or the
computed first/final dispatchable-bar timestamps that §6 item 2 derives from those boundaries. Both
gaps are confirmed, not inferred from a field name.

**Consequence.** After a restart, the exact expected-bar set from §6 item 2 can only be
*recomputed*, never *durably re-read*, because the grace-seconds and interval inputs to bounds 4/5
are sourced from the current process's mutable environment/assignment configuration
(`MultiSymbolRuntimeConfig`, provider-registry `configured_grace_seconds`), not from anything
written to durable storage at the time the operation actually ran. If that configuration changes
between the operation's original run and a later finalization attempt (an operator edits
`MQK_STRATEGY_BAR_INTERVAL_SECS`, a provider's configured grace value, or the watchlist artifact),
the recomputed expected-bar set can silently diverge from the set that was actually in force when
the operation ran — with no durable evidence available to detect the divergence. This is a genuine
evidence-foundation gap, not a hypothetical: §6 item 2's assignment/runtime-binding identity check
(steps 1–2) already fails closed for a *different* silent-drift risk (config identity mismatch); the
coverage-policy identity is the same class of risk, unaddressed by that check, because
`assignment_identity`/`runtime_binding_identity` do not fold in timeframe/grace-seconds at all
(confirmed by `derive_assignment_identity`, `state/autonomous_daily_operation.rs:506-529` — it
covers `config.symbols`/source label only).

**Bundle 1 static-configuration limitation, explicitly not promoted into durable proof.** The
existing Phase A/B design already assumes single-process-lifetime-static configuration for other
purposes (§13 of the `01a` spec, "same-day identity conflict" handling). That assumption is
documented there as a known, accepted limitation for *identity conflict detection*, not as durable
*coverage-policy proof*. This contract does not silently extend that limitation to cover coverage
anchoring — doing so would let a config-drift scenario the identity check cannot see (timeframe/
grace-seconds, not symbol/strategy/binding) produce an unprovable `completed_no_trade` claim.

**Required rule.** No durable proof of the original coverage policy (first dispatchable bar
identity, final dispatchable bar identity, timeframe, effective grace seconds, and the session-plan
identity they were derived against) → no `completed_no_trade` finalization, full stop. A recomputed
value is not durable proof of what the value was at the time the operation actually ran.

**Preferred future authority (decided, not implemented in E1).** Extend the existing
`daily_data_readiness_evaluated` pre-start evidence event — not a new event type, not a new table —
with the additional fields §6 item 2 needs to reconstruct bounds 4/5 exactly after restart:

```text
operation_id
first_dispatchable_bar_end_ts
final_dispatchable_bar_end_ts
local_symbol
timeframe
timeframe_secs
effective_grace_seconds
exchange_session_identity / effective_session_boundary identity (already available via
  session_plan_identity, §13 of the 01a spec — reused, not reinvented)
assignment_identity
runtime_binding_identity
coverage_schema_version
coverage-bound timestamp (evaluated_at_utc already serves this role)
```

This is preferred over a new table because: (a) the write already happens at exactly the right
moment (pre-start, before any bar is dispatched) and is already keyed by `evaluation_id` with
idempotent `ON CONFLICT (id) DO NOTHING` semantics (`persist_pre_start_readiness_evidence`,
`daily_data_readiness.rs:1489-1510`); (b) it is already linked to the operation via the existing
`daily_data_readiness_run_linked` companion event; (c) it avoids a second, competing durable
coverage authority alongside the existing readiness-evidence event. A new dedicated table/migration
is authorized only if a later implementation patch finds the existing event's JSON `detail` payload
cannot safely carry this data (e.g. a query-performance or schema-versioning need this audit did not
find) — E1 does not authorize that new table now; it names the extension as the default seam and
defers the final decision, with proof, to E2A (§13).

## 6b. Full run lineage (new — Correction pass 2, Repair 6)

**The operation's mutable `run_id` is not the whole-day run authority.** A recovered operation can
bind `run A -> terminal interruption -> recovery_retrying -> run B`, and potentially further
replacement runs across additional interruptions (§4 point 4 of the `01d` closure doc proves exactly
one such cycle end-to-end; nothing in the schema or the coordinator bounds the number of cycles in a
single day). Reading only `operation.run_id` (the current mutable value) at finalization time would
silently drop every earlier run's activity and evaluation evidence from the classifier's view.

**Authority.** `sys_autonomous_daily_operation_events` (§1.2, migration `0048`) is append-only and
already records `run_id` on every transition row that carries one
(`0048_autonomous_daily_operations.sql:171-200`). The full run lineage for one operation is:

```sql
select distinct run_id
from sys_autonomous_daily_operation_events
where operation_id = $1
  and to_state = 'running'
  and run_id is not null
order by transition_seq
```

**Confirmed gap.** No existing `mqk-db` read helper performs this aggregation today.
`list_autonomous_daily_operation_events` (`autonomous_daily_operation.rs:1280-1298`, confirmed by
direct source read) returns the raw, transition-ordered event list bounded by an explicit `limit`
parameter (the API-layer caller clamps this to `[1,100]`, §11) — it does not filter on `to_state`,
does not deduplicate `run_id`, and a caller passing too small a `limit` could silently truncate the
lineage for an operation with an unusually long transition history. A dedicated, narrow read helper
(unbounded by the general-purpose event-list cap, or explicitly bounded high enough to guarantee
full-day coverage) is required — named here, not implemented in E1 (§13, E2A).

**Required aggregation rules:**
- Collect every distinct `run_id` bound via a `to_state = 'running'` transition, in `transition_seq`
  order — this is the full lineage.
- The operation's current `run_id` column (§1.1) must equal the lineage's final entry whenever it is
  non-`NULL`. A mismatch is a contradiction, not a fact to silently prefer one source over the
  other — fail closed to `unknown_run_lineage_unavailable` (§7, new).
- Duplicate `run_id` values appearing out of monotonic `transition_seq` order, or any other internal
  contradiction in the lineage read, fail closed the same way.
- An operation with zero `to_state = 'running'` transitions (it never started) has an empty lineage
  by construction and cannot use run-scoped evidence (`oms_outbox`/`oms_inbox`/
  `strategy_signal_evaluations` are all keyed on `run_id`) — this is the same case §6 item 2 step 6
  already routes to `unknown_missing_evaluation_evidence`, not a new code.
- **All** run-scoped evidence reads in §5/§6 (strategy evaluations, outbox rows, inbox rows, fills,
  acknowledgments) must aggregate across the **complete** run-ID set from the lineage, not merely
  the operation's current `run_id`. An earlier run's fill, order, or evaluation must never disappear
  from the classifier's view merely because a later recovery cycle overwrote the operation's mutable
  `run_id` column. This directly supersedes every "for this `run_id`" phrase in §5/§6/§10 that
  predates this correction — read as "for this operation's full run lineage" throughout.

**Coverage across recovery.** See §6 item 2 steps 7–8: the expected-bar coverage window is anchored
to the durable first/final coverage anchors (§6a), not to any single run's own start/stop instants,
so a recovery cycle's gap is evaluated for missing coverage exactly like any other gap — never
silently excused because it falls "between runs."

**New reason code**: `unknown_run_lineage_unavailable` — the lineage read failed, is missing,
contradictory (duplicate/out-of-order `run_id`, or a mismatch against the operation's current
`run_id`), or unreadable. See §7, §10.

---

## 7. Unknown / insufficient evidence (E1.6)

**Representation decision**: `unknown_insufficient_evidence` is **not** a new terminal DB state. It
is a `state_reason_code` value applied while the operation transitions to (or remains in) the
existing nonterminal `evidence_degraded` state — already a legal value in the `state` CHECK
constraint (migration `0048`) and in `is_legal_operation_transition`'s edge set, so **no migration
is required for E2** to implement this.

**Rationale** (source-grounded, not an arbitrary schema choice):
- Terminal states in this schema have **no legal outbound edge**
  (`is_legal_operation_transition`, `autonomous_daily_operation.rs:199-204`: every `completed*`
  variant as `from_state` returns `false` for all targets). "Insufficient evidence" is a statement
  about our own inability to observe/reconstruct truth, not a truth about the trading day itself —
  unlike `completed_no_trade`/`completed_with_activity`, it should remain open to correction if the
  missing evidence later becomes available (a delayed journal write lands, or an operator manually
  reconciles) or an operator manually resolves it. A terminal state would permanently foreclose that
  without a bespoke administrative-override write path, which is unnecessary complexity this
  contract does not need to introduce.
- Reusing `evidence_degraded` keeps the schema's three-terminal-state design frozen exactly as
  Phase B built it, minimizing blast radius per this repo's minimal-scope discipline.
- **Resolved (corrected — Correction 1; was an open question in the original E1 draft)**: the E1
  draft's claim that "no current production call site was found that transitions any operation into
  `evidence_degraded`" was **false**, contradicted directly by source. A confirmed production writer
  already exists: `apply_critical_completed_bar_blocker`
  (`autonomous_daily_coordinator.rs:1367-1395`), reached from
  `apply_completed_bar_driver_outcome`'s evidence-critical branch
  (`autonomous_daily_coordinator.rs:1217-1230`, `CompletedBarCriticalClass::Evidence`), takes the
  `running -> evidence_degraded` edge for exactly these evidence-critical completed-bar driver
  outcomes: an unresolved dispatch claim (`DispatchClaimUnresolved`), missing evaluation-lineage
  evidence (`DispatchEvaluationEvidenceMissing`), an unconfirmed completion write
  (`DispatchCompletionUnconfirmed`), and any other outcome the driver classifies as
  `CompletedBarCriticalClass::Evidence` rather than `Control`. Its existing, already-live meaning is
  exactly: *"durable evidence required for safe automatic operation is missing, contradictory,
  corrupt, or unresolved."* This is semantically identical to what §3.3/§7 need for a post-stop
  finalization evidence gap — both describe the same underlying fact (durable evidence needed for
  automatic processing is not trustworthy/complete, pending resolution), just observed at two
  different lifecycle instants (mid-run vs. post-stop). **The state graph and retry behavior must
  still be explicitly extended** (§3.3 authorizes the two new `stopping`/`stop_retrying ->
  evidence_degraded` edges; the existing `evidence_degraded -> stopping` recovery edge already
  covers the retry path) — reuse is safe, but is not a no-op; E2 must implement the graph extension,
  not merely assume the existing edge already covers this case. This is no longer left as an open
  question for E2 to resolve.

**Mandatory triggers** (all fail closed to `evidence_degraded` + the matching `state_reason_code`,
never guessed toward no-trade or activity; corrected — Corrections 4/5/6 change which triggers apply
and add two new ones):
- `stopped_at_utc` present but any `sys_autonomous_daily_bar_dispatches` row in the expected-bar set
  (§6 item 2) is still `claimed`/`uncertain`/`failed` → `unknown_unresolved_dispatch_claim`.
- Zero durable `strategy_signal_evaluations` rows exist — whether the operation reached `running`
  and observed nothing, or it legally reached `stopping` without ever reaching `running` at all
  (extended by Correction 6, replacing the removed `no_trade_no_bar_expected`'s empty-expected-set
  case, §6 item 2) → `unknown_missing_evaluation_evidence`.
- A nonzero signal exists with no `oms_outbox` row (corrected — Correction 4: the risk-denial branch
  is removed; this trigger now fires unconditionally on the nonzero-signal/no-outbox fact alone, per
  §5 tier 4/5) → `unknown_order_evidence_conflict`.
- **New (Correction 5)**: any expected bar identity in §6 item 2's derived grid is missing, has no
  confirmed evaluation row, or the aggregate counters disagree with the per-bar rows →
  `unknown_incomplete_bar_coverage`. This supersedes the original draft's narrower
  `bars_observed = 0` check (`unknown_insufficient_bar_evidence`, retired — a partial-coverage day is
  exactly as unproven as a zero-coverage day and must use the same one authority). **Extended
  (Correction pass 2, Repairs 1/2/7/8)**: this trigger now also fires for a recovery-gap bar (§6 item
  2 step 7) and a late-start-gap bar (§6 item 2 step 8) with no completed claim/evaluation under any
  run in the full lineage (§6b) — coverage evaluation is never narrowed to only the currently-active
  run's own window.
- **New (Correction 5)**: the current process cannot resolve the operation's own assignment/runtime
  binding, or the freshly-derived `assignment_identity`/`runtime_binding_identity` does not exactly
  match the operation's persisted values (§6 item 2, step 2) → `unknown_assignment_identity_
  unavailable` — the classifier must never assume "probably still the same config" and proceed.
- **New (Correction pass 2, Repair 6)**: the operation's full run lineage (§6b) cannot be read, is
  empty for an operation with a non-`NULL` current `run_id`, contains a duplicate or out-of-order
  `run_id`, or its final entry does not match the operation's own current `run_id` column →
  `unknown_run_lineage_unavailable`. Every run-scoped evidence read in §5/§6 depends on first
  establishing this lineage, so this check runs immediately after assignment/runtime-binding identity
  (§8) and before any bar-coverage or activity evidence is gathered.
- **New (Correction pass 2, Repair 3/4)**: the durable coverage-anchor evidence §6a requires (the
  first/final dispatchable-bar identity, timeframe, and effective grace-seconds actually in force
  when this operation ran) cannot be durably confirmed — because no production writer persists it
  today (§6a) — → the classifier must not proceed to `completed_no_trade` on a merely *recomputed*
  coverage window; until §6a's preferred future authority exists and is populated for this operation,
  every `completed_no_trade` attempt fails closed to `unknown_incomplete_bar_coverage` on this basis
  alone, even when the recomputed §6 item 2 coverage otherwise appears complete. This is the direct,
  binding consequence of §6a's "no durable proof of the original coverage policy → no
  `completed_no_trade` finalization" rule, not a new reason code.
- Any DB read required to gather evidence fails → `unknown_database_unavailable` — this check runs
  **first**, before any other evidence read, and short-circuits the entire classification (§8).
  **Corrected (Correction pass 2, Repair 9, §9)**: this is a classifier/result-level truth only — it
  never by itself guarantees a durable blocker write occurred; see §9 for the exact database-failure
  contract, including the distinction between a complete-outage retry and a partial-read-failure
  best-effort write.
- `stopped_at_utc` is present but the eligibility conditions of §3.2 cannot otherwise be fully
  confirmed (e.g. a genuinely conflicting/contradictory pair of durable facts) →
  `unknown_runtime_stop_unproven`, used narrowly for this residual case only — it does not mean
  "stop was never attempted" (that case is simply not yet eligible, §3, and produces no
  classification attempt at all, successful or otherwise).

None of these may be silently reclassified as `no_trade_*` to "fill the gap" — this is the explicit,
already-frozen rule in the existing Phase A contract (§16 of the `01a` spec): *"When the durable
evidence needed to classify an outcome is itself incomplete or unavailable, the outcome is
`unknown_insufficient_evidence` — never a fabricated no-trade reason invented to fill the gap."* This
now applies symmetrically to `completed_with_activity` as well (§8, Correction pass 2, Repair 8):
confirmed activity evidence does not exempt an operation from the same evidence-integrity chain — it
only decides which terminal classification applies *once* that chain resolves clean.

---

## 8. Evidence precedence and conflict resolution (E1.7)

**Corrected in full (Correction pass 2, Repair 8).** Pass 1's rule 4 below ("any confirmed fill …
wins over everything else … `completed_with_activity` is decided and no further evidence-gap check
downgrades it") directly contradicted §4's already-correct definition of `completed_with_activity`
as requiring "a durable, **evidence-complete** proof." A confirmed fill is real evidence that
activity occurred, but it is not by itself proof that the classifier has gathered a trustworthy,
complete evidentiary picture of the *entire* operation — an unresolved dispatch claim elsewhere, an
unreadable run lineage, or an unprovable coverage anchor means the classifier cannot yet certify
*anything* durably, including a clean `completed_with_activity`. This section replaces pass 1's
9-step order with the mission's binding global order: global evidence-integrity checks first
(operation/DB authority, identity, run lineage, coverage anchor, coverage completeness, zero
unresolved claims), and only once every one of those resolves clean does the classifier choose
between the §5 activity hierarchy and the §6 no-trade hierarchy.

Deterministic, fail-closed order, evaluated top-to-bottom; the first matching rule decides:

1. **Operation and DB authority available.** The durable operation row must be readable, and every
   DB read required for the remaining evidence-gathering pass must succeed. Any DB read failure at
   any point in the pass → `unknown_database_unavailable`. Nothing else is evaluated once this
   fires — a partial evidence read must never be treated as a complete one. Runs strictly first,
   before every check below (§9 defines the exact database-failure write contract).
2. **Assignment/runtime-binding identity valid** (§6 item 2 steps 1–2). The current process must be
   able to resolve the operation's own assignment/runtime binding, and the freshly-derived
   `assignment_identity`/`runtime_binding_identity` must exactly match the operation's persisted
   values → otherwise `unknown_assignment_identity_unavailable`. Runs before any bar or lineage
   evidence is gathered, because both depend on knowing the correct symbol/timeframe/binding.
3. **Full run lineage valid** (§6b). The operation's complete `run_id` lineage must be readable,
   non-contradictory, and (when the operation's current `run_id` is non-`NULL`) consistent with the
   lineage's final entry → otherwise `unknown_run_lineage_unavailable`. Runs before any run-scoped
   evidence (strategy evaluations, outbox, inbox, dispatch claims) is gathered, because every one of
   those reads must be scoped to the full lineage's run-ID set, not a single run_id.
4. **Durable coverage anchor valid** (§6a). The exact coverage policy (first/final dispatchable-bar
   identity, timeframe, effective grace-seconds) that was actually in force while this operation ran
   must be durably provable, not merely recomputed from current mutable configuration → otherwise
   the classifier cannot proceed past `unknown_incomplete_bar_coverage` for a `completed_no_trade`
   attempt, per §6a's binding rule, regardless of how clean the recomputed coverage otherwise looks.
5. **Complete expected-bar coverage** (§6 item 2, corrected — Repairs 1/2/5/7: the real production
   first/final dispatchable-bar bounds, coverage across recovery gaps and late-start gaps, evaluated
   across the full run lineage from step 3) → any expected bar identity missing, unconfirmed, or
   aggregate-counter-inconsistent blocks further progress, routes to
   `unknown_incomplete_bar_coverage`.
6. **Zero unresolved or contradictory claims.** Any unresolved `sys_autonomous_daily_bar_dispatches`
   claim within the expected-bar set (`claimed`/`uncertain`/`failed`) blocks `completed_no_trade` outright,
   routes to `unknown_unresolved_dispatch_claim`, even if every other bar for the day looks clean.
   Any nonzero signal with no `oms_outbox` row (§5 tier 4/5) → `unknown_order_evidence_conflict`. Any
   extra/future/inconsistent claim outside the expected set (§6 item 2 step 11) → the same
   contradiction handling. **This step runs regardless of whether tier-1/tier-2
   activity evidence (§5) has already been observed** — a confirmed fill for one bar does not resolve
   an unresolved claim or evidence gap for a *different* bar within the same operation. Concretely:
   confirmed fill + unresolved claim elsewhere → `evidence_degraded` under the matching `unknown_*`
   code, **not** a terminal classification yet. Once the unresolved evidence is subsequently repaired
   (a delayed write lands, an operator reconciles, a retried read succeeds), the same operation may
   finalize — as `completed_with_activity`, since the fill evidence from step 7 below still applies
   once the chain resolves clean.
7. **Only then, classify activity versus no-trade.** With steps 1–6 all resolved clean, apply §5 (the
   activity hierarchy) then §6 (the no-trade hierarchy) in order: any confirmed fill or durable
   order-reaching-broker evidence (§5 tiers 1–2) decides `completed_with_activity` and nothing below
   it in §5 downgrades that; otherwise the §6 no-trade hierarchy applies. If neither fully resolves
   (e.g. a genuine `unknown_runtime_stop_unproven` residual), fall through to §7's remaining
   `unknown_*` triggers.
8. **Terminal broker/order activity cannot be erased by process-local zero counters** — this is
   structural, not a runtime check: the classifier never reads `AppState`'s in-memory diagnostic
   fields at all (§6), so there is no code path by which a zero-valued process-local counter could
   ever override a durable `oms_outbox`/`oms_inbox` row.

**No reason code in §10 is unconditionally immune to an earlier step above.** In particular,
`activity_fill_confirmed`/`activity_order_submitted`/`activity_decision_accepted` are the
highest-precedence evidence *within* step 7's activity-vs-no-trade choice — they are not immune to
steps 1–6. §10's reason-code table is corrected accordingly (the "Prohibited contradictory evidence"
column for each activity code now names the applicable global blocker, not "none").

---

## 9. Restart and idempotency contract (E1.8)

- **Finalization identity**: the finalization write is itself an ordinary CAS transition through
  the existing `transition_autonomous_daily_operation` machinery (or a narrowly extended sibling
  that additionally sets `outcome`/`finalized_at_utc` atomically in the same `UPDATE`), guarded by
  `WHERE operation_id=$ AND state=$expected_from AND state_version=$expected_version`. Same
  `operation_id` + same evidence snapshot → same computed classification → the same CAS write either
  applies once or (on retry) is refused because `state` has already advanced — **zero duplicate
  lifecycle events**, exactly the guarantee already proven for every other transition in this table
  by `scenario_autonomous_daily_operation_store_01.rs`'s `stale_version_writes_nothing` /
  `wrong_expected_state_writes_nothing` / `exact_retry_returns_already_applied` tests. No new
  mechanism is required — E2 reuses the existing CAS contract verbatim.
- **Already-terminal operation → read-only, no reclassification.** Any finalization attempt against
  an operation whose `state` is already `completed*` must short-circuit to a read of the existing
  row and return it unchanged — it must never re-run the classifier or re-evaluate evidence for an
  already-terminal row. This is enforced for free by the CAS guard (the expected `from_state` for a
  finalization transition is `stopping`/`stop_retrying`, never a `completed*` state, so a retried
  finalization attempt against an already-terminal row simply fails the CAS precondition) — E2's
  finalizer must treat that CAS failure as "already finalized, re-read and return," not as an error.
- **Classification write committed but acknowledgment lost**: the same authoritative-re-read pattern
  D4 already established for dispatch-claim completion (`reconfirm_dispatch_completion_or_fail_
  closed`) applies here verbatim — on any ambiguous write result (`Ok(false)` interpreted as "CAS
  didn't apply" or a connection error after send), re-read the row by `operation_id`; if it shows the
  exact expected `outcome`/`state`/`finalized_at_utc` already committed, accept that as authoritative
  success; otherwise treat as not-yet-finalized and retry on a future tick (never fabricate success,
  never re-attempt a second classification of the same evidence snapshot without first re-reading).
- **Evidence changes after terminal finalization**: no silent rewrite. Because `completed*` states
  have no legal outbound edge in the existing transition graph, a normal coordinator tick
  structurally cannot alter a terminal row's `outcome` once written. Any future correction (e.g. a
  late-arriving fill discovered by reconcile after `completed_no_trade` was recorded) requires an
  explicit, separate, out-of-scope reconciliation/manual process — not an automatic Phase E
  reclassification path. This is deliberately deferred rather than designed here (§14).
- **Schema sufficiency**: `state_version` (optimistic-concurrency token) plus the existing CAS
  UPDATE pattern is sufficient for E2's finalization write. No additional schema field or table is
  required — `outcome`/`finalized_at_utc` already exist (migration `0048`), and the transition-event
  table already durably records the `stopping`/`stop_retrying` → `completed*` transition once E2
  starts taking that edge.
- **Deferred: durable risk-denial correlation (corrected — Correction 4, binding disposition)**.
  `sys_risk_denial_events` (migration `0026`) cannot today be durably joined to an operation, run,
  or strategy evaluation (§5 tier 4). `no_trade_all_signals_blocked` is therefore **not** authorized
  for E2 to implement from the current schema, under this or any other name. A future, separately
  authorized patch — out of scope for E1 and E2 alike — would need to add a durable correlation
  column (e.g. `operation_id`/`run_id`/`evaluation_id` on `sys_risk_denial_events`, or an equivalent
  join table) plus a migration, before this reason family could be safely implemented. Until then,
  every nonzero uncovered signal routes to `unknown_order_evidence_conflict` (§6 item 4, §7), never
  to a no-trade reason.
- **Database-failure contract (new — Correction pass 2, Repair 9).** `unknown_database_unavailable`
  (§7, §8 step 1) is a **classifier/result-level** truth — it describes what the finalizer could
  determine on this attempt, not a guarantee that any durable write happened. Two distinct cases,
  never conflated:
  - **Complete outage** (the operation row itself cannot be loaded, or the DB is unreachable before
    any evidence read succeeds): the finalizer returns a typed database-unavailable result and
    performs **no** durable write attempt of any kind. No terminal outcome, no blocker transition,
    nothing claimed persisted. The finalization attempt is simply retried on a later coordinator
    tick (§3's eligibility conditions still hold — the operation remains `stopping`/`stop_retrying`
    until a real transition succeeds). This is process-local operator truth only, exactly like every
    other `unknown_database_unavailable` case, until durability returns.
  - **Partial read failure** (the operation row *was* loaded successfully, but a later evidence query
    — lineage, coverage, activity — fails): a best-effort fail-closed blocker write (the
    `stopping`/`stop_retrying -> evidence_degraded` edge, §3.3) **may** be attempted, but only when
    the existing DB transition seam used to load the operation row remains usable (i.e. the failure
    is scoped to the specific evidence query, not the connection/pool itself). This write is subject
    to the same authoritative-re-read discipline as every other CAS write in this section: the
    finalizer must never claim the blocker was durably persisted without re-reading the row and
    confirming the exact expected `state`/`state_reason_code` landed. If the write itself also fails
    or cannot be confirmed, the attempt reports `unknown_database_unavailable` exactly as the
    complete-outage case does — never a fabricated "blocker written" claim.
  - **No raw SQL, connection string, or credential text ever enters durable truth** — `state_reason_
    code`/`state_blocker_signature`/any bounded detail field carries only the closed-set reason codes
    from §10, never a rendered DB error (this is already the standing rule the existing CAS/blocker
    machinery follows for every other blocker type in §1.1; this contract does not loosen it for
    database-failure blockers specifically).
  - **No busy loop**: because a failed finalization attempt never advances `state_version` (the CAS
    guard already prevents that structurally), a repeated `unknown_database_unavailable` result on
    successive coordinator ticks produces no duplicate transition events and no duplicate
    notification (§12's structural CAS-success-gated dedup already covers this) — it is simply
    retried, at the coordinator's own existing tick cadence, with no additional backoff mechanism
    required beyond what the coordinator's ordinary tick loop already provides.

---

## 10. Bounded reason-code matrix (E1.9)

All codes below are ≤128 chars (matches the existing `outcome` column's bound). No free-form
SQL/provider/credential/panic text may ever become part of a durable `outcome` value — only these
closed codes.

Corrected (pass 1, Corrections 4/5/6): `no_trade_all_signals_blocked` and `no_trade_no_bar_expected`
are removed from the initial reason set (§6, §9); `unknown_insufficient_bar_evidence` is retired in
favor of the broader `unknown_incomplete_bar_coverage`; `unknown_assignment_identity_unavailable`
is new (§6 item 2, §7); `unknown_order_evidence_conflict`'s required evidence no longer references
a risk-denial check. Corrected further (pass 2, Repairs 6/8): `unknown_run_lineage_unavailable` is
new (§6b, §7); every "Prohibited contradictory evidence" cell for the three activity codes is
corrected from "none"/an in-hierarchy-only caveat to name the applicable §8 steps 1–6 global
blocker, resolving the precedence contradiction (§8); `unknown_incomplete_bar_coverage`'s required
evidence is widened to cover recovery-gap and late-start-gap bars and the durable coverage-anchor
precondition.

| Reason code | Terminal state | Required evidence | Prohibited contradictory evidence | Bounded detail | Operator remediation |
|---|---|---|---|---|---|
| `activity_fill_confirmed` | `completed_with_activity` | ≥1 `oms_inbox` row, `event_kind IN ('fill','partial_fill')`, for a `run_id` in this operation's full run lineage (§6b) | any unresolved §8 step 1–6 global blocker (DB/identity/lineage/coverage-anchor/coverage/unresolved-claim) — corrected pass 2: within the activity-vs-no-trade choice itself (§8 step 7) this is the highest-precedence evidence and cannot be overridden by a weaker activity/no-trade tier, but it is not immune to steps 1–6 | fill count, first/last fill `broker_message_id` (bounded) | none — this is a clean close |
| `activity_order_submitted` | `completed_with_activity` | `oms_outbox.status IN ('SENT','ACKED')` or `dispatching_at_utc IS NOT NULL`, or `oms_inbox event_kind IN ('ack','cancel_ack','replace_ack','reject')`, zero fills, for a `run_id` in the full run lineage | any unresolved §8 step 1–6 global blocker; a fill exists (would upgrade to `activity_fill_confirmed`) | order count, statuses observed | none — clean close, no fill this session |
| `activity_decision_accepted` | `completed_with_activity` | ≥1 `oms_outbox` row for a `run_id` in the full run lineage that never advanced past `PENDING`/`CLAIMED`/`FAILED` | any unresolved §8 step 1–6 global blocker; a fill or broker-reaching status exists (would upgrade to a stronger code) | outbox row count, last status | review why the enqueue never reached the broker |
| `no_trade_strategy_evaluated_no_signal` | `completed_no_trade` | §8 steps 1–6 all resolved clean (operation/DB authority, identity, full run lineage, durable coverage anchor per §6a, complete expected-bar coverage per §6 item 2 including recovery/late-start gaps, zero unresolved claims), every evaluated signal flat/zero | any nonzero signal, any incomplete bar coverage, any unresolved §8 step 1–6 global blocker | evaluation count, expected-bar count | none — clean no-trade day |
| `unknown_assignment_identity_unavailable` | nonterminal (`evidence_degraded`) | current assignment/runtime-binding resolution fails, or its derived identity does not match the operation's persisted `assignment_identity`/`runtime_binding_identity` | — | which identity component mismatched | investigate config drift between the persisted operation and the process attempting finalization |
| `unknown_run_lineage_unavailable` | nonterminal (`evidence_degraded`) | the operation's full `run_id` lineage (§6b) cannot be read, is empty despite a non-`NULL` current `run_id`, contains a duplicate/out-of-order `run_id`, or its final entry mismatches the operation's current `run_id` | — | lineage read failure detail / mismatched `run_id` pair (bounded) | investigate `sys_autonomous_daily_operation_events` for this `operation_id`; a genuine mismatch may indicate a concurrent writer or data corruption |
| `unknown_incomplete_bar_coverage` | nonterminal (`evidence_degraded`) | any expected bar identity (§6 item 2, including a recovery-gap bar per step 7 or a late-start-gap bar per step 8, across the full run lineage) missing, unconfirmed, or aggregate-counter-inconsistent; or the durable coverage-anchor precondition (§6a, §6 item 2 step 13) is not met | — | expected vs. proven bar counts | investigate completed-bar driver/provider health for this date, or (if the coverage-anchor precondition is the cause) confirm §6a's future durable authority has been populated for this operation |
| `unknown_unresolved_dispatch_claim` | nonterminal (`evidence_degraded`) | ≥1 `sys_autonomous_daily_bar_dispatches` row in `claimed`/`uncertain`/`failed` within the expected-bar set | — | claim identity `(local_symbol, timeframe, bar_end_ts)` | manual dispatch-claim recovery per Phase C's documented "Phase D owns manual recovery" note |
| `unknown_missing_evaluation_evidence` | nonterminal (`evidence_degraded`) | zero `strategy_signal_evaluations` rows exist across the full run lineage, whether or not the operation reached `running` (Correction 6: covers the removed `no_trade_no_bar_expected`'s never-ran case) | — | — | investigate signal-evaluation journal write path, or confirm the pre-running blocker that prevented `running` from ever being reached |
| `unknown_order_evidence_conflict` | nonterminal (`evidence_degraded`) | nonzero signal, no outbox row (no risk-denial check — `sys_risk_denial_events` is not durably correlatable, §5 tier 4) | — | evaluation identity | investigate the decision→outbox seam for a silent drop |
| `unknown_database_unavailable` | nonterminal (`evidence_degraded`) | any evidence-gathering DB read failed | — | none (never surface raw DB error text) | retry once DB is reachable; this code short-circuits all other checks; §9 defines the exact complete-outage-vs-partial-read-failure write contract |
| `unknown_runtime_stop_unproven` | nonterminal (`evidence_degraded`) | `stopped_at_utc` present but eligibility (§3.2) cannot be otherwise fully confirmed | — | — | operator investigation of the specific contradictory facts found |

---

## 11. Read-only API contract (E1.10)

Not implemented in E1. Frozen for E4.

- **Canonical route(s)**: net-new, per the already-frozen §17/§11 rule in the `01a` spec —
  `GET /api/v1/autonomous/daily-operation` (single operation, defaults to the current market date)
  and `GET /api/v1/autonomous/daily-operations?limit=` (history, backed by the already-existing
  `mqk_db::list_recent_autonomous_daily_operations`, limit clamped `[1,100]`). Existing
  `readiness`/`paper-status`/`preflight` routes are extended **additively only** with a small
  outcome summary block — never restructured.
- **Backend**: reads only from `sys_autonomous_daily_operations` /
  `sys_autonomous_daily_operation_events` via the already-existing `fetch_autonomous_daily_
  operation_by_id`/`_for_slot`/`list_recent_autonomous_daily_operations` functions. Strictly
  read-only — per §13 of the `01a` spec, new daily-operation GET routes must **not** repeat the
  `GET /api/v1/autonomous/readiness` DB-write deviation.
- **`truth_state` vocabulary** (net-new for this response type, since none of the three existing
  autonomous response types implement the canonical `no_db`/`backend_unavailable`/`active` trio
  verbatim): `active` (row found, fields authoritative) | `not_found` (no operation row exists yet
  for the requested date — a legitimate empty state, distinct from unavailability, per
  `gui_rules.md`'s "distinguish unavailable, empty, and present") | `backend_unavailable` (DB pool
  not configured) | a query-failure variant matching the existing `NoTradeDiagnosticsResponse`
  precedent (`query_failed`) for a DB present but the read itself failing.
- **Not-finalized behavior**: when the row exists but `state` is nonterminal, `outcome_class`/
  `outcome_reason_code`/`finalized_at_utc` are `null`, and a distinct `finalization_status` field
  (e.g. `not_yet_eligible` | `awaiting_finalization` | `blocked_insufficient_evidence` — the last
  mapped from `state=evidence_degraded` with an `unknown_*` reason) communicates why, without ever
  fabricating a default `no_trade`/`with_activity` value while pending.
- **Response fields** (matching the mission's required list, all sourced from already-existing
  columns/functions — no new evidence computation happens in the route handler itself, only in the
  E2B classifier that already wrote `outcome`): `operation_id`, `market_date`, `state`,
  `outcome_class` (derived from `state` when terminal), `outcome_reason_code` (= `outcome` column),
  `finalized_at_utc`, `run_id`, `bars_observed`, `bars_dispatched`, `strategy_evaluation_count`,
  `order_activity_count`, `fill_count`, `evidence_state`, `evidence_blockers` (bounded list of the
  §10 nonterminal reason codes that currently apply, if any).
- **Pagination**: `daily-operations?limit=` follows the existing `list_recent_autonomous_daily_
  operations` clamp `[1,100]`; no cursor/offset scheme is introduced.

---

## 12. Notification contract (E1.11)

Not implemented in E1. Frozen for E3.

- **One daily outcome notification**, sent exactly once per operation, on the transition **into**
  `completed_no_trade` or `completed_with_activity` — reusing the exact same dispatcher pattern
  `log_coordinator_outcome` already uses for `Started`/`RuntimeStopped`
  (`session_controller.rs:294-463`), via `discord_notifier.notify_run_status` or an equivalent new
  narrow call. Dedup is structural, not a new mechanism: because the finalization write is a CAS
  transition (§9), a second tick can never observe the transition succeeding twice — the coordinator
  only calls the notifier on the tick where the CAS write itself reports success, exactly like the
  existing `newly_applied`-gated `ManualInterventionRequired` alert.
- **One warning-level notification** for the transition into `evidence_degraded` with an
  `unknown_*` reason at finalization time — parallel to the existing `ManualInterventionRequired`
  critical-alert pattern but at `severity: "warning"`, since it is an evidence gap requiring
  investigation, not necessarily an unsafe condition.
- **No notification for repeated reads/replays** — structurally guaranteed, since the new
  `daily-operation[s]` GET routes (§11) never call the CAS transition path at all, and an
  already-terminal operation's finalization attempt is a read-only no-op (§9).

---

## 13. Implementation decomposition (E1.12)

Authorized after independent acceptance of this E1 contract. **No patch beyond E2A is authorized by
this document alone** — each subsequent phase requires its own explicit go-ahead per this bundle's
one-patch-per-turn discipline. Where §§3–10 above use the bare shorthand "E2" in ambient prose
inherited from pass 1, read it as "whichever of E2A/E2B implements that specific piece" per the split
below — E2A owns durable evidence-foundation reads/writes, E2B owns the pure classifier and the
finalization CAS write.

### E2 vs. E2A/E2B decision (corrected — Correction pass 2, Repair 10, supersedes pass 1's Correction 5)

Pass 1 resolved this question **no** — it found every primitive §6 item 2's coverage proof needs
already existed and composed without new schema, so a single E2 was authorized. That finding is now
corrected: §6a and §6b (new, this correction) each confirm a genuine durable-evidence-foundation gap
pass 1's audit missed:
- **§6a**: the existing `daily_data_readiness_evaluated` evidence payload
  (`build_pre_start_evidence_detail`, `daily_data_readiness.rs:1444-1483`, confirmed by direct source
  read) does not persist the exact first/final dispatchable-bar identity, timeframe, or effective
  grace-seconds actually in force when an operation ran — only assignment/timeframe/readiness-state/
  blockers. §6 item 2's coverage composition can be *recomputed* from current mutable configuration,
  but not *durably re-read* as what was actually true when the operation ran. This is precisely the
  "exact expected set cannot be reconstructed after restart without assuming current mutable
  environment configuration is unchanged" gap the mission's REPAIR 3 named.
- **§6b**: no existing `mqk-db` read helper aggregates an operation's full `run_id` lineage across
  recovery — `list_autonomous_daily_operation_events` (`autonomous_daily_operation.rs:1280-1298`,
  confirmed by direct source read) returns the raw, `limit`-bounded transition list only, with no
  `to_state`/`run_id` filtering or deduplication. Reading only `operation.run_id` at finalization
  time would silently drop earlier runs' activity/evaluation evidence.

Both gaps are durable-evidence-foundation work — they must be built, proven against a real test DB,
and independently accepted **before** a classifier that depends on them is safe to build on top of.
This is exactly the mission's own dividing line ("E2A should likely: extend the existing readiness/
start evidence payload with exact coverage authority; add an exact operation-run-lineage read
helper; prove restart reconstruction; contain no classifier, finalization, coordinator invocation,
API, GUI, or notification"). **Resolved: E2A/E2B split is now authorized. E2A is the next patch
after this E1 correction is accepted — nothing beyond E2A is authorized yet.**

### E2A — durable coverage-anchor and run-lineage evidence foundation (next authorized patch, not implemented in E1)

- **Mission**: (1) extend the existing `daily_data_readiness_evaluated` pre-start evidence payload
  (§6a's preferred future authority — reusing the existing event, not a new table, unless E2A's own
  audit proves the existing JSON `detail` payload cannot safely carry the additional fields) with the
  durable coverage-anchor fields §6a lists (first/final dispatchable-bar identity, timeframe,
  effective grace-seconds, session-plan identity, coverage schema version); (2) add a narrow,
  purpose-built `mqk-db` read helper that aggregates an operation's full `run_id` lineage per §6b's
  exact query shape, unbounded by (or explicitly bounded well above) the general-purpose
  `list_autonomous_daily_operation_events` cap; (3) prove both survive a restart (a fresh process,
  fresh `AppState`, reading only durable storage, reconstructs the exact same coverage anchor and
  run lineage a live process would compute).
- **Likely files**: `core-rs/crates/mqk-daemon/src/daily_data_readiness.rs` (evidence payload
  extension), `core-rs/crates/mqk-db/src/autonomous_daily_operation.rs` (new run-lineage read
  helper).
- **Schema impact**: none expected — the preferred seam extends an existing JSON `detail` payload on
  an existing event type; a new migration is authorized only if E2A's own audit finds the existing
  payload cannot safely carry the new fields (§6a already names this as the fallback, not the
  default).
- **Test binaries**: new `scenario_autonomous_daily_coverage_anchor_and_run_lineage_01.rs` (or
  equivalent) — proves the extended evidence payload round-trips exactly, proves restart
  reconstruction of both the coverage anchor and the run lineage across a synthetic recovery cycle
  (reusing the `phase_d_full_day_lifecycle` recovery fixture pattern), and proves the lineage
  helper's contradiction-detection (duplicate/out-of-order `run_id`, mismatch against current
  `run_id`) fails closed to the shape §6b/§7 require.
- **Hard exclusions**: no classifier, no finalization CAS write, no coordinator invocation, no API
  route, no GUI, no notification wiring, no change to the `is_legal_operation_transition` graph.
- **Acceptance boundary**: both durable authorities (coverage anchor, run lineage) proven correct and
  restart-safe in isolation against a real test DB; zero production call sites invoke either as
  outcome-classification input yet.

### E2B — strict outcome classifier and finalization CAS (not authorized until E2A is independently accepted)

- **Mission**: implement the pure evidence-gathering reads (§1, §5, §6, consuming E2A's durable
  coverage-anchor and run-lineage authorities rather than recomputing them) and the pure
  classification function (§8's corrected precedence, §10's code table) in isolation, plus the CAS
  finalization write path (extends or sits beside `transition_autonomous_daily_operation` to also
  set `outcome`/`finalized_at_utc` atomically in the same `UPDATE`), plus the two new
  `stopping`/`stop_retrying -> evidence_degraded` legal-transition edges authorized by §3.3, plus
  the §9 database-failure write contract (complete-outage vs. partial-read-failure, authoritative
  re-read before any success claim). Not called from any production tick.
- **Likely files**: `core-rs/crates/mqk-db/src/autonomous_daily_operation.rs` (the two new legal
  transition edges, finalization CAS function), a new
  `core-rs/crates/mqk-daemon/src/state/autonomous_daily_outcome_classifier.rs` (pure logic,
  DB-read-only, consuming E2A's helpers rather than re-deriving coverage/lineage).
- **Schema impact**: none — every column/state this patch needs already exists (§1, §3.3, §7); the
  `evidence_degraded` open question that would have forced a possible schema decision is already
  resolved (§7 Correction 1, pass 1) with no migration required.
- **Explicitly deferred, not part of E2B**: `no_trade_all_signals_blocked` (§9's binding disposition
  — requires a separately authorized `sys_risk_denial_events` correlation migration first);
  `no_trade_no_bar_expected` (§6 Correction 6 — no source-provable path found).
- **Test binaries**: new `scenario_autonomous_daily_outcome_01.rs` (already anticipated in the `01a`
  spec's §18 test matrix) — pure classifier unit tests (including full/partial/zero bar-coverage
  cases, recovery-gap and late-start-gap coverage cases, an assignment-identity-mismatch case, a
  run-lineage-contradiction case, and the §8 step 6 "fill + unresolved claim → evidence_degraded,
  then resolves to completed_with_activity once repaired" case) plus DB-backed CAS finalization
  tests (idempotent replay, concurrent finalize race, terminal-state immutability, the
  `stopping`/`stop_retrying -> evidence_degraded -> stopping` recovery round-trip, and the §9
  complete-outage vs. partial-read-failure distinction).
- **Hard exclusions**: no coordinator wiring, no API route, no GUI, no notification wiring, no
  migration, no `sys_risk_denial_events` schema change.
- **Acceptance boundary**: classifier + finalize CAS function + transition-graph extension proven
  correct and idempotent in isolation against a real test DB, built entirely on top of E2A's already-
  accepted durable authorities; zero production call sites invoke it yet.

### E3 — coordinator finalization integration and restart-safe reconciliation
- **Mission**: wire E2B's classifier into `autonomous_daily_coordinator.rs`'s handling of
  `AwaitingOutcomeFinalization` so the coordinator invokes it exactly when §3.2's eligibility holds;
  wire the §12 notifications; prove restart-safety (crash between stop and finalization resumes
  correctly, never double-notifies, never re-classifies a terminal row).
- **Likely files**: `autonomous_daily_coordinator.rs`, `session_controller.rs`'s
  `log_coordinator_outcome`.
- **Schema impact**: none.
- **Test binaries**: extends `scenario_autonomous_daily_phase_d_integration_01.rs` or a new sibling
  with a full-day-to-finalization proof for each of the three terminal paths plus the
  `evidence_degraded` path, and a restart-mid-finalization proof.
- **Hard exclusions**: no API route, no GUI, no migration.
- **Acceptance boundary**: a full synthetic day proves start→running→dispatch→stop→finalize end to
  end for all classification outcomes against an isolated test DB.

### E4 — read-only API/read-model projection
- **Mission**: implement §11's routes and additive summary fields exactly as frozen.
- **Likely files**: `api_types.rs`, a new `routes/autonomous_daily_operation.rs` (or extends
  `autonomous_paper_status.rs`), `routes.rs` registration.
- **Schema impact**: none.
- **Test binaries**: new `scenario_autonomous_daily_operation_api_01.rs` (already anticipated).
- **Hard exclusions**: no GUI (Phase F), strictly read-only (no DB write of any kind from these
  handlers).
- **Acceptance boundary**: routes proven against every `truth_state`/not-finalized/no-db/not-found
  case in §11.

### E5 — integrated Phase E proof and reconciliation
- **Mission**: full regression pass across E2A–E4, a dedicated Phase E closure guard script, ledger/
  README reconciliation marking Phase E complete, explicit reaffirmation of Phase F/G/Bundle 4
  boundaries (§14).
- **Likely files**: `docs/specs/autonomous_daily_paper_operations_01e_phase_e_closure.md`,
  `scripts/guards/validate_autonomous_daily_paper_operations_01e_phase_e_closure.ps1`, ledger,
  README, README_TECHNICAL.
- **Schema impact**: none additional.
- **Hard exclusions**: Phase F, Phase G, Bundle 4, live capital, GUI, soak.
- **Acceptance boundary**: same rigor as D4's closure — every regression passes, every guard passes,
  ledger/README accurately reflect the final state.

---

## 14. Known limitations

- **No durable coverage-anchor authority exists today** (§6a, new — Correction pass 2, Repair 3/4) —
  the exact first/final dispatchable-bar identity, timeframe, and effective grace-seconds actually
  in force when an operation ran are not persisted anywhere durable today; only recomputable from
  current mutable configuration. `completed_no_trade` cannot safely finalize until E2A builds and
  proves the extension named in §6a. This is the primary reason the single-E2 decomposition (pass 1)
  is retired in favor of E2A/E2B (§13).
- **No run-lineage aggregation helper exists today** (§6b, new — Correction pass 2, Repair 6) — the
  existing `list_autonomous_daily_operation_events` read is a raw, bounded transition list with no
  `to_state`/`run_id` filtering. A recovered operation's earlier-run activity/evaluation evidence
  cannot be safely aggregated until E2A builds the narrow read helper §6b names.
- **`sys_risk_denial_events` cannot be durably correlated to an operation/run/evaluation today**
  (§5 tier 4, §9) — `no_trade_all_signals_blocked` is deferred, not authorized for E2B, and requires
  a separately authorized migration adding a correlation column before it can be implemented safely.
- **No source-provable path to `no_trade_no_bar_expected` was found** (§6 Correction 6) — removed
  from the initial reason set; a genuinely calendar-unavailable-for-the-whole-day operation remains
  operator-blocked (`manual_intervention_required`) rather than auto-classified, which is already the
  correct existing behavior and needs no new code.
- **The §6 item 2 bar-coverage composition is specified, not implemented or tested by this patch** —
  it reuses only already-existing pure helpers and persisted columns for the recomputation half, plus
  E2A's new durable coverage-anchor authority for the restart-safe-proof half; E2B must still write
  and prove the actual composition (full/partial/zero-coverage cases, recovery-gap and late-start-gap
  cases, the assignment-identity-mismatch fail-closed path, the run-lineage-contradiction fail-closed
  path) against a real test DB; this document does not itself constitute that proof.
- **No dedicated Phase B or Phase C closure guard script exists** in `scripts/guards/` (only Phase
  A/D/D4 have dedicated `autonomous_daily_paper_operations_*` validators) — Phase B/C closure
  integrity currently rests on the general guards and the ledger's own safety-confirmation blocks.
  This is a pre-existing gap, not introduced or required to be fixed by E1; noted for completeness.
- **The `01a` current-truth-and-contract document's own guard**
  (`validate_autonomous_daily_paper_operations_01a_audit.ps1`) enumerates its own closed set of
  required phrases; this E1 contract intentionally reuses `01a`'s already-frozen §13/§16/§17
  language verbatim in several places rather than re-deriving it, to avoid introducing a second,
  competing description of the same binding facts.
- **`unknown_runtime_stop_unproven`** (§10) is intentionally narrow and expected to be rare in
  practice — most "stop not yet proven" cases are simply not yet finalization-eligible (§3) and
  never reach the classifier at all. E2B should treat a high observed rate of this code as a signal
  that §3.2's eligibility conditions need re-auditing, not that the code's definition needs
  loosening.
- **This contract does not define the exact SQL joins/queries E2A/E2B will use** — only the evidence
  sources, precedence, and required durable proof for each reason code. Query construction is an
  implementation detail of the respective sub-phase.

---

## 15. Explicit Phase F/G and Bundle 4 boundaries

Unchanged from the bundle's standing contract (`01a` spec §17, §19; `01d` closure doc §10–§11),
reaffirmed here with no loosening:

- **Phase F** (GUI panel, runbook corrections, soak-evidence capture script) does not begin until
  E5 is independently accepted. No GUI file is touched by E1–E5.
- **Phase G** (closure audit, focused test matrix, ledger reconciliation for the whole bundle) does
  not begin until Phase F is independently accepted.
- **Bundle 4** (`DURABLE-PAPER-PORTFOLIO-AND-PNL-01-COMBINED`) remains untouched by this bundle
  entirely. The daily-operation outcome this contract defines never computes or claims a P&L figure,
  per the already-frozen `01a` §16 "outcome limitations" rule.
- **The 10–20-session unattended soak** does not begin until Phase F/G close Bundle 3 in full. This
  E1 patch does not start it, does not claim readiness for it, and does not change any readiness
  labeling in README/README_TECHNICAL beyond recording this E1 contract audit's own status.
- **Live capital** remains not ready and is untouched by any part of this contract or its
  downstream E2–E5 implementation.
