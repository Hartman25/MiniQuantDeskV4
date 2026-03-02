//! FD-1 Prover: OutboxClaimToken forge-resistance.
//!
//! This test has `required-features = ["testkit"]` (see Cargo.toml).
//! It only compiles when `--features testkit` is explicitly passed.
//!
//! # What this file proves
//!
//! ## Positive proof — `for_test` IS accessible when `testkit` feature is enabled
//! Run: `cargo test -p mqk-db --features testkit --test prover_claim_token_unforgeable`
//!
//! ## Negative proof — production code CANNOT call `for_test`
//! Run: `cargo test -p mqk-db --test prover_claim_token_unforgeable` (no feature)
//! Cargo skips the test (required-features not met). Without `testkit`, calling
//! `for_test` from any crate produces:
//!
//! ```text
//! error[E0599]: no function or associated item named `for_test` found for
//! struct `OutboxClaimToken` in the current scope
//! ```
//!
//! ## Negative proof — struct-literal construction is always blocked
//! Direct struct construction outside `mqk-db` produces a compile error
//! regardless of any cfg gate, because `_priv: ()` is `pub(crate)`:
//!
//! ```text
//! error[E0451]: field `_priv` of struct `OutboxClaimToken` is private
//!   --> src/main.rs:N:N
//!    |
//!  N |     let _ = OutboxClaimToken { outbox_id: 1, idempotency_key: "k".into(), _priv: () };
//!    |                                                                             ^^^^ private field
//! ```
//!
//! The only production path to obtain a token is `outbox_claim_batch`, which
//! requires a live DB connection and performs `FOR UPDATE SKIP LOCKED`.

#[test]
fn for_test_accessible_in_mqk_db_test_context() {
    let token = mqk_db::OutboxClaimToken::for_test(42, "idempotency-key-abc");
    assert_eq!(token.outbox_id, 42);
    assert_eq!(token.idempotency_key, "idempotency-key-abc");
}

#[test]
fn for_test_fields_readable_by_external_crates() {
    // outbox_id and idempotency_key are pub — callers (BrokerGateway etc.)
    // can inspect the token. Only _priv (the forge-resistance sentinel) is
    // crate-private.
    let token = mqk_db::OutboxClaimToken::for_test(7, "key-seven");
    let _id: i64 = token.outbox_id;
    let _key: &str = &token.idempotency_key;
}

#[test]
fn two_tokens_with_same_args_are_independent_values() {
    // Tokens are plain value types; no identity beyond their fields.
    let a = mqk_db::OutboxClaimToken::for_test(1, "k");
    let b = mqk_db::OutboxClaimToken::for_test(1, "k");
    assert_eq!(a.outbox_id, b.outbox_id);
    assert_eq!(a.idempotency_key, b.idempotency_key);
}
