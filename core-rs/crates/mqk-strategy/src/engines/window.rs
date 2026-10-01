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
