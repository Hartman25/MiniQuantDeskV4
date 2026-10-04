use crate::BarStub;

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
