//! Sizing provenance artifact round-trip: new policy persists and reloads
//! exactly; legacy artifacts keep their bytes and still decode; tampered
//! provenance fails closed.

use mqk_artifacts::{
    init_run_artifacts, load_canonical_backtest_report, write_backtest_report,
    write_canonical_backtest_report, BacktestReportArtifactError, InitRunArtifactsArgs,
};
use mqk_backtest::{
    BacktestReport, SizingEntryProvenance, SizingPolicy, SizingProvenance, SizingRefusalProvenance,
};

fn entry() -> SizingEntryProvenance {
    SizingEntryProvenance {
        symbol: "SPY".into(),
        reference_bar_end_ts: 172_800,
        allocation_fraction_bps: 2_500,
        initial_allocated_capital_micros: 100_000_000_000,
        position_budget_micros: 25_000_000_000,
        causal_reference_price_micros: 100_000_000,
        uncapped_target_qty_micros: 250_000_000,
        resolved_target_qty_micros: 250_000_000,
        capped_by: "none".into(),
    }
}

fn capital_fraction_report() -> BacktestReport {
    BacktestReport {
        sizing_provenance: SizingProvenance {
            policy: SizingPolicy::capital_fraction_v1(2_500).unwrap(),
            entries: vec![entry()],
            refusals: vec![SizingRefusalProvenance {
                symbol: "SPY".into(),
                reference_bar_end_ts: Some(259_200),
                reason_code: "insufficient_budget_for_minimum_quantity".into(),
            }],
        },
        ..BacktestReport::test_fixture()
    }
}

fn write(report: &BacktestReport, dir: &std::path::Path) -> std::path::PathBuf {
    let now_utc: chrono::DateTime<chrono::Utc> = "2023-11-14T22:13:20Z".parse().unwrap();
    let init = init_run_artifacts(InitRunArtifactsArgs {
        exports_root: dir,
        schema_version: 1,
        run_id: report.run_id,
        strategy_name: report.strategy_name.as_str(),
        engine_id: "mqk-backtest",
        mode: "backtest",
        timeframe: None,
        timeframe_secs: Some(60),
        git_hash: "test",
        config_hash: &report.config_id.to_string(),
        host_fingerprint: "sizing-roundtrip",
        now_utc,
    })
    .unwrap();
    write_canonical_backtest_report(&init.run_dir, report).unwrap();
    init.run_dir
}

#[test]
fn capital_fraction_provenance_round_trips_exactly() {
    let tmp = tempfile::tempdir().unwrap();
    let report = capital_fraction_report();
    let run_dir = write(&report, tmp.path());
    let loaded = load_canonical_backtest_report(&run_dir).unwrap();
    assert_eq!(loaded.sizing_provenance, report.sizing_provenance);
    assert!(loaded.sizing_provenance.policy.is_capital_fraction());
}

#[test]
fn legacy_report_bytes_carry_no_sizing_key_and_decode_as_legacy() {
    let tmp = tempfile::tempdir().unwrap();
    let report = BacktestReport::test_fixture();
    let run_dir = write(&report, tmp.path());
    let raw = std::fs::read_to_string(run_dir.join("backtest_report.json")).unwrap();
    assert!(!raw.contains("sizing_provenance"), "legacy bytes unchanged");
    let loaded = load_canonical_backtest_report(&run_dir).unwrap();
    assert_eq!(loaded.sizing_provenance, SizingProvenance::default());
    assert_eq!(
        loaded.sizing_provenance.policy,
        SizingPolicy::FixedQuantityV1
    );
}

fn tamper(
    mutate: impl FnOnce(&mut serde_json::Value),
) -> Result<BacktestReport, BacktestReportArtifactError> {
    let tmp = tempfile::tempdir().unwrap();
    let run_dir = write(&capital_fraction_report(), tmp.path());
    let path = run_dir.join("backtest_report.json");
    let mut v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    mutate(&mut v["sizing_provenance"]);
    std::fs::write(&path, serde_json::to_string(&v).unwrap()).unwrap();
    load_canonical_backtest_report(&run_dir)
}

#[test]
fn tampered_provenance_fails_closed() {
    let unknown = tamper(|s| s["sizing_policy_id"] = "kelly_v9".into());
    assert!(matches!(
        unknown,
        Err(BacktestReportArtifactError::InvalidSizingProvenance(_))
    ));
    let zero_bps = tamper(|s| s["allocation_fraction_bps"] = 0.into());
    assert!(matches!(
        zero_bps,
        Err(BacktestReportArtifactError::InvalidSizingProvenance(_))
    ));
    let missing_bps = tamper(|s| {
        s.as_object_mut().unwrap().remove("allocation_fraction_bps");
    });
    assert!(matches!(
        missing_bps,
        Err(BacktestReportArtifactError::InvalidSizingProvenance(_))
    ));
    let legacy_with_entries = tamper(|s| {
        s["sizing_policy_id"] = "fixed_quantity_v1".into();
        s["allocation_fraction_bps"] = serde_json::Value::Null;
    });
    assert!(matches!(
        legacy_with_entries,
        Err(BacktestReportArtifactError::InvalidSizingProvenance(_))
    ));
}

#[test]
fn report_md_surfaces_realized_quantity_only_for_capital_fraction() {
    let tmp = tempfile::tempdir().unwrap();
    let cf = capital_fraction_report();
    let dir = tmp.path().join("cf");
    write_backtest_report(&dir, &cf, 100_000_000_000).unwrap();
    let md = std::fs::read_to_string(dir.join("report.md")).unwrap();
    assert!(md.contains("## Capital-Fraction Sizing"));
    assert!(md.contains("250000000"));

    let legacy = BacktestReport::test_fixture();
    let dir = tmp.path().join("legacy");
    write_backtest_report(&dir, &legacy, 100_000_000_000).unwrap();
    let md = std::fs::read_to_string(dir.join("report.md")).unwrap();
    assert!(!md.contains("Capital-Fraction Sizing"));
}
