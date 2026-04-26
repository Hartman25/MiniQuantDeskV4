-- EXEC-CONT-01: Add 'InboundContinuityUnproven' to the sys_arm_state.reason
-- CHECK constraint so the orchestrator can durably disarm with an explicit
-- reason when Alpaca WS inbound continuity is cold-start unproven or gap-detected.
--
-- The reason string matches the `persist_halt_and_disarm` call-site in
-- orchestrator.rs Phase 2.  It appears in audit_events (topic='orchestrator',
-- event_type='InboundContinuityUnproven') and in sys_arm_state.reason after
-- the continuity-failure halt+disarm path fires.

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
            'InboundContinuityUnproven'::text
        ])
    );
