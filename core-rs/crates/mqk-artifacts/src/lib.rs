use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RunManifest {
    pub schema_version: i32,
    pub run_id: Uuid,
    pub engine_id: String,
    pub mode: String,
    pub git_hash: String,
    pub config_hash: String,
    pub host_fingerprint: String,
    pub created_at_utc: DateTime<Utc>,
    pub artifacts: ArtifactList,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactList {
    pub audit_jsonl: String,
    pub manifest_json: String,
    pub orders_csv: String,
    pub fills_csv: String,
    pub equity_curve_csv: String,
    pub metrics_json: String,
}

pub struct InitRunArtifactsArgs<'a> {
    pub exports_root: &'a Path, // e.g. ../exports
    pub schema_version: i32,
    pub run_id: Uuid,
    pub engine_id: &'a str,
    pub mode: &'a str,
    pub git_hash: &'a str,
    pub config_hash: &'a str,
    pub host_fingerprint: &'a str,
    /// I9-4: injected creation time for deterministic manifest.
    ///
    /// Pass `time_source.now_utc()` in production or a fixed timestamp in
    /// tests.  Eliminates `Utc::now()` from the artifacts path so that two
    /// independent runs with identical inputs produce byte-identical manifests.
    pub now_utc: DateTime<Utc>,
}

pub struct InitRunArtifactsResult {
    pub run_dir: PathBuf,
    pub manifest_path: PathBuf,
}

pub fn init_run_artifacts(args: InitRunArtifactsArgs<'_>) -> Result<InitRunArtifactsResult> {
    // exports/<run_id>/
    let run_dir = args.exports_root.join(args.run_id.to_string());
    fs::create_dir_all(&run_dir)
        .with_context(|| format!("create exports dir failed: {}", run_dir.display()))?;

    // Create placeholder files if missing (do not overwrite existing).
    ensure_file_exists_with(&run_dir.join("audit.jsonl"), "")?;
    ensure_file_exists_with(
        &run_dir.join("orders.csv"),
        "ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status\n",
    )?;
    ensure_file_exists_with(
        &run_dir.join("fills.csv"),
        "ts_utc,fill_id,order_id,symbol,side,qty,price,fee\n",
    )?;
    ensure_file_exists_with(&run_dir.join("equity_curve.csv"), "ts_utc,equity\n")?;
    ensure_file_exists_with(&run_dir.join("metrics.json"), "{}\n")?;

    // Write manifest.json (overwrite is OK; it’s deterministic for a run start).
    let manifest = RunManifest {
        schema_version: args.schema_version,
        run_id: args.run_id,
        engine_id: args.engine_id.to_string(),
        mode: args.mode.to_string(),
        git_hash: args.git_hash.to_string(),
        config_hash: args.config_hash.to_string(),
        host_fingerprint: args.host_fingerprint.to_string(),
        created_at_utc: args.now_utc, // I9-4: injected, not Utc::now()
        artifacts: ArtifactList {
            audit_jsonl: "audit.jsonl".to_string(),
            manifest_json: "manifest.json".to_string(),
            orders_csv: "orders.csv".to_string(),
            fills_csv: "fills.csv".to_string(),
            equity_curve_csv: "equity_curve.csv".to_string(),
            metrics_json: "metrics.json".to_string(),
        },
    };

    let manifest_path = run_dir.join("manifest.json");
    let json = serde_json::to_string_pretty(&manifest).context("serialize manifest failed")?;
    fs::write(&manifest_path, format!("{json}\n"))
        .with_context(|| format!("write manifest failed: {}", manifest_path.display()))?;

    Ok(InitRunArtifactsResult {
        run_dir,
        manifest_path,
    })
}

fn ensure_file_exists_with(path: &Path, contents_if_create: &str) -> Result<()> {
    if path.exists() {
        return Ok(());
    }
    fs::write(path, contents_if_create)
        .with_context(|| format!("create placeholder failed: {}", path.display()))?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Backtest report writer (deterministic outputs)
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
struct BacktestMetrics<'a> {
    schema_version: i32,
    halted: bool,
    halt_reason: Option<&'a str>,
    execution_blocked: bool,
    bars: usize,
    orders: usize,
    orders_filled: usize,
    orders_rejected: usize,
    fills: usize,
    final_equity_micros: i64,
    symbols: Vec<&'a str>,
    last_prices_micros: std::collections::BTreeMap<&'a str, i64>,
}

/// Write deterministic backtest artifacts into an existing run directory.
///
/// This function performs explicit IO. It is intended to be called by CLI/daemons.
/// No wall-clock time is used; timestamps are derived from `report.equity_curve` / bar end_ts.
///
/// Files written (overwritten):
/// - `fills.csv`
/// - `equity_curve.csv`
/// - `metrics.json`
pub fn write_backtest_report(run_dir: &Path, report: &mqk_backtest::BacktestReport) -> Result<()> {
    fs::create_dir_all(run_dir).with_context(|| {
        format!(
            "create backtest artifacts dir failed: {}",
            run_dir.display()
        )
    })?;

    // orders.csv — BKT-04P: one row per intent (filled AND rejected).
    let mut orders_csv =
        String::from("ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status\n");
    for o in &report.orders {
        let side_str = match o.side {
            mqk_backtest::BacktestOrderSide::Buy => "BUY",
            mqk_backtest::BacktestOrderSide::Sell => "SELL",
        };
        let status_str = match o.status {
            mqk_backtest::OrderStatus::Filled => "FILLED",
            mqk_backtest::OrderStatus::Rejected => "REJECTED",
            mqk_backtest::OrderStatus::HaltTriggered => "HALT_TRIGGERED",
        };
        // 9 columns: ts_utc,order_id,symbol,side,qty,order_type,limit_price,stop_price,status
        // Backtest orders are always MARKET with no limit or stop price (both empty).
        orders_csv.push_str(&format!(
            "{},{},{},{},{},MARKET,,,{}\n",
            o.bar_end_ts,
            o.order_id,
            o.symbol,
            side_str,
            o.qty,
            status_str,
        ));
    }
    let orders_path = run_dir.join("orders.csv");
    fs::write(&orders_path, orders_csv)
        .with_context(|| format!("write orders.csv failed: {}", orders_path.display()))?;

    // fills.csv — BKT-01P: per-fill provenance with real bar timestamp and deterministic UUIDs.
    let mut fills_csv = String::from("ts_utc,fill_id,order_id,symbol,side,qty,price,fee\n");
    for f in &report.fills {
        let side = format!("{:?}", f.side).to_uppercase(); // BUY / SELL deterministically
        fills_csv.push_str(&format!(
            "{},{},{},{},{},{},{},{}\n",
            f.bar_end_ts, // exact bar end timestamp — unique per fill
            f.fill_id,    // deterministic UUIDv5 per fill
            f.order_id,   // deterministic UUIDv5 per originating order intent
            f.symbol,
            side,
            f.qty,
            f.price_micros,
            f.fee_micros
        ));
    }
    let fills_path = run_dir.join("fills.csv");
    fs::write(&fills_path, fills_csv)
        .with_context(|| format!("write fills.csv failed: {}", fills_path.display()))?;

    // equity_curve.csv (match placeholder header)
    let mut eq_csv = String::from("ts_utc,equity\n");
    for (ts, eq) in &report.equity_curve {
        eq_csv.push_str(&format!("{},{}\n", ts, eq));
    }
    let eq_path = run_dir.join("equity_curve.csv");
    fs::write(&eq_path, eq_csv)
        .with_context(|| format!("write equity_curve.csv failed: {}", eq_path.display()))?;

    // metrics.json
    let final_equity = report.equity_curve.last().map(|(_, eq)| *eq).unwrap_or(0);

    // deterministic symbol listing
    let mut symbols: Vec<&str> = report.last_prices.keys().map(|s| s.as_str()).collect();
    symbols.sort();

    let last_prices_micros = report
        .last_prices
        .iter()
        .map(|(k, v)| (k.as_str(), *v))
        .collect::<std::collections::BTreeMap<_, _>>();

    let orders_filled = report
        .orders
        .iter()
        .filter(|o| o.status == mqk_backtest::OrderStatus::Filled)
        .count();
    let orders_rejected = report
        .orders
        .iter()
        .filter(|o| o.status == mqk_backtest::OrderStatus::Rejected)
        .count();

    let metrics = BacktestMetrics {
        schema_version: 1,
        halted: report.halted,
        halt_reason: report.halt_reason.as_deref(),
        execution_blocked: report.execution_blocked,
        bars: report.equity_curve.len(),
        orders: report.orders.len(),
        orders_filled,
        orders_rejected,
        fills: report.fills.len(),
        final_equity_micros: final_equity,
        symbols,
        last_prices_micros,
    };

    let metrics_path = run_dir.join("metrics.json");
    let json = serde_json::to_string_pretty(&metrics).context("serialize metrics failed")?;
    fs::write(&metrics_path, format!("{json}\n"))
        .with_context(|| format!("write metrics.json failed: {}", metrics_path.display()))?;

    Ok(())
}

// ---------------------------------------------------------------------------
// Schema-correctness tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_backtest::{
        BacktestFill, BacktestOrder, BacktestOrderSide, BacktestReport, OrderStatus,
    };
    use mqk_portfolio::{Fill, Side};
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn test_report_with_orders() -> BacktestReport {
        let order_id = Uuid::new_v5(&Uuid::from_bytes([0u8; 16]), b"test_order");
        let fill_id = Uuid::new_v5(&Uuid::from_bytes([1u8; 16]), order_id.as_bytes());
        BacktestReport {
            strategy_name: "test_strategy".to_string(),
            run_id: Uuid::new_v5(&Uuid::from_bytes([2u8; 16]), b"run"),
            config_id: Uuid::new_v5(&Uuid::from_bytes([3u8; 16]), b"cfg"),
            halted: false,
            halt_reason: None,
            equity_curve: vec![(1_000, 1_000_000_000), (2_000, 1_010_000_000)],
            orders: vec![
                BacktestOrder {
                    order_id,
                    bar_end_ts: 1_000,
                    symbol: "SPY".to_string(),
                    side: BacktestOrderSide::Buy,
                    qty: 10,
                    status: OrderStatus::Filled,
                },
                BacktestOrder {
                    order_id: Uuid::new_v5(&Uuid::from_bytes([0u8; 16]), b"order2"),
                    bar_end_ts: 2_000,
                    symbol: "SPY".to_string(),
                    side: BacktestOrderSide::Sell,
                    qty: 10,
                    status: OrderStatus::Rejected,
                },
            ],
            fills: vec![BacktestFill {
                fill_id,
                order_id,
                bar_end_ts: 1_000,
                inner: Fill::new("SPY", Side::Buy, 10, 150_000_000, 5_000),
            }],
            last_prices: {
                let mut m = BTreeMap::new();
                m.insert("SPY".to_string(), 151_000_000i64);
                m
            },
            execution_blocked: false,
        }
    }

    /// ART-01: orders.csv header column count must match every data row.
    ///
    /// Regression guard for the schema mismatch where the 9-column header
    /// had only 8 fields per row (stop_price was missing, status landed in
    /// the wrong slot).
    #[test]
    fn orders_csv_header_matches_row_column_count() {
        let tmp = std::env::temp_dir()
            .join(format!("mqk_art_test_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report).unwrap();

        let orders_csv = std::fs::read_to_string(tmp.join("orders.csv")).unwrap();
        let lines: Vec<&str> =
            orders_csv.lines().filter(|l| !l.is_empty()).collect();

        assert!(!lines.is_empty(), "orders.csv must not be empty");
        let header = lines[0];
        let header_cols: usize = header.split(',').count();
        assert_eq!(
            header_cols, 9,
            "header must declare exactly 9 columns, got {}: '{}'",
            header_cols, header
        );

        for (i, row) in lines[1..].iter().enumerate() {
            let row_cols = row.split(',').count();
            assert_eq!(
                row_cols, header_cols,
                "data row {} has {} columns but header has {}; row: '{}'",
                i + 1, row_cols, header_cols, row
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ART-02: fills.csv header column count must match every data row.
    #[test]
    fn fills_csv_header_matches_row_column_count() {
        let tmp = std::env::temp_dir()
            .join(format!("mqk_art_test_fills_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report).unwrap();

        let fills_csv = std::fs::read_to_string(tmp.join("fills.csv")).unwrap();
        let lines: Vec<&str> =
            fills_csv.lines().filter(|l| !l.is_empty()).collect();

        assert!(!lines.is_empty(), "fills.csv must not be empty");
        let header = lines[0];
        let header_cols = header.split(',').count();
        assert_eq!(
            header_cols, 8,
            "fills.csv header must declare 8 columns, got {}: '{}'",
            header_cols, header
        );

        for (i, row) in lines[1..].iter().enumerate() {
            let row_cols = row.split(',').count();
            assert_eq!(
                row_cols, header_cols,
                "fills row {} has {} columns but header has {}; row: '{}'",
                i + 1, row_cols, header_cols, row
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ART-03: equity_curve.csv header/row agreement.
    #[test]
    fn equity_csv_header_matches_row_column_count() {
        let tmp = std::env::temp_dir()
            .join(format!("mqk_art_test_eq_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report).unwrap();

        let eq_csv = std::fs::read_to_string(tmp.join("equity_curve.csv")).unwrap();
        let lines: Vec<&str> =
            eq_csv.lines().filter(|l| !l.is_empty()).collect();

        assert!(!lines.is_empty());
        let header_cols = lines[0].split(',').count();
        assert_eq!(header_cols, 2, "equity_curve header must be 2 columns");
        for (i, row) in lines[1..].iter().enumerate() {
            let c = row.split(',').count();
            assert_eq!(c, header_cols, "equity_curve row {} mismatch", i + 1);
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ART-04: metrics.json is valid JSON with schema_version=1 and required fields.
    #[test]
    fn metrics_json_is_valid_and_versioned() {
        let tmp = std::env::temp_dir()
            .join(format!("mqk_art_test_metrics_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report).unwrap();

        let contents = std::fs::read_to_string(tmp.join("metrics.json")).unwrap();
        let v: serde_json::Value =
            serde_json::from_str(&contents).expect("metrics.json must be valid JSON");
        assert_eq!(v["schema_version"], 1, "schema_version must be 1");
        assert!(v["fills"].is_number(), "fills count must be present");
        assert!(v["orders"].is_number(), "orders count must be present");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// ART-05: orders.csv status column carries correct values (not in stop_price slot).
    ///
    /// Regression: before the fix, `MARKET,,{}` put status in column 7 (stop_price)
    /// and left column 8 (status) empty.
    #[test]
    fn orders_csv_status_column_is_correct() {
        let tmp = std::env::temp_dir()
            .join(format!("mqk_art_test_status_{}", std::process::id()));
        std::fs::create_dir_all(&tmp).unwrap();

        let report = test_report_with_orders();
        write_backtest_report(&tmp, &report).unwrap();

        let csv = std::fs::read_to_string(tmp.join("orders.csv")).unwrap();
        let mut lines = csv.lines().filter(|l| !l.is_empty());
        let header: Vec<&str> = lines.next().unwrap().split(',').collect();

        let status_idx = header
            .iter()
            .position(|&h| h == "status")
            .expect("status column must exist in header");
        let stop_price_idx = header
            .iter()
            .position(|&h| h == "stop_price")
            .expect("stop_price column must exist in header");

        for row_str in lines {
            let cols: Vec<&str> = row_str.split(',').collect();
            let status_val = cols[status_idx];
            let stop_price_val = cols[stop_price_idx];

            assert!(
                matches!(status_val, "FILLED" | "REJECTED" | "HALT_TRIGGERED"),
                "status column must be a valid status string, got '{}'",
                status_val
            );
            assert_eq!(
                stop_price_val, "",
                "stop_price must be empty for MARKET backtest orders, got '{}'",
                stop_price_val
            );
        }

        let _ = std::fs::remove_dir_all(&tmp);
    }
}
