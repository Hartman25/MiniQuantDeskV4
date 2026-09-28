-- 0084 (D1, V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION):
-- durable, restart-safe, deduplicated options lifecycle activity ingestion.
--
-- Per Alpaca's official "Non-Trade Activities for Option Events" docs
-- (verified 2026-09-27): exercise/assignment/expiration are reported as
-- three distinct non-trade account activities -- OPEXC (exercise), OPASN
-- (assignment), OPEXP (OTM expiration) -- each carrying net_amount = "0"
-- (the activity itself moves no cash). The actual underlying-share
-- delivery and strike cash consideration for OPEXC/OPASN is reported
-- separately via a same-day paired OPTRD trade activity (price = strike
-- price per share, qty = underlying shares). OPEXP has no paired trade --
-- a worthless lapse delivers nothing.
--
-- This table stores each of the four raw activity types as its own
-- evidence row, keyed by Alpaca's own activity id (the natural idempotency
-- key -- an activity must never be applied twice, exactly mirroring
-- sys_crypto_fee_activity_ledger's own activity_id contract, migration
-- 0083). Pairing an OPEXC/OPASN row with its OPTRD row (by option_symbol +
-- calendar date) and applying the resulting PairedLifecycleEffect
-- (mqk_portfolio::option_lifecycle) is a separate, idempotent accounting
-- step (D2) that reads this ledger -- this migration only owns durable,
-- deduplicated raw-evidence capture.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid() -- callers supply every
-- timestamp and identity, per this repo's DB rules.

CREATE TABLE IF NOT EXISTS sys_option_lifecycle_activity_ledger (
    activity_id                 text                     NOT NULL,
    engine_id                   text                     NOT NULL,
    mode                        text                     NOT NULL,
    activity_type                text                     NOT NULL,
    option_symbol                text                     NOT NULL,
    activity_date                text                     NOT NULL,
    -- OPEXC/OPASN/OPEXP: contracts affected (signed, per Alpaca's own
    -- convention). OPTRD: underlying shares (signed: positive for
    -- exercise-side delivery, negative for assignment-side delivery).
    qty_raw                      text                     NOT NULL,
    -- OPTRD only: strike price per share. NULL for OPEXC/OPASN/OPEXP (they
    -- carry no price of their own).
    price_raw                    text                     NULL,
    ingested_at_utc              timestamp with time zone NOT NULL,
    CONSTRAINT sys_option_lifecycle_activity_ledger_pkey PRIMARY KEY (activity_id),
    CONSTRAINT sys_option_lifecycle_activity_ledger_activity_type_check
        CHECK (activity_type in ('OPEXC', 'OPASN', 'OPEXP', 'OPTRD')),
    -- Exactly OPTRD carries a price; every other type must not.
    CONSTRAINT sys_option_lifecycle_activity_ledger_price_shape_check
        CHECK (
            (activity_type = 'OPTRD' AND price_raw IS NOT NULL)
            OR (activity_type != 'OPTRD' AND price_raw IS NULL)
        )
);

CREATE INDEX IF NOT EXISTS idx_option_lifecycle_activity_ledger_engine_mode
    ON sys_option_lifecycle_activity_ledger (engine_id, mode, activity_type, ingested_at_utc);

-- Correlation lookup: given an OPEXC/OPASN row, find its paired OPTRD row
-- by (option_symbol, activity_date) without a full table scan.
CREATE INDEX IF NOT EXISTS idx_option_lifecycle_activity_ledger_symbol_date
    ON sys_option_lifecycle_activity_ledger (option_symbol, activity_date, activity_type);

CREATE TABLE IF NOT EXISTS sys_option_lifecycle_ingestion_cursor (
    engine_id                   text                     NOT NULL,
    mode                        text                     NOT NULL,
    activity_type                text                     NOT NULL,
    last_activity_id             text                     NOT NULL,
    updated_at_utc               timestamp with time zone NOT NULL,
    CONSTRAINT sys_option_lifecycle_ingestion_cursor_pkey
        PRIMARY KEY (engine_id, mode, activity_type),
    CONSTRAINT sys_option_lifecycle_ingestion_cursor_activity_type_check
        CHECK (activity_type in ('OPEXC', 'OPASN', 'OPEXP', 'OPTRD'))
);

-- Idempotent application marker: an OPEXC/OPASN/OPEXP activity_id that has
-- already had its PairedLifecycleEffect applied to the ledger/portfolio.
-- One row per applied lifecycle activity -- re-attempting apply for an
-- activity_id already present here (checked via
-- fetch_applied_option_lifecycle_effect before any mutation) is a pure
-- no-op, mirroring outbox/inbox idempotency conventions used elsewhere in
-- this schema.
CREATE TABLE IF NOT EXISTS sys_option_lifecycle_applied (
    lifecycle_activity_id        text                     NOT NULL,
    engine_id                   text                     NOT NULL,
    mode                        text                     NOT NULL,
    option_symbol                text                     NOT NULL,
    underlying_symbol            text                     NULL,
    option_contracts_removed_raw text                     NOT NULL,
    underlying_shares_delivered_raw text                  NULL,
    cash_effect_micros           bigint                   NULL,
    applied_at_utc                timestamp with time zone NOT NULL,
    CONSTRAINT sys_option_lifecycle_applied_pkey PRIMARY KEY (lifecycle_activity_id)
);
