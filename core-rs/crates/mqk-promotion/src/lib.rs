pub mod artifact_gate; // Patch B6
mod evaluator;
pub mod research_evidence; // P7C
mod research_registry; // P7C-REPAIR-03: crate-internal only, no public authority constructor
mod types;

pub use artifact_gate::{lock_artifact_from_str, ArtifactLock, LockError}; // Patch B6
pub use evaluator::{
    build_report, check_metrics_finite, compute_metrics, evaluate_promotion, pick_winner,
    select_best,
};
pub use research_evidence::{verify_promotion_oos_evidence, VerifiedPromotionOosEvidence}; // P7C-REPAIR-01/-02/-03
pub use types::{
    write_promotion_report_json, Candidate, PromotionConfig, PromotionDecision, PromotionInput,
    PromotionMetrics, PromotionReport, RunProvenance, StressSuiteResult,
};
