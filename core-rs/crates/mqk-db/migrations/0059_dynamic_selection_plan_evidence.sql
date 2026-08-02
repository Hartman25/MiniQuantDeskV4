-- 0059: Dynamic strategy/symbol selection plan evidence (DYNAMIC-STRATEGY-
-- SYMBOL-SELECTION-01 Phase 7C, Part 1).
--
-- Three additive tables. Does not modify any existing table.
--
-- sys_dynamic_selection_plans: one immutable durable row per resolved
-- mqk_portfolio::dynamic_selection::DynamicSelectionPlan. plan_id is the
-- deterministic UUIDv5 minted by
-- mqk-daemon::dynamic_selection_dispatch_authority::derive_dynamic_selection_plan_id
-- from mqk_portfolio::dynamic_selection::canonical_plan_identity_material, so
-- re-persisting the same logical plan is idempotent by construction. Written
-- only when mode is 'shadow' or 'paper_enforced' -- 'off' never writes here.
-- approved_for_live is constrained false at the DB level: this schema can
-- never durably record a live-approved plan.
--
-- sys_dynamic_selection_plan_symbols: one immutable row per
-- SymbolSelectionResult -- always present for every eligible symbol in the
-- plan, even a symbol with zero candidates (e.g. NoValidCandidate), so the
-- symbol-level disposition/reason/exact_reason facts that
-- canonical_plan_identity_material folds in are never lost to an absent
-- candidate row.
--
-- sys_dynamic_selection_plan_candidates: one immutable row per
-- SelectionCandidateResult -- every result-affecting evidence field
-- canonical_plan_identity_material serializes, so the read-side validator
-- (Part 3) can reconstruct the exact plan and recompute plan_id
-- independently, never trusting a stored identity value on its own.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid() -- callers supply every
-- timestamp and identity.

CREATE TABLE IF NOT EXISTS sys_dynamic_selection_plans (
    plan_id                     uuid                     NOT NULL,
    run_id                      uuid                     NOT NULL REFERENCES runs(run_id),
    library_schema_version      text                     NOT NULL,
    context_schema_version      text                     NOT NULL,
    configured_mode             text                     NOT NULL,
    effective_mode              text                     NOT NULL,
    live_lock_applied           boolean                  NOT NULL,
    approved_for_live           boolean                  NOT NULL,
    source_kind                 text                     NOT NULL,
    source_identity              text                     NOT NULL,
    market_date                 text                     NOT NULL,
    -- The DynamicSelectionStartGateDisposition this plan was resolved
    -- under: 'shadow_allowed' | 'shadow_invalid' | 'paper_enforced_allowed'
    -- | 'paper_enforced_refused'. 'off' is excluded -- Off never builds a
    -- plan, so no row is ever written for it. This is distinct from
    -- effective_mode: a 'paper_enforced_refused' row still carries
    -- effective_mode = 'paper_enforced', so refusal evidence is never
    -- confused with an allowed header for the same mode.
    disposition                 text                     NOT NULL,
    truth_state                 text                     NOT NULL,
    blockers                    text[]                   NOT NULL,
    symbol_count                integer                  NOT NULL,
    candidate_count             integer                  NOT NULL,
    selected_count              integer                  NOT NULL,
    refused_count               integer                  NOT NULL,
    writer_version               text                     NOT NULL,
    created_at_utc               timestamp with time zone NOT NULL,
    CONSTRAINT sys_dynamic_selection_plans_pkey PRIMARY KEY (plan_id),
    CONSTRAINT sys_dynamic_selection_plans_configured_mode_check
        CHECK (configured_mode IN ('off', 'shadow', 'paper_enforced')),
    CONSTRAINT sys_dynamic_selection_plans_effective_mode_check
        CHECK (effective_mode IN ('off', 'shadow', 'paper_enforced')),
    CONSTRAINT sys_dynamic_selection_plans_disposition_check
        CHECK (disposition IN (
            'shadow_allowed', 'shadow_invalid',
            'paper_enforced_allowed', 'paper_enforced_refused'
        )),
    CONSTRAINT sys_dynamic_selection_plans_approved_for_live_check
        CHECK (approved_for_live = false),
    CONSTRAINT sys_dynamic_selection_plans_counts_check
        CHECK (
            symbol_count >= 0 AND candidate_count >= 0
            AND selected_count >= 0 AND refused_count >= 0
            AND selected_count <= symbol_count
            AND selected_count <= candidate_count
            AND refused_count <= candidate_count
        )
);

CREATE INDEX IF NOT EXISTS idx_dynamic_selection_plans_run
    ON sys_dynamic_selection_plans (run_id, created_at_utc DESC);

CREATE TABLE IF NOT EXISTS sys_dynamic_selection_plan_symbols (
    plan_id                   uuid    NOT NULL
        REFERENCES sys_dynamic_selection_plans(plan_id),
    symbol                    text    NOT NULL,
    selected_strategy_id      text    NULL,
    disposition               text    NOT NULL,
    reason_code               text    NOT NULL,
    exact_reason_code         text    NOT NULL,
    CONSTRAINT sys_dynamic_selection_plan_symbols_pkey
        PRIMARY KEY (plan_id, symbol),
    CONSTRAINT sys_dynamic_selection_plan_symbols_disposition_check
        CHECK (disposition IN ('selected', 'not_selected', 'refused')),
    CONSTRAINT sys_dynamic_selection_plan_symbols_selected_coherence_check
        CHECK (
            (disposition = 'selected' AND selected_strategy_id IS NOT NULL)
            OR (disposition <> 'selected' AND selected_strategy_id IS NULL)
        )
);

CREATE INDEX IF NOT EXISTS idx_dynamic_selection_plan_symbols_plan
    ON sys_dynamic_selection_plan_symbols (plan_id, symbol);

CREATE TABLE IF NOT EXISTS sys_dynamic_selection_plan_candidates (
    plan_id                          uuid    NOT NULL
        REFERENCES sys_dynamic_selection_plans(plan_id),
    ordinal                          integer NOT NULL,
    symbol                           text    NOT NULL,
    strategy_id                      text    NOT NULL,
    timeframe_secs                   bigint  NOT NULL,
    canonical_score_decimal          text    NULL,
    canonical_score_micros           bigint  NULL,
    scanner_rank                     integer NULL,
    watchlist_assigned               boolean NOT NULL,
    promotion_query_ok               boolean NOT NULL,
    promotion_state                  text    NULL,
    promotion_effective               boolean NOT NULL,
    promotion_expired                boolean NOT NULL,
    evidence_resolved                boolean NOT NULL,
    review_state_is_paper_candidate  boolean NOT NULL,
    evidence_review_state            text    NULL,
    durable_legacy_fingerprint       text    NULL,
    recomputed_legacy_fingerprint    text    NULL,
    legacy_fingerprint_matches       boolean NOT NULL,
    durable_exact_fingerprint_v2     text    NULL,
    recomputed_exact_fingerprint_v2  text    NULL,
    exact_fingerprint_v2_matches     boolean NOT NULL,
    registry_enabled                 boolean NOT NULL,
    plugin_instantiable              boolean NOT NULL,
    timeframe_matches                boolean NOT NULL,
    data_ready                       boolean NOT NULL,
    evidence_review_id               text    NULL,
    evidence_scanner_scan_id         text    NULL,
    evidence_artifact_path           text    NULL,
    evidence_git_hash                text    NULL,
    promotion_transition_id          text    NULL,
    promotion_effective_at           text    NULL,
    promotion_expires_at             text    NULL,
    evidence_transition_id           text    NULL,
    exact_reason_code                text    NOT NULL,
    selected                         boolean NOT NULL,
    disposition                      text    NOT NULL,
    reason_code                      text    NOT NULL,
    CONSTRAINT sys_dynamic_selection_plan_candidates_pkey
        PRIMARY KEY (plan_id, ordinal),
    CONSTRAINT sys_dynamic_selection_plan_candidates_symbol_fkey
        FOREIGN KEY (plan_id, symbol)
        REFERENCES sys_dynamic_selection_plan_symbols(plan_id, symbol),
    CONSTRAINT sys_dynamic_selection_plan_candidates_disposition_check
        CHECK (disposition IN ('selected', 'not_selected', 'refused'))
);

CREATE INDEX IF NOT EXISTS idx_dynamic_selection_plan_candidates_plan
    ON sys_dynamic_selection_plan_candidates (plan_id, symbol, ordinal);
