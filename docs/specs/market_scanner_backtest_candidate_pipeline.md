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

---

## 11. Off-Hours Backtest Stage

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
| 13 | PAPER-HANDOFF-01 | Autonomous paper handoff: daemon reads watchlist, signal filter |
| 14 | GUI-SCANNER-01 | GUI/Discord surfaces for scanner/watchlist state |
| 15 | EVIDENCE-REVIEW-01 | Post-session evidence review integration |
| 16 | PAPER-REPEAT-01 | Repeated paper validation tracking (N sessions gate) |
| 17 | LIVE-SHADOW-LATER | Live-shadow promotion path — future; not started in this roadmap |

Patches 2–13 target the `research-py` layer and script runners. Patches 14–16 touch the daemon GUI and evidence tooling. Patch 17 is deferred.

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
