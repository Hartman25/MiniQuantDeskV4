alter table sys_arm_state
    drop constraint if exists sys_arm_state_reason_check;

alter table sys_arm_state
    add constraint sys_arm_state_reason_check
    check (
        reason is null or reason = any (array[
            'BootDefault'::text,
            'ManualDisarm'::text,
            'OperatorDisarm'::text,
            'OperatorHalt'::text,
            'DeadmanHalt'::text,
            'DeadmanExpired'::text,
            'DeadmanSupervisorFailure'::text,
            'DeadmanHeartbeatPersistFailed'::text,
            'IntegrityViolation'::text,
            'ReconcileDrift'::text,
            'RecoveryQuarantine'::text,
            'AmbiguousSubmit'::text,
            'AuthSession'::text,
            'LeaderLeaseLost'::text,
            'LeaderLeaseUnavailable'::text
        ])
    );