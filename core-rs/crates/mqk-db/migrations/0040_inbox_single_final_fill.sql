-- FILL-DEDUP-WS-REST-PRECISION-01: prevent WS+REST duplicate final fills.
--
-- Problem: Alpaca delivers the same economic fill on both the WS lane (no
-- broker_fill_id, high-precision timestamp) and the REST lane (broker_fill_id
-- present, lower-precision timestamp).  The two rows produce different
-- broker_message_id values ("...fill:2026-...174478594Z" vs "...fill:2026-...174479Z"),
-- so the existing uq_inbox_run_broker_message_id constraint does not deduplicate
-- them.  The existing uq_inbox_run_broker_fill_id partial index only fires when
-- broker_fill_id IS NOT NULL, so it does not catch the WS row (broker_fill_id=null).
--
-- Fix: enforce at most one final-fill row per (run_id, internal_order_id).
-- An order can only have exactly one terminal fill event.  Whichever lane
-- (WS or REST) wins the insert holds the economic truth; the second insert
-- returns a 23505 conflict that inbox_insert_deduped treats as Ok(false).
--
-- This index is intentionally narrow: it targets only event_kind='fill'
-- (final fills).  Partial fills (event_kind='partial_fill') are NOT covered
-- here because a single order may legitimately produce multiple partial-fill
-- inbox rows, each with a distinct quantity and broker execution ID.

create unique index if not exists uq_inbox_run_order_single_fill
    on oms_inbox (run_id, internal_order_id)
    where event_kind = 'fill';
