-- 0055: Runtime opportunity allocation plans (RUNTIME-OPPORTUNITY-ALLOCATION-01
-- Phase G).
--
-- Two additive tables. Does not modify any existing table.
--
-- sys_runtime_opportunity_allocation_plans: one durable row per allocation
-- cycle (RUNTIME-OPPORTUNITY-ALLOCATION-01 Phase D's
-- AllocationCycleContext/AllocationCycleResult), written only when
-- MQK_RUNTIME_OPPORTUNITY_ALLOCATION_MODE is 'shadow' or 'paper_enforced' --
-- the default 'off' mode never writes here. plan_id is the deterministic
-- cycle_id (UUIDv5 of run_id + tick timestamp + sorted dispatched-symbol
-- set, mqk-daemon::runtime_opportunity_allocation::compute_cycle_id), so
-- re-persisting the same logical cycle is idempotent by construction.
--
-- sys_runtime_opportunity_allocation_candidates: one row per candidate
-- considered in a plan (new/increasing-buy decisions only -- sell/reduce/
-- exit/flatten decisions never enter this model and are never rows here).
--
-- This is allocation evidence, not portfolio/P&L/order truth: it records
-- what the allocator computed and why, never a fill, a broker event, or a
-- durable NAV/equity value of its own -- equity_micros here is copied from
-- the already-durable sys_paper_portfolio_snapshots row the cycle read (via
-- source_snapshot_id), not a second source of truth for NAV.
--
-- Scores and weights are stored as scaled integers (x1,000,000, matching
-- the codebase's existing "micros" convention) -- no binary float column.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid() -- callers supply every
-- timestamp and identity.

CREATE TABLE IF NOT EXISTS sys_runtime_opportunity_allocation_plans (
    plan_id                   uuid                     NOT NULL,
    cycle_id                  uuid                     NOT NULL,
    run_id                    uuid                     NOT NULL REFERENCES runs(run_id),
    mode                      text                     NOT NULL,
    opportunity_artifact_id   text                     NOT NULL,
    source_snapshot_id        uuid                     NULL
        REFERENCES sys_paper_portfolio_snapshots(snapshot_id),
    equity_micros             bigint                   NOT NULL,
    candidate_count           integer                  NOT NULL,
    allowed_count             integer                  NOT NULL,
    gross_weight_micros       bigint                   NOT NULL,
    net_weight_micros         bigint                   NOT NULL,
    truth_state               text                     NOT NULL,
    blockers                  text[]                   NOT NULL,
    created_at_utc            timestamp with time zone NOT NULL,
    CONSTRAINT sys_runtime_opportunity_allocation_plans_pkey PRIMARY KEY (plan_id),
    CONSTRAINT sys_runtime_opportunity_allocation_plans_mode_check
        CHECK (mode IN ('shadow', 'paper_enforced')),
    CONSTRAINT sys_runtime_opportunity_allocation_plans_counts_check
        CHECK (candidate_count >= 0 AND allowed_count >= 0 AND allowed_count <= candidate_count)
);

CREATE INDEX IF NOT EXISTS idx_runtime_opportunity_allocation_plans_run
    ON sys_runtime_opportunity_allocation_plans (run_id, created_at_utc DESC);

CREATE TABLE IF NOT EXISTS sys_runtime_opportunity_allocation_candidates (
    plan_id                   uuid    NOT NULL
        REFERENCES sys_runtime_opportunity_allocation_plans(plan_id),
    ordinal                   integer NOT NULL,
    symbol                    text    NOT NULL,
    strategy_id               text    NOT NULL,
    input_score_micros        bigint  NOT NULL,
    target_weight_micros      bigint  NOT NULL,
    current_qty               bigint  NOT NULL,
    strategy_target_qty       bigint  NOT NULL,
    allocation_target_qty     bigint  NOT NULL,
    final_target_qty          bigint  NOT NULL,
    disposition               text    NOT NULL,
    reason_code               text    NOT NULL,
    evaluation_price_micros   bigint  NOT NULL,
    CONSTRAINT sys_runtime_opportunity_allocation_candidates_pkey
        PRIMARY KEY (plan_id, symbol),
    CONSTRAINT sys_runtime_opportunity_allocation_candidates_disposition_check
        CHECK (disposition IN (
            'allowed', 'clamped_down', 'refused_no_capital', 'refused_fail_closed'
        ))
);

CREATE INDEX IF NOT EXISTS idx_runtime_opportunity_allocation_candidates_plan
    ON sys_runtime_opportunity_allocation_candidates (plan_id, ordinal);
