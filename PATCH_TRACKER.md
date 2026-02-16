# MiniQuantDeskV4 — Patch Tracker

## Exact insert text requested (use this if you already have a tracker)

### Add to docs list
- docs/specs/strategy_evaluation_and_ranking.md
- docs/specs/config_layering_and_hashing.md
- docs/specs/testing_strategy.md
- docs/specs/testing_fixtures_and_harness.md

### Add to PATCH 01 deliverables
- Config loader (merge layers) + config_hash + validation (minimal v0)

---

Rules:
- One patch at a time.
- Each patch has explicit acceptance tests.
- No scope creep.
- No external I/O integrations (Discord/HTTP/UI) until after PATCH 12. Only interface stubs are allowed.
- Any EXPERIMENTAL engine live arming requires separate broker keys/account and remains capped by config.

Docs that define invariants:
- docs/specs/backtest_policy.md
- docs/specs/execution_model.md
- docs/specs/portfolio_and_accounting_model.md
- docs/specs/data_pipeline_and_integrity.md
- docs/specs/reconciliation.md
- docs/specs/arming_model.md
- docs/specs/event_taxonomy.md
- docs/specs/strategy_framework_and_plugin_model.md
- docs/specs/run_artifacts_and_reproducibility.md
- docs/specs/kill_switches_and_limits.md
- docs/specs/secrets_and_config_management.md
- docs/specs/broker_adapter_contract.md
- docs/specs/strategy_evaluation_and_ranking.md
- docs/specs/config_layering_and_hashing.md
- docs/specs/testing_strategy.md
- docs/specs/testing_fixtures_and_harness.md
