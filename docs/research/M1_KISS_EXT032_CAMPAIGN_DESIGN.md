# M1 KISS campaign `M1-KISS-EXT032-ETF-01`: authorization record and frozen design

Mission `V4-M1-KISS-CALENDAR-CAMPAIGN-PREDECLARATION-AND-ENGINE-01` · baseline `a49cae1f9a2849c769707d239a61e10cb0f39903` (= origin/main; GitHub CI run #637, all six jobs success).
Local commits only, **not pushed**. Engineering and prospective predeclaration only: no economic attempt, no trial registered in a real registry, no Final-Holdout read, no Promotion, no Paper/Live action.

Status target: `LOCALLY_COMPLETE`, research execution **not authorized** (`OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED`).

## 1. Operator economic-policy decisions (recorded verbatim in effect)

| ID | Decision | Consequence in this campaign |
|---|---|---|
| OD-8 | Keep the existing promotion and benchmark policy | `capital_fraction_matched_passive_buy_hold_v1`; profitable months >= 0.40; Sharpe >= 0.5, CAGR >= 0, max drawdown <= 25 %, profit factor >= 1.05; DSR >= 0.5; PBO <= 0.5; cost-aware alpha >= 0; existing robustness, scanner, execution-fidelity and provenance gates; a zero-signal fold is non-evaluable. No new benchmark, exposure denominator, adjusted gate or relaxed threshold. EXT-169 stays cataloged and is excluded from the promotable population (its calendar-only profitable-months bound cannot reach 0.40 over the long window); its historical analysis is not re-run. |
| OD-1 / OD-7 | Evidence grade `EXPOSED_DEVELOPMENT` | The historical window is declared-exposed development, never independent confirmation. A positive result is never labelled `INDEPENDENTLY_CONFIRMED`. The Final Holdout stays unavailable. If Promotion requires evidence this path cannot provide, the unmet gate is reported `BLOCKED`; no OOS evidence is fabricated, Gate 3b is not overridden and no alternative promotion route is created. |
| OD-2 | Instruments | SPY, QQQ, IWM, DIA; US-listed equity ETFs; long/flat; canonical daily; accepted instrument-registry membership; no substitution, expansion or performance-dependent exclusion. |
| OD-4 | Multiple-testing accounting | Cumulative native-trial history on the same exposed window is preserved and disclosed; the new population is four trials; no results-based denominator reduction. Inventory and the pooled-statistics finding: `M1_KISS_EXT032_SEARCH_ACCOUNTING.md`. |
| OD-6 | Versioned US-equity calendar | New contract `us_equity_regular_sessions_v2`; v1 is preserved byte-for-byte. Design in section 3. |
| OD-3 | Final Holdout | `RESERVED_UNCONSUMED`; no read, trial, query or economic calculation of that window. |
| OD-5 | Stress | Existing Batch 03 contract: `half_exposure_capital_fraction_500bps_v1`, 500 bps allocation, 15 bps slippage, 10 bps volatility multiplier, 4000 bps drawdown ceiling. An evaluation scenario of the registered trial, never a trial. Baseline sizing `fixed_initial_capital_fraction_v1`, 1000 bps of immutable USD 100,000, whole shares, no compounding. |

## 2. EXT-032 frozen economic semantics

Catalog row (hash-verified workbook, committed ledger): *Pre-Holiday Effect*, long a broad equity ETF "during two trading days before holidays", "exit at holiday window end", holding horizon 1-2 days, "use exchange holiday calendar", known bias risk "small sample, holiday definition drift". The short side is a derived negative control and is **not** part of this identity.

Native identity: `pre_holiday_two_session_long_v1` (long/flat, canonical daily, one instrument, stateless).

**Holiday event.** A weekday on which the regular session does not occur and which the v2 calendar classifies `SCHEDULED` (a closure derivable from the published exchange observation rules and the civil date alone). Weekends are not holiday events. Unscheduled closures (national mourning) are never holiday events. A Saturday New Year's Day that the exchange does not observe is not an event.

**Window.** The two scheduled-regular sessions immediately preceding a holiday event: `P2 < P1 < H`, `P1` = last scheduled session before `H`, `P2` = the session before `P1`. Early-close sessions are sessions. Consecutive or overlapping events produce the union of their windows (one continuous position, no flat-then-rebuy).

**Decision rule.** At the completed daily bar of session `t`, let `N(t)` be the next *scheduled* session. Target is `+1` iff `N(t)` is in a window, otherwise `0`. The rule reads no price, no volume and no bar other than the latest two; unscheduled closures cannot create, cancel or move a signal.

**Causal timeline (worked example: Thanksgiving 2024-11-28).**

| Step | Session | Event |
|---|---|---|
| Decision (entry signal) | `P3` = Mon 2024-11-25, bar completed | `N` = Tue 11-26 = `P2` -> target long; capital-fraction wrapper resolves `Q = floor(USD 10,000 / close(P3))` |
| Entry fill | Tue 11-26 (`P2`), next permitted bar | conservative next-bar fill (bar high plus slippage); never the signal bar |
| Intended first exposed session | `P2` | the `P3` close to `P2` close return is **not** capturable: a signal cannot fill on its own bar |
| Hold decision | Tue 11-26 close | `N` = Wed 11-27 = `P1` -> target long (no re-entry, no resize) |
| Intended last exposed session | Wed 11-27 (`P1`) | position carried through the holiday gap |
| Exit signal | Wed 11-27 bar completed | `N` = Fri 11-29, not in a window -> target flat |
| Exit fill | Fri 11-29, first session after the holiday | conservative next-bar fill (bar low minus slippage) |

Unavoidable exposure: the overnight gaps and the holiday gap between the `P1` close and the exit fill, plus the reopening session until the exit fill. Calendar-window membership is never represented as a position or fill; only the backtest/runtime fill makes a position.

**Edge behaviour (each pinned by a test).**
* Incomplete latest bar, non-midnight or weekend/holiday label, out-of-coverage session, or a lookahead that leaves coverage: flat.
* The two latest bars are not consecutive actual sessions (missing or repeated bar, stale input): flat. A degraded input never establishes or extends a position.
* Evaluation-fold ends (`force_flat_last_bar`) and the 2016-03-01 / 2026-03-01 boundaries: no window straddles a 1 March boundary in 2016-2028, so no event is truncated by a fold or by the holdout cut.
* Restart: stateless; target is a pure function of the latest two bars. Continuous execution equals restart-at-every-boundary. The capital-fraction wrapper's durable held-sizing record is unchanged and supplies the entry quantity after restart.
* No stop, trailing stop, offset, alternate window length, indicator, grid, short side or filter exists in this identity.

## 3. Calendar contract `us_equity_regular_sessions_v2`

* Own contract id, version and deterministic content string/sha256; coverage `2016-01-01..2028-12-31`.
* Closure table = v1 closures (2016-2026) plus the Paper-runtime table for 2027-2028; the overlap 2023-2026 is reconciled by tests that fail on any contradiction between v1, v2 and the Paper-runtime table.
* Each closure is classified `SCHEDULED` (rule-derived; an independent test derives them from the observation rules) or `UNSCHEDULED` (2018-12-05, 2025-01-09). Early closes are recorded separately; they are sessions.
* As-of knowledge: scheduled closures are knowable at any decision date; an unscheduled closure is treated as knowable only from its own date (announcement dates are deliberately not recorded; the policy is conservative). Native decision rules consume scheduled structure only.
* Out-of-coverage dates are typed refusals, never weekday extrapolation.
* v1 (`us_equity_regular_sessions_v1`, hash `3249ee51...a76de`), its coverage and every registered fingerprint are untouched. No strategy is silently remapped; a strategy bound to v2 carries v2 identity and a distinct fingerprint.

## 4. Deferred stop-loss ideas (not executed, not trial variants)

Possible later, separately declared hypotheses (each changes economic identity and needs its own prospective declaration and honest accounting): fixed percentage stop; ATR stop; ATR trailing stop. This campaign uses the accepted execution, risk and halt controls unchanged. A stop does not create positive expectancy and does not bound realized loss (gaps, next-bar execution).

## 5. Campaign shape

`M1-KISS-EXT032-ETF-01`: 1 hypothesis, 4 trials (EXT-032 x SPY, QQQ, IWM, DIA), 0 attempts, `execution_gate.executable = false`, blocker `OPERATOR_EXECUTION_AUTHORIZATION_REQUIRED`. Machine-readable authority: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_KISS_EXT032_ETF_01.json`.
