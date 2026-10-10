# Strategy Factory — Pre-Edit Census (`V4-STRATEGY-FACTORY-RESEARCH-BACKTEST-FULL-COMPLETION-01`)

Baseline: `origin/main` `e190cce93672405263733ab5a10b4d9d5903adf2` (verified `git rev-parse`; the local `main` checkout was an ancestor 114 commits behind and was left untouched). Work branch `strategy-factory/full-completion-01`, worktree `MiniQuantDeskV4-StrategyFactory`, upstream deliberately unset (no accidental push to `main`). Other worktrees (`codex/local-alpha-lab`, `gui-workstation-layout-wave-02`, `integration/m1-alpha-reuse-closure-01`) are untouched.

Authority order used: code/tests/Git, then the accepted Master Program Plan §I2C (Strategy Factory roadmap), then the mission text. Dispositions below are the PRE-EDIT classification; the closure table in `STRATEGY_FACTORY_CLOSURE_01.md` carries the final dispositions.

## Existing owners (reuse, do not replace)

| Function | Owner (verified in source) |
|---|---|
| Research identity, trials, attempts, slices, judge artifacts, holdout ledger | `exp_distributed/storage.py::ResearchResultStore` |
| Bounded asset-neutral hypothesis grammar and population identity (dry-run) | `strategy_mining/grammar.py`, `population.py` |
| Hash-bound read-only catalog freeze (one pinned 200-row workbook) | `experiments/external_idea_intake/intake.py`; reviewer-authored `dispositions_data.py` |
| Native executable strategies (21 fixed-parameter identities) | `mqk-strategy/src/engines/*`, `REGISTERED_STRATEGY_IDS`, `register_builtin_strategies_with_sizing` |
| Native signal stream, economic bridge, trial identity | `ml/native_signal_registry_integration.py` + `mqk-cli backtest native-fingerprint / native-signals` |
| Stage-authorized declaration-driven batch (check/data/register/gate/trials/judge/backtest/finalize/review) | `experiments/m1_native_trend_campaign/run_batch.py`, `stage_authorization.py`, `holdout_guard.py` |
| Population-wide DSR/PBO judge | `ml/multiple_testing_judge.py` |
| Scanner/review, Promotion, Paper/Live authority | `mqk-backtest::strategy_scanner`, `mqk-promotion`, daemon gates |
| Experimental distributed Python simulator (EXP, non-canonical economics) | `exp_distributed/{runner,worker,strategies}.py` |

## Findings (pre-edit)

| ID | Finding | Pre-edit class | Planned owner of fix |
|---|---|---|---|
| F-01 | No generic catalog importer; `intake.py` is hard-pinned to one workbook (sha, 200 rows, 26 columns, `EXT-nnn`). The eight operator catalogs (Reddit xlsx+csv, Academic/Official, Shocks/Attention/Institutions, Investor Behavior/News, Technical Mechanisms, Four-Area, Psychology/Order-Flow) have heterogeneous schemas, overlapping sheets and rows that are research *diagnostics*, not strategies. | NEEDS FIX | `strategy_factory/catalog_import.py` (profile registry, fail-closed on unknown schema, every row dispositioned) |
| F-02 | Field typing (`EXPLICIT_SOURCE_RULE / INFERRED_RULE / UNDERSPECIFIED / UNKNOWN`) exists only as roadmap text; dispositions are reviewer-authored tables. | NEEDS FIX | `strategy_factory/formalize.py` |
| F-03 | No cross-campaign semantic deduplication index (native cards, historical declarations, EXT ledger, prior intake). Relationship vocabulary exists only in `disposition.py`. | NEEDS FIX | `strategy_factory/dedup.py` + `known_index.py` |
| F-04 | `strategy_mining` is identity-only; nothing maps a fingerprint to an executable. Native engines are hand-written, fixed-parameter; no grammar-driven executable. | NEEDS FIX | admission classifier + Rust `grammar_rule_v1` engine (controlled admission) |
| F-05 | Every campaign needs a hand-authored `PREDECLARED_*.json` plus bespoke tests; no generator, no queue, no resume, no scheduler, no final report assembly. | NEEDS FIX | `campaign.py`, `store.py`, `scheduler.py`, `reporting.py` |
| F-06 | `run_batch.py::stage_trials` re-evaluates every trial on rerun (new attempt per call even after a terminal success) and `trials_index.json` is rewritten non-atomically by one process: a rerun duplicates economic outcomes; concurrent per-trial writers would race. | NEEDS FIX (idempotent resume + atomic index) | `run_batch.py` minimal patch, then Factory single-writer rule |
| F-07 | Stage authorization (HMAC, <=7 days, bound to declaration identity) gates every effectful stage. An unattended campaign cannot run without an operator-minted authorization; the Factory must report this precisely and must never mint or bypass. | ALREADY CORRECT (consume, do not weaken) | executor maps refusals to `BLOCKED_AUTHORIZATION` |
| F-08 | `run_batch.SCAN_REGISTRY_SUPPLEMENT` hard-codes XBI; any other symbol absent from `config/instruments/equities.json` aborts review. | NEEDS FIX (campaign-declared identity-only rows, membership-pinned) | campaign compiler + runner |
| F-09 | `exp_distributed` is the experimental weight-return simulator; its results are not promotion-grade economics. | ALREADY CORRECT; must not be used by the Factory for economics | none (documented boundary + guard test) |
| F-10 | Local Ollama 0.40.0 serves `nomic-embed-text` (embedding) and `deepseek-coder:6.7b` (code model). No extraction-grade general LLM is configured; no torch/sklearn/scipy/openpyxl. | EXTERNAL DEPENDENCY | adapter + conformance probe; extraction backend `BLOCKED_DEPENDENCY` unless the golden extraction check passes |
| F-11 | No read-only operator visibility of Factory work. GUI has `strategyScanner`/`backtests`; daemon has `artifact_intake`, `strategy_scan_jobs`. | NEEDS FIX (extend, no second dashboard) | read-only status snapshot + daemon route + GUI panel if a minimal extension is sufficient |
| F-12 | Real data readiness: verified research bars exist locally only under the main checkout's git-ignored `runs/run_batch_0N`; fetch needs provider credentials and operator authorization. | EXTERNAL (authorization) | readiness path explains blockers; reuse of pinned existing bars is the only data action |
| F-13 | Historical per-trial `net_pnl_usd` double-counting caveat; pending holdout access incident `HOA-KISS-EXT032-01`. | KEEP / not reopened | none |
| F-14 | Provenance already distinguishes `diagnostic_synthetic` authority from `official_provider`. | ALREADY CORRECT | campaign `evidence_grade`; synthetic campaigns are structurally ineligible for review/Promotion |

## Boundaries carried from the roadmap (not re-decided here)

AI/ML may generate intelligence; deterministic MQD code retains authority. No Factory path promotes, registers a deployment, touches Paper/Live, consumes the reserved holdout, or defines identity from results. Web content is untrusted data and is never executed. The stress contract, sizing fraction, costs and economic policy are operator policy: a campaign may only reference a previously frozen protocol profile and must be authorized against its exact declaration hash.
