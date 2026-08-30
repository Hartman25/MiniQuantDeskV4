//! PROMOTION-CONFIG-IDENTITY-01 (C1): negative-control proofs for the
//! promotion-transition route's config-identity binding, isolated from the
//! expensive full Research/Backtest evidence pipeline.
//!
//! Continuity transitions (`shadow_approved -> paper_approved`,
//! `paper_approved -> active_paper`) and safety exits (`-> demoted`,
//! `-> retired`, `-> rejected`) never require fresh evidence
//! (`transition_requires_evidence` is `false`), so every fixture here seeds
//! its parent row directly via `insert_strategy_promotion_transition`
//! (bypassing evidence validation entirely, exactly like
//! `scenario_strategy_promotion_runtime_gate_01.rs` already does for the
//! runtime gate) and then calls the REAL
//! `POST /api/v1/strategy/promotions/transition` route only for the
//! continuity/safety transition actually under test. The full evidence-gated
//! happy path (fresh `shadow_approved` -> ... -> `active_paper`, each hop
//! through the real route with real evidence) is covered by
//! `scenario_strategy_promotion_closure_proof_01f.rs`.
//!
//! `strategy_id = "swing_momentum"`, `timeframe_secs = 86400` throughout --
//! the one built-in engine whose semantic fingerprint is a pure function of
//! `(symbol,)` with no ambient env dependency, so "the real current
//! fingerprint" is trivially reproducible via
//! `mqk_daemon::strategy_config_identity::resolve_server_semantic_fingerprint`.
//!
//! Requires `MQK_DATABASE_URL` and is marked `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_promotion_config_identity_01 \
//!     -- --include-ignored --test-threads=1

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_daemon::strategy_config_identity::resolve_server_semantic_fingerprint;
use mqk_daemon::{routes, state};
use sqlx::Row;
use tower::ServiceExt;
use uuid::Uuid;

const STRATEGY_ID: &str = "swing_momentum";
const TIMEFRAME_SECS: i64 = 86_400;
const TRANSITION_ROUTE: &str = "/api/v1/strategy/promotions/transition";
/// A syntactically-valid-looking 64-lowercase-hex string that is
/// deliberately NOT the real `swing_momentum` fingerprint for any symbol --
/// used to simulate a "config has drifted since approval" parent row.
const WRONG_FINGERPRINT: &str = "dead00000000000000000000000000000000000000000000000000000000be";

fn unique_symbol() -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("Z{}", &u[..9]).to_uppercase()
}

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_promotion_config_identity_01 \
             -- --include-ignored --test-threads=1"
        )
    });
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(2)
        .connect(&url)
        .await
        .expect("connect to test DB");
    mqk_db::migrate(&pool).await.expect("run migrations");
    pool
}

/// Insert one promotion transition row directly, with an explicit
/// `config_fingerprint`/`config_identity_status` -- no route/evidence
/// validation involved. `parent_transition_id` is always `None` here: these
/// fixtures each seed exactly one parent row per identity, so the identity
/// has no prior chain to link to.
#[allow(clippy::too_many_arguments)]
async fn seed_transition(
    pool: &sqlx::PgPool,
    symbol: &str,
    previous_state: Option<&str>,
    new_state: &str,
    config_fingerprint: Option<&str>,
    config_identity_status: &str,
    seed_suffix: &str,
) {
    let transition_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("promo-config-identity-seed:{STRATEGY_ID}:{symbol}:{TIMEFRAME_SECS}:{seed_suffix}")
            .as_bytes(),
    );
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: STRATEGY_ID.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs: TIMEFRAME_SECS,
            config_fingerprint: config_fingerprint.map(|s| s.to_string()),
            config_identity_status: config_identity_status.to_string(),
            previous_state: previous_state.map(|s| s.to_string()),
            new_state: new_state.to_string(),
            parent_transition_id: None,
            evidence_transition_id: None,
            evidence_review_id: None,
            evidence_scanner_scan_id: None,
            evidence_git_hash: None,
            evidence_artifact_path: None,
            evidence_fingerprint: None,
            evidence_fingerprint_v2: None,
            effective_at_utc: Utc::now(),
            expires_at_utc: None,
            initiated_by: "config-identity-test-seed".to_string(),
            reason: "test seed".to_string(),
            created_at_utc: Utc::now(),
        },
    )
    .await
    .expect("seed_transition failed");
}

async fn call(
    router: axum::Router,
    req: Request<axum::body::Body>,
) -> (StatusCode, serde_json::Value) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    let json: serde_json::Value = serde_json::from_slice(&body).expect("valid JSON body");
    (status, json)
}

fn transition_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(TRANSITION_ROUTE)
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

fn app_state(pool: sqlx::PgPool) -> Arc<state::AppState> {
    Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ))
}

async fn cleanup(pool: &sqlx::PgPool, symbol: &str) {
    sqlx::query(
        "DELETE FROM sys_strategy_promotion_transitions WHERE strategy_id = $1 AND symbol = $2",
    )
    .bind(STRATEGY_ID)
    .bind(symbol)
    .execute(pool)
    .await
    .expect("cleanup");
}

/// Positive control: parent's persisted `config_fingerprint` exactly equals
/// the current server-resolved fingerprint -> continuity transition
/// succeeds and re-persists the SAME verified fingerprint.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn continuity_match_advances_and_persists_same_fingerprint() {
    let pool = make_db_pool().await;
    let symbol = unique_symbol();
    let real_fp = resolve_server_semantic_fingerprint(STRATEGY_ID, &symbol, TIMEFRAME_SECS)
        .expect("swing_momentum must resolve");
    assert_eq!(real_fp.len(), 64, "fingerprint must be 64 hex chars");
    assert!(
        real_fp.bytes().all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()),
        "fingerprint must be lowercase hex"
    );

    seed_transition(
        &pool,
        &symbol,
        None,
        "shadow_approved",
        Some(&real_fp),
        "verified_v1",
        "1",
    )
    .await;

    let st = app_state(pool.clone());
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": STRATEGY_ID,
            "symbol": symbol,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "paper_approved",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "config-identity-test",
            "reason": "continuity match",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "continuity match must succeed: {json}");
    assert_eq!(json["disposition"], "transitioned");

    let transition_id: Uuid = Uuid::parse_str(json["transition_id"].as_str().unwrap()).unwrap();
    let row = sqlx::query("SELECT config_fingerprint, config_identity_status FROM sys_strategy_promotion_transitions WHERE transition_id = $1")
        .bind(transition_id)
        .fetch_one(&pool)
        .await
        .expect("readback");
    let persisted_fp: Option<String> = row.try_get("config_fingerprint").unwrap();
    let persisted_status: String = row.try_get("config_identity_status").unwrap();
    assert_eq!(persisted_fp.as_deref(), Some(real_fp.as_str()));
    assert_eq!(persisted_status, "verified_v1");

    cleanup(&pool, &symbol).await;
}

async fn row_count_for(pool: &sqlx::PgPool, symbol: &str) -> i64 {
    let row: (i64,) = sqlx::query_as(
        "SELECT COUNT(*) FROM sys_strategy_promotion_transitions WHERE strategy_id = $1 AND symbol = $2",
    )
    .bind(STRATEGY_ID)
    .bind(symbol)
    .fetch_one(pool)
    .await
    .expect("count rows");
    row.0
}

/// A semantic config change since the evidence-bearing approval (parent
/// carries a fingerprint that does not match the current real one) must
/// fail closed on `shadow_approved -> paper_approved`, never silently
/// carrying the stale approval forward, and must create no new row.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn continuity_mismatch_shadow_to_paper_is_rejected() {
    let pool = make_db_pool().await;
    let symbol = unique_symbol();
    let real_fp = resolve_server_semantic_fingerprint(STRATEGY_ID, &symbol, TIMEFRAME_SECS)
        .expect("swing_momentum must resolve");
    assert_ne!(real_fp, WRONG_FINGERPRINT, "sanity: fixture constant must not collide with reality");

    seed_transition(
        &pool,
        &symbol,
        None,
        "shadow_approved",
        Some(WRONG_FINGERPRINT),
        "verified_v1",
        "1",
    )
    .await;
    let before = row_count_for(&pool, &symbol).await;

    let st = app_state(pool.clone());
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": STRATEGY_ID,
            "symbol": symbol,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "paper_approved",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "config-identity-test",
            "reason": "continuity mismatch",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "mismatch must be refused: {json}");
    assert_eq!(json["disposition"], "config_identity_mismatch");
    assert_eq!(
        row_count_for(&pool, &symbol).await,
        before,
        "a rejected continuity transition must create no new row"
    );

    cleanup(&pool, &symbol).await;
}

/// Same invariant, one hop later in the chain: config drift between
/// `paper_approved` and `active_paper` must also fail closed.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn continuity_mismatch_paper_to_active_is_rejected() {
    let pool = make_db_pool().await;
    let symbol = unique_symbol();

    seed_transition(
        &pool,
        &symbol,
        Some("shadow_approved"),
        "paper_approved",
        Some(WRONG_FINGERPRINT),
        "verified_v1",
        "1",
    )
    .await;

    let st = app_state(pool.clone());
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": STRATEGY_ID,
            "symbol": symbol,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "active_paper",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "config-identity-test",
            "reason": "continuity mismatch at active_paper",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "mismatch must be refused: {json}");
    assert_eq!(json["disposition"], "config_identity_mismatch");

    cleanup(&pool, &symbol).await;
}

/// Legacy NULL policy: a parent row with no verified `config_fingerprint`
/// (pre-C1 legacy shape) must never wildcard-match -- it can never authorize
/// advancing the identity, exactly like a genuine mismatch.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn continuity_legacy_null_parent_is_rejected() {
    let pool = make_db_pool().await;
    let symbol = unique_symbol();

    seed_transition(
        &pool,
        &symbol,
        None,
        "shadow_approved",
        None, // legacy NULL fingerprint
        "unavailable_in_current_runtime",
        "1",
    )
    .await;

    let st = app_state(pool.clone());
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": STRATEGY_ID,
            "symbol": symbol,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "paper_approved",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "config-identity-test",
            "reason": "legacy null parent",
        })),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT, "legacy NULL must never wildcard-match: {json}");
    assert_eq!(json["disposition"], "config_identity_mismatch");

    cleanup(&pool, &symbol).await;
}

/// Safety exits must remain reachable even when the parent's config
/// fingerprint has drifted -- demotion/retirement must never be blocked by
/// a config mismatch that would refuse a continuity transition.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn safety_exit_demoted_succeeds_despite_mismatched_parent() {
    let pool = make_db_pool().await;
    let symbol = unique_symbol();
    let real_fp = resolve_server_semantic_fingerprint(STRATEGY_ID, &symbol, TIMEFRAME_SECS)
        .expect("swing_momentum must resolve");

    seed_transition(
        &pool,
        &symbol,
        Some("paper_approved"),
        "active_paper",
        Some(WRONG_FINGERPRINT),
        "verified_v1",
        "1",
    )
    .await;

    let st = app_state(pool.clone());
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": STRATEGY_ID,
            "symbol": symbol,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "demoted",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "config-identity-test",
            "reason": "safety exit despite drift",
        })),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "demotion must remain possible despite config drift: {json}"
    );
    assert_eq!(json["disposition"], "transitioned");

    // Resolution is attempted (for observability) but never blocks -- the
    // real current fingerprint is persisted, not the stale WRONG_FINGERPRINT.
    let transition_id: Uuid = Uuid::parse_str(json["transition_id"].as_str().unwrap()).unwrap();
    let row = sqlx::query("SELECT config_fingerprint, config_identity_status FROM sys_strategy_promotion_transitions WHERE transition_id = $1")
        .bind(transition_id)
        .fetch_one(&pool)
        .await
        .expect("readback");
    let persisted_fp: Option<String> = row.try_get("config_fingerprint").unwrap();
    assert_eq!(persisted_fp.as_deref(), Some(real_fp.as_str()));

    cleanup(&pool, &symbol).await;
}

/// Caller-forgery proof: `StrategyPromotionTransitionRequest` has no
/// `config_fingerprint` field at all (see `api_types.rs`) -- an attacker
/// cannot even express a fingerprint claim in the request schema. This test
/// empirically confirms an extra, unrecognized `config_fingerprint` JSON
/// field in the request body has zero effect: the persisted value is always
/// the server-derived real fingerprint, never the attacker-supplied string.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn caller_supplied_fingerprint_field_is_ignored() {
    let pool = make_db_pool().await;
    let symbol = unique_symbol();
    let real_fp = resolve_server_semantic_fingerprint(STRATEGY_ID, &symbol, TIMEFRAME_SECS)
        .expect("swing_momentum must resolve");

    seed_transition(
        &pool,
        &symbol,
        None,
        "shadow_approved",
        Some(&real_fp),
        "verified_v1",
        "1",
    )
    .await;

    let st = app_state(pool.clone());
    let forged = "f".repeat(64);
    let (status, json) = call(
        routes::build_router(Arc::clone(&st)),
        transition_req(serde_json::json!({
            "strategy_id": STRATEGY_ID,
            "symbol": symbol,
            "timeframe_secs": TIMEFRAME_SECS,
            "target_state": "paper_approved",
            "review_dir": null,
            "effective_at_utc": Utc::now().to_rfc3339(),
            "expires_at_utc": null,
            "initiated_by": "config-identity-test",
            "reason": "caller forgery attempt",
            // Not a real request field -- must be silently ignored, never
            // become authority.
            "config_fingerprint": forged,
        })),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "transition must succeed: {json}");

    let transition_id: Uuid = Uuid::parse_str(json["transition_id"].as_str().unwrap()).unwrap();
    let row = sqlx::query("SELECT config_fingerprint FROM sys_strategy_promotion_transitions WHERE transition_id = $1")
        .bind(transition_id)
        .fetch_one(&pool)
        .await
        .expect("readback");
    let persisted_fp: Option<String> = row.try_get("config_fingerprint").unwrap();
    assert_eq!(
        persisted_fp.as_deref(),
        Some(real_fp.as_str()),
        "persisted fingerprint must be the server-derived real value, never the forged one"
    );
    assert_ne!(persisted_fp.as_deref(), Some(forged.as_str()));

    cleanup(&pool, &symbol).await;
}
