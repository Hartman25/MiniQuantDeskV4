# Strategy Lab Scanner 01A — Current Truth Audit & Design

Patch group: `STRATEGY-LAB-COMPLETION-AND-SCANNER-FOUNDATION-01-COMBINED`
Patch: `STRATEGY-LAB-SCANNER-01A-CURRENT-TRUTH-AUDIT-AND-DESIGN-01`

## 1. Current HEAD

`fdd5b1fe` (`docs: close persistent paper lifecycle visibility`), branch
`main`. Pre-flight confirmed no dirty tracked files, no staged files, and
only the expected untracked files (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`,
`smoke_logs/`).

## 2. Existing research / backtest / scanner tools (current repo evidence)

- `core-rs/crates/mqk-backtest/src/engine.rs` — `BacktestEngine`: pure,
  deterministic, event-sourced single-symbol backtest engine
  (`BAR -> STRATEGY -> EXECUTION -> PORTFOLIO -> RISK`). Consumes an
  in-memory `Vec<BacktestBar>` and a `Box<dyn mqk_strategy::Strategy>`; no
  file IO, no network IO inside the engine itself.
- `core-rs/crates/mqk-backtest/src/loader.rs` — `load_csv_file` /
  `parse_csv_bars`: deterministic CSV bar loader. This is the exact format
  used by `exports/md_backup/**/*.csv` (see §5).
- `core-rs/crates/mqk-backtest/src/sweep.rs` — `run_sweep`,
  `sweep_row_from_report`, `rank_sweep_results`: an existing deterministic
  parameter-sweep engine for a **single symbol**. `sweep_row_from_report`
  already extracts `total_return_pct`, `max_drawdown_pct`,
  `buy_and_hold_return_pct`, `alpha_pct`, `trade_count`, `win_rate_pct`,
  `profit_factor`, `fill_count`, `halted` from a `BacktestReport` via
  tested FIFO round-trip P&L logic. This is directly reusable by the
  scanner instead of re-deriving trade metrics.
- `core-rs/crates/mqk-backtest/src/strategy_lab.rs` +
  `core-rs/crates/mqk-artifacts/src/lib.rs` (`evaluate_strategy_lab*`,
  `rank_strategy_lab_evaluations`, `evaluate_strategy_lab_artifact_dir`,
  `rank_strategy_lab_artifact_tree`) — **Strategy Lab**: evaluates and
  ranks **already-completed backtest artifact folders** (reads
  `metrics.json` from an existing `exports/<run>/` directory tree). It
  does **not** run bars through the engine itself and does **not**
  resolve a symbol universe from the instrument registry. This is a
  distinct, narrower tool than what this patch needs — a scanner that
  starts from raw local bars + the registry and runs the engine itself.
  Strategy Lab and the new scanner are complementary, not duplicative:
  the scanner could theoretically also rank existing artifact folders
  later, but that is out of scope here.
- `core-rs/crates/mqk-backtest/src/regime.rs` — `detect_market_regime`:
  pure market-regime classifier over a bar sequence (`bull_trend`,
  `sideways`, etc.). Research-only, already CLI-wired
  (`mqk backtest regime-detect`). Not used by the scanner in this patch;
  noted as a candidate scanner input dimension for a future patch
  (matches ledger §14 `REGIME-DETECTION-01 — OPEN / ROADMAP`).
- `core-rs/crates/mqk-cli/src/commands/bkt.rs` +
  `core-rs/crates/mqk-cli/src/main.rs` — CLI command group `mqk backtest
  <subcmd>` (`Csv`, `CsvSweep`, `Db`, `StrategyLabEvaluate`,
  `StrategyLabRank`, `RegimeDetect`). **There is no `mqk research`
  namespace in the current repo** — all backtest/research tooling lives
  under the existing `Backtest { cmd: BacktestCmd }` top-level subcommand
  (see `core-rs/crates/mqk-cli/src/main.rs:44-79`). The scanner CLI
  command in this patch is added as `mqk backtest scan-strategies` to
  match this existing convention rather than inventing a new top-level
  namespace, per this bundle's own "or use existing naming conventions if
  different" allowance.
- Ledger §14 (`Alpha Scanner / Intraday Data / Strategy Roadmap`) already
  lists `MULTI-SYMBOL-SCANNER-01 — OPEN / ROADMAP` and
  `REGIME-DETECTION-01 — OPEN / ROADMAP` as unimplemented roadmap rows.
  Nothing under those headings has been built yet in the current repo
  (grep for `scanner`/`regime` across `core-rs` confirms only the
  research-only regime detector and the Python watchlist-promotion
  scanner in `research-py/src/mqk_research/scanner/` exist today — the
  latter is a promotion-pipeline gate, not a symbol/strategy ranking
  scanner, and is out of scope here).

## 3. Existing local data coverage by timeframe

`exports/md_backup/`:

- `1D/` — **88 files**, one per registry symbol (`{SYMBOL}_1D.csv`),
  matching the 88 enabled equities in `config/instruments/equities.json`
  exactly. `AAPL_1D.csv` has 8,375 bars (`symbol,timeframe,end_ts,
  open_micros,high_micros,low_micros,close_micros,volume,is_complete,
  ingested_at` header — a superset of the `mqk-backtest` loader's
  required/optional columns, so it parses without modification).
- `5m/` — **directory exists, 0 files**. Confirmed empty via directory
  listing. Any scan requesting `--timeframe 5m` must honestly report
  every candidate as `data_missing`, not silently skip or crash.
- `daily/` — a second 88-file `{SYMBOL}_1D.csv` tree, byte-identical in
  naming convention to `1D/`. Not used by this patch (the mission's
  `--bars-root` convention points at `exports/md_backup` with a
  `{timeframe}/` subfolder, which matches `1D/`, not the legacy `daily/`
  alias). Noted for completeness only; no other timeframe subfolder
  exists.

## 4. Existing strategies that can be evaluated off-market

`core-rs/crates/mqk-strategy/src/engines/mod.rs` registers four builtin
strategy identities via `register_builtin_strategies_with_sizing`, each
with a fixed required timeframe (`StrategyMeta.timeframe_secs`):

| strategy_id                | timeframe_secs | timeframe label | local data available |
|-----------------------------|-----------------|------------------|------------------------|
| `swing_momentum`            | 86,400 (1D)     | `1D`             | yes (88/88 symbols)   |
| `mean_reversion`             | 3,600 (1H)      | `1H`             | no                     |
| `volatility_breakout`        | 3,600 (1H)      | `1H`             | no                     |
| `intraday_scalper`           | 300 (5m)        | `5m`             | no (empty dir)         |
| `intraday_short_scalper`     | 300 (5m)        | `5m`             | no (empty dir)         |

Only `swing_momentum` has a local bar file for every registry symbol
today. This is the proven positive path for Phase D's real-data proof;
`intraday_scalper` against the empty `5m/` directory is the proven
honest-skip path.

## 5. Existing artifact schema conventions

`mqk-artifacts` (`init_run_artifacts`, `write_backtest_report`) writes a
`manifest.json` + typed report files per completed backtest run, with
`schema_version`, `run_id`, `git_hash`, `config_hash`, and a
`host_fingerprint` — the established pattern for "durable, reviewable
artifact directory" in this repo. The scanner artifact (Phase C) follows
the same shape (`manifest.json` + typed candidate files) but is a
**new, independent artifact kind** — it is not a per-run backtest
artifact tree and is not read by `rank_strategy_lab_artifact_tree`
(different schema, different purpose: ranking candidates across a
universe, not evaluating one completed run).

## 6. Existing CLI / daemon / GUI surfaces

CLI-first is already the established pattern for every backtest/research
tool in this repo (§2). No daemon job-scheduling surface currently runs
backtests (`mqk-daemon`'s existing jobs are ingest/reconcile/heartbeat
jobs, not backtest jobs). No GUI screen reads backtest or Strategy Lab
output today. This confirms the mission's preferred shape: **CLI-first,
no daemon route, no GUI** for this bundle.

## 7. First proven gap

There is no existing path from "local bars + instrument registry" to
"ranked candidate list." `run_sweep` and `BacktestEngine` can run a
single symbol's bars through a single strategy, and `strategy_lab`/
`mqk-artifacts` can rank *already-completed* artifact folders — but
nothing resolves a registry universe, iterates it against local bar
files per timeframe, and produces a fresh ranked-candidate artifact
honestly reporting missing/insufficient data. That is the gap this
bundle closes.

## 8. Selected implementation design

1. **Phase B — pure scanner core** (`mqk-backtest::strategy_scanner`):
   given an already-resolved `bars: Option<&[BacktestBar]>` (the caller
   already did the file read), an already-instantiated
   `strategy: Option<Box<dyn Strategy>>`, and the strategy's registered
   `timeframe_secs` (from `PluginRegistry::list()`, already in-memory —
   no new IO), decide the `truth_state`/`reason_code`, and — only when
   all preconditions pass — run the (already-loaded) bars through
   `BacktestEngine` (CPU-only, deterministic, no IO) and reduce the
   `BacktestReport` via the existing `sweep_row_from_report` into
   `StrategyScanMetrics`. Score = `alpha_pct` if a benchmark is
   available, else `total_return_pct` (mirrors `rank_sweep_results`'s own
   documented fallback). A separate `rank_scan_candidates` function
   performs the deterministic stable sort.
2. **Phase C — CLI runner** (`mqk backtest scan-strategies`): the only
   layer that touches the filesystem. Loads the registry via the
   existing `mqk_md::instrument_registry::{load_instrument_registry,
   enabled_equity_symbols}` pure loader, resolves
   `{bars_root}/{timeframe}/{symbol}_{timeframe}.csv` per symbol (matching
   the existing `exports/md_backup/1D/{SYMBOL}_1D.csv` convention
   observed in §3), loads bars via the existing `load_csv_file`,
   constructs a fresh per-symbol `PluginRegistry` via the existing
   `register_builtin_strategies_with_sizing`, calls into the Phase B pure
   core per `(symbol, strategy_id)` pair, ranks, and writes the artifact
   directory (`manifest.json`, `candidates.json`, `candidates.csv`,
   `summary.json`).

## 9. Why the selected design is safe

- The pure core (Phase B) never opens a file, a socket, or a DB
  connection — it only receives already-loaded in-memory data and
  already-constructed strategy instances from its caller. This makes the
  determinism and no-IO tests in Phase B enforceable by construction, not
  by convention.
- `BacktestEngine` is the same deterministic, replay-only engine already
  used by every other backtest CLI command in this repo (`mqk backtest
  csv`, `csv-sweep`, `db`) — it does not import or call any broker
  adapter, does not write to `oms_outbox`/`oms_inbox`, and cannot submit
  a live or paper order (`mqk_execution::targets_to_order_intents`
  produces in-memory `BacktestOrder`/`BacktestFill` records consumed only
  by the report, never an outbox row).
- The CLI runner (Phase C) only *reads* `config/instruments/equities.json`
  and `exports/md_backup/**/*.csv`, and only *writes* to a new,
  operator-specified `--out-dir` (default `exports/strategy_scans`) —
  never to a DB table, never to a config file, never to `.env.local`.

## 10. Why it does not require provider/broker calls

Every input (registry JSON, bar CSVs) is already a local file in the
repo. `mqk_md::instrument_registry::load_instrument_registry` is a pure
`serde_json` parse of a local file — the exact same function already used
read-only by `mqk-daemon`'s coverage/status routes. No `mqk-md` provider
client, no `mqk-broker-alpaca` type, and no network crate is imported by
either the Phase B scanner core or the Phase C CLI command.

## 11. Whether DB migration is needed

**No.** The scanner is a stateless, on-demand CLI computation over local
files. Its only durable output is a file artifact directory, matching
this bundle's stated preference ("Prefer file artifact output first").
No `oms_outbox`, `oms_inbox`, or execution-lifecycle table is read or
written.

## 12. Exact scanner artifact schema

Directory: `{out_dir}/{scan_id}/` where `scan_id` is a UUIDv5 derived
deterministically from `(registry_path, bars_root, timeframe,
strategies, universe_symbols)` — same determinism posture as
`BacktestConfig::config_id()` (UUIDv5, not `Uuid::new_v4()`), so re-running
the identical scan inputs produces the identical `scan_id`.

- `manifest.json`: `{schema_version, scan_id, created_at_utc, git_hash,
  registry_path, bars_root, timeframe, strategies: [..],
  universe_count, ranked_count, skipped_count, blockers: [..],
  warnings: [..]}`.
- `candidates.json`: `Vec<StrategyScanCandidate>` (Phase B struct,
  `serde`-derived) — `symbol, timeframe, strategy_id, bars_available,
  truth_state, reason_code, score, rank, metrics, warnings, blockers`.
- `candidates.csv`: same rows, flattened, stable header order.
- `summary.json`: top-N ranked candidates + top skip-reason histogram
  (operator-convenience view over `candidates.json`; derived, not a new
  source of truth).

## 13. Exact tests

See Phase B / Phase C sections of the master prompt for the full list.
Summary: pure ranking/truth-state tests in
`core-rs/crates/mqk-backtest/tests/scenario_strategy_lab_scanner_01.rs`
(no IO); CLI artifact/fixture tests in
`core-rs/crates/mqk-cli/tests/scenario_strategy_lab_scanner_cli_01.rs`
(tiny local fixture directories only, no repo-root `exports/` dependency,
no DB env, no provider/broker env).

## 14. Non-goals

- No live or paper order submission of any kind.
- No broker or provider call of any kind (Alpaca, TwelveData, Kraken, or
  otherwise).
- No strategy threshold or sizing change to any existing strategy engine.
- No GUI screen in this bundle.
- No multi-asset production execution change.
- No DB migration, no new DB table, no write to `oms_outbox`/`oms_inbox`/
  any execution-lifecycle table.
- No change to risk, session, integrity, reconcile, broker, or OMS gate
  behavior for any existing (live/paper) code path — the scanner's own
  internal `BacktestEngine` config disables the integrity gate for its
  own runs only (see rationale below), which is a property of this new,
  isolated research tool, not a change to any existing gate.

### Note on integrity-gate configuration inside the scanner

`BacktestConfig::conservative_defaults()` (the default used by every
existing single-timeframe backtest CLI command) hardcodes
`integrity_stale_threshold_ticks: 120`, which is correct for
intraday/minute-scale bars but would spuriously flag *every* daily bar
gap as stale (daily bars are 86,400 s apart) — the existing `--target-qty`
CLI help text for `mqk backtest csv` already documents this exact
caveat for daily data. Because the scanner runs multiple timeframes in
one invocation, no single hardcoded threshold is correct for all of
them. The scanner's own `StrategyScanPolicy` therefore disables the
integrity gate (`integrity_enabled: false`) for its own internal
`BacktestEngine` runs, and this is documented in code. This affects only
the scanner's own research computation and does not touch, weaken, or
bypass the integrity gate used by any live, paper, or existing single-
timeframe backtest path.

## Answers to the pre-flight questions

1. Existing tools: deterministic backtest engine, CSV loader, single-symbol
   parameter sweep, Strategy Lab artifact evaluator/ranker, market-regime
   classifier — all CLI-wired under `mqk backtest`. See §2.
2. Strategy Lab evaluator: yes, but it evaluates completed artifact
   folders, not raw bars — see §2.
3. Artifact evaluator/ranker: yes (`mqk-artifacts`), same caveat as #2.
4. Scanner concept: no scanner exists yet in Rust; ledger §14 lists it as
   `MULTI-SYMBOL-SCANNER-01 — OPEN / ROADMAP`. A Python watchlist-promotion
   scanner exists in `research-py` but is a different concern (admission
   gating, not ranking).
5. Local bar data: `1D` (88/88 symbols), `5m` (empty), `daily` (legacy
   alias of `1D`, unused by this patch). See §3.
6. 1D data for full registry: yes, 88/88.
7. 5m data: directory exists, zero files.
8. Runnable off-market strategy: `swing_momentum` (1D). The other three
   registered strategies require timeframes with no local data today.
9. Strategy engine reusable by scanner: yes — same `Strategy` trait,
   `PluginRegistry`, and `BacktestEngine` already used by every backtest
   CLI command.
10. Artifact location: `exports/strategy_scans/{scan_id}/` (operator
    `--out-dir`, matching the mission's suggested default), untracked
    (matches existing `exports/` convention — not staged).
11. CLI is the safest first operator surface — see §6.
12. DB persistence: not needed — see §11.
13. Tests proving no order/provider/broker path was touched: pure unit
    tests assert the scanner module does not import
    `mqk-broker-alpaca`/daemon runtime/OMS DB writer types (source grep
    test), and CLI fixture tests assert zero DB/provider/broker
    environment variables are required and zero rows are written to any
    outbox/inbox table (no DB connection opened at all by the CLI
    command).

local data only. no provider calls. no broker calls. no live orders. no forced paper orders. no strategy threshold changes. scanner artifact. ranked candidates. truth_state. All satisfied above.
