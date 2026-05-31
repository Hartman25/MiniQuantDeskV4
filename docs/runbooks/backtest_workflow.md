# Backtest Workflow — Operator Runbook

This runbook explains how to run backtests, where artifacts are written, how to
interpret the results, and what the benchmark comparison means.

---

## When to use a backtest

Run a backtest before any paper or live trading session to confirm that a
strategy's logic produces expected behavior on historical data. A backtest is a
**necessary but not sufficient** condition for deploying a strategy. It does not
prove future profitability.

---

## Prerequisites

- Market data loaded into DB (for DB backtest), or a CSV bar file available.
- Strategy registered in `config/strategies/` or the default MQK strategy path.
- Daemon is **not running** — backtests are offline; they do not connect to brokers.

---

## Running a CSV backtest

```
mqk backtest csv --bars <path/to/bars.csv> [--output <exports-dir>]
```

Example:

```
mqk backtest csv --bars data/bars/SPY_1min.csv --output exports/
```

The engine reads every row from the CSV, runs the strategy bar-by-bar, and
writes artifacts under `exports/<run_id>/`.

### CSV format

```
symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete
SPY,1700000060,450000000,452000000,449000000,451000000,1200,1
```

- `end_ts`: bar close time, epoch seconds (UTC).
- `*_micros`: prices in micros (1 USD = 1,000,000 micros).
- `is_complete`: `1` = complete bar; `0` = incomplete (rejected by engine).

---

## Running a DB backtest

```
mqk backtest db --symbols SPY,QQQ --start <YYYY-MM-DD> --end <YYYY-MM-DD> [--output <exports-dir>]
```

Loads bars from the market-data DB for the specified symbols and date range.
Requires `MQK_DATABASE_URL` to be set.

---

## Artifact output tree

```
exports/
└── <run_id>/
    ├── manifest.json      — run identity and file list
    ├── metrics.json       — full quantitative metrics including benchmark
    ├── report.md          — human-readable Markdown summary
    ├── orders.csv         — every order intent (filled and rejected)
    ├── fills.csv          — every fill with price and fee
    └── equity_curve.csv   — equity at each bar end timestamp
```

`run_id` is a deterministic UUID derived from strategy name + config + input bar
data.  The same input always produces the same `run_id`.

---

## Reading `metrics.json`

Key sections:

| Field | Meaning |
|---|---|
| `total_return_pct` | Strategy net return over the full run |
| `max_drawdown_pct` | Largest peak-to-trough equity decline |
| `win_rate_pct` | % of FIFO-paired round trips that were profitable |
| `profit_factor` | Gross profit / gross loss (null = no losing trades) |
| `sharpe_ratio` | Per-bar Sharpe (non-annualized; null if < 2 bars) |
| `exposure_time_pct` | % of bars with a non-zero position |
| `total_commission_micros` | Total fees paid across all fills |
| `benchmark` | Buy-and-hold comparison (see below) |

---

## Reading `report.md`

Open `report.md` in any Markdown viewer.  It contains:

- **Identity**: run_id, config_id, input_data_hash, strategy name.
- **Equity Performance**: starting/ending equity, total return, drawdown, commissions.
- **Benchmark Comparison**: buy-and-hold return, strategy return, and alpha.
- **Trade Statistics**: order/fill counts, win rate, profit factor, best/worst trade.
- **Risk-Adjusted Ratios**: Sharpe, Sortino (per-bar, non-annualized).
- **Exposure**: bars in market, time-in-market percent.
- **Assumptions & Disclaimers**: fill model, slippage, commission, benchmark method.

---

## What the buy-and-hold benchmark means

The benchmark answers: **"What would have happened if I just held the asset for
the entire backtest period?"**

Calculation:

```
first_price = open price of the first bar processed
last_price  = close price of the last bar processed
buy_and_hold_return_pct = ((last_price / first_price) - 1.0) * 100
```

**Assumptions:**

- Single-symbol benchmark only. Multi-symbol backtests show the benchmark derived
  from whichever symbol appears first in bar order.
- No commissions are applied to the benchmark (it is a gross return proxy).
- The benchmark uses first bar **open** and last bar **close** (not fill prices).

**Benchmark is absent when:**

- Fewer than 2 valid bars were processed (field is omitted from `metrics.json`).
- First bar open price is zero or negative (invalid data).

---

## What alpha means

```
alpha_pct = strategy_total_return_pct - buy_and_hold_return_pct
```

- **Positive alpha**: strategy outperformed buy-and-hold over this period.
- **Negative alpha**: strategy underperformed buy-and-hold (common with active
  trading costs on trending assets).
- Alpha is gross of benchmark commissions but net of strategy commissions.

A positive alpha in a backtest does not guarantee positive alpha in live trading.

---

## What fill and slippage assumptions mean

| Setting | Conservative default | Meaning |
|---|---|---|
| Fill model | BUY@HIGH, SELL@LOW | Worst-case price within the bar |
| `slippage_bps` | 5 bps | Flat slippage floor added to every fill |
| `volatility_mult_bps` | 5000 (50% of spread) | Extra slippage for volatile bars |
| Commission | $0.005/share | Per-share flat fee |

The conservative defaults are intentionally pessimistic.  Better-than-expected
live fill quality means real P&L could exceed the backtest — but never assume it.

---

## Parameter sweeps

A sweep runs the same bar sequence under multiple config combinations and ranks the results.
Use sweeps to **compare hypotheses**, not to blindly pick the highest-return config.

### When to use a sweep

- You have a hypothesis about how slippage or sizing affects strategy behavior.
- You want to compare a small number of discrete configs without guessing.
- You need a summary table for operator review before committing to a config.

**Do not use sweeps to auto-optimize a strategy.** Selecting the best sweep result and
deploying it is data snooping — you have trained the parameter on the same data you are
evaluating it on.

### Running a CSV sweep

```
mqk backtest csv-sweep \
  --bars <path/to/bars.csv> \
  --strategy intraday_scalper \
  --symbol SPY \
  --target-qty "1,3,5" \
  --slippage-bps "5,10" \
  --out-dir exports/sweeps/2026-05-30/
```

This runs 2×3 = 6 combinations and writes artifacts to `exports/sweeps/2026-05-30/`.

The sweep refuses grids larger than 100 combinations (use `--max-combinations` to override
if you understand the risk). An empty grid is also refused.

### Sweep artifact tree

```
exports/sweeps/2026-05-30/
├── sweep_summary.csv         — ranked table of all runs (one row per combination)
├── sweep_summary.json        — same data in JSON (schema_version: "sweep-summary-v1")
├── sweep_report.md           — Markdown table with key metrics and overfitting warning
└── <run_id>/                 — individual run artifacts (one directory per combination)
    ├── manifest.json
    ├── metrics.json
    ├── report.md
    ├── orders.csv
    ├── fills.csv
    └── equity_curve.csv
```

### Reading `sweep_summary.csv`

| Column | Meaning |
|---|---|
| `rank` | 1-based rank (1 = best by alpha then drawdown) |
| `run_id` | Deterministic UUID for this combination |
| `config_id` | Deterministic UUID for the config parameters |
| `target_qty` | Target share count for this combination |
| `slippage_bps` | Flat slippage floor in basis points |
| `volatility_mult_bps` | Spread-volatility multiplier |
| `total_return_pct` | Strategy net return |
| `alpha_pct` | Strategy return minus buy-and-hold (positive = outperformed) |
| `max_drawdown_pct` | Largest peak-to-trough decline |
| `fill_count` | Number of fills executed |
| `trade_count` | Completed FIFO round-trip trades |
| `win_rate_pct` | % of round trips with positive PnL |
| `halted` | Whether the run halted early |
| `artifact_path` | Path to the individual run artifact directory |

### Ranking method

Results are ranked by:
1. `alpha_pct` descending (higher alpha = better rank)
2. `max_drawdown_pct` ascending (lower drawdown = better rank on ties)
3. `run_id` ascending (deterministic tie-breaker)

If buy-and-hold benchmark is unavailable (< 2 valid bars), ranking falls back to
`total_return_pct` descending.

### Overfitting and data snooping warnings

- **A high-ranked sweep result is not predictive.** It tells you which config worked
  best on this specific historical period. It does not tell you which will work best
  on future data.
- Never select a config solely because it ranks first in a sweep.
- Use sweeps to **falsify hypotheses** (e.g., "higher slippage should reduce returns")
  not to maximize historical returns.
- If you run multiple sweeps on the same data, the probability of finding a spurious
  winner increases with each sweep. Treat each additional sweep as increasing
  your data-snooping risk.
- Always validate the selected config in a separate paper trading session before
  committing any live capital.

---

## Recommended pre-live workflow

1. **Refresh data** — run the premarket data refresh or ingest script.
2. **Run backtest** — `mqk backtest csv --bars ...` or `mqk backtest db ...`.
3. **Inspect `report.md`** — open it; read drawdown, alpha, trade count.
4. **Compare to benchmark** — is alpha positive? Is drawdown acceptable?
5. **Verify trade statistics** — win rate, profit factor, expectancy look reasonable?
6. **If all checks pass** — proceed to paper live smoke (not live capital).
7. **Paper smoke result** — only after a successful paper session consider live capital.

Do not skip steps. A clean backtest with bad paper smoke means the backtest was
insufficient (gap in data, overfitted parameters, data-snooping).

---

## Disclaimers

- **Backtest results do not prove future profitability.**
- Backtests are subject to lookahead bias, data snooping, and survivorship bias.
- The MQK engine enforces no-lookahead-bias (incomplete bars are rejected), but
  parameter selection bias and data selection bias are the operator's responsibility.
- Do not deploy real capital based solely on backtest results.
- Always validate with paper trading before live capital.
