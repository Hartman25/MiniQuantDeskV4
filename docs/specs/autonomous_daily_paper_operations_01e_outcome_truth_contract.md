# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1 — Durable Outcome Authority and Evidence Contract

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-DURABLE-OUTCOME-AUTHORITY-AND-EVIDENCE-CONTRACT`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Phase: Phase E1 — durable daily outcome authority and evidence contract audit.
Scope: **read-only architecture audit plus documentation/guard patch.** No production code, test
code, or migration is added or modified by this patch. This document is the binding contract for
Phases E2–E5; it does not implement any part of them.

Starting HEAD: `544ec628708d0b8a5381aaaaef6c220af2f98253` ("fix: bind autonomous claims to
evaluation lineage").

**Correction pass** (`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-OUTCOME-CONTRACT-RECONCILIATION-01`,
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
  to conclusion for **every** expected completed bar in the operation's actual running interval and
  chose not to trade (§6's strict complete-coverage requirement, corrected — Correction 5). **Never**
  derived from "zero rows in `oms_inbox` with `event_kind='fill'`" alone — that fact is necessary
  but nowhere near sufficient (§6). Partial or unprovable bar coverage is never silently treated as
  no-trade — it routes to nonterminal `unknown_incomplete_bar_coverage` (§7, §10).
- **`completed_with_activity`**: reserved for a durable, evidence-complete proof that the strategy
  or its downstream pipeline produced real, DB-observable order/fill/decision activity — never
  inferred from `AutonomousPaperReadinessResponse`'s process-local `bar_tick_dispatch_count` /
  `last_bar_signal_qty` fields (§5).
- **Generic `completed`**: per §2, never chosen by the automatic classifier; reserved for a future
  out-of-scope manual path.
- **`unknown_insufficient_evidence`**: not a fourth terminal state (§7) — a `state_reason_code`
  applied while the operation remains (or moves to) the existing nonterminal `evidence_degraded`
  state (already a legal DB value; no migration required). See §7 for the full rationale.

---

## 5. Activity evidence hierarchy (E1.4)

Ordered from strongest to weakest signal, matching the real pipeline stages (never conflating a
completed *bar dispatch* with trading *activity* — a completed `sys_autonomous_daily_bar_dispatches`
row proves a strategy evaluation was confirmed, nothing about whether an order resulted):

1. **Broker fill** — `oms_inbox` row(s) with `event_kind IN ('fill','partial_fill')` for a
   `run_id` bound to this operation. Strongest possible evidence; if present, classification is
   `completed_with_activity` unconditionally (§8's precedence — nothing below can override this).
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
and unrelated no-signal evaluations for other bars — the presence of *any* tier-1 or tier-2 evidence
makes the whole operation `completed_with_activity`, regardless of what any individual bar's
evaluation otherwise showed.

---

## 6. No-trade evidence hierarchy (E1.5)

`completed_no_trade` requires **all** of the following to hold, durably:

1. `stopped_at_utc IS NOT NULL` (finalization-eligible, §3).
2. **Complete expected-bar coverage (corrected — Correction 5, strengthened)**. The original draft's
   "every existing dispatch row is `completed`" is necessary but not sufficient — it says nothing
   about bars that were expected but for which **no** dispatch row exists at all. The classifier
   must instead prove complete coverage using canonical session/timeframe truth, reusing only
   already-existing pure helpers and persisted columns (no new algorithm, no new schema):
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
   4. Derive the exact expected completed-bar end-timestamps as the subset of that session's full
      grid whose bar-close instant falls inside `[operation.started_at_utc,
      min(operation.stopped_at_utc, operation.effective_operation_close_utc)]` — the operation's
      actual running interval through effective close, per the mission's exact wording. If
      `started_at_utc` is `NULL` (the operation legally reached `stopping` — e.g. via the D2
      pre-running-state edges, `preflight_blocked -> stopping` and similar — without ever reaching
      `running`), the expected-bar set is empty by construction and item 3 below cannot be
      satisfied. This is **not** classified `completed_no_trade` under any reason code (Correction 6
      removes `no_trade_no_bar_expected` — no durable evidence proves "zero bars were ever
      applicable" as opposed to "the operation was blocked before it could find out"); it routes to
      `unknown_missing_evaluation_evidence` (§7), extended by this correction to cover both "reached
      `running` but zero evaluations exist" and "never reached `running` at all."
   5. Require every expected bar identity `(local_symbol, timeframe, bar_end_ts)` to have: a
      completed `sys_autonomous_daily_bar_dispatches` row; a non-null `evaluation_id` on that row;
      and a matching durable `strategy_signal_evaluations` row confirmed by the same exact-lookup
      pattern D4 already uses (`mqk_db::fetch_strategy_signal_evaluation`,
      `autonomous_completed_bar_driver.rs`, §12.1 of the `01d` closure doc).
   6. **Zero** missing expected bar identities.
   7. **Zero** extra, future, or inconsistent claims for this `operation_id` outside the expected
      set (a claim with a `bar_end_ts` the grid does not produce is itself a contradiction, not
      evidence to ignore).
   8. Aggregate counters must agree with the per-bar rows: `bars_observed`/`bars_dispatched` equal
      the proven-covered count, and `last_completed_bar_ts`/`last_dispatched_bar_ts` equal the final
      expected identity's `bar_end_ts`.

   Any missing, unprovable, or contradictory expected-bar identity — including a `claimed`/
   `uncertain`/`failed` dispatch row anywhere in the expected set — produces the nonterminal
   `unknown_incomplete_bar_coverage` (§7, §10), never a silent no-trade classification. This
   supersedes and retires the E1 draft's narrower `unknown_insufficient_bar_evidence` code (which
   only checked `bars_observed=0`): a day with some, but not all, expected bars proven is exactly as
   unproven as a day with zero, and both must use the same one authority — no free-form SQL join is
   specified here beyond what is stated; the exact query shape remains an E2 implementation detail
   (§14).
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
  exactly as unproven as a zero-coverage day and must use the same one authority).
- **New (Correction 5)**: the current process cannot resolve the operation's own assignment/runtime
  binding, or the freshly-derived `assignment_identity`/`runtime_binding_identity` does not exactly
  match the operation's persisted values (§6 item 2, step 2) → `unknown_assignment_identity_
  unavailable` — the classifier must never assume "probably still the same config" and proceed.
- Any DB read required to gather evidence fails →  `unknown_database_unavailable` — this check runs
  **first**, before any other evidence read, and short-circuits the entire classification (§8).
- `stopped_at_utc` is present but the eligibility conditions of §3.2 cannot otherwise be fully
  confirmed (e.g. a genuinely conflicting/contradictory pair of durable facts) →
  `unknown_runtime_stop_unproven`, used narrowly for this residual case only — it does not mean
  "stop was never attempted" (that case is simply not yet eligible, §3, and produces no
  classification attempt at all, successful or otherwise).

None of these may be silently reclassified as `no_trade_*` to "fill the gap" — this is the explicit,
already-frozen rule in the existing Phase A contract (§16 of the `01a` spec): *"When the durable
evidence needed to classify an outcome is itself incomplete or unavailable, the outcome is
`unknown_insufficient_evidence` — never a fabricated no-trade reason invented to fill the gap."*

---

## 8. Evidence precedence and conflict resolution (E1.7)

Deterministic, fail-closed order, evaluated top-to-bottom; the first matching rule decides:

1. **DB read failure anywhere in the evidence-gathering pass** → `unknown_database_unavailable`.
   Nothing else is evaluated once this fires — a partial evidence read must never be treated as a
   complete one.
2. **Assignment/runtime-binding identity cannot be resolved or does not match** (§6 item 2 steps
   1–2, corrected — Correction 5) → `unknown_assignment_identity_unavailable`. Runs before any bar
   evidence is gathered, because the expected-bar grid itself depends on knowing the correct
   symbol/timeframe.
3. **Any unresolved `sys_autonomous_daily_bar_dispatches` claim within the expected-bar set**
   (`claimed`/`uncertain`/`failed`) → blocks `completed_no_trade` outright, routes to
   `unknown_unresolved_dispatch_claim`, even if every other bar for the day looks clean.
4. **Any confirmed fill or durable order-reaching-broker evidence** (§5 tiers 1–2) → wins over
   everything else, including any coexisting no-trade-shaped diagnostic evidence for other bars.
   `completed_with_activity` is decided and no further evidence-gap check downgrades it.
5. **Missing journal evidence** (zero `strategy_signal_evaluations` rows for an operation that
   reached `running`) → blocks `completed_no_trade`, routes to
   `unknown_missing_evaluation_evidence`.
6. **Incomplete expected-bar coverage** (§6 item 2, corrected — Correction 5: any expected bar
   identity missing, unconfirmed, or aggregate-counter-inconsistent) → blocks `completed_no_trade`,
   routes to `unknown_incomplete_bar_coverage`.
7. **A nonzero signal with no `oms_outbox` row** (§5 tier 4/5, corrected — Correction 4: no
   risk-denial branch) → blocks `completed_no_trade`, routes to `unknown_order_evidence_conflict`.
8. **Terminal broker/order activity cannot be erased by process-local zero counters** — this is
   structural, not a runtime check: the classifier never reads `AppState`'s in-memory diagnostic
   fields at all (§6), so there is no code path by which a zero-valued process-local counter could
   ever override a durable `oms_outbox`/`oms_inbox` row.
9. **Otherwise**, apply §5 (activity hierarchy) then §6 (no-trade hierarchy) in order; if neither
   fully resolves, fall through to §7's residual `unknown_*` triggers.

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

---

## 10. Bounded reason-code matrix (E1.9)

All codes below are ≤128 chars (matches the existing `outcome` column's bound). No free-form
SQL/provider/credential/panic text may ever become part of a durable `outcome` value — only these
closed codes.

Corrected (Corrections 4/5/6): `no_trade_all_signals_blocked` and `no_trade_no_bar_expected` are
removed from the initial reason set (§6, §9); `unknown_insufficient_bar_evidence` is retired in
favor of the broader `unknown_incomplete_bar_coverage`; `unknown_assignment_identity_unavailable`
is new (§6 item 2, §7); `unknown_order_evidence_conflict`'s required evidence no longer references
a risk-denial check.

| Reason code | Terminal state | Required evidence | Prohibited contradictory evidence | Bounded detail | Operator remediation |
|---|---|---|---|---|---|
| `activity_fill_confirmed` | `completed_with_activity` | ≥1 `oms_inbox` row, `event_kind IN ('fill','partial_fill')`, for a `run_id` bound to this operation | none — highest-precedence evidence, cannot be overridden | fill count, first/last fill `broker_message_id` (bounded) | none — this is a clean close |
| `activity_order_submitted` | `completed_with_activity` | `oms_outbox.status IN ('SENT','ACKED')` or `dispatching_at_utc IS NOT NULL`, or `oms_inbox event_kind IN ('ack','cancel_ack','replace_ack','reject')`, zero fills | none | order count, statuses observed | none — clean close, no fill this session |
| `activity_decision_accepted` | `completed_with_activity` | ≥1 `oms_outbox` row for this `run_id` that never advanced past `PENDING`/`CLAIMED`/`FAILED` | a fill or broker-reaching status exists (would upgrade to a stronger code) | outbox row count, last status | review why the enqueue never reached the broker |
| `no_trade_strategy_evaluated_no_signal` | `completed_no_trade` | §6 items 1–7 (including complete expected-bar coverage), every evaluated signal flat/zero | any nonzero signal, any incomplete bar coverage | evaluation count, expected-bar count | none — clean no-trade day |
| `unknown_assignment_identity_unavailable` | nonterminal (`evidence_degraded`) | current assignment/runtime-binding resolution fails, or its derived identity does not match the operation's persisted `assignment_identity`/`runtime_binding_identity` | — | which identity component mismatched | investigate config drift between the persisted operation and the process attempting finalization |
| `unknown_incomplete_bar_coverage` | nonterminal (`evidence_degraded`) | any expected bar identity (§6 item 2) missing, unconfirmed, or aggregate-counter-inconsistent | — | expected vs. proven bar counts | investigate completed-bar driver/provider health for this date |
| `unknown_unresolved_dispatch_claim` | nonterminal (`evidence_degraded`) | ≥1 `sys_autonomous_daily_bar_dispatches` row in `claimed`/`uncertain`/`failed` within the expected-bar set | — | claim identity `(local_symbol, timeframe, bar_end_ts)` | manual dispatch-claim recovery per Phase C's documented "Phase D owns manual recovery" note |
| `unknown_missing_evaluation_evidence` | nonterminal (`evidence_degraded`) | zero `strategy_signal_evaluations` rows exist, whether or not the operation reached `running` (Correction 6: covers the removed `no_trade_no_bar_expected`'s never-ran case) | — | — | investigate signal-evaluation journal write path, or confirm the pre-running blocker that prevented `running` from ever being reached |
| `unknown_order_evidence_conflict` | nonterminal (`evidence_degraded`) | nonzero signal, no outbox row (no risk-denial check — `sys_risk_denial_events` is not durably correlatable, §5 tier 4) | — | evaluation identity | investigate the decision→outbox seam for a silent drop |
| `unknown_database_unavailable` | nonterminal (`evidence_degraded`) | any evidence-gathering DB read failed | — | none (never surface raw DB error text) | retry once DB is reachable; this code short-circuits all other checks |
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
  E2 classifier that already wrote `outcome`): `operation_id`, `market_date`, `state`,
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

Authorized after independent acceptance of this E1 contract. **No patch beyond E2 is authorized by
this document alone** — each subsequent phase requires its own explicit go-ahead per this bundle's
one-patch-per-turn discipline.

### E2 — durable outcome classifier and finalization store seam (decomposition decided — Correction 5)

**E2 vs. E2A/E2B decision**: the mission requires this contract to state, based on actual source
proof, whether bar-coverage evidence aggregation needs its own dedicated evidence-foundation patch
before the classifier is authorized. Resolved: **a single E2 is authorized — no E2A/E2B split.**
Every primitive §6 item 2's strict coverage proof needs already exists and composes without new
schema or a newly-invented algorithm:
- the pure session-grid helper (`daily_data_readiness::intraday_grid_starts`),
- the per-operation persisted boundary columns (`session_open_utc`, `session_close_utc`,
  `effective_operation_close_utc`, `started_at_utc`, `stopped_at_utc`, migrations `0048`/`0049`),
- the existing assignment/runtime-binding derivation and identity-comparison helpers
  (`derive_assignment_identity`, `derive_runtime_binding_identity`,
  `state/autonomous_daily_operation.rs:506,537`), and
- the durable per-bar dispatch-claim rows already keyed on exact bar identity
  (`sys_autonomous_daily_bar_dispatches`, migration `0050`).

No new persisted evidence and no new pure-calculation algorithm is required — only new *composition*
of these existing pieces, which is ordinary classifier-implementation work, not a foundation gap.
The `unknown_assignment_identity_unavailable`/`unknown_incomplete_bar_coverage` nonterminal codes
(§7, §10) are the fail-closed escape hatch for the one genuine risk this composition carries (the
process attempting finalization resolving a different assignment/binding than the one that actually
ran) — E2 must implement that check before relying on any config-derived value, never assume
current-process config still matches.

- **Mission**: implement the pure evidence-gathering reads (§1, §5, §6, including the §6 item 2
  bar-coverage composition above) and the pure classification function (§8's precedence, §9's code
  table) in isolation, plus the CAS finalization write path (extends or sits beside
  `transition_autonomous_daily_operation` to also set `outcome`/`finalized_at_utc` atomically in the
  same `UPDATE`), plus the two new `stopping`/`stop_retrying -> evidence_degraded` legal-transition
  edges authorized by §3.3. Not called from any production tick.
- **Likely files**: `core-rs/crates/mqk-db/src/autonomous_daily_operation.rs` (the two new legal
  transition edges, finalization CAS function), a new
  `core-rs/crates/mqk-daemon/src/state/autonomous_daily_outcome_classifier.rs` (pure logic,
  DB-read-only), reusing `daily_data_readiness::intraday_grid_starts` and the existing
  identity-derivation helpers rather than duplicating them.
- **Schema impact**: none — every column/state this patch needs already exists (§1, §3.3, §7); the
  `evidence_degraded` open question that would have forced a possible schema decision is now
  resolved (§7 Correction 1) with no migration required.
- **Explicitly deferred, not part of E2**: `no_trade_all_signals_blocked` (§9's binding disposition
  — requires a separately authorized `sys_risk_denial_events` correlation migration first);
  `no_trade_no_bar_expected` (§6 Correction 6 — no source-provable path found).
- **Test binaries**: new `scenario_autonomous_daily_outcome_01.rs` (already anticipated in the `01a`
  spec's §18 test matrix) — pure classifier unit tests (including full/partial/zero bar-coverage
  cases and an assignment-identity-mismatch case) plus DB-backed CAS finalization tests (idempotent
  replay, concurrent finalize race, terminal-state immutability, the new
  `stopping`/`stop_retrying -> evidence_degraded -> stopping` recovery round-trip).
- **Hard exclusions**: no coordinator wiring, no API route, no GUI, no notification wiring, no
  migration, no `sys_risk_denial_events` schema change.
- **Acceptance boundary**: classifier + finalize CAS function + transition-graph extension proven
  correct and idempotent in isolation against a real test DB; zero production call sites invoke it
  yet.

### E3 — coordinator finalization integration and restart-safe reconciliation
- **Mission**: wire E2's classifier into `autonomous_daily_coordinator.rs`'s handling of
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
- **Mission**: full regression pass across E2–E4, a dedicated Phase E closure guard script, ledger/
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

- **`sys_risk_denial_events` cannot be durably correlated to an operation/run/evaluation today**
  (§5 tier 4, §9) — `no_trade_all_signals_blocked` is deferred, not authorized for E2, and requires a
  separately authorized migration adding a correlation column before it can be implemented safely.
- **No source-provable path to `no_trade_no_bar_expected` was found** (§6 Correction 6) — removed
  from the initial E2 reason set; a genuinely calendar-unavailable-for-the-whole-day operation
  remains operator-blocked (`manual_intervention_required`) rather than auto-classified, which is
  already the correct existing behavior and needs no new code.
- **The §6 item 2 bar-coverage composition is specified, not implemented or tested by this patch** —
  it reuses only already-existing pure helpers and persisted columns (no new schema, no new
  algorithm), but E2 must still write and prove the actual composition (full/partial/zero-coverage
  cases, the assignment-identity-mismatch fail-closed path) against a real test DB; this document
  does not itself constitute that proof.
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
  never reach the classifier at all. E2 should treat a high observed rate of this code as a signal
  that §3.2's eligibility conditions need re-auditing, not that the code's definition needs
  loosening.
- **This contract does not define the exact SQL joins/queries E2 will use** — only the evidence
  sources, precedence, and required durable proof for each reason code. Query construction is an
  E2 implementation detail.

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
