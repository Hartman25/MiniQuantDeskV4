-- 0057: Runtime strategy conflict evidence provenance repair
-- (MULTI-STRATEGY-CONFLICT-POLICY-01-AUTHORITY-AND-EVIDENCE-REPAIR Defect 5).
--
-- Additive only. Does not modify, rename, or drop any column, constraint, or
-- row created by 0056. 0056 stored only partial candidate provenance (no
-- bar symbol/strategy/timeframe, no order semantics, no requested/effective
-- mode distinction). This migration adds the missing columns so future
-- evidence is fully self-proving; it never fabricates missing provenance
-- for rows written before this migration -- every new column is nullable
-- and every pre-existing row round-trips the new columns as NULL, which the
-- read-side evidence validator (routes/strategy_conflict.rs) must treat as
-- legacy/incomplete evidence, never as active current truth.
--
-- sys_runtime_strategy_conflict_plans:
--   configured_mode: the mode as configured/requested before the paper+
--   Alpaca live-lock was applied (mode remains the already live-lock-
--   resolved effective mode). NULL on 0056-era rows.
--   plan_id/cycle_id consistency is now a DB-enforced invariant, not just
--   an application convention -- both fields have always been set equal by
--   construction (mqk-daemon::runtime_strategy_conflict::plan_to_new_db_plan),
--   so this CHECK is safe to add without a backfill.
--
-- sys_runtime_strategy_conflict_candidates:
--   order_type / time_in_force / limit_price: order semantics used in the
--   cycle economic identity (compute_conflict_cycle_id), now also
--   persisted as durable evidence. NULL on 0056-era rows.
--   bar_present / bar_symbol / bar_strategy_id / bar_timeframe /
--   close_micros: full candidate bar provenance and presence state,
--   closing the "sell candidates never had bar facts" evidence gap
--   (bar_end_ts already existed in 0056; the rest did not).
--   bar_present is an explicit tri-state (NULL = unknown/legacy row, not
--   "false") so "provenance was never recorded" and "provenance was
--   recorded absent" stay distinguishable.

ALTER TABLE sys_runtime_strategy_conflict_plans
    ADD COLUMN IF NOT EXISTS configured_mode text NULL;

ALTER TABLE sys_runtime_strategy_conflict_plans
    ADD CONSTRAINT sys_runtime_strategy_conflict_plans_configured_mode_check
        CHECK (configured_mode IS NULL OR configured_mode IN ('off', 'shadow', 'paper_enforced'));

ALTER TABLE sys_runtime_strategy_conflict_plans
    ADD CONSTRAINT sys_runtime_strategy_conflict_plans_plan_cycle_consistency_check
        CHECK (plan_id = cycle_id);

ALTER TABLE sys_runtime_strategy_conflict_candidates
    ADD COLUMN IF NOT EXISTS order_type text NULL,
    ADD COLUMN IF NOT EXISTS time_in_force text NULL,
    ADD COLUMN IF NOT EXISTS limit_price bigint NULL,
    ADD COLUMN IF NOT EXISTS bar_present boolean NULL,
    ADD COLUMN IF NOT EXISTS bar_symbol text NULL,
    ADD COLUMN IF NOT EXISTS bar_strategy_id text NULL,
    ADD COLUMN IF NOT EXISTS bar_timeframe text NULL,
    ADD COLUMN IF NOT EXISTS close_micros bigint NULL;

ALTER TABLE sys_runtime_strategy_conflict_candidates
    ADD CONSTRAINT sys_runtime_strategy_conflict_candidates_order_type_check
        CHECK (order_type IS NULL OR order_type IN ('market', 'limit'));

ALTER TABLE sys_runtime_strategy_conflict_candidates
    ADD CONSTRAINT sys_runtime_strategy_conflict_candidates_tif_check
        CHECK (time_in_force IS NULL OR time_in_force IN ('day', 'gtc', 'ioc', 'fok'));

-- Coherence: when bar_present is recorded (not NULL/legacy), it must agree
-- with the actual presence of every bar-identity field. A NULL bar_present
-- (legacy 0056 row) trivially satisfies this CHECK (SQL NULL comparisons
-- are neither true nor false, never blocking the constraint).
ALTER TABLE sys_runtime_strategy_conflict_candidates
    ADD CONSTRAINT sys_runtime_strategy_conflict_candidates_bar_present_coherence_check
        CHECK (
            bar_present IS NULL
            OR bar_present = (
                bar_symbol IS NOT NULL
                AND bar_strategy_id IS NOT NULL
                AND bar_timeframe IS NOT NULL
                AND bar_end_ts IS NOT NULL
                AND close_micros IS NOT NULL
            )
        );
