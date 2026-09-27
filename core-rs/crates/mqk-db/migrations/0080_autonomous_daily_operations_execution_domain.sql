-- 0080 (D1, B1, V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01): add the
-- canonical execution-domain component to the autonomous daily operation
-- daily-slot identity.
--
-- Before this migration, `unique (market_date, deployment_mode, adapter_id)`
-- was the only applicable-operation identity anchor. That is correct for a
-- single execution domain, but D1 requires equity_nyse and crypto_24_7 to
-- run independently on the SAME broker/deployment/day without colliding on
-- one applicable-operation slot.
--
-- Historical-row classification: every row committed before this migration
-- is Alpaca-Paper/equity-only in production -- `supports_asset_class(Crypto)`
-- on the Alpaca adapter has never returned `true` in any shipped commit (see
-- crates/mqk-broker-alpaca/src/lib.rs), so no historical operation could
-- have been a crypto_24_7 operation. A deterministic `equity_nyse`
-- historical classification is therefore proven, not guessed, and the
-- DEFAULT below is applied to every existing row by the ADD COLUMN itself
-- (no separate backfill/rescale step, no inference).
--
-- `execution_domain` becomes part of the daily-slot identity: the old
-- 3-column unique constraint is dropped and replaced by a 4-column one
-- (drop+re-add by a new name, idempotent) so equity_nyse and crypto_24_7 can
-- coexist for the same (market_date, deployment_mode, adapter_id).

ALTER TABLE sys_autonomous_daily_operations
    ADD COLUMN IF NOT EXISTS execution_domain text NOT NULL DEFAULT 'equity_nyse';

-- The DEFAULT above only applies to the ADD COLUMN statement itself and to
-- future inserts that omit the column; strip it so a future caller cannot
-- silently omit execution_domain (D1: "must distinguish execution domain"
-- -- every new row must supply it explicitly, mirroring this repo's no-
-- DEFAULT-now()/no-fabricated-DEFAULT convention for every other column on
-- this table).
ALTER TABLE sys_autonomous_daily_operations
    ALTER COLUMN execution_domain DROP DEFAULT;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'sys_autonomous_daily_operations_execution_domain_known'
          AND conrelid = 'sys_autonomous_daily_operations'::regclass
    ) THEN
        ALTER TABLE sys_autonomous_daily_operations
            ADD CONSTRAINT sys_autonomous_daily_operations_execution_domain_known
            CHECK (execution_domain in ('equity_nyse', 'crypto_24_7'));
    END IF;
END
$$;

ALTER TABLE sys_autonomous_daily_operations
    DROP CONSTRAINT IF EXISTS sys_autonomous_daily_operations_daily_slot_unique;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'sys_autonomous_daily_operations_daily_slot_domain_unique'
          AND conrelid = 'sys_autonomous_daily_operations'::regclass
    ) THEN
        ALTER TABLE sys_autonomous_daily_operations
            ADD CONSTRAINT sys_autonomous_daily_operations_daily_slot_domain_unique
            UNIQUE (market_date, deployment_mode, adapter_id, execution_domain);
    END IF;
END
$$;
