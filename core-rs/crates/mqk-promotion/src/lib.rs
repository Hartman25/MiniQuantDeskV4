mod evaluator;
mod types;

pub use evaluator::{compare_candidates, compute_metrics, evaluate_promotion};
pub use types::{
    PromotionCandidate, PromotionDecision, PromotionMetrics, PromotionReport, PromotionThresholds,
    TieBreakOrder, TieBreakRules,
};
