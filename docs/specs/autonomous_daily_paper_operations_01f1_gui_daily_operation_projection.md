# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1 — GUI Daily-Operation Truth Projection

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-GUI-DAILY-OPERATION-TRUTH-PROJECTION`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Phase: Phase F1 — GUI autonomous daily-operation truth projection.

Starting HEAD: `4b6eec72cb65dec1fc2a8793e9d9d7bdde8328b4` (`test: harden autonomous
daily phase e proof` — the accepted E5/Phase E closing commit).

Status: **IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE.**
This document records what F1 built; it is not itself an acceptance record,
and it does not close Phase F, Phase G, or Bundle 3.

## 0. Accepted foundation (recorded, not re-litigated)

```text
D1–D4: ACCEPTED — COMPLETE
PHASE D: ACCEPTED — COMPLETE

E1: ACCEPTED — COMPLETE
E2A: ACCEPTED — COMPLETE
E2B: ACCEPTED — COMPLETE
E3: ACCEPTED — COMPLETE
E4: ACCEPTED — COMPLETE
E5: ACCEPTED — COMPLETE
PHASE E: ACCEPTED — COMPLETE

F1: IMPLEMENTATION COMPLETE — AWAITING CHATGPT AND OPERATOR ACCEPTANCE
F2: NOT STARTED
F3: NOT STARTED
PHASE F: OPEN
PHASE G: NOT STARTED
BUNDLE 3: OPEN
BUNDLE 4: NOT STARTED
```

F1 is built entirely on top of the accepted E1 outcome-truth contract and the
accepted E4 read-only daily-operation API (`176b4149` + repair passes
`b2328a93` + `11664945`, closed by E5's `4b6eec72`). No coverage-anchor logic,
run-lineage validation, terminal reason-code taxonomy, evidence-degraded
semantics, classifier behavior, or notification behavior is reopened,
re-derived, or reinterpreted by this patch. F1 adds a **consumption layer
only** — a GUI screen that renders the daemon's own already-projected truth
verbatim.

## 1. Scope

F1 implements exactly the GUI projection the mission authorizes:

1. Strict TypeScript response types mirroring the daemon's E4 API shapes.
2. A GUI-only `transport_state` distinction (`available` |
   `endpoint_unavailable`) layered in front of the daemon's own `truth_state`
   vocabulary, so a network/HTTP failure to reach a route is never reported
   as the daemon's own authoritative `not_found`.
3. Two new probes (`/api/v1/autonomous/daily-operation`,
   `/api/v1/autonomous/daily-operations?limit=20`) wired into the existing
   tracked probe assembly in `fetchOperatorModel`.
4. A dedicated, read-only `Daily Operations` operator screen.
5. Current-operation and recent-history rendering that preserves every
   backend truth distinction verbatim.
6. Null-count-vs-zero handling for the three full-run-lineage activity
   counts.
7. A screen-local source-authority helper for section-level (current vs.
   history) availability.
8. A focused GUI test matrix.

**No daemon route, response field, classifier, finalizer, coordinator,
notification, coverage, or lineage change is made by this patch.** F1 touches
no production Rust file and no migration.

## 2. Canonical backend routes (consumed verbatim)

```text
GET /api/v1/autonomous/daily-operation[?market_date=YYYY-MM-DD]
GET /api/v1/autonomous/daily-operations[?limit=N]
```

The GUI never sends an explicit `market_date` query parameter in F1 — it
always requests the daemon's own canonical-current-slot resolution. The
`invalid_request` truth state (paired with HTTP 400, requires a malformed
`market_date`) is therefore not reachable from this GUI screen; it is
excluded from the GUI's typed truth-state unions on that basis, not because
the daemon route contract changed.

Full route contract: `docs/specs/autonomous_daily_paper_operations_01e4_read_only_daily_operation_api.md`.

## 3. GUI response types

`core-rs/mqk-gui/src/features/system/types/autonomousDailyOperations.ts`
defines:

- `AutonomousDailyOperationApiRow` — strict mirror of the daemon's
  `AutonomousDailyOperationApiRow` (`api_types.rs`). Every field name and
  nullability matches the daemon type exactly.
- `AutonomousDailyOperationSurface` — `{ transport_state, canonical_route,
  truth_state, operation, message }` for the single-operation route.
- `AutonomousDailyOperationsSurface` — `{ transport_state, canonical_route,
  truth_state, requested_limit, effective_limit, rows, message }` for the
  history route.

`truth_state` on `AutonomousDailyOperationSurface` is typed
`"active" | "not_found" | "backend_unavailable" | "query_failed" | null`;
on `AutonomousDailyOperationsSurface` it is typed
`"active" | "backend_unavailable" | "query_failed" | null`. `null` occurs
only when `transport_state === "endpoint_unavailable"`.

## 4. Transport-vs-daemon truth distinction

`transport_state` is a GUI-only concept, never sent by the daemon:

```text
transport_state = "available"            -> truth_state carries the daemon's own verbatim value
transport_state = "endpoint_unavailable" -> truth_state is always null; operation/rows are absent
```

`core-rs/mqk-gui/src/features/system/legacy.ts`'s
`mapAutonomousDailyOperationResponse` / `mapAutonomousDailyOperationsResponse`
are the sole mapping functions. A successful HTTP response whose
`truth_state` is one of the closed daemon-authoritative values (including
`not_found`) is preserved as `transport_state: "available"`. Any other
outcome — a network error, a non-2xx HTTP status (`fetchJsonCandidate`
reports these as `ok: false`), a missing/malformed body, or an unrecognized
`truth_state` string — maps to the fixed `endpoint_unavailable` sentinel.
This is the same "fail closed on the unexpected, never guess" convention
every other wrapper mapper in `legacy.ts` already follows
(`mapAutonomousPaperStatusWrapper`, `mapWatchlistStatusWrapper`, etc.).

A daemon `backend_unavailable` or `query_failed` response is **not**
converted into `endpoint_unavailable` — those are the daemon's own degraded
truth and remain visible as such (§6).

## 5. Operator-model integration

`core-rs/mqk-gui/src/features/system/api.ts`'s `fetchOperatorModel` adds both
routes to the existing tracked `probes` `Promise.all` array (not a
fire-and-forget sibling fetch, unlike `autonomousReadinessR`), so:

- a successful fetch lands the exact requested path in
  `dataSource.realEndpoints`;
- a failed fetch lands it in `dataSource.missingEndpoints`;
- the existing polling cycle (`useOperatorModel`'s single `setInterval`) is
  reused — no second timer is introduced.

`SystemModel` gained exactly two new fields:
`autonomousDailyOperation: AutonomousDailyOperationSurface` and
`autonomousDailyOperations: AutonomousDailyOperationsSurface`. The existing
`autonomousPaperStatus` (in-memory/session composite truth),
`sessionState` (calendar/trading-window truth), and `preflight` surfaces are
unchanged — they answer different questions and are not overloaded.

## 6. Dedicated screen contract

`core-rs/mqk-gui/src/features/autonomousDailyOperations/AutonomousDailyOperationsScreen.tsx`,
registered under screen key `dailyOperations`:

```text
title: "Daily Operations"
description: "Durable autonomous paper-day state, final outcome, evidence
              posture, and recent history."
monitorGroup: "operator"
```

Registered in `MONITOR_GROUPS.operator` next to `session`/`ops`/`dashboard`,
and in `LEFT_RAIL_SECONDARY` next to `session` (`screenRegistry.tsx`,
`leftRailNav.ts`).

The screen contains **zero** action, mutation, or control elements: no
button, form, input, `onClick` handler, `postJson`/`invokeOperatorAction`
reference, or direct `fetch` call. It only renders `model.autonomousDailyOperation`
/ `model.autonomousDailyOperations`, which are populated exclusively by the
existing read-only polling cycle.

### 6.1 Current-operation rendering

- `not_found` (or a missing `operation`) renders the fixed neutral copy: "No
  autonomous daily operation exists for the current canonical slot." — never
  an error state, never a fabricated operation.
- `backend_unavailable` / `query_failed` / `endpoint_unavailable` each render
  a distinct fail-closed notice (`truthNoticeFor`), distinguishable by title
  and tone.
- An `active` row renders every field listed in the mission verbatim:
  finalization status, outcome class/reason, evidence state/blockers, bar
  counters, activity counters (via the null-count formatter, §8), operation
  identity, and timestamps.
- Tone mapping (`finalizationTone`/`evidenceTone`) follows the mission's
  vocabulary: `finalized` + `evidence_state === "complete"` → good;
  `awaiting_finalization` / `not_yet_eligible` → neutral;
  `blocked_insufficient_evidence` → warn; `evidence_state` of `degraded` or
  `unavailable` → warn. A `finalized` row whose evidence is anything other
  than `complete` (i.e. generic `completed` with `evidence_state: "pending"`)
  renders warn, not good — it is never colored as if it were an
  automatic-classifier evidence-complete proof.
- Text labels are always rendered alongside color (finalization status,
  outcome class, evidence state, and every blocker code all render as plain
  text) — color is never the only signal.

### 6.2 History rendering

- Rows render in the exact order the daemon returns them
  (`market_date desc, created_at_utc desc, operation_id desc`) — the screen
  never re-sorts or re-groups.
- An authoritative `active` response with an empty `rows` array renders "No
  autonomous daily operations recorded yet." — distinct from any unavailable
  state.
- A top-level `query_failed` response keeps its bounded partial `rows`
  visible, but the panel renders an explicit "Partial — some rows
  unavailable" notice above the table; it is never presented as a complete
  authoritative history.
- `backend_unavailable` / `endpoint_unavailable` (non-partial) render the
  same fail-closed notice as the current-operation panel.
- No pagination, cursor, offset, or date-filter control exists in F1 — the
  default request remains `limit=20`.

## 7. Evidence blockers

`evidence_blockers` (bounded closed reason codes) are rendered verbatim as a
list whenever non-empty, for both the current-operation panel and (per row)
the history table's evidence-state column.

## 8. Null-count-vs-zero handling

`core-rs/mqk-gui/src/features/autonomousDailyOperations/formatDailyOperationCount.ts`
is the single pure formatter for `strategy_evaluation_count` /
`order_activity_count` / `fill_count`:

```ts
formatDailyOperationCount(null) === "Unavailable"
formatDailyOperationCount(0)    === "0"
formatDailyOperationCount(7)    === "7"
```

Both the current-operation panel and the history table route every count
field through this formatter — there is no other rendering path for these
three fields. `null` never renders as `"0"`, `"None recorded"`, `"No
activity"`, or `"No fills"`.

## 9. Source-authority behavior

`core-rs/mqk-gui/src/features/system/sourceAuthority.ts` gained
`classifyDailyOperationsSourceAuthority(single, history)`, a dedicated helper
distinct from the existing coarse `classifyPanelSources` per-panel
db/runtime/broker/placeholder classification (which the Daily Operations
screen also participates in, via a new `dailyOperations` `CorePanelKey`
entry, purely for the generic `WorkspaceFrame` authority badge every screen
already shows). `classifyDailyOperationsSourceAuthority` derives
section-level availability directly from each surface's own
`transport_state`:

```text
single.transport_state === "available"  -> current  = "available"
single.transport_state !== "available"  -> current  = "unavailable"
history.transport_state === "available" -> history = "available"
history.transport_state !== "available" -> history = "unavailable"
bothUnavailable = current === "unavailable" && history === "unavailable"
```

The screen's render logic then implements the mission's exact matrix:

```text
both available            -> screen renders both sections' own typed truth
single available, history unavailable -> current section renders, history section shows unavailable
single unavailable, history available -> history section renders, current section shows unavailable
both unavailable           -> one screen-level fail-closed truth notice, neither section attempted
```

Only `authority.bothUnavailable` gates the whole-screen notice — a single
section being unavailable never blocks the GUI's other screens or the other
section on this screen.

## 10. Test matrix

```text
core-rs/mqk-gui/src/features/autonomousDailyOperations/__tests__/formatDailyOperationCount.test.ts
core-rs/mqk-gui/src/features/autonomousDailyOperations/__tests__/sourceAuthority.test.ts
core-rs/mqk-gui/src/features/autonomousDailyOperations/__tests__/api.test.ts
core-rs/mqk-gui/src/features/autonomousDailyOperations/__tests__/screenSource.test.ts
```

Covers (numbered per the mission's F1.11 list): both routes requested;
`not_found` stays authoritative; `backend_unavailable` / `query_failed`
remain distinct from each other and from `endpoint_unavailable`; a network
failure maps to `endpoint_unavailable` never daemon `not_found`; an active
no-trade row and an active with-activity row both map without
reinterpretation; `awaiting_finalization` is retained as pending, not
finalized; evidence-degraded blockers are retained and rendered; generic
`completed` stays generic (`evidence_state: "pending"`, never `"complete"`);
null counts render `"Unavailable"`, real zero renders `"0"`; history order
is preserved verbatim; an authoritative empty `active` history renders as
such, not as unavailable; a `query_failed` history keeps bounded rows but
renders a partial notice; the screen is registered under the operator
monitor group and reachable from the left rail; the screen's source and
rendered output contain no button/form/input/onClick/`postJson`/
`invokeOperatorAction`/direct `fetch` call; `classifyDailyOperationsSourceAuthority`
is proven for all four availability combinations; the null/malformed-wrapper
fallback mapping fabricates no healthy truth; existing
dashboard/session/preflight model fields are unaffected by the new probes
being spliced into the tracked probe array's positional destructuring.

All four files are registered in `core-rs/mqk-gui/package.json`'s `test`
script. `npm test` (832 tests) and `npm run build` both pass.

## 11. Backend regression proof (no daemon change)

This patch requires no daemon source change. The following existing scenario
binaries were re-run, unmodified, against unmodified daemon source, as
required regression evidence:

```text
scenario_autonomous_daily_operation_api_01   50/50 (19 non-DB + 31 DB-backed, --include-ignored)
scenario_gui_daemon_contract_gate            23/23
scenario_daemon_routes                       73/73 (11 ignored, not required for this patch)
```

## 12. Known compatibility-session-event follow-up

E5's own closure spec (`autonomous_daily_paper_operations_01e_phase_e_closure.md`
§2) already documents that the coordinator's per-tick compatibility write
(`AutonomousSessionTruth` → `sys_autonomous_session_events`, via
`log_coordinator_outcome`) is unconditionally re-asserted on every tick,
including a read-only replay, because its `detail` text embeds per-tick
state. This is orthogonal to the E1–E4 durable lifecycle/outcome truth this
GUI screen reads, and F1 does not touch, invoke, or read from that write
path in any way — the GUI's two routes never call
`persist_autonomous_session_event` or any coordinator tick function (proven
structurally by the accepted E4 `b23`/`b24` tests, re-run clean in §11, and
dynamically by the accepted E5 Proof E). This item remains recorded, as it
was in E5, as a **Phase G efficiency/audit follow-up** — it is not an API GET
side effect (the GUI's GET routes cause zero DB mutation, per the accepted
E5 read-only proof), and it is not operation lifecycle authority (it is a
compatibility/observability write, not a `sys_autonomous_daily_operations`
state transition). F1 does not modify it and does not reopen this question.

## 13. F2 boundary

F2 (operator runbook and startup/shutdown procedure correction) is not
started by this patch. No runbook file is edited by F1.

## 14. F3 boundary

F3 (supervised soak-evidence preparation and evidence bundle) is not started
by this patch. No soak manifest or soak evidence is created by F1.

## 15. Phase G boundary

Phase G (final Bundle 3 closure audit) is not started by this patch. Bundle
3 remains open.

## 16. Soak / live-capital boundaries

The 10–20-session unattended autonomous paper soak has not started and is
not authorized by this patch. Live trading is not ready and is not
authorized by this patch.

## 17. F1 repair pass (RUNTIME-SHAPE-AND-HISTORY-BLOCKER-REPAIR-01)

`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-RUNTIME-SHAPE-AND-HISTORY-BLOCKER-REPAIR-01`
hardens two defects in the F1 implementation above, on top of starting HEAD
`c7ddccafebcd3dd761ef2fa54bb8cadeb6144b2a`. It does not reopen any other part
of this spec — §§1–16 above remain the F1 contract as originally built.

**Defect 1 — incomplete runtime validation.** §4/§5 above describe
`mapAutonomousDailyOperationResponse`/`mapAutonomousDailyOperationsResponse`
as validating truth_state and falling back safely on a malformed wrapper.
In the original F1 implementation this was true only at the wrapper's
top level: `operation: wrapper.operation ?? null` and
`rows: wrapper.rows ?? []` meant a successful HTTP 200 body with a missing
or structurally invalid `operation`/`rows` payload was not rejected — an
`active` truth_state with a missing `operation`, for example, would carry
`operation: null` through to the screen exactly like an authoritative
`not_found`.

**Defect 2 — history blockers not rendered.** §7 above states
`evidence_blockers` are "rendered verbatim as a list whenever non-empty, for
both the current-operation panel and (per row) the history table's
evidence-state column." Only the current-operation panel actually did this;
`HistoryPanel`'s row template rendered `row.evidence_state` but never
`row.evidence_blockers`.

### 17.1 Closed GUI-side vocabulary types

`types/autonomousDailyOperations.ts` now defines
`AutonomousDailyFinalizationStatus`, `AutonomousDailyOutcomeClass`, and
`AutonomousDailyEvidenceState` as closed string-literal unions mirroring the
three bounded vocabularies documented on the daemon's
`AutonomousDailyOperationApiRow` (`api_types.rs`). `finalization_status`,
`outcome_class`, and `evidence_state` on the GUI's own
`AutonomousDailyOperationApiRow` are typed against these unions instead of
`string`. The durable `state` field remains `string` — the backend lifecycle
state set is broader and is already represented verbatim, not reinterpreted
by the GUI.

### 17.2 Complete row validator

`isAutonomousDailyOperationApiRow(value: unknown): value is
AutonomousDailyOperationApiRow` is the single pure runtime validator for one
API row. It checks every required field's presence, type, and nullability;
rejects NaN/infinite/non-integer numbers on every count/timestamp field;
requires `evidence_blockers` to be an actual array of strings; and requires
`finalization_status`/`outcome_class`/`evidence_state` to be an exact member
of their closed vocabulary. It never repairs, defaults, coerces, trims,
sorts, or reinterprets a malformed field — a missing/`undefined` field is
rejected outright, never silently converted to `null` or `0`.

### 17.3 Complete wrapper validation

Both mapper functions in `legacy.ts` now enforce, before returning
`transport_state: "available"`:

```text
canonical_route must equal the exact expected daemon route string
truth_state must be a recognized value
message must be string | null

single response:
  active            -> operation must be exactly one valid row
  not_found          -> operation must be null
  backend_unavailable -> operation must be null
  query_failed       -> operation may be null or one valid row

history response:
  requested_limit must be a finite integer
  effective_limit must be a finite integer in [1, 100]
  rows must satisfy Array.isArray
  every row in rows must pass isAutonomousDailyOperationApiRow
```

Any violation returns the existing `endpoint_unavailable` sentinel
(`ENDPOINT_UNAVAILABLE_DAILY_OPERATION(S)`) unchanged from §4 — no new
transport/truth-state vocabulary was introduced. `active` + a
missing/invalid `operation` can now never reach the `truth_state: "active"`
branch at all; it fails closed before that value is ever assigned. Neither
mapper contains a `?? []`/`?? null` fallback on `operation` or `rows` for a
response that is otherwise structurally accepted — the fallback sentinel
objects (`ENDPOINT_UNAVAILABLE_DAILY_OPERATION(S)`) are the only path to an
empty/null result, and that path is always `transport_state:
"endpoint_unavailable"`, never `"available"`.

### 17.4 Defensive screen branch (defense in depth)

`CurrentOperationPanel` in the screen component now branches
`truth_state === "not_found"` on its own, separately from a new fallback
branch: `truth_state !== "active" || operation == null` renders a distinct
"Malformed daily-operation response" notice. Since the mapper (§17.3)
already guarantees `active` always carries a valid non-null operation, this
branch is unreachable through the normal fetch path — it exists as a second,
independent line of defense so the screen itself never trusts the mapper's
invariant silently, and so a directly-constructed malformed
`AutonomousDailyOperationSurface` (e.g. in a future caller or test) cannot
render the neutral not-found copy either.

### 17.5 History blocker rendering

`HistoryPanel`'s row template now renders each row's `evidence_state`
followed by every `evidence_blockers` entry stacked beneath it, always
visible with no expansion control, in daemon-provided order, with no
sorting or deduplication and no raw error text — only the same bounded
closed reason codes already rendered by the current-operation panel. An
empty blocker list renders no additional lines.

### 17.6 Guard strengthening

The F1 guard's check `[7]` previously passed as soon as
`row.evidence_blockers` appeared anywhere in the screen source — satisfied
by the current-operation panel alone. A new `[R1]`–`[R12]` section isolates
`HistoryPanel`'s own function body and requires `row.evidence_blockers`
inside it specifically, and proves (by source content, not just presence of
a truth-state string) that: the row validator and its three closed
vocabulary sets exist; the active/not_found/query_failed operation
invariants and the `Array.isArray(rows)` + per-row history invariant are
present in `legacy.ts`; neither mapper contains the old `rows ?? []`
fallback; the screen's `not_found` branch is no longer combined with a
null-operation check; the malformed-response notice text exists; and the
new malformed-active-null, malformed-history-row, and history-blocker-
rendering tests exist. Every existing F1 scope/no-mutation/no-daemon/
no-migration/Phase-E-range/status check is unchanged.

### 17.7 Tests

`api.test.ts` gained 9 new cases; `screenSource.test.ts` gained 1 new case;
a new file `__tests__/isAutonomousDailyOperationApiRow.test.ts` (8 cases)
was added and registered in `package.json`. Full list in the ledger entry
`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F1-RUNTIME-SHAPE-AND-HISTORY-BLOCKER-REPAIR-01`.

### 17.8 Scope discipline

No daemon route, response field, classifier, finalizer, coordinator,
notification, coverage, or lineage change is made by this repair — same as
§1's boundary for F1 itself. No production Rust file and no migration is
touched. F2, F3, and Phase G remain untouched and unstarted.
