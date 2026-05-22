-- MIGRATION-0040-BACKFILL-DUP-FILLS-01
--
-- Step 1: Deterministic historical backfill.
-- ==========================================
-- Before enforcing the unique index, remove duplicate event_kind='fill' rows
-- accumulated from WS+REST dual delivery and historical REST reingestion.
-- The cleanup is idempotent: when no duplicates exist it deletes 0 rows.
--
-- Root causes observed in paper DB:
--   Pattern A (2 groups, both rows applied):
--     Alpaca delivers the same economic fill on two lanes:
--       WS:   broker_fill_id=null, nanosecond-precision timestamp
--       REST: broker_fill_id present, lower-precision timestamp
--     Both rows produce different broker_message_id values so the existing
--     transport-dedupe index (uq_inbox_run_broker_message_id) does not catch
--     them.  Both rows were inserted and applied, causing historical
--     double-apply (+2 shares vs broker truth of +1).
--   Pattern B (32 groups, all unapplied):
--     Historical REST reingestion produced multiple fill rows per order
--     (applied_at_utc=NULL on all, broker_fill_id present on all, received
--     within the same REST polling window).
--
-- Canonical keep rule (deterministic, priority-ordered):
--   Priority 1: applied_at_utc IS NOT NULL
--     A row already applied to the portfolio represents committed economic truth.
--   Priority 2: broker_fill_id IS NOT NULL
--     The REST lane carries authoritative broker execution identity.
--   Priority 3: earliest applied_at_utc (ASC NULLS LAST)
--     Among applied rows, the first-applied row is the economic anchor.
--   Priority 4: earliest received_at_utc (ASC NULLS LAST)
--     Among unapplied rows, the first-received row is the ingest anchor.
--   Priority 5: lowest inbox_id (ASC)
--     Durable insertion-order tiebreak.
--
-- Scope: event_kind='fill' and internal_order_id IS NOT NULL only.
-- Rows where internal_order_id IS NULL cannot be meaningfully constrained
-- and are excluded from both this cleanup and the unique index predicate.
-- Partial fills (event_kind='partial_fill') are NOT touched.

with ranked as (
    select
        inbox_id,
        row_number() over (
            partition by run_id, internal_order_id
            order by
                case when applied_at_utc  is not null then 0 else 1 end,
                case when broker_fill_id  is not null then 0 else 1 end,
                applied_at_utc  asc nulls last,
                received_at_utc asc nulls last,
                inbox_id        asc
        ) as rn
    from oms_inbox
    where event_kind          = 'fill'
      and internal_order_id is not null
)
delete from oms_inbox
using ranked
where oms_inbox.inbox_id = ranked.inbox_id
  and ranked.rn > 1;

-- Step 2: Enforce one-final-fill-per-order going forward.
-- =========================================================
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
-- The predicate includes internal_order_id IS NOT NULL to match the cleanup
-- scope above — rows with null internal_order_id are excluded from both the
-- backfill and the index, making the scope contract explicit and consistent.
-- In practice the Rust insert path never produces null internal_order_id
-- (it falls back to broker_message_id), so this predicate is a safety net.
--
-- Partial fills (event_kind='partial_fill') are NOT covered here because
-- a single order may legitimately produce multiple partial-fill inbox rows,
-- each with a distinct quantity and broker execution ID.

create unique index if not exists uq_inbox_run_order_single_fill
    on oms_inbox (run_id, internal_order_id)
    where event_kind          = 'fill'
      and internal_order_id is not null;
