//! GUI frontend CORS policy for mqk-daemon.
//!
//! [`gui_cors_layer`] returns a [`CorsLayer`] that allows all known local
//! frontend origins — including Tauri webview origins that differ by OS:
//!
//! | Origin                     | Context                               |
//! |----------------------------|---------------------------------------|
//! | `http://localhost:1420`    | Vite dev server (Tauri dev mode)      |
//! | `http://127.0.0.1:1420`    | Vite dev server alt-address           |
//! | `tauri://localhost`        | Tauri webview on macOS / Linux        |
//! | `http://tauri.localhost`   | Tauri webview on Windows (HTTP)       |
//! | `https://tauri.localhost`  | Tauri webview on Windows (HTTPS)      |
//! | `http://localhost:5173`    | Vite standalone dev server            |
//! | `http://127.0.0.1:5173`    | Vite standalone dev server alt        |
//! | `http://localhost:3000`    | Generic dev server                    |
//! | `http://127.0.0.1:3000`    | Generic dev server alt                |
//! | `http://localhost`         | Direct localhost access               |
//! | `http://127.0.0.1`         | Direct loopback access                |
//!
//! All other origins receive no CORS headers (effectively blocked).
//! No wildcard is used; each origin is enumerated explicitly.

use axum::http::{HeaderValue, Method};
use tower_http::cors::CorsLayer;

/// All frontend origins the daemon accepts cross-origin requests from.
///
/// Enumerated explicitly — no wildcard.  Add new entries here when new
/// trusted local origins are introduced; removing an entry blocks that
/// origin immediately.
const ALLOWED_ORIGINS: &[&str] = &[
    // --- Tauri webview origins (must match exactly what the webview sends) ---
    "tauri://localhost",       // macOS / Linux Tauri webview
    "http://tauri.localhost",  // Windows Tauri webview (HTTP)
    "https://tauri.localhost", // Windows Tauri webview (HTTPS)
    // --- Tauri + Vite dev-mode server ---
    "http://localhost:1420",
    "http://127.0.0.1:1420",
    // --- Vite standalone ---
    "http://localhost:5173",
    "http://127.0.0.1:5173",
    // --- Generic dev server ---
    "http://localhost:3000",
    "http://127.0.0.1:3000",
    // --- Bare loopback ---
    "http://localhost",
    "http://127.0.0.1",
];

/// Build the GUI CORS layer with the full frontend origin allowlist.
///
/// Applied in `main.rs` as the outermost layer; also importable by
/// integration tests that need to validate CORS behaviour.
pub fn gui_cors_layer() -> CorsLayer {
    let origins: Vec<HeaderValue> = ALLOWED_ORIGINS
        .iter()
        .filter_map(|o| HeaderValue::from_str(o).ok())
        .collect();

    CorsLayer::new()
        .allow_origin(origins)
        .allow_methods([Method::GET, Method::POST])
        .allow_headers(tower_http::cors::Any)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: every entry in ALLOWED_ORIGINS parses as a valid HeaderValue.
    /// A parse failure here would silently drop the origin from the layer.
    #[test]
    fn all_allowed_origins_parse_as_header_values() {
        for origin in ALLOWED_ORIGINS {
            assert!(
                HeaderValue::from_str(origin).is_ok(),
                "ALLOWED_ORIGINS entry does not parse as HeaderValue: {origin}"
            );
        }
    }

    /// The required GUI frontend origins are all present in the allowlist.
    #[test]
    fn required_gui_origins_are_in_allowlist() {
        let required = [
            "http://localhost:1420",
            "http://127.0.0.1:1420",
            "tauri://localhost",
            "http://tauri.localhost",
            "https://tauri.localhost",
        ];
        for origin in required {
            assert!(
                ALLOWED_ORIGINS.contains(&origin),
                "required GUI origin missing from allowlist: {origin}"
            );
        }
    }
}
