-- Patch L7: Sticky DISARM persistence — fail-closed boot
--
-- sys_arm_state is a singleton table (one row, sentinel_id = 1).
-- On every boot, the runtime loads this row to determine the prior arm state,
-- then applies fail-closed semantics via ArmState::boot():
--   - No row  → DISARMED (BootDefault)
--   - 'ARMED' → DISARMED (BootDefault)   [never auto-arm from persisted state]
--   - 'DISARMED' + reason → DISARMED with reason preserved
--
-- Disarm reasons are string-encoded DisarmReason variants:
--   BootDefault | ManualDisarm | DeadmanHalt | IntegrityViolation | ReconcileDrift

create table if not exists sys_arm_state (
    sentinel_id    integer      primary key default 1 check (sentinel_id = 1),
    state          text         not null,   -- 'ARMED' | 'DISARMED'
    reason         text         null,       -- DisarmReason name when DISARMED; null when ARMED
    updated_at_utc timestamptz  not null default now()
);

comment on table sys_arm_state is
    'Singleton arm/disarm state record for fail-closed boot semantics (Patch L7). '
    'Updated on every arm/disarm event; read on every process startup.';

comment on column sys_arm_state.state is 'ARMED | DISARMED';
comment on column sys_arm_state.reason is
    'BootDefault | ManualDisarm | DeadmanHalt | IntegrityViolation | ReconcileDrift';
