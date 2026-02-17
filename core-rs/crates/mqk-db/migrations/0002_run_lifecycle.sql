-- PATCH 14: Run lifecycle enforcement

alter table runs
  add column if not exists status text not null default 'CREATED';

alter table runs
  add column if not exists armed_at_utc timestamptz null;

alter table runs
  add column if not exists running_at_utc timestamptz null;

alter table runs
  add column if not exists stopped_at_utc timestamptz null;

alter table runs
  add column if not exists halted_at_utc timestamptz null;

alter table runs
  add column if not exists last_heartbeat_utc timestamptz null;

-- Keep status values constrained.
alter table runs
  drop constraint if exists runs_status_check;

alter table runs
  add constraint runs_status_check
  check (status in ('CREATED','ARMED','RUNNING','STOPPED','HALTED'));

-- One LIVE run at a time per engine in active states.
-- Active = ARMED or RUNNING.
drop index if exists uq_live_engine_active_run;

create unique index uq_live_engine_active_run
on runs(engine_id)
where mode = 'LIVE' and status in ('ARMED','RUNNING');
