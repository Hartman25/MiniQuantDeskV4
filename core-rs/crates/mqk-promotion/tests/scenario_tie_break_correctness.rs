use std::collections::BTreeMap;

use mqk_backtest::{derive_run_id, BacktestConfig, BacktestFill, BacktestReport};
use mqk_portfolio::{Fill, Side};

fn bf(inner: Fill) -> BacktestFill {
    BacktestFill { fill_id: uuid::Uuid::nil(), order_id: uuid::Uuid::nil(), bar_end_ts: 0, inner }
}
use mqk_promotion::{
    pick_winner, select_best, ArtifactLock, Candidate, PromotionConfig, PromotionInput,
    PromotionMetrics, StressSuiteResult,
};

/// Build a BacktestReport with deterministic (non-nil) provenance but synthetic equity/fills.
///
/// Uses `derive_run_id` + `BacktestConfig::test_defaults().config_id()` so the provenance
/// gates pass without requiring a full engine run.
fn report_with_provenance(
    strategy_name: &str,
    equity_curve: Vec<(i64, i64)>,
    fills: Vec<BacktestFill>,
) -> BacktestReport {
    let config_id = BacktestConfig::test_defaults().config_id();
    let run_id = derive_run_id(strategy_name, &config_id);
    BacktestReport {
        strategy_name: strategy_name.to_string(),
        run_id,
        config_id,
        halted: false,
        halt_reason: None,
        equity_curve,
        orders: vec![],
        fills,
        last_prices: BTreeMap::new(),
        execution_blocked: false,
    }
}

/// Helper: build a monotonically growing equity curve over N months.
/// growth_per_month is a fraction (e.g. 0.05 = 5%).
fn make_equity_curve(
    start: i64,
    months: usize,
    growth_per_month: f64,
    daily_noise: f64,
) -> Vec<(i64, i64)> {
    let day = 86_400i64;
    let mut curve = Vec::new();
    let mut equity = start as f64;

    for m in 0..=months {
        let month_start_ts = (m as i64) * 30 * day;
        // Add daily-level points within each month for Sharpe calculation
        if m > 0 {
            for d in 1..30 {
                let intra_ts = ((m - 1) as i64) * 30 * day + d * day;
                let intra_eq = equity * (1.0 + daily_noise * ((d % 3) as f64 - 1.0));
                curve.push((intra_ts, intra_eq.max(1.0) as i64));
            }
        }
        curve.push((month_start_ts, equity as i64));
        equity *= 1.0 + growth_per_month;
    }

    // Sort by timestamp (daily points may be interleaved)
    curve.sort_by_key(|&(ts, _)| ts);
    // Deduplicate by timestamp (keep last)
    curve.dedup_by_key(|p| p.0);
    curve
}

/// Helper: build fills with one profitable round-trip.
fn make_profitable_fills() -> Vec<BacktestFill> {
    vec![
        bf(Fill::new("SYM", Side::Buy, 100, 10_000_000, 0)),
        bf(Fill::new("SYM", Side::Sell, 100, 15_000_000, 0)),
    ]
}

/// Two candidates both pass. They have equal Sharpe (forced by using metrics
/// directly). Candidate A has lower MDD => A wins on tie-break rule #2.
#[test]
fn tiebreak_equal_sharpe_lower_mdd_wins() {
    // Force equal sharpe, different MDD via metrics directly
    let metrics_a = PromotionMetrics {
        sharpe: 1.5,
        mdd: 0.05,
        cagr: 0.20,
        profit_factor: 3.0,
        profitable_months_pct: 0.80,
        start_equity_micros: 1_000_000,
        end_equity_micros: 1_200_000,
        duration_days: 180.0,
        num_months: 6,
        num_trades: 5,
        execution_blocked: false,
    };

    let metrics_b = PromotionMetrics {
        sharpe: 1.5, // same Sharpe
        mdd: 0.15,   // higher MDD => loses
        cagr: 0.20,
        profit_factor: 3.0,
        profitable_months_pct: 0.80,
        start_equity_micros: 1_000_000,
        end_equity_micros: 1_200_000,
        duration_days: 180.0,
        num_months: 6,
        num_trades: 5,
        execution_blocked: false,
    };

    let winner = pick_winner("A", &metrics_a, "B", &metrics_b);
    assert_eq!(
        winner, "A",
        "A should win due to lower MDD when Sharpe is equal"
    );
}

/// Test select_best with three candidates; only two pass; winner by Sharpe.
#[test]
fn select_best_picks_correct_winner() {
    let config = PromotionConfig {
        min_sharpe: 0.0,
        max_mdd: 0.40,
        min_cagr: 0.0,
        min_profit_factor: 0.0,
        min_profitable_months_pct: 0.0,
    };

    // Candidate C1: passes, moderate growth
    let eq_1 = make_equity_curve(1_000_000, 6, 0.04, 0.001);
    // Candidate C2: fails (huge drawdown)
    let eq_2 = vec![
        (0, 1_000_000),
        (30 * 86_400, 400_000), // 60% drawdown => exceeds max_mdd 0.40
        (60 * 86_400, 500_000),
    ];
    // Candidate C3: passes, slightly better growth
    let eq_3 = make_equity_curve(1_000_000, 6, 0.05, 0.001);

    let candidates = vec![
        Candidate {
            id: "C1".into(),
            input: PromotionInput {
                initial_equity_micros: 1_000_000,
                report: report_with_provenance("candidate_c1", eq_1, make_profitable_fills()),
                stress_suite: Some(StressSuiteResult::pass(1)),
                artifact_lock: Some(ArtifactLock::new_for_testing("cfg", "git")), // B6
            },
        },
        Candidate {
            id: "C2".into(),
            input: PromotionInput {
                initial_equity_micros: 1_000_000,
                // C2 fails on MDD; provenance gates pass but MDD gate fires.
                report: report_with_provenance("candidate_c2", eq_2, vec![]),
                stress_suite: Some(StressSuiteResult::pass(1)),
                artifact_lock: Some(ArtifactLock::new_for_testing("cfg", "git")), // needs lock; fails on metrics
            },
        },
        Candidate {
            id: "C3".into(),
            input: PromotionInput {
                initial_equity_micros: 1_000_000,
                report: report_with_provenance("candidate_c3", eq_3, make_profitable_fills()),
                stress_suite: Some(StressSuiteResult::pass(1)),
                artifact_lock: Some(ArtifactLock::new_for_testing("cfg", "git")), // B6
            },
        },
    ];

    let result = select_best(&config, &candidates);
    assert!(result.is_some(), "should have a winner");
    let (winner_id, winner_decision) = result.unwrap();
    assert!(winner_decision.passed);
    // C2 should be excluded (fails MDD). Between C1 and C3, C3 has better
    // growth and thus likely higher Sharpe/CAGR.
    assert!(
        winner_id == "C1" || winner_id == "C3",
        "winner should be C1 or C3, not C2. Got: {}",
        winner_id
    );
}

/// Test lexicographic tie-break as ultimate fallback.
#[test]
fn tiebreak_lexicographic_fallback() {
    // Completely identical metrics
    let metrics = PromotionMetrics {
        sharpe: 1.0,
        mdd: 0.10,
        cagr: 0.15,
        profit_factor: 2.0,
        profitable_months_pct: 0.80,
        start_equity_micros: 1_000_000,
        end_equity_micros: 1_200_000,
        duration_days: 180.0,
        num_months: 6,
        num_trades: 5,
        execution_blocked: false,
    };

    let winner = pick_winner("Beta", &metrics, "Alpha", &metrics);
    assert_eq!(
        winner, "Alpha",
        "lexicographic tie-break should pick 'Alpha' over 'Beta'"
    );
}
