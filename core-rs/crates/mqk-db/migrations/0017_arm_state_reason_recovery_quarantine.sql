-- Patch 2: allow explicit arm-state reason for restart quarantine when
-- ambiguous outbox rows (DISPATCHING/SENT) are detected on startup/tick.

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
            'RecoveryQuarantine'
        )
    );