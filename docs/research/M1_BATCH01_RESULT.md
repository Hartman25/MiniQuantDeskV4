# M1 native hypothesis batch 01 — Result

Mission `V4-M1-NATIVE-HYPOTHESIS-BATCH-01`. Definition: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_BATCH_01.json` (commit `952a6d70`, committed before the engines and before any batch economics). Census: `M1_BATCH01_CENSUS.md`. Run evidence is local and untracked (`runs/run_batch_01`).

## Verdict

**BATCH_REJECTED.** All fifteen predeclared trials ran. Zero reach `paper_candidate`; no trial reaches the frozen DSR minimum of 0.5 (best 0.214); PBO is 0.238 (passes). No promotion attempt, no Paper change, holdout reserved in every artifact. The three hypotheses are closed; no variation of any of them is authorized.

## Hypotheses (exact semantics frozen in the predeclaration)

- H1 `absolute_momentum_252` — long iff `close[t] > close[t-252]` (253 closes).
- H2 `near_high_momentum_252_3pct` — long iff `100*c >= 97*high252` and `50*c > sum50` (252 closes).
- H3 `trend_pullback_5d_4pct_hold5` — long iff an entry event (`200*c > sum200`, `100*c <= 96*c[j-5]`) occurred on any of the latest five bars (204 closes).

All three are stateless, long/flat, exact `i128` integer comparisons, fail closed on short history, incomplete or non-positive bars, and bind every semantic in their fingerprint. Emitted streams matched an independent integer reference at every bar; nine boundary/count mutations were each killed by a unit test.

## Population and judge

One experiment (`M1-NATIVE-HYPOTHESIS-BATCH-01-REAL`), three hypotheses, fifteen trials, all registered before any attempt existed. Registered 15; attempted 15 (one attempt each, all succeeded); evaluable 14; excluded 1 (`trend_pullback_5d_4pct_hold5`/IEF, `degenerate_returns:zero_variance_returns` — the engine never traded it). One batch-wide judge over the whole experiment (`hypothesis_id` unset): `evaluated`, effective independent trials 12.38 of 14 (mean pairwise correlation 0.125), PBO 0.238 over 252 combinations, DSR range 0.000–0.214.

Prior closed families have their own experiment ids and registries and are disclosed, not merged; adding their trials to the population could only lower every DSR, so the rejection is not sensitive to that omission.

## Results (frozen sizing 100,000 / 50,000, `rust_conservative_bar_range_v1`, commission 10 bps, no separate slippage)

Net/gross are economic walk-forward returns; CAGR annualized net; profit factor is the Rust backtest's (trade-based); profitable months computed from the economic daily net series (descriptive); position agreement 1.000 for all.

| # | Strategy | Sym | Net | Gross | Sharpe | DSR | CAGR | Max DD | PF | Prof. mo. | Trades | Cost drag | Robustness failures | Stress failures | Review |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| 1 | H1 | SPY | +36.4% | +64.8% | 0.46 | 0.137 | 3.2% | -23.3% | 1.33 | 0.56 | 11 | 28.4% | regime concentration | 2x, 3x cost | non_positive_alpha |
| 2 | H1 | EFA | +5.1% | +28.8% | 0.11 | 0.014 | 0.5% | -28.7% | 0.32 | 0.45 | 25 | 23.7% | regime concentration | 2x, 3x cost | non_positive_alpha |
| 3 | H1 | IEF | -0.9% | +8.2% | -0.04 | 0.004 | -0.1% | -11.4% | 0.61 | 0.26 | 22 | 9.1% | regime conc., temporal placebo | none | negative_total_return |
| 4 | H1 | VNQ | -31.0% | +4.3% | -0.53 | 0.000 | -3.6% | -36.5% | 0.09 | 0.35 | 42 | 35.3% | regime conc., temporal placebo | 3x cost | negative_total_return |
| 5 | H1 | GLD | +48.2% | +83.2% | 0.56 | 0.214 | 4.0% | -25.6% | 0.43 | 0.37 | 33 | 35.1% | regime concentration | none | non_positive_alpha |
| 6 | H2 | SPY | -15.8% | +27.1% | -0.37 | 0.000 | -1.7% | -22.8% | 0.40 | 0.33 | 60 | 42.9% | delay, regime conc., parameter nbhd, temporal placebo, capacity | 2x, 3x cost, risk limits | negative_total_return |
| 7 | H2 | EFA | -6.1% | +25.4% | -0.14 | 0.001 | -0.6% | -24.8% | 0.39 | 0.28 | 50 | 31.5% | temporal placebo | none | negative_total_return |
| 8 | H2 | IEF | -12.2% | +6.9% | -0.73 | 0.000 | -1.3% | -16.4% | 0.38 | 0.17 | 53 | 19.1% | regime conc., temporal placebo | none | negative_total_return |
| 9 | H2 | VNQ | -35.2% | +5.9% | -1.09 | 0.000 | -4.3% | -37.0% | 0.13 | 0.17 | 61 | 41.1% | regime conc., temporal placebo | 3x cost | negative_total_return |
| 10 | H2 | GLD | -10.2% | +32.4% | -0.18 | 0.001 | -1.1% | -19.2% | 0.52 | 0.17 | 69 | 42.5% | delay, regime conc., parameter nbhd, temporal placebo | 2x, 3x cost | negative_total_return |
| 11 | H3 | SPY | -12.4% | +5.8% | -0.47 | 0.000 | -1.3% | -16.6% | 0.13 | 0.03 | 13 | 18.1% | delay, regime concentration | 2x, 3x cost | negative_total_return |
| 12 | H3 | EFA | +7.6% | +12.0% | 0.55 | 0.188 | 0.7% | -2.1% | 2.41 | 0.04 | 6 | 4.4% | regime concentration | 2x, 3x cost | non_positive_alpha |
| 13 | H3 | IEF | 0.0% | 0.0% | n/a | n/a | 0.0% | 0.0% | n/a | 0.00 | 0 | 0.0% | temporal placebo, sensitivity, shuffled placebo | none | non_positive_alpha |
| 14 | H3 | VNQ | -15.4% | -5.2% | -0.69 | 0.000 | -1.7% | -17.1% | 0.00 | 0.03 | 10 | 10.3% | regime conc., temporal placebo | none | negative_total_return |
| 15 | H3 | GLD | +0.8% | +14.1% | 0.04 | 0.007 | 0.1% | -6.0% | 0.58 | 0.06 | 15 | 13.3% | regime concentration | none | negative_total_return |

Review states are all `rejected`: 5 of 5 per strategy. The robustness column lists every applicable P9 scenario that failed; `symbol_leave_one_out` is not applicable to a single symbol.

## Reading

- The decisive failures are statistical and economic, not policy: the best DSR is 0.214 against 0.5, and nine of fifteen trials lose money after costs. The scanner's alpha gate (account return versus a fully invested buy-and-hold price return) did not decide the outcome: every trial also fails the DSR gate, and every positive-net trial fails regime concentration.
- H1 is profitable gross on every symbol; costs consume 9–35% of equity in 11–42 round trips. H2 flips constantly (50–69 trades) and loses on four of five symbols. H3 trades rarely (0–15 trades), is the least cost-sensitive, and its only positive-Sharpe symbols (EFA, GLD) sit on 6 and 15 trades with tiny CAGR.
- Recurring pattern across all seven closed hypotheses: conservative bar-range execution cost versus daily-bar edge. No cost model, threshold or review policy was changed.

## Governance notes (not acted on)

1. Review alpha compares account return with a fully invested buy-and-hold price return and is not exposure- or capital-matched (`M1_BATCH01_CENSUS.md` §0A). Non-binding for this batch.
2. DSR deflates over this batch's population only; closed families live in separate registries.

## Second adversarial sweep (`7dd31e5d..HEAD`, M1 bridge, campaign and shared seams)

| Check | Disposition | Evidence |
|---|---|---|
| Duplicate strategy authority | ALREADY_CORRECT+PROVEN | Each rule exists only in its `mqk-strategy` engine; `research-py/src` contains none of the batch strategy ids; the runner emits through `mqk backtest native-signals`; `test_batch01_engine_binding.py` binds the predeclaration to the Rust constants. |
| Hard-coded history limits | ALREADY_CORRECT+PROVEN | Backtest window is `max(config, required_history_bars())`; Paper `STRATEGY_CONTEXT_LOAD_LIMIT` 256 is guard-tested against every registered engine (widest 253). |
| Stale registry-count mirrors | FIXED+PROVEN | Universe 8 to 11 across `REGISTERED_STRATEGY_IDS`, `MAX_STRATEGY_UNIVERSE`, plan-builder and runtime tests (all pass); a stale "25 today" comment in `mqk-db` replaced by a non-numeric statement. |
| State reset / reconstruction | ALREADY_CORRECT+PROVEN | Batch engines are stateless; a fresh instance fed only its bounded history equals a long-lived instance at every bar of a 900-bar fixture; the stateful pullback replay is covered by its own tests. |
| Benchmark comparability | POLICY_AMBIGUOUS (recorded, unchanged) | Matches the documented contract; not exposure-matched; non-binding because all 15 trials fail DSR independently. |
| Execution cost double counting | ALREADY_CORRECT+PROVEN | All 15 economic artifacts carry `rust_conservative_bar_range_v1`, commission 10 bps, `slippage_bps_per_side` 0, identical sizing; the economic spec's double-charge guard is unchanged. |
| Batch selection scope | ALREADY_CORRECT+PROVEN | One judge over all 15 registered trials (`hypothesis_id` unset); the Rust verifier (`research_registry.rs`) accepts a whole-experiment judge for any trial of that experiment. |
| Trial / attempt confusion; hidden failed trials | ALREADY_CORRECT+PROVEN | 15 trials, 15 attempts (index 1 each), registry `succeeded` 15; the excluded trial is reported with its reason, not dropped. |
| Result-derived identity | ALREADY_CORRECT+PROVEN | Trial identity is strategy, fingerprint, provenance, partition and protocol only; the predeclaration guard bans result keys; the runner fails closed on trial-id drift between `register` and `trials`. |
| Holdout leakage | ALREADY_CORRECT+PROVEN | Every artifact says `reserved_not_evaluated` (2026-03-01 to 2026-09-01); the bridge refuses a backtest CSV that is not the holdout-truncated conversion. |
| Fingerprint mismatch / scanner or promotion bypass | ALREADY_CORRECT+PROVEN | The evidence gate rejects a different, missing or malformed native fingerprint and label-only evidence when binding is required (12 gate tests pass); no promotion transition was attempted. |
| Paper activation without authority | ALREADY_CORRECT+PROVEN | `MQK_STRATEGY_IDS` and all promotion-policy keys are ABSENT in `.env.local`; fleet unset. |
| Secret exposure; Live mutation | ALREADY_CORRECT+PROVEN | No webhook/key/secret pattern and no `.env`/`smoke_logs` path in the range diff; no Live enablement added. |
| Commit message of `2d52c2fd` carries two stray `@` lines | NOTED (cosmetic) | Created with the wrong here-string syntax; amend is forbidden, content is correct. |

Remaining ordinary deterministic in-scope defects: NONE.

## Acceptance proofs run

mqk-strategy lib 203 pass; mqk-backtest batch/lifecycle/required-history scenarios 10 pass and lib 109 pass; mqk-promotion 100% of its suites pass; mqk-portfolio lib 287 pass; mqk-runtime native_strategy 9 pass; mqk-daemon plan-builder and Paper context-window guards 12 pass (1 ignored), evidence gate 12 pass, native bridge b1c 13 pass (1 ignored, DB-dependent); research-py bridge, judge and predeclaration guards 88 pass; clippy `-D warnings` clean on mqk-strategy and mqk-backtest; rustfmt clean on touched files. Nine engine mutations killed. The full promotion transition against a live DB was not exercised because no candidate qualified.
