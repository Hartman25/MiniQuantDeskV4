use std::io;
use std::path::{Path, PathBuf};

use mqk_backtest::BacktestReport;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------
// Config
// ---------------------------------------------------------------------------

/// Thresholds for promotion gating.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PromotionConfig {
    /// Minimum annualized Sharpe ratio.
    pub min_sharpe: f64,
    /// Maximum drawdown as fraction (e.g. 0.20 = 20%).
    pub max_mdd: f64,
    /// Minimum CAGR as fraction (e.g. 0.10 = 10%).
    pub min_cagr: f64,
    /// Minimum profit factor (sum profits / abs sum losses).
    pub min_profit_factor: f64,
    /// Minimum fraction of profitable months (0..1).
    pub min_profitable_months_pct: f64,
}

// ---------------------------------------------------------------------------
// Patch B2 — Stress suite result
// ---------------------------------------------------------------------------

/// Results of the adversarial partial-fill + cancel/replace stress suite.
///
/// Promotion is **blocked** when:
/// - `PromotionInput.stress_suite` is `None` (suite not run).
/// - The suite ran but `passed == false`.
/// - The suite ran with zero scenarios (`scenarios_run == 0`), which is invalid.
///
/// Build with [`StressSuiteResult::pass`] or [`StressSuiteResult::fail`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct StressSuiteResult {
    /// True if all scenarios passed.
    pub passed: bool,
    /// Number of stress scenarios that ran. Must be ≥ 1 for the suite to be valid.
    pub scenarios_run: u32,
    /// Number of scenarios that passed.
    pub scenarios_passed: u32,
    /// Human-readable descriptions of failed scenarios (empty when passed).
    pub failed_scenarios: Vec<String>,
}

impl StressSuiteResult {
    /// All `scenarios_run` scenarios passed.
    pub fn pass(scenarios_run: u32) -> Self {
        Self {
            passed: true,
            scenarios_run,
            scenarios_passed: scenarios_run,
            failed_scenarios: Vec::new(),
        }
    }

    /// Some scenarios failed.
    pub fn fail(scenarios_run: u32, scenarios_passed: u32, failed_scenarios: Vec<String>) -> Self {
        Self {
            passed: false,
            scenarios_run,
            scenarios_passed,
            failed_scenarios,
        }
    }
}

// ---------------------------------------------------------------------------
// Input
// ---------------------------------------------------------------------------

/// Input bundle for promotion evaluation.
pub struct PromotionInput {
    /// Initial equity in micros (required for CAGR denominator).
    pub initial_equity_micros: i64,
    /// The backtest report to evaluate.
    pub report: BacktestReport,
    /// Results of the partial-fill + cancel/replace stress suite.
    ///
    /// **Promotion is blocked if `None`** (suite not run) or if the suite
    /// ran but failed. Set to `Some(StressSuiteResult::pass(n))` after the
    /// stress suite completes successfully.
    pub stress_suite: Option<StressSuiteResult>, // Patch B2
}

// ---------------------------------------------------------------------------
// Metrics
// ---------------------------------------------------------------------------

/// Computed promotion metrics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionMetrics {
    pub sharpe: f64,
    pub mdd: f64,
    pub cagr: f64,
    pub profit_factor: f64,
    pub profitable_months_pct: f64,
    // Intermediate reporting values
    pub start_equity_micros: i64,
    pub end_equity_micros: i64,
    pub duration_days: f64,
    pub num_months: usize,
    pub num_trades: usize,
}

// ---------------------------------------------------------------------------
// Decision
// ---------------------------------------------------------------------------

/// Gate result for a single candidate.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionDecision {
    pub passed: bool,
    /// Stable-ordered list of human-readable fail reasons (empty when passed).
    pub fail_reasons: Vec<String>,
    pub metrics: PromotionMetrics,
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// Full promotion report artifact (serializable to JSON).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PromotionReport {
    pub config: PromotionConfig,
    pub metrics: PromotionMetrics,
    pub decision: PromotionDecision,
    /// Winner candidate id (set by select_best, None for single evaluation).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub winner_id: Option<String>,
}

/// Write the report as pretty-printed JSON to `out_dir/promotion_report.json`.
/// Returns the path written.
pub fn write_promotion_report_json(
    out_dir: &Path,
    report: &PromotionReport,
) -> io::Result<PathBuf> {
    std::fs::create_dir_all(out_dir)?;
    let path = out_dir.join("promotion_report.json");
    let json = serde_json::to_string_pretty(report).map_err(io::Error::other)?;
    std::fs::write(&path, json)?;
    Ok(path)
}

// ---------------------------------------------------------------------------
// Tie-break types
// ---------------------------------------------------------------------------

/// A candidate for comparative evaluation.
pub struct Candidate {
    pub id: String,
    pub input: PromotionInput,
}
