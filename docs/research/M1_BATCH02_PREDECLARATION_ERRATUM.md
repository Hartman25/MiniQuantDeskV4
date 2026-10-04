# M1 Batch 02 — predeclaration erratum (IR-B02-01)

Mission `V4-M1-NATIVE-HYPOTHESIS-BATCH-02-INDEPENDENT-REVIEW-CORRECTION-01`. Machine-readable authority: `research-py/experiments/m1_native_trend_campaign/PREDECLARED_BATCH_02_ERRATUM.json`. Status: `POST_RUN_NON_ECONOMIC_METADATA_ERRATUM`.

## What the independent review found

The formal predeclaration `PREDECLARED_BATCH_02.json` (commit `368c1f47`, committed before any engine, registration or result) contains descriptive strings copied verbatim from the Batch 01 corrected declaration. They describe Batch 01 and contradict the Batch 02 contract that the same file states and that actually controlled the run. The original file was **not** perfect and is **not** edited: it stays byte-for-byte as committed.

| Id | Path | Stale text (Batch 01) | Batch 02 truth |
|---|---|---|---|
| E1 | `/partition/warmup_note` | 252/253/204-bar SMA warm-ups | required history 1 / 1 / 60 for H1 / H2 / H3 |
| E2 | `/economic_protocol/quantity_semantics/executable_targets` | "0 or exactly +1 share … fixed-one-share" | raw direction 0/+1; executable quantity is the capital-fraction wrapped Q (1000 bps of USD 100,000) |
| E3 | `/economic_protocol/quantity_semantics/rule` | "+1 desired / +N held mismatch is a fidelity failure" | the wrapper converts +1 into Q; fidelity compares the wrapped desired Q with the held quantity |
| E4 | `/native_backtest/initial_cash_rule` | Benchmark V2 shares the capital basis | `capital_fraction_matched_passive_buy_hold_v1`; Benchmark V2 is not Batch 02 authority |
| E5 | `/native_backtest/evidence_root_layout` | `runs/run_batch_01_corrected/…` | `runs/run_batch_02/…` |
| E6 | `/universe/survivorship_note` | "the judge deflates over all five" | one batch-wide judge over fifteen (hypothesis, symbol) trials |

E1–E5 were named by the review; E6 was found by the census the review authorised (the five-trial wording survived from the single-strategy campaigns).

## Byte identity

The reviewer observed `bee59261…ed05` on the Windows (CRLF) working copy. The committed LF blob is `68103582…2b82`. They differ by the 645 line endings only (the LF-normalised working copy equals the committed blob byte for byte); the audit accepts exactly these two forms and nothing else (`batch02_erratum.original_bytes_intact`). The original file is unmodified in Git.

## Why these are descriptive and cannot have controlled the run

Every program that reads a declaration (`run_batch.py`, `summarize_batch.py`, `select_batch.py`, `holdout_guard.py`) was inspected for the keys it consumes: none reads any of the six paths. That is a claim, so it is also measured. A disposable in-memory copy of the original is given ONLY the six corrected descriptions, and the real `run_batch.py` functions then recompute, for the original and the corrected copy, and compare as canonical JSON:

* the 15 canonical trial ids and their `identity_json` (hypotheses, trial order, required history, partition, economic spec, capital-sizing block);
* baseline `sizing_args`, `native_bridge_args`, `research_capital_sizing`, and the stress `native_bridge_args`/`stress_plan` at 500 bps;
* the `_economic_spec` derivation and the economic inputs;
* benchmark policy and the `scan-strategies` / `review-scan` / `backtest csv` argument vectors;
* the registration-gate inventory (15 registered, 0 attempts; 2 attempts when 2 are begun) and the selection (`promotion_policy`, `max_trials`) and holdout-guard inputs.

Result: identical for the six together and for each one alone (committed test `test_batch02_predeclaration_erratum.py`, 35 tests). Negative controls show the same harness notices 15 executable edits (baseline/stress bps, commission, slippage, entry threshold, block counts, partition, H3 history, rationale text, trial order, promotion threshold, run dir, data pin, cap) and refuses the benchmark/capital substitutions, and that listing an executable path as "descriptive" would fail the proof.

Against the real local artifacts (`python batch02_erratum.py --real`, read-only, no CLI call, registry copied before opening): the 15 recorded trial ids are reproduced from the recorded native fingerprints and data provenance under **both** the original and the corrected declaration; their digest equals the erratum's; the run artifacts (`batch_outcome.json`, `batch_results.json`, `trials_index.json`, judge, holdout post-guard, registration proof) are byte-identical before and after; the post-run holdout guard output is identical for both declarations and equals the recorded one.

## Test authority fixed

`test_frozen_blocks_are_byte_identical_to_the_accepted_contract` copied whole Batch 01 blocks as Batch 02 truth and so froze stale prose along with policy. It is replaced by explicit frozen-policy paths (data, partition, economic policy, capital basis, promotion thresholds, rejection gates, fidelity floor, robustness block counts / sensitivity ceilings / non-sizing stress knobs, holdout). A detector (`unreviewed_copied_descriptions`) additionally requires every descriptive string a declaration shares byte-for-byte with Batch 01 to be individually reviewed and listed, so a future declaration cannot pass by copying older text.

## Disposition

Non-operative descriptive metadata only. No attempt, result, threshold, strategy parameter or holdout was touched or rerun. **`BATCH_REJECTED` stands.**
