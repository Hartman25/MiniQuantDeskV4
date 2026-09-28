-- 0085 (B6 correction, V4-M5-M8-INDEPENDENT-REVIEW-CORRECTION-01): scope
-- Crypto fee-activity ledger + cursor by durable broker/account identity,
-- not merely (engine_id, mode, activity_type).
--
-- 0083's PRIMARY KEY (activity_id) alone assumed Alpaca activity ids are
-- globally unique across every broker account this daemon could ever
-- connect to. They are not: Alpaca scopes activity ids to one account, so
-- the identical id can appear under two different accounts. Without an
-- account-scoped key, a second account's genuinely-new activity sharing an
-- id with a first account's already-ingested activity would silently
-- collide on ON CONFLICT (activity_id) DO NOTHING and be dropped as a false
-- duplicate. The paired cursor (keyed only (engine_id, mode, activity_type))
-- has the same gap: two accounts sharing one engine_id/mode label (e.g. two
-- operator credential sets, or a credential rotation) would read and
-- advance one shared watermark, letting account A's fetch position gate
-- account B's evidence.
--
-- broker_account_id is the authenticated account's own Alpaca
-- APCA-API-KEY-ID (never the secret key) -- the natural, already-available
-- raw identity for "which Alpaca account", supplied by the caller from its
-- authenticated transport context (mirrors mqk-broker-ibkr::identity's
-- account_id pattern; a natural raw identity, not a derived hash).
--
-- Idempotent: safe to re-run. Every column/constraint change below is
-- existence-guarded. ADD COLUMN ... NOT NULL is issued without a DEFAULT --
-- if either table already carries a row from before this migration (no
-- production caller has ever driven either table -- 0083/0084's own
-- manifest intent record this -- so in every real deployment both tables
-- are empty), this migration fails loudly rather than fabricating an
-- account identity for evidence this migration cannot honestly attribute.

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE table_name = 'sys_crypto_fee_activity_ledger'
           AND column_name = 'broker_account_id'
    ) THEN
        ALTER TABLE sys_crypto_fee_activity_ledger
            ADD COLUMN broker_account_id text NOT NULL;
    END IF;
END $$;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'sys_crypto_fee_activity_ledger_pkey'
    ) THEN
        ALTER TABLE sys_crypto_fee_activity_ledger
            DROP CONSTRAINT sys_crypto_fee_activity_ledger_pkey;
    END IF;
END $$;

ALTER TABLE sys_crypto_fee_activity_ledger
    ADD CONSTRAINT sys_crypto_fee_activity_ledger_pkey
        PRIMARY KEY (broker_account_id, activity_id);

DROP INDEX IF EXISTS idx_crypto_fee_activity_ledger_engine_mode;

CREATE INDEX IF NOT EXISTS idx_crypto_fee_activity_ledger_account_engine_mode
    ON sys_crypto_fee_activity_ledger
        (broker_account_id, engine_id, mode, activity_type, ingested_at_utc);

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE table_name = 'sys_crypto_fee_ingestion_cursor'
           AND column_name = 'broker_account_id'
    ) THEN
        ALTER TABLE sys_crypto_fee_ingestion_cursor
            ADD COLUMN broker_account_id text NOT NULL;
    END IF;
END $$;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'sys_crypto_fee_ingestion_cursor_pkey'
    ) THEN
        ALTER TABLE sys_crypto_fee_ingestion_cursor
            DROP CONSTRAINT sys_crypto_fee_ingestion_cursor_pkey;
    END IF;
END $$;

ALTER TABLE sys_crypto_fee_ingestion_cursor
    ADD CONSTRAINT sys_crypto_fee_ingestion_cursor_pkey
        PRIMARY KEY (broker_account_id, engine_id, mode, activity_type);
