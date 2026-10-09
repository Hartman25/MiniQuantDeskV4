# Final-Holdout access incident `HOA-KISS-EXT032-01`

State: **`ACCESS_INCIDENT_PENDING_ADJUDICATION`**. Machine record: `research-py/experiments/m1_native_trend_campaign/HOLDOUT_ACCESS_INCIDENTS.json` (hash-chained, append-only; first-entry hash pinned by `test_holdout_incident.py`). This document is the human-readable companion; the ledger is authoritative.

## What is known

During the controller `V4-M1-KISS-CALENDAR-CAMPAIGN-PREDECLARATION-AND-ENGINE-01`, a mutation test opened the declaration's `execution_gate`. The spawn tests then executed `run_batch.py fetch --execute`, and the environment held Alpaca credentials. The official extractor requested SPY, QQQ, IWM and DIA over 2016-01-01 to 2026-09-01, which includes the reserved Final-Holdout months 2026-03-01 onward (OD-3).

## Event classes (kept apart; none implies another)

| Class | Status | Basis |
|---|---|---|
| Provider access | OCCURRED | request executed, about 10,720 bars returned |
| OHLCV parsing | OCCURRED | parsed into a DataFrame; counts and history validated by `verify_fetched_bars` |
| Human price inspection | NOT_REPORTED | none reported; absence of a report is not proof of absence |
| Strategy evaluation | NOT_REPORTED | the stage that ran (fetch) performs none; nothing else was reported |
| Trial / attempt registration | NOT_REPORTED | an empty registry file was created; no trial or attempt reported |
| Formal holdout consumption | NOT_REPORTED | no ledger reservation/consumption reported; the per-run ledger cannot show cross-process access |

Data and corporate-action artifacts were written to a git-ignored run directory and later deleted. Deletion does not undo the provider access. Provider or proxy logs may exist outside the container and were not inspected.

## What is NOT asserted

The Final Holdout is not declared untouched, not declared statistically clean, not declared permanently consumed, and not declared restored by the deletion.

## Unknowns

Whether anyone inspected the prices; whether provider/proxy logs retain the request; whether the rows were copied before deletion; whether the operator treats provider access plus parsing, without inspection or evaluation, as consuming the holdout's single-use character.

## Effect in code (enforced, not advisory)

* `holdout_guard.py` reports `access_incident` in every guard report and never states an independent clearance.
* Stage authorization refuses `promotion` and `paper_deployment` while an affecting incident is pending, and requires every graded-campaign authorization to name the pending incident ids it acknowledges.
* `near_miss_review.py` cannot emit a qualifying status while an affecting incident is pending.
* The declaration's holdout status is `RESERVED_NOT_FORMALLY_CONSUMED__ACCESS_INCIDENT_PENDING_ADJUDICATION`; the earlier wording "RESERVED / UNCONSUMED" overstated what a per-run ledger proves.

## Outstanding adjudication (operator decision)

Decide whether the Final Holdout remains usable for its single-use purpose, is consumed, or is replaced by a different reserved window. Record the decision by appending an `ADJUDICATION` entry to the ledger; do not edit existing entries.

## Corrections to earlier statements (additive)

* `M1_KISS_EXT032_CLOSEOUT.md` C15 reported the incident but called the fix complete and left the holdout characterization to the operator without a durable state; this record and the ledger supersede that wording.
* Any document describing the holdout as "RESERVED / UNCONSUMED" for the KISS campaign describes the formal ledger only. Historical campaigns share the same calendar window; their historical results are unchanged and not re-opened, but they inherit the same pending incident for the purposes of any future independence claim.
