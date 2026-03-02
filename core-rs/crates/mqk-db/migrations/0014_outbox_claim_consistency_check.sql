-- FC-4: Outbox claim metadata consistency CHECKs.
--
-- Invariants enforced:
--
--   1. CLAIMED row must carry claim metadata:
--        status = 'CLAIMED'  ⟹  claimed_at_utc IS NOT NULL
--                            ⟹  claimed_by      IS NOT NULL
--
--   2. PENDING row must NOT carry claim metadata (no pre-claiming):
--        status = 'PENDING'  ⟹  claimed_at_utc IS NULL
--                            ⟹  claimed_by      IS NULL
--
-- Rows in SENT / ACKED / FAILED retain whatever claim metadata was written
-- when they transitioned through CLAIMED; no constraint is applied to
-- those terminal states because the metadata is historically correct and
-- should be preserved for audit purposes.
--
-- These constraints are evaluated at the storage layer independently of
-- any application-layer validation, closing the gap that previously allowed
-- a CLAIMED row to be inserted without claimed_at_utc / claimed_by (or a
-- PENDING row to carry stale claim metadata from a previous attempt).

-- ---------------------------------------------------------------------------
-- 1. CLAIMED rows must have claim metadata
-- ---------------------------------------------------------------------------

alter table oms_outbox
    drop constraint if exists oms_outbox_claimed_metadata_present;

alter table oms_outbox
    add constraint oms_outbox_claimed_metadata_present
    check (
        status != 'CLAIMED'
        or (claimed_at_utc is not null and claimed_by is not null)
    );

-- ---------------------------------------------------------------------------
-- 2. PENDING rows must NOT have claim metadata
-- ---------------------------------------------------------------------------

alter table oms_outbox
    drop constraint if exists oms_outbox_pending_metadata_absent;

alter table oms_outbox
    add constraint oms_outbox_pending_metadata_absent
    check (
        status != 'PENDING'
        or (claimed_at_utc is null and claimed_by is null)
    );
