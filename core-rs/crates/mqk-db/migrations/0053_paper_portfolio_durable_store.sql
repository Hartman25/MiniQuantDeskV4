-- 0053: Durable paper portfolio store (DURABLE-PAPER-PORTFOLIO-AND-PNL-01B).
--
-- Three additive tables. Does not modify any existing table.
--
-- sys_paper_portfolio_snapshots / sys_paper_portfolio_snapshot_positions:
-- an append-only, deterministically-identified durable copy of an accepted
-- authoritative broker account/position snapshot (account equity/cash plus
-- per-symbol position rows). `source` distinguishes real Alpaca truth
-- ('external_alpaca') from a synthesized/diagnostic snapshot
-- ('synthetic_diagnostic') that must never be treated as authoritative
-- Paper+Alpaca portfolio truth.
--
-- sys_paper_portfolio_accounting_state: one durable row per run_id, caching
-- the result of replaying that run's already-durable, already-ordered,
-- already-deduped oms_inbox applied fills through mqk-portfolio's FIFO
-- accounting engine. This is NOT a second fill ledger -- no fill content
-- (symbol/side/qty/price) is duplicated here, only the computed cash/
-- realized-P&L/fees summary and the last_applied_inbox_id watermark that
-- makes re-application idempotent. The replay computation itself lives in
-- mqk-daemon (B4-D), which already has the oms_inbox -> mqk_portfolio::Fill
-- conversion; this table is the persistence layer only.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid() -- callers supply every
-- timestamp and identity.

CREATE TABLE IF NOT EXISTS sys_paper_portfolio_snapshots (
    snapshot_id      uuid                     NOT NULL,
    captured_at_utc  timestamp with time zone NOT NULL,
    deployment_mode  text                     NOT NULL,
    source           text                     NOT NULL,
    equity_micros    bigint                   NOT NULL,
    cash_micros      bigint                   NOT NULL,
    currency         text                     NOT NULL,
    truth_state      text                     NOT NULL,
    run_id           uuid                     NULL REFERENCES runs(run_id),
    operation_id     uuid                     NULL,
    CONSTRAINT sys_paper_portfolio_snapshots_pkey PRIMARY KEY (snapshot_id),
    CONSTRAINT sys_paper_portfolio_snapshots_source_check
        CHECK (source IN ('external_alpaca', 'synthetic_diagnostic'))
);

CREATE INDEX IF NOT EXISTS idx_paper_portfolio_snapshots_lookup
    ON sys_paper_portfolio_snapshots (deployment_mode, source, captured_at_utc DESC);

CREATE TABLE IF NOT EXISTS sys_paper_portfolio_snapshot_positions (
    snapshot_id             uuid   NOT NULL
        REFERENCES sys_paper_portfolio_snapshots(snapshot_id),
    symbol                  text   NOT NULL,
    qty_signed              bigint NOT NULL,
    avg_entry_price_micros  bigint NOT NULL,
    provenance              text   NOT NULL,
    CONSTRAINT sys_paper_portfolio_snapshot_positions_pkey
        PRIMARY KEY (snapshot_id, symbol)
);

CREATE TABLE IF NOT EXISTS sys_paper_portfolio_accounting_state (
    run_id                    uuid                     NOT NULL REFERENCES runs(run_id),
    cash_micros               bigint                   NOT NULL,
    realized_pnl_micros       bigint                   NOT NULL,
    fees_micros               bigint                   NOT NULL,
    last_applied_inbox_id     bigint                   NOT NULL,
    accounting_epoch          text                     NOT NULL,
    accounting_epoch_reason   text                     NULL,
    updated_at_utc            timestamp with time zone NOT NULL,
    CONSTRAINT sys_paper_portfolio_accounting_state_pkey PRIMARY KEY (run_id),
    CONSTRAINT sys_paper_portfolio_accounting_state_epoch_check
        CHECK (accounting_epoch IN ('complete', 'incomplete')),
    CONSTRAINT sys_paper_portfolio_accounting_state_watermark_check
        CHECK (last_applied_inbox_id >= 0)
);
