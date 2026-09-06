# WAVE06-ALPHA-CANDIDATE-CAMPAIGN-01 — Closeout

Mission: `W06-FINAL-CLOSURE-CONTROLLER-01`. Campaign: LIQ-01 (Amihud illiquidity)
then VOL-01 (volume surprise), frozen order per
`research-py/experiments/wave06_campaign/PREDECLARED_CAMPAIGN.json`.

## 1. Bridge repairs (Research → replay → Backtest → P9)

Two independently-proven deterministic defects were repaired before either
candidate executed:

- **Patch A** (`backtest: preserve replay no-decision semantics`) — an empty
  `StrategyOutput` on a same-`end_ts` batch's intermediate row (or a
  schedule-absent timestamp) was being translated as a complete-target
  "flatten everything" decision instead of "no new decision, carry forward."
  Added `Strategy::empty_output_is_noop()` (default `false`, preserving every
  other strategy's existing contract); `ResearchOosReplayStrategy` and its
  `TimestampBatchDelayedStrategy` wrapper opt in.
- **Patch B** (`backtest: fail closed on duplicate order identity`) — the same
  deterministic order/fill identity could be produced twice in one run
  (portfolio/economics applied twice). `BacktestEngine::run` now tracks every
  order identity it constructs and fails closed with
  `BacktestError::DuplicateOrderId` before any side effect for the colliding
  intent. Surfaced the identical latent defect in three pre-existing test
  fixtures (naive multi-row-per-batch strategies), repaired alongside.
- **Patch C** (`test: prove fully passing Research replay P9 path`) — the
  R3.5 synthetic E2E fixture's price data previously had zero relationship to
  the traded signal (pure per-symbol noise), so no stress scenario could ever
  pass. Its synthetic bars now carry a genuine (if synthetic) predictive
  relationship to the ranked feature; all 9 required canonical P9 scenarios
  now genuinely pass end to end through the real production path. Proves
  PLUMBING, not alpha.
- **Emergent repair** (`research: bind campaign closeout benchmark gate to
  real family_result.json shape`) — `campaign_closeout_authority.
  resolve_authoritative_evidence`'s `benchmark_relative_requirement` gate
  expected a nested `family_result.json["long_short"]["registry"]` object
  that the real `run_wave.py::run_family()` driver never produces (flat
  `trial_id`/`hypothesis_id`/`experiment_id`/`economic_eval_id` fields
  instead) — discovered only by actually executing LIQ-01 end to end for the
  first time. Rebinds the check to the real shape via the already-verified
  `economic_eval_id`. Without this repair, no Wave06 candidate could ever
  receive a machine-verified closeout past the insolvency short-circuit.

See git history on `wave06-alpha-candidate-paper-entry-01` for exact SHAs and
diffs; per repo convention this document does not embed a commit-hash table
that would drift.

## 2. LIQ-01 — `pooled_single_feature_xs_amihud_illiquidity_direct_rank_v1`

Real trials registered under `WAVE06-CAMPAIGN-ALPHA-CANDIDATE-REAL-V1`
(shared campaign registry, `wave06_campaign/runs/run_01/registry/research.sqlite3`),
built from a real, verified bars cache (identical universe/window/feed/asof
to DISCOVERY-01's own already-fetched, provenance-verified extraction — reused
byte-for-byte, no new provider/network call made by this controller).

| | long_only | long_short (primary) |
|---|---|---|
| net_total_return | -0.99999999633 | -0.99999999992 |
| net_sharpe | -12.755 | -7.529 |
| max_drawdown | -0.99999999635 | -0.99999999992 |
| cost_drag | 1.9397 (194% of equity) | 0.1574 |
| total_turnover | 107,522,999.68 | 70,304,602.37 |

Dynamic rankable benchmark (long_short) net_sharpe ≈ 0.845 →
**benchmark_relative excess = -8.374** (policy requires strictly > 0).

DSR = 1.22e-10 (≈0), PBO = 0.0, DSR/PBO block-count sensitivity range = 0.0 /
0.0 (both well inside the 0.15 ceiling) — computed and stored, but never
inspected by `classify_verdict`, which terminally rejects at the earlier
`benchmark_relative_requirement` gate. `genuine_shuffled_placebo`: not
evaluable (`"Fail-closed: cannot annualize a total_return <= -100%"` — the
underlying economics are too far negative for that control's own annualization
math, a genuine, honest "not evaluable," not a defect).

**Terminal verdict: `REJECTED_NOT_ADVANCED`** — machine-computed and
independently re-verified via `campaign_order_guard.load_verified_closeout`.
Written through the sole authorized writer
(`campaign_order_guard.write_closeout_status`) to
`research-py/experiments/wave06_candidate_liq01_amihud_illiquidity/CANDIDATE_CLOSEOUT_STATUS.json`
(untracked local evidence, per this repo's existing `runs/`-directory
convention). Holdout status: `reserved_not_evaluated` (untouched).

## 3. VOL-01 — `pooled_single_feature_xs_volume_surprise_direct_rank_v1`

Same bars cache/universe/window reused. Real trials registered under the same
shared experiment_id, authorized to execute only after LIQ-01's verified
`REJECTED_NOT_ADVANCED` closeout (`campaign_order_guard.
require_authorized_to_execute` — confirmed programmatically).

| | long_only | long_short (primary) |
|---|---|---|
| net_total_return | -0.99999999999750 | -0.99999999999999996 |
| net_sharpe | -10.139 | -15.255081358511292 |
| max_drawdown | -0.99999999999755 | -1.0 (rounding) |
| cost_drag | 3.4445 (344% of equity) | 1.0127 |
| total_turnover | 84,148,210.71 | 84,274,835.36 |

Dynamic rankable benchmark (long_short) net_sharpe ≈ 0.766 → benchmark
excess = **-16.021184928699608** — the same catastrophic, unambiguous
failure pattern as LIQ-01.

**Prior closeout attempt (historical, superseded below):** the shared
multiple-testing judge (`build_multiple_testing_judge`) groups candidates by
their EXACT OOS return-series date set and admits only the largest such
group; LIQ-01's long_only/long_short share one date set, VOL-01's
long_only/long_short share a different one (both size 2 — a tie broken
deterministically by `canonical_json(dates)` ordering), so VOL-01's trials
are excluded from the admitted comparison group with reason
`return_series_date_misalignment`. The resolver as it existed at the time
eagerly required `dsr_requirement`/`pbo_requirement` judge authority for
every candidate regardless of whether the frozen early-rejection cascade had
already terminally rejected it earlier, so it raised `AuthorityRefusal` for
VOL-01 even though `benchmark_relative_requirement` (excess -16.02) had
already deterministically rejected the candidate — a real resolver defect,
not the judge's fail-closed behavior working as intended.
**W06-FINAL-CLOSEOUT-LAZY-AUTHORITY-REPAIR-01**
(`research: honor campaign early-rejection authority cascade`) fixed
`campaign_closeout_authority.resolve_authoritative_evidence` to honor the
same frozen early-rejection cascade `classify_verdict` itself applies: a
downstream evidence authority (judge DSR/PBO, genuine shuffled placebo,
DSR/PBO sensitivity) is required only once the cascade genuinely reaches a
gate that needs it, generalizing the pre-existing insolvency short-circuit.
The judge's `return_series_date_misalignment` exclusion is now only
historical diagnostic context for the repaired resolver — VOL-01 never
needed judge authority in the first place, since
`benchmark_relative_requirement` alone already terminally decided it.

**Terminal verdict: `REJECTED_NOT_ADVANCED`** — machine-computed by the
repaired resolver from VOL-01's existing, previously-registered real
trials/attempts (no rerun, no new attempt, no provider call) and
independently re-verified via `campaign_order_guard.load_verified_closeout`.
Written through the sole authorized writer
(`campaign_order_guard.write_closeout_status`) to
`research-py/experiments/wave06_candidate_vol01_volume_surprise/CANDIDATE_CLOSEOUT_STATUS.json`
(untracked local evidence, per this repo's existing `runs/`-directory
convention; sha256
`e16ba0c83cc32d7bf81e9767a123d1c24bfbd2ca7fbf21c3d87c6e8f0277db84`). Every
gate at/after `matched_diagnostic_placebo_requirement` is stored as
`NOT_RUN_AFTER_DETERMINISTIC_REJECTION` with no fabricated DSR/PBO value —
`dsr_requirement`/`pbo_requirement` were never resolved, because the cascade
never reached them. Holdout status: `reserved_not_evaluated` (untouched;
never reached).

## 4. Systemic context (not reopened here)

Both candidates' catastrophic net-return collapse (gross signal frequently
positive or near-zero; cost/turnover drag consuming essentially all equity)
matches the SAME systemic pattern already found and closed as out-of-scope in
DISCOVERY-01 (`docs/research/DISCOVERY_01_LOW_VOLATILITY_ANOMALY_RESULT.md`,
§6-7; see also project memory
`project_discovery_01_closure_and_diagnostic_mission_boundary.md`): a property
of the shared `economic_walkforward.py` evaluation path (rank_side_count=5
against a highly dynamic/thin rankable set, daily rebalancing, commission +
slippage), not of any individual feature. Per that prior operator decision,
this is **not** reopened or diagnosed here — a future, separately-predeclared
diagnostic mission against `economic_walkforward.py` would be required.

## 5. Promotion / Paper entry

Neither candidate reached `canonical_p9_robustness_gauntlet_requirement` (both
terminally rejected earlier). No promotion evaluation was run — there is no
evidence to evaluate. **Paper-entry status: `NOT_EARNED`.** Zero Paper orders,
zero Live orders, zero broker/runtime touches by this controller.

## 6. Wave06 status

`WAVE06_LOCALLY_CLOSED_NO_CANDIDATE_ADVANCED` — both frozen campaign
candidates received a machine-computed, hash-verified, independently
re-verified `CANDIDATE_CLOSEOUT_STATUS.json` with terminal verdict
`REJECTED_NOT_ADVANCED`; neither advanced past the frozen
`benchmark_relative_requirement` gate; zero candidates advancing is an
explicitly valid, successful Wave06 outcome per this controller's own
mission.

Active production ledger remains **43** — no row is decremented by this
closeout; Wave06 infrastructure/bridge work does not itself close a ledger
row absent an exact, independently-matched row.
