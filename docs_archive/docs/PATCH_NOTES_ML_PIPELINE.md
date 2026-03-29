MiniQuantDesk – ML Research Pipeline Scaffold (LEAN-like shape)

What this zip adds (NO runtime/backtest wiring yet):
- Deterministic ML module (train/score) with feature_schema.json gate
- Signal pack exporter (runs/<run_id>/signal_pack/)
- Signal pack promoter (research-py/promoted/signal_packs/<id>/)
- Simple registry index (research-py/registry/index.jsonl)
- Example ml_policy YAML (not consumed yet by code unless you add it to your policy loader later)

Suggested pyproject.toml scripts to add (optional):
[project.scripts]
mqk-ml-train = "mqk_research.ml.train:main_train"
mqk-ml-score = "mqk_research.ml.score:main_score"
mqk-signal-pack-export = "mqk_research.signal_pack.export:main_export"
mqk-signal-pack-promote = "mqk_research.signal_pack.promote:main_promote"

Typical usage:
1) Run your existing research pipeline to produce runs/<run_id>/features.csv and targets.csv
2) mqk-ml-train --run-dir runs/<run_id>
3) mqk-ml-score --run-dir runs/<run_id> --model runs/<run_id>/ml/model_logreg_v1.json
4) mqk-signal-pack-export --run-dir runs/<run_id>
5) mqk-signal-pack-promote --run-dir runs/<run_id>

Notes:
- Determinism: feature_schema.json locks column order + file hash. If features.csv changes, training/scoring fail closed.
- Promotion: promoted signal packs are the ONLY artifact intended for later consumption by Rust.


Added in v3:
- Walk-forward evaluator: mqk_research/ml/eval_walkforward.py (writes runs/<run_id>/eval/walk_forward_eval.json)
- Promotion gating (optional): mqk-signal-pack-promote --require-eval --min-auc-mean 0.52 --min-folds-used 2


Added in v4:
- Feature generator: mqk_research/features/feature_set_v1.py
- Example feature policy: research-py/src/mqk_research/policies/feature_set_v1_example.yaml
- Optional CLI: mqk-features-v1 = mqk_research.features.feature_set_v1:main_features_v1


Added in v5:
- Shadow intents contract + labeler:
  - mqk_research/shadow/contracts.py
  - mqk_research/shadow/label_shadow_intents.py (produces targets.csv from shadow_intents.csv + bars.csv)
- Example policy: policies/shadow_label_example.yaml
- Optional CLI: mqk-shadow-label = mqk_research.shadow.label_shadow_intents:main_label
