Tax reporting scaffold (Option B) + backtest tax drag

New files:
- research-py/src/mqk_research/tax/lot_fifo.py
- research-py/src/mqk_research/tax/tax_drag.py
- research-py/src/mqk_research/tax/contracts.py
- research-py/src/mqk_research/policies/tax_reporting_example.yaml
- TAX_REPORTING_DESIGN.md

Suggested usage (manual, offline):
1) FIFO realized trades:
   mqk-tax-fifo --fills fills.csv --out runs/<run_id>/tax/realized_trades.csv

2) Tax drag sim:
   mqk-tax-drag --realized runs/<run_id>/tax/realized_trades.csv --out-dir runs/<run_id>/tax \
     --equity-curve runs/<run_id>/equity_curve.csv --mode annual_settlement

Optional pyproject.toml scripts to add:
[project.scripts]
mqk-tax-fifo = "mqk_research.tax.lot_fifo:main_fifo"
mqk-tax-drag = "mqk_research.tax.tax_drag:main_tax_drag"


Added in v2:
- research-py/src/mqk_research/tax/metrics.py
- Optional CLI: mqk-tax-metrics = mqk_research.tax.metrics:main_metrics
- Produces tax_aware_metrics.json comparing pre-tax vs after-tax CAGR/Sharpe/MaxDD
