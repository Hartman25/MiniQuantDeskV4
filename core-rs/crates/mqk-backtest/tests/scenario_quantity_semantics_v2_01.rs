//! D6/A1/A2: engine-level proof that the additive `FractionalQtyMicrosV1`
//! quantity domain (Crypto only) round-trips a genuinely fractional order
//! exactly, that the default `WholeUnitsV1` domain is byte-for-byte
//! unchanged, and that a fractional domain configured against a
//! multiplier-bearing (futures/options-style) instrument fails closed
//! before any bar is processed.

use mqk_backtest::{
    BacktestBar, BacktestConfig, BacktestEngine, BacktestError, BacktestInstrumentEconomics,
    OrderStatus, QuantitySemanticsId,
};
use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_portfolio::QtyMicros;
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

const M: i64 = 1_000_000;

fn flat_bar(symbol: &str, end_ts: i64, price_usd_micros: i64, volume: i64) -> BacktestBar {
    BacktestBar::new(
        symbol,
        end_ts,
        price_usd_micros,
        price_usd_micros,
        price_usd_micros,
        price_usd_micros,
        volume,
    )
}

/// Targets a fixed `QtyMicros` position on bar 1, then flat thereafter.
struct FixedTarget {
    symbol: String,
    qty: QtyMicros,
    bar_idx: u64,
}

impl FixedTarget {
    fn new(symbol: &str, qty: QtyMicros) -> Self {
        Self {
            symbol: symbol.to_string(),
            qty,
            bar_idx: 0,
        }
    }
}

impl Strategy for FixedTarget {
    fn spec(&self) -> StrategySpec {
        StrategySpec::new("FixedTarget", 60)
    }

    fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
        self.bar_idx += 1;
        // Target the same quantity every bar -- `targets_to_order_intents`
        // treats an omitted symbol as an implicit target of 0, so the hold
        // must be explicitly restated every bar, not just on the entry bar.
        StrategyOutput::new(vec![TargetPosition::new(self.symbol.clone(), self.qty)])
    }
}

fn wide_cap_config(quantity_semantics: QuantitySemanticsId) -> BacktestConfig {
    let mut cfg = BacktestConfig::test_defaults();
    cfg.max_gross_exposure_mult_micros = 100_000_000; // 100x equity, never binds
    cfg.quantity_semantics = quantity_semantics;
    cfg
}

// --- QS-01: FractionalQtyMicrosV1 + multiplier=1 (Crypto) round-trips an
// exact fractional (0.5 BTC) fill: portfolio position, fee, and the V2 order
// record are all exact -- and the V1 `orders` collection never receives a
// fabricated/truncated entry for it. ---
#[test]
fn qs01_fractional_crypto_run_round_trips_exact_quantity() {
    let bars = vec![
        flat_bar("BTC/USD", 1_700_000_060, 50_000 * M, 1_000_000),
        flat_bar("BTC/USD", 1_700_000_120, 50_100 * M, 1_000_000),
    ];
    let mut cfg = wide_cap_config(QuantitySemanticsId::FractionalQtyMicrosV1);
    cfg.commission.per_share_micros = 10_000; // exercise fee math too
    let mut engine = BacktestEngine::new(cfg).with_economics(BacktestInstrumentEconomics::equity()); // multiplier=1
    engine
        .add_strategy(Box::new(FixedTarget::new(
            "BTC/USD",
            QtyMicros::new(500_000), // 0.5 BTC
        )))
        .unwrap();

    let report = engine.run(&bars).unwrap();

    assert!(!report.halted, "halt_reason={:?}", report.halt_reason);
    assert_eq!(report.fills.len(), 1);
    let fill = &report.fills[0];
    assert_eq!(
        fill.inner.qty,
        QtyMicros::new(500_000),
        "fill qty must be exact, not truncated"
    );
    // Fee: per_share_micros(10_000) * 0.5 = 5_000 exactly.
    assert_eq!(fill.inner.fee_micros, 5_000);

    // The fractional order lands in the additive V2 collection, never as a
    // truncated/fabricated whole-unit V1 BacktestOrder.
    assert_eq!(report.orders.len(), 0, "no V1 order for a fractional order");
    assert_eq!(report.orders_v2.len(), 1);
    assert_eq!(report.orders_v2[0].qty_micros, QtyMicros::new(500_000));
    assert_eq!(report.orders_v2[0].status, OrderStatus::Filled);
    assert_eq!(
        report.quantity_semantics,
        QuantitySemanticsId::FractionalQtyMicrosV1
    );

    // realized_pnl_micros is sourced from the authoritative portfolio (never
    // the whole-unit-only shadow ledger) for this domain -- see engine.rs.
    // No round-trip closing trade happened, so it must be exactly zero, not
    // an omitted/garbage value.
    assert_eq!(report.economics.realized_pnl_micros, 0);
}

// --- QS-02: the default WholeUnitsV1 domain still refuses a fractional
// intent exactly as before D6/A2 (regression control). ---
#[test]
fn qs02_whole_units_v1_still_refuses_fractional_intent() {
    let bars = vec![flat_bar("BTC/USD", 1_700_000_060, 50_000 * M, 1_000_000)];
    let cfg = wide_cap_config(QuantitySemanticsId::WholeUnitsV1);
    let mut engine = BacktestEngine::new(cfg);
    engine
        .add_strategy(Box::new(FixedTarget::new(
            "BTC/USD",
            QtyMicros::new(500_000), // 0.5 -- fractional
        )))
        .unwrap();

    let report = engine.run(&bars).unwrap();

    assert!(report.halted);
    assert_eq!(
        report.halt_reason.as_deref(),
        Some("fractional_intent_qty_unsupported_in_backtest: BTC/USD 0.5")
    );
    assert_eq!(report.fills.len(), 0);
    assert_eq!(report.orders_v2.len(), 0);
}

// --- QS-03: FractionalQtyMicrosV1 configured against a multiplier-bearing
// (futures-style) instrument refuses at run() start, before any bar is
// processed -- no fractional futures/options contracts. ---
#[test]
fn qs03_fractional_domain_refuses_non_unit_multiplier() {
    let bars = vec![flat_bar("ES", 1_700_000_060, 4_500 * M, 1_000)];
    let cfg = wide_cap_config(QuantitySemanticsId::FractionalQtyMicrosV1);
    let mut engine = BacktestEngine::new(cfg)
        .with_economics(BacktestInstrumentEconomics::new(50, None, None).unwrap());
    engine
        .add_strategy(Box::new(FixedTarget::new(
            "ES",
            QtyMicros::from_whole_units(1).unwrap(),
        )))
        .unwrap();

    let err = engine.run(&bars).unwrap_err();
    assert_eq!(
        err,
        BacktestError::InvalidQuantitySemantics {
            contract_multiplier: 50
        }
    );
}

// --- QS-04: a whole-unit position under FractionalQtyMicrosV1 (e.g. a
// strategy that happens to target exactly 2.0 BTC) still lands in the V1
// `orders`/report path, not the V2 one -- V2 is reserved for a genuinely
// fractional quantity, never used merely because the domain is configured. ---
#[test]
fn qs04_whole_quantity_under_fractional_domain_still_uses_v1_order_record() {
    let bars = vec![
        flat_bar("BTC/USD", 1_700_000_060, 50_000 * M, 1_000_000),
        flat_bar("BTC/USD", 1_700_000_120, 50_100 * M, 1_000_000),
    ];
    let cfg = wide_cap_config(QuantitySemanticsId::FractionalQtyMicrosV1);
    let mut engine = BacktestEngine::new(cfg).with_economics(BacktestInstrumentEconomics::equity());
    engine
        .add_strategy(Box::new(FixedTarget::new(
            "BTC/USD",
            QtyMicros::from_whole_units(2).unwrap(),
        )))
        .unwrap();

    let report = engine.run(&bars).unwrap();

    assert!(!report.halted);
    assert_eq!(report.orders_v2.len(), 0);
    assert_eq!(report.orders.len(), 1);
    assert_eq!(report.orders[0].qty, 2);
    assert_eq!(report.orders[0].status, OrderStatus::Filled);
}
