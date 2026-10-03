-- V4-M1-NATIVE-SIZING-V1-INDEPENDENT-REVIEW-CORRECTION-01 / IR-SZ-02:
-- restart-recoverable held-quantity state for FixedInitialCapitalFractionV1.
--
-- The capital-fraction wrapper resolves an entry quantity Q ONCE, at the causal
-- close of the completed entry bar, and re-emits exactly that Q while the
-- strategy stays long. That held Q must survive a process restart: it may not
-- be re-resolved from a later bar, reset to one share, or lost. One row per
-- (deployment, strategy, symbol) records the current/last entry.
--
-- * `entry_generation` increments on every genuine flat->long entry and orders
--   retries: the same generation with identical content is the same entry.
-- * `status` is `active` while the entry is held; an exit to flat transitions it
--   to `released` (the row stays as the generation floor and audit trail). A
--   later entry replaces the released row with generation + 1.
-- * Every identity field of the sizing contract (policy, fraction, immutable
--   initial allocated capital, caps) is stored with the entry so a stale or
--   foreign row is detected on recovery. Broker equity/buying power is never a
--   sizing input and is not stored here.
--
-- No SQL-generated timestamps or UUIDs and no operational defaults: every value
-- is supplied explicitly by the caller
-- (mqk_db::held_sizing_state::held_sizing_apply).

create table if not exists sys_strategy_held_sizing_state (
    deployment_id                    text        not null,
    strategy_id                      text        not null,
    symbol                           text        not null,
    state_version                    integer     not null,
    entry_generation                 bigint      not null,
    status                           text        not null,
    policy_id                        text        not null,
    allocation_fraction_bps          bigint      not null,
    initial_allocated_capital_micros bigint      not null,
    max_target_qty_micros            bigint,
    max_notional_usd                 bigint,
    resolved_target_qty_micros       bigint      not null,
    reference_bar_end_ts             bigint      not null,
    reference_price_micros           bigint      not null,
    recorded_at_utc                  timestamptz not null,
    released_at_utc                  timestamptz,

    primary key (deployment_id, strategy_id, symbol),

    constraint sys_strategy_held_sizing_deployment_not_blank
        check (length(trim(deployment_id)) > 0),
    constraint sys_strategy_held_sizing_strategy_not_blank
        check (length(trim(strategy_id)) > 0),
    constraint sys_strategy_held_sizing_symbol_not_blank
        check (length(trim(symbol)) > 0 and symbol = trim(symbol)),
    constraint sys_strategy_held_sizing_state_version_v1
        check (state_version = 1),
    constraint sys_strategy_held_sizing_generation_positive
        check (entry_generation >= 1),
    constraint sys_strategy_held_sizing_status_vocabulary
        check (status in ('active', 'released')),
    constraint sys_strategy_held_sizing_policy_v1
        check (policy_id = 'fixed_initial_capital_fraction_v1'),
    constraint sys_strategy_held_sizing_fraction_range
        check (allocation_fraction_bps between 1 and 10000),
    constraint sys_strategy_held_sizing_capital_positive
        check (initial_allocated_capital_micros > 0),
    constraint sys_strategy_held_sizing_caps_positive
        check ((max_target_qty_micros is null or max_target_qty_micros > 0)
           and (max_notional_usd is null or max_notional_usd > 0)),
    constraint sys_strategy_held_sizing_qty_positive_whole_share
        check (resolved_target_qty_micros > 0 and resolved_target_qty_micros % 1000000 = 0),
    constraint sys_strategy_held_sizing_reference_bar_positive
        check (reference_bar_end_ts > 0),
    constraint sys_strategy_held_sizing_reference_price_positive
        check (reference_price_micros > 0),
    constraint sys_strategy_held_sizing_release_shape
        check ((status = 'active' and released_at_utc is null)
            or (status = 'released' and released_at_utc is not null))
);
