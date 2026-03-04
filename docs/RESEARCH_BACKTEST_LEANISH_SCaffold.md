MiniQuantDesk – Research + Backtest "LEAN-ish" Additions (Scaffold)

Goal
- Add a small number of LEAN-like ergonomics without turning your project into LEAN.
- Everything here is optional and NOT wired into runtime/execution.

What this adds (new files only)
1) Research workspace + experiment runner skeleton
2) Signal Pack contract (research -> backtest)
3) Deterministic consolidator (resampling)
4) Small indicator library (core set)
5) Sweep runner (parameter grid)
6) Report builder (stats + artifacts, pre-tax + after-tax hooks)

All tools are in research-py to keep Rust core clean.
