-- EXEC-02: Order lifecycle event table for replace/cancel chain lineage.
--
-- Purpose: append-only record of non-fill lifecycle events per order per run.
-- Enables /api/v1/execution/replace-cancel-chains to surface real chain data
-- instead of truth_state="not_wired".
--
-- event_id = broker_message_id — the same deduplication identity used by
-- oms_inbox.  ON CONFLICT DO NOTHING makes repeated best-effort writes
-- from the orchestrator idempotent.
--
-- Operations recorded: cancel_ack, replace_ack, cancel_reject, replace_reject.
-- Fill events are NOT recorded here — those live in fill_quality_telemetry.

create table if not exists oms_order_lifecycle_events (
    event_id            text        primary key,
    run_id              uuid        not null,
    internal_order_id   text        not null,
    -- 'cancel_ack' | 'replace_ack' | 'cancel_reject' | 'replace_reject'
    operation           text        not null,
    -- broker-assigned order ID; null for paper adapters that do not carry it.
    broker_order_id     text,
    -- populated for replace_ack: post-replace authoritative total qty.
    new_total_qty       bigint,
    recorded_at_utc     timestamptz not null,
    constraint oms_order_lifecycle_events_op_check
        check (operation in ('cancel_ack', 'replace_ack', 'cancel_reject', 'replace_reject'))
);

create index if not exists oms_order_lifecycle_events_run_idx
    on oms_order_lifecycle_events (run_id, recorded_at_utc asc);

comment on table oms_order_lifecycle_events is
    'EXEC-02: append-only lifecycle events for cancel/replace operations per order per run. '
    'Populated best-effort from Phase 3b of ExecutionOrchestrator::tick().';

comment on column oms_order_lifecycle_events.event_id is
    'Equals broker_message_id — deduplication identity shared with oms_inbox.';

comment on column oms_order_lifecycle_events.new_total_qty is
    'For replace_ack: authoritative post-replace total qty from BrokerEvent::ReplaceAck. '
    'NULL for all other operation types.';
