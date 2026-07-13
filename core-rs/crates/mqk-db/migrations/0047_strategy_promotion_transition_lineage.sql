-- STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01: additive lineage columns
-- on sys_strategy_promotion_transitions (migration 0046). Repairs two
-- defects found in the original CLOSED_LOCAL patch group:
--
-- 1. Concurrency/branching (Phase B): `parent_transition_id` records the
--    exact transition this one was chained from (NULL only for the very
--    first transition of an identity). The daemon route layer
--    (`mqk_db::insert_strategy_promotion_transition_serialized`) locks the
--    identity with a transaction-scoped `pg_advisory_xact_lock`, re-reads
--    the actual current transition inside that lock, and rejects the
--    insert with a stable `transition_conflict` reason if the freshly
--    re-read current transition's id does not match the caller's expected
--    `parent_transition_id` -- so two concurrent transitions can never
--    both be accepted as children of the same parent. This column is
--    nullable and carries no uniqueness constraint of its own: it is
--    lineage bookkeeping, not the enforcement mechanism itself (the
--    advisory lock + re-read-then-compare inside one transaction is) --
--    deliberately, so pre-existing direct low-level test fixtures across
--    the daemon test suite that seed sequential (non-concurrent) states
--    via the original `insert_strategy_promotion_transition` are not
--    forced to reconstruct real chain lineage to remain valid.
--
-- 2. Evidence-lineage loss (Phase D): `evidence_transition_id` is an
--    immutable pointer to the exact transition that established the
--    currently-authoritative evidence for this identity -- either this
--    row itself (when it is itself evidence-bearing: `no-state ->
--    shadow_approved` or `demoted -> shadow_approved` re-approval) or the
--    prior current record's own `evidence_transition_id`, carried
--    forward unchanged by every non-evidence-bearing transition
--    (`shadow_approved -> paper_approved`, `paper_approved ->
--    active_paper`, any `-> demoted`, any `-> retired`). This lets
--    `mqk_db::resolve_evidence_lineage` durably resolve the exact
--    evidence-bearing transition (review id, scanner scan id, git hash,
--    fingerprint, artifact path) for any current state, including
--    `active_paper`, without the caller ever supplying or trusting a
--    claimed evidence value -- it is always derived from durable history.
--
-- Both columns are nullable: this is a schema-safety choice (no backfill
-- decision is required for any pre-existing row), not a statement that
-- either may be legitimately absent for any row inserted from this point
-- forward via the daemon route -- the route always populates both.
--
-- No existing column, row, or constraint from migration 0046 is altered.

alter table sys_strategy_promotion_transitions
    add column if not exists parent_transition_id uuid
        references sys_strategy_promotion_transitions (transition_id),
    add column if not exists evidence_transition_id uuid
        references sys_strategy_promotion_transitions (transition_id);

-- Supports evidence-lineage resolution reads
-- (`mqk_db::resolve_evidence_lineage` / `fetch_promotion_transition_by_id`).
create index if not exists sys_strategy_promotion_transitions_evidence_transition_idx
    on sys_strategy_promotion_transitions (evidence_transition_id)
    where evidence_transition_id is not null;

-- Supports parent-chain lookups/audits.
create index if not exists sys_strategy_promotion_transitions_parent_transition_idx
    on sys_strategy_promotion_transitions (parent_transition_id)
    where parent_transition_id is not null;
