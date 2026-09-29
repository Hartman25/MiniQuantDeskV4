//! D1 (V4-M5-M8-FINAL-INDEPENDENT-REVIEW-CORRECTION-02): dedicated
//! options-lifecycle provider model, evidence-based correlation, and a real
//! production caller.
//!
//! | Test | Claim                                                                    |
//! |------|--------------------------------------------------------------------------|
//! | M01  | OPEXC/OPASN/OPEXP/OPTRD all ingest through the real cycle; each         |
//! |      | lifecycle row gets a state row; same-id pairs coexist and correlate      |
//! | M02  | Different ids correlate when (and only when) an explicit provider group  |
//! |      | id proves it                                                              |
//! | M03  | Two indistinguishable candidates stay PENDING_AMBIGUOUS (never first-wins)|
//! | M04  | No trade yet = PENDING_EVIDENCE; the trade arriving later promotes it     |
//! | M05  | An OCC contract is never confused with its underlying (both directions)   |
//! | M06  | Account A never consumes account B's evidence                             |
//! | M07  | A restart resumes from the account-scoped cursor of EACH activity type    |
//! | M08  | Provider correlation fields + raw record persist verbatim                 |
//! | M09  | The production seam (poll tick / spawned task / main wiring) invokes      |
//! |      | ingestion                                                                 |
//! | M10  | The database itself refuses identity edits and unproven applied states    |
//!
//! DB-backed (port 5434 test Postgres); every fetcher is a mock -- no broker
//! call. Each test uses a fresh provider account so runs never interact.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use mqk_broker_alpaca::types::AlpacaOptionLifecycleActivity;
use mqk_daemon::state::option_lifecycle_cycle::{
    run_option_lifecycle_cycle, OPTION_LIFECYCLE_ENGINE_ID,
};
use mqk_daemon::state::OptionLifecycleActivityFetcher;
use mqk_db::option_lifecycle_activity::{
    fetch_option_lifecycle_activity, list_option_lifecycle_activities, OptionLifecycleActivityType,
};
use mqk_db::{
    fetch_option_lifecycle_event_state, BrokerAccountAuthority, CorrelationBasis,
    LifecycleEventState,
};
use sqlx::PgPool;
use uuid::Uuid;

const CALL: &str = "AAPL230721C00150000";
const PUT: &str = "AAPL230721P00150000";
const CALL_160: &str = "AAPL230721C00160000";
const DATE: &str = "2023-07-21";

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
        &format!("d1-{label}-{}", Uuid::new_v4().simple()),
        "paper",
    )
    .unwrap()
}

#[allow(clippy::too_many_arguments)]
fn act(
    id: &str,
    ty: &str,
    symbol: &str,
    qty: &str,
    price: Option<&str>,
    net: &str,
    group: Option<&str>,
) -> AlpacaOptionLifecycleActivity {
    AlpacaOptionLifecycleActivity {
        id: id.to_string(),
        activity_type: ty.to_string(),
        date: Some(DATE.to_string()),
        net_amount: net.to_string(),
        description: Some(format!("{ty} test")),
        symbol: Some(symbol.to_string()),
        qty: Some(qty.to_string()),
        price: price.map(str::to_string),
        status: Some("executed".to_string()),
        group_id: group.map(str::to_string),
        ref_id: group.map(|g| format!("ref-{g}")),
    }
}

/// Mock provider transport: serves `rows` per requested activity type,
/// honouring `after_id` exactly like the real cursor semantics (strictly
/// after), and records every call.
struct MockFetcher {
    authority: BrokerAccountAuthority,
    rows: Mutex<Vec<AlpacaOptionLifecycleActivity>>,
    calls: Mutex<Vec<(String, Option<String>)>>,
}

impl MockFetcher {
    fn new(
        authority: BrokerAccountAuthority,
        rows: Vec<AlpacaOptionLifecycleActivity>,
    ) -> Arc<Self> {
        Arc::new(Self {
            authority,
            rows: Mutex::new(rows),
            calls: Mutex::new(Vec::new()),
        })
    }
    fn set_rows(&self, rows: Vec<AlpacaOptionLifecycleActivity>) {
        *self.rows.lock().unwrap() = rows;
    }
    fn after_ids_for(&self, ty: &str) -> Vec<Option<String>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(t, _)| t == ty)
            .map(|(_, a)| a.clone())
            .collect()
    }
}

impl OptionLifecycleActivityFetcher for MockFetcher {
    fn fetch_option_lifecycle_activities_since(
        &self,
        activity_type: &str,
        after_id: Option<&str>,
    ) -> Result<Vec<AlpacaOptionLifecycleActivity>, String> {
        self.calls
            .lock()
            .unwrap()
            .push((activity_type.to_string(), after_id.map(str::to_string)));
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

async fn cycle(
    pool: &PgPool,
    f: &MockFetcher,
) -> anyhow::Result<mqk_daemon::state::option_lifecycle_cycle::LifecycleCycleReport> {
    run_option_lifecycle_cycle(pool, f, OPTION_LIFECYCLE_ENGINE_ID, Utc::now()).await
}

async fn state_of(
    pool: &PgPool,
    key: &str,
    id: &str,
    ty: OptionLifecycleActivityType,
) -> mqk_db::OptionLifecycleEventStateRow {
    fetch_option_lifecycle_event_state(pool, key, id, ty)
        .await
        .unwrap()
        .unwrap_or_else(|| panic!("no state row for {id}"))
}

#[tokio::test]
async fn m01_all_four_types_ingest_and_correlate_same_id_pairs_coexist() {
    use OptionLifecycleActivityType::*;
    let pool = require_pool().await;
    let auth = fresh_authority("m01");
    let key = auth.key();
    let f = MockFetcher::new(
        auth,
        vec![
            act("X1", "OPEXC", CALL, "-2", None, "0", None),
            act("X1", "OPTRD", "AAPL", "200", Some("150"), "-30000", None),
            act("Y1", "OPASN", PUT, "2", None, "0", None),
            act("Y1", "OPTRD", "AAPL", "200", Some("150"), "-30000", None),
            act("Z1", "OPEXP", CALL_160, "-1", None, "0", None),
        ],
    );
    let report = cycle(&pool, &f).await.expect("cycle");
    assert_eq!(report.ingested["OPEXC"], (1, 0));
    assert_eq!(report.ingested["OPASN"], (1, 0));
    assert_eq!(report.ingested["OPEXP"], (1, 0));
    // Two OPTRD rows: X1 and Y1 -- distinct ids, both stored.
    assert_eq!(report.ingested["OPTRD"], (2, 0));
    assert_eq!(report.ready, 3);

    // The same id coexists under two activity types.
    assert!(fetch_option_lifecycle_activity(&pool, &key, "X1", Exercise)
        .await
        .unwrap()
        .is_some());
    assert!(
        fetch_option_lifecycle_activity(&pool, &key, "X1", PairedTrade)
            .await
            .unwrap()
            .is_some()
    );

    let x = state_of(&pool, &key, "X1", Exercise).await;
    assert_eq!(x.state, LifecycleEventState::ReadyToApply);
    assert_eq!(x.correlation_basis, Some(CorrelationBasis::SameActivityId));
    assert_eq!(x.correlated_optrd_activity_id.as_deref(), Some("X1"));
    assert_eq!(x.underlying_symbol.as_deref(), Some("AAPL"));
    assert_eq!(x.option_symbol, CALL);
    let y = state_of(&pool, &key, "Y1", Assignment).await;
    assert_eq!(y.state, LifecycleEventState::ReadyToApply);
    let z = state_of(&pool, &key, "Z1", Expiration).await;
    assert_eq!(z.state, LifecycleEventState::ReadyToApply);
    assert_eq!(z.correlation_basis, Some(CorrelationBasis::NotRequired));
    assert_eq!(z.correlated_optrd_activity_id, None);
}

#[tokio::test]
async fn m02_different_ids_correlate_only_through_an_explicit_group_id() {
    use OptionLifecycleActivityType::*;
    let pool = require_pool().await;
    let auth = fresh_authority("m02");
    let key = auth.key();
    let f = MockFetcher::new(
        auth,
        vec![
            act("L1", "OPEXC", CALL, "-2", None, "0", Some("GRP-1")),
            act(
                "T9",
                "OPTRD",
                "AAPL",
                "200",
                Some("150"),
                "-30000",
                Some("GRP-1"),
            ),
        ],
    );
    cycle(&pool, &f).await.expect("cycle");
    let l = state_of(&pool, &key, "L1", Exercise).await;
    assert_eq!(l.state, LifecycleEventState::ReadyToApply);
    assert_eq!(l.correlation_basis, Some(CorrelationBasis::ExplicitGroupId));
    assert_eq!(l.correlated_optrd_activity_id.as_deref(), Some("T9"));
}

#[tokio::test]
async fn m03_indistinguishable_candidates_stay_ambiguous_never_first_wins() {
    use OptionLifecycleActivityType::*;
    let pool = require_pool().await;
    let auth = fresh_authority("m03");
    let key = auth.key();
    let f = MockFetcher::new(
        auth,
        vec![
            act("L1", "OPEXC", CALL, "-2", None, "0", None),
            act("T1", "OPTRD", "AAPL", "200", Some("150"), "-30000", None),
            act("T2", "OPTRD", "AAPL", "200", Some("150"), "-30000", None),
        ],
    );
    let report = cycle(&pool, &f).await.expect("cycle");
    assert_eq!(report.pending_ambiguous, 1);
    assert_eq!(report.ready, 0);
    let l = state_of(&pool, &key, "L1", Exercise).await;
    assert_eq!(l.state, LifecycleEventState::PendingAmbiguous);
    assert_eq!(
        l.correlated_optrd_activity_id, None,
        "no candidate is chosen"
    );
}

#[tokio::test]
async fn m04_missing_trade_is_pending_evidence_then_its_arrival_promotes() {
    use OptionLifecycleActivityType::*;
    let pool = require_pool().await;
    let auth = fresh_authority("m04");
    let key = auth.key();
    let f = MockFetcher::new(auth, vec![act("L1", "OPEXC", CALL, "-2", None, "0", None)]);
    cycle(&pool, &f).await.expect("cycle 1");
    assert_eq!(
        state_of(&pool, &key, "L1", Exercise).await.state,
        LifecycleEventState::PendingEvidence
    );

    f.set_rows(vec![
        act("L1", "OPEXC", CALL, "-2", None, "0", None),
        act("L1", "OPTRD", "AAPL", "200", Some("150"), "-30000", None),
    ]);
    cycle(&pool, &f).await.expect("cycle 2");
    assert_eq!(
        state_of(&pool, &key, "L1", Exercise).await.state,
        LifecycleEventState::ReadyToApply
    );
}

#[tokio::test]
async fn m05_option_contract_is_never_confused_with_its_underlying() {
    let pool = require_pool().await;
    // Underlying ticker on a lifecycle row -> whole batch refused.
    let auth = fresh_authority("m05a");
    let key = auth.key();
    let f = MockFetcher::new(
        auth,
        vec![act("L1", "OPEXC", "AAPL", "-2", None, "0", None)],
    );
    let err = cycle(&pool, &f)
        .await
        .expect_err("underlying on OPEXC must refuse");
    assert!(
        err.to_string()
            .contains("not a standard OCC option contract"),
        "{err}"
    );
    assert!(
        list_option_lifecycle_activities(&pool, &key, OptionLifecycleActivityType::Exercise)
            .await
            .unwrap()
            .is_empty()
    );

    // Option contract on an OPTRD row -> refused.
    let auth = fresh_authority("m05b");
    let key = auth.key();
    let f = MockFetcher::new(
        auth,
        vec![act("T1", "OPTRD", CALL, "200", Some("150"), "-30000", None)],
    );
    let err = cycle(&pool, &f)
        .await
        .expect_err("contract on OPTRD must refuse");
    assert!(err.to_string().contains("is an option contract"), "{err}");
    assert!(list_option_lifecycle_activities(
        &pool,
        &key,
        OptionLifecycleActivityType::PairedTrade
    )
    .await
    .unwrap()
    .is_empty());
}

#[tokio::test]
async fn m06_account_a_never_consumes_account_b_evidence() {
    use OptionLifecycleActivityType::*;
    let pool = require_pool().await;
    let a = fresh_authority("m06a");
    let b = fresh_authority("m06b");
    let (key_a, key_b) = (a.key(), b.key());
    // A has the lifecycle event; only B has the matching trade.
    let fa = MockFetcher::new(a, vec![act("L1", "OPEXC", CALL, "-2", None, "0", None)]);
    let fb = MockFetcher::new(
        b,
        vec![act(
            "L1",
            "OPTRD",
            "AAPL",
            "200",
            Some("150"),
            "-30000",
            None,
        )],
    );
    cycle(&pool, &fb).await.expect("account B cycle");
    cycle(&pool, &fa).await.expect("account A cycle");
    assert_eq!(
        state_of(&pool, &key_a, "L1", Exercise).await.state,
        LifecycleEventState::PendingEvidence,
        "account A must not see account B's same-id trade"
    );
    assert!(
        list_option_lifecycle_activities(&pool, &key_b, PairedTrade)
            .await
            .unwrap()
            .len()
            == 1
    );
    assert!(list_option_lifecycle_activities(&pool, &key_a, PairedTrade)
        .await
        .unwrap()
        .is_empty());
}

#[tokio::test]
async fn m07_restart_resumes_from_the_account_scoped_cursor_of_each_type() {
    let pool = require_pool().await;
    let auth = fresh_authority("m07");
    let rows = vec![
        act("E1", "OPEXP", CALL, "-1", None, "0", None),
        act("E2", "OPEXP", CALL_160, "-1", None, "0", None),
    ];
    let first = MockFetcher::new(auth.clone(), rows.clone());
    cycle(&pool, &first).await.expect("first process");
    assert_eq!(first.after_ids_for("OPEXP"), vec![None]);

    // A brand-new process (fresh mock, same durable DB).
    let second = MockFetcher::new(auth, rows);
    let report = cycle(&pool, &second).await.expect("second process");
    assert_eq!(
        second.after_ids_for("OPEXP"),
        vec![Some("E2".to_string())],
        "the persisted account-scoped cursor is threaded as after_id"
    );
    assert_eq!(
        report.ingested["OPEXP"],
        (0, 0),
        "nothing new past the cursor"
    );
    // Types with no rows never gained a cursor.
    assert_eq!(second.after_ids_for("OPEXC"), vec![None]);
}

#[tokio::test]
async fn m08_provider_correlation_fields_and_raw_record_persist_verbatim() {
    use OptionLifecycleActivityType::*;
    let pool = require_pool().await;
    let auth = fresh_authority("m08");
    let key = auth.key();
    let f = MockFetcher::new(
        auth,
        vec![act("L1", "OPEXC", CALL, "-2", None, "0", Some("GRP-9"))],
    );
    cycle(&pool, &f).await.expect("cycle");
    let row = fetch_option_lifecycle_activity(&pool, &key, "L1", Exercise)
        .await
        .unwrap()
        .expect("row");
    assert_eq!(row.provenance.group_id.as_deref(), Some("GRP-9"));
    assert_eq!(row.provenance.ref_id.as_deref(), Some("ref-GRP-9"));
    assert_eq!(row.provenance.status.as_deref(), Some("executed"));
    assert_eq!(row.provenance.description.as_deref(), Some("OPEXC test"));
    let raw = row.provenance.raw_json.expect("raw json persisted");
    assert_eq!(raw["id"], "L1");
    assert_eq!(raw["symbol"], CALL);
    assert_eq!(raw["group_id"], "GRP-9");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn m09_the_production_seam_invokes_ingestion() {
    use mqk_daemon::state::option_lifecycle_poll::{
        run_poll_tick, spawn_option_lifecycle_poll_task_with_interval,
    };
    use OptionLifecycleActivityType::*;
    let pool = require_pool().await;
    let auth = fresh_authority("m09");
    let key = auth.key();
    let f = MockFetcher::new(auth, vec![act("E1", "OPEXP", CALL, "-1", None, "0", None)]);

    let mut st = mqk_daemon::state::AppState::new_with_db_and_operator_auth(
        pool.clone(),
        mqk_daemon::state::OperatorAuthMode::ExplicitDevNoToken,
    );
    st.set_option_lifecycle_activity_fetcher_for_test(f.clone());
    let st = Arc::new(st);

    // (a) one poll tick == one cycle against the configured pool + fetcher.
    let report = run_poll_tick(&st).await.expect("tick");
    assert_eq!(report.ingested["OPEXP"], (1, 0));
    assert!(
        fetch_option_lifecycle_activity(&pool, &key, "E1", Expiration)
            .await
            .unwrap()
            .is_some()
    );

    // (b) the spawned task actually ticks on its own.
    f.set_rows(vec![
        act("E1", "OPEXP", CALL, "-1", None, "0", None),
        act("E2", "OPEXP", CALL_160, "-1", None, "0", None),
    ]);
    let handle = spawn_option_lifecycle_poll_task_with_interval(
        Arc::clone(&st),
        std::time::Duration::from_millis(50),
    )
    .expect("task must start with a DB and a fetcher");
    let mut seen = false;
    for _ in 0..80 {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        if fetch_option_lifecycle_activity(&pool, &key, "E2", Expiration)
            .await
            .unwrap()
            .is_some()
        {
            seen = true;
            break;
        }
    }
    handle.abort();
    assert!(seen, "M09: the spawned poll task never ingested E2");

    // (c) the daemon binary actually spawns it, and the task body runs the
    // real cycle (a zero-caller regression turns this RED).
    let main_src = include_str!("../src/main.rs");
    assert!(
        main_src.contains("state::spawn_option_lifecycle_poll_task(Arc::clone(&shared))"),
        "M09: main.rs must spawn the options-lifecycle poll task"
    );
    let poll_src = include_str!("../src/state/option_lifecycle_poll.rs");
    assert!(
        poll_src.contains("run_option_lifecycle_cycle("),
        "M09: the poll task must run the real cycle"
    );
}

#[tokio::test]
async fn m10_database_refuses_identity_edits_and_unproven_applied_states() {
    use OptionLifecycleActivityType::*;
    let pool = require_pool().await;
    let auth = fresh_authority("m10");
    let key = auth.key();
    let f = MockFetcher::new(auth, vec![act("E1", "OPEXP", CALL, "-1", None, "0", None)]);
    cycle(&pool, &f).await.expect("cycle");
    assert_eq!(
        state_of(&pool, &key, "E1", Expiration).await.state,
        LifecycleEventState::ReadyToApply
    );

    // Identity columns are immutable.
    let err = sqlx::query(
        "update sys_option_lifecycle_event_state set option_symbol = 'AAPL230721C00999000' \
         where broker_account_id = $1 and lifecycle_activity_id = 'E1'",
    )
    .bind(&key)
    .execute(&pool)
    .await
    .expect_err("identity edit must be refused");
    assert!(err.to_string().contains("immutable"), "{err}");

    // APPLIED/RECONCILED without an economic apply identity violates the CHECK.
    assert!(sqlx::query(
        "update sys_option_lifecycle_event_state set state = 'RECONCILED', \
         state_reason = 'forced' where broker_account_id = $1 and lifecycle_activity_id = 'E1'",
    )
    .bind(&key)
    .execute(&pool)
    .await
    .is_err());

    // PENDING_EVIDENCE cannot jump straight to APPLIED_AWAITING_BROKER.
    let seeded = act("E2", "OPEXP", CALL_160, "-1", None, "0", None);
    f.set_rows(vec![
        act("E1", "OPEXP", CALL, "-1", None, "0", None),
        AlpacaOptionLifecycleActivity {
            status: Some("pending".to_string()),
            ..seeded
        },
    ]);
    cycle(&pool, &f).await.expect("cycle 2");
    assert_eq!(
        state_of(&pool, &key, "E2", Expiration).await.state,
        LifecycleEventState::PendingEvidence,
        "a non-executed lifecycle row is never ready"
    );
    let err = sqlx::query(
        "update sys_option_lifecycle_event_state set state = 'APPLIED_AWAITING_BROKER', \
         economic_apply_id = 'x', underlying_symbol = 'AAPL', correlation_basis = 'not_required' \
         where broker_account_id = $1 and lifecycle_activity_id = 'E2'",
    )
    .bind(&key)
    .execute(&pool)
    .await
    .expect_err("PENDING_EVIDENCE -> APPLIED must be refused");
    assert!(err.to_string().contains("READY_TO_APPLY"), "{err}");
}
