-- 0076: CUTOVER-1D-A1 - explicit QtyMicros encoding for runtime
-- opportunity-allocation and strategy-conflict evidence.
--
-- OPERATOR CONTRACT A:
--   quantity means actual asset quantity.
--   QtyMicros uses 1e-6 scale (1.0 unit = 1_000_000 raw micros).
--
-- Historical rows written by migrations 0055/0056 and their original
-- writers use whole-unit bigint quantity columns. Those values MUST NOT be
-- reinterpreted as QtyMicros.
--
-- Therefore this migration introduces an explicit encoding discriminator and
-- separate *_qty_micros columns. Exactly one representation is authoritative
-- per row:
--
--   quantity_schema_version IS NULL
--       => historical whole_units_v1 columns are authoritative
--
--   quantity_schema_version = 'qty_micros_v1'
--       => *_qty_micros columns are authoritative and legacy quantity
--          columns must be NULL
--
-- No historical values are rewritten or scaled.
-- No DEFAULT is supplied for quantity_schema_version.
-- Existing writers remain valid until CUTOVER-1D-A2 moves them atomically
-- onto qty_micros_v1.
--
-- This migration changes evidence representation only. It does not enable
-- Crypto execution, change risk policy, size an order, submit an order, or
-- mutate Paper/Live trading state.

-- ========================================================================
-- Bundle 5: runtime opportunity allocation
-- ========================================================================

ALTER TABLE sys_runtime_opportunity_allocation_candidates
    ADD COLUMN IF NOT EXISTS quantity_schema_version text NULL,
    ADD COLUMN IF NOT EXISTS current_qty_micros bigint NULL,
    ADD COLUMN IF NOT EXISTS strategy_target_qty_micros bigint NULL,
    ADD COLUMN IF NOT EXISTS allocation_target_qty_micros bigint NULL,
    ADD COLUMN IF NOT EXISTS final_target_qty_micros bigint NULL;

-- Legacy fields must become nullable so a qty_micros_v1 row has only one
-- quantity authority.
ALTER TABLE sys_runtime_opportunity_allocation_candidates
    ALTER COLUMN current_qty DROP NOT NULL,
    ALTER COLUMN strategy_target_qty DROP NOT NULL,
    ALTER COLUMN allocation_target_qty DROP NOT NULL,
    ALTER COLUMN final_target_qty DROP NOT NULL;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname =
            'sys_runtime_opportunity_allocation_candidates_qty_encoding_check'
          AND conrelid =
            'sys_runtime_opportunity_allocation_candidates'::regclass
    ) THEN
        ALTER TABLE sys_runtime_opportunity_allocation_candidates
            ADD CONSTRAINT
                sys_runtime_opportunity_allocation_candidates_qty_encoding_check
            CHECK (
                (
                    quantity_schema_version IS NULL

                    AND current_qty IS NOT NULL
                    AND strategy_target_qty IS NOT NULL
                    AND allocation_target_qty IS NOT NULL
                    AND final_target_qty IS NOT NULL

                    AND current_qty_micros IS NULL
                    AND strategy_target_qty_micros IS NULL
                    AND allocation_target_qty_micros IS NULL
                    AND final_target_qty_micros IS NULL
                )
                OR
                (
                    quantity_schema_version = 'qty_micros_v1'

                    AND current_qty IS NULL
                    AND strategy_target_qty IS NULL
                    AND allocation_target_qty IS NULL
                    AND final_target_qty IS NULL

                    AND current_qty_micros IS NOT NULL
                    AND strategy_target_qty_micros IS NOT NULL
                    AND allocation_target_qty_micros IS NOT NULL
                    AND final_target_qty_micros IS NOT NULL
                )
            );
    END IF;
END
$$;

CREATE INDEX IF NOT EXISTS
    idx_runtime_opportunity_allocation_candidates_qty_encoding
ON sys_runtime_opportunity_allocation_candidates (
    plan_id,
    quantity_schema_version
);

-- ========================================================================
-- Bundle 6: runtime strategy conflict
-- ========================================================================

ALTER TABLE sys_runtime_strategy_conflict_candidates
    ADD COLUMN IF NOT EXISTS quantity_schema_version text NULL,
    ADD COLUMN IF NOT EXISTS qty_micros bigint NULL,
    ADD COLUMN IF NOT EXISTS current_qty_micros bigint NULL,
    ADD COLUMN IF NOT EXISTS proposed_target_qty_micros bigint NULL;

-- `proposed_target_qty` was already nullable in 0056.
ALTER TABLE sys_runtime_strategy_conflict_candidates
    ALTER COLUMN qty DROP NOT NULL,
    ALTER COLUMN current_qty DROP NOT NULL;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname =
            'sys_runtime_strategy_conflict_candidates_qty_encoding_check'
          AND conrelid =
            'sys_runtime_strategy_conflict_candidates'::regclass
    ) THEN
        ALTER TABLE sys_runtime_strategy_conflict_candidates
            ADD CONSTRAINT
                sys_runtime_strategy_conflict_candidates_qty_encoding_check
            CHECK (
                (
                    quantity_schema_version IS NULL

                    AND qty IS NOT NULL
                    AND current_qty IS NOT NULL

                    AND qty_micros IS NULL
                    AND current_qty_micros IS NULL
                    AND proposed_target_qty_micros IS NULL
                )
                OR
                (
                    quantity_schema_version = 'qty_micros_v1'

                    AND qty IS NULL
                    AND current_qty IS NULL
                    AND proposed_target_qty IS NULL

                    AND qty_micros IS NOT NULL
                    AND current_qty_micros IS NOT NULL
                    -- proposed_target_qty_micros remains nullable because
                    -- refusal candidates may truthfully have no target.
                )
            );
    END IF;
END
$$;

CREATE INDEX IF NOT EXISTS
    idx_runtime_strategy_conflict_candidates_qty_encoding
ON sys_runtime_strategy_conflict_candidates (
    plan_id,
    quantity_schema_version
);
