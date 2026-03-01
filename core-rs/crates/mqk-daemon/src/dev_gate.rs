//! S7-3: Disable Snapshot Inject in Release Builds
//!
//! The snapshot inject/clear endpoint (`POST /DELETE /v1/trading/snapshot`)
//! is a developer convenience that loads a fake broker snapshot into memory
//! so integration tests can exercise the trading read APIs without a live
//! broker connection.
//!
//! It must never be active in a production (release) binary — even if
//! `MQK_DEV_ALLOW_SNAPSHOT_INJECT=1` is accidentally set in the environment.
//!
//! This module provides a single gate function with two layers:
//!
//! 1. **Compile-time layer** — `#[cfg(not(debug_assertions))]` makes the
//!    function return `false` unconditionally in release builds.  The
//!    environment variable is not read at all; no runtime configuration
//!    can re-enable the endpoint.
//!
//! 2. **Runtime layer** — in debug builds, the env var
//!    `MQK_DEV_ALLOW_SNAPSHOT_INJECT` must also be set to `"1"` or `"true"`.
//!    An absent or falsy env var disables the endpoint even in debug builds.

/// Returns `true` iff snapshot injection is permitted in the current build
/// and environment.
///
/// # Compile-time guarantee (S7-3)
///
/// In **release builds** (`cfg(not(debug_assertions))`), this function always
/// returns `false`.  The `MQK_DEV_ALLOW_SNAPSHOT_INJECT` env var is never
/// read — no runtime configuration can re-enable the endpoint in prod.
///
/// In **debug builds**, returns `true` only when `MQK_DEV_ALLOW_SNAPSHOT_INJECT`
/// is `"1"` or `"true"`.
///
/// The equivalent of `debug_assertions` being `false` is when the crate is
/// compiled with `--release`.  Calling `cargo test --release` would also
/// exercise the compile-time gate, causing this function to return `false`
/// even when the env var is set.
pub fn snapshot_inject_allowed() -> bool {
    let env_val = std::env::var("MQK_DEV_ALLOW_SNAPSHOT_INJECT").ok();
    snapshot_inject_allowed_with_env(env_val.as_deref())
}

/// Pure, testable form of [`snapshot_inject_allowed`].
///
/// Accepts the env var value as a parameter so tests can exercise the logic
/// without racing on `std::env::set_var` across parallel test threads.
///
/// - `env_val = None`    → env var absent → `false`.
/// - `env_val = Some("1")` or `Some("true")` → `true` **in debug builds only**.
/// - **Release builds**: always returns `false` regardless of `env_val`.
pub fn snapshot_inject_allowed_with_env(env_val: Option<&str>) -> bool {
    // S7-3 compile-time gate: release builds can never enable this endpoint.
    #[cfg(not(debug_assertions))]
    {
        let _ = env_val; // suppress unused-variable warning in release
        return false;
    }

    // Debug builds: honour the env var.
    #[cfg(debug_assertions)]
    {
        env_val
            .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
            .unwrap_or(false)
    }
}
