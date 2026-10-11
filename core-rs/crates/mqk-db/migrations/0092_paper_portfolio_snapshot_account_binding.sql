-- 0092: durable provider-account binding for accepted Paper portfolio snapshots.
--
-- sys_paper_portfolio_snapshots (0053) records equity/cash/positions but not
-- WHICH provider account produced them, so after a credential or account change
-- durable portfolio truth cannot be attributed to an account. This additive
-- table binds a snapshot to a registered sys_broker_account_authority row
-- (0088), set once and only for snapshots whose capturing GET /v2/account
-- observation proved the account id.
--
-- Legacy / unproven disposition (no fabrication): snapshots persisted before
-- this migration, or whose account observation could not be proven, simply have
-- no row here and read as account-unverified. Nothing is inferred or backfilled.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid(); callers supply every value.
-- Idempotent: CREATE TABLE IF NOT EXISTS. Does not modify any earlier migration.

CREATE TABLE IF NOT EXISTS sys_paper_portfolio_snapshot_account (
    snapshot_id        uuid        NOT NULL
        REFERENCES sys_paper_portfolio_snapshots(snapshot_id),
    broker_account_id  text        NOT NULL
        REFERENCES sys_broker_account_authority(authority_key),
    bound_at_utc       timestamptz NOT NULL,
    CONSTRAINT sys_paper_portfolio_snapshot_account_pkey PRIMARY KEY (snapshot_id)
);

CREATE INDEX IF NOT EXISTS idx_paper_portfolio_snapshot_account_by_account
    ON sys_paper_portfolio_snapshot_account (broker_account_id);
