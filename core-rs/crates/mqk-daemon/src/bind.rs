//! S7-2: Loopback-only Default Bind
//!
//! Provides the canonical bind-address resolution for mqk-daemon.
//!
//! The daemon's default posture is loopback-only: it listens on
//! `127.0.0.1:8899` unless explicitly configured otherwise.  Exposing
//! the daemon on a non-loopback interface requires both:
//!
//! 1. Setting `MQK_DAEMON_ADDR` to the desired network address.
//! 2. Setting `MQK_DAEMON_ALLOW_NETWORK_BIND=1` as an explicit opt-in.
//!
//! Without the opt-in flag, any non-loopback `MQK_DAEMON_ADDR` causes
//! `resolve_bind_addr` to return an error and the daemon to refuse startup.
//! This prevents accidental exposure from a misconfigured environment.

use std::net::SocketAddr;

use anyhow::{bail, Context};

/// Default bind address: loopback interface, port 8899.
pub const DEFAULT_BIND_PORT: u16 = 8899;

/// Resolve the daemon bind address from configuration inputs.
///
/// # S7-2 Resolution Rules
///
/// | `daemon_addr`       | `allow_network_bind` | Result                          |
/// |---------------------|----------------------|---------------------------------|
/// | `None`              | any                  | `127.0.0.1:8899` (loopback)     |
/// | `Some(loopback)`    | any                  | parsed loopback address         |
/// | `Some(non-loopback)`| `false`              | `Err` — refuse to bind          |
/// | `Some(non-loopback)`| `true`               | parsed address (explicit opt-in)|
///
/// # Parameters
///
/// - `daemon_addr` — value of the `MQK_DAEMON_ADDR` env var (or `None`).
/// - `allow_network_bind` — `true` iff `MQK_DAEMON_ALLOW_NETWORK_BIND=1`.
///
/// # Errors
///
/// Returns `Err` when:
/// - `daemon_addr` is present but cannot be parsed as a `SocketAddr`.
/// - `daemon_addr` resolves to a non-loopback IP and `allow_network_bind`
///   is `false`.
pub fn resolve_bind_addr(
    daemon_addr: Option<&str>,
    allow_network_bind: bool,
) -> anyhow::Result<SocketAddr> {
    let addr = match daemon_addr {
        // No override → safe loopback default.
        None => return Ok(SocketAddr::from(([127, 0, 0, 1], DEFAULT_BIND_PORT))),
        Some(s) => s
            .parse::<SocketAddr>()
            .with_context(|| format!("invalid MQK_DAEMON_ADDR: {:?}", s))?,
    };

    if addr.ip().is_loopback() {
        // Loopback is always safe.
        return Ok(addr);
    }

    if allow_network_bind {
        // Explicit operator opt-in: accept any interface.
        Ok(addr)
    } else {
        bail!(
            "S7-2: refusing non-loopback bind address {} — \
             set MQK_DAEMON_ALLOW_NETWORK_BIND=1 to explicitly permit network exposure",
            addr
        )
    }
}

/// Read bind configuration from environment variables and call [`resolve_bind_addr`].
///
/// Reads:
/// - `MQK_DAEMON_ADDR` — optional bind address override.
/// - `MQK_DAEMON_ALLOW_NETWORK_BIND` — set to `1` or `true` to permit
///   non-loopback addresses.
pub fn resolve_bind_addr_from_env() -> anyhow::Result<SocketAddr> {
    let daemon_addr = std::env::var("MQK_DAEMON_ADDR").ok();
    let allow_network = std::env::var("MQK_DAEMON_ALLOW_NETWORK_BIND")
        .ok()
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    resolve_bind_addr(daemon_addr.as_deref(), allow_network)
}
