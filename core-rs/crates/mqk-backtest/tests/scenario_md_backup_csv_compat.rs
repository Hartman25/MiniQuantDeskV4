/// BACKTEST-DATASET-01: md_backup/1D CSV compatibility proof.
///
/// The exports/md_backup/1D CSV schema uses `t`/`f` for is_complete and
/// carries two extra columns (timeframe, ingested_at) not in the bkt4 fixture.
/// These tests prove the loader handles both without change to strategy logic.
use mqk_backtest::loader::{parse_csv_bars, LoadError};

/// Minimal inline fixture matching the exact md_backup/1D schema.
const MD_BACKUP_STYLE: &str = "\
symbol,timeframe,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete,ingested_at\n\
AAPL,1D,726105600,531250,535713,515625,520088,129136000,t,2026-04-19 12:21:42.508004-10\n\
AAPL,1D,726192000,517857,529017,511161,529017,186256000,f,2026-04-19 12:21:42.686801-10\n\
AAPL,1D,726278400,542411,553570,540179,551338,281400000,t,2026-04-19 12:21:42.703932-10\n\
";

/// DS-01: md_backup-style CSV with `t`/`f` booleans and extra columns is parsed correctly.
///
/// Proves:
/// - `t` → is_complete=true
/// - `f` → is_complete=false
/// - extra `timeframe` and `ingested_at` columns are silently ignored
/// - all required numeric fields parse correctly
#[test]
fn md_backup_csv_t_f_booleans_parse_correctly() {
    let bars = parse_csv_bars(MD_BACKUP_STYLE).expect("md_backup CSV must parse");

    assert_eq!(bars.len(), 3, "should load 3 bars");

    // Bars are sorted by (end_ts ASC, symbol ASC)
    assert_eq!(bars[0].symbol, "AAPL");
    assert_eq!(bars[0].end_ts, 726_105_600);
    assert_eq!(bars[0].open_micros, 531_250);
    assert_eq!(bars[0].high_micros, 535_713);
    assert_eq!(bars[0].low_micros, 515_625);
    assert_eq!(bars[0].close_micros, 520_088);
    assert_eq!(bars[0].volume, 129_136_000);
    assert!(bars[0].is_complete, "first bar: t → is_complete=true");

    assert_eq!(bars[1].end_ts, 726_192_000);
    assert!(!bars[1].is_complete, "second bar: f → is_complete=false");

    assert_eq!(bars[2].end_ts, 726_278_400);
    assert!(bars[2].is_complete, "third bar: t → is_complete=true");
}

/// DS-02: existing bkt4 fixture schema (numeric 1/0 for is_complete) is unaffected.
///
/// Regression guard — the `1`/`0` path through parse_bool must still work
/// after the `t`/`f` addition.
#[test]
fn bkt4_fixture_schema_still_parses() {
    let csv = "\
symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n\
TEST,60,995000,1010000,990000,1000000,1000,1\n\
TEST,120,1000000,1030000,1010000,1020000,1100,0\n\
";
    let bars = parse_csv_bars(csv).expect("bkt4-style CSV must parse");
    assert_eq!(bars.len(), 2);
    assert!(bars[0].is_complete);
    assert!(!bars[1].is_complete);
}

/// DS-03: an unrecognized is_complete value produces a clear ParseBool error.
#[test]
fn unrecognized_is_complete_value_returns_parse_bool_error() {
    let csv = "\
symbol,end_ts,open_micros,high_micros,low_micros,close_micros,volume,is_complete\n\
AAPL,726105600,531250,535713,515625,520088,129136000,x\n\
";
    let err = parse_csv_bars(csv).expect_err("invalid is_complete must error");
    match err {
        LoadError::ParseBool { column, value } => {
            assert_eq!(column, "is_complete");
            assert_eq!(value, "x");
        }
        other => panic!("expected ParseBool, got {:?}", other),
    }
}

/// DS-04: day_id is derived from end_ts when not present in md_backup CSV.
///
/// end_ts=726105600 / 86400 = 8403 whole days since epoch → 1993-01-04.
#[test]
fn day_id_derived_from_end_ts_for_md_backup_rows() {
    let bars = parse_csv_bars(MD_BACKUP_STYLE).expect("parse");
    assert_eq!(bars[0].day_id, 19_930_104);
}
