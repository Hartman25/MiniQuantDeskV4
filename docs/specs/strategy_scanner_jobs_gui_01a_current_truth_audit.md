# Strategy Scanner Jobs/GUI 01A — Current Truth Audit & Design

Patch group: `STRATEGY-SCANNER-DAEMON-JOBS-AND-GUI-REVIEW-01-COMBINED`
Patch: `STRATEGY-SCANNER-JOBS-GUI-01A-CURRENT-TRUTH-AUDIT-AND-DESIGN-01`

## 1. Current HEAD

`928b9fbe` (`docs: close strategy lab scanner foundation`). Branch `main`,
no dirty tracked files, no staged files at audit time. Only allowed
untracked paths present (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`,
`smoke_logs/`).

## 2. Current scanner foundation status

`STRATEGY-LAB-COMPLETION-AND-SCANNER-FOUNDATION-01-COMBINED` is
`CLOSED_LOCAL` (see
`docs/specs/strategy_lab_scanner_01e_closure_decision.md`). Confirmed
present in the current tree:

- `core-rs/crates/mqk-backtest/src/strategy_scanner.rs` — pure
  `evaluate_scan_candidate` / `rank_scan_candidates`, `StrategyScanPolicy`,
  `StrategyScanCandidate`, `StrategyScanMetrics`, `StrategyScanTruthState`,
  `StrategyScanReasonCode`. No file/network/DB IO.
- `mqk backtest scan-strategies` CLI
  (`core-rs/crates/mqk-cli/src/commands/bkt.rs::run_strategy_scan`, wired
  in `main.rs`), writing `manifest.json` / `candidates.json` /
  `candidates.csv` / `summary.json` under
  `exports/strategy_scans/{scan_id}/`, `scan_id` a deterministic UUIDv5
  (`derive_scan_id`, never `Uuid::new_v4()`).
- Real local-data proof exists:
  `docs/specs/strategy_lab_scanner_01d_real_local_data_proof.md` — 1D/
  `swing_momentum` 88/88 ranked; 5m/`intraday_scalper --limit-symbols 10`
  10/10 honest `data_missing`.
- Caveat carried forward unchanged: every top-ranked 1D candidate in the
  Phase D proof run had a **negative** absolute return
  (`total_return_pct`); `score` is `alpha_pct` (strategy return minus
  buy-and-hold benchmark), which is **research evidence only**, not a
  profitability or promotion claim.

No daemon route or GUI screen currently reads scanner output — CLI-only
today.

## 3. Current daemon job conventions

Two existing in-memory job registries were inspected:

- `core-rs/crates/mqk-daemon/src/backtest_jobs.rs` +
  `core-rs/crates/mqk-daemon/src/routes/backtests.rs`
  (`BACKTEST-DAEMON-JOBS-01`): `HashMap<Uuid, BacktestJobRecord>` behind
  `Arc<Mutex<..>>` (`BacktestJobStore`), **process-lifetime only, no DB
  persistence**. `POST /api/v1/backtests/jobs` (operator-auth) validates
  the request, inserts a `Queued` record, then `tokio::spawn`s a
  background task that flips the record to `Running`, runs the backtest
  (via `tokio::task::spawn_blocking` for the CSV path), and flips it to
  `Completed`/`Failed` with `artifact_dir`/`manifest_path`/`metrics_path`
  or `error`. `GET /api/v1/backtests/jobs` (public) lists all jobs
  (newest-first). `GET /api/v1/backtests/jobs/:job_id` (public) returns
  full status or 404.
- `core-rs/crates/mqk-daemon/src/ingest_jobs.rs` +
  `core-rs/crates/mqk-daemon/src/routes/ingest.rs`
  (`DATA-INGEST-DAEMON-JOBS-01`): similar in-memory store, but this one
  **also has an optional DB-persistence path** (`sys_ingest_jobs` table,
  `persist_ingest_job_record`/`list_persisted_ingest_jobs`) — used only
  when a DB pool is configured; the in-memory store remains authoritative
  otherwise.

Selected precedent for this bundle: the **backtest-jobs** pattern
(in-memory only, no DB table) — it is the closer analog (CPU-bound local
computation over local data, not a market-data provider integration with
its own persistence needs) and matches the mission's explicit preference
("in-memory jobs are acceptable only if current backtest jobs are also
in-memory ... no DB migration unless Phase A proves it's required").

## 4. Current GUI review conventions

- `core-rs/mqk-gui/src/features/backtests/BacktestResultsScreen.tsx` +
  `api.ts` + `types.ts` + `parsers.ts`: reviews **CLI-produced** backtest
  artifacts by reading local files (Tauri file access via
  `pathHelpers.ts`/`desktop/bootstrap.ts`), not by submitting jobs through
  the daemon. This screen does not submit jobs.
- `core-rs/mqk-gui/src/features/ingest/IngestScreen.tsx` +
  `api.ts` + `types.ts`: the closer analog for this bundle — submits a
  job via `POST /api/v1/ingest/jobs` (operator token), then polls
  `GET /api/v1/ingest/jobs/:job_id` every 2s until terminal, with an
  explicit status badge, rows/skip-reason display, and safety-notice
  banner at the top of the screen ("this writes market data ... does not
  submit broker orders"). This submit+poll+status pattern is the template
  for Phase D.
- `core-rs/mqk-gui/src/features/screens/screenRegistry.tsx`: central
  `SCREEN_REGISTRY` map plus `MONITOR_GROUPS`/`ROLE_SCREENS` placement.
  `backtests` is already registered under `monitorGroup: "diagnostics"`.
  A new `strategyScanner` screen key follows the same pattern.

## 5. Current artifact conventions

The scanner already writes the artifact set named in the mission
(`manifest.json`, `candidates.json`, `candidates.csv`, `summary.json`)
under `exports/strategy_scans/{scan_id}/` — this bundle does not change
that schema. There is **no existing generic daemon artifact-file-read
route** for arbitrary local directories; the closest precedent is
`system_artifact_intake`/`system_run_artifact`
(`core-rs/crates/mqk-daemon/src/routes/system_artifact.rs`), which read a
single fixed, env-configured file path, not an operator-supplied
directory. Phase C's artifact-readback route is new but narrowly scoped
(reads only the four known filenames inside a validated scan artifact
directory — no arbitrary file access).

## 6. First proven operator gap

No daemon job route and no GUI panel exist for the scanner — an operator
today must open a terminal, run `mqk-cli`, and manually read the artifact
JSON files. This bundle closes that specific gap only.

## 7. Selected daemon route/job design

New module `core-rs/crates/mqk-daemon/src/strategy_scan_jobs.rs`
(job store, modeled on `backtest_jobs.rs`) plus
`core-rs/crates/mqk-daemon/src/routes/strategy_scans.rs` (handlers,
modeled on `routes/backtests.rs`):

```text
POST /api/v1/strategy-scans/jobs          (operator auth)
GET  /api/v1/strategy-scans/jobs          (public)
GET  /api/v1/strategy-scans/jobs/:job_id  (public)
GET  /api/v1/strategy-scans/artifact      (public, ?artifact_dir=...)
```

The scan itself is CPU-bound and local-data-only (no network, no DB) —
same shape as the existing CSV backtest job — so it runs via
`tokio::task::spawn_blocking` inside a `tokio::spawn`'d background task,
identical to `backtest_job_submit`'s CSV path. The scanner's own
artifact-writing logic in `mqk-cli/src/commands/bkt.rs::run_strategy_scan`
is refactored so the artifact-writing half (manifest/candidates/csv/
summary serialization) becomes a small shared function in
`mqk-backtest::strategy_scanner` that both the CLI and the daemon call —
the daemon must not shell out to `mqk-cli`.

## 8. Selected GUI design

Reuse the existing `backtests` diagnostics screen area's visual
conventions (`Panel`, `bt-job-status-badge`, `bt-job-form-grid` CSS
classes already in `styles.css`) but add a **new** screen
(`strategyScanner`) rather than folding into `BacktestResultsScreen.tsx`,
because that screen is purely a local-artifact viewer with no job-submit
form today, and the strategy scanner's request shape (registry path,
bars root, timeframe, strategy, top, limit-symbols, out-dir) is distinct
enough from a single-symbol CSV backtest job that reusing the existing
form would require conditional branching throughout. New screen key
`strategyScanner`, registered in `screenRegistry.tsx` under
`monitorGroup: "diagnostics"` (same group as `backtests`), and added to
`MONITOR_GROUPS.diagnostics`.

## 9. Jobs in-memory or durable?

**In-memory only** — `Arc<Mutex<HashMap<Uuid, StrategyScanJobRecord>>>`,
process-lifetime, exactly like `BacktestJobStore`. No DB table. The GUI
must label job history as daemon-lifetime only (a daemon restart loses
the job list; the artifact files on disk are unaffected and remain
readable via the Phase C route by directory path).

## 10. Is a DB migration needed?

**No.** The scanner itself has no DB dependency today (proven in the
01D/01E closure docs) and the job registry follows the in-memory
`backtest_jobs` precedent, not the DB-backed `ingest_jobs` precedent.

## 11. Exact API contract

`POST /api/v1/strategy-scans/jobs` (operator auth) — request:

```json
{
  "registry_path": "config/instruments/equities.json",
  "bars_root": "exports/md_backup",
  "timeframe": "1D",
  "strategy": "swing_momentum",
  "top": 20,
  "limit_symbols": 20,
  "out_dir": "exports/strategy_scans"
}
```

`registry_path`/`bars_root`/`out_dir` default as specified in the
mission when omitted. `timeframe`/`strategy` required non-blank. `top`
bounded `1..=100` (default 20 if omitted). `limit_symbols` bounded
`1..=200` when supplied. Response (`202 Accepted` on success, `400` on
refusal):

```json
{
  "accepted": true,
  "job_id": "<uuid>",
  "status": "queued",
  "artifact_dir": null,
  "error": null
}
```

`GET /api/v1/strategy-scans/jobs` (public) — list, newest-first, same
summary shape as `BacktestJobSummary` (job_id, status, request echo,
timestamps, artifact_dir, ranked/skipped counts once completed, error).

`GET /api/v1/strategy-scans/jobs/:job_id` (public) — full
`StrategyScanJobResponse` per the mission's schema (`truth_state`,
`status`, `submitted_at_utc`, `completed_at_utc`, `request`,
`artifact_dir`, `summary`, `blockers`, `warnings`).

`GET /api/v1/strategy-scans/artifact?artifact_dir=<path>` (public) —
reads `manifest.json`/`summary.json`/`candidates.json` from the given
directory. `truth_state` one of `active` / `missing_artifact` /
`invalid_artifact` / `path_rejected` / `read_failed`. Path must resolve
(after canonicalization) inside the configured scan artifact root
(default `exports/strategy_scans`) — any directory outside that root is
`path_rejected`, never read. Candidate rows capped at 200 in the
response.

## 12. Exact GUI behavior

`StrategyScannerScreen.tsx`: a submit form (registry path, bars root,
timeframe, strategy, top, limit symbols, out dir — all pre-filled with
the same defaults as the API), a job-status panel (poll every 2s while
non-terminal, same pattern as `IngestScreen`), and — once a job
completes — an artifact-review panel that calls the Phase C route with
the job's `artifact_dir` and renders: ranked/skipped counts, a top-
candidates table (rank, symbol, strategy, timeframe, score,
total_return_pct, alpha_pct, max_drawdown_pct, trade_count, truth_state,
reason_code), a skip-reason summary, and the three required warnings
(§14) rendered prominently and unconditionally whenever any result is
shown. No trade/promote/approve control anywhere on the screen.

## 13. Exact tests

Daemon (Phase B, `scenario_strategy_scanner_jobs_01.rs`): submit valid
fixture scan → completed; all four artifact files written; summary has
ranked/skipped counts; missing bars file → `data_missing` not failure;
empty/limited universe → honest skip; job list returns the job; job
status by id works; invalid request (blank timeframe/strategy, `top`/
`limit_symbols` out of bounds) rejected with `400`; zero
`oms_outbox`/`oms_inbox` writes; no DB env required; no provider/broker
env required.

Daemon (Phase C, same test file): valid artifact → `active`; missing
artifact dir → `missing_artifact`; malformed JSON → `invalid_artifact`;
path-escape attempt (`../../etc`, absolute path outside root) →
`path_rejected`; candidate list capped at 200; top candidates in rank
order; skip reasons summarized; warnings present; no DB/provider/broker
env required.

GUI (Phase D): API client submit builds expected request body; API
client parses job list/status/artifact responses including every
`truth_state`; screen renders the three required warnings; screen
renders a completed job's summary and top-candidates table; screen
renders missing/invalid/path-rejected artifact states honestly; screen
has no trade/promote/approve button (asserted negatively); screen is
reachable via `screenRegistry` (registry test asserts the key exists and
is not hidden).

## 14. Required warning text

Every GUI surface that displays scan results must show, verbatim or with
equivalent unambiguous meaning:

- "Scanner ranking is research evidence only."
- "Scanner output is not autonomous trading approval."
- "Candidates can rank well while still having negative absolute
  returns."

no autonomous trading approval. no provider calls. no broker calls. no
live orders. no forced paper orders. no strategy threshold changes. no
DB migration. This bundle submits no live orders and no paper orders at
any phase.

## 15. Non-goals

- No order submission of any kind (live or paper).
- No provider/broker/network call anywhere in the new daemon routes.
- No change to any existing strategy's logic, sizing default, or signal
  threshold.
- No admission/promotion gate consumes scanner output.
- No GUI trading control (submit order, arm, promote, approve) is added
  anywhere on the new screen.
- No change to risk/session/OMS/integrity gates on any existing path.

## Reference: patch group and required terms

`STRATEGY-SCANNER-DAEMON-JOBS-AND-GUI-REVIEW-01` — daemon job routes
(`POST`/`GET /api/v1/strategy-scans/jobs`, `GET
/api/v1/strategy-scans/jobs/:job_id`) reuse the existing
`scan-strategies` CLI's scanner core and write the same `manifest.json`
/ `candidates.json` / `summary.json` artifact set under
`strategy_scans` directories. Every GUI/API surface displaying results
carries the fixed warning set: "research evidence only", "no autonomous
trading approval", "no provider calls", "no broker calls", "no live
orders", "no forced paper orders", "no strategy threshold changes", "no
DB migration".
