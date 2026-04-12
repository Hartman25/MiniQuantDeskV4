-- OPS-01: Durable incident tracking.
--
-- One row per operator-declared incident.  An incident is an explicit
-- decision by the operator that a fault condition warrants a tracked
-- record — distinct from an alert acknowledgment (OPS-02).
--
-- `linked_alert_id` references the fault-signal class slug from
-- `sys_alert_acks` / `/api/v1/alerts/active` that prompted the incident.
-- The link is advisory (TEXT, no FK) because alerts are ephemeral
-- in-memory signals; only acks are durable.
--
-- `status` lifecycle: open → resolved.  No DB constraint enforces this;
-- the route layer validates the value on write.
--
-- No DEFAULT now() — all timestamps injected by the caller.
-- Guard [N] applies at migration >= 0012.

CREATE TABLE sys_incidents (
    incident_id     TEXT        PRIMARY KEY,
    opened_at_utc   TIMESTAMPTZ NOT NULL,
    title           TEXT        NOT NULL,
    severity        TEXT        NOT NULL,
    status          TEXT        NOT NULL DEFAULT 'open',
    linked_alert_id TEXT,
    opened_by       TEXT        NOT NULL DEFAULT 'operator'
);
