#![forbid(unsafe_code)]

//! Deterministic in-memory paper fill engine (bar-driven).
//!
//! Determinism rules:
//! - no wall clock reads
//! - no RNG
//! - stable broker_message_id generation via per-order monotonic fill_seq

use std::collections::BTreeMap;

use mqk_execution::types::Side as ExecSide;
use mqk_execution::BrokerEvent;

/// Fill pricing mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FillMode {
    /// Fill at bar close.
    #[default]
    Close,
    /// Fill at bar mid (hl2).
    #[allow(dead_code)]
    Mid,
}

/// Per-symbol fill configuration.
#[derive(Debug, Clone, Default)]
pub struct FillSpec {
    /// Slippage applied in integer micros:
    /// - Buy: +slippage
    /// - Sell: -slippage
    pub slippage_micros: i64,
}

/// Minimal OHLC bar snapshot used for deterministic paper fills.
#[derive(Clone, Debug)]
pub struct Bar {
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
}

/// In-memory per-order state used by the paper broker.
#[derive(Debug, Clone)]
pub struct PaperOrderState {
    pub internal_order_id: String,
    pub symbol: String,
    pub side: ExecSide,
    pub original_qty: i64,
    pub remaining_qty: i64,
    pub fill_seq: u64,
}

impl PaperOrderState {
    pub fn new(internal_order_id: String, symbol: String, side: ExecSide, abs_qty: i64) -> Self {
        Self {
            internal_order_id,
            symbol,
            side,
            original_qty: abs_qty,
            remaining_qty: abs_qty,
            fill_seq: 0,
        }
    }
}

#[derive(Debug, Default)]
pub struct DeterministicFillEngine {
    mode: FillMode,
    specs: BTreeMap<String, FillSpec>,
}

impl DeterministicFillEngine {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub fn set_fill_mode(&mut self, mode: FillMode) {
        self.mode = mode;
    }

    #[allow(dead_code)]
    pub fn set_fill_spec(&mut self, symbol: &str, spec: FillSpec) {
        self.specs.insert(symbol.to_string(), spec);
    }

    pub fn apply_bar_to_order(&mut self, bar: &Bar, ord: &mut PaperOrderState) -> Vec<BrokerEvent> {
        if ord.remaining_qty <= 0 {
            return Vec::new();
        }

        let spec = self.specs.get(&ord.symbol).cloned().unwrap_or_default();

        let base_px = match self.mode {
            FillMode::Close => bar.close_micros,
            FillMode::Mid => (bar.high_micros + bar.low_micros) / 2,
        };

        // Deterministic slippage: buy worse, sell worse.
        let px = match ord.side {
            ExecSide::Buy => base_px + spec.slippage_micros,
            ExecSide::Sell => base_px - spec.slippage_micros,
        };

        // Fill entire remaining qty on this bar (simple deterministic behavior).
        let fill_qty_abs = ord.remaining_qty;
        ord.remaining_qty = 0;

        // P1-01: delta_qty is ALWAYS positive. Direction is conveyed exclusively
        // by the `side` field. Both the OMS state machine and portfolio accounting
        // require positive delta_qty (OMS rejects delta_qty <= 0 with TransitionError,
        // which the orchestrator treats as a halt condition). Sell and short-sell
        // fills carry side=Sell; buy and buy-to-cover fills carry side=Buy.
        let delta_qty: i64 = fill_qty_abs;

        ord.fill_seq = ord.fill_seq.saturating_add(1);
        let broker_message_id = format!("paper:fill:{}:{}", ord.internal_order_id, ord.fill_seq);

        vec![BrokerEvent::Fill {
            broker_message_id,
            internal_order_id: ord.internal_order_id.clone(),
            broker_order_id: Some(ord.internal_order_id.clone()),
            symbol: ord.symbol.clone(),
            side: ord.side,
            delta_qty,
            price_micros: px,
            fee_micros: 0,
        }]
    }
}

// ---------------------------------------------------------------------------
// P1-01 proof tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_execution::types::Side;

    fn flat_bar(px: i64) -> Bar {
        Bar {
            open_micros: px,
            high_micros: px,
            low_micros: px,
            close_micros: px,
        }
    }

    // P1-01 proof 1: buy fill emits positive delta_qty with side=Buy.
    #[test]
    fn buy_fill_delta_qty_is_positive() {
        let mut engine = DeterministicFillEngine::new();
        let bar = flat_bar(100_000_000);
        let mut ord = PaperOrderState::new("o1".into(), "AAPL".into(), Side::Buy, 50);
        let evs = engine.apply_bar_to_order(&bar, &mut ord);
        assert_eq!(evs.len(), 1);
        let BrokerEvent::Fill { delta_qty, side, .. } = &evs[0] else {
            panic!("expected Fill");
        };
        assert_eq!(*delta_qty, 50, "buy fill delta_qty must equal abs fill qty");
        assert!(*delta_qty > 0, "buy fill delta_qty must be positive");
        assert!(matches!(side, Side::Buy), "buy fill must carry side=Buy");
    }

    // P1-01 proof 2: sell fill emits positive delta_qty with side=Sell.
    // Before this patch, the fill engine emitted delta_qty = -50, which would
    // cause the OMS state machine to return TransitionError and halt the run.
    #[test]
    fn sell_fill_delta_qty_is_positive() {
        let mut engine = DeterministicFillEngine::new();
        let bar = flat_bar(100_000_000);
        let mut ord = PaperOrderState::new("o2".into(), "AAPL".into(), Side::Sell, 50);
        let evs = engine.apply_bar_to_order(&bar, &mut ord);
        assert_eq!(evs.len(), 1);
        let BrokerEvent::Fill { delta_qty, side, .. } = &evs[0] else {
            panic!("expected Fill");
        };
        assert_eq!(*delta_qty, 50, "sell fill delta_qty must equal abs fill qty");
        assert!(*delta_qty > 0, "sell fill delta_qty must be positive, not signed");
        assert!(matches!(side, Side::Sell), "sell fill must carry side=Sell");
    }

    // P1-01 proof 3: partial remaining-qty sell fill is also positive.
    // Simulates an order where 30 of 100 have already been consumed externally,
    // leaving remaining_qty=70. The fill of the residual must still be positive.
    #[test]
    fn partial_remaining_sell_fill_delta_qty_is_positive() {
        let mut engine = DeterministicFillEngine::new();
        let bar = flat_bar(200_000_000);
        let mut ord = PaperOrderState {
            internal_order_id: "o3".into(),
            symbol: "MSFT".into(),
            side: Side::Sell,
            original_qty: 100,
            remaining_qty: 70, // 30 already consumed
            fill_seq: 1,
        };
        let evs = engine.apply_bar_to_order(&bar, &mut ord);
        assert_eq!(evs.len(), 1);
        let BrokerEvent::Fill { delta_qty, side, .. } = &evs[0] else {
            panic!("expected Fill");
        };
        assert_eq!(*delta_qty, 70, "fill must equal remaining_qty");
        assert!(*delta_qty > 0, "partial sell delta_qty must be positive");
        assert!(matches!(side, Side::Sell));
    }

    // P1-01 proof 4: short-sell fill has positive delta_qty and side=Sell.
    // A short sell is submitted as Side::Sell with no pre-existing long position.
    // Portfolio accounting (FIFO lot logic) opens a short lot from the Side::Sell
    // fill. The fill engine need only ensure positive delta_qty + side=Sell.
    #[test]
    fn short_sell_fill_has_positive_delta_qty_and_sell_side() {
        let mut engine = DeterministicFillEngine::new();
        let bar = flat_bar(50_000_000);
        let mut ord = PaperOrderState::new("short-1".into(), "GME".into(), Side::Sell, 200);
        let evs = engine.apply_bar_to_order(&bar, &mut ord);
        assert_eq!(evs.len(), 1);
        let BrokerEvent::Fill { delta_qty, side, .. } = &evs[0] else {
            panic!("expected Fill");
        };
        assert_eq!(*delta_qty, 200);
        assert!(*delta_qty > 0, "short sell delta_qty must be positive");
        assert!(matches!(side, Side::Sell), "short sell must carry side=Sell");
    }

    // P1-01 proof 5: buy-to-cover fill has positive delta_qty and side=Buy.
    // A buy-to-cover is submitted as Side::Buy. Portfolio accounting closes
    // the existing short lot via FIFO. The fill engine ensures positive delta_qty.
    #[test]
    fn cover_fill_has_positive_delta_qty_and_buy_side() {
        let mut engine = DeterministicFillEngine::new();
        let bar = flat_bar(50_000_000);
        let mut ord = PaperOrderState::new("cover-1".into(), "GME".into(), Side::Buy, 200);
        let evs = engine.apply_bar_to_order(&bar, &mut ord);
        assert_eq!(evs.len(), 1);
        let BrokerEvent::Fill { delta_qty, side, .. } = &evs[0] else {
            panic!("expected Fill");
        };
        assert_eq!(*delta_qty, 200);
        assert!(*delta_qty > 0, "cover fill delta_qty must be positive");
        assert!(matches!(side, Side::Buy), "cover fill must carry side=Buy");
    }

    // P1-01 proof 6: the fill engine never emits non-positive delta_qty for any side.
    #[test]
    fn no_negative_delta_qty_from_any_side() {
        let mut engine = DeterministicFillEngine::new();
        let bar = flat_bar(100_000_000);
        for side in [Side::Buy, Side::Sell] {
            let mut ord = PaperOrderState::new("probe".into(), "X".into(), side, 100);
            let evs = engine.apply_bar_to_order(&bar, &mut ord);
            for ev in &evs {
                if let BrokerEvent::Fill { delta_qty, .. } = ev {
                    assert!(
                        *delta_qty > 0,
                        "fill engine must never emit non-positive delta_qty; \
                         side={:?}, got delta_qty={}",
                        side,
                        delta_qty
                    );
                }
            }
        }
    }

    // P1-01 proof 7: sell fill delta_qty passes the OMS positive-qty guard.
    // The OMS state machine checks `if *delta_qty <= 0 { return Err(TransitionError) }`.
    // That guard, if triggered, causes the orchestrator to persist HALT+DISARM.
    // This test proves the generated sell fill would clear that guard.
    #[test]
    fn sell_fill_delta_qty_passes_oms_positive_guard() {
        let mut engine = DeterministicFillEngine::new();
        let bar = flat_bar(100_000_000);
        let mut ord = PaperOrderState::new("o-oms".into(), "TSLA".into(), Side::Sell, 75);
        let evs = engine.apply_bar_to_order(&bar, &mut ord);
        assert_eq!(evs.len(), 1);
        if let BrokerEvent::Fill { delta_qty, .. } = &evs[0] {
            // OMS guard in state_machine.rs: `if *delta_qty <= 0 { return Err(...) }`
            assert!(
                *delta_qty > 0,
                "delta_qty={} would be rejected by the OMS positive-qty guard, \
                 triggering a HALT+DISARM",
                delta_qty
            );
        }
    }

    // P1-01 proof 8: sell fill side field routes correctly for portfolio accounting.
    // broker_event_to_fill in the orchestrator maps the side field to portfolio::Side.
    // portfolio::apply_fill uses side=Sell to reduce longs / open shorts via FIFO.
    // This test proves the fill carries the correct (side, delta_qty) pair for routing.
    #[test]
    fn sell_fill_side_and_qty_correct_for_portfolio_routing() {
        let mut engine = DeterministicFillEngine::new();
        let bar = flat_bar(100_000_000);
        let mut ord = PaperOrderState::new("o-pf".into(), "SPY".into(), Side::Sell, 10);
        let evs = engine.apply_bar_to_order(&bar, &mut ord);
        assert_eq!(evs.len(), 1);
        if let BrokerEvent::Fill { side, delta_qty, .. } = &evs[0] {
            assert!(
                matches!(side, Side::Sell),
                "fill side must be Sell so portfolio routes to sell_fifo"
            );
            assert!(
                *delta_qty > 0,
                "fill delta_qty must be positive so broker_event_to_fill does not drop it"
            );
        }
    }

    // P1-01 proof 9: buy fill accounting invariant — qty matches order qty.
    #[test]
    fn buy_fill_qty_matches_order_qty() {
        let mut engine = DeterministicFillEngine::new();
        let bar = flat_bar(150_000_000);
        let mut ord = PaperOrderState::new("b1".into(), "NVDA".into(), Side::Buy, 33);
        let evs = engine.apply_bar_to_order(&bar, &mut ord);
        assert_eq!(evs.len(), 1);
        if let BrokerEvent::Fill { delta_qty, .. } = &evs[0] {
            assert_eq!(*delta_qty, 33);
            assert_eq!(ord.remaining_qty, 0, "remaining_qty must be 0 after full fill");
        }
    }
}
