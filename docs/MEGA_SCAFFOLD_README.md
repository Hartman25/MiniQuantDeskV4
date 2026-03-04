MiniQuantDesk – Mega Scaffold Bundle

This bundle combines:
- ML scaffold (v5): shadow intents + feature set + labeling pipeline
- Hot restart scaffold: DB lease/epoch + control plane stubs + GUI control panel stubs
- Tax reporting scaffold (v2): FIFO lots + tax drag + tax-aware metrics
- Research/Backtest LEAN-ish scaffold: experiment workspace, signal pack, consolidator, indicators, sweeps, report builder

IMPORTANT
- These are scaffolds (new files). Most are NOT wired into your repo.
- Integrate one patch at a time with compile+fmt+clippy+tests gating.

Optional CLI scripts to add (research-py/pyproject.toml)
[project.scripts]
mqk-shadow-label = "mqk_research.shadow.label_shadow_intents:main_label"
mqk-tax-fifo     = "mqk_research.tax.lot_fifo:main_fifo"
mqk-tax-drag     = "mqk_research.tax.tax_drag:main_tax_drag"
mqk-tax-metrics  = "mqk_research.tax.metrics:main_metrics"
mqk-consolidate  = "mqk_research.data.consolidate:main_consolidate"
mqk-signal-pack  = "mqk_research.signal_pack.build_signal_pack:main_build"
mqk-sweep        = "mqk_research.sweeps.run_sweep:main_sweep"
mqk-report       = "mqk_research.reporting.build_report:main_report"


Added in mega v2:
- Alpaca broker adapter scaffold crate: core-rs/crates/mqk-broker-alpaca
