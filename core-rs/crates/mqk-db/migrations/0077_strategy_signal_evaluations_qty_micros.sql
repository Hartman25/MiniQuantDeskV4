-- 0077: CUTOVER-1D-A3 - explicit QtyMicros encoding for the durable
-- strategy signal-evaluation journal.
--
-- OPERATOR CONTRACT A:
--   quantity means actual asset quantity.
--   QtyMicros uses 1e-6 scale (1.0 unit = 1_000_000 raw micros).
--
-- Historical rows carry `signal_qty` as a whole-unit bigint. Those values MUST
-- NOT be reinterpreted or rescaled. A successfully evaluated fractional signal
-- cannot be represented there and must never be journaled as an absent (NULL)
-- quantity, which is reserved for "on_bar never ran".
--
-- Exactly one representation is authoritative per row:
--
--   quantity_schema_version IS NULL
--       => historical whole-unit `signal_qty` is authoritative
--          (NULL there means the quantity is genuinely absent, e.g. a
--          pre-dispatch gate refused before on_bar ran);
--          `signal_qty_micros` must be NULL
--
--   quantity_schema_version = 'qty_micros_v1'
--       => `signal_qty_micros` is authoritative and `signal_qty` must be NULL.
--          A NULL `signal_qty_micros` on such a row means on_bar ran but the
--          summed target total overflowed (no exact quantity exists).
--
-- No historical value is rewritten. No DEFAULT is supplied for
-- quantity_schema_version. This migration changes evidence representation
-- only; it does not enable Crypto execution or change any trading behavior.

ALTER TABLE strategy_signal_evaluations
    ADD COLUMN IF NOT EXISTS quantity_schema_version text NULL,
    ADD COLUMN IF NOT EXISTS signal_qty_micros bigint NULL;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'strategy_signal_evaluations_qty_encoding_check'
          AND conrelid = 'strategy_signal_evaluations'::regclass
    ) THEN
        ALTER TABLE strategy_signal_evaluations
            ADD CONSTRAINT strategy_signal_evaluations_qty_encoding_check
            CHECK (
                (
                    quantity_schema_version IS NULL
                    AND signal_qty_micros IS NULL
                )
                OR
                (
                    -- IS NOT NULL is explicit: `NULL = 'qty_micros_v1'` is
                    -- unknown, which a CHECK treats as satisfied.
                    quantity_schema_version IS NOT NULL
                    AND quantity_schema_version = 'qty_micros_v1'
                    AND signal_qty IS NULL
                )
            );
    END IF;
END
$$;
