-- Patch A2: Durable broker event cursor.
--
-- Tracks the last-consumed cursor value per adapter so the orchestrator can
-- resume fetching events from the correct position after a crash or planned
-- restart.
--
-- Design:
--   adapter_id     TEXT PK  — stable per-adapter string (e.g. "paper", "alpaca").
--   cursor_value   TEXT     — opaque cursor; for paper this is a u64 seq number
--                            as decimal string; for live adapters it is a
--                            broker-assigned event ID.
--   updated_at     TIMESTAMPTZ — wall-clock of last cursor advancement (ops
--                            metadata only; not used in correctness logic).
--
-- Cursor advancement contract (enforced by the orchestrator):
--   1. Fetch events from broker using current cursor.
--   2. Persist all events to oms_inbox (dedup on broker_message_id).
--   3. ONLY THEN advance the cursor here via upsert.
--
-- This means: if the process crashes between step 2 and step 3, the cursor
-- is NOT advanced.  On restart, the orchestrator re-fetches events from the
-- old cursor.  Because oms_inbox uses ON CONFLICT DO NOTHING, duplicate
-- delivery is safe and idempotent.
--
-- Fail-closed: if the cursor row is absent at startup, the orchestrator starts
-- from the beginning (cursor = None), which is always safe.

create table broker_event_cursor (
    adapter_id   text        not null,
    cursor_value text        not null,
    updated_at   timestamptz not null,
    primary key (adapter_id)
);
