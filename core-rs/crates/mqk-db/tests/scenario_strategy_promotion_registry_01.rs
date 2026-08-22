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
    evaluate_promotion_tradability, fetch_current_promotion_state,
    fetch_promotion_transition_lineage_v3, fetch_promotion_history,
    insert_strategy_promotion_transition, insert_strategy_promotion_transition_serialized,
    is_legal_transition, resolve_evidence_lineage, transition_requires_evidence,
    upsert_strategy_registry_entry, InsertStrategyPromotionTransitionArgs, PromotionEvidenceLineageV3,
    PromotionReasonCode, TransitionInsertOutcome, UpsertStrategyRegistryArgs, ENV_DB_URL,
    PROMOTION_STATE_ACTIVE_PAPER, PROMOTION_STATE_DEMOTED, PROMOTION_STATE_PAPER_APPROVED,
    PROMOTION_STATE_REJECTED, PROMOTION_STATE_RETIRED, PROMOTION_STATE_SHADOW_APPROVED,
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

/// Defect 3: a valid-shaped 64-lowercase-hex legacy fingerprint fixture --
/// production insert paths now refuse a fresh evidence-bearing transition
/// whose `evidence_fingerprint` isn't this shape, so every test fixture in
/// this file that flows through the validated insert paths must use one.
const FIXTURE_LEGACY_FINGERPRINT: &str =
    "6f0ef4b13f13d6e2df411454ab0475bc608e59fccce2d226379b97a2e2d63c11";
/// Defect 3: a valid-shaped 64-lowercase-hex v2 fingerprint fixture,
/// distinct from [`FIXTURE_LEGACY_FINGERPRINT`] so the two never
/// accidentally collide in an equality assertion.
const FIXTURE_V2_FINGERPRINT: &str =
    "13b520c87278ee76536032a742a025694975698b866eade5ee5386e226559ead";

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
        // Defect 3: a fresh evidence-bearing insert now requires both
        // fingerprints, each exactly 64 lowercase hex characters -- this
        // fixture default is a genuine fresh (production-shaped) bundle;
        // tests that specifically need a legacy (pre-0058, v2-absent) row
        // seed it via direct SQL (`seed_legacy_v2_absent_transition_row`
        // below) -- no public library function can construct that shape.
        evidence_fingerprint: Some(FIXTURE_LEGACY_FINGERPRINT.to_string()),
        evidence_fingerprint_v2: Some(FIXTURE_V2_FINGERPRINT.to_string()),
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
        evidence_fingerprint_v2: None,
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
        evidence_fingerprint_v2: None,
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
            insert_strategy_promotion_transition_serialized(&pool_a, &args_a, None).await
        }),
        tokio::spawn(async move {
            insert_strategy_promotion_transition_serialized(&pool_b, &args_b, None).await
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
    insert_strategy_promotion_transition_serialized(&pool, &root_args, None).await?;

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
    let outcome = insert_strategy_promotion_transition_serialized(&pool, &advance_args, None).await?;
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
    let stale_outcome = insert_strategy_promotion_transition_serialized(&pool, &stale_args, None).await?;
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

    let first = insert_strategy_promotion_transition_serialized(&pool, &args, None).await?;
    assert!(matches!(first, TransitionInsertOutcome::Inserted(_)));

    let second = insert_strategy_promotion_transition_serialized(&pool, &args, None).await?;
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

// ---------------------------------------------------------------------------
// PROMOTION-LINEAGE-ATOMICITY-01: Research/Backtest evidence lineage is
// written durably atomically with the transition row itself, never as a
// best-effort follow-up step.
// ---------------------------------------------------------------------------

fn sample_lineage(trial_id: &str, backtest_run_id: Uuid) -> PromotionEvidenceLineageV3 {
    PromotionEvidenceLineageV3 {
        research_trial_id: Some(trial_id.to_string()),
        research_economic_eval_id: Some(format!("{trial_id}_eval")),
        research_deflated_sharpe_ratio: Some(0.72),
        research_probability_backtest_overfitting: Some(0.11),
        backtest_run_id: Some(backtest_run_id),
        research_judge_artifact_sha256: Some(format!("{trial_id}_judge_sha256")),
        stress_protocol_version: Some("bkt_stress_suite_v1".to_string()),
        stress_artifact_sha256: Some(format!("{trial_id}_stress_sha256")),
        robustness_protocol_version: Some("bkt_robustness_gauntlet_v1".to_string()),
        finalized_robustness_artifact_sha256: Some(format!("{trial_id}_robustness_sha256")),
        promotion_policy_fingerprint: Some(format!("{trial_id}_policy_fingerprint")),
    }
}

/// A fresh evidence-bearing transition insert with `Some(lineage)` commits
/// the lineage in the SAME row, readable immediately via
/// `fetch_promotion_transition_lineage_v3` -- not a separate, independently
/// failable follow-up write.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn lineage_atomicity_fresh_insert_commits_lineage_with_transition() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_lineage_atomic_fresh");
    let symbol = "LNAT";
    let timeframe_secs = 900;
    let now = Utc::now();

    let args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:lineage_fresh")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    let lineage = sample_lineage("trial-fresh-001", Uuid::new_v4());

    let outcome =
        insert_strategy_promotion_transition_serialized(&pool, &args, Some(&lineage)).await?;
    assert!(matches!(outcome, TransitionInsertOutcome::Inserted(_)));

    let recorded = fetch_promotion_transition_lineage_v3(&pool, args.transition_id)
        .await?
        .expect("lineage must be readable for the transition_id just inserted");
    assert_eq!(recorded, lineage, "recorded lineage must exactly match what was supplied");

    Ok(())
}

/// A non-evidence-bearing transition inserted with `None` lineage (the
/// common case -- every non-`shadow_approved` transition) leaves the
/// lineage columns genuinely unset, never a fabricated/defaulted value.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn lineage_atomicity_none_leaves_columns_unset() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_lineage_atomic_none");
    let symbol = "LNNO";
    let timeframe_secs = 900;
    let now = Utc::now();

    let args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:lineage_none")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );

    let outcome = insert_strategy_promotion_transition_serialized(&pool, &args, None).await?;
    assert!(matches!(outcome, TransitionInsertOutcome::Inserted(_)));

    let recorded = fetch_promotion_transition_lineage_v3(&pool, args.transition_id)
        .await?
        .expect("row must exist");
    assert!(
        recorded.research_trial_id.is_none() && recorded.backtest_run_id.is_none(),
        "no lineage was supplied -- columns must stay genuinely unset, got {recorded:?}"
    );

    Ok(())
}

/// PROMOTION-EVIDENCE-LINEAGE-V3 LEGACY DUPLICATE RULE (REQUIRED NEGATIVE
/// CONTROL): a transition_id that was originally committed with NO lineage
/// (predates complete lineage, or was never evidence-bearing) must NEVER be
/// retroactively backfilled by a later "duplicate" request that happens to
/// supply valid lineage -- that would falsely imply the historical decision
/// was authorized by evidence it never actually had. The insert must be
/// refused (`Err`), and the row's lineage columns must remain exactly as
/// they were: genuinely NULL, not silently populated.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn lineage_atomicity_duplicate_of_null_lineage_row_refuses_backfill() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_lineage_legacy_null_dup");
    let symbol = "LNLG";
    let timeframe_secs = 900;
    let now = Utc::now();

    let args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:lineage_legacy_null")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );

    // Original commit: the identical request, but with NO lineage supplied
    // -- simulating a historical evidence-requiring transition that
    // predates complete lineage tracking.
    let first = insert_strategy_promotion_transition_serialized(&pool, &args, None).await?;
    assert!(matches!(first, TransitionInsertOutcome::Inserted(_)));
    let before = fetch_promotion_transition_lineage_v3(&pool, args.transition_id)
        .await?
        .expect("row must exist after first insert");
    assert!(before.research_trial_id.is_none(), "precondition: lineage must start NULL");

    // A later "duplicate" request for the EXACT SAME transition_id, now
    // supplying genuinely valid lineage -- must be refused, never silently
    // attached.
    let lineage = sample_lineage("trial-legacy-backfill-attempt-001", Uuid::new_v4());
    let err = insert_strategy_promotion_transition_serialized(&pool, &args, Some(&lineage))
        .await
        .expect_err(
            "a duplicate request supplying lineage for a historically NULL-lineage row must be \
             refused, never silently backfilled",
        );
    assert!(
        format!("{err:#}").contains("duplicate of a historical transition with NO recorded evidence lineage"),
        "got: {err:#}"
    );

    // The historical row's lineage must remain exactly as it was: NULL.
    let after = fetch_promotion_transition_lineage_v3(&pool, args.transition_id)
        .await?
        .expect("row must still exist");
    assert!(
        after.research_trial_id.is_none() && after.backtest_run_id.is_none(),
        "the refused lineage must never have been backfilled onto the historical row, got {after:?}"
    );
    assert_eq!(before, after, "lineage state must be byte-identical before and after the refused attempt");

    Ok(())
}

/// Retry/idempotency: replaying the exact same evidence-bearing transition
/// with the exact same lineage a second time is accepted (`Duplicate`) and
/// the recorded lineage is unchanged.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn lineage_atomicity_duplicate_replay_with_matching_lineage_is_idempotent(
) -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_lineage_atomic_replay_match");
    let symbol = "LNRM";
    let timeframe_secs = 900;
    let now = Utc::now();

    let args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:lineage_replay_match")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    let lineage = sample_lineage("trial-replay-match-001", Uuid::new_v4());

    let first =
        insert_strategy_promotion_transition_serialized(&pool, &args, Some(&lineage)).await?;
    assert!(matches!(first, TransitionInsertOutcome::Inserted(_)));

    let second =
        insert_strategy_promotion_transition_serialized(&pool, &args, Some(&lineage)).await?;
    assert!(matches!(second, TransitionInsertOutcome::Duplicate(_)));

    let recorded = fetch_promotion_transition_lineage_v3(&pool, args.transition_id)
        .await?
        .expect("lineage must still be present after the idempotent replay");
    assert_eq!(recorded, lineage);

    Ok(())
}

/// Mismatched lineage cannot replace existing authoritative lineage: a
/// same-`transition_id` replay of an otherwise-identical payload but with a
/// DIFFERENT lineage is refused (`Err`), and the originally-recorded
/// lineage remains untouched -- proving both that no forged lineage can
/// silently overwrite the real one, and that a lineage-write failure never
/// leaves a transition (or its lineage) in an inconsistent state.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn lineage_atomicity_mismatched_replay_lineage_is_rejected() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("promo_lineage_atomic_replay_mismatch");
    let symbol = "LNRX";
    let timeframe_secs = 900;
    let now = Utc::now();

    let args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:lineage_replay_mismatch")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    let original_lineage = sample_lineage("trial-original-001", Uuid::new_v4());
    let forged_lineage = sample_lineage("trial-FORGED-999", Uuid::new_v4());

    let first = insert_strategy_promotion_transition_serialized(&pool, &args, Some(&original_lineage))
        .await?;
    assert!(matches!(first, TransitionInsertOutcome::Inserted(_)));

    let err = insert_strategy_promotion_transition_serialized(&pool, &args, Some(&forged_lineage))
        .await
        .expect_err("a divergent lineage for an already-lineage-bound transition_id must be refused");
    assert!(
        format!("{err:#}").contains("refusing to overwrite authoritative lineage"),
        "got: {err:#}"
    );

    let recorded = fetch_promotion_transition_lineage_v3(&pool, args.transition_id)
        .await?
        .expect("original lineage row must still exist");
    assert_eq!(
        recorded, original_lineage,
        "the rejected forged lineage must never have replaced the original"
    );

    Ok(())
}

/// Cross-candidate lineage independence: lineage recorded for one
/// transition_id is never readable/attributable to a different, unrelated
/// transition_id -- each row's lineage is exactly what was supplied for
/// that row, never inherited, defaulted, or contaminated across rows.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn lineage_atomicity_cross_transition_lineage_is_independent() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id_a = unique_id("promo_lineage_cross_a");
    let strategy_id_b = unique_id("promo_lineage_cross_b");
    let symbol = "LNCX";
    let timeframe_secs = 900;
    let now = Utc::now();

    let args_a = make_args(
        transition_id_for(&format!("{strategy_id_a}:{symbol}:lineage_cross_a")),
        &strategy_id_a,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    let lineage_a = sample_lineage("trial-cross-a-001", Uuid::new_v4());
    insert_strategy_promotion_transition_serialized(&pool, &args_a, Some(&lineage_a)).await?;

    let args_b = make_args(
        transition_id_for(&format!("{strategy_id_b}:{symbol}:lineage_cross_b")),
        &strategy_id_b,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now + Duration::milliseconds(1),
        now + Duration::milliseconds(1),
    );
    // B is inserted with NO lineage of its own.
    insert_strategy_promotion_transition_serialized(&pool, &args_b, None).await?;

    let recorded_a = fetch_promotion_transition_lineage_v3(&pool, args_a.transition_id)
        .await?
        .expect("A's row must exist");
    assert_eq!(recorded_a, lineage_a);

    let recorded_b = fetch_promotion_transition_lineage_v3(&pool, args_b.transition_id)
        .await?
        .expect("B's row must exist");
    assert!(
        recorded_b.research_trial_id.is_none(),
        "B must not inherit A's lineage: got {recorded_b:?}"
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
    root_args.evidence_fingerprint =
        Some("67684119ecbfa8391eee4cc571d59adab9c6dad3d41f7788a984b36da1028dc5".to_string());
    let root_outcome = insert_strategy_promotion_transition_serialized(&pool, &root_args, None).await?;
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
    paper_args.evidence_fingerprint_v2 = None;
    let paper_outcome = insert_strategy_promotion_transition_serialized(&pool, &paper_args, None).await?;
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
    active_args.evidence_fingerprint_v2 = None;
    let active_outcome =
        insert_strategy_promotion_transition_serialized(&pool, &active_args, None).await?;
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
        Some("67684119ecbfa8391eee4cc571d59adab9c6dad3d41f7788a984b36da1028dc5")
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
    insert_strategy_promotion_transition_serialized(&pool, &root_args, None).await?;

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
    paper_args.evidence_fingerprint_v2 = None;
    let paper_outcome = insert_strategy_promotion_transition_serialized(&pool, &paper_args, None).await?;
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
    demoted_args.evidence_fingerprint_v2 = None;
    let demoted_outcome =
        insert_strategy_promotion_transition_serialized(&pool, &demoted_args, None).await?;
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
        insert_strategy_promotion_transition_serialized(&pool, &reapproval_args, None).await?;
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

// ---------------------------------------------------------------------------
// DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-FINAL-FOUNDATION-PROVENANCE-AND-
// ARTIFACT-HARDENING: evidence_fingerprint_v2 insertion hardening.
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn malformed_v2_fingerprint_is_refused_on_insert() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("v2_malformed");
    let symbol = "AAPL";
    let timeframe_secs = 86_400;
    let now = Utc::now();

    let mut args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:malformed_v2")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    args.evidence_fingerprint_v2 = Some("not-a-valid-hex-fingerprint".to_string());

    let err = insert_strategy_promotion_transition_serialized(&pool, &args, None)
        .await
        .expect_err("a malformed v2 fingerprint must be refused before it reaches storage");
    assert!(
        format!("{err:#}").contains("64 lowercase hex"),
        "got: {err:#}"
    );

    // Never actually inserted -- no row exists for this identity.
    let current =
        fetch_current_promotion_state(&pool, &strategy_id, symbol, timeframe_secs).await?;
    assert!(
        current.is_none(),
        "a refused insert must leave zero durable trace"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn wrong_length_v2_fingerprint_is_refused_on_insert() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("v2_wronglen");
    let symbol = "AAPL";
    let timeframe_secs = 86_400;
    let now = Utc::now();

    let mut args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:wrong_len_v2")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    // 63 hex chars, not 64.
    args.evidence_fingerprint_v2 = Some("a".repeat(63));

    let err = insert_strategy_promotion_transition(&pool, &args)
        .await
        .expect_err("a wrong-length v2 fingerprint must be refused");
    assert!(
        format!("{err:#}").contains("64 lowercase hex"),
        "got: {err:#}"
    );

    Ok(())
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn valid_v2_fingerprint_inserts_and_replays_idempotently() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("v2_valid");
    let symbol = "AAPL";
    let timeframe_secs = 86_400;
    let now = Utc::now();
    let valid_v2 = "b".repeat(64);

    let mut args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:valid_v2")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    args.evidence_fingerprint_v2 = Some(valid_v2.clone());

    let outcome = insert_strategy_promotion_transition_serialized(&pool, &args, None).await?;
    let TransitionInsertOutcome::Inserted(record) = outcome else {
        panic!("expected Inserted");
    };
    assert_eq!(
        record.evidence_fingerprint_v2.as_deref(),
        Some(valid_v2.as_str())
    );

    // Exact replay (same transition_id, same content) is idempotent -- no
    // second row, no error, and the v2 fingerprint round-trips unchanged.
    let replay_outcome = insert_strategy_promotion_transition_serialized(&pool, &args, None).await?;
    match replay_outcome {
        TransitionInsertOutcome::Duplicate(dup) => {
            assert_eq!(
                dup.evidence_fingerprint_v2.as_deref(),
                Some(valid_v2.as_str())
            );
        }
        other => panic!("expected Duplicate on exact replay, got {other:?}"),
    }

    let history = fetch_promotion_history(&pool, &strategy_id, symbol, timeframe_secs, 10).await?;
    assert_eq!(
        history.len(),
        1,
        "a replayed insert must never produce a second row (collision proof)"
    );

    Ok(())
}

/// Defect 3, test 2: v2 present with legacy `evidence_fingerprint` absent is
/// refused just as symmetrically as the reverse (legacy without v2).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn v2_fingerprint_without_legacy_is_refused_on_insert() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("v2_no_legacy");
    let symbol = "AAPL";
    let timeframe_secs = 86_400;
    let now = Utc::now();

    let mut args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:v2_no_legacy")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    args.evidence_fingerprint = None;

    let err = insert_strategy_promotion_transition_serialized(&pool, &args, None)
        .await
        .expect_err("a fresh evidence-bearing insert with no legacy fingerprint must be refused");
    assert!(
        format!("{err:#}").contains("all-present or all-absent"),
        "got: {err:#}"
    );

    Ok(())
}

/// Defect 3, test 3: any partial evidence bundle (here: only `evidence_review_id`
/// present, everything else absent, including both fingerprints) is refused.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn partial_evidence_bundle_is_refused_on_insert() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("v2_partial");
    let symbol = "AAPL";
    let timeframe_secs = 86_400;
    let now = Utc::now();

    let mut args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:partial")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    args.evidence_scanner_scan_id = None;
    args.evidence_git_hash = None;
    args.evidence_artifact_path = None;
    args.evidence_fingerprint = None;
    args.evidence_fingerprint_v2 = None;
    assert!(args.evidence_review_id.is_some(), "only review_id present");

    let err = insert_strategy_promotion_transition_serialized(&pool, &args, None)
        .await
        .expect_err("a partial evidence bundle must be refused");
    assert!(
        format!("{err:#}").contains("all-present or all-absent"),
        "got: {err:#}"
    );

    Ok(())
}

/// Defect 3, tests 7 and 9: the same `transition_id` submitted a second time
/// with a divergent `evidence_fingerprint_v2` is a `Conflict`, never a
/// `Duplicate` -- and the original durable row is preserved unchanged.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn same_transition_id_different_v2_is_a_collision_not_duplicate() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("collide_v2");
    let symbol = "AAPL";
    let timeframe_secs = 86_400;
    let now = Utc::now();
    let tid = transition_id_for(&format!("{strategy_id}:{symbol}:collide_v2"));

    let first_args = make_args(
        tid,
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    let first = insert_strategy_promotion_transition_serialized(&pool, &first_args, None).await?;
    assert!(matches!(first, TransitionInsertOutcome::Inserted(_)));

    let mut second_args = first_args.clone();
    second_args.evidence_fingerprint_v2 = Some("c".repeat(64));
    let second = insert_strategy_promotion_transition_serialized(&pool, &second_args, None).await?;
    match second {
        TransitionInsertOutcome::Conflict { current } => {
            let current = current.expect("existing row must be returned on collision");
            assert_eq!(
                current.evidence_fingerprint_v2.as_deref(),
                Some(FIXTURE_V2_FINGERPRINT),
                "the original durable row must be preserved unchanged, not overwritten"
            );
        }
        other => panic!("expected Conflict on divergent same-transition_id insert, got {other:?}"),
    }

    let history = fetch_promotion_history(&pool, &strategy_id, symbol, timeframe_secs, 10).await?;
    assert_eq!(
        history.len(),
        1,
        "a collision must never produce a second row"
    );
    assert_eq!(
        history[0].evidence_fingerprint_v2.as_deref(),
        Some(FIXTURE_V2_FINGERPRINT)
    );

    Ok(())
}

/// Defect 3, test 8: the same `transition_id` submitted a second time with a
/// divergent evidence field *other than* v2 (here: `evidence_git_hash`) is
/// also a `Conflict`, not just a v2-specific special case.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn same_transition_id_different_evidence_field_is_a_collision() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("collide_git_hash");
    let symbol = "AAPL";
    let timeframe_secs = 86_400;
    let now = Utc::now();
    let tid = transition_id_for(&format!("{strategy_id}:{symbol}:collide_git_hash"));

    let first_args = make_args(
        tid,
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    let first = insert_strategy_promotion_transition_serialized(&pool, &first_args, None).await?;
    assert!(matches!(first, TransitionInsertOutcome::Inserted(_)));

    let mut second_args = first_args.clone();
    second_args.evidence_git_hash = Some("divergent-git-hash".to_string());
    let second = insert_strategy_promotion_transition_serialized(&pool, &second_args, None).await?;
    match second {
        TransitionInsertOutcome::Conflict { current } => {
            let current = current.expect("existing row must be returned on collision");
            assert_eq!(
                current.evidence_git_hash.as_deref(),
                Some("deadbeef"),
                "the original durable row must be preserved unchanged"
            );
        }
        other => panic!("expected Conflict on divergent same-transition_id insert, got {other:?}"),
    }

    Ok(())
}

/// Defect 3, test 1: a *new* evidence-bearing transition (fresh evidence --
/// content fields present) with no v2 fingerprint is refused by the normal,
/// validated insert path -- this is exactly the legacy shape Bundle 7's
/// read-side validator separately refuses to *rank*
/// (`DurableFingerprintV2Missing`), but it must never reach durable storage
/// as a *new* row in the first place.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn absent_v2_fingerprint_on_a_new_row_is_refused_on_insert() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("v2_absent_new");
    let symbol = "AAPL";
    let timeframe_secs = 86_400;
    let now = Utc::now();

    let mut args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:legacy_no_v2")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    args.evidence_fingerprint_v2 = None;
    assert!(
        args.evidence_review_id.is_some(),
        "evidence-bearing fixture"
    );

    let err = insert_strategy_promotion_transition_serialized(&pool, &args, None)
        .await
        .expect_err("a fresh evidence-bearing insert with no v2 fingerprint must be refused");
    assert!(
        format!("{err:#}").contains("all-present or all-absent"),
        "got: {err:#}"
    );

    let current =
        fetch_current_promotion_state(&pool, &strategy_id, symbol, timeframe_secs).await?;
    assert!(
        current.is_none(),
        "a refused insert must leave zero durable trace"
    );

    Ok(())
}

/// BUNDLE-7-FOUNDATION-ACCEPTANCE-REPAIR-04: direct pre-0058 fixture
/// seeding. Migration 0058 added `evidence_fingerprint_v2` with no backfill,
/// so a genuine legacy row (recorded before that migration) is durably
/// `evidence_fingerprint_v2 IS NULL`. No public `mqk_db` function can
/// construct that shape any more -- both real insert paths
/// ([`insert_strategy_promotion_transition`] and
/// [`insert_strategy_promotion_transition_serialized`]) refuse a *new*
/// evidence-bearing transition with an absent v2 fingerprint (Defect 3, see
/// `absent_v2_fingerprint_on_a_new_row_is_refused_on_insert`), and the prior
/// `insert_legacy_evidence_transition_unchecked` bypass seam has been
/// removed from the production library entirely (it was reachable from any
/// production code path despite its "test-only" documentation). A test that
/// needs a genuine legacy row must therefore build it directly against the
/// table, clearly labeled as such -- never through a library function.
async fn seed_legacy_v2_absent_transition_row(
    pool: &sqlx::PgPool,
    args: &InsertStrategyPromotionTransitionArgs,
) -> anyhow::Result<()> {
    sqlx::query(
        r#"
        insert into sys_strategy_promotion_transitions (
            transition_id, strategy_id, symbol, timeframe_secs,
            config_fingerprint, config_identity_status,
            previous_state, new_state,
            parent_transition_id, evidence_transition_id,
            evidence_review_id, evidence_scanner_scan_id, evidence_git_hash,
            evidence_artifact_path, evidence_fingerprint, evidence_fingerprint_v2,
            effective_at_utc, expires_at_utc, initiated_by, reason, created_at_utc
        )
        values ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18,$19,$20,$21)
        "#,
    )
    .bind(args.transition_id)
    .bind(&args.strategy_id)
    .bind(&args.symbol)
    .bind(args.timeframe_secs)
    .bind(&args.config_fingerprint)
    .bind(&args.config_identity_status)
    .bind(&args.previous_state)
    .bind(&args.new_state)
    .bind(args.parent_transition_id)
    .bind(args.evidence_transition_id)
    .bind(&args.evidence_review_id)
    .bind(&args.evidence_scanner_scan_id)
    .bind(&args.evidence_git_hash)
    .bind(&args.evidence_artifact_path)
    .bind(&args.evidence_fingerprint)
    .bind(&args.evidence_fingerprint_v2)
    .bind(args.effective_at_utc)
    .bind(args.expires_at_utc)
    .bind(&args.initiated_by)
    .bind(&args.reason)
    .bind(args.created_at_utc)
    .execute(pool)
    .await?;
    Ok(())
}

/// Defect 3, test 5 (BUNDLE-7-FOUNDATION-ACCEPTANCE-REPAIR-04): a genuine
/// pre-migration-0058-shaped row (evidence-bearing, no v2 fingerprint),
/// seeded via direct SQL (never a library bypass function -- see
/// [`seed_legacy_v2_absent_transition_row`]), remains fully readable through
/// the normal read path -- and its durably-`NULL` `evidence_fingerprint_v2`
/// is exactly the precondition
/// `mqk_daemon::promotion_evidence_validation::validate_active_paper_candidate`
/// checks (`evidence.evidence_fingerprint_v2 ... .ok_or(DurableFingerprintV2Missing)`)
/// to refuse ranking a legacy identity under Bundle 7 -- so this row is
/// provably Bundle-7-ineligible without needing a second, duplicated
/// cross-crate DB round-trip to prove the same single-field check.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn legacy_v2_absent_row_is_readable_and_bundle7_ineligible() -> anyhow::Result<()> {
    let pool = test_pool().await?;
    let strategy_id = unique_id("v2_absent_legacy_seam");
    let symbol = "AAPL";
    let timeframe_secs = 86_400;
    let now = Utc::now();

    let mut args = make_args(
        transition_id_for(&format!("{strategy_id}:{symbol}:legacy_no_v2_seam")),
        &strategy_id,
        symbol,
        timeframe_secs,
        None,
        PROMOTION_STATE_SHADOW_APPROVED,
        now,
        now,
    );
    args.evidence_fingerprint_v2 = None;
    assert!(
        args.evidence_review_id.is_some(),
        "evidence-bearing fixture"
    );

    seed_legacy_v2_absent_transition_row(&pool, &args)
        .await
        .expect("direct pre-0058 fixture seed must succeed");

    let current = fetch_current_promotion_state(&pool, &strategy_id, symbol, timeframe_secs)
        .await?
        .expect("legacy-seeded row must be immediately queryable");
    assert_eq!(
        current.evidence_fingerprint.as_deref(),
        Some(FIXTURE_LEGACY_FINGERPRINT)
    );
    assert_eq!(
        current.evidence_fingerprint_v2, None,
        "durably NULL v2 fingerprint is the exact precondition that makes this identity \
         Bundle-7-ineligible (DurableFingerprintV2Missing) -- never backfilled from the \
         mutable artifact filesystem at read time"
    );

    Ok(())
}
