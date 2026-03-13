-- DB-01: Repair sys_arm_state.reason contract to include all runtime-produced
-- halt/disarm reasons currently written by daemon/runtime paths.
--
-- This is a forward-only repair migration. We do not rewrite prior migrations;
-- we re-assert the latest authoritative CHECK constraint for safety on upgraded DBs.

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
            'AuthSession',
            'OperatorDisarm',
            'OperatorHalt',
            'DeadmanExpired',
            'DeadmanSupervisorFailure',
            'DeadmanHeartbeatPersistFailed',
            'LeaderLeaseLost',
            'LeaderLeaseUnavailable'
        )
    );
