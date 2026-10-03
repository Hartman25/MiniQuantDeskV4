# Native sizing V1 — defect census, second sweep and mutation proof

Controller `V4-M1-NATIVE-SIZING-V1-01`. Contract: `docs/research/NATIVE_SIZING_V1_CONTRACT.md`. Infrastructure and proof only; no campaign, no Batch 02, no holdout consumption, no promotion, no Paper/Live. Batch 01 remains `BATCH_REJECTED` (0/15 `paper_candidate`) and is not reinterpreted. M1 remains `M1_BLOCKED`.

## Defect census

| # | Finding | Status | Proof |
|---|---|---|---|
| C1 | No capital-relative sizing existed; native candidates held an exact fixed quantity (one share against USD 100,000) so absolute returns were economically meaningless | FIXED + PROVEN | `FixedInitialCapitalFractionV1` and one shared pure resolver; resolver, wrapper and engine scenarios |
| C2 | Historical identity could be disturbed by new sizing fields | ALREADY_CORRECT + PROVEN | fixed-quantity suffix is empty; constant-based identity scenario; mutation M07 |
| C3 | Benchmark V2 is exact-target-quantity specific and mismatches a capital-fraction candidate | FIXED + PROVEN | new `capital_fraction_matched_passive_buy_hold_v1`; Benchmark V2 not mutated |
| C4 | Scanner/review could pair a candidate with the wrong benchmark | FIXED + PROVEN | cross-substitution fails closed in scanner, review and promotion |
| C5 | Promotion did not bind sizing policy/parameters | FIXED + PROVEN | `verify_review_capital_fraction_binding`; mutations P01-P04 |
| C6 | `backtest csv` and `scan-strategies` could resolve different sizing | FIXED + PROVEN | shared flags produce the same `config_id`/`run_id`; CLI scenario; mutation C01 |
| C7 | Sizing provenance was not persisted | FIXED + PROVEN | artifact round-trip scenario; mutation A01 |
| C8 | Test gaps found by mutation: incomplete-bar reference (M12), benchmark quantity/entry-bar causality/first-entry choice/alpha sign (B01, B02, B04, B05), promotion fraction agreement and Benchmark V2 co-presence (P02, P03) | FIXED + PROVEN | wrapper scenario and benchmark oracle/re-entry/alpha tests added; all now killed |
| C9 | Paper/runtime activation of capital-fraction sizing | BLOCKED (by design) | contract validated and bound into deployment identity but refused at registry build: the held quantity is memory-only and not restart-recoverable |
| C10 | Research economic bridge, registry trial identity, `backtest db`/`csv-sweep`/daemon backtest routes | DEFERRED | recorded in the contract; not required by the completion gate |

## Second 20-question adversarial sweep

| # | Question | Disposition |
|---|---|---|
| 1 | Can a missing/zero/oversized/float fraction select the policy? | No: refused at resolver, CLI, declaration and deployment contract |
| 2 | Can the policy be selected implicitly (default)? | No: explicit selection only; mutation C01 |
| 3 | Can budget math overflow or round up? | Checked i128 floor math; mutations M01, M02 |
| 4 | Can an unaffordable entry fall back to one share? | No: zero target plus recorded refusal; M03, M14 |
| 5 | Can the capital basis drift with P&L? | No: immutable initial capital, bound into the wrapper fingerprint |
| 6 | Can the reference price be a future or incomplete bar? | No: last completed bar only; M11, M12 |
| 7 | Is quantity resized while held? | No; M09 |
| 8 | Does re-entry reuse a stale quantity or compound? | Re-resolves from the same initial budget and the new close; M10 |
| 9 | Can caps raise a quantity? | No: reduce/refuse only through the one cap engine; M06 |
| 10 | Does a cap-reduced quantity below one share trade? | Refused (`CapsReducedBelowMinimumQuantity`) |
| 11 | Do fraction, capital, caps and policy change identity? | Yes; M08, M13 |
| 12 | Did any historical id change? | No; M07 and constant-based scenarios |
| 13 | Can the benchmark hold a different quantity or enter earlier/later than the candidate? | No; B01, B02, B04 |
| 14 | Are benchmark costs/execution identical to the candidate's? | Same config clone except sizing; B03 (cost) and B06 (separation) |
| 15 | Is the alpha sign pinned? | Yes; B05 |
| 16 | Can scanner and backtest disagree on sizing? | Mismatch refused; S01 |
| 17 | Can promotion accept Benchmark V2 evidence for a capital-fraction candidate, or the reverse? | No; P01-P04 |
| 18 | Does the artifact round-trip preserve sizing provenance? | Yes; A01 |
| 19 | Can Paper/Live be reached? | No: refused at registry build; R01, R02; no runtime touched |
| 20 | Could any new test pass on a shared None/default/empty value? | Fixtures use distinct non-default values; mutation proof is the control |

## Mutation proof

Each mutation was applied to the production file, the load-bearing scenario(s) run, and the file restored byte-for-byte (asserted by the harness) before the next. Results:

- 25 fast mutations (M01-M14, B01-B06, S01, R01, R02, A01, C01): all KILLED. M11, B03 first failed to compile and were corrected; M12, B01, B02, B04, B05 first SURVIVED, exposing the test gaps in C8, and were killed after the tests were strengthened.
- 4 daemon mutations (P01 dispatch, P02 fraction agreement, P03 Benchmark V2 co-presence, P04 benchmark policy id): all KILLED; P02 and P03 first SURVIVED and were killed after the promotion scenario gained a mixed-evidence case and a self-consistent other-fraction case (specific refusal reasons asserted).
- Total 29/29 KILLED. The working tree was verified free of production-file residue after the run.
