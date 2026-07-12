-- 0045: Account equity baseline for daily P&L computation.
--
-- Stores a durable, provenance-tagged account-equity snapshot keyed per
-- trading day (PAPER-DAILY-PNL-BASELINE-01-COMBINED). Each row is a
-- previous-session-close (or otherwise explicitly captured) equity value
-- that /api/v1/portfolio/summary.daily_pnl compares the current equity
-- against, once a real capture has produced a row for the required prior
-- trading day. No automatic capture mechanism is added by this migration —
-- it only creates the table.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid() — callers supply
-- captured_at_utc and audit_event_id.

CREATE TABLE IF NOT EXISTS sys_account_equity_baseline (
    trading_date            date                     NOT NULL,
    equity_micros            bigint                   NOT NULL,
    cash_micros              bigint                   NOT NULL,
    currency                  text                    NOT NULL,
    captured_at_utc           timestamp with time zone NOT NULL,
    captured_by               text                    NOT NULL,
    broker_snapshot_source    text                    NOT NULL,
    audit_event_id            uuid                    NOT NULL,
    CONSTRAINT sys_account_equity_baseline_pkey PRIMARY KEY (trading_date)
);
