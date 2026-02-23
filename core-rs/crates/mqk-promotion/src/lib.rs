pub mod artifact_gate; // Patch B6
mod evaluator;
mod types;

pub use artifact_gate::{lock_artifact_from_str, ArtifactLock, LockError}; // Patch B6
pub use evaluator::{build_report, compute_metrics, evaluate_promotion, pick_winner, select_best};
pub use types::{
    write_promotion_report_json, Candidate, PromotionConfig, PromotionDecision, PromotionInput,
    PromotionMetrics, PromotionReport, StressSuiteResult,
};
