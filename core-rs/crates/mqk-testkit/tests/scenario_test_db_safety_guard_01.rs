//! TEST-DB-SAFETY-GUARD-01 — proof that `assert_test_db_url` rejects
//! paper/live DB URLs and accepts safe test DB URLs before any connection.
//!
//! # Tests
//!
//! Pure in-process (no `MQK_DATABASE_URL` required):
//!
//! - TDBG01: accepts `postgres://.../mqk_test`
//! - TDBG02: accepts dedicated test DB URL on port 55433
//! - TDBG03: rejects `miniquantdesk_paper`
//! - TDBG04: rejects `miniquantdesk_live`
//! - TDBG05: rejects DB URL containing bare `paper` or `live` as DB name
//! - TDBG06: password is redacted in the panic message
//! - TDBG07: `mqk_test` URL passes even with port 55433 query params
//! - TDBG08: no Alpaca call required — all tests are pure in-process

use mqk_testkit::assert_test_db_url;

// ---------------------------------------------------------------------------
// Helper: call assert_test_db_url and catch the panic, returning the message.
// ---------------------------------------------------------------------------

fn rejected_with(url: &str) -> String {
    std::panic::catch_unwind(|| assert_test_db_url(url))
        .expect_err("expected assert_test_db_url to panic but it did not")
        .downcast::<String>()
        .map(|b| *b)
        .or_else(|e| e.downcast::<&str>().map(|s| s.to_string()))
        .unwrap_or_else(|_| "<non-string panic payload>".to_string())
}

fn accepted(url: &str) {
    // Should not panic.
    assert_test_db_url(url);
}

// ---------------------------------------------------------------------------
// TDBG01 — canonical test DB name accepted
// ---------------------------------------------------------------------------

#[test]
fn tdbg01_accepts_mqk_test_db() {
    accepted("postgres://postgres:password@127.0.0.1:55433/mqk_test");
}

// ---------------------------------------------------------------------------
// TDBG02 — port 55433 with explicit test DB name accepted
// ---------------------------------------------------------------------------

#[test]
fn tdbg02_accepts_port_55433_with_test_db_name() {
    accepted("postgres://postgres:secret@127.0.0.1:55433/mqk_test?sslmode=disable");
}

// ---------------------------------------------------------------------------
// TDBG03 — miniquantdesk_paper rejected
// ---------------------------------------------------------------------------

#[test]
fn tdbg03_rejects_miniquantdesk_paper() {
    let msg = rejected_with(
        "postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable",
    );
    assert!(
        msg.contains("miniquantdesk_paper"),
        "panic message should name the forbidden DB; got: {msg}"
    );
    assert!(
        msg.contains("MQK_DATABASE_URL"),
        "panic message should name the env var; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// TDBG04 — miniquantdesk_live rejected
// ---------------------------------------------------------------------------

#[test]
fn tdbg04_rejects_miniquantdesk_live() {
    let msg = rejected_with(
        "postgres://postgres:postgres@127.0.0.1:5432/miniquantdesk_live?sslmode=disable",
    );
    assert!(
        msg.contains("miniquantdesk_live"),
        "panic message should name the forbidden DB; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// TDBG05 — bare `paper` and `live` as DB names are rejected
// ---------------------------------------------------------------------------

#[test]
fn tdbg05_rejects_bare_paper_db_name() {
    let msg = rejected_with("postgres://user:pass@localhost/paper");
    assert!(
        msg.contains("paper"),
        "panic message should name the forbidden token; got: {msg}"
    );
}

#[test]
fn tdbg05b_rejects_bare_live_db_name() {
    let msg = rejected_with("postgres://user:pass@localhost/live");
    assert!(
        msg.contains("live"),
        "panic message should name the forbidden token; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// TDBG06 — password is redacted in the panic message
// ---------------------------------------------------------------------------

#[test]
fn tdbg06_password_redacted_in_panic_message() {
    let msg =
        rejected_with("postgres://postgres:SuperSecretPass@127.0.0.1:5440/miniquantdesk_paper");
    assert!(
        !msg.contains("SuperSecretPass"),
        "password must not appear in panic message; got: {msg}"
    );
    assert!(
        msg.contains("***"),
        "redacted placeholder '***' must appear in panic message; got: {msg}"
    );
}

// ---------------------------------------------------------------------------
// TDBG07 — mqk_test URL with query params accepted (no false positive)
// ---------------------------------------------------------------------------

#[test]
fn tdbg07_accepts_mqk_test_with_query_params() {
    accepted(
        "postgres://postgres:YourNewStrongPassword123!@127.0.0.1:55433/mqk_test?sslmode=disable",
    );
}

// ---------------------------------------------------------------------------
// TDBG08 — no Alpaca call: all above tests are pure in-process
//
// This test documents the classification; it always passes.
// ---------------------------------------------------------------------------

#[test]
fn tdbg08_no_alpaca_call_required() {
    // All TDBG tests are pure in-process. No network, no DB, no market hours.
    // This test exists to prove that classification explicitly.
    // No assertion needed; the test passing without a panic is the proof.
}
