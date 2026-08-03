//! DYNAMIC-STRATEGY-SYMBOL-SELECTION-01 Phase 3: DB-backed proof for
//! `promotion_evidence_validation::validate_active_paper_candidate` — the
//! shared read-side evidence validator Bundle 7's plan builder (Phase 4)
//! will consume.
//!
//! Every identity is driven to its target promotion state through the real
//! `POST /api/v1/strategy/promotions/transition` route (same as
//! `scenario_strategy_promotion_routes_01.rs`), so these tests exercise the
//! validator against genuine durable evidence lineage, never a hand-rolled
//! DB row.
//!
//! All tests require `MQK_DATABASE_URL` and are marked `#[ignore]`. Each test
//! runs against its own disposable per-test database (`mqk_db::run_isolated`,
//! FULL-AUDIT-FAIL-017) and holds a process-global async lock for the
//! duration of its `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT` env-var-sensitive
//! section, so this file is safe under the default (parallel) test runner as
//! well as `--test-threads=1`. Run with:
//!   MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//!   cargo test -p mqk-daemon --features testkit \
//!     --test scenario_dynamic_selection_evidence_validation_01 -- --include-ignored
//!
//! # Proof matrix
//!
//! | Test                              | What it proves                                    |
//! |------------------------------------|----------------------------------------------------|
//! | valid_active_paper_candidate_succeeds | active_paper + matching evidence -> Ok with correct score/rank |
//! | no_promotion_record_is_refused    | never-promoted identity -> NoPromotionRecord        |
//! | shadow_approved_is_not_active_paper | shadow_approved only -> PromotionNotActivePaper   |
//! | not_yet_effective_is_refused      | authority_ts before effective_at_utc -> PromotionNotYetEffective |
//! | expired_is_refused                | authority_ts after expires_at_utc -> PromotionExpired |
//! | tampered_durable_fingerprint_is_refused | durable evidence_fingerprint UPDATE'd -> FingerprintMismatch |
//! | missing_score_is_refused          | evidence row with scanner_score=None -> ScoreMissing |

use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::http::{header, Request, StatusCode};
use chrono::{DateTime, Duration, Utc};
use http_body_util::BodyExt;
use mqk_backtest::{
    write_review_artifacts, ReviewManifest, ReviewRunOutput, ReviewSummary,
    StrategyScanReviewDecision, StrategyScanReviewState,
};
use mqk_daemon::promotion_evidence_validation::{
    validate_active_paper_candidate, CandidateEvidenceReason,
};
use mqk_daemon::{routes, state};
use tower::ServiceExt;
use uuid::Uuid;

// ---------------------------------------------------------------------------
// Fixtures / helpers (deliberately duplicated from
// scenario_strategy_promotion_routes_01.rs -- integration test binaries
// cannot share code without a support crate, and this repo's convention is
// one self-contained fixture set per scenario file).
// ---------------------------------------------------------------------------

/// Deterministic UUIDv5, namespaced by an explicit per-call-site seed string
/// -- never `Uuid::new_v4()`. Fixture uniqueness comes from the seed text
/// (always the caller-supplied label/prefix, itself unique per call site in
/// this file), not from randomness.
fn det_uuid(seed: &str) -> Uuid {
    Uuid::new_v5(&Uuid::NAMESPACE_URL, seed.as_bytes())
}

fn temp_dir(label: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "mqk_daemon_dyn_sel_evidence_{label}_{}",
        det_uuid(&format!(
            "scenario_dynamic_selection_evidence_validation_01::temp_dir::{label}"
        ))
    ));
    std::fs::create_dir_all(&dir).expect("create temp dir");
    dir
}

fn unique_id(prefix: &str) -> String {
    let u = det_uuid(&format!(
        "scenario_dynamic_selection_evidence_validation_01::unique_id::{prefix}"
    ))
    .to_string()
    .replace('-', "");
    format!("{prefix}_{}", &u[..12])
}

fn write_fixture(out_dir: &Path, decisions: Vec<StrategyScanReviewDecision>) -> PathBuf {
    let paper_candidate_count = decisions
        .iter()
        .filter(|d| d.review_state == StrategyScanReviewState::PaperCandidate)
        .count();
    let review_id = det_uuid(&format!(
        "scenario_dynamic_selection_evidence_validation_01::write_fixture::{}",
        out_dir.display()
    ));
    let manifest = ReviewManifest {
        schema_version: 1,
        review_id: review_id.to_string(),
        scanner_scan_id: "scan-fixture".to_string(),
        source_artifact_dir: "fixture-source-not-on-disk".to_string(),
        created_at_utc: "2026-07-01T00:00:00Z".to_string(),
        git_hash: "test-git-hash".to_string(),
        policy_min_bars_used: 252,
        policy_min_trade_count: 5,
        policy_min_total_return_pct: 0.0,
        policy_min_alpha_pct: 0.0,
        policy_max_drawdown_pct: 25.0,
        policy_min_profit_factor: 1.05,
        candidate_count: decisions.len(),
        blocked_count: 0,
        needs_review_count: 0,
        watchlist_candidate_count: 0,
        paper_candidate_count,
        rejected_count: 0,
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    let summary = ReviewSummary {
        scanner_scan_id: "scan-fixture".to_string(),
        review_id: review_id.to_string(),
        candidate_count: decisions.len(),
        blocked_count: 0,
        needs_review_count: 0,
        watchlist_candidate_count: 0,
        paper_candidate_count,
        rejected_count: 0,
        top_paper_candidates: Vec::new(),
        top_watchlist_candidates: Vec::new(),
        blockers: Vec::new(),
        warnings: Vec::new(),
    };
    let output = ReviewRunOutput {
        review_id,
        manifest,
        decisions,
        summary,
    };
    write_review_artifacts(out_dir, &output).expect("write fixture review artifacts")
}

fn paper_candidate(strategy_id: &str, symbol: &str, timeframe: &str) -> StrategyScanReviewDecision {
    StrategyScanReviewDecision {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        strategy_id: strategy_id.to_string(),
        scanner_rank: Some(1),
        scanner_score: Some(9.0),
        review_state: StrategyScanReviewState::PaperCandidate,
        reason_codes: vec!["eligible_paper_candidate".to_string()],
        blockers: Vec::new(),
        warnings: Vec::new(),
    }
}

fn paper_candidate_no_score(
    strategy_id: &str,
    symbol: &str,
    timeframe: &str,
) -> StrategyScanReviewDecision {
    StrategyScanReviewDecision {
        symbol: symbol.to_string(),
        timeframe: timeframe.to_string(),
        strategy_id: strategy_id.to_string(),
        scanner_rank: Some(1),
        scanner_score: None,
        review_state: StrategyScanReviewState::PaperCandidate,
        reason_codes: vec!["eligible_paper_candidate".to_string()],
        blockers: Vec::new(),
        warnings: Vec::new(),
    }
}

// `MQK_STRATEGY_REVIEW_ARTIFACT_ROOT` is process-global state. Every test in
// this file sets it to its own private temp dir and relies on route handlers
// reading it back later in the same test (not just at set-time), so the
// hazard isn't only a torn write -- it's any interleaving where test A's
// route call reads test B's root after B has already overwritten it. Real
// failure observed: 6 of 7 tests in this file fail under the default
// (parallel) libtest runner, each independently, non-deterministically,
// while every one passes cleanly under `--test-threads=1` -- textbook
// global-environment-mutation contamination, not a logic defect. A
// process-global async lock, held for each test's entire env-var-sensitive
// section (returned to the caller and dropped at end of the test function),
// serializes these tests against each other without requiring the whole
// binary to run single-threaded.
static ENV_MUTATION_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

async fn make_state_with_db(
    root: &Path,
    pool: sqlx::PgPool,
) -> (Arc<state::AppState>, tokio::sync::MutexGuard<'static, ()>) {
    let guard = ENV_MUTATION_LOCK.lock().await;
    std::env::set_var("MQK_STRATEGY_REVIEW_ARTIFACT_ROOT", root);
    let st = Arc::new(state::AppState::new_with_db_and_operator_auth(
        pool,
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    (st, guard)
}

async fn call(router: axum::Router, req: Request<axum::body::Body>) -> (StatusCode, bytes::Bytes) {
    let resp = router.oneshot(req).await.unwrap();
    let status = resp.status();
    let body = resp.into_body().collect().await.unwrap().to_bytes();
    (status, body)
}

const TRANSITION_ROUTE: &str = "/api/v1/strategy/promotions/transition";

fn transition_body(
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    target_state: &str,
    review_dir: Option<&str>,
    effective_at_utc: DateTime<Utc>,
    expires_at_utc: Option<DateTime<Utc>>,
) -> serde_json::Value {
    serde_json::json!({
        "strategy_id": strategy_id,
        "symbol": symbol,
        "timeframe_secs": timeframe_secs,
        "target_state": target_state,
        "review_dir": review_dir,
        "effective_at_utc": effective_at_utc.to_rfc3339(),
        "expires_at_utc": expires_at_utc.map(|t| t.to_rfc3339()),
        "initiated_by": "test-operator",
        "reason": "scenario test",
    })
}

fn post_json_req(uri: &str, body: serde_json::Value) -> Request<axum::body::Body> {
    Request::builder()
        .method("POST")
        .uri(uri)
        .header(header::CONTENT_TYPE, "application/json")
        .body(axum::body::Body::from(serde_json::to_vec(&body).unwrap()))
        .unwrap()
}

/// Drives an identity through shadow_approved -> paper_approved ->
/// active_paper via the real POST route, asserting 200 OK at each step.
///
/// `effective_at_offset`/`expires_at_offset` are applied relative to
/// `Utc::now()` captured *immediately before the active_paper POST*, not
/// before the earlier shadow_approved/paper_approved calls -- current-state
/// resolution orders by `effective_at_utc desc` first, so an absolute
/// timestamp computed before the intermediate transitions can sort *behind*
/// them and leave `paper_approved` (not `active_paper`) as the identity's
/// resolved "current" row, even though the active_paper POST itself
/// returned 200.
async fn promote_to_active_paper(
    st: Arc<state::AppState>,
    strategy_id: &str,
    symbol: &str,
    timeframe_secs: i64,
    review_dir: &Path,
    effective_at_offset: Duration,
    expires_at_offset: Option<Duration>,
) {
    let body = transition_body(
        strategy_id,
        symbol,
        timeframe_secs,
        "shadow_approved",
        Some(review_dir.to_str().unwrap()),
        Utc::now(),
        None,
    );
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "shadow_approved failed: {:?}",
        String::from_utf8_lossy(&resp_body)
    );

    let body = transition_body(
        strategy_id,
        symbol,
        timeframe_secs,
        "paper_approved",
        None,
        Utc::now(),
        None,
    );
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "paper_approved failed: {:?}",
        String::from_utf8_lossy(&resp_body)
    );

    let effective_at_utc = Utc::now() + effective_at_offset;
    let expires_at_utc = expires_at_offset.map(|d| effective_at_utc + d);
    let body = transition_body(
        strategy_id,
        symbol,
        timeframe_secs,
        "active_paper",
        None,
        effective_at_utc,
        expires_at_utc,
    );
    let (status, resp_body) = call(
        routes::build_router(Arc::clone(&st)),
        post_json_req(TRANSITION_ROUTE, body),
    )
    .await;
    assert_eq!(
        status,
        StatusCode::OK,
        "active_paper failed: {:?}",
        String::from_utf8_lossy(&resp_body)
    );
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn valid_active_paper_candidate_succeeds() {
    // `unique_id`/`temp_dir` are deterministic (UUIDv5 over a fixed string,
    // by design -- see their doc comments), so a shared, never-cleaned-up
    // MQK_DATABASE_URL database retains this test's promotion row forever
    // after the first successful run; every later run then finds the
    // identity already at `active_paper` and fails the very first
    // shadow_approved transition as "illegal_transition" (real failure
    // observed). A disposable per-test database removes that residue.
    mqk_db::run_isolated("dyn_sel_valid", |pool| async move {
        let root = temp_dir("valid");
        let strategy_id = unique_id("dyn_sel_valid");
        let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        promote_to_active_paper(
            Arc::clone(&st),
            &strategy_id,
            "AAPL",
            86400,
            &review_dir,
            Duration::zero(),
            None,
        )
        .await;

        let result =
            validate_active_paper_candidate(&pool, &st, &strategy_id, "AAPL", 86400, Utc::now())
                .await;
        let evidence = result.expect("expected Ok for a valid active_paper candidate");
        assert_eq!(evidence.canonical_score_decimal, "9");
        assert_eq!(evidence.canonical_score_micros, Some(9_000_000));
        assert_eq!(evidence.scanner_rank, Some(1));
        assert!(!evidence.recomputed_legacy_fingerprint.is_empty());
        assert!(!evidence.recomputed_exact_fingerprint_v2.is_empty());
        assert_eq!(
            evidence.durable_legacy_fingerprint,
            evidence.recomputed_legacy_fingerprint
        );
        assert_eq!(
            evidence.durable_exact_fingerprint_v2,
            evidence.recomputed_exact_fingerprint_v2
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn no_promotion_record_is_refused() {
    mqk_db::run_isolated("dyn_sel_no_record", |pool| async move {
        let root = temp_dir("no_record");
        let strategy_id = unique_id("dyn_sel_no_record");
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        let result =
            validate_active_paper_candidate(&pool, &st, &strategy_id, "AAPL", 86400, Utc::now())
                .await;
        assert_eq!(result, Err(CandidateEvidenceReason::NoPromotionRecord));
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn shadow_approved_is_not_active_paper() {
    mqk_db::run_isolated("dyn_sel_shadow", |pool| async move {
        let root = temp_dir("shadow_only");
        let strategy_id = unique_id("dyn_sel_shadow");
        let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        let body = transition_body(
            &strategy_id,
            "AAPL",
            86400,
            "shadow_approved",
            Some(review_dir.to_str().unwrap()),
            Utc::now(),
            None,
        );
        let (status, _) = call(
            routes::build_router(Arc::clone(&st)),
            post_json_req(TRANSITION_ROUTE, body),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let result =
            validate_active_paper_candidate(&pool, &st, &strategy_id, "AAPL", 86400, Utc::now())
                .await;
        assert_eq!(
            result,
            Err(CandidateEvidenceReason::PromotionNotActivePaper)
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn not_yet_effective_is_refused() {
    mqk_db::run_isolated("dyn_sel_nye", |pool| async move {
        let root = temp_dir("not_yet_effective");
        let strategy_id = unique_id("dyn_sel_nye");
        let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        // effective_at_utc 2 minutes ahead -- inside the route's 5-minute
        // clock-skew tolerance, so the transition itself is accepted.
        promote_to_active_paper(
            Arc::clone(&st),
            &strategy_id,
            "AAPL",
            86400,
            &review_dir,
            Duration::minutes(2),
            None,
        )
        .await;

        // authority_ts before effective_at_utc.
        let result =
            validate_active_paper_candidate(&pool, &st, &strategy_id, "AAPL", 86400, Utc::now())
                .await;
        assert_eq!(
            result,
            Err(CandidateEvidenceReason::PromotionNotYetEffective)
        );
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn expired_is_refused() {
    mqk_db::run_isolated("dyn_sel_expired", |pool| async move {
        let root = temp_dir("expired");
        let strategy_id = unique_id("dyn_sel_expired");
        let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        promote_to_active_paper(
            Arc::clone(&st),
            &strategy_id,
            "AAPL",
            86400,
            &review_dir,
            Duration::zero(),
            Some(Duration::minutes(1)),
        )
        .await;

        // authority_ts far past expires_at (effective+1min, computed inside
        // the helper relative to its own call time) -- "+1 hour from now" is
        // safely past it regardless of the few milliseconds of test overhead
        // above.
        let authority_ts = Utc::now() + Duration::hours(1);
        let result =
            validate_active_paper_candidate(&pool, &st, &strategy_id, "AAPL", 86400, authority_ts)
                .await;
        assert_eq!(result, Err(CandidateEvidenceReason::PromotionExpired));
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn tampered_durable_fingerprint_is_refused() {
    mqk_db::run_isolated("dyn_sel_tampered", |pool| async move {
        let root = temp_dir("tampered_fingerprint");
        let strategy_id = unique_id("dyn_sel_tampered");
        let review_dir = write_fixture(&root, vec![paper_candidate(&strategy_id, "AAPL", "1D")]);
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        promote_to_active_paper(
            Arc::clone(&st),
            &strategy_id,
            "AAPL",
            86400,
            &review_dir,
            Duration::zero(),
            None,
        )
        .await;

        // Simulate durable drift: the artifact on disk still matches what was
        // originally validated, but the DB's evidence_fingerprint column has
        // since diverged (e.g. corruption, manual tampering).
        sqlx::query(
            "update sys_strategy_promotion_transitions set evidence_fingerprint = 'deliberately-wrong-fingerprint' \
             where strategy_id = $1 and symbol = $2 and timeframe_secs = $3 and evidence_fingerprint is not null",
        )
        .bind(&strategy_id)
        .bind("AAPL")
        .bind(86400i64)
        .execute(&pool)
        .await
        .expect("tamper update");

        let result =
            validate_active_paper_candidate(&pool, &st, &strategy_id, "AAPL", 86400, Utc::now())
                .await;
        assert_eq!(result, Err(CandidateEvidenceReason::FingerprintMismatch));
    })
    .await;
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; see module doc for run command"]
async fn missing_score_is_refused() {
    mqk_db::run_isolated("dyn_sel_no_score", |pool| async move {
        let root = temp_dir("missing_score");
        let strategy_id = unique_id("dyn_sel_no_score");
        let review_dir = write_fixture(
            &root,
            vec![paper_candidate_no_score(&strategy_id, "AAPL", "1D")],
        );
        let (st, _env_guard) = make_state_with_db(&root, pool.clone()).await;

        promote_to_active_paper(
            Arc::clone(&st),
            &strategy_id,
            "AAPL",
            86400,
            &review_dir,
            Duration::zero(),
            None,
        )
        .await;

        let result =
            validate_active_paper_candidate(&pool, &st, &strategy_id, "AAPL", 86400, Utc::now())
                .await;
        assert_eq!(result, Err(CandidateEvidenceReason::ScoreMissing));
    })
    .await;
}
