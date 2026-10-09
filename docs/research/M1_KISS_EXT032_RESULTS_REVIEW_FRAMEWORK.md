# M1 KISS campaign: results and near-miss review framework

Tool: `research-py/experiments/m1_native_trend_campaign/near_miss_review.py` (schema `m1_near_miss_review_v2`; v1 was replaced before any execution, see section 7). Tests: `test_kiss_ext032_near_miss_review.py`. Prepared before any execution; there are no results, and the tool refuses the non-executable declaration. It is read-only, deterministic, and selects nothing.

## 1. What it does

For `M1-KISS-EXT032-ETF-01` it reviews **every slot** (4), in the declaration's order, whether the trial succeeded, failed, was excluded by the judge or has no evidence at all (a missing slot is listed as `MISSING_EVIDENCE`, never dropped). Inputs: the evidence table `batch_results.json` (written by `summarize_batch.py`), the batch judge, a read-only extract of the registry, the bars provenance manifest, the post-run holdout-guard report, and, where they exist, canonical Promotion reports. It writes nothing into the run directory or the registry (an `--out` inside the run is refused) and makes no network call. Building a report opens only the committed frozen declarations and the incident ledger (the accounting is recomputed from them; audit-hook test).

Per trial: slot identity, the recomputed expected trial id, evidence grade, data partition, evaluability, every authoritative gate with unit / comparator / threshold / evidence source / basis / value / signed margin / shortfall, the execution-fidelity gate, robustness and stress outcomes, the canonical Promotion proof status, reason codes, caveats, `withheld_by` and the final status. There is no rank, winner or recommendation field (`selection` is always `null`).

## 2. Identity is recomputed, never trusted

The expected trial id of each slot is recomputed from the declaration, the declared capital-fraction-wrapped fingerprint and the bars provenance manifest through the registration authority (`run_batch.expected_trial_ids`). An evidence row, the registry record (id, strategy column, canonical identity text) and the judge inventory must each equal it. An arbitrary id, a missing or wrong fingerprint, another slot's id, a missing registry record, a tampered registry identity, a judge row that contradicts the judge artifact, a DSR that differs from the judge's, or an attempt status that contradicts the registry is `INVALID`. Without the manifest or registry, identity is unverified and the best possible status is `INSUFFICIENT_PROVENANCE_OR_INDEPENDENT_VALIDATION`.

## 3. Gates (unchanged thresholds, OD-8)

| Gate | Comparator, threshold, unit | Authority and evidence |
|---|---|---|
| `scanner_min_bars_used` | >= 252 bars | scanner (`StrategyScanReviewPolicy::default`), scan `candidates.json` `bars_used` |
| `scanner_min_trade_count` | >= 5 | scanner, Backtest `trade_count` |
| `scanner_min_total_return_pct` | >= 0 % | scanner, Backtest |
| `scanner_min_alpha_pct` | >= 0 % | scanner, capital-fraction benchmark evidence |
| `scanner_max_drawdown_pct` | <= 25 % | scanner, Backtest |
| `scanner_min_profit_factor` | >= 1.05 | scanner, Backtest |
| `judge_min_dsr` | >= 0.5 (probability) | batch judge `dsr_results` |
| `judge_max_pbo` | <= 0.5 (probability) | batch judge `pbo_result` |
| `promotion_min_sharpe` | >= 0.5 | `evaluate_promotion` metrics |
| `promotion_max_drawdown` | <= 0.25 (fraction) | `evaluate_promotion` metrics |
| `promotion_min_cagr` | >= 0 | `evaluate_promotion` metrics |
| `promotion_min_profit_factor` | >= 1.05 | `evaluate_promotion` metrics |
| `promotion_min_profitable_months_pct` | >= 0.40 | `evaluate_promotion` metrics |
| `execution_fidelity` | >= 0.95 | campaign floor, registry |

Equality passes (the evaluators fail only on strict `<` / `>`; pinned to the Rust source by test). Promotion's structural gates (provenance, artifact lock, stress suite, verified OOS evidence and its trial bindings, robustness evidence, finite metrics) are represented by the canonical Promotion report's verdict.

**Strict numeric evidence.** Only real JSON numbers count: strings, booleans, lists, NaN and infinity are `INVALID`, as are values outside their domain (probabilities outside [0, 1], drawdown percent outside [0, 100], negative profit factor, fractional counts). An omitted metric is `NOT_AVAILABLE` and fails closed.

**Proxy versus canonical.** Sharpe, drawdown fraction, CAGR, profit factor and profitable months from the Research walk-forward aggregate are screening proxies. A proxy can fail a canonical gate (`PROXY_FAIL`) but can never satisfy one (`PROXY_ONLY_PASS` is unproven). Only a validated, run-bound canonical Promotion report supplies `CANONICAL` values.

## 4. Promotion proof

`PromotionProof` cannot be constructed by a caller; `PromotionProof.from_report` validates a canonical `PromotionReport`: its `run_id` equals the Backtest run the trial was evaluated on (and the Backtest manifest), its strategy matches, its `config` equals the declared policy exactly, `passed` is a boolean consistent with `fail_reasons` and with its own metrics, metrics are finite, the run was not execution-blocked. A free-form mapping such as `{"trial": {"passed": true}}` is not a proof: the result is `PROMOTION_PROOF_UNAVAILABLE`. The canonical location is `run/promotion/<strategy>/<symbol>/promotion_report.json`. **No producer writes these files today** (the canonical evaluator runs in the daemon promotion route and the Promotion contract is unchanged), so until one does, every trial is at best `INSUFFICIENT_PROVENANCE_OR_INDEPENDENT_VALIDATION`. The report is unsigned JSON from the Rust evaluator bound to the run; it is labelled `RUN_BOUND_UNSIGNED`, and no authenticated channel exists in the repository to do better.

## 5. Final status (mutually exclusive, first match wins)

1. `INVALID_EXECUTION_IDENTITY_OR_SAFETY_EVIDENCE`
2. `STRUCTURALLY_UNQUALIFIED_OR_NON_EVALUABLE`
3. `INSUFFICIENT_PROVENANCE_OR_INDEPENDENT_VALIDATION`: identity, data or holdout-guard provenance unverified, or every research gate passes but no valid canonical Promotion proof exists. **Decided before the numbers**: missing provenance is never presented as a quantitative near-miss.
4. `QUANTITATIVE_ECONOMIC_NEAR_MISS`: evaluable, valid, provenanced, at least one gate fails by number; every failed gate is listed with its distance.
5. `QUALIFICATION_WITHHELD`: the trial's own evidence passes every gate with a passing canonical proof, but a global condition is open (`withheld_by`).
6. `QUALIFIES_UNDER_EVERY_APPLICABLE_GATE`: all of the above hold and no global condition is open.

## 6. Global conditions (population and statistics)

A qualification label requires: a complete population (exactly the four declared slots, one row each; missing, duplicate and unexpected rows stay visible and withhold every would-be qualifier); a judge inventory that partitions exactly the recomputed ids; a registry that equals the recomputed identities; verified provenance (bars manifest inside the declared window, a complete post-run holdout-guard report with every category checked on the declared boundary); a valid accounting recomputed from the frozen declarations (there is no parameter that supplies a search count; a declaration whose block contradicts the recomputation makes the accounting `INVALID`); `CUMULATIVE_SEARCH_VALIDATED`; and no pending Final-Holdout access incident. Today `CUMULATIVE_SEARCH_VALIDATION_BLOCKED` (named blocker `SAB-1`) and `ACCESS_INCIDENT_PENDING_ADJUDICATION` hold, so even a perfect result is `QUALIFICATION_WITHHELD`. The four-trial judge is not deflated for the 119 disclosed trials; the review states `FOUR_TRIAL_JUDGE_RESULT`, `CUMULATIVE_SEARCH_DISCLOSED` and the validation status separately.

A near-miss is a measurement, not a candidate; at most one focused follow-up hypothesis may be authorized in a separately declared campaign with a new semantic identity and honest accounting, and relabelling a retest never makes the exposed window fresh.

## 7. History and scope

v1 trusted any non-empty trial id, tolerated a missing fingerprint, took `judge_status` from the evidence row, accepted a free-form `{trial_id: {"passed": bool}}` verdict, accepted a caller-supplied search count, parsed numeric strings, omitted the 252-bar scanner gate and used proxy values as gates. It was never run against a result and was replaced, not versioned alongside. `select_batch.py` still consumes the free-form `promotion_eligibility.json` for **historical** declarations only; it refuses the KISS declaration (`automatic_selection: false`).

Not covered: no row exists to review; the real field values will be produced by the execution stages after authorization.
