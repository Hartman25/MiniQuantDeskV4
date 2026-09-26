-- 0078: explicit QtyMicros encoding for durable Paper portfolio snapshot
-- positions (M5/M6 asset-neutral quantity; no trading behavior change).
--
-- OPERATOR CONTRACT A:
--   quantity means actual asset quantity.
--   QtyMicros uses 1e-6 scale (1.0 unit = 1_000_000 raw micros).
--
-- Historical rows carry `qty_signed` as a whole-unit bigint. Those values MUST
-- NOT be reinterpreted or rescaled. A fractional (Crypto) broker position
-- cannot be represented there and must never be truncated, rounded, or
-- stored as an absent quantity.
--
-- Exactly one representation is authoritative per row:
--
--   quantity_schema_version IS NULL
--       => historical whole-unit `qty_signed` is authoritative;
--          `qty_signed_micros` must be NULL.
--          Whole-unit positions (all Equity) keep being written this way.
--
--   quantity_schema_version = 'qty_micros_v1'
--       => `qty_signed_micros` is authoritative (raw QtyMicros) and
--          `qty_signed` must be NULL.
--
-- `qty_signed` therefore becomes nullable; the CHECK below keeps "no quantity
-- at all" impossible. No DEFAULT is supplied for quantity_schema_version and no
-- historical row is rewritten.

ALTER TABLE sys_paper_portfolio_snapshot_positions
    ALTER COLUMN qty_signed DROP NOT NULL;

ALTER TABLE sys_paper_portfolio_snapshot_positions
    ADD COLUMN IF NOT EXISTS quantity_schema_version text NULL,
    ADD COLUMN IF NOT EXISTS qty_signed_micros bigint NULL;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'sys_paper_portfolio_snapshot_positions_qty_encoding_check'
          AND conrelid = 'sys_paper_portfolio_snapshot_positions'::regclass
    ) THEN
        ALTER TABLE sys_paper_portfolio_snapshot_positions
            ADD CONSTRAINT sys_paper_portfolio_snapshot_positions_qty_encoding_check
            CHECK (
                (
                    quantity_schema_version IS NULL
                    AND qty_signed IS NOT NULL
                    AND qty_signed_micros IS NULL
                )
                OR
                (
                    -- IS NOT NULL is explicit: `NULL = 'qty_micros_v1'` is
                    -- unknown, which a CHECK treats as satisfied.
                    quantity_schema_version IS NOT NULL
                    AND quantity_schema_version = 'qty_micros_v1'
                    AND qty_signed IS NULL
                    AND qty_signed_micros IS NOT NULL
                )
            );
    END IF;
END
$$;
