use std::collections::BTreeMap;

use crate::types::PositionState;
use crate::MarkMap;

fn mul_qty_price_micros_i128(qty: i64, price_micros: i64) -> i128 {
    (qty as i128) * (price_micros as i128)
}

fn i128_to_i64_clamp(x: i128) -> i64 {
    if x > i64::MAX as i128 {
        i64::MAX
    } else if x < i64::MIN as i128 {
        i64::MIN
    } else {
        x as i64
    }
}

/// Exposure metrics (micros).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExposureMetrics {
    pub gross_exposure_micros: i64,
    pub net_exposure_micros: i64,
}

/// Equity metrics (micros).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EquityMetrics {
    pub equity_micros: i64,
    pub unrealized_pnl_micros: i64,
    pub realized_pnl_micros: i64,
    pub exposure: ExposureMetrics,
}

/// Compute exposure from positions and marks.
/// gross = Σ |qty| * mark
/// net   = Σ qty * mark
pub fn compute_exposure_micros(
    positions: &BTreeMap<String, PositionState>,
    marks: &MarkMap,
) -> ExposureMetrics {
    let mut gross: i128 = 0;
    let mut net: i128 = 0;

    // deterministic iteration (BTreeMap)
    for (sym, pos) in positions {
        let mark = *marks.get(sym).unwrap_or(&0);
        let qty = pos.qty_signed();
        gross += mul_qty_price_micros_i128(qty.abs(), mark);
        net += mul_qty_price_micros_i128(qty, mark);
    }

    ExposureMetrics {
        gross_exposure_micros: i128_to_i64_clamp(gross),
        net_exposure_micros: i128_to_i64_clamp(net),
    }
}

/// Compute unrealized PnL from FIFO lots and marks.
/// long lot: (mark - entry) * qty
/// short lot: (entry - mark) * abs(qty)
pub fn compute_unrealized_pnl_micros(
    positions: &BTreeMap<String, PositionState>,
    marks: &MarkMap,
) -> i64 {
    let mut pnl: i128 = 0;

    for (sym, pos) in positions {
        let mark = *marks.get(sym).unwrap_or(&0);

        for lot in &pos.lots {
            let entry = lot.entry_price_micros;
            let q = lot.qty_signed;
            if q > 0 {
                pnl += (mark as i128 - entry as i128) * (q as i128);
            } else if q < 0 {
                pnl += (entry as i128 - mark as i128) * ((-q) as i128);
            }
        }
    }

    i128_to_i64_clamp(pnl)
}

/// Compute equity = cash + Σ(qty * mark).
pub fn compute_equity_micros(
    cash_micros: i64,
    positions: &BTreeMap<String, PositionState>,
    marks: &MarkMap,
) -> i64 {
    let mut mv: i128 = cash_micros as i128;

    for (sym, pos) in positions {
        let mark = *marks.get(sym).unwrap_or(&0);
        let qty = pos.qty_signed();
        mv += mul_qty_price_micros_i128(qty, mark);
    }

    i128_to_i64_clamp(mv)
}

/// Exposure breach error (deterministic and minimal).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExposureBreach {
    pub gross_exposure_micros: i64,
    pub max_gross_exposure_micros: i64,
}

/// Enforce a max gross exposure.
/// Returns Ok(()) if within limit, Err(ExposureBreach) if exceeded.
pub fn enforce_max_gross_exposure(
    positions: &BTreeMap<String, PositionState>,
    marks: &MarkMap,
    max_gross_exposure_micros: i64,
) -> Result<(), ExposureBreach> {
    let exposure = compute_exposure_micros(positions, marks);
    if exposure.gross_exposure_micros > max_gross_exposure_micros {
        Err(ExposureBreach {
            gross_exposure_micros: exposure.gross_exposure_micros,
            max_gross_exposure_micros,
        })
    } else {
        Ok(())
    }
}
