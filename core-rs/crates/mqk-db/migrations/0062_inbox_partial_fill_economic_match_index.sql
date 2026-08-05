-- PAPER-SOAK-PARTIAL-FILL-DEDUP-01
--
-- Support index for the economic-match lookup added to
-- inbox_insert_partial_fill_deduped (mqk-db::inbox): every partial_fill
-- insert now checks for an existing (run_id, internal_order_id) row with
-- matching delta_qty/price_micros within a bounded event_ts_ms window before
-- inserting. Without this index that check is a sequential scan of every
-- oms_inbox row for the run on each partial-fill insert.
--
-- Purely additive: narrows an existing full-table scan to an index range
-- scan. Does not change any constraint, column, or existing row.

create index if not exists idx_inbox_run_order_event_kind
    on oms_inbox (run_id, internal_order_id, event_kind);
