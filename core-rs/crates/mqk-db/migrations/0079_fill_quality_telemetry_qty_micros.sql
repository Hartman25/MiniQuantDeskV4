-- 0079 (D6/A5, D7): explicit QtyMicros encoding for durable fill-quality
-- telemetry (fill_quality_telemetry) so a genuinely fractional Crypto fill
-- is captured exactly instead of being silently skipped entirely.
--
-- OPERATOR CONTRACT A:
--   quantity means actual asset/order quantity.
--   QtyMicros uses 1e-6 scale (1.0 unit = 1_000_000 raw micros).
--
-- Historical rows carry `ordered_qty`/`fill_qty` as whole-unit bigint. Those
-- values MUST NOT be reinterpreted or rescaled. Before this migration, a
-- fractional (Crypto) fill event was silently skipped by
-- mqk_runtime::orchestrator::fill_quality::build_fill_quality_row -- no row
-- was ever written for it, whole-unit or otherwise. There is therefore no
-- historical fractional row to reconcile: the historical gap is a genuine
-- absence of telemetry, not a truncated/corrupted value, and remains
-- explicitly UNKNOWN (no row) rather than fabricated.
--
-- Exactly one representation is authoritative per row:
--
--   quantity_schema_version IS NULL
--       => historical whole-unit `ordered_qty`/`fill_qty` are authoritative;
--          `ordered_qty_micros`/`fill_qty_micros` must be NULL.
--          Whole-unit fills (all Equity, and any Crypto fill that happens to
--          be a whole amount) keep being written this way.
--
--   quantity_schema_version = 'qty_micros_v1'
--       => `fill_qty_micros` is authoritative (raw QtyMicros) and `fill_qty`
--          must be NULL. `ordered_qty_micros` mirrors the same encoding for
--          the order side and is authoritative together with
--          `ordered_qty_micros IS NOT NULL AND ordered_qty IS NULL` --
--          UNLESS the order quantity was never resolvable at all (best-
--          effort outbox lookup miss), in which case both `ordered_qty` and
--          `ordered_qty_micros` are NULL (genuine absence, not a claim of a
--          zero-quantity order) while `fill_qty`/`fill_qty_micros` remain
--          governed by the fill-side rule above.
--
-- `ordered_qty` and `fill_qty` therefore become nullable; `fill_qty`'s own
-- `> 0` check moves onto the combined encoding constraint below (a NULL
-- `fill_qty` trivially satisfies a bare `fill_qty > 0` check under SQL's
-- three-valued logic, so the original inline check alone would no longer
-- reject a row with neither `fill_qty` nor `fill_qty_micros` -- the new
-- constraint closes that gap explicitly). No DEFAULT is supplied for
-- quantity_schema_version and no historical row is rewritten.

ALTER TABLE fill_quality_telemetry
    ALTER COLUMN ordered_qty DROP NOT NULL;

ALTER TABLE fill_quality_telemetry
    ALTER COLUMN fill_qty DROP NOT NULL;

ALTER TABLE fill_quality_telemetry
    DROP CONSTRAINT IF EXISTS fill_quality_telemetry_fill_qty_check;

ALTER TABLE fill_quality_telemetry
    ADD COLUMN IF NOT EXISTS quantity_schema_version text NULL,
    ADD COLUMN IF NOT EXISTS ordered_qty_micros bigint NULL,
    ADD COLUMN IF NOT EXISTS fill_qty_micros bigint NULL;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'fill_quality_telemetry_fill_qty_encoding_check'
          AND conrelid = 'fill_quality_telemetry'::regclass
    ) THEN
        ALTER TABLE fill_quality_telemetry
            ADD CONSTRAINT fill_quality_telemetry_fill_qty_encoding_check
            CHECK (
                (
                    quantity_schema_version IS NULL
                    AND fill_qty IS NOT NULL
                    AND fill_qty > 0
                    AND fill_qty_micros IS NULL
                )
                OR
                (
                    -- IS NOT NULL is explicit: `NULL = 'qty_micros_v1'` is
                    -- unknown, which a CHECK treats as satisfied.
                    quantity_schema_version IS NOT NULL
                    AND quantity_schema_version = 'qty_micros_v1'
                    AND fill_qty IS NULL
                    AND fill_qty_micros IS NOT NULL
                    AND fill_qty_micros > 0
                )
            );
    END IF;
END
$$;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'fill_quality_telemetry_ordered_qty_encoding_check'
          AND conrelid = 'fill_quality_telemetry'::regclass
    ) THEN
        ALTER TABLE fill_quality_telemetry
            ADD CONSTRAINT fill_quality_telemetry_ordered_qty_encoding_check
            CHECK (
                -- Both absent: the order quantity was never resolvable
                -- (best-effort outbox lookup miss) -- a genuine unknown, not
                -- a fabricated zero.
                (ordered_qty IS NULL AND ordered_qty_micros IS NULL)
                OR
                (ordered_qty IS NOT NULL AND ordered_qty_micros IS NULL)
                OR
                (
                    ordered_qty IS NULL
                    AND ordered_qty_micros IS NOT NULL
                    AND ordered_qty_micros > 0
                )
            );
    END IF;
END
$$;
