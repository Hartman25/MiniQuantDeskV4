-- PRE-SOAK-DAEMON-SUPERVISOR-HALT-FENCE-CLOSURE-01: add
-- 'ExecutionLoopTickFailure' to the sys_arm_state.reason CHECK constraint so
-- mqk-daemon's execution-loop supervisor generic tick-error fallback
-- (spawn_execution_loop's persist_execution_loop_safety_halt seam) can
-- durably disarm with an explicit reason instead of failing the fenced
-- halt write with a constraint violation.
--
-- The reason string matches the `persist_execution_loop_safety_halt(...,
-- "ExecutionLoopTickFailure")` call-site in mqk-daemon/src/state/
-- loop_runner.rs (the generic `orchestrator.tick()` error fallback, used
-- only when the tick error is not already covered by a more specific
-- reason such as DeadmanExpired/DeadmanSupervisorFailure/
-- DeadmanHeartbeatPersistFailed/InboundContinuityUnproven).

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
            'CancelTargetMissing'::text,
            'ExecutionLoopTickFailure'::text
        ])
    );
