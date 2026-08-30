-- DYNAMIC-SELECTION-CONFIG-ELIGIBILITY-CLOSURE-01: extends migration 0059's
-- sys_dynamic_selection_plan_candidates with the config-identity gate result
-- (mqk_portfolio::dynamic_selection::SelectionCandidateEvidence::
-- config_identity_verified / durable_config_fingerprint /
-- current_config_fingerprint) -- reuses the SAME canonical config-identity
-- comparison mechanism C1/C2/R2 already established
-- (config_identity_status == 'verified_v1' + structurally valid + byte-equal
-- fingerprints), never a new parallel policy.
--
-- All three columns are nullable: schema-safety only (no backfill decision
-- required for any pre-existing row, mirroring migration 0066's identical
-- precedent) -- the writer populates all three for every NEW plan candidate
-- row from this point forward. A NULL config_identity_verified on a
-- pre-migration row is read back as fail-closed `false` by the daemon-side
-- read model, never optimistically treated as verified. No existing column,
-- row, or constraint from migration 0059/0060 is altered.

alter table sys_dynamic_selection_plan_candidates
    add column if not exists config_identity_verified boolean,
    add column if not exists durable_config_fingerprint text,
    add column if not exists current_config_fingerprint text;
