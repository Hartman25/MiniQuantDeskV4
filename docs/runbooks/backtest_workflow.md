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
