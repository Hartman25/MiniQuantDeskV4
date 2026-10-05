use crate::{BarStub, HeldPositionSeed};

/// The latest `n` bars when every one is complete with a positive close and
/// there are at least `n` bars; `None` otherwise (callers fail closed to flat).
/// An incomplete latest bar is rejected by the same rule, so it can never
/// create an action.
pub(crate) fn complete_positive_tail(recent: &[BarStub], n: usize) -> Option<&[BarStub]> {
    if recent.len() < n {
        return None;
    }
    let tail = &recent[recent.len() - n..];
    if tail.iter().any(|b| !b.is_complete || b.close_micros <= 0) {
        return None;
    }
    Some(tail)
}

/// Like [`complete_positive_tail`], and additionally every open/high/low is positive with
/// `low <= high` (engines that read the full bar range).
pub(crate) fn complete_valid_ohlc_tail(recent: &[BarStub], n: usize) -> Option<&[BarStub]> {
    let tail = complete_positive_tail(recent, n)?;
    tail.iter()
        .all(|b| b.open_micros > 0 && b.low_micros > 0 && b.low_micros <= b.high_micros)
        .then_some(tail)
}

/// Shared stateful-engine driver: the instance owns `state`; on its first call a fresh
/// instance replays `step` over every earlier completed bar of that call's window (a no-op
/// for a one-bar window). A window shorter than `required` resets to `flat`; an incomplete
/// latest bar holds the state. `step` receives exactly `required` bars ending at the bar
/// being decided and fails closed to `flat` on a malformed window.
pub(crate) fn advance_state<S: Copy>(
    state: &mut S,
    initialized: &mut bool,
    bars: &[BarStub],
    required: usize,
    flat: S,
    step: impl Fn(S, &[BarStub]) -> S,
) {
    if !*initialized {
        *initialized = true;
        for t in 0..bars.len().saturating_sub(1) {
            if t + 1 >= required && bars[t].is_complete {
                *state = step(*state, &bars[t + 1 - required..=t]);
            }
        }
    }
    if bars.len() < required {
        *state = flat;
        return;
    }
    if bars.last().is_some_and(|b| b.is_complete) {
        *state = step(*state, &bars[bars.len() - required..]);
    }
}

/// Durable seed for an engine whose whole state is long/flat: a held record for `symbol` is
/// Long, no record is Flat. The state is then authoritative, so the first call never replays.
pub(crate) fn restore_long_flat<S: Copy>(
    state: &mut S,
    initialized: &mut bool,
    symbol: &str,
    held: &[HeldPositionSeed],
    flat: S,
    long: S,
) {
    *initialized = true;
    *state = if held.iter().any(|h| h.symbol == symbol) {
        long
    } else {
        flat
    };
}

/// Hold counter of a fixed-length hold machine at the last complete bar of `bars`, derived from
/// the durable entry anchor: the entry bar is `held = 1`, then every later complete bar steps the
/// machine (`step` receives exactly `required` bars ending at that bar).
///
/// Fails closed to `0` (flat) when the state is not provable from `bars`: a window shorter than
/// `required`, an anchor bar that is absent or does not itself start a cycle, or a cycle boundary
/// whose full decision window lies before the first supplied bar. Counting inside a cycle needs no
/// earlier history, so the first cycle is exact for any window containing the anchor.
pub(crate) fn hold_phase_from_anchor(
    bars: &[BarStub],
    anchor_end_ts: i64,
    required: usize,
    hold_outputs: u8,
    step: impl Fn(u8, &[BarStub]) -> u8,
) -> u8 {
    if bars.len() < required {
        return 0;
    }
    let Some(i0) = bars
        .iter()
        .position(|b| b.is_complete && b.end_ts == anchor_end_ts)
    else {
        return 0;
    };
    if i0 + 1 >= required && step(0, &bars[i0 + 1 - required..=i0]) != 1 {
        return 0;
    }
    let mut held = 1u8;
    for j in i0 + 1..bars.len() {
        if !bars[j].is_complete {
            continue;
        }
        if j + 1 >= required {
            held = step(held, &bars[j + 1 - required..=j]);
        } else if (1..hold_outputs).contains(&held) {
            held += 1;
        } else {
            return 0;
        }
    }
    held
}

/// Restart-equivalence harness shared by the stateful engines' tests.
#[cfg(test)]
pub(crate) mod restart_proof {
    use crate::{BarStub, HeldPositionSeed, RecentBarsWindow, Strategy, StrategyContext};

    pub(crate) const DAY: i64 = 86_400;

    pub(crate) fn stamped(mut bars: Vec<BarStub>) -> Vec<BarStub> {
        for (i, b) in bars.iter_mut().enumerate() {
            b.end_ts = (i as i64 + 1) * DAY;
        }
        bars
    }

    pub(crate) fn decide<S: Strategy>(s: &mut S, window: &[BarStub]) -> i64 {
        let ctx = StrategyContext::new(
            DAY,
            0,
            RecentBarsWindow::new(window.len().max(1), window.to_vec()),
        );
        s.on_bar(&ctx).targets[0]
            .qty
            .to_whole_units_checked()
            .unwrap()
    }

    pub(crate) fn continuous<S: Strategy>(mut s: S, bars: &[BarStub]) -> Vec<i64> {
        (1..=bars.len())
            .map(|i| decide(&mut s, &bars[..i]))
            .collect()
    }

    /// Entry bar of the contiguous Long run ending at `r - 1`: the anchor the durable record
    /// of a never-restarted instance holds when bar `r` is next to decide.
    pub(crate) fn anchor(cont: &[i64], bars: &[BarStub], r: usize) -> Option<i64> {
        if r == 0 || cont[r - 1] <= 0 {
            return None;
        }
        let start = (0..r).rev().take_while(|&i| cont[i] > 0).last()?;
        Some(bars[start].end_ts)
    }

    /// Decisions for bars `r..` of an instance started at bar `r`; `restore` seeds it from the
    /// durable record for `symbol` (a mutant passes `None` with a pre-set instance).
    pub(crate) fn restarted<S: Strategy>(
        mut s: S,
        restore: Option<&str>,
        cont: &[i64],
        bars: &[BarStub],
        r: usize,
        cap: usize,
    ) -> Vec<i64> {
        if let Some(symbol) = restore {
            let seeds: Vec<HeldPositionSeed> = anchor(cont, bars, r)
                .map(|ts| HeldPositionSeed {
                    symbol: symbol.to_string(),
                    entry_bar_end_ts: ts,
                })
                .into_iter()
                .collect();
            s.restore_held_positions(&seeds);
        }
        (r..bars.len())
            .map(|i| decide(&mut s, &bars[(i + 1).saturating_sub(cap)..=i]))
            .collect()
    }

    /// Boundaries `r` in `from..len` whose restarted stream differs from the continuous one.
    pub(crate) fn diverging_restarts<S: Strategy>(
        make: impl Fn() -> S,
        restore: Option<&str>,
        cont: &[i64],
        bars: &[BarStub],
        from: usize,
        cap: usize,
    ) -> Vec<usize> {
        (from..bars.len())
            .filter(|&r| restarted(make(), restore, cont, bars, r, cap) != cont[r..])
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bar(close: i64, complete: bool) -> BarStub {
        BarStub::new(0, complete, close, 1)
    }

    #[test]
    fn requires_length_completeness_and_positive_closes() {
        let ok: Vec<BarStub> = (0..5).map(|_| bar(10, true)).collect();
        assert_eq!(complete_positive_tail(&ok, 5).map(|t| t.len()), Some(5));
        assert!(complete_positive_tail(&ok, 6).is_none());
        let mut bad = ok.clone();
        bad[0] = bar(10, false);
        assert!(complete_positive_tail(&bad, 5).is_none());
        let mut bad = ok.clone();
        bad[2] = bar(0, true);
        assert!(complete_positive_tail(&bad, 5).is_none());
        // Bars older than the tail do not matter.
        let mut old_bad = vec![bar(-1, false)];
        old_bad.extend(ok);
        assert_eq!(
            complete_positive_tail(&old_bad, 5).map(|t| t.len()),
            Some(5)
        );
    }
}
