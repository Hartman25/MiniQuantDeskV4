-- PROMOTION-EVIDENCE-LINEAGE-V3: extends migration 0065's durable lineage
-- columns on sys_strategy_promotion_transitions so a transition can later
-- prove the EXACT evidence judged, not merely identity pointers + the two
-- judged statistics 0065 already recorded. Migration 0065's five columns
-- (research_trial_id, research_economic_eval_id,
-- research_deflated_sharpe_ratio, research_probability_backtest_overfitting,
-- backtest_run_id) are untouched.
--
-- Every new hash below reuses an EXISTING accepted audit hash already
-- computed and verified elsewhere in the evidence chain -- never a new
-- parallel hash invented for this migration alone:
--
--   research_judge_artifact_sha256       <- research_judge_artifacts.
--                                            judge_artifact_sha256 (the
--                                            Research SQLite registry's own
--                                            primary key), already verified
--                                            by verify_promotion_oos_evidence
--                                            / VerifiedResearchAuthority.
--   stress_artifact_sha256               <- stress_suite.json's own
--                                            `stress_suite_sha256` audit
--                                            hash (mqk_artifacts::
--                                            stress_suite_artifact), already
--                                            verified by
--                                            load_canonical_stress_suite.
--   finalized_robustness_artifact_sha256 <- robustness_gauntlet.json's own
--                                            `robustness_gauntlet_sha256`
--                                            audit hash (mqk_artifacts::
--                                            robustness_gauntlet_artifact),
--                                            already verified by
--                                            load_canonical_robustness_gauntlet.
--
-- stress_protocol_version / robustness_protocol_version record the exact
-- protocol identity already carried by StressSuiteResult::protocol_version /
-- RobustnessEvidence::protocol_version and independently re-checked by
-- evaluate_promotion against REQUIRED_STRESS_PROTOCOL_VERSION /
-- REQUIRED_ROBUSTNESS_PROTOCOL_VERSION -- durably recorded here for the
-- first time so a later audit does not have to re-resolve the artifact just
-- to learn which protocol authorized the decision.
--
-- promotion_policy_fingerprint is the one genuinely NEW fingerprint this
-- migration introduces: PromotionConfig (min_sharpe/max_mdd/min_cagr/
-- min_profit_factor/min_profitable_months_pct/min_deflated_sharpe_ratio/
-- max_probability_backtest_overfitting) is assembled fresh from trusted
-- daemon config on every request and was never durably hashed anywhere
-- before -- see mqk_promotion::PromotionConfig::deterministic_fingerprint,
-- which follows the SAME canonical-byte-buffer-then-SHA-256 pattern already
-- established by evidence_fingerprint_v2.
--
-- All six columns are nullable: schema-safety only (no backfill decision
-- required for any pre-existing row) -- the route populates all six for
-- every NEW evidence-bearing transition from this point forward. No
-- existing column, row, or constraint from migration 0046/0047/0058/0065 is
-- altered.

alter table sys_strategy_promotion_transitions
    add column if not exists research_judge_artifact_sha256 text,
    add column if not exists stress_protocol_version text,
    add column if not exists stress_artifact_sha256 text,
    add column if not exists robustness_protocol_version text,
    add column if not exists finalized_robustness_artifact_sha256 text,
    add column if not exists promotion_policy_fingerprint text;
