# Four-Way Mission — Program Sequence Record (for later ledger reconciliation)

Written from the isolated clone of `V4-MQD-FOUR-WAY-ASSET-NEUTRAL-INTEGRATION-ISOLATED-01`.
Not appended directly into `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md` or
`MiniQuantDesk_Master_Patch_Ledger_v2.md`: both are large, live documents the
concurrently-running EXT-032 correction controller may be editing in the
primary repository at the same time this branch was built. Appending here
risks a stale snapshot or a merge conflict against work this mission must
not touch. This note is a precise, bounded, append-only record for whoever
reconciles this branch into the canonical ledgers.

## M1-RESEARCH-OPPORTUNITY-CADENCE-01 (operator-approved policy)

MQD seeks recurring, independently justified portfolio-level opportunities
across strategies and instruments. Three or more opportunities per week is
a desirable research characteristic, not a quota, trading requirement, or
reason to lower economic/risk standards. Zero trades is a correct result
when no qualifying signal exists.

## Program sequence (as recorded by this mission; execution order is the
operator's to set)

1. Finish and independently accept the existing EXT-032 correction.
2. Close Census 02's outstanding accounting/statistical-evidence questions
   without rerunning frozen results.
3. Reconcile the cross-campaign hypothesis inventory.
4. Design a comprehensive, finite, asset-extensible, frequency-aware US
   equity/ETF discovery campaign.
5. Specify valid evidence partitions and cumulative multiple-testing
   accounting before economic execution.
6. Run the new campaign only under a separate authorization.
7. Continue candidate-independent operational testing in parallel.
8. Promote only genuinely qualified strategies.
9. Finish genuine Paper operational validation using existing runbooks.
10. Consider another asset class after a finite, evidence-based
    reassessment if equities remain economically unproductive.

None of steps 4–10 are started or authorized by this mission. This mission
(Components A–D) prepared devtools/Research *infrastructure* only — no
economic trial, no Promotion, no Paper/Live action.

## Cross-component asset-neutral acceptance matrix

| Invariant | Required result | Verified |
|---|---|---|
| Existing equity/ETF semantics | Unchanged | Zero existing production files modified across all 4 commits (11 files changed, all new; `.gitignore` +6 lines only) |
| Existing trial identities | Unchanged unless justified | `register_trial`/`register_hypothesis` untouched; Component C only shapes an `identity` payload, never registers one itself |
| Existing production sizing | Unchanged | Zero Rust sizing files touched; Component B is new Research-only code reusing `tax/metrics._equity_metrics`, not `mqk-strategy`/`mqk-backtest` |
| Unknown asset | Explicit rejection | Component C: `AssetClassTag.UNKNOWN`/unsupported classes are tagged, never coerced to `EQUITY` (tested) |
| Unsupported quantity unit | Explicit rejection | Component B: `TradeRecord.validate()` rejects any unit other than `"shares"` (tested) |
| Unsupported quote currency conversion | No guessed conversion | Component B: only `"USD"` accepted; no conversion code path exists at all |
| Changed economic cost/asset contract | Relevant identity changes | Component C: changing `cost_model_contract` or `asset_class` changes the semantic fingerprint/manifest_id (tested) |
| Pure display/layout change | No new economic identity | Component C: parameter/data-input insertion order does not change the fingerprint (tested) |
| Same hypothesis across assets | Distinct realization when economics differ | `asset_class` is part of `economic_fields()`, so it participates in the fingerprint |
| Same economic exposure across instruments | Not automatically independent | Component B reports `correlated_exposure` as `not_evaluable` rather than assuming independence |
| Missing asset-specific provider | Fail closed | Component D: `fetch_from_provider` raises for every unverified provider, including `stocknest` |
| Missing calendar/execution model | Fail closed | Component C: `PopulationDeclaration.validate()` rejects an empty `session_calendar_contract`/`execution_model_contract`/etc. — **found missing and fixed during this mission's own final sweep** (commit `25c0fc70`), not assumed correct from the first pass |
| New asset operational enablement | None | `OPERATIONAL_ASSET_CLASSES == {EQUITY}` only (tested); Component D's registry stays all-disabled |
| Paper/Live authority | None | No module in this mission imports a broker, daemon, or Promotion path (verified by import-statement scan in each component's tests) |

The one row that initially failed its own check (missing calendar/execution
model validation in Component C) is recorded here rather than silently
corrected and forgotten: it is real evidence the final adversarial sweep
was not a formality.
