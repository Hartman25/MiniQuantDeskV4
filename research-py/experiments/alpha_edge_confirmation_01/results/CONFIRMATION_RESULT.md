# V4-ALPHA-EDGE-CONFIRMATION-01 — Result

VERDICT: **CONFIRMATION_COMPLETE — 0 CONFIRMED_STRONG, 6 CONFIRMED_DIRECTIONAL_ONLY, 12 NOT_CONFIRMED, 0 NOT_EVALUABLE, 0 BLOCKED.**
Nothing advances toward Final Holdout. All rows: `VALIDATION_STATUS=NOT_VALIDATED`, `PROMOTION_AUTHORITY=NONE`, `EXECUTABLE_PNL=false`.

## Status and authority
- `CONFIRMATION_STATUS = LOCALLY_COMPLETE_PENDING_FINAL_CHATGPT_ACCEPTANCE`. Confirmation validation: `NOT_VALIDATED` for every row; no factor is an executable or profitable strategy.
- Promotion: `PROMOTION_AUTHORITY = NONE` (not claimed). Paper: INACTIVE. Live: DISABLED. Final Holdout: RESERVED / UNCONSUMED (0 rows read, 0 scored).
- DSR/PBO: not applicable to this stage (factor-level diagnostics only; no Strategy trials or walk-forward artifacts); Strategy DSR/PBO stays `DEFERRED_FULL_POPULATION` as recorded in Pass-2.
- Pass-1/Pass-2 evidence and result files are unmodified by this mission (`git diff 9d3635c..HEAD` touches only `alpha_edge_confirmation_01/` and `test_alpha_edge_confirmation_01.py`).
- Full `research-py` acceptance (2463 passed, 7 skipped) was run in the local continuation at C5 (94d7fbc7) and is not re-run in the cloud closure; this closure changes documentation only.
- Latest evidence timestamp: provider raw max `2026-02-27T05:00:00+00:00` (< fence); max scored observation `2026-02-26`; Confirmation period `[2025-01-01, 2026-03-01)`; 2024 is indicator warm-up only (no 2024 row scored).
- RKLB exclusion: `EXCLUDED_UNSUPPORTED_CORPORATE_ACTION` (`name_change`, effective 2025-05-27, not covered by Alpaca `adjustment=all` semantics and not role-aware-resolved); 0 bar requests; the universe stays 87 of 88 with no silent shrink.

## Scope
- Denominator: exactly the 18 Pass-2 Conditional survivors (S06 x12, S05 x4, S13 x2), frozen and committed (187f79b7) before any provider request.
- Strategy rows (789, PROCESS_CONTAMINATED_PRE_FREEZE_REAL_COHORT_SMOKE) were not confirmed; the six S2 near-misses were not rescued.
- Reserve consumed once: [2025-01-01, 2026-03-01). 2024 is warm-up only (no 2024 row scored). Final Holdout [2026-03-01, ...) was never requested, fetched, loaded or scored (`confirmation_consumption_proof.json`: provider max < fence, holdout rows read/scored = 0, label endpoints < fence).
- Universe: 88 symbols, 87 eligible; RKLB excluded with a typed unsupported-corporate-action disposition. 433,956 scored factor-rows.
- Statistics: accepted census estimator, min 30 events, two-sided null with 200 permutations and base seed 0, BH alpha 0.10 over the complete frozen 18.

## Outcome
| factor (prefix) | family | h | events | effect | p | q | status |
|---|---|---|---|---|---|---|---|
| 2ba560eee3 | S13 | 20 | 152 | +0.0594 | 0.0398 | 0.1433 | DIRECTIONAL_ONLY |
| c0a0fe8e27 | S06 | 20 | 265 | +0.0358 | 0.0547 | 0.1642 | DIRECTIONAL_ONLY |
| 2d6ddd29f2 | S06 | 20 | 914 | +0.0093 | 0.1393 | 0.3134 | DIRECTIONAL_ONLY |
| 47fe69152d | S13 | 20 | 91 | +0.0694 | 0.2786 | 0.5015 | DIRECTIONAL_ONLY |
| c7bb0c5ba2 | S06 | 20 | 2319 | +0.0016 | 0.4677 | 0.6475 | DIRECTIONAL_ONLY |
| 910d7e56b2 | S05 | 1 | 5354 | +0.0010 | 0.5672 | 0.6806 | DIRECTIONAL_ONLY |

The other 12 factors have effect <= 0 (NOT_CONFIRMED). Smallest BH q = 0.1433 > 0.10, so zero BH rejections and zero STRONG.

Disclosure: four NOT_CONFIRMED factors (f13f8dc0f7 p=0.0100, b5bd9508a2 p=0.0249, 7a8ff7065e p=0.0249, ceae5c392a p=0.0348) have small two-sided p with a NEGATIVE effect, i.e. a significant sign reversal, not support. The null is two-sided by predeclaration; the decision table requires effect>0, so they are NOT_CONFIRMED. No post-hoc direction flipping.

Rankings (`confirmation_rankings_readonly.json`) are read-only: the CONFIRMED_STRONG table is empty; the directional-only table is separate and carries no promotion meaning.

## Integrity evidence
- Attempts: 18 evaluations, 18 succeeded attempts, 0 failed/retried; second `evaluate` run executed 0 (terminal 18/18), no new evaluations or attempts.
- Mutation/negative proof: 33 mutations (C01-C25 incl. sub-cases), 33 KILLED_RED_THEN_GREEN, 0 not proven, every restore byte-identical (`confirmation_mutation_log.json`; isolated git-archive copy because the runtime freeze guard binds the committed sources). C11's bar-level fence filter is an equivalent mutant (bars are already fenced at load), so the proof targets the `assert_scored` backstop; this defense-in-depth is disclosed, not hidden. C01's first mutation was ineffective (a second committed-ness check masked it); it was retargeted to `require_committed` and now fails RED for the intended reason.
- Second adversarial sweep (`second_sweep.py`, `confirmation_second_sweep.json`): 31 checks, 0 defects. It re-derives from raw bars and committed files, without c1_eval's estimator, decision rule or BH helper: the 18 denominator, event counts (max abs diff 0), effect sign and value (max abs diff 1.4e-17), p-value formula, BH q (<1e-12), dispositions, FDR family completeness, no 2024 scored rows, no holdout reads, label endpoints < fence, attempt counts and evaluation-id binding. All findings: ALREADY CORRECT + PROVEN; none FIXED, none BLOCKED.

## Disclosures
- Pass-2 Strategy rows were pre-freeze real-cohort smoke contaminated; they stay excluded here and unadvanced.
- Tooling: `mqk_readonly` used first; Srclight index was stale (0752f406) and not used; Graft not used this phase.
- First authorized read UTC 2026-10-07T04:20:53Z (sandbox clock ahead of wall time).
- Labels (`close_{t+h}/close_t - 1`) are diagnostic only, never executable P&L.
- Confirmation is consumed: the reserve is never fresh again.

## Next
INDEPENDENT CHATGPT REVIEW. No push. Final Holdout untouched. No factor is eligible to advance on this evidence.
