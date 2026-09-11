-- RUNTIME-LEASE-STRICT-QUIESCENCE-FORWARD-01
--
-- Migration 0069 was deployed before its RUNNING-run quiescence predicate was
-- tightened in source. Historical migrations are immutable: 0069 remains its
-- exact deployed form, and this migration carries the later fail-closed
-- quiescence repair forward.
--
-- If an unbound runtime_leader_lease row (run_id IS NULL) exists, refuse to
-- touch it while the lease is unexpired or while ANY run reports ARMED or
-- RUNNING status, regardless of heartbeat freshness. begin_run does not
-- atomically establish the first heartbeat, so stale/NULL heartbeat is never
-- proof that RUNNING authority is absent.
--
-- No FK/schema change is repeated here; deployed 0069 already changed
-- runtime_leader_lease_run_id_fkey to ON DELETE RESTRICT.
do $$
declare
    legacy_row record;
    active_authority_count bigint;
begin
    select run_id, lease_expires_at, updated_at
      into legacy_row
      from runtime_leader_lease
     where id = 1 and run_id is null
     for update;

    if not found then
        return;
    end if;

    if legacy_row.lease_expires_at > now() then
        raise exception 'runtime_leader_lease legacy migration safety: an unexpired run_id-IS-NULL lease row exists (expires %); refusing to touch ambiguous authority while it may still be legitimately held. Retry once it naturally expires.', legacy_row.lease_expires_at;
    end if;

    lock table runs in share mode;

    select count(*) into active_authority_count
      from runs
     where status in ('ARMED', 'RUNNING');

    if active_authority_count > 0 then
        raise exception 'runtime_leader_lease legacy migration safety: % run(s) report ARMED or RUNNING status while an unbound (run_id IS NULL) lease row exists; its true owner is unknowable, so refusing to remove it until the system is quiescent. Heartbeat freshness is not evidence either way -- begin_run does not atomically establish a heartbeat. Resolve (halt/clear) active runs before retrying this migration.', active_authority_count;
    end if;

    delete from runtime_leader_lease where id = 1 and run_id is null;
end $$;
