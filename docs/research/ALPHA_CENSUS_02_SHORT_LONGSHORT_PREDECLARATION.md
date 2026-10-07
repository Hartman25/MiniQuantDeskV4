# Alpha Census 02 — Short / Long-Short Predeclaration (capability census + operator-freeze package)

Mission `V4-ALPHA-CENSUS-02-SHORT-LONGSHORT-PREDECLARATION-01`. Baseline `c005e56791f34f1fd5d32e559dd594ec8e30d2c4`.
Status: `CENSUS02_PREDECLARATION_READY_FOR_OPERATOR_FREEZE` — the proposal package is complete and tested, but the freeze itself is **not**
executed: eight scientific / economic policies are not resolved by any binding repo authority (section 9), so no freeze file exists and no
real Census-02 data was read. The session stops at the operator-decision boundary. `VALIDATION_STATUS=NOT_VALIDATED`, `PROMOTION_AUTHORITY=NONE`. Paper INACTIVE, Live DISABLED.

Machine-readable proposal: `research-py/experiments/alpha_edge_census_02/CENSUS02_PREDECLARATION_PROPOSAL.json` (status
`PROPOSED_NOT_FROZEN`; it can never satisfy `c2_protocol.require_freeze`). Code: `research-py/experiments/alpha_edge_census_02/c2_*.py`;
tests `research-py/tests/test_alpha_edge_census_02{,_protocol}.py`; mutation proof `.../results/c2_mutation_proof_log.json`.

## 1. Fences and consumption (this mission)

| Item | Value |
|---|---|
| Discovery window used | `[2016-01-01, 2024-01-01)` (only synthetic bars were ever loaded) |
| 2024 contaminated rows scored / Confirmation rows / Final Holdout rows | 0 / 0 / 0 |
| Real Census-02 attempts executed | 0 |
| Real-data smoke before freeze | none (Pass-2's pre-freeze in-memory smoke is the cautionary precedent; no module here has a data-acquisition path — `test_census02_modules_have_no_data_acquisition_path`) |

## 2. Capability census (evidence = current code/tests/accepted artifacts)

| Capability | Finding | Consequence |
|---|---|---|
| Census-01 grammar / engine | 434 configs S01–S14 x 88 symbols = 38,192 trials; every signal is a `bool` long state; `simulate.simulate` casts `d` to `bool` | A signed series passed to it silently becomes **long** (proven: `test_census01_simulator_is_bool_only_and_silently_flips_shorts_to_longs`). Census-02 uses its own signed simulator and never routes through it. Census-01 itself is unchanged (frozen). |
| Position / Side / negative qty | No short arithmetic in the Census path. Separate Research long/short exists only in `ml/economic_walkforward.py` (ML pooled policies), not the S-family engine | New `c2_simulate.simulate_signed` (signed qty, SELL entry at low-slip, BUY cover at high+slip, flip = exit leg + entry leg) |
| Benchmark | Census-01 alpha = net − **long** buy-and-hold | Wrong for any short-bearing strategy: a constant short would score about −2x the market. Census-02 records a same-direction passive hold and cash; the qualification rule is an operator decision |
| Cross-sectional | Accepted `min_cross_section=20`; no portfolio-level census simulator; `economic_walkforward` has a ML rank policy only | Cross-sectional LS = deferred (section 5) |
| Factor machinery | `FactorSpec.direction` supports `lower_is_better`; V3 condition projection (execution-only params never mint a condition) | Reused in design: 215 short conditions x 5 horizons = 1,075 factors; Census-02 conditional runner is NOT implemented (post-freeze) |
| Borrow / locate / fee / recall | **No historical data anywhere.** Alpaca asset `shortable` / `easy_to_borrow` are current-state snapshots used only by the Paper short preflight; `alpaca_historical` has no asset/borrow endpoint; the only Research borrow model is `research_assumed_shortable_universe_v1` (SHORT-01/-WAVE-02 reports: "never promotion-ready evidence") | Class B (point-in-time borrow truth) is **unreachable**. See section 6 |
| Corporate actions for shorts | `adjustment=all` total-return series (a short bears dividends through the adjusted series); unsupported actions are typed `EXCLUDED_UNSUPPORTED_CORPORATE_ACTION` (RKLB precedent); the CA fail-closed gate exists | Inherited; splits/M&A/delisting recall dynamics are unobservable here (limit, not asserted away) |
| SSR (Rule 201) | Not represented in Research | Trigger is derivable from daily bars (`low <= 0.9 x prior close`, restriction through the next session). Implemented as a recorded per-fill flag (`ssr_flag`); acting on it is an operator decision |
| Delisting / survivorship | Universe is a **current-registry snapshot, not point-in-time** (88 symbols). The registry has no instrument-type or sector field | Mandatory survivorship label on every record; sign of the bias for shorts is not asserted; ETF membership cannot be derived from the repo and must be an explicit frozen list |
| Liquidation / cover | Census-01: open position marked to last close, no liquidation cost | Unchanged for shorts (stated in the protocol) |
| Identity / attempts | `trial_id = sha256(identity)`; attempt = new row of the same trial in `ResearchResultStore` | Reused; `side` added to identity; results/attempts proven not to enter identity |
| Resume / idempotency | Chunked resumable runner + factor resume exist for Census-01 | Reusable after freeze; not rebuilt now |
| Fences | `partitions.require_discovery_only`, canonical-session check in `SymbolData` | Reused; `c2_signals.build_symbol_data` fences before any array is built |

## 3. Proposed grammar (exact) — tiers are an operator decision

Tier **H** (short-only mirrors, 430 configs; signal semantics causal, `d in {0,-1}`):

| Family | Short hypothesis (mirror of) | Grid | Configs |
|---|---|---|---|
| SH01 | short while `log(c_t/c_{t-L}) < 0`, cadence daily/month_end (S01) | L {21,63,126,252} x cadence 2 | 8 |
| SH02 | short while close < SMA(n) (S02) | n {20,50,100,150,200,250} | 6 |
| SH03 | short while SMA(fast) < SMA(slow) — bearish state/cross (S03) | fast<slow pairs | 14 |
| SH04 | short on close < prior `entry`-lower channel; cover on close > prior `exit`-upper channel (S04) | 5 x 3, exit<entry | 12 |
| SH05 | RSI overbought: short RSI > entry_above, cover RSI < exit_below, trend none/below_sma200 (S05) | 6 x {90,80,70} x {50,30} x 2 | 72 |
| SH06 | short z >= entry_z, cover z <= 0 (S06) | 4 x {1.5,2,2.5} x 2 | 24 |
| SH07 | large rally >= mult x prior ATR over `rise_sessions`, hold h (S07) | 3x2x3x3x2 | 108 |
| SH08 | up-gap fade: open − prior close >= mult x prior ATR, hold h (S08) | 2x3x3x2 | 36 |
| SH09 | vol-contraction **downside** breakdown, cover on exit-channel break (S09) | 2x2x2x2x2x2 | 64 |
| SH10 | low-proximity momentum: short while close <= (1+d) x rolling-min close (S10; heuristic mirror, no external methodological claim) | 2 x 3 x 2 | 12 |
| SH11 | consecutive-up reversal, hold h (S11) | 3 x 3 x 2 | 18 |
| SH12 | volume/price surprise: `continuation` = down impulse, `reversal` = up impulse faded (S12) | 2x2x2x2x2 | 32 |
| SH13 | **volatility-expansion**: `breakdown` = expansion on a down day; `reversal` = expansion on an UP day faded short (exact mirror of Census-01 S13) | 2x2x2x3 | 24 |

S14 (calendar) is not mirrored (no coherent short form). Tier **L** (symmetric long/short state pairs, 40 configs, `d in {-1,0,+1}`,
conflicting states cancel to flat): LS01 8 (S01+SH01), LS02 6, LS03 14, LS04 12 (S10 near-high long + SH10 near-low short).
`S13 reversal` (the strongest Census-01 conditional clue) appears as SH13/`reversal` (and its conditional factor); nothing assumes it works.

**Count arithmetic** (computed by `c2_grammar.candidate_arithmetic`, pinned by test): configs 430 (H) / 470 (H+L); short conditions 215
(SH01 8, 02 6, 03 14, 04 5, 05 36, 06 24, 07 36, 08 12, 09 32, 10 12, 11 6, 12 16, 13 8); conditional factors 215 x 5 horizons = **1,075**.
Strategy trials = configs x executable-scope symbols: e.g. frozen scope of 20 ETFs -> 9,400 (H+L), 8,600 (H), 8,280 (H+L, complements
excluded); no executable shorts -> 0 trials, 1,075 label factors. 470 x 88 = 41,360 is **not** an authorized population (individual-equity shorts
are never executable).

**Duplicated mirrored candidates (finding).** SH01–SH03 are exact state complements of S01–S03 (proven disjoint and exhaustive:
`test_state_mirrors_are_exact_complements_of_census01`). Short-only P&L ≈ Census-01 long P&L minus buy-and-hold; LS01–03 = their sum. They
are information already observed in Census-01, so 56 configs (SH01–03, LS01–03) are **tagged** `complement_of_census01` (never part of
identity) and the operator chooses register-all vs exclude-before-freeze. Event/latched mirrors are disjoint events, not complements.

## 4. Short execution, cost, benchmark semantics (structural protocol, no new threshold)

* Decision after completed bar `t` uses bars `<= t`; fill on bar `t+1`: short entry = SELL at `low − slip`, cover = BUY at `high + slip`
  (`rust_conservative_bar_range_v1`, 5 bps, integer micros); same-bar fill impossible (`h[1:] = d[:-1]`); commission 10 bps/side.
* `qty = sign x floor(10,000 USD / close of the signal bar)`, constant per run, no compounding. P&L marks `q_before x (close_t − close_{t-1})`
  (negative q gains when price falls). A long→short flip is one exit leg + one entry leg on the same fill bar, each priced at that bar's fill.
* Borrow cost: `|q| x prior close x annual_fee_bps/10,000/252` per held-short bar; **no default** — a short with no explicit finite fee raises.
  Rebate zero; availability and recall are disclosed assumptions, never asserted.
* Benchmarks recorded for a short strategy: same-direction passive **short** hold (same window, same cost model) and cash. Long buy-and-hold is
  refused as a short benchmark. Warning for the operator: over 2016–2023 a short-hold benchmark is a weak bar in a rising market, so
  `alpha vs short-hold` alone can reward merely being flat — hence the rule options below.
* `fwd_ret` / `close_{t+h}/close_t − 1` is a label: `simulate_signed` accepts only int64 micro OHLC with `high >= close >= low > 0`.

## 5. Feasibility of the other requested forms

| Form | Verdict |
|---|---|
| Symmetric long/short | Implemented (tier L, state families only; stop-and-reverse event/latched pairs deferred) |
| Top-vs-bottom cross-sectional momentum | **Deferred**: needs a portfolio simulator and universe-scope identity; `min_cross_section=20` makes a liquid-ETF-only short leg marginal; equity short leg is hypothesis-only. If included later: lookback {21,63,126,252} x month_end x quintile = 4 candidates |
| Sector-relative / residual mean reversion | **Not feasible**: no sector authority (inferring sector from a symbol is forbidden) |
| Beta / market-neutral spreads | **Deferred**: long-name/short-SPY needs only a liquid-ETF short but requires a causal rolling-beta contract; applying it to existing long candidates would be retroactive rescue of consumed results |

## 6. Borrow policy (hard stop before real execution)

What the repo can establish: **no** point-in-time borrow availability, easy/hard-to-borrow state, locate, fee or recall for any symbol.

| Evidence class | Meaning | Reachable now |
|---|---|---|
| A — individual-equity short hypothesis | label-based `ConditionalEdge`-style evidence only; **no P&L is ever produced** (`evaluate_short_cell` returns `HYPOTHESIS_ONLY…`, `m=None`) | yes |
| B — executable short with point-in-time borrow truth | needs a provenance-bound historical borrow dataset | **no** (`POINT_IN_TIME_BORROW_SUPPORTED=False`; `require_executable` refuses) |
| C — liquid-ETF short under a frozen conservative assumption | executable economics under an explicit operator-frozen scope, fee, availability `ALWAYS_AVAILABLE_FOR_SCOPE_ASSUMED`, recall `NONE_ASSUMED`, rebate `ZERO` | only after the operator freezes the assumption |

Binding repo precedent already resolved: the old `research_assumed_shortable_universe_v1` model may at most yield a "development promising with
borrow-model limitation" label and is never promotion-ready; no result may be promoted from hypothetical availability.
**Recommended option (not selected by this session):** `EQUITY_HYPOTHESIS_ONLY_ETF_EXECUTABLE_FROZEN_ASSUMPTION` — individual equities
hypothesis-only; a frozen explicit ETF list (proposal in the JSON: DIA EEM EFA GLD IEF IWM QQQ SLV SPY TLT VTI XLB XLE XLF XLI XLK XLP XLU
XLV XLY, 20 symbols; the list is the operator's to confirm because the registry carries no instrument type) executable as class C with an
operator-supplied fee.

## 7. Identity, attempts, denominator, freeze

* `trial_id = "ac02-" + sha256(side, family, params, scope, universe_id, partitions_id, protocol_id)[:32]`; config id includes `side` so a
  mirror never collides with a Census-01 coordinate (collision refused, proven). Results, attempt ids, status and tags never enter identity.
* Hypothesis = family idea; trial = unique (config, symbol); attempt = invocation/retry (new attempt of the same trial; outcome-based retry
  forbidden); slice/job = window. Retries/windows never inflate trials.
* `assert_complete_ledger` / `register_edges`: the ledger must equal the frozen population (no missing, extra or duplicated trial) before any
  registry write — the denominator cannot shrink and winner-only registration is refused.
* **Freeze mechanism**: `require_freeze` allows real-data access only if `CENSUS02_PREDECLARATION.json` exists, is committed and clean at HEAD,
  has `status=FROZEN_BY_OPERATOR`, `attempts_at_freeze=0`, a structural protocol equal to the code's, complete valid operator decisions and a
  recomputed `protocol_id`. A proposal file can never satisfy it. Because the frozen decisions enter `protocol_id`, they enter every trial id.
* Multiple testing (proposal): local BH family over the 1,075 Census-02 factors (`factor_fdr_bh_v1`, alpha 0.10) with Census-01 totals
  (38,192 trials, 1,095 factors) disclosed beside every verdict; strategy DSR/PBO stays `DEFERRED_FULL_POPULATION` with the pooled population as
  denominator; complements counted in the registered denominator, excluded from the effective-independent estimate. Pooled-BH is an option.

## 8. Proposed funnel (no new threshold encoded)

`DISCOVERY → STATISTICAL_ROBUSTNESS → ECONOMIC_ROBUSTNESS → INDEPENDENT_CONFIRMATION → PORTFOLIO_RISK_SUITABILITY → PROMOTION → PAPER`.
Edge existence (stages 1–3) is separated from deployment suitability (stage 5). Pass-2 values are already accepted constants and may be reused
or relaxed only by an explicit operator choice; no old Pass-2 candidate is rescued. Options per item (also in the JSON):

| Item | Options |
|---|---|
| Trade-count confidence bands | reuse Pass-2 `>=30 trades` / report a confidence interval, not a gate / report-only |
| Year stability | reuse Pass-2 6-of-8 positive years + leave-best-year-out / report-only / operator value |
| Regime concentration | reuse Pass-2 share `<=0.80` / report-only / operator value |
| Parameter neighbourhood | reuse Pass-2 adjacent share `>=25%` / report-only / operator value |
| Portfolio MDD / worst-5-day | defer to the portfolio-suitability stage under the MAIN risk bar (recommended: not a stage-1 gate) / reuse Pass-2 20% & 6% / operator value |

## 9. Decisions still required from the operator (`c2_protocol.DECISIONS`)

1. `borrow_policy` (+ `etf_borrow_assumption`: explicit sorted ETF list and annual fee bps).
2. `grammar_tiers`: H / H+L (tier X, cross-sectional, is deferred and needs a later amendment).
3. `complement_handling`: register all and tag / exclude 56 complements before freeze.
4. `benchmark_rule`: net>0 AND short-hold alpha>0 (conservative) / net>0 only / short-hold alpha only.
5. `multiple_testing_denominator`: local / local with global disclosure / globally pooled.
6. `conditional_scope`: all 88 seed symbols / equities only (label evidence is borrow-independent).
7. `ssr_handling`: flag only / defer entry on a known SSR day.
8. `funnel_thresholds`: the five items above.

## 10. Second sweep

| Challenge | Disposition | Proof |
|---|---|---|
| Hidden long-only assumptions | Census-01 `simulate` bool cast = ALREADY CORRECT for its bool contract; Census-02 isolated = FIXED+PROVEN | hazard test + all builders emit signed int8; M15 |
| Quantity sign / short cash P&L | FIXED+PROVEN | scalar-reference equality incl. flips and borrow; hand arithmetic; M01, M21, M22 |
| Benchmark sign | FIXED+PROVEN | constant short vs short-hold alpha 0; long-hold would fabricate; M13 |
| Cover / same-bar chronology | FIXED+PROVEN | fill bar = decision+1; future bars cannot change the cover; all 470 configs causal-prefix invariant at 3 cuts; M01, M02 |
| `fwd_ret` as P&L | FIXED+PROVEN | float and integer-cast labels refused; M03 |
| Borrow assumptions | FIXED+PROVEN | no assumption/outside scope => hypothesis-only, no P&L; class B refused; missing fee refused; M08–M10, M14, M23 |
| Identity collisions / duplicates | FIXED+PROVEN | `side` in id, forced-collision refusal, tags non-identity; M04, M05, M24 |
| Complement duplicates | FIXED+PROVEN as tagging; policy BLOCKED on operator | disjoint+exhaustive proof |
| Local vs global multiple testing | BLOCKED on operator decision (proposal given) | — |
| Result / reserve leakage | FIXED+PROVEN | fence on 2024/2025/2026 rows and empty input; no data path in any `c2_*` module; M07 |
| Vacuous causality proof (mqd-test-proof finding: on plain noise SH08 never fired and SH13 fired in 6/24 configs, so a lookahead mutation stayed green) | FIXED+PROVEN | gappy/volatility-burst fixture, per-family non-vacuity test, M25 RED |
| Retry / resume | identity + ledger level FIXED+PROVEN; runner/resume machinery not yet built (post-freeze reuse of Census-01 store) | retry test; M06 |
| Denominator shrink / winner-only | FIXED+PROVEN | M11, M12 |
| Stale Census-01 executable paths | ALREADY CORRECT+PROVEN | Census-01 grammar authority refuses C02 configs; no C02 family in its builders; C02 refuses S-family ids |
| Corporate actions / delisting for shorts | BLOCKED by data: inherited adjusted-series + typed-exclusion contract, M&A/recall/delisting unobservable | disclosed limit |
| Survivorship | BLOCKED (no point-in-time universe); mandatory label | disclosed limit |
| Margin / buying power / squeezes | out of scope: per-cell fixed notional; belongs to the portfolio-suitability stage | — |

## 11. Not done / deferred (not required by this gate)

Census-02 conditional runner (FactorSpec registration with `lower_is_better`, label fence), strategy chunk runner, cross-sectional and
beta-hedged simulators, population/universe manifests (need data dispositions), `CENSUS02_PREDECLARATION.json` itself. The freeze package (C3)
is intentionally **not** committed: it requires the operator decisions above.

Full local workspace acceptance: NOT RUN — prohibited by laptop resource-safety rule; broad workspace proof delegated to GitHub CI.
