-- PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE: additive durable
-- lineage columns on sys_strategy_promotion_transitions (migration 0046),
-- closing the "missing durable Research lineage" gap independent review of
-- commit 242cb7c3 found. Before this migration, the Deflated Sharpe Ratio /
-- Probability of Backtest Overfitting values that authorized a
-- shadow_approved transition were only ever logged via `tracing::info!` --
-- never durably recorded, so a later audit of *why* a strategy was
-- promoted could not answer "which Research trial, with what verified
-- DSR/PBO?" from the registry alone. There was also no column at all
-- recording which canonical Backtest evidence (`BacktestReport::run_id`,
-- PROMOTION-BACKTEST-EVIDENCE-SEAM-01) a transition was judged against.
--
-- Only identity POINTERS are persisted here (research_trial_id,
-- research_economic_eval_id, backtest_run_id) plus the two verified
-- Research statistics actually judged at transition time
-- (research_deflated_sharpe_ratio, research_probability_backtest_overfitting)
-- -- never a copy of the underlying artifact bytes. A full audit can always
-- re-resolve the complete Backtest evidence bundle via
-- `mqk_promotion::resolve_backtest_evidence` (given the daemon's trusted
-- `MQK_BACKTEST_EVIDENCE_ARTIFACT_ROOT` and this row's `backtest_run_id`)
-- and the complete Research evidence via the Research registry (given
-- `MQK_RESEARCH_REGISTRY_DB` and this row's `research_trial_id`), exactly
-- like the promotion-policy config thresholds themselves (min_sharpe,
-- max_mdd, ... -- deployment config, not per-row data, so not duplicated
-- here either).
--
-- All five columns are nullable: schema-safety only (no backfill decision
-- required for any pre-existing row) -- the route always populates all
-- five for every NEW evidence-bearing transition from this point forward.
-- No existing column, row, or constraint from migration 0046/0047/0058 is
-- altered.

alter table sys_strategy_promotion_transitions
    add column if not exists research_trial_id text,
    add column if not exists research_economic_eval_id text,
    add column if not exists research_deflated_sharpe_ratio double precision,
    add column if not exists research_probability_backtest_overfitting double precision,
    add column if not exists backtest_run_id uuid;

-- Supports lineage lookups/audits by Research trial or Backtest run.
create index if not exists sys_strategy_promotion_transitions_research_trial_idx
    on sys_strategy_promotion_transitions (research_trial_id)
    where research_trial_id is not null;

create index if not exists sys_strategy_promotion_transitions_backtest_run_idx
    on sys_strategy_promotion_transitions (backtest_run_id)
    where backtest_run_id is not null;
