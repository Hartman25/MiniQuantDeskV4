//! D2: the canonical options-lifecycle adjustment journal and its one atomic
//! apply transaction (migration 0090).
//!
//! | Test | Claim                                                                   |
//! |------|-------------------------------------------------------------------------|
//! | J01  | Apply inserts the signed journal row and moves READY -> APPLIED in one  |
//! |      | tx; an exact retry is AlreadyApplied with ONE row and zero new mutation  |
//! | J02  | Signed option / underlying / cash deltas round-trip exactly              |
//! | J03  | Only READY_TO_APPLY may be applied (pending states refuse, no mutation)  |
//! | J04  | Wrong domain / unknown account refuses with zero mutation                |
//! | J05  | A derivation that disagrees with the proven state refuses                |
//! | J06  | An expiration cannot carry delivery/cash; an exercise requires them      |
//! | J07  | A settlement trade is consumed at most once: the second apply fails      |
//! |      | atomically (journal + state untouched)                                   |
//! | J08  | The journal is immutable (trigger); subsumption is set-once              |
//! | J09  | APPLIED -> RECONCILED is compare-and-set; reconciled entries subsume     |
//! | J10  | economic_apply_id is deterministic and account/domain/event scoped       |
//!
//! DB-backed (port 5434 test Postgres); fresh account per test.

use chrono::Utc;
use mqk_db::option_lifecycle_activity::{
    insert_option_lifecycle_activity_if_new, NewOptionLifecycleActivity,
    OptionLifecycleActivityType, OptionLifecycleRawProvenance,
};
use mqk_db::{
    apply_lifecycle_adjustment_tx, economic_apply_id, fetch_lifecycle_journal_entry,
    fetch_option_lifecycle_event_state, list_unsubsumed_lifecycle_journal,
    mark_lifecycle_event_reconciled, record_lifecycle_evaluation,
    subsume_reconciled_lifecycle_journal, verify_or_register_broker_account_authority,
    ApplyAdjustmentOutcome, BrokerAccountAuthority, CorrelationBasis, LifecycleEvaluation,
    LifecycleEventState, LifecycleStateSeed, NewLifecycleAdjustment, UnderlyingDelivery,
};
use sqlx::PgPool;
use uuid::Uuid;

const DOMAIN: &str = "equity_nyse";
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

async fn account(pool: &PgPool, label: &str) -> BrokerAccountAuthority {
    let a = BrokerAccountAuthority::new(
        "alpaca",
        &format!("j-{label}-{}", Uuid::new_v4().simple()),
        "paper",
    )
    .unwrap();
    verify_or_register_broker_account_authority(pool, &a, Utc::now())
        .await
        .unwrap();
    a
}

#[allow(clippy::too_many_arguments)]
fn raw(
    key: &str,
    id: &str,
    ty: OptionLifecycleActivityType,
    symbol: Option<&str>,
    underlying: Option<&str>,
    qty: &str,
    price: Option<&str>,
    net: &str,
) -> NewOptionLifecycleActivity {
    let lifecycle = ty != OptionLifecycleActivityType::PairedTrade;
    NewOptionLifecycleActivity {
        activity_id: id.to_string(),
        broker_account_id: key.to_string(),
        engine_id: "mqk-daemon".to_string(),
        mode: "paper".to_string(),
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
        state_seed: lifecycle.then(|| LifecycleStateSeed {
            execution_domain: DOMAIN.to_string(),
            underlying_symbol: Some("AAPL".to_string()),
        }),
    }
}

/// Seed one lifecycle event (and, for exercise/assignment, its trade) and move
/// it to READY_TO_APPLY exactly as the correlation cycle would.
async fn seed_ready(
    pool: &PgPool,
    key: &str,
    id: &str,
    ty: OptionLifecycleActivityType,
    optrd_id: Option<&str>,
) {
    insert_option_lifecycle_activity_if_new(
        pool,
        &raw(
            key,
            id,
            ty,
            Some(CALL),
            None,
            if ty == OptionLifecycleActivityType::Assignment {
                "2"
            } else {
                "-2"
            },
            None,
            "0",
        ),
    )
    .await
    .unwrap();
    if let Some(t) = optrd_id {
        insert_option_lifecycle_activity_if_new(
            pool,
            &raw(
                key,
                t,
                OptionLifecycleActivityType::PairedTrade,
                None,
                Some("AAPL"),
                "200",
                Some("150"),
                "-30000",
            ),
        )
        .await
        .unwrap();
    }
    assert!(record_lifecycle_evaluation(
        pool,
        key,
        id,
        ty,
        &LifecycleEvaluation {
            state: LifecycleEventState::ReadyToApply,
            reason: "test".to_string(),
            underlying_symbol: Some("AAPL".to_string()),
            correlated_optrd_activity_id: optrd_id.map(str::to_string),
            correlation_basis: Some(if optrd_id.is_some() {
                CorrelationBasis::SameActivityId
            } else {
                CorrelationBasis::NotRequired
            }),
        },
        Utc::now(),
    )
    .await
    .unwrap());
}

fn exercise_adjustment(key: &str, id: &str, optrd: &str) -> NewLifecycleAdjustment {
    NewLifecycleAdjustment {
        broker_account_id: key.to_string(),
        execution_domain: DOMAIN.to_string(),
        lifecycle_activity_id: id.to_string(),
        lifecycle_activity_type: OptionLifecycleActivityType::Exercise,
        option_symbol: CALL.to_string(),
        underlying_symbol: "AAPL".to_string(),
        option_qty_delta_micros: -2_000_000,
        underlying: Some(UnderlyingDelivery {
            qty_delta_micros: 200_000_000,
            strike_micros: 150_000_000,
            cash_delta_micros: -30_000_000_000,
            optrd_activity_id: optrd.to_string(),
        }),
        correlation_basis: CorrelationBasis::SameActivityId,
        applied_at_utc: Utc::now(),
    }
}

fn expiration_adjustment(key: &str, id: &str) -> NewLifecycleAdjustment {
    NewLifecycleAdjustment {
        broker_account_id: key.to_string(),
        execution_domain: DOMAIN.to_string(),
        lifecycle_activity_id: id.to_string(),
        lifecycle_activity_type: OptionLifecycleActivityType::Expiration,
        option_symbol: CALL.to_string(),
        underlying_symbol: "AAPL".to_string(),
        option_qty_delta_micros: -1_000_000,
        underlying: None,
        correlation_basis: CorrelationBasis::NotRequired,
        applied_at_utc: Utc::now(),
    }
}

async fn state(
    pool: &PgPool,
    key: &str,
    id: &str,
    ty: OptionLifecycleActivityType,
) -> mqk_db::OptionLifecycleEventStateRow {
    fetch_option_lifecycle_event_state(pool, key, id, ty)
        .await
        .unwrap()
        .expect("state row")
}

#[tokio::test]
async fn j01_apply_is_atomic_and_an_exact_retry_adds_nothing() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let a = account(&pool, "j01").await;
    let key = a.key();
    seed_ready(&pool, &key, "X1", Exercise, Some("X1")).await;
    let adj = exercise_adjustment(&key, "X1", "X1");

    let first = apply_lifecycle_adjustment_tx(&pool, &adj)
        .await
        .expect("apply");
    let ApplyAdjustmentOutcome::Applied(entry) = first else {
        panic!("first apply must be Applied")
    };
    let expected_id = economic_apply_id(&key, DOMAIN, "X1", Exercise);
    assert_eq!(entry.economic_apply_id, expected_id);
    let st = state(&pool, &key, "X1", Exercise).await;
    assert_eq!(st.state, LifecycleEventState::AppliedAwaitingBroker);
    assert_eq!(st.economic_apply_id.as_deref(), Some(expected_id.as_str()));

    let retry = apply_lifecycle_adjustment_tx(&pool, &adj)
        .await
        .expect("retry");
    assert_eq!(retry, ApplyAdjustmentOutcome::AlreadyApplied(entry.clone()));
    let all = list_unsubsumed_lifecycle_journal(&pool, DOMAIN, "paper")
        .await
        .unwrap();
    assert_eq!(
        all.iter().filter(|e| e.broker_account_id == key).count(),
        1,
        "a retry must never add a second economic effect"
    );
}

#[tokio::test]
async fn j02_signed_deltas_round_trip_exactly() {
    use OptionLifecycleActivityType::{Exercise, Expiration};
    let pool = require_pool().await;
    let a = account(&pool, "j02").await;
    let key = a.key();
    seed_ready(&pool, &key, "X1", Exercise, Some("X1")).await;
    seed_ready(&pool, &key, "E1", Expiration, None).await;
    let ApplyAdjustmentOutcome::Applied(x) =
        apply_lifecycle_adjustment_tx(&pool, &exercise_adjustment(&key, "X1", "X1"))
            .await
            .unwrap()
    else {
        panic!()
    };
    assert_eq!(x.option_qty_delta_micros, -2_000_000);
    assert_eq!(x.underlying_qty_delta_micros, Some(200_000_000));
    assert_eq!(x.strike_micros, Some(150_000_000));
    assert_eq!(x.cash_delta_micros, Some(-30_000_000_000));
    assert_eq!(x.optrd_activity_id.as_deref(), Some("X1"));
    let ApplyAdjustmentOutcome::Applied(e) =
        apply_lifecycle_adjustment_tx(&pool, &expiration_adjustment(&key, "E1"))
            .await
            .unwrap()
    else {
        panic!()
    };
    assert_eq!(e.option_qty_delta_micros, -1_000_000);
    assert_eq!(
        (
            e.underlying_qty_delta_micros,
            e.strike_micros,
            e.cash_delta_micros,
            e.optrd_activity_id
        ),
        (None, None, None, None),
        "an expiration invents no delivery and no cash"
    );
}

#[tokio::test]
async fn j03_only_ready_to_apply_may_be_applied() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let a = account(&pool, "j03").await;
    let key = a.key();
    // Ingested but never correlated: PENDING_EVIDENCE.
    insert_option_lifecycle_activity_if_new(
        &pool,
        &raw(&key, "X1", Exercise, Some(CALL), None, "-2", None, "0"),
    )
    .await
    .unwrap();
    let err = apply_lifecycle_adjustment_tx(&pool, &exercise_adjustment(&key, "X1", "X1"))
        .await
        .expect_err("pending must refuse");
    assert!(err.to_string().contains("READY_TO_APPLY"), "{err}");
    assert_eq!(
        state(&pool, &key, "X1", Exercise).await.state,
        LifecycleEventState::PendingEvidence
    );
    assert!(
        fetch_lifecycle_journal_entry(&pool, &economic_apply_id(&key, DOMAIN, "X1", Exercise))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn j04_wrong_domain_or_unknown_account_refuses_with_zero_mutation() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let a = account(&pool, "j04").await;
    let key = a.key();
    seed_ready(&pool, &key, "X1", Exercise, Some("X1")).await;

    let mut wrong_domain = exercise_adjustment(&key, "X1", "X1");
    wrong_domain.execution_domain = "crypto_24_7".to_string();
    assert!(apply_lifecycle_adjustment_tx(&pool, &wrong_domain)
        .await
        .is_err());

    let other = account(&pool, "j04-other").await;
    let mut wrong_account = exercise_adjustment(&other.key(), "X1", "X1");
    wrong_account.broker_account_id = other.key();
    let err = apply_lifecycle_adjustment_tx(&pool, &wrong_account)
        .await
        .expect_err("another account has no such event");
    assert!(
        err.to_string().contains("no lifecycle event state"),
        "{err}"
    );

    assert_eq!(
        state(&pool, &key, "X1", Exercise).await.state,
        LifecycleEventState::ReadyToApply,
        "the event is untouched"
    );
}

#[tokio::test]
async fn j05_a_derivation_that_disagrees_with_the_proven_state_refuses() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let a = account(&pool, "j05").await;
    let key = a.key();
    seed_ready(&pool, &key, "X1", Exercise, Some("X1")).await;

    let mut wrong_underlying = exercise_adjustment(&key, "X1", "X1");
    wrong_underlying.underlying_symbol = "MSFT".to_string();
    let mut wrong_trade = exercise_adjustment(&key, "X1", "OTHER");
    wrong_trade.underlying.as_mut().unwrap().optrd_activity_id = "OTHER".to_string();
    let mut wrong_basis = exercise_adjustment(&key, "X1", "X1");
    wrong_basis.correlation_basis = CorrelationBasis::UniqueEvidence;
    let mut wrong_symbol = exercise_adjustment(&key, "X1", "X1");
    wrong_symbol.option_symbol = "AAPL230721C00999000".to_string();
    for (label, bad) in [
        ("underlying", wrong_underlying),
        ("trade", wrong_trade),
        ("basis", wrong_basis),
        ("option symbol", wrong_symbol),
    ] {
        let err = apply_lifecycle_adjustment_tx(&pool, &bad)
            .await
            .expect_err(label);
        assert!(err.to_string().contains("does not match"), "{label}: {err}");
    }
    assert_eq!(
        state(&pool, &key, "X1", Exercise).await.state,
        LifecycleEventState::ReadyToApply
    );
}

#[tokio::test]
async fn j06_shape_rules_expiration_carries_nothing_exercise_requires_evidence() {
    use OptionLifecycleActivityType::{Exercise, Expiration};
    let pool = require_pool().await;
    let a = account(&pool, "j06").await;
    let key = a.key();
    seed_ready(&pool, &key, "E1", Expiration, None).await;
    seed_ready(&pool, &key, "X1", Exercise, Some("X1")).await;

    let mut exp_with_delivery = expiration_adjustment(&key, "E1");
    exp_with_delivery.underlying = exercise_adjustment(&key, "X1", "X1").underlying;
    assert!(apply_lifecycle_adjustment_tx(&pool, &exp_with_delivery)
        .await
        .is_err());

    let mut ex_without = exercise_adjustment(&key, "X1", "X1");
    ex_without.underlying = None;
    assert!(apply_lifecycle_adjustment_tx(&pool, &ex_without)
        .await
        .is_err());

    let mut zero_option_delta = expiration_adjustment(&key, "E1");
    zero_option_delta.option_qty_delta_micros = 0;
    assert!(apply_lifecycle_adjustment_tx(&pool, &zero_option_delta)
        .await
        .is_err());

    for (id, ty) in [("E1", Expiration), ("X1", Exercise)] {
        assert_eq!(
            state(&pool, &key, id, ty).await.state,
            LifecycleEventState::ReadyToApply
        );
    }
}

#[tokio::test]
async fn j07_a_settlement_trade_is_consumed_once_and_the_loser_rolls_back_atomically() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let a = account(&pool, "j07").await;
    let key = a.key();
    seed_ready(&pool, &key, "X1", Exercise, Some("T1")).await;
    // A second event whose (mis)correlation claims the SAME trade.
    seed_ready(&pool, &key, "X2", Exercise, Some("T1")).await;

    apply_lifecycle_adjustment_tx(&pool, &exercise_adjustment(&key, "X1", "T1"))
        .await
        .expect("first consumer wins");
    let err = apply_lifecycle_adjustment_tx(&pool, &exercise_adjustment(&key, "X2", "T1"))
        .await
        .expect_err("second consumer must fail");
    assert!(err.to_string().contains("journal insert failed"), "{err}");
    assert_eq!(
        state(&pool, &key, "X2", Exercise).await.state,
        LifecycleEventState::ReadyToApply,
        "the loser's state transition rolled back with its journal insert"
    );
    assert!(
        fetch_lifecycle_journal_entry(&pool, &economic_apply_id(&key, DOMAIN, "X2", Exercise))
            .await
            .unwrap()
            .is_none()
    );
}

#[tokio::test]
async fn j08_the_journal_is_immutable_and_subsumption_is_set_once() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let a = account(&pool, "j08").await;
    let key = a.key();
    seed_ready(&pool, &key, "X1", Exercise, Some("X1")).await;
    apply_lifecycle_adjustment_tx(&pool, &exercise_adjustment(&key, "X1", "X1"))
        .await
        .unwrap();
    let apply_id = economic_apply_id(&key, DOMAIN, "X1", Exercise);

    let err = sqlx::query(
        "update sys_option_lifecycle_adjustment_journal set cash_delta_micros = 0 \
         where economic_apply_id = $1",
    )
    .bind(&apply_id)
    .execute(&pool)
    .await
    .expect_err("economic fields are immutable");
    assert!(err.to_string().contains("immutable"), "{err}");
    let err = sqlx::query(
        "delete from sys_option_lifecycle_adjustment_journal where economic_apply_id = $1",
    )
    .bind(&apply_id)
    .execute(&pool)
    .await
    .expect_err("journal rows cannot be deleted");
    assert!(err.to_string().contains("immutable"), "{err}");

    // Subsumption: settable once, never re-set or cleared.
    sqlx::query(
        "update sys_option_lifecycle_adjustment_journal set baseline_subsumed_at_utc = now() \
         where economic_apply_id = $1",
    )
    .bind(&apply_id)
    .execute(&pool)
    .await
    .expect("first subsumption is allowed");
    assert!(sqlx::query(
        "update sys_option_lifecycle_adjustment_journal set baseline_subsumed_at_utc = null \
         where economic_apply_id = $1",
    )
    .bind(&apply_id)
    .execute(&pool)
    .await
    .is_err());
}

#[tokio::test]
async fn j09_reconciled_is_compare_and_set_and_reconciled_entries_subsume() {
    use OptionLifecycleActivityType::Exercise;
    let pool = require_pool().await;
    let a = account(&pool, "j09").await;
    let key = a.key();
    seed_ready(&pool, &key, "X1", Exercise, Some("X1")).await;

    // Not applied yet: cannot be reconciled.
    assert!(
        !mark_lifecycle_event_reconciled(&pool, &key, "X1", Exercise, Utc::now())
            .await
            .unwrap()
    );
    apply_lifecycle_adjustment_tx(&pool, &exercise_adjustment(&key, "X1", "X1"))
        .await
        .unwrap();

    // Awaiting the broker: subsumption never touches it and it stays replayable.
    subsume_reconciled_lifecycle_journal(&pool, DOMAIN, "paper", Utc::now())
        .await
        .unwrap();
    assert!(list_unsubsumed_lifecycle_journal(&pool, DOMAIN, "paper")
        .await
        .unwrap()
        .iter()
        .any(|e| e.broker_account_id == key));

    assert!(
        mark_lifecycle_event_reconciled(&pool, &key, "X1", Exercise, Utc::now())
            .await
            .unwrap()
    );
    assert!(
        !mark_lifecycle_event_reconciled(&pool, &key, "X1", Exercise, Utc::now())
            .await
            .unwrap(),
        "compare-and-set: a second reconcile is a no-op"
    );
    assert_eq!(
        state(&pool, &key, "X1", Exercise).await.state,
        LifecycleEventState::Reconciled
    );
    // RECONCILED is terminal at the database level.
    assert!(sqlx::query(
        "update sys_option_lifecycle_event_state set state = 'READY_TO_APPLY', \
         economic_apply_id = null where broker_account_id = $1 and lifecycle_activity_id = 'X1'",
    )
    .bind(&key)
    .execute(&pool)
    .await
    .is_err());

    let n = subsume_reconciled_lifecycle_journal(&pool, DOMAIN, "paper", Utc::now())
        .await
        .unwrap();
    assert!(n >= 1);
    assert!(!list_unsubsumed_lifecycle_journal(&pool, DOMAIN, "paper")
        .await
        .unwrap()
        .iter()
        .any(|e| e.broker_account_id == key));
}

#[tokio::test]
async fn j10_economic_apply_id_is_deterministic_and_fully_scoped() {
    use OptionLifecycleActivityType::{Assignment, Exercise};
    let base = economic_apply_id("alpaca:a", DOMAIN, "X1", Exercise);
    assert_eq!(base, economic_apply_id("alpaca:a", DOMAIN, "X1", Exercise));
    assert_ne!(base, economic_apply_id("alpaca:b", DOMAIN, "X1", Exercise));
    assert_ne!(
        base,
        economic_apply_id("alpaca:a", "crypto_24_7", "X1", Exercise)
    );
    assert_ne!(base, economic_apply_id("alpaca:a", DOMAIN, "X2", Exercise));
    assert_ne!(
        base,
        economic_apply_id("alpaca:a", DOMAIN, "X1", Assignment)
    );
}
