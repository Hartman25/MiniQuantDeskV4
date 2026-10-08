# M1 candidate supply, OOS design and Paper readiness — consolidated closure

Mission `V4-M1-CANDIDATE-SUPPLY-OOS-DESIGN-AND-PAPER-READINESS-CONSOLIDATED-01` · 2026-10-08 · baseline `e1fba7adfac6850241474156e75439a03e9e92f8` (= origin/main; GitHub CI run 636 on that exact SHA: success).
Local commits, **not pushed**. No economic evaluation, trial, attempt, provider call, OOS/Confirmation/Final-Holdout read, Promotion, Paper or Live action happened. No frozen result, population or threshold changed.

**VERDICT: `CATALOG_INTAKE_BLOCKED_WORKBOOK_NOT_SUPPLIED` · `ALL_CATALOG_INDEPENDENT_WORK_COMPLETE` · `M1_BLOCKED` (unchanged).** The workbook `MQD_External_Strategy_Idea_Catalog_2026-10-07.xlsx` (expected SHA-256 `6fc945a873733cda6a1552049923a153ed2f7213c488c6d5f07ffeb8077a37f3`) is not present in this environment (whole-filesystem search for `*.xlsx` and the file name; nothing). No row was fabricated, no row text is quoted from memory, and the deduplication, eligibility matrix, bounded population and the catalog part of the predeclaration are **BLOCKED**, not guessed. Everything that does not need the 200 rows is done and proven below.

## 1. Tool usage (actual)

Installed MQD skills read: `mqd-test-proof`, `mqd-diagnose`, `mqd-review-patch`, `mqd-handoff`, `mqd-external-research`, `archify` (listing; `mqd-test-proof` false-positive discipline applied to the intake tests and the pin test). `mqk_readonly`, Srclight and Graft MCP servers: **not available** in this cloud session (no such tools were offered); discovery used restricted Grep/Read and focused `cargo`/`pytest`. No subagents, Context7, Firecrawl or Playwright. Real Paper DB: not reachable, not touched. No Postgres cluster was started (no DB-backed code changed).

## 2. Catalog intake: what exists, what is blocked, and the pre-frozen rulebook

**Done and proven (`research-py/experiments/external_idea_intake/`).** `intake.py` freezes the workbook as bytes: the SHA-256 is compared **before** any parse; then sheet set and order (`Strategy_Catalog, Summary, Source_Index, Asset_Test_Matrix, Testing_Guardrails, README`), exactly 200 rows, 26 unique non-empty columns, exactly one column of `EXT-nnn` ids, ids unique/contiguous/ordered `EXT-001..EXT-200`. It keeps every original cell string verbatim and separate from any later normalization, refuses formula cells, unknown cell types and oversized package members, uses only the standard library, evaluates nothing, calls no provider, and emits a canonical ledger marked `UNTRUSTED_IDEA_INTAKE` with `trial_registered=false`, `economic_attempt=false`. 24 intake tests pass (1 conditional skip: the committed-workbook test, skipped only because the file is absent); 9 mutants killed with byte-identical restore (hash check removed, formula allowed, contiguity dropped, sheet order ignored, id-column guess, text normalization, row count unchecked, size bound removed, ledger claiming registration). Fixtures are synthetic and say nothing about the real catalog.

**To run when the file is supplied** (one operator action: place the original at `docs/research/intake/MQD_External_Strategy_Idea_Catalog_2026-10-07.xlsx`, ~66 KB): `python research-py/experiments/external_idea_intake/intake.py <workbook> --out docs/research/intake/`. A different byte sequence under the same name is refused by the hash.

**Pre-frozen deduplication rulebook (fixed before any row is read, so no row can bend it).**

Canonical spec per row (no inference; a missing element is recorded as missing): asset class, instrument universe, direction (`long`, `short`, `mirrored`, `hedged`, `long_short`), signal inputs, transforms with lookbacks, comparators, thresholds, entry rule, exit/hold rule, cadence, sizing, data requirements, state class (`bounded_history` vs `durable_state`), unresolved assumptions. A row without a computable rule is `INSUFFICIENTLY_SPECIFIED`; the reviewer never completes it.

| Novelty class | Rule (applied in this precedence; first match is primary, the rest listed as secondary relations) |
|---|---|
| `EXACT_DUPLICATE` | canonical spec equals an existing identity including every parameter |
| `PARAMETER_VARIANT` | same structure, only numeric parameters differ; **never a new hypothesis and never a trial by itself** |
| `MIRROR` | same signal, sign-flipped direction (identity-bearing; explicit `mirror_of`) |
| `COMPLEMENT` | the opposite state of an existing identity (for example short/flat when the other is long/flat; explicit link) |
| `COMPOSITE_OF_EXISTING` | conjunction, vote or sequence of two or more existing identities' signals |
| `SEMANTIC_VARIANT` | same economic mechanism, a materially different structural rule (extra filter, different exit, different input); needs a written difference |
| `GENUINELY_NEW` | no existing identity shares the mechanism |
| `UNKNOWN_NEEDS_REVIEW` | spec ambiguous; carries a written reason; never silently dropped |

Feasibility is a separate field: `M1_READY_SINGLE_SYMBOL_LONG_FLAT_DAILY_OHLCV`, `NEEDS_ADDITIONAL_AUTHORITATIVE_DATA`, `NEEDS_MULTI_SYMBOL_PORTFOLIO_ENGINE`, `NEEDS_SHORT_OR_BORROW_AUTHORITY`, `NEEDS_FUTURE_ASSET_SUPPORT`, `NEEDS_ML_OR_ALT_DATA_FRAMEWORK`, `INSUFFICIENTLY_SPECIFIED`. Result values, claimed returns, source marketing and the catalog's own `MQD_Priority`/`Implementation_Readiness`/`Source_Fidelity` never enter either field or any identity. EXT-023 and EXT-041 are flagged by the catalog itself as source-page mismatches and are `INSUFFICIENTLY_SPECIFIED` until the underlying article is read.

**Prospective admission rule for the bounded population** (a rule, not a selection): a row is admitted iff (a) feasibility = M1-ready, (b) the rule is fully specified in the source text with no guessed parameter, (c) novelty is `GENUINELY_NEW` or `SEMANTIC_VARIANT` with a written material difference, (d) no data beyond completed daily OHLCV and the session calendar, (e) direction long or long/flat. Mirrors, complements and hedged forms are excluded (short opens are blocked in Paper, B5). If more rows qualify than the cap K (OD-4), keep one representative per mechanism family and break ties by ascending `EXT-nnn`; never by returns, popularity or any score. Trials = admitted hypotheses x instruments (OD-2).

**Capacity prerequisite for implementation.** `REGISTERED_STRATEGY_IDS` has 24 identities and `MAX_STRATEGY_UNIVERSE = 24`; a daemon drift test pins equality, so the first new engine must raise both in the same commit (done in that controller, not pre-emptively here).

## 3. Native strategy semantic inventory (the comparison base for every future row)

| Identity | Rule (long/flat unless noted) | State class | Calendar-bound | Campaign / disposition |
|---|---|---|---|---|
| `trend_sma50` | close > SMA50 | bounded | no | campaigns 01-03: REJECTED |
| `dual_sma_50_200_trend` | SMA50 > SMA200 | bounded | no | dual-SMA campaign: REJECTED |
| `pullback_mean_reversion_20_2` | enter close <= mean20 - 2 population sd; exit when close returns to the trailing mean | durable | no | pullback campaign: REJECTED |
| `absolute_momentum_252` (B01 H1) | close > close 252 bars earlier | bounded | no | Batch 01: REJECTED |
| `near_high_momentum_252_3pct` (B01 H2) | close >= 97% of the 252-bar high close and close > mean of the latest 50 closes | bounded | no | REJECTED |
| `trend_pullback_5d_4pct_hold5` (B01 H3) | close > SMA200 and close <= 96% of the close 5 bars earlier (entry event); long while an entry event occurred on any of the last 5 bars | durable | no | REJECTED |
| `turn_of_month_last1_first3` (B02 H1) | last session of month and first three of next | bounded | **yes** | REJECTED |
| `halloween_nov_apr` (B02 H2) | long Nov-Apr, flat May-Oct | bounded | **yes** | REJECTED |
| `trading_range_breakout_50d_hold10` (B02 H3) | close > prior 50-bar max close; hold 10 | durable | no | REJECTED |
| `monthly_multihorizon_abs_momentum_consensus_v1` (F01) | month-end: at least 2 of 3 of close > close 21/63/252 sessions earlier | bounded | **yes** | Batch 03 advanced mechanically; not eligible |
| `trend_filtered_rsi5_reversion_v1` (F02) | close > SMA200; enter RSI5 < 30, exit > 70 or trend lost | durable | no | non-evaluable majority |
| `trend_filtered_extreme_3d_atr_reversal_v1` (F03) | trend; 3-day drop > 1.5 x prior ATR fraction; hold 5 | durable | no | non-evaluable majority |
| `close_channel_100_50_trend_v1` (F04) | close > prior-100 high close; exit < prior-50 low close | durable | no | advanced mechanically; not eligible |
| `monthly_10month_trend_timing_v1` (F05) | month-end: close > mean of prior 10 month-end closes | bounded | **yes** | advanced mechanically; not eligible |
| `trend_filtered_zscore20_reversion_v1` (F06) | trend; z20 < -2 enter; exit at mean or trend lost | durable | no | not advanced |
| `volatility_contraction_breakout_v1` (F07) | range10 x 4 < range60 and close > prior-20 high; exit < prior-10 low | durable | no | not advanced |
| `monthly_12_minus_1_abs_momentum_v1` (F08) | month-end: close[t-21] > close[t-252] | bounded | **yes** | not advanced |
| `delayed_overnight_gap_reversal_v1` (F09) | gap > 1.5 x prior ATR fraction; hold 3 | durable | no | not advanced |
| `monthly_52week_high_proximity_v1` (F10) | month-end: close >= 95% of 252-session high | bounded | **yes** | not advanced |
| `swing_momentum`, `mean_reversion`, `volatility_breakout` | legacy built-in daily engines (trailing-average, deviation, prior-window min/max); no Research declaration names them except one `mean_reversion` mention | bounded | no | legacy registry engines; not candidates |
| `intraday_scalper`, `intraday_short_scalper` | intraday 5m; short variant opt-in | durable | no | deployed legacy (AAPL/5m), no Research trial; not a candidate |

Preserved-rejected research families that count as comparison base though they have no native engine: Alpha Census-01 grammar, Census-02 (Strategy SH01-SH13 and LS01-LS04; 1,075 conditional factors), Confirmation-01, Pass-2, Discovery_01 low-volatility, SHORT_01 and SHORT_WAVE_02, Wave06. Exposure windows: §5.

## 4. Research-to-native readiness classification

Classes: **EXISTING_EXACT_ENGINE**, **EXISTING_ENGINE_BEHAVIOR_IDENTICAL_CONFIG**, **NEW_ENGINE_NEEDED**, **MISSING_AUTHORITATIVE_DATA**, **REQUIRES_NEW_POLICY**, **NOT_EXECUTABLE_IN_M1**. A row can be EXISTING_* only if the 24-identity table above reproduces its actual economics; a `PARAMETER_VARIANT` of an existing engine is not a configuration of it, because every engine's fingerprint binds its constants and there is no parameter surface (a parameter change is a new engine and a new identity). New engines must follow the Batch 03 pattern: exact integer (i128) arithmetic, an independent integer reference test, a state class, restart proof for `durable` engines, registration in `REGISTERED_STRATEGY_IDS`, and no Research trial, attempt or Promotion authority from the engine commit itself.

Preliminary reading of the five operator-listed ids, from the structural notes recorded in the authority doc (§16 there), **not** reviewed against row text and not selections: EXT-032 pre-holiday: NEW_ENGINE_NEEDED and calendar-bound (the holiday set comes from the session calendar, so it inherits the 2026-12-31 horizon, E1/OD-6); EXT-024 VIX percentile: MISSING_AUTHORITATIVE_DATA (no VIX provenance contract) and its short leg blocked; EXT-045 and EXT-070 style/sector rotation: NOT_EXECUTABLE_IN_M1 (no multi-symbol native engine; nine of eleven SPDR sector ETFs already exposed in Census-02 and Confirmation-01); EXT-141 breakout + trend + volume: NEW_ENGINE_NEEDED, stateful, adjacent to F04/F07/`trading_range_breakout_50d_hold10`.

## 5. OOS and data-consumption authority: re-verified, with the design comparison

Re-verification: the partition truth (`c2_protocol.partition_truth`, pinned by `test_alpha_edge_census_02_partition_truth.py`), the `PREDECLARED_*` partitions and the Batch 03 holdout guard were re-read; no discrepancy with the authority matrix in `M1_POST_DISCOVERY_CANDIDATE_AUTHORITY.md` §4 was found, and the full m1_native_trend_campaign pytest directory passes (382 passed with the intake tests, 1 conditional skip). Exposed: `[2016-01-01, 2024-01-01)` Discovery; `[2024-01-01, 2025-01-01)` contaminated; `[2025-01-01, 2026-03-01)` consumed by Confirmation-01 and scored by native folds 9-10; Final Holdout `[2026-03-01, ...)` `RESERVED_UNCONSUMED`, single-use. Recorded gap unchanged: there is no pipeline-spanning consumption ledger; consumption is procedural.

| Design | Data actually available (committed evidence) | Provider / provenance authority | PIT universe | Chronological isolation | Statistical dependence | Leakage risk | Denominator | Candidate / attempt identity | Remaining operator decision |
|---|---|---|---|---|---|---|---|---|---|
| A. Exposed-window development | 2016-2026-02 Alpaca SIP daily, adjustment `all`, in the frozen run data | Alpaca SIP (accepted research provider) | ETF membership only; listing dates bound start | none: every bar already seen by earlier campaigns | very high with all prior ETF work | author knowledge of the window is total | must include prior native trials on the window (about 55 + 60) to be honest | normal trial/attempt rules | OD-4; grade must be declared `EXPOSED_DEVELOPMENT`, never OOS |
| B. Instrument-disjoint replication | only symbols outside every prior universe; MDY has no registry row or history metadata; XBI missing from the registry (supplement only) | Alpaca SIP, but coverage unverified without a provider call (not authorized) | registry identity rows required | same dates as Discovery | correlated ETFs (same-date) | moderate | its own N plus disclosure | normal | OD-1(b), OD-2; evidence grade `INSTRUMENT_DISJOINT_REPLICATION` (weaker) |
| C. Backward-temporal (pre-2016) | **none verified**: committed Tiingo files cover 10 symbols with unverified adjustment semantics and are not the research provider authority; Alpaca pre-2016 coverage is unverified | needs a provenance-authorized provider contract first | needs a point-in-time listing set (many ETFs did not exist) | genuinely before every recorded read (no ledger records a pre-2016 read) | different regimes; author knows the history | moderate (author knowledge only) | its own N | normal; an exposure-ledger entry is written **before** the first read | OD-1(d), provider authority, and a data-availability probe that is itself a (separate) provider action |
| D. Future forward data | accrues only after a deployment | the Paper runtime itself | n/a | strongest | n/a | none | n/a | this is M1.9/M1.10 | needs an eligible candidate first |
| E. Final Holdout `[2026-03-01, ...)` | bars to 2026-09-01 cached but never evaluated | Alpaca SIP | ETF membership | genuine, **single-use** | highest value, one shot | consumed on first read; never fresh again | the judge's N | one frozen candidate, one use, ledger-recorded | OD-3 (explicit authorization) |

No design is selected here. The minimum recommended policy package is in §6.

## 6. Operator decisions OD-1..OD-8: reduced

Existing accepted defaults are the frozen Batch 03 / Native Sizing v1 policy; none of the recommendations below is authorization.

| OD | Existing default | Unresolved question | Options (authority boundary preserved) | Recommended | Needed for the next population? |
|---|---|---|---|---|---|
| OD-1 Candidate bar and OOS regime | none (only the Promotion gates exist; no Confirmation stage in code) | Must the first candidate show independent alpha, or is it a pipeline-validation candidate that still clears every gate? Which regime provides independence? | (a) one-shot Final Holdout for one frozen candidate; (b) instrument-disjoint replication; (c) forward Paper as the only independent evidence; (d) backward-temporal on pre-2016 data | **(c) plus an explicit label**: the candidate is "pipeline-validation, Promotion gates only; independence comes from forward Paper (M1.10)". It spends no reserved data, creates no unprovable OOS claim, and uses the exposed window only as declared-exposed development. Tradeoff: weaker pre-deployment evidence. (a) is the strongest pre-deployment option but is irreversible | **Yes** |
| OD-2 Population | six prepared symbols (five exposed, MDY unverified), no silent substitution | Which instruments? | keep six; substitute with a written list; MDY-only; a bounded high-liquidity ETF set from the existing registry | every ETF already used by Batches 01-03 is exposed anyway, so choose by liquidity and registry identity, not performance; XBI and MDY need registry identity rows first | **Yes** (fixes N) |
| OD-3 Holdout use | refuse (reserved) | Spend the Final Holdout? | refuse; authorize one frozen candidate | **refuse for now**: decide only after a frozen candidate exists; with OD-1(c) it is not needed | No (gated by OD-1) |
| OD-4 Denominator and cap K | each experiment judges its own N; no cross-batch pooling exists | What denominator does the new judge use? Cap K? | own N only; own N + 60 Discovery trials; cumulative native trials on the window (about 55 + 60 + own N) | **cumulative on the window** if the window is exposed (most honest for DSR); K small (suggest at most 8 hypotheses x instruments of OD-2) to keep N_eff meaningful | **Yes** |
| OD-5 Stress contract | Batch 03 frozen contract: `half_exposure_capital_fraction_500bps_v1`, slippage 15, vol 10, drawdown ceiling 4000 bps, baseline 1000 bps / 5 / 0 | Reuse? | reuse unchanged; define a new one | **reuse unchanged** (already accepted, strictly adverse, no new choice) | No if reused |
| OD-6 Calendar migration | monthly engines fail closed flat from 2026-12-31 | Authorize a new session-calendar contract (2023-2028)? | authorize; or exclude monthly-engine and calendar-event hypotheses (EXT-032-style) | **exclude calendar-bound hypotheses from the next population** unless one is wanted; then OD-6 is unnecessary and the 2026-12-31 horizon stays a recorded, fail-closed limit | No (conditional) |
| OD-7 Outcome rule | none | What is "confirmed" for any independent stage? | Promotion gates only (DSR >= 0.5, PBO <= 0.5, Sharpe >= 0.5, CAGR >= 0, profitable months >= 40%, cost-aware alpha >= 0, paper_candidate) plus forward Paper; or add a Confirmation threshold | **Promotion gates only plus forward Paper** under OD-1(c); no new threshold invented | **Yes** (can be answered together with OD-1) |
| OD-8 Benchmark and no-trade semantics | cost-aware alpha vs `capital_fraction_matched_passive_buy_hold_v1`; a zero-signal fold is non-evaluable | Keep? | keep; add a prospective risk-adjusted secondary diagnostic | **keep** for the new population. Changing criteria after 0/60 positive alpha is a result-informed policy change; if the operator wants a risk-adjusted rule it must be declared before any new trial and disclosed as such | No if kept |

**Minimum decision set for the next controller: OD-1 (with OD-7 answered alongside), OD-2, OD-4.** OD-3, OD-5, OD-6, OD-8 take their existing defaults and need no answer. Previously tested families: unchanged (rejected stay rejected; F04/F05/F01 stay mechanically advanced but not eligible). Novelty and OOS integrity: with OD-1(c) no reserved or fresh data is consumed and no independence claim is made before forward Paper.

**Expectation, stated plainly.** Promotion requires non-negative cost-aware alpha against matched passive buy-and-hold, and 0 of 60 Batch 03 trials met it; long/flat timing in a drifting window rarely does. A new population under the same gate may also yield no candidate. OD-8 is the only lever, and the only legitimate time to change it is before any new trial exists.

## 7. Non-executable research predeclaration (draft; no JSON file, so nothing machine-loads it)

`NOT_AUTHORIZED` · `NOT_EXECUTABLE` · no registered trial · no economic attempt · no OOS consumption · `execution_gate.executable = false`.

| Field | Status |
|---|---|
| Hypothesis population and admission rule | rule **frozen** in §2; population **BLOCKED** (needs the 200 rows) |
| Instrument eligibility | undecided: OD-2 (registry identity rows required; SIP daily; ETFs only; no single stocks) |
| Direction / timeframe | frozen: long/flat; canonical daily `1D` (`data.timeframe_identity = canonical_semantic_v1`) |
| Rule parameters | from the source text only; none guessed; a missing parameter excludes the row |
| Sizing | frozen: `fixed_initial_capital_fraction_v1`, 1000 bps of immutable USD 100,000, whole shares, no compounding |
| Benchmark | frozen: `capital_fraction_matched_passive_buy_hold_v1` (OD-8 default) |
| Execution and costs | frozen: completed-bar signal, next-bar fill, baseline slippage 5 bps, volatility 0 (no same-bar fills) |
| Statistical denominator | undecided: OD-4 |
| Stress | frozen default: Batch 03 contract (OD-5 default), registered per trial before its first attempt |
| Failure / non-evaluable | frozen: non-evaluable stays in the population and counts worst; no retry adds a trial |
| Partitions | undecided: OD-1 (declared evidence grade, window, consumption entry before first read) |
| Robustness | frozen: existing gauntlet and gates, thresholds unchanged |
| Promotion eligibility | frozen: §3 gate list of the authority doc plus exact fingerprint and registered stress contract |
| Outcome rule | undecided: OD-7 |

Once OD-1/2/4 are answered and the workbook is frozen and deduplicated, this table plus the intake ledger is the whole basis of the next controller; no broad discovery audit is needed.

## 8. M1.9 and M1.10

M1.9: the ordered read-only preflight is `docs/runbooks/m1_9_paper_deployment_preflight.md` (every item maps to an existing seam; migration 0091 present and last in the manifest, governance guard OK; real Paper DB was at 76 and was not touched; Gate 3b exact `active_paper`; legacy strategies inactive; native calendar horizon and versioned fingerprint limits recorded).

M1.10: `soak_ledger` re-read: 10 countable / 5 consecutive clean, calendar 2023-2028, SHA, deployment identity, `active_paper`, completed outcome, idempotent duplicates and conflicting duplicates, regular-session gaps all enforced; `cargo test -p mqk-integrity` passes. **E4 (automated evidence producer): not added, with evidence.** A producer must bind every fact to durable authority. Bindable: market date, `runs.git_hash`, daily-operation state/outcome/run_id, promotion-transition history, `approved_for_live` (DB-constrained false in dynamic-selection evidence). **Not durably recorded anywhere**: an unattended supervision gap and a failed evidence write (no migration or DB module mentions either), and the 4-tuple deployment identity is stored only as a hash (`runtime_binding_identity`). A producer would therefore have to accept operator attestations for the invalidators, which is the manual capture the runbook already defines; adding it would not remove a manual step. DEFERRED, not required by the documented gate.

## 9. Defect census and second sweep

| Finding | Disposition | Evidence |
|---|---|---|
| Workbook not supplied | BLOCKED (operator action) | filesystem search; intake tool ready and proven |
| Intake had no frozen, hash-first, fail-closed seam | FIXED+PROVEN (`91b3e9b`) | 24 tests, 9 killed mutants |
| Readiness claims (engine inventory, universe bound, migration head, ledger gate, catalog hash, runbook non-authorization) could go stale unnoticed | FIXED+PROVEN | `test_consolidated_readiness_pins.py` (7 tests parse the Rust sources and the manifest); 6 mutants killed with byte-identical restore |
| TODO/FIXME/`unimplemented!` in promotion, integrity, runtime, strategy, backtest and research source | ALREADY_CORRECT+PROVEN | grep: none (only the documented `NotImplementedError` futures/options stubs) |
| Migration chain / manifest / eol | ALREADY_CORRECT+PROVEN | `check_migration_governance.sh` OK; 0091 last |
| `#[ignore]` load-bearing proofs outside the DB lane | ALREADY_CORRECT+PROVEN | `check_ignored_load_bearing_proofs.sh` OK |
| `soak_ledger` chronology / gap / SHA / identity logic | ALREADY_CORRECT+PROVEN | re-read; 80+ tests in `mqk-integrity` pass |
| `MAX_STRATEGY_UNIVERSE` equals registry count (24) | ALREADY_CORRECT; capacity item for the first new engine | drift test; §2 |
| E1 monthly-engine calendar horizon | BLOCKED (OD-6) | unchanged; avoidable by excluding calendar-bound hypotheses |
| E4 evidence producer | DEFERRED with evidence | §8 |
| Parameter mining via catalog variants | ALREADY_CORRECT+PROVEN (by rule) | `PARAMETER_VARIANT` is never a trial; engines have no parameter surface; admission is rule-based |
| Winner-only registration; post-result criteria change | ALREADY_CORRECT (no registration exists; OD-8 warning recorded) | no `PREDECLARED_*` names an `EXT-` id (pinned) |
| Reserved data read | NO | no data read in this controller |

No ordinary unresolved in-scope deterministic defect remains. Full local workspace acceptance: NOT RUN — prohibited by laptop resource-safety rule; broad workspace proof delegated to GitHub CI.

## 10. Exact remaining path

1. Operator: place the original workbook at `docs/research/intake/` (hash must equal `6fc945a8...a37f3`).
2. Operator: answer OD-1 (with OD-7), OD-2, OD-4 (§6); every other OD takes its default.
3. **Next executable controller: `V4-M1-EXTERNAL-IDEA-INTAKE-DEDUP-AND-PREDECLARATION-01`** — run `intake.py`, commit the ledger, classify all 200 rows with the §2 rulebook (one primary disposition each, relations with evidence), emit the bounded population by the §2 admission rule, and complete the §7 table as a non-executable predeclaration. Minimum approval to start: the workbook placement plus OD-1/OD-2/OD-4.
4. Then: operator re-issues an executable declaration; new engines (with the universe-bound bump); register N and prove N/0; execute once; batch judge and scanner review; Promotion route with exact fingerprint and stress contract; `active_paper`; M1.9 deployment per the preflight runbook; 10 countable / 5 consecutive clean sessions.
