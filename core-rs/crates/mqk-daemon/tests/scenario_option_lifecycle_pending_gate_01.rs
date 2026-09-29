//! D3 (V4-M5-M8-FINAL-INDEPENDENT-REVIEW-CORRECTION-02): the options-lifecycle
//! pending gate over the persisted state machine.
//!
//! | Test | Claim                                                                     |
//! |------|---------------------------------------------------------------------------|
//! | G01  | The gate is CLOSED at every state except RECONCILED -- in particular at   |
//! |      | APPLIED_AWAITING_BROKER: an applied marker alone never clears it          |
//! | G02  | Both the option contract AND its underlying are fenced; an unrelated       |
//! |      | symbol, another account and another execution domain are not               |
//! | G03  | Only RECONCILED clears the gate                                            |
//! | G04  | The state is durable: a fresh connection (restart) sees the same fence     |
//! | G05  | The whole-account fence covers an option that vanished from a snapshot     |
//!
//! DB-backed (port 5434 test Postgres); mock provider only; fresh account per
//! test.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use mqk_broker_alpaca::types::AlpacaOptionLifecycleActivity;
use mqk_daemon::state::option_lifecycle_apply::apply_ready_lifecycle_events;
use mqk_daemon::state::option_lifecycle_cycle::{
    run_option_lifecycle_cycle, OPTION_LIFECYCLE_ENGINE_ID,
};
use mqk_daemon::state::option_lifecycle_pending_gate::{
    check_account_fence, check_symbol_fence, evaluate_option_lifecycle_pending_gate,
    GateCheckError, OptionLifecycleGateStatus,
};
use mqk_daemon::state::OptionLifecycleActivityFetcher;
use mqk_db::option_lifecycle_activity::OptionLifecycleActivityType;
use mqk_db::{mark_lifecycle_event_reconciled, BrokerAccountAuthority, LifecycleEventState};
use sqlx::PgPool;
use uuid::Uuid;

const CALL: &str = "AAPL230721C00150000";
const SPY_CALL: &str = "SPY230721C00450000";

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

fn fresh_authority(label: &str) -> BrokerAccountAuthority {
    BrokerAccountAuthority::new(
        "alpaca",
        &format!("g-{label}-{}", Uuid::new_v4().simple()),
        "paper",
    )
    .unwrap()
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

async fn cycle(pool: &PgPool, f: &MockFetcher) {
    run_option_lifecycle_cycle(pool, f, OPTION_LIFECYCLE_ENGINE_ID, Utc::now())
        .await
        .expect("cycle");
}

async fn gate(pool: &PgPool, key: &str, symbol: &str) -> OptionLifecycleGateStatus {
    evaluate_option_lifecycle_pending_gate(pool, key, symbol)
        .await
        .expect("gate")
}

fn blocked_state(s: &OptionLifecycleGateStatus) -> Option<LifecycleEventState> {
    match s {
        OptionLifecycleGateStatus::Pending { state, .. } => Some(*state),
        OptionLifecycleGateStatus::Clear => None,
    }
}

#[tokio::test]
async fn g01_closed_at_every_state_until_reconciled_including_applied() {
    let pool = require_pool().await;
    let auth = fresh_authority("g01");
    let key = auth.key();

    // PENDING_EVIDENCE: lifecycle row ingested, no trade yet.
    let f = mock(
        auth.clone(),
        vec![act("L1", "OPEXC", CALL, "-2", None, "0")],
    );
    cycle(&pool, &f).await;
    assert_eq!(
        blocked_state(&gate(&pool, &key, CALL).await),
        Some(LifecycleEventState::PendingEvidence)
    );

    // READY_TO_APPLY.
    *f.rows.lock().unwrap() = vec![
        act("L1", "OPEXC", CALL, "-2", None, "0"),
        act("L1", "OPTRD", "AAPL", "200", Some("150"), "-30000"),
    ];
    cycle(&pool, &f).await;
    assert_eq!(
        blocked_state(&gate(&pool, &key, CALL).await),
        Some(LifecycleEventState::ReadyToApply)
    );

    // APPLIED_AWAITING_BROKER: the economic adjustment is durably applied and
    // the applied marker exists -- the gate must STILL be closed.
    assert_eq!(
        apply_ready_lifecycle_events(&pool, &key, Utc::now())
            .await
            .unwrap()
            .applied,
        1
    );
    assert_eq!(
        blocked_state(&gate(&pool, &key, CALL).await),
        Some(LifecycleEventState::AppliedAwaitingBroker),
        "G01: an applied marker alone must never clear the gate"
    );
    assert_eq!(
        blocked_state(&gate(&pool, &key, "AAPL").await),
        Some(LifecycleEventState::AppliedAwaitingBroker),
        "G01: the underlying stays fenced too"
    );
}

#[tokio::test]
async fn g01b_pending_ambiguous_keeps_the_gate_closed() {
    let pool = require_pool().await;
    let auth = fresh_authority("g01b");
    let key = auth.key();
    let f = mock(
        auth,
        vec![
            act("L1", "OPEXC", CALL, "-2", None, "0"),
            act("T1", "OPTRD", "AAPL", "200", Some("150"), "-30000"),
            act("T2", "OPTRD", "AAPL", "200", Some("150"), "-30000"),
        ],
    );
    cycle(&pool, &f).await;
    assert_eq!(
        blocked_state(&gate(&pool, &key, CALL).await),
        Some(LifecycleEventState::PendingAmbiguous)
    );
}

#[tokio::test]
async fn g02_option_and_underlying_are_fenced_and_nothing_else_is() {
    let pool = require_pool().await;
    let auth = fresh_authority("g02");
    let key = auth.key();
    let other = fresh_authority("g02-other");
    let f = mock(auth, vec![act("L1", "OPEXC", SPY_CALL, "-1", None, "0")]);
    cycle(&pool, &f).await;

    assert!(
        gate(&pool, &key, SPY_CALL).await.must_fail_closed(),
        "the option"
    );
    assert!(
        gate(&pool, &key, "SPY").await.must_fail_closed(),
        "its underlying"
    );
    for unrelated in ["AAPL", "MSFT", "BTC/USD", CALL] {
        assert!(
            !gate(&pool, &key, unrelated).await.must_fail_closed(),
            "{unrelated} must not be fenced by a SPY option event"
        );
    }
    assert!(
        !gate(&pool, &other.key(), "SPY").await.must_fail_closed(),
        "another account is unaffected"
    );
    // Another execution domain is unaffected.
    let crypto = mqk_db::find_fencing_lifecycle_event(&pool, &key, "crypto_24_7", "SPY")
        .await
        .unwrap();
    assert!(crypto.is_none());
}

#[tokio::test]
async fn g03_only_reconciled_clears_the_gate() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let auth = fresh_authority("g03");
    let key = auth.key();
    let f = mock(
        auth,
        vec![
            act("L1", "OPEXC", CALL, "-2", None, "0"),
            act("L1", "OPTRD", "AAPL", "200", Some("150"), "-30000"),
        ],
    );
    cycle(&pool, &f).await;
    apply_ready_lifecycle_events(&pool, &key, Utc::now())
        .await
        .unwrap();
    assert!(gate(&pool, &key, CALL).await.must_fail_closed());

    assert!(
        mark_lifecycle_event_reconciled(&pool, &key, "L1", Exercise, Utc::now())
            .await
            .unwrap()
    );
    assert_eq!(
        gate(&pool, &key, CALL).await,
        OptionLifecycleGateStatus::Clear
    );
    assert_eq!(
        gate(&pool, &key, "AAPL").await,
        OptionLifecycleGateStatus::Clear
    );
}

#[tokio::test]
async fn g04_the_fence_is_durable_across_a_restart() {
    let pool = require_pool().await;
    let auth = fresh_authority("g04");
    let key = auth.key();
    let f = mock(auth, vec![act("L1", "OPEXC", CALL, "-2", None, "0")]);
    cycle(&pool, &f).await;
    drop(pool);

    // A brand-new pool == a restarted process reading only durable state.
    let restarted = require_pool().await;
    assert!(gate(&restarted, &key, CALL).await.must_fail_closed());
    assert!(gate(&restarted, &key, "AAPL").await.must_fail_closed());
}

#[tokio::test]
async fn g05_the_account_fence_and_the_fetcher_helpers_fail_closed() {
    let pool = require_pool().await;
    let auth = fresh_authority("g05");
    let f = mock(auth, vec![act("L1", "OPEXP", CALL, "-1", None, "0")]);
    cycle(&pool, &f).await;
    let fetcher: Arc<dyn OptionLifecycleActivityFetcher> = f.clone();

    // Whole-account fence: an option that has vanished from a broker snapshot
    // cannot bypass it because ANY unreconciled event blocks adoption.
    let account = check_account_fence(&pool, Some(&fetcher)).await.unwrap();
    assert!(account.must_fail_closed());
    assert!(check_symbol_fence(&pool, Some(&fetcher), "AAPL")
        .await
        .unwrap()
        .must_fail_closed());
    assert!(!check_symbol_fence(&pool, Some(&fetcher), "MSFT")
        .await
        .unwrap()
        .must_fail_closed());

    // No fetcher = no Alpaca account = vacuously clear.
    assert_eq!(
        check_account_fence(&pool, None).await.unwrap(),
        OptionLifecycleGateStatus::Clear
    );

    // Unavailable authority is a refusal to evaluate, never a silent Clear.
    struct NoAuthority;
    impl OptionLifecycleActivityFetcher for NoAuthority {
        fn fetch_option_lifecycle_activities_since(
            &self,
            _t: &str,
            _a: Option<&str>,
        ) -> Result<Vec<AlpacaOptionLifecycleActivity>, String> {
            unreachable!()
        }
        fn broker_account_authority(&self) -> Result<BrokerAccountAuthority, String> {
            Err("account endpoint down".to_string())
        }
    }
    let broken: Arc<dyn OptionLifecycleActivityFetcher> = Arc::new(NoAuthority);
    assert!(matches!(
        check_symbol_fence(&pool, Some(&broken), "AAPL").await,
        Err(GateCheckError::AuthorityUnavailable(_))
    ));
    assert!(matches!(
        check_account_fence(&pool, Some(&broken)).await,
        Err(GateCheckError::AuthorityUnavailable(_))
    ));
}
