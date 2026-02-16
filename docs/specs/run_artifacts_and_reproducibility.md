# Run Artifacts and Reproducibility Spec (V4)

Each run stores:
- git_hash (+ dirty)
- config_hash + config_json (non-secret)
- data version ids
- execution/eval profile versions
- seed
- host fingerprint (non-sensitive)

Required exports:
- manifest.json
- audit.jsonl
- orders.csv
- fills.csv
- equity_curve.csv
- metrics.json

Replay must reproduce parity backtest outputs given same inputs.
