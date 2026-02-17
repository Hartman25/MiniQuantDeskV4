use mqk_promotion::{compare_candidates, PromotionCandidate, PromotionMetrics, TieBreakOrder, TieBreakRules};
use std::cmp::Ordering;

#[test]
fn tie_break_prefers_lower_mdd_then_higher_cagr() {
    let a = PromotionCandidate {
        name: "A".to_string(),
        metrics: PromotionMetrics {
            cagr: 0.10,
            max_drawdown: 0.10,
            sharpe: 1.0,
            profit_factor: 1.5,
            profitable_months_frac: 0.7,
        },
    };

    let b = PromotionCandidate {
        name: "B".to_string(),
        metrics: PromotionMetrics {
            cagr: 0.12,
            max_drawdown: 0.15,
            sharpe: 1.0,
            profit_factor: 1.5,
            profitable_months_frac: 0.7,
        },
    };

    let rules = TieBreakRules {
        within_points: 1e9, // force tie-break path
        order: vec![TieBreakOrder::LowerMdd, TieBreakOrder::HigherCagr],
    };

    // A should win due to lower MDD.
    assert_eq!(compare_candidates(&a, &b, &rules), Ordering::Less);
}
