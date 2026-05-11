-- 0039: Broker position baseline for operator-confirmed adoption.
--
-- Stores a single-row baseline snapshot that the operator explicitly adopts
-- as starting local truth when a pre-existing broker-side position or order
-- blocks reconcile at paper startup.  Sentinel pattern (sentinel_id=1) matches
-- sys_arm_state / sys_reconcile_status_state for consistency.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid() — callers supply all fields.

CREATE TABLE sys_broker_position_baseline (
    sentinel_id           integer                  NOT NULL,
    broker_snapshot_json  jsonb                    NOT NULL,
    adopted_at_utc        timestamp with time zone NOT NULL,
    adopted_by            text                     NOT NULL,
    audit_event_id        uuid                     NOT NULL,
    CONSTRAINT sys_broker_position_baseline_pkey PRIMARY KEY (sentinel_id),
    CONSTRAINT sys_broker_position_baseline_sentinel CHECK (sentinel_id = 1)
);
