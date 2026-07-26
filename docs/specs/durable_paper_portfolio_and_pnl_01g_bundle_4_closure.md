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

One repair commit: `fix: harden durable paper portfolio closure`.
