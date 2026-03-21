# EXP Distributed Research Backtest Engine v1

This is an EXP-only, research-side, local distributed backtest engine.

It is built to:
- define immutable research jobs
- expand deterministic batch sweeps
- execute jobs across local workers
- persist research-only batch/job/result metadata in isolated SQLite
- emit reproducible EXP-only artifacts

It is **not**:
- a MAIN engine
- a daemon service
- a broker/execution engine
- a canonical operator surface
- a readiness-bearing subsystem
- a shared DB truth source

## Boundaries

Hard boundaries for this lane:
- engine_id is always `EXP`
- outputs live only under `research-py/experiments/exp_distributed/...`
- state lives only in isolated SQLite under `experiments/exp_distributed/state/...`
- no daemon/runtime/operator/GUI truth is widened
- no canonical `mqk-db` integration exists here

## Quick start

From `research-py/`:

```bash
pip install -e .
mqk-exp-dist create-batch --spec experiments/exp_distributed/example_batch.json
mqk-exp-dist run-batch --spec experiments/exp_distributed/example_batch.json --workers 1
mqk-exp-dist batch-summary --batch-id <batch_id>
mqk-exp-dist failed-jobs --batch-id <batch_id>
mqk-exp-dist rerun-failed --batch-id <batch_id> --workers 1
python -m unittest tests.test_exp_distributed
```

Notes:
- `example_batch.json` is a self-contained EXP smoke batch that resolves its dataset relative to the spec file.
- Default artifact/state roots are anchored to `research-py/experiments/exp_distributed`, not to the caller's current working directory.
- Generic `pytest` collection is intentionally limited to `research-py/tests` so dormant experiment-local tests do not widen canonical proof collection.

## Outputs

Everything stays EXP-namespaced:
- `experiments/exp_distributed/state/exp_research.sqlite3`
- `experiments/exp_distributed/artifacts/exp_distributed/batches/<batch_id>/...`

Per job:
- `manifest.json`
- `params.json`
- `dataset_fingerprint.json`
- `summary_metrics.json`
- `daily_returns.csv`
- `positions.csv`
- `trade_events.csv`
- `status.json`
- `run.log`
- `artifact_metadata.json`

Per batch:
- `batch_manifest.json`
- `leaderboard.csv`
- `comparison_table.csv`
- `sweep_summary.json`
- `reproducibility_manifest.json`
- `aggregate_failure_report.json`

## Supported v1 strategies

- `exp.buy_hold_v1`
- `exp.cross_sectional_momentum_v1`

This is a truthful v1: local multiprocessing first, filesystem artifacts first, SQLite first, one-machine distributed first.
