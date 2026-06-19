//! MULTI-STRATEGY-RUNTIME-DRY-RUN-01: dry-run secondary strategy evaluator.
//!
//! Evaluates one or more *secondary* strategies against the same bar window
//! the primary strategy was just dispatched against, without ever submitting
//! an order. Every function in this module is synchronous and takes only
//! plain data (strategy id, symbol, bar window, current position) — there is
//! no `AppState`, `PgPool`, or broker handle anywhere in these signatures, so
//! it is structurally impossible for this module to call
//! [`crate::decision::submit_internal_strategy_decision`] or write an outbox
//! row. Diagnostics are a pure projection of "what would this strategy have
//! done", never an action.
//!
//! # Why a fresh `PluginRegistry` + `StrategyHost` per call
//!
//! [`mqk_strategy::StrategyHost`] enforces an architectural single-strategy
//! invariant (`MultiStrategyNotAllowed`) — see `mqk-strategy/src/host.rs`.
//! This patch does not weaken that invariant and does not change `StrategyHost`.
//! The primary strategy keeps its own dedicated
//! [`mqk_runtime::native_strategy::NativeStrategyBootstrap`] (one host, one
//! strategy, untouched by this module). Each dry-run evaluation builds its
//! *own* independent, throwaway registry + host pair, registers exactly one
//! strategy on it, runs `on_bar` once, and discards both. No `StrategyHost`
//! instance ever holds more than one strategy.
//!
//! # Default-off
//!
//! [`dry_run_strategy_ids_from_env`] returns an empty `Vec` unless
//! `MQK_DRY_RUN_STRATEGY_IDS` is set to a non-blank comma-separated list.
//! Callers must treat an empty list as "do nothing" — existing single-strategy
//! behavior is unchanged when the env var is absent.

use mqk_strategy::{PluginRegistry, RecentBarsWindow, ShadowMode, StrategyContext, StrategyHost};

use crate::capital_policy::{
    classify_order_intent, evaluate_short_entry_policy, order_intent_to_short_entry_intent,
    OrderIntent, ShortEntryConfig, ShortEntryPolicyDecision,
};

/// Env var: comma-separated list of strategy ids to evaluate in dry-run mode
/// alongside the primary strategy. Absent or blank → no dry-run strategies
/// (default; existing single-strategy behavior unchanged).
pub const DRY_RUN_STRATEGY_IDS_ENV: &str = "MQK_DRY_RUN_STRATEGY_IDS";

/// Read [`DRY_RUN_STRATEGY_IDS_ENV`] from the environment.
///
/// Returns an empty `Vec` when the variable is absent or blank. Entries are
/// comma-separated and trimmed; empty entries are dropped. Pure: no
/// validation against the strategy registry happens here — an unknown id
/// simply produces an `"evaluation_unavailable"` diagnostic later.
pub fn dry_run_strategy_ids_from_env() -> Vec<String> {
    std::env::var(DRY_RUN_STRATEGY_IDS_ENV)
        .ok()
        .map(|raw| {
            raw.split(',')
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .collect()
        })
        .unwrap_or_default()
}

/// Read-only diagnostic snapshot produced by evaluating one dry-run strategy
/// against one bar window.
///
/// `submitted` is always `false`. Nothing in this module holds a DB handle or
/// an `AppState` reference, so a value of this type can never have been
/// passed to [`crate::decision::submit_internal_strategy_decision`] or used
/// to enqueue an outbox row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DryRunStrategyDiagnostic {
    pub strategy_id: String,
    pub symbol: String,
    /// Strategy's own timeframe, in seconds (from `StrategySpec::timeframe_secs`).
    /// `0` when the strategy could not be evaluated (`decision = "evaluation_unavailable"`).
    pub timeframe_secs: i64,
    pub current_qty: i64,
    pub target_qty: i64,
    pub delta_qty: i64,
    /// One of: `"already_at_target"`, `"b5_short_sale_guard"`,
    /// `"order_would_be_submitted"`, `"evaluation_unavailable"`.
    pub decision: &'static str,
    pub reason: String,
    /// Stable label for the classified [`OrderIntent`] (e.g. `"ShortOpen"`,
    /// `"LongOpen"`, `"SellToClose"`, `"NoOp"`), or `"unavailable"` when the
    /// strategy could not be evaluated.
    pub would_classify_as: &'static str,
    /// `true` when this intent would be dropped by the B5 short-sale guard in
    /// `bar_result_to_decisions` (mirrors `OrderIntent::requires_short_entry_policy`,
    /// which is also the set of intents the default fail-closed short-entry
    /// policy would block).
    pub would_b5_block: bool,
    /// `true` when the default fail-closed short-entry policy
    /// (`ShortEntryConfig::fail_closed`, evaluated as if in paper mode) would
    /// also block this intent. Always `false` for non-short-open intents —
    /// the short-entry policy gate does not apply to them, mirroring
    /// `ShortEntryPolicyDecision::NotShortOpen`.
    pub would_policy_block: bool,
    /// Machine-readable short-entry policy block reason (e.g.
    /// `"short_entries_disabled"`), from `ShortEntryPolicyDecision::ShortOpenBlocked`.
    /// `None` when `would_policy_block` is `false`.
    pub policy_reason_code: Option<&'static str>,
    /// Always `false`. This struct is a diagnostic only; no order is ever
    /// submitted from a dry-run evaluation.
    pub submitted: bool,
}

fn intent_label(intent: OrderIntent) -> &'static str {
    match intent {
        OrderIntent::LongOpen => "LongOpen",
        OrderIntent::SellToClose => "SellToClose",
        OrderIntent::SellToFlat => "SellToFlat",
        OrderIntent::SellBeyondLongToShort => "SellBeyondLongToShort",
        OrderIntent::ShortOpen => "ShortOpen",
        OrderIntent::BuyToCover => "BuyToCover",
        OrderIntent::BuyToFlat => "BuyToFlat",
        OrderIntent::BuyBeyondShortToLong => "BuyBeyondShortToLong",
        OrderIntent::NoOp => "NoOp",
    }
}

fn unavailable(
    strategy_id: &str,
    symbol: &str,
    current_qty: i64,
    reason: String,
) -> DryRunStrategyDiagnostic {
    DryRunStrategyDiagnostic {
        strategy_id: strategy_id.trim().to_string(),
        symbol: symbol.trim().to_string(),
        timeframe_secs: 0,
        current_qty,
        target_qty: 0,
        delta_qty: 0,
        decision: "evaluation_unavailable",
        reason,
        would_classify_as: "unavailable",
        would_b5_block: false,
        would_policy_block: false,
        policy_reason_code: None,
        submitted: false,
    }
}

/// Evaluate the default fail-closed short-entry policy for a classified
/// [`OrderIntent`], purely (no IO, no env reads, no wall-clock).
///
/// Mirrors the `ShortEntryConfig::fail_closed()` cross-check already asserted
/// in `scenario_multi_strategy_runtime_dry_run_01.rs` (DR08) — this just makes
/// that cross-check part of the diagnostic itself instead of a side
/// assertion. Always evaluated as paper mode: a dry-run diagnostic answers
/// "would the default policy block this", and the paper-mode reason codes
/// (e.g. `"short_entries_disabled"`) are strictly more informative than the
/// unconditional live-mode block, which would otherwise mask them.
///
/// Non-short-open intents short-circuit to `(false, None)` without calling
/// the evaluator — the gate does not apply to them.
fn would_default_short_entry_policy_block(intent: OrderIntent) -> (bool, Option<&'static str>) {
    if !intent.requires_short_entry_policy() {
        return (false, None);
    }
    let short_intent = order_intent_to_short_entry_intent(intent);
    match evaluate_short_entry_policy(
        &ShortEntryConfig::fail_closed(),
        short_intent,
        true, // paper mode: see doc comment above
        None, // no broker shortable check is performed for a dry-run evaluation
        None, // projected_short_shares: not computed for a dry-run evaluation
        None, // projected_short_notional_usd: not computed for a dry-run evaluation
    ) {
        ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. } => (true, Some(reason_code)),
        ShortEntryPolicyDecision::ShortOpenAllowed { .. }
        | ShortEntryPolicyDecision::NotShortOpen => (false, None),
    }
}

/// Evaluate exactly one dry-run strategy against `recent`.
///
/// Builds a throwaway [`PluginRegistry`] + [`StrategyHost`] pair — independent
/// from the primary strategy's bootstrap — registers `strategy_id`, and calls
/// `on_bar` once. The strategy's own targets are filtered to `symbol` (the
/// same fail-closed guard the primary dispatch loop applies via
/// `AppState::retain_targets_matching_symbol`) before computing the delta
/// against `current_qty`.
///
/// Never submits a decision, never enqueues an outbox row, never touches a
/// broker — this function takes no DB/AppState/broker handle, so it has no
/// capability to do so.
pub fn evaluate_dry_run_strategy(
    strategy_id: &str,
    symbol: &str,
    now_tick: u64,
    recent: RecentBarsWindow,
    current_qty: i64,
) -> DryRunStrategyDiagnostic {
    let strategy_id = strategy_id.trim();
    let symbol = symbol.trim();

    let mut registry = PluginRegistry::new();
    if let Err(e) = mqk_strategy::engines::register_builtin_strategies(&mut registry, symbol) {
        return unavailable(
            strategy_id,
            symbol,
            current_qty,
            format!("dry-run registry build failed: {e}"),
        );
    }

    let instance = match registry.instantiate_verified(strategy_id) {
        Ok(s) => s,
        Err(e) => {
            return unavailable(
                strategy_id,
                symbol,
                current_qty,
                format!("dry-run strategy '{strategy_id}' unavailable: {e}"),
            )
        }
    };

    // Independent dry-run host: exactly one strategy, never shared with the
    // primary NativeStrategyBootstrap's StrategyHost.
    let mut host = StrategyHost::new(ShadowMode::Off);
    if let Err(e) = host.register(instance) {
        return unavailable(
            strategy_id,
            symbol,
            current_qty,
            format!("dry-run host registration failed: {e:?}"),
        );
    }

    let spec = match host.spec() {
        Ok(s) => s,
        Err(e) => {
            return unavailable(
                strategy_id,
                symbol,
                current_qty,
                format!("dry-run spec unavailable: {e:?}"),
            )
        }
    };

    let ctx = StrategyContext::new(spec.timeframe_secs, now_tick, recent);
    let bar_result = match host.on_bar(&ctx) {
        Ok(r) => r,
        Err(e) => {
            return unavailable(
                strategy_id,
                symbol,
                current_qty,
                format!("dry-run on_bar failed: {e:?}"),
            )
        }
    };

    let target_qty: i64 = bar_result
        .intents
        .output
        .targets
        .iter()
        .filter(|t| t.symbol.trim().eq_ignore_ascii_case(symbol))
        .map(|t| t.qty)
        .sum();

    let delta_qty = target_qty - current_qty;
    let intent = classify_order_intent(current_qty, delta_qty);
    let would_b5_block = intent.requires_short_entry_policy();
    let (would_policy_block, policy_reason_code) = would_default_short_entry_policy_block(intent);

    let (decision, reason): (&'static str, String) = if delta_qty == 0 {
        (
            "already_at_target",
            format!(
                "dry-run target_qty={target_qty} equals current_qty={current_qty}; \
                 no order would be submitted"
            ),
        )
    } else if would_b5_block {
        (
            "b5_short_sale_guard",
            format!(
                "dry-run delta={delta_qty} current={current_qty} intent={intent:?}: would open \
                 or extend a short position; the B5 guard in bar_result_to_decisions drops this \
                 intent before it reaches the outbox, and the default short-entry policy \
                 (ShortEntryConfig::fail_closed) blocks ShortOpen regardless of which strategy \
                 produced it"
            ),
        )
    } else {
        (
            "order_would_be_submitted",
            format!(
                "dry-run delta={delta_qty} current={current_qty} intent={intent:?}: would pass \
                 the B5 guard if '{strategy_id}' were the primary strategy; not submitted \
                 (dry-run only)"
            ),
        )
    };

    DryRunStrategyDiagnostic {
        strategy_id: strategy_id.to_string(),
        symbol: symbol.to_string(),
        timeframe_secs: spec.timeframe_secs,
        current_qty,
        target_qty,
        delta_qty,
        decision,
        reason,
        would_classify_as: intent_label(intent),
        would_b5_block,
        would_policy_block,
        policy_reason_code,
        submitted: false,
    }
}

/// Evaluate every id in `strategy_ids` against the same `recent` window.
///
/// Empty input → empty output (no-op). Each strategy id gets its own
/// independent registry/host pair via [`evaluate_dry_run_strategy`]; none of
/// them observe or affect one another.
pub fn evaluate_dry_run_strategies(
    strategy_ids: &[String],
    symbol: &str,
    now_tick: u64,
    recent: &RecentBarsWindow,
    current_qty: i64,
) -> Vec<DryRunStrategyDiagnostic> {
    strategy_ids
        .iter()
        .map(|id| evaluate_dry_run_strategy(id, symbol, now_tick, recent.clone(), current_qty))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_strategy::BarStub;

    fn bar(close_micros: i64, is_complete: bool) -> BarStub {
        BarStub::new(0, is_complete, close_micros, 1)
    }

    fn bullish_window() -> RecentBarsWindow {
        let base = 200_000_000i64; // $200
        RecentBarsWindow::new(
            10,
            vec![
                bar(base, true),
                bar(base, true),
                bar(base, true),
                bar(base, true),
                bar(base + base / 400, true), // +25 bps >= 20 bps threshold
            ],
        )
    }

    fn bearish_window() -> RecentBarsWindow {
        let base = 200_000_000i64;
        RecentBarsWindow::new(
            10,
            vec![
                bar(base + base / 400, true),
                bar(base + base / 400, true),
                bar(base + base / 400, true),
                bar(base + base / 400, true),
                bar(base, true), // -25 bps <= -20 bps threshold
            ],
        )
    }

    // --- DRS-01/DRS-02: env parsing ------------------------------------------
    //
    // Both cases share one test function (not split across two `#[test]`
    // fns) because `cargo test` runs tests in parallel threads within one
    // process by default; two functions mutating the same env var would
    // race. Keeping the remove -> assert -> set -> assert -> remove sequence
    // inside a single function matches the existing convention in
    // `engines/intraday_scalper.rs` (e.g. `sps05_env_invalid_defaults_to_1`).
    #[test]
    fn drs01_02_env_absent_is_empty_and_parses_trimmed_comma_separated_list() {
        std::env::remove_var(DRY_RUN_STRATEGY_IDS_ENV);
        assert!(
            dry_run_strategy_ids_from_env().is_empty(),
            "DRS-01: absent env var -> empty Vec (default off)"
        );

        std::env::set_var(
            DRY_RUN_STRATEGY_IDS_ENV,
            " intraday_short_scalper , , swing_momentum ",
        );
        let ids = dry_run_strategy_ids_from_env();
        std::env::remove_var(DRY_RUN_STRATEGY_IDS_ENV);
        assert_eq!(
            ids,
            vec![
                "intraday_short_scalper".to_string(),
                "swing_momentum".to_string()
            ],
            "DRS-02: comma-separated entries trimmed; blanks dropped"
        );
    }

    // --- DRS-03..06: core evaluator behavior --------------------------------

    /// DRS-03: short dry-run strategy on a bearish window from flat ->
    /// negative target, classified ShortOpen, B5-blocked by default.
    #[test]
    fn drs03_short_strategy_bearish_from_flat_is_short_open_and_b5_blocked() {
        let diag = evaluate_dry_run_strategy(
            "intraday_short_scalper",
            "AAPL",
            0,
            bearish_window(),
            0, // flat
        );
        assert_eq!(
            diag.would_classify_as, "ShortOpen",
            "DRS-03: classify ShortOpen"
        );
        assert!(diag.would_b5_block, "DRS-03: B5 would block a short-open");
        assert_eq!(
            diag.decision, "b5_short_sale_guard",
            "DRS-03: decision reflects B5 block"
        );
        assert!(
            diag.target_qty < 0,
            "DRS-03: short strategy target is negative"
        );
        assert!(!diag.submitted, "DRS-03: submitted is always false");
    }

    /// DRS-04: long dry-run strategy on a bullish window from flat -> positive
    /// target, classified LongOpen, not B5-blocked.
    #[test]
    fn drs04_long_strategy_bullish_from_flat_is_long_open_not_blocked() {
        let diag = evaluate_dry_run_strategy("intraday_scalper", "AAPL", 0, bullish_window(), 0);
        assert_eq!(
            diag.would_classify_as, "LongOpen",
            "DRS-04: classify LongOpen"
        );
        assert!(!diag.would_b5_block, "DRS-04: LongOpen is never B5-blocked");
        assert_eq!(
            diag.decision, "order_would_be_submitted",
            "DRS-04: would-submit decision for an eligible long open"
        );
        assert!(!diag.submitted, "DRS-04: submitted is always false");
    }

    /// DRS-05: already-at-target produces no_order regardless of strategy.
    #[test]
    fn drs05_already_at_target_is_no_order() {
        // Short strategy target on a bearish window is -1 (default sizing);
        // starting already at -1 means delta=0.
        let diag =
            evaluate_dry_run_strategy("intraday_short_scalper", "AAPL", 0, bearish_window(), -1);
        assert_eq!(diag.delta_qty, 0, "DRS-05: precondition delta=0");
        assert_eq!(
            diag.decision, "already_at_target",
            "DRS-05: delta=0 -> already_at_target"
        );
        assert!(!diag.would_b5_block, "DRS-05: no order means no B5 concern");
    }

    /// DRS-06: unknown strategy id is fail-closed (`evaluation_unavailable`),
    /// not a panic.
    #[test]
    fn drs06_unknown_strategy_id_is_evaluation_unavailable() {
        let diag = evaluate_dry_run_strategy("not_a_real_strategy", "AAPL", 0, bullish_window(), 0);
        assert_eq!(
            diag.decision, "evaluation_unavailable",
            "DRS-06: unknown id is fail-closed, not a panic"
        );
        assert!(!diag.submitted, "DRS-06: submitted is always false");
    }

    /// DRS-07: evaluate_dry_run_strategies evaluates every id independently
    /// against the same window.
    #[test]
    fn drs07_multiple_ids_evaluated_independently_same_window() {
        let ids = vec![
            "intraday_scalper".to_string(),
            "intraday_short_scalper".to_string(),
        ];
        let diags = evaluate_dry_run_strategies(&ids, "AAPL", 0, &bearish_window(), 0);
        assert_eq!(diags.len(), 2, "DRS-07: one diagnostic per id");
        let long = diags
            .iter()
            .find(|d| d.strategy_id == "intraday_scalper")
            .unwrap();
        let short = diags
            .iter()
            .find(|d| d.strategy_id == "intraday_short_scalper")
            .unwrap();
        // On a bearish window: long-only strategy goes flat (target=0, delta=0
        // from flat); short-only strategy opens short (target<0).
        assert_eq!(
            long.target_qty, 0,
            "DRS-07: long-only strategy is flat on bearish"
        );
        assert!(
            short.target_qty < 0,
            "DRS-07: short-only strategy opens short on bearish"
        );
        assert!(
            !long.submitted && !short.submitted,
            "DRS-07: neither ever submits"
        );
    }

    /// DRS-08: target filtering ignores a symbol mismatch (defensive guard;
    /// in practice register_builtin_strategies bakes in the requested symbol).
    #[test]
    fn drs08_diagnostic_symbol_matches_requested_symbol() {
        let diag = evaluate_dry_run_strategy("intraday_scalper", "MSFT", 0, bullish_window(), 0);
        assert_eq!(
            diag.symbol, "MSFT",
            "DRS-08: diagnostic symbol echoes request"
        );
    }
}
