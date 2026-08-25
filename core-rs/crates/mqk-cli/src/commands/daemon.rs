//! CLI-DAEMON-CONTROL-PASSTHROUGH-01: thin CLI client for the existing
//! mqk-daemon operator/control HTTP routes.
//!
//! This module NEVER touches the database, a broker, or runtime state
//! directly. Every mutating action is a single `POST /api/v1/ops/action`
//! call carrying the same `action_key` literal the daemon's own dispatcher
//! already recognizes (see `mqk-daemon/src/routes/control_plane.rs`) and the
//! same `Authorization: Bearer <MQK_OPERATOR_TOKEN>` convention every other
//! accepted operator surface (GUI, PowerShell scripts) already uses. It is
//! not a second control authority -- it is a passthrough to the one that
//! already exists.
//!
//! A daemon response is never reinterpreted: a non-2xx status, an
//! unparseable body, or `accepted: false` are all reported as a truthful
//! CLI failure (nonzero exit via `anyhow::bail!`), never silently treated
//! as success.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use serde_json::Value;

const DEFAULT_DAEMON_BASE_URL: &str = "http://127.0.0.1:8899";

/// Resolve the daemon base URL: explicit `--base-url` > `MQK_DAEMON_URL` env
/// > the same default every existing PowerShell operator script uses.
pub fn resolve_daemon_base_url(base_url: Option<String>) -> String {
    if let Some(explicit) = base_url {
        let trimmed = explicit.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    if let Ok(env_url) = std::env::var("MQK_DAEMON_URL") {
        let trimmed = env_url.trim().trim_end_matches('/');
        if !trimmed.is_empty() {
            return trimmed.to_string();
        }
    }
    DEFAULT_DAEMON_BASE_URL.to_string()
}

/// Resolve the operator Bearer token. Fails closed (no HTTP call is made by
/// any caller of this function) when the token is not configured -- mirrors
/// the same `MQK_OPERATOR_TOKEN is not configured` fail-closed contract the
/// official launcher and GUI already enforce.
fn resolve_operator_token() -> Result<String> {
    std::env::var("MQK_OPERATOR_TOKEN")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .context(
            "MQK_OPERATOR_TOKEN is not configured; refusing to call a privileged daemon operator route",
        )
}

// ---------------------------------------------------------------------------
// status (read-only, unauthenticated -- mirrors Start-MiniQuantDesk.ps1's own
// GET /api/v1/system/status usage)
// ---------------------------------------------------------------------------

pub async fn daemon_status(base_url: Option<String>) -> Result<()> {
    let base = resolve_daemon_base_url(base_url);
    let url = format!("{base}/api/v1/system/status");

    let client = reqwest::Client::new();
    let resp = client
        .get(&url)
        .send()
        .await
        .with_context(|| format!("GET {url} failed (daemon unreachable)"))?;

    let status = resp.status();
    let raw_text = resp
        .text()
        .await
        .with_context(|| format!("GET {url}: failed to read response body"))?;

    // Print the daemon's real body verbatim (pretty-printed when it parses
    // as JSON) -- never a hand-copied/typed subset that could silently drop
    // a field the daemon added later.
    match serde_json::from_str::<Value>(&raw_text) {
        Ok(json) => println!("{}", serde_json::to_string_pretty(&json).unwrap_or(raw_text)),
        Err(_) => println!("{raw_text}"),
    }

    if !status.is_success() {
        bail!("daemon status request failed: HTTP {status}");
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Mutating operator actions -- POST /api/v1/ops/action
// ---------------------------------------------------------------------------

/// Narrow, defensive view of `mqk-daemon::api_types::OperatorActionResponse`.
/// Every field is optional/defaulted: an unrecognized or partial body still
/// deserializes (as `None`/empty), it is never treated as a parse failure
/// that would mask the real HTTP status, and no field is ever invented when
/// absent.
#[derive(Debug, Default, Deserialize)]
struct OperatorActionResponseView {
    #[serde(default)]
    requested_action: Option<String>,
    #[serde(default)]
    accepted: Option<bool>,
    #[serde(default)]
    disposition: Option<String>,
    #[serde(default)]
    blockers: Vec<String>,
    #[serde(default)]
    warnings: Vec<String>,
}

/// The one HTTP call every mutating `mqk daemon <verb>` command makes.
/// Never called for `status` (read-only, no token required).
async fn invoke_ops_action(base_url: Option<String>, action_key: &str) -> Result<()> {
    let base = resolve_daemon_base_url(base_url);
    let token = resolve_operator_token()?;
    let url = format!("{base}/api/v1/ops/action");

    let client = reqwest::Client::new();
    let resp = client
        .post(&url)
        .bearer_auth(token)
        .json(&serde_json::json!({ "action_key": action_key }))
        .send()
        .await
        .with_context(|| format!("POST {url} failed (daemon unreachable)"))?;

    let status = resp.status();
    let raw_text = resp
        .text()
        .await
        .with_context(|| format!("POST {url}: failed to read response body"))?;

    // Malformed/non-JSON/empty body -> `parsed` is None. Never reinterpreted
    // as an accepted action.
    let parsed: Option<OperatorActionResponseView> = serde_json::from_str(&raw_text).ok();

    match &parsed {
        Some(p) => {
            println!(
                "requested_action={} accepted={} disposition={}",
                p.requested_action.as_deref().unwrap_or(action_key),
                p.accepted.unwrap_or(false),
                p.disposition.as_deref().unwrap_or("unknown"),
            );
            for b in &p.blockers {
                println!("blocker: {b}");
            }
            for w in &p.warnings {
                println!("warning: {w}");
            }
        }
        None => {
            println!("HTTP {status} (no parseable operator-action response body)");
            if !raw_text.trim().is_empty() {
                println!("body: {raw_text}");
            }
        }
    }

    let accepted = status.is_success() && parsed.as_ref().and_then(|p| p.accepted).unwrap_or(false);

    if !accepted {
        let blocker_suffix = match &parsed {
            Some(p) if !p.blockers.is_empty() => format!(" blockers={:?}", p.blockers),
            _ => String::new(),
        };
        bail!("daemon refused action '{action_key}': HTTP {status}{blocker_suffix}");
    }

    Ok(())
}

pub async fn daemon_arm(base_url: Option<String>) -> Result<()> {
    invoke_ops_action(base_url, "arm-execution").await?;
    Ok(())
}

pub async fn daemon_disarm(base_url: Option<String>) -> Result<()> {
    invoke_ops_action(base_url, "disarm-execution").await?;
    Ok(())
}

/// Safety-sensitive: requires `--yes`. Refusal happens BEFORE any HTTP
/// request is made -- there is no mutation attempt to guard against, only a
/// refusal to attempt one.
pub async fn daemon_halt(base_url: Option<String>, yes: bool) -> Result<()> {
    if !yes {
        bail!("refusing to halt execution without --yes (safety-sensitive action; no HTTP request was made)");
    }
    invoke_ops_action(base_url, "kill-switch").await?;
    Ok(())
}

/// Safety-sensitive: requires `--yes`. Refusal happens BEFORE any HTTP
/// request is made.
pub async fn daemon_clear_halted_run(base_url: Option<String>, yes: bool) -> Result<()> {
    if !yes {
        bail!(
            "refusing to clear-halted-run without --yes (safety-sensitive action; no HTTP request was made)"
        );
    }
    invoke_ops_action(base_url, "clear-halted-run").await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    // cargo test runs test fns in parallel threads within this same binary.
    // Every test below that reads/mutates MQK_OPERATOR_TOKEN or
    // MQK_DAEMON_URL (both process-global state) takes this lock first, so
    // they run serialized with respect to each other -- never a torn read
    // of another test's in-flight env mutation. Tests that only exercise
    // the mocked HTTP path with an explicit `base_url` and a fixed token
    // value don't strictly need it, but taking it uniformly is cheap and
    // removes any need to reason about which subset is actually safe to
    // skip it.
    // A tokio (async-aware) mutex, not std::sync::Mutex: several tests below
    // hold this guard across a `.await` (the mocked HTTP call), which is the
    // documented misuse clippy's `await_holding_lock` flags for a std/
    // parking_lot guard. tokio::sync::MutexGuard is specifically designed to
    // be held across await points.
    static ENV_GUARD: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

    async fn lock_env() -> tokio::sync::MutexGuard<'static, ()> {
        ENV_GUARD.lock().await
    }

    /// SAFETY: only ever called while holding `ENV_GUARD`, so no other test
    /// in this module observes a torn read of a concurrent mutation. No
    /// daemon/broker/DB side effect -- this only ever sets a process-local
    /// env var read back by `resolve_operator_token`/`resolve_daemon_base_url`.
    fn set_env(key: &str, value: &str) {
        unsafe {
            std::env::set_var(key, value);
        }
    }

    /// SAFETY: see `set_env`.
    fn remove_env(key: &str) {
        unsafe {
            std::env::remove_var(key);
        }
    }

    // -------------------------------------------------------------------
    // resolve_daemon_base_url / resolve_operator_token
    // -------------------------------------------------------------------

    #[test]
    fn resolve_daemon_base_url_explicit_override_wins() {
        assert_eq!(
            resolve_daemon_base_url(Some("http://example.test:9999/".to_string())),
            "http://example.test:9999"
        );
    }

    #[test]
    fn resolve_daemon_base_url_default_matches_existing_powershell_scripts() {
        // No other test in this module touches MQK_DAEMON_URL, so this
        // fully-synchronous test needs no cross-test lock.
        let prior = std::env::var("MQK_DAEMON_URL").ok();
        remove_env("MQK_DAEMON_URL");
        assert_eq!(resolve_daemon_base_url(None), "http://127.0.0.1:8899");
        if let Some(p) = prior {
            set_env("MQK_DAEMON_URL", &p);
        }
    }

    // -------------------------------------------------------------------
    // C1: 409 + real blocker -> visible, propagated as failure
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn c1_409_blocker_is_printed_and_propagated_as_failure() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/ops/action");
            then.status(409).json_body(serde_json::json!({
                "requested_action": "clear-halted-run",
                "accepted": false,
                "disposition": "active_runtime_lease",
                "resulting_integrity_state": null,
                "resulting_desired_armed": null,
                "blockers": ["runtime.clear_halted.active_runtime_lease: unexpired lease"],
                "warnings": [],
                "environment": "paper",
                "scope": "daemon_instance",
                "audit": { "durable_db_write": false, "durable_targets": [], "audit_event_id": null },
                "pending_restart_intent": null,
                "captured_baseline": null
            }));
        });

        let _guard = lock_env().await;
        set_env("MQK_OPERATOR_TOKEN", "test-token");
        let err = daemon_clear_halted_run(Some(server.base_url()), true)
            .await
            .expect_err("409 refusal must be a CLI failure");
        let msg = format!("{err:#}");
        assert!(msg.contains("active_runtime_lease"), "message was: {msg}");
    }

    // -------------------------------------------------------------------
    // C2: daemon unreachable -> nonzero truthful error
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn c2_daemon_unreachable_is_a_truthful_error() {
        let _guard = lock_env().await;
        set_env("MQK_OPERATOR_TOKEN", "test-token");
        // Port 1 is reserved/unlikely to have a listener; connection refused.
        let err = daemon_arm(Some("http://127.0.0.1:1".to_string()))
            .await
            .expect_err("unreachable daemon must fail");
        let msg = format!("{err:#}");
        assert!(msg.contains("unreachable") || msg.contains("failed"), "message was: {msg}");
    }

    // -------------------------------------------------------------------
    // C3: malformed response body cannot be success
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn c3_malformed_body_cannot_be_success() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/ops/action");
            then.status(200).body("not valid json {{{");
        });

        let _guard = lock_env().await;
        set_env("MQK_OPERATOR_TOKEN", "test-token");
        let err = daemon_arm(Some(server.base_url()))
            .await
            .expect_err("malformed body on a 200 must still fail closed (no accepted:true parsed)");
        let msg = format!("{err:#}");
        assert!(msg.contains("daemon refused"), "message was: {msg}");
    }

    // -------------------------------------------------------------------
    // C4: unauthorized (401) response surfaced, not silently swallowed
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn c4_unauthorized_response_is_surfaced() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/ops/action");
            then.status(401).json_body(serde_json::json!({
                "error": "GATE_REFUSED: valid Bearer token required on operator routes",
                "gate": "operator_token"
            }));
        });

        let _guard = lock_env().await;
        set_env("MQK_OPERATOR_TOKEN", "test-token");
        let err = daemon_disarm(Some(server.base_url()))
            .await
            .expect_err("401 must be a CLI failure");
        let msg = format!("{err:#}");
        assert!(msg.contains("401"), "message was: {msg}");
    }

    // -------------------------------------------------------------------
    // C5: missing confirmation -> no HTTP mutation request is ever made
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn c5_halt_without_yes_makes_no_http_request() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        // If the CLI ever sent a request here, this mock would match and the
        // hits() assertion below would catch it.
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/ops/action");
            then.status(200).json_body(serde_json::json!({
                "requested_action": "kill-switch", "accepted": true, "disposition": "applied",
                "blockers": [], "warnings": []
            }));
        });

        let _guard = lock_env().await;
        set_env("MQK_OPERATOR_TOKEN", "test-token");
        let err = daemon_halt(Some(server.base_url()), false)
            .await
            .expect_err("halt without --yes must refuse");
        let msg = format!("{err:#}");
        assert!(msg.contains("--yes"), "message was: {msg}");
        mock.assert_hits(0);
    }

    #[tokio::test]
    async fn c5_clear_halted_run_without_yes_makes_no_http_request() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/ops/action");
            then.status(200).json_body(serde_json::json!({
                "requested_action": "clear-halted-run", "accepted": true, "disposition": "applied",
                "blockers": [], "warnings": []
            }));
        });

        let _guard = lock_env().await;
        set_env("MQK_OPERATOR_TOKEN", "test-token");
        let err = daemon_clear_halted_run(Some(server.base_url()), false)
            .await
            .expect_err("clear-halted-run without --yes must refuse");
        let msg = format!("{err:#}");
        assert!(msg.contains("--yes"), "message was: {msg}");
        mock.assert_hits(0);
    }

    // -------------------------------------------------------------------
    // Positive controls: arm / disarm / halt(--yes) / clear-halted-run(--yes)
    // route correctly and succeed on a genuine accepted:true response.
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn positive_arm_accepted_true_succeeds() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/ops/action")
                .json_body(serde_json::json!({ "action_key": "arm-execution" }))
                .header("authorization", "Bearer test-token");
            then.status(200).json_body(serde_json::json!({
                "requested_action": "arm-execution", "accepted": true, "disposition": "applied",
                "blockers": [], "warnings": []
            }));
        });

        let _guard = lock_env().await;
        set_env("MQK_OPERATOR_TOKEN", "test-token");
        daemon_arm(Some(server.base_url())).await.expect("arm must succeed");
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn positive_disarm_accepted_true_succeeds() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/ops/action")
                .json_body(serde_json::json!({ "action_key": "disarm-execution" }));
            then.status(200).json_body(serde_json::json!({
                "requested_action": "disarm-execution", "accepted": true, "disposition": "applied",
                "blockers": [], "warnings": []
            }));
        });

        let _guard = lock_env().await;
        set_env("MQK_OPERATOR_TOKEN", "test-token");
        daemon_disarm(Some(server.base_url())).await.expect("disarm must succeed");
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn positive_halt_with_yes_sends_kill_switch_and_succeeds() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/ops/action")
                .json_body(serde_json::json!({ "action_key": "kill-switch" }));
            then.status(200).json_body(serde_json::json!({
                "requested_action": "kill-switch", "accepted": true, "disposition": "applied",
                "blockers": [], "warnings": []
            }));
        });

        let _guard = lock_env().await;
        set_env("MQK_OPERATOR_TOKEN", "test-token");
        daemon_halt(Some(server.base_url()), true).await.expect("halt --yes must succeed");
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn positive_clear_halted_run_with_yes_succeeds() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST)
                .path("/api/v1/ops/action")
                .json_body(serde_json::json!({ "action_key": "clear-halted-run" }));
            then.status(200).json_body(serde_json::json!({
                "requested_action": "clear-halted-run", "accepted": true, "disposition": "applied",
                "blockers": [], "warnings": []
            }));
        });

        let _guard = lock_env().await;
        set_env("MQK_OPERATOR_TOKEN", "test-token");
        daemon_clear_halted_run(Some(server.base_url()), true)
            .await
            .expect("clear-halted-run --yes must succeed");
        mock.assert_hits(1);
    }

    // -------------------------------------------------------------------
    // Missing operator token: fails closed before any HTTP request.
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn missing_operator_token_refuses_before_any_http_call() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(POST).path("/api/v1/ops/action");
            then.status(200).json_body(serde_json::json!({
                "requested_action": "arm-execution", "accepted": true, "disposition": "applied",
                "blockers": [], "warnings": []
            }));
        });

        let _guard = lock_env().await;
        remove_env("MQK_OPERATOR_TOKEN");
        let err = daemon_arm(Some(server.base_url()))
            .await
            .expect_err("missing token must refuse");
        let msg = format!("{err:#}");
        assert!(msg.contains("MQK_OPERATOR_TOKEN"), "message was: {msg}");
        mock.assert_hits(0);
    }

    // -------------------------------------------------------------------
    // status: read-only, prints body, exit reflects HTTP status
    // -------------------------------------------------------------------
    #[tokio::test]
    async fn status_success_does_not_require_a_token() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let mock = server.mock(|when, then| {
            when.method(GET).path("/api/v1/system/status");
            then.status(200).json_body(serde_json::json!({
                "runtime_status": "running",
                "arm_state": "armed",
                "daemon_mode": "paper",
                "adapter_id": "alpaca",
                "live_routing_enabled": false,
                "kill_switch_active": false
            }));
        });

        let _guard = lock_env().await;
        remove_env("MQK_OPERATOR_TOKEN");
        daemon_status(Some(server.base_url())).await.expect("status must succeed without a token");
        mock.assert_hits(1);
    }

    #[tokio::test]
    async fn status_failure_http_is_truthful() {
        use httpmock::prelude::*;
        let server = MockServer::start();
        let _mock = server.mock(|when, then| {
            when.method(GET).path("/api/v1/system/status");
            then.status(503).body("service unavailable");
        });

        let err = daemon_status(Some(server.base_url()))
            .await
            .expect_err("503 must be a CLI failure");
        let msg = format!("{err:#}");
        assert!(msg.contains("503"), "message was: {msg}");
    }
}
