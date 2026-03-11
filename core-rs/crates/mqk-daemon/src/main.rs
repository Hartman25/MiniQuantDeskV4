//! mqk-daemon entry point.
//!
//! This file is intentionally thin: it sets up tracing, builds the shared
//! state, wires middleware, and starts the HTTP server. All route handlers
//! live in `routes.rs`; all shared state types live in `state.rs`.

#[path = "routes/control.rs"]
mod control;

use std::{sync::Arc, time::Duration};

use anyhow::Context;
use axum::http::{HeaderValue, Method};
use mqk_daemon::{bind, routes, state};
use tower_http::{
    cors::CorsLayer,
    trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer},
};
use tracing::{info, Level};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::from_filename(".env.local");

    init_tracing();

    let db = mqk_db::connect_from_env()
        .await
        .context("mqk-daemon requires MQK_DATABASE_URL for real runtime lifecycle control")?;

    let shared = Arc::new(state::AppState::new_with_db(db));
    state::spawn_heartbeat(shared.bus.clone(), Duration::from_secs(1));

    let app = routes::build_router(Arc::clone(&shared))
        .merge(control::router(Arc::clone(&shared)))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(cors_localhost_only());

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

/// CORS: allow only localhost origins.
fn cors_localhost_only() -> CorsLayer {
    let allowed_origins = [
        "http://localhost",
        "http://127.0.0.1",
        "http://localhost:3000",
        "http://127.0.0.1:3000",
        "http://localhost:5173",
        "http://127.0.0.1:5173",
        "http://localhost:1420",
        "http://127.0.0.1:1420",
    ];

    let origins: Vec<HeaderValue> = allowed_origins
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(tower_http::cors::Any)
}
