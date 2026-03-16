//! mqk-daemon entry point.
//!
//! This file is intentionally thin: it sets up tracing, builds the shared
//! state, wires middleware, and starts the HTTP server. All route handlers
//! live in `routes.rs`; all shared state types live in `state.rs`.

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use mqk_daemon::{bind, cors, routes, state};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::{info, warn, Level};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::from_filename(".env.local");

    init_tracing();

    let db = mqk_db::connect_from_env()
        .await
        .context("mqk-daemon requires MQK_DATABASE_URL for real runtime lifecycle control")?;

    let shared = Arc::new(state::AppState::new_with_db(db));
    match shared.operator_auth_mode() {
        state::OperatorAuthMode::TokenRequired(_) => {
            info!(
                operator_auth = shared.operator_auth_mode().label(),
                "operator auth configured; privileged routes require Bearer token"
            );
        }
        state::OperatorAuthMode::ExplicitDevNoToken => {
            warn!(operator_auth = shared.operator_auth_mode().label(), "explicit debug-only no-token operator mode enabled; do not treat loopback bind as sufficient authorization");
        }
        state::OperatorAuthMode::MissingTokenFailClosed => {
            warn!(operator_auth = shared.operator_auth_mode().label(), "operator token missing; privileged routes will fail closed until MQK_OPERATOR_TOKEN is configured");
        }
    }
    state::spawn_heartbeat(shared.bus.clone(), Duration::from_secs(1));

    let app = routes::build_router(Arc::clone(&shared))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(cors::gui_cors_layer());

    let addr = bind::resolve_bind_addr_from_env()?;
    info!("mqk-daemon listening on http://{}", addr);

    let shutdown_state = Arc::clone(&shared);
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app)
        .with_graceful_shutdown(async move {
            let _ = tokio::signal::ctrl_c().await;
            shutdown_state.stop_for_shutdown().await;
        })
        .await
        .context("server crashed")?;

    Ok(())
}

fn init_tracing() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
}
