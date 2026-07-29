-- DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-FOUNDATION-AUTHORITY-CLOSURE-02
-- Defect B: add nullable evidence_fingerprint_v2 to
-- sys_strategy_promotion_transitions, additive, does not modify 0046/0047.
--
-- The legacy evidence_fingerprint (0046) is a SHA-256 of the matched review
-- row serialized *after* its scanner_score has already been deserialized to
-- f64 -- two distinct raw JSON decimal tokens can collide to the same f64
-- (and therefore the same legacy fingerprint) while producing different
-- exact Bundle 7 rankings. evidence_fingerprint_v2 is a versioned SHA-256
-- computed directly from the exact raw score token (never through f64) plus
-- every other matched-row field affecting evidence identity -- see
-- mqk-daemon's promotion_evidence_validation::compute_evidence_fingerprint_v2.
--
-- Legacy rows (inserted before this migration) round-trip
-- evidence_fingerprint_v2 as NULL -- never backfilled from the mutable
-- review-artifact filesystem at read time. Bundle 7's read-side validator
-- (validate_active_paper_candidate) refuses any identity whose current
-- evidence-bearing transition lacks a v2 fingerprint with a distinct
-- durable_fingerprint_v2_missing reason, requiring a new evidence-bearing
-- operator transition (e.g. a fresh shadow_approved re-approval) rather than
-- silently trusting the legacy-only row for ranking.

alter table sys_strategy_promotion_transitions
    add column if not exists evidence_fingerprint_v2 text;
