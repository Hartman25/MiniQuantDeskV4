-- A4: Explicit AMBIGUOUS outbox status for quarantined orders whose
-- broker submission outcome is definitively unknown.
--
-- Background (A3):
--   BrokerError::AmbiguousSubmit left the outbox row in DISPATCHING.
--   DISPATCHING is also used for "dispatch in flight" (outbox_mark_dispatching
--   writes it just before gateway.submit()). Rows left in DISPATCHING on crash
--   are detected by Phase-0b restart quarantine.
--
-- Gap (A4):
--   After gateway.submit() returns AmbiguousSubmit, the outcome is definitively
--   known to be unknown — the broker may or may not have accepted the order.
--   Leaving the row in DISPATCHING conflates two meanings:
--     - "dispatch was initiated (may have crashed mid-flight)"  — crash scenario
--     - "broker confirmed: outcome is unknown"                  — AmbiguousSubmit
--
--   A dedicated AMBIGUOUS status:
--     1. Explicitly encodes "broker confirmed unknown outcome" vs crash residue.
--     2. outbox_claim_batch (WHERE status = 'PENDING') cannot claim it —
--        structural prevention of silent re-dispatch.
--     3. outbox_load_restart_ambiguous always returns AMBIGUOUS rows regardless
--        of broker_order_map presence.
--     4. The only exit path is outbox_reset_ambiguous_to_pending, which requires
--        explicit operator/reconcile-proof invocation.
--
-- Additional constraint fix:
--   AmbiguousSubmit and AuthSession were already used by the orchestrator as
--   arm-state disarm reasons (persist_halt_and_disarm) but were absent from
--   the DB CHECK constraint, causing silent persist_arm_state failures on those
--   error paths (the let _ suppressed the anyhow error). This migration adds
--   them so the disarm is durable and auditable.

-- ---------------------------------------------------------------------------
-- 1. Extend the outbox status constraint to include AMBIGUOUS.
-- ---------------------------------------------------------------------------

alter table oms_outbox
    drop constraint if exists oms_outbox_status_check;

alter table oms_outbox
    add constraint oms_outbox_status_check
    check (status in ('PENDING','CLAIMED','DISPATCHING','SENT','ACKED','FAILED','AMBIGUOUS'));

comment on column oms_outbox.status is
    'PENDING | CLAIMED | DISPATCHING | SENT | ACKED | FAILED | AMBIGUOUS';

-- ---------------------------------------------------------------------------
-- 2. Extend the arm-state reason constraint.
--    Adds AmbiguousSubmit and AuthSession to the list that already
--    included BootDefault/ManualDisarm/DeadmanHalt/IntegrityViolation/
--    ReconcileDrift/RecoveryQuarantine (added in migration 0017).
-- ---------------------------------------------------------------------------

alter table sys_arm_state
    drop constraint if exists sys_arm_state_reason_check;

alter table sys_arm_state
    add constraint sys_arm_state_reason_check
    check (
        reason is null
        or reason in (
            'BootDefault',
            'ManualDisarm',
            'DeadmanHalt',
            'IntegrityViolation',
            'ReconcileDrift',
            'RecoveryQuarantine',
            'AmbiguousSubmit',
            'AuthSession'
        )
    );
