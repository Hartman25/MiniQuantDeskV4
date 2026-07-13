//! STRATEGY-PROMOTION-REGISTRY-01B: durable strategy paper-promotion
//! registry scenarios.
//!
//! Proves:
//! - migration applies cleanly (implicit: every test below calls
//!   `mqk_db::migrate` via `test_pool`)
//! - an empty registry is authoritative zero approvals (`Ok(None)`, never
//!   a synthesized row)
//! - a valid transition persists and round-trips exactly, including
//!   caller-injected timestamps/ids (no DB-generated defaults)
//! - a duplicate `transition_id` insert is idempotent (no second row, no
//!   error)
//! - exact identity (`strategy_id` + `symbol` + `timeframe_secs`) is
//!   preserved and does not cross-match a near-identical identity
//! - a symbol or timeframe mismatch never matches an existing approval
//! - the legal transition graph accepts every documented legal edge
//! - the legal transition graph rejects illegal edges (`CHECK` violation)
//! - an `active_paper` row past `expires_at_utc` classifies as
//!   `promotion_expired`, not tradable
//! - `demoted` / `retired` / `rejected` all classify as non-tradable with
//!   their own distinct reason codes
//! - history remains fully readable after a later transition is appended
//!   (append-only, nothing is overwritten)
//! - a genuine query failure (e.g. a closed pool) surfaces as `Err`, never
//!   silently coerced into `Ok(None)` ("authoritative empty")
//! - registering + enabling a strategy in `sys_strategy_registry` never
//!   produces an automatic promotion row for the same identity
//!
//! All DB-backed tests require `MQK_DATABASE_URL` and are marked
//! `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-db --test scenario_strategy_promotion_registry_01 -- --include-ignored

use chrono::{Duration, Utc};
use mqk_db::{
    evaluate_promotion_tradability, fetch_current_promotion_state, fetch_promotion_history,
    insert_strategy_promotion_transition, is_legal_transition, upsert_strategy_registry_entry,
    InsertStrategyPromotionTransitionArgs, PromotionReasonCode, UpsertStrategyRegistryArgs,
    ENV_DB_URL, PROMOTION_STATE_ACTIVE_PAPER, PROMOTION_STATE_DEMOTED,
    PROMOTION_STATE_PAPER_APPROVED, PROMOTION_STATE_REJECTED, PROMOTION_STATE_RETIRED,
    PROMOTION_STATE_SHADOW_APPROVED,
};
use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn test_pool() -> anyhow::Result<sqlx::PgPool> {
    let url = match std::env::var(ENV_DB_URL) {
        Ok(v) => v,
        Err(_) => panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-db --test scenario_strategy_promotion_registry_01 -- --include-ignored"
        ),
    };
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await?;
    mqk_db::migrate(&pool).await?;
    Ok(pool)
}

/// Generate a unique strategy_id for test isolation.
fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

fn transition_id_for(seed: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes())
}

#[allow(clippy::too_many_arguments)]
fn make_args(
    transition_id: Uuid,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    previous_state: Option<&str>,
    new_state: &str,
    effective_at: chrono::DateTime<Utc>,
    created_at: chrono::DateTime<Utc>,
) -> InsertStrategyPromotionTransitionArgs {
    InsertStrategyPromotionTransitionArgs {
        transition_id,
        strategy_id: strategy_id.to_string(),
        symbol: symbol.to_string(),
        timeframe_secs,
        config_fingerprint: None,
        config_identity_status: "unavailable_in_current_runtime".to_string(),
        previous_state: previous_state.map(|s| s.to_string()),
        new_state: new_state.to_string(),
        evidence_review_id: Some("review-evidence-id".to_string()),
        evidence_scanner_scan_id: Some("scan-evidence-id".to_string()),
        evidence_git_hash: Some("deadbeef".to_string()),
        evidence_artifact_path: Some("exports/strategy_reviews/test".to_string()),
        evidence_fingerprint: Some("fingerprint-abc123".to_string()),
        effective_at_utc: effective_at,
        expires_at_utc: None,
        initiated_by: "test-operator".to_string(),
        reason: "scenario test".to_string(),
        created_at_utc: created_at,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

/// An identity that has never had a transition returns `Ok(None)` —
/// authoritative "no approval", never a synthesized row.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn empty_registry_is_authoritative_zero_approvals() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_empty");

    let current = fetch_current_promotion_state(&pool, &strategy_id, "AAPL", 86400).await?;
    assert!(
        current.is_none(),
        "unknown identity must return None, not a synthesized approval"
    );

    let (tradable, reason) = evaluate_promotion_tradability(None, Utc::now());
    assert!(!tradable);
    assert_eq!(reason, PromotionReasonCode::PromotionMissing);

    Ok(())
}

/// A valid transition persists and round-trips exactly, including
/// caller-injected timestamps/evidence fields — nothing is DB-generated.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn valid_transition_persists_and_round_trips() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_persist");
    let symbol = "AAPL";
    let timeframe_secs = 86400;
    let effective_at = Utc::now();
    let created_at = effective_at + Duration::milliseconds(5);
    let tid = transition_id_for(&format!(
        "{strategy_id}:{symbol}:{timeframe_secs}:shadow_approved:1"
    ));

    let inserted = insert_strategy_promotion_transition(
        &pool,
        &make_args(
            tid,
            &strategy_id,
            symbol,
            timeframe_secs,
            None,
            PROMOTION_STATE_SHADOW_APPROVED,
            effective_at,
            created_at,
        ),
    )
    .await?;
    assert!(inserted, "first insert of a new transition_id must succeed");

    let current = fetch_current_promotion_state(&pool, &strategy_id, symbol, timeframe_secs)
        .await?
        .expect("transition must be immediately queryable");

    assert_eq!(current.transition_id, tid);
    assert_eq!(current.strategy_id, strategy_id);
    assert_eq!(current.symbol, symbol);
    assert_eq!(current.timeframe_secs, timeframe_secs);
    assert_eq!(current.previous_state, None);
    assert_eq!(current.new_state, PROMOTION_STATE_SHADOW_APPROVED);
    assert_eq!(current.config_fingerprint, None);
    assert_eq!(
        current.config_identity_status,
        "unavailable_in_current_runtime"
    );
    assert_eq!(
        current.evidence_review_id.as_deref(),
        Some("review-evidence-id")
    );
    assert_eq!(
        current.effective_at_utc.timestamp(),
        effective_at.timestamp()
    );
    assert_eq!(current.created_at_utc.timestamp(), created_at.timestamp());
    assert_eq!(current.initiated_by, "test-operator");

    let (tradable, reason) = evaluate_promotion_tradability(Some(&current), Utc::now());
    assert!(!tradable, "shadow_approved must never be paper-tradable");
    assert_eq!(reason, PromotionReasonCode::PromotionShadowOnly);

    Ok(())
}

/// A duplicate `transition_id` insert is idempotent: no error, no second
/// row, `Ok(false)` on the repeat.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn duplicate_transition_id_is_idempotent() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_dup");
    let symbol = "MSFT";
    let timeframe_secs = 3600;
    let effective_at = Utc::now();
    let tid = transition_id_for(&format!("{strategy_id}:{symbol}:{timeframe_secs}:dup"));

    let args = make_args(
        tid,
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        effective_at,
        effective_at,
    );

    let first = insert_strategy_promotion_transition(&pool, &args).await?;
    assert!(first, "first insert must succeed");

    let second = insert_strategy_promotion_transition(&pool, &args).await?;
    assert!(
        !second,
        "duplicate transition_id must be a no-op, not an error"
    );

    let history = fetch_promotion_history(&pool, &strategy_id, symbol, timeframe_secs, 100).await?;
    assert_eq!(
        history.len(),
        1,
        "duplicate insert must not create a second row"
    );

    Ok(())
}

/// Exact identity (strategy_id + symbol + timeframe_secs) is preserved:
/// a near-identical identity (different symbol, or different timeframe)
/// never matches an existing approval.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn symbol_and_timeframe_mismatch_never_matches() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_identity");
    let effective_at = Utc::now();

    let tid = transition_id_for(&format!("{strategy_id}:AAPL:86400:identity"));
    insert_strategy_promotion_transition(
        &pool,
        &make_args(
            tid,
            &strategy_id,
            "AAPL",
            86400,
            None,
            PROMOTION_STATE_SHADOW_APPROVED,
            effective_at,
            effective_at,
        ),
    )
    .await?;

    // Same strategy_id, different symbol -> no match.
    let wrong_symbol = fetch_current_promotion_state(&pool, &strategy_id, "MSFT", 86400).await?;
    assert!(wrong_symbol.is_none(), "different symbol must not match");

    // Same strategy_id/symbol, different timeframe -> no match.
    let wrong_timeframe = fetch_current_promotion_state(&pool, &strategy_id, "AAPL", 3600).await?;
    assert!(
        wrong_timeframe.is_none(),
        "different timeframe_secs must not match"
    );

    // Exact identity -> matches.
    let exact = fetch_current_promotion_state(&pool, &strategy_id, "AAPL", 86400).await?;
    assert!(exact.is_some(), "exact identity must match");

    Ok(())
}

/// Every documented legal edge in the transition graph is accepted by
/// [`is_legal_transition`] and by the DB `CHECK` constraint (proven by a
/// successful insert walking the full lifecycle for one identity).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn legal_transition_graph_accepted() -> anyhow::Result<()> {
    assert!(is_legal_transition(None, PROMOTION_STATE_SHADOW_APPROVED));
    assert!(is_legal_transition(
        Some(PROMOTION_STATE_SHADOW_APPROVED),
        PROMOTION_STATE_PAPER_APPROVED
    ));
    assert!(is_legal_transition(
        Some(PROMOTION_STATE_SHADOW_APPROVED),
        PROMOTION_STATE_REJECTED
    ));
    assert!(is_legal_transition(
        Some(PROMOTION_STATE_PAPER_APPROVED),
        PROMOTION_STATE_ACTIVE_PAPER
    ));
    assert!(is_legal_transition(
        Some(PROMOTION_STATE_PAPER_APPROVED),
        PROMOTION_STATE_DEMOTED
    ));
    assert!(is_legal_transition(
        Some(PROMOTION_STATE_ACTIVE_PAPER),
        PROMOTION_STATE_DEMOTED
    ));
    assert!(is_legal_transition(
        Some(PROMOTION_STATE_DEMOTED),
        PROMOTION_STATE_SHADOW_APPROVED
    ));
    for from in [
        PROMOTION_STATE_SHADOW_APPROVED,
        PROMOTION_STATE_PAPER_APPROVED,
        PROMOTION_STATE_ACTIVE_PAPER,
        PROMOTION_STATE_DEMOTED,
    ] {
        assert!(
            is_legal_transition(Some(from), PROMOTION_STATE_RETIRED),
            "{from} -> retired must be legal"
        );
    }

    // Walk a real lifecycle through the DB to prove the CHECK constraint
    // accepts every one of these edges end to end.
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_lifecycle");
    let symbol = "GOOG";
    let timeframe_secs = 900;
    let t0 = Utc::now();

    let steps: [(Option<&str>, &str); 4] = [
        (None, PROMOTION_STATE_SHADOW_APPROVED),
        (
            Some(PROMOTION_STATE_SHADOW_APPROVED),
            PROMOTION_STATE_PAPER_APPROVED,
        ),
        (
            Some(PROMOTION_STATE_PAPER_APPROVED),
            PROMOTION_STATE_ACTIVE_PAPER,
        ),
        (Some(PROMOTION_STATE_ACTIVE_PAPER), PROMOTION_STATE_DEMOTED),
    ];
    for (i, (prev, new)) in steps.iter().enumerate() {
        let effective_at = t0 + Duration::seconds(i as i64 + 1);
        let tid = transition_id_for(&format!(
            "{strategy_id}:{symbol}:{timeframe_secs}:{new}:{i}"
        ));
        let inserted = insert_strategy_promotion_transition(
            &pool,
            &make_args(
                tid,
                &strategy_id,
                symbol,
                timeframe_secs,
                *prev,
                new,
                effective_at,
                effective_at,
            ),
        )
        .await?;
        assert!(inserted, "legal step {prev:?} -> {new} must insert cleanly");
    }

    let current = fetch_current_promotion_state(&pool, &strategy_id, symbol, timeframe_secs)
        .await?
        .expect("must have a current state after lifecycle walk");
    assert_eq!(current.new_state, PROMOTION_STATE_DEMOTED);

    let (tradable, reason) = evaluate_promotion_tradability(Some(&current), Utc::now());
    assert!(!tradable);
    assert_eq!(reason, PromotionReasonCode::PromotionDemoted);

    Ok(())
}

/// Illegal transitions are rejected by both the pure `is_legal_transition`
/// mirror and the DB `CHECK` constraint (insert must error, not silently
/// succeed).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn illegal_transition_rejected() -> anyhow::Result<()> {
    // Pure mirror rejects the same illegal edges the DB CHECK rejects.
    assert!(!is_legal_transition(None, PROMOTION_STATE_ACTIVE_PAPER));
    assert!(!is_legal_transition(
        Some(PROMOTION_STATE_SHADOW_APPROVED),
        PROMOTION_STATE_ACTIVE_PAPER
    ));
    assert!(!is_legal_transition(
        Some(PROMOTION_STATE_RETIRED),
        PROMOTION_STATE_SHADOW_APPROVED
    ));
    assert!(!is_legal_transition(
        Some(PROMOTION_STATE_REJECTED),
        PROMOTION_STATE_PAPER_APPROVED
    ));

    // DB CHECK constraint rejects the same illegal edge at the storage layer.
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_illegal");
    let symbol = "TSLA";
    let timeframe_secs = 300;
    let effective_at = Utc::now();
    let tid = transition_id_for(&format!("{strategy_id}:{symbol}:{timeframe_secs}:illegal"));

    // no state -> active_paper is never legal; must be rejected by the CHECK.
    let result = insert_strategy_promotion_transition(
        &pool,
        &make_args(
            tid,
            &strategy_id,
            symbol,
            timeframe_secs,
            None,
            PROMOTION_STATE_ACTIVE_PAPER,
            effective_at,
            effective_at,
        ),
    )
    .await;
    assert!(
        result.is_err(),
        "no-state -> active_paper must be rejected by the DB CHECK constraint"
    );

    // No row must have been written.
    let current =
        fetch_current_promotion_state(&pool, &strategy_id, symbol, timeframe_secs).await?;
    assert!(current.is_none(), "rejected transition must leave no row");

    Ok(())
}

/// An `active_paper` row past its `expires_at_utc` classifies as
/// `promotion_expired`, not tradable.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn expired_active_paper_is_not_tradable() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_expired");
    let symbol = "NFLX";
    let timeframe_secs = 86400;
    let now = Utc::now();

    // shadow_approved -> paper_approved -> active_paper, with an
    // expires_at_utc already in the past.
    let steps: [(Option<&str>, &str); 3] = [
        (None, PROMOTION_STATE_SHADOW_APPROVED),
        (
            Some(PROMOTION_STATE_SHADOW_APPROVED),
            PROMOTION_STATE_PAPER_APPROVED,
        ),
        (
            Some(PROMOTION_STATE_PAPER_APPROVED),
            PROMOTION_STATE_ACTIVE_PAPER,
        ),
    ];
    for (i, (prev, new)) in steps.iter().enumerate() {
        let effective_at = now - Duration::hours(2) + Duration::seconds(i as i64);
        let tid = transition_id_for(&format!(
            "{strategy_id}:{symbol}:{timeframe_secs}:{new}:expired:{i}"
        ));
        let mut args = make_args(
            tid,
            &strategy_id,
            symbol,
            timeframe_secs,
            *prev,
            new,
            effective_at,
            effective_at,
        );
        if *new == PROMOTION_STATE_ACTIVE_PAPER {
            args.expires_at_utc = Some(now - Duration::hours(1));
        }
        insert_strategy_promotion_transition(&pool, &args).await?;
    }

    let current = fetch_current_promotion_state(&pool, &strategy_id, symbol, timeframe_secs)
        .await?
        .expect("must have a current state");
    assert_eq!(current.new_state, PROMOTION_STATE_ACTIVE_PAPER);

    let (tradable, reason) = evaluate_promotion_tradability(Some(&current), now);
    assert!(!tradable, "expired active_paper must not be tradable");
    assert_eq!(reason, PromotionReasonCode::PromotionExpired);

    // An active_paper row NOT past expiry must be tradable.
    let (tradable_before_expiry, reason_before_expiry) =
        evaluate_promotion_tradability(Some(&current), now - Duration::hours(3));
    assert!(tradable_before_expiry);
    assert_eq!(reason_before_expiry, PromotionReasonCode::PromotionActive);

    Ok(())
}

/// `demoted`, `retired`, and `rejected` all classify as non-tradable, each
/// with its own distinct reason code.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn demoted_retired_rejected_are_not_tradable() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let now = Utc::now();

    // demoted
    {
        let strategy_id = unique_id("promo_state_demoted");
        let symbol = "AMD";
        let timeframe_secs = 3600;
        for (i, (prev, new)) in [
            (None, PROMOTION_STATE_SHADOW_APPROVED),
            (
                Some(PROMOTION_STATE_SHADOW_APPROVED),
                PROMOTION_STATE_PAPER_APPROVED,
            ),
            (
                Some(PROMOTION_STATE_PAPER_APPROVED),
                PROMOTION_STATE_DEMOTED,
            ),
        ]
        .iter()
        .enumerate()
        {
            let effective_at = now + Duration::seconds(i as i64);
            let tid = transition_id_for(&format!("{strategy_id}:{symbol}:{new}:{i}"));
            insert_strategy_promotion_transition(
                &pool,
                &make_args(
                    tid,
                    &strategy_id,
                    symbol,
                    timeframe_secs,
                    *prev,
                    new,
                    effective_at,
                    effective_at,
                ),
            )
            .await?;
        }
        let current = fetch_current_promotion_state(&pool, &strategy_id, symbol, timeframe_secs)
            .await?
            .unwrap();
        let (tradable, reason) = evaluate_promotion_tradability(Some(&current), now);
        assert!(!tradable);
        assert_eq!(reason, PromotionReasonCode::PromotionDemoted);
    }

    // retired
    {
        let strategy_id = unique_id("promo_state_retired");
        let symbol = "AMD";
        let timeframe_secs = 3600;
        for (i, (prev, new)) in [
            (None, PROMOTION_STATE_SHADOW_APPROVED),
            (
                Some(PROMOTION_STATE_SHADOW_APPROVED),
                PROMOTION_STATE_RETIRED,
            ),
        ]
        .iter()
        .enumerate()
        {
            let effective_at = now + Duration::seconds(i as i64);
            let tid = transition_id_for(&format!("{strategy_id}:{symbol}:{new}:{i}"));
            insert_strategy_promotion_transition(
                &pool,
                &make_args(
                    tid,
                    &strategy_id,
                    symbol,
                    timeframe_secs,
                    *prev,
                    new,
                    effective_at,
                    effective_at,
                ),
            )
            .await?;
        }
        let current = fetch_current_promotion_state(&pool, &strategy_id, symbol, timeframe_secs)
            .await?
            .unwrap();
        let (tradable, reason) = evaluate_promotion_tradability(Some(&current), now);
        assert!(!tradable);
        assert_eq!(reason, PromotionReasonCode::PromotionRetired);
    }

    // rejected
    {
        let strategy_id = unique_id("promo_state_rejected");
        let symbol = "AMD";
        let timeframe_secs = 3600;
        for (i, (prev, new)) in [
            (None, PROMOTION_STATE_SHADOW_APPROVED),
            (
                Some(PROMOTION_STATE_SHADOW_APPROVED),
                PROMOTION_STATE_REJECTED,
            ),
        ]
        .iter()
        .enumerate()
        {
            let effective_at = now + Duration::seconds(i as i64);
            let tid = transition_id_for(&format!("{strategy_id}:{symbol}:{new}:{i}"));
            insert_strategy_promotion_transition(
                &pool,
                &make_args(
                    tid,
                    &strategy_id,
                    symbol,
                    timeframe_secs,
                    *prev,
                    new,
                    effective_at,
                    effective_at,
                ),
            )
            .await?;
        }
        let current = fetch_current_promotion_state(&pool, &strategy_id, symbol, timeframe_secs)
            .await?
            .unwrap();
        let (tradable, reason) = evaluate_promotion_tradability(Some(&current), now);
        assert!(!tradable);
        assert_eq!(reason, PromotionReasonCode::PromotionRejected);
    }

    Ok(())
}

/// History remains fully readable after a later transition is appended —
/// nothing is overwritten (append-only).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn history_remains_after_later_transition() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_history");
    let symbol = "IBM";
    let timeframe_secs = 86400;
    let t0 = Utc::now();

    let steps: [(Option<&str>, &str); 3] = [
        (None, PROMOTION_STATE_SHADOW_APPROVED),
        (
            Some(PROMOTION_STATE_SHADOW_APPROVED),
            PROMOTION_STATE_PAPER_APPROVED,
        ),
        (
            Some(PROMOTION_STATE_PAPER_APPROVED),
            PROMOTION_STATE_ACTIVE_PAPER,
        ),
    ];
    for (i, (prev, new)) in steps.iter().enumerate() {
        let effective_at = t0 + Duration::seconds(i as i64);
        let tid = transition_id_for(&format!("{strategy_id}:{symbol}:{new}:hist:{i}"));
        insert_strategy_promotion_transition(
            &pool,
            &make_args(
                tid,
                &strategy_id,
                symbol,
                timeframe_secs,
                *prev,
                new,
                effective_at,
                effective_at,
            ),
        )
        .await?;
    }

    let history_before =
        fetch_promotion_history(&pool, &strategy_id, symbol, timeframe_secs, 100).await?;
    assert_eq!(history_before.len(), 3);

    // Append one more transition.
    let effective_at = t0 + Duration::seconds(10);
    let tid = transition_id_for(&format!("{strategy_id}:{symbol}:demoted:hist:final"));
    insert_strategy_promotion_transition(
        &pool,
        &make_args(
            tid,
            &strategy_id,
            symbol,
            timeframe_secs,
            Some(PROMOTION_STATE_ACTIVE_PAPER),
            PROMOTION_STATE_DEMOTED,
            effective_at,
            effective_at,
        ),
    )
    .await?;

    let history_after =
        fetch_promotion_history(&pool, &strategy_id, symbol, timeframe_secs, 100).await?;
    assert_eq!(
        history_after.len(),
        4,
        "history must grow, not replace, on a new transition"
    );
    // Newest-first ordering: the just-appended demoted transition is first.
    assert_eq!(history_after[0].new_state, PROMOTION_STATE_DEMOTED);
    // Every earlier state must still be present and unaltered.
    let states: Vec<&str> = history_after.iter().map(|r| r.new_state.as_str()).collect();
    assert!(states.contains(&PROMOTION_STATE_SHADOW_APPROVED));
    assert!(states.contains(&PROMOTION_STATE_PAPER_APPROVED));
    assert!(states.contains(&PROMOTION_STATE_ACTIVE_PAPER));

    Ok(())
}

/// A genuine query failure (closed pool) surfaces as `Err`, never silently
/// coerced into `Ok(None)` — "unavailable" and "authoritative empty" must
/// stay distinguishable.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn query_failure_is_not_treated_as_empty_approval() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    pool.close().await;

    let result = fetch_current_promotion_state(&pool, "any_strategy", "AAPL", 86400).await;
    assert!(
        result.is_err(),
        "a query against a closed pool must return Err, not Ok(None)"
    );

    Ok(())
}

/// Registering + enabling a strategy in `sys_strategy_registry` must never
/// produce an automatic promotion row for the same identity.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn enabled_registry_entry_never_auto_promotes() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_no_autobackfill");
    let ts = Utc::now();

    upsert_strategy_registry_entry(
        &pool,
        &UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.clone(),
            display_name: "Should Not Auto-Promote".to_string(),
            enabled: true,
            kind: "bar_driven".to_string(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: "".to_string(),
        },
    )
    .await?;

    let current = fetch_current_promotion_state(&pool, &strategy_id, "AAPL", 86400).await?;
    assert!(
        current.is_none(),
        "registering + enabling a strategy must never create a promotion row"
    );
    let current_any_symbol =
        fetch_current_promotion_state(&pool, &strategy_id, "SPY", 3600).await?;
    assert!(current_any_symbol.is_none());

    Ok(())
}
