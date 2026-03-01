//! S7-2: Loopback-only Default Bind
//!
//! Proves that the daemon's bind-address resolution enforces a loopback-only
//! default posture, preventing accidental network exposure.
//!
//! Five bind-policy properties tested:
//!
//! 1. **Default is `127.0.0.1:8899`** — when no `MQK_DAEMON_ADDR` is set
//!    the resolved address is the loopback interface on port 8899.
//!
//! 2. **Explicit loopback address is accepted** — `127.0.0.1:9000` (or any
//!    loopback variant) is accepted without requiring the allow-network flag.
//!
//! 3. **IPv6 loopback `[::1]` is accepted** — `[::1]:8899` resolves as a
//!    loopback address and is accepted without the allow-network flag.
//!
//! 4. **Non-loopback address without allow flag is rejected** — `0.0.0.0:8899`
//!    returns `Err` when `allow_network_bind = false`.  This prevents
//!    accidental exposure from a misconfigured `MQK_DAEMON_ADDR`.
//!
//! 5. **Non-loopback address with allow flag is accepted** — `0.0.0.0:8899`
//!    resolves successfully when `allow_network_bind = true`, providing a
//!    deliberate escape hatch for multi-machine deployment.

use mqk_daemon::bind::resolve_bind_addr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

// ---------------------------------------------------------------------------
// Bind policy 1: default is 127.0.0.1:8899
// ---------------------------------------------------------------------------

/// BIND 1 of 5.
///
/// `resolve_bind_addr(None, _)` must return `127.0.0.1:8899`.
/// This is the safe production default — the daemon never accidentally
/// binds to a network interface without an explicit configuration.
#[test]
fn default_bind_addr_is_loopback_127_0_0_1_port_8899() {
    let addr = resolve_bind_addr(None, false)
        .expect("default resolve must succeed");

    assert_eq!(
        addr.ip(),
        IpAddr::V4(Ipv4Addr::LOCALHOST),
        "default bind IP must be 127.0.0.1, got {}",
        addr.ip()
    );
    assert_eq!(
        addr.port(),
        8899,
        "default bind port must be 8899, got {}",
        addr.port()
    );
    assert!(
        addr.ip().is_loopback(),
        "default bind address must be loopback"
    );
}

// ---------------------------------------------------------------------------
// Bind policy 2: explicit loopback address is accepted
// ---------------------------------------------------------------------------

/// BIND 2 of 5.
///
/// An explicit `127.0.0.1:<port>` passed as `daemon_addr` must be accepted
/// without the `allow_network_bind` flag.  Operators who want a different
/// loopback port can set `MQK_DAEMON_ADDR=127.0.0.1:9000` freely.
#[test]
fn explicit_loopback_addr_is_accepted_without_allow_flag() {
    let addr = resolve_bind_addr(Some("127.0.0.1:9000"), false)
        .expect("explicit loopback addr must be accepted");

    assert!(
        addr.ip().is_loopback(),
        "resolved address must be loopback, got {}",
        addr
    );
    assert_eq!(addr.port(), 9000);
}

// ---------------------------------------------------------------------------
// Bind policy 3: IPv6 loopback [::1] is accepted
// ---------------------------------------------------------------------------

/// BIND 3 of 5.
///
/// `[::1]:8899` is an IPv6 loopback address.  It must be accepted without
/// the allow-network flag because it is still loopback — no network exposure.
#[test]
fn ipv6_loopback_addr_is_accepted_without_allow_flag() {
    let addr = resolve_bind_addr(Some("[::1]:8899"), false)
        .expect("IPv6 loopback [::1]:8899 must be accepted");

    assert_eq!(
        addr.ip(),
        IpAddr::V6(Ipv6Addr::LOCALHOST),
        "resolved IP must be ::1, got {}",
        addr.ip()
    );
    assert!(
        addr.ip().is_loopback(),
        "IPv6 loopback must satisfy is_loopback()"
    );
}

// ---------------------------------------------------------------------------
// Bind policy 4: non-loopback without allow flag is rejected
// ---------------------------------------------------------------------------

/// BIND 4 of 5.
///
/// `0.0.0.0:8899` would expose the daemon on ALL network interfaces.
/// Without `allow_network_bind = true`, `resolve_bind_addr` must return
/// `Err` — the daemon refuses to start rather than expose itself accidentally.
///
/// This is the core S7-2 safety invariant: accidental network exposure is
/// impossible by default; it requires deliberate operator action.
#[test]
fn non_loopback_addr_without_allow_flag_is_rejected() {
    let result = resolve_bind_addr(Some("0.0.0.0:8899"), false);

    assert!(
        result.is_err(),
        "non-loopback address without allow flag must return Err; got Ok({})",
        result.unwrap()
    );

    let msg = result.unwrap_err().to_string();
    assert!(
        msg.contains("S7-2"),
        "error message must mention S7-2, got: {msg}"
    );
    assert!(
        msg.contains("MQK_DAEMON_ALLOW_NETWORK_BIND"),
        "error message must name the override env var, got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// Bind policy 5: non-loopback with allow flag is accepted
// ---------------------------------------------------------------------------

/// BIND 5 of 5.
///
/// When `allow_network_bind = true` the operator has explicitly opted in to
/// network exposure.  `resolve_bind_addr` must accept the address, enabling
/// multi-machine deployment scenarios without fighting the safety default.
#[test]
fn non_loopback_addr_with_allow_flag_is_accepted() {
    let addr = resolve_bind_addr(Some("0.0.0.0:8899"), true)
        .expect("non-loopback with allow_network_bind=true must succeed");

    assert_eq!(
        addr.port(),
        8899,
        "resolved port must be 8899, got {}",
        addr.port()
    );
    assert!(
        !addr.ip().is_loopback(),
        "0.0.0.0 must not be treated as loopback"
    );
}
