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
    insert_strategy_promotion_transition, insert_strategy_promotion_transition_serialized,
    is_legal_transition, resolve_evidence_lineage, transition_requires_evidence,
    upsert_strategy_registry_entry, InsertStrategyPromotionTransitionArgs, PromotionReasonCode,
    TransitionInsertOutcome, UpsertStrategyRegistryArgs, ENV_DB_URL, PROMOTION_STATE_ACTIVE_PAPER,
    PROMOTION_STATE_DEMOTED, PROMOTION_STATE_PAPER_APPROVED, PROMOTION_STATE_REJECTED,
    PROMOTION_STATE_RETIRED, PROMOTION_STATE_SHADOW_APPROVED,
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
        parent_transition_id: None,
        evidence_transition_id: None,
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

    // An active_paper row NOT past expiry, and already effective, must be
    // tradable -- evaluated at a time between this row's effective_at_utc
    // (now - 2h) and expires_at_utc (now - 1h), not before the former
    // (STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01 Phase C: a future- or
    // not-yet-effective evaluation point must not be conflated with
    // "tradable").
    let (tradable_before_expiry, reason_before_expiry) =
        evaluate_promotion_tradability(Some(&current), now - Duration::minutes(90));
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

// ---------------------------------------------------------------------------
// STRATEGY-PROMOTION-REGISTRY-CLOSURE-REPAIR-01
// ---------------------------------------------------------------------------
//
// Repairs five defects found in the original CLOSED_LOCAL patch group (see
// docs/specs/strategy_promotion_registry_closure_repair_01a_audit.md):
// 1. future-effective transitions becoming active prematurely
// 2. concurrent transitions branching history
// 3. evidence provenance lost past shadow_approved
// 4. no explicit paper-only runtime authorization boundary (proven in
//    mqk-daemon's scenario_strategy_promotion_runtime_gate_01.rs)
// 5. incorrect CLOSED_LOCAL disposition (docs-only; see closure doc)

/// Defect 1 (pure, no DB): an `active_paper` row whose `effective_at_utc`
/// is in the future must not be tradable before that time, even though it
/// is the identity's only/current transition.
#[test]
fn future_effective_active_paper_not_yet_tradable() {
    let now = Utc::now();
    let record = mqk_db::StrategyPromotionTransitionRecord {
        transition_id: Uuid::new_v4(),
        strategy_id: "future_eff_test".to_string(),
        symbol: "AAPL".to_string(),
        timeframe_secs: 86400,
        config_fingerprint: None,
        config_identity_status: "unavailable_in_current_runtime".to_string(),
        previous_state: Some(PROMOTION_STATE_PAPER_APPROVED.to_string()),
        new_state: PROMOTION_STATE_ACTIVE_PAPER.to_string(),
        parent_transition_id: None,
        evidence_transition_id: None,
        evidence_review_id: None,
        evidence_scanner_scan_id: None,
        evidence_git_hash: None,
        evidence_artifact_path: None,
        evidence_fingerprint: None,
        effective_at_utc: now + Duration::hours(1),
        expires_at_utc: None,
        initiated_by: "test".to_string(),
        reason: String::new(),
        created_at_utc: now,
    };

    let (tradable, reason) = evaluate_promotion_tradability(Some(&record), now);
    assert!(
        !tradable,
        "a future-effective active_paper row must not be tradable before its effective time"
    );
    assert_eq!(reason, PromotionReasonCode::PromotionNotYetEffective);

    // Once evaluation time reaches the effective time, it becomes tradable.
    let (tradable_after, reason_after) =
        evaluate_promotion_tradability(Some(&record), now + Duration::hours(2));
    assert!(tradable_after);
    assert_eq!(reason_after, PromotionReasonCode::PromotionActive);
}

/// Defect 1 (pure, no DB): expiry uses `<=`, not `<` -- a transition
/// expiring exactly at the evaluation instant is expired, not tradable.
#[test]
fn expiry_exactly_at_boundary_is_expired() {
    let now = Utc::now();
    let record = mqk_db::StrategyPromotionTransitionRecord {
        transition_id: Uuid::new_v4(),
        strategy_id: "expiry_boundary_test".to_string(),
        symbol: "AAPL".to_string(),
        timeframe_secs: 86400,
        config_fingerprint: None,
        config_identity_status: "unavailable_in_current_runtime".to_string(),
        previous_state: Some(PROMOTION_STATE_PAPER_APPROVED.to_string()),
        new_state: PROMOTION_STATE_ACTIVE_PAPER.to_string(),
        parent_transition_id: None,
        evidence_transition_id: None,
        evidence_review_id: None,
        evidence_scanner_scan_id: None,
        evidence_git_hash: None,
        evidence_artifact_path: None,
        evidence_fingerprint: None,
        effective_at_utc: now - Duration::hours(1),
        expires_at_utc: Some(now),
        initiated_by: "test".to_string(),
        reason: String::new(),
        created_at_utc: now - Duration::hours(1),
    };

    let (tradable, reason) = evaluate_promotion_tradability(Some(&record), now);
    assert!(
        !tradable,
        "expires_at_utc == now must classify as expired (<=, not <)"
    );
    assert_eq!(reason, PromotionReasonCode::PromotionExpired);

    // One microsecond before expiry, still tradable.
    let (tradable_before, reason_before) =
        evaluate_promotion_tradability(Some(&record), now - Duration::microseconds(1));
    assert!(tradable_before);
    assert_eq!(reason_before, PromotionReasonCode::PromotionActive);
}

/// Defect 2: two concurrent transitions racing off the same parent for
/// the same identity must never both be accepted -- exactly one succeeds,
/// the other is rejected with a stable `Conflict` outcome.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn concurrent_transitions_from_same_parent_only_one_accepted() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_race");
    let symbol = "RACE";
    let timeframe_secs = 86400;
    let now = Utc::now();

    // Seed the root transition (no-state -> shadow_approved), then advance
    // to paper_approved so both racers attempt a transition *out of*
    // paper_approved (both are legal individually: -> active_paper and
    // -> demoted are both legal from paper_approved).
    let root_id = transition_id_for(&format!("{strategy_id}:{symbol}:race:root"));
    insert_strategy_promotion_transition(
        &pool,
        &make_args(
            root_id,
            &strategy_id,
            symbol,
            timeframe_secs,
            None,
            PROMOTION_STATE_SHADOW_APPROVED,
            now,
            now,
        ),
    )
    .await?;
    let parent_id = transition_id_for(&format!("{strategy_id}:{symbol}:race:parent"));
    insert_strategy_promotion_transition(
        &pool,
        &make_args(
            parent_id,
            &strategy_id,
            symbol,
            timeframe_secs,
            Some(PROMOTION_STATE_SHADOW_APPROVED),
            PROMOTION_STATE_PAPER_APPROVED,
            now + Duration::milliseconds(1),
            now + Duration::milliseconds(1),
        ),
    )
    .await?;

    let racer_args = |transition_id: Uuid, new_state: &str, effective_at: chrono::DateTime<Utc>| {
        let mut args = make_args(
            transition_id,
            &strategy_id,
            symbol,
            timeframe_secs,
            Some(PROMOTION_STATE_PAPER_APPROVED),
            new_state,
            effective_at,
            effective_at,
        );
        args.parent_transition_id = Some(parent_id);
        args
    };

    let args_a = racer_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:race:a")),
        PROMOTION_STATE_ACTIVE_PAPER,
        now + Duration::milliseconds(2),
    );
    let args_b = racer_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:race:b")),
        PROMOTION_STATE_DEMOTED,
        now + Duration::milliseconds(2),
    );

    let pool_a = pool.clone();
    let pool_b = pool.clone();
    let (result_a, result_b) = tokio::join!(
        tokio::spawn(async move {
            insert_strategy_promotion_transition_serialized(&pool_a, &args_a).await
        }),
        tokio::spawn(async move {
            insert_strategy_promotion_transition_serialized(&pool_b, &args_b).await
        }),
    );
    let outcome_a = result_a.expect("task a panicked")?;
    let outcome_b = result_b.expect("task b panicked")?;

    let inserted_count = [&outcome_a, &outcome_b]
        .iter()
        .filter(|o| matches!(o, TransitionInsertOutcome::Inserted(_)))
        .count();
    let conflict_count = [&outcome_a, &outcome_b]
        .iter()
        .filter(|o| matches!(o, TransitionInsertOutcome::Conflict { .. }))
        .count();
    assert_eq!(
        inserted_count, 1,
        "exactly one of two concurrent same-parent transitions must be accepted; \
         outcomes: {outcome_a:?} / {outcome_b:?}"
    );
    assert_eq!(
        conflict_count, 1,
        "the other concurrent transition must be rejected as a conflict; \
         outcomes: {outcome_a:?} / {outcome_b:?}"
    );

    // History must be linear: exactly one child of `parent_id`, not two.
    let history = fetch_promotion_history(&pool, &strategy_id, symbol, timeframe_secs, 100).await?;
    let children_of_parent = history
        .iter()
        .filter(|r| r.parent_transition_id == Some(parent_id))
        .count();
    assert_eq!(
        children_of_parent, 1,
        "history must never branch: exactly one child of the raced parent"
    );

    Ok(())
}

/// Defect 2: a transition built from a now-stale parent (a concurrent
/// transition already advanced the identity) is rejected deterministically
/// with `Conflict`, even without real thread-level concurrency -- proves
/// the re-read-inside-the-lock check, not just the race outcome above.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn stale_parent_rejected_as_conflict() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_stale_parent");
    let symbol = "STALE";
    let timeframe_secs = 3600;
    let now = Utc::now();

    let root_id = transition_id_for(&format!("{strategy_id}:{symbol}:stale:root"));
    let root_args = make_args(
        root_id,
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    insert_strategy_promotion_transition_serialized(&pool, &root_args).await?;

    // Advance the identity for real: shadow_approved -> paper_approved.
    let mut advance_args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:stale:advance")),
        &strategy_id,
        symbol,
        timeframe_secs,
        Some(PROMOTION_STATE_SHADOW_APPROVED),
        PROMOTION_STATE_PAPER_APPROVED,
        now + Duration::milliseconds(1),
        now + Duration::milliseconds(1),
    );
    advance_args.parent_transition_id = Some(root_id);
    let outcome = insert_strategy_promotion_transition_serialized(&pool, &advance_args).await?;
    assert!(matches!(outcome, TransitionInsertOutcome::Inserted(_)));

    // A caller that still believes `root_id` is current (stale read) tries
    // to reject the strategy -- must be refused as a conflict, not silently
    // inserted alongside the real current (paper_approved) state.
    let mut stale_args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:stale:rejected")),
        &strategy_id,
        symbol,
        timeframe_secs,
        Some(PROMOTION_STATE_SHADOW_APPROVED),
        PROMOTION_STATE_REJECTED,
        now + Duration::milliseconds(2),
        now + Duration::milliseconds(2),
    );
    stale_args.parent_transition_id = Some(root_id);
    let stale_outcome = insert_strategy_promotion_transition_serialized(&pool, &stale_args).await?;
    match stale_outcome {
        TransitionInsertOutcome::Conflict { current } => {
            let current = current.expect("conflict must carry the actual current record");
            assert_eq!(current.new_state, PROMOTION_STATE_PAPER_APPROVED);
        }
        other => panic!("expected Conflict, got {other:?}"),
    }

    // Confirm no branching row was created.
    let history = fetch_promotion_history(&pool, &strategy_id, symbol, timeframe_secs, 100).await?;
    assert_eq!(
        history.len(),
        2,
        "the rejected stale-parent attempt must not have created a third row"
    );

    Ok(())
}

/// Defect 2 (idempotency preserved): replaying the exact same
/// `transition_id` through the serialized insert path is a no-op,
/// identical in spirit to the plain insert function's idempotency.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn serialized_insert_duplicate_replay_is_idempotent() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_serialized_dup");
    let symbol = "DUP";
    let timeframe_secs = 900;
    let now = Utc::now();

    let args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:dup")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );

    let first = insert_strategy_promotion_transition_serialized(&pool, &args).await?;
    assert!(matches!(first, TransitionInsertOutcome::Inserted(_)));

    let second = insert_strategy_promotion_transition_serialized(&pool, &args).await?;
    match second {
        TransitionInsertOutcome::Duplicate(record) => {
            assert_eq!(record.transition_id, args.transition_id);
        }
        other => panic!("expected Duplicate, got {other:?}"),
    }

    let history = fetch_promotion_history(&pool, &strategy_id, symbol, timeframe_secs, 100).await?;
    assert_eq!(
        history.len(),
        1,
        "a replayed duplicate must not create a second row"
    );

    Ok(())
}

/// Defect 3: `shadow_approved -> paper_approved -> active_paper` retains
/// the exact same evidence lineage throughout -- the current `active_paper`
/// record resolves back to the original `shadow_approved` transition's
/// evidence via `resolve_evidence_lineage`, even though neither
/// intermediate transition carries its own evidence_* columns.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn evidence_lineage_carried_forward_through_paper_and_active() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_evidence_lineage");
    let symbol = "EVID";
    let timeframe_secs = 86400;
    let now = Utc::now();

    // Root transition: itself evidence-bearing.
    let root_id = transition_id_for(&format!("{strategy_id}:{symbol}:evid:root"));
    assert!(transition_requires_evidence(
        None,
        PROMOTION_STATE_SHADOW_APPROVED
    ));
    let mut root_args = make_args(
        root_id,
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    root_args.evidence_transition_id = Some(root_id); // self: this row IS the evidence.
    root_args.evidence_review_id = Some("review-001".to_string());
    root_args.evidence_scanner_scan_id = Some("scan-001".to_string());
    root_args.evidence_git_hash = Some("deadbeef01".to_string());
    root_args.evidence_artifact_path = Some("exports/strategy_reviews/evid".to_string());
    root_args.evidence_fingerprint = Some("fingerprint-001".to_string());
    let root_outcome = insert_strategy_promotion_transition_serialized(&pool, &root_args).await?;
    assert!(matches!(root_outcome, TransitionInsertOutcome::Inserted(_)));

    // shadow_approved -> paper_approved: no fresh evidence required; must
    // inherit evidence_transition_id from the root (never re-derive it).
    assert!(!transition_requires_evidence(
        Some(PROMOTION_STATE_SHADOW_APPROVED),
        PROMOTION_STATE_PAPER_APPROVED
    ));
    let mut paper_args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:evid:paper")),
        &strategy_id,
        symbol,
        timeframe_secs,
        Some(PROMOTION_STATE_SHADOW_APPROVED),
        PROMOTION_STATE_PAPER_APPROVED,
        now + Duration::milliseconds(1),
        now + Duration::milliseconds(1),
    );
    paper_args.parent_transition_id = Some(root_id);
    paper_args.evidence_transition_id = Some(root_id); // inherited, not self.
    paper_args.evidence_review_id = None;
    paper_args.evidence_scanner_scan_id = None;
    paper_args.evidence_git_hash = None;
    paper_args.evidence_artifact_path = None;
    paper_args.evidence_fingerprint = None;
    let paper_outcome = insert_strategy_promotion_transition_serialized(&pool, &paper_args).await?;
    let TransitionInsertOutcome::Inserted(paper_record) = paper_outcome else {
        panic!("expected Inserted");
    };

    // paper_approved -> active_paper: same inheritance.
    assert!(!transition_requires_evidence(
        Some(PROMOTION_STATE_PAPER_APPROVED),
        PROMOTION_STATE_ACTIVE_PAPER
    ));
    let mut active_args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:evid:active")),
        &strategy_id,
        symbol,
        timeframe_secs,
        Some(PROMOTION_STATE_PAPER_APPROVED),
        PROMOTION_STATE_ACTIVE_PAPER,
        now + Duration::milliseconds(2),
        now + Duration::milliseconds(2),
    );
    active_args.parent_transition_id = Some(paper_record.transition_id);
    active_args.evidence_transition_id = paper_record.evidence_transition_id;
    active_args.evidence_review_id = None;
    active_args.evidence_scanner_scan_id = None;
    active_args.evidence_git_hash = None;
    active_args.evidence_artifact_path = None;
    active_args.evidence_fingerprint = None;
    let active_outcome =
        insert_strategy_promotion_transition_serialized(&pool, &active_args).await?;
    let TransitionInsertOutcome::Inserted(active_record) = active_outcome else {
        panic!("expected Inserted");
    };

    // The current active_paper record's OWN evidence_* columns are null --
    // this is the reproduced defect (see audit doc item 3) -- but
    // resolve_evidence_lineage must still recover the exact original
    // evidence from the root transition.
    assert!(active_record.evidence_review_id.is_none());
    let resolved = resolve_evidence_lineage(&pool, &active_record)
        .await?
        .expect("evidence lineage must resolve for a legally-chained transition");
    assert_eq!(resolved.transition_id, root_id);
    assert_eq!(resolved.evidence_review_id.as_deref(), Some("review-001"));
    assert_eq!(
        resolved.evidence_scanner_scan_id.as_deref(),
        Some("scan-001")
    );
    assert_eq!(resolved.evidence_git_hash.as_deref(), Some("deadbeef01"));
    assert_eq!(
        resolved.evidence_fingerprint.as_deref(),
        Some("fingerprint-001")
    );

    Ok(())
}

/// Defect 3: a `demoted -> shadow_approved` re-approval establishes fresh
/// evidence lineage -- `resolve_evidence_lineage` on the reapproved chain
/// must return the NEW evidence transition, not the original one.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn reapproval_from_demoted_establishes_fresh_evidence_lineage() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_reapproval_evidence");
    let symbol = "REAP";
    let timeframe_secs = 3600;
    let now = Utc::now();

    // Original evidence-bearing root.
    let root_id = transition_id_for(&format!("{strategy_id}:{symbol}:reap:root"));
    let mut root_args = make_args(
        root_id,
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    root_args.evidence_transition_id = Some(root_id);
    root_args.evidence_review_id = Some("review-original".to_string());
    insert_strategy_promotion_transition_serialized(&pool, &root_args).await?;

    // shadow_approved -> paper_approved -> demoted (all inherit root evidence).
    let mut paper_args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:reap:paper")),
        &strategy_id,
        symbol,
        timeframe_secs,
        Some(PROMOTION_STATE_SHADOW_APPROVED),
        PROMOTION_STATE_PAPER_APPROVED,
        now + Duration::milliseconds(1),
        now + Duration::milliseconds(1),
    );
    paper_args.parent_transition_id = Some(root_id);
    paper_args.evidence_transition_id = Some(root_id);
    // Not itself evidence-bearing: clear the make_args() defaults so
    // resolve_evidence_lineage's self-check doesn't short-circuit here.
    paper_args.evidence_review_id = None;
    paper_args.evidence_scanner_scan_id = None;
    paper_args.evidence_git_hash = None;
    paper_args.evidence_artifact_path = None;
    paper_args.evidence_fingerprint = None;
    let paper_outcome = insert_strategy_promotion_transition_serialized(&pool, &paper_args).await?;
    let TransitionInsertOutcome::Inserted(paper_record) = paper_outcome else {
        panic!("expected Inserted");
    };

    let mut demoted_args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:reap:demoted")),
        &strategy_id,
        symbol,
        timeframe_secs,
        Some(PROMOTION_STATE_PAPER_APPROVED),
        PROMOTION_STATE_DEMOTED,
        now + Duration::milliseconds(2),
        now + Duration::milliseconds(2),
    );
    demoted_args.parent_transition_id = Some(paper_record.transition_id);
    demoted_args.evidence_transition_id = paper_record.evidence_transition_id;
    // Not itself evidence-bearing either (demotion never carries fresh
    // evidence) -- same clearing as paper_args above.
    demoted_args.evidence_review_id = None;
    demoted_args.evidence_scanner_scan_id = None;
    demoted_args.evidence_git_hash = None;
    demoted_args.evidence_artifact_path = None;
    demoted_args.evidence_fingerprint = None;
    let demoted_outcome =
        insert_strategy_promotion_transition_serialized(&pool, &demoted_args).await?;
    let TransitionInsertOutcome::Inserted(demoted_record) = demoted_outcome else {
        panic!("expected Inserted");
    };

    // Reapproval: demoted -> shadow_approved, WITH fresh evidence.
    assert!(transition_requires_evidence(
        Some(PROMOTION_STATE_DEMOTED),
        PROMOTION_STATE_SHADOW_APPROVED
    ));
    let reapproval_id = transition_id_for(&format!("{strategy_id}:{symbol}:reap:reapproved"));
    let mut reapproval_args = make_args(
        reapproval_id,
        &strategy_id,
        symbol,
        timeframe_secs,
        Some(PROMOTION_STATE_DEMOTED),
        PROMOTION_STATE_SHADOW_APPROVED,
        now + Duration::milliseconds(3),
        now + Duration::milliseconds(3),
    );
    reapproval_args.parent_transition_id = Some(demoted_record.transition_id);
    reapproval_args.evidence_transition_id = Some(reapproval_id); // fresh: self, not inherited.
    reapproval_args.evidence_review_id = Some("review-reapproved".to_string());
    let reapproval_outcome =
        insert_strategy_promotion_transition_serialized(&pool, &reapproval_args).await?;
    let TransitionInsertOutcome::Inserted(reapproval_record) = reapproval_outcome else {
        panic!("expected Inserted");
    };

    let resolved = resolve_evidence_lineage(&pool, &reapproval_record)
        .await?
        .expect("fresh evidence lineage must resolve");
    assert_eq!(
        resolved.transition_id, reapproval_id,
        "reapproval must establish itself as the new evidence root, not the original"
    );
    assert_eq!(
        resolved.evidence_review_id.as_deref(),
        Some("review-reapproved")
    );

    // The demoted record (still in history) still resolves to the
    // *original* evidence -- demotion never rewrites past lineage.
    let demoted_resolved = resolve_evidence_lineage(&pool, &demoted_record)
        .await?
        .expect("demoted record must still resolve its original evidence");
    assert_eq!(demoted_resolved.transition_id, root_id);
    assert_eq!(
        demoted_resolved.evidence_review_id.as_deref(),
        Some("review-original")
    );

    Ok(())
}
