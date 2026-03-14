use chrono::{TimeZone, Utc};
use mqk_schemas::Bar;
use mqk_testkit::run_parity_scenario_stub;

fn mk_bar(ts: i64, close: &str) -> Bar {
    Bar {
        ts_close_utc: Utc.timestamp_opt(ts, 0).single().expect("valid ts"),
        open: close.to_string(),
        high: close.to_string(),
        low: close.to_string(),
        close: close.to_string(),
        volume: "1000".to_string(),
    }
}

#[test]
fn test01_parity_runner_produces_non_empty_artifacts() {
    let bars = vec![
        mk_bar(1_700_000_000, "100.00"),
        mk_bar(1_700_000_060, "101.50"),
        mk_bar(1_700_000_120, "99.25"),
    ];

    let out = run_parity_scenario_stub(&bars).expect("parity run must succeed");

    assert!(out.orders_csv.contains("parity-entry"));
    assert!(out.fills_csv.contains("filled_qty"));
    assert!(out.equity_curve_csv.contains("equity_micros"));
    assert!(out.metrics_json.contains("gross_return_bps"));
    assert!(out.audit_jsonl.contains("bar_replay"));
}

#[test]
fn test01_parity_runner_is_deterministic_for_same_bars() {
    let bars = vec![
        mk_bar(1_700_000_000, "100.00"),
        mk_bar(1_700_000_060, "101.00"),
        mk_bar(1_700_000_120, "102.00"),
    ];

    let a = run_parity_scenario_stub(&bars).expect("run a");
    let b = run_parity_scenario_stub(&bars).expect("run b");

    assert_eq!(a.orders_csv, b.orders_csv);
    assert_eq!(a.fills_csv, b.fills_csv);
    assert_eq!(a.equity_curve_csv, b.equity_curve_csv);
    assert_eq!(a.metrics_json, b.metrics_json);
    assert_eq!(a.audit_jsonl, b.audit_jsonl);
}
