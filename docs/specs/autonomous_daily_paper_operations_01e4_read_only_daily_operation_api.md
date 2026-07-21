# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4 — Read-Only Daily-Operation API Projection

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E4-READ-ONLY-DAILY-OPERATION-API-PROJECTION`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Phase: Phase E4 — read-only autonomous daily-operation API projection.

Starting HEAD: `6f96b984d2f3ccaf2679e26b3202908d530e6e0e` (`fix: preserve finalization
eligibility on policy failure` — the accepted E3 repair commit).

Status: **IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE.**
This document records what E4 built and proved; it is not itself an
acceptance record, and it does not close Phase E or Bundle 3.

## 0. Accepted foundation (recorded, not re-litigated)

```text
D1–D4: ACCEPTED — COMPLETE
PHASE D: ACCEPTED — COMPLETE

E1: ACCEPTED — COMPLETE
E2A: ACCEPTED — COMPLETE
E2B: ACCEPTED — COMPLETE
E3: ACCEPTED — COMPLETE

E4: IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE
E5: NOT STARTED
PHASE E: OPEN
BUNDLE 3: OPEN
```

E4 is built entirely on top of E1's binding read-only API contract (§11 of
`docs/specs/autonomous_daily_paper_operations_01e_outcome_truth_contract.md`),
E2A's accepted coverage-anchor/run-lineage authorities, E2B's accepted
strict classifier/finalization CAS, and E3's accepted coordinator
integration. None of the coverage-anchor logic, run-lineage validation,
terminal reason-code taxonomy, evidence-degraded semantics, or notification
behavior is reopened or re-derived by this patch.

## 1. Scope

E4 implements exactly the read-only projection layer the E1 contract §11
authorizes and the E3 document's own §16 "E4 boundary" describes:

1. the canonical single-operation and history GET routes;
2. typed API response models and one shared pure projection function;
3. the full `active`/`not_found`/`backend_unavailable`/`query_failed` truth
   vocabulary;
4. terminal and nonterminal outcome projection, sourced verbatim from
   already-durable columns — no classifier rerun;
5. full-run-lineage activity counts (strategy evaluations, order activity,
   fills) as a read-model aggregate, never a false zero;
6. an additive `daily_operation` summary block on the readiness,
   paper-status, and preflight responses;
7. proof that every route is read-only and fail-soft.

**No coordinator invocation, no classifier/finalizer call, no notification,
and no GUI surface are introduced.** E5 (integrated Phase E proof and
closure) is not started here.

## 2. Routes

```text
GET /api/v1/autonomous/daily-operation[?market_date=YYYY-MM-DD]
GET /api/v1/autonomous/daily-operations[?limit=N]
```

Both routes are mounted on the existing public (unauthenticated) router in
`core-rs/crates/mqk-daemon/src/routes.rs`, alongside `autonomous_readiness`
and `autonomous_paper_status`. Neither route accepts any HTTP method other
than `GET`.

### 2.1 Single-operation query

- No `market_date` parameter: the current market date is resolved via
  `state::resolve_autonomous_daily_session_plan_from_env(Utc::now(), &AutonomousDailyPlanTiming::production_default())`
  — the exact pure resolver the coordinator itself uses to derive today's
  session plan. This call performs no DB read, no write, and creates no
  operation or coverage-bound event; it is calendar/timing math only.
- Explicit `market_date=YYYY-MM-DD`: parsed with `NaiveDate::parse_from_str(_, "%Y-%m-%d")`.
  A parse failure returns HTTP `400` with `truth_state: "invalid_request"` —
  the only truth-state value paired with a non-200 status.
- The resolved date, together with `AppState::deployment_mode().as_db_mode()`
  and `AppState::adapter_id()`, is passed to
  `mqk_db::fetch_autonomous_daily_operation_for_slot` — the exact
  `(market_date, deployment_mode, adapter_id)` slot lookup, never a
  different query.

### 2.2 History query

`mqk_db::list_recent_autonomous_daily_operations(pool, effective_limit)` —
the same function the mission requires, ordered
`market_date desc, created_at_utc desc, operation_id desc` (unchanged,
defined in `mqk-db`). `requested_limit` defaults to `20` when the `limit`
query parameter is absent; `effective_limit = requested_limit.clamp(1, 100)`.
Both values are surfaced in the response so an operator can see when a
request was adjusted. No cursor, no offset.

## 3. Truth-state vocabulary

```text
active               -- operation row and every required read-model field
                         were queried successfully
not_found             -- DB reachable, no row exists for the requested slot
backend_unavailable   -- AppState has no configured DB pool
query_failed          -- DB pool exists but a required read failed
invalid_request       -- (single route only) malformed market_date; HTTP 400
```

All values other than `invalid_request` return HTTP `200`. `not_found` and
`backend_unavailable`/`query_failed` are never conflated: a `None` result
from `fetch_autonomous_daily_operation_for_slot` is `not_found`; an absent
`AppState.db` is `backend_unavailable`; a DB-present read that returns `Err`
is `query_failed`. An authoritative empty history list is `active`, never
`backend_unavailable`.

## 4. Response types

Defined in `core-rs/crates/mqk-daemon/src/api_types.rs`:

- `AutonomousDailyOperationApiRow` — one projected operation row (all
  datetimes RFC3339 strings; `operation_id`/`run_id` as UUID strings).
- `AutonomousDailyOperationResponse` — `{ canonical_route, truth_state,
  operation: Option<Row>, message }` for the single route.
- `AutonomousDailyOperationsResponse` — `{ canonical_route, truth_state,
  requested_limit, effective_limit, rows: Vec<Row>, message }` for the
  history route.
- `AutonomousDailyOperationSummary` — the compact additive block: `{
  truth_state, operation_id, market_date, state, finalization_status,
  outcome_class, outcome_reason_code, finalized_at_utc, evidence_state,
  evidence_blockers }`.

## 5. Shared pure projection (E4.10)

`core-rs/crates/mqk-daemon/src/routes/autonomous_daily_operations.rs` defines
exactly one projection function, `project_daily_operation_outcome`, used by
both routes and by the summary-block builder — no second terminal-state or
finalization-status mapping exists anywhere in this patch.

### 5.1 Terminal projection

Gated strictly on `record.state`:

```text
state == completed_no_trade       -> outcome_class = "no_trade"
state == completed_with_activity  -> outcome_class = "with_activity"
state == completed                -> outcome_class = "completed"
```

For all three: `outcome_reason_code = record.outcome` (verbatim, `None` for
generic `completed` in the ordinary case), `finalized_at_utc =
record.finalized_at_utc.map(rfc3339)`, `finalization_status = "finalized"`,
`evidence_state = "complete"`, `evidence_blockers = []`. The classifier
(`classify_autonomous_daily_outcome`) and the finalization CAS
(`mqk_db::finalize_autonomous_daily_operation`) are never called — every
field above is read from the already-durable row.

### 5.2 Nonterminal projection

Every other `state` value returns `outcome_class = None`,
`outcome_reason_code = None`, `finalized_at_utc = None` — no fabricated
default.

`finalization_status`:

```text
state == evidence_degraded AND stopped_at_utc.is_some()
  -> "blocked_insufficient_evidence"
state IN (stopping, stop_retrying) AND stopped_at_utc.is_some()
  -> "awaiting_finalization"
otherwise
  -> "not_yet_eligible"
```

`evidence_state`/`evidence_blockers` depend on the full-lineage activity-
count outcome (§6):

```text
counts unavailable due to invalid lineage -> "unavailable",
    blockers = ["unknown_run_lineage_unavailable"]
counts unavailable due to a DB read failure -> "unavailable",
    blockers = ["unknown_database_unavailable"]
counts available AND state == evidence_degraded -> "degraded"
counts available AND state != evidence_degraded -> "pending"
```

When `evidence_blockers` is otherwise empty and `record.state_reason_code`
is one of the eight closed `unknown_*` codes the E1/E2B contract defines,
that exact code is also surfaced in `evidence_blockers` — read verbatim,
never re-derived.

## 6. Full-lineage activity counts (E4.9)

`gather_daily_operation_activity_counts` computes, for one operation:

```text
1. lineage = mqk_db::fetch_and_validate_autonomous_daily_operation_run_lineage(pool, operation)
   Err            -> DatabaseUnavailable
   Ok(Err(_))     -> LineageUnavailable
   Ok(Ok(lineage))-> continue
2. lineage.is_empty() -> Available { 0, 0, 0 }  (authoritative: operation.run_id IS NULL)
3. strategy_evaluation_count = mqk_db::count_strategy_signal_evaluations_for_runs(pool, &lineage)
   (new, narrow, unbounded COUNT(*) helper — no truncation risk, unlike the
   existing bounded per-run list functions)
4. for each run_id in lineage:
     order_activity_count += mqk_db::outbox_load_all_for_run(pool, run_id).len()
     for each inbox row via mqk_db::inbox_load_all_for_run(pool, run_id):
       event_kind IN (fill, partial_fill)               -> fill_count += 1
       event_kind IN (ack, cancel_ack, replace_ack, reject) -> order_activity_count += 1
```

`outbox_load_all_for_run`/`inbox_load_all_for_run` are the exact unbounded,
any-status/any-event-kind helpers E2B's own evidence-gathering pass already
uses — reused verbatim, not re-derived. Fills are never double-counted in
`order_activity_count`. Any read failure at any step maps to
`DatabaseUnavailable`. `count_strategy_signal_evaluations_for_runs` is the
one new mqk-db helper this patch adds (`core-rs/crates/mqk-db/src/strategy.rs`) —
a single narrow, read-only `SELECT COUNT(*) ... WHERE run_id = ANY($1)`
query, added because no existing unbounded count helper exists for this
table and the existing bounded list functions risk silent truncation on a
busy day. No migration.

An unreadable/contradictory lineage or a downstream read failure never
produces a false zero: the API row's three count fields are `Option<i64>`,
serialized as JSON `null` whenever counts are unavailable, never `0`.

## 7. Additive summary blocks (E4.11)

`compute_daily_operation_summary(state: &AppState, now_utc: DateTime<Utc>) -> AutonomousDailyOperationSummary`
resolves the current slot exactly as the single-operation route does (same
canonical market-date resolver, same exact-slot lookup), then reuses
`project_daily_operation_outcome`. It never returns a `Result` — a failure
at any step (no DB pool, unresolvable market date, DB read failure) is
represented entirely within the summary's own `truth_state` field, so it is
structurally impossible for this function to change its caller's HTTP
status or any other field.

Every response-construction branch in the following three handlers supplies
`daily_operation: compute_daily_operation_summary(&st, now).await`:

```text
autonomous_readiness       (routes/system.rs)             — 2 branches
autonomous_paper_status    (routes/autonomous_paper_status.rs) — 3 branches
system_preflight           (routes/system.rs)              — 1 branch
```

(`system_preflight`'s only other return point, the early `current_status_snapshot`
failure, returns a different response type — `RuntimeErrorResponse` — and is
unaffected.) No existing field on any of these three responses was
restructured; `daily_operation` is a pure addition. No existing gate result
(`overall_ready`, `blockers`, `readiness_classification`,
`deployment_start_allowed`, etc.) changes because of this block.

## 8. Read-only enforcement (E4.12/E4.13)

The route module never calls: `create_or_recover_autonomous_daily_operation`,
`transition_autonomous_daily_operation`/`transition_autonomous_daily_operation_to_running`,
`finalize_autonomous_daily_operation`,
`classify_and_finalize_autonomous_daily_operation`,
`persist_autonomous_daily_finalization_blocker`,
`persist_autonomous_session_event`, `write_and_confirm_coverage_authority`,
`start_execution_runtime`, `stop_execution_runtime`, or
`tick_autonomous_daily_coordinator`. It never constructs a provider client,
broker client, or Discord notifier. Proven both structurally (a source-text
scan in `scenario_autonomous_daily_operation_api_01.rs`'s `b23`/`b24` tests,
mirrored by the new guard's checks `[9]`/`[10]`) and behaviorally (DB-backed
before/after row-count and lifecycle-event-count proofs, `b21`/`b22`).

Query-failure proof (`b04`/`b05`/`b05b`): a no-DB-pool `AppState` returns
`backend_unavailable`; a real-but-unreachable `PgPool` (constructed via
`connect_lazy` against an invalid address, so pool construction itself never
blocks or fails) returns `query_failed` for both routes.

## 9. Test matrix

File: `core-rs/crates/mqk-daemon/tests/scenario_autonomous_daily_operation_api_01.rs`
(37 tests: 11 non-DB router-level tests plus one non-DB structural source
scan, run every time; 25 DB-backed tests marked `#[ignore]`, run against the
real isolated port-5434 test database with `--include-ignored
--test-threads=1`). All 37 pass.

```text
Group A (router-level, no DB):        a01-a10
Group B (DB-backed truth/projection/counts/history/summary/read-only proof):
  b01-b24 (b12b added for the ack-vs-fill count distinction)
```

Regressions re-run clean, one binary at a time
(`--include-ignored --test-threads=1` where the file has ignored tests):

```text
scenario_autonomous_daily_outcome_coordinator_integration_01       16/16
scenario_autonomous_daily_outcome_classifier_and_finalization_01   67/67
scenario_autonomous_daily_coverage_anchor_and_run_lineage_01       41/41
scenario_autonomous_readiness_auton_truth01                        18/18
scenario_autonomous_paper_status_summary_01                       21/21
scenario_daemon_routes (covers GET /api/v1/system/preflight)       84/84
scenario_route_contract_rt01 (GUI-vs-daemon route contract)         2/2
scenario_gui_daemon_contract_gate (general API contract)           23/23
```

The full `mqk-daemon` integration suite was not run, per the mission's own
instruction.

## 10. Known limitations

- No pagination cursor/offset — `limit` clamping only, matching the E1
  contract's own frozen §11 pagination rule.
- `evidence_blockers` surfaces only the single current `state_reason_code`
  (when it is one of the eight closed `unknown_*` codes) plus the
  lineage/database-unavailable codes this route itself can detect — it does
  not attempt to reconstruct a fuller multi-cause evidence history; that
  remains the transition-event log's job (`GET` operation events, out of
  scope here).
- Generic `completed` rows project `evidence_state = "complete"` by the same
  rule as the two automatic terminal states, since a manually/administratively
  completed row has no further evidence gap this route can meaningfully
  surface; this is a read projection choice, not a new evidence-integrity
  claim.

## 11. E5 boundary

E4 implements no integrated end-to-end Phase E proof, no unattended-soak
preparation, no GUI surface, and no further coordinator/notification
behavior. E5 (not started, not authorized here) owns the integrated Phase E
proof and closure that determines whether Bundle 3 can proceed toward a
supervised soak. Phase E remains open; Bundle 3 remains open.
