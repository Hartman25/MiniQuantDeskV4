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
| C9 | Runtime seam: the held quantity was memory-only so the registry refused every capital-fraction deployment (IR-SZ-02); runtime fingerprint was the unwrapped one | FIXED + PROVEN (superseded wording: first recorded "BLOCKED by design") | restart-recoverable host + durable state (migration 0091); runtime/daemon identity equals canonical Backtest wrapper fingerprint; Paper NOT activated |
| C10 | Research economic bridge and trial identity (IR-SZ-01) | FIXED + PROVEN (first recorded DEFERRED, which was wrong: the controller required it) | sized stream parity, wrapper fingerprint at registration, `capital_sizing` in trial identity |
| C11 | `backtest db` / `csv-sweep` / daemon backtest job route could not be classified as safe | FIXED + PROVEN | classified C/C/B; daemon route now refuses a sizing policy instead of ignoring it; CLI routes refuse the flags |
| C12 | Benchmark `run_id` did not bind exact Q or causal entry (IR-SZ-03) | FIXED + PROVEN | benchmark semantic fingerprint + verifier recompute; Benchmark V2 historical identity unchanged |

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
| 19 | Can Paper/Live be reached? | No: stateless seams refuse `DurableStateRequired` strategies (registry build now succeeds, instantiation does not); R01, R02 and the correction scenarios; no runtime dispatch touched |
| 20 | Could any new test pass on a shared None/default/empty value? | Fixtures use distinct non-default values; mutation proof is the control |

## Mutation proof

Each mutation was applied to the production file, the load-bearing scenario(s) run, and the file restored byte-for-byte (asserted by the harness) before the next. Results:

- 25 fast mutations (M01-M14, B01-B06, S01, R01, R02, A01, C01): all KILLED. M11, B03 first failed to compile and were corrected; M12, B01, B02, B04, B05 first SURVIVED, exposing the test gaps in C8, and were killed after the tests were strengthened.
- 4 daemon mutations (P01 dispatch, P02 fraction agreement, P03 Benchmark V2 co-presence, P04 benchmark policy id): all KILLED; P02 and P03 first SURVIVED and were killed after the promotion scenario gained a mixed-evidence case and a self-consistent other-fraction case (specific refusal reasons asserted).
- Total 29/29 KILLED. The working tree was verified free of production-file residue after the run.

# Independent-review correction (`V4-M1-NATIVE-SIZING-V1-INDEPENDENT-REVIEW-CORRECTION-01`)

The stack was declared locally complete while deferring seams the controller required; this correction closes them. Commit-level history is in git; the seven earlier commits are preserved.

## Defect census (correction)

| # | Finding | Status | Proof |
|---|---|---|---|
| R1 | Research emitter refused a capital-fraction config; batch `register`/`trials` refused a declaration (IR-SZ-01) | FIXED + PROVEN | `scenario_native_signal_capital_fraction_01` (targets == sized Backtest targets, run id and fingerprint equal, future bars cannot change Q, fixed-quantity stream unchanged); `test_native_capital_fraction_bridge.py` (identity, retry, negatives, real CLI round trip) |
| R2 | A recorder placed before the sizing wrapper would record the inner `+1` | FIXED + PROVEN | `add_strategy_observed`; mutation: recording at the inner seam fails 4 tests |
| R3 | Runtime registry refused capital-fraction; held Q not restart-recoverable (IR-SZ-02) | FIXED + PROVEN | `scenario_capital_fraction_restart_01` (real disposable Postgres): restart while long keeps exactly the original Q at a 3x later price, no duplicate entry, exit releases, restart while flat stays flat, re-entry is generation 2 with a new Q from the same capital; crash-after-persist replay; negative controls |
| R4 | Runtime fingerprint was the unwrapped engine's | FIXED + PROVEN | runtime identity == canonical `BacktestEngine` report fingerprint for all four engines and cap/capital/fraction variants; fixed-quantity identity unchanged |
| R5 | Benchmark run id did not bind exact Q / entry (IR-SZ-03); verifier could not detect a substituted benchmark run id | FIXED + PROVEN | `scenario_capital_fraction_benchmark_identity_01`, tamper tables in backtest and daemon scenarios |
| R6 | Daemon backtest job route silently ignored an unknown sizing field | FIXED + PROVEN | `bj01b` |
| R7 | Benchmark V2 `benchmark_run_id` does not bind Q/eligibility | OUT OF SCOPE (frozen authority; see contract) | not changed |

## Second adversarial sweep (correction)

| Question | Disposition |
|---|---|
| Can Research still refuse a valid capital-fraction declaration? | No. FIXED + PROVEN |
| Can Research emit +1 while Backtest emits capital-sized Q? | No: same wrapper output is recorded. FIXED + PROVEN |
| Can trial identity omit fraction/capital? | No: wrapped fingerprint plus explicit `capital_sizing`. FIXED + PROVEN |
| Can a retry manufacture a trial? | No: same identity, new attempt. ALREADY_CORRECT + PROVEN |
| Can native-fingerprint differ from the canonical Backtest wrapper fingerprint? | No. FIXED + PROVEN |
| Can runtime restart re-resolve Q from a later price? | No. FIXED + PROVEN |
| Can runtime infer capital from broker/account equity? | No: capital is the explicit contract value; no broker input exists. ALREADY_CORRECT + PROVEN |
| Can a stale/foreign record cross strategy/symbol/deployment? | No: scope-keyed fetch, scope and contract proven per record. FIXED + PROVEN |
| Can an exit fail to clear active sizing state? | No: Released transition persisted; mutation kills it. FIXED + PROVEN |
| Can benchmark Q or entry timing change without the run id changing? | No. FIXED + PROVEN |
| Can a substituted benchmark run id reach Promotion? | No: `verify_internal` recomputes it at review and promotion. FIXED + PROVEN |
| Can Benchmark V2 authorize a capital-fraction candidate, or the reverse? | No. ALREADY_CORRECT + PROVEN |
| Can an unsupported route silently fall back to fixed quantity? | No. FIXED + PROVEN |
| Did any historical fixed-quantity identity change? | No (pinned V2 benchmark run id, constant identity scenarios, fixed trial id unchanged). ALREADY_CORRECT + PROVEN |
| Did any code activate Paper, touch Live or consume the holdout? | No. ALREADY_CORRECT |
| Do docs still call an in-scope defect DEFERRED/BLOCKED BY DESIGN? | Corrected (C9/C10 above). FIXED |

## Mutation proof (correction)

Each mutation was applied, the focused scenarios run, and the file restored byte-for-byte (asserted by the harness):

- Recorder at the inner seam (4 KILLED); restore drops the held Q (2 KILLED); persistence skipped (5 KILLED); stored-Q reproducibility check removed (1 KILLED); exit without release (1 KILLED).
- Research: sizing-block verification removed (8 KILLED); `capital_sizing` removed from trial identity (2 KILLED).
- Benchmark: target Q removed from the benchmark fingerprint (3 KILLED); entry identity removed (3 + 1 KILLED); benchmark run-id verification disabled (2 KILLED).
