//! SHORT-SIDE-RISK-GATES-01 — Fail-closed short-entry policy gate.
//!
//! Establishes the minimum short-entry control infrastructure so the repo is
//! explicitly aware of short-entry policy and can prove that short entries
//! remain blocked by default with clear diagnostics.
//!
//! This patch does NOT enable short trading.  It does NOT remove the B5 guard
//! in `decision.rs`.  It does NOT make strategy layers emit negative signals.
//! B5 remains the final backstop at the `bar_result_to_decisions` boundary;
//! this gate is the upstream explicit policy classifier that future patches
//! (`SHORT-SIDE-INTENT-MODEL-01`) can call before B5 acts.
//!
//! # Policy configuration
//!
//! Short-entry policy fields live in the `short_entry_policy` section of the
//! `capital_allocation_policy.json` file (schema `policy-v1`).  If the section
//! is absent the gate behaves as if all flags are at their most restrictive
//! defaults — absence is not authorization.
//!
//! ```json
//! {
//!   "schema_version": "policy-v1",
//!   "short_entry_policy": {
//!     "allow_short_entries": false,
//!     "paper_short_entries_enabled": false,
//!     "live_short_entries_enabled": false,
//!     "require_shortable_check": true,
//!     "max_short_notional_usd": null,
//!     "max_short_shares": null
//!   }
//! }
//! ```
//!
//! # Default is fail-closed
//!
//! Unlike the capital allocation gate where `NotConfigured` means "no
//! enforcement", the short-entry gate is **always active** — the absence of
//! a policy section is equivalent to all gates being closed.  Short trading
//! requires explicit opt-in at every level.
//!
//! # Live shorting invariant
//!
//! `live_short_entries_enabled` is parsed and stored, but
//! [`evaluate_short_entry_policy`] unconditionally returns
//! [`ShortEntryPolicyDecision::ShortOpenBlocked`] with reason code
//! `"live_short_entries_blocked"` when `is_paper_mode = false`.  The config
//! flag value is never consulted for live mode in this patch.

// ---------------------------------------------------------------------------
// Intent classification
// ---------------------------------------------------------------------------

/// Classification of a buy/sell intent from the perspective of short-entry
/// policy.
///
/// Used to determine whether the short-entry policy gate is applicable for a
/// given `(current_qty, delta)` pair.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShortEntryIntent {
    /// Buying shares into a flat or existing long position.
    ///
    /// `delta > 0` and `current_qty >= 0`.  Not a short concern.
    LongOpen,

    /// Selling shares to reduce or close an existing long position without
    /// going net-short.
    ///
    /// `delta < 0` and `current_qty > 0` and `abs(delta) <= current_qty`.
    /// Not a short concern.
    LongClose,

    /// Buying shares to reduce or close an existing short position.
    ///
    /// `delta > 0` and `current_qty < 0`.  Risk-reducing cover — not a new
    /// short entry.  Not a short concern for this policy gate.
    BuyToCover,

    /// Sell intent that would open or extend a short position.
    ///
    /// Occurs when:
    /// - `delta < 0` and `current_qty <= 0` (flat or already short; any sell
    ///   opens/deepens short), or
    /// - `delta < 0` and `abs(delta) > current_qty` (sell exceeds long
    ///   holdings; excess drives position net-short).
    ///
    /// This is the intent that requires explicit short-entry policy
    /// authorization.  B5 in `decision.rs` drops these silently; this
    /// classifier surfaces a diagnostic reason before B5 acts.
    ShortOpen,
}

/// Classify the short-entry intent for a `(current_qty, delta)` pair.
///
/// Pure: no IO, no side effects.
///
/// # Parameters
///
/// - `current_qty` — current signed portfolio quantity for the symbol
///   (negative = existing short; 0 = flat; positive = long).
/// - `delta` — intended change in quantity (positive = buy, negative = sell).
///   A delta of zero has no meaningful short-entry intent; returns `LongOpen`
///   as a safe pass-through (callers should skip zero-delta before this).
pub fn classify_short_entry_intent(current_qty: i64, delta: i64) -> ShortEntryIntent {
    if delta > 0 {
        if current_qty < 0 {
            ShortEntryIntent::BuyToCover
        } else {
            ShortEntryIntent::LongOpen
        }
    } else if delta < 0 {
        let qty_to_sell = -delta; // positive by construction (delta < 0)
        if current_qty <= 0 || qty_to_sell > current_qty {
            ShortEntryIntent::ShortOpen
        } else {
            ShortEntryIntent::LongClose
        }
    } else {
        // delta == 0: no intent; safe pass-through.
        ShortEntryIntent::LongOpen
    }
}

// ---------------------------------------------------------------------------
// Policy config
// ---------------------------------------------------------------------------

/// Parsed short-entry policy config.
///
/// Constructed from the `short_entry_policy` section of
/// `capital_allocation_policy.json` via [`parse_short_entry_config`], or from
/// the fail-closed default via [`ShortEntryConfig::fail_closed`].
///
/// All fields default to the most restrictive fail-closed values when absent
/// from the JSON section.
#[derive(Debug, Clone, PartialEq)]
pub struct ShortEntryConfig {
    /// Master gate: short entries are enabled at all.
    ///
    /// Must be `true` for any short-open to be permitted.  Default: `false`.
    pub allow_short_entries: bool,

    /// Paper-mode gate: short entries permitted in paper trading.
    ///
    /// Must be `true` AND `allow_short_entries` must be `true` before a
    /// paper short-open can be allowed.  Default: `false`.
    pub paper_short_entries_enabled: bool,

    /// Live-mode gate field (parsed and stored; not consulted by the evaluator
    /// in this patch).
    ///
    /// [`evaluate_short_entry_policy`] always rejects live short entries
    /// regardless of this flag — it is a hard invariant in this patch.
    /// Default: `false`.
    pub live_short_entries_enabled: bool,

    /// Require a broker shortable/easy-to-borrow check before a short-open.
    ///
    /// When `true` and `shortable_status` passed to the evaluator is `None`
    /// (unknown), the gate rejects fail-closed.  Default: `true`.
    pub require_shortable_check: bool,

    /// Maximum projected short notional in USD.
    ///
    /// `None` means the cap is not configured.  Default: `None`.
    pub max_short_notional_usd: Option<f64>,

    /// Maximum projected short share count.
    ///
    /// `None` means the cap is not configured.  Default: `None`.
    pub max_short_shares: Option<i64>,
}

impl ShortEntryConfig {
    /// Return the fail-closed default: all gates closed, shortable check
    /// required, no caps defined.
    ///
    /// Equivalent to the `short_entry_policy` section being absent from the
    /// policy file.
    pub fn fail_closed() -> Self {
        Self {
            allow_short_entries: false,
            paper_short_entries_enabled: false,
            live_short_entries_enabled: false,
            require_shortable_check: true,
            max_short_notional_usd: None,
            max_short_shares: None,
        }
    }
}

/// Parse the `short_entry_policy` section from a `serde_json::Value`
/// representing the full policy file.
///
/// Returns [`ShortEntryConfig::fail_closed`] when the `short_entry_policy`
/// key is absent — absence is not authorization; it is the most restrictive
/// state.
///
/// Per-field parsing: unknown or malformed field values fall back to the
/// fail-closed default for that field (never to an optimistic value).
pub fn parse_short_entry_config(j: &serde_json::Value) -> ShortEntryConfig {
    let section = match j.get("short_entry_policy") {
        Some(v) => v,
        None => return ShortEntryConfig::fail_closed(),
    };

    ShortEntryConfig {
        allow_short_entries: section
            .get("allow_short_entries")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        paper_short_entries_enabled: section
            .get("paper_short_entries_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        live_short_entries_enabled: section
            .get("live_short_entries_enabled")
            .and_then(|v| v.as_bool())
            .unwrap_or(false),
        require_shortable_check: section
            .get("require_shortable_check")
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        max_short_notional_usd: section
            .get("max_short_notional_usd")
            .and_then(|v| v.as_f64())
            .filter(|&n| n > 0.0),
        max_short_shares: section
            .get("max_short_shares")
            .and_then(|v| v.as_i64())
            .filter(|&n| n > 0),
    }
}

// ---------------------------------------------------------------------------
// Policy decision
// ---------------------------------------------------------------------------

/// Result of evaluating the short-entry policy gate.
///
/// Non-`ShortOpen` intents return [`ShortEntryPolicyDecision::NotShortOpen`]
/// immediately — the gate does not apply to `LongOpen`, `LongClose`, or
/// `BuyToCover` intents.
#[derive(Debug, Clone, PartialEq)]
pub enum ShortEntryPolicyDecision {
    /// The intent is not a short-open (`LongOpen`, `LongClose`, or
    /// `BuyToCover`).  The gate does not apply; callers pass through.
    NotShortOpen,

    /// The intent is a short-open and all policy gates passed.
    ///
    /// In this patch, `ShortOpenAllowed` is never returned in live mode.
    /// In paper mode it requires both `allow_short_entries` and
    /// `paper_short_entries_enabled` to be `true`, a passing shortable check
    /// (or check disabled), and all caps to be within bounds.
    ShortOpenAllowed {
        /// Human-readable explanation of which gates permitted this.
        detail: String,
    },

    /// The intent is a short-open and the policy blocked it.
    ///
    /// B5 in `decision.rs` is still the final backstop; this decision surfaces
    /// a diagnostic `reason_code` and `detail` before B5 silently drops the
    /// order.
    ShortOpenBlocked {
        /// Machine-readable reason code.  One of:
        /// `"live_short_entries_blocked"`, `"short_entries_disabled"`,
        /// `"paper_short_entries_disabled"`, `"shortable_check_required"`,
        /// `"max_short_shares_exceeded"`, `"max_short_notional_exceeded"`.
        reason_code: &'static str,
        /// Human-readable explanation for operator diagnostics.
        detail: String,
    },
}

impl ShortEntryPolicyDecision {
    /// Machine-readable truth-state label.
    pub fn truth_state(&self) -> &'static str {
        match self {
            Self::NotShortOpen => "not_short_open",
            Self::ShortOpenAllowed { .. } => "short_open_allowed",
            Self::ShortOpenBlocked { .. } => "short_open_blocked",
        }
    }

    /// Whether the policy allows the short-open to proceed.
    ///
    /// `true` only for `ShortOpenAllowed`.  Both `NotShortOpen` and
    /// `ShortOpenBlocked` return `false` — a non-short-open intent does not
    /// grant permission for a short; the gate simply does not apply.
    pub fn is_short_open_allowed(&self) -> bool {
        matches!(self, Self::ShortOpenAllowed { .. })
    }
}

// ---------------------------------------------------------------------------
// Pure evaluator
// ---------------------------------------------------------------------------

/// Evaluate the short-entry policy for a classified intent.
///
/// Pure: no IO, no env reads, no state mutation.  All context is passed
/// explicitly.
///
/// # Parameters
///
/// - `config` — parsed short-entry policy config; use
///   [`ShortEntryConfig::fail_closed`] when no policy file is configured.
/// - `intent` — the classified intent; use [`classify_short_entry_intent`].
/// - `is_paper_mode` — `true` = paper trading; `false` = live (live always
///   rejects, regardless of config).
/// - `shortable_status` — broker shortable/ETB result: `Some(true)` =
///   confirmed shortable; `Some(false)` = confirmed NOT shortable; `None` =
///   unknown.  When `require_shortable_check = true`, `None` rejects
///   fail-closed.
/// - `projected_short_shares` — absolute share count of the intended short
///   position after the trade.  Used for `max_short_shares` cap evaluation.
///   `None` = not computed.
/// - `projected_short_notional_usd` — projected notional in USD of the short
///   position after the trade.  Used for `max_short_notional_usd` cap.
///   `None` = not computed (e.g., market order with no price reference).
///
/// # Gate sequence (fail-fast)
///
/// 1. Non-`ShortOpen` intent → `NotShortOpen` (gate not applicable).
/// 2. Live mode (`!is_paper_mode`) → `ShortOpenBlocked { "live_short_entries_blocked" }`.
/// 3. `allow_short_entries = false` → `ShortOpenBlocked { "short_entries_disabled" }`.
/// 4. `paper_short_entries_enabled = false` → `ShortOpenBlocked { "paper_short_entries_disabled" }`.
/// 5. `require_shortable_check = true` and `shortable_status = None` →
///    `ShortOpenBlocked { "shortable_check_required" }`.
/// 6. `shortable_status = Some(false)` → `ShortOpenBlocked { "shortable_check_required" }`.
/// 7. `max_short_shares` exceeded → `ShortOpenBlocked { "max_short_shares_exceeded" }`.
/// 8. `max_short_notional_usd` exceeded → `ShortOpenBlocked { "max_short_notional_exceeded" }`.
/// 9. All gates pass → `ShortOpenAllowed`.
pub fn evaluate_short_entry_policy(
    config: &ShortEntryConfig,
    intent: ShortEntryIntent,
    is_paper_mode: bool,
    shortable_status: Option<bool>,
    projected_short_shares: Option<i64>,
    projected_short_notional_usd: Option<f64>,
) -> ShortEntryPolicyDecision {
    // Gate 1: only ShortOpen intents require evaluation.
    if intent != ShortEntryIntent::ShortOpen {
        return ShortEntryPolicyDecision::NotShortOpen;
    }

    // Gate 2: live mode — always blocked in this patch (hard invariant).
    if !is_paper_mode {
        return ShortEntryPolicyDecision::ShortOpenBlocked {
            reason_code: "live_short_entries_blocked",
            detail: "short-open rejected: live short entries are not supported in this version; \
                     the live_short_entries_blocked invariant is enforced regardless of \
                     live_short_entries_enabled in the policy file"
                .to_string(),
        };
    }

    // Gate 3: master short-entry gate.
    if !config.allow_short_entries {
        return ShortEntryPolicyDecision::ShortOpenBlocked {
            reason_code: "short_entries_disabled",
            detail: "short-open rejected: allow_short_entries=false in short_entry_policy \
                     (fail-closed default); set allow_short_entries=true to enable short trading"
                .to_string(),
        };
    }

    // Gate 4: paper-mode gate (requires both flags true).
    if !config.paper_short_entries_enabled {
        return ShortEntryPolicyDecision::ShortOpenBlocked {
            reason_code: "paper_short_entries_disabled",
            detail:
                "short-open rejected: paper_short_entries_enabled=false in short_entry_policy; \
                     allow_short_entries=true but paper_short_entries_enabled must also be true \
                     for paper short entries to be permitted"
                    .to_string(),
        };
    }

    // Gates 5+6: shortable/ETB check.
    if config.require_shortable_check {
        match shortable_status {
            None => {
                return ShortEntryPolicyDecision::ShortOpenBlocked {
                    reason_code: "shortable_check_required",
                    detail: "short-open rejected: require_shortable_check=true and broker \
                             shortable status is unknown (None); the easy-to-borrow check must \
                             be performed and confirmed before a short-open is allowed"
                        .to_string(),
                };
            }
            Some(false) => {
                return ShortEntryPolicyDecision::ShortOpenBlocked {
                    reason_code: "shortable_check_required",
                    detail: "short-open rejected: broker shortable check returned false; \
                             this security is not available for short selling (not easy-to-borrow)"
                        .to_string(),
                };
            }
            Some(true) => {
                // Confirmed shortable — continue.
            }
        }
    }

    // Gate 7: max_short_shares cap.
    if let (Some(cap), Some(projected)) = (config.max_short_shares, projected_short_shares) {
        if projected > cap {
            return ShortEntryPolicyDecision::ShortOpenBlocked {
                reason_code: "max_short_shares_exceeded",
                detail: format!(
                    "short-open rejected: projected short shares {} exceeds \
                     max_short_shares={} in short_entry_policy",
                    projected, cap
                ),
            };
        }
    }

    // Gate 8: max_short_notional_usd cap.
    if let (Some(cap), Some(projected)) =
        (config.max_short_notional_usd, projected_short_notional_usd)
    {
        if projected > cap {
            return ShortEntryPolicyDecision::ShortOpenBlocked {
                reason_code: "max_short_notional_exceeded",
                detail: format!(
                    "short-open rejected: projected short notional ${:.2} exceeds \
                     max_short_notional_usd=${:.2} in short_entry_policy",
                    projected, cap
                ),
            };
        }
    }

    // All gates passed.
    ShortEntryPolicyDecision::ShortOpenAllowed {
        detail: "short-open allowed: allow_short_entries=true, \
                 paper_short_entries_enabled=true, shortable check satisfied or not required, \
                 all caps within bounds"
            .to_string(),
    }
}
