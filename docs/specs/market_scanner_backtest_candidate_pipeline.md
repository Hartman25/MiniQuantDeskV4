# Market Scanner → Backtest → Candidate Pipeline
## Spec ID: MARKET-SCANNER-BACKTEST-CANDIDATE-PIPELINE-SPEC-01

---

## 1. Mission

Build a continuous, artifact-driven pipeline that:

1. Scans a controlled liquid-symbol universe on a schedule.
2. Filters candidates through data-quality, liquidity, volatility, risk, and regime gates.
3. Writes candidate journals and rejection artifacts.
4. Queues off-hours backtests for scanner candidates against the existing backtest engine.
5. Determines which strategy best fits each symbol via strategy-fit reports.
6. Produces strategy/symbol recommendations and a next-day autonomous paper watchlist.
7. Allows autonomous paper trading to ingest only approved symbol/strategy pairs.
8. Preserves the live-trading lock until repeated clean paper evidence exists.

The pipeline is **artifact-only**: it writes files and records. It does not submit orders, call broker endpoints, or mutate the production DB during scanner or backtest phases.

---

## 2. Non-Goals

- This pipeline does not submit orders of any kind.
- It does not call broker REST or WS endpoints.
- It does not mutate the production DB from the scanner or backtest stage.
- It does not change strategy logic, thresholds, or signal generation in the runtime engine.
- It does not enable live routing.
- It does not produce final live eligibility: `eligible_for_live` stays `false` until explicit promotion through repeated paper evidence.
- Multi-symbol concurrent live trading is future work. First version supports at most one selected symbol at a time in paper mode.
- It is not a backtesting framework replacement — it integrates with the existing `mqk-backtest` parity engine.

---

## 3. Safety Boundaries

These are hard rules, not configuration options:

| Boundary | Rule |
|---|---|
| Scanner stage | Never imports broker adapters, OMS, or execution orchestrator |
| Backtest stage | Never submits orders; reads bar history only |
| Watchlist stage | Never bypasses risk/reconcile/kill-switch/WS gates |
| Artifact writes | Scanner, backtest, and watchlist stages write artifacts only |
| Live eligibility | Always `false` until explicit promotion through repeated paper evidence |
| Autonomous paper | Consumes approved candidates but still requires live strategy signal from the running engine |
| First-version constraint | At most one selected symbol at a time in paper mode |
| Strategy recommendations | Advisory until promoted through repeated paper evidence |
| Backtest recommendations | Do not directly trade; only inform watchlist promotion |
| Secrets | No secrets, keys, or broker credentials anywhere in scanner/backtest/watchlist artifacts |

---

## 4. Full Pipeline Stages

```
[1] Universe Load
      ↓
[2] Data Quality Filter
      ↓
[3] Liquidity / Execution Cost Filter
      ↓
[4] Intraday Movement / Volatility Scan
      ↓
[5] Regime Classification
      ↓
[6] Candidate Journal Write
      ↓
[7] End-of-Day Ranked Candidate Export
      ↓ (off-hours)
[8] Backtest / Replay Queue
      ↓
[9] Strategy-Fit Report
      ↓
[10] Watchlist Promotion Gate
      ↓ (premarket)
[11] Premarket Revalidation
      ↓ (market open)
[12] Market-Open Autonomous Paper Ingestion
      ↓ (intraday)
[13] Intraday Observe-Only Re-Ranking
      ↓ (post-session)
[14] Post-Session Evidence Review
```

Each stage writes artifacts. No stage submits orders. Each stage may reject candidates and write rejection records.

---

## 5. Universe Model

### 5.1 Controlled Universe

- Universe is a pre-approved, operator-curated symbol list. Default: large-cap liquid US equities.
- Universe file: `config/scanner/universe_v1.csv` (symbol, exchange, asset_class, enabled).
- Universe version is recorded in every scan artifact.
- Survivorship and inclusion methodology must be declared per the backtest policy spec.

### 5.2 Universe Size Limits

- First version: maximum 50 symbols.
- Expansion requires explicit operator configuration.
- No dynamic universe expansion (e.g., no automatic penny screener ingestion into the main pipeline in v1).

### 5.3 Symbol Requirements

- Exchange: NYSE or NASDAQ only (v1).
- Asset class: `us_equity` only (v1).
- Must appear on the operator-curated list.
- Must not appear on a suppression list (`config/scanner/suppressed_symbols.txt`).

---

## 6. Data-Quality Gate

A symbol fails the data-quality gate if any of the following apply:

| Check | Criterion |
|---|---|
| Bar freshness | Latest bar timestamp > `max_bar_age_minutes` (default: 20 min during market hours) |
| Bar count | Fewer than `min_bars_required` (default: 30 completed 5m bars) in the lookback window |
| Missing bars | Any gap > 2 consecutive bars in the core lookback window |
| Duplicate bars | Any bar with a repeated timestamp |
| Zero-volume bars | More than `max_zero_volume_fraction` (default: 5%) of bars have zero volume |
| Zero-price bars | Any bar with open/high/low/close = 0 |
| Inverted OHLC | Any bar where high < low or close outside [low, high] |
| Stale reference data | Symbol metadata (exchange, asset class) absent or older than `max_metadata_age_days` (default: 7) |

Rejection reason must be written to the candidate journal with the specific check that failed.

`data_quality_score` is a float in [0.0, 1.0]:
- 1.0 = all checks pass cleanly
- < 1.0 = partial issues that did not hard-reject but reduce confidence
- 0.0 = hard rejection (gate failed)

### Implementation (SCANNER-DQ-01)

- Module: `research-py/src/mqk_research/scanner/data_quality.py`
- Entry point: `evaluate_data_quality(symbol, timeframe, bars, config, now_utc)` → `DataQualityResult`
- Rejection helper: `build_data_quality_rejection_candidate(...)` → `scanner-candidate-v1` record
- Stable rejection reason constants: `REASON_NO_BARS`, `REASON_INSUFFICIENT_COMPLETED_BARS`,
  `REASON_LATEST_BAR_STALE`, `REASON_DUPLICATE_BAR_TIMESTAMP`, `REASON_TOO_MANY_ZERO_VOLUME_BARS`,
  `REASON_ZERO_PRICE_BAR`, `REASON_INVERTED_OHLC`, `REASON_CLOSE_OUTSIDE_OHLC_RANGE`,
  `REASON_MISSING_BAR_GAP`, `REASON_LATEST_BAR_INCOMPLETE`
- Rejection records use `scanner-candidate-v1` schema with all eligibility flags False
- EXP penny scanner (`exp-candidate-v1`) is separate and unaffected

---

## 7. Liquidity / Execution-Cost Gate

A symbol fails if:

| Check | Criterion |
|---|---|
| ADV (USD) | avg_daily_dollar_volume_20d < `min_adv_usd` (default: 10,000,000) |
| Relative volume | relative_volume < `min_rvol` (default: 0.5 intraday; 1.0 for scan consideration) |
| Spread estimate | spread_estimate_bps > `max_spread_bps` (default: 20 bps) |
| Round-trip cost | round_trip_cost_bps > `max_round_trip_cost_bps` (default: 40 bps) |
| Slippage estimate | slippage_estimate_bps > `max_slippage_bps` (default: 15 bps) |
| Price floor | latest_close < `min_price` (default: 5.00 USD) |

Spread, slippage, and round-trip cost estimates are computed from bar data; they are conservative estimates, not live quote data.

`liquidity_score` is a float in [0.0, 1.0]:
- Derived from distance-to-threshold across ADV, RVOL, spread, and cost gates.
- Only symbols with `liquidity_score >= min_liquidity_score` (default: 0.6) proceed to regime classification.

### Implementation note (SCANNER-LIQUIDITY-01)

- MAIN module: `research-py/src/mqk_research/scanner/liquidity.py`
- Stable rejection reason names: `price_below_min`, `adv_usd_below_min`, `relative_volume_below_min`,
  `spread_too_wide`, `round_trip_cost_too_high`, `slippage_too_high`, `liquidity_score_below_min`
- Liquidity rejection candidates use `scanner-candidate-v1` schema via `build_liquidity_rejection_candidate`
- EXP penny scanner remains separate on `exp-candidate-v1`; this module does not affect EXP

---

## 8. Volatility / Regime Classification

### 8.1 Volatility Measures

Computed from the 5m bar lookback (default: 78 bars = ~1 trading day):

| Measure | Description |
|---|---|
| `atr` | Average True Range over lookback |
| `atr_pct` | ATR / latest_close × 100 |
| `gap_pct` | (open − prior_close) / prior_close × 100 |
| `move_bps` | (latest_close − lookback_close) / lookback_close × 10000 |
| `abs_move_bps` | abs(move_bps) |
| `volatility_score` | Composite of ATR rank, abs_move_bps rank within universe (higher = more volatile) |

### 8.2 Momentum / Trend Scores

| Score | Description |
|---|---|
| `trend_score` | Direction and strength of price trend over lookback |
| `momentum_score` | Rate-of-change over short and medium lookbacks |
| `mean_reversion_score` | Distance from VWAP / MA; reversion potential |

### 8.3 Regime Classification

`regime_label` is one of:

| Label | Meaning |
|---|---|
| `trending_up` | Clear uptrend with strong momentum |
| `trending_down` | Clear downtrend |
| `range_bound` | Low directional movement; mean-reversion candidates |
| `high_volatility` | ATR spike; breakout or fade candidates |
| `low_volatility` | Compressed; potential breakout setup |
| `gapping` | Large overnight or intraday gap |
| `unclassified` | Insufficient data or ambiguous signals |

`regime_score` is a float in [0.0, 1.0] measuring classification confidence.

Only symbols with `regime_label != unclassified` and `regime_score >= min_regime_score` (default: 0.5) proceed to strategy selection.

### Implementation note (SCANNER-REGIME-01)

- MAIN module: `research-py/src/mqk_research/scanner/regime.py`
- Entry point: `evaluate_regime(symbol, bars, config)` → `RegimeResult`
- Rejection helper: `build_regime_rejection_candidate(...)` → `scanner-candidate-v1` record
- Helper: `apply_regime_to_candidate_fields(result)` → dict of regime-derived candidate fields (no writes; for SCANNER-SCORE-01)
- Stable rejection reason constants: `regime_unclassified`, `regime_score_below_min`
- Regime labels: `trending_up`, `trending_down`, `range_bound`, `high_volatility`, `low_volatility`, `gapping`, `unclassified`
- Classification priority: gapping → high_volatility → low_volatility → trending_up → trending_down → range_bound → unclassified
- Rejection records use `scanner-candidate-v1` schema with all eligibility flags False; `eligible_for_live` is always False
- EXP penny scanner (`exp-candidate-v1`) is separate and unaffected

---

## 9. Candidate Schema

Schema version: `scanner-candidate-v1`

Written as JSONL under `exports/candidates/YYYYMMDD/`.

```json
{
  "schema_version": "scanner-candidate-v1",
  "generated_at_utc": "<ISO8601>",
  "scanner_id": "<string>",
  "symbol": "<string>",
  "asset_class": "us_equity",
  "exchange": "<NYSE|NASDAQ>",
  "timeframe": "5m",
  "source": "<universe_file_path>",
  "latest_bar_ts": "<ISO8601>",
  "latest_close": "<float>",
  "lookback_close": "<float>",
  "move_bps": "<float>",
  "abs_move_bps": "<float>",
  "volume": "<int>",
  "avg_volume": "<float>",
  "relative_volume": "<float>",
  "atr": "<float>",
  "gap_pct": "<float>",
  "spread_estimate_bps": "<float>",
  "slippage_estimate_bps": "<float>",
  "round_trip_cost_bps": "<float>",
  "data_quality_score": "<float [0,1]>",
  "liquidity_score": "<float [0,1]>",
  "trend_score": "<float [0,1]>",
  "momentum_score": "<float [0,1]>",
  "mean_reversion_score": "<float [0,1]>",
  "volatility_score": "<float [0,1]>",
  "regime_label": "<string>",
  "regime_score": "<float [0,1]>",
  "risk_score": "<float [0,1]>",
  "total_score": "<float>",
  "reason_tags": ["<string>", ...],
  "risk_tags": ["<string>", ...],
  "rejection_reason": "<string|null>",
  "eligible_for_scan": "<bool>",
  "eligible_for_backtest": "<bool>",
  "eligible_for_paper": "<bool>",
  "eligible_for_live": false,
  "recommended_strategy": "<string|null>",
  "notes": "<string>"
}
```

Hard invariant: `eligible_for_live` is always `false` in scanner output. It is never set to `true` at the scanner stage.

---

## 10. Candidate Scoring

Total score is computed only for symbols that pass all hard gates.

```
risk_score = f(atr_pct, abs_move_bps, gap_pct, spread_estimate_bps)
             — higher risk = lower score contribution

total_score = (
    w_liquidity  × liquidity_score
  + w_volatility × volatility_score
  + w_regime     × regime_score
  + w_momentum   × momentum_score
  + w_trend      × trend_score
  - w_risk       × (1.0 - risk_score)
  - w_cost       × normalized_round_trip_cost
)
```

Default weights (v1):

| Weight | Default |
|---|---|
| w_liquidity | 0.25 |
| w_volatility | 0.20 |
| w_regime | 0.20 |
| w_momentum | 0.15 |
| w_trend | 0.10 |
| w_risk | 0.05 |
| w_cost | 0.05 |

Scores are advisory. Final selection is operator-reviewed in v1.

### Implementation note (SCANNER-SCORE-01)

- MAIN scoring module: `research-py/src/mqk_research/scanner/scoring.py`
- Public API: `ScoringConfig`, `ScoringInputs`, `ScoringResult`, `score_candidate`, `build_scored_scanner_candidate`
- Accepted candidates (`eligible_for_scan=True`, `eligible_for_backtest=True`) are produced only after DQ + liquidity + regime all pass.
- `eligible_for_live=False` always — enforced by `build_scanner_candidate`; not overrideable by caller.
- `eligible_for_paper=False` until a watchlist/promotion gate is added (future patch).
- `recommended_strategy` is advisory only; default is `"intraday_scalper"`.
- EXP penny scanner (`exp-candidate-v1`) is not affected by this module.

### Implementation note (SCANNER-SELECTOR-01)

- MAIN selector module: `research-py/src/mqk_research/scanner/selector.py`
- Public API: `SelectorConfig`, `RankedCandidate`, `RankedCandidateExport`, `WatchlistArtifact`, `select_ranked_candidates`, `build_ranked_candidate_export`, `build_watchlist_artifact`, `write_ranked_candidate_export`, `write_watchlist_artifact`
- Ranked export schema: `ranked-candidates-v1`; written under `exports/watchlist/YYYYMMDD/`
- Watchlist artifact schema: `watchlist-v1`; written under `exports/watchlist/YYYYMMDD/`
- `approved_for_autonomous_paper=False` always in this patch — requires future promotion gate.
- `approved_for_live=False` always — hard invariant, not overrideable by config or caller.
- `max_symbols_to_trade=1` and `max_concurrent_positions=1` forced in v1; config is ignored.
- Candidates with `eligible_for_live=True` are always excluded.
- Ranking: `total_score` desc → `liquidity_score` desc → `regime_score` desc → `symbol` asc (deterministic tie-break).
- Deduplicates by symbol; keeps highest-ranked record per symbol.
- EXP penny scanner (`exp-candidate-v1`) is not affected; this module is MAIN-only.
- No broker/OMS/execution imports; no network/DB imports; no orders placed.

---

## 11. Off-Hours Backtest Stage

### 11.0 Implementation Note (BACKTEST-BRIDGE-BUNDLE-01)

Backtest bridge: `research-py/src/mqk_research/scanner/backtest_bridge.py`
Backtest runner: `research-py/src/mqk_research/scanner/backtest_runner.py`

**Python bridge to Rust `mqk-backtest` CLI output (CLOSED — BACKTEST-BRIDGE-BUNDLE-01)**

- Bridge module: `backtest_bridge.py` — parses `metrics.json` written by `mqk backtest csv --out-dir <dir>`.
- Runner default: **`mode="dry_run"`** — never executes subprocess, produces `status="blocked_no_backtest_interface"`.
- Runner real mode: **`mode="real"` + `BacktestBridgeConfig`** — invokes local Rust binary, reads `metrics.json`, maps to `strategy-fit-v1`, applies gates.
- Real execution is **local/offline only**. Never contacts broker, OMS, daemon, or production DB.
- Subprocess is imported inside `run_backtest_for_entry()` only — NOT at module level.

**Metric scale convention (BT-WALKFORWARD-VALIDATION-BUNDLE-01):**

Rust `metrics.json` pct fields use a 0–100 percentage scale (not 0–1 fractions):
- `win_rate_pct = 60.0` means 60%; Python divides by 100 to get fraction `0.60`.
- `max_drawdown_pct = 5.0` means 5%; Python multiplies by 100 to get `500 bps`.

The config option `metrics_pct_scale` controls this conversion:
- `"percent_0_100"` (default, Rust-compatible): always divide/multiply by 100. Unambiguous.
- `"auto"` (legacy fixture testing only): heuristic > 1.0 check. Not for production.

**Metric mapping (Rust metrics.json → strategy-fit-v1):**

| Rust field | Python field | Conversion |
|---|---|---|
| `win_rate_pct` | `win_rate` | ÷ 100 (percent_0_100 default). 60.0 → 0.60. |
| `profit_factor` | `profit_factor` | Direct pass-through |
| `sharpe_ratio` | `sharpe` | Direct pass-through |
| `sortino_ratio` | `sortino` | Direct pass-through |
| `max_drawdown_pct` | `max_drawdown_bps` | × 100 (percent_0_100 default). 5.0 → 500 bps. |
| `expectancy_micros` | `expectancy_bps` | micros / (price × qty) × 10 000; `expectancy_basis_missing` if no price |
| `bars` | `bars_used` | Direct pass-through |
| `trade_count` | `trades` | Direct pass-through |
| `exposure_time_pct` | `exposure_time_pct` | Direct pass-through |
| `net_expectancy_after_cost_bps` | computed | (expectancy − commission/trade) / notional × 10 000 |
| `validation_profit_factor` | `validation_profit_factor` | Direct (if present in metrics.json) |
| `validation_trades` | `validation_trades` | Direct (if present in metrics.json) |
| `largest_trade_profit_fraction` | `largest_trade_profit_fraction` | Direct or derived from best_trade_micros / gross_profit_micros |
| `sample_quality` | `sample_quality` | Direct (if present in metrics.json) |
| `parameter_stability_score` | `parameter_stability_score` | Direct (if present in metrics.json) |

**Failure reasons (stable string constants):**

- `metrics_file_missing` — `metrics.json` not found at expected path
- `metrics_schema_invalid` — JSON parse error or wrong `schema_version`
- `backtest_command_failed` — CLI non-zero exit or subprocess error
- `expectancy_basis_missing` — cannot derive notional for expectancy conversion
- `validation_metrics_missing` — appended when `validation_profit_factor` or `validation_trades` absent from metrics.json; conditional, not always-appended
- `bars_file_missing` — bars CSV not found for symbol/timeframe
- `binary_not_found` — compiled mqk binary not at configured path
- `command_build_failed` — missing config (no binary or bars root)

**Gate behavior:**
- Gates are applied after metric mapping via `apply_backtest_gates()`.
- `recommended_for_live=False` always — hard invariant.
- `recommended_for_paper=True` only if ALL required + additional gates pass.
- Out-of-sample gate passes when both `validation_profit_factor` and `validation_trades` are present and meet thresholds. Fails closed when either is absent.
- `sample_quality`, `parameter_stability_score`, and `largest_trade_profit_fraction` gates fail closed when fields are absent.
- Existing `mode="dry_run"` behavior is unchanged.

**Walk-forward execution (WALKFORWARD-SPLIT-01 / WALKFORWARD-RUNNER-01):**
- `walkforward.py` provides a pure Python split planner: `WalkForwardConfig`, `WalkForwardSplit`, `build_date_splits(start_date, end_date, config)`.
- `walkforward_runner.py` is the walk-forward runner (WALKFORWARD-RUNNER-01 — CLOSED). It filters the bars CSV to per-split validation windows using Python (the Rust CLI has no date-window filtering flag) and optionally invokes the Rust CLI on each window.
- `recommended_for_live=False` always, regardless of walk-forward results.

**WALKFORWARD-RUNNER-01 implementation note (CLOSED):**
- Module: `research-py/src/mqk_research/scanner/walkforward_runner.py`
- Runner is offline/local. No broker, daemon, OMS, or DB access of any kind.
- `mode="dry_run"` is the default. Split CSV files are written but no subprocess is invoked.
- `mode="real"` is opt-in. subprocess is deferred inside `_run_single_split()` only.
- Real subprocess is mocked in all tests — never actually executed in CI.
- Aggregation is conservative: `validation_profit_factor` = minimum across splits; `validation_trades` = sum; `parameter_stability_score` = passed/total; `sample_quality` = clamp(trades/min_required, 0, 1).
- Validation split CSVs are written under `exports/backtests/walkforward/<queue_id>/`.
- `map_walkforward_to_validation_metrics()` maps the aggregate to the fields consumed by `evaluate_strategy_fit()` and `apply_backtest_gates()`.
- Scanner-driven paper still requires operator review and paper proof after walk-forward completion.
- No live recommendation is produced at any stage of walk-forward validation.

**BACKTEST-WALKFORWARD-VALIDATION-INTEGRATION-01 implementation note (wiring, opt-in):**
- `BacktestRunnerConfig.enable_walkforward_validation` (default `False`) and `BacktestRunnerConfig.walkforward_config` (default `None`) added to `backtest_runner.py`.
- Disabled by default: real-mode behavior is byte-for-byte identical to before — `validation_profit_factor`/`validation_trades`/`sample_quality`/`parameter_stability_score` remain whatever `metrics.json` provided (`None` today) and `validation_metrics_missing` is appended exactly as before.
- When enabled (and `mode="real"` with metrics already produced), `_run_and_merge_walkforward_validation()` resolves the entry's bars CSV via `resolve_bars_csv_path()` (public wrapper added to `backtest_bridge.py`), runs `run_walkforward_entry()`, and merges the aggregate via the pure adapter `merge_walkforward_validation_into_mapped()` (added to `walkforward_runner.py`).
- The merge only fills fields that are currently `None` (never overrides a real `metrics.json` value), and removes `validation_metrics_missing` only when the merged result carries non-`None` `validation_profit_factor` AND `validation_trades`.
- Subprocess invocation remains independently double-gated: `enable_walkforward_validation=True` alone never authorizes a subprocess — `walkforward_config.mode` must also be `"real"`. The default `WalkForwardRunnerConfig()` is `dry_run`, so enabling the flag with default config plans splits but never executes the CLI.
- Fail-closed paths: an unresolvable bars CSV, a blocked/incomplete walk-forward run, or a missing `validation_profit_factor`/`validation_trades` in the aggregate all append `walkforward_validation_blocked` (new constant in `walkforward_runner.py`) and never fabricate values.

**Remaining open:**
- Rust `metrics.json` does not yet emit `validation_profit_factor`, `validation_trades`, `sample_quality`, or `parameter_stability_score` natively; these can now be supplied by the opt-in walk-forward integration above when explicitly enabled and successful — they are not populated by default.
- `recommended_for_paper=True` requires all gate fields to be present and passing.

Schema written: `strategy-fit-v1`
`recommended_for_live=False` — hard invariant enforced in code.
EXP penny scanner (`exp-candidate-v1`) is separate and not affected by this module.
Artifact paths are deterministic: `_artifact_filename(queue_id)` = SHA-256-prefixed filename.

### 11.0a Implementation Note (BACKTEST-QUEUE-01)

Backtest queue writer: `research-py/src/mqk_research/scanner/backtest_queue.py`

- Schema: `backtest-queue-v1`
- Strategy compatibility matrix is defined in `backtest_queue.py` (`_COMPATIBILITY` dict);
  maps each `strategy_id` to a frozenset of compatible `regime_label` values.
- Queue does **not** run backtests; entries are consumed by BACKTEST-RUNNER-01.
- `recommended_for_live=False` is a hard invariant enforced in code, not configuration.
- EXP penny scanner (`exp-candidate-v1`) is separate and not affected by this module.
- Queue ordering: source_rank ascending → approved_strategy_ids order → symbol ascending.
- Queue limit: `max_queue_entries=25` (configurable via `BacktestQueueConfig`).

### 11.1 Trigger

Triggered after market close, after the ranked candidate export is written. The backtest queue is a file: `exports/backtest_queue/YYYYMMDD_queue.json`. The queue lists candidate artifact paths and strategy IDs to test.

### 11.2 What It Does

1. Reads the ranked candidate export.
2. Refreshes and validates bar history for each candidate symbol.
3. Runs each candidate through the approved strategy list via `mqk-backtest` parity engine.
4. Writes one strategy-fit artifact per (symbol, strategy_id) pair.
5. Does not promote any candidate without meeting minimum evidence criteria.
6. Separates training and validation windows (see §15).
7. Supports walk-forward validation.

### 11.3 What It Does Not Do

- Does not submit orders.
- Does not call broker endpoints.
- Does not write to the production OMS.
- Does not set `eligible_for_live = true`.
- Does not automatically promote a symbol to the paper watchlist (human review required in v1).

### 11.4 Approved Strategy List (v1)

| Strategy ID | Description |
|---|---|
| `intraday_scalper` | Current MAIN intraday 5m scalper |
| `volatility_breakout` | ATR/RVOL breakout on 5m bars |
| `mean_reversion` | VWAP/MA reversion on 5m bars |
| `swing_momentum` | Multi-day momentum on daily bars (future) |
| `vwap_mean_reversion` | VWAP distance fade (future) |
| `opening_range_fade` | Opening range rejection fade (future) |

v1 delivers only `intraday_scalper`. All others are future work with placeholder IDs.

### 11.5 Backtest Engine Integration

The backtest runner calls the existing `mqk-backtest` parity engine (in `core-rs/crates/mqk-backtest/`) with:
- `ambiguity_policy = CONSERVATIVE_WORST_CASE`
- `stress_profile >= slippage_x2`
- Bar data from the same source as the live 5m ingest (`Refresh-IntradayMarketData.ps1` / Alpaca intraday)
- A deterministic config hash recorded in every artifact

The parity engine (not the vectorized research backtester) is the promotion-bound source of truth per the backtest policy spec.

---

## 12. Strategy Compatibility Matrix

Defines which strategy is eligible to test against which regime:

| Strategy | trending_up | trending_down | range_bound | high_volatility | low_volatility | gapping |
|---|---|---|---|---|---|---|
| intraday_scalper | ✓ | ✓ | ✗ | ✓ | ✗ | ✗ |
| volatility_breakout | ✓ | ✓ | ✗ | ✓ | ✓ (setup) | ✓ |
| mean_reversion | ✗ | ✗ | ✓ | ✗ | ✓ | ✗ |
| swing_momentum | ✓ | ✓ | ✗ | ✗ | ✗ | ✗ |
| vwap_mean_reversion | ✗ | ✗ | ✓ | ✗ | ✓ | ✗ |
| opening_range_fade | ✗ | ✗ | ✗ | ✓ | ✗ | ✓ |

A strategy is not queued for backtest if the symbol's regime is incompatible. This reduces wasted backtest compute and avoids false-positive results from testing momentum strategies in mean-reversion regimes.

---

## 13. Strategy-Fit Schema

Schema version: `strategy-fit-v1`

Written as JSON under `exports/strategy_fit/YYYYMMDD/`.

```json
{
  "schema_version": "strategy-fit-v1",
  "generated_at_utc": "<ISO8601>",
  "source_candidate_artifact": "<path>",
  "symbol": "<string>",
  "strategy_id": "<string>",
  "timeframe": "5m",
  "regime_label": "<string>",
  "training_window": { "start": "<ISO8601>", "end": "<ISO8601>" },
  "validation_window": { "start": "<ISO8601>", "end": "<ISO8601>" },
  "bars_used": "<int>",
  "trades": "<int>",
  "win_rate": "<float>",
  "profit_factor": "<float>",
  "expectancy_bps": "<float>",
  "avg_trade_bps": "<float>",
  "max_drawdown_bps": "<float>",
  "sharpe": "<float>",
  "sortino": "<float>",
  "exposure_time_pct": "<float>",
  "turnover": "<float>",
  "round_trip_cost_bps": "<float>",
  "net_expectancy_after_cost_bps": "<float>",
  "sample_quality": "<float [0,1]>",
  "parameter_stability_score": "<float [0,1]>",
  "passed_min_bars": "<bool>",
  "passed_min_trades": "<bool>",
  "passed_max_drawdown": "<bool>",
  "passed_profit_factor": "<bool>",
  "passed_expectancy": "<bool>",
  "passed_cost_adjusted_edge": "<bool>",
  "passed_out_of_sample_check": "<bool>",
  "recommended_for_paper": "<bool>",
  "recommended_for_live": false,
  "failure_reasons": ["<string>", ...],
  "notes": "<string>"
}
```

Hard invariant: `recommended_for_live` is always `false` in strategy-fit artifacts.

---

## 14. Backtest Pass/Fail Gates

A strategy-fit result passes promotion-to-paper consideration only if ALL of:

| Gate | Criterion |
|---|---|
| min_bars | bars_used >= 200 (minimum ~5 trading days at 5m) |
| min_trades | trades >= 30 |
| max_drawdown | max_drawdown_bps <= 800 (8% notional equivalent) |
| profit_factor | profit_factor >= 1.3 |
| expectancy | expectancy_bps > 0 |
| cost_adjusted_edge | net_expectancy_after_cost_bps >= 5 |
| out_of_sample | validation window profit_factor >= 1.1 AND validation trades >= 10 |
| no_single_trade_dependency | no single trade represents > 30% of total profit |

Failure on any gate:
- Sets `recommended_for_paper = false`.
- Appends all failed gate names to `failure_reasons`.
- Still writes the artifact (rejected artifacts are kept as evidence).

### Implementation Note (BACKTEST-GATES-01)

- MAIN gate module: `research-py/src/mqk_research/scanner/backtest_gates.py`
- Public API: `BacktestGateConfig`, `BacktestGateResult`, `evaluate_strategy_fit(artifact, config)`,
  `apply_backtest_gates(artifact, config)`, `write_evaluated_strategy_fit_artifact(artifact, path)`
- Blocked/null-metric artifacts are fail-closed: `status == "blocked_no_backtest_interface"` or any
  null required metric → all gates False, `recommended_for_paper=False`.
- `recommended_for_paper=True` only if all 7 required gates pass AND sample_quality,
  parameter_stability, and no_single_trade_dependency additional checks pass.
- `recommended_for_live=False` always (hard invariant; not overrideable by caller or config).
- Additional gates (sample_quality, parameter_stability, single_trade_dependency) tracked in
  `failure_reasons` only — no new `passed_*` schema fields added (schema-compatible).
- Out-of-sample gate uses `validation_profit_factor` + `validation_trades` if present; fails closed
  if absent (these fields are not in the current blocked-runner artifact).
- Does not execute backtest runs; evaluates existing artifacts only.
- EXP penny scanner (`exp-candidate-v1`) is separate and not affected by this module.

---

## 15. Walk-Forward Validation

### 15.1 Window Split

| Window | Default |
|---|---|
| Training | 70% of available bar history |
| Validation | 30% of available bar history (most recent) |
| Minimum training bars | 140 bars (~3.5 trading days) |
| Minimum validation bars | 60 bars (~1.5 trading days) |

### 15.2 Walk-Forward Folds (extended validation)

If enough bars are available (>= 500 bars, ~12.5 days), run up to 5 rolling folds:
- Each fold: train on 60%, validate on 20%, step forward 20%.
- Require: validation profit_factor >= 1.0 in >= 60% of folds.
- `passed_out_of_sample_check` requires both the fixed-split check and (if folds run) the fold check.

### 15.3 Parameter Stability

`parameter_stability_score` measures sensitivity of profit_factor to ±10% changes in key strategy parameters.
- 1.0 = no sensitivity detected.
- < 0.7 = fragile; blocks `recommended_for_paper`.

---

## 16. Risk Simulation

Before any symbol reaches the watchlist, a lightweight risk simulation is run:

| Check | Criterion |
|---|---|
| Max position notional | sim_max_position_usd <= `paper_notional_limit` (default: 5,000 USD) |
| Max daily loss | sim_max_daily_loss_bps <= 200 bps of deployed capital |
| Max drawdown replay | Run equity curve against 2× slippage scenario; MDD must still pass gate |
| Concentration | Symbol must not represent > 100% of the paper position budget in v1 (trivially met for single-symbol) |
| Regime consistency | Regime label at scan time must match dominant regime during training window |

Risk simulation is not a live risk check. It is a pre-screening sanity check on the strategy-fit artifact before watchlist promotion.

### Implementation Note (RISK-SIM-01 — CLOSED)

- MAIN risk module: `research-py/src/mqk_research/scanner/risk_simulation.py`
- Public API: `RiskSimulationConfig`, `RiskSimulationResult`,
  `evaluate_watchlist_risk(watchlist, strategy_fit_artifacts, config)`,
  `apply_risk_simulation_to_watchlist(watchlist, result)`,
  `write_risk_simulation_artifact(result, path)`
- Output schema: `risk-simulation-v1`
- Risk checks evaluated in order: `live_lock`, `has_candidates`, `strategy_fit_present`,
  `concentration`, `max_position_notional`, `max_daily_loss`, `drawdown_stress`,
  `regime_consistency`, `cost_adjusted_edge`
- `max_drawdown_bps` from the strategy-fit artifact is used as the daily-loss proxy (no separate
  daily-loss field exists in the strategy-fit schema v1).
- `drawdown_stress` check: `max_drawdown_bps × slippage_stress_multiplier <= max_stressed_drawdown_bps`
  (default stress multiplier = 2.0; threshold = 1000 bps).
- `regime_consistency` uses inline copy of the backtest_queue compatibility matrix — self-contained.
- Fail-closed: missing `max_drawdown_bps` or `net_expectancy_after_cost_bps` → early exit, passes=False.
- `approved_for_live=True` in input is forced False; adds `risk_live_approval_forbidden` reason.
- No broker/OMS/execution imports; no network/DB imports; no subprocess; artifact-only.
- `approved_for_live=False` always enforced in `apply_risk_simulation_to_watchlist` output.
- Result dict integrates with `evaluate_watchlist_promotion` via `risk_simulation_result` parameter.

**Failure reason constants:** `risk_no_candidates`, `risk_strategy_fit_missing`,
`risk_notional_limit_failed`, `risk_daily_loss_failed`, `risk_drawdown_stress_failed`,
`risk_concentration_failed`, `risk_regime_mismatch`, `risk_cost_adjusted_edge_failed`,
`risk_live_approval_forbidden`, `risk_missing_required_metric`

---

## 17. Watchlist Schema

Schema version: `watchlist-v1`

Written as JSON under `exports/watchlist/YYYYMMDD/`.

```json
{
  "schema_version": "watchlist-v1",
  "trade_date": "<YYYY-MM-DD>",
  "generated_at_utc": "<ISO8601>",
  "mode": "paper",
  "source_candidate_artifact": "<path>",
  "source_strategy_fit_artifact": ["<path>", ...],
  "symbols": ["<string>", ...],
  "ranked_candidates": [
    {
      "rank": 1,
      "symbol": "<string>",
      "strategy_id": "<string>",
      "total_score": "<float>",
      "regime_label": "<string>",
      "net_expectancy_after_cost_bps": "<float>",
      "paper_qty_limit": "<int>",
      "notional_limit_usd": "<float>",
      "selection_reason": "<string>"
    }
  ],
  "approved_for_autonomous_paper": "<bool>",
  "approved_for_live": false,
  "max_symbols_to_trade": 1,
  "max_concurrent_positions": 1,
  "selection_reason": "<string>",
  "strategy_assignments": {
    "<symbol>": "<strategy_id>"
  },
  "risk_limits": {
    "max_daily_loss_bps": 200,
    "max_position_notional_usd": 5000
  },
  "paper_qty_limits": {
    "<symbol>": "<int>"
  },
  "notional_limits": {
    "<symbol>": "<float>"
  }
}
```

Hard invariants:
- `approved_for_live` is always `false`.
- `mode` is always `"paper"` in v1.
- `max_symbols_to_trade` is 1 in v1. Multi-symbol is future work.
- `max_concurrent_positions` is 1 in v1.

---

## 18. Promotion / Demotion Rules

### 18.1 Promotion to Watchlist (paper)

A symbol/strategy pair is eligible for the next-day paper watchlist only when ALL:

1. Scanner candidate passes all hard gates (DQ, liquidity, regime).
2. Strategy-fit artifact exists for the symbol/strategy pair with `recommended_for_paper = true`.
3. Risk simulation passes.
4. **Operator review** has approved the watchlist artifact (v1: manual sign-off required).
5. Premarket revalidation (§19) passes on trade day.

`approved_for_autonomous_paper` is set to `true` only after operator review in v1.

### Implementation Note (WATCHLIST-PROMO-01)

- MAIN promotion module: `research-py/src/mqk_research/scanner/watchlist_promotion.py`
- Public API: `WatchlistPromotionConfig`, `PromotionInput`, `PromotionDecision`,
  `evaluate_watchlist_promotion(watchlist, strategy_fit_artifacts, config,
  risk_simulation_result=None, premarket_revalidation_result=None)`,
  `apply_watchlist_promotion(watchlist, decision, config)`,
  `write_promoted_watchlist(watchlist, path)`
- Promotion gates evaluated in order: `watchlist_schema_valid`, `watchlist_mode_paper`,
  `watchlist_live_locked`, `has_ranked_candidates`, `strategy_fit_present`,
  `strategy_fit_recommended_for_paper`, `risk_simulation_passed`, `operator_review_approved`,
  `premarket_revalidation`
- `operator_review_approved` defaults `False` — fail closed until explicit operator sign-off.
- `risk_simulation_passed` defaults `False` in config; overridden by `risk_simulation_result["passed"]`
  when the result dict is supplied (RISK-SIM-01).
- `premarket_revalidation_required` defaults `True` in config; overridden by
  `not premarket_revalidation_result["passed"]` when the result dict is supplied (WATCHLIST-PREMARKET-01).
- Config booleans remain supported for backward compatibility and direct testing.
- `approved_for_autonomous_paper=False` unless every gate passes.
- `approved_for_live=False` always — hard invariant, not overrideable by caller or config.
- `max_symbols_to_trade=1` and `max_concurrent_positions=1` forced in v1.
- Input `approved_for_live=True` is forced `False` and adds `live_approval_forbidden` reason.
- Output goes to `exports/watchlist/`; never writes to `config/watchlists`.
- No daemon integration in this patch. Daemon reads the promoted artifact at startup (§21).
- EXP penny scanner (`exp-candidate-v1`) is separate and not affected by this module.

### Implementation Note (WATCHLIST-PREMARKET-01 — CLOSED)

- MAIN premarket module: `research-py/src/mqk_research/scanner/premarket_revalidation.py`
- Public API: `PremarketRevalidationConfig`, `PremarketRevalidationResult`,
  `evaluate_premarket_watchlist(watchlist, symbol_inputs, config, reference_utc=None)`,
  `apply_premarket_revalidation_to_watchlist(watchlist, result)`,
  `write_premarket_revalidation_artifact(result, path)`
- Output schema: `premarket-revalidation-v1`
- All inputs are artifact/dict-based — no broker calls, no live market data API, no DB.
- Checks evaluated per top symbol: `watchlist_schema_valid`, `mode_paper`, `live_lock`,
  `symbol_input_present`, `symbol_not_suppressed`, `data_quality_passed`, `liquidity_passed`,
  `regime_compatible`, `bar_freshness`, `spread_limit`, `slippage_limit`, `rvol_threshold`,
  `price_threshold`
- Spread/slippage/rvol/price checks only fire when the field is present in `symbol_inputs`; absent
  means not checked (not failed). Bar freshness is fail-closed: absent `latest_bar_ts` fails.
- `reference_utc` parameter allows deterministic time-based testing without mocking datetime.
- Regime compatibility uses inline copy of the backtest_queue compatibility matrix — self-contained.
- `approved_for_live=True` in watchlist input forces False; adds `premarket_live_approval_forbidden`.
- No broker/OMS/execution imports; no network/DB imports; no subprocess; artifact-only.
- `approved_for_live=False` always enforced in `apply_premarket_revalidation_to_watchlist` output.
- Result dict integrates with `evaluate_watchlist_promotion` via `premarket_revalidation_result` parameter.
- Previously required fresh `symbol_inputs` from a premarket data refresh runner to be
  operationally useful — that producer now exists (see SYMBOL-INPUTS-PRODUCER-01 below).
- Does NOT prove that scanner-driven autonomous paper is ready — daemon handoff remains future work.
- Does NOT submit orders or enable live routing.

**Failure reason constants:** `premarket_watchlist_schema_invalid`, `premarket_mode_not_paper`,
`premarket_live_approval_forbidden`, `premarket_symbol_input_missing`, `premarket_symbol_suppressed`,
`premarket_data_quality_failed`, `premarket_liquidity_failed`, `premarket_regime_incompatible`,
`premarket_bar_stale`, `premarket_spread_too_wide`, `premarket_slippage_too_high`,
`premarket_rvol_too_low`, `premarket_price_too_low`

### Implementation Note (SYMBOL-INPUTS-PRODUCER-01)

- MAIN producer module: `research-py/src/mqk_research/scanner/symbol_inputs.py`
- Public API: `SymbolInputSpec`, `build_symbol_input_record(spec, ...)`,
  `build_symbol_inputs(specs, *, trade_date, source, ...)`,
  `write_symbol_inputs_artifact(artifact, path)`, `load_symbol_inputs_artifact(path)`,
  `extract_symbol_inputs_map(artifact)`
- Output schema: `symbol-inputs-v1` — `{schema_version, generated_at_utc, trade_date, source,
  symbols: {symbol: record}, approved_for_live: false, notes}`
- **Producer role**: converts in-memory bar history (plus optional precomputed liquidity
  metrics) into the `symbol_inputs` artifact that `evaluate_premarket_watchlist` consumes.
  It is an artifact *assembler*, not a gate author — it delegates honestly to the existing
  MAIN scanner gates (`evaluate_data_quality`, `evaluate_liquidity`, `evaluate_regime`) and
  reports their output verbatim per symbol.
- **Artifact-only boundary**: caller supplies bars (and, optionally, `LiquidityMetrics`); this
  module does not fetch market data, does not derive ADV/spread/slippage from intraday bars,
  and does not write to any MAIN-operational location. No broker/OMS/daemon/runtime/network/DB
  imports; no subprocess; no order or strategy-signal endpoint references.
- **Fail-closed per-symbol records** (never silently disappear — always represented via
  `rejection_reason`/`reason_tags`): no bars (`no_bars`), insufficient bars
  (`insufficient_completed_bars`), stale latest bar (`latest_bar_stale`), other data-quality
  failures (gate-native reason), liquidity metrics absent
  (`symbol_input_liquidity_metrics_missing`), liquidity gate failure (gate-native reason),
  regime unclassified/incompatible (gate-native reason, e.g. `regime_unclassified`), and
  missing/invalid latest price (`symbol_input_missing_latest_price`).
- Liquidity/regime are evaluated only once `data_quality_passed=True` (both gates assume
  DQ-clean bars); when DQ fails, liquidity/regime are honestly reported as not evaluated
  (`liquidity_passed=False`, `regime_label=None`) rather than fabricated as healthy.
- "Strategy compatibility" of a regime is intentionally NOT decided here — it requires the
  watchlist's `strategy_assignments`, which is a premarket-level concern. The producer reports
  `regime_label`/`regime_score` honestly; compatibility is judged downstream by
  `evaluate_premarket_watchlist`.
- **How premarket consumes it**: `extract_symbol_inputs_map(artifact)` adapts a
  `symbol-inputs-v1` artifact into the flat `dict[symbol, record]` shape
  `evaluate_premarket_watchlist` expects (or callers may pass `artifact["symbols"]` directly,
  since per-symbol record field names already match what premarket revalidation reads). A
  malformed or schema-mismatched artifact yields an empty map — the same honest
  `premarket_symbol_input_missing` failure as a genuinely absent artifact; never a
  fabricated/partial map.
- `approved_for_live=False` is a hard invariant enforced at four independent layers: forced at
  artifact assembly (not overrideable by caller), forced again on the persisted copy in
  `write_symbol_inputs_artifact` (defense-in-depth against a tampered in-memory artifact),
  forced again on the returned copy in `load_symbol_inputs_artifact` (defense-in-depth against
  a forged on-disk artifact), and never read or propagated by `extract_symbol_inputs_map`
  (premarket revalidation only consults the watchlist's own `approved_for_live`).
- No broker/OMS/execution imports; no network/DB imports; no subprocess; no daemon/runtime
  imports; artifact-only, matching the `premarket_revalidation` boundary.
- **Required-before-trust note**: a `symbol_inputs` artifact produced by this module is a
  necessary input for `evaluate_premarket_watchlist` to be operationally meaningful (rather
  than perpetually reporting `premarket_symbol_input_missing`), but its presence alone does
  NOT make scanner-driven autonomous paper trustworthy. That requires watchlist promotion +
  premarket revalidation + operator review + daemon handoff to all be proven together — this
  patch closes one link in that chain, not the chain itself.
- Producer-local reason constants: `symbol_input_liquidity_metrics_missing`,
  `symbol_input_missing_latest_price`.
- Tests: `research-py/tests/test_scanner_symbol_inputs.py` (builder unit tests SI01-SI12 plus
  premarket-integration tests SI13-SI18 exercising `evaluate_premarket_watchlist` against
  produced artifacts). Script guard: `tests/script_guards/test_scanner_symbol_inputs.ps1`.
- Does NOT submit orders, enable live routing, or change live eligibility.

### Implementation Note (SYMBOL-INPUTS-RUNNER-01)

- MAIN runner module: `research-py/src/mqk_research/scanner/symbol_inputs_runner.py`
- Public API: `SymbolInputsRunnerConfig`, `SymbolInputsRunnerResult`,
  `run_symbol_inputs_producer(config)`, plus pure helpers `load_symbols_from_watchlist`,
  `resolve_bars_path`, `load_bars_for_symbol`, `load_liquidity_metrics_for_symbol`,
  `normalize_bars_json`, `normalize_bars_csv`; CLI entry point via `main(argv)`.
- **Runner role**: this module is the *operational* counterpart to
  `SYMBOL-INPUTS-PRODUCER-01` — it resolves a symbol list (from a `watchlist-v1`
  artifact or an explicit `--symbols` list), loads caller-local bar files (and an
  optional liquidity sidecar) from disk, builds `SymbolInputSpec` records, and
  delegates assembly/persistence entirely to the existing
  `build_symbol_inputs` / `write_symbol_inputs_artifact` producer functions. It
  invents no producer/gate logic of its own.
- **Artifact-only / local-only boundary**: the runner reads only local files supplied
  by the caller (`--bars-root`, `--liquidity-root`, `--watchlist`) and writes only the
  local `symbol-inputs-v1` artifact at `--output`. It does not fetch market data, does
  not call any broker/API/daemon, does not derive liquidity from bars, and does not
  write to any MAIN-operational location. No broker/OMS/daemon/runtime/network/DB
  imports; no subprocess; no order or strategy-signal endpoint references.
- **Bars file conventions** under `bars_root` (first match wins):
  `<SYMBOL>_<TIMEFRAME>.json`, `<SYMBOL>_<TIMEFRAME>.csv`, `<SYMBOL>.json`, `<SYMBOL>.csv`.
  JSON bars may be a top-level list or a dict with a `bars` list (passed through as-is,
  already in the scanner-style shape `symbol_inputs` accepts). CSV bars support both
  scanner-style headers (`ts,open,high,low,close,volume[,is_complete]`) and
  backtest-style micros headers (`symbol,end_ts,open_micros,high_micros,low_micros,
  close_micros,volume,is_complete`), auto-detected from the header row; backtest-style
  micros are converted to decimal dollars (`value / 1_000_000`) and `end_ts` (Unix epoch
  seconds, UTC) is converted to an ISO-8601 `ts` string.
- **Liquidity sidecar convention** (new, minimal — no prior repo convention existed):
  `<SYMBOL>.json` or `<SYMBOL>_liquidity.json` under `--liquidity-root`, a flat JSON
  object carrying the seven `LiquidityMetrics` fields. Absent sidecar →
  `liquidity_metrics=None` (producer fail-closes liquidity honestly, same as no
  liquidity supplied at all); malformed sidecar → also `None`, plus a per-symbol
  `symbol_inputs_runner_liquidity_sidecar_malformed:<symbol>` reason. The runner never
  derives liquidity from bars.
- **Fail-closed cases** (every one surfaces an explicit, stable reason string —
  never silent omission, never a fabricated healthy result):
  - no watchlist/symbol input supplied → blocked, `symbol_inputs_runner_no_input_source`
  - watchlist fails to load/parse → blocked, `symbol_inputs_runner_watchlist_load_failed`
  - resolved symbol list is empty → blocked, `symbol_inputs_runner_no_symbols_resolved`
  - bars root missing/not a directory → blocked, `symbol_inputs_runner_bars_root_missing`
  - no bars file found for a symbol → symbol still written via the producer's own
    no-bars fail-closed record; symbol listed in `missing_bars`
  - malformed bars file for a symbol → same fail-closed record; symbol listed in
    `failed_symbols` with `symbol_inputs_runner_malformed_bars_file:<symbol>`
  - output artifact cannot be written → blocked, `symbol_inputs_runner_output_write_failed`
- **Forged live-approval handling**: a watchlist carrying `approved_for_live: true` is
  read for its `symbols` list only — the forged flag never propagates into the runner
  config, result, or output artifact. It is instead reported via
  `watchlist_live_approval_forbidden=True` and the reason
  `symbol_inputs_runner_watchlist_live_approval_forbidden`, so the forgery is visible to
  the operator rather than silently dropped or silently honored.
- **Result shape** (`SymbolInputsRunnerResult.to_dict()`): `symbols_requested,
  symbols_written, output_path, missing_bars, failed_symbols, failure_reasons,
  watchlist_live_approval_forbidden, approved_for_live, status, notes`. `status` is
  `"complete"` (all symbols resolved cleanly), `"partial"` (artifact written but some
  symbols missing/failed), or `"blocked"` (nothing written).
- `approved_for_live=False` is a hard invariant enforced independently at both the
  config and result layers via `__post_init__` + `object.__setattr__` (the same pattern
  used by `WalkForwardRunnerConfig`/`BacktestBridgeConfig`/`PremarketRevalidationConfig`),
  in addition to the four independent layers already enforced inside the
  `symbol_inputs` producer itself.
- No broker/OMS/execution imports; no network/DB imports; no subprocess; no
  daemon/runtime imports — matching the `symbol_inputs` / `premarket_revalidation`
  artifact-only boundary.
- **Required-before-trust note**: this runner makes `symbol_inputs` *operationally
  reachable* from a watchlist or symbol list plus local bar files — closing the
  "how does an operator actually produce this artifact" gap left open by
  `SYMBOL-INPUTS-PRODUCER-01`. It does NOT wire watchlist enforcement into the daemon,
  does NOT fetch live/broker market data, and does NOT make scanner-driven autonomous
  paper trading trustworthy on its own. Daemon handoff and live market-data sourcing
  remain explicitly open.
- Runner-local reason constants: `symbol_inputs_runner_no_input_source`,
  `symbol_inputs_runner_watchlist_load_failed`, `symbol_inputs_runner_no_symbols_resolved`,
  `symbol_inputs_runner_bars_root_missing`, `symbol_inputs_runner_malformed_bars_file`,
  `symbol_inputs_runner_liquidity_sidecar_malformed`, `symbol_inputs_runner_output_write_failed`,
  `symbol_inputs_runner_watchlist_live_approval_forbidden`.
- Tests: `research-py/tests/test_scanner_symbol_inputs_runner.py` (symbol resolution,
  bars loading incl. micros conversion, artifact output incl. premarket-consumption
  proof, liquidity handling, safety-import checks, hard-invariant checks). Script guard:
  `tests/script_guards/test_scanner_symbol_inputs_runner.ps1`.
- Does NOT submit orders, enable live routing, change live eligibility, fetch market
  data, or call any broker/daemon/API.

### Implementation Note (WATCHLIST-PROMOTION-END-TO-END-ARTIFACT-CHAIN-01)

- **Proof module (test-only, no new production module)**:
  `research-py/tests/test_scanner_watchlist_promotion_end_to_end_artifact_chain.py`
- **Purpose**: closes the remaining trust-chain gap by proving — with real
  evaluator functions and deterministic fixtures, never hand-forged derived
  fields — that the full research-layer artifact chain wires together honestly
  end to end:

  ```
  ranked/scanner candidate (watchlist-v1)
    -> backtest queue (regime/strategy compatibility, strategy_ids_for_regime)
    -> strategy-fit-v1 (evaluated through the real backtest_gates evaluator)
    -> backtest gates (recommended_for_paper derived honestly via apply_backtest_gates)
    -> risk simulation (evaluate_watchlist_risk)
    -> symbol-inputs runner (run_symbol_inputs_producer, real temp-dir bars + liquidity sidecar)
    -> premarket revalidation (evaluate_premarket_watchlist)
    -> watchlist promotion (evaluate_watchlist_promotion / apply_watchlist_promotion)
    -> promoted paper watchlist artifact (written + reloaded from a temp dir)
  ```

- **Scenario A (passing chain)**: a single comprehensive walk asserts, at every
  stage, that `approved_for_live=False`/`recommended_for_live=False` hold, that
  `recommended_for_paper` and `data_quality_passed`/`liquidity_passed`/
  `regime_label` are *derived* by the real evaluators (not asserted into the
  fixture), and that the final promoted-and-reloaded artifact carries
  `approved_for_autonomous_paper=True`, `approved_for_live=False`,
  `max_symbols_to_trade=1`, `max_concurrent_positions=1`.
- **Scenarios B1-B10 (fail-closed)**: each blocks promotion via a distinct,
  real gate failure and asserts `approved_for_autonomous_paper=False` /
  `approved_for_live=False` propagate through to the applied artifact:
  - B1 `TestFailClosedMissingStrategyFit` — no strategy-fit artifact for the top symbol
  - B2 `TestFailClosedStrategyFitNotRecommended` — fit genuinely fails backtest gates (too few trades)
  - B3 `TestFailClosedRiskSimulationFails` — passes backtest gates but fails risk daily-loss check
  - B4 `TestFailClosedOperatorReviewMissing` — `operator_review_approved` left at its fail-closed default
  - B5 `TestFailClosedPremarketMissingSymbolInput` — symbol absent from the symbol-inputs map (and entirely missing artifact)
  - B6 `TestFailClosedPremarketStaleBar` — bar was fresh when the symbol-inputs producer ran but is stale by the time premarket revalidation re-checks it against a later `reference_utc` (the realistic "time has passed since the artifact was produced" revalidation case — distinct from B5's missing-input case)
  - B7 `TestFailClosedForgedLiveFlag` — input watchlist forges `approved_for_live=true`; every downstream artifact (risk, premarket, promotion, applied, written-and-reloaded) is asserted to force it back to `False` and record `*_live_approval_forbidden`
  - B8 `TestFailClosedMismatchedStrategy` — strategy-fit `strategy_id` does not match the watchlist's `strategy_assignments` entry for the top symbol
  - B9 `TestFailClosedMismatchedSymbol` — strategy-fit `symbol` field does not match the watchlist's top-ranked symbol (including the case where the fit artifact is keyed entirely to a different symbol)
  - B10 `TestFailClosedMissingOrMalformedSymbolInputs` — symbol-inputs artifact file absent (`load_symbol_inputs_artifact` raises, fails closed) or malformed (`extract_symbol_inputs_map` returns an empty map, fails closed)
- **Real integration gap closed in `watchlist_promotion.py`** (minimal,
  additive — no existing gate weakened): gate 5 (`strategy_fit_present`)
  previously checked only artifact *presence* and `recommended_for_paper`,
  trusting the `strategy_fit_artifacts[top_symbol]` dict key and the artifact's
  self-reported `symbol`/`strategy_id` fields without verifying they actually
  match the watchlist's top-symbol identity and `strategy_assignments` entry.
  A forged or mismatched fit artifact (wrong symbol, or a strategy_id the
  watchlist never assigned) would have silently passed. Two new fail-closed
  reasons close this gap:
  - `strategy_fit_symbol_mismatch` — fit artifact's `symbol` ≠ watchlist's top symbol
  - `strategy_fit_strategy_mismatch` — fit artifact's `strategy_id` ≠ watchlist's assigned `strategy_id` for that symbol
  (`strategy_fit_missing` and `strategy_fit_not_recommended_for_paper` were
  already present; both are now named reason constants alongside the two new
  ones.) Existing `test_scanner_watchlist_promotion.py` fixtures already use
  matching symbol/strategy identities, so this patch only *adds* coverage —
  it does not change any existing passing/failing outcome.
- **Artifact-only / no-wiring boundary (unchanged)**: deterministic in-memory
  and temp-dir fixtures only; no daemon/broker/DB/network/subprocess imports;
  no order submission; no strategy-signal injection; no live routing. This
  proof does NOT establish market/live/paper truth — it proves the research
  modules wire together honestly and fail closed at every link. Daemon
  enforcement wiring remains explicitly open (§22).
- Script guard: `tests/script_guards/test_scanner_watchlist_promotion_end_to_end.ps1`
  (asserts presence of all eleven scenario classes, all five chain schema
  strings, and that `approved_for_live=True`/`recommended_for_live=True`/
  `eligible_for_live=True` never appear as constructed or asserted values
  anywhere in the proof — except the single deliberate forged-input fixture
  call exercised by B7).

### 18.2 Demotion from Watchlist

A symbol is removed from the watchlist if ANY:

- Premarket revalidation fails (data quality, regime shift, liquidity drop).
- Symbol appears on the suppression list.
- Daemon or paper session is halted or disarmed.
- Reconcile is dirty.
- WS continuity is not Live.
- Operator explicitly removes it.

Demotion is permanent for the session. A demoted symbol requires a fresh scan cycle.

### 18.3 Promotion to Live

`approved_for_live` remains `false` until:

1. Minimum `N` clean paper sessions with the symbol/strategy pair (default N = 10 sessions).
2. No reconcile drift events in any of those sessions.
3. Fill quality telemetry shows slippage within 1.5× backtest estimate for >= 80% of fills.
4. Explicit operator promotion action (not automated).

This spec does not implement the live-promotion path. It defines the evidence threshold only.

---

## 19. Schedule Model

All times are US Eastern.

| Phase | Schedule | Description |
|---|---|---|
| Premarket universe load | 05:30 | Load and validate universe list |
| Premarket bar refresh | 06:00 | Refresh daily/hourly bar history for universe |
| Premarket watchlist validation | 07:00 | Revalidate prior-day watchlist against current data |
| Market-open ingestion | 09:30 | Ingest approved watchlist into autonomous paper session |
| Intraday scanner refresh | Every 15 min during market hours | Re-score universe; observe-only |
| Market-close final ranking | 16:00–16:15 | Final intraday bar collection; produce ranked candidate export |
| Off-hours backtest queue write | 16:30 | Write backtest queue for new candidates |
| Off-hours backtest run | 18:00–22:00 | Run parity backtests; write strategy-fit artifacts |
| Overnight watchlist generation | 22:30 | Produce next-day watchlist; await operator review |
| Post-session evidence review | After 16:15 | Review fill quality, reconcile, paper session summary |

The schedule model is advisory. Exact timing is operator-configured. All phases are idempotent: re-running a phase produces the same artifacts given the same input data.

---

## 20. Data Requirements

| Data Type | Source | Freshness Requirement |
|---|---|---|
| 5m intraday bars | Alpaca intraday API (`Refresh-IntradayMarketData.ps1`) | Within 20 min during market hours |
| Daily bars | Alpaca historical API | Within 1 business day |
| Symbol metadata | Alpaca assets API or static config | Within 7 days |
| Universe list | `config/scanner/universe_v1.csv` | Operator-managed |
| Suppression list | `config/scanner/suppressed_symbols.txt` | Operator-managed |

Missing or stale data does not fall through as "authoritative empty." The scanner hard-rejects any symbol whose data fails the freshness check and writes the rejection reason.

---

## 21. Main-Engine Integration

The scanner and backtest pipeline are **read-only** consumers of data and produce **read-only artifacts**. They do not push state into the main engine.

The main engine consumes the watchlist artifact at market open through the existing `POST /api/v1/strategy/signal` gate. The autonomous paper session reads the watchlist to determine which symbol/strategy pair to accept signals for.

Integration contract:
- The daemon reads `exports/watchlist/YYYYMMDD/watchlist.json` at startup (configurable path).
- If the watchlist is absent, the daemon treats it as `approved_for_autonomous_paper = false`.
- If the watchlist is present and `approved_for_autonomous_paper = true`, the daemon accepts signals for listed symbol/strategy pairs.
- The daemon does not use the watchlist to bypass any existing gate (arm, halt, WS continuity, reconcile, etc.).
- The watchlist is advisory to signal admission; it does not override runtime safety.

---

## 22. Autonomous Paper Handoff — Enforcement Design Contract

### PAPER-HANDOFF-ENFORCE-DESIGN-ONLY-01 (CLOSED — b committed after this patch)

**Status: Design-only. Enforcement NOT active. Dry-run surface available.**

#### What is implemented (this patch)

- `GET /api/v1/watchlist/admission-check?symbol=<sym>&strategy_id=<id>` — dry-run admission check.
  - Returns whether a (symbol, strategy_id) pair would be admitted under the current watchlist.
  - Response always includes `note: "dry_run_only_not_enforced"`.
  - `approved_for_live` is always `false`.
  - Pure read-only: no broker calls, no DB mutations, no orders, no outbox/inbox writes.
- `WatchlistAdmissionCheckResponse` in `api_types.rs`.
- 14 proof tests (AD01–AD14) in `scenario_watchlist_admission_dryrun_01.rs`.
- Script guard `test_watchlist_admission_dryrun.ps1`.

#### What is NOT implemented (deferred to PAPER-HANDOFF-ENFORCE-01)

- Watchlist admission is **not** wired into `POST /api/v1/strategy/signal`.
- The live signal path is unmodified in this patch.
- No orders are gated on the watchlist in this patch.

#### Future enforcement wiring contract (PAPER-HANDOFF-ENFORCE-01)

When enforcement is wired, the admission gate must be inserted into `strategy_signal()` in `routes/strategy.rs` **before the outbox enqueue step (Gate 7)** and **after the existing gate sequence**.

The future gate must:
1. Call `evaluate_watchlist_intake_from_env()` to load the artifact.
2. Call `evaluate_watchlist_signal_admission(&outcome, &signal.symbol, &signal.strategy_id)`.
3. If `allowed == false`, refuse with a structured 403/409 response — same pattern as existing gates.
4. Must not bypass arm/halt/WS-continuity/session/reconcile/risk gates already in the chain.
5. Must fail closed when watchlist is missing or invalid (`watchlist_not_configured` → 503, `watchlist_missing` → 503, `watchlist_invalid` → 503, `watchlist_not_approved` → 409).
6. Must preserve `approved_for_live=false` — the watchlist gate must never authorize live trading.
7. Symbol-level and strategy-level checks must pass independently.

#### Enforcement prerequisite conditions

Enforcement (PAPER-HANDOFF-ENFORCE-01) must not be wired until ALL of:
- AAPL sell/flatten market proof is complete and evidenced.
- At least one full autonomous paper cycle (open → fill → close → reconcile-clean) is observed.
- The dry-run admission check endpoint has been exercised in live market conditions.
- Monday smoke evidence confirms stable operation.

#### approved_for_live invariant (permanent)

`approved_for_live=false` is a hard invariant in all watchlist-related code paths.
It must never be set to `true` by any watchlist, admission check, or scanner artifact.
This invariant is enforced at multiple layers:
- `evaluate_watchlist_intake` — `approved_for_live=true` in artifact → `Invalid`.
- `WatchlistIntakeOutcome::approved_for_live()` — always returns `false`.
- `WatchlistStatusResponse.approved_for_live` — always `false`.
- `WatchlistAdmissionCheckResponse.approved_for_live` — always `false`.
- Script guards verify the literal `false` assignment in all route response builders.

---

## 22. Autonomous Paper Handoff

The handoff is a one-way read:

```
watchlist artifact (file) → daemon reads at startup
                         → filters incoming signals to approved symbol/strategy pairs
                         → all other runtime gates still apply
```

Rejected signals (symbol not in watchlist, strategy not assigned) are logged but do not fault the session.

First-version behavior:
- At most one symbol is active at a time.
- If the watchlist has multiple candidates, the daemon picks the top-ranked `approved_for_autonomous_paper = true` entry.
- During the session, the intraday re-ranking produces updated scores (observe-only; no live position changes based on re-ranking).

### Implementation Note — PAPER-HANDOFF-READONLY-01 (DONE, 2026-06-06)

`PAPER-HANDOFF-READONLY-01` adds daemon-side read-only watchlist artifact intake and a
status surface at `GET /api/v1/watchlist/status`.

**What this patch does:**
- Loads a `watchlist-v1` JSON artifact from `MQK_PAPER_WATCHLIST_PATH`.
- Validates schema_version, mode=paper, approved_for_live=false, symbols, strategy_assignments, max_symbols_to_trade=1, max_concurrent_positions=1.
- Returns one of: `not_configured`, `missing`, `invalid`, `loaded_not_approved`, `loaded_approved`.
- Exposes a pure dry signal-admission contract (`evaluate_watchlist_signal_admission`) for future use.
- Hard live lock: any artifact with `approved_for_live=true` → `invalid` outcome.

**What this patch does NOT do:**
- Does NOT submit orders.
- Does NOT call broker endpoints.
- Does NOT mutate production DB.
- Does NOT bypass arm/halt/WS-continuity/reconcile/risk gates.
- Does NOT wire `evaluate_watchlist_signal_admission` into the live strategy signal path.
- Does NOT make scanner-driven autonomous paper ready.
- `approved_for_live` is always `false` in every response.

**Next step:** `PAPER-HANDOFF-ENFORCE-01` will wire signal admission into the strategy signal
route (after market smoke proof for Monday AAPL sell/flatten).

---

## 23. Live-Trading Lock

The live-trading lock is preserved at multiple levels:

| Level | Mechanism |
|---|---|
| Scanner artifact | `eligible_for_live: false` (hardcoded) |
| Strategy-fit artifact | `recommended_for_live: false` (hardcoded) |
| Watchlist artifact | `approved_for_live: false` (hardcoded) |
| Daemon gate | PT-TRUTH-01: paper+paper is fail-closed; paper+alpaca requires WS Live |
| Promotion gate | TV-02A: artifact deployability gate at runtime start boundary |
| Capital policy | TV-04F: live capital requires explicit policy; absent policy → 403 |
| Manual promotion | No automated path from paper to live exists in this pipeline |

Breaking this lock requires: (a) explicit operator action, (b) repeated clean paper evidence meeting the thresholds in §18.3, and (c) a separate live-promotion workflow (future work, not in this spec).

---

## 24. Evidence Artifacts

All artifacts are immutable once written. They are named with timestamps and UUIDs for deduplication.

| Artifact | Path | Schema Version |
|---|---|---|
| Scanner candidate journal | `exports/candidates/YYYYMMDD/<scanner_id>_<ts>.jsonl` | `scanner-candidate-v1` |
| Rejection log | `exports/candidates/YYYYMMDD/<scanner_id>_rejected_<ts>.jsonl` | `scanner-candidate-v1` (eligible_for_scan=false) |
| Ranked candidate export | `exports/candidates/YYYYMMDD/ranked_candidates.json` | `scanner-candidate-v1` |
| Backtest queue | `exports/backtest_queue/YYYYMMDD_queue.json` | `backtest-queue-v1` |
| Strategy-fit report | `exports/strategy_fit/YYYYMMDD/<symbol>_<strategy_id>_<ts>.json` | `strategy-fit-v1` |
| Watchlist | `exports/watchlist/YYYYMMDD/watchlist.json` | `watchlist-v1` |
| Post-session evidence review | `exports/evidence/YYYYMMDD/session_review.md` | text |

Evidence artifacts integrate with the existing evidence review tooling (`Review-PaperSmokeEvidence.ps1`).

---

## 25. Required Tests / Guards

Each implementation patch must include scenario tests following the existing repo proof standard (committed code + passing tests = CLOSED):

| Component | Required Tests |
|---|---|
| Data-quality gate | All 8 rejection checks + pass case |
| Liquidity gate | All 5 rejection checks + pass case |
| Regime classifier | All 7 labels + unclassified fallback |
| Candidate schema | Required fields present; eligible_for_live always false |
| Candidate scorer | Weight sum check; score bounds [0,1] per component |
| Backtest queue writer | Queue is idempotent; no broker calls |
| Strategy-fit schema | Required fields; recommended_for_live always false |
| Backtest pass/fail gates | All 8 gate checks, pass case |
| Walk-forward split | Window sizes correct; no lookahead |
| Watchlist schema | Required fields; approved_for_live always false; mode always paper |
| Promotion gate | Positive path (all criteria met); rejection for each missing criterion |
| Demotion rules | Each demotion trigger removes symbol |
| Live-trading lock | eligible_for_live / recommended_for_live / approved_for_live always false (unit proof) |

Static guard: the CI guard script must verify that no scanner, backtest, or watchlist module imports broker adapter, OMS, or execution orchestrator modules.

---

## 26. Implementation Roadmap

One patch per turn. No bundling.

| Patch | ID | Description |
|---|---|---|
| 1 | This spec | MARKET-SCANNER-BACKTEST-CANDIDATE-PIPELINE-SPEC-01 — docs only |
| 2 | SCANNER-CANDIDATE-01 | Candidate artifact writer: schema, writer class, required field validation, live-lock proof test |
| 3 | SCANNER-DQ-01 | Data-quality gate: all 8 checks, score computation, rejection writer |
| 4 | SCANNER-LIQUIDITY-01 | Liquidity/execution-cost gate: all 5 checks, score computation |
| 5 | SCANNER-REGIME-01 | Volatility/regime classifier: all 7 labels, score computation, unclassified fallback |
| 6 | SCANNER-SCORE-01 | Candidate scorer: weighted score, total_score, reason tags |
| 7 | SCANNER-SELECTOR-01 | End-of-day ranked export and watchlist artifact writer |
| 8 | BACKTEST-QUEUE-01 | Backtest queue writer: reads ranked export, applies compatibility matrix, writes queue file |
| 9 | BACKTEST-RUNNER-01 | Off-hours backtest runner: calls mqk-backtest parity engine, writes strategy-fit artifacts |
| 10 | BACKTEST-GATES-01 | Strategy-fit pass/fail gates and walk-forward validation |
| 11 | WATCHLIST-PROMO-01 | Watchlist promotion gate: all criteria, operator review hook |
| 12 | WATCHLIST-PREMARKET-01 | Premarket revalidation runner |
| 12b | PAPER-READINESS-RUNNER-01 | Operational composite runner (`paper_readiness_runner.py`): composes symbol-inputs → risk-sim → premarket → promotion into a single toggle-controlled, fail-closed entry point; 28 tests + 14 script-guard assertions; market-data bridge gap documented below |
| 13 | PAPER-HANDOFF-01 | Autonomous paper handoff: daemon reads watchlist, signal filter |
| 14 | GUI-SCANNER-01 | GUI/Discord surfaces for scanner/watchlist state |
| 15 | EVIDENCE-REVIEW-01 | Post-session evidence review integration |
| 16 | PAPER-REPEAT-01 | Repeated paper validation tracking (N sessions gate) |
| 17 | LIVE-SHADOW-LATER | Live-shadow promotion path — future; not started in this roadmap |

Patches 2–13 target the `research-py` layer and script runners. Patches 14–16 touch the daemon GUI and evidence tooling. Patch 17 is deferred.

### MARKET-DATA-EXPORT-01 - md_bars to bars_root Bridge

`paper_readiness_runner.py` reads **local JSON/CSV bar files** from a `bars_root` directory (matching `<SYMBOL>_<TIMEFRAME>.{json,csv}`) via `symbol_inputs_runner.py`. `Refresh-IntradayMarketData.ps1` writes refreshed bars into the **Postgres `md_bars` table** (paper DB, port 5440) via `mqk-cli md sync-provider`.

`MARKET-DATA-EXPORT-01` adds a read-only Python export bridge:

```powershell
cd research-py
python -m mqk_research.scanner.market_data_export `
  --database-url "<paper/test postgres url>" `
  --symbols AAPL,MSFT `
  --timeframe 5m `
  --start-utc 2026-06-08T13:30:00Z `
  --end-utc 2026-06-08T20:00:00Z `
  --bars-root ..\exports\scanner\bars\20260608 `
  --trade-date 2026-06-08
```

The exporter reads existing completed `md_bars` rows only. It writes one CSV per requested symbol/timeframe:

- `<bars_root>/<SYMBOL>_<TIMEFRAME>.csv`
- Example: `exports/scanner/bars/20260608/AAPL_5m.csv`

The CSV schema is the scanner-supported backtest-style micros format:

```text
symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete
```

`end_ts` is Unix epoch seconds UTC; OHLC prices remain integer micros; `volume` remains the canonical integer volume; `is_complete` is emitted as a parseable boolean. `symbol_inputs_runner.normalize_bars_csv()` auto-detects this header and converts it into scanner-style bar dictionaries for the symbol-inputs stage.

This unblocks `paper_readiness_runner.py` only after refreshed `md_bars` rows already exist and the operator provides an explicit `bars_root`. The bridge does not fetch market data, does not write to Postgres, does not call a daemon, does not submit orders, does not call broker APIs, and does not wire daemon enforcement. Live routing remains hard-disabled; paper-readiness output still requires the existing runner gates and operator review before any paper handoff is considered.

This patch does not by itself prove real refreshed market data exists. Real-data proof requires a separate DB-gated run against a populated paper/test `md_bars` table and review of the resulting local bar files.

### MARKET-DATA-EXPORT-DB-PROOF-01 - Operator DB Proof Harness

`MARKET-DATA-EXPORT-DB-PROOF-01` adds an operator-run proof harness for the existing `MARKET-DATA-EXPORT-01` bridge. It connects to a caller-provided paper/test Postgres URL, runs the existing read-only exporter, inspects the generated `<SYMBOL>_<TIMEFRAME>.csv` files, proves those CSV files parse through `symbol_inputs_runner.normalize_bars_csv()`, and writes a local JSON proof report.

Required command shape:

```powershell
cd research-py
python scripts/prove_market_data_export_db.py `
  --database-url "<redacted paper/test db url>" `
  --symbols AAPL `
  --timeframe 5m `
  --start-utc 2026-06-08T13:30:00Z `
  --end-utc 2026-06-08T20:00:00Z `
  --bars-root ..\exports\scanner\bars\20260608 `
  --trade-date 2026-06-08 `
  --proof-report-path ..\exports\scanner\proofs\20260608\market_data_export_db_proof.json
```

The database URL may also be supplied through `MQK_PAPER_DB_URL`; the proof output redacts the URL and never documents credentials.

The proof report schema is `market-data-export-db-proof-v1`. It records requested/exported symbols, bars root, output paths, row counts, first/last `end_ts`, CSV parse pass/fail by symbol, optional downstream runner statuses, failure reasons, and fixed safety flags:

- `approved_for_live: false`
- `daemon_enforcement_executed: false`
- `broker_calls_executed: false`
- `db_writes_executed: false`

To additionally prove the exported bars can feed the existing symbol-inputs runner, add:

```powershell
--run-symbol-inputs
```

To additionally run the existing paper-readiness artifact chain, provide the required local artifacts:

```powershell
--run-paper-readiness `
  --watchlist-path <path> `
  --strategy-fit-dir <path>
```

`--operator-review-approved` is off by default. Without it, a valid paper-readiness run may stop at `ready_for_operator_review`; with it, the existing runner may report `ready_for_paper_handoff`. In both cases this proof remains artifact-only.

This proof does not trade, does not start a daemon, does not arm paper trading, does not submit orders, does not call broker APIs, does not inject strategy signals, does not write to Postgres, does not wire watchlist enforcement, and does not prove live readiness. Live remains hard-disabled.

---

## Appendix A: Relationship to Existing Specs

| This Spec | Related Existing Spec |
|---|---|
| Backtest pass/fail gates | `docs/specs/backtest_policy.md` — parity engine is the promotion source of truth |
| Strategy scoring | `docs/specs/strategy_evaluation_and_ranking.md` — consistency score and hard gates |
| Data pipeline | `docs/specs/data_pipeline_and_integrity.md` — bar freshness and integrity requirements |
| Promotion artifacts | `research-py/src/mqk_research/deployment/` — TV-01/TV-02/TV-03 chain |
| Capital policy | `research-py/src/mqk_research/` — TV-04A/TV-04B/TV-04F gates |
| Autonomous paper signal | `docs/specs/execution_model.md` — signal admission gates |
| Live lock | `docs/specs/kill_switches_and_limits.md` — kill-switch and halt invariants |

## Appendix B: Relationship to Existing Scanner Code

The existing `research-py/experiments/exp_penny/` scanner (EXP-PENNY-01A) is an **experimental** (EXP) implementation targeting penny stocks. This pipeline spec defines the **main-engine** (MAIN) scanner targeting large-cap liquid equities. They share:

- `ScannerBase` interface from `experiments/exp_engine/scanner_base.py`
- `CandidateJournalWriter` from `experiments/exp_engine/candidate_journal.py`

They differ in:

- Universe (large-cap liquid vs penny)
- Gate thresholds
- Strategy compatibility matrix
- Integration path (MAIN pipeline integrates with parity backtest and daemon watchlist; EXP does not)

The MAIN pipeline does not replace the EXP scanner. EXP remains isolated from MAIN per the canonical engine separation rule in CLAUDE.md.
