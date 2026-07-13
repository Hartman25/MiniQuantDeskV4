-- STRATEGY-PROMOTION-REGISTRY-01B: durable, append-only strategy paper-
-- promotion registry.
--
-- sys_strategy_promotion_transitions is the authoritative history of every
-- promotion state transition for an exact (strategy_id, symbol,
-- timeframe_secs) identity. There is no separate mutable "current state"
-- row: current state is always derived by querying the latest transition
-- per identity (see mqk-db/src/strategy_promotion.rs
-- fetch_current_promotion_state). This avoids a dual-write consistency
-- hazard between a history table and a materialized current-state table.
--
-- registered + enabled (sys_strategy_registry.enabled) is NOT promotion
-- approval. This table is empty by default after migration — an empty
-- table is authoritative "no strategy is approved for autonomous paper
-- execution", not an unavailable/unproven state. No backfill from
-- sys_strategy_registry is performed by this migration or any code in
-- this patch.
--
-- Identity is scoped to strategy_id + symbol + timeframe_secs (identity-v1;
-- see docs/specs/strategy_promotion_registry_01a_current_truth_and_contract.md
-- section 3). config_fingerprint is a nullable bounded-fallback column,
-- always NULL in this patch; config_identity_status defaults to
-- 'unavailable_in_current_runtime' and is not currently set to any other
-- value by any writer.
--
-- Timestamps and transition_id are caller-injected per determinism rules
-- (no DEFAULT now(), no DEFAULT gen_random_uuid()). transition_id is a
-- deterministic UUIDv5 derived by the daemon route layer from the full
-- transition request content, so a replayed identical request is
-- idempotent via ON CONFLICT (transition_id) DO NOTHING.
--
-- The CHECK constraint on (previous_state, new_state) is a structural
-- backstop for the legal transition graph documented in the design doc.
-- It cannot verify that a caller's claimed previous_state matches the
-- actual current state for that identity -- that ordering guarantee is
-- enforced by the daemon route layer, which always computes the current
-- state itself before constructing an insert (never trusts a
-- caller-supplied previous_state).

create table if not exists sys_strategy_promotion_transitions (
    transition_id            uuid        primary key,
    strategy_id               text        not null,
    symbol                    text        not null,
    timeframe_secs            bigint      not null,
    config_fingerprint        text,
    config_identity_status    text        not null default 'unavailable_in_current_runtime',
    previous_state            text,
    new_state                 text        not null,
    evidence_review_id        text,
    evidence_scanner_scan_id  text,
    evidence_git_hash         text,
    evidence_artifact_path    text,
    evidence_fingerprint      text,
    effective_at_utc          timestamptz not null,
    expires_at_utc            timestamptz,
    initiated_by              text        not null,
    reason                    text        not null default '',
    created_at_utc            timestamptz not null,

    constraint sys_strategy_promotion_transitions_strategy_id_not_blank
        check (length(trim(strategy_id)) > 0),
    constraint sys_strategy_promotion_transitions_symbol_not_blank
        check (length(trim(symbol)) > 0),
    constraint sys_strategy_promotion_transitions_symbol_not_wildcard
        check (symbol <> '*'),
    constraint sys_strategy_promotion_transitions_timeframe_positive
        check (timeframe_secs > 0),
    constraint sys_strategy_promotion_transitions_initiated_by_not_blank
        check (length(trim(initiated_by)) > 0),
    constraint sys_strategy_promotion_transitions_new_state_valid
        check (new_state in (
            'shadow_approved', 'paper_approved', 'active_paper',
            'demoted', 'retired', 'rejected'
        )),
    constraint sys_strategy_promotion_transitions_previous_state_valid
        check (previous_state is null or previous_state in (
            'shadow_approved', 'paper_approved', 'active_paper',
            'demoted', 'retired', 'rejected'
        )),
    -- Legal transition graph (structural backstop; see design doc section 6).
    -- retired/rejected never appear as previous_state -- both terminal (the
    -- ELSE branch below rejects them, and any other unrecognized value).
    --
    -- Uses CASE/WHEN rather than an OR-chain of `previous_state = 'x'`
    -- comparisons deliberately: in SQL three-valued logic, `NULL = 'x'`
    -- evaluates to NULL (not FALSE), and a CHECK constraint PASSES when its
    -- expression evaluates to NULL/UNKNOWN. An OR-chain would therefore let
    -- previous_state IS NULL rows through with *any* new_state once even one
    -- branch's equality comparison went NULL. CASE/WHEN's first matching
    -- branch (`previous_state is null`) is evaluated as an explicit IS NULL
    -- test, not an equality comparison, so it can never spuriously pass via
    -- NULL propagation the way the OR-chain did.
    constraint sys_strategy_promotion_transitions_legal_graph
        check (
            case
                when previous_state is null then new_state = 'shadow_approved'
                when previous_state = 'shadow_approved' then new_state in ('paper_approved', 'rejected', 'retired')
                when previous_state = 'paper_approved' then new_state in ('active_paper', 'demoted', 'retired')
                when previous_state = 'active_paper' then new_state in ('demoted', 'retired')
                when previous_state = 'demoted' then new_state in ('shadow_approved', 'retired')
                else false
            end
        )
);

-- Supports the deterministic "current state" query: latest transition per
-- identity, ordered by effective_at_utc desc (tie-broken by created_at_utc
-- desc, then transition_id desc for full determinism).
create index if not exists sys_strategy_promotion_transitions_identity_idx
    on sys_strategy_promotion_transitions (strategy_id, symbol, timeframe_secs, effective_at_utc desc, created_at_utc desc);

-- Supports full-history-ordered reads (GET .../promotions/history).
create index if not exists sys_strategy_promotion_transitions_effective_idx
    on sys_strategy_promotion_transitions (effective_at_utc desc);

-- Supports evidence-provenance lookups/audits by review_id.
create index if not exists sys_strategy_promotion_transitions_evidence_review_idx
    on sys_strategy_promotion_transitions (evidence_review_id)
    where evidence_review_id is not null;
