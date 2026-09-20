-- MULTI-STRATEGY-RUNTIME-DISPATCH-01 / autonomous completed-bar driver
-- multi-binding repair (R2B, V4-BULK-CODE-COMPLETION-STAGE-B-M2-01 C4):
-- extend migration 0071's closed binding-local reason-code vocabulary with
-- one new value.
--
-- Additive only -- 0071 is not modified. Widens
-- sys_autonomous_daily_binding_state_reason_known (drop + re-add by the same
-- name, both guarded by IF EXISTS/idempotent, so a re-run is a no-op) to
-- also accept 'binding_strategy_engine_not_active'.
--
-- Why this reason is new and distinct from 0071's original five: the
-- autonomous daily-operations runtime has exactly one active native-strategy
-- engine per process (Tier A single-strategy policy --
-- `mqk_runtime::native_strategy::NativeStrategyBootstrap::bootstrap` docs:
-- "Multi-strategy fleet execution is deferred to a later patch"). A
-- multi-symbol assignment config (watchlist-v2, or any future multi-binding
-- source) may legitimately name a `strategy_id` for one of its symbols that
-- differs from the one strategy the process actually bootstrapped. That
-- specific binding can never be dispatched through this path until the
-- single-engine limitation itself is lifted -- it is a real, durable,
-- operator-visible binding-local fault (not a global-critical one: every
-- *other* configured binding whose strategy_id does match the active engine
-- must continue to progress independently), and none of 0071's five reasons
-- describe it honestly:
--   - not `binding_no_new_bar_waiting` -- there is no new-bar wait; the
--     binding is not runnable on this engine at all;
--   - not `binding_market_data_missing_or_stale` -- no data-freshness issue;
--   - not `binding_readiness_blocked` -- Bundle 2 readiness is not evaluated
--     for a mismatched binding at all, so it never reaches that check;
--   - not `binding_unsupported_or_invalid_symbol_timeframe` -- the symbol
--     and timeframe are both valid; only the strategy binding is wrong;
--   - not `binding_provider_failure_isolated` -- no provider call is
--     involved.

alter table sys_autonomous_daily_binding_state
    drop constraint if exists sys_autonomous_daily_binding_state_reason_known;

alter table sys_autonomous_daily_binding_state
    add constraint sys_autonomous_daily_binding_state_reason_known
        check (
            reason_code is null or reason_code in (
                'binding_no_new_bar_waiting',
                'binding_market_data_missing_or_stale',
                'binding_readiness_blocked',
                'binding_unsupported_or_invalid_symbol_timeframe',
                'binding_provider_failure_isolated',
                'binding_strategy_engine_not_active'
            )
        );
