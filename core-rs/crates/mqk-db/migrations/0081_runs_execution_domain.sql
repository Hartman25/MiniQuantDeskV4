-- 0081 (B2, V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01): add the canonical
-- execution-domain component to `runs` identity.
--
-- Before this migration, `runs` had no notion of which execution domain a
-- run belonged to, and the single-active-run gate
-- (`fetch_active_run_for_engine` keyed on `(engine_id, mode)`) admitted at
-- most one ARMED/RUNNING run system-wide. B2 requires equity_nyse and
-- crypto_24_7 to each hold independent, durable start/stop/runtime-ownership
-- state -- two runs, one per domain, coexisting for the same
-- (engine_id, mode) without either being able to observe or block the
-- other.
--
-- Historical-row classification: identical justification to migration 0080
-- (D1/B1) -- the Alpaca adapter's `supports_asset_class(Crypto)` has never
-- returned `true` in any shipped commit, so no historical run could have
-- been a crypto_24_7 run. Every existing row is deterministically
-- `equity_nyse`, proven not guessed. The DEFAULT is dropped immediately
-- after backfilling existing rows so every future insert must supply it
-- explicitly (mirrors this table's existing no-DEFAULT-now() convention for
-- every other column).

ALTER TABLE runs
    ADD COLUMN IF NOT EXISTS execution_domain text NOT NULL DEFAULT 'equity_nyse';

ALTER TABLE runs
    ALTER COLUMN execution_domain DROP DEFAULT;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'runs_execution_domain_known'
          AND conrelid = 'runs'::regclass
    ) THEN
        ALTER TABLE runs
            ADD CONSTRAINT runs_execution_domain_known
            CHECK (execution_domain in ('equity_nyse', 'crypto_24_7'));
    END IF;
END
$$;

-- Supports the single-active-run-per-domain gate
-- (`fetch_active_run_for_engine`/`fetch_latest_run_for_engine`), which now
-- filters on `(engine_id, mode, execution_domain)`.
CREATE INDEX IF NOT EXISTS runs_engine_mode_domain_idx
    ON runs (engine_id, mode, execution_domain);
