# exp_meanrev_v1

This folder is a fully isolated EXP-only research/backtest engine.

It is deliberately **not** wired into:
- daemon truth
- runtime orchestration
- DB semantics
- GUI/operator surfaces
- readiness proof
- live/shadow/capital authority

What it does own:
- its own config
- its own fixture bars
- its own signal generation
- its own local backtest
- its own artifact outputs under `research-py/runs/EXP/exp_meanrev_v1/...`
- its own containment checks

## Local commands

From `research-py/`:

Dry run:

```bash
python experiments/exp_meanrev_v1/run.py --allow-exp-local
```

Write EXP-only artifacts:

```bash
python experiments/exp_meanrev_v1/run.py --allow-exp-local --write-artifacts
```

Containment smoke check:

```bash
python experiments/exp_meanrev_v1/smoke_check.py
```

Experiment-local unittest:

```bash
python -m unittest experiments.exp_meanrev_v1.test_exp_meanrev_v1
```

This experiment stays local-on-purpose and is not part of generic `pytest` collection.
