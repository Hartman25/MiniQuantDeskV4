//! PAPER-HANDOFF-READONLY-01 / WATCHLIST-V2-SCHEMA-01: Read-only daemon-side
//! watchlist artifact loader.
//!
//! Loads a `watchlist-v1`, `watchlist-v2`, or `watchlist-v3` JSON artifact
//! produced by the Python scanner promotion pipeline (WATCHLIST-PROMO-01 /
//! WATCHLIST-PREMKT-RISK-BUNDLE-01) or hand-authored (v3) and returns one of
//! seven honest outcomes:
//!
//! - **`NotConfigured`** — `MQK_PAPER_WATCHLIST_PATH` env var is absent or empty.
//!   No watchlist is in scope.
//! - **`Missing`** — path configured but file does not exist at that path.
//!   Fail-closed; approved_for_autonomous_paper=false.
//! - **`Invalid`** — file found but structurally invalid (malformed JSON,
//!   unsupported schema_version, mode != paper, approved_for_live=true,
//!   missing required fields, constraint violations).  Fail-closed.
//! - **`LoadedNotApproved`** — v1/v2 file valid but `approved_for_autonomous_paper=false`.
//!   Operator must re-run promotion before the watchlist is usable.
//! - **`LoadedApproved`** — v1/v2 file valid and `approved_for_autonomous_paper=true`.
//!   Top symbol and strategy assignment are surfaced for status visibility only.
//! - **`LoadedNotApprovedV3`** / **`LoadedApprovedV3`** — the v3 (explicit
//!   per-symbol multi-strategy) analogs, carrying a separate
//!   [`LoadedWatchlistArtifactV3`] artifact type (never a v1/v2
//!   [`LoadedWatchlistArtifact`] and never unified with it — see §2.3 of the
//!   frozen contract referenced below).
//!
//! # Schema versions
//! - **`watchlist-v1`** — single-symbol.  `max_symbols_to_trade` and
//!   `max_concurrent_positions` must both equal 1.
//! - **`watchlist-v2`** (WATCHLIST-V2-SCHEMA-01) — multi-symbol schema.
//!   `max_symbols_to_trade` may be in `1..=MULTI_SYMBOL_HARD_CEILING` (5);
//!   `max_concurrent_positions` may be in `1..=max_symbols_to_trade`; when
//!   `symbols.len()` exceeds a *valid* `max_symbols_to_trade`
//!   (`MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01`), the artifact is truncated
//!   to the first `max_symbols_to_trade` entries in artifact order rather
//!   than rejected — the dropped tail is surfaced on
//!   [`LoadedWatchlistArtifact::dropped_symbols`]; every entry in the FULL
//!   originally-requested `symbols` list, including any symbol that will be
//!   dropped by truncation, must have a corresponding `strategy_assignments`
//!   entry — truncation is decided only after that check passes.
//!
//!   v2 is schema/validation only in this patch — it does NOT implement
//!   runtime multi-symbol dispatch, does NOT modify `loop_runner.rs` or
//!   strategy dispatch in `state.rs`, and does NOT wire watchlist admission
//!   into the live signal path.
//!
//! # Hard live-lock invariants (v1 and v2)
//! - `approved_for_live` is ALWAYS false in all outcomes.
//! - Any artifact with `approved_for_live=true` → `Invalid` (hard live lock).
//! - v1: `max_symbols_to_trade` must be 1 — anything else → `Invalid`.
//! - v1: `max_concurrent_positions` must be 1 — anything else → `Invalid`.
//! - v2: `max_symbols_to_trade` must be in `1..=MULTI_SYMBOL_HARD_CEILING`.
//! - v2: `max_concurrent_positions` must be in `1..=max_symbols_to_trade`.
//!
//! # Safety
//! - Pure function: no env reads, no network, no DB in `evaluate_watchlist_intake`.
//! - `evaluate_watchlist_intake_from_env` is the production entry point.
//! - No broker, OMS, outbox, inbox, or portfolio imports.
//! - This patch does NOT wire the outcome into order creation or signal admission.
//!   That is deferred to PAPER-HANDOFF-ENFORCE-01.
//!
//! # Signal admission dry contract
//! [`evaluate_watchlist_signal_admission`] is a pure, unwired helper that
//! future patches (PAPER-HANDOFF-ENFORCE-01) can promote into the live signal
//! path.  In this patch it is only reachable via tests.

use std::path::Path;

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// Env var the operator sets to the path of the promoted watchlist JSON file.
///
/// Example:
/// `MQK_PAPER_WATCHLIST_PATH=/home/user/exports/watchlist/AAPL_20260606.json`
pub const ENV_PAPER_WATCHLIST_PATH: &str = "MQK_PAPER_WATCHLIST_PATH";

/// Single-symbol schema version (original).  `max_symbols_to_trade` and
/// `max_concurrent_positions` must both equal [`REQUIRED_MAX_SYMBOLS`] /
/// [`REQUIRED_MAX_CONCURRENT`] (both 1).
pub const WATCHLIST_SCHEMA_VERSION_V1: &str = "watchlist-v1";

/// Multi-symbol schema version (WATCHLIST-V2-SCHEMA-01).  Allows
/// `max_symbols_to_trade` and `max_concurrent_positions` greater than 1, up
/// to [`MULTI_SYMBOL_HARD_CEILING`], and requires a `strategy_assignments`
/// entry for every symbol.  Validation only — does not enable runtime
/// multi-symbol dispatch.
pub const WATCHLIST_SCHEMA_VERSION_V2: &str = "watchlist-v2";

/// Back-compat alias for the original (v1) schema version constant.
pub const WATCHLIST_SCHEMA_VERSION: &str = WATCHLIST_SCHEMA_VERSION_V1;

/// Explicit per-symbol multi-strategy schema version
/// (`MULTI-STRATEGY-RUNTIME-DISPATCH-01`, frozen contract:
/// `docs/specs/multi_strategy_runtime_dispatch_01a_frozen_contract.md`).
/// `strategy_assignments` is `symbol -> Vec<String>` (one or more strategy
/// identities) instead of v1/v2's `symbol -> String`. A wholly separate,
/// additive evaluation path ([`evaluate_watchlist_intake_v3`]) — v1/v2
/// parsing via [`evaluate_watchlist_intake`] is unchanged byte-for-byte.
pub const WATCHLIST_SCHEMA_VERSION_V3: &str = "watchlist-v3";

/// Hard ceiling on `max_symbols_to_trade` (and therefore on `symbols.len()`)
/// for `watchlist-v2`/`watchlist-v3` artifacts (native multi-symbol dispatch
/// design, cap #12).  v1 artifacts remain capped at [`REQUIRED_MAX_SYMBOLS`]
/// regardless of this value.
pub const MULTI_SYMBOL_HARD_CEILING: u64 = 5;

/// Hard ceiling on `strategy_assignments[symbol].len()` for `watchlist-v3`
/// artifacts — defense-in-depth bound on total isolated `StrategyHost`
/// construction/dispatch cost per tick (frozen contract §2.1).
pub const MAX_STRATEGIES_PER_SYMBOL: u64 = 3;

/// Hard constraint: v1 only allows one symbol to trade.
const REQUIRED_MAX_SYMBOLS: u64 = 1;

/// Hard constraint: v1 only allows one concurrent position.
const REQUIRED_MAX_CONCURRENT: u64 = 1;

// ---------------------------------------------------------------------------
// LoadedWatchlistArtifact
// ---------------------------------------------------------------------------

/// Validated content extracted from a `watchlist-v1` or `watchlist-v2`
/// artifact.
///
/// Only constructed when all structural checks pass.  Never carries
/// `approved_for_live=true` — that is enforced by [`evaluate_watchlist_intake`].
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedWatchlistArtifact {
    /// `"watchlist-v1"` or `"watchlist-v2"`.
    pub schema_version: String,
    /// Admitted symbols, already truncated to `max_symbols_to_trade` where
    /// applicable (see [`Self::dropped_symbols`]).  Exactly one entry in v1;
    /// up to [`MULTI_SYMBOL_HARD_CEILING`] entries in v2.
    pub symbols: Vec<String>,
    /// Top-ranked symbol (symbols[0]) if present.
    pub top_symbol: Option<String>,
    /// Strategy assignment map: symbol → strategy_id.
    pub strategy_assignments: std::collections::HashMap<String, String>,
    /// Always 1 in v1.  In v2, in `1..=MULTI_SYMBOL_HARD_CEILING` (enforced
    /// by validation).
    pub max_symbols_to_trade: u64,
    /// Always 1 in v1.  In v2, in `1..=max_symbols_to_trade` (enforced by
    /// validation).
    pub max_concurrent_positions: u64,
    /// Whether scanner-level promotion approved autonomous paper trading.
    pub approved_for_autonomous_paper: bool,
    /// v2 only, cap #1 (`max_symbols_to_trade`) truncate-and-surface
    /// (`MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01`): the tail of the
    /// artifact's original `symbols` array dropped because it exceeded
    /// `max_symbols_to_trade`, in the artifact's own order. Empty when no
    /// truncation occurred (including always, for v1). `symbols ++
    /// dropped_symbols` (in that order) reconstructs the artifact's
    /// originally-requested symbol list.
    pub dropped_symbols: Vec<String>,
}

// ---------------------------------------------------------------------------
// WatchlistIntakeOutcome
// ---------------------------------------------------------------------------

/// Result of evaluating a `watchlist-v1` artifact for daemon read-only intake.
///
/// `approved_for_live` is ALWAYS false for every variant — there is no
/// outcome in which live trading is authorized from this path.
#[derive(Debug, Clone, PartialEq)]
pub enum WatchlistIntakeOutcome {
    /// Env var `MQK_PAPER_WATCHLIST_PATH` is absent or empty.
    ///
    /// No watchlist is configured.  This is honest absence — not implicit
    /// permission to proceed without one.
    NotConfigured,

    /// Path is configured but the file does not exist at that path.
    ///
    /// Fail-closed: approved_for_autonomous_paper=false.
    Missing {
        /// The configured path that was not found.
        configured_path: String,
    },

    /// File found but structurally invalid.
    ///
    /// Reasons include: malformed JSON, wrong schema_version, mode != paper,
    /// approved_for_live=true (hard live lock), missing required fields, or
    /// constraint violations (max_symbols_to_trade != 1, etc.).
    ///
    /// Fail-closed: approved_for_autonomous_paper=false.
    Invalid {
        /// Human-readable reasons for all validation failures.
        failure_reasons: Vec<String>,
    },

    /// File is structurally valid but `approved_for_autonomous_paper=false`.
    ///
    /// Operator must re-run the scanner promotion pipeline.
    LoadedNotApproved {
        /// Validated artifact content.
        artifact: LoadedWatchlistArtifact,
    },

    /// File is structurally valid and `approved_for_autonomous_paper=true`.
    ///
    /// Top symbol and strategy assignment are surfaced for status visibility.
    /// This outcome does NOT authorize trading in this patch.
    LoadedApproved {
        /// Validated artifact content.
        artifact: LoadedWatchlistArtifact,
    },

    /// File is structurally valid `watchlist-v3`
    /// (`MULTI-STRATEGY-RUNTIME-DISPATCH-01`, frozen contract §2.3) but
    /// `approved_for_autonomous_paper=false`.  A separate variant from
    /// [`Self::LoadedNotApproved`] — never a v1/v2 `LoadedWatchlistArtifact`
    /// masquerading as v3, or vice versa.
    LoadedNotApprovedV3 { artifact: LoadedWatchlistArtifactV3 },

    /// File is structurally valid `watchlist-v3` and
    /// `approved_for_autonomous_paper=true`.  Does NOT by itself authorize
    /// trading — every binding still independently passes frozen contract §4.
    LoadedApprovedV3 { artifact: LoadedWatchlistArtifactV3 },
}

impl WatchlistIntakeOutcome {
    /// Status label for the `/api/v1/watchlist/status` surface.
    pub fn status_label(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Missing { .. } => "missing",
            Self::Invalid { .. } => "invalid",
            Self::LoadedNotApproved { .. } => "loaded_not_approved",
            Self::LoadedApproved { .. } => "loaded_approved",
            Self::LoadedNotApprovedV3 { .. } => "loaded_not_approved_v3",
            Self::LoadedApprovedV3 { .. } => "loaded_approved_v3",
        }
    }

    /// Whether `approved_for_autonomous_paper` is true.
    ///
    /// Only `LoadedApproved`/`LoadedApprovedV3` return true.  All other
    /// variants return false.
    pub fn approved_for_autonomous_paper(&self) -> bool {
        matches!(
            self,
            Self::LoadedApproved { .. } | Self::LoadedApprovedV3 { .. }
        )
    }

    /// `approved_for_live` is always false — hard invariant.
    pub fn approved_for_live(&self) -> bool {
        false
    }

    /// Return the validated v1/v2 artifact if present (`LoadedNotApproved` or
    /// `LoadedApproved`). Returns `None` for a v3 outcome — see
    /// [`Self::artifact_v3`] — never a silent type-punned fallback.
    pub fn artifact(&self) -> Option<&LoadedWatchlistArtifact> {
        match self {
            Self::LoadedNotApproved { artifact } | Self::LoadedApproved { artifact } => {
                Some(artifact)
            }
            _ => None,
        }
    }

    /// Return the validated v3 artifact if present (`LoadedNotApprovedV3` or
    /// `LoadedApprovedV3`). A separate accessor from [`Self::artifact`] — the
    /// two artifact types are deliberately not unified (frozen contract
    /// §2.3).
    pub fn artifact_v3(&self) -> Option<&LoadedWatchlistArtifactV3> {
        match self {
            Self::LoadedNotApprovedV3 { artifact } | Self::LoadedApprovedV3 { artifact } => {
                Some(artifact)
            }
            _ => None,
        }
    }

    /// Failure reasons if status is `Invalid`.
    pub fn failure_reasons(&self) -> &[String] {
        match self {
            Self::Invalid { failure_reasons } => failure_reasons.as_slice(),
            _ => &[],
        }
    }
}

// ---------------------------------------------------------------------------
// Pure evaluator
// ---------------------------------------------------------------------------

/// Evaluate a `watchlist-v1` or `watchlist-v2` artifact at `path` and return
/// the intake outcome.
///
/// Pure: no env-var reads, no network, no DB.
///
/// # Validation contract (applied in order; failures accumulate)
/// 1. `path` is `Some` and non-empty → otherwise `NotConfigured`.
/// 2. File readable → otherwise `Missing`.
/// 3. Valid JSON → otherwise `Invalid`.
/// 4. `schema_version` ∈ `{"watchlist-v1", "watchlist-v2"}` → otherwise
///    `Invalid` (`watchlist_schema_invalid`).
/// 5. `mode == "paper"` → otherwise `Invalid` (`watchlist_mode_not_paper`).
/// 6. `approved_for_live` must NOT be `true` → otherwise `Invalid`
///    (`watchlist_live_approval_forbidden`, hard live lock).
/// 7. `approved_for_autonomous_paper` must be a bool → otherwise `Invalid`
///    (`watchlist_approved_for_autonomous_paper_invalid`).
/// 8. `symbols` must be an array → otherwise `Invalid`
///    (`watchlist_symbols_invalid`).
/// 9. `strategy_assignments` must be an object → otherwise `Invalid`
///    (`watchlist_strategy_assignments_invalid`).
/// 10. `max_symbols_to_trade`:
///     - v1: must equal [`REQUIRED_MAX_SYMBOLS`] (1) → otherwise `Invalid`
///       (`watchlist_max_symbols_invalid`).
///     - v2: must be in `1..=MULTI_SYMBOL_HARD_CEILING` → values `< 1` are
///       `watchlist_max_symbols_invalid`; values `> MULTI_SYMBOL_HARD_CEILING`
///       are `watchlist_multi_symbol_ceiling_exceeded`.
/// 11. `max_concurrent_positions`:
///     - v1: must equal [`REQUIRED_MAX_CONCURRENT`] (1) → otherwise `Invalid`
///       (`watchlist_max_concurrent_positions_invalid`).
///     - v2: must be in `1..=max_symbols_to_trade` → otherwise `Invalid`
///       (`watchlist_max_concurrent_positions_invalid`).
/// 12. `symbols.len() <= max_symbols_to_trade`:
///     - v1: no truncation — exceeding the cap is `Invalid`
///       (`watchlist_max_symbols_invalid`), matching the v1 single-symbol
///       hard constraint.
///     - v2, when `max_symbols_to_trade` is itself valid (step 10 above
///       raised no reason): eligible for truncate-and-surface
///       (`MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01`) — see step 14 below.
///       Eligibility does not by itself raise `watchlist_max_symbols_invalid`.
///     - v2, when `max_symbols_to_trade` is itself invalid: no truncation
///       is attempted — `Invalid` (`watchlist_max_symbols_invalid`), same
///       as before this patch.
/// 13. v2 only: every entry in the FULL originally-requested `symbols` list
///     (i.e. before any step-14 truncation) must have a corresponding
///     `strategy_assignments` entry → otherwise `Invalid`
///     (`watchlist_strategy_assignment_missing`). A symbol that would
///     otherwise be dropped by truncation is not exempt: an unassigned
///     symbol anywhere in the requested list fails the whole artifact.
/// 14. Only once steps 1-13 have raised no reason: if step 12 found the
///     artifact eligible for truncate-and-surface, `symbols` is truncated to
///     the first `max_symbols_to_trade` entries in artifact order and the
///     dropped tail is recorded on
///     [`LoadedWatchlistArtifact::dropped_symbols`]. Otherwise a no-op.
/// 15. If `approved_for_autonomous_paper=true`, `symbols` must be non-empty
///     (internal consistency) → otherwise `Invalid`
///     (`watchlist_symbols_invalid`).
///
/// Multiple validation failures are accumulated before returning `Invalid`.
pub fn evaluate_watchlist_intake(path: Option<&Path>) -> WatchlistIntakeOutcome {
    let path = match path {
        None => return WatchlistIntakeOutcome::NotConfigured,
        Some(p) if p.as_os_str().is_empty() => return WatchlistIntakeOutcome::NotConfigured,
        Some(p) => p,
    };

    // Step 1: read file.
    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => {
            return WatchlistIntakeOutcome::Missing {
                configured_path: path.display().to_string(),
            }
        }
    };

    // Step 2: parse JSON.
    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return WatchlistIntakeOutcome::Invalid {
                failure_reasons: vec![format!("invalid JSON in '{}': {e}", path.display())],
            }
        }
    };

    // Route watchlist-v3 through the dedicated v3 evaluator before any
    // v1/v2-specific parsing below — v3 is a new validation branch of this
    // same canonical entry point (frozen contract §9), not a v1/v2
    // reinterpretation. v1/v2 parsing (steps below) is reached only when
    // schema_version is absent, unrecognized, or exactly v1/v2 — unchanged
    // byte-for-byte from before this branch existed.
    if j.get("schema_version").and_then(|v| v.as_str()) == Some(WATCHLIST_SCHEMA_VERSION_V3) {
        return match evaluate_watchlist_intake_v3(Some(path)) {
            WatchlistIntakeOutcomeV3::Missing { configured_path } => {
                WatchlistIntakeOutcome::Missing { configured_path }
            }
            WatchlistIntakeOutcomeV3::Invalid { failure_reasons } => {
                WatchlistIntakeOutcome::Invalid { failure_reasons }
            }
            WatchlistIntakeOutcomeV3::LoadedNotApproved { artifact } => {
                WatchlistIntakeOutcome::LoadedNotApprovedV3 { artifact }
            }
            WatchlistIntakeOutcomeV3::LoadedApproved { artifact } => {
                WatchlistIntakeOutcome::LoadedApprovedV3 { artifact }
            }
            // Provably unreachable here: `path` is `Some` (this function
            // already returned `NotConfigured` earlier otherwise) and
            // schema_version is confirmed == v3 above, so
            // evaluate_watchlist_intake_v3 cannot return NotConfigured or
            // NotV3 for this same path/contents. Fail closed rather than
            // panic in this canonical read path.
            WatchlistIntakeOutcomeV3::NotConfigured | WatchlistIntakeOutcomeV3::NotV3 => {
                WatchlistIntakeOutcome::Invalid {
                    failure_reasons: vec![
                        "watchlist_schema_invalid: internal routing invariant violated \
                         evaluating a watchlist-v3 artifact"
                            .to_string(),
                    ],
                }
            }
        };
    }

    // Accumulate all validation failures rather than stopping at first error.
    let mut reasons: Vec<String> = Vec::new();

    // Step 3: schema_version must be "watchlist-v1" or "watchlist-v2".
    let schema_version: String = match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == WATCHLIST_SCHEMA_VERSION_V1 || sv == WATCHLIST_SCHEMA_VERSION_V2 => {
            sv.to_string()
        }
        Some(other) => {
            reasons.push(format!(
                "watchlist_schema_invalid: unsupported schema_version '{other}'; expected \
                 '{WATCHLIST_SCHEMA_VERSION_V1}' or '{WATCHLIST_SCHEMA_VERSION_V2}'"
            ));
            WATCHLIST_SCHEMA_VERSION_V1.to_string()
        }
        None => {
            reasons.push("watchlist_schema_invalid: missing 'schema_version' field".to_string());
            WATCHLIST_SCHEMA_VERSION_V1.to_string()
        }
    };
    let is_v2 = schema_version == WATCHLIST_SCHEMA_VERSION_V2;

    // Step 4: mode must be "paper".
    match j.get("mode").and_then(|v| v.as_str()) {
        Some("paper") => {}
        Some(other) => reasons.push(format!(
            "watchlist_mode_not_paper: mode '{other}' is not allowed; only 'paper' is accepted"
        )),
        None => reasons.push("watchlist_mode_not_paper: missing 'mode' field".to_string()),
    }

    // Step 5: approved_for_live must NOT be true (hard live lock).
    match j.get("approved_for_live") {
        Some(v) if v.as_bool() == Some(true) => {
            reasons.push(
                "watchlist_live_approval_forbidden: approved_for_live=true in artifact; \
                 hard live lock — artifact is invalid"
                    .to_string(),
            );
        }
        _ => {} // false, null, missing, or non-bool → pass (fail-closed on true only)
    }

    // Step 6: approved_for_autonomous_paper must be a bool.
    let approved_for_paper = match j.get("approved_for_autonomous_paper") {
        Some(v) => match v.as_bool() {
            Some(b) => b,
            None => {
                reasons.push(
                    "watchlist_approved_for_autonomous_paper_invalid: field \
                     'approved_for_autonomous_paper' is not a boolean"
                        .to_string(),
                );
                false
            }
        },
        None => {
            reasons.push(
                "watchlist_approved_for_autonomous_paper_invalid: missing required field \
                 'approved_for_autonomous_paper'"
                    .to_string(),
            );
            false
        }
    };

    // Step 7: symbols must be an array.
    let mut symbols: Vec<String> = match j.get("symbols") {
        Some(v) => match v.as_array() {
            Some(arr) => arr
                .iter()
                .filter_map(|e| e.as_str().map(|s| s.to_string()))
                .collect(),
            None => {
                reasons
                    .push("watchlist_symbols_invalid: field 'symbols' is not an array".to_string());
                vec![]
            }
        },
        None => {
            reasons.push("watchlist_symbols_invalid: missing required field 'symbols'".to_string());
            vec![]
        }
    };

    // Step 8: strategy_assignments must be an object.
    let strategy_assignments: std::collections::HashMap<String, String> =
        match j.get("strategy_assignments") {
            Some(v) => match v.as_object() {
                Some(obj) => obj
                    .iter()
                    .filter_map(|(k, val)| val.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect(),
                None => {
                    reasons.push(
                        "watchlist_strategy_assignments_invalid: field 'strategy_assignments' \
                         is not an object"
                            .to_string(),
                    );
                    std::collections::HashMap::new()
                }
            },
            None => {
                reasons.push(
                    "watchlist_strategy_assignments_invalid: missing required field \
                     'strategy_assignments'"
                        .to_string(),
                );
                std::collections::HashMap::new()
            }
        };

    // Step 9: max_symbols_to_trade — v1 must equal 1; v2 must be in
    // 1..=MULTI_SYMBOL_HARD_CEILING.
    let max_symbols_to_trade: u64 = match j.get("max_symbols_to_trade").and_then(|v| v.as_u64()) {
        Some(n) => {
            if is_v2 {
                if n < 1 {
                    reasons.push(format!(
                        "watchlist_max_symbols_invalid: max_symbols_to_trade={n}; must be >= 1 in v2"
                    ));
                } else if n > MULTI_SYMBOL_HARD_CEILING {
                    reasons.push(format!(
                        "watchlist_multi_symbol_ceiling_exceeded: max_symbols_to_trade={n} \
                         exceeds MULTI_SYMBOL_HARD_CEILING={MULTI_SYMBOL_HARD_CEILING}"
                    ));
                }
            } else if n != REQUIRED_MAX_SYMBOLS {
                reasons.push(format!(
                    "watchlist_max_symbols_invalid: max_symbols_to_trade={n}; must be \
                     {REQUIRED_MAX_SYMBOLS} in v1"
                ));
            }
            n
        }
        None => {
            reasons.push(format!(
                "watchlist_max_symbols_invalid: missing or non-integer 'max_symbols_to_trade'; \
                 must be {REQUIRED_MAX_SYMBOLS} in v1 or 1..={MULTI_SYMBOL_HARD_CEILING} in v2"
            ));
            0
        }
    };

    // Step 10: max_concurrent_positions — v1 must equal 1; v2 must be in
    // 1..=max_symbols_to_trade.
    let max_concurrent_positions: u64 =
        match j.get("max_concurrent_positions").and_then(|v| v.as_u64()) {
            Some(n) => {
                if is_v2 {
                    if n < 1 || n > max_symbols_to_trade {
                        reasons.push(format!(
                            "watchlist_max_concurrent_positions_invalid: \
                             max_concurrent_positions={n}; must be between 1 and \
                             max_symbols_to_trade={max_symbols_to_trade} in v2"
                        ));
                    }
                } else if n != REQUIRED_MAX_CONCURRENT {
                    reasons.push(format!(
                        "watchlist_max_concurrent_positions_invalid: \
                         max_concurrent_positions={n}; must be {REQUIRED_MAX_CONCURRENT} in v1"
                    ));
                }
                n
            }
            None => {
                reasons.push(format!(
                    "watchlist_max_concurrent_positions_invalid: missing or non-integer \
                     'max_concurrent_positions'; must be {REQUIRED_MAX_CONCURRENT} in v1 or \
                     1..=max_symbols_to_trade in v2"
                ));
                0
            }
        };

    // Step 11 (MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01): determine whether
    // symbols.len() exceeds max_symbols_to_trade, and whether that excess is
    // eligible for truncate-and-surface rather than outright rejection. This
    // does NOT mutate `symbols` yet — validation below (step 12) must run
    // against the full originally-requested list, not a truncated one, or a
    // missing strategy assignment on the dropped tail would silently pass.
    // v1 (single-symbol) keeps the original fail-closed behavior —
    // truncating a v1 artifact would violate its exactly-one-symbol
    // invariant. An invalid max_symbols_to_trade (step 9 already raised a
    // reason for it) is not trustworthy as a truncation point, so it also
    // keeps the original fail-closed behavior.
    let max_symbols_to_trade_is_valid =
        (1..=MULTI_SYMBOL_HARD_CEILING).contains(&max_symbols_to_trade);
    let exceeds_cap = symbols.len() as u64 > max_symbols_to_trade;
    let eligible_for_truncation = exceeds_cap && is_v2 && max_symbols_to_trade_is_valid;
    if exceeds_cap && !eligible_for_truncation {
        reasons.push(format!(
            "watchlist_max_symbols_invalid: symbols.len()={} exceeds \
             max_symbols_to_trade={}",
            symbols.len(),
            max_symbols_to_trade
        ));
    }

    // Step 12: v2 only — every ORIGINALLY REQUESTED symbol (i.e. the full
    // pre-truncation list) must have a strategy_assignments entry. Checking
    // against the full requested list — not the post-truncation admitted
    // list — is load-bearing: a malformed dropped-tail symbol must still
    // fail the whole artifact, not be silently discarded by truncation.
    if is_v2 {
        let missing: Vec<&String> = symbols
            .iter()
            .filter(|s| !strategy_assignments.contains_key(*s))
            .collect();
        if !missing.is_empty() {
            reasons.push(format!(
                "watchlist_strategy_assignment_missing: symbols missing \
                 strategy_assignments entries: {missing:?}"
            ));
        }
    }

    // Only after all unrelated validation above (steps 1-12) has been
    // evaluated may the effective symbol list actually be truncated.
    let mut dropped_symbols: Vec<String> = Vec::new();
    if eligible_for_truncation {
        dropped_symbols = symbols.split_off(max_symbols_to_trade as usize);
    }

    // Step 13: internal consistency — if approved, symbols must be non-empty.
    if approved_for_paper && symbols.is_empty() && reasons.is_empty() {
        reasons.push(
            "watchlist_symbols_invalid: approved_for_autonomous_paper=true but symbols list \
             is empty; internal consistency violation"
                .to_string(),
        );
    }

    // Return Invalid if any reasons accumulated.
    if !reasons.is_empty() {
        return WatchlistIntakeOutcome::Invalid {
            failure_reasons: reasons,
        };
    }

    // Artifact is structurally valid.
    let top_symbol = symbols.first().cloned();
    let artifact = LoadedWatchlistArtifact {
        schema_version,
        symbols,
        top_symbol,
        strategy_assignments,
        max_symbols_to_trade,
        max_concurrent_positions,
        approved_for_autonomous_paper: approved_for_paper,
        dropped_symbols,
    };

    if approved_for_paper {
        WatchlistIntakeOutcome::LoadedApproved { artifact }
    } else {
        WatchlistIntakeOutcome::LoadedNotApproved { artifact }
    }
}

// ---------------------------------------------------------------------------
// Production entry point (reads env var)
// ---------------------------------------------------------------------------

/// Read [`ENV_PAPER_WATCHLIST_PATH`] from the environment and evaluate intake.
///
/// Returns `NotConfigured` when the env var is absent or empty.
/// Delegates all validation to [`evaluate_watchlist_intake`].
pub fn evaluate_watchlist_intake_from_env() -> WatchlistIntakeOutcome {
    let raw = std::env::var(ENV_PAPER_WATCHLIST_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(raw.trim()))
    };
    evaluate_watchlist_intake(path.as_deref())
}

// ---------------------------------------------------------------------------
// MULTI-STRATEGY-RUNTIME-DISPATCH-01: watchlist-v3 (explicit per-symbol
// multi-strategy authorization). [`evaluate_watchlist_intake`] (the
// canonical entry point) now recognizes v3 and delegates to
// [`evaluate_watchlist_intake_v3`] below, mapping its outcome into
// [`WatchlistIntakeOutcome::LoadedApprovedV3`]/[`WatchlistIntakeOutcome::LoadedNotApprovedV3`].
// v1/v2 validation logic is untouched by this branch. The v3-specific
// validator/types below remain additive and separate from the v1/v2 ones —
// see docs/specs/multi_strategy_runtime_dispatch_01a_frozen_contract.md §2.3.
// ---------------------------------------------------------------------------

/// Validated content extracted from a `watchlist-v3` artifact. A separate
/// type from [`LoadedWatchlistArtifact`] — deliberately not a widened v1/v2
/// type, so a v1/v2-only caller can never be handed a `Vec`-valued
/// `strategy_assignments` it does not expect (frozen contract §2.3).
#[derive(Debug, Clone, PartialEq)]
pub struct LoadedWatchlistArtifactV3 {
    /// Always [`WATCHLIST_SCHEMA_VERSION_V3`].
    pub schema_version: String,
    /// Admitted symbols, already truncated to `max_symbols_to_trade` where
    /// applicable (see [`Self::dropped_symbols`]).
    pub symbols: Vec<String>,
    pub top_symbol: Option<String>,
    /// symbol -> ordered, non-empty list of authorized strategy identities
    /// (artifact JSON array order preserved — never re-sorted).
    pub strategy_assignments: std::collections::HashMap<String, Vec<String>>,
    pub max_symbols_to_trade: u64,
    pub max_concurrent_positions: u64,
    pub approved_for_autonomous_paper: bool,
    pub dropped_symbols: Vec<String>,
}

/// Result of evaluating a path for `watchlist-v3` intake. Mirrors
/// [`WatchlistIntakeOutcome`]'s five-outcome shape, plus one v3-specific
/// case: [`Self::NotV3`], reached when the file at `path` is well-formed but
/// its `schema_version` is not `"watchlist-v3"` (a v1/v2 artifact, or an
/// artifact this evaluator otherwise cannot recognize as v3). `NotV3` exists
/// only for routing (a caller that wants "v3 if configured, else fall back
/// to legacy/v2" — mirrors the existing precedent in
/// `build_multi_symbol_runtime_config_from_env_and_watchlist`); it is never
/// a v3 validation failure and must never be displayed as one.
///
/// `approved_for_live` is ALWAYS false for every variant — identical
/// hard-lock to v1/v2.
#[derive(Debug, Clone, PartialEq)]
pub enum WatchlistIntakeOutcomeV3 {
    /// `MQK_PAPER_WATCHLIST_PATH` is absent or empty.
    NotConfigured,
    /// Path is configured but the file does not exist.
    Missing { configured_path: String },
    /// File found, parses as JSON, and its `schema_version` field is
    /// exactly `"watchlist-v3"`, but is otherwise structurally invalid.
    Invalid { failure_reasons: Vec<String> },
    /// File found and parses as JSON, but `schema_version` is not
    /// `"watchlist-v3"` — routing signal only, see the enum's own docs.
    NotV3,
    /// Structurally valid v3 artifact, `approved_for_autonomous_paper=false`.
    LoadedNotApproved { artifact: LoadedWatchlistArtifactV3 },
    /// Structurally valid v3 artifact, `approved_for_autonomous_paper=true`.
    LoadedApproved { artifact: LoadedWatchlistArtifactV3 },
}

impl WatchlistIntakeOutcomeV3 {
    pub fn status_label(&self) -> &'static str {
        match self {
            Self::NotConfigured => "not_configured",
            Self::Missing { .. } => "missing",
            Self::Invalid { .. } => "invalid",
            Self::NotV3 => "not_v3",
            Self::LoadedNotApproved { .. } => "loaded_not_approved",
            Self::LoadedApproved { .. } => "loaded_approved",
        }
    }

    pub fn approved_for_autonomous_paper(&self) -> bool {
        matches!(self, Self::LoadedApproved { .. })
    }

    pub fn artifact(&self) -> Option<&LoadedWatchlistArtifactV3> {
        match self {
            Self::LoadedNotApproved { artifact } | Self::LoadedApproved { artifact } => {
                Some(artifact)
            }
            _ => None,
        }
    }
}

/// Evaluate a `watchlist-v3` artifact at `path`. Pure: no env-var reads, no
/// network, no DB — mirrors [`evaluate_watchlist_intake`]'s shape exactly,
/// with `strategy_assignments` parsed as `symbol -> Vec<String>` and one
/// additional per-symbol bound ([`MAX_STRATEGIES_PER_SYMBOL`]).
///
/// A file whose `schema_version` is not `"watchlist-v3"` returns
/// [`WatchlistIntakeOutcomeV3::NotV3`] — not `Invalid` — since it is not a
/// broken v3 artifact, merely not a v3 artifact at all (see the enum's own
/// docs on `NotV3`'s routing-only purpose).
pub fn evaluate_watchlist_intake_v3(path: Option<&Path>) -> WatchlistIntakeOutcomeV3 {
    let path = match path {
        None => return WatchlistIntakeOutcomeV3::NotConfigured,
        Some(p) if p.as_os_str().is_empty() => return WatchlistIntakeOutcomeV3::NotConfigured,
        Some(p) => p,
    };

    let contents = match std::fs::read_to_string(path) {
        Ok(s) => s,
        Err(_) => {
            return WatchlistIntakeOutcomeV3::Missing {
                configured_path: path.display().to_string(),
            }
        }
    };

    let j: serde_json::Value = match serde_json::from_str(&contents) {
        Ok(v) => v,
        Err(e) => {
            return WatchlistIntakeOutcomeV3::Invalid {
                failure_reasons: vec![format!("invalid JSON in '{}': {e}", path.display())],
            }
        }
    };

    match j.get("schema_version").and_then(|v| v.as_str()) {
        Some(sv) if sv == WATCHLIST_SCHEMA_VERSION_V3 => {}
        _ => return WatchlistIntakeOutcomeV3::NotV3,
    }

    let mut reasons: Vec<String> = Vec::new();

    match j.get("mode").and_then(|v| v.as_str()) {
        Some("paper") => {}
        Some(other) => reasons.push(format!(
            "watchlist_mode_not_paper: mode '{other}' is not allowed; only 'paper' is accepted"
        )),
        None => reasons.push("watchlist_mode_not_paper: missing 'mode' field".to_string()),
    }

    match j.get("approved_for_live") {
        Some(v) if v.as_bool() == Some(true) => {
            reasons.push(
                "watchlist_live_approval_forbidden: approved_for_live=true in artifact; \
                 hard live lock — artifact is invalid"
                    .to_string(),
            );
        }
        _ => {}
    }

    let approved_for_paper = match j.get("approved_for_autonomous_paper") {
        Some(v) => match v.as_bool() {
            Some(b) => b,
            None => {
                reasons.push(
                    "watchlist_approved_for_autonomous_paper_invalid: field \
                     'approved_for_autonomous_paper' is not a boolean"
                        .to_string(),
                );
                false
            }
        },
        None => {
            reasons.push(
                "watchlist_approved_for_autonomous_paper_invalid: missing required field \
                 'approved_for_autonomous_paper'"
                    .to_string(),
            );
            false
        }
    };

    let mut symbols: Vec<String> = match j.get("symbols") {
        Some(v) => match v.as_array() {
            Some(arr) => arr
                .iter()
                .filter_map(|e| e.as_str().map(|s| s.to_string()))
                .collect(),
            None => {
                reasons
                    .push("watchlist_symbols_invalid: field 'symbols' is not an array".to_string());
                vec![]
            }
        },
        None => {
            reasons.push("watchlist_symbols_invalid: missing required field 'symbols'".to_string());
            vec![]
        }
    };

    // strategy_assignments: symbol -> Vec<String> (v3's one structural
    // difference from v1/v2). Non-string / non-array entries are dropped
    // (mirrors v1/v2's filter_map convention) rather than causing a parse
    // panic; a resulting empty list still fails closed via the per-symbol
    // bound check below (§2.1: 0 is `watchlist_strategy_assignment_missing`).
    let strategy_assignments: std::collections::HashMap<String, Vec<String>> =
        match j.get("strategy_assignments") {
            Some(v) => match v.as_object() {
                Some(obj) => obj
                    .iter()
                    .map(|(k, val)| {
                        let list: Vec<String> = val
                            .as_array()
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|e| e.as_str().map(|s| s.to_string()))
                                    .collect()
                            })
                            .unwrap_or_default();
                        (k.clone(), list)
                    })
                    .collect(),
                None => {
                    reasons.push(
                        "watchlist_strategy_assignments_invalid: field 'strategy_assignments' \
                         is not an object"
                            .to_string(),
                    );
                    std::collections::HashMap::new()
                }
            },
            None => {
                reasons.push(
                    "watchlist_strategy_assignments_invalid: missing required field \
                     'strategy_assignments'"
                        .to_string(),
                );
                std::collections::HashMap::new()
            }
        };

    let max_symbols_to_trade: u64 = match j.get("max_symbols_to_trade").and_then(|v| v.as_u64()) {
        Some(n) => {
            if n < 1 {
                reasons.push(format!(
                    "watchlist_max_symbols_invalid: max_symbols_to_trade={n}; must be >= 1 in v3"
                ));
            } else if n > MULTI_SYMBOL_HARD_CEILING {
                reasons.push(format!(
                    "watchlist_multi_symbol_ceiling_exceeded: max_symbols_to_trade={n} \
                     exceeds MULTI_SYMBOL_HARD_CEILING={MULTI_SYMBOL_HARD_CEILING}"
                ));
            }
            n
        }
        None => {
            reasons.push(format!(
                "watchlist_max_symbols_invalid: missing or non-integer 'max_symbols_to_trade'; \
                 must be 1..={MULTI_SYMBOL_HARD_CEILING} in v3"
            ));
            0
        }
    };

    let max_concurrent_positions: u64 =
        match j.get("max_concurrent_positions").and_then(|v| v.as_u64()) {
            Some(n) => {
                if n < 1 || n > max_symbols_to_trade {
                    reasons.push(format!(
                        "watchlist_max_concurrent_positions_invalid: \
                         max_concurrent_positions={n}; must be between 1 and \
                         max_symbols_to_trade={max_symbols_to_trade} in v3"
                    ));
                }
                n
            }
            None => {
                reasons.push(
                    "watchlist_max_concurrent_positions_invalid: missing or non-integer \
                     'max_concurrent_positions'; must be 1..=max_symbols_to_trade in v3"
                        .to_string(),
                );
                0
            }
        };

    let max_symbols_to_trade_is_valid =
        (1..=MULTI_SYMBOL_HARD_CEILING).contains(&max_symbols_to_trade);
    let exceeds_cap = symbols.len() as u64 > max_symbols_to_trade;
    let eligible_for_truncation = exceeds_cap && max_symbols_to_trade_is_valid;
    if exceeds_cap && !eligible_for_truncation {
        reasons.push(format!(
            "watchlist_max_symbols_invalid: symbols.len()={} exceeds \
             max_symbols_to_trade={}",
            symbols.len(),
            max_symbols_to_trade
        ));
    }

    // §2.1: every ORIGINALLY REQUESTED symbol (pre-truncation) must have a
    // non-empty strategy_assignments list, bounded by
    // MAX_STRATEGIES_PER_SYMBOL. Checked against the full requested list —
    // not the post-truncation admitted list — mirroring v2's identical
    // "malformed dropped-tail symbol must still fail the whole artifact"
    // rule.
    for s in &symbols {
        match strategy_assignments.get(s) {
            None => reasons.push(format!(
                "watchlist_strategy_assignment_missing: symbol '{s}' has no \
                 strategy_assignments entry"
            )),
            Some(list) if list.is_empty() => reasons.push(format!(
                "watchlist_strategy_assignment_missing: symbol '{s}' has an empty \
                 strategy_assignments list"
            )),
            Some(list) if list.len() as u64 > MAX_STRATEGIES_PER_SYMBOL => {
                reasons.push(format!(
                    "watchlist_strategy_per_symbol_ceiling_exceeded: symbol '{s}' has \
                     {} strategy_assignments entries; exceeds MAX_STRATEGIES_PER_SYMBOL={}",
                    list.len(),
                    MAX_STRATEGIES_PER_SYMBOL
                ));
            }
            Some(_) => {}
        }
    }

    let mut dropped_symbols: Vec<String> = Vec::new();
    if eligible_for_truncation {
        dropped_symbols = symbols.split_off(max_symbols_to_trade as usize);
    }

    if approved_for_paper && symbols.is_empty() && reasons.is_empty() {
        reasons.push(
            "watchlist_symbols_invalid: approved_for_autonomous_paper=true but symbols list \
             is empty; internal consistency violation"
                .to_string(),
        );
    }

    if !reasons.is_empty() {
        return WatchlistIntakeOutcomeV3::Invalid {
            failure_reasons: reasons,
        };
    }

    let top_symbol = symbols.first().cloned();
    let artifact = LoadedWatchlistArtifactV3 {
        schema_version: WATCHLIST_SCHEMA_VERSION_V3.to_string(),
        symbols,
        top_symbol,
        strategy_assignments,
        max_symbols_to_trade,
        max_concurrent_positions,
        approved_for_autonomous_paper: approved_for_paper,
        dropped_symbols,
    };

    if approved_for_paper {
        WatchlistIntakeOutcomeV3::LoadedApproved { artifact }
    } else {
        WatchlistIntakeOutcomeV3::LoadedNotApproved { artifact }
    }
}

/// [`evaluate_watchlist_intake_v3`], reading [`ENV_PAPER_WATCHLIST_PATH`]
/// from the environment. Same env var as the v1/v2 path — a v3 artifact and
/// a v1/v2 artifact are never simultaneously configured (one file, one
/// `schema_version`).
pub fn evaluate_watchlist_intake_v3_from_env() -> WatchlistIntakeOutcomeV3 {
    let raw = std::env::var(ENV_PAPER_WATCHLIST_PATH).unwrap_or_default();
    let path = if raw.trim().is_empty() {
        None
    } else {
        Some(std::path::PathBuf::from(raw.trim()))
    };
    evaluate_watchlist_intake_v3(path.as_deref())
}

// ---------------------------------------------------------------------------
// Dry signal-admission contract (PAPER-HANDOFF-ENFORCE-01 seam)
// ---------------------------------------------------------------------------

/// Reason for a watchlist signal admission decision.
///
/// The string values are stable and suitable for logging / API surfaces.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchlistAdmissionReason {
    WatchlistNotConfigured,
    WatchlistMissing,
    WatchlistInvalid,
    WatchlistNotApproved,
    SymbolNotApproved,
    StrategyNotAssigned,
    Allowed,
}

impl WatchlistAdmissionReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::WatchlistNotConfigured => "watchlist_not_configured",
            Self::WatchlistMissing => "watchlist_missing",
            Self::WatchlistInvalid => "watchlist_invalid",
            Self::WatchlistNotApproved => "watchlist_not_approved",
            Self::SymbolNotApproved => "symbol_not_approved",
            Self::StrategyNotAssigned => "strategy_not_assigned",
            Self::Allowed => "allowed",
        }
    }
}

/// Result of the dry signal-admission contract.
///
/// This struct is the return type of [`evaluate_watchlist_signal_admission`].
/// It is **not wired into the live signal path in this patch** (PAPER-HANDOFF-READONLY-01).
/// PAPER-HANDOFF-ENFORCE-01 will promote this into the live admission gate.
#[derive(Debug, Clone, PartialEq)]
pub struct WatchlistSignalAdmission {
    pub allowed: bool,
    pub reason: WatchlistAdmissionReason,
}

/// Pure dry signal-admission contract.
///
/// Evaluates whether a given `(symbol, strategy_id)` pair would be admitted
/// under the current watchlist status.
///
/// # NOT WIRED — this patch only
/// This function is **not called from any live runtime path**.  It is a seam
/// for PAPER-HANDOFF-ENFORCE-01 to wire into the strategy signal route.
///
/// # Returns
/// - `allowed=true, reason=Allowed` — symbol and strategy match the approved watchlist.
/// - `allowed=false, reason=*` — any other case.
pub fn evaluate_watchlist_signal_admission(
    outcome: &WatchlistIntakeOutcome,
    symbol: &str,
    strategy_id: &str,
) -> WatchlistSignalAdmission {
    let deny = |reason: WatchlistAdmissionReason| WatchlistSignalAdmission {
        allowed: false,
        reason,
    };

    let artifact = match outcome {
        WatchlistIntakeOutcome::NotConfigured => {
            return deny(WatchlistAdmissionReason::WatchlistNotConfigured)
        }
        WatchlistIntakeOutcome::Missing { .. } => {
            return deny(WatchlistAdmissionReason::WatchlistMissing)
        }
        WatchlistIntakeOutcome::Invalid { .. } => {
            return deny(WatchlistAdmissionReason::WatchlistInvalid)
        }
        WatchlistIntakeOutcome::LoadedNotApproved { .. } => {
            return deny(WatchlistAdmissionReason::WatchlistNotApproved)
        }
        WatchlistIntakeOutcome::LoadedNotApprovedV3 { .. } => {
            return deny(WatchlistAdmissionReason::WatchlistNotApproved)
        }
        WatchlistIntakeOutcome::LoadedApprovedV3 { artifact } => {
            // v3's strategy_assignments is symbol -> Vec<String> (one or
            // more strategy identities per symbol) — membership check, never
            // v1/v2's single-string equality (frozen contract §2.3: the two
            // shapes are never unified).
            if !artifact.symbols.contains(&symbol.to_string()) {
                return deny(WatchlistAdmissionReason::SymbolNotApproved);
            }
            return match artifact.strategy_assignments.get(symbol) {
                Some(assigned) if assigned.iter().any(|s| s == strategy_id) => {
                    WatchlistSignalAdmission {
                        allowed: true,
                        reason: WatchlistAdmissionReason::Allowed,
                    }
                }
                _ => deny(WatchlistAdmissionReason::StrategyNotAssigned),
            };
        }
        WatchlistIntakeOutcome::LoadedApproved { artifact } => artifact,
    };

    // Symbol must be in the approved symbols list.
    if !artifact.symbols.contains(&symbol.to_string()) {
        return deny(WatchlistAdmissionReason::SymbolNotApproved);
    }

    // Strategy must match the assignment for this symbol.
    match artifact.strategy_assignments.get(symbol) {
        Some(assigned) if assigned == strategy_id => WatchlistSignalAdmission {
            allowed: true,
            reason: WatchlistAdmissionReason::Allowed,
        },
        Some(_) | None => deny(WatchlistAdmissionReason::StrategyNotAssigned),
    }
}
