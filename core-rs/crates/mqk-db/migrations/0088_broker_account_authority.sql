-- 0088 (B6/D1/D2 final exception correction, V4-M5-M8-FINAL-INDEPENDENT-REVIEW-
-- CORRECTION-02): durable broker-account authority keyed by the PROVIDER's own
-- account id, never by the API credential.
--
-- 0085/0086/0087 scoped fee/lifecycle evidence, cursors and applied markers by
-- `broker_account_id`, populated from the Alpaca APCA-API-KEY-ID. A credential
-- identifies how we authenticate, not which economic account we act on: a key
-- rotation on the same account would have manufactured a brand-new economic
-- account (fresh cursors, re-ingested evidence, un-fenced applied markers).
--
-- From here `broker_account_id` carries the canonical authority key
-- `{broker}:{provider_account_id}` (Alpaca: `GET /v2/account` `id`), and every
-- NEW row in every table below must reference a row of
-- sys_broker_account_authority that the daemon registered only after the
-- authenticated provider account endpoint proved the id.
--
-- Legacy disposition (no fabrication, no inference from the API key id): the
-- foreign keys are added NOT VALID. Postgres enforces them for every new or
-- updated row but does not re-check existing rows, so any pre-0088 row (keyed
-- by a credential id that no authority row can name) is RETAINED as
-- legacy-unverified: it stays readable for audit, can never satisfy an
-- authoritative account-scoped read/cursor/apply (those look up by authority
-- key), and can never be re-written. mqk_db::count_legacy_unverified_account_rows
-- reports them. Establishing account truth means registering the real
-- authority and re-ingesting under it; nothing is silently re-labelled.
--
-- Idempotent: safe to re-run. Does not modify 0083-0087.

CREATE TABLE IF NOT EXISTS sys_broker_account_authority (
    authority_key         text        NOT NULL,
    broker                text        NOT NULL,
    provider_account_id   text        NOT NULL,
    deployment_mode       text        NOT NULL,
    first_verified_at_utc timestamptz NOT NULL,
    CONSTRAINT sys_broker_account_authority_pkey PRIMARY KEY (authority_key),
    CONSTRAINT sys_broker_account_authority_provider_unique
        UNIQUE (broker, provider_account_id),
    CONSTRAINT sys_broker_account_authority_key_shape_check
        CHECK (authority_key = broker || ':' || provider_account_id),
    CONSTRAINT sys_broker_account_authority_nonblank_check
        CHECK (broker <> '' AND provider_account_id <> '' AND deployment_mode <> ''),
    CONSTRAINT sys_broker_account_authority_canonical_check
        CHECK (broker = lower(btrim(broker))
           AND provider_account_id = lower(btrim(provider_account_id))
           AND deployment_mode = lower(btrim(deployment_mode))
           AND position(':' in provider_account_id) = 0)
);

DO $$
DECLARE
    t text;
    c text;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'sys_crypto_fee_activity_ledger',
        'sys_crypto_fee_ingestion_cursor',
        'sys_option_lifecycle_activity_ledger',
        'sys_option_lifecycle_ingestion_cursor',
        'sys_option_lifecycle_applied'
    ] LOOP
        c := t || '_account_authority_fk';
        IF NOT EXISTS (SELECT 1 FROM pg_constraint WHERE conname = c) THEN
            EXECUTE format(
                'ALTER TABLE %I ADD CONSTRAINT %I FOREIGN KEY (broker_account_id) '
                'REFERENCES sys_broker_account_authority (authority_key) NOT VALID',
                t, c);
        END IF;
    END LOOP;
END $$;
