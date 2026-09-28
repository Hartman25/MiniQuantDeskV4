-- 0087 (D2 correction, V4-M5-M8-INDEPENDENT-REVIEW-CORRECTION-01): scope
-- the options-lifecycle idempotent-apply marker by durable broker/account
-- identity, mirroring 0085 (B6)/0086 (D1) exactly.
--
-- sys_option_lifecycle_applied's PRIMARY KEY (lifecycle_activity_id) alone
-- assumed Alpaca activity ids are globally unique across every broker
-- account this daemon could ever connect to -- the same class of gap 0085/
-- 0086 already closed for the fee ledger and the raw lifecycle ledger.
-- Two distinct accounts whose lifecycle activity ids happened to collide
-- would share one applied-marker row, letting account A's applied effect
-- silently gate account B's genuinely distinct lifecycle event as
-- "AlreadyApplied".
--
-- Idempotent: safe to re-run, existence-guarded. ADD COLUMN ... NOT NULL
-- carries no DEFAULT -- fails loudly rather than fabricating an account
-- identity for any pre-existing row (no production caller has ever driven
-- this table -- 0084's own manifest intent records this).

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM information_schema.columns
         WHERE table_name = 'sys_option_lifecycle_applied'
           AND column_name = 'broker_account_id'
    ) THEN
        ALTER TABLE sys_option_lifecycle_applied
            ADD COLUMN broker_account_id text NOT NULL;
    END IF;
END $$;

DO $$
BEGIN
    IF EXISTS (
        SELECT 1 FROM pg_constraint WHERE conname = 'sys_option_lifecycle_applied_pkey'
    ) THEN
        ALTER TABLE sys_option_lifecycle_applied
            DROP CONSTRAINT sys_option_lifecycle_applied_pkey;
    END IF;
END $$;

ALTER TABLE sys_option_lifecycle_applied
    ADD CONSTRAINT sys_option_lifecycle_applied_pkey
        PRIMARY KEY (broker_account_id, lifecycle_activity_id);
