-- 0056: Runtime strategy conflict plans (MULTI-STRATEGY-CONFLICT-POLICY-01
-- Phase C).
--
-- Two additive tables. Does not modify any existing table.
--
-- sys_runtime_strategy_conflict_plans: one durable row per conflict-policy
-- cycle (mqk_portfolio::conflict_policy::ConflictCycleResult), written only
-- when MQK_STRATEGY_CONFLICT_POLICY_MODE is 'shadow' or 'paper_enforced' --
-- the default 'off' mode never writes here. plan_id is the deterministic
-- cycle_id (UUIDv5 of run_id + market_date + timeframe + policy schema
-- version + sorted candidate tuple set,
-- mqk-daemon::runtime_strategy_conflict::compute_conflict_cycle_id), so
-- re-persisting the same logical cycle is idempotent by construction.
--
-- sys_runtime_strategy_conflict_candidates: one row per candidate considered
-- in a plan, across every symbol group in that cycle (both buy and sell
-- candidates -- unlike Bundle 5's allocation candidates, which are
-- buy-only, this policy resolves conflicts across both sides of one
-- symbol's decisions).
--
-- This is conflict-resolution evidence, not portfolio/P&L/order truth: it
-- records which exact original candidate (if any) this policy selected per
-- symbol and why, never a fill, a broker event, or a rebuilt decision.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid() -- callers supply every
-- timestamp and identity.

CREATE TABLE IF NOT EXISTS sys_runtime_strategy_conflict_plans (
    plan_id                   uuid                     NOT NULL,
    cycle_id                  uuid                     NOT NULL,
    run_id                    uuid                     NOT NULL REFERENCES runs(run_id),
    mode                      text                     NOT NULL,
    market_date               text                     NOT NULL,
    policy_schema_version     text                     NOT NULL,
    symbol_group_count        integer                  NOT NULL,
    candidate_count           integer                  NOT NULL,
    selected_count            integer                  NOT NULL,
    refused_count             integer                  NOT NULL,
    truth_state               text                     NOT NULL,
    blockers                  text[]                   NOT NULL,
    created_at_utc            timestamp with time zone NOT NULL,
    CONSTRAINT sys_runtime_strategy_conflict_plans_pkey PRIMARY KEY (plan_id),
    CONSTRAINT sys_runtime_strategy_conflict_plans_mode_check
        CHECK (mode IN ('shadow', 'paper_enforced')),
    CONSTRAINT sys_runtime_strategy_conflict_plans_counts_check
        CHECK (
            symbol_group_count >= 0 AND candidate_count >= 0
            AND selected_count >= 0 AND refused_count >= 0
            AND selected_count <= symbol_group_count
            AND selected_count <= candidate_count
        )
);

CREATE INDEX IF NOT EXISTS idx_runtime_strategy_conflict_plans_run
    ON sys_runtime_strategy_conflict_plans (run_id, created_at_utc DESC);

CREATE TABLE IF NOT EXISTS sys_runtime_strategy_conflict_candidates (
    plan_id                   uuid    NOT NULL
        REFERENCES sys_runtime_strategy_conflict_plans(plan_id),
    ordinal                   integer NOT NULL,
    symbol                    text    NOT NULL,
    strategy_id               text    NOT NULL,
    timeframe_secs            bigint  NOT NULL,
    side                      text    NOT NULL,
    qty                       bigint  NOT NULL,
    current_qty               bigint  NOT NULL,
    proposed_target_qty       bigint  NULL,
    bar_end_ts                bigint  NULL,
    selected                  boolean NOT NULL,
    disposition               text    NOT NULL,
    reason_code               text    NOT NULL,
    CONSTRAINT sys_runtime_strategy_conflict_candidates_pkey
        PRIMARY KEY (plan_id, ordinal),
    CONSTRAINT sys_runtime_strategy_conflict_candidates_side_check
        CHECK (side IN ('buy', 'sell')),
    CONSTRAINT sys_runtime_strategy_conflict_candidates_disposition_check
        CHECK (disposition IN (
            'passthrough', 'selected', 'not_selected',
            'refused_invalid', 'refused_conflict'
        ))
);

CREATE INDEX IF NOT EXISTS idx_runtime_strategy_conflict_candidates_plan
    ON sys_runtime_strategy_conflict_candidates (plan_id, symbol, ordinal);
