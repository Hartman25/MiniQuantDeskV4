-- RUNTIME-LEASE-LEGACY-UNBOUND-MIGRATION-SAFETY-01: migration 0068 added a
-- nullable runtime_leader_lease.run_id and documented a run_id-IS-NULL row as
-- "anonymous lease under the pre-migration fail-closed contract, raw TTL
-- expiry only". That is unsafe: the pre-0068 lease contract used
-- RUNTIME_LEASE_TTL_SECS=90 / DEADMAN_TTL_SECS=120 (see
-- core-rs/crates/mqk-db/src/runtime_lease.rs), so a legacy NULL lease can be
-- raw-expired (>90s since last write) while the runtime that holds it is
-- still deadman-healthy (<120s since last write). Because run_id is NULL,
-- nothing durable identifies which run that is, so no run's heartbeat can
-- ever corroborate or refute liveness the way the same-run branch of
-- acquire_or_refresh_lease_for_running_run already does. Raw expiry alone
-- must never authorize touching that row's ambiguous authority; this
-- migration eliminates it instead of carrying it forward indefinitely.
--
-- Reconciliation (runs on every apply of this migration, effectively once
-- per deployment, since it deletes what it acts on):
--   1. If no run_id-IS-NULL row exists (the overwhelming common case, and
--      the only state every current writer -- acquire_or_refresh_lease_for_
--      running_run -- ever produces), do nothing.
--   2. If one exists but is not yet raw-expired, fail closed: an unexpired
--      lease is never touchable regardless of ownership (mirrors
--      acquire_or_refresh_lease_for_running_run's own "an unexpired lease is
--      never stealable" invariant).
--   3. If it is raw-expired, lock `runs` (SHARE MODE -- conflicts with the
--      ROW EXCLUSIVE any INSERT/UPDATE/DELETE on `runs` takes, so no run can
--      be created, armed, begun, heartbeat, or halted for the duration of
--      this check+delete; compatible with the ROW SHARE plain `SELECT ...
--      FOR UPDATE` takes, so it does not deadlock against
--      acquire_or_refresh_lease_for_running_run's own row lock) and refuse
--      to proceed if any run reports ARMED status, or RUNNING status with a
--      last_heartbeat_utc inside the deadman window -- either could
--      plausibly be the row's unknowable owner. NULL last_heartbeat_utc is
--      treated as already-stale, mirroring acquire_or_refresh_lease_for_
--      running_run's own same-run deadman check (no heartbeat ever recorded
--      is no evidence of life).
--   4. Otherwise the system is quiescent: the row cannot correspond to any
--      currently live authority, so delete it.
--
-- run_id remains nullable: mqk-db's legacy, non-run-aware acquire_lease /
-- refresh_lease / verify_lease / release_lease primitives (confirmed to have
-- no production call site -- only their own and orchestrator/tests.rs unit
-- tests use them; the orchestrator's real production path is exclusively
-- acquire_or_refresh_lease_for_running_run / verify_lease_for_run /
-- release_lease_for_run) still write run_id as NULL by construction. Adding
-- NOT NULL here would either break those primitives' existing tests or
-- require reworking/removing them -- an unrelated, separately-scoped change.
-- The reconciliation above plus runtime_lease.rs's corresponding
-- deadman-gated legacy-row handling (this same patch) together mean no
-- run_id-IS-NULL row can ever be treated as ambiguous-safe-to-touch again,
-- with or without the NOT NULL constraint.
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
     where status = 'ARMED'
        or (
             status = 'RUNNING'
             and last_heartbeat_utc is not null
             and now() - last_heartbeat_utc <= interval '120 seconds'
           );

    if active_authority_count > 0 then
        raise exception 'runtime_leader_lease legacy migration safety: % run(s) report ARMED or deadman-fresh RUNNING authority while an unbound (run_id IS NULL) lease row exists; its true owner is unknowable, so refusing to remove it until the system is quiescent. Resolve (halt/clear) active runs before retrying this migration.', active_authority_count;
    end if;

    delete from runtime_leader_lease where id = 1 and run_id is null;
end $$;

-- FK delete-action review (RUNTIME-LEASE-LEGACY-UNBOUND-MIGRATION-SAFETY-01):
-- 0068 chose ON DELETE CASCADE, justified by test-fixture convenience ("many
-- TEST fixtures DELETE FROM runs"). No test in this workspace actually both
-- binds the lease to a run (via acquire_or_refresh_lease_for_running_run) and
-- deletes that same run row -- so CASCADE was never exercised, and the
-- justification does not hold independently of hypothetical fixture
-- convenience. Production never deletes a runs row (confirmed by 0068's own
-- comment). The correct production invariant is therefore RESTRICT: a run
-- row that a leadership lease still durably points to carries live authority
-- and must not be silently destroyed as a side effect of deleting the run --
-- an attempt to do so is anomalous and should fail loudly (fail-closed),
-- never disappear quietly.
alter table runtime_leader_lease
    drop constraint runtime_leader_lease_run_id_fkey;

alter table runtime_leader_lease
    add constraint runtime_leader_lease_run_id_fkey
    foreign key (run_id) references runs(run_id) on delete restrict;
