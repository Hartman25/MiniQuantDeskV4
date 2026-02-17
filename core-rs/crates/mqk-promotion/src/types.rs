use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PromotionThresholds {
    /// Minimum annualized return (e.g., 0.10 = 10%)
    pub cagr_min: f64,
    /// Maximum drawdown fraction (e.g., 0.20 = 20%)
    pub mdd_max: f64,
    /// Minimum Sharpe ratio (unitless)
    pub sharpe_min: f64,
    /// Minimum profit factor (>= 1.0)
    pub profit_factor_min: f64,
    /// Minimum fraction of profitable "months" (0..=1)
    pub profitable_months_min: f64,
}

impl Default for PromotionThresholds {
    fn default() -> Self {
        Self {
            cagr_min: 0.0,
            mdd_max: 1.0,
            sharpe_min: 0.0,
            profit_factor_min: 1.0,
            profitable_months_min: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionMetrics {
    pub cagr: f64,
    pub max_drawdown: f64,
    pub sharpe: f64,
    pub profit_factor: f64,
    pub profitable_months_frac: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PromotionDecision {
    Pass,
    Fail,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionReport {
    pub decision: PromotionDecision,
    pub thresholds: PromotionThresholds,
    pub metrics: PromotionMetrics,
    /// Human-readable fail reasons (empty when Pass).
    pub reasons: Vec<String>,
}

impl PromotionReport {
    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).unwrap_or_else(|_| "{}".to_string())
    }
}

/// A candidate for tie-break comparison.
#[derive(Debug, Clone)]
pub struct PromotionCandidate {
    pub name: String,
    pub metrics: PromotionMetrics,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TieBreakOrder {
    /// Lower max drawdown wins.
    LowerMdd,
    /// Higher CAGR wins.
    HigherCagr,
    /// Higher Sharpe wins.
    HigherSharpe,
    /// Higher profit factor wins.
    HigherProfitFactor,
    /// Higher profitable-month fraction wins.
    HigherProfitableMonths,
}

#[derive(Debug, Clone)]
pub struct TieBreakRules {
    pub within_points: f64,
    pub order: Vec<TieBreakOrder>,
}

impl Default for TieBreakRules {
    fn default() -> Self {
        Self {
            within_points: 0.0,
            order: vec![
                TieBreakOrder::LowerMdd,
                TieBreakOrder::HigherCagr,
                TieBreakOrder::HigherSharpe,
                TieBreakOrder::HigherProfitFactor,
                TieBreakOrder::HigherProfitableMonths,
            ],
        }
    }
}
