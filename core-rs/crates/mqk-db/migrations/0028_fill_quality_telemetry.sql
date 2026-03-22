-- TV-EXEC-01: Durable fill-quality telemetry.
--
-- Each row captures the authoritative truth for one broker fill event:
-- actual execution price, timing, reference price, slippage, and provenance.
--
-- `telemetry_id` is UUIDv5(NAMESPACE_DNS, "mqk.fill-quality.v1|{run_id}|{broker_message_id}")
-- — deterministic so best-effort writes from the orchestrator are idempotent.
--
-- `fill_kind` is 'partial_fill' or 'final_fill'.
-- `reference_price_micros` is the limit_price stored in the outbox row (i64 micros).
--   NULL for market orders — slippage is undefined and must not be fabricated.
-- `slippage_bps` is computed only when reference_price_micros is non-NULL.
-- `submit_ts_utc` is outbox.sent_at_utc — NULL if the outbox row is absent or not yet sent.
-- `submit_to_fill_ms` is derived when both submit_ts_utc and fill_received_at_utc exist.
-- `provenance_ref` is always 'oms_inbox:{broker_message_id}'.

create table if not exists fill_quality_telemetry (
    telemetry_id            uuid         primary key,
    run_id                  uuid         not null references runs(run_id) on delete cascade,
    internal_order_id       text         not null,
    broker_order_id         text,
    broker_fill_id          text,
    broker_message_id       text         not null,
    symbol                  text         not null,
    side                    text         not null check (side in ('buy', 'sell')),
    ordered_qty             bigint       not null,
    fill_qty                bigint       not null check (fill_qty > 0),
    fill_price_micros       bigint       not null,
    reference_price_micros  bigint,
    slippage_bps            bigint,
    submit_ts_utc           timestamptz,
    fill_received_at_utc    timestamptz  not null,
    submit_to_fill_ms       bigint,
    fill_kind               text         not null check (fill_kind in ('partial_fill', 'final_fill')),
    provenance_ref          text         not null,
    created_at_utc          timestamptz  not null
);

-- Most-recent-N query pattern used by GET /api/v1/execution/fill-quality.
create index if not exists fill_quality_telemetry_run_received_idx
    on fill_quality_telemetry (run_id, fill_received_at_utc desc);
