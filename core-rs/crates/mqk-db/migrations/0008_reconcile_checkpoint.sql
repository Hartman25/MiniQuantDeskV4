-- Patch B1: Reconcile checkpoint — arming must not trust a forgeable audit string
--
-- arm_preflight previously checked audit_events.event_type = 'CLEAN' (topic='reconcile').
-- Any process with DB write access could forge that check by inserting a single row via
-- the general-purpose insert_audit_event() function.
--
-- sys_reconcile_checkpoint is a SEPARATE, dedicated table written only by the reconcile
-- engine via reconcile_checkpoint_write(). Arming now requires a CLEAN row here,
-- not just a CLEAN string in audit_events.
--
-- The structural separation is the primary defence: the attacker must call a specific
-- reconcile-checkpoint write function (not the general audit-event insert), making
-- the provenance of the clean signal explicit and auditable.
--
-- Fields:
--   run_id               — the run this checkpoint belongs to
--   verdict              — 'CLEAN' | 'DIRTY' (result of the reconcile pass)
--   snapshot_watermark_ms — SnapshotWatermark.last_accepted_ms() at reconcile time
--   result_hash          — caller-computed hash of the reconcile payload (for auditability)
--   created_at_utc       — when the checkpoint was written

create table if not exists sys_reconcile_checkpoint (
    checkpoint_id         bigserial   primary key,
    run_id                uuid        not null,
    verdict               text        not null,
    snapshot_watermark_ms bigint      not null,
    result_hash           text        not null,
    created_at_utc        timestamptz not null default now()
);

create index if not exists idx_reconcile_checkpoint_run_ts
    on sys_reconcile_checkpoint (run_id, created_at_utc desc);

comment on table sys_reconcile_checkpoint is
    'Dedicated reconcile-engine checkpoint table (Patch B1). '
    'arm_preflight checks this table, NOT audit_events, for reconcile cleanliness. '
    'Only reconcile_checkpoint_write() inserts here; insert_audit_event() cannot satisfy arming.';

comment on column sys_reconcile_checkpoint.verdict is 'CLEAN | DIRTY';
comment on column sys_reconcile_checkpoint.snapshot_watermark_ms is
    'SnapshotWatermark.last_accepted_ms() at the time reconcile completed.';
comment on column sys_reconcile_checkpoint.result_hash is
    'Caller-computed hash of the reconcile payload (e.g. SHA-256 of JSON). '
    'Provides an auditability hook; not cryptographically verified by arming itself.';
