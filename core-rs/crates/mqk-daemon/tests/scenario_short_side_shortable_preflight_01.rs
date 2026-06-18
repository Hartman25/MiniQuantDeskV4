//! SHORT-SIDE-SHORTABLE-PREFLIGHT-01 — Shortability preflight gate tests.
//!
//! Proves that:
//! - `ShortabilityPreflightResult::Unknown` rejects when `require_shortable_check=true`.
//! - `ShortabilityPreflightResult::Unavailable` rejects when `require_shortable_check=true`.
//! - `ShortabilityPreflightResult::SymbolNotFound` always rejects.
//! - `ShortabilityPreflightResult::NotTradable` always rejects.
//! - `ShortabilityPreflightResult::NotShortable` always rejects.
//! - `ShortabilityPreflightResult::NotEasyToBorrow` always rejects.
//! - Positive paper shortability (`Confirmed { shortable=true }`) with all paper flags
//!   enabled passes the policy and reaches `ShortOpenAllowed`.
//! - Live mode rejects regardless of shortability status.
//! - Unknown shortability with `require_shortable_check=false` passes through to
//!   cap evaluation (allowing `ShortOpenAllowed` when no caps are configured).
//! - Unavailable shortability with `require_shortable_check=false` passes through.
//! - `evaluate_short_entry_policy` (the raw `Option<bool>` path) is unaffected by
//!   this patch — existing G01..G18 tests still pass.
//!
//! # Test inventory
//!
//! | ID   | Scenario                                                                  |
//! |------|---------------------------------------------------------------------------|
//! | P01  | Unknown shortability rejects when require_shortable_check=true            |
//! | P02  | Unavailable preflight rejects when require_shortable_check=true           |
//! | P03  | SymbolNotFound rejects regardless of require_shortable_check              |
//! | P04  | NotTradable rejects                                                       |
//! | P05  | NotShortable rejects                                                      |
//! | P06  | NotEasyToBorrow rejects                                                   |
//! | P07  | Positive paper shortability passes (all paper flags=true, Confirmed)      |
//! | P08  | Live mode rejects with Confirmed shortability (live invariant intact)     |
//! | P09  | Unknown + require_shortable_check=false + both flags=true → allowed       |
//! | P10  | Unavailable + require_shortable_check=false + both flags=true → allowed   |
//! | P11  | Non-ShortOpen intent returns NotShortOpen (gate not applicable)          |
//! | P12  | Reason codes are distinct for each preflight rejection variant            |

use mqk_daemon::capital_policy::{
    classify_short_entry_intent, evaluate_short_entry_policy_with_preflight, ShortEntryConfig,
    ShortEntryIntent, ShortEntryPolicyDecision, ShortabilityPreflightResult, ShortabilitySource,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const PAPER: bool = true;
const LIVE: bool = false;

/// Config with both paper flags enabled, shortable check required, no caps.
fn paper_enabled_with_check() -> ShortEntryConfig {
    ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: false,
        require_shortable_check: true,
        max_short_notional_usd: None,
        max_short_shares: None,
    }
}

/// Config with both paper flags enabled, shortable check disabled, no caps.
fn paper_enabled_no_check() -> ShortEntryConfig {
    ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: false,
        require_shortable_check: false,
        max_short_notional_usd: None,
        max_short_shares: None,
    }
}

fn short_open_intent() -> ShortEntryIntent {
    classify_short_entry_intent(0, -5) // flat → sell = ShortOpen
}

// ---------------------------------------------------------------------------
// P01 — Unknown shortability rejects when require_shortable_check=true
// ---------------------------------------------------------------------------

#[test]
fn p01_unknown_shortability_rejects_when_check_required() {
    let preflight = ShortabilityPreflightResult::Unknown {
        symbol: "AAPL".to_string(),
    };

    let decision = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_with_check(),
        short_open_intent(),
        PAPER,
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "shortable_check_required"
        ),
        "P01: Unknown shortability + require_shortable_check=true must block; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// P02 — Unavailable preflight rejects when require_shortable_check=true
// ---------------------------------------------------------------------------

#[test]
fn p02_unavailable_preflight_rejects_when_check_required() {
    let preflight = ShortabilityPreflightResult::Unavailable {
        symbol: "AAPL".to_string(),
        reason: "broker asset endpoint returned 503".to_string(),
    };

    let decision = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_with_check(),
        short_open_intent(),
        PAPER,
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "shortable_check_unavailable"
        ),
        "P02: Unavailable preflight + require_shortable_check=true must block with \
         shortable_check_unavailable; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// P03 — SymbolNotFound rejects regardless of require_shortable_check
// ---------------------------------------------------------------------------

#[test]
fn p03_symbol_not_found_always_rejects() {
    let preflight = ShortabilityPreflightResult::SymbolNotFound {
        symbol: "FAKE".to_string(),
    };

    // Test with require_shortable_check=true
    let decision_check = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_with_check(),
        short_open_intent(),
        PAPER,
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(
            &decision_check,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "shortable_symbol_not_found"
        ),
        "P03a: SymbolNotFound + require_shortable_check=true must block; got: {:?}",
        decision_check
    );

    // Test with require_shortable_check=false — must still block
    let decision_no_check = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_no_check(),
        short_open_intent(),
        PAPER,
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(
            &decision_no_check,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "shortable_symbol_not_found"
        ),
        "P03b: SymbolNotFound + require_shortable_check=false must still block; got: {:?}",
        decision_no_check
    );
}

// ---------------------------------------------------------------------------
// P04 — NotTradable rejects
// ---------------------------------------------------------------------------

#[test]
fn p04_not_tradable_rejects() {
    let preflight = ShortabilityPreflightResult::NotTradable {
        symbol: "AAPL".to_string(),
    };

    let decision = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_with_check(),
        short_open_intent(),
        PAPER,
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "shortable_not_tradable"
        ),
        "P04: NotTradable must block with shortable_not_tradable; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// P05 — NotShortable rejects
// ---------------------------------------------------------------------------

#[test]
fn p05_not_shortable_rejects() {
    let preflight = ShortabilityPreflightResult::NotShortable {
        symbol: "AAPL".to_string(),
    };

    let decision = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_with_check(),
        short_open_intent(),
        PAPER,
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "shortable_not_shortable"
        ),
        "P05: NotShortable must block with shortable_not_shortable; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// P06 — NotEasyToBorrow rejects
// ---------------------------------------------------------------------------

#[test]
fn p06_not_easy_to_borrow_rejects() {
    let preflight = ShortabilityPreflightResult::NotEasyToBorrow {
        symbol: "MEME".to_string(),
    };

    let decision = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_with_check(),
        short_open_intent(),
        PAPER,
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "shortable_not_easy_to_borrow"
        ),
        "P06: NotEasyToBorrow must block with shortable_not_easy_to_borrow; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// P07 — Positive paper shortability passes policy evaluation
// ---------------------------------------------------------------------------

#[test]
fn p07_positive_paper_shortability_passes_policy() {
    // All paper flags enabled, shortable check required, positive preflight.
    let preflight = ShortabilityPreflightResult::Confirmed {
        symbol: "AAPL".to_string(),
        tradable: true,
        shortable: true,
        easy_to_borrow: Some(true),
        source: ShortabilitySource::Test,
    };

    let decision = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_with_check(),
        short_open_intent(),
        PAPER,
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(&decision, ShortEntryPolicyDecision::ShortOpenAllowed { .. }),
        "P07: Confirmed shortability with all paper flags true must yield ShortOpenAllowed; \
         got: {:?}",
        decision
    );
    assert!(
        decision.is_short_open_allowed(),
        "P07: is_short_open_allowed() must be true"
    );
}

// ---------------------------------------------------------------------------
// P08 — Live mode rejects with Confirmed shortability (invariant intact)
// ---------------------------------------------------------------------------

#[test]
fn p08_live_mode_rejects_even_with_confirmed_shortability() {
    // Even with a fully positive preflight, live mode must always block.
    let preflight = ShortabilityPreflightResult::Confirmed {
        symbol: "AAPL".to_string(),
        tradable: true,
        shortable: true,
        easy_to_borrow: Some(true),
        source: ShortabilitySource::AlpacaAsset,
    };

    let permissive = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: true, // even if true, must still block in live mode
        require_shortable_check: true,
        max_short_notional_usd: None,
        max_short_shares: None,
    };

    let decision = evaluate_short_entry_policy_with_preflight(
        &permissive,
        short_open_intent(),
        LIVE, // live mode — always blocked
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "live_short_entries_blocked"
        ),
        "P08: live mode must always block regardless of shortability; got: {:?}",
        decision
    );
    assert!(
        !decision.is_short_open_allowed(),
        "P08: is_short_open_allowed() must be false in live mode"
    );
}

// ---------------------------------------------------------------------------
// P09 — Unknown + require_shortable_check=false + both flags=true → allowed
// ---------------------------------------------------------------------------

#[test]
fn p09_unknown_shortability_with_check_disabled_passes() {
    // When the operator explicitly disables the shortable check, Unknown status
    // passes through to the cap gates.  With no caps configured, ShortOpenAllowed.
    let preflight = ShortabilityPreflightResult::Unknown {
        symbol: "AAPL".to_string(),
    };

    let decision = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_no_check(), // require_shortable_check=false
        short_open_intent(),
        PAPER,
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(&decision, ShortEntryPolicyDecision::ShortOpenAllowed { .. }),
        "P09: Unknown shortability + require_shortable_check=false must yield ShortOpenAllowed \
         (no caps); got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// P10 — Unavailable + require_shortable_check=false + both flags=true → allowed
// ---------------------------------------------------------------------------

#[test]
fn p10_unavailable_shortability_with_check_disabled_passes() {
    let preflight = ShortabilityPreflightResult::Unavailable {
        symbol: "AAPL".to_string(),
        reason: "network timeout".to_string(),
    };

    let decision = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_no_check(), // require_shortable_check=false
        short_open_intent(),
        PAPER,
        &preflight,
        None,
        None,
    );

    assert!(
        matches!(&decision, ShortEntryPolicyDecision::ShortOpenAllowed { .. }),
        "P10: Unavailable shortability + require_shortable_check=false must yield \
         ShortOpenAllowed (no caps); got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// P11 — Non-ShortOpen intent returns NotShortOpen (gate not applicable)
// ---------------------------------------------------------------------------

#[test]
fn p11_non_short_open_intent_returns_not_short_open() {
    // LongClose intent — gate does not apply.
    let long_close = classify_short_entry_intent(10, -5); // partial reduce
    assert_eq!(long_close, ShortEntryIntent::LongClose, "P11: setup check");

    let preflight = ShortabilityPreflightResult::Unknown {
        symbol: "AAPL".to_string(),
    };

    let decision = evaluate_short_entry_policy_with_preflight(
        &paper_enabled_with_check(),
        long_close,
        PAPER,
        &preflight,
        None,
        None,
    );

    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "P11: LongClose intent → NotShortOpen regardless of preflight"
    );
}

// ---------------------------------------------------------------------------
// P12 — Reason codes are distinct for each preflight rejection variant
// ---------------------------------------------------------------------------

#[test]
fn p12_preflight_rejection_reason_codes_are_distinct() {
    let config = paper_enabled_with_check();
    let intent = short_open_intent();

    let cases: &[(&str, ShortabilityPreflightResult)] = &[
        (
            "shortable_check_required",
            ShortabilityPreflightResult::Unknown {
                symbol: "X".to_string(),
            },
        ),
        (
            "shortable_check_unavailable",
            ShortabilityPreflightResult::Unavailable {
                symbol: "X".to_string(),
                reason: "test".to_string(),
            },
        ),
        (
            "shortable_symbol_not_found",
            ShortabilityPreflightResult::SymbolNotFound {
                symbol: "X".to_string(),
            },
        ),
        (
            "shortable_not_tradable",
            ShortabilityPreflightResult::NotTradable {
                symbol: "X".to_string(),
            },
        ),
        (
            "shortable_not_shortable",
            ShortabilityPreflightResult::NotShortable {
                symbol: "X".to_string(),
            },
        ),
        (
            "shortable_not_easy_to_borrow",
            ShortabilityPreflightResult::NotEasyToBorrow {
                symbol: "X".to_string(),
            },
        ),
    ];

    for (expected_code, preflight) in cases {
        let decision = evaluate_short_entry_policy_with_preflight(
            &config, intent, PAPER, preflight, None, None,
        );
        assert!(
            matches!(
                &decision,
                ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
                if *reason_code == *expected_code
            ),
            "P12: expected reason_code='{}' for {:?}; got: {:?}",
            expected_code,
            preflight,
            decision
        );
    }
}
