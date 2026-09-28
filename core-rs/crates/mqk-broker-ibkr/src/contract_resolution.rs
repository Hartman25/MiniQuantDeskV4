//! C4 (V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01-CONTINUATION): IBKR
//! contract resolution, built on Wave C1's provider-neutral identity
//! (`mqk_schemas::ContractSpec`/`BrokerContractProvenance`).
//!
//! - Canonical Forex base/quote and explicit dated-future identity remain
//!   MQD's own (`ContractSpec`) — this module only ever *confirms* a
//!   specific broker-tradable instance of an already-canonical MQD shape,
//!   never redefines or substitutes for it (same "additive, not a second
//!   identity" contract C1's own `BrokerContractProvenance` doc states).
//! - `ContractSpec::Future.expiry_yyyymm` is validated against
//!   [`mqk_schemas::is_canonical_yyyymm`] *before* any lookup call — there
//!   is no continuous/perpetual `ContractSpec::Future` variant to validate
//!   against in the first place (C1: "no automatic futures roll... by
//!   construction"), and nothing in `mqk-backtest`/the research domain
//!   constructs one (verified: zero occurrences of a "continuous" contract
//!   concept anywhere in this workspace) — so a continuous research series
//!   is structurally incapable of reaching this resolver at all, not
//!   merely refused by convention.
//! - [`IbkrContractLookup`] is the injectable, mockable seam — no real IB
//!   Gateway lookup is made anywhere in this crate; every test here drives
//!   a fake.
//! - Zero matches and more than one match are both refused
//!   (`Unresolved`/`Ambiguous`) — this resolver never guesses which
//!   candidate the caller meant.

use mqk_schemas::{is_canonical_yyyymm, BrokerContractProvenance, ContractSpec};

/// One raw contract-details row as IBKR's contract-details lookup would
/// report it (`conId`/`localSymbol`/`exchange` — the exact fields C1's
/// `BrokerContractProvenance` already carries).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIbkrContractDetails {
    pub con_id: i64,
    pub local_symbol: String,
    pub exchange: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IbkrContractResolutionError {
    /// This resolver only handles `ContractSpec::Future`/`ContractSpec::Forex`
    /// — every other shape (Equity/Option/Crypto) is out of scope and
    /// refused before any lookup.
    UnsupportedContractSpec,
    /// `expiry_yyyymm` is not a canonical `YYYYMM` — refused before any
    /// lookup; a malformed or absent-month value is never coerced into a
    /// "nearest" or "front" contract.
    MalformedFutureExpiry { expiry_yyyymm: String },
    /// A currency code is not a well-formed 3-letter uppercase ISO-shaped
    /// code — refused before any lookup.
    MalformedCurrencyCode { field: &'static str, raw: String },
    /// The lookup returned zero candidates — never silently treated as "no
    /// position to worry about."
    Unresolved,
    /// The lookup returned more than one candidate — never guessed at by
    /// taking the first, the cheapest, or any other implicit tiebreak.
    Ambiguous { candidate_count: usize },
    /// The lookup transport itself failed (REST/socket error).
    TransportError(String),
}

impl std::fmt::Display for IbkrContractResolutionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedContractSpec => {
                write!(f, "unsupported ContractSpec for IBKR contract resolution")
            }
            Self::MalformedFutureExpiry { expiry_yyyymm } => {
                write!(f, "malformed future expiry_yyyymm: {expiry_yyyymm:?}")
            }
            Self::MalformedCurrencyCode { field, raw } => {
                write!(f, "malformed currency code in field {field:?}: {raw:?}")
            }
            Self::Unresolved => write!(f, "contract resolution found zero matching candidates"),
            Self::Ambiguous { candidate_count } => {
                write!(
                    f,
                    "contract resolution found {candidate_count} ambiguous candidates"
                )
            }
            Self::TransportError(detail) => write!(f, "contract lookup transport error: {detail}"),
        }
    }
}

impl std::error::Error for IbkrContractResolutionError {}

/// Injectable abstraction over IBKR's contract-details lookup. Production
/// implementation backed by a real `ibapi::Client` is explicit future work
/// (none exists in this crate); every test drives a fake.
pub trait IbkrContractLookup: Send + Sync {
    fn lookup(&self, spec: &ContractSpec) -> Result<Vec<RawIbkrContractDetails>, String>;
}

fn is_canonical_currency_code(code: &str) -> bool {
    code.len() == 3 && code.bytes().all(|b| b.is_ascii_uppercase())
}

/// Validate `spec`'s own MQD-canonical shape, then resolve it to exactly
/// one broker-confirmed [`BrokerContractProvenance`] via `lookup`. The
/// validation step never calls `lookup` at all for a malformed/unsupported
/// spec — proven directly by this module's own tests using a
/// panic-if-called fake.
pub fn resolve_contract(
    lookup: &dyn IbkrContractLookup,
    spec: &ContractSpec,
) -> Result<BrokerContractProvenance, IbkrContractResolutionError> {
    match spec {
        ContractSpec::Future { expiry_yyyymm, .. } => {
            if !is_canonical_yyyymm(expiry_yyyymm) {
                return Err(IbkrContractResolutionError::MalformedFutureExpiry {
                    expiry_yyyymm: expiry_yyyymm.clone(),
                });
            }
        }
        ContractSpec::Forex {
            base_currency,
            quote_currency,
        } => {
            for (field, code) in [
                ("base_currency", base_currency),
                ("quote_currency", quote_currency),
            ] {
                if !is_canonical_currency_code(code) {
                    return Err(IbkrContractResolutionError::MalformedCurrencyCode {
                        field,
                        raw: code.clone(),
                    });
                }
            }
        }
        _ => return Err(IbkrContractResolutionError::UnsupportedContractSpec),
    }

    let candidates = lookup
        .lookup(spec)
        .map_err(IbkrContractResolutionError::TransportError)?;

    match candidates.len() {
        0 => Err(IbkrContractResolutionError::Unresolved),
        1 => {
            let only = &candidates[0];
            Ok(BrokerContractProvenance {
                provider: "ibkr".to_string(),
                native_contract_id: only.con_id.to_string(),
                local_symbol: only.local_symbol.clone(),
                exchange: only.exchange.clone(),
            })
        }
        n => Err(IbkrContractResolutionError::Ambiguous { candidate_count: n }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedLookup(Vec<RawIbkrContractDetails>);

    impl IbkrContractLookup for FixedLookup {
        fn lookup(&self, _spec: &ContractSpec) -> Result<Vec<RawIbkrContractDetails>, String> {
            Ok(self.0.clone())
        }
    }

    /// Proves validation genuinely short-circuits before any lookup call —
    /// not merely "the returned error happens to be right" — by panicking
    /// if `lookup` is ever invoked.
    struct PanicIfCalledLookup;

    impl IbkrContractLookup for PanicIfCalledLookup {
        fn lookup(&self, _spec: &ContractSpec) -> Result<Vec<RawIbkrContractDetails>, String> {
            panic!(
                "IbkrContractLookup::lookup must never be called for a spec that fails \
                 validation before any lookup"
            )
        }
    }

    fn dated_future(expiry_yyyymm: &str) -> ContractSpec {
        ContractSpec::Future {
            root: "MES".to_string(),
            expiry_yyyymm: expiry_yyyymm.to_string(),
            multiplier: 5,
            tick_size_micros: 250_000,
        }
    }

    fn forex_pair(base: &str, quote: &str) -> ContractSpec {
        ContractSpec::Forex {
            base_currency: base.to_string(),
            quote_currency: quote.to_string(),
        }
    }

    #[test]
    fn single_candidate_future_resolves_to_provenance() {
        let lookup = FixedLookup(vec![RawIbkrContractDetails {
            con_id: 620_731_015,
            local_symbol: "MESM6".to_string(),
            exchange: "CME".to_string(),
        }]);
        let provenance = resolve_contract(&lookup, &dated_future("202606")).expect("must resolve");
        assert_eq!(provenance.provider, "ibkr");
        assert_eq!(provenance.native_contract_id, "620731015");
        assert_eq!(provenance.local_symbol, "MESM6");
        assert_eq!(provenance.exchange, "CME");
    }

    #[test]
    fn single_candidate_forex_resolves_to_provenance() {
        let lookup = FixedLookup(vec![RawIbkrContractDetails {
            con_id: 12_087_792,
            local_symbol: "EUR.USD".to_string(),
            exchange: "IDEALPRO".to_string(),
        }]);
        let provenance =
            resolve_contract(&lookup, &forex_pair("EUR", "USD")).expect("must resolve");
        assert_eq!(provenance.native_contract_id, "12087792");
        assert_eq!(provenance.exchange, "IDEALPRO");
    }

    #[test]
    fn zero_candidates_is_unresolved() {
        let lookup = FixedLookup(vec![]);
        let err = resolve_contract(&lookup, &dated_future("202606")).expect_err("must refuse");
        assert_eq!(err, IbkrContractResolutionError::Unresolved);
    }

    #[test]
    fn multiple_candidates_is_ambiguous_never_guessed() {
        let lookup = FixedLookup(vec![
            RawIbkrContractDetails {
                con_id: 1,
                local_symbol: "MESM6".to_string(),
                exchange: "CME".to_string(),
            },
            RawIbkrContractDetails {
                con_id: 2,
                local_symbol: "MESM6".to_string(),
                exchange: "CME".to_string(),
            },
        ]);
        let err = resolve_contract(&lookup, &dated_future("202606")).expect_err("must refuse");
        assert_eq!(
            err,
            IbkrContractResolutionError::Ambiguous { candidate_count: 2 }
        );
    }

    #[test]
    fn malformed_expiry_is_refused_and_lookup_is_never_called() {
        for bad in ["2026-06", "20266", "999913", "", "20260"] {
            let err = resolve_contract(&PanicIfCalledLookup, &dated_future(bad))
                .expect_err("must refuse before any lookup");
            assert_eq!(
                err,
                IbkrContractResolutionError::MalformedFutureExpiry {
                    expiry_yyyymm: bad.to_string()
                }
            );
        }
    }

    #[test]
    fn malformed_currency_code_is_refused_and_lookup_is_never_called() {
        for (base, quote) in [("eur", "USD"), ("EU", "USD"), ("", "USD"), ("EUR", "us")] {
            let err = resolve_contract(&PanicIfCalledLookup, &forex_pair(base, quote))
                .expect_err("must refuse before any lookup");
            assert!(matches!(
                err,
                IbkrContractResolutionError::MalformedCurrencyCode { .. }
            ));
        }
    }

    #[test]
    fn unsupported_contract_spec_is_refused_and_lookup_is_never_called() {
        let equity = ContractSpec::Equity;
        let err = resolve_contract(&PanicIfCalledLookup, &equity).expect_err("must refuse");
        assert_eq!(err, IbkrContractResolutionError::UnsupportedContractSpec);

        let crypto = ContractSpec::Crypto;
        let err = resolve_contract(&PanicIfCalledLookup, &crypto).expect_err("must refuse");
        assert_eq!(err, IbkrContractResolutionError::UnsupportedContractSpec);
    }

    /// C4's "reconnect/replay cannot duplicate broker events" requirement,
    /// applied to contract resolution: resolving the identical spec twice
    /// against a lookup that deterministically reports the identical
    /// candidate must produce byte-identical provenance — a caller that
    /// re-resolves after a reconnect never silently drifts to a "new"
    /// contract identity for what is genuinely the same broker contract.
    #[test]
    fn resolving_the_same_spec_twice_is_idempotent() {
        let lookup = FixedLookup(vec![RawIbkrContractDetails {
            con_id: 620_731_015,
            local_symbol: "MESM6".to_string(),
            exchange: "CME".to_string(),
        }]);
        let spec = dated_future("202606");
        let first = resolve_contract(&lookup, &spec).unwrap();
        let second = resolve_contract(&lookup, &spec).unwrap();
        assert_eq!(first, second);
    }
}
