mod evaluator;
mod types;

pub use evaluator::{build_report, compute_metrics, evaluate_promotion, pick_winner, select_best};
pub use types::{
    write_promotion_report_json, Candidate, PromotionConfig, PromotionDecision, PromotionInput,
    PromotionMetrics, PromotionReport, StressSuiteResult,
};
