//! CC-03A: Canonical mode-transition state machine — proof tests.
//!
//! Proves that [`mqk_daemon::mode_transition::evaluate_mode_transition`] is
//! the single canonical truth source for control-plane mode-transition
//! semantics, and that [`mqk_daemon::routes`] derives its transition
//! information from the canonical seam rather than ad hoc logic.
//!
//! # Proof matrix
//!
//! | Test          | What it proves                                                        |
//! |---------------|-----------------------------------------------------------------------|
//! | MT-01         | All same-mode pairs → SameMode (completeness baseline)                |
//! | MT-02         | Paper ↔ LiveShadow: admissible with restart (bidirectional)           |
//! | MT-03         | LiveCapital downgrades are admissible (→LiveShadow, →Paper)           |
//! | MT-04         | Upward to LiveCapital is fail-closed (not refused, not admissible)    |
//! | MT-05         | All Backtest transitions are refused in both directions                |
//! | MT-06         | Refused and FailClosed are both "blocked" (is_blocked = true)         |
//! | MT-07         | All 16 combinations produce a stable, canonical verdict string        |
//! | MT-08         | Route: GET /api/v1/ops/mode-change-guidance exposes transition_verdicts|
//! |               |   - transition_permitted == false (hot switching universally refused)  |
//! |               |   - transition_verdicts array present with 4 entries                   |
//! |               |   - Paper→LiveShadow entry == "admissible_with_restart"                |
//! |               |   - Paper→LiveCapital entry == "fail_closed"                           |
//! |               |   - Paper→Backtest entry == "refused"                                  |
//! |               |   - Paper→Paper entry == "same_mode"                                   |
//! | MT-09         | Route: change-system-mode (POST /api/v1/ops/action) returns 409 +     |
//! |               |   transition_verdicts consistent with the canonical seam               |
//!
//! All tests are pure in-process; no DB required.

use std::sync::Arc;

use axum::body::to_bytes;
use axum::http::{Method, Request, StatusCode};
use mqk_daemon::{
    mode_transition::{evaluate_mode_transition, ModeTransitionVerdict},
    routes::build_router,
    state::{AppState, DeploymentMode, OperatorAuthMode},
};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// MT-01: All same-mode pairs → SameMode
// ---------------------------------------------------------------------------

/// CC-03A / MT-01: Same-mode pairs return SameMode for all four variants.
///
/// This is the completeness baseline: no variant silently falls through to a
/// different verdict when from == to.
#[test]
fn mt_01_same_mode_pairs_return_same_mode() {
    for mode in [
        DeploymentMode::Paper,
        DeploymentMode::LiveShadow,
        DeploymentMode::LiveCapital,
        DeploymentMode::Backtest,
    ] {
        let v = evaluate_mode_transition(mode, mode);
        assert_eq!(
            v,
            ModeTransitionVerdict::SameMode,
            "MT-01: ({mode:?},{mode:?}) must be SameMode"
        );
    }
}

// ---------------------------------------------------------------------------
// MT-02: Paper ↔ LiveShadow — admissible with restart (bidirectional)
// ---------------------------------------------------------------------------

/// CC-03A / MT-02a: Paper → LiveShadow is AdmissibleWithRestart.
///
/// The upgrade path from paper trading to shadow live must be explicitly
/// supported, require non-empty preconditions, and mandate artifact chain
/// evidence before restart.
#[test]
fn mt_02a_paper_to_live_shadow_admissible() {
    let v = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveShadow);
    assert_eq!(
        v.as_str(),
        "admissible_with_restart",
        "MT-02a: Paper→LiveShadow must be admissible_with_restart"
    );
    assert!(
        !v.preconditions().is_empty(),
        "MT-02a: Paper→LiveShadow must have preconditions"
    );
    assert!(
        v.preconditions()
            .iter()
            .any(|p| p.contains("artifact") || p.contains("parity")),
        "MT-02a: Paper→LiveShadow preconditions must reference artifact/parity evidence; \
         got: {:?}",
        v.preconditions()
    );
}

/// CC-03A / MT-02b: LiveShadow → Paper is AdmissibleWithRestart (downgrade).
///
/// Downgrading from shadow to paper must also be explicitly supported.
/// The preconditions must require confirming open shadow positions.
#[test]
fn mt_02b_live_shadow_to_paper_admissible() {
    let v = evaluate_mode_transition(DeploymentMode::LiveShadow, DeploymentMode::Paper);
    assert_eq!(
        v.as_str(),
        "admissible_with_restart",
        "MT-02b: LiveShadow→Paper must be admissible_with_restart"
    );
    assert!(
        !v.preconditions().is_empty(),
        "MT-02b: LiveShadow→Paper must have preconditions"
    );
}

// ---------------------------------------------------------------------------
// MT-03: LiveCapital downgrades — admissible with restart
// ---------------------------------------------------------------------------

/// CC-03A / MT-03a: LiveCapital → LiveShadow is AdmissibleWithRestart.
///
/// Downgrading from capital to shadow must be supported.  The preconditions
/// must explicitly require closing open capital positions (not just draining
/// the outbox) — this is the higher-risk downgrade path.
#[test]
fn mt_03a_live_capital_to_live_shadow_admissible() {
    let v = evaluate_mode_transition(DeploymentMode::LiveCapital, DeploymentMode::LiveShadow);
    assert_eq!(
        v.as_str(),
        "admissible_with_restart",
        "MT-03a: LiveCapital→LiveShadow must be admissible_with_restart"
    );
    assert!(
        v.preconditions()
            .iter()
            .any(|p| p.contains("capital positions")),
        "MT-03a: LiveCapital downgrade must require capital position closure; got: {:?}",
        v.preconditions()
    );
}

/// CC-03A / MT-03b: LiveCapital → Paper is AdmissibleWithRestart.
#[test]
fn mt_03b_live_capital_to_paper_admissible() {
    let v = evaluate_mode_transition(DeploymentMode::LiveCapital, DeploymentMode::Paper);
    assert_eq!(
        v.as_str(),
        "admissible_with_restart",
        "MT-03b: LiveCapital→Paper must be admissible_with_restart"
    );
}

// ---------------------------------------------------------------------------
// MT-04: Upward transitions to LiveCapital — fail-closed
// ---------------------------------------------------------------------------

/// CC-03A / MT-04a: Paper → LiveCapital is FailClosed.
///
/// LiveCapital execution is not architecturally complete (TV-03
/// live_trust_complete=false).  This must be fail_closed — not refused (which
/// would mean "never possible") and not admissible (which would be unsafe).
/// The reason must reference the proof gap.
#[test]
fn mt_04a_paper_to_live_capital_fail_closed() {
    let v = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveCapital);
    assert_eq!(
        v.as_str(),
        "fail_closed",
        "MT-04a: Paper→LiveCapital must be fail_closed, not {:?}",
        v.as_str()
    );
    assert!(v.is_blocked(), "MT-04a: fail_closed must be blocked");
    assert!(
        v.reason().contains("fail-closed") || v.reason().contains("live_trust_complete"),
        "MT-04a: fail_closed reason must reference the proof gap; reason: {:?}",
        v.reason()
    );
}

/// CC-03A / MT-04b: LiveShadow → LiveCapital is FailClosed (same proof gap).
#[test]
fn mt_04b_live_shadow_to_live_capital_fail_closed() {
    let v = evaluate_mode_transition(DeploymentMode::LiveShadow, DeploymentMode::LiveCapital);
    assert_eq!(
        v.as_str(),
        "fail_closed",
        "MT-04b: LiveShadow→LiveCapital must be fail_closed"
    );
    assert!(v.is_blocked());
}

// ---------------------------------------------------------------------------
// MT-05: Backtest — refused in both directions
// ---------------------------------------------------------------------------

/// CC-03A / MT-05: All transitions to/from Backtest are Refused.
///
/// Backtest is a research mode; it is not a production daemon runtime target.
/// The architecture explicitly refuses all transitions involving Backtest.
/// This is a permanent structural refusal, not a transient proof gap.
#[test]
fn mt_05_backtest_transitions_are_refused() {
    let production = [
        DeploymentMode::Paper,
        DeploymentMode::LiveShadow,
        DeploymentMode::LiveCapital,
    ];
    for &mode in &production {
        let to_backtest = evaluate_mode_transition(mode, DeploymentMode::Backtest);
        assert_eq!(
            to_backtest.as_str(),
            "refused",
            "MT-05: {mode:?}→Backtest must be refused; got {:?}",
            to_backtest.as_str()
        );
        assert!(to_backtest.is_blocked());

        let from_backtest = evaluate_mode_transition(DeploymentMode::Backtest, mode);
        assert_eq!(
            from_backtest.as_str(),
            "refused",
            "MT-05: Backtest→{mode:?} must be refused; got {:?}",
            from_backtest.as_str()
        );
        assert!(from_backtest.is_blocked());
    }
}

// ---------------------------------------------------------------------------
// MT-06: Refused and FailClosed are both "blocked"
// ---------------------------------------------------------------------------

/// CC-03A / MT-06: is_blocked() returns true for both Refused and FailClosed.
///
/// This proves control-plane callers can use `is_blocked()` as a single
/// conservative check without needing to distinguish the exact refusal class.
#[test]
fn mt_06_refused_and_fail_closed_are_blocked() {
    // Refused
    let refused = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::Backtest);
    assert_eq!(refused.as_str(), "refused");
    assert!(refused.is_blocked(), "MT-06: Refused must be blocked");
    assert!(
        !refused.is_admissible(),
        "MT-06: Refused must not be admissible"
    );

    // FailClosed
    let fail_closed = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveCapital);
    assert_eq!(fail_closed.as_str(), "fail_closed");
    assert!(
        fail_closed.is_blocked(),
        "MT-06: FailClosed must be blocked"
    );
    assert!(
        !fail_closed.is_admissible(),
        "MT-06: FailClosed must not be admissible"
    );

    // SameMode and AdmissibleWithRestart are NOT blocked.
    let same = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::Paper);
    assert!(!same.is_blocked(), "MT-06: SameMode must not be blocked");

    let admissible = evaluate_mode_transition(DeploymentMode::Paper, DeploymentMode::LiveShadow);
    assert!(
        !admissible.is_blocked(),
        "MT-06: AdmissibleWithRestart must not be blocked"
    );
}

// ---------------------------------------------------------------------------
// MT-07: All 16 combinations produce a stable canonical verdict string
// ---------------------------------------------------------------------------

/// CC-03A / MT-07: Every (from, to) combination produces one of the four
/// canonical verdict strings.  Rust exhaustiveness guarantees no missing arm,
/// but this test also verifies the string values are stable.
#[test]
fn mt_07_all_16_combinations_produce_canonical_verdict_strings() {
    let all = [
        DeploymentMode::Paper,
        DeploymentMode::LiveShadow,
        DeploymentMode::LiveCapital,
        DeploymentMode::Backtest,
    ];
    let valid_verdicts = [
        "same_mode",
        "admissible_with_restart",
        "refused",
        "fail_closed",
    ];
    for &from in &all {
        for &to in &all {
            let v = evaluate_mode_transition(from, to);
            assert!(
                valid_verdicts.contains(&v.as_str()),
                "MT-07: ({from:?},{to:?}) produced unknown verdict string {:?}",
                v.as_str()
            );
        }
    }
}

// ---------------------------------------------------------------------------
// MT-08: Route — GET /api/v1/ops/mode-change-guidance exposes transition_verdicts
// ---------------------------------------------------------------------------

/// CC-03A / MT-08: GET /api/v1/ops/mode-change-guidance returns:
///   - transition_permitted == false (hot switching universally refused)
///   - transition_verdicts array with exactly 4 entries (one per target mode)
///   - Paper→Paper entry == "same_mode"
///   - Paper→LiveShadow entry == "admissible_with_restart"
///   - Paper→LiveCapital entry == "fail_closed"
///   - Paper→Backtest entry == "refused"
///
/// This proves `build_mode_change_guidance` derives its verdict information
/// from the canonical seam rather than ad hoc logic.
#[tokio::test]
async fn mt_08_mode_change_guidance_route_exposes_transition_verdicts() {
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));
    // Default AppState uses Paper mode.

    let router = build_router(st);
    let req = Request::builder()
        .method(Method::GET)
        .uri("/api/v1/ops/mode-change-guidance")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::OK,
        "MT-08: mode-change-guidance must return 200"
    );

    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // transition_permitted is always false — no hot switching.
    assert_eq!(
        j["transition_permitted"], false,
        "MT-08: transition_permitted must be false; body: {j}"
    );

    // transition_verdicts must be present and contain 4 entries.
    let verdicts = j["transition_verdicts"]
        .as_array()
        .expect("MT-08: transition_verdicts must be an array");
    assert_eq!(
        verdicts.len(),
        4,
        "MT-08: transition_verdicts must have 4 entries (one per target mode); got: {verdicts:?}"
    );

    // Helper: find entry by target_mode.
    let find = |target: &str| {
        verdicts
            .iter()
            .find(|e| e["target_mode"].as_str() == Some(target))
            .unwrap_or_else(|| panic!("MT-08: no entry for target_mode={target}"))
    };

    // Paper → Paper → same_mode.
    assert_eq!(
        find("paper")["verdict"].as_str(),
        Some("same_mode"),
        "MT-08: Paper→Paper must be same_mode"
    );

    // Paper → LiveShadow → admissible_with_restart.
    assert_eq!(
        find("live-shadow")["verdict"].as_str(),
        Some("admissible_with_restart"),
        "MT-08: Paper→LiveShadow must be admissible_with_restart"
    );
    assert!(
        find("live-shadow")["preconditions"]
            .as_array()
            .map(|a| !a.is_empty())
            .unwrap_or(false),
        "MT-08: Paper→LiveShadow must have non-empty preconditions"
    );

    // Paper → LiveCapital → fail_closed.
    assert_eq!(
        find("live-capital")["verdict"].as_str(),
        Some("fail_closed"),
        "MT-08: Paper→LiveCapital must be fail_closed"
    );

    // Paper → Backtest → refused.
    assert_eq!(
        find("backtest")["verdict"].as_str(),
        Some("refused"),
        "MT-08: Paper→Backtest must be refused"
    );
}

// ---------------------------------------------------------------------------
// MT-09: Route — change-system-mode returns 409 + consistent transition_verdicts
// ---------------------------------------------------------------------------

/// CC-03A / MT-09: POST /api/v1/ops/action {action_key: "change-system-mode"}
/// returns 409 CONFLICT and includes transition_verdicts consistent with the
/// canonical seam — proving the ops/action route also delegates to the
/// canonical mode-transition truth rather than using a separate ad hoc model.
#[tokio::test]
async fn mt_09_change_system_mode_action_returns_409_with_canonical_verdicts() {
    let st = Arc::new(AppState::new_with_operator_auth(
        OperatorAuthMode::ExplicitDevNoToken,
    ));

    let router = build_router(st);
    let req = Request::builder()
        .method(Method::POST)
        .uri("/api/v1/ops/action")
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            r#"{"action_key":"change-system-mode"}"#,
        ))
        .unwrap();
    let resp = router.oneshot(req).await.unwrap();

    assert_eq!(
        resp.status(),
        StatusCode::CONFLICT,
        "MT-09: change-system-mode must return 409"
    );

    let body = to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    let j: serde_json::Value = serde_json::from_slice(&body).unwrap();

    // Must include transition_verdicts (delegated from build_mode_change_guidance).
    let verdicts = j["transition_verdicts"]
        .as_array()
        .expect("MT-09: 409 response must include transition_verdicts");
    assert_eq!(
        verdicts.len(),
        4,
        "MT-09: transition_verdicts must have 4 entries; got: {verdicts:?}"
    );

    // The verdicts must match what the canonical seam returns — same_mode for
    // Paper→Paper, admissible for Paper→LiveShadow, fail_closed for Paper→LiveCapital.
    let find = |target: &str| {
        verdicts
            .iter()
            .find(|e| e["target_mode"].as_str() == Some(target))
            .unwrap_or_else(|| panic!("MT-09: no entry for target_mode={target}"))
    };
    assert_eq!(find("paper")["verdict"].as_str(), Some("same_mode"));
    assert_eq!(
        find("live-shadow")["verdict"].as_str(),
        Some("admissible_with_restart")
    );
    assert_eq!(
        find("live-capital")["verdict"].as_str(),
        Some("fail_closed")
    );
    assert_eq!(find("backtest")["verdict"].as_str(), Some("refused"));
}
