# DURABLE-PAPER-PORTFOLIO-AND-PNL-01G — Integrated Closure and Bundle 4 Audit

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01G-INTEGRATED-CLOSURE`
Final phase of `DURABLE-PAPER-PORTFOLIO-AND-PNL-01-COMBINED`. Closes the
durable, restart-surviving portfolio and P&L truth gap for paper+Alpaca,
single-symbol long-only US equity/ETF, supervised.

## Integrated proof

`core-rs/crates/mqk-daemon/tests/scenario_durable_paper_portfolio_and_pnl_01.rs`,
4 tests, all passing stable across multiple runs against the isolated
port-5434 test DB, using real production seams
(`AppState::accept_external_broker_snapshot_for_test`, the real
`GET /api/v1/execution/paper-lifecycle` route via `mqk_daemon::routes::build_router`)
and real `oms_outbox`/`oms_inbox` fixture rows — never a shortcut that
bypasses the actual durable data shape:

- **Proof A+B+C (snapshot durability, fill accounting, restart)** —
  `proof_a_b_c_snapshot_fill_accounting_and_restart`: a buy + partial sell
  persists a durable snapshot and durable FIFO accounting
  (`realized_pnl_micros == 40_000_000`, `accounting_epoch == "complete"`);
  a fresh `AppState` + fresh connection pool (simulating a restart) replays
  the identical durable `oms_inbox` history to byte-identical
  `cash_micros`/`realized_pnl_micros`/`last_applied_inbox_id`.
- **Proof D (incomplete history, no fabrication)** —
  `proof_d_incomplete_history_blocks_epoch_without_fabrication`: a broker
  position with zero fill history in this run's `oms_inbox` remains visible
  via the durable snapshot (`qty_signed == 30`), while
  `accounting_epoch == "incomplete"` with an explicit reason,
  `realized_pnl_micros == 0`, and the run's `oms_inbox` row count is
  asserted exactly `0` — proving no synthetic opening fill was ever
  inserted to close the gap.
- **Proof E (P&L truth, independently positive/negative/zero)** —
  `proof_e_realized_pnl_positive_negative_and_zero`: three independent
  runs prove `realized_pnl_micros` is exactly `+100_000_000` (sell above
  entry), exactly `-100_000_000` (sell below entry), and exactly `0` with
  `accounting_epoch == "complete"` (a flat portfolio with zero fills — known
  zero, not absent truth).
- **Proof F (full lifecycle + restart via the paper-lifecycle route)** —
  `proof_f_full_chain_and_restart_reconstructable_via_paper_lifecycle`:
  order → ack → fill → durable accounting → `GET
  /api/v1/execution/paper-lifecycle` reports `portfolio_truth_state:
  "active"`, `pnl_truth_state: "active"`, and
  `overall_lifecycle_state: "order_filled_portfolio_durable_pnl_available"`;
  a second call through a brand-new `AppState` + brand-new pool (restart)
  reaches the identical result, proving the full chain is reconstructable
  from durable state alone.

**Proofs G and H are not duplicated here** — they were proven, with real
evidence, in earlier phases and are not re-implemented as new tests:
- **Proof G (API read-only)**:
  `scenario_durable_paper_portfolio_read_only_api_01.rs`'s
  `repeated_gets_across_all_durable_routes_never_mutate_any_row` calls
  every new durable route plus paper-lifecycle three times each and asserts
  zero row-count delta across
  snapshots/positions/accounting-state/outbox/inbox/runs (B4-E).
- **Proof H (GUI contract)**: `scripts/guards/validate_durable_paper_portfolio_and_pnl_01f_operator_integration.ps1`
  statically proves the GUI requests the canonical routes and contains no
  mutation-control keyword; this session's live browser verification (B4-F
  spec) proved the durable section renders `null` as "Unavailable" (never
  silently zero) and renders independently of the in-memory panel's
  hard-close notice, with zero console errors and the exact three expected
  network requests observed firing.

**Proof I (existing-surface regressions)** — proven by the regression
matrix below: every pre-existing scenario file this bundle's changes could
plausibly affect passes unchanged.

## Required closure questions

**Is portfolio truth restart-surviving?** Yes — `sys_paper_portfolio_snapshots`/
`sys_paper_portfolio_snapshot_positions` (B4-B/B4-C) and
`sys_paper_portfolio_accounting_state` (B4-B/B4-D) are Postgres tables, read
back identically across an `AppState`/pool replacement (Proof C, and
B4-C's own `restart_reads_back_durable_snapshot`).

**Are fills applied exactly once?** Yes — the durable accounting replay
reuses `recover_oms_and_portfolio` directly (not a second, simpler loop),
which already applies the exact duplicate-fill guard the live apply path
uses (per-order OMS state machine keyed on economic event identity); B4-D's
`duplicate_refresh_has_zero_delta` and
`partial_then_final_fill_applies_exact_total_once` prove this at the
accounting-projection level.

**Is realized P&L complete only with complete fill authority?** Yes —
`accounting_epoch` is `"incomplete"` whenever any nonzero broker-reported
position's FIFO-replayed quantity doesn't exactly match the durably-known
fill history (total absence or partial mismatch, both proven in B4-D), and
`realized_pnl` is `null` on the API surface (B4-E) whenever
`accounting_truth_state != "active"` — proven end-to-end in this phase's
Proof D.

**Is unrealized P&L mark-proven?** Yes, and unmodified — B4-E's
`durable-summary` route reuses the existing `compute_broker_positions_pnl`
mark-lookup helper (`md_bars`-sourced) verbatim; this bundle adds no new
mark source and no new mark-truth-state vocabulary.

**Is daily P&L still baseline-proven?** Yes, and unmodified —
`resolve_daily_pnl` (the existing daily-equity-baseline reader) is reused
verbatim by the new `durable-summary` route; B4-D's
`daily_pnl_baseline_table_untouched` proves zero row-count delta on
`sys_account_equity_baseline` across a full accounting refresh.

**Are pre-existing positions handled fail-closed?** Yes — Proof D: a
pre-existing/adopted position is visible (broker-sourced truth, never
hidden) while its realized P&L is explicitly unavailable; no synthetic
opening fill is ever fabricated to force a match, proven both by direct
assertion and by an `oms_inbox` row-count-zero check.

**Are API routes read-only?** Yes — B4-E's dedicated read-only proof
(zero row-count delta across every durable table for repeated GETs); B4-E's
guard statically confirms no write-helper call exists in the route file.

**Does GUI preserve unavailable truth?** Yes — `formatDurableMoney`/
`formatDurableCount` render `null` as the literal word "Unavailable", never
silently as `0` or blank; verified live in this session's browser check.

**Does evidence capture include durable portfolio truth?** Yes — B4-F
extended `capture_autonomous_paper_session_evidence.ps1` /
`validate_autonomous_paper_session_evidence.ps1` to fetch, record, and
validate the three new routes' truth-state fields, with five new negative
tests proving the validator actually rejects a missing truth-state or an
invalid `accounting_epoch`.

**Did any paper/live order execute during patching?** No. Every test in
every phase of this bundle constructs `oms_outbox`/`oms_inbox` fixture rows
directly via DB helpers — no `BrokerGateway`/order-submission code path is
exercised anywhere in Bundle 4's own test suites.

**Did any external network call occur?** No. Every DB-backed test requires
and uses only `MQK_DATABASE_URL` pointed at the isolated port-5434 test
database; the GUI's live browser check ran against the local dev server
with the daemon intentionally *not* running (all requests to
`127.0.0.1:8899` failed with `ERR_CONNECTION_REFUSED`, proving no other
network target was reachable or attempted).

**Is live capital still prohibited?** Yes — no file in this bundle touches
`DeploymentMode::LiveShadow`/`LiveCapital` handling, live routing gates, or
risk/sizing logic. `persist_external_broker_snapshot_best_effort` and
`refresh_paper_portfolio_accounting_state_best_effort` both explicitly gate
on `DeploymentMode::Paper` + `BrokerKind::Alpaca` and are no-ops otherwise.

## Regression matrix (all named binaries run individually, all green)

| Binary | Result |
|---|---|
| `mqk-db --test scenario_paper_portfolio_store_01` | 14/14 |
| `mqk-daemon --test scenario_durable_paper_portfolio_and_pnl_01` | 4/4 |
| `mqk-daemon --test scenario_paper_daily_pnl_baseline_01` | 11/11 |
| `mqk-daemon --test scenario_paper_daily_pnl_baseline_capture_01` | 22/22 |
| `mqk-daemon --test scenario_paper_order_lifecycle_visibility_01` | 13/13 |
| `mqk-daemon --test scenario_durable_paper_portfolio_read_only_api_01` | 11/11 |
| `mqk-daemon --test scenario_paper_pnl_operator_visibility_01` | 13/13 |
| `mqk-daemon --test scenario_reconcile_tick_disarms_on_drift` | 5/5 |
| `mqk-daemon --test scenario_monotonic_reconcile_in_run_baseline_01` | 7/7 |
| `mqk-daemon --test scenario_autonomous_daily_phase_e_closure_01` | 0/0 (6 pre-existing `#[ignore]`d, unrelated) |
| `mqk-daemon --test scenario_autonomous_daily_operation_api_01` | 20/20 (30 pre-existing `#[ignore]`d, unrelated) |
| `mqk-daemon --test scenario_gui_daemon_contract_gate` | 23/23 |
| `mqk-daemon --test scenario_daemon_routes` | 73/73 (11 pre-existing `#[ignore]`d, unrelated) |
| `mqk-daemon --test scenario_autonomous_completed_bar_driver_01` | 56/56 |
| `mqk-daemon --lib` (full unit suite) | 342/342 |
| `core-rs/mqk-gui`: `npm run build` | clean (tsc + vite) |
| `core-rs/mqk-gui`: `npm test` | 850/850 |
| `scripts/soak/tests/test_autonomous_paper_session_evidence.ps1` | 40/40 |

`cargo check -p mqk-db -p mqk-portfolio -p mqk-runtime -p mqk-daemon`: clean.
`cargo clippy` (scoped to every changed crate/test binary, `-D warnings`):
clean throughout every phase. `cargo fmt --check` on every touched file:
no diff throughout every phase (pre-existing, unrelated formatting drift
elsewhere on this box was observed and never staged — see each phase's own
commit message). `git diff --check` / `git diff --cached --check`: clean.

## Known limitations

- `fetch_latest_paper_portfolio_snapshot`/`fetch_recent_paper_portfolio_snapshots`
  (B4-B) are global across all runs sharing `(deployment_mode, source)`, not
  run-scoped — a documented property (B4-A §2, B4-E spec), correct for the
  one-run-at-a-time supervised lane this bundle targets, not a general
  multi-run-concurrent design.
- Open FIFO lots are not stored durably; they are recomputed on read via
  `recompute_from_ledger` replaying applied `oms_inbox` rows up to the
  stored watermark (B4-A §3) — acceptable for this bundle's supervised,
  single-symbol scope; a future bundle should revisit if replay cost ever
  becomes material.
- The terminal-fill-expiry-refresher's durable-persist half runs via
  `tokio::spawn` (fire-and-forget) rather than being awaited inline, since
  that call site is a sync closure (B4-C) — best-effort by design, matching
  this bundle's fail-soft persistence contract, but means a persistence
  failure on that specific path is only ever visible in logs, not in the
  return value of the call that triggered it.
- Two pre-existing, unrelated test-hygiene issues were found and fixed
  during this bundle (both test-only, never production code): B4-0's
  completed-bar-driver fixture wall-clock drift and cross-test DB race, and
  B4-E's discovery that B4-D's new accounting-refresh side effect required
  a cleanup-helper fix in B4-C's own test file. Both are documented in their
  respective phase commits/specs.

## Explicit non-claims

Bundle 5 is not started. Multi-symbol autonomous trading is not enabled.
The unattended 10–20-session soak has not started and is not claimed
started anywhere in this bundle's commits or documentation. Live capital
is not ready and no file in this bundle touches live-mode gating.

## FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR addendum

Patch ID: `DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR`

This is the consolidated correctness/closure repair the Bundle 4 final
closure review required before ChatGPT/operator acceptance. Six confirmed
defects (cross-run contamination, same-watermark accounting staleness,
unconfirmed-snapshot accounting, one-directional completeness, leaked/
collapsed API errors, and a live Bundle-3-guard canary that could never
pass once Bundle 4 legitimately existed) are closed across `mqk-db`,
`mqk-daemon`, the GUI, and the guard scripts — see the 01B/01C/01D/01E/01F
addenda above for the per-phase detail.

**Bundle 3 final guard reconciliation (Defect 6).** Check `[10]` in
`validate_autonomous_daily_paper_operations_01g_bundle_3_final_closure.ps1`
was a live canary scanning `$PhaseEAcceptedHead..HEAD` (ever-widening) for
any Bundle-4-named path — it necessarily failed the moment Bundle 4
legitimately started, which made it impossible for this very guard (a
prerequisite of the Bundle 4 closure guard) to ever pass again. It is now a
fixed historical proof over `$PhaseEAcceptedHead..$FinalRepairCommit` (the
same immutable range checks `[8]`/`[9]` already established, never widened
to `..HEAD`): Bundle 4 did not exist inside Bundle 3's own committed range,
which is a permanent fact independent of how far HEAD advances afterward.
Verified: this range (`4b6eec72..e3eb2fe2`) contains zero Bundle-4-named
paths; the live canary's growing range (`4b6eec72..HEAD` at the time of this
repair) contained two.

**Bundle 4 closure guard strengthening.** Added checks `[12]`–`[22]` to
`validate_durable_paper_portfolio_and_pnl_01g_bundle_4_closure.ps1`,
independently proving: the run-scoped snapshot helper exists and is the
only one `durable_portfolio.rs`/`paper_lifecycle.rs` call (never the global
non-run-scoped one); the echoed `run_id` is tied to the same resolved run
used for the lookups; accounting carries `source_snapshot_id`; the
accounting refresh is gated on a `Confirmed` persistence outcome;
same-watermark upserts compare full content (`Conflict`/`UpdatedForSnapshot`
exist and are tested); completeness compares both symbol directions and
cannot skip an unparseable quantity; no public route formats a raw error
onto the wire; explicit-run query failure is distinguished from not_found;
and the GUI runtime validators exist.

Verification (this repair): every focused Bundle 4 test binary re-run
against the port-5434 test DB (`mqk-db` paper-portfolio-store: 15/15;
`mqk-daemon` durable snapshot-persistence: 9/9, accounting: 13/13,
and-pnl: 4/4, read-only-api: 17/17, paper-order-lifecycle-visibility:
13/13) plus the adjacent regression set (autonomous completed-bar-driver:
56/56, autonomous daily-operation API: 49/50 — the one failure,
`b26_history_response_database_unavailable_counts_is_query_failed`, is a
pre-existing shared-test-DB row-accumulation flake unrelated to this
patch: it fails identically with this patch's changes fully reverted, in
an area (`autonomous_daily_operations`) this repair does not touch — and
autonomous daily-phase-E closure: 6/6, GUI/daemon contract gate: 23/23,
full daemon routes: 84/84, daily-P&L baseline: 11/11, daily-P&L baseline
capture: 22/22). `cargo check`/`clippy -D warnings`/`rustfmt --check` clean
on every file this repair touched. GUI: `npm test` 866/866, `npm run build`
clean.

## DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-COHERENCE-AND-ACCEPTANCE-PROOF

Final Bundle 4 coherence and closure repair, on top of the
FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR above. Reconciles Bundle
3's now-accepted status and closes the remaining source-proven Bundle 4
accounting/API/GUI coherence gaps found by a fresh false-truth review of the
changed dependency cone.

**Accounting coherence (mqk-db).** `upsert_paper_portfolio_accounting_state`
now validates `source_snapshot_id` transactionally, inside the same
transaction as the write, via a new `fetch_snapshot_authority_tx` helper
and a new `SnapshotAuthorityRow`/`candidate_snapshot_is_authoritative`
deterministic-ordering pair: a null, missing, cross-run, null-run,
non-paper, non-`external_alpaca`, or non-USD source snapshot is rejected as
`InvalidSourceSnapshot` before any write. Snapshot authority ordering
`(captured_at_utc, snapshot_id)` — newer wins, ties broken by greater
`snapshot_id` — is now enforced on every transition: a same-watermark
different-snapshot update requires the candidate to be strictly newer, and
a higher-watermark update can never regress recorded provenance; both
violations reject as `RejectedStaleSnapshot`. A same-snapshot epoch/reason
drift with unchanged fill-derived values is now `Conflict` (previously
silently accepted as `UpdatedForSnapshot`) — an identical snapshot replayed
against identical fills must always reproduce the identical epoch.
`persist_external_broker_snapshot_best_effort` (mqk-daemon) now requires
non-null run ownership, USD currency, nonblank position symbols, and a
strictly positive average entry price on every nonzero position.
`normalize_broker_positions` no longer silently skips a blank broker
position symbol (`broker_position_symbol_blank`).

**API/lifecycle coherence (mqk-daemon).** New module
`routes/portfolio_provenance.rs` exports one pure, shared classifier
(`classify_portfolio_provenance` / `PortfolioProvenanceState`: `active` |
`fill_history_incomplete` | `accounting_epoch_unavailable` |
`accounting_snapshot_mismatch` | `not_found` | `query_failed` |
`unsupported_source`), used identically by `durable_portfolio.rs` and
`paper_lifecycle.rs`. Closes the defect where `durable_portfolio.rs`'s
`accounting_fields` never compared an accounting row's `source_snapshot_id`
against the currently-selected snapshot at all — a stale accounting row
pointing at a superseded snapshot could be reported `"active"` beside a
newer snapshot, and `paper_lifecycle.rs`'s `classify_overall_lifecycle_state`
(which inspected the raw accounting row directly) could still report
`order_filled_portfolio_durable_pnl_available` even though provenance no
longer matched. `paper_lifecycle.rs`'s invalid-`run_id` handling now uses
the identical fixed, bounded `INVALID_RUN_ID_MESSAGE` as
`durable_portfolio.rs` instead of formatting `"run_id is not a valid UUID: {s}"`
(which echoed the raw supplied value).

**GUI runtime contract (mqk-gui).** `durablePortfolio.ts` closes every
nested truth-state vocabulary: adds `accounting_snapshot_mismatch`; closes
`unrealized_pnl_truth_state`/`daily_pnl_truth_state`, which were previously
unchecked strings; adds a closed vocabulary for each snapshot-history row's
own `truth_state`. Numeric fields now reject non-finite (NaN/Infinity/
-Infinity) values and non-integral/unsafe integers (`last_applied_inbox_id`,
`qty_signed`) instead of coercing them. State invariants: an
`"active"`/`"snapshot_stale"` snapshot response must carry real identity
fields (`source == external_alpaca`, `deployment_mode == paper`,
`currency == USD`, non-null `run_id`/`snapshot_id`, finite equity/cash); an
`"active"` accounting/P&L response must carry `accounting_epoch ==
"complete"`, a non-null `accounting_source_snapshot_id` equal to
`snapshot_id`, and a finite `realized_pnl`. `enforceRunScopeConsistency` now
also compares `snapshot_id` (previously only `run_id`), so positions from a
different snapshot generation of the same run are rejected too.
`PortfolioScreen`'s accounting-completeness chip now gates on the
authoritative `accounting_truth_state` instead of the raw `accounting_epoch`
string (a stale `accounting_snapshot_mismatch` row's own epoch may still
read `"complete"` from when it was current).

**Bundle 3 status reconciliation.** Bundle 3 (D1 through Phase G) has now
received independent ChatGPT/operator acceptance.
`validate_autonomous_daily_paper_operations_01g_bundle_3_final_closure.ps1`
is updated to match: `"BUNDLE 3: ACCEPTED"` (and, since Bundle 3's
acceptance necessarily includes its own F3/Phase G constituents, `"F3:
ACCEPTED -- COMPLETE"` and `"PHASE G: ACCEPTED -- COMPLETE"`) are retired
from the forbidden-claims list, and check `[15]` now requires `"BUNDLE 3:
ACCEPTED -- COMPLETE"` in the ledger and README instead of forbidding it.
Fixed a real encoding bug found while making this check pass: Windows
PowerShell 5.1's `Get-Content -Raw` (without an explicit `-Encoding`)
silently misdecodes a BOM-less UTF-8 file's em-dash characters under the
system default codepage, which made any `Test-ContentContains` check for a
needle containing an em-dash always fail even when the exact text was
genuinely present — verified directly (default-encoding read reports
`Contains()=false`, `-Encoding UTF8` read reports `Contains()=true` for the
identical file/needle). All three `Get-Content -Raw` calls for
README/README_TECHNICAL/ledger in that guard now specify `-Encoding UTF8`.

**Bundle 4 closure guard strengthening.** Added checks `[23]`–`[36]` to
`validate_durable_paper_portfolio_and_pnl_01g_bundle_4_closure.ps1`,
independently proving as real source code (not just documentation): USD
currency enforcement in both `snapshot.rs` and the mqk-db authority check;
non-null run ownership enforcement for external snapshot acceptance; blank
symbols cannot be skipped (both the snapshot-acceptance refusal and the
accounting-completeness bounded blocker); the source snapshot must resolve
and belong to the accounting run, checked transactionally
(`fetch_snapshot_authority_tx`); a same-snapshot epoch/reason drift rejects
as `Conflict`; `RejectedStaleSnapshot` exists; the deterministic
`(captured_at_utc, snapshot_id)` ordering helper exists and is reused; the
shared provenance classifier exists and is used by both
`durable_portfolio.rs` and `paper_lifecycle.rs` (and `paper_lifecycle.rs` no
longer classifies its lifecycle summary from a raw accounting row);
paper-lifecycle's invalid-`run_id` handling is bounded and never echoes raw
input; every required nested GUI truth state is present in the closed
vocabulary, including non-finite/unsafe-numeric rejection; the GUI
cross-response check compares `snapshot_id`; no migration file changed
since this patch's starting commit; and no broker-adapter/strategy/risk/
execution-core file changed since the same starting commit.

No new database migration. No broker adapter, order-submission, strategy,
or risk-limit change anywhere in this patch.

One repair commit: `fix: harden durable paper portfolio closure`.

## DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-READ-SIDE-AUTHORITY-REPAIR

Final Bundle 4 read-side authority repair, on top of the
FINAL-COHERENCE-AND-ACCEPTANCE-PROOF above.

**Confirmed defect.** The write seam
(`persist_external_broker_snapshot_best_effort` in `state/snapshot.rs`,
`upsert_paper_portfolio_accounting_state` in `mqk-db`) already refuses to
*persist* a new non-USD, runless, blank-symbol, or otherwise invalid
authoritative snapshot. But every durable read seam
(`GET /api/v1/portfolio/durable-summary`, `durable-positions`,
`durable-snapshots`, and `GET /api/v1/execution/paper-lifecycle`) selected
the latest run-scoped row using only `deployment_mode`/`source`/`run_id`,
never independently re-validating its currency, stored `truth_state`, or
position content before reporting it as authoritative. A legacy or
directly-inserted malformed row (wrong currency, non-active `truth_state`,
blank/duplicate symbol, non-positive average entry price on a nonzero
position, or non-`external_alpaca` position provenance) could therefore
still be reported as active durable truth.

**Shared validator (`routes/portfolio_provenance.rs`).** Two new pure
functions, reusing the same module the Phase B shared provenance classifier
already lives in:

- `validate_run_scoped_snapshot_authority(snapshot, resolved_run_id)` — the
  full contract for a selected `PaperPortfolioSnapshotWithPositions`:
  `run_id == resolved_run_id`, `deployment_mode == "paper"`,
  `source == "external_alpaca"`, `currency == "USD"`,
  `truth_state == "active"`, and for every position: nonblank-after-trim
  symbol, per-snapshot symbol uniqueness, `avg_entry_price_micros > 0`
  whenever `qty_signed != 0`, and `provenance == "external_alpaca"`. Used by
  durable-summary, durable-positions, and paper-lifecycle.
- `validate_snapshot_scalar_authority(record)` — the scalar-only subset
  (no position children are fetched for the history surface): non-null
  `run_id` plus the same `deployment_mode`/`source`/`currency`/`truth_state`
  checks. Used by durable-snapshots history.

Both return a bounded, closed-vocabulary `SnapshotAuthorityViolation`
(never raw row content, ids, or DB error text) via `blocker_message()`.
`PortfolioProvenanceState` gained an eighth closed state, `InvalidSnapshot`
(`"invalid_snapshot"`); `classify_portfolio_provenance` gained a
`snapshot_invalid: bool` parameter checked immediately after the
query-failure gate and before the accounting row is even matched, so a
malformed selected snapshot can never be paired with an accounting row,
however well-formed that accounting row is. No fallback to an older
snapshot anywhere: every check operates on the selected latest row only, so
corruption or legacy-incompatible truth fails closed and stays visible
instead of being silently masked by an earlier, valid snapshot.

**Route behavior.**

- **Durable summary** — a new `invalid_snapshot_summary(run_id, violation)`
  builder returns before any equity/cash/unrealized/daily-P&L computation is
  attempted: `truth_state`/`snapshot_truth_state`/`accounting_truth_state`/
  `realized_pnl_truth_state`/`unrealized_pnl_truth_state`/
  `daily_pnl_truth_state` are all `"invalid_snapshot"`; every financial and
  accounting field is `null`; `run_id` (the already-resolved run, independent
  of the malformed row) is preserved; every other snapshot-sourced field
  (identity, account fields) is `null` — never leaked from the malformed row.
- **Durable positions** — same validator; on failure, `truth_state =
  "invalid_snapshot"`, `positions = []`, and snapshot identity is omitted
  from the response.
- **Paper lifecycle** — the same shared validator, computed once and threaded
  into `classify_portfolio_provenance`'s new `snapshot_invalid` parameter:
  `portfolio_truth_state` and `pnl_truth_state` both read `"invalid_snapshot"`.
  `classify_overall_lifecycle_state`'s match on `PortfolioProvenanceState` is
  exhaustive over the new variant (a compile-time guarantee, not a
  convention), so `"order_filled_portfolio_durable_pnl_available"` is
  architecturally unreachable from an invalid snapshot.
- **Snapshot history** — every row `fetch_recent_paper_portfolio_snapshots`
  returns is checked via `validate_snapshot_scalar_authority`; a single
  malformed row fails the *whole* wrapper closed
  (`truth_state = "invalid_snapshot"`, `snapshots = []`) rather than being
  silently dropped while the rest render as proven-active history.

All four routes remain GET-only; zero writes on every fail-closed path
(proven by a dedicated zero-row-count-delta test, matching the existing B4-E
read-only proof's convention).

**GUI (`durablePortfolio.ts`).** `"invalid_snapshot"` added to every
applicable closed vocabulary (summary/snapshot truth_state, accounting/
realized/unrealized/daily-P&L truth_state, positions truth_state,
snapshot-history wrapper truth_state). New state invariants: a summary
claiming `snapshot_truth_state === "invalid_snapshot"` must carry `null`
`account_equity`/`cash` (a response smuggling non-null financial fields
alongside `invalid_snapshot` fails closed); positions/snapshots wrappers
claiming `invalid_snapshot` must carry an empty array. `isValidSnapshotRow`
(the per-row history validator) is strengthened from generic string/nullable
checks to exact-value checks — `deployment_mode === "paper"`,
`source === "external_alpaca"`, `currency === "USD"`, non-null `run_id` — so
a row with the "right shape" but the wrong deployment_mode/source/currency
or a null `run_id` fails closed instead of rendering as proven truth,
matching the daemon's own per-row scalar authority contract. Paper-lifecycle
has no GUI dependency (`grep` of `core-rs/mqk-gui/src` confirms no caller of
`/api/v1/execution/paper-lifecycle`), so no paper-lifecycle-specific GUI
change was needed or made.

**Tests.** 12 new DB-backed scenario tests added to the existing
`scenario_durable_paper_portfolio_read_only_api_01.rs` (directly
constructing malformed rows via `insert_or_confirm_paper_portfolio_snapshot`
itself, which — like the read seam being repaired — validates only
`source`, never currency/truth_state/position content, proving these rows
are reachable exactly as a legacy/pre-repair row would be): non-USD
currency, non-active `truth_state`, a `run_id = NULL` history row, blank
position symbol (summary + positions), non-positive average price on a
nonzero position, non-`external_alpaca` position provenance, a malformed
snapshot paired with a fully well-formed matching accounting row (proving
the write-side snapshot-authority check on the accounting seam validates
`run_id`/`deployment_mode`/`source`/`currency` but not `truth_state` or
position content, so this exact combination is reachable through the
existing write path and must be caught read-side), the same scenario through
paper-lifecycle, durable-positions returning zero rows, the history wrapper
failing whole closed on one malformed row, a fully valid snapshot remaining
unaffected (`"active"`), and zero writes across every invalid-snapshot GET.
Plus 20 new pure unit tests (`portfolio_provenance.rs` validator/classifier
tests, `paper_lifecycle.rs` lifecycle-classification test) and 11 new GUI
tests (`durablePortfolio.test.ts`).

**Guard.** `validate_durable_paper_portfolio_and_pnl_01g_bundle_4_closure.ps1`
gained checks `[37]`–`[44]`: the shared validator functions exist; exact
USD/active/run-owned scalar authority is enforced as real source (not just
documented); blank-symbol/duplicate-symbol/invalid-price/invalid-provenance
position refusal is real source; `InvalidSnapshot` is a real closed-vocabulary
state (not just documentation) and `classify_portfolio_provenance` threads
the new flag; every affected route calls the shared validator; the GUI
closed-state and history scalar-authority strengthening exist; all 12
required scenario tests exist by name; and the no-migration/no-broker/
no-strategy/no-risk-limit invariants (reusing the existing fixed-range
checks `[35]`/`[36]`, which already bound every commit after this repair's
starting commit) hold.

No new database migration. No broker adapter change. No order submission,
cancellation, replacement, or routing change. No strategy change. No
risk-limit change. No provider or external network call. No real daemon
start. No Paper or Live order. No Bundle 5 work.

One repair commit: `fix: fail closed on malformed durable portfolio
snapshots`.

```text
BUNDLE 3: ACCEPTED — COMPLETE

BUNDLE 4:
FINAL READ-SIDE AUTHORITY REPAIR AND CLOSURE PROOF COMPLETE —
AWAITING FINAL CHATGPT AND OPERATOR ACCEPTANCE

BUNDLE 5: NOT STARTED
UNATTENDED 10–20-SESSION SOAK: NOT STARTED
LIVE CAPITAL: NOT READY
```

Bundle 4 is **not** marked accepted or closed in the repository by this
patch.
