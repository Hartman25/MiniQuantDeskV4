//! BKT-PROMOTION-ARTIFACT-AUTHORITY-01 — canonical, schema-versioned,
//! lossless [`mqk_backtest::BacktestReport`] durable artifact.
//!
//! `metrics.json`/`orders.csv`/`fills.csv`/`equity_curve.csv` are derived,
//! lossy views for human/GUI consumption. They are not sufficient to
//! reconstruct the exact `BacktestReport` the engine produced (e.g. per-fill
//! `Fill` internals, `OrderStatus` variants beyond filled/rejected, execution
//! model identity). `backtest_report.json` is the lossless authority: a
//! direct, schema-versioned mirror of the engine's own report shape, using
//! plain DTOs (never `mqk_backtest`/`mqk_portfolio` types directly) so this
//! artifact's wire format never silently changes just because an unrelated
//! internal engine or portfolio-accounting type gains or loses a field.
//!
//! [`load_canonical_backtest_report`] is the promotion-grade loader: it
//! cross-validates `backtest_report.json` against the run's `manifest.json`
//! (run_id, strategy_name, config identity, execution model identity) and
//! fails closed on any mismatch, malformed content, missing file, or
//! unsupported schema version. It never reconstructs a report from
//! `metrics.json`/CSVs, and it never optimistically upgrades an artifact that
//! predates this schema (missing `backtest_report.json` is a hard failure,
//! not a fallback trigger).

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use mqk_backtest::{
    BacktestEconomicsReport, BacktestFill, BacktestOrder, BacktestOrderSide, BacktestReport,
    OrderStatus, StrategySizingConfig,
};
use mqk_portfolio::{Fill, Side};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::RunManifest;

/// Current schema version of `backtest_report.json`. Bump on any
/// non-additive shape change; [`load_canonical_backtest_report`] fails
/// closed on any version it does not recognize.
pub const BACKTEST_REPORT_ARTIFACT_SCHEMA_VERSION: u32 = 1;

// ---------------------------------------------------------------------------
// DTOs — plain mirrors, never the engine/portfolio types directly.
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum SideDto {
    Buy,
    Sell,
}

impl From<Side> for SideDto {
    fn from(s: Side) -> Self {
        match s {
            Side::Buy => SideDto::Buy,
            Side::Sell => SideDto::Sell,
        }
    }
}

impl From<SideDto> for Side {
    fn from(s: SideDto) -> Self {
        match s {
            SideDto::Buy => Side::Buy,
            SideDto::Sell => Side::Sell,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum BacktestOrderSideDto {
    Buy,
    Sell,
}

impl From<&BacktestOrderSide> for BacktestOrderSideDto {
    fn from(s: &BacktestOrderSide) -> Self {
        match s {
            BacktestOrderSide::Buy => BacktestOrderSideDto::Buy,
            BacktestOrderSide::Sell => BacktestOrderSideDto::Sell,
        }
    }
}

impl From<BacktestOrderSideDto> for BacktestOrderSide {
    fn from(s: BacktestOrderSideDto) -> Self {
        match s {
            BacktestOrderSideDto::Buy => BacktestOrderSide::Buy,
            BacktestOrderSideDto::Sell => BacktestOrderSide::Sell,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
enum OrderStatusDto {
    Filled,
    Rejected,
    HaltTriggered,
    UnfilledEndOfData,
    CanceledOnHalt,
}

impl From<&OrderStatus> for OrderStatusDto {
    fn from(s: &OrderStatus) -> Self {
        match s {
            OrderStatus::Filled => OrderStatusDto::Filled,
            OrderStatus::Rejected => OrderStatusDto::Rejected,
            OrderStatus::HaltTriggered => OrderStatusDto::HaltTriggered,
            OrderStatus::UnfilledEndOfData => OrderStatusDto::UnfilledEndOfData,
            OrderStatus::CanceledOnHalt => OrderStatusDto::CanceledOnHalt,
        }
    }
}

impl From<OrderStatusDto> for OrderStatus {
    fn from(s: OrderStatusDto) -> Self {
        match s {
            OrderStatusDto::Filled => OrderStatus::Filled,
            OrderStatusDto::Rejected => OrderStatus::Rejected,
            OrderStatusDto::HaltTriggered => OrderStatus::HaltTriggered,
            OrderStatusDto::UnfilledEndOfData => OrderStatus::UnfilledEndOfData,
            OrderStatusDto::CanceledOnHalt => OrderStatus::CanceledOnHalt,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct FillDto {
    symbol: String,
    side: SideDto,
    qty: i64,
    price_micros: i64,
    fee_micros: i64,
}

impl From<&Fill> for FillDto {
    fn from(f: &Fill) -> Self {
        Self {
            symbol: f.symbol.clone(),
            side: f.side.into(),
            qty: f.qty,
            price_micros: f.price_micros,
            fee_micros: f.fee_micros,
        }
    }
}

impl From<FillDto> for Fill {
    fn from(f: FillDto) -> Self {
        Fill::new(f.symbol, f.side.into(), f.qty, f.price_micros, f.fee_micros)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct BacktestFillDto {
    fill_id: Uuid,
    order_id: Uuid,
    signal_ts: i64,
    fill_ts: i64,
    inner: FillDto,
}

impl From<&BacktestFill> for BacktestFillDto {
    fn from(f: &BacktestFill) -> Self {
        Self {
            fill_id: f.fill_id,
            order_id: f.order_id,
            signal_ts: f.signal_ts,
            fill_ts: f.fill_ts,
            inner: (&f.inner).into(),
        }
    }
}

impl From<BacktestFillDto> for BacktestFill {
    fn from(f: BacktestFillDto) -> Self {
        BacktestFill {
            fill_id: f.fill_id,
            order_id: f.order_id,
            signal_ts: f.signal_ts,
            fill_ts: f.fill_ts,
            inner: f.inner.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BacktestOrderDto {
    order_id: Uuid,
    signal_ts: i64,
    symbol: String,
    side: BacktestOrderSideDto,
    qty: i64,
    status: OrderStatusDto,
}

impl From<&BacktestOrder> for BacktestOrderDto {
    fn from(o: &BacktestOrder) -> Self {
        Self {
            order_id: o.order_id,
            signal_ts: o.signal_ts,
            symbol: o.symbol.clone(),
            side: (&o.side).into(),
            qty: o.qty,
            status: (&o.status).into(),
        }
    }
}

impl From<BacktestOrderDto> for BacktestOrder {
    fn from(o: BacktestOrderDto) -> Self {
        BacktestOrder {
            order_id: o.order_id,
            signal_ts: o.signal_ts,
            symbol: o.symbol,
            side: o.side.into(),
            qty: o.qty,
            status: o.status.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct StrategySizingConfigDto {
    target_qty: i64,
    max_target_qty: Option<i64>,
    max_position_notional_usd: Option<i64>,
}

impl From<&StrategySizingConfig> for StrategySizingConfigDto {
    fn from(s: &StrategySizingConfig) -> Self {
        Self {
            target_qty: s.target_qty,
            max_target_qty: s.max_target_qty,
            max_position_notional_usd: s.max_position_notional_usd,
        }
    }
}

impl From<StrategySizingConfigDto> for StrategySizingConfig {
    fn from(s: StrategySizingConfigDto) -> Self {
        StrategySizingConfig {
            target_qty: s.target_qty,
            max_target_qty: s.max_target_qty,
            max_position_notional_usd: s.max_position_notional_usd,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct BacktestEconomicsReportDto {
    contract_multiplier: i64,
    initial_margin_micros: Option<i64>,
    maintenance_margin_micros: Option<i64>,
    realized_pnl_micros: i64,
    margin_enforced: bool,
}

impl From<&BacktestEconomicsReport> for BacktestEconomicsReportDto {
    fn from(e: &BacktestEconomicsReport) -> Self {
        Self {
            contract_multiplier: e.contract_multiplier,
            initial_margin_micros: e.initial_margin_micros,
            maintenance_margin_micros: e.maintenance_margin_micros,
            realized_pnl_micros: e.realized_pnl_micros,
            margin_enforced: e.margin_enforced,
        }
    }
}

impl From<BacktestEconomicsReportDto> for BacktestEconomicsReport {
    fn from(e: BacktestEconomicsReportDto) -> Self {
        BacktestEconomicsReport {
            contract_multiplier: e.contract_multiplier,
            initial_margin_micros: e.initial_margin_micros,
            maintenance_margin_micros: e.maintenance_margin_micros,
            realized_pnl_micros: e.realized_pnl_micros,
            margin_enforced: e.margin_enforced,
        }
    }
}

/// Canonical, schema-versioned, lossless mirror of [`BacktestReport`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct BacktestReportArtifact {
    pub schema_version: u32,
    strategy_name: String,
    /// PROMOTION-EVIDENCE-SEMANTIC-BINDING-01: additive field, defaults to
    /// empty string when absent (an artifact written before this field
    /// existed) -- an empty/default value can never equal a real resolved
    /// `Strategy::semantic_fingerprint()`, so historical evidence fails
    /// closed for semantic-identity binding without needing a schema-version
    /// bump for this purely additive change.
    #[serde(default)]
    strategy_semantic_fingerprint: String,
    run_id: Uuid,
    config_id: Uuid,
    input_data_hash: String,
    halted: bool,
    halt_reason: Option<String>,
    equity_curve: Vec<(i64, i64)>,
    orders: Vec<BacktestOrderDto>,
    fills: Vec<BacktestFillDto>,
    last_prices: BTreeMap<String, i64>,
    execution_blocked: bool,
    first_bar_open_micros: Option<i64>,
    last_bar_close_micros: Option<i64>,
    sizing: StrategySizingConfigDto,
    economics: BacktestEconomicsReportDto,
    execution_model_id: String,
}

impl From<&BacktestReport> for BacktestReportArtifact {
    fn from(r: &BacktestReport) -> Self {
        Self {
            schema_version: BACKTEST_REPORT_ARTIFACT_SCHEMA_VERSION,
            strategy_name: r.strategy_name.clone(),
            strategy_semantic_fingerprint: r.strategy_semantic_fingerprint.clone(),
            run_id: r.run_id,
            config_id: r.config_id,
            input_data_hash: r.input_data_hash.clone(),
            halted: r.halted,
            halt_reason: r.halt_reason.clone(),
            equity_curve: r.equity_curve.clone(),
            orders: r.orders.iter().map(BacktestOrderDto::from).collect(),
            fills: r.fills.iter().map(BacktestFillDto::from).collect(),
            last_prices: r.last_prices.clone(),
            execution_blocked: r.execution_blocked,
            first_bar_open_micros: r.first_bar_open_micros,
            last_bar_close_micros: r.last_bar_close_micros,
            sizing: (&r.sizing).into(),
            economics: (&r.economics).into(),
            execution_model_id: r.execution_model_id.clone(),
        }
    }
}

impl From<BacktestReportArtifact> for BacktestReport {
    fn from(a: BacktestReportArtifact) -> Self {
        BacktestReport {
            strategy_name: a.strategy_name,
            strategy_semantic_fingerprint: a.strategy_semantic_fingerprint,
            run_id: a.run_id,
            config_id: a.config_id,
            input_data_hash: a.input_data_hash,
            halted: a.halted,
            halt_reason: a.halt_reason,
            equity_curve: a.equity_curve,
            orders: a.orders.into_iter().map(BacktestOrder::from).collect(),
            fills: a.fills.into_iter().map(BacktestFill::from).collect(),
            last_prices: a.last_prices,
            execution_blocked: a.execution_blocked,
            first_bar_open_micros: a.first_bar_open_micros,
            last_bar_close_micros: a.last_bar_close_micros,
            sizing: a.sizing.into(),
            economics: a.economics.into(),
            execution_model_id: a.execution_model_id,
        }
    }
}

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// Errors returned by [`load_canonical_backtest_report`].
///
/// Every variant is a fail-closed rejection -- there is no "best effort"
/// or optimistic-upgrade path from any of these states.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BacktestReportArtifactError {
    /// `manifest.json` is missing from the run directory.
    ManifestMissing,
    /// `manifest.json` exists but is not valid [`RunManifest`] JSON.
    ManifestParse(String),
    /// `backtest_report.json` is missing (e.g. an artifact written before
    /// this schema existed). Never optimistically reconstructed from
    /// `metrics.json`/CSVs.
    MissingCanonicalReport,
    /// `backtest_report.json` exists but is not valid JSON / does not match
    /// [`BacktestReportArtifact`]'s shape.
    MalformedJson(String),
    /// `schema_version` is not [`BACKTEST_REPORT_ARTIFACT_SCHEMA_VERSION`].
    UnsupportedSchemaVersion(u32),
    /// The canonical report's `run_id` disagrees with `manifest.json`'s.
    RunIdMismatch { manifest: Uuid, report: Uuid },
    /// The canonical report's `strategy_name` disagrees with `manifest.json`'s.
    StrategyNameMismatch { manifest: String, report: String },
    /// The canonical report's `config_id` (as a string) disagrees with
    /// `manifest.json`'s `config_hash`.
    ConfigIdMismatch {
        manifest_config_hash: String,
        report_config_id: String,
    },
    /// The canonical report's `execution_model_id` disagrees with
    /// `manifest.json`'s (only checked once the manifest carries a
    /// non-empty value -- see [`RunManifest::execution_model_id`]).
    ExecutionModelMismatch { manifest: String, report: String },
}

impl std::fmt::Display for BacktestReportArtifactError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ManifestMissing => write!(f, "manifest.json missing"),
            Self::ManifestParse(e) => write!(f, "manifest.json parse failed: {e}"),
            Self::MissingCanonicalReport => write!(
                f,
                "backtest_report.json missing (artifact predates canonical schema; not reconstructed)"
            ),
            Self::MalformedJson(e) => write!(f, "backtest_report.json parse failed: {e}"),
            Self::UnsupportedSchemaVersion(v) => {
                write!(f, "backtest_report.json schema_version {v} unsupported")
            }
            Self::RunIdMismatch { manifest, report } => write!(
                f,
                "run_id mismatch: manifest={manifest} backtest_report.json={report}"
            ),
            Self::StrategyNameMismatch { manifest, report } => write!(
                f,
                "strategy_name mismatch: manifest={manifest:?} backtest_report.json={report:?}"
            ),
            Self::ConfigIdMismatch {
                manifest_config_hash,
                report_config_id,
            } => write!(
                f,
                "config identity mismatch: manifest.config_hash={manifest_config_hash:?} backtest_report.json.config_id={report_config_id:?}"
            ),
            Self::ExecutionModelMismatch { manifest, report } => write!(
                f,
                "execution_model_id mismatch: manifest={manifest:?} backtest_report.json={report:?}"
            ),
        }
    }
}

impl std::error::Error for BacktestReportArtifactError {}

// ---------------------------------------------------------------------------
// Write / Load
// ---------------------------------------------------------------------------

/// Serialize `report` as the canonical `backtest_report.json` artifact and
/// write it to `run_dir`. Returns the path written.
///
/// Does not require `manifest.json` to exist -- callers that write artifacts
/// directly into a bare directory (without `init_run_artifacts`) still get a
/// loadable canonical report; only [`load_canonical_backtest_report`]'s
/// cross-validation requires a manifest.
pub fn write_canonical_backtest_report(run_dir: &Path, report: &BacktestReport) -> Result<PathBuf> {
    fs::create_dir_all(run_dir)
        .with_context(|| format!("create run dir failed: {}", run_dir.display()))?;
    let artifact = BacktestReportArtifact::from(report);
    let json = serde_json::to_string_pretty(&artifact)
        .context("serialize canonical backtest_report.json failed")?;
    let path = run_dir.join("backtest_report.json");
    fs::write(&path, format!("{json}\n"))
        .with_context(|| format!("write backtest_report.json failed: {}", path.display()))?;
    Ok(path)
}

/// Load and validate the canonical `BacktestReport` for `run_dir`.
///
/// Promotion-grade, fail-closed loader:
/// - `manifest.json` must exist and parse.
/// - `backtest_report.json` must exist, parse, and carry a supported
///   `schema_version`.
/// - `run_id`, `strategy_name`, config identity, and (when the manifest
///   carries one) `execution_model_id` must agree between the two files.
///
/// Never reconstructs a report from `metrics.json`/CSVs, and never
/// optimistically upgrades an artifact that predates this schema.
pub fn load_canonical_backtest_report(
    run_dir: &Path,
) -> Result<BacktestReport, BacktestReportArtifactError> {
    let manifest_path = run_dir.join("manifest.json");
    if !manifest_path.exists() {
        return Err(BacktestReportArtifactError::ManifestMissing);
    }
    let manifest_raw = fs::read_to_string(&manifest_path)
        .map_err(|e| BacktestReportArtifactError::ManifestParse(e.to_string()))?;
    let manifest: RunManifest = serde_json::from_str(&manifest_raw)
        .map_err(|e| BacktestReportArtifactError::ManifestParse(e.to_string()))?;

    let report_path = run_dir.join("backtest_report.json");
    if !report_path.exists() {
        return Err(BacktestReportArtifactError::MissingCanonicalReport);
    }
    let report_raw = fs::read_to_string(&report_path)
        .map_err(|e| BacktestReportArtifactError::MalformedJson(e.to_string()))?;
    let artifact: BacktestReportArtifact = serde_json::from_str(&report_raw)
        .map_err(|e| BacktestReportArtifactError::MalformedJson(e.to_string()))?;

    if artifact.schema_version != BACKTEST_REPORT_ARTIFACT_SCHEMA_VERSION {
        return Err(BacktestReportArtifactError::UnsupportedSchemaVersion(
            artifact.schema_version,
        ));
    }

    if artifact.run_id != manifest.run_id {
        return Err(BacktestReportArtifactError::RunIdMismatch {
            manifest: manifest.run_id,
            report: artifact.run_id,
        });
    }
    if artifact.strategy_name != manifest.strategy_name {
        return Err(BacktestReportArtifactError::StrategyNameMismatch {
            manifest: manifest.strategy_name,
            report: artifact.strategy_name,
        });
    }
    let report_config_id = artifact.config_id.to_string();
    if report_config_id != manifest.config_hash {
        return Err(BacktestReportArtifactError::ConfigIdMismatch {
            manifest_config_hash: manifest.config_hash,
            report_config_id,
        });
    }
    // Only enforced once the manifest actually carries an execution-model
    // identity (additive field -- see `RunManifest::execution_model_id`);
    // an empty manifest value means "not recorded", not "empty string".
    if !manifest.execution_model_id.is_empty()
        && manifest.execution_model_id != artifact.execution_model_id
    {
        return Err(BacktestReportArtifactError::ExecutionModelMismatch {
            manifest: manifest.execution_model_id,
            report: artifact.execution_model_id,
        });
    }

    Ok(artifact.into())
}
