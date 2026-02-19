use crate::{
    Bar, FeedId, IntegrityAction, IntegrityConfig, IntegrityDecision, IntegrityReason,
    IntegrityState, Timeframe,
};

fn expected_next_end_ts(prev_end_ts: i64, tf: Timeframe) -> i64 {
    prev_end_ts + tf.interval_secs
}

/// Record a feed tick (used for stale detection).
pub fn tick_feed(
    cfg: &IntegrityConfig,
    st: &mut IntegrityState,
    feed: &FeedId,
    now_tick: u64,
) -> IntegrityDecision {
    // Sticky states take precedence.
    if st.halted {
        return IntegrityDecision {
            action: IntegrityAction::Halt,
            reason: IntegrityReason::AlreadyHalted,
        };
    }
    if st.disarmed {
        return IntegrityDecision {
            action: IntegrityAction::Disarm,
            reason: IntegrityReason::AlreadyDisarmed,
        };
    }

    st.last_feed_tick.insert(feed.clone(), now_tick);

    // If stale threshold is set, enforce that *all known feeds* are fresh relative to now_tick.
    if cfg.stale_threshold_ticks > 0 {
        for last in st.last_feed_tick.values() {
            if now_tick.saturating_sub(*last) > cfg.stale_threshold_ticks {
                st.disarmed = true;
                return IntegrityDecision {
                    action: IntegrityAction::Disarm,
                    reason: IntegrityReason::StaleFeed,
                };
            }
        }
    }

    IntegrityDecision {
        action: IntegrityAction::Allow,
        reason: IntegrityReason::Allowed,
    }
}

/// Evaluate a bar under integrity rules.
///
/// Inputs:
/// - feed: which source this bar came from
/// - now_tick: monotonic tick id from runtime (not wall clock)
pub fn evaluate_bar(
    cfg: &IntegrityConfig,
    st: &mut IntegrityState,
    feed: &FeedId,
    now_tick: u64,
    bar: &Bar,
) -> IntegrityDecision {
    // Sticky states take precedence.
    if st.halted {
        return IntegrityDecision {
            action: IntegrityAction::Halt,
            reason: IntegrityReason::AlreadyHalted,
        };
    }
    if st.disarmed {
        return IntegrityDecision {
            action: IntegrityAction::Disarm,
            reason: IntegrityReason::AlreadyDisarmed,
        };
    }

    // Update feed freshness and enforce stale feed policy.
    st.last_feed_tick.insert(feed.clone(), now_tick);
    if cfg.stale_threshold_ticks > 0 {
        for last in st.last_feed_tick.values() {
            if now_tick.saturating_sub(*last) > cfg.stale_threshold_ticks {
                st.disarmed = true;
                return IntegrityDecision {
                    action: IntegrityAction::Disarm,
                    reason: IntegrityReason::StaleFeed,
                };
            }
        }
    }

    // 1) No lookahead ever: reject incomplete bars.
    if !bar.is_complete {
        return IntegrityDecision {
            action: IntegrityAction::Reject,
            reason: IntegrityReason::IncompleteBar,
        };
    }

    // 2) Gap detection on complete bars.
    let sym_tf = (bar.key.symbol.clone(), bar.key.tf);
    if let Some(prev_end) = st.last_complete_end_ts.get(&sym_tf).copied() {
        let expected = expected_next_end_ts(prev_end, bar.key.tf);
        if bar.key.end_ts > expected {
            // Missing bars count = (delta / interval) - 1 (integer, deterministic)
            let delta = bar.key.end_ts - prev_end;
            let interval = bar.key.tf.interval_secs;
            let steps = delta / interval;
            let missing = if steps > 0 { (steps - 1) as u32 } else { 0 };

            if missing > cfg.gap_tolerance_bars {
                st.halted = true;
                return IntegrityDecision {
                    action: IntegrityAction::Halt,
                    reason: IntegrityReason::GapDetected,
                };
            }
        }
    }

    // 3) Feed disagreement detection (same BarKey, different fingerprint).
    if cfg.enforce_feed_disagreement {
        let entry = st.fingerprints.entry(bar.key.clone()).or_default();

        entry.insert(feed.clone(), (bar.close_micros, bar.volume));

        // If we have 2+ feeds for this key, require all fingerprints identical.
        if entry.len() >= 2 {
            let mut it = entry.values();
            let first = it.next().copied().unwrap();
            if it.any(|v| *v != first) {
                st.halted = true;
                return IntegrityDecision {
                    action: IntegrityAction::Halt,
                    reason: IntegrityReason::FeedDisagreement,
                };
            }
        }
    }

    // Update last complete end_ts (monotonic per symbol/tf).
    st.last_complete_end_ts.insert(sym_tf, bar.key.end_ts);

    IntegrityDecision {
        action: IntegrityAction::Allow,
        reason: IntegrityReason::Allowed,
    }
}
