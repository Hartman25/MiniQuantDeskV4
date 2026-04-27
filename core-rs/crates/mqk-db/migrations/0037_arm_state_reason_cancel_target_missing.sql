-- EXEC-CANCEL-01: Add 'CancelTargetMissing' to the sys_arm_state.reason
-- CHECK constraint so the orchestrator can durably disarm with an explicit
-- reason when a cancel outbox row targets an order absent from the live OMS map.
--
-- The reason string matches the `persist_halt_and_disarm("CancelTargetMissing")`
-- call-site in orchestrator/dispatch.rs (dispatch_cancel_claimed_outbox_row).
-- It appears in audit_events (topic='orchestrator', event_type='CancelTargetMissing')
-- and in sys_arm_state.reason after the cancel-target-missing halt+disarm path fires.

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
            'LeaderLeaseUnavailable'::text,
            'InboundContinuityUnproven'::text,
            'CancelTargetMissing'::text
        ])
    );
