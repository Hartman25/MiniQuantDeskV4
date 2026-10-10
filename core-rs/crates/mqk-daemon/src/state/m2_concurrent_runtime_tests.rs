//! M2 concurrent multi-strategy / multi-symbol runtime proofs.
//!
//! Every test drives a real production entry point: the selected-host tick
//! (`AppState::tick_strategy_dispatch_selected_hosts_with_bar_facts`) over a
//! real `DynamicSelectionHostPool` of real strategy engines, the driver's
//! deposit/confirm step, or the per-tick conflict seam `gather_and_resolve`.
//! DB-backed tests use the disposable port-5434 database through the shared
//! `db_or_skip` policy (a configured-but-unusable database fails, never
//! skips). Nothing here touches a broker, submits an order, or activates
//! Paper/Live.

use super::*;
use crate::decision::InternalStrategyDecision;
use crate::runtime_opportunity_allocation::PendingDecisionWithBarFacts;
use crate::runtime_strategy_conflict::{
    gather_and_resolve, refuse_unarbitrated_competition, UnarbitratedRefusal,
};
use mqk_schemas::QtyMicros;
use std::collections::BTreeMap;

const TF_5M: &str = "5m";
const FIVE_MIN: i64 = 300;

fn run_id(tag: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, format!("m2rt.run.{tag}").as_bytes())
}

// ---------------------------------------------------------------------------
// Same-symbol competing strategies need enforced arbitration.
// ---------------------------------------------------------------------------

fn qty(units: i64) -> QtyMicros {
    QtyMicros::from_whole_units(units).expect("qty")
}

fn pending(symbol: &str, strategy_id: &str, side: &str, units: i64) -> PendingDecisionWithBarFacts {
    PendingDecisionWithBarFacts {
        decision: InternalStrategyDecision {
            decision_id: format!("{side}-{symbol}-{strategy_id}"),
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs: FIVE_MIN,
            strategy_semantic_fingerprint: String::new(),
            side: side.to_string(),
            qty: qty(units),
            order_type: "market".to_string(),
            time_in_force: "day".to_string(),
            limit_price: None,
        },
        bar_facts: Some(EvaluatedBarFacts {
            symbol: symbol.to_string(),
            strategy_id: strategy_id.to_string(),
            timeframe: TF_5M.to_string(),
            bar_end_ts: 1_000,
            close_micros: 100_000_000,
        }),
        dynamic_selection_provenance: None,
    }
}

fn ids(v: &[PendingDecisionWithBarFacts]) -> Vec<(String, String)> {
    v.iter()
        .map(|p| (p.decision.symbol.clone(), p.decision.strategy_id.clone()))
        .collect()
}

#[test]
fn unarbitrated_competition_is_withheld_per_symbol_and_nothing_else() {
    // (decisions, expected kept, expected refusals)
    let cases: Vec<(
        &str,
        Vec<PendingDecisionWithBarFacts>,
        Vec<(&str, &str)>,
        Vec<UnarbitratedRefusal>,
    )> = vec![
        (
            "two strategies, one symbol: both withheld",
            vec![
                pending("AAPL", "a", "buy", 1),
                pending("AAPL", "b", "buy", 1),
            ],
            vec![],
            vec![UnarbitratedRefusal {
                symbol: "AAPL".into(),
                strategy_ids: vec!["a".into(), "b".into()],
            }],
        ),
        (
            "opposite sides are competition too",
            vec![
                pending("AAPL", "a", "buy", 1),
                pending("AAPL", "b", "sell", 1),
            ],
            vec![],
            vec![UnarbitratedRefusal {
                symbol: "AAPL".into(),
                strategy_ids: vec!["a".into(), "b".into()],
            }],
        ),
        (
            "symbol casing cannot hide competition",
            vec![
                pending("AAPL", "a", "buy", 1),
                pending("aapl", "b", "buy", 1),
            ],
            vec![],
            vec![UnarbitratedRefusal {
                symbol: "AAPL".into(),
                strategy_ids: vec!["a".into(), "b".into()],
            }],
        ),
        (
            "competition on one symbol leaves other symbols untouched",
            vec![
                pending("AAPL", "a", "buy", 1),
                pending("AAPL", "b", "buy", 1),
                pending("MSFT", "a", "buy", 1),
            ],
            vec![("MSFT", "a")],
            vec![UnarbitratedRefusal {
                symbol: "AAPL".into(),
                strategy_ids: vec!["a".into(), "b".into()],
            }],
        ),
        (
            "one strategy per symbol passes through",
            vec![
                pending("AAPL", "a", "buy", 1),
                pending("MSFT", "b", "buy", 1),
            ],
            vec![("AAPL", "a"), ("MSFT", "b")],
            vec![],
        ),
        (
            "one strategy emitting two decisions is not competition",
            vec![
                pending("AAPL", "a", "buy", 1),
                pending("AAPL", "a", "sell", 1),
            ],
            vec![("AAPL", "a"), ("AAPL", "a")],
            vec![],
        ),
        ("empty batch", vec![], vec![], vec![]),
    ];
    for (name, decisions, kept, refusals) in cases {
        let (got_kept, got_refusals) = refuse_unarbitrated_competition(decisions);
        let kept: Vec<(String, String)> = kept
            .into_iter()
            .map(|(s, i)| (s.to_string(), i.to_string()))
            .collect();
        assert_eq!(ids(&got_kept), kept, "{name}: kept");
        assert_eq!(got_refusals, refusals, "{name}: refusals");
    }
}

const CONFLICT_ENV: &str = "MQK_STRATEGY_CONFLICT_POLICY_MODE";

/// The production per-tick seam, in every effective mode, over the same
/// competing batch plus an uncontested symbol.
#[tokio::test]
async fn gather_and_resolve_never_lets_competing_proposals_through_unarbitrated() {
    let _env = shared_test_locks::strategy_fleet_env_test_lock()
        .lock()
        .await;
    let state = Arc::new(AppState::new_for_test_with_mode_and_broker(
        DeploymentMode::Paper,
        BrokerKind::Alpaca,
    ));
    let batch = || {
        vec![
            pending("AAPL", "a", "buy", 5),
            pending("AAPL", "b", "buy", 3),
            pending("MSFT", "a", "buy", 2),
        ]
    };
    let positions: BTreeMap<String, QtyMicros> = BTreeMap::new();

    // (env value, expected submitted decisions)
    let cases: [(Option<&str>, Vec<(&str, &str)>); 3] = [
        (None, vec![("MSFT", "a")]),
        (Some("shadow"), vec![("MSFT", "a")]),
        // Enforced: Bundle 6 arbitrates.
        (Some("paper_enforced"), vec![]),
    ];
    for (env, expected) in cases {
        match env {
            Some(v) => std::env::set_var(CONFLICT_ENV, v),
            None => std::env::remove_var(CONFLICT_ENV),
        }
        let out = gather_and_resolve(
            &state,
            run_id("gather"),
            0,
            "2026-01-02".to_string(),
            batch(),
            &positions,
        )
        .await;
        std::env::remove_var(CONFLICT_ENV);
        let got = ids(&out.decisions);
        match env {
            Some("paper_enforced") => {
                // Bundle 6 arbitrates (zero or one AAPL survivor, plan recorded);
                // the fail-closed withholding is not what decided it.
                let aapl = got.iter().filter(|(s, _)| s == "AAPL").count();
                assert!(
                    aapl <= 1,
                    "enforced Bundle 6 keeps at most one AAPL: {got:?}"
                );
                assert!(got.iter().any(|(s, _)| s == "MSFT"));
                assert!(
                    out.plan.is_some(),
                    "enforced mode must produce a conflict plan"
                );
                assert!(out.unarbitrated_refusals.is_empty());
            }
            _ => {
                let want: Vec<(String, String)> = expected
                    .into_iter()
                    .map(|(s, i)| (s.to_string(), i.to_string()))
                    .collect();
                assert_eq!(got, want, "mode {env:?}: competing AAPL must be withheld");
                assert_eq!(out.unarbitrated_refusals.len(), 1, "mode {env:?}");
                assert_eq!(out.unarbitrated_refusals[0].symbol, "AAPL");
            }
        }
    }
}
