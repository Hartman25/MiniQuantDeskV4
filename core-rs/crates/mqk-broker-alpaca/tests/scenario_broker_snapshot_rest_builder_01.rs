//! BROKER-SNAPSHOT-REST-BUILDER-ERROR-01 proof tests.
//!
//! Proves that `AlpacaBrokerAdapter::new()` trims whitespace and control
//! characters (CR, LF, tab, space) from all three `AlpacaConfig` fields so
//! that reqwest HTTP header construction never fails with an opaque
//! "builder error" due to CRLF-contaminated env var values.
//!
//! # Root cause
//!
//! When `.env.local` uses Windows CRLF line endings, `std::env::var()` returns
//! values with a trailing `\r`.  Passing `"key\r"` to
//! `reqwest::RequestBuilder::header()` causes reqwest to store an internal
//! build error and return it immediately on `.send()` — the error message is
//! literally `"builder error"`.  No TCP connection is ever attempted.
//!
//! The fix is in `AlpacaBrokerAdapter::new()`: trim `base_url`, `api_key_id`,
//! and `api_secret_key` before storing them.  After the fix, a connection
//! attempt is made and the error is a TCP-level failure (connection refused /
//! transport error) rather than a header-construction failure.
//!
//! # Test strategy
//!
//! All tests use `http://127.0.0.1:1` as the target URL.  Port 1 is
//! universally connection-refused and fails fast without TLS overhead.
//!
//! No real Alpaca credentials or network calls are used in any test.
//!
//! | Test              | What it proves                                                  |
//! |-------------------|-----------------------------------------------------------------|
//! | BSR_BUILDER_01    | CRLF in credentials → transport/auth error, NOT "builder error" |
//! | BSR_BUILDER_02    | base_url trailing CRLF is trimmed (visible via Debug)           |
//! | BSR_BUILDER_03    | Space/tab padding in credentials → not "builder error"          |
//! | BSR_BUILDER_04    | fetch_broker_snapshot with CRLF creds → not "builder error"     |
//! | BSR_BUILDER_05    | All-whitespace credential trimmed to empty → no panic           |
//! | BSR_BUILDER_06    | clean inputs are unchanged by trim (idempotent)                 |

use mqk_broker_alpaca::{AlpacaBrokerAdapter, AlpacaConfig};
use mqk_execution::BrokerError;

// ---------------------------------------------------------------------------
// Helper
// ---------------------------------------------------------------------------

fn adapter_with_creds(api_key_id: &str, api_secret_key: &str) -> AlpacaBrokerAdapter {
    AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url: "http://127.0.0.1:1".to_string(),
        api_key_id: api_key_id.to_string(),
        api_secret_key: api_secret_key.to_string(),
    })
}

/// Assert that a `BrokerError` is not a "builder error" (the opaque reqwest
/// error produced when a header value contains invalid control characters).
fn assert_not_builder_error(result: &Result<mqk_schemas::BrokerSnapshot, BrokerError>, ctx: &str) {
    if let Err(BrokerError::Transient { detail }) = result {
        assert!(
            !detail.contains("builder error"),
            "{ctx}: got 'builder error' — credential trim not applied. detail: {detail}"
        );
    }
    // Any other error variant (Transport, AuthSession, etc.) is acceptable —
    // they indicate the TCP/HTTP layer was reached, not header construction.
}

// ---------------------------------------------------------------------------
// BSR_BUILDER_01: CRLF in credentials → transport/auth error, NOT "builder error".
//
// If credentials contain `\r\n` and are NOT trimmed, reqwest fails with
// "builder error" before any TCP connection.  After the fix the error must be
// Transport (connection refused) or AuthSession (401) — anything except
// the opaque "builder error".
// ---------------------------------------------------------------------------

#[test]
fn bsr_builder_01_crlf_credentials_do_not_produce_builder_error() {
    let adapter = adapter_with_creds("test_key_id\r\n", "test_secret\r\n");
    let result = adapter.fetch_broker_snapshot(chrono::Utc::now());
    assert!(result.is_err(), "unreachable host must return Err");
    assert_not_builder_error(&result, "BSR_BUILDER_01");
}

// ---------------------------------------------------------------------------
// BSR_BUILDER_02: Trailing CRLF on base_url is trimmed (Debug shows clean URL).
//
// `AlpacaBrokerAdapter`'s Debug impl exposes `base_url`; we use that to verify
// the stored value has no trailing CR, LF, or space.
// ---------------------------------------------------------------------------

#[test]
fn bsr_builder_02_base_url_trailing_crlf_is_trimmed() {
    let adapter = AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url: "https://paper-api.alpaca.markets \r\n".to_string(),
        api_key_id: "key".to_string(),
        api_secret_key: "secret".to_string(),
    });
    let debug = format!("{adapter:?}");
    assert!(
        debug.contains("https://paper-api.alpaca.markets"),
        "base_url must appear in debug output: {debug}"
    );
    assert!(
        !debug.contains('\r'),
        "CR must not appear in debug output after trim: {debug}"
    );
    assert!(
        !debug.contains('\n'),
        "LF must not appear in debug output after trim: {debug}"
    );
    assert!(
        !debug.contains("markets "),
        "trailing space must be trimmed from base_url: {debug}"
    );
}

// ---------------------------------------------------------------------------
// BSR_BUILDER_03: Space/tab-padded credentials → not "builder error".
//
// Spaces and tabs are ASCII control-adjacent but are technically valid in
// HTTP header values.  Trimming them is still the right policy so the actual
// credential value is clean.
// ---------------------------------------------------------------------------

#[test]
fn bsr_builder_03_space_padded_credentials_do_not_produce_builder_error() {
    let adapter = adapter_with_creds("  test_key_id  ", "\tsecret_key\t");
    let result = adapter.fetch_broker_snapshot(chrono::Utc::now());
    assert!(result.is_err(), "unreachable host must return Err");
    assert_not_builder_error(&result, "BSR_BUILDER_03");
}

// ---------------------------------------------------------------------------
// BSR_BUILDER_04: fetch_broker_snapshot with CRLF creds → not "builder error".
//
// This is the exact production path exercised by `AlpacaSnapshotFetcher` in
// the daemon's adopt-broker-position-baseline route.
// ---------------------------------------------------------------------------

#[test]
fn bsr_builder_04_fetch_broker_snapshot_with_crlf_creds_does_not_produce_builder_error() {
    // Simulate env vars with Windows CRLF endings.
    let adapter = AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url: "http://127.0.0.1:1".to_string(),
        api_key_id: "APCA_PAPER_KEY\r".to_string(),
        api_secret_key: "APCA_PAPER_SECRET\r".to_string(),
    });
    let result = adapter.fetch_broker_snapshot(chrono::Utc::now());
    assert!(result.is_err(), "unreachable host must return Err");
    assert_not_builder_error(&result, "BSR_BUILDER_04");
}

// ---------------------------------------------------------------------------
// BSR_BUILDER_05: All-whitespace credential is trimmed to empty — no panic.
//
// An env var set to all spaces trims to "".  An empty header value is sent
// as-is; Alpaca will 401.  The important property is: no panic and no
// "builder error".
// ---------------------------------------------------------------------------

#[test]
fn bsr_builder_05_all_whitespace_credential_does_not_panic() {
    let adapter = adapter_with_creds("   ", " \r\n ");
    // Must not panic.  The error is acceptable.
    let result = adapter.fetch_broker_snapshot(chrono::Utc::now());
    assert!(result.is_err(), "unreachable host must return Err");
    assert_not_builder_error(&result, "BSR_BUILDER_05");
}

// ---------------------------------------------------------------------------
// BSR_BUILDER_06: Clean inputs are unchanged by trim (idempotent).
//
// Existing tests (live adapter, contract tests) rely on clean inputs being
// passed through unchanged.  Verify the Debug base_url is not mutated.
// ---------------------------------------------------------------------------

#[test]
fn bsr_builder_06_clean_inputs_are_unchanged_by_trim() {
    let adapter = AlpacaBrokerAdapter::new(AlpacaConfig {
        base_url: "https://paper-api.alpaca.markets".to_string(),
        api_key_id: "PKABCDEF1234".to_string(),
        api_secret_key: "supersecret".to_string(),
    });
    let debug = format!("{adapter:?}");
    assert!(
        debug.contains("https://paper-api.alpaca.markets"),
        "clean base_url must be unchanged: {debug}"
    );
    assert!(
        !debug.contains('\r') && !debug.contains('\n'),
        "debug must not contain control chars on clean input: {debug}"
    );
}
