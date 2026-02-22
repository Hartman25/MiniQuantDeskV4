//! Data Quality Gate report builder for normalized OHLCV bars.
//!
//! Accepts a slice of [`NormalizedBar`] and produces a [`QualityReport`]
//! covering:
//! - total bar count
//! - incomplete bar count (`is_complete == false`)
//! - duplicate canonical keys `(symbol, timeframe, end_ts)`
//! - monotonicity violations per `(symbol, timeframe)` series
//! - gap events per `(symbol, timeframe)` series for known timeframes
//! - unknown timeframe count
//! - earliest / latest `end_ts` overall
//!
//! This module does **not**:
//! - fetch data (no providers)
//! - write to the database
//! - perform normalization (see `normalizer.rs`)

use std::collections::BTreeMap;
use std::fmt;

use crate::normalizer::NormalizedBar;

// ---------------------------------------------------------------------------
// Timeframe step inference
// ---------------------------------------------------------------------------

/// Returns the expected bar-to-bar step in seconds for a canonical timeframe
/// string, or `None` if the timeframe is not recognised.
fn expected_step_secs(timeframe: &str) -> Option<i64> {
    match timeframe {
        "1m" => Some(60),
        "5m" => Some(300),
        "1D" => Some(86_400),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Issue types
// ---------------------------------------------------------------------------

/// The canonical key that uniquely identifies a bar.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct BarKey {
    pub symbol: String,
    pub timeframe: String,
    pub end_ts: i64,
}

impl fmt::Display for BarKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {}, {})", self.symbol, self.timeframe, self.end_ts)
    }
}

/// The canonical series key `(symbol, timeframe)`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct SeriesKey {
    pub symbol: String,
    pub timeframe: String,
}

impl fmt::Display for SeriesKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "({}, {})", self.symbol, self.timeframe)
    }
}

/// A duplicate occurrence: the canonical key appeared more than once.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DuplicateIssue {
    pub key: BarKey,
    /// How many times the key appears in the input (always >= 2).
    pub count: usize,
}

/// A monotonicity violation: within a series, `end_ts` is not strictly
/// increasing between consecutive bars (when sorted by `end_ts`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonotonicityIssue {
    pub series: SeriesKey,
    /// The offending `end_ts` (the one that is <= its predecessor).
    pub end_ts: i64,
    /// The predecessor `end_ts`.
    pub prev_end_ts: i64,
}

/// A gap event: the delta between consecutive `end_ts` values in a series
/// exceeds the expected step for that timeframe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GapIssue {
    pub series: SeriesKey,
    /// `end_ts` of the bar *before* the gap.
    pub prev_end_ts: i64,
    /// `end_ts` of the bar *after* the gap.
    pub next_end_ts: i64,
    /// Observed delta in seconds.
    pub delta_secs: i64,
    /// Expected step in seconds for this timeframe.
    pub expected_step_secs: i64,
}

// ---------------------------------------------------------------------------
// Report
// ---------------------------------------------------------------------------

/// Summary statistics and issue lists produced by [`build_quality_report`].
#[derive(Debug, Clone)]
pub struct QualityReport {
    /// Total number of input bars.
    pub total_bars: usize,
    /// Number of bars where `is_complete == false`.
    pub incomplete_bars: usize,
    /// Earliest `end_ts` seen across all bars, or `None` if input was empty.
    pub earliest_end_ts: Option<i64>,
    /// Latest `end_ts` seen across all bars, or `None` if input was empty.
    pub latest_end_ts: Option<i64>,
    /// Number of series whose timeframe string was not recognised.
    /// Gap detection is skipped for these series.
    pub unknown_timeframe_series_count: usize,
    /// Duplicate bar keys, sorted deterministically by key.
    pub duplicates: Vec<DuplicateIssue>,
    /// Monotonicity violations, sorted by `(series, end_ts)`.
    pub monotonicity_violations: Vec<MonotonicityIssue>,
    /// Gap events, sorted by `(series, prev_end_ts)`.
    pub gaps: Vec<GapIssue>,
}

impl QualityReport {
    /// Returns `true` when the report contains no issues of any kind.
    pub fn is_clean(&self) -> bool {
        self.duplicates.is_empty()
            && self.monotonicity_violations.is_empty()
            && self.gaps.is_empty()
    }
}

impl fmt::Display for QualityReport {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        writeln!(f, "QualityReport {{")?;
        writeln!(f, "  total_bars: {}", self.total_bars)?;
        writeln!(f, "  incomplete_bars: {}", self.incomplete_bars)?;
        writeln!(
            f,
            "  earliest_end_ts: {}",
            self.earliest_end_ts
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string())
        )?;
        writeln!(
            f,
            "  latest_end_ts: {}",
            self.latest_end_ts
                .map(|v| v.to_string())
                .unwrap_or_else(|| "none".to_string())
        )?;
        writeln!(
            f,
            "  unknown_timeframe_series: {}",
            self.unknown_timeframe_series_count
        )?;
        writeln!(f, "  duplicates: {}", self.duplicates.len())?;
        for d in &self.duplicates {
            writeln!(f, "    key={} count={}", d.key, d.count)?;
        }
        writeln!(
            f,
            "  monotonicity_violations: {}",
            self.monotonicity_violations.len()
        )?;
        for m in &self.monotonicity_violations {
            writeln!(
                f,
                "    series={} end_ts={} prev_end_ts={}",
                m.series, m.end_ts, m.prev_end_ts
            )?;
        }
        writeln!(f, "  gaps: {}", self.gaps.len())?;
        for g in &self.gaps {
            writeln!(
                f,
                "    series={} prev={} next={} delta={}s expected={}s",
                g.series, g.prev_end_ts, g.next_end_ts, g.delta_secs, g.expected_step_secs
            )?;
        }
        write!(f, "}}")
    }
}

// ---------------------------------------------------------------------------
// Main entry point
// ---------------------------------------------------------------------------

/// Build a [`QualityReport`] from a slice of normalized bars.
///
/// The function is **deterministic**: it sorts bars internally and produces
/// the same report regardless of the order in which bars appear in `bars`.
/// No mutation of the caller's data occurs.
pub fn build_quality_report(bars: &[NormalizedBar]) -> QualityReport {
    let total_bars = bars.len();
    let incomplete_bars = bars.iter().filter(|b| !b.is_complete).count();

    let earliest_end_ts = bars.iter().map(|b| b.end_ts).min();
    let latest_end_ts = bars.iter().map(|b| b.end_ts).max();

    // --- Duplicate detection ---
    // Count occurrences of each canonical key using a BTreeMap for
    // deterministic iteration order.
    let mut key_counts: BTreeMap<BarKey, usize> = BTreeMap::new();
    for bar in bars {
        let key = BarKey {
            symbol: bar.symbol.clone(),
            timeframe: bar.timeframe.clone(),
            end_ts: bar.end_ts,
        };
        *key_counts.entry(key).or_insert(0) += 1;
    }
    let duplicates: Vec<DuplicateIssue> = key_counts
        .into_iter()
        .filter(|(_, count)| *count >= 2)
        .map(|(key, count)| DuplicateIssue { key, count })
        .collect();
    // Already sorted by key because we iterated a BTreeMap.

    // --- Group bars by (symbol, timeframe) ---
    // Use BTreeMap so series iteration order is deterministic.
    let mut series_map: BTreeMap<SeriesKey, Vec<i64>> = BTreeMap::new();
    let mut unknown_tf_series: std::collections::BTreeSet<SeriesKey> =
        std::collections::BTreeSet::new();

    for bar in bars {
        let sk = SeriesKey {
            symbol: bar.symbol.clone(),
            timeframe: bar.timeframe.clone(),
        };
        series_map.entry(sk.clone()).or_default().push(bar.end_ts);

        if expected_step_secs(&bar.timeframe).is_none() {
            unknown_tf_series.insert(sk);
        }
    }

    // Sort timestamps within each series.
    for timestamps in series_map.values_mut() {
        timestamps.sort_unstable();
    }

    let unknown_timeframe_series_count = unknown_tf_series.len();

    // --- Monotonicity and gap detection ---
    let mut monotonicity_violations: Vec<MonotonicityIssue> = Vec::new();
    let mut gaps: Vec<GapIssue> = Vec::new();

    for (series, timestamps) in &series_map {
        let step = expected_step_secs(&series.timeframe);

        for window in timestamps.windows(2) {
            let prev = window[0];
            let next = window[1];

            // Monotonicity: after sorting, equal timestamps are a violation
            // (duplicates were already counted separately; here we flag
            // non-strictly-increasing pairs).
            if next <= prev {
                monotonicity_violations.push(MonotonicityIssue {
                    series: series.clone(),
                    end_ts: next,
                    prev_end_ts: prev,
                });
            }

            // Gap detection (only for known timeframes and strictly
            // increasing pairs so we don't double-count monotonicity issues).
            if let Some(expected) = step {
                if next > prev {
                    let delta = next - prev;
                    if delta > expected {
                        gaps.push(GapIssue {
                            series: series.clone(),
                            prev_end_ts: prev,
                            next_end_ts: next,
                            delta_secs: delta,
                            expected_step_secs: expected,
                        });
                    }
                }
            }
        }
    }

    // Issues are already grouped by series (BTreeMap order), then by
    // ascending end_ts within each series, so no further sort is needed.

    QualityReport {
        total_bars,
        incomplete_bars,
        earliest_end_ts,
        latest_end_ts,
        unknown_timeframe_series_count,
        duplicates,
        monotonicity_violations,
        gaps,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::normalizer::NormalizedBar;

    /// Construct a minimal `NormalizedBar` for testing.
    fn nb(symbol: &str, timeframe: &str, end_ts: i64, is_complete: bool) -> NormalizedBar {
        NormalizedBar {
            symbol: symbol.to_string(),
            timeframe: timeframe.to_string(),
            end_ts,
            open_micros: 100_000_000,
            high_micros: 105_000_000,
            low_micros: 99_000_000,
            close_micros: 103_000_000,
            volume: 1_000,
            is_complete,
        }
    }

    fn complete(symbol: &str, timeframe: &str, end_ts: i64) -> NormalizedBar {
        nb(symbol, timeframe, end_ts, true)
    }

    // --- basic counts ---

    #[test]
    fn empty_input_produces_zero_counts() {
        let report = build_quality_report(&[]);
        assert_eq!(report.total_bars, 0);
        assert_eq!(report.incomplete_bars, 0);
        assert!(report.earliest_end_ts.is_none());
        assert!(report.latest_end_ts.is_none());
        assert!(report.is_clean());
    }

    #[test]
    fn total_and_incomplete_counts() {
        let bars = vec![
            complete("AAPL", "1D", 1000),
            nb("AAPL", "1D", 2000, false),
            nb("MSFT", "1D", 1000, false),
        ];
        let report = build_quality_report(&bars);
        assert_eq!(report.total_bars, 3);
        assert_eq!(report.incomplete_bars, 2);
    }

    #[test]
    fn earliest_and_latest_end_ts() {
        let bars = vec![
            complete("AAPL", "1D", 3000),
            complete("AAPL", "1D", 1000),
            complete("AAPL", "1D", 2000),
        ];
        let report = build_quality_report(&bars);
        assert_eq!(report.earliest_end_ts, Some(1000));
        assert_eq!(report.latest_end_ts, Some(3000));
    }

    // --- duplicates ---

    #[test]
    fn no_duplicates_when_all_keys_unique() {
        let bars = vec![
            complete("AAPL", "1D", 1000),
            complete("AAPL", "1D", 2000),
            complete("MSFT", "1D", 1000),
        ];
        let report = build_quality_report(&bars);
        assert!(report.duplicates.is_empty());
    }

    #[test]
    fn duplicates_detected_for_repeated_key() {
        let bars = vec![
            complete("AAPL", "1D", 1000),
            complete("AAPL", "1D", 1000), // duplicate
            complete("AAPL", "1D", 2000),
        ];
        let report = build_quality_report(&bars);
        assert_eq!(report.duplicates.len(), 1);
        assert_eq!(report.duplicates[0].key.symbol, "AAPL");
        assert_eq!(report.duplicates[0].key.end_ts, 1000);
        assert_eq!(report.duplicates[0].count, 2);
    }

    #[test]
    fn triple_duplicate_counted_correctly() {
        let bars = vec![
            complete("SPY", "1m", 60),
            complete("SPY", "1m", 60),
            complete("SPY", "1m", 60),
        ];
        let report = build_quality_report(&bars);
        assert_eq!(report.duplicates.len(), 1);
        assert_eq!(report.duplicates[0].count, 3);
    }

    #[test]
    fn duplicates_sorted_by_key() {
        let bars = vec![
            complete("MSFT", "1D", 2000),
            complete("MSFT", "1D", 2000),
            complete("AAPL", "1D", 1000),
            complete("AAPL", "1D", 1000),
        ];
        let report = build_quality_report(&bars);
        assert_eq!(report.duplicates.len(), 2);
        // BTreeMap order: AAPL before MSFT
        assert_eq!(report.duplicates[0].key.symbol, "AAPL");
        assert_eq!(report.duplicates[1].key.symbol, "MSFT");
    }

    // --- monotonicity ---

    #[test]
    fn no_monotonicity_violation_for_strictly_increasing_series() {
        let bars = vec![
            complete("AAPL", "1D", 86_400),
            complete("AAPL", "1D", 172_800),
            complete("AAPL", "1D", 259_200),
        ];
        let report = build_quality_report(&bars);
        assert!(report.monotonicity_violations.is_empty());
    }

    #[test]
    fn monotonicity_violation_detected_for_equal_timestamps() {
        // Two bars with same end_ts in the same series (after dedup sorting,
        // next == prev triggers the violation).
        let bars = vec![
            complete("AAPL", "1D", 86_400),
            complete("AAPL", "1D", 86_400),
        ];
        let report = build_quality_report(&bars);
        // Duplicate AND monotonicity violation.
        assert!(!report.monotonicity_violations.is_empty());
        assert_eq!(report.monotonicity_violations[0].series.symbol, "AAPL");
    }

    #[test]
    fn monotonicity_violations_sorted_by_series_then_ts() {
        let bars = vec![
            complete("MSFT", "1D", 200),
            complete("MSFT", "1D", 200), // triggers violation at ts=200
            complete("AAPL", "1D", 100),
            complete("AAPL", "1D", 100), // triggers violation at ts=100
        ];
        let report = build_quality_report(&bars);
        assert_eq!(report.monotonicity_violations.len(), 2);
        // AAPL series comes first alphabetically.
        assert_eq!(report.monotonicity_violations[0].series.symbol, "AAPL");
        assert_eq!(report.monotonicity_violations[1].series.symbol, "MSFT");
    }

    // --- gaps ---

    #[test]
    fn no_gap_for_perfectly_spaced_1m_series() {
        let bars = vec![
            complete("AAPL", "1m", 60),
            complete("AAPL", "1m", 120),
            complete("AAPL", "1m", 180),
        ];
        let report = build_quality_report(&bars);
        assert!(report.gaps.is_empty());
    }

    #[test]
    fn gap_detected_for_missing_1m_bar() {
        let bars = vec![
            complete("AAPL", "1m", 60),
            complete("AAPL", "1m", 120),
            // gap: next bar is at 240 instead of 180
            complete("AAPL", "1m", 240),
        ];
        let report = build_quality_report(&bars);
        assert_eq!(report.gaps.len(), 1);
        let gap = &report.gaps[0];
        assert_eq!(gap.series.symbol, "AAPL");
        assert_eq!(gap.series.timeframe, "1m");
        assert_eq!(gap.prev_end_ts, 120);
        assert_eq!(gap.next_end_ts, 240);
        assert_eq!(gap.delta_secs, 120);
        assert_eq!(gap.expected_step_secs, 60);
    }

    #[test]
    fn gap_detected_for_5m_series() {
        let bars = vec![
            complete("SPY", "5m", 300),
            complete("SPY", "5m", 600),
            // gap: 1200 is 600s after 600, expected 300s
            complete("SPY", "5m", 1200),
        ];
        let report = build_quality_report(&bars);
        assert_eq!(report.gaps.len(), 1);
        assert_eq!(report.gaps[0].expected_step_secs, 300);
        assert_eq!(report.gaps[0].delta_secs, 600);
    }

    #[test]
    fn gap_detected_for_1d_series() {
        let day = 86_400_i64;
        let bars = vec![
            complete("MSFT", "1D", day),
            complete("MSFT", "1D", day * 2),
            // gap: skipped a day
            complete("MSFT", "1D", day * 4),
        ];
        let report = build_quality_report(&bars);
        assert_eq!(report.gaps.len(), 1);
        assert_eq!(report.gaps[0].expected_step_secs, 86_400);
        assert_eq!(report.gaps[0].delta_secs, 86_400 * 2);
    }

    #[test]
    fn unknown_timeframe_not_gap_checked() {
        let bars = vec![
            complete("ETH", "15m", 900),
            // large jump — but timeframe unknown so no gap flagged
            complete("ETH", "15m", 9000),
        ];
        let report = build_quality_report(&bars);
        assert!(report.gaps.is_empty());
        assert_eq!(report.unknown_timeframe_series_count, 1);
    }

    #[test]
    fn multiple_unknown_timeframe_series_counted() {
        let bars = vec![complete("ETH", "15m", 900), complete("BTC", "4h", 14400)];
        let report = build_quality_report(&bars);
        assert_eq!(report.unknown_timeframe_series_count, 2);
    }

    // --- determinism ---

    #[test]
    fn deterministic_same_order() {
        let bars = vec![
            complete("AAPL", "1m", 60),
            complete("AAPL", "1m", 120),
            complete("AAPL", "1m", 240), // gap
            complete("AAPL", "1m", 60),  // duplicate
        ];
        let r1 = build_quality_report(&bars);
        let r2 = build_quality_report(&bars);
        // Reports must be identical.
        assert_eq!(r1.total_bars, r2.total_bars);
        assert_eq!(r1.duplicates.len(), r2.duplicates.len());
        assert_eq!(r1.gaps.len(), r2.gaps.len());
        assert_eq!(
            r1.monotonicity_violations.len(),
            r2.monotonicity_violations.len()
        );
        if !r1.gaps.is_empty() {
            assert_eq!(r1.gaps[0].prev_end_ts, r2.gaps[0].prev_end_ts);
        }
    }

    #[test]
    fn deterministic_shuffled_order() {
        // Original order
        let bars_a = vec![
            complete("AAPL", "1m", 60),
            complete("AAPL", "1m", 120),
            complete("AAPL", "1m", 240),
            complete("MSFT", "1D", 86_400),
            complete("MSFT", "1D", 86_400 * 3), // gap
        ];
        // Shuffled order
        let bars_b = vec![
            complete("MSFT", "1D", 86_400 * 3),
            complete("AAPL", "1m", 240),
            complete("MSFT", "1D", 86_400),
            complete("AAPL", "1m", 60),
            complete("AAPL", "1m", 120),
        ];
        let ra = build_quality_report(&bars_a);
        let rb = build_quality_report(&bars_b);

        assert_eq!(ra.total_bars, rb.total_bars);
        assert_eq!(ra.incomplete_bars, rb.incomplete_bars);
        assert_eq!(ra.earliest_end_ts, rb.earliest_end_ts);
        assert_eq!(ra.latest_end_ts, rb.latest_end_ts);
        assert_eq!(ra.duplicates.len(), rb.duplicates.len());
        assert_eq!(ra.gaps.len(), rb.gaps.len());
        assert_eq!(
            ra.monotonicity_violations.len(),
            rb.monotonicity_violations.len()
        );

        // Verify gap details are identical
        for (ga, gb) in ra.gaps.iter().zip(rb.gaps.iter()) {
            assert_eq!(ga.series, gb.series);
            assert_eq!(ga.prev_end_ts, gb.prev_end_ts);
            assert_eq!(ga.next_end_ts, gb.next_end_ts);
            assert_eq!(ga.delta_secs, gb.delta_secs);
        }
    }

    // --- is_clean helper ---

    #[test]
    fn is_clean_true_when_no_issues() {
        let bars = vec![
            complete("AAPL", "1D", 86_400),
            complete("AAPL", "1D", 172_800),
        ];
        let report = build_quality_report(&bars);
        assert!(report.is_clean());
    }

    #[test]
    fn is_clean_false_when_gap_present() {
        let bars = vec![
            complete("AAPL", "1D", 86_400),
            complete("AAPL", "1D", 86_400 * 3), // gap
        ];
        let report = build_quality_report(&bars);
        assert!(!report.is_clean());
    }

    // --- Display smoke test ---

    #[test]
    fn display_does_not_panic() {
        let bars = vec![
            complete("AAPL", "1m", 60),
            complete("AAPL", "1m", 60),  // dup + monotonicity
            complete("AAPL", "1m", 240), // gap
            nb("MSFT", "1D", 86_400, false),
        ];
        let report = build_quality_report(&bars);
        let s = report.to_string();
        assert!(s.contains("QualityReport"));
        assert!(s.contains("total_bars: 4"));
    }
}
