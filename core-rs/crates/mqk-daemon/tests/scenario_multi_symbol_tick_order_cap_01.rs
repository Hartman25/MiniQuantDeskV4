//! MULTI-SYMBOL-TICK-ORDER-CAP-01: Proof tests for cap #6
//! (`max_new_orders_per_tick`, design doc §6, "Patch 7").
//!
//! Cap #6 is optional (`Option<u32>`-typed, default `None`/disabled) and
//! follows the same lightweight env-var pattern as caps #2/#3/#4/#5
//! (`MULTI-SYMBOL-CAPITAL-CAPS-01` / `MULTI-SYMBOL-DAY-ORDER-CAP-01`) rather
//! than the `MultiSymbolRiskCaps` JSON schema sketched in design doc §4.7.
//!
//! Unlike caps #2-#5, cap #6 is a cross-symbol *per-tick sequencing* concern
//! (design doc §5 Q4): once `new_orders_this_tick >= cap`, the remaining
//! symbols in this tick's iteration order are skipped entirely with
//! `no_order_reason = "max_new_orders_per_tick_reached"` — re-evaluated fresh
//! next tick from then-current bar/position state (no queuing mechanism is
//! needed).
//!
//! `AppState::max_new_orders_per_tick_reason(new_orders_this_tick, cap)` is
//! the pure predicate the B1C dispatch loop (`loop_runner.rs`) calls at the
//! top of each per-symbol iteration; `new_orders_this_tick` is incremented
//! once per accepted decision.
//!
//! # Proof matrix
//!
//! ## `max_new_orders_per_tick_reason` (pure)
//!
//! | Test  | Proves                                                                    |
//! |-------|----------------------------------------------------------------------------|
//! | C6-01 | `cap = None` → always `None`, regardless of counter (unbounded default)|
//! | C6-02 | `cap = Some(n)`, counter `< n` → `None` (not yet reached)               |
//! | C6-03 | `cap = Some(n)`, counter `== n` → `Some("max_new_orders_per_tick_reached")` (boundary, `>=`) |
//! | C6-04 | `cap = Some(n)`, counter `> n` → `Some("max_new_orders_per_tick_reached")` (already exceeded) |
//! | C6-05 | `cap = Some(0)` → counter `0` already returns `Some("max_new_orders_per_tick_reached")` (no new orders this tick) |
//!
//! ## Sequential per-tick simulation (design doc §6 test)
//!
//! | Test  | Proves                                                                    |
//! |-------|----------------------------------------------------------------------------|
//! | C6-06 | 3 symbols, all produce accepted decisions, `cap = Some(1)` → symbol[0] proceeds (counter→1); symbols[1] and [2] return `"max_new_orders_per_tick_reached"` |
//!
//! ## `AppState` config + test seam
//!
//! | Test  | Proves                                                                    |
//! |-------|----------------------------------------------------------------------------|
//! | C6-07 | Default `max_new_orders_per_tick()` is `None` (cap #6 disabled) when `MQK_MAX_NEW_ORDERS_PER_TICK` is unset — the default per design doc §6 |
//! | C6-08 | `set_max_new_orders_per_tick_for_test` overrides the configured cap, round trip `Some` → `None` |
//! | C6-09 | `MQK_MAX_NEW_ORDERS_PER_TICK` read once at `AppState` construction: `"3"` → `Some(3)`, `"0"` → `Some(0)` (not filtered, unlike cap #2's `per_symbol_max_position_qty`), invalid (`"abc"`) / unset → `None` |

use std::sync::{Arc, OnceLock};

use mqk_daemon::state;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// Env-var serialisation — protects MQK_MAX_NEW_ORDERS_PER_TICK mutations.
// C6-07 (default-is-None) and C6-09 (env-var-read-at-construction) both touch
// this var; both acquire this lock so they cannot observe each other's state.
// ---------------------------------------------------------------------------

static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn env_lock() -> &'static Mutex<()> {
    ENV_LOCK.get_or_init(|| Mutex::new(()))
}

/// RAII guard: sets an env var on construction, restores the previous value
/// (or removes it) on drop.
struct EnvVarGuard {
    key: &'static str,
    prev: Option<String>,
}

impl EnvVarGuard {
    fn set(key: &'static str, value: &str) -> Self {
        let prev = std::env::var(key).ok();
        std::env::set_var(key, value);
        Self { key, prev }
    }
}

impl Drop for EnvVarGuard {
    fn drop(&mut self) {
        match &self.prev {
            Some(v) => std::env::set_var(self.key, v),
            None => std::env::remove_var(self.key),
        }
    }
}

// ===========================================================================
// max_new_orders_per_tick_reason (pure)
// ===========================================================================

/// C6-01: `cap = None` (the default, unset `MQK_MAX_NEW_ORDERS_PER_TICK`) is
/// unbounded — always `None`, regardless of how many decisions have already
/// been accepted this tick.
#[test]
fn c6_01_unbounded_when_cap_is_none() {
    for new_orders_this_tick in [0u32, 1, 5, 1000] {
        assert_eq!(
            state::AppState::max_new_orders_per_tick_reason(new_orders_this_tick, None),
            None,
            "cap=None must always be unbounded; counter={new_orders_this_tick}"
        );
    }
}

/// C6-02: `cap = Some(n)` with `new_orders_this_tick < n` → `None` — the cap
/// has not yet been reached, symbol proceeds.
#[test]
fn c6_02_not_yet_reached_below_cap() {
    let cap = Some(3u32);
    for new_orders_this_tick in [0u32, 1, 2] {
        assert_eq!(
            state::AppState::max_new_orders_per_tick_reason(new_orders_this_tick, cap),
            None,
            "counter={new_orders_this_tick} < cap=3 must not be capped"
        );
    }
}

/// C6-03: `cap = Some(n)` with `new_orders_this_tick == n` → reached at the
/// boundary (`>=`, not strict `>`).
#[test]
fn c6_03_reached_at_exact_boundary() {
    let cap = Some(3u32);
    assert_eq!(
        state::AppState::max_new_orders_per_tick_reason(3, cap),
        Some("max_new_orders_per_tick_reached"),
        "counter==cap must be capped (boundary is >=, not strict >)"
    );
}

/// C6-04: `cap = Some(n)` with `new_orders_this_tick > n` → still reached
/// (already exceeded).
#[test]
fn c6_04_reached_when_already_exceeded() {
    let cap = Some(3u32);
    assert_eq!(
        state::AppState::max_new_orders_per_tick_reason(4, cap),
        Some("max_new_orders_per_tick_reached")
    );
}

/// C6-05: `cap = Some(0)` — no new orders are accepted at all this tick; even
/// the first symbol (counter=0) is immediately capped.
#[test]
fn c6_05_cap_zero_blocks_first_symbol() {
    assert_eq!(
        state::AppState::max_new_orders_per_tick_reason(0, Some(0)),
        Some("max_new_orders_per_tick_reached"),
        "cap=Some(0) means no new orders this tick, including the first symbol"
    );
}

// ===========================================================================
// Sequential per-tick simulation (design doc §6 test)
// ===========================================================================

/// C6-06: simulates the B1C dispatch loop's per-symbol algorithm
/// (`loop_runner.rs`) for 3 symbols that each produce an accepted decision,
/// with `cap = Some(1)`. At the top of each iteration,
/// `max_new_orders_per_tick_reason` is checked against the running counter;
/// if `None`, the symbol proceeds and (since its decision is accepted) the
/// counter is incremented.
///
/// Design doc §6 expected result: `symbols[0]` is accepted; `symbols[1:]`
/// show `no_order_reason = "max_new_orders_per_tick_reached"`.
#[test]
fn c6_06_three_symbols_cap_one_first_proceeds_rest_capped() {
    let cap = Some(1u32);
    let mut new_orders_this_tick: u32 = 0;
    let symbols = ["AAPL", "MSFT", "TSLA"];
    let mut results: Vec<(&str, Option<&'static str>)> = Vec::new();

    for symbol in symbols {
        match state::AppState::max_new_orders_per_tick_reason(new_orders_this_tick, cap) {
            Some(reason) => results.push((symbol, Some(reason))),
            None => {
                // Symbol proceeds. Per this fixture, every symbol's decision
                // is accepted, so the per-tick counter advances.
                new_orders_this_tick += 1;
                results.push((symbol, None));
            }
        }
    }

    assert_eq!(
        results,
        vec![
            ("AAPL", None),
            ("MSFT", Some("max_new_orders_per_tick_reached")),
            ("TSLA", Some("max_new_orders_per_tick_reached")),
        ],
        "with cap=1, only the first symbol proceeds; remaining symbols are \
         capped with max_new_orders_per_tick_reached (design doc §6 test)"
    );
    assert_eq!(new_orders_this_tick, 1);
}

// ===========================================================================
// AppState config + test seam
// ===========================================================================

/// C6-07: `max_new_orders_per_tick()` defaults to `None` (cap #6 disabled)
/// when `MQK_MAX_NEW_ORDERS_PER_TICK` is unset — the default per design doc
/// §6 (unbounded; every configured symbol is dispatched every tick, today's
/// implicit behavior).
#[tokio::test]
async fn c6_07_default_max_new_orders_per_tick_is_none() {
    let _lock = env_lock().lock().await;
    std::env::remove_var("MQK_MAX_NEW_ORDERS_PER_TICK");

    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));
    assert_eq!(
        st.max_new_orders_per_tick().await,
        None,
        "default per-tick new-order cap must be None (cap #6 disabled)"
    );
}

/// C6-08: `set_max_new_orders_per_tick_for_test` overrides the configured cap
/// independent of env state, in both directions (`Some` → `None`).
#[tokio::test]
async fn c6_08_test_seam_overrides_cap() {
    let st = Arc::new(state::AppState::new_with_operator_auth(
        state::OperatorAuthMode::ExplicitDevNoToken,
    ));

    st.set_max_new_orders_per_tick_for_test(Some(2));
    assert_eq!(st.max_new_orders_per_tick().await, Some(2));

    st.set_max_new_orders_per_tick_for_test(None);
    assert_eq!(st.max_new_orders_per_tick().await, None);
}

/// C6-09: `MQK_MAX_NEW_ORDERS_PER_TICK` is read once at `AppState`
/// construction (`max_new_orders_per_tick_from_env`):
/// - `"3"` → `Some(3)`
/// - `"0"` → `Some(0)` — cap #6 deliberately does NOT filter `0` (unlike
///   cap #2's `per_symbol_max_position_qty`, which filters `n > 0`), since
///   `Some(0)` is a meaningful "no new orders this tick" configuration
///   (proven structurally by C6-05).
/// - invalid (`"abc"`) → `None` (unparsable, falls back to disabled)
/// - unset → `None` (the default)
#[tokio::test]
async fn c6_09_env_var_read_at_construction() {
    let _lock = env_lock().lock().await;

    {
        let _env = EnvVarGuard::set("MQK_MAX_NEW_ORDERS_PER_TICK", "3");
        let st = Arc::new(state::AppState::new_with_operator_auth(
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        assert_eq!(st.max_new_orders_per_tick().await, Some(3));
    }

    {
        let _env = EnvVarGuard::set("MQK_MAX_NEW_ORDERS_PER_TICK", "0");
        let st = Arc::new(state::AppState::new_with_operator_auth(
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        assert_eq!(
            st.max_new_orders_per_tick().await,
            Some(0),
            "\"0\" must parse to Some(0), not be filtered out"
        );
    }

    {
        let _env = EnvVarGuard::set("MQK_MAX_NEW_ORDERS_PER_TICK", "abc");
        let st = Arc::new(state::AppState::new_with_operator_auth(
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        assert_eq!(
            st.max_new_orders_per_tick().await,
            None,
            "unparsable value must fall back to disabled (None)"
        );
    }

    {
        std::env::remove_var("MQK_MAX_NEW_ORDERS_PER_TICK");
        let st = Arc::new(state::AppState::new_with_operator_auth(
            state::OperatorAuthMode::ExplicitDevNoToken,
        ));
        assert_eq!(st.max_new_orders_per_tick().await, None);
    }
}
