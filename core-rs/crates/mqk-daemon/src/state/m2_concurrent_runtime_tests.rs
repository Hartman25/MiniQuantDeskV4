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

use super::autonomous_completed_bar_driver::deposit_and_confirm_binding_evaluation;
use super::capital_fraction_dispatch_tests::db_or_skip;
use super::*;
use crate::decision::InternalStrategyDecision;
use crate::dynamic_selection_dispatch_authority::SelectedDispatchBinding;
use crate::dynamic_selection_host_pool::DynamicSelectionHostPool;
use crate::runtime_opportunity_allocation::PendingDecisionWithBarFacts;
use crate::runtime_strategy_conflict::{
    gather_and_resolve, refuse_unarbitrated_competition, UnarbitratedRefusal,
};
use mqk_schemas::QtyMicros;
use sqlx::PgPool;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use std::time::Duration;

const SCALPER: &str = "intraday_scalper";
const SHORT_SCALPER: &str = "intraday_short_scalper";
const TF_5M: &str = "5m";
const FIVE_MIN: i64 = 300;

fn run_id(tag: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_DNS, format!("m2rt.run.{tag}").as_bytes())
}

fn binding(symbol: &str, strategy_id: &str) -> SelectedDispatchBinding {
    SelectedDispatchBinding {
        symbol: symbol.to_string(),
        strategy_id: strategy_id.to_string(),
        timeframe_secs: FIVE_MIN,
        db_timeframe_label: TF_5M.to_string(),
        selection_reason_code: "test_fixture".to_string(),
        plan_id: Uuid::new_v5(&Uuid::NAMESPACE_DNS, b"m2rt.plan"),
    }
}

fn keys(bindings: &[SelectedDispatchBinding]) -> Vec<(String, String, i64)> {
    bindings
        .iter()
        .map(|b| (b.symbol.clone(), b.strategy_id.clone(), b.timeframe_secs))
        .collect()
}

fn paper_state_with_db(pool: &PgPool) -> Arc<AppState> {
    let mut state =
        AppState::new_for_test_with_mode_and_broker(DeploymentMode::Paper, BrokerKind::Paper);
    state.db = Some(pool.clone());
    Arc::new(state)
}

/// Seed one completed 5m bar.
async fn seed_5m_one(pool: &PgPool, symbol: &str, end_ts: i64, close: i64) {
    sqlx::query(
        r#"insert into md_bars (symbol, timeframe, end_ts, open_micros, high_micros,
           low_micros, close_micros, volume, is_complete, provider_id, provider_source,
           provider_symbol, ingest_mode, ingested_at)
           values ($1,'5m',$2,$3,$3,$3,$3,1000,true,'m2rt_test','m2rt_test',$1,
                   'historical_sync',now())
           on conflict do nothing"#,
    )
    .bind(symbol)
    .bind(end_ts)
    .bind(close)
    .execute(pool)
    .await
    .expect("seed bar");
}

/// Seed `closes` as consecutive 5m completed bars ending at `last_end_ts`.
async fn seed_5m(pool: &PgPool, symbol: &str, last_end_ts: i64, closes: &[i64]) {
    let n = closes.len() as i64;
    for (i, close) in closes.iter().enumerate() {
        seed_5m_one(
            pool,
            symbol,
            last_end_ts - (n - 1 - i as i64) * FIVE_MIN,
            *close,
        )
        .await;
    }
}

async fn cleanup(pool: &PgPool, symbols: &[&str]) {
    for s in symbols {
        let _ = sqlx::query("delete from md_bars where symbol = $1")
            .bind(s)
            .execute(pool)
            .await;
        let _ = sqlx::query("delete from strategy_signal_evaluations where symbol = $1")
            .bind(s)
            .execute(pool)
            .await;
    }
}

/// Four flat bars then a +50bps bar: a genuine `intraday_scalper` buy.
const BUY_SPIKE: [i64; 5] = [
    100_000_000,
    100_000_000,
    100_000_000,
    100_000_000,
    100_500_000,
];

fn trigger(ts: i64) -> StrategyBarInput {
    StrategyBarInput {
        now_tick: ts as u64,
        end_ts: ts,
        limit_price: None,
        qty: 1,
    }
}

fn host_calls(state: &AppState) -> Vec<String> {
    state
        .loop_call_trace_snapshot_for_test()
        .into_iter()
        .filter(|e| e.starts_with("host_call:"))
        .collect()
}

// ---------------------------------------------------------------------------
// Replay / double-consumption: the driver's confirm step must leave nothing
// pending, so the execution loop evaluates a claimed bar exactly once.
// ---------------------------------------------------------------------------

/// Producer = the driver's real `deposit_and_confirm_binding_evaluation`;
/// consumer = the real selected-host tick, run every 100 ms. If the confirm
/// step leaves a redundant deposit behind, the next consumer tick evaluates
/// the already-claimed bar a second time, outside any dispatch claim.
#[tokio::test]
async fn confirm_leaves_no_pending_deposit_and_the_claimed_bar_is_evaluated_once() {
    let Some(pool) = db_or_skip("M2RT-CONFIRM-01").await else {
        return;
    };
    let sym = "M2RTCONF1";
    cleanup(&pool, &[sym]).await;
    let ts = Utc::now().timestamp() - 60;
    seed_5m(&pool, sym, ts, &BUY_SPIKE).await;

    let b = binding(sym, SCALPER);
    let mut host_pool = DynamicSelectionHostPool::build(&keys(&[b.clone()])).expect("pool");
    let bindings = vec![b];
    let rid = run_id("confirm");
    let state = paper_state_with_db(&pool);
    state.loop_call_trace_clear_for_test();

    let expected =
        AppState::derive_strategy_signal_evaluation_id(Some(rid), SCALPER, sym, TF_5M, ts as u64);
    let stop = AtomicBool::new(false);
    let producer = async {
        let confirmed = deposit_and_confirm_binding_evaluation(
            &state,
            &pool,
            rid,
            sym,
            SCALPER,
            TF_5M,
            ts,
            ts as u64,
            expected,
            8,
            Duration::from_millis(250),
        )
        .await
        .expect("confirm step");
        // Several further consumer ticks: anything left pending is consumed.
        tokio::time::sleep(Duration::from_millis(900)).await;
        stop.store(true, AtomicOrdering::SeqCst);
        confirmed
    };
    let consumer = async {
        while !stop.load(AtomicOrdering::SeqCst) {
            state
                .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                    rid,
                    &bindings,
                    &mut host_pool,
                )
                .await
                .expect("selected-host tick");
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    let (confirmed, ()) = tokio::join!(producer, consumer);

    let calls = host_calls(&state);
    let pending = state
        .pending_binding_strategy_bar_input_count_for_test()
        .await;
    cleanup(&pool, &[sym]).await;

    assert!(confirmed, "the loop's evaluation row must be confirmed");
    assert_eq!(pending, 0, "confirm must leave no pending deposit");
    assert_eq!(
        calls.len(),
        1,
        "the claimed bar must be evaluated exactly once: {calls:?}"
    );
}

/// No consumer ever evaluates the deposit: the step reports unconfirmed and
/// must not leave its deposit behind for a later, unrelated tick to consume.
#[tokio::test]
async fn unconfirmed_deposit_is_drained_not_left_for_a_later_tick() {
    let Some(pool) = db_or_skip("M2RT-CONFIRM-02").await else {
        return;
    };
    let sym = "M2RTCONF2";
    cleanup(&pool, &[sym]).await;
    let ts = Utc::now().timestamp() - 60;
    let rid = run_id("confirm-unconfirmed");
    let state = paper_state_with_db(&pool);
    let expected =
        AppState::derive_strategy_signal_evaluation_id(Some(rid), SCALPER, sym, TF_5M, ts as u64);

    let confirmed = deposit_and_confirm_binding_evaluation(
        &state,
        &pool,
        rid,
        sym,
        SCALPER,
        TF_5M,
        ts,
        ts as u64,
        expected,
        3,
        Duration::from_millis(10),
    )
    .await
    .expect("confirm step");
    let pending = state
        .pending_binding_strategy_bar_input_count_for_test()
        .await;
    cleanup(&pool, &[sym]).await;

    assert!(!confirmed, "nothing evaluated the deposit");
    assert_eq!(pending, 0, "an unconfirmed deposit must be drained");
}

// ---------------------------------------------------------------------------
// Fan-out, multi-symbol, same-symbol distinct strategies, determinism,
// single consumption, failure attribution.
// ---------------------------------------------------------------------------

type Summary = Vec<(String, String, i64, Vec<(String, i64)>)>;

fn summarize(
    results: &[(
        SymbolStrategyAssignment,
        mqk_strategy::StrategyBarResult,
        Option<EvaluatedBarFacts>,
    )],
) -> Summary {
    let mut out: Summary = results
        .iter()
        .map(|(a, r, facts)| {
            (
                a.symbol.clone(),
                a.strategy_id.clone(),
                facts.as_ref().map_or(-1, |f| f.bar_end_ts),
                r.intents
                    .output
                    .targets
                    .iter()
                    .map(|t| (t.symbol.clone(), t.qty.raw()))
                    .collect(),
            )
        })
        .collect();
    out.sort();
    out
}

#[tokio::test]
async fn one_bar_fans_out_to_every_authorized_binding_deterministically_and_is_consumed_once() {
    let Some(pool) = db_or_skip("M2RT-FANOUT-01").await else {
        return;
    };
    let (a, b) = ("M2RTFANA", "M2RTFANB");
    cleanup(&pool, &[a, b]).await;
    let ts = Utc::now().timestamp() - 60;
    seed_5m(&pool, a, ts, &BUY_SPIKE).await;
    seed_5m(&pool, b, ts, &BUY_SPIKE).await;

    // Two strategies on `a`, one on `b`.
    let forward = vec![
        binding(a, SCALPER),
        binding(a, SHORT_SCALPER),
        binding(b, SCALPER),
    ];
    let reversed: Vec<_> = forward.iter().rev().cloned().collect();

    let mut summaries = Vec::new();
    for (tag, order) in [("fwd", &forward), ("rev", &reversed)] {
        let mut host_pool = DynamicSelectionHostPool::build(&keys(order)).expect("pool");
        let state = paper_state_with_db(&pool);
        state.deposit_strategy_bar_input(trigger(ts)).await;
        let results = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                run_id(tag),
                order,
                &mut host_pool,
            )
            .await
            .expect("tick");
        // Output follows the authorized binding order (deterministic).
        let order_seen: Vec<_> = results
            .iter()
            .map(|(asg, _, _)| (asg.symbol.clone(), asg.strategy_id.clone()))
            .collect();
        let order_expected: Vec<_> = order
            .iter()
            .map(|x| (x.symbol.clone(), x.strategy_id.clone()))
            .collect();
        assert_eq!(
            order_seen, order_expected,
            "dispatch order follows bindings"
        );

        // The shared trigger is consumed once: a second tick evaluates nothing.
        let again = state
            .tick_strategy_dispatch_selected_hosts_with_bar_facts(
                run_id(tag),
                order,
                &mut host_pool,
            )
            .await
            .expect("second tick");
        assert!(again.is_empty(), "a consumed bar must not be re-evaluated");
        summaries.push(summarize(&results));
    }
    cleanup(&pool, &[a, b]).await;

    let fwd = &summaries[0];
    assert_eq!(fwd.len(), 3, "exactly the three authorized bindings ran");
    assert_eq!(fwd, &summaries[1], "result is independent of binding order");
    let buy = |sym: &str, sid: &str| {
        fwd.iter()
            .find(|r| r.0 == sym && r.1 == sid)
            .map(|r| r.3.iter().any(|(_, q)| *q > 0))
    };
    assert_eq!(buy(a, SCALPER), Some(true));
    assert_eq!(buy(b, SCALPER), Some(true));
    assert_eq!(
        buy(a, SHORT_SCALPER),
        Some(false),
        "the short variant must not inherit its sibling's signal"
    );
}

#[tokio::test]
async fn unusable_bindings_yield_no_result_are_attributable_and_do_not_affect_siblings() {
    let Some(pool) = db_or_skip("M2RT-ISOLATE-01").await else {
        return;
    };
    let (ok, bare, stale) = ("M2RTISOOK", "M2RTISOBARE", "M2RTISOSTALE");
    cleanup(&pool, &[ok, bare, stale]).await;
    let ts = Utc::now().timestamp() - 60;
    seed_5m(&pool, ok, ts, &BUY_SPIKE).await;
    // A buy-shaped window that is ten days old: stale, must never trade.
    seed_5m(&pool, stale, ts - 10 * 86_400, &BUY_SPIKE).await;

    let bindings = vec![
        binding(bare, SCALPER),
        binding(stale, SCALPER),
        binding(ok, SCALPER),
    ];
    let mut host_pool = DynamicSelectionHostPool::build(&keys(&bindings)).expect("pool");
    let state = paper_state_with_db(&pool);
    state.deposit_strategy_bar_input(trigger(ts)).await;
    let results = state
        .tick_strategy_dispatch_selected_hosts_with_bar_facts(
            run_id("isolate"),
            &bindings,
            &mut host_pool,
        )
        .await
        .expect("unusable bindings are skipped, not a tick fault");

    let rows = mqk_db::fetch_recent_strategy_signal_evaluations(&pool, 200)
        .await
        .expect("evaluations");
    let row_for = |sym: &str| rows.iter().find(|r| r.symbol == sym).cloned();
    let (bare_row, stale_row, ok_row) = (row_for(bare), row_for(stale), row_for(ok));
    cleanup(&pool, &[ok, bare, stale]).await;

    assert_eq!(
        results.len(),
        1,
        "no result may be manufactured: {results:?}"
    );
    assert_eq!(results[0].0.symbol, ok);
    assert!(results[0]
        .1
        .intents
        .output
        .targets
        .iter()
        .any(|t| t.qty.raw() > 0));
    let bare_row = bare_row.expect("a binding without bars must be durably attributable");
    let stale_row = stale_row.expect("a stale binding must be durably attributable");
    for row in [&bare_row, &stale_row] {
        assert!(!row.signal_generated);
        assert_eq!(row.decision_stage, "pre_dispatch_gate");
    }
    assert_eq!(
        bare_row.reason_code,
        crate::market_data_freshness::REASON_CODE_INTRADAY_BAR_NOT_CURRENT
    );
    assert_eq!(
        stale_row.reason_code,
        crate::market_data_freshness::REASON_CODE_INTRADAY_BAR_STALE
    );
    assert!(ok_row.is_some_and(|r| r.decision_stage != "pre_dispatch_gate"));
}

/// Bars inserted newest-first must not change which bar is evaluated: the
/// evaluated bar is the latest by `end_ts`, and the signal is computed over
/// the chronologically ordered window.
#[tokio::test]
async fn bar_insertion_order_cannot_change_the_evaluated_bar_or_signal() {
    let Some(pool) = db_or_skip("M2RT-ORDER-01").await else {
        return;
    };
    let sym = "M2RTORDER1";
    cleanup(&pool, &[sym]).await;
    let ts = Utc::now().timestamp() - 60;
    let n = BUY_SPIKE.len() as i64;
    for (i, close) in BUY_SPIKE.iter().enumerate().rev() {
        seed_5m_one(&pool, sym, ts - (n - 1 - i as i64) * FIVE_MIN, *close).await;
    }

    let bindings = vec![binding(sym, SCALPER)];
    let mut host_pool = DynamicSelectionHostPool::build(&keys(&bindings)).expect("pool");
    let state = paper_state_with_db(&pool);
    state.deposit_strategy_bar_input(trigger(ts)).await;
    let results = state
        .tick_strategy_dispatch_selected_hosts_with_bar_facts(
            run_id("order"),
            &bindings,
            &mut host_pool,
        )
        .await
        .expect("tick");
    cleanup(&pool, &[sym]).await;

    assert_eq!(results.len(), 1);
    assert_eq!(results[0].2.as_ref().map(|f| f.bar_end_ts), Some(ts));
    assert!(results[0]
        .1
        .intents
        .output
        .targets
        .iter()
        .any(|t| t.qty.raw() > 0));
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
