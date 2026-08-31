-- RUNTIME-LEASE-RUN-IDENTITY-AUTHORITY-01: runtime_leader_lease is a global
-- singleton (CHECK(id=1)) whose durable authority previously carried no
-- notion of which run it belonged to. acquire_or_refresh_lease_for_running_run
-- could therefore only judge steal-eligibility by raw TTL expiry, with no way
-- to distinguish "this run's own prior lease" from "a different, already-
-- terminated run's orphaned lease" left behind because stop_run_if_evidence_
-- clean does not delete it -- the confirmed independent-review defect where a
-- brand-new run's own fresh heartbeat was being read as if it were evidence
-- about a completely different, older run's lease holder.
--
-- run_id is nullable: no backfill decision is required for the (at most one)
-- pre-existing row -- mirrors migration 0067's identical precedent. Every NEW
-- acquire writes run_id going forward. A pre-migration row with run_id IS
-- NULL is treated by the writer-side code as an anonymous legacy lease --
-- exactly the pre-migration fail-closed contract (raw TTL expiry only, no
-- cross-run inference attempted, since its owning run is unknowable) --
-- never optimistically bound to whichever run next reads it. This is pure
-- transient coordination state (re-acquired every lease TTL, never
-- historical/audit truth), so a legacy NULL row is naturally replaced by a
-- proper run_id-bound row on the very next successful acquisition -- no
-- permanent legacy state, and no deletion of possibly-still-live authority
-- during migration (a raw ADD COLUMN never touches existing row contents or
-- takes the table offline).
--
-- References runs(run_id) ON DELETE CASCADE: every row this table ever
-- holds is written only after acquire_or_refresh_lease_for_running_run has
-- already locked and confirmed that exact runs row exists (SELECT ... FOR
-- UPDATE). Production code never deletes a runs row, but disposable test
-- fixtures across this workspace routinely do ("delete from runs where
-- run_id = $1" test cleanup); a plain (non-cascading) reference would make
-- that delete fail with a foreign-key violation whenever a lease row still
-- pointed at it. Cascading the delete is correct either way: a lease bound
-- to a run that no longer exists carries no remaining authority, so it
-- should disappear with its run, not orphan or block the delete.

alter table runtime_leader_lease
    add column if not exists run_id uuid references runs(run_id) on delete cascade;

-- Supports "does this run currently hold/contest the lease" lookups without
-- a full-table scan (the table has at most one row today, but the index
-- costs nothing and documents the intended access pattern).
create index if not exists runtime_leader_lease_run_id_idx
    on runtime_leader_lease (run_id)
    where run_id is not null;
