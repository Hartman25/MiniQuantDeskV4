//! D3 (V4-M5-M8-FINAL-INDEPENDENT-REVIEW-CORRECTION-02): the lifecycle state
//! machine gates every economic entry seam, and only broker agreement clears
//! it.
//!
//! | Test | Claim                                                                    |
//! |------|--------------------------------------------------------------------------|
//! | R01  | Broker agreement (fresh snapshot, ledger absorbed, option + underlying   |
//! |      | equal) moves APPLIED_AWAITING_BROKER -> RECONCILED; each missing         |
//! |      | condition leaves the event fenced                                        |
//! | R02  | Until then the underlying (not just the option) stays fenced, even when  |
//! |      | the option has vanished from the broker snapshot                         |
//! | R03  | The external strategy-signal route is fenced by the lifecycle gate       |
//! | R04  | An OCC contract with no asset_class is never implied to be Equity        |
//! | R05  | The route consults the shared gate (wiring proof) and unrelated symbols  |
//! |      | pass the fence                                                            |
//!
//! DB-backed (port 5434 test Postgres); mock provider only.

use std::collections::{BTreeMap, BTreeSet};
use std::sync::{Arc, Mutex};

use axum::http::{Request, StatusCode};
use chrono::{Duration, Utc};
use http_body_util::BodyExt;
use mqk_broker_alpaca::types::AlpacaOptionLifecycleActivity;
use mqk_daemon::state::option_lifecycle_apply::apply_ready_lifecycle_events;
use mqk_daemon::state::option_lifecycle_cycle::{
    run_option_lifecycle_cycle, OPTION_LIFECYCLE_ENGINE_ID,
};
use mqk_daemon::state::option_lifecycle_pending_gate::evaluate_option_lifecycle_pending_gate;
use mqk_daemon::state::option_lifecycle_reconcile::reconcile_awaiting_lifecycle_events;
use mqk_daemon::state::OptionLifecycleActivityFetcher;
use mqk_daemon::{routes, state};
use mqk_db::option_lifecycle_activity::OptionLifecycleActivityType;
use mqk_db::{fetch_option_lifecycle_event_state, BrokerAccountAuthority, LifecycleEventState};
use mqk_portfolio::QtyMicros;
use sqlx::PgPool;
use tower::ServiceExt;
use uuid::Uuid;

const CALL: &str = "AAPL230721C00150000";

async fn require_pool() -> PgPool {
    let url = match std::env::var(mqk_db::ENV_DB_URL) {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!("PROOF: MQK_DATABASE_URL is not set; load-bearing proof cannot be skipped"),
    };
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(8)
        .acquire_timeout(std::time::Duration::from_secs(5))
        .connect(&url)
        .await
        .expect("connect");
    mqk_db::migrate(&pool).await.expect("migrate");
    pool
}

/// Fresh provider account under a unique deployment-mode label.
fn fresh_authority(label: &str) -> BrokerAccountAuthority {
    let u = Uuid::new_v4().simple().to_string();
    BrokerAccountAuthority::new("alpaca", &format!("d3-{label}-{u}"), &format!("m{u}")).unwrap()
}

fn act(
    id: &str,
    ty: &str,
    symbol: &str,
    qty: &str,
    price: Option<&str>,
    net: &str,
) -> AlpacaOptionLifecycleActivity {
    AlpacaOptionLifecycleActivity {
        id: id.to_string(),
        activity_type: ty.to_string(),
        date: Some("2023-07-21".to_string()),
        net_amount: net.to_string(),
        description: None,
        symbol: Some(symbol.to_string()),
        qty: Some(qty.to_string()),
        price: price.map(str::to_string),
        status: Some("executed".to_string()),
        group_id: None,
        ref_id: None,
    }
}

struct MockFetcher {
    authority: BrokerAccountAuthority,
    rows: Mutex<Vec<AlpacaOptionLifecycleActivity>>,
}

impl OptionLifecycleActivityFetcher for MockFetcher {
    fn fetch_option_lifecycle_activities_since(
        &self,
        activity_type: &str,
        after_id: Option<&str>,
    ) -> Result<Vec<AlpacaOptionLifecycleActivity>, String> {
        let of_type: Vec<_> = self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.activity_type == activity_type)
            .cloned()
            .collect();
        Ok(match after_id {
            None => of_type,
            Some(a) => match of_type.iter().position(|r| r.id == a) {
                Some(i) => of_type[i + 1..].to_vec(),
                None => of_type,
            },
        })
    }

    fn broker_account_authority(&self) -> Result<BrokerAccountAuthority, String> {
        Ok(self.authority.clone())
    }
}

fn mock(
    authority: BrokerAccountAuthority,
    rows: Vec<AlpacaOptionLifecycleActivity>,
) -> Arc<MockFetcher> {
    Arc::new(MockFetcher {
        authority,
        rows: Mutex::new(rows),
    })
}

fn units(n: i64) -> QtyMicros {
    QtyMicros::from_whole_units(n).unwrap()
}

fn book(entries: &[(&str, i64)]) -> BTreeMap<String, QtyMicros> {
    entries
        .iter()
        .map(|(s, n)| (s.to_string(), units(*n)))
        .collect()
}

/// Exercise (long call) ingested, correlated and applied: APPLIED_AWAITING_BROKER.
/// Returns the account key and the entry's `economic_apply_id`.
async fn applied_exercise(pool: &PgPool, f: &MockFetcher) -> (String, String) {
    run_option_lifecycle_cycle(pool, f, OPTION_LIFECYCLE_ENGINE_ID, Utc::now())
        .await
        .expect("cycle");
    let key = f.authority.key();
    assert_eq!(
        apply_ready_lifecycle_events(pool, &key, Utc::now())
            .await
            .unwrap()
            .applied,
        1
    );
    let st =
        fetch_option_lifecycle_event_state(pool, &key, "X1", OptionLifecycleActivityType::Exercise)
            .await
            .unwrap()
            .unwrap();
    let entry =
        mqk_db::fetch_lifecycle_journal_entry(pool, st.economic_apply_id.as_deref().unwrap())
            .await
            .unwrap()
            .unwrap();
    (key, entry.economic_apply_id)
}

fn exercise_rows() -> Vec<AlpacaOptionLifecycleActivity> {
    vec![
        act("X1", "OPEXC", CALL, "-2", None, "0"),
        act("X1", "OPTRD", "AAPL", "200", Some("150"), "-30000"),
    ]
}

#[tokio::test]
async fn r01_only_a_fresh_agreeing_snapshot_over_an_absorbed_ledger_reconciles() {
    let pool = require_pool().await;
    let auth = fresh_authority("r01");
    let f = mock(auth.clone(), exercise_rows());
    let (key, apply_id) = applied_exercise(&pool, &f).await;

    let agree = book(&[("AAPL", 200)]);
    let fresh = Utc::now() + Duration::seconds(5);
    let stale = Utc::now() - Duration::seconds(60);

    let absorbed: BTreeSet<String> = [apply_id.clone()].into_iter().collect();
    let not_absorbed: BTreeSet<String> = BTreeSet::new();
    // The exercise's provider-signed cash (-150 x 200), as the ledger applied it.
    let cash_ok: BTreeMap<String, i64> = [(apply_id.clone(), -30_000_000_000)].into();
    let cash_off: BTreeMap<String, i64> = [(apply_id.clone(), -30_000_000_001)].into();
    let cash_unapplied: BTreeMap<String, i64> = BTreeMap::new();

    // Ledger has not absorbed the entry.
    let r = reconcile_awaiting_lifecycle_events(
        &pool,
        &auth,
        &not_absorbed,
        &cash_ok,
        &agree,
        &agree,
        fresh,
        Utc::now(),
    )
    .await
    .unwrap();
    assert_eq!((r.reconciled, r.waiting_for_absorb), (0, 1));

    // Broker snapshot older than the apply proves nothing.
    let r = reconcile_awaiting_lifecycle_events(
        &pool,
        &auth,
        &absorbed,
        &cash_ok,
        &agree,
        &agree,
        stale,
        Utc::now(),
    )
    .await
    .unwrap();
    assert_eq!((r.reconciled, r.waiting_for_fresh_snapshot), (0, 1));

    // Broker still holds the option: disagreement.
    let broker_holds = book(&[("AAPL", 200), (CALL, 2)]);
    let r = reconcile_awaiting_lifecycle_events(
        &pool,
        &auth,
        &absorbed,
        &cash_ok,
        &agree,
        &broker_holds,
        fresh,
        Utc::now(),
    )
    .await
    .unwrap();
    assert_eq!((r.reconciled, r.disagreeing), (0, 1));

    // Underlying disagrees.
    let r = reconcile_awaiting_lifecycle_events(
        &pool,
        &auth,
        &absorbed,
        &cash_ok,
        &agree,
        &book(&[("AAPL", 100)]),
        fresh,
        Utc::now(),
    )
    .await
    .unwrap();
    assert_eq!((r.reconciled, r.disagreeing), (0, 1));

    // Option and underlying AGREE with the broker, but the cash the local
    // ledger applied does not match the provider-evidenced settlement cash (or
    // is absent): positions alone never clear the gate.
    for wrong in [&cash_off, &cash_unapplied] {
        let r = reconcile_awaiting_lifecycle_events(
            &pool,
            &auth,
            &absorbed,
            wrong,
            &agree,
            &agree,
            fresh,
            Utc::now(),
        )
        .await
        .unwrap();
        assert_eq!((r.reconciled, r.cash_disagreeing), (0, 1));
        assert_eq!(
            fetch_option_lifecycle_event_state(
                &pool,
                &key,
                "X1",
                OptionLifecycleActivityType::Exercise,
            )
            .await
            .unwrap()
            .unwrap()
            .state,
            LifecycleEventState::AppliedAwaitingBroker
        );
    }

    // Still fenced after every refusal.
    assert!(evaluate_option_lifecycle_pending_gate(&pool, &key, "AAPL")
        .await
        .unwrap()
        .must_fail_closed());

    // Fresh, agreeing, absorbed: RECONCILED, gate clears.
    let r = reconcile_awaiting_lifecycle_events(
        &pool,
        &auth,
        &absorbed,
        &cash_ok,
        &agree,
        &agree,
        fresh,
        Utc::now(),
    )
    .await
    .unwrap();
    assert_eq!(r.reconciled, 1);
    let st = fetch_option_lifecycle_event_state(
        &pool,
        &key,
        "X1",
        OptionLifecycleActivityType::Exercise,
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(st.state, LifecycleEventState::Reconciled);
    assert!(!evaluate_option_lifecycle_pending_gate(&pool, &key, "AAPL")
        .await
        .unwrap()
        .must_fail_closed());
    assert!(!evaluate_option_lifecycle_pending_gate(&pool, &key, CALL)
        .await
        .unwrap()
        .must_fail_closed());
}

#[tokio::test]
async fn r02_the_underlying_stays_fenced_even_when_the_option_vanished_from_the_snapshot() {
    let pool = require_pool().await;
    let auth = fresh_authority("r02");
    let f = mock(auth, exercise_rows());
    let (key, _apply_id) = applied_exercise(&pool, &f).await;
    // Broker snapshot without the option and with (mutated) underlying: the
    // option-symbol-only gate of the old design would have seen nothing.
    // The state row fences the underlying independently.
    let gate = evaluate_option_lifecycle_pending_gate(&pool, &key, "AAPL")
        .await
        .unwrap();
    assert!(
        gate.must_fail_closed(),
        "underlying fenced while APPLIED_AWAITING_BROKER"
    );
    let option = evaluate_option_lifecycle_pending_gate(&pool, &key, CALL)
        .await
        .unwrap();
    assert!(option.must_fail_closed());
}

// ---------------------------------------------------------------------------
// External strategy-signal route
// ---------------------------------------------------------------------------

async fn post_signal(
    router: axum::Router,
    body: serde_json::Value,
) -> (StatusCode, serde_json::Value) {
    let req = Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(body.to_string()))
        .unwrap();
    let resp = router.oneshot(req).await.expect("oneshot");
    let status = resp.status();
    let bytes = resp.into_body().collect().await.expect("body").to_bytes();
    (status, serde_json::from_slice(&bytes).expect("json"))
}

fn signal(symbol: &str) -> serde_json::Value {
    serde_json::json!({
        "signal_id": format!("d3-{}", Uuid::new_v4().simple()),
        "strategy_id": "d3-strategy",
        "symbol": symbol,
        "side": "buy",
        "qty": 1
    })
}

fn blockers(json: &serde_json::Value) -> String {
    json["blockers"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| b.as_str())
                .collect::<Vec<_>>()
                .join(" | ")
        })
        .unwrap_or_default()
}

fn paper_alpaca_state_with_fetcher(
    pool: PgPool,
    fetcher: Arc<dyn OptionLifecycleActivityFetcher>,
) -> Arc<state::AppState> {
    let mut st = state::AppState::new_for_test_with_db_mode_and_broker(
        pool,
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    );
    st.set_option_lifecycle_activity_fetcher_for_test(fetcher);
    Arc::new(st)
}

#[tokio::test]
async fn r03_the_external_signal_route_is_fenced_by_the_lifecycle_gate() {
    let pool = require_pool().await;
    let auth = fresh_authority("r03");
    let f = mock(auth, vec![act("L1", "OPEXC", CALL, "-2", None, "0")]);
    run_option_lifecycle_cycle(&pool, &*f, OPTION_LIFECYCLE_ENGINE_ID, Utc::now())
        .await
        .expect("cycle");

    let st = paper_alpaca_state_with_fetcher(pool.clone(), f.clone());
    let router = routes::build_router(st);

    // The underlying is fenced (PENDING_EVIDENCE).
    let (status, json) = post_signal(router.clone(), signal("AAPL")).await;
    assert_eq!(status, StatusCode::CONFLICT, "{json}");
    assert_eq!(json["disposition"], "rejected", "{json}");
    assert!(blockers(&json).contains("options-lifecycle"), "{json}");

    // An unrelated symbol passes the lifecycle fence (it may be refused by a
    // LATER gate, but never for the lifecycle reason).
    let (_status, json) = post_signal(router, signal("MSFT")).await;
    assert!(
        !blockers(&json).contains("options-lifecycle"),
        "MSFT must not be fenced by an AAPL option event: {json}"
    );
}

#[tokio::test]
async fn r04_an_occ_contract_without_asset_class_is_never_implied_to_be_equity() {
    let st = Arc::new(state::AppState::new());
    let router = routes::build_router(st);

    let (status, json) = post_signal(router.clone(), signal(CALL)).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert_eq!(json["disposition"], "rejected");
    assert!(
        blockers(&json).contains("OCC option contract"),
        "explicit refusal for an option-looking symbol: {json}"
    );

    // A plain equity ticker without asset_class keeps legacy behavior (Gate 0
    // passes; a later gate refuses on the default state).
    let (_s, json) = post_signal(router.clone(), signal("AAPL")).await;
    assert!(!blockers(&json).contains("OCC option contract"), "{json}");

    // Explicit `option` stays disabled.
    let mut body = signal(CALL);
    body["asset_class"] = serde_json::json!("option");
    let (status, json) = post_signal(router, body).await;
    assert_eq!(status, StatusCode::BAD_REQUEST, "{json}");
    assert!(blockers(&json).contains("not supported"), "{json}");
}

#[tokio::test]
async fn r05_the_route_consults_the_shared_lifecycle_fence() {
    let src = include_str!("../src/routes/strategy.rs");
    assert!(
        src.contains("check_symbol_fence("),
        "the external signal route must consult the shared lifecycle fence"
    );
    let decision = include_str!("../src/decision.rs");
    assert!(
        decision.contains("check_symbol_fence("),
        "internal decision path"
    );
    let manual = include_str!("../src/routes/execution.rs");
    assert!(manual.contains("check_symbol_fence("), "manual order route");
    let repair = include_str!("../src/routes/repair.rs");
    assert!(
        repair.contains("check_account_fence("),
        "baseline adoption fence"
    );
}
