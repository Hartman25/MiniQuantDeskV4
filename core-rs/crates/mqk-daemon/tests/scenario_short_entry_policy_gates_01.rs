//! SHORT-SIDE-RISK-GATES-01 — Fail-closed short-entry policy gate tests.
//!
//! Proves that:
//! - `classify_short_entry_intent` correctly identifies short-open vs other intents.
//! - `evaluate_short_entry_policy` rejects short-open by default.
//! - Live short entries are always rejected regardless of config.
//! - Both paper flags must be true before paper short entries are allowed.
//! - Shortable check blocks when status unknown and require_shortable_check=true.
//! - Cap gates block when projected values exceed configured limits.
//! - Long close, long open, and buy-to-cover intents pass through (not short-open).
//! - `parse_short_entry_config` reads JSON correctly with fail-closed defaults.
//!
//! # Test inventory
//!
//! | ID   | Scenario                                                              |
//! |------|-----------------------------------------------------------------------|
//! | G01  | Default policy rejects short-open from flat                           |
//! | G02  | Default policy rejects short-open (sell beyond holdings)              |
//! | G03  | Long sell-to-close is classified as LongClose, not ShortOpen          |
//! | G04  | Buy while short is BuyToCover, not ShortOpen                         |
//! | G05  | Paper: allow_short_entries=false blocks even if paper flag true       |
//! | G06  | Paper: paper_short_entries_enabled=false blocks when master is true   |
//! | G07  | Paper: both flags true → ShortOpenAllowed (no shortable check needed) |
//! | G08  | Live mode blocks short-open even if all flags true                    |
//! | G09  | Unknown shortable status rejects when require_shortable_check=true    |
//! | G10  | Confirmed not-shortable (false) rejects                               |
//! | G11  | max_short_shares exceeded rejects                                     |
//! | G12  | max_short_notional_usd exceeded rejects                               |
//! | G13  | Caps within bounds → ShortOpenAllowed                                |
//! | G14  | LongOpen intent → NotShortOpen (gate not applicable)                 |
//! | G15  | parse_short_entry_config: absent section → fail-closed defaults       |
//! | G16  | parse_short_entry_config: explicit true flags parsed correctly         |
//! | G17  | classify_short_entry_intent: delta=0 → LongOpen (safe pass-through)  |
//! | G18  | Partial long reduce (within holdings) → LongClose, not ShortOpen     |

use mqk_daemon::capital_policy::{
    classify_short_entry_intent, evaluate_short_entry_policy, parse_short_entry_config,
    ShortEntryConfig, ShortEntryIntent, ShortEntryPolicyDecision,
};

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

const PAPER: bool = true;
const LIVE: bool = false;

fn fail_closed() -> ShortEntryConfig {
    ShortEntryConfig::fail_closed()
}

// ---------------------------------------------------------------------------
// G01 — Default policy rejects short-open from flat
// ---------------------------------------------------------------------------

#[test]
fn g01_default_policy_rejects_short_open_from_flat() {
    // current=0 (flat), delta=-10 → ShortOpen
    let intent = classify_short_entry_intent(0, -10);
    assert_eq!(
        intent,
        ShortEntryIntent::ShortOpen,
        "G01: flat + sell → ShortOpen"
    );

    let decision = evaluate_short_entry_policy(
        &fail_closed(),
        intent,
        PAPER,
        None, // shortable unknown — but blocked earlier by allow_short_entries
        Some(10),
        Some(1000.0),
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "short_entries_disabled"
        ),
        "G01: default policy must block with short_entries_disabled; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// G02 — Default policy rejects short-open (sell beyond long holdings)
// ---------------------------------------------------------------------------

#[test]
fn g02_default_policy_rejects_short_open_sell_beyond_holdings() {
    // current=5, delta=-13 → abs(delta)=13 > 5 → ShortOpen
    let intent = classify_short_entry_intent(5, -13);
    assert_eq!(
        intent,
        ShortEntryIntent::ShortOpen,
        "G02: sell > holdings → ShortOpen"
    );

    let decision =
        evaluate_short_entry_policy(&fail_closed(), intent, PAPER, None, Some(8), Some(800.0));

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "short_entries_disabled"
        ),
        "G02: default policy must block sell-beyond-holdings as short_entries_disabled; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// G03 — Long sell-to-close is LongClose, not ShortOpen
// ---------------------------------------------------------------------------

#[test]
fn g03_long_sell_to_close_is_not_short_open() {
    // current=7, target=0 → delta=-7 → qty_to_sell=7 == current(7) → LongClose
    let intent = classify_short_entry_intent(7, -7);
    assert_eq!(
        intent,
        ShortEntryIntent::LongClose,
        "G03: sell exactly to flat → LongClose, not ShortOpen"
    );

    // Policy gate must return NotShortOpen (gate not applicable).
    let decision = evaluate_short_entry_policy(&fail_closed(), intent, PAPER, None, None, None);

    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "G03: LongClose intent must return NotShortOpen — gate does not apply"
    );
}

// ---------------------------------------------------------------------------
// G04 — Buy while short is BuyToCover, not ShortOpen
// ---------------------------------------------------------------------------

#[test]
fn g04_buy_while_short_is_buy_to_cover_not_short_open() {
    // current=-5 (existing short), delta=+5 → covering the short
    let intent = classify_short_entry_intent(-5, 5);
    assert_eq!(
        intent,
        ShortEntryIntent::BuyToCover,
        "G04: buy into short position → BuyToCover"
    );

    let decision = evaluate_short_entry_policy(&fail_closed(), intent, PAPER, None, None, None);

    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "G04: BuyToCover must return NotShortOpen — it is risk-reducing, not a new short entry"
    );
}

// ---------------------------------------------------------------------------
// G05 — Paper: allow_short_entries=false blocks even if paper flag true
// ---------------------------------------------------------------------------

#[test]
fn g05_allow_short_entries_false_blocks_even_if_paper_enabled() {
    let config = ShortEntryConfig {
        allow_short_entries: false,
        paper_short_entries_enabled: true, // paper enabled but master gate is false
        live_short_entries_enabled: false,
        require_shortable_check: false,
        max_short_notional_usd: None,
        max_short_shares: None,
    };

    let intent = classify_short_entry_intent(0, -10);
    assert_eq!(intent, ShortEntryIntent::ShortOpen, "G05: setup check");

    let decision = evaluate_short_entry_policy(&config, intent, PAPER, Some(true), None, None);

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "short_entries_disabled"
        ),
        "G05: master gate (allow_short_entries=false) must block regardless of paper flag; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// G06 — Paper: paper_short_entries_enabled=false blocks when master is true
// ---------------------------------------------------------------------------

#[test]
fn g06_paper_short_entries_disabled_blocks_when_master_true() {
    let config = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: false, // paper gate closed
        live_short_entries_enabled: false,
        require_shortable_check: false,
        max_short_notional_usd: None,
        max_short_shares: None,
    };

    let intent = classify_short_entry_intent(0, -10);
    let decision = evaluate_short_entry_policy(&config, intent, PAPER, Some(true), None, None);

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "paper_short_entries_disabled"
        ),
        "G06: paper gate must block when paper_short_entries_enabled=false; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// G07 — Paper: both flags true, no shortable check → ShortOpenAllowed
// ---------------------------------------------------------------------------

#[test]
fn g07_both_paper_flags_true_no_shortable_check_allows() {
    let config = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: false,
        require_shortable_check: false, // check disabled
        max_short_notional_usd: None,
        max_short_shares: None,
    };

    let intent = classify_short_entry_intent(0, -5);
    assert_eq!(intent, ShortEntryIntent::ShortOpen, "G07: setup check");

    let decision = evaluate_short_entry_policy(
        &config,
        intent,
        PAPER,
        None, // shortable status irrelevant when check disabled
        Some(5),
        Some(500.0),
    );

    assert!(
        matches!(&decision, ShortEntryPolicyDecision::ShortOpenAllowed { .. }),
        "G07: both flags true + check disabled → ShortOpenAllowed; got: {:?}",
        decision
    );
    assert!(
        decision.is_short_open_allowed(),
        "G07: is_short_open_allowed() must be true"
    );
}

// ---------------------------------------------------------------------------
// G08 — Live mode always rejects short-open even if all flags true
// ---------------------------------------------------------------------------

#[test]
fn g08_live_mode_always_rejects_short_open() {
    let config = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: true, // even if true, must still reject
        require_shortable_check: false,
        max_short_notional_usd: None,
        max_short_shares: None,
    };

    let intent = classify_short_entry_intent(0, -10);
    let decision = evaluate_short_entry_policy(
        &config,
        intent,
        LIVE,
        Some(true), // confirmed shortable — but irrelevant in live mode
        Some(10),
        Some(1000.0),
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "live_short_entries_blocked"
        ),
        "G08: live mode must always block with live_short_entries_blocked; got: {:?}",
        decision
    );
    assert!(
        !decision.is_short_open_allowed(),
        "G08: is_short_open_allowed() must be false in live mode"
    );
}

// ---------------------------------------------------------------------------
// G09 — Unknown shortable status rejects when require_shortable_check=true
// ---------------------------------------------------------------------------

#[test]
fn g09_unknown_shortable_status_rejects_when_check_required() {
    let config = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: false,
        require_shortable_check: true,
        max_short_notional_usd: None,
        max_short_shares: None,
    };

    let intent = classify_short_entry_intent(0, -5);
    let decision = evaluate_short_entry_policy(
        &config, intent, PAPER, None, // unknown shortable status
        None, None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "shortable_check_required"
        ),
        "G09: unknown shortable status + require_shortable_check=true must reject; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// G10 — Confirmed not-shortable rejects
// ---------------------------------------------------------------------------

#[test]
fn g10_not_shortable_confirmed_rejects() {
    let config = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: false,
        require_shortable_check: true,
        max_short_notional_usd: None,
        max_short_shares: None,
    };

    let intent = classify_short_entry_intent(0, -5);
    let decision = evaluate_short_entry_policy(
        &config,
        intent,
        PAPER,
        Some(false), // confirmed NOT shortable
        None,
        None,
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "shortable_check_required"
        ),
        "G10: shortable_status=Some(false) must block; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// G11 — max_short_shares exceeded rejects
// ---------------------------------------------------------------------------

#[test]
fn g11_max_short_shares_exceeded_rejects() {
    let config = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: false,
        require_shortable_check: false,
        max_short_notional_usd: None,
        max_short_shares: Some(100), // cap at 100 shares
    };

    let intent = classify_short_entry_intent(0, -5);
    let decision = evaluate_short_entry_policy(
        &config,
        intent,
        PAPER,
        None,
        Some(150), // projected 150 > cap 100
        Some(15_000.0),
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "max_short_shares_exceeded"
        ),
        "G11: projected short shares (150) > max_short_shares (100) must block; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// G12 — max_short_notional_usd exceeded rejects
// ---------------------------------------------------------------------------

#[test]
fn g12_max_short_notional_exceeded_rejects() {
    let config = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: false,
        require_shortable_check: false,
        max_short_notional_usd: Some(5_000.0), // cap at $5,000
        max_short_shares: None,
    };

    let intent = classify_short_entry_intent(0, -5);
    let decision = evaluate_short_entry_policy(
        &config,
        intent,
        PAPER,
        None,
        None,
        Some(7_500.0), // projected $7,500 > cap $5,000
    );

    assert!(
        matches!(
            &decision,
            ShortEntryPolicyDecision::ShortOpenBlocked { reason_code, .. }
            if *reason_code == "max_short_notional_exceeded"
        ),
        "G12: projected notional ($7,500) > max_short_notional_usd ($5,000) must block; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// G13 — Caps within bounds → ShortOpenAllowed
// ---------------------------------------------------------------------------

#[test]
fn g13_caps_within_bounds_allows() {
    let config = ShortEntryConfig {
        allow_short_entries: true,
        paper_short_entries_enabled: true,
        live_short_entries_enabled: false,
        require_shortable_check: true,
        max_short_notional_usd: Some(10_000.0),
        max_short_shares: Some(200),
    };

    let intent = classify_short_entry_intent(0, -5);
    let decision = evaluate_short_entry_policy(
        &config,
        intent,
        PAPER,
        Some(true),    // confirmed shortable
        Some(50),      // 50 < 200 cap
        Some(5_000.0), // $5,000 < $10,000 cap
    );

    assert!(
        matches!(&decision, ShortEntryPolicyDecision::ShortOpenAllowed { .. }),
        "G13: all gates pass, caps within bounds → ShortOpenAllowed; got: {:?}",
        decision
    );
}

// ---------------------------------------------------------------------------
// G14 — LongOpen intent → NotShortOpen (gate not applicable)
// ---------------------------------------------------------------------------

#[test]
fn g14_long_open_returns_not_short_open() {
    // current=0, delta=+10 → LongOpen
    let intent = classify_short_entry_intent(0, 10);
    assert_eq!(
        intent,
        ShortEntryIntent::LongOpen,
        "G14: buy from flat → LongOpen"
    );

    let decision = evaluate_short_entry_policy(&fail_closed(), intent, PAPER, None, None, None);

    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "G14: LongOpen intent → NotShortOpen; gate does not apply"
    );
    assert!(
        !decision.is_short_open_allowed(),
        "G14: is_short_open_allowed() must be false for NotShortOpen"
    );
}

// ---------------------------------------------------------------------------
// G15 — parse_short_entry_config: absent section → fail-closed defaults
// ---------------------------------------------------------------------------

#[test]
fn g15_parse_absent_section_returns_fail_closed() {
    let j = serde_json::json!({
        "schema_version": "policy-v1",
        "policy_id": "paper-test",
        "enabled": true
        // no short_entry_policy section
    });

    let config = parse_short_entry_config(&j);
    let expected = ShortEntryConfig::fail_closed();

    assert_eq!(
        config, expected,
        "G15: absent short_entry_policy section must yield fail-closed defaults"
    );
    assert!(
        !config.allow_short_entries,
        "G15: allow_short_entries must default false"
    );
    assert!(
        !config.paper_short_entries_enabled,
        "G15: paper flag must default false"
    );
    assert!(
        !config.live_short_entries_enabled,
        "G15: live flag must default false"
    );
    assert!(
        config.require_shortable_check,
        "G15: require_shortable_check must default true"
    );
    assert!(
        config.max_short_notional_usd.is_none(),
        "G15: notional cap must default None"
    );
    assert!(
        config.max_short_shares.is_none(),
        "G15: shares cap must default None"
    );
}

// ---------------------------------------------------------------------------
// G16 — parse_short_entry_config: explicit flags parsed correctly
// ---------------------------------------------------------------------------

#[test]
fn g16_parse_explicit_flags_parsed_correctly() {
    let j = serde_json::json!({
        "schema_version": "policy-v1",
        "short_entry_policy": {
            "allow_short_entries": true,
            "paper_short_entries_enabled": true,
            "live_short_entries_enabled": false,
            "require_shortable_check": false,
            "max_short_notional_usd": 8000.0,
            "max_short_shares": 150
        }
    });

    let config = parse_short_entry_config(&j);

    assert!(config.allow_short_entries, "G16: allow_short_entries");
    assert!(
        config.paper_short_entries_enabled,
        "G16: paper_short_entries_enabled"
    );
    assert!(
        !config.live_short_entries_enabled,
        "G16: live_short_entries_enabled"
    );
    assert!(
        !config.require_shortable_check,
        "G16: require_shortable_check"
    );
    assert_eq!(
        config.max_short_notional_usd,
        Some(8_000.0),
        "G16: max_short_notional_usd"
    );
    assert_eq!(config.max_short_shares, Some(150), "G16: max_short_shares");
}

// ---------------------------------------------------------------------------
// G17 — classify_short_entry_intent: delta=0 → LongOpen (safe pass-through)
// ---------------------------------------------------------------------------

#[test]
fn g17_zero_delta_returns_long_open_safe_passthrough() {
    let intent = classify_short_entry_intent(10, 0);
    assert_eq!(
        intent,
        ShortEntryIntent::LongOpen,
        "G17: delta=0 → LongOpen safe pass-through (already at target)"
    );
}

// ---------------------------------------------------------------------------
// G18 — Partial long reduce (within holdings) → LongClose, not ShortOpen
// ---------------------------------------------------------------------------

#[test]
fn g18_partial_long_reduce_is_long_close_not_short_open() {
    // current=10, delta=-4 → sell 4, leaves 6 long → LongClose
    let intent = classify_short_entry_intent(10, -4);
    assert_eq!(
        intent,
        ShortEntryIntent::LongClose,
        "G18: partial reduce (current=10, delta=-4) → LongClose"
    );

    // Confirm policy gate does not apply.
    let decision = evaluate_short_entry_policy(&fail_closed(), intent, PAPER, None, None, None);
    assert_eq!(
        decision,
        ShortEntryPolicyDecision::NotShortOpen,
        "G18: LongClose → NotShortOpen; gate does not apply"
    );
}

// ---------------------------------------------------------------------------
// Verify truth_state labels
// ---------------------------------------------------------------------------

#[test]
fn truth_state_labels_are_correct() {
    let not_short_open = ShortEntryPolicyDecision::NotShortOpen;
    assert_eq!(not_short_open.truth_state(), "not_short_open");

    let allowed = ShortEntryPolicyDecision::ShortOpenAllowed {
        detail: "test".to_string(),
    };
    assert_eq!(allowed.truth_state(), "short_open_allowed");

    let blocked = ShortEntryPolicyDecision::ShortOpenBlocked {
        reason_code: "short_entries_disabled",
        detail: "test".to_string(),
    };
    assert_eq!(blocked.truth_state(), "short_open_blocked");
}
