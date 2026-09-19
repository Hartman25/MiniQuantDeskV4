-- 0073: complete the explicit multi-strategy authority binding's durable
-- evidence identity (MULTI-STRATEGY-RUNTIME-DISPATCH-01,
-- V4-STAGE-B-M2-C1-C3-REPAIR-04 Patch D2).
--
-- Additive only -- 0072 is not modified. Migration 0072's
-- sys_explicit_multi_strategy_authority_bindings persisted only a subset of
-- mqk_portfolio::SelectionCandidateEvidence's fields. Independent review
-- found every field capable of changing a candidate's authorization,
-- provenance validity, semantic identity, or evidence interpretation must
-- either be durably persisted and bound into authority_id, or have a
-- code-backed proof it cannot affect this mechanism. This migration adds
-- the columns for every previously-omitted evidence field so the complete
-- shape can be persisted and bound (see
-- mqk-daemon::multi_strategy_runtime_dispatch::derive_explicit_multi_strategy_authority_id).
--
-- Every new column is nullable-or-defaulted so this migration is safe to
-- re-run and never corrupts any row inserted under 0072's original, narrower
-- shape (there is no production data yet for this brand-new table, but the
-- migration itself makes no assumption about that).

ALTER TABLE sys_explicit_multi_strategy_authority_bindings
    ADD COLUMN IF NOT EXISTS promotion_expired boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS evidence_resolved boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS review_state_is_paper_candidate boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS evidence_review_state text NULL,
    ADD COLUMN IF NOT EXISTS durable_legacy_fingerprint text NULL,
    ADD COLUMN IF NOT EXISTS recomputed_legacy_fingerprint text NULL,
    ADD COLUMN IF NOT EXISTS legacy_fingerprint_matches boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS durable_exact_fingerprint_v2 text NULL,
    ADD COLUMN IF NOT EXISTS recomputed_exact_fingerprint_v2 text NULL,
    ADD COLUMN IF NOT EXISTS exact_fingerprint_v2_matches boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS plugin_instantiable boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS timeframe_matches boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS canonical_score_decimal text NULL,
    ADD COLUMN IF NOT EXISTS canonical_score_micros bigint NULL,
    ADD COLUMN IF NOT EXISTS scanner_rank integer NULL,
    ADD COLUMN IF NOT EXISTS watchlist_assigned boolean NOT NULL DEFAULT false,
    ADD COLUMN IF NOT EXISTS evidence_review_id text NULL,
    ADD COLUMN IF NOT EXISTS evidence_scanner_scan_id text NULL,
    ADD COLUMN IF NOT EXISTS evidence_artifact_path text NULL,
    ADD COLUMN IF NOT EXISTS evidence_git_hash text NULL,
    ADD COLUMN IF NOT EXISTS promotion_effective_at text NULL,
    ADD COLUMN IF NOT EXISTS promotion_expires_at text NULL,
    ADD COLUMN IF NOT EXISTS exact_reason text NULL;
