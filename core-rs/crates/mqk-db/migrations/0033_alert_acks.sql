-- 0033_alert_acks.sql
-- OPS-02: durable operator alert-ack table.
--
-- alert_id is the fault-signal class slug
-- (e.g. "reconcile.dispatch_block.dirty").
-- Ack is advisory: active alert source remains the in-memory fault-signal
-- computation.  Ack persists until explicitly superseded by a later upsert.
--
-- acked_at_utc has NO DEFAULT now() — timestamp is injected by caller.
-- (guard [N]: DEFAULT now() is forbidden in migrations >= 0012)

CREATE TABLE sys_alert_acks (
    alert_id     TEXT        PRIMARY KEY,
    acked_at_utc TIMESTAMPTZ NOT NULL,
    acked_by     TEXT        NOT NULL DEFAULT 'operator'
);
