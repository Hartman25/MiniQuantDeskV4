//! mqk-backtest — Deterministic Backtest Engine (Event-Sourced Replay)
//!
//! Pipeline: BAR -> STRATEGY -> EXECUTION -> PORTFOLIO -> RISK
//!
//! - Deterministic replay (same bars + config + input hash => identical results)
//! - Input-data provenance: bar sequence hashed into run identity (BKT-PROV-01)
//! - No lookahead (incomplete bars rejected)
//! - Conservative fill pricing (worst-case ambiguity: BUY@HIGH, SELL@LOW)
//! - Stress profiles: flat slippage + volatility-spread component (Patch B5)
//! - Commission/fee modeling per fill (BKT-03P)
//! - Corporate action exclusion policy (Patch B4)
//! - Integrity gate: stale/gap/disagreement detection (PATCH 22 / B3)
//! - Shadow mode support (strategy runs but trades not executed)
//! - Risk enforcement via mqk-risk (daily loss, drawdown, PDT, reject storm)
//! - FIFO portfolio accounting via mqk-portfolio

pub mod corporate_actions; // Patch B4
pub mod dsr_pbo_sensitivity; // P9 BKT-ROBUSTNESS-GAUNTLET-01 / PROMOTION-STRESS-AUTHORITY-REPAIR-01
mod economics; // BACKTEST-MULTIPLIER-MARGIN-01
mod engine;
pub mod genuine_shuffled_placebo; // P9 BKT-ROBUSTNESS-GAUNTLET-01 / FINAL-P9-ROBUSTNESS-SEMANTICS-01
pub mod loader;
pub mod market_frame; // BKT-MULTISYMBOL-MARKET-FRAME-01
pub mod p7a_p7b_economic_replay_stress; // P9 BKT-ROBUSTNESS-GAUNTLET-01 / P7A-P7B-ECONOMIC-REPLAY-STRESS-01
pub mod regime;
pub mod robustness_gauntlet; // P9 BKT-ROBUSTNESS-GAUNTLET-01
pub mod strategy_lab;
pub mod strategy_scan_review;
pub mod strategy_scanner;
pub mod stress_suite; // PROMOTION-STRESS-SUITE-AUTHORITY-01
pub mod sweep;
pub mod types;

pub use corporate_actions::{CorporateActionPolicy, ForbidEntry}; // Patch B4
pub use economics::{
    mark_to_market_value_micros, notional_micros, realized_pnl_micros, BacktestEconomicsReport,
    BacktestInstrumentEconomics, EconomicsError,
}; // BACKTEST-MULTIPLIER-MARGIN-01 / BACKTEST-REPORT-ECONOMICS-ARTIFACT-01
pub use engine::{BacktestEngine, BacktestError};
pub use loader::{load_csv_file, parse_csv_bars, LoadError};
pub use market_frame::{
    build_market_frames, evaluate_market_frames, strategy_context_for_symbol, MarketFrame,
    MarketFrameError, MarketFrameEvaluator, MarketFrameView,
}; // BKT-MULTISYMBOL-MARKET-FRAME-01
pub use regime::{
    detect_market_regime, MarketRegimeClassification, MarketRegimeConfidence, MarketRegimeFeatures,
    MarketRegimeInput, MarketRegimeKind, MarketRegimePolicy, MarketRegimeReasonCode,
};
pub use strategy_lab::{
    evaluate_strategy_lab, evaluate_strategy_lab_with_policy, rank_strategy_lab_evaluations,
    strategy_lab_input_from_sweep_row, StrategyLabDecision, StrategyLabEvaluation,
    StrategyLabGrade, StrategyLabInput, StrategyLabMetrics, StrategyLabPolicy,
    StrategyLabReasonCode,
};
pub use strategy_scan_review::{
    build_review_decisions, derive_review_id, evaluate_scan_review_decision,
    execute_strategy_scan_review, review_decisions_to_csv, write_review_artifacts, ReviewManifest,
    ReviewRunOutput, ReviewRunRequest, ReviewSummary, StrategyScanReviewDecision,
    StrategyScanReviewPolicy, StrategyScanReviewState,
};
pub use strategy_scanner::{
    candidates_to_csv, derive_scan_id, evaluate_scan_candidate, execute_strategy_scan,
    rank_scan_candidates, resolve_timeframe_secs, write_scan_artifacts, ScanManifest,
    ScanRunOutput, ScanRunRequest, ScanSkipReasonCount, ScanSummary, StrategyScanCandidate,
    StrategyScanMetrics, StrategyScanPolicy, StrategyScanReasonCode, StrategyScanTruthState,
    DEFAULT_MIN_BARS,
};
pub use dsr_pbo_sensitivity::{dsr_pbo_sensitivity_scenario, DSR_PBO_SENSITIVITY_SCENARIO_NAME};
pub use genuine_shuffled_placebo::{
    genuine_shuffled_placebo_scenario, GENUINE_SHUFFLED_PLACEBO_PROTOCOL_ID,
    GENUINE_SHUFFLED_PLACEBO_SCENARIO_NAME,
};
pub use p7a_p7b_economic_replay_stress::{
    p7a_p7b_economic_replay_stress_scenario, P7A_P7B_ECONOMIC_REPLAY_STRESS_PROTOCOL_ID,
    P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME,
};
pub use robustness_gauntlet::{
    run_robustness_gauntlet, DeferredScenario, RobustnessGauntletOutput,
    RobustnessScenarioOutcome, REQUIRED_ROBUSTNESS_SCENARIO_NAMES,
    ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION,
}; // P9 BKT-ROBUSTNESS-GAUNTLET-01
pub use stress_suite::{
    run_backtest_stress_suite, StressScenarioOutcome, StressSuiteRunOutput,
    REQUIRED_SCENARIO_NAMES, STRESS_SUITE_PROTOCOL_VERSION,
}; // PROMOTION-STRESS-SUITE-AUTHORITY-01
pub use sweep::{
    rank_sweep_results, run_sweep, sweep_row_from_report, SweepError, SweepGrid, SweepPoint,
    SweepRowResult, SWEEP_MAX_COMBINATIONS,
};
pub use types::{
    derive_input_data_hash, derive_run_id, derive_run_id_with_economics,
    derive_run_id_with_execution_model, derive_run_id_with_semantic_identity, BacktestBar,
    BacktestConfig, BacktestFill, BacktestOrder, BacktestOrderSide, BacktestReport,
    CommissionModel, OrderStatus, StrategySizingConfig, StressProfile,
    BACKTEST_EXECUTION_MODEL_ID,
};
