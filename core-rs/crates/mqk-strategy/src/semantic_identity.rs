//! STRATEGY-SEMANTIC-IDENTITY-SEAM-01 (S1): deterministic semantic identity
//! for an actual, instantiated strategy's decision-affecting configuration.
//!
//! `StrategySpec` (name + timeframe_secs) is a *registry* identity: it is
//! sufficient to route a bar to the right host, but it is NOT sufficient to
//! prove which decision-affecting configuration produced a given decision.
//! Two `IntradayScalperStrategy` instances with different `target_qty` (or
//! different sizing caps, or opposite `allow_short_signals`) return an
//! identical `StrategySpec` today, yet make materially different trading
//! decisions. [`Strategy::semantic_fingerprint`] closes that gap: it is a
//! canonical, versioned SHA-256 digest over exactly the fields that affect
//! `on_bar`'s decision, computed by the strategy implementation itself from
//! its own effective (post-normalization) fields — never reconstructed by a
//! caller from Debug output, ambient environment state, or registry
//! metadata.
//!
//! # Determinism contract
//!
//! - identical effective semantics -> identical fingerprint
//! - any decision-affecting semantic mutation -> different fingerprint
//! - no wall clock, run_id, result values, artifact paths, or Debug/HashMap
//!   iteration ever participate
//!
//! [`SemanticIdentityBuilder`] is a small append-only, explicitly
//! length-prefixed byte encoder so that no two distinct field sequences can
//! ever collide onto the same byte stream (e.g. pushing `["ab", "c"]` can
//! never hash the same as `["a", "bc"]`).

use sha2::{Digest, Sha256};

/// Schema/version marker mixed into every fingerprint. Bumping the hashed
/// field set or encoding for any strategy requires bumping this constant so
/// a durably-stored fingerprint can never be silently reinterpreted under a
/// changed recipe.
pub const SEMANTIC_IDENTITY_SCHEMA_V1: &str = "mqk-strategy-semantic-identity-v1";

/// Append-only canonical byte encoder for strategy semantic identity
/// material. Every push is length- or tag-prefixed; nothing here ever
/// serializes via `Debug`, a `HashMap`, or float/decimal text formatting.
#[derive(Default)]
pub struct SemanticIdentityBuilder {
    buf: Vec<u8>,
}

impl SemanticIdentityBuilder {
    /// Start a new builder, seeding it with the schema marker and the
    /// engine's own name + version literals (never a caller-suppliable
    /// value).
    pub fn new(schema: &str, engine_name: &str, engine_version: &str) -> Self {
        let mut b = Self::default();
        b.push_str(schema);
        b.push_str(engine_name);
        b.push_str(engine_version);
        b
    }

    /// Push a length-prefixed UTF-8 string.
    pub fn push_str(&mut self, s: &str) -> &mut Self {
        let bytes = s.as_bytes();
        self.buf
            .extend_from_slice(&(bytes.len() as u64).to_be_bytes());
        self.buf.extend_from_slice(bytes);
        self
    }

    /// Push a fixed-width big-endian `i64`.
    pub fn push_i64(&mut self, v: i64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }

    /// Push an `Option<i64>` with an explicit presence tag so `None` can
    /// never collide with any representable `Some(_)` value.
    pub fn push_opt_i64(&mut self, v: Option<i64>) -> &mut Self {
        match v {
            Some(x) => {
                self.buf.push(1);
                self.push_i64(x);
            }
            None => self.buf.push(0),
        }
        self
    }

    /// Push a single boolean byte.
    pub fn push_bool(&mut self, v: bool) -> &mut Self {
        self.buf.push(v as u8);
        self
    }

    /// Finish: hex-encoded SHA-256 digest of the accumulated byte stream.
    pub fn finish(&self) -> String {
        let mut hasher = Sha256::new();
        hasher.update(&self.buf);
        hex::encode(hasher.finalize())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn encoding_is_not_ambiguous_across_field_boundaries() {
        let a = SemanticIdentityBuilder::new("s", "e", "v")
            .push_str("ab")
            .push_str("c")
            .finish();
        let b = SemanticIdentityBuilder::new("s", "e", "v")
            .push_str("a")
            .push_str("bc")
            .finish();
        assert_ne!(a, b, "length-prefixing must prevent field-boundary collisions");
    }

    #[test]
    fn same_inputs_produce_same_digest() {
        let a = SemanticIdentityBuilder::new("s", "e", "v")
            .push_i64(5)
            .push_opt_i64(Some(3))
            .push_bool(true)
            .finish();
        let b = SemanticIdentityBuilder::new("s", "e", "v")
            .push_i64(5)
            .push_opt_i64(Some(3))
            .push_bool(true)
            .finish();
        assert_eq!(a, b);
    }

    #[test]
    fn none_never_collides_with_a_representable_some() {
        let none_fp = SemanticIdentityBuilder::new("s", "e", "v")
            .push_opt_i64(None)
            .finish();
        let some_zero_fp = SemanticIdentityBuilder::new("s", "e", "v")
            .push_opt_i64(Some(0))
            .finish();
        assert_ne!(none_fp, some_zero_fp);
    }
}
