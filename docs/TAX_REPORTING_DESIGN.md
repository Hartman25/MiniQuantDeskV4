MiniQuantDesk – Tax Reporting (Option B) + Backtest Tax Drag (Scaffold)

Scope (intentionally limited)
- Reporting-only (offline). This must NOT be on the live execution path.
- FIFO lot tracking + holding period classification (short vs long).
- Estimated tax drag simulation for backtests using realized trades.
- No wash sale rules, no corporate actions, no cross-account transfers, no options tax rules.

Inputs (flexible CSV contracts)
1) fills.csv with columns:
   - symbol (str)
   - fill_ts (UTC timestamp string)
   - side ("BUY" or "SELL")
   - qty (float/int, positive)
   - price (float)
   - fee (float, optional; default 0)

2) Optional: equity_curve.csv with columns:
   - ts (UTC timestamp string)
   - equity (float)

Outputs
- realized_trades.csv (one row per closed lot slice)
- realized_trades_meta.json
- tax_summary.json (per-year + totals)
- equity_after_tax.csv (if equity_curve.csv provided)
- tax_drag_summary.json (headline tax drag metrics)
- tax_drag_meta.json

Tax drag simulation modes
A) annual_settlement (recommended default)
   - Taxes applied once per calendar year based on realized gains that year.
B) immediate_withholding
   - Taxes deducted at each sale timestamp (conservative).

Safety notes
- Estimator only. Use broker 1099-B + tax software/CPA for filing.
