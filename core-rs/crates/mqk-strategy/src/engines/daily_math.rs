//! Exact integer (`i128`, micros) building blocks shared by the daily long/flat engines.
//! Every function takes a window whose LAST bar is the decision bar `t` and whose length the
//! caller has already validated (complete, positive, `low <= high`); none reads a bar after `t`.

use crate::BarStub;

pub(crate) fn close(b: &BarStub) -> i128 {
    i128::from(b.close_micros)
}

pub(crate) fn close_sum(bars: &[BarStub]) -> i128 {
    bars.iter().map(close).sum()
}

/// `close[t] > SMA` of the latest `n` closes including `t`, as `n * close > sum`.
/// Equality is NOT above the average.
pub(crate) fn above_sma(win: &[BarStub], n: usize) -> bool {
    let t = win.len() - 1;
    n as i128 * close(&win[t]) > close_sum(&win[t + 1 - n..])
}

/// Sum of the true ranges of the `n` bars immediately BEFORE `t` (bars `t-n..=t-1`, each
/// against its own prior close, so closes from `t-n-1` are read). `ATR = sum / n`.
pub(crate) fn prior_true_range_sum(win: &[BarStub], n: usize) -> i128 {
    let t = win.len() - 1;
    (t - n..t)
        .map(|i| {
            let prev = close(&win[i - 1]);
            let (h, l) = (
                i128::from(win[i].high_micros),
                i128::from(win[i].low_micros),
            );
            (h - l).max((h - prev).abs()).max((l - prev).abs())
        })
        .sum()
}

/// `(gain_sum, loss_sum)` of the `n` one-session close differences ending at `t`
/// (`close[i] - close[i-1]` for `i = t-n+1..=t`). The Cutler means divide both by `n`, so the
/// RSI ratio `gain / (gain + loss)` is the same on the sums.
pub(crate) fn gain_loss_sums(win: &[BarStub], n: usize) -> (i128, i128) {
    let t = win.len() - 1;
    (t + 1 - n..=t).fold((0, 0), |(g, l), i| {
        let d = close(&win[i]) - close(&win[i - 1]);
        (g + d.max(0), l + (-d).max(0))
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn closes(v: &[i64]) -> Vec<BarStub> {
        v.iter().map(|&c| BarStub::new(0, true, c, 1)).collect()
    }

    #[test]
    fn sma_comparison_is_strict_and_exact() {
        // mean of [1,2,3] = 2 -> close 3 above, close 2 equal, 1 below.
        assert!(above_sma(&closes(&[1, 2, 3]), 3));
        assert!(!above_sma(&closes(&[1, 3, 2]), 3));
        assert!(!above_sma(&closes(&[3, 2, 1]), 3));
    }

    #[test]
    fn true_range_uses_prior_close_gaps_and_excludes_the_decision_bar() {
        let mut w = closes(&[100, 100, 100, 100]);
        // bar 1: gap up above prior close; bar 2: plain range; decision bar 3 is excluded.
        w[1] = BarStub::with_ohlcv(0, true, 100, 130, 100, 110, 1);
        w[2] = BarStub::with_ohlcv(0, true, 110, 120, 105, 110, 1);
        w[3] = BarStub::with_ohlcv(0, true, 100, 10_000, 1, 100, 1);
        // bar1: max(30, |130-100|, |100-100|) = 30; bar2: max(15, |120-110|, |105-110|) = 15.
        assert_eq!(prior_true_range_sum(&w, 2), 45);
    }

    #[test]
    fn gain_loss_sums_cover_exactly_n_diffs() {
        let w = closes(&[50, 10, 12, 11, 15, 14]);
        // last 5 diffs: +2 -1 +4 -1 and the first diff (10-50) is excluded for n=4.
        assert_eq!(gain_loss_sums(&w, 4), (6, 2));
        assert_eq!(gain_loss_sums(&w, 5), (6, 42));
    }
}
