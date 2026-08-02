-- 0060: exact candidate key uniqueness for sys_dynamic_selection_plan_candidates
-- (DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C-FORMAL-SOAK-GATE-TRUTH-
-- REPAIR-01 Part 7).
--
-- Additive only. Does not modify migration 0059.
--
-- Migration 0059's only uniqueness constraint on
-- sys_dynamic_selection_plan_candidates was (plan_id, ordinal) -- the
-- physical insert-order identity, not the candidate's economic identity.
-- mqk_portfolio::dynamic_selection::resolve_dynamic_selection_plan's own doc
-- comment states the caller-built candidate set "must already be
-- deduplicated" by (symbol, strategy_id, timeframe_secs) before it reaches
-- the pure model, but the pure model does not itself re-verify that, and
-- nothing at the DB layer enforced it either -- two candidate rows for the
-- same plan_id could carry an identical (symbol, strategy_id,
-- timeframe_secs) exact key under two different ordinals with no error.
--
-- This constraint makes that key uniqueness a durable DB-level invariant:
-- any write attempt with a duplicate exact key fails the whole transaction
-- (insert_dynamic_selection_plan's caller already fails closed on any
-- write error for a PaperEnforcedAllowed start -- see
-- state/lifecycle.rs's PayloadCollision/Err handling). The read-side
-- validator (dynamic_selection_evidence_validator.rs) independently
-- re-checks the same invariant as defense-in-depth for any row written
-- before this migration existed.
ALTER TABLE sys_dynamic_selection_plan_candidates
    ADD CONSTRAINT sys_dynamic_selection_plan_candidates_exact_key_uniq
    UNIQUE (plan_id, symbol, strategy_id, timeframe_secs);
