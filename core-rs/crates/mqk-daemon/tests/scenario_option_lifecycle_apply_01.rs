//! D2 (V4-M5-M8-FINAL-INDEPENDENT-REVIEW-CORRECTION-02): atomic, signed,
//! provider-evidenced options-lifecycle economics through the real
//! production path: mock provider -> ingestion -> correlation -> atomic
//! journal apply -> canonical ledger.
//!
//! | Test | Claim                                                                   |
//! |------|-------------------------------------------------------------------------|
//! | P01  | The four required directions (long call exercise, long put exercise,   |
//! |      | short call assignment, short put assignment): option removed,            |
//! |      | underlying and cash move by exactly the provider-signed evidence in the  |
//! |      | canonical ledger                                                          |
//! | P02  | Expiration removes the option and invents no delivery and no cash         |
//! | P03  | Ambiguous pairing stays pending: nothing is applied                       |
//! | P04  | Another account's apply pass cannot apply this account's event            |
//! | P05  | An exact retry is a no-op: ONE journal row, state unchanged               |
//! | P06  | A READY event whose evidence overflows/is inexact is demoted to pending   |
//! |      | and never applied (checked arithmetic, no unwrap-or-zero)                 |
//! | P07  | Run recovery replays the journal into the ledger with the same economics  |
//! |      | and reports the watermark; replaying twice is refused by the ledger       |
//!
//! DB-backed (port 5434 test Postgres); mock provider only. Each test uses a
//! unique deployment-mode label and account so the journal replay is isolated.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use mqk_broker_alpaca::types::AlpacaOptionLifecycleActivity;
use mqk_daemon::state::option_lifecycle_apply::apply_ready_lifecycle_events;
use mqk_daemon::state::option_lifecycle_cycle::{
    run_option_lifecycle_cycle, OPTION_LIFECYCLE_ENGINE_ID,
};
use mqk_daemon::state::option_lifecycle_ledger::{
    journal_entry_to_ledger_adjustment, replay_lifecycle_journal_into_portfolio,
};
use mqk_daemon::state::OptionLifecycleActivityFetcher;
use mqk_db::option_lifecycle_activity::OptionLifecycleActivityType;
use mqk_db::{
    fetch_lifecycle_journal_entry, fetch_option_lifecycle_event_state, BrokerAccountAuthority,
    LifecycleEventState,
};
use mqk_portfolio::{apply_entry, Fill, LedgerEntry, PortfolioState, QtyMicros, Side};
use sqlx::PgPool;
use uuid::Uuid;

const DOMAIN: &str = "equity_nyse";
const CALL: &str = "AAPL230721C00150000";
const PUT: &str = "AAPL230721P00150000";
const START_CASH: i64 = 1_000_000_000_000;
const PREMIUM: i64 = 5_000_000;

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

/// A fresh account under a unique deployment-mode label (journal replay is
/// mode-scoped, so parallel tests never see each other's entries).
fn fresh_authority(label: &str) -> BrokerAccountAuthority {
    let u = Uuid::new_v4().simple().to_string();
    BrokerAccountAuthority::new("alpaca", &format!("p-{label}-{u}"), &format!("m{u}")).unwrap()
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

impl MockFetcher {
    fn new(
        authority: BrokerAccountAuthority,
        rows: Vec<AlpacaOptionLifecycleActivity>,
    ) -> Arc<Self> {
        Arc::new(Self {
            authority,
            rows: Mutex::new(rows),
        })
    }
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

async fn ingest_and_correlate(pool: &PgPool, f: &MockFetcher) {
    run_option_lifecycle_cycle(pool, f, OPTION_LIFECYCLE_ENGINE_ID, Utc::now())
        .await
        .expect("cycle");
}

fn units(n: i64) -> QtyMicros {
    QtyMicros::from_whole_units(n).unwrap()
}

fn holding(symbol: &str, contracts: i64) -> PortfolioState {
    let mut pf = PortfolioState::new(START_CASH);
    let side = if contracts > 0 { Side::Buy } else { Side::Sell };
    apply_entry(
        &mut pf,
        LedgerEntry::Fill(Fill::new(symbol, side, units(contracts.abs()), PREMIUM, 0)),
    );
    pf
}

fn qty(pf: &PortfolioState, symbol: &str) -> QtyMicros {
    pf.positions
        .get(symbol)
        .map(|p| p.qty_signed())
        .unwrap_or(QtyMicros::ZERO)
}

#[tokio::test]
async fn p01_the_four_directions_move_option_underlying_and_cash_by_the_provider_evidence() {
    use OptionLifecycleActivityType::{Assignment, Exercise};
    // (label, type, wire type, contract, held contracts (signed), lifecycle qty,
    //  OPTRD shares, OPTRD net cash, expected underlying, cash delta)
    let cases = [
        (
            "long-call-exercise",
            Exercise,
            "OPEXC",
            CALL,
            2,
            "-2",
            "200",
            "-30000",
            200,
            -30_000_000_000i64,
        ),
        (
            "long-put-exercise",
            Exercise,
            "OPEXC",
            PUT,
            2,
            "-2",
            "-200",
            "30000",
            -200,
            30_000_000_000,
        ),
        (
            "short-call-assignment",
            Assignment,
            "OPASN",
            CALL,
            -2,
            "2",
            "-200",
            "30000",
            -200,
            30_000_000_000,
        ),
        (
            "short-put-assignment",
            Assignment,
            "OPASN",
            PUT,
            -2,
            "2",
            "200",
            "-30000",
            200,
            -30_000_000_000,
        ),
    ];
    for (label, ty, wire, contract, held, lqty, tqty, net, expect_shares, cash_delta) in cases {
        let pool = require_pool().await;
        let auth = fresh_authority(label);
        let key = auth.key();
        let f = MockFetcher::new(
            auth,
            vec![
                act("X1", wire, contract, lqty, None, "0"),
                act("X1", "OPTRD", "AAPL", tqty, Some("150"), net),
            ],
        );
        ingest_and_correlate(&pool, &f).await;
        let report = apply_ready_lifecycle_events(&pool, &key, Utc::now())
            .await
            .expect("apply");
        assert_eq!(report.applied, 1, "{label}");

        let st = fetch_option_lifecycle_event_state(&pool, &key, "X1", ty)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            st.state,
            LifecycleEventState::AppliedAwaitingBroker,
            "{label}"
        );
        let entry = fetch_lifecycle_journal_entry(&pool, st.economic_apply_id.as_deref().unwrap())
            .await
            .unwrap()
            .expect("journal row");
        // Signed evidence exactly as the provider reported it.
        assert_eq!(
            entry.option_qty_delta_micros,
            lqty.parse::<i64>().unwrap() * 1_000_000
        );
        assert_eq!(
            entry.underlying_qty_delta_micros,
            Some(expect_shares * 1_000_000)
        );
        assert_eq!(entry.cash_delta_micros, Some(cash_delta));

        // Through the canonical ledger.
        let mut pf = holding(contract, held);
        let cash_before = pf.cash_micros;
        apply_entry(
            &mut pf,
            LedgerEntry::LifecycleAdjustment(journal_entry_to_ledger_adjustment(&entry).unwrap()),
        );
        assert_eq!(
            qty(&pf, contract),
            QtyMicros::ZERO,
            "{label}: option removed"
        );
        assert_eq!(
            qty(&pf, "AAPL"),
            units(expect_shares),
            "{label}: underlying"
        );
        assert_eq!(pf.cash_micros, cash_before + cash_delta, "{label}: cash");
        // Never a synthetic fill: the only fill is the original premium fill.
        assert_eq!(
            pf.ledger
                .iter()
                .filter(|e| matches!(e, LedgerEntry::Fill(_)))
                .count(),
            1,
            "{label}"
        );
    }
}

#[tokio::test]
async fn p02_expiration_removes_the_option_and_invents_nothing() {
    use OptionLifecycleActivityType::Expiration;
    let pool = require_pool().await;
    let auth = fresh_authority("p02");
    let key = auth.key();
    let f = MockFetcher::new(auth, vec![act("E1", "OPEXP", CALL, "-3", None, "0")]);
    ingest_and_correlate(&pool, &f).await;
    let report = apply_ready_lifecycle_events(&pool, &key, Utc::now())
        .await
        .unwrap();
    assert_eq!(report.applied, 1);
    let st = fetch_option_lifecycle_event_state(&pool, &key, "E1", Expiration)
        .await
        .unwrap()
        .unwrap();
    let entry = fetch_lifecycle_journal_entry(&pool, st.economic_apply_id.as_deref().unwrap())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(entry.option_qty_delta_micros, -3_000_000);
    assert_eq!(
        (
            entry.underlying_qty_delta_micros,
            entry.cash_delta_micros,
            entry.optrd_activity_id.clone()
        ),
        (None, None, None)
    );
    let mut pf = holding(CALL, 3);
    let cash_before = pf.cash_micros;
    apply_entry(
        &mut pf,
        LedgerEntry::LifecycleAdjustment(journal_entry_to_ledger_adjustment(&entry).unwrap()),
    );
    assert_eq!(qty(&pf, CALL), QtyMicros::ZERO);
    assert!(!pf.positions.contains_key("AAPL"));
    assert_eq!(pf.cash_micros, cash_before);
}

#[tokio::test]
async fn p03_ambiguous_pairing_stays_pending_and_nothing_is_applied() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let auth = fresh_authority("p03");
    let key = auth.key();
    let f = MockFetcher::new(
        auth,
        vec![
            act("L1", "OPEXC", CALL, "-2", None, "0"),
            act("T1", "OPTRD", "AAPL", "200", Some("150"), "-30000"),
            act("T2", "OPTRD", "AAPL", "200", Some("150"), "-30000"),
        ],
    );
    ingest_and_correlate(&pool, &f).await;
    let report = apply_ready_lifecycle_events(&pool, &key, Utc::now())
        .await
        .unwrap();
    assert_eq!(report.applied, 0);
    let st = fetch_option_lifecycle_event_state(&pool, &key, "L1", Exercise)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(st.state, LifecycleEventState::PendingAmbiguous);
    assert!(st.economic_apply_id.is_none());
}

#[tokio::test]
async fn p04_another_accounts_apply_pass_cannot_apply_this_accounts_event() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let a = fresh_authority("p04a");
    let b = fresh_authority("p04b");
    let (key_a, key_b) = (a.key(), b.key());
    let fa = MockFetcher::new(
        a,
        vec![
            act("X1", "OPEXC", CALL, "-2", None, "0"),
            act("X1", "OPTRD", "AAPL", "200", Some("150"), "-30000"),
        ],
    );
    let fb = MockFetcher::new(b, vec![]);
    ingest_and_correlate(&pool, &fa).await;
    ingest_and_correlate(&pool, &fb).await;

    let b_report = apply_ready_lifecycle_events(&pool, &key_b, Utc::now())
        .await
        .unwrap();
    assert_eq!(
        b_report.applied, 0,
        "B has nothing and cannot touch A's event"
    );
    assert_eq!(
        fetch_option_lifecycle_event_state(&pool, &key_a, "X1", Exercise)
            .await
            .unwrap()
            .unwrap()
            .state,
        LifecycleEventState::ReadyToApply
    );
    let a_report = apply_ready_lifecycle_events(&pool, &key_a, Utc::now())
        .await
        .unwrap();
    assert_eq!(a_report.applied, 1);
}

#[tokio::test]
async fn p05_an_exact_retry_is_a_noop() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let auth = fresh_authority("p05");
    let key = auth.key();
    let f = MockFetcher::new(
        auth,
        vec![
            act("X1", "OPEXC", CALL, "-2", None, "0"),
            act("X1", "OPTRD", "AAPL", "200", Some("150"), "-30000"),
        ],
    );
    ingest_and_correlate(&pool, &f).await;
    assert_eq!(
        apply_ready_lifecycle_events(&pool, &key, Utc::now())
            .await
            .unwrap()
            .applied,
        1
    );
    let first_state = fetch_option_lifecycle_event_state(&pool, &key, "X1", Exercise)
        .await
        .unwrap()
        .unwrap();

    // A full retry cycle (fresh ingestion + correlation + apply): zero effect.
    ingest_and_correlate(&pool, &f).await;
    let again = apply_ready_lifecycle_events(&pool, &key, Utc::now())
        .await
        .unwrap();
    assert_eq!(again.applied, 0);
    let second_state = fetch_option_lifecycle_event_state(&pool, &key, "X1", Exercise)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        second_state.state,
        LifecycleEventState::AppliedAwaitingBroker
    );
    assert_eq!(
        second_state.economic_apply_id,
        first_state.economic_apply_id
    );
    let n: (i64,) = sqlx::query_as(
        "select count(*) from sys_option_lifecycle_adjustment_journal where broker_account_id = $1",
    )
    .bind(&key)
    .fetch_one(&pool)
    .await
    .unwrap();
    assert_eq!(n.0, 1, "exactly one economic application");
}

#[tokio::test]
async fn p06_inexact_or_overflowing_evidence_is_demoted_and_never_applied() {
    use mqk_db::option_lifecycle_activity::{
        insert_option_lifecycle_activity_if_new, NewOptionLifecycleActivity,
        OptionLifecycleRawProvenance,
    };
    use mqk_db::{
        record_lifecycle_evaluation, CorrelationBasis, LifecycleEvaluation, LifecycleStateSeed,
    };
    use OptionLifecycleActivityType::{Exercise, PairedTrade};
    let pool = require_pool().await;
    let auth = fresh_authority("p06");
    let key = auth.key();
    mqk_db::verify_or_register_broker_account_authority(&pool, &auth, Utc::now())
        .await
        .unwrap();
    let base = |id: &str,
                ty,
                symbol: Option<&str>,
                underlying: Option<&str>,
                qty: &str,
                price: Option<&str>,
                net: &str| {
        NewOptionLifecycleActivity {
            activity_id: id.to_string(),
            broker_account_id: key.clone(),
            engine_id: "mqk-daemon".to_string(),
            mode: auth.deployment_mode().to_string(),
            activity_type: ty,
            option_symbol: symbol.map(str::to_string),
            underlying_symbol_raw: underlying.map(str::to_string),
            activity_date: "2023-07-21".to_string(),
            qty_raw: qty.to_string(),
            price_raw: price.map(str::to_string),
            net_amount_raw: net.to_string(),
            ingested_at_utc: Utc::now(),
            provenance: OptionLifecycleRawProvenance {
                status: Some("executed".to_string()),
                ..Default::default()
            },
            state_seed: (ty != PairedTrade).then(|| LifecycleStateSeed {
                execution_domain: DOMAIN.to_string(),
                underlying_symbol: Some("AAPL".to_string()),
            }),
        }
    };
    insert_option_lifecycle_activity_if_new(
        &pool,
        &base("X1", Exercise, Some(CALL), None, "-2", None, "0"),
    )
    .await
    .unwrap();
    // The trade carries a cash value no fixed-point i64 can represent.
    insert_option_lifecycle_activity_if_new(
        &pool,
        &base(
            "X1",
            PairedTrade,
            None,
            Some("AAPL"),
            "200",
            Some("150"),
            "-99999999999999999999",
        ),
    )
    .await
    .unwrap();
    // Force READY as a (buggy) upstream might; apply must re-verify.
    record_lifecycle_evaluation(
        &pool,
        &key,
        "X1",
        Exercise,
        &LifecycleEvaluation {
            state: LifecycleEventState::ReadyToApply,
            reason: "forced".to_string(),
            underlying_symbol: Some("AAPL".to_string()),
            correlated_optrd_activity_id: Some("X1".to_string()),
            correlation_basis: Some(CorrelationBasis::SameActivityId),
        },
        Utc::now(),
    )
    .await
    .unwrap();

    let report = apply_ready_lifecycle_events(&pool, &key, Utc::now())
        .await
        .unwrap();
    assert_eq!(report.applied, 0);
    assert_eq!(report.demoted_to_pending, 1);
    let st = fetch_option_lifecycle_event_state(&pool, &key, "X1", Exercise)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(st.state, LifecycleEventState::PendingEvidence);
    assert!(
        st.state_reason.starts_with("apply_refused:"),
        "{}",
        st.state_reason
    );
    assert!(fetch_lifecycle_journal_entry(
        &pool,
        &mqk_db::economic_apply_id(&key, DOMAIN, "X1", Exercise)
    )
    .await
    .unwrap()
    .is_none());
}

#[tokio::test]
async fn p07_recovery_replays_the_journal_into_the_ledger_with_the_same_economics() {
    let pool = require_pool().await;
    let auth = fresh_authority("p07");
    let key = auth.key();
    let mode = auth.deployment_mode().to_string();
    let f = MockFetcher::new(
        auth.clone(),
        vec![
            act("X1", "OPEXC", CALL, "-2", None, "0"),
            act("X1", "OPTRD", "AAPL", "200", Some("150"), "-30000"),
        ],
    );
    ingest_and_correlate(&pool, &f).await;
    apply_ready_lifecycle_events(&pool, &key, Utc::now())
        .await
        .unwrap();

    let mut recovered = holding(CALL, 2);
    let cash_before = recovered.cash_micros;
    let replay = replay_lifecycle_journal_into_portfolio(&pool, Some(&auth), &mode, &mut recovered)
        .await
        .expect("replay");
    assert_eq!(replay.applied_ids.len(), 1);
    assert_eq!(qty(&recovered, CALL), QtyMicros::ZERO);
    assert_eq!(qty(&recovered, "AAPL"), units(200));
    assert_eq!(recovered.cash_micros, cash_before - 30_000_000_000);

    // The ledger refuses a second application of the same entry.
    let entries = mqk_db::list_unsubsumed_lifecycle_journal(&pool, DOMAIN, &auth)
        .await
        .unwrap();
    let mut ledger = mqk_portfolio::Ledger::new(START_CASH);
    ledger
        .append_fill(Fill::new(CALL, Side::Buy, units(2), PREMIUM, 0))
        .unwrap();
    let adj = journal_entry_to_ledger_adjustment(&entries[0]).unwrap();
    ledger.append_lifecycle_adjustment(adj.clone()).unwrap();
    assert!(matches!(
        ledger.append_lifecycle_adjustment(adj),
        Err(mqk_portfolio::LedgerError::DuplicateLifecycleAdjustment { .. })
    ));
}
