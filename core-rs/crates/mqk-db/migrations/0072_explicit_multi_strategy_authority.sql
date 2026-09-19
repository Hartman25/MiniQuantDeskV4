-- 0072: Explicit multi-strategy runtime authority durable evidence
-- (MULTI-STRATEGY-RUNTIME-DISPATCH-01, V4-STAGE-B-M2-REPAIR-03 Patch C2).
--
-- Two additive tables. Does not modify any existing table, and is wholly
-- separate from sys_dynamic_selection_plans/* (migration 0059/0060), which
-- represents one Bundle 7 ranking plan -- this schema represents the
-- explicit watchlist-v3 per-symbol multi-strategy authorization mechanism
-- instead (frozen contract:
-- docs/specs/multi_strategy_runtime_dispatch_01a_frozen_contract.md). Never
-- store v3 authority as though it were a Bundle 7 DynamicSelectionPlan.
--
-- sys_explicit_multi_strategy_authority: one immutable durable row per
-- resolved explicit authority. authority_id is a caller-minted deterministic
-- UUIDv5 binding every result-affecting input (run_id, source artifact
-- identity/hash, semantic config fingerprint, market_date, and every
-- authorized binding's own evidence) -- a changed source artifact or a
-- changed config/evidence fact can never reuse a stale authority_id, and
-- re-persisting the same logical authority is idempotent by construction.
-- approved_for_live is constrained false at the DB level in addition to
-- always being constructed false by the writer.
--
-- sys_explicit_multi_strategy_authority_bindings: one immutable row per
-- (symbol, strategy_id, timeframe_secs) binding the artifact named,
-- authorized or not -- an unauthorized binding still gets a row (authorized
-- = false, with its own reason_code), so a refused sibling's evidence is
-- never lost to an absent row. UNIQUE(authority_id, symbol, strategy_id,
-- timeframe_secs) enforces at the DB level that the exact same binding can
-- never appear twice under one authority.
--
-- No DEFAULT now() or DEFAULT gen_random_uuid() -- callers supply every
-- timestamp and identity.

CREATE TABLE IF NOT EXISTS sys_explicit_multi_strategy_authority (
    authority_id                uuid                     NOT NULL,
    run_id                      uuid                     NOT NULL REFERENCES runs(run_id),
    source_kind                 text                     NOT NULL,
    source_identity             text                     NOT NULL,
    source_artifact_hash        text                     NOT NULL,
    config_fingerprint          text                     NOT NULL,
    market_date                 text                     NOT NULL,
    approved_for_live           boolean                  NOT NULL,
    binding_count               integer                  NOT NULL,
    authorized_count            integer                  NOT NULL,
    writer_version              text                     NOT NULL,
    created_at_utc              timestamp with time zone NOT NULL,
    CONSTRAINT sys_explicit_multi_strategy_authority_pkey PRIMARY KEY (authority_id),
    CONSTRAINT sys_explicit_multi_strategy_authority_source_kind_check
        CHECK (source_kind = 'explicit_watchlist_v3'),
    CONSTRAINT sys_explicit_multi_strategy_authority_approved_for_live_check
        CHECK (approved_for_live = false),
    CONSTRAINT sys_explicit_multi_strategy_authority_counts_check
        CHECK (
            binding_count >= 0 AND authorized_count >= 0
            AND authorized_count <= binding_count
        )
);

CREATE INDEX IF NOT EXISTS idx_explicit_multi_strategy_authority_run
    ON sys_explicit_multi_strategy_authority (run_id, created_at_utc DESC);

CREATE TABLE IF NOT EXISTS sys_explicit_multi_strategy_authority_bindings (
    authority_id                uuid    NOT NULL
        REFERENCES sys_explicit_multi_strategy_authority(authority_id),
    ordinal                      integer NOT NULL,
    symbol                       text    NOT NULL,
    strategy_id                  text    NOT NULL,
    timeframe_secs               bigint  NOT NULL,
    authorized                   boolean NOT NULL,
    reason_code                  text    NOT NULL,
    promotion_query_ok           boolean NOT NULL,
    promotion_state              text    NULL,
    promotion_effective          boolean NOT NULL,
    config_identity_verified     boolean NOT NULL,
    durable_config_fingerprint   text    NULL,
    current_config_fingerprint   text    NULL,
    registry_enabled             boolean NOT NULL,
    data_ready                   boolean NOT NULL,
    promotion_transition_id      text    NULL,
    evidence_transition_id       text    NULL,
    CONSTRAINT sys_explicit_multi_strategy_authority_bindings_pkey
        PRIMARY KEY (authority_id, ordinal),
    CONSTRAINT sys_explicit_multi_strategy_authority_bindings_exact_key_unique
        UNIQUE (authority_id, symbol, strategy_id, timeframe_secs)
);

CREATE INDEX IF NOT EXISTS idx_explicit_multi_strategy_authority_bindings_authority
    ON sys_explicit_multi_strategy_authority_bindings (authority_id, symbol, strategy_id, timeframe_secs);
