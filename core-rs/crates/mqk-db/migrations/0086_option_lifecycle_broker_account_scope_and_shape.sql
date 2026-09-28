-- 0086 (D1 correction, V4-M5-M8-INDEPENDENT-REVIEW-CORRECTION-01): scope
-- options-lifecycle raw evidence by durable broker/account identity, fix
-- the activity_id PRIMARY KEY collision between a lifecycle activity and
-- its paired trade, and stop conflating an OPTRD row's underlying symbol
-- with an OPEXC/OPASN/OPEXP row's OCC option-contract symbol.
--
-- Per Alpaca's official "Non-Trade Activities for Option Events" docs
-- (re-verified live via Context7 during this correction): an OPEXC/OPASN
-- activity and its paired OPTRD activity are reported with the IDENTICAL
-- `id` value -- e.g.
--   {"id": "20190801011955195::5f596936-...", "activity_type": "OPEXC", ...}
--   {"id": "20190801011955195::5f596936-...", "activity_type": "OPTRD", ...}
-- 0084's PRIMARY KEY (activity_id) alone cannot hold both rows: inserting
-- the OPTRD row after its OPEXC/OPASN sibling silently no-ops on
-- ON CONFLICT (activity_id) DO NOTHING, permanently losing the paired
-- trade's strike price and share quantity -- the exact evidence D2's
-- accounting depends on. This shared id is also the correct,
-- provider-documented correlation key between a lifecycle activity and its
-- paired trade -- superseding 0084's (option_symbol, activity_date) lookup,
-- which additionally depended on OPTRD.symbol meaning "the option
-- contract" when Alpaca's own docs show OPTRD.symbol is the UNDERLYING
-- ticker (e.g. "AAPL"), not the OCC contract.
--
-- Fixes:
-- - broker_account_id (the authenticated Alpaca APCA-API-KEY-ID) added to
--   the ledger and cursor, mirroring migration 0085's identical B6 fix.
-- - PRIMARY KEY widened to (broker_account_id, activity_id, activity_type)
--   on the ledger -- a lifecycle activity and its identically-id'd paired
--   trade now coexist as two distinct rows, disambiguated by type.
-- - option_symbol becomes NULLable: NOT NULL (the OCC contract) for
--   OPEXC/OPASN/OPEXP, NULL for OPTRD, enforced by a CHECK mirroring
--   0084's existing price-shape CHECK exactly.
-- - underlying_symbol_raw added: NULL for OPEXC/OPASN/OPEXP, NOT NULL (the
--   raw Alpaca `symbol` field, the underlying ticker) for OPTRD.
-- - net_amount_raw added, NOT NULL always -- Alpaca's own authoritative
--   signed cash evidence (always "0" for OPEXC/OPASN/OPEXP; the real
--   signed strike consideration for OPTRD), preserved as reported so D2
--   never has to manufacture a cash effect from strike*contracts*multiplier
--   alone.
--
-- Idempotent: safe to re-run, every change existence-guarded. ADD COLUMN
-- ... NOT NULL carries no DEFAULT -- fails loudly rather than fabricating
-- identity/evidence for any pre-existing row (no production caller has
-- ever driven either table -- 0084's own manifest intent records this).

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE table_name = 'sys_option_lifecycle_activity_ledger'
           AND column_name = 'broker_account_id'
    ) THEN
        ALTER TABLE sys_option_lifecycle_activity_ledger
            ADD COLUMN broker_account_id text NOT NULL;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE table_name = 'sys_option_lifecycle_activity_ledger'
           AND column_name = 'underlying_symbol_raw'
    ) THEN
        ALTER TABLE sys_option_lifecycle_activity_ledger
            ADD COLUMN underlying_symbol_raw text NULL;
    END IF;
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE table_name = 'sys_option_lifecycle_activity_ledger'
           AND column_name = 'net_amount_raw'
    ) THEN
        ALTER TABLE sys_option_lifecycle_activity_ledger
            ADD COLUMN net_amount_raw text NOT NULL;
    END IF;
END $$;

ALTER TABLE sys_option_lifecycle_activity_ledger
    ALTER COLUMN option_symbol DROP NOT NULL;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'sys_option_lifecycle_activity_ledger_pkey'
    ) THEN
        ALTER TABLE sys_option_lifecycle_activity_ledger
            DROP CONSTRAINT sys_option_lifecycle_activity_ledger_pkey;
    END IF;
END $$;

ALTER TABLE sys_option_lifecycle_activity_ledger
    ADD CONSTRAINT sys_option_lifecycle_activity_ledger_pkey
        PRIMARY KEY (broker_account_id, activity_id, activity_type);

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'sys_option_lifecycle_activity_ledger_symbol_shape_check'
    ) THEN
        ALTER TABLE sys_option_lifecycle_activity_ledger
            DROP CONSTRAINT sys_option_lifecycle_activity_ledger_symbol_shape_check;
    END IF;
END $$;

ALTER TABLE sys_option_lifecycle_activity_ledger
    ADD CONSTRAINT sys_option_lifecycle_activity_ledger_symbol_shape_check
        CHECK (
            (activity_type != 'OPTRD'
                AND option_symbol IS NOT NULL AND underlying_symbol_raw IS NULL)
            OR (activity_type = 'OPTRD'
                AND option_symbol IS NULL AND underlying_symbol_raw IS NOT NULL)
        );

DROP INDEX IF EXISTS idx_option_lifecycle_activity_ledger_engine_mode;
CREATE INDEX IF NOT EXISTS idx_option_lifecycle_activity_ledger_account_engine_mode
    ON sys_option_lifecycle_activity_ledger
        (broker_account_id, engine_id, mode, activity_type, ingested_at_utc);

-- The old (option_symbol, activity_date, activity_type) correlation index
-- is superseded by activity_id-based correlation (the pairing lookup now
-- filters on the PRIMARY KEY's own leading columns); D3's pending-gate
-- lookup by option_symbol still benefits from an account-scoped index.
DROP INDEX IF EXISTS idx_option_lifecycle_activity_ledger_symbol_date;
CREATE INDEX IF NOT EXISTS idx_option_lifecycle_activity_ledger_account_symbol
    ON sys_option_lifecycle_activity_ledger
        (broker_account_id, option_symbol, activity_type);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE table_name = 'sys_option_lifecycle_ingestion_cursor'
           AND column_name = 'broker_account_id'
    ) THEN
        ALTER TABLE sys_option_lifecycle_ingestion_cursor
            ADD COLUMN broker_account_id text NOT NULL;
    END IF;
END $$;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'sys_option_lifecycle_ingestion_cursor_pkey'
    ) THEN
        ALTER TABLE sys_option_lifecycle_ingestion_cursor
            DROP CONSTRAINT sys_option_lifecycle_ingestion_cursor_pkey;
    END IF;
END $$;

ALTER TABLE sys_option_lifecycle_ingestion_cursor
    ADD CONSTRAINT sys_option_lifecycle_ingestion_cursor_pkey
        PRIMARY KEY (broker_account_id, engine_id, mode, activity_type);
