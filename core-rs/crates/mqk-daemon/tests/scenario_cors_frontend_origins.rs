//! CORS policy integration tests for mqk-daemon.
//!
//! Verifies that `gui_cors_layer` returns `Access-Control-Allow-Origin` for
//! every required GUI frontend origin and blocks unknown origins.
//!
//! Uses an in-process Axum router — no TCP socket required.

use std::sync::Arc;

use axum::http::{Request, StatusCode};
use http_body_util::BodyExt;
use mqk_daemon::{cors, routes, state};
use tower::ServiceExt;

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Build a fresh in-process router with the GUI CORS layer attached,
/// matching the production wiring in main.rs.
fn make_cors_router() -> axum::Router {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    routes::build_router(st).layer(cors::gui_cors_layer())
}

/// Send a real preflight (OPTIONS) request with the given Origin header and
/// return the `access-control-allow-origin` response header value (if any).
async fn preflight_acao(origin: &str) -> Option<String> {
    let app = make_cors_router();
    let req = Request::builder()
        .method("OPTIONS")
        .uri("/api/v1/system/status")
        .header("Origin", origin)
        .header("Access-Control-Request-Method", "GET")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    resp.headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
}

/// Send a real GET request with the given Origin header and return the
/// `access-control-allow-origin` response header value (if any).
async fn get_acao(origin: &str) -> Option<String> {
    let app = make_cors_router();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .header("Origin", origin)
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    resp.headers()
        .get("access-control-allow-origin")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_owned())
}

// ---------------------------------------------------------------------------
// C1: Required GUI origins receive ACAO on GET
// ---------------------------------------------------------------------------

/// The five origins required by the GUI must receive Access-Control-Allow-Origin
/// on a real GET request.
#[tokio::test]
async fn required_gui_origins_receive_acao_on_get() {
    let required = [
        "http://localhost:1420",
        "http://127.0.0.1:1420",
        "tauri://localhost",
        "http://tauri.localhost",
        "https://tauri.localhost",
    ];

    for origin in required {
        let acao = get_acao(origin).await;
        assert!(
            acao.is_some(),
            "GET from required origin '{origin}' must receive Access-Control-Allow-Origin — got none"
        );
        assert_eq!(
            acao.as_deref().unwrap(),
            origin,
            "ACAO header must echo the exact origin for '{origin}'"
        );
    }
}

// ---------------------------------------------------------------------------
// C2: Required GUI origins receive ACAO on preflight
// ---------------------------------------------------------------------------

#[tokio::test]
async fn required_gui_origins_receive_acao_on_preflight() {
    let required = [
        "http://localhost:1420",
        "http://127.0.0.1:1420",
        "tauri://localhost",
        "http://tauri.localhost",
        "https://tauri.localhost",
    ];

    for origin in required {
        let acao = preflight_acao(origin).await;
        assert!(
            acao.is_some(),
            "preflight from required origin '{origin}' must receive Access-Control-Allow-Origin — got none"
        );
    }
}

// ---------------------------------------------------------------------------
// C3: Unknown origin receives no ACAO
// ---------------------------------------------------------------------------

#[tokio::test]
async fn unknown_origin_receives_no_acao() {
    let blocked = ["https://evil.example.com", "http://attacker.local", "null"];

    for origin in blocked {
        let acao = get_acao(origin).await;
        assert!(
            acao.is_none(),
            "unknown origin '{origin}' must NOT receive Access-Control-Allow-Origin — got: {acao:?}"
        );
    }
}

// ---------------------------------------------------------------------------
// C4: No wildcard — explicit origin echo only
// ---------------------------------------------------------------------------

#[tokio::test]
async fn acao_is_never_wildcard() {
    // Even for allowed origins, the value must be the specific origin, never "*".
    let allowed = [
        "http://localhost:1420",
        "tauri://localhost",
        "http://tauri.localhost",
    ];

    for origin in allowed {
        let acao = get_acao(origin).await;
        if let Some(val) = acao {
            assert_ne!(val, "*", "ACAO must not be wildcard for origin '{origin}'");
        }
    }
}

// ---------------------------------------------------------------------------
// C5: Vite dev-server origins also allowed
// ---------------------------------------------------------------------------

#[tokio::test]
async fn vite_dev_origins_receive_acao() {
    let vite_origins = [
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://localhost:3000",
        "http://127.0.0.1:3000",
        "http://localhost",
        "http://127.0.0.1",
    ];

    for origin in vite_origins {
        let acao = get_acao(origin).await;
        assert!(
            acao.is_some(),
            "Vite/dev origin '{origin}' must receive Access-Control-Allow-Origin — got none"
        );
    }
}

// ---------------------------------------------------------------------------
// C6: Requests without Origin header are unaffected (no ACAO returned)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_origin_header_returns_200_without_acao() {
    let app = make_cors_router();
    let req = Request::builder()
        .method("GET")
        .uri("/api/v1/system/status")
        .body(axum::body::Body::empty())
        .unwrap();

    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert!(
        resp.headers().get("access-control-allow-origin").is_none(),
        "no Origin header → no ACAO header expected"
    );

    // consume body to avoid leak warnings
    let _ = resp.into_body().collect().await.unwrap();
}
