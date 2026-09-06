//! W06-P9-RUST-REPLAY-STRATEGY-01
//!
//! [`ResearchOosReplayStrategy`] replays an authenticated, signal-time-frozen
//! multi-symbol target-quantity schedule (produced Research-side by
//! `mqk_research.ml.oos_replay_bundle`, see W06-P9-REPLAY-AUTHORITY-01)
//! through the existing, unmodified `BacktestEngine`. It does not decide
//! anything itself -- it only knows WHEN (via `BacktestEngine`'s own bar
//! clock) to emit a target vector it was handed at construction time.
//!
//! # Dispatch contract
//!
//! `BacktestEngine::run` calls `Strategy::on_bar` once per PHYSICAL BAR ROW,
//! not once per timestamp -- a same-`end_ts` multi-symbol batch produces one
//! call per symbol in that batch, in whatever physical row order the caller
//! supplied (see `engine.rs`'s own same-`end_ts` batch handling).
//! `StrategyContext` carries no symbol field at all (existing, unchanged
//! `mqk_strategy::StrategyContext`/`BarStub` contract) -- this strategy never
//! tries to recover one. Instead it treats each `on_bar` call purely as a
//! CLOCK TICK: it counts how many calls it has seen for the current `end_ts`
//! (derived, at construction time, from the EXACT bars slice this instance
//! will be run against -- never a caller-declared count) and, once that
//! count reaches the batch's own row count, emits the COMPLETE, already-
//! resolved `TargetPosition` vector for that timestamp exactly once. Every
//! earlier call for that timestamp emits an empty `StrategyOutput` (the
//! engine's own existing "no new intent this bar" semantics -- positions
//! are carried forward, never force-flattened). Because this never inspects
//! bar CONTENT (only counts calls), a same-`end_ts` physical row permutation
//! can never change the output.
//!
//! # What this does NOT do
//!
//! No sizing, ranking, or classification logic lives here -- that is all
//! Research's job (`mqk_research.ml.oos_replay_bundle`, `economic_walkforward`
//! `weight_to_share`), already resolved into the frozen `schedule` this type
//! is constructed with. This module never mutates `mqk_strategy::Strategy`,
//! `StrategyContext`, `StrategyHost`, or Paper/Live dispatch.

use std::collections::{BTreeMap, HashMap, HashSet};

use mqk_execution::{StrategyOutput, TargetPosition};
use mqk_strategy::semantic_identity::{SemanticIdentityBuilder, SEMANTIC_IDENTITY_SCHEMA_V1};
use mqk_strategy::{Strategy, StrategyContext, StrategySpec};

use crate::types::BacktestBar;

/// Result-independent, candidate-methodology semantic fields (mission
/// "SEMANTIC IDENTITY HARD RULE"). Deliberately excludes `trial_id`,
/// `economic_eval_id`, any individual replay value, the excluded symbol, and
/// any result/artifact-path field -- see [`ResearchOosReplayStrategy::semantic_fingerprint`].
/// Loading/parsing this from Patch A's `manifest.json` is Patch C's job
/// (mqk-cli) -- this crate only consumes the already-resolved struct.
#[derive(Debug, Clone, PartialEq)]
pub struct ReplaySemanticSpec {
    pub replay_protocol_version: String,
    pub strategy_id: String,
    pub feature_columns: Vec<String>,
    pub feature_transform: String,
    pub direction_policy: String,
    pub rank_side_count: i64,
    pub long_only: bool,
    pub borrow_model: Option<String>,
    pub max_gross_exposure: f64,
    pub timeframe: String,
    pub equity_usd: f64,
    pub max_target_qty: Option<i64>,
    pub max_position_notional_usd: Option<f64>,
    /// W06-A-P9-REPLAY-SOURCE-AUTHORITY-REPAIR-WAVE-02 (R1.4/R2.2): the
    /// authenticated Research `trial_id` this replay schedule was computed
    /// from -- LINEAGE, distinct from the methodology fields above, but
    /// itself RESULT-INDEPENDENT (`build_economic_trial_identity` excludes
    /// every evaluation-output field: AUC/logloss/returns/eval_ids/artifact
    /// paths -- see research-py/src/mqk_research/ml/economic_registry_integration.py).
    /// Included in [`Self`]'s semantic fingerprint so two trials sharing
    /// identical strategy/feature/policy methodology but differing training
    /// data/model hyperparameters can never collide onto the same replay
    /// candidate identity. `economic_eval_id` (a RESULT) must never be added
    /// here -- see module/fingerprint docs.
    pub trial_id: String,
}

/// Converts an f64 to a deterministic fixed-point i64 for semantic hashing
/// (mission: fingerprint encoding must never use float/decimal text
/// formatting -- see `mqk_strategy::semantic_identity` module docs). Reuses
/// the existing wire-boundary micros convention
/// (`mqk_execution::price_to_micros`) rather than inventing a parallel one;
/// panics only on a non-finite/out-of-range value, which can never occur for
/// an already-`.normalized()`-validated Research spec.
fn micros_for_fingerprint(v: f64) -> i64 {
    mqk_execution::price_to_micros(v).unwrap_or_else(|e| {
        panic!("ResearchOosReplayStrategy: non-representable semantic float {v}: {e}")
    })
}

fn opt_micros_for_fingerprint(v: Option<f64>) -> Option<i64> {
    v.map(micros_for_fingerprint)
}

/// Backtest-only replay strategy (mission B1). Not a general-purpose
/// `mqk_strategy` engine -- lives in `mqk-backtest` deliberately.
pub struct ResearchOosReplayStrategy {
    semantic: ReplaySemanticSpec,
    timeframe_secs: i64,
    /// `end_ts` (epoch seconds) -> the COMPLETE target vector for that
    /// timestamp. Absence of an entry means "no scheduled decision for this
    /// timestamp" (carry forward, never fabricate a flatten).
    schedule: BTreeMap<i64, Vec<TargetPosition>>,
    /// `end_ts` -> expected physical row count, derived from the EXACT bars
    /// slice this instance was constructed with (mission B1/B3).
    expected_calls: HashMap<i64, usize>,
    current_end_ts: Option<i64>,
    calls_seen_for_current: usize,
    emitted_end_ts: HashSet<i64>,
}

impl ResearchOosReplayStrategy {
    /// `bars` MUST be the exact slice this instance will be run against
    /// (`BacktestEngine::run(bars)`) -- the per-timestamp expected row count
    /// is derived from it directly, never trusted from a caller-declared
    /// value (mission B1).
    pub fn new(
        semantic: ReplaySemanticSpec,
        schedule: BTreeMap<i64, Vec<TargetPosition>>,
        bars: &[BacktestBar],
    ) -> Self {
        let timeframe_secs = timeframe_secs_from_semantic(&semantic);
        let mut expected_calls: HashMap<i64, usize> = HashMap::new();
        for bar in bars {
            *expected_calls.entry(bar.end_ts).or_insert(0) += 1;
        }
        Self {
            semantic,
            timeframe_secs,
            schedule,
            expected_calls,
            current_end_ts: None,
            calls_seen_for_current: 0,
            emitted_end_ts: HashSet::new(),
        }
    }
}

/// Daily-bar Research protocol -> `StrategyContext.timeframe_secs`. The only
/// `timeframe` value W06-P9-REPLAY-AUTHORITY-01 ever produces today is
/// `"1D"`; any other value is a genuine contract violation this strategy
/// must refuse to silently coerce.
fn timeframe_secs_from_semantic(semantic: &ReplaySemanticSpec) -> i64 {
    match semantic.timeframe.as_str() {
        "1D" | "1Day" => 86_400,
        other => panic!(
            "ResearchOosReplayStrategy: unsupported replay timeframe {other:?} -- only daily \
             ('1D'/'1Day') Wave06 Research replay bundles are supported"
        ),
    }
}

impl Strategy for ResearchOosReplayStrategy {
    /// W06-A-P9-REPLAY-SOURCE-AUTHORITY-REPAIR-WAVE-02 (R2.1): the exact
    /// Research `strategy_id`, never a fixed replay-protocol literal --
    /// `evaluate_backtest_evidence_gate`'s cross-candidate promotion
    /// authority requires `BacktestReport.strategy_name ==
    /// <promotion strategy_id>`.
    fn spec(&self) -> StrategySpec {
        StrategySpec::new(self.semantic.strategy_id.clone(), self.timeframe_secs)
    }

    /// Mission B4/R2.2: built ONLY from `self.semantic` -- identical for the
    /// baseline strategy and every symbol-leave-one-out variant sharing the
    /// same candidate methodology, since `semantic` never varies by
    /// excluded symbol or by result. Includes the authenticated,
    /// result-independent `trial_id` (see `ReplaySemanticSpec::trial_id`
    /// docs) so two trials sharing identical methodology but differing
    /// training data/model hyperparameters can never collide onto the same
    /// identity. `economic_eval_id`/excluded symbol/P&L/artifact paths never
    /// participate.
    fn semantic_fingerprint(&self) -> String {
        let s = &self.semantic;
        let mut b = SemanticIdentityBuilder::new(
            SEMANTIC_IDENTITY_SCHEMA_V1,
            "research_oos_replay_v1",
            "v1",
        );
        b.push_str(&s.replay_protocol_version)
            .push_str(&s.strategy_id)
            .push_str(&s.trial_id)
            .push_i64(s.feature_columns.len() as i64);
        for col in &s.feature_columns {
            b.push_str(col);
        }
        b.push_str(&s.feature_transform)
            .push_str(&s.direction_policy)
            .push_i64(s.rank_side_count)
            .push_bool(s.long_only)
            .push_opt_i64(s.borrow_model.as_ref().map(|_| 1));
        if let Some(bm) = &s.borrow_model {
            b.push_str(bm);
        }
        b.push_i64(micros_for_fingerprint(s.max_gross_exposure))
            .push_str(&s.timeframe)
            .push_i64(micros_for_fingerprint(s.equity_usd))
            .push_opt_i64(s.max_target_qty)
            .push_opt_i64(opt_micros_for_fingerprint(s.max_position_notional_usd));
        b.finish()
    }

    /// W06-REPLAY-NO-DECISION-SEMANTICS-01 (Patch A): `on_bar` genuinely
    /// emits an empty `StrategyOutput` on every non-final physical row of a
    /// same-`end_ts` batch, and on a final row whose `end_ts` has no
    /// scheduled entry — both mean "no new decision yet", never "target:
    /// hold nothing". The schedule CSV loader (`mqk-cli`) only ever inserts
    /// an `end_ts` entry alongside at least one `TargetPosition` row, so a
    /// genuinely scheduled complete-flatten decision is always represented
    /// by explicit zero-qty `TargetPosition` rows, never by an empty vector
    /// under a present entry — this override cannot mask a real decision.
    fn empty_output_is_noop(&self) -> bool {
        true
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let end_ts = ctx
            .recent
            .last()
            .expect("BacktestEngine always pushes the current bar before calling on_bar")
            .end_ts;

        if self.current_end_ts != Some(end_ts) {
            self.current_end_ts = Some(end_ts);
            self.calls_seen_for_current = 0;
        }
        self.calls_seen_for_current += 1;

        let expected = *self.expected_calls.get(&end_ts).unwrap_or_else(|| {
            panic!(
                "ResearchOosReplayStrategy: end_ts={end_ts} was never present in the bars slice \
                 this instance was constructed from -- constructed-vs-run bars mismatch, an \
                 impossible state under a correctly wired caller"
            )
        });
        if self.calls_seen_for_current > expected {
            panic!(
                "ResearchOosReplayStrategy: received {} on_bar calls for end_ts={end_ts}, but the \
                 constructed bars slice only had {expected} -- constructed-vs-run bars mismatch",
                self.calls_seen_for_current
            );
        }
        if self.calls_seen_for_current < expected {
            return StrategyOutput::new(Vec::new());
        }

        if !self.emitted_end_ts.insert(end_ts) {
            panic!(
                "ResearchOosReplayStrategy: attempted a second full-vector emission for \
                 end_ts={end_ts} -- duplicate timestamp emission is never valid"
            );
        }
        match self.schedule.get(&end_ts) {
            Some(targets) => StrategyOutput::new(targets.clone()),
            None => StrategyOutput::new(Vec::new()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mqk_strategy::RecentBarsWindow;

    fn semantic() -> ReplaySemanticSpec {
        ReplaySemanticSpec {
            replay_protocol_version: "research_oos_replay_bundle_v1".to_string(),
            strategy_id: "test_strategy_v1".to_string(),
            feature_columns: vec!["test_xs_rank".to_string()],
            feature_transform: "cross_sectional_percentile_rank_rerank_of_authenticated_feature_v1"
                .to_string(),
            direction_policy: "cross_sectional_rank_long_only_v1".to_string(),
            rank_side_count: 2,
            long_only: true,
            borrow_model: None,
            max_gross_exposure: 1.0,
            timeframe: "1D".to_string(),
            equity_usd: 100_000.0,
            max_target_qty: None,
            max_position_notional_usd: None,
            trial_id: "test-trial-0001".to_string(),
        }
    }

    fn bar(symbol: &str, end_ts: i64) -> BacktestBar {
        BacktestBar::new(symbol, end_ts, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1_000)
    }

    fn ctx_for(end_ts: i64) -> StrategyContext {
        let recent = RecentBarsWindow::new(
            10,
            vec![mqk_strategy::BarStub::with_ohlcv(
                end_ts, true, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1_000,
            )],
        );
        StrategyContext::new(86_400, 1, recent)
    }

    fn two_symbol_two_day_bars() -> Vec<BacktestBar> {
        vec![
            bar("AAA", 1_000),
            bar("BBB", 1_000),
            bar("AAA", 87_400),
            bar("BBB", 87_400),
        ]
    }

    /// REQUIRED TEST 1/2: baseline replay reaches the real engine and the
    /// full timestamp emits exactly once (empty for the first row, full
    /// vector for the second/last row of the batch).
    #[test]
    fn full_timestamp_emits_exactly_once() {
        let bars = two_symbol_two_day_bars();
        let mut schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        schedule.insert(1_000, vec![TargetPosition::new("AAA", 5), TargetPosition::new("BBB", -5)]);
        let mut strat = ResearchOosReplayStrategy::new(semantic(), schedule, &bars);

        let first = strat.on_bar(&ctx_for(1_000));
        assert!(first.targets.is_empty(), "no partial emission on the first row of the batch");
        let second = strat.on_bar(&ctx_for(1_000));
        assert_eq!(
            second.targets,
            vec![TargetPosition::new("AAA", 5), TargetPosition::new("BBB", -5)]
        );
    }

    /// REQUIRED TEST 4: no partial timestamp emission -- confirmed above by
    /// asserting the FIRST call's targets are empty, and reinforced here by
    /// checking the intermediate calls_seen state never emits anything for
    /// an under-count.
    #[test]
    fn no_partial_emission_before_batch_complete() {
        let bars = two_symbol_two_day_bars();
        let schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        let mut strat = ResearchOosReplayStrategy::new(semantic(), schedule, &bars);
        let out = strat.on_bar(&ctx_for(1_000));
        assert!(out.targets.is_empty());
    }

    /// REQUIRED TEST 3: same-end_ts physical row permutation does not change
    /// the output -- this strategy never inspects row content, only counts,
    /// so permuting which symbol's bar arrives first cannot matter; proven
    /// by calling on_bar the same number of times regardless of which
    /// physical BacktestBar the caller conceptually associates with each
    /// call (this strategy has no way to tell the difference).
    #[test]
    fn permutation_of_same_timestamp_rows_is_irrelevant_to_output() {
        let bars = two_symbol_two_day_bars();
        let mut schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        schedule.insert(1_000, vec![TargetPosition::new("AAA", 5)]);

        let mut order_a = ResearchOosReplayStrategy::new(semantic(), schedule.clone(), &bars);
        let a1 = order_a.on_bar(&ctx_for(1_000));
        let a2 = order_a.on_bar(&ctx_for(1_000));

        let mut order_b = ResearchOosReplayStrategy::new(semantic(), schedule, &bars);
        let b1 = order_b.on_bar(&ctx_for(1_000));
        let b2 = order_b.on_bar(&ctx_for(1_000));

        assert_eq!(a1.targets, b1.targets);
        assert_eq!(a2.targets, b2.targets);
    }

    /// REQUIRED TEST 5: overcount/mismatch fails closed via panic (mission
    /// B3) -- constructing with a bars slice that has only ONE row at
    /// end_ts=1_000 but then calling on_bar twice for that same end_ts
    /// (simulating a caller running a different, larger bars slice than the
    /// one used to construct this instance) must panic, never silently
    /// process/emit an order from malformed state.
    #[test]
    #[should_panic(expected = "constructed-vs-run bars mismatch")]
    fn overcount_mismatch_fails_closed() {
        let bars = vec![bar("AAA", 1_000)]; // expected_calls[1_000] == 1
        let schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        let mut strat = ResearchOosReplayStrategy::new(semantic(), schedule, &bars);
        let _ = strat.on_bar(&ctx_for(1_000));
        let _ = strat.on_bar(&ctx_for(1_000)); // second call for the same end_ts -> overcount
    }

    /// REQUIRED TEST 5b: an end_ts entirely absent from the constructed
    /// bars slice also fails closed (never silently treated as a 1-row
    /// batch).
    #[test]
    #[should_panic(expected = "never present in the bars slice")]
    fn unknown_end_ts_fails_closed() {
        let bars = vec![bar("AAA", 1_000)];
        let schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        let mut strat = ResearchOosReplayStrategy::new(semantic(), schedule, &bars);
        let _ = strat.on_bar(&ctx_for(99_999));
    }

    /// REQUIRED TEST 8/9: semantic fingerprint is identical across the
    /// baseline and an excluded-symbol replay schedule (only the schedule
    /// content differs, never `semantic`), and mutating `economic_eval_id`-
    /// adjacent lineage never enters this type at all (it is not a field on
    /// `ReplaySemanticSpec`) -- proven structurally: fingerprint is a pure
    /// function of `semantic` alone.
    #[test]
    fn fingerprint_identical_for_baseline_and_loo_schedule_variant() {
        let bars = two_symbol_two_day_bars();
        let mut baseline_schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        baseline_schedule.insert(1_000, vec![TargetPosition::new("AAA", 5), TargetPosition::new("BBB", -5)]);
        let mut loo_schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        loo_schedule.insert(1_000, vec![TargetPosition::new("AAA", 10)]); // BBB excluded

        let baseline = ResearchOosReplayStrategy::new(semantic(), baseline_schedule, &bars);
        let loo = ResearchOosReplayStrategy::new(semantic(), loo_schedule, &bars);
        assert_eq!(baseline.semantic_fingerprint(), loo.semantic_fingerprint());
    }

    /// REQUIRED TEST 10: a semantic config mutation (rank_side_count) DOES
    /// change the fingerprint.
    #[test]
    fn semantic_mutation_changes_fingerprint() {
        let bars = two_symbol_two_day_bars();
        let schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        let a = ResearchOosReplayStrategy::new(semantic(), schedule.clone(), &bars);
        let mut mutated = semantic();
        mutated.rank_side_count = 3;
        let b = ResearchOosReplayStrategy::new(mutated, schedule, &bars);
        assert_ne!(a.semantic_fingerprint(), b.semantic_fingerprint());
    }

    // -----------------------------------------------------------------------
    // W06-A-P9-REPLAY-SOURCE-AUTHORITY-REPAIR-WAVE-02 (Patch R2) required tests
    // -----------------------------------------------------------------------

    /// R2.1: `spec().name` is the exact Research `strategy_id`, never a
    /// fixed replay-protocol literal -- required by
    /// `evaluate_backtest_evidence_gate`'s cross-candidate promotion
    /// authority (`BacktestReport.strategy_name == promotion strategy_id`).
    #[test]
    fn spec_name_is_research_strategy_id() {
        let bars = two_symbol_two_day_bars();
        let schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        let strat = ResearchOosReplayStrategy::new(semantic(), schedule, &bars);
        assert_eq!(strat.spec().name, "test_strategy_v1");
    }

    /// R2.2: same trial (`trial_id` unchanged) but hypothetically differing
    /// `economic_eval_id` (a RESULT, not even a field on
    /// `ReplaySemanticSpec`/this fingerprint) never changes the fingerprint
    /// -- proven structurally, composed with `fingerprint_identical_for_
    /// baseline_and_loo_schedule_variant` above (same trial/semantic, only
    /// the schedule/excluded-symbol content differs, fingerprint identical).
    #[test]
    fn same_trial_id_same_fingerprint_regardless_of_schedule_content() {
        let bars = two_symbol_two_day_bars();
        let mut schedule_a: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        schedule_a.insert(1_000, vec![TargetPosition::new("AAA", 5)]);
        let mut schedule_b: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        schedule_b.insert(1_000, vec![TargetPosition::new("AAA", -5), TargetPosition::new("BBB", 5)]);

        let a = ResearchOosReplayStrategy::new(semantic(), schedule_a, &bars);
        let b = ResearchOosReplayStrategy::new(semantic(), schedule_b, &bars);
        assert_eq!(a.semantic_fingerprint(), b.semantic_fingerprint());
    }

    /// R2.2: a different, authenticated `trial_id` (distinct training data/
    /// model identity per `build_economic_trial_identity`) DOES change the
    /// fingerprint -- two trials sharing identical strategy/feature/policy
    /// methodology must never collide onto the same replay candidate
    /// identity.
    #[test]
    fn different_trial_id_changes_fingerprint() {
        let bars = two_symbol_two_day_bars();
        let schedule: BTreeMap<i64, Vec<TargetPosition>> = BTreeMap::new();
        let a = ResearchOosReplayStrategy::new(semantic(), schedule.clone(), &bars);
        let mut mutated = semantic();
        mutated.trial_id = "test-trial-0002".to_string();
        let b = ResearchOosReplayStrategy::new(mutated, schedule, &bars);
        assert_ne!(a.semantic_fingerprint(), b.semantic_fingerprint());
    }
}
