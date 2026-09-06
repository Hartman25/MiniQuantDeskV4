//! P9 `BKT-ROBUSTNESS-GAUNTLET-01` — deterministic robustness evidence for a
//! candidate, built on top of `PROMOTION-STRESS-SUITE-AUTHORITY-01`'s real
//! cost-stress scenarios.
//!
//! # Investigation summary (what already exists, what this module adds, what is deferred)
//!
//! The ledger's own required scope for P9 lists eight items. Audited before
//! writing any code:
//!
//! - **2x/3x cost stress** — already real and durable via
//!   [`crate::stress_suite::run_backtest_stress_suite`]
//!   (`PROMOTION-STRESS-SUITE-AUTHORITY-01`). This module does NOT
//!   duplicate that machinery; the stress suite result is a separate,
//!   already-committed authority a caller resolves alongside this one.
//! - **execution-delay stress** — implemented here as [`DelayedStrategy`],
//!   a strategy-layer decorator that buffers and re-emits the wrapped
//!   strategy's own decisions `delay_bars` bars late. Deliberately NOT
//!   implemented by touching `BacktestEngine::resolve_pending_orders_for_batch`'s
//!   `batch_end_ts > p.signal_ts` eligibility rule (the accepted
//!   `BKT-FUTURE-EXECUTION-01` causal-execution invariant) -- that is
//!   exactly the "rewriting accepted causal execution" the wave's hard-stop
//!   list forbids. The wrapper achieves a genuine, deterministic execution
//!   lag entirely at the strategy layer, leaving the engine untouched.
//! - **symbol leave-one-out** — implemented for genuinely multi-symbol
//!   candidates; a single-symbol candidate reports this scenario
//!   `applicable: false` with an honest reason rather than fabricating a
//!   result.
//! - **month/year concentration** — implemented as a pure analysis over the
//!   baseline `BacktestReport.equity_curve` (no re-run). Regime CONTEXT
//!   (not per-bucket regime CONCENTRATION, which would require a separate
//!   per-window classification design beyond this patch's scope) is
//!   reused from the existing [`crate::regime::detect_market_regime`] and
//!   reported alongside for audit context.
//! - **parameter-neighborhood execution** — reuses the existing
//!   [`crate::sweep::run_sweep`] machinery exactly as-is (a small grid
//!   around the candidate's own slippage configuration), rather than
//!   building a second sweep engine.
//! - **shuffled/random-label placebo** — this project deliberately avoids
//!   RNG everywhere (determinism is a core invariant), so a literal random
//!   shuffle is not the right primitive here. Implemented as a deterministic
//!   temporal-offset placebo: the SAME [`DelayedStrategy`] wrapper with a
//!   delay of roughly half the run length, so the wrapped strategy's
//!   decisions are applied against a materially different point in the
//!   timeline than the one that produced them. Per the ledger's explicit
//!   instruction, if this placebo scores as well as or better than the real
//!   signal, that is reported as a genuine finding (`passed: false`) --
//!   never tuned away.
//! - **conservative execution/capacity stress** — implemented here as
//!   [`conservative_capacity_stress_scenario`]: re-runs under the SAME
//!   accepted `BacktestConfig::conservative_defaults()` daily-loss/max-
//!   drawdown ratios `stress_suite::conservative_risk_limits` already uses,
//!   combined with a halved `max_gross_exposure_mult_micros` and any
//!   candidate-declared per-position caps (never a fabricated cap the
//!   candidate didn't already opt into) -- simulating reduced market
//!   capacity/liquidity on top of conservative risk limits.
//! - **DSR/PBO sensitivity** — PROMOTION-STRESS-AUTHORITY-REPAIR-01 wave
//!   correction: an earlier version of this module deferred this item,
//!   reasoning P7A/P7B were themselves still `OPEN`. That was WRONG -- the
//!   ledger records P7A (`RESEARCH-EXECUTION-PRICING-PARITY-01`) and P7B
//!   (`RESEARCH-WEIGHT-TO-SHARE-PARITY-01`) as `ACCEPTED`/`ACCEPTED_LOCALLY,
//!   PUSHED` and FROZEN. Implemented via cross-language orchestration
//!   (`crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario`), which
//!   shells out to `research-py`'s `mqk_research.ml.dsr_pbo_sensitivity_cli`
//!   -- itself only calling the existing, FROZEN
//!   `mqk_research.ml.multiple_testing_judge.build_multiple_testing_judge`
//!   multiple times under different `cscv_target_block_count` values (never
//!   redefining DSR/PBO statistics in Rust). Deliberately NOT included in
//!   [`run_robustness_gauntlet`] itself (a pure, engine-only function) --
//!   it needs subprocess/filesystem I/O the caller must supply trusted
//!   application/config state for (Python executable, `research-py` root,
//!   registry path), exactly like every other Research-registry-touching
//!   seam in this codebase. Callers assembling the complete P9 artifact
//!   call it separately and merge the result via
//!   [`RobustnessGauntletOutput::merge_dsr_pbo_sensitivity`]; until merged,
//!   it remains honestly recorded in [`RobustnessGauntletOutput::deferred`]
//!   (never silently absent) and [`RobustnessGauntletOutput::is_complete`]
//!   reports `false`.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};

use chrono::Datelike;
use mqk_execution::StrategyOutput;
use mqk_strategy::{
    SemanticIdentityBuilder, Strategy, StrategyContext, StrategySpec, SEMANTIC_IDENTITY_SCHEMA_V1,
};
use uuid::Uuid;

use crate::regime::{detect_market_regime, MarketRegimeInput, MarketRegimePolicy};
use crate::sweep::{run_sweep, SweepGrid};
use crate::types::{BacktestBar, BacktestConfig, BacktestReport};
use crate::BacktestEngine;

/// FINAL-P9-ROBUSTNESS-SEMANTICS-01: the P9 contract materially changed --
/// distinct-block-count-required DSR/PBO sensitivity, a genuine shuffled
/// placebo (replacing temporal-offset as the required placebo evidence),
/// month+year+regime concentration (not just month + whole-run regime
/// context), and edge-collapse semantics on execution-delay/leave-one-out/
/// parameter-neighborhood/cost-stress -- so this is a NEW protocol version,
/// never a silent reinterpretation of `bkt_robustness_gauntlet_v1`. Old v1
/// artifacts remain on disk and human/audit-readable, but
/// `load_canonical_robustness_gauntlet`'s own protocol-version check (and
/// therefore canonical promotion) requires this exact v2 string -- a v1
/// artifact can never satisfy final promotion.
pub const ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION: &str = "bkt_robustness_gauntlet_v2";

/// Every scenario name required under [`ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION`]
/// for a P9 artifact to be [`RobustnessGauntletOutput::is_complete`].
/// Deliberately excludes `cost_stress_2x`/`cost_stress_3x` -- those are
/// `PROMOTION-STRESS-SUITE-AUTHORITY-01`'s own required scenarios
/// (`mqk_backtest::REQUIRED_SCENARIO_NAMES`), a separate durable authority
/// this module reuses rather than duplicates (see module docs above).
pub const REQUIRED_ROBUSTNESS_SCENARIO_NAMES: &[&str] = &[
    "execution_delay_stress",
    "symbol_leave_one_out",
    "month_year_regime_concentration",
    "parameter_neighborhood_execution",
    "placebo_temporal_offset",
    "conservative_capacity_stress",
    crate::dsr_pbo_sensitivity::DSR_PBO_SENSITIVITY_SCENARIO_NAME,
    // P7A-P7B-ECONOMIC-REPLAY-STRESS-01: the genuine "conservative P7A/P7B
    // execution/capacity stress" ledger item -- distinct from, and never a
    // substitute for, `conservative_capacity_stress` above (a real but
    // differently-scoped Rust-only stress that was previously mislabeled as
    // satisfying this requirement).
    crate::p7a_p7b_economic_replay_stress::P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME,
    // FINAL-P9-ROBUSTNESS-SEMANTICS-01: the genuine shuffled/random-label
    // placebo -- distinct from, and never a substitute for,
    // `placebo_temporal_offset` above (a real but differently-scoped
    // temporal-delay check that does not satisfy a shuffled-placebo
    // requirement -- delaying a decision still trades its own real score).
    crate::genuine_shuffled_placebo::GENUINE_SHUFFLED_PLACEBO_SCENARIO_NAME,
];

/// The conservative max-drawdown ceiling every re-run scenario is judged
/// against, shared with `stress_suite`'s own pass criterion (computed from
/// `BacktestConfig::conservative_defaults()`, never a hardcoded literal).
fn conservative_max_drawdown_fraction() -> f64 {
    let c = BacktestConfig::conservative_defaults();
    if c.initial_cash_micros > 0 {
        c.max_drawdown_limit_micros as f64 / c.initial_cash_micros as f64
    } else {
        0.0
    }
}

fn max_drawdown_fraction(starting_equity: i64, curve: &[(i64, i64)]) -> f64 {
    let mut hwm = starting_equity;
    let mut max_dd: i64 = 0;
    for &(_, eq) in curve {
        if eq > hwm {
            hwm = eq;
        }
        let dd = hwm.saturating_sub(eq);
        if dd > max_dd {
            max_dd = dd;
        }
    }
    if hwm > 0 {
        max_dd as f64 / hwm as f64
    } else {
        0.0
    }
}

/// Bankruptcy/drawdown pass check shared by every re-run scenario below.
fn clears_conservative_bar(initial_cash: i64, curve: &[(i64, i64)]) -> (bool, String) {
    let final_equity = curve.last().map(|(_, eq)| *eq).unwrap_or(initial_cash);
    if final_equity <= 0 {
        return (
            false,
            format!("bankruptcy: final_equity_micros={final_equity} <= 0"),
        );
    }
    let dd = max_drawdown_fraction(initial_cash, curve);
    let ceiling = conservative_max_drawdown_fraction();
    if dd > ceiling {
        return (
            false,
            format!("max_drawdown_fraction={dd:.6} exceeds conservative ceiling={ceiling:.6}"),
        );
    }
    (true, format!("max_drawdown_fraction={dd:.6}, final_equity_micros={final_equity}"))
}

/// FINAL-P9-ROBUSTNESS-SEMANTICS-01: edge-collapse check shared by every
/// re-run scenario below -- robustness cannot mean only "didn't go
/// bankrupt". When the candidate's BASELINE was genuinely profitable
/// (`baseline_final_equity_micros > initial_cash`), the re-run must also
/// remain profitable (net non-negative return); a re-run that turns a real
/// edge into a net non-positive result fails, even if it clears drawdown/
/// bankruptcy. A baseline that was never profitable to begin with has no
/// edge to collapse -- this check is then vacuously satisfied (that
/// candidate's lack of an edge is already caught by other gates, not this
/// one). Zero net profitability is sufficient to fail; no invented
/// relative-performance percentage threshold.
fn clears_economic_edge(
    baseline_final_equity_micros: i64,
    initial_cash: i64,
    curve: &[(i64, i64)],
) -> (bool, String) {
    if baseline_final_equity_micros <= initial_cash {
        return (true, "baseline was not profitable; edge-collapse check not applicable".to_string());
    }
    let final_equity = curve.last().map(|(_, eq)| *eq).unwrap_or(initial_cash);
    if final_equity <= initial_cash {
        return (
            false,
            format!(
                "economic edge collapsed: baseline was profitable \
                 (final_equity_micros={baseline_final_equity_micros} > \
                 initial_cash_micros={initial_cash}) but this scenario's own \
                 final_equity_micros={final_equity} is not profitable"
            ),
        );
    }
    (true, format!("remains profitable: final_equity_micros={final_equity}"))
}

// ---------------------------------------------------------------------------
// DelayedStrategy — strategy-layer execution-lag / temporal-offset decorator
// ---------------------------------------------------------------------------

/// Wraps any [`Strategy`] and re-emits its own decisions `delay_bars` bars
/// later than it made them. Every decision the inner strategy makes is
/// still driven by that strategy's own real, unmodified `on_bar` call (so
/// its internal state evolves exactly as it would un-wrapped) -- only the
/// OUTPUT is deferred. While the buffer is filling (the first `delay_bars`
/// calls), no targets are emitted (an empty `StrategyOutput`, which the
/// engine treats as "no new intent this bar" -- existing positions are
/// simply carried forward, never force-flattened).
struct DelayedStrategy {
    inner: Box<dyn Strategy>,
    delay_bars: usize,
    buffer: VecDeque<StrategyOutput>,
}

impl DelayedStrategy {
    fn new(inner: Box<dyn Strategy>, delay_bars: usize) -> Self {
        Self {
            inner,
            delay_bars,
            buffer: VecDeque::with_capacity(delay_bars + 1),
        }
    }
}

impl Strategy for DelayedStrategy {
    fn spec(&self) -> StrategySpec {
        self.inner.spec()
    }

    /// STRESS-TRANSFORM-SEMANTIC-IDENTITY-01: execution delay changes the
    /// EFFECTIVE semantics actually executed (decisions are re-emitted
    /// `delay_bars` bars late) -- this must have its own fingerprint,
    /// distinct from the wrapped candidate's, so a delayed run can never
    /// collide on `run_id` with the baseline (or with a different delay) it
    /// shares strategy_name/config/bars/economics/execution_model with.
    /// UNDERLYING CANDIDATE IDENTITY (proving the factory produced the
    /// right baseline strategy) is a separate concern, checked via
    /// `verify_candidate_identity` against `self.inner` BEFORE this wrapper
    /// is ever constructed -- see callers below. Never hashes P&L, fills,
    /// score, or any other result value.
    fn semantic_fingerprint(&self) -> String {
        SemanticIdentityBuilder::new(
            SEMANTIC_IDENTITY_SCHEMA_V1,
            "robustness_delayed_strategy",
            "v1",
        )
        .push_str(&self.inner.semantic_fingerprint())
        .push_i64(self.delay_bars as i64)
        .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let real = self.inner.on_bar(ctx);
        self.buffer.push_back(real);
        if self.buffer.len() > self.delay_bars {
            self.buffer.pop_front().expect("just checked len > delay_bars >= 0")
        } else {
            StrategyOutput::new(Vec::new())
        }
    }
}

// ---------------------------------------------------------------------------
// TimestampBatchDelayedStrategy — W06-P9-REPLAY-IDENTITY-AND-BATCH-DELAY-REPAIR-01
// (R2.3/R2.4): logical timestamp-BATCH execution-lag decorator for the
// Research replay path.
// ---------------------------------------------------------------------------

/// Like [`DelayedStrategy`], but delays by whole distinct `end_ts`
/// CROSS-SECTIONAL BATCHES rather than physical `on_bar` CALL count.
///
/// `DelayedStrategy` is correct for a strategy that makes one independent
/// decision per physical `on_bar` call. [`crate::research_replay_strategy::ResearchOosReplayStrategy`]
/// is not that: it emits ONE complete cross-sectional vector on the LAST
/// physical row of a same-`end_ts` batch (every earlier row of that batch is
/// an empty "not yet" output). Wrapping it in `DelayedStrategy(delay_bars=1)`
/// would therefore re-emit that complete vector on the FIRST physical row of
/// the NEXT timestamp batch -- before every symbol at that later timestamp
/// has updated its own current mark, and dependent on that batch's own
/// physical row count/order.
///
/// This type instead tracks batch completion the same way
/// `ResearchOosReplayStrategy` does (an `expected_calls` map derived from
/// the EXACT bars slice this instance will be run against, never a
/// caller-declared count): it always drives the inner strategy's own
/// `on_bar` on every physical call (so the inner strategy's internal state
/// evolves exactly as it would unwrapped), but only treats a call as a
/// "batch decision" when it is that batch's OWN final physical row, and only
/// ever emits a buffered batch's decision on the FINAL physical row of a
/// LATER batch, once `delay_batches` complete batches have elapsed. Never
/// touches `DelayedStrategy` itself -- existing builtin-strategy P9 delay
/// semantics are completely unchanged.
struct TimestampBatchDelayedStrategy {
    inner: Box<dyn Strategy>,
    delay_batches: usize,
    /// `end_ts` -> expected physical row count, derived from the EXACT bars
    /// slice this instance will be run against (mirrors
    /// `ResearchOosReplayStrategy::new`'s own technique).
    expected_calls: HashMap<i64, usize>,
    current_end_ts: Option<i64>,
    calls_seen_for_current: usize,
    /// FIFO of completed-batch outputs awaiting their delayed emission slot,
    /// oldest first. Pushed exactly once per distinct `end_ts`, on that
    /// batch's own final physical row; popped (and returned) on the final
    /// physical row of a later batch once `delay_batches` batches have
    /// accumulated.
    buffer: VecDeque<StrategyOutput>,
}

impl TimestampBatchDelayedStrategy {
    fn new(inner: Box<dyn Strategy>, delay_batches: usize, bars: &[BacktestBar]) -> Self {
        let mut expected_calls: HashMap<i64, usize> = HashMap::new();
        for bar in bars {
            *expected_calls.entry(bar.end_ts).or_insert(0) += 1;
        }
        Self {
            inner,
            delay_batches,
            expected_calls,
            current_end_ts: None,
            calls_seen_for_current: 0,
            buffer: VecDeque::with_capacity(delay_batches + 1),
        }
    }
}

impl Strategy for TimestampBatchDelayedStrategy {
    fn spec(&self) -> StrategySpec {
        self.inner.spec()
    }

    /// See `DelayedStrategy::semantic_fingerprint` -- same
    /// STRESS-TRANSFORM-SEMANTIC-IDENTITY-01 rationale (an effective-semantics
    /// fingerprint distinct from the wrapped candidate's, never hashing any
    /// result value), using a distinct schema name so this wrapper's
    /// fingerprint can never collide with `DelayedStrategy`'s for the same
    /// inner candidate and delay count.
    fn semantic_fingerprint(&self) -> String {
        SemanticIdentityBuilder::new(
            SEMANTIC_IDENTITY_SCHEMA_V1,
            "robustness_timestamp_batch_delayed_strategy",
            "v1",
        )
        .push_str(&self.inner.semantic_fingerprint())
        .push_i64(self.delay_batches as i64)
        .finish()
    }

    fn on_bar(&mut self, ctx: &StrategyContext) -> StrategyOutput {
        let real = self.inner.on_bar(ctx);

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
                "TimestampBatchDelayedStrategy: end_ts={end_ts} was never present in the bars \
                 slice this instance was constructed from -- constructed-vs-run bars mismatch"
            )
        });
        if self.calls_seen_for_current < expected {
            // Not yet this batch's final physical row -- never emit a
            // decision early, whatever `real` was.
            return StrategyOutput::new(Vec::new());
        }

        // This IS the batch's final physical row: `real` is that batch's
        // complete decision (possibly an intentionally empty one -- "no
        // scheduled decision this timestamp" is itself a real decision, not
        // a partial-batch placeholder). Enqueue it, then emit whatever
        // batch has now aged past `delay_batches`, if any.
        self.buffer.push_back(real);
        if self.buffer.len() > self.delay_batches {
            self.buffer.pop_front().expect("just checked len > delay_batches >= 0")
        } else {
            StrategyOutput::new(Vec::new())
        }
    }
}

// ---------------------------------------------------------------------------
// Output types
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq)]
pub struct RobustnessScenarioOutcome {
    pub name: String,
    /// `false` when this scenario's real-world precondition is not met by
    /// this candidate (e.g. leave-one-out on a single-symbol run) -- an
    /// honest "not applicable", never a fabricated pass or fail.
    pub applicable: bool,
    /// Meaningless when `applicable == false`.
    pub passed: bool,
    pub reason: Option<String>,
    pub detail: String,
    /// PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01: the exact Research
    /// `trial_id` this scenario's evidence was computed against, when the
    /// scenario is Research-registry-anchored (currently only
    /// `dsr_pbo_sensitivity` -- every pure-engine scenario leaves this
    /// `None`). Durably carried through the artifact so a later promotion
    /// decision can prove the SAME trial produced both this P9 evidence and
    /// the P7C/OOS evidence, never merely that both share a `strategy_id`.
    pub research_trial_id: Option<String>,
    /// FINAL-P7A-P7B-REPLAY-AUTHORITY-01 Section G: structured, TYPED replay
    /// evidence for a Research-registry-anchored scenario (currently only
    /// `p7a_p7b_economic_replay_stress`) -- the exact JSON object the
    /// underlying Python CLI reported, carrying every durable identity/hash
    /// field (baseline/stressed `economic_eval_id`, artifact SHA-256s,
    /// input SHA-256s, bars-provenance hash, stress-spec identity, actual
    /// pass/fail metrics). Deliberately NOT reduced to a human-readable
    /// `detail` string -- promotion-grade replay proof must be machine-
    /// verifiable. `None` for every pure-engine scenario and for outcomes
    /// where no structured evidence was ever produced (e.g. a spawn
    /// failure).
    pub evidence: Option<serde_json::Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeferredScenario {
    pub name: String,
    pub reason: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RobustnessGauntletOutput {
    pub protocol_version: String,
    pub run_id: Uuid,
    pub config_id: Uuid,
    pub strategy_name: String,
    pub scenarios: Vec<RobustnessScenarioOutcome>,
    pub deferred: Vec<DeferredScenario>,
}

impl RobustnessGauntletOutput {
    /// True iff every APPLICABLE scenario passed, and at least one scenario
    /// was applicable. A candidate for which every scenario was
    /// inapplicable (degenerate/empty run) is never vacuously "robust".
    pub fn all_applicable_passed(&self) -> bool {
        let applicable: Vec<&RobustnessScenarioOutcome> =
            self.scenarios.iter().filter(|s| s.applicable).collect();
        !applicable.is_empty() && applicable.iter().all(|s| s.passed)
    }

    /// True iff every scenario REQUIRED under [`ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION`]
    /// ([`REQUIRED_ROBUSTNESS_SCENARIO_NAMES`]) is present in `scenarios` by
    /// name AND `deferred` is empty.
    ///
    /// Distinct from [`Self::all_applicable_passed`]: completeness is about
    /// EVIDENCE COVERAGE (every required slice was genuinely evaluated one
    /// way or another -- pass, fail, or an honest `applicable: false` -- not
    /// silently missing or still deferred), never about whether the
    /// candidate's own numbers were good. A candidate can be complete and
    /// still fail (`all_applicable_passed() == false`); it can never be
    /// incomplete and reported as a promotion-grade pass -- callers must
    /// check both.
    pub fn is_complete(&self) -> bool {
        if !self.deferred.is_empty() {
            return false;
        }
        let present: std::collections::BTreeSet<&str> =
            self.scenarios.iter().map(|s| s.name.as_str()).collect();
        REQUIRED_ROBUSTNESS_SCENARIO_NAMES
            .iter()
            .all(|required| present.contains(required))
    }

    /// Merge a separately-computed
    /// [`crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario`] result
    /// (or, P7A-P7B-ECONOMIC-REPLAY-STRESS-01, a
    /// [`crate::p7a_p7b_economic_replay_stress::p7a_p7b_economic_replay_stress_scenario`]
    /// result) into this output: appends it to `scenarios` and removes the
    /// matching entry from `deferred` (a no-op if none exists), so
    /// [`Self::is_complete`] can become `true` once every required scenario
    /// has genuinely been evaluated. Genuinely name-agnostic -- matches
    /// purely on `outcome.name` against whatever `deferred` entry shares it;
    /// only ever call this with an `outcome` whose `name` is one of the
    /// scenario-name constants this module or `dsr_pbo_sensitivity` /
    /// `p7a_p7b_economic_replay_stress` export.
    pub fn merge_dsr_pbo_sensitivity(mut self, outcome: RobustnessScenarioOutcome) -> Self {
        self.deferred.retain(|d| d.name != outcome.name);
        self.scenarios.push(outcome);
        self
    }
}

// ---------------------------------------------------------------------------
// Scenario implementations
// ---------------------------------------------------------------------------

/// STRESS-TRANSFORM-SEMANTIC-IDENTITY-01: UNDERLYING CANDIDATE IDENTITY --
/// proves `strategy` (a fresh, UNWRAPPED factory instance) is the same
/// candidate the baseline was computed from, before it is ever added to an
/// engine. This is distinct from EFFECTIVE STRATEGY IDENTITY (what
/// `run_id` actually binds to once a stress transform is applied) --
/// callers that wrap the verified instance in a decorator like
/// `DelayedStrategy` must call this against the UNWRAPPED instance and
/// must never compare the wrapper's own (intentionally different)
/// fingerprint against `expected_semantic_fingerprint`. A mismatch fails
/// closed and is never comparable-by-result -- caught before any bar is
/// ever processed.
fn verify_candidate_identity(
    strategy: &dyn Strategy,
    expected_semantic_fingerprint: &str,
) -> Result<(), String> {
    let actual = strategy.semantic_fingerprint();
    if actual != expected_semantic_fingerprint {
        return Err(format!(
            "strategy semantic fingerprint mismatch: baseline candidate expects \
             {expected_semantic_fingerprint:?}, robustness-gauntlet factory produced {actual:?} \
             -- this strategy instance does not match the baseline candidate this evidence \
             purports to test"
        ));
    }
    Ok(())
}

/// Adds `strategy` to a fresh engine and runs it -- no identity check of its
/// own. Used directly by scenarios that execute the candidate unwrapped
/// (via [`run_with_strategy`], which checks first) and, after a separate
/// [`verify_candidate_identity`] call against the unwrapped inner instance,
/// by scenarios that execute a stress-transformed wrapper (whose own
/// fingerprint legitimately differs from the baseline's).
fn run_engine(
    config: BacktestConfig,
    bars: &[BacktestBar],
    strategy: Box<dyn Strategy>,
) -> Result<BacktestReport, String> {
    let mut engine = BacktestEngine::new(config);
    engine
        .add_strategy(strategy)
        .map_err(|e| format!("add_strategy failed: {e}"))?;
    engine.run(bars).map_err(|e| format!("engine.run failed: {e}"))
}

/// The choke point every scenario that executes the candidate UNWRAPPED
/// funnels through -- validates the exact `Box<dyn Strategy>` about to be
/// executed carries the same semantic fingerprint as the baseline candidate
/// this gauntlet purports to test, before that instance is ever added to an
/// engine. Never call this with a stress-wrapped decorator (e.g.
/// `DelayedStrategy`) as `strategy` -- its effective fingerprint legitimately
/// differs from the baseline; verify the unwrapped inner instance separately
/// via [`verify_candidate_identity`] before wrapping it instead.
fn run_with_strategy(
    config: BacktestConfig,
    bars: &[BacktestBar],
    strategy: Box<dyn Strategy>,
    expected_semantic_fingerprint: &str,
) -> Result<BacktestReport, String> {
    verify_candidate_identity(strategy.as_ref(), expected_semantic_fingerprint)?;
    run_engine(config, bars, strategy)
}

fn execution_delay_scenario(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "execution_delay_stress".to_string();
    let initial_cash = base_config.initial_cash_micros;
    let baseline_final = baseline.equity_curve.last().map(|(_, eq)| *eq).unwrap_or(initial_cash);
    let inner = make_strategy();
    if let Err(e) = verify_candidate_identity(inner.as_ref(), &baseline.strategy_semantic_fingerprint) {
        return RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
            evidence: None,
        };
    }
    let delayed = DelayedStrategy::new(inner, 1);
    match run_engine(base_config.clone(), bars, Box::new(delayed)) {
        Ok(report) => {
            let (bar_passed, bar_detail) = clears_conservative_bar(initial_cash, &report.equity_curve);
            let (edge_passed, edge_detail) =
                clears_economic_edge(baseline_final, initial_cash, &report.equity_curve);
            let passed = bar_passed && edge_passed;
            RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed,
                reason: if passed {
                    None
                } else if !bar_passed {
                    Some(bar_detail.clone())
                } else {
                    Some(edge_detail.clone())
                },
                detail: format!("{bar_detail}; {edge_detail}"),
                research_trial_id: None,
                evidence: None,
            }
        }
        Err(e) => RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
            evidence: None,
        },
    }
}

/// R2.4: `execution_delay_stress` for the Research replay path, using
/// [`TimestampBatchDelayedStrategy`] (one whole distinct-timestamp batch of
/// logical delay) instead of [`DelayedStrategy`]'s physical-row count.
/// Otherwise byte-for-byte identical judgment logic to
/// [`execution_delay_scenario`].
fn execution_delay_scenario_batch_aware(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "execution_delay_stress".to_string();
    let initial_cash = base_config.initial_cash_micros;
    let baseline_final = baseline.equity_curve.last().map(|(_, eq)| *eq).unwrap_or(initial_cash);
    let inner = make_strategy();
    if let Err(e) = verify_candidate_identity(inner.as_ref(), &baseline.strategy_semantic_fingerprint) {
        return RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
            evidence: None,
        };
    }
    let delayed = TimestampBatchDelayedStrategy::new(inner, 1, bars);
    match run_engine(base_config.clone(), bars, Box::new(delayed)) {
        Ok(report) => {
            let (bar_passed, bar_detail) = clears_conservative_bar(initial_cash, &report.equity_curve);
            let (edge_passed, edge_detail) =
                clears_economic_edge(baseline_final, initial_cash, &report.equity_curve);
            let passed = bar_passed && edge_passed;
            RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed,
                reason: if passed {
                    None
                } else if !bar_passed {
                    Some(bar_detail.clone())
                } else {
                    Some(edge_detail.clone())
                },
                detail: format!("{bar_detail}; {edge_detail}"),
                research_trial_id: None,
                evidence: None,
            }
        }
        Err(e) => RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
            evidence: None,
        },
    }
}

fn symbol_leave_one_out_scenario(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    // W06-P9-RUST-REPLAY-STRATEGY-01 (B5): zero-behavior-change delegation
    // -- the bar argument is ignored, exactly reproducing this function's
    // own pre-existing behavior (every prior caller's `make_strategy` never
    // depended on the filtered bars either).
    symbol_leave_one_out_scenario_with_factory(baseline, base_config, bars, &|_filtered| make_strategy())
}

/// W06-P9-RUST-REPLAY-STRATEGY-01 (B5): bar-aware counterpart used by
/// [`run_robustness_gauntlet_with_symbol_loo_factory`]. Identical logic to
/// [`symbol_leave_one_out_scenario`], except the strategy for each excluded-
/// symbol rerun is built from `make_strategy_for_bars(&filtered)` -- letting
/// a caller (e.g. `ResearchOosReplayStrategy`) construct a strategy instance
/// whose own precomputed per-symbol schedule and expected-row-count table
/// are correct for THAT filtered universe, rather than reusing whatever a
/// single no-argument factory produced for the baseline's full universe.
fn symbol_leave_one_out_scenario_with_factory(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy_for_bars: &impl Fn(&[BacktestBar]) -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "symbol_leave_one_out".to_string();
    let symbols: BTreeSet<&str> = bars.iter().map(|b| b.symbol.as_str()).collect();
    if symbols.len() < 2 {
        return RobustnessScenarioOutcome {
            name,
            applicable: false,
            passed: false,
            reason: Some(
                "single-symbol backtest; leave-one-out requires 2+ distinct symbols".to_string(),
            ),
            detail: format!("distinct_symbols={}", symbols.len()),
            research_trial_id: None,
                evidence: None,
        };
    }

    let initial_cash = base_config.initial_cash_micros;
    let baseline_final = baseline.equity_curve.last().map(|(_, eq)| *eq).unwrap_or(initial_cash);
    let mut worst: Option<(String, f64)> = None;
    for symbol in &symbols {
        let filtered: Vec<BacktestBar> =
            bars.iter().filter(|b| b.symbol != *symbol).cloned().collect();
        let report = match run_with_strategy(
            base_config.clone(),
            &filtered,
            make_strategy_for_bars(&filtered),
            &baseline.strategy_semantic_fingerprint,
        ) {
            Ok(r) => r,
            Err(e) => {
                return RobustnessScenarioOutcome {
                    name,
                    applicable: true,
                    passed: false,
                    reason: Some(format!("excluding {symbol}: {e}")),
                    detail: format!("excluding {symbol}: {e}"),
                    research_trial_id: None,
                    evidence: None,
                }
            }
        };
        let dd = max_drawdown_fraction(initial_cash, &report.equity_curve);
        if worst.as_ref().map(|(_, d)| dd > *d).unwrap_or(true) {
            worst = Some((symbol.to_string(), dd));
        }
        let (bar_passed, _) = clears_conservative_bar(initial_cash, &report.equity_curve);
        if !bar_passed {
            return RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(format!(
                    "excluding symbol {symbol} breaches the conservative bar (max_drawdown_fraction={dd:.6})"
                )),
                detail: format!("worst excluded symbol so far: {symbol} dd={dd:.6}"),
                research_trial_id: None,
                evidence: None,
            };
        }
        // FINAL-P9-ROBUSTNESS-SEMANTICS-01: excluding a symbol that removes
        // the candidate's positive result must fail even if the remaining
        // result is merely flat (zero net profitability is sufficient).
        let (edge_passed, edge_detail) =
            clears_economic_edge(baseline_final, initial_cash, &report.equity_curve);
        if !edge_passed {
            return RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(format!("excluding symbol {symbol}: {edge_detail}")),
                detail: format!("excluding symbol {symbol}: {edge_detail}"),
                research_trial_id: None,
                evidence: None,
            };
        }
    }

    let (worst_symbol, worst_dd) = worst.expect("symbols.len() >= 2 checked above");
    RobustnessScenarioOutcome {
        name,
        applicable: true,
        passed: true,
        reason: None,
        detail: format!(
            "{} symbols tested; worst excluded symbol={worst_symbol} max_drawdown_fraction={worst_dd:.6}",
            symbols.len()
        ),
        research_trial_id: None,
                evidence: None,
    }
}

/// The concentration ceiling every bucketed dimension (month/year/regime) is
/// judged against -- a single bucket contributing more than half of a run's
/// total positive gain is fragile, regardless of which dimension buckets it.
const CONCENTRATION_CEILING_FRACTION: f64 = 0.5;

/// One bucketed dimension's concentration result -- `None` when this
/// dimension has fewer than 2 distinct buckets with data (not a meaningful
/// signal, never a fabricated pass/fail).
struct ConcentrationDimension {
    concentration_fraction: f64,
    worst_bucket_desc: String,
}

fn concentration_dimension<K: Ord + Clone + std::fmt::Debug>(
    gain_by_bucket: &BTreeMap<K, i64>,
) -> Option<ConcentrationDimension> {
    if gain_by_bucket.len() < 2 {
        return None;
    }
    let total_positive: i64 = gain_by_bucket.values().filter(|&&v| v > 0).sum();
    let worst = gain_by_bucket.iter().filter(|(_, &v)| v > 0).max_by_key(|(_, &v)| v);
    let concentration_fraction = match (total_positive, worst) {
        (tp, Some((_, &max_gain))) if tp > 0 => max_gain as f64 / tp as f64,
        _ => 0.0,
    };
    let worst_bucket_desc = worst
        .map(|(k, &g)| format!("{k:?} (gain_micros={g})"))
        .unwrap_or_else(|| "none".to_string());
    Some(ConcentrationDimension {
        concentration_fraction,
        worst_bucket_desc,
    })
}

/// FINAL-P9-ROBUSTNESS-SEMANTICS-01: month + year + regime concentration --
/// three INDEPENDENT bucketed-concentration checks (never just "whole-run
/// regime context"). Each dimension is judged against the same
/// [`CONCENTRATION_CEILING_FRACTION`]; the scenario is `applicable` iff at
/// least one dimension has 2+ distinct buckets of data, and `passed` iff
/// EVERY applicable dimension clears the ceiling -- a candidate failing any
/// one dimension (e.g. genuinely regime-concentrated profit while month/year
/// happen to look diversified) fails the whole scenario.
///
/// REGIME buckets are built by classifying EACH calendar month's own bars
/// via the existing, accepted [`detect_market_regime`] (reused, never a
/// second classifier) and accumulating that month's gain under its
/// classified [`MarketRegimeKind`] -- genuine per-window regime
/// concentration, not the whole run's own aggregate classification.
fn month_year_regime_concentration_scenario(
    baseline: &BacktestReport,
    bars: &[BacktestBar],
) -> RobustnessScenarioOutcome {
    let name = "month_year_regime_concentration".to_string();

    let mut monthly_gain: BTreeMap<(i32, u32), i64> = BTreeMap::new();
    let mut prev_equity: Option<i64> = None;
    for &(ts, eq) in &baseline.equity_curve {
        if let Some(prev) = prev_equity {
            if let Some(dt) = chrono::DateTime::from_timestamp(ts, 0) {
                let key = (dt.year(), dt.month());
                *monthly_gain.entry(key).or_insert(0) += eq - prev;
            }
        }
        prev_equity = Some(eq);
    }

    let mut yearly_gain: BTreeMap<i32, i64> = BTreeMap::new();
    for (&(y, _), &g) in &monthly_gain {
        *yearly_gain.entry(y).or_insert(0) += g;
    }

    let policy = MarketRegimePolicy::conservative_defaults();
    let mut regime_gain: BTreeMap<&'static str, i64> = BTreeMap::new();
    for (&(y, m), &gain) in &monthly_gain {
        let month_bars: Vec<BacktestBar> = bars
            .iter()
            .filter(|b| {
                chrono::DateTime::from_timestamp(b.end_ts, 0)
                    .map(|dt| dt.year() == y && dt.month() == m)
                    .unwrap_or(false)
            })
            .cloned()
            .collect();
        let input = MarketRegimeInput::from_bars(month_bars, None::<String>, None::<String>);
        let classification = detect_market_regime(&input, &policy);
        *regime_gain.entry(classification.kind.code()).or_insert(0) += gain;
    }

    let month_dim = concentration_dimension(&monthly_gain);
    let year_dim = concentration_dimension(&yearly_gain);
    let regime_dim = concentration_dimension(&regime_gain);

    if month_dim.is_none() && year_dim.is_none() && regime_dim.is_none() {
        return RobustnessScenarioOutcome {
            name,
            applicable: false,
            passed: false,
            reason: Some(format!(
                "run spans only {} distinct calendar month(s) and {} distinct regime bucket(s); \
                 concentration analysis requires 2+ distinct buckets in at least one dimension",
                monthly_gain.len(),
                regime_gain.len()
            )),
            detail: format!("distinct_months={}, distinct_regimes={}", monthly_gain.len(), regime_gain.len()),
            research_trial_id: None,
            evidence: None,
        };
    }

    let mut failures: Vec<String> = Vec::new();
    for (label, dim) in [("month", &month_dim), ("year", &year_dim), ("regime", &regime_dim)] {
        if let Some(d) = dim {
            if d.concentration_fraction > CONCENTRATION_CEILING_FRACTION {
                failures.push(format!(
                    "{label}: worst bucket contributed {:.4} of total positive gain \
                     (> {CONCENTRATION_CEILING_FRACTION} ceiling): {}",
                    d.concentration_fraction, d.worst_bucket_desc
                ));
            }
        }
    }
    let passed = failures.is_empty();

    RobustnessScenarioOutcome {
        name,
        applicable: true,
        passed,
        reason: if passed { None } else { Some(failures.join("; ")) },
        detail: format!(
            "distinct_months={}, month_concentration={:.4?}, distinct_years={}, \
             year_concentration={:.4?}, distinct_regimes={}, regime_concentration={:.4?}",
            monthly_gain.len(),
            month_dim.as_ref().map(|d| d.concentration_fraction),
            yearly_gain.len(),
            year_dim.as_ref().map(|d| d.concentration_fraction),
            regime_gain.len(),
            regime_dim.as_ref().map(|d| d.concentration_fraction),
        ),
        research_trial_id: None,
        evidence: None,
    }
}

fn parameter_neighborhood_scenario(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "parameter_neighborhood_execution".to_string();
    let initial_cash = base_config.initial_cash_micros;
    let baseline_final = baseline.equity_curve.last().map(|(_, eq)| *eq).unwrap_or(initial_cash);
    let baseline_profitable = baseline_final > initial_cash;
    let base_slippage = base_config.stress.slippage_bps;
    let grid = SweepGrid {
        target_qty: vec![base_config.sizing.target_qty.max(1)],
        slippage_bps: vec![base_slippage, base_slippage + 5, base_slippage + 10],
        volatility_mult_bps: Vec::new(),
        max_target_qty: vec![base_config.sizing.max_target_qty],
        max_position_notional_usd: vec![base_config.sizing.max_position_notional_usd],
        // Not swept here -- empty carries base_config's own cap through
        // unchanged, matching volatility_mult_bps's own default-passthrough.
        max_participation_rate_bps: Vec::new(),
    };

    // STRESS-ROBUSTNESS-SEMANTIC-BINDING-01: `run_sweep` calls this factory
    // fresh for every neighborhood point (the sweep parameter-neighborhood
    // path must not become a bypass for the semantic-identity check every
    // other scenario enforces) -- returning `None` fails the point closed
    // via `SweepError::RunFailed`, which the `Err(e)` arm below already
    // converts into a failed, never-passing outcome. `mismatch_detail`
    // captures the specific, attributable identity-mismatch reason (rather
    // than surfacing `SweepError`'s generic "returned None" text) so this
    // scenario's failure is as auditable as every other identity-bound
    // scenario in this module.
    let expected_fp = baseline.strategy_semantic_fingerprint.as_str();
    let mismatch_detail: std::cell::Cell<Option<String>> = std::cell::Cell::new(None);
    let rows = match run_sweep(bars, base_config, &grid, |_pt| {
        let s = make_strategy();
        let actual = s.semantic_fingerprint();
        if actual == expected_fp {
            Some(s)
        } else {
            mismatch_detail.set(Some(format!(
                "strategy semantic fingerprint mismatch: baseline candidate expects \
                 {expected_fp:?}, parameter-neighborhood factory produced {actual:?} -- this \
                 strategy instance does not match the baseline candidate this evidence \
                 purports to test"
            )));
            None
        }
    }) {
        Ok(r) => r,
        Err(e) => {
            let reason = mismatch_detail.take().unwrap_or_else(|| e.to_string());
            return RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed: false,
                reason: Some(reason.clone()),
                detail: reason,
                research_trial_id: None,
                evidence: None,
            }
        }
    };

    let ceiling = conservative_max_drawdown_fraction() * 100.0; // rows report pct, not fraction
    let worst_row = rows
        .iter()
        .max_by(|a, b| a.max_drawdown_pct.partial_cmp(&b.max_drawdown_pct).unwrap());
    let bar_passed = rows.iter().all(|r| r.max_drawdown_pct <= ceiling && !r.halted);
    // FINAL-P9-ROBUSTNESS-SEMANTICS-01: a neighboring parameter point that
    // becomes economically non-profitable fails, when the baseline itself
    // was genuinely profitable -- zero net return is sufficient to fail.
    let collapsed_row = if baseline_profitable {
        rows.iter().find(|r| r.total_return_pct <= 0.0)
    } else {
        None
    };
    let passed = bar_passed && collapsed_row.is_none();

    RobustnessScenarioOutcome {
        name,
        applicable: true,
        passed,
        reason: if passed {
            None
        } else if !bar_passed {
            Some(format!(
                "at least one neighboring parameter point breached the conservative drawdown \
                 ceiling ({ceiling:.4}%) or halted"
            ))
        } else {
            Some(format!(
                "economic edge collapsed at a neighboring parameter point (slippage_bps={:?}): \
                 total_return_pct={:.4} is not profitable, though baseline was",
                collapsed_row.map(|r| r.slippage_bps),
                collapsed_row.map(|r| r.total_return_pct).unwrap_or(0.0)
            ))
        },
        detail: format!(
            "{} neighborhood points tested; worst max_drawdown_pct={:.4}%",
            rows.len(),
            worst_row.map(|r| r.max_drawdown_pct).unwrap_or(0.0)
        ),
        research_trial_id: None,
        evidence: None,
    }
}

fn placebo_temporal_offset_scenario(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "placebo_temporal_offset".to_string();
    let initial_cash = base_config.initial_cash_micros;
    let delay_bars = (bars.len() / 2).max(1);

    let baseline_final = baseline
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial_cash);

    let inner = make_strategy();
    if let Err(e) = verify_candidate_identity(inner.as_ref(), &baseline.strategy_semantic_fingerprint) {
        return RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
            evidence: None,
        };
    }
    let delayed = DelayedStrategy::new(inner, delay_bars);

    match run_engine(base_config.clone(), bars, Box::new(delayed)) {
        Ok(report) => {
            let placebo_final = report
                .equity_curve
                .last()
                .map(|(_, eq)| *eq)
                .unwrap_or(initial_cash);
            // Per the ledger's explicit hard stop: if the placebo performs
            // as well as or better than the real signal, that is a genuine
            // finding to report -- never tuned away.
            let passed = placebo_final < baseline_final;
            RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed,
                reason: if passed {
                    None
                } else {
                    Some(format!(
                        "placebo (delay={delay_bars} bars) final_equity_micros={placebo_final} \
                         is NOT worse than the real signal's baseline_final_equity_micros={baseline_final} \
                         -- this candidate's real signal may not be distinguishable from a \
                         temporally-decorrelated one; reported as found, not adjusted away"
                    ))
                },
                detail: format!(
                    "delay_bars={delay_bars}, baseline_final_equity_micros={baseline_final}, \
                     placebo_final_equity_micros={placebo_final}"
                ),
                research_trial_id: None,
                evidence: None,
            }
        }
        Err(e) => RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
                evidence: None,
        },
    }
}

/// R2.4: `placebo_temporal_offset` for the Research replay path, using
/// [`TimestampBatchDelayedStrategy`] (a logical whole-batch offset, derived
/// from the count of DISTINCT `end_ts` batches rather than physical bar
/// rows) instead of [`DelayedStrategy`]'s physical-row count. Preserves the
/// canonical scenario intent of a roughly half-run temporal displacement --
/// only the unit of measurement changes (batches, not rows). Otherwise
/// byte-for-byte identical judgment logic to [`placebo_temporal_offset_scenario`].
fn placebo_temporal_offset_scenario_batch_aware(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "placebo_temporal_offset".to_string();
    let initial_cash = base_config.initial_cash_micros;
    let distinct_batches = bars.iter().map(|b| b.end_ts).collect::<BTreeSet<_>>().len();
    let delay_batches = (distinct_batches / 2).max(1);

    let baseline_final = baseline
        .equity_curve
        .last()
        .map(|(_, eq)| *eq)
        .unwrap_or(initial_cash);

    let inner = make_strategy();
    if let Err(e) = verify_candidate_identity(inner.as_ref(), &baseline.strategy_semantic_fingerprint) {
        return RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
            evidence: None,
        };
    }
    let delayed = TimestampBatchDelayedStrategy::new(inner, delay_batches, bars);

    match run_engine(base_config.clone(), bars, Box::new(delayed)) {
        Ok(report) => {
            let placebo_final = report
                .equity_curve
                .last()
                .map(|(_, eq)| *eq)
                .unwrap_or(initial_cash);
            // Per the ledger's explicit hard stop: if the placebo performs
            // as well as or better than the real signal, that is a genuine
            // finding to report -- never tuned away.
            let passed = placebo_final < baseline_final;
            RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed,
                reason: if passed {
                    None
                } else {
                    Some(format!(
                        "placebo (delay={delay_batches} timestamp batches) \
                         final_equity_micros={placebo_final} is NOT worse than the real signal's \
                         baseline_final_equity_micros={baseline_final} -- this candidate's real \
                         signal may not be distinguishable from a temporally-decorrelated one; \
                         reported as found, not adjusted away"
                    ))
                },
                detail: format!(
                    "delay_batches={delay_batches}, baseline_final_equity_micros={baseline_final}, \
                     placebo_final_equity_micros={placebo_final}"
                ),
                research_trial_id: None,
                evidence: None,
            }
        }
        Err(e) => RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
            evidence: None,
        },
    }
}

/// Build the `conservative_capacity_stress` adversarial config: the SAME
/// accepted conservative daily-loss/max-drawdown ratios
/// `stress_suite::conservative_risk_limits_config` uses, combined with a
/// halved `max_gross_exposure_mult_micros` and any candidate-declared
/// per-position caps -- reduced market capacity/liquidity on top of
/// conservative risk limits. Never fabricates a per-position cap the
/// candidate didn't already opt into (`None` stays `None`).
fn conservative_capacity_config(base: &BacktestConfig) -> BacktestConfig {
    let conservative = BacktestConfig::conservative_defaults();
    let daily_loss_fraction = if conservative.initial_cash_micros > 0 {
        conservative.daily_loss_limit_micros as f64 / conservative.initial_cash_micros as f64
    } else {
        0.0
    };
    let mut cfg = base.clone();
    cfg.daily_loss_limit_micros = (base.initial_cash_micros as f64 * daily_loss_fraction) as i64;
    cfg.max_drawdown_limit_micros =
        (base.initial_cash_micros as f64 * conservative_max_drawdown_fraction()) as i64;
    cfg.max_gross_exposure_mult_micros = (base.max_gross_exposure_mult_micros / 2).max(1);
    cfg.sizing.max_target_qty = base.sizing.max_target_qty.map(|q| (q / 2).max(1));
    cfg.sizing.max_position_notional_usd =
        base.sizing.max_position_notional_usd.map(|n| (n / 2).max(1));
    cfg
}

fn conservative_capacity_stress_scenario(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: &impl Fn() -> Box<dyn Strategy>,
) -> RobustnessScenarioOutcome {
    let name = "conservative_capacity_stress".to_string();
    let initial_cash = base_config.initial_cash_micros;
    let cfg = conservative_capacity_config(base_config);
    match run_with_strategy(cfg, bars, make_strategy(), &baseline.strategy_semantic_fingerprint) {
        Ok(report) => {
            let (passed, detail) = clears_conservative_bar(initial_cash, &report.equity_curve);
            RobustnessScenarioOutcome {
                name,
                applicable: true,
                passed,
                reason: if passed { None } else { Some(detail.clone()) },
                detail,
                research_trial_id: None,
                evidence: None,
            }
        }
        Err(e) => RobustnessScenarioOutcome {
            name,
            applicable: true,
            passed: false,
            reason: Some(e.clone()),
            detail: e,
            research_trial_id: None,
                evidence: None,
        },
    }
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

/// Run the real, deterministic robustness gauntlet for `baseline`. See the
/// module docs for exactly which of P9's eight scoped items are
/// implemented here versus honestly deferred.
pub fn run_robustness_gauntlet(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: impl Fn() -> Box<dyn Strategy>,
) -> RobustnessGauntletOutput {
    let scenarios = vec![
        execution_delay_scenario(baseline, base_config, bars, &make_strategy),
        symbol_leave_one_out_scenario(baseline, base_config, bars, &make_strategy),
        month_year_regime_concentration_scenario(baseline, bars),
        parameter_neighborhood_scenario(baseline, base_config, bars, &make_strategy),
        placebo_temporal_offset_scenario(baseline, base_config, bars, &make_strategy),
        conservative_capacity_stress_scenario(baseline, base_config, bars, &make_strategy),
    ];

    let deferred = vec![
        DeferredScenario {
            name: crate::dsr_pbo_sensitivity::DSR_PBO_SENSITIVITY_SCENARIO_NAME.to_string(),
            reason: "requires subprocess/filesystem I/O (Python executable, research-py root, \
                 registry path) this pure, engine-only function does not accept as input -- \
                 call crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario separately and \
                 merge it in via RobustnessGauntletOutput::merge_dsr_pbo_sensitivity before \
                 treating this artifact as complete (see RobustnessGauntletOutput::is_complete)"
                .to_string(),
        },
        DeferredScenario {
            name: crate::p7a_p7b_economic_replay_stress::P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME
                .to_string(),
            reason: "requires a completed, registered Research trial plus subprocess/filesystem \
                 I/O this pure, engine-only function does not accept as input -- call \
                 crate::p7a_p7b_economic_replay_stress::p7a_p7b_economic_replay_stress_scenario \
                 separately and merge it in via \
                 RobustnessGauntletOutput::merge_dsr_pbo_sensitivity (name-agnostic) before \
                 treating this artifact as complete (see RobustnessGauntletOutput::is_complete)"
                .to_string(),
        },
        DeferredScenario {
            name: crate::genuine_shuffled_placebo::GENUINE_SHUFFLED_PLACEBO_SCENARIO_NAME.to_string(),
            reason: "requires a completed, registered Research trial plus subprocess/filesystem \
                 I/O this pure, engine-only function does not accept as input -- call \
                 crate::genuine_shuffled_placebo::genuine_shuffled_placebo_scenario separately \
                 and merge it in via RobustnessGauntletOutput::merge_dsr_pbo_sensitivity \
                 (name-agnostic) before treating this artifact as complete (see \
                 RobustnessGauntletOutput::is_complete)"
                .to_string(),
        },
    ];

    RobustnessGauntletOutput {
        protocol_version: ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION.to_string(),
        run_id: baseline.run_id,
        config_id: baseline.config_id,
        strategy_name: baseline.strategy_name.clone(),
        scenarios,
        deferred,
    }
}

/// W06-P9-RUST-REPLAY-STRATEGY-01 (B5): additive counterpart to
/// [`run_robustness_gauntlet`] for candidates (e.g. `ResearchOosReplayStrategy`)
/// whose `symbol_leave_one_out` rerun needs a strategy built FROM the exact
/// filtered bars slice for that exclusion, not from a single no-argument
/// factory. Every OTHER scenario is byte-for-byte identical to
/// `run_robustness_gauntlet` (same six pure-engine scenarios via
/// `make_strategy`, same three deferred Research-registry-anchored
/// placeholders) -- only `symbol_leave_one_out` differs, via
/// `make_strategy_for_bars`. `run_robustness_gauntlet` itself is completely
/// unchanged and continues to work for every existing built-in strategy.
pub fn run_robustness_gauntlet_with_symbol_loo_factory(
    baseline: &BacktestReport,
    base_config: &BacktestConfig,
    bars: &[BacktestBar],
    make_strategy: impl Fn() -> Box<dyn Strategy>,
    make_strategy_for_bars: impl Fn(&[BacktestBar]) -> Box<dyn Strategy>,
) -> RobustnessGauntletOutput {
    let scenarios = vec![
        // R2.4: batch-aware execution-delay/placebo-offset for the Research
        // replay path -- `ResearchOosReplayStrategy` emits one complete
        // decision per TIMESTAMP BATCH (potentially many physical rows),
        // never one per physical row, so the physical-row-counting
        // `DelayedStrategy` these two scenarios use everywhere else would
        // emit a delayed decision mid-batch (see `TimestampBatchDelayedStrategy`
        // module docs). `run_robustness_gauntlet` (the builtin-strategy
        // entry point) is completely untouched.
        execution_delay_scenario_batch_aware(baseline, base_config, bars, &make_strategy),
        symbol_leave_one_out_scenario_with_factory(baseline, base_config, bars, &make_strategy_for_bars),
        month_year_regime_concentration_scenario(baseline, bars),
        parameter_neighborhood_scenario(baseline, base_config, bars, &make_strategy),
        placebo_temporal_offset_scenario_batch_aware(baseline, base_config, bars, &make_strategy),
        conservative_capacity_stress_scenario(baseline, base_config, bars, &make_strategy),
    ];

    let deferred = vec![
        DeferredScenario {
            name: crate::dsr_pbo_sensitivity::DSR_PBO_SENSITIVITY_SCENARIO_NAME.to_string(),
            reason: "requires subprocess/filesystem I/O (Python executable, research-py root, \
                 registry path) this pure, engine-only function does not accept as input -- \
                 call crate::dsr_pbo_sensitivity::dsr_pbo_sensitivity_scenario separately and \
                 merge it in via RobustnessGauntletOutput::merge_dsr_pbo_sensitivity before \
                 treating this artifact as complete (see RobustnessGauntletOutput::is_complete)"
                .to_string(),
        },
        DeferredScenario {
            name: crate::p7a_p7b_economic_replay_stress::P7A_P7B_ECONOMIC_REPLAY_STRESS_SCENARIO_NAME
                .to_string(),
            reason: "requires a completed, registered Research trial plus subprocess/filesystem \
                 I/O this pure, engine-only function does not accept as input -- call \
                 crate::p7a_p7b_economic_replay_stress::p7a_p7b_economic_replay_stress_scenario \
                 separately and merge it in via \
                 RobustnessGauntletOutput::merge_dsr_pbo_sensitivity (name-agnostic) before \
                 treating this artifact as complete (see RobustnessGauntletOutput::is_complete)"
                .to_string(),
        },
        DeferredScenario {
            name: crate::genuine_shuffled_placebo::GENUINE_SHUFFLED_PLACEBO_SCENARIO_NAME.to_string(),
            reason: "requires a completed, registered Research trial plus subprocess/filesystem \
                 I/O this pure, engine-only function does not accept as input -- call \
                 crate::genuine_shuffled_placebo::genuine_shuffled_placebo_scenario separately \
                 and merge it in via RobustnessGauntletOutput::merge_dsr_pbo_sensitivity \
                 (name-agnostic) before treating this artifact as complete (see \
                 RobustnessGauntletOutput::is_complete)"
                .to_string(),
        },
    ];

    RobustnessGauntletOutput {
        protocol_version: ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION.to_string(),
        run_id: baseline.run_id,
        config_id: baseline.config_id,
        strategy_name: baseline.strategy_name.clone(),
        scenarios,
        deferred,
    }
}

// ---------------------------------------------------------------------------
// STRESS-TRANSFORM-SEMANTIC-IDENTITY-01 negative controls
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// W06-P9-RUST-REPLAY-STRATEGY-01 (Patch B) integration tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod research_oos_replay_integration_tests {
    use super::*;
    use crate::corporate_actions::{CorporateActionPolicy, ForbidEntry};
    use crate::research_replay_strategy::{ReplaySemanticSpec, ResearchOosReplayStrategy};
    use crate::types::BacktestConfig;
    use mqk_execution::TargetPosition;

    const DAY: i64 = 86_400;

    fn semantic() -> ReplaySemanticSpec {
        ReplaySemanticSpec {
            replay_protocol_version: "research_oos_replay_bundle_v1".to_string(),
            strategy_id: "test_strategy_v1".to_string(),
            feature_columns: vec!["test_xs_rank".to_string()],
            feature_transform: "cross_sectional_percentile_rank_rerank_of_authenticated_feature_v1"
                .to_string(),
            direction_policy: "cross_sectional_rank_long_only_v1".to_string(),
            rank_side_count: 1,
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

    fn daily_config() -> BacktestConfig {
        BacktestConfig {
            timeframe_secs: DAY,
            ..BacktestConfig::test_defaults()
        }
    }

    fn two_symbol_bars(days: i64) -> Vec<BacktestBar> {
        let mut out = Vec::new();
        for d in 0..days {
            let ts = DAY * (d + 1);
            out.push(BacktestBar::new("AAA", ts, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1_000));
            out.push(BacktestBar::new("BBB", ts, 50_000_000, 50_000_000, 50_000_000, 50_000_000, 1_000));
        }
        out
    }

    fn schedule_for(bars: &[BacktestBar], qty_aaa: i64) -> BTreeMap<i64, Vec<TargetPosition>> {
        let mut schedule = BTreeMap::new();
        for ts in bars.iter().map(|b| b.end_ts).collect::<BTreeSet<_>>() {
            schedule.insert(ts, vec![TargetPosition::new("AAA", qty_aaa), TargetPosition::new("BBB", 0)]);
        }
        schedule
    }

    /// REQUIRED TEST 1: baseline replay reaches the real BacktestEngine and
    /// produces a real report.
    #[test]
    fn baseline_replay_reaches_real_engine() {
        let bars = two_symbol_bars(3);
        let schedule = schedule_for(&bars, 10);
        let strategy = ResearchOosReplayStrategy::new(semantic(), schedule, &bars);
        let report = run_engine(daily_config(), &bars, Box::new(strategy)).expect("engine run succeeds");
        assert!(!report.equity_curve.is_empty());
    }

    /// REQUIRED TEST 6: a corporate-action halt before Strategy dispatch
    /// produces no partial target emission -- the strategy never even sees
    /// on_bar for the halted batch (existing, unmodified engine behavior),
    /// so it cannot have emitted a partial vector for that timestamp.
    #[test]
    fn corporate_action_halt_before_dispatch_produces_no_partial_emission() {
        let bars = two_symbol_bars(3);
        let halt_ts = DAY * 2;
        let schedule = schedule_for(&bars, 10);
        let strategy = ResearchOosReplayStrategy::new(semantic(), schedule, &bars);
        let config = BacktestConfig {
            corporate_action_policy: CorporateActionPolicy::ForbidPeriods(vec![ForbidEntry::new(
                "AAA", halt_ts, halt_ts,
            )]),
            ..daily_config()
        };
        let report = run_engine(config, &bars, Box::new(strategy)).expect("engine run succeeds");
        assert!(report.halted, "engine must halt on the forbidden corporate-action period");
        // Day 1's full 2-bar batch reaches the strategy and records one
        // equity_curve entry per row; day 2's batch halts on its FIRST bar
        // (before any dispatch), so it contributes zero entries -- the
        // engine never even calls on_bar for the halted batch.
        assert_eq!(report.equity_curve.len(), 2);
    }

    /// REQUIRED TEST 7: signal-time pre-sized qty remains identical through
    /// `DelayedStrategy` -- wrapping only shifts WHEN the already-frozen
    /// target vector is emitted, never its content.
    #[test]
    fn signal_time_qty_survives_delayed_strategy_wrapper() {
        let bars = two_symbol_bars(4);
        let schedule = schedule_for(&bars, 7);
        let inner = ResearchOosReplayStrategy::new(semantic(), schedule.clone(), &bars);
        let plain_report =
            run_engine(daily_config(), &bars, Box::new(inner)).expect("plain run succeeds");

        let delayed_inner = ResearchOosReplayStrategy::new(semantic(), schedule, &bars);
        let delayed = DelayedStrategy::new(Box::new(delayed_inner), 1);
        let delayed_report =
            run_engine(daily_config(), &bars, Box::new(delayed)).expect("delayed run succeeds");

        // The delayed run's FINAL equity must match the plain run's -- the
        // same qty=7 target is eventually reached in both, only later in
        // the delayed case, never a different value.
        assert_eq!(
            plain_report.equity_curve.last().map(|(_, eq)| *eq),
            delayed_report.equity_curve.last().map(|(_, eq)| *eq),
        );
    }

    /// REQUIRED TEST 11: the symbol-leave-one-out scenario uses the schedule
    /// derived for the ACTUAL filtered bars, not the naive full-universe
    /// frozen schedule -- proven by a bar-aware factory that fails closed
    /// (via `run_with_strategy`'s own semantic-fingerprint check) whenever
    /// it is NOT given the correctly-filtered bars slice for that exclusion.
    #[test]
    fn symbol_loo_uses_schedule_derived_for_actual_filtered_bars() {
        let bars = two_symbol_bars(3);
        let baseline_schedule = schedule_for(&bars, 10);
        let baseline_strategy = ResearchOosReplayStrategy::new(semantic(), baseline_schedule, &bars);
        let baseline_config = daily_config();
        let baseline_report = run_engine(baseline_config.clone(), &bars, Box::new(baseline_strategy))
            .expect("baseline run succeeds");

        // The bar-aware factory asserts its `bars` argument really is the
        // filtered (1-symbol) slice for THIS exclusion -- constructing the
        // strategy from the wrong (unfiltered) bars would silently use the
        // wrong expected-row-count table and panic on the very first on_bar
        // call for this scenario, which would surface as a scenario
        // failure below rather than a passing false positive.
        let make_strategy_for_bars = |filtered: &[BacktestBar]| -> Box<dyn Strategy> {
            let distinct: BTreeSet<&str> = filtered.iter().map(|b| b.symbol.as_str()).collect();
            assert_eq!(distinct.len(), 1, "leave-one-out must be called with exactly one symbol removed");
            Box::new(ResearchOosReplayStrategy::new(
                semantic(),
                schedule_for(filtered, 10),
                filtered,
            ))
        };

        let outcome = symbol_leave_one_out_scenario_with_factory(
            &baseline_report,
            &baseline_config,
            &bars,
            &make_strategy_for_bars,
        );
        assert!(outcome.applicable);
        assert!(outcome.passed, "{:?}", outcome.reason);
    }

    // -----------------------------------------------------------------------
    // W06-A-P9-REPLAY-SOURCE-AUTHORITY-REPAIR-WAVE-02 (Patch R2) required
    // tests: R2.1 strategy_name contract, R2.5 batch-delay order-independence.
    // -----------------------------------------------------------------------

    /// R2.1 (gauntlet-level companion to `spec_name_is_research_strategy_id`):
    /// the REAL `RobustnessGauntletOutput` produced through the actual
    /// entry point carries the Research `strategy_id` as `strategy_name`,
    /// since it is copied verbatim from `baseline.strategy_name` (itself
    /// `Strategy::spec().name`).
    #[test]
    fn gauntlet_strategy_name_is_research_strategy_id() {
        let bars = two_symbol_bars(3);
        let schedule = schedule_for(&bars, 10);
        let strategy = ResearchOosReplayStrategy::new(semantic(), schedule.clone(), &bars);
        let config = daily_config();
        let baseline_report = run_engine(config.clone(), &bars, Box::new(strategy)).unwrap();
        assert_eq!(baseline_report.strategy_name, "test_strategy_v1");

        let make_strategy = || -> Box<dyn Strategy> {
            Box::new(ResearchOosReplayStrategy::new(semantic(), schedule.clone(), &bars))
        };
        let make_strategy_for_bars = |filtered: &[BacktestBar]| -> Box<dyn Strategy> {
            Box::new(ResearchOosReplayStrategy::new(semantic(), schedule_for(filtered, 10), filtered))
        };
        let gauntlet = run_robustness_gauntlet_with_symbol_loo_factory(
            &baseline_report,
            &config,
            &bars,
            make_strategy,
            make_strategy_for_bars,
        );
        assert_eq!(gauntlet.strategy_name, "test_strategy_v1");
    }

    /// Same two symbols/schedule/config as [`two_symbol_bars`]/[`schedule_for`],
    /// but with each timestamp batch's two physical rows in the OPPOSITE
    /// order -- `schedule_for` keys only on `end_ts`, so the schedule content
    /// is identical either way; only physical row order within each batch
    /// differs.
    fn two_symbol_bars_swapped_order(days: i64) -> Vec<BacktestBar> {
        let mut out = Vec::new();
        for d in 0..days {
            let ts = DAY * (d + 1);
            out.push(BacktestBar::new("BBB", ts, 50_000_000, 50_000_000, 50_000_000, 50_000_000, 1_000));
            out.push(BacktestBar::new("AAA", ts, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1_000));
        }
        out
    }

    /// R2.5 test 1: permuting all symbols within each timestamp under
    /// BASELINE replay produces an identical final report (equity, order
    /// count, fill count) -- `ResearchOosReplayStrategy` never inspects bar
    /// CONTENT, only counts calls, so a same-`end_ts` physical row
    /// permutation can never change its output.
    #[test]
    fn baseline_replay_is_order_independent_under_symbol_permutation() {
        let bars_a = two_symbol_bars(3);
        let bars_b = two_symbol_bars_swapped_order(3);
        let strat_a = ResearchOosReplayStrategy::new(semantic(), schedule_for(&bars_a, 10), &bars_a);
        let strat_b = ResearchOosReplayStrategy::new(semantic(), schedule_for(&bars_b, 10), &bars_b);
        let report_a = run_engine(daily_config(), &bars_a, Box::new(strat_a)).unwrap();
        let report_b = run_engine(daily_config(), &bars_b, Box::new(strat_b)).unwrap();
        assert_eq!(
            report_a.equity_curve.last().map(|(_, eq)| *eq),
            report_b.equity_curve.last().map(|(_, eq)| *eq),
        );
        assert_eq!(report_a.orders.len(), report_b.orders.len());
        assert_eq!(report_a.fills.len(), report_b.fills.len());
    }

    /// R2.5 test 2: the same permutation invariance holds under
    /// `execution_delay_stress` once it uses [`TimestampBatchDelayedStrategy`]
    /// (`execution_delay_scenario_batch_aware`) -- proven by comparing the
    /// REAL scenario's own `detail` string (which embeds the resulting
    /// final-equity value) across both physical row orderings.
    #[test]
    fn execution_delay_stress_is_order_independent_under_symbol_permutation() {
        let bars_a = two_symbol_bars(4);
        let bars_b = two_symbol_bars_swapped_order(4);
        let config = daily_config();

        let baseline_a =
            run_engine(config.clone(), &bars_a, Box::new(ResearchOosReplayStrategy::new(
                semantic(), schedule_for(&bars_a, 10), &bars_a,
            )))
            .unwrap();
        let baseline_b =
            run_engine(config.clone(), &bars_b, Box::new(ResearchOosReplayStrategy::new(
                semantic(), schedule_for(&bars_b, 10), &bars_b,
            )))
            .unwrap();

        let make_a = || -> Box<dyn Strategy> {
            Box::new(ResearchOosReplayStrategy::new(semantic(), schedule_for(&bars_a, 10), &bars_a))
        };
        let make_b = || -> Box<dyn Strategy> {
            Box::new(ResearchOosReplayStrategy::new(semantic(), schedule_for(&bars_b, 10), &bars_b))
        };

        let outcome_a = execution_delay_scenario_batch_aware(&baseline_a, &config, &bars_a, &make_a);
        let outcome_b = execution_delay_scenario_batch_aware(&baseline_b, &config, &bars_b, &make_b);
        assert_eq!(outcome_a.passed, outcome_b.passed);
        assert_eq!(outcome_a.detail, outcome_b.detail);
    }

    /// R2.5 test 3: the same permutation invariance holds under
    /// `placebo_temporal_offset` once it uses [`TimestampBatchDelayedStrategy`]
    /// (`placebo_temporal_offset_scenario_batch_aware`).
    #[test]
    fn placebo_temporal_offset_is_order_independent_under_symbol_permutation() {
        let bars_a = two_symbol_bars(6);
        let bars_b = two_symbol_bars_swapped_order(6);
        let config = daily_config();

        let baseline_a =
            run_engine(config.clone(), &bars_a, Box::new(ResearchOosReplayStrategy::new(
                semantic(), schedule_for(&bars_a, 10), &bars_a,
            )))
            .unwrap();
        let baseline_b =
            run_engine(config.clone(), &bars_b, Box::new(ResearchOosReplayStrategy::new(
                semantic(), schedule_for(&bars_b, 10), &bars_b,
            )))
            .unwrap();

        let make_a = || -> Box<dyn Strategy> {
            Box::new(ResearchOosReplayStrategy::new(semantic(), schedule_for(&bars_a, 10), &bars_a))
        };
        let make_b = || -> Box<dyn Strategy> {
            Box::new(ResearchOosReplayStrategy::new(semantic(), schedule_for(&bars_b, 10), &bars_b))
        };

        let outcome_a = placebo_temporal_offset_scenario_batch_aware(&baseline_a, &config, &bars_a, &make_a);
        let outcome_b = placebo_temporal_offset_scenario_batch_aware(&baseline_b, &config, &bars_b, &make_b);
        assert_eq!(outcome_a.passed, outcome_b.passed);
        assert_eq!(outcome_a.detail, outcome_b.detail);
    }

    /// R2.5 test 4: the delayed target vector never appears before the FINAL
    /// physical row of the delayed-to timestamp -- proven directly against
    /// [`TimestampBatchDelayedStrategy`] (not `DelayedStrategy`, which this
    /// mission explicitly forbids using for the multi-row-per-timestamp
    /// replay case): with `delay_batches=1` and a 2-row batch, the FIRST row
    /// of the second timestamp must still emit empty; only the SECOND
    /// (final) row of the second timestamp emits the first batch's decision.
    #[test]
    fn timestamp_batch_delay_never_emits_before_final_row_of_delayed_to_batch() {
        let bars = two_symbol_bars(2);
        let inner = ResearchOosReplayStrategy::new(semantic(), schedule_for(&bars, 10), &bars);
        let mut delayed = TimestampBatchDelayedStrategy::new(Box::new(inner), 1, &bars);

        let ctx = |end_ts: i64| -> StrategyContext {
            let recent = mqk_strategy::RecentBarsWindow::new(
                10,
                vec![mqk_strategy::BarStub::with_ohlcv(
                    end_ts, true, 100_000_000, 100_000_000, 100_000_000, 100_000_000, 1_000,
                )],
            );
            StrategyContext::new(DAY, 1, recent)
        };

        // Batch 1 (end_ts=DAY), both rows: buffering, nothing emitted yet.
        assert!(delayed.on_bar(&ctx(DAY)).targets.is_empty());
        assert!(delayed.on_bar(&ctx(DAY)).targets.is_empty());
        // Batch 2 (end_ts=2*DAY), FIRST row: must still be empty -- batch 2
        // is not yet complete, so batch 1's decision must not leak early.
        assert!(delayed.on_bar(&ctx(2 * DAY)).targets.is_empty());
        // Batch 2's FINAL row: batch 1's decision is emitted now.
        let emitted = delayed.on_bar(&ctx(2 * DAY));
        assert!(!emitted.targets.is_empty(), "batch 1's decision must emit on batch 2's final row");
    }
}

#[cfg(test)]
mod stress_transform_semantic_identity_tests {
    use super::*;
    use mqk_strategy::RecentBarsWindow;

    /// A strategy whose `spec()` is fixed but whose `semantic_fingerprint()`
    /// is caller-controlled -- lets these tests simulate two materially
    /// different candidates (A vs B) sharing an identical name/on_bar
    /// behavior, so a real mismatch can never be explained away as "the
    /// results merely differed".
    struct FingerprintedStrategy {
        fingerprint: &'static str,
    }

    impl Strategy for FingerprintedStrategy {
        fn spec(&self) -> StrategySpec {
            StrategySpec::new("fp_strategy", 60)
        }

        fn on_bar(&mut self, _ctx: &StrategyContext) -> StrategyOutput {
            StrategyOutput::new(vec![])
        }

        fn semantic_fingerprint(&self) -> String {
            self.fingerprint.to_string()
        }
    }

    fn bars() -> Vec<BacktestBar> {
        (0..5)
            .map(|i| BacktestBar::new("AAPL", 1_000 + i * 60, 100, 100, 100, 100, 1_000))
            .collect()
    }

    // 1. raw candidate A matches baseline A and is accepted for wrapping.
    #[test]
    fn raw_candidate_matching_baseline_is_accepted() {
        let a = FingerprintedStrategy { fingerprint: "fp-a" };
        assert!(verify_candidate_identity(&a, "fp-a").is_ok());
    }

    // 2. raw candidate B presented as baseline A is refused BEFORE wrapping.
    #[test]
    fn raw_candidate_b_presented_as_a_is_refused_before_wrapping() {
        let b = FingerprintedStrategy { fingerprint: "fp-b" };
        let err = verify_candidate_identity(&b, "fp-a")
            .expect_err("mismatched raw candidate must be refused before any wrapping occurs");
        assert!(err.contains("semantic fingerprint mismatch"));
    }

    // 3. DelayedStrategy(A,1).fp != A.fp
    #[test]
    fn wrapper_fingerprint_differs_from_inner_fingerprint() {
        let a = Box::new(FingerprintedStrategy { fingerprint: "fp-a" }) as Box<dyn Strategy>;
        let inner_fp = a.semantic_fingerprint();
        let wrapped = DelayedStrategy::new(a, 1);
        assert_ne!(
            wrapped.semantic_fingerprint(),
            inner_fp,
            "the execution-delay wrapper must carry its own effective fingerprint, distinct \
             from the wrapped candidate's"
        );
    }

    // 4. DelayedStrategy(A,1).fp != DelayedStrategy(A,2).fp
    #[test]
    fn wrapper_fingerprint_differs_by_delay_bars() {
        let a1 = Box::new(FingerprintedStrategy { fingerprint: "fp-a" }) as Box<dyn Strategy>;
        let a2 = Box::new(FingerprintedStrategy { fingerprint: "fp-a" }) as Box<dyn Strategy>;
        let delayed_1 = DelayedStrategy::new(a1, 1);
        let delayed_2 = DelayedStrategy::new(a2, 2);
        assert_ne!(
            delayed_1.semantic_fingerprint(),
            delayed_2.semantic_fingerprint(),
            "different delay_bars on the SAME underlying candidate must produce different \
             wrapper fingerprints"
        );
    }

    // 5. identical A + identical delay -> identical wrapper fingerprint
    // (repeated construction is deterministic).
    #[test]
    fn wrapper_fingerprint_is_deterministic_for_same_candidate_and_delay() {
        let a1 = Box::new(FingerprintedStrategy { fingerprint: "fp-a" }) as Box<dyn Strategy>;
        let a2 = Box::new(FingerprintedStrategy { fingerprint: "fp-a" }) as Box<dyn Strategy>;
        let fp1 = DelayedStrategy::new(a1, 1).semantic_fingerprint();
        let fp2 = DelayedStrategy::new(a2, 1).semantic_fingerprint();
        assert_eq!(
            fp1, fp2,
            "identical underlying candidate + identical delay must reproduce the identical \
             wrapper fingerprint"
        );
    }

    // 6. A != B -> wrapped A and wrapped B remain different.
    #[test]
    fn wrapper_fingerprint_changes_when_underlying_candidate_changes() {
        let a = Box::new(FingerprintedStrategy { fingerprint: "fp-a" }) as Box<dyn Strategy>;
        let b = Box::new(FingerprintedStrategy { fingerprint: "fp-b" }) as Box<dyn Strategy>;
        let fp_a = DelayedStrategy::new(a, 1).semantic_fingerprint();
        let fp_b = DelayedStrategy::new(b, 1).semantic_fingerprint();
        assert_ne!(
            fp_a, fp_b,
            "wrapping two different underlying candidates at the same delay must never collide"
        );
    }

    // 7. Same strategy_name/config/bars/economics/execution_model: a
    // baseline run and a DelayedStrategy(A,1) run of the SAME candidate must
    // never collide on run_id -- the R9 defect this repair closes.
    #[test]
    fn baseline_and_delayed_run_never_collide_on_run_id() {
        let config = BacktestConfig::test_defaults();
        let bars = bars();

        let mut baseline_engine = BacktestEngine::new(config.clone());
        baseline_engine
            .add_strategy(Box::new(FingerprintedStrategy { fingerprint: "fp-a" }))
            .unwrap();
        let baseline_report = baseline_engine.run(&bars).unwrap();

        let delayed = DelayedStrategy::new(
            Box::new(FingerprintedStrategy { fingerprint: "fp-a" }),
            1,
        );
        let mut delayed_engine = BacktestEngine::new(config);
        delayed_engine.add_strategy(Box::new(delayed)).unwrap();
        let delayed_report = delayed_engine.run(&bars).unwrap();

        assert_eq!(baseline_report.strategy_name, delayed_report.strategy_name);
        assert_eq!(baseline_report.config_id, delayed_report.config_id);
        assert_eq!(baseline_report.input_data_hash, delayed_report.input_data_hash);
        assert_ne!(
            baseline_report.run_id, delayed_report.run_id,
            "an execution-delay-wrapped run of the SAME candidate must never collide on run_id \
             with the unwrapped baseline run sharing strategy_name/config/bars"
        );
    }

    // 8. With the SAME underlying candidate/config/bars, two different
    // delays must never collide on run_id.
    #[test]
    fn delayed_runs_with_different_delays_never_collide_on_run_id() {
        let config = BacktestConfig::test_defaults();
        let bars = bars();

        let mut engine_1 = BacktestEngine::new(config.clone());
        engine_1
            .add_strategy(Box::new(DelayedStrategy::new(
                Box::new(FingerprintedStrategy { fingerprint: "fp-a" }),
                1,
            )))
            .unwrap();
        let report_1 = engine_1.run(&bars).unwrap();

        let mut engine_2 = BacktestEngine::new(config);
        engine_2
            .add_strategy(Box::new(DelayedStrategy::new(
                Box::new(FingerprintedStrategy { fingerprint: "fp-a" }),
                2,
            )))
            .unwrap();
        let report_2 = engine_2.run(&bars).unwrap();

        assert_eq!(report_1.strategy_name, report_2.strategy_name);
        assert_eq!(report_1.config_id, report_2.config_id);
        assert_eq!(report_1.input_data_hash, report_2.input_data_hash);
        assert_ne!(
            report_1.run_id, report_2.run_id,
            "the same underlying candidate wrapped at two different delays must never collide \
             on run_id"
        );
    }

    // 11. A single make_strategy() invocation returning the wrong underlying
    // candidate still fails closed before any wrapping/execution occurs.
    #[test]
    fn wrong_underlying_candidate_fails_closed_before_wrapping() {
        let b = FingerprintedStrategy { fingerprint: "fp-b" };
        assert!(
            verify_candidate_identity(&b, "fp-a").is_err(),
            "a candidate presented under the wrong baseline fingerprint must fail closed"
        );
    }

    // 12. No P&L/result/score/fill value participates in the wrapper
    // fingerprint -- mutating the wrapper's internal buffer state via real
    // `on_bar` calls must never change its fingerprint.
    #[test]
    fn wrapper_fingerprint_is_unaffected_by_on_bar_execution_state() {
        let inner = Box::new(FingerprintedStrategy { fingerprint: "fp-a" }) as Box<dyn Strategy>;
        let mut wrapped = DelayedStrategy::new(inner, 1);
        let fp_before = wrapped.semantic_fingerprint();

        let ctx = StrategyContext::new(60, 0, RecentBarsWindow::new(1, vec![]));
        for _ in 0..3 {
            wrapped.on_bar(&ctx);
        }

        assert_eq!(
            fp_before,
            wrapped.semantic_fingerprint(),
            "the wrapper's fingerprint must be fixed at construction, never influenced by \
             execution/result state accumulated via on_bar"
        );
    }
}
