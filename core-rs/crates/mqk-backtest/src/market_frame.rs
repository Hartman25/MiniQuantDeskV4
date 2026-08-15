//! BKT-MULTISYMBOL-MARKET-FRAME-01 — deterministic multi-symbol market frames.
//!
//! # Problem
//!
//! [`crate::engine::BacktestEngine`] processes [`BacktestBar`]s one at a time
//! into a single global `recent_bars: Vec<BarStub>`. For a single-symbol
//! backtest that is harmless (every bar shares the same symbol), but it is
//! architecturally unsafe for cross-sectional research: if bars from several
//! symbols are ever fed through that path, one symbol's history stream would
//! be contaminated with another symbol's bars.
//!
//! This module adds a standalone, additive data layer — it does not modify
//! `BacktestEngine`, `StrategyHost`, or any existing single-symbol execution
//! path. Nothing here is wired into `BacktestEngine::run`, and no order or
//! fill can be submitted from this module. It exists so a cross-sectional
//! research evaluator can be run against a coherent, symbol-isolated,
//! anti-lookahead view of the market, while every existing single-symbol
//! backtest keeps using the unmodified engine path unchanged. Multi-symbol
//! *economic execution* (orders/fills) is deferred to the separate
//! `BKT-FUTURE-EXECUTION-01` patch — nothing here submits an order.
//!
//! # Two entry points
//!
//! - [`evaluate_market_frames`] — the real evaluation seam. Streams frames to
//!   a [`MarketFrameEvaluator`] one timestamp at a time via a borrowed
//!   [`MarketFrameView`]. This is the path a research/backtest caller should
//!   use: its retained-state footprint is bounded by `symbols ×
//!   bar_history_len`, not by the number of frames in the run (see
//!   "Bounded evaluation" below).
//! - [`build_market_frames`] — a small-scale diagnostic/testing helper that
//!   *does* materialize a `Vec<MarketFrame>` with an owned history snapshot
//!   per frame (`O(frames × symbols × bar_history_len)`). It exists for
//!   tests and small ad-hoc inspection where holding the whole run in memory
//!   at once is convenient. Do not use it for multi-year/high-frequency
//!   research universes — use [`evaluate_market_frames`] instead.
//!
//! # Anti-lookahead
//!
//! Both entry points group the input bars into a
//! `BTreeMap<end_ts, BTreeMap<symbol, BacktestBar>>` before any frame is
//! produced, and fold each symbol's bounded history forward one `end_ts`
//! bucket at a time, in ascending key order. Because the grouping key is the
//! map key (not input position), **the resulting frame sequence is
//! independent of the order bars were passed in** — reordering rows in a CSV,
//! or shuffling a `Vec<BacktestBar>` before calling either function, cannot
//! change which bars land in which frame or what any frame's `history()`
//! contains. A frame for timestamp `T` is built strictly from bars with
//! `end_ts <= T`; bars with `end_ts > T` are not visited until their own
//! bucket is processed, so no future bar can be observed while evaluating `T`.
//!
//! # Missing-symbol contract
//!
//! - `current(symbol)` returns `None` for a symbol absent from this exact
//!   timestamp. No previous bar is substituted — a caller must not be able to
//!   mistake a stale bar for a contemporaneous one.
//! - `history(symbol)` returns the symbol's bounded window of bars *actually
//!   observed* up to and including this frame. If a symbol is momentarily
//!   absent, its history is simply not extended this frame (no
//!   synthetic/forward-filled bar is inserted) — the window keeps whatever
//!   was last observed. A symbol that has never appeared at or before this
//!   frame returns `None`.
//!
//! # History contract
//!
//! `history(symbol)` is bounded to `bar_history_len` (same truncate-the-tail
//! semantics as [`RecentBarsWindow`]) and, when the symbol is present in the
//! current frame, its last entry *is* that frame's `current(symbol)` bar —
//! i.e. history is inclusive of the current bar, matching the existing
//! single-symbol engine convention where `ctx.recent.last()` is always the
//! bar that was just fed.
//!
//! # Duplicate `(symbol, end_ts)` contract — fail closed
//!
//! A second bar for the same `(symbol, end_ts)` pair is ambiguous data
//! provenance: there is no principled, order-invariant way to decide which
//! row is "correct". Both entry points therefore refuse the *entire* input
//! deterministically — [`MarketFrameError::DuplicateBar`] — before producing
//! or evaluating a single frame. This is checked purely by key identity
//! (symbol, end_ts), so it fires the same way regardless of which duplicate
//! arrived first, whether the rows are byte-identical, or how the rest of
//! the input is ordered.
//!
//! # Bounded evaluation
//!
//! [`evaluate_market_frames`] keeps exactly one mutable state across the
//! whole run: `histories: BTreeMap<String, RecentBarsWindow>`, one bounded
//! window per symbol. Each timestamp bucket updates that map in place and
//! then hands the evaluator a [`MarketFrameView`] that *borrows* it — no
//! `Vec<MarketFrame>` and no per-frame history snapshot is ever
//! materialized. Peak retained history is therefore `O(symbols ×
//! bar_history_len)`, independent of how many timestamps the run covers. (The
//! grouped raw input bars themselves remain resident for the duration of the
//! call, same as `BacktestEngine::run(&self, bars: &[BacktestBar])` already
//! requires the whole input slice.)
//!
//! # Execution safety
//!
//! This module is signal/observation only. It defines no order, fill, or
//! execution semantics, and nothing here can cause an order for symbol B to
//! be priced from symbol A's bar — there is no execution path here at all.
//! Multi-symbol economic execution is deferred to `BKT-FUTURE-EXECUTION-01`.

use std::collections::BTreeMap;

use mqk_strategy::{BarStub, RecentBarsWindow, StrategyContext};

use crate::types::BacktestBar;

/// Errors from building or evaluating market frames.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MarketFrameError {
    /// A second bar for the same `(symbol, end_ts)` pair was seen in the
    /// input. Duplicate source rows are ambiguous data provenance — there is
    /// no determinism-preserving way to pick a winner, so the whole input is
    /// refused: zero frames are built, zero evaluations happen.
    DuplicateBar { symbol: String, end_ts: i64 },
}

impl core::fmt::Display for MarketFrameError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            MarketFrameError::DuplicateBar { symbol, end_ts } => write!(
                f,
                "duplicate bar for symbol={} end_ts={}: ambiguous data provenance, refusing to guess a winner",
                symbol, end_ts
            ),
        }
    }
}

impl std::error::Error for MarketFrameError {}

/// A deterministic, timestamp-synchronous snapshot of every symbol's bar at
/// one canonical `end_ts`, plus each symbol's isolated bounded history up to
/// and including that timestamp.
///
/// Construct sequences of these via [`build_market_frames`] — a small-scale
/// diagnostic helper that owns a full history snapshot per frame. For real
/// research runs, prefer [`evaluate_market_frames`], which streams the
/// equivalent data via the borrowed [`MarketFrameView`] without that
/// per-frame duplication.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MarketFrame {
    end_ts: i64,
    /// Symbols present at exactly this `end_ts`. Keyed by symbol for
    /// deterministic (alphabetical) iteration; absence is explicit (no entry).
    bars: BTreeMap<String, BacktestBar>,
    /// Every symbol ever observed at or before this `end_ts`, each with its
    /// own bounded, isolated window. A symbol not present this frame keeps
    /// its previous window unchanged (no synthetic bar inserted).
    histories: BTreeMap<String, RecentBarsWindow>,
}

impl MarketFrame {
    /// Canonical timestamp shared by every bar in this frame.
    pub fn end_ts(&self) -> i64 {
        self.end_ts
    }

    /// The bar for `symbol` at exactly this frame's `end_ts`, or `None` if
    /// `symbol` did not report a bar at this timestamp. Never a substitute
    /// from an earlier timestamp.
    pub fn current(&self, symbol: &str) -> Option<&BacktestBar> {
        self.bars.get(symbol)
    }

    /// `symbol`'s bounded history of actually-observed bars, ending at or
    /// before this frame's `end_ts` (inclusive of the current bar when
    /// `symbol` is present this frame). `None` if `symbol` has never been
    /// observed at or before this frame.
    pub fn history(&self, symbol: &str) -> Option<&RecentBarsWindow> {
        self.histories.get(symbol)
    }

    /// `true` if `symbol` reported a bar at exactly this frame's `end_ts`.
    pub fn is_present(&self, symbol: &str) -> bool {
        self.bars.contains_key(symbol)
    }

    /// Symbols present at exactly this frame's `end_ts`, in deterministic
    /// (alphabetical) order.
    pub fn symbols(&self) -> impl Iterator<Item = &str> {
        self.bars.keys().map(|s| s.as_str())
    }
}

/// A borrowed, single-timestamp view over the same data [`MarketFrame`]
/// exposes, used by [`evaluate_market_frames`]. Unlike `MarketFrame`, this
/// never owns a history snapshot: `history()` reads straight through to the
/// evaluation loop's single shared `histories` map, so producing one of
/// these costs nothing beyond the reference itself.
///
/// Valid only for the duration of the [`MarketFrameEvaluator::on_frame`]
/// call it was passed to — the evaluation loop mutates the underlying
/// histories for the *next* timestamp only after that call returns.
#[derive(Debug)]
pub struct MarketFrameView<'a> {
    end_ts: i64,
    bars: &'a BTreeMap<String, BacktestBar>,
    histories: &'a BTreeMap<String, RecentBarsWindow>,
}

impl<'a> MarketFrameView<'a> {
    /// Canonical timestamp shared by every bar in this frame.
    pub fn end_ts(&self) -> i64 {
        self.end_ts
    }

    /// The bar for `symbol` at exactly this frame's `end_ts`, or `None` if
    /// `symbol` did not report a bar at this timestamp.
    pub fn current(&self, symbol: &str) -> Option<&BacktestBar> {
        self.bars.get(symbol)
    }

    /// `symbol`'s bounded history of actually-observed bars, ending at or
    /// before this frame's `end_ts` (inclusive of the current bar when
    /// `symbol` is present this frame). `None` if `symbol` has never been
    /// observed at or before this frame.
    pub fn history(&self, symbol: &str) -> Option<&RecentBarsWindow> {
        self.histories.get(symbol)
    }

    /// `true` if `symbol` reported a bar at exactly this frame's `end_ts`.
    pub fn is_present(&self, symbol: &str) -> bool {
        self.bars.contains_key(symbol)
    }

    /// Symbols present at exactly this frame's `end_ts`, in deterministic
    /// (alphabetical) order.
    pub fn symbols(&self) -> impl Iterator<Item = &str> {
        self.bars.keys().map(|s| s.as_str())
    }

    /// Of `universe`, the symbols *not* present at exactly this frame's
    /// `end_ts` — explicit absence, in the same order as `universe`.
    pub fn missing<'u>(&self, universe: &'u [&str]) -> Vec<&'u str> {
        universe
            .iter()
            .copied()
            .filter(|s| !self.is_present(s))
            .collect()
    }
}

/// A cross-sectional research/signal evaluator driven by
/// [`evaluate_market_frames`]. Implementations may inspect any symbol's
/// current bar or bounded history via `frame`, and compare multiple symbols
/// within the same call — but this seam has no order/fill submission
/// capability, so nothing an implementation does here can cause a trade.
pub trait MarketFrameEvaluator {
    /// Called once per distinct `end_ts`, in strictly ascending timestamp
    /// order, with a frame containing every symbol observed at that
    /// timestamp plus each symbol's bounded history up to and including it.
    fn on_frame(&mut self, frame: &MarketFrameView<'_>);
}

/// Group `bars` into a deterministic `end_ts -> symbol -> bar` structure,
/// failing closed on the first duplicate `(symbol, end_ts)` key encountered.
/// Because the check is by key identity alone, the failure is identical
/// regardless of input row order or which duplicate arrived first.
fn group_bars_by_ts_then_symbol(
    bars: &[BacktestBar],
) -> Result<BTreeMap<i64, BTreeMap<String, BacktestBar>>, MarketFrameError> {
    let mut by_ts: BTreeMap<i64, BTreeMap<String, BacktestBar>> = BTreeMap::new();
    for b in bars {
        let symbol_bars = by_ts.entry(b.end_ts).or_default();
        if symbol_bars.contains_key(&b.symbol) {
            return Err(MarketFrameError::DuplicateBar {
                symbol: b.symbol.clone(),
                end_ts: b.end_ts,
            });
        }
        symbol_bars.insert(b.symbol.clone(), b.clone());
    }
    Ok(by_ts)
}

/// Fold one timestamp bucket's bars into each present symbol's bounded
/// window in place. Symbols absent this bucket are left untouched — no
/// forward fill.
fn fold_frame_into_histories(
    symbol_bars: &BTreeMap<String, BacktestBar>,
    max_len: usize,
    histories: &mut BTreeMap<String, RecentBarsWindow>,
) {
    for (symbol, bar) in symbol_bars {
        let stub = BarStub::with_ohlcv(
            bar.end_ts,
            bar.is_complete,
            bar.open_micros,
            bar.high_micros,
            bar.low_micros,
            bar.close_micros,
            bar.volume,
        );
        let window = histories
            .entry(symbol.clone())
            .or_insert_with(|| RecentBarsWindow::new(max_len, Vec::new()));
        window.bars.push(stub);
        if window.bars.len() > max_len {
            let start = window.bars.len() - max_len;
            window.bars = window.bars.split_off(start);
        }
    }
}

/// Stream `bars` through `evaluator` as a deterministic, ascending-`end_ts`
/// sequence of frames — the real cross-sectional evaluation seam.
///
/// This is the primary entry point for research/backtest callers. It never
/// materializes a `Vec<MarketFrame>` or a per-frame history snapshot: the
/// only state carried across timestamps is one bounded `RecentBarsWindow`
/// per symbol, mutated in place and exposed to `evaluator` through a
/// borrowed [`MarketFrameView`]. See the module docs' "Bounded evaluation"
/// section.
///
/// `bar_history_len` must be > 0 (mirrors [`RecentBarsWindow::new`]'s
/// contract); a value of `0` is defensively treated as `1` in release builds
/// and debug-asserts in debug builds, matching `RecentBarsWindow`'s own
/// guard.
///
/// # Errors
///
/// Returns [`MarketFrameError::DuplicateBar`] if `bars` contains two entries
/// for the same `(symbol, end_ts)` pair. Validation happens for the entire
/// input before any frame is evaluated, so on error `evaluator.on_frame` is
/// never called — zero evaluations, not a partial run.
///
/// No order, fill, or execution call is ever made from this function or from
/// `evaluator.on_frame` — `MarketFrameEvaluator` has no such capability.
pub fn evaluate_market_frames<E: MarketFrameEvaluator>(
    bars: &[BacktestBar],
    bar_history_len: usize,
    evaluator: &mut E,
) -> Result<(), MarketFrameError> {
    debug_assert!(bar_history_len > 0, "bar_history_len must be > 0");
    let max_len = bar_history_len.max(1);

    let by_ts = group_bars_by_ts_then_symbol(bars)?;

    // Entire retained cross-timestamp state: one bounded window per symbol.
    // O(symbols x bar_history_len), never O(frames x symbols x bar_history_len).
    let mut histories: BTreeMap<String, RecentBarsWindow> = BTreeMap::new();

    for (end_ts, symbol_bars) in by_ts {
        fold_frame_into_histories(&symbol_bars, max_len, &mut histories);

        let view = MarketFrameView {
            end_ts,
            bars: &symbol_bars,
            histories: &histories,
        };
        evaluator.on_frame(&view);
    }

    Ok(())
}

/// Group `bars` into a deterministic, ascending-`end_ts` sequence of owned
/// [`MarketFrame`]s, with each symbol's history isolated and bounded to
/// `bar_history_len`.
///
/// This is a small-scale diagnostic/testing helper: it owns a full history
/// snapshot per frame (`O(frames × symbols × bar_history_len)` total). Real
/// research runs over a large timestamp range should use
/// [`evaluate_market_frames`] instead, which never materializes this `Vec`.
///
/// `bar_history_len` must be > 0 (mirrors [`RecentBarsWindow::new`]'s
/// contract); a value of `0` is defensively treated as `1` in release builds
/// and debug-asserts in debug builds, matching `RecentBarsWindow`'s own
/// guard.
///
/// # Determinism
///
/// The result depends only on the *set* of `(symbol, end_ts)` bars in
/// `bars`, never their input order — see the module-level docs for why.
///
/// # Errors
///
/// Returns [`MarketFrameError::DuplicateBar`] if `bars` contains two entries
/// for the same `(symbol, end_ts)` pair — see the module docs' "Duplicate
/// `(symbol, end_ts)` contract" section. On error, no frames are returned.
pub fn build_market_frames(
    bars: &[BacktestBar],
    bar_history_len: usize,
) -> Result<Vec<MarketFrame>, MarketFrameError> {
    debug_assert!(bar_history_len > 0, "bar_history_len must be > 0");
    let max_len = bar_history_len.max(1);

    let by_ts = group_bars_by_ts_then_symbol(bars)?;

    let mut cumulative_histories: BTreeMap<String, RecentBarsWindow> = BTreeMap::new();
    let mut frames = Vec::with_capacity(by_ts.len());

    for (end_ts, symbol_bars) in by_ts {
        fold_frame_into_histories(&symbol_bars, max_len, &mut cumulative_histories);

        // Snapshot every symbol's current window (including symbols absent
        // this frame but seen previously) into this frame's own copy, so
        // later frames' mutation of `cumulative_histories` can never be
        // observed through an already-built frame. This clone is exactly
        // the cost `evaluate_market_frames` avoids.
        frames.push(MarketFrame {
            end_ts,
            bars: symbol_bars,
            histories: cumulative_histories.clone(),
        });
    }

    Ok(frames)
}

/// Compatibility adapter: extract a `mqk_strategy::StrategyContext` for a
/// single `symbol` from `frame`, so an existing Tier-A [`mqk_strategy::Strategy`]
/// can run unmodified against the market-frame data path.
///
/// Returns `None` if `symbol` has no history in `frame` (never observed at or
/// before `frame.end_ts()`) — mirrors [`MarketFrame::history`].
///
/// This does not register with [`mqk_strategy::StrategyHost`] or change any
/// existing engine behavior; it is a pure extraction helper proving that the
/// new frame layer is additive, not a replacement for the existing
/// single-symbol context construction in `BacktestEngine::run`.
pub fn strategy_context_for_symbol(
    frame: &MarketFrame,
    symbol: &str,
    timeframe_secs: i64,
    now_tick: u64,
) -> Option<StrategyContext> {
    let window = frame.history(symbol)?.clone();
    Some(StrategyContext::new(timeframe_secs, now_tick, window))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(symbol: &str, end_ts: i64, close: i64) -> BacktestBar {
        BacktestBar::new(symbol, end_ts, close, close + 1, close - 1, close, 100)
    }

    #[test]
    fn single_frame_groups_same_timestamp_symbols() {
        let bars = vec![bar("AAPL", 60, 100), bar("AMD", 60, 200), bar("SPY", 60, 400)];
        let frames = build_market_frames(&bars, 10).unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].end_ts(), 60);
        assert!(frames[0].current("AAPL").is_some());
        assert!(frames[0].current("AMD").is_some());
        assert!(frames[0].current("SPY").is_some());
    }

    #[test]
    fn missing_symbol_is_explicit_none_not_forward_filled() {
        let bars = vec![
            bar("AAPL", 60, 100),
            bar("NVDA", 60, 300),
            // NVDA absent at 120
            bar("AAPL", 120, 101),
        ];
        let frames = build_market_frames(&bars, 10).unwrap();
        assert_eq!(frames.len(), 2);
        let f120 = &frames[1];
        assert!(f120.current("NVDA").is_none(), "NVDA missing at 120 must be None");
        // History persists last-known NVDA data, unchanged, not extended.
        let nvda_hist = f120.history("NVDA").expect("NVDA history exists from t=60");
        assert_eq!(nvda_hist.len(), 1);
        assert_eq!(nvda_hist.last().unwrap().end_ts, 60);
    }

    #[test]
    fn duplicate_symbol_end_ts_fails_closed_in_build_market_frames() {
        let bars = vec![bar("AAPL", 60, 100), bar("AAPL", 60, 101)];
        let err = build_market_frames(&bars, 10).unwrap_err();
        assert_eq!(
            err,
            MarketFrameError::DuplicateBar {
                symbol: "AAPL".to_string(),
                end_ts: 60
            }
        );
    }

    #[test]
    fn duplicate_symbol_end_ts_fails_closed_in_evaluate_market_frames() {
        struct Counter(u32);
        impl MarketFrameEvaluator for Counter {
            fn on_frame(&mut self, _frame: &MarketFrameView<'_>) {
                self.0 += 1;
            }
        }
        let bars = vec![bar("AAPL", 60, 100), bar("AAPL", 60, 101)];
        let mut counter = Counter(0);
        let err = evaluate_market_frames(&bars, 10, &mut counter).unwrap_err();
        assert_eq!(
            err,
            MarketFrameError::DuplicateBar {
                symbol: "AAPL".to_string(),
                end_ts: 60
            }
        );
        assert_eq!(counter.0, 0, "no evaluation may happen when the input is refused");
    }
}
