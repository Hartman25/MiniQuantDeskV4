//! MULTI-STRATEGY-RUNTIME-DISPATCH-01 Patch C2 (V4-STAGE-B-M2-REPAIR-03):
//! durable explicit multi-strategy authority evidence store proof.
//!
//! Proves `insert_explicit_multi_strategy_authority` /
//! `fetch_explicit_multi_strategy_authority` round-trip, idempotent-replay,
//! payload-collision, and DB-level constraint behavior, with zero writes to
//! any portfolio/P&L/order table beyond the one fixture run row each test
//! creates for itself.
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked `#[ignore]`.
//! Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-db --test scenario_explicit_multi_strategy_authority_01 -- \
//!     --include-ignored --test-threads=1

use chrono::{TimeZone, Utc};
use mqk_db::{
    fetch_explicit_multi_strategy_authority, insert_explicit_multi_strategy_authority, insert_run,
    ExplicitMultiStrategyAuthorityValidationError, InsertExplicitMultiStrategyAuthorityOutcome,
    NewExplicitMultiStrategyAuthority, NewExplicitMultiStrategyAuthorityBinding, NewRun,
    ENV_DB_URL, EXPLICIT_MULTI_STRATEGY_SOURCE_KIND,
};
use uuid::Uuid;

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    if std::env::var(ENV_DB_URL).is_err() {
        anyhow::bail!("SKIP: requires MQK_DATABASE_URL");
    }
    let pool = mqk_db::testkit_db_pool().await?;
    Ok(pool)
}

async fn cleanup(pool: &sqlx::PgPool, run_ids: &[Uuid]) {
    for run_id in run_ids {
        let _ = sqlx::query(
            "delete from sys_explicit_multi_strategy_authority_bindings where authority_id in \
             (select authority_id from sys_explicit_multi_strategy_authority where run_id = $1)",
        )
        .bind(run_id)
        .execute(pool)
        .await;
        let _ = sqlx::query("delete from sys_explicit_multi_strategy_authority where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
        let _ = sqlx::query("delete from runs where run_id = $1")
            .bind(run_id)
            .execute(pool)
            .await;
    }
}

async fn fixture_run(pool: &sqlx::PgPool, run_id: Uuid) {
    insert_run(
        pool,
        &NewRun {
            run_id,
            engine_id: "test-explicit-multi-strategy-authority".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 0, 0).unwrap(),
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({}),
            host_fingerprint: "test".to_string(),
        },
    )
    .await
    .expect("fixture run insert should succeed");
}

fn fixed_run_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.explicit-multi-strategy-authority-01.v1|{seed}").as_bytes(),
    )
}

fn fixed_authority_id(seed: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_DNS,
        format!("test.explicit-multi-strategy-authority-01.authority.v1|{seed}").as_bytes(),
    )
}

fn authorized_binding(symbol: &str, strategy_id: &str) -> NewExplicitMultiStrategyAuthorityBinding {
    NewExplicitMultiStrategyAuthorityBinding {
        symbol: symbol.to_string(),
        strategy_id: strategy_id.to_string(),
        timeframe_secs: 300,
        authorized: true,
        reason_code: "explicit_multi_strategy_authorized".to_string(),
        promotion_query_ok: true,
        promotion_state: Some("active_paper".to_string()),
        promotion_effective: true,
        promotion_expired: false,
        evidence_resolved: true,
        review_state_is_paper_candidate: true,
        evidence_review_state: Some("paper_candidate".to_string()),
        durable_legacy_fingerprint: Some("l".repeat(64)),
        recomputed_legacy_fingerprint: Some("l".repeat(64)),
        legacy_fingerprint_matches: true,
        durable_exact_fingerprint_v2: Some("v".repeat(64)),
        recomputed_exact_fingerprint_v2: Some("v".repeat(64)),
        exact_fingerprint_v2_matches: true,
        config_identity_verified: true,
        durable_config_fingerprint: Some("c".repeat(64)),
        current_config_fingerprint: Some("c".repeat(64)),
        registry_enabled: true,
        plugin_instantiable: true,
        timeframe_matches: true,
        data_ready: true,
        canonical_score_decimal: Some("1".to_string()),
        canonical_score_micros: Some(1_000_000),
        scanner_rank: Some(1),
        watchlist_assigned: true,
        evidence_review_id: None,
        evidence_scanner_scan_id: None,
        evidence_artifact_path: None,
        evidence_git_hash: None,
        promotion_transition_id: Some("transition-1".to_string()),
        promotion_effective_at: None,
        promotion_expires_at: None,
        evidence_transition_id: Some("transition-1".to_string()),
        exact_reason: None,
    }
}

fn refused_binding(symbol: &str, strategy_id: &str) -> NewExplicitMultiStrategyAuthorityBinding {
    let mut b = authorized_binding(symbol, strategy_id);
    b.authorized = false;
    b.reason_code = "explicit_multi_strategy_not_promoted".to_string();
    b.promotion_state = None;
    b.promotion_effective = false;
    b.config_identity_verified = false;
    b.durable_config_fingerprint = None;
    b.current_config_fingerprint = None;
    b
}

fn sample_authority(
    run_id: Uuid,
    authority_id: Uuid,
    source_artifact_hash: &str,
    config_fingerprint: &str,
    bindings: Vec<NewExplicitMultiStrategyAuthorityBinding>,
) -> NewExplicitMultiStrategyAuthority {
    NewExplicitMultiStrategyAuthority {
        authority_id,
        run_id,
        source_kind: EXPLICIT_MULTI_STRATEGY_SOURCE_KIND.to_string(),
        source_identity: "watchlist-v3.json".to_string(),
        source_artifact_hash: source_artifact_hash.to_string(),
        config_fingerprint: config_fingerprint.to_string(),
        market_date: "2099-03-01".to_string(),
        approved_for_live: false,
        writer_version: "mqk-daemon-test".to_string(),
        created_at_utc: Utc.with_ymd_and_hms(2099, 3, 1, 12, 5, 0).unwrap(),
        bindings,
    }
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn insert_and_fetch_round_trips() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("round_trip");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let authority_id = fixed_authority_id("round_trip");
    let authority = sample_authority(
        run_id,
        authority_id,
        "hash-a",
        "cfg-a",
        vec![
            authorized_binding("AAPL", "intraday_scalper"),
            authorized_binding("AAPL", "intraday_short_scalper"),
            refused_binding("MSFT", "swing_momentum"),
        ],
    );

    let outcome = insert_explicit_multi_strategy_authority(&pool, authority.clone())
        .await
        .expect("insert should succeed");
    assert_eq!(
        outcome,
        InsertExplicitMultiStrategyAuthorityOutcome::Inserted
    );

    let (header, bindings) = fetch_explicit_multi_strategy_authority(&pool, authority_id)
        .await
        .expect("fetch should succeed")
        .expect("row must exist after insert");

    assert_eq!(header.authority_id, authority_id);
    assert_eq!(header.run_id, run_id);
    assert_eq!(header.source_kind, EXPLICIT_MULTI_STRATEGY_SOURCE_KIND);
    assert_eq!(header.source_artifact_hash, "hash-a");
    assert_eq!(header.config_fingerprint, "cfg-a");
    assert!(!header.approved_for_live);
    assert_eq!(header.binding_count, 3);
    assert_eq!(header.authorized_count, 2);
    assert_eq!(bindings.len(), 3);
    assert!(bindings.iter().any(|b| b.symbol == "MSFT" && !b.authorized));

    // Exhaustive field-by-field round trip: every evidence column, not just
    // the header/symbol/authorized spot-checks above -- a silent column
    // type/bind-order mismatch on any one of the 33 evidence fields (e.g.
    // `canonical_score_micros` truncated by a wrong Postgres column type)
    // would otherwise pass this test despite corrupting durable evidence.
    let fetched_scalper = bindings
        .iter()
        .find(|b| b.symbol == "AAPL" && b.strategy_id == "intraday_scalper")
        .expect("AAPL/intraday_scalper binding must be present");
    let written_scalper = authorized_binding("AAPL", "intraday_scalper");
    assert_eq!(fetched_scalper.timeframe_secs, written_scalper.timeframe_secs);
    assert_eq!(fetched_scalper.authorized, written_scalper.authorized);
    assert_eq!(fetched_scalper.reason_code, written_scalper.reason_code);
    assert_eq!(
        fetched_scalper.promotion_query_ok,
        written_scalper.promotion_query_ok
    );
    assert_eq!(fetched_scalper.promotion_state, written_scalper.promotion_state);
    assert_eq!(
        fetched_scalper.promotion_effective,
        written_scalper.promotion_effective
    );
    assert_eq!(fetched_scalper.promotion_expired, written_scalper.promotion_expired);
    assert_eq!(fetched_scalper.evidence_resolved, written_scalper.evidence_resolved);
    assert_eq!(
        fetched_scalper.review_state_is_paper_candidate,
        written_scalper.review_state_is_paper_candidate
    );
    assert_eq!(
        fetched_scalper.evidence_review_state,
        written_scalper.evidence_review_state
    );
    assert_eq!(
        fetched_scalper.durable_legacy_fingerprint,
        written_scalper.durable_legacy_fingerprint
    );
    assert_eq!(
        fetched_scalper.recomputed_legacy_fingerprint,
        written_scalper.recomputed_legacy_fingerprint
    );
    assert_eq!(
        fetched_scalper.legacy_fingerprint_matches,
        written_scalper.legacy_fingerprint_matches
    );
    assert_eq!(
        fetched_scalper.durable_exact_fingerprint_v2,
        written_scalper.durable_exact_fingerprint_v2
    );
    assert_eq!(
        fetched_scalper.recomputed_exact_fingerprint_v2,
        written_scalper.recomputed_exact_fingerprint_v2
    );
    assert_eq!(
        fetched_scalper.exact_fingerprint_v2_matches,
        written_scalper.exact_fingerprint_v2_matches
    );
    assert_eq!(
        fetched_scalper.config_identity_verified,
        written_scalper.config_identity_verified
    );
    assert_eq!(
        fetched_scalper.durable_config_fingerprint,
        written_scalper.durable_config_fingerprint
    );
    assert_eq!(
        fetched_scalper.current_config_fingerprint,
        written_scalper.current_config_fingerprint
    );
    assert_eq!(fetched_scalper.registry_enabled, written_scalper.registry_enabled);
    assert_eq!(
        fetched_scalper.plugin_instantiable,
        written_scalper.plugin_instantiable
    );
    assert_eq!(fetched_scalper.timeframe_matches, written_scalper.timeframe_matches);
    assert_eq!(fetched_scalper.data_ready, written_scalper.data_ready);
    assert_eq!(
        fetched_scalper.canonical_score_decimal,
        written_scalper.canonical_score_decimal
    );
    assert_eq!(
        fetched_scalper.canonical_score_micros,
        written_scalper.canonical_score_micros
    );
    assert_eq!(fetched_scalper.scanner_rank, written_scalper.scanner_rank);
    assert_eq!(
        fetched_scalper.watchlist_assigned,
        written_scalper.watchlist_assigned
    );
    assert_eq!(fetched_scalper.evidence_review_id, written_scalper.evidence_review_id);
    assert_eq!(
        fetched_scalper.evidence_scanner_scan_id,
        written_scalper.evidence_scanner_scan_id
    );
    assert_eq!(
        fetched_scalper.evidence_artifact_path,
        written_scalper.evidence_artifact_path
    );
    assert_eq!(fetched_scalper.evidence_git_hash, written_scalper.evidence_git_hash);
    assert_eq!(
        fetched_scalper.promotion_transition_id,
        written_scalper.promotion_transition_id
    );
    assert_eq!(
        fetched_scalper.promotion_effective_at,
        written_scalper.promotion_effective_at
    );
    assert_eq!(
        fetched_scalper.promotion_expires_at,
        written_scalper.promotion_expires_at
    );
    assert_eq!(
        fetched_scalper.evidence_transition_id,
        written_scalper.evidence_transition_id
    );
    assert_eq!(fetched_scalper.exact_reason, written_scalper.exact_reason);

    cleanup(&pool, &[run_id]).await;
}

/// Required negative control: replaying the exact same logical authority
/// (same authority_id, same payload) is an idempotent no-op -- never a
/// duplicate row, never an error.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn replay_of_identical_payload_is_idempotent() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("idempotent_replay");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let authority_id = fixed_authority_id("idempotent_replay");
    let authority = sample_authority(
        run_id,
        authority_id,
        "hash-a",
        "cfg-a",
        vec![authorized_binding("AAPL", "intraday_scalper")],
    );

    let first = insert_explicit_multi_strategy_authority(&pool, authority.clone())
        .await
        .expect("first insert should succeed");
    assert_eq!(first, InsertExplicitMultiStrategyAuthorityOutcome::Inserted);

    let second = insert_explicit_multi_strategy_authority(&pool, authority.clone())
        .await
        .expect("replay insert should succeed");
    assert_eq!(
        second,
        InsertExplicitMultiStrategyAuthorityOutcome::AlreadyExists
    );

    let (_, bindings) = fetch_explicit_multi_strategy_authority(&pool, authority_id)
        .await
        .expect("fetch should succeed")
        .expect("row must exist");
    assert_eq!(
        bindings.len(),
        1,
        "replay must never duplicate binding rows"
    );

    cleanup(&pool, &[run_id]).await;
}

/// Required negative control: payload collision fails closed. Two different
/// binding-set payloads presented under the same authority_id must never be
/// silently accepted as a replay and must never overwrite the original row.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn divergent_payload_under_same_authority_id_is_a_collision() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("payload_collision");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let authority_id = fixed_authority_id("payload_collision");
    let original = sample_authority(
        run_id,
        authority_id,
        "hash-a",
        "cfg-a",
        vec![authorized_binding("AAPL", "intraday_scalper")],
    );
    insert_explicit_multi_strategy_authority(&pool, original)
        .await
        .expect("first insert should succeed");

    // Same id, different binding set -- in real production use this could
    // never happen (authority_id is derived from the payload), but the
    // storage layer must refuse it defensively regardless of how the
    // mismatch arose.
    let divergent = sample_authority(
        run_id,
        authority_id,
        "hash-a",
        "cfg-a",
        vec![authorized_binding("MSFT", "swing_momentum")],
    );
    let outcome = insert_explicit_multi_strategy_authority(&pool, divergent)
        .await
        .expect("collision insert call itself should not error");
    assert!(
        matches!(
            outcome,
            InsertExplicitMultiStrategyAuthorityOutcome::PayloadCollision { .. }
        ),
        "expected PayloadCollision, got {outcome:?}"
    );

    let (_, bindings) = fetch_explicit_multi_strategy_authority(&pool, authority_id)
        .await
        .expect("fetch should succeed")
        .expect("row must exist");
    assert_eq!(
        bindings[0].symbol, "AAPL",
        "the original row must never be overwritten by the colliding payload"
    );

    cleanup(&pool, &[run_id]).await;
}

/// Required negative control: `approved_for_live` can never become true --
/// the DB-level CHECK constraint refuses it even if the app-level validator
/// were bypassed via a raw SQL insert.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn approved_for_live_true_is_refused_at_the_db_level() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let run_id = fixed_run_id("approved_for_live_db_check");
    cleanup(&pool, &[run_id]).await;
    fixture_run(&pool, run_id).await;

    let authority_id = fixed_authority_id("approved_for_live_db_check");
    let result = sqlx::query(
        r#"
        insert into sys_explicit_multi_strategy_authority
            (authority_id, run_id, source_kind, source_identity, source_artifact_hash,
             config_fingerprint, market_date, approved_for_live, binding_count,
             authorized_count, writer_version, created_at_utc)
        values ($1, $2, $3, $4, $5, $6, $7, true, 1, 1, $8, $9)
        "#,
    )
    .bind(authority_id)
    .bind(run_id)
    .bind(EXPLICIT_MULTI_STRATEGY_SOURCE_KIND)
    .bind("watchlist-v3.json")
    .bind("hash-a")
    .bind("cfg-a")
    .bind("2099-03-01")
    .bind("test-writer")
    .bind(Utc.with_ymd_and_hms(2099, 3, 1, 12, 5, 0).unwrap())
    .execute(&pool)
    .await;

    assert!(
        result.is_err(),
        "a raw insert with approved_for_live=true must be refused by the DB CHECK constraint"
    );

    cleanup(&pool, &[run_id]).await;
}

/// Required negative control: the app-level validator refuses to even
/// attempt a DB write for a duplicate exact binding, an empty binding set,
/// or `approved_for_live = true` -- these never reach the DB at all (no
/// fixture run/cleanup needed since no row is ever written).
#[test]
fn app_level_validation_fails_closed_before_any_db_attempt() {
    use mqk_db::validate_new_authority;

    let run_id = fixed_run_id("app_level_validation");
    let base = sample_authority(
        run_id,
        fixed_authority_id("app_level_validation"),
        "hash-a",
        "cfg-a",
        vec![authorized_binding("AAPL", "intraday_scalper")],
    );

    let mut approved_for_live_true = base.clone();
    approved_for_live_true.approved_for_live = true;
    assert_eq!(
        validate_new_authority(&approved_for_live_true),
        Err(ExplicitMultiStrategyAuthorityValidationError::ApprovedForLive)
    );

    let mut empty = base.clone();
    empty.bindings = vec![];
    assert_eq!(
        validate_new_authority(&empty),
        Err(ExplicitMultiStrategyAuthorityValidationError::NoBindings)
    );

    let mut duplicate = base;
    duplicate.bindings = vec![
        authorized_binding("AAPL", "intraday_scalper"),
        authorized_binding("AAPL", "intraday_scalper"),
    ];
    assert_eq!(
        validate_new_authority(&duplicate),
        Err(
            ExplicitMultiStrategyAuthorityValidationError::DuplicateBinding {
                symbol: "AAPL".to_string(),
                strategy_id: "intraday_scalper".to_string(),
                timeframe_secs: 300,
            }
        )
    );
}

/// Required negative control: `fetch_explicit_multi_strategy_authority`
/// returns `Ok(None)` for a missing authority -- callers (Patch C3's
/// read-validate step) must fail closed on this, never treat it as an
/// empty-but-valid authorization.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL"]
async fn fetch_of_nonexistent_authority_returns_none() {
    let Ok(pool) = test_pool().await else {
        eprintln!("skipped: requires MQK_DATABASE_URL");
        return;
    };
    let missing = fixed_authority_id("does_not_exist_ever");
    let result = fetch_explicit_multi_strategy_authority(&pool, missing)
        .await
        .expect("fetch itself should not error for a missing row");
    assert!(result.is_none());
}
