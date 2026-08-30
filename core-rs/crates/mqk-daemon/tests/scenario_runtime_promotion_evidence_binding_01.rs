//! RUNTIME-PROMOTION-EVIDENCE-BINDING-01 (C2): negative-control proofs that
//! the runtime dispatch gate (`promotion_gate::evaluate_paper_promotion_gate`,
//! shared by `decision::submit_internal_strategy_decision` Gate 3b and the
//! external signal route Gate 2b) refuses a promotion whose durable
//! `config_fingerprint` does not agree with the actual executing strategy
//! semantic identity — on top of, never instead of, every existing durable-
//! state check already proven by `scenario_strategy_promotion_runtime_gate_01.rs`.
//!
//! Every fixture seeds its `active_paper` row directly via
//! `insert_strategy_promotion_transition` (bypassing the operator transition
//! route entirely), so this file controls the durable fingerprint/status
//! independently of the decision's own claimed fingerprint — the two are
//! deliberately made to disagree in the "mismatch" tests.
//!
//! Requires `MQK_DATABASE_URL` and is marked `#[ignore]`. Run with:
//!   MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
//!   cargo test -p mqk-daemon --test scenario_runtime_promotion_evidence_binding_01 \
//!     -- --include-ignored --test-threads=1

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use chrono::Utc;
use http_body_util::BodyExt;
use mqk_daemon::{
    decision::{submit_internal_strategy_decision, InternalStrategyDecision},
    routes, state,
};
use tower::ServiceExt;
use uuid::Uuid;

const SYMBOL: &str = "AAPL";
const TIMEFRAME_SECS: i64 = 86_400;
const REAL_FINGERPRINT_A: fn() -> String = || "a".repeat(64);
const REAL_FINGERPRINT_B: fn() -> String = || "b".repeat(64);

fn unique_id(prefix: &str) -> String {
    let u = Uuid::new_v4().to_string().replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

async fn make_db_pool() -> sqlx::PgPool {
    let url = std::env::var(mqk_db::ENV_DB_URL).unwrap_or_else(|_| {
        panic!(
            "DB tests require MQK_DATABASE_URL; run: \
             MQK_DATABASE_URL=postgres://user:pass@localhost/mqk_test \
             cargo test -p mqk-daemon --test scenario_runtime_promotion_evidence_binding_01 \
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

async fn seed_registry(pool: &sqlx::PgPool, strategy_id: &str, enabled: bool) {
    let ts = Utc::now();
    mqk_db::upsert_strategy_registry_entry(
        pool,
        &mqk_db::UpsertStrategyRegistryArgs {
            strategy_id: strategy_id.to_string(),
            display_name: format!("C2 Test Strategy {strategy_id}"),
            enabled,
            kind: String::new(),
            registered_at_utc: ts,
            updated_at_utc: ts,
            note: String::new(),
        },
    )
    .await
    .expect("seed_registry failed");
}

/// Seed one `active_paper` row directly, with a caller-controlled
/// `config_fingerprint`/`config_identity_status` pair — no route/evidence
/// validation, no legality-chain walk (this file only ever needs the
/// terminal `active_paper` state, unlike the runtime-gate durable-state
/// matrix file).
async fn seed_active_paper(
    pool: &sqlx::PgPool,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    config_fingerprint: Option<&str>,
    config_identity_status: &str,
) {
    let transition_id = Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("c2-seed:{strategy_id}:{symbol}:{timeframe_secs}").as_bytes(),
    );
    mqk_db::insert_strategy_promotion_transition(
        pool,
        &mqk_db::InsertStrategyPromotionTransitionArgs {
            transition_id,
            strategy_id: strategy_id.to_string(),
            symbol: symbol.to_string(),
            timeframe_secs,
            config_fingerprint: config_fingerprint.map(|s| s.to_string()),
            config_identity_status: config_identity_status.to_string(),
            // `is_legal_transition` requires `paper_approved -> active_paper`
            // for the graph CHECK constraint -- the constraint validates only
            // this row's own (previous_state, new_state) edge, so it is
            // satisfied without an actual preceding row existing.
            previous_state: Some("paper_approved".to_string()),
            new_state: "active_paper".to_string(),
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
            initiated_by: "c2-test-seed".to_string(),
            reason: "test seed".to_string(),
            created_at_utc: Utc::now(),
        },
    )
    .await
    .expect("seed_active_paper failed");
}

async fn seed_active_run(st: &Arc<state::AppState>) -> Uuid {
    let pool = st.db.as_ref().expect("db configured");
    let run_id = Uuid::new_v4();
    let now = Utc::now();
    mqk_db::insert_run(
        pool,
        &mqk_db::NewRun {
            run_id,
            engine_id: "mqk-daemon".to_string(),
            mode: "PAPER".to_string(),
            started_at_utc: now,
            git_hash: "test".to_string(),
            config_hash: "test".to_string(),
            config_json: serde_json::json!({"source": "scenario_runtime_promotion_evidence_binding_01"}),
            host_fingerprint: "test-host".to_string(),
        },
    )
    .await
    .expect("insert_run");
    mqk_db::arm_run(pool, run_id).await.expect("arm_run");
    mqk_db::begin_run(pool, run_id).await.expect("begin_run");
    mqk_db::heartbeat_run(pool, run_id, now)
        .await
        .expect("heartbeat_run");
    st.inject_running_loop_for_test(run_id).await;
    run_id
}

fn make_decision(
    decision_id: &str,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    strategy_semantic_fingerprint: &str,
) -> InternalStrategyDecision {
    InternalStrategyDecision {
        decision_id: decision_id.to_string(),
        strategy_id: strategy_id.to_string(),
        symbol: symbol.to_string(),
        timeframe_secs,
        strategy_semantic_fingerprint: strategy_semantic_fingerprint.to_string(),
        side: "buy".to_string(),
        qty: 10,
        order_type: "market".to_string(),
        time_in_force: "day".to_string(),
        limit_price: None,
    }
}

async fn outbox_row_count(pool: &sqlx::PgPool, decision_id: &str) -> i64 {
    let row: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM oms_outbox WHERE idempotency_key = $1")
        .bind(decision_id)
        .fetch_one(pool)
        .await
        .expect("count outbox rows");
    row.0
}

async fn state_with_arm_and_run(pool: sqlx::PgPool) -> (Arc<state::AppState>, Uuid) {
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    let run_id = seed_active_run(&st).await;
    (st, run_id)
}

// ---------------------------------------------------------------------------
// Internal path
// ---------------------------------------------------------------------------

/// Positive control: exact fingerprint match → decision accepted, one outbox
/// row.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_exact_fingerprint_match_is_accepted() {
    let pool = make_db_pool().await;
    let sid = unique_id("c2i_match");
    seed_registry(&pool, &sid, true).await;
    let fp = REAL_FINGERPRINT_A();
    seed_active_paper(&pool, &sid, SYMBOL, TIMEFRAME_SECS, Some(&fp), "verified_v1").await;
    let (st, run_id) = state_with_arm_and_run(pool.clone()).await;

    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS, &fp),
    )
    .await;
    assert!(out.accepted, "exact match must be accepted: {out:?}");
    assert_eq!(out.disposition, "accepted");
    assert_eq!(out.active_run_id, Some(run_id));
    assert_eq!(outbox_row_count(&pool, &dec_id).await, 1);
}

/// Core C2 negative control: same `(strategy_id, symbol, timeframe_secs)`,
/// durable `active_paper`, but the decision's own semantic fingerprint
/// disagrees with the promoted one (config genuinely changed since
/// approval, e.g. an operator changed sizing without re-approving) — must
/// be refused with `promotion_config_mismatch` and create zero outbox rows.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_changed_semantic_config_is_refused() {
    let pool = make_db_pool().await;
    let sid = unique_id("c2i_drift");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper(
        &pool,
        &sid,
        SYMBOL,
        TIMEFRAME_SECS,
        Some(&REAL_FINGERPRINT_A()),
        "verified_v1",
    )
    .await;
    let (st, _run_id) = state_with_arm_and_run(pool.clone()).await;

    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS, &REAL_FINGERPRINT_B()),
    )
    .await;
    assert!(!out.accepted, "drifted config must be refused: {out:?}");
    assert_eq!(out.disposition, "promotion_config_mismatch");
    assert_eq!(outbox_row_count(&pool, &dec_id).await, 0);
}

/// Legacy NULL promoted fingerprint must never wildcard-match any decision,
/// however plausible its own fingerprint looks.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_legacy_null_promoted_fingerprint_is_refused() {
    let pool = make_db_pool().await;
    let sid = unique_id("c2i_legacy");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper(
        &pool,
        &sid,
        SYMBOL,
        TIMEFRAME_SECS,
        None,
        "unavailable_in_current_runtime",
    )
    .await;
    let (st, _run_id) = state_with_arm_and_run(pool.clone()).await;

    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS, &REAL_FINGERPRINT_A()),
    )
    .await;
    assert!(!out.accepted, "legacy NULL must never authorize: {out:?}");
    assert_eq!(out.disposition, "promotion_config_mismatch");
    assert_eq!(outbox_row_count(&pool, &dec_id).await, 0);
}

/// An `unavailable`/unverified `config_identity_status` refuses even when a
/// (stale/legacy-shaped) `config_fingerprint` value happens to be present —
/// `verified_v1` is required, not merely a non-null fingerprint.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_unverified_config_identity_status_is_refused() {
    let pool = make_db_pool().await;
    let sid = unique_id("c2i_unverified");
    seed_registry(&pool, &sid, true).await;
    let fp = REAL_FINGERPRINT_A();
    seed_active_paper(
        &pool,
        &sid,
        SYMBOL,
        TIMEFRAME_SECS,
        Some(&fp),
        "unavailable_in_current_runtime",
    )
    .await;
    let (st, _run_id) = state_with_arm_and_run(pool.clone()).await;

    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS, &fp),
    )
    .await;
    assert!(
        !out.accepted,
        "unverified config_identity_status must refuse even with a matching fingerprint: {out:?}"
    );
    assert_eq!(out.disposition, "promotion_config_mismatch");
    assert_eq!(outbox_row_count(&pool, &dec_id).await, 0);
}

/// A malformed (not 64 lowercase hex) decision-side fingerprint is refused
/// even though the promoted fingerprint is genuinely verified.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_malformed_decision_fingerprint_is_refused() {
    let pool = make_db_pool().await;
    let sid = unique_id("c2i_malformed");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper(
        &pool,
        &sid,
        SYMBOL,
        TIMEFRAME_SECS,
        Some(&REAL_FINGERPRINT_A()),
        "verified_v1",
    )
    .await;
    let (st, _run_id) = state_with_arm_and_run(pool.clone()).await;

    let dec_id = unique_id("dec");
    let out = submit_internal_strategy_decision(
        &st,
        make_decision(&dec_id, &sid, SYMBOL, TIMEFRAME_SECS, "not-a-real-fingerprint"),
    )
    .await;
    assert!(!out.accepted, "malformed fingerprint must refuse: {out:?}");
    assert_eq!(out.disposition, "promotion_config_mismatch");
    assert_eq!(outbox_row_count(&pool, &dec_id).await, 0);
}

/// Result values never participate: two decisions with identical config
/// identity but different qty/side both pass the config-identity check
/// identically (any subsequent difference in outcome must come from a
/// different gate, never this one).
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn internal_result_values_do_not_affect_config_identity_check() {
    let pool = make_db_pool().await;
    let sid = unique_id("c2i_resultagnostic");
    seed_registry(&pool, &sid, true).await;
    let fp = REAL_FINGERPRINT_A();
    seed_active_paper(&pool, &sid, SYMBOL, TIMEFRAME_SECS, Some(&fp), "verified_v1").await;
    let (st, _run_id) = state_with_arm_and_run(pool.clone()).await;

    let mut d1 = make_decision(&unique_id("dec"), &sid, SYMBOL, TIMEFRAME_SECS, &fp);
    d1.qty = 1;
    d1.side = "buy".to_string();
    let mut d2 = make_decision(&unique_id("dec"), &sid, SYMBOL, TIMEFRAME_SECS, &fp);
    d2.qty = 999;
    d2.side = "sell".to_string();

    let out1 = submit_internal_strategy_decision(&st, d1).await;
    let out2 = submit_internal_strategy_decision(&st, d2).await;
    assert!(out1.accepted, "d1 must pass config identity: {out1:?}");
    assert!(out2.accepted, "d2 must pass config identity regardless of differing qty/side: {out2:?}");
}

// ---------------------------------------------------------------------------
// External signal path
// ---------------------------------------------------------------------------

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

fn signal_req(body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri("/api/v1/strategy/signal")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

async fn make_external_signal_state(pool: sqlx::PgPool) -> Arc<state::AppState> {
    let st = Arc::new(state::AppState::new_for_test_with_db_mode_and_broker(
        pool,
        state::DeploymentMode::Paper,
        state::BrokerKind::Alpaca,
    ));
    st.update_ws_continuity(state::AlpacaWsContinuityState::Live {
        last_message_id: "alpaca:c2:new:2024-01-08T14:00:00Z".to_string(),
        last_event_at: "2024-01-08T14:00:00Z".to_string(),
    })
    .await;
    st.set_session_clock_ts_for_test(1_704_726_000).await;
    st
}

fn external_signal_body(
    signal_id: &str,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: Option<i64>,
) -> serde_json::Value {
    serde_json::json!({
        "signal_id": signal_id,
        "strategy_id": strategy_id,
        "symbol": symbol,
        "side": "buy",
        "qty": 10,
        "timeframe_secs": timeframe_secs,
    })
}

/// External-path fail-closed proof: `active_paper` is durably recorded for a
/// `strategy_id` that has NO server-authoritative semantic identity (unknown
/// to the built-in registry — this bypasses C1's promotion-route protection
/// entirely by seeding the row directly, simulating a residual legacy/
/// out-of-band row). EXTERNAL-SIGNAL-SEMANTIC-PROVENANCE-FAIL-CLOSED-01:
/// resolvability is irrelevant here — the external signal path never
/// attempts (or trusts) a server-side reconstruction at all, so this
/// resolves identically to any other `active_paper` identity submitted via
/// this path: refused as provenance-unavailable, not merely "unresolvable
/// mismatch".
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_unresolvable_strategy_identity_fails_closed() {
    let pool = make_db_pool().await;
    let sid = unique_id("c2e_unresolvable");
    seed_registry(&pool, &sid, true).await;
    // Even a "verified_v1"-looking durable row must not help: the identity
    // itself can never be resolved server-side for a non-built-in name.
    seed_active_paper(
        &pool,
        &sid,
        SYMBOL,
        TIMEFRAME_SECS,
        Some(&REAL_FINGERPRINT_A()),
        "verified_v1",
    )
    .await;
    let st = make_external_signal_state(pool.clone()).await;

    let signal_id = unique_id("sig");
    let (status, json) = call(
        routes::build_router(st),
        signal_req(external_signal_body(&signal_id, &sid, SYMBOL, Some(TIMEFRAME_SECS))),
    )
    .await;
    assert_eq!(status, StatusCode::FORBIDDEN, "must be refused: {json}");
    assert_eq!(
        json["disposition"], "promotion_external_semantic_provenance_unavailable"
    );
    assert_eq!(outbox_row_count(&pool, &signal_id).await, 0);
}

/// EXTERNAL-SIGNAL-SEMANTIC-PROVENANCE-FAIL-CLOSED-01: independent review
/// found this test previously asserted the DEFECT itself -- a genuinely
/// `active_paper`, config-bound identity was accepted on the external
/// signal path merely because the daemon reconstructed a matching NATIVE
/// fingerprint, which proves this daemon's own current configuration, never
/// the actual external producer's decision logic. Now proves the opposite:
/// even this exact real, correctly-promoted identity must be REFUSED on the
/// external path (no trusted provenance channel exists there at all), and a
/// forged request field claiming the (genuinely correct) fingerprint value
/// still has zero effect -- it does not "rescue" the request into
/// acceptance, exactly as it also could not before invent one that was
/// never legitimate.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn external_forged_fingerprint_field_has_no_effect() {
    let pool = make_db_pool().await;
    let sid = "swing_momentum".to_string();
    let symbol = unique_id("C2FORGE").to_uppercase();
    let real_fp = mqk_daemon::strategy_config_identity::resolve_server_semantic_fingerprint(
        &sid,
        &symbol,
        TIMEFRAME_SECS,
    )
    .expect("swing_momentum must resolve");
    seed_registry(&pool, &sid, true).await;
    seed_active_paper(&pool, &sid, &symbol, TIMEFRAME_SECS, Some(&real_fp), "verified_v1").await;
    mqk_db::persist_arm_state(&pool, "ARMED", None)
        .await
        .expect("persist ARMED");
    let st = make_external_signal_state(pool.clone()).await;
    seed_active_run(&st).await;

    let signal_id = unique_id("sig");
    let mut body = external_signal_body(&signal_id, &sid, &symbol, Some(TIMEFRAME_SECS));
    // Not a real field on the request schema -- must be silently ignored,
    // and must not "rescue" this identity into acceptance even though the
    // forged value here is the genuinely correct fingerprint.
    body["config_fingerprint"] = serde_json::json!(real_fp);
    let (status, json) = call(routes::build_router(st), signal_req(body)).await;
    assert_eq!(
        status,
        StatusCode::FORBIDDEN,
        "external signals must be refused for config-bound Paper authority regardless of a \
         forged (even genuinely-matching) fingerprint field -- there is no trusted provenance \
         channel on this path: {json}"
    );
    assert_eq!(
        json["disposition"], "promotion_external_semantic_provenance_unavailable"
    );
    assert_eq!(outbox_row_count(&pool, &signal_id).await, 0);
}

/// Live mode remains unconditionally denied even with an exact fingerprint
/// match — config-identity binding never becomes a live-authorization path.
#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn exact_match_never_authorizes_live_mode() {
    let pool = make_db_pool().await;
    let sid = unique_id("c2_live");
    seed_registry(&pool, &sid, true).await;
    let fp = REAL_FINGERPRINT_A();
    seed_active_paper(&pool, &sid, SYMBOL, TIMEFRAME_SECS, Some(&fp), "verified_v1").await;

    let outcome = mqk_daemon::promotion_gate::evaluate_paper_promotion_gate(
        &pool,
        mqk_daemon::promotion_gate::PromotionRunMode::Live,
        &sid,
        SYMBOL,
        TIMEFRAME_SECS,
        mqk_daemon::promotion_gate::SemanticProvenance::Fingerprint(Some(fp.as_str())),
    )
    .await;
    assert!(
        !outcome.paper_tradable,
        "exact fingerprint match must not authorize Live mode"
    );
    assert_eq!(
        outcome.reason_code.code(),
        "promotion_live_not_authorized"
    );
}
