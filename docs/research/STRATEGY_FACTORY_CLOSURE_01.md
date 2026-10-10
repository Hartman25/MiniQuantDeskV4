# Strategy Factory — Closure Record (`V4-STRATEGY-FACTORY-RESEARCH-BACKTEST-FULL-COMPLETION-01`)

Baseline `origin/main` `e190cce93672405263733ab5a10b4d9d5903adf2`; branch `strategy-factory/full-completion-01` (dedicated worktree
`MiniQuantDeskV4-StrategyFactory`). Guarded fast-forward checkpoint pushes only; nothing pushed to or merged into `main`.
This document is a record, not a living status table: the operator procedures are in
[`docs/runbooks/strategy_factory_operator.md`](../runbooks/strategy_factory_operator.md); the pre-edit census is
[`STRATEGY_FACTORY_CENSUS_01.md`](STRATEGY_FACTORY_CENSUS_01.md).

## Statuses (mission section 17)

| State | Status | Basis |
|---|---|---|
| `FACTORY_CODE_COMPLETE` | **YES, with the disclosed deferrals below** | intake -> formalization -> dedup -> admission -> predeclaration -> queue -> stage-authorized execution -> judge/review -> report, plus AI normalization, scout and CLI |
| `FACTORY_E2E_PROVEN` | **YES (synthetic data, real native engine)** | E2E-01..12 below; real `mqk-cli` `grammar_v1`/native engines, real registry, judge, scanner/review |
| `FACTORY_UNATTENDED_PROVEN` | **YES (synthetic)** | one `run` call drove 13 stages; kill/recovery; 3 concurrent campaigns across 2 real worker processes |
| `FACTORY_EXTERNAL_DEPENDENCY_BLOCKED` | **YES for specific items** | real-data execution needs the operator's HMAC secret + released gate + signed authorization; live web scouting needs an operator-approved policy; paid AI needs a cost authorization; GUI/daemon route is `BLOCKED_OWNERSHIP` (Codex owns GUI/observability files) |
| `FACTORY_INDEPENDENT_ACCEPTANCE_PENDING` | **YES** | not independently reviewed |
| `FACTORY_INDEPENDENTLY_ACCEPTED` | no | |
| `M1_BLOCKED` | **unchanged** | no qualified candidate; the Factory finishing does not clear M1 |
| `M1_OPERATIONALLY_COMPLETE` | no | |

`STRATEGY_FACTORY_FULLY_COMPLETE` is **not** claimed: stateful templates (channel, RSI, z-score, gap/ATR reversals, monthly families)
and multi-symbol/portfolio ideas intentionally end at `NEEDS_IMPLEMENTATION` with an inert implementation request; the operator
read-only GUI/API panel is deferred by ownership; live web scouting and cloud AI were not exercised.

## Architecture

```
catalogs (xlsx/csv, 8 profiles + operator profile JSON) --hash-bound, verbatim rows--> StrategyIdea (typed, EXPLICIT/INFERRED/UNDERSPECIFIED/UNKNOWN)
  optional AI proposes (Ollama; verified-span rule) --> deterministic dedup vs native cards + Census-01/02 + prior intake
  --> admission (native | grammar_v1 | new implementation | unsupported | rejected) --> explicit disposition for every entry
campaign spec (population sources + data pins + partition) --> compile --> frozen declaration (identity, complete trial population)
  --> operator releases gate + signs stage authorization --> durable queue (SQLite, atomic claims, leases)
  --> run_batch.py <stage> --resume per stage: check, data, register, gate, holdout_pre, trials, judge, backtest, finalize, review,
      holdout_post, summary  (+ internal report)   [ResearchResultStore, Rust engine, judge, scanner/review: unchanged owners]
```

Created: `research-py/src/mqk_research/strategy_factory/` (`catalog_import`, `xlsx_reader`, `formalize`, `templates`, `known_index`, `dedup`,
`admission`, `pipeline`, `ai_normalize`, `knowledge`, `implementation`, `campaign`, `store`, `executor`, `scheduler`, `reporting`,
`status`, `scout`, `service`, `cli`) and the Rust `grammar_rule_v1` engine. Reused unchanged: `ResearchResultStore`, native signal
bridge, `stage_authorization`, `holdout_guard/incident`, judge, scanner/review, `strategy_mining` vocabulary, `exp_distributed.hashing`.
Modified owners (minimal, additive): `run_batch.py` (opt-in `--resume`; atomic index write), `mqk-cli` `bkt.rs` (4 call sites) and
`strategy_scanner.rs` (1) to register a named `grammar_v1` strategy; `mqk-strategy` engines module. `register_builtin_strategies*`
(daemon/runtime) is untouched, so no grammar strategy is deployable without a separate authorized builtin registration + Promotion.
Duplicate systems retired: none created; the Alpha Lab, `exp_distributed` simulator and per-batch scripts remain reference/EXP.

## Census closure (pre-edit F-01..F-14, plus findings during the work)

| ID | Final disposition | Evidence |
|---|---|---|
| F-01 generic importer | `FIXED+PROVEN` | 8 profiles import 668 entries exactly (147/121/36/26/86/40/48 + csv 147); unknown schema, subset violation, duplicate/empty ids, formulas, zip-bomb, DTD refused |
| F-02 field typing | `FIXED+PROVEN` | `formalize.py`; no default ever fills a parameter; decisions are INFERRED with a reference |
| F-03 cross-campaign dedup | `FIXED+PROVEN` | 212-entry known index (native cards, Census-01/02 grids cross-checked against the census authority), relationship classes, population duplicates, order invariance |
| F-04 executable path | `FIXED+PROVEN` | Rust `grammar_v1` (4 stateless exact-integer templates) + admission rules + controlled implementation workflow |
| F-05 generator/queue/resume/scheduler/report | `FIXED+PROVEN` | `campaign`, `store`, `scheduler`, `reporting`, E2E |
| F-06 runner re-evaluation / non-atomic index | `FIXED+PROVEN` (opt-in `--resume`; historical default unchanged) | unit reconciliation tests, kill/recovery E2E, 3 mutants |
| F-07 stage authorization | `ALREADY CORRECT+PROVEN` (consumed, never weakened) | preflight/readiness tests; Factory cannot mint (static test) |
| F-08 hard-coded XBI scan row | `OUT OF SCOPE / not triggered` | campaigns use symbols present in `config/instruments/equities.json`; an absent symbol still aborts review fail-closed (existing behaviour) |
| F-09 EXP simulator | `ALREADY CORRECT+PROVEN` | architecture test forbids importing `exp_distributed` runner/worker/strategies or `ml.economics*` |
| F-10 AI dependency | `PARTIAL`: local Ollama `deepseek-coder:6.7b` passed the golden extraction check on this machine; `nomic-embed-text` present but embeddings not exercised live; paid cloud `BLOCKED_DEPENDENCY` (no cost authorization) | live probe output below |
| F-11 operator visibility | `PARTIAL`: read-only `factory_status_v1` export + report; GUI/daemon route `BLOCKED_OWNERSHIP` | `status.py`, CLI `status --export` |
| F-12 real-data readiness | `PROVEN readiness; execution BLOCKED (operator authorization)` | E2E-12 on the verified local Batch-03 bars |
| F-13 net_pnl caveat / pending incident | `KEEP` | untouched; surfaced in readiness |
| F-14 synthetic vs official | `FIXED+PROVEN` | compile refuses a market grade for synthetic data (and vice-versa); report carries `promotion_eligible=false` |
| F-15 (found) 120 pre-existing failures in `experiments/m1_native_trend_campaign` subprocess/audit-sink tests on this laptop | `PRE-EXISTING, NOT CAUSED` | identical failure sets on pristine `origin/main` and on this branch (120 failed, 846 passed, 1 error on both) |
| F-16 (found) editable `mqk-research` install points at the main checkout | `OPERATOR NOTE` | use `PYTHONPATH=research-py/src`; pytest and every runner script already resolve the worktree |
| F-17 (found) stage `register` is single-pass by frozen test | `FIXED+PROVEN` additively | `--resume` converges; default still raises |

## Second adversarial sweep (all fixed with tests and mutants)

* Catalog reader: total-uncompressed-size bound and DTD/entity refusal (zip/XML bombs).
* AI prompt: `<source>` tags inside source text are neutralized so text cannot close the data block.
* AI evidence: a bare number elsewhere in the text ("50 shares") no longer counts as a stated parameter (unit/indicator cue required);
  the claimed value must be inside a verbatim span.
* Admission: the `UNRECOGNIZED` branch was dead code (caught by a negative test) and now yields `REJECTED_UNDERSPECIFIED`.
* Store: decision-reference clash detection used SQL `LIKE` on JSON (wildcard collisions); replaced by an exact unique key.
* Declaration/spec control files are written atomically; CLI maps campaign/scout refusals to exit 2 (an uncaught exception was found by a negative test).
* Report/status never default: sections are `PRESENT`/`EMPTY`/`UNAVAILABLE`; `status` on a missing store creates nothing.
* Declared data column outranks free-text keyword mentions in blocker derivation (diagnostics that merely mention news are no longer `UNSUPPORTED_DATA`).

## Proof

Python Factory suite (this machine): **212 non-E2E + 15 real-engine E2E = 227 passed**, 0 failed (final run). The first full-suite run
found two defects in the TESTS (E2E-12 shared a store that earlier scenarios had left `FAILED`; E2E-06 asserted immutability over a trial
whose attempt had not reached a terminal state, so a legitimate retry replaced its same-path partial artifacts); both were corrected
(own store; immutability asserted for registry-`succeeded` attempts only) and E2E-06 passed 3/3 repeats afterwards. Retry replacing the
partial artifacts of a never-terminal attempt is intended; durably terminal evidence is never rewritten. Rust: `cargo test -p mqk-strategy` 364 lib + doc/integration tests green
(including 10 new `grammar_rule_v1` tests with an independent reference over a pseudo-random walk); `cargo test -p mqk-backtest --lib` 131
passed; `mqk-cli` native-signal and sizing scenario tests 2 + 7 passed; `rustfmt --check` clean; `check_unsafe_patterns.sh` passes.

Mutation proof (`research-py/scripts/factory_mutation_proof.py`, in-place mutant, red tests required, byte-exact restore verified):
intake 20/20, AI 18/18, knowledge 8/8, store 9/9, campaign/executor/scheduler 17/17, resume 3/3, scout 11/11, implementation 5/5,
Rust `grammar_v1` 11/11 = **102/102 killed**. Surviving mutants found during development were each traced to a test gap
(or a redundant guard, which was removed) and closed before this record.

Integrated acceptance (`tests/test_strategy_factory_e2e*.py`, real entrypoints, real `mqk-cli`, SYNTHETIC bars):

| ID | Result |
|---|---|
| E2E-01 successful evaluation | one `run` call: 13 stages `succeeded`; 8 trials registered before the first attempt, 8 evaluated; frozen-declaration reproducibility |
| E2E-02 rejected candidate | all candidates honestly `rejected` (`negative_total_return`); no `paper_candidate`; no promotion artifact; no Paper/Live path touched |
| E2E-03 unsupported idea | futures/news/under-specified/diagnostic ideas dispositioned, not executable, campaign compile refused, nothing created |
| E2E-04 complete population | invalid grid combinations excluded visibly; registered = declared; one attempt per trial; judge population = declared; identities unique |
| E2E-05 concurrency | 3 campaigns, 2 real worker processes: every job exactly once, overlap observed, identical economics across campaigns, distinct experiments/registries |
| E2E-06 interruption | worker tree killed mid-`trials`; lease recovered; `interrupted` then `succeeded`; one success per trial; prior evidence byte-identical; no orphan `started` attempt |
| E2E-07 Python/Rust parity | native `absolute_momentum_252` vs `grammar_v1 abs_momentum 252`: identical net/gross/Rust return, trade count, exposure, turnover per symbol; declared fingerprints equal the CLI's; required history 253 both sides |
| E2E-08 statistical judgment | `evaluated`, `registered_unique_trials`= population, included+excluded = population |
| E2E-09 authority refusal | unreleased gate and missing authorization block before any data/registry access; holdout-reaching bars fail `holdout_pre` before any attempt; synthetic can never be graded as market evidence; no Factory stage reaches Promotion/Paper/Live |
| E2E-10 unattended bounded campaign | the same single `run`, durable queue/attempt rows, report with artifact hashes |
| E2E-11 scheduled/no-work | repeat pass executes 0 jobs and ends `NO_ELIGIBLE_WORK`; attempt counts unchanged |
| E2E-12 real-data readiness | verified local Batch-03 bars (official provider attestation) pinned; readiness names the unreleased gate, each missing authorization class and the pending holdout incident; `run` ends `BLOCKED`; no registry/data created; source files byte-identical |

Synthetic proof is software proof only. **No real-market Factory campaign was run**: that needs the operator's secret, gate release
and signed authorization, which the Factory neither holds nor creates.

## Real-catalog intake (deterministic, no model)

Eight files, 668 entries -> 521 unique ideas (147 are the csv copy of the Reddit workbook) with exactly one disposition each:
`DIAGNOSTIC_NOT_STRATEGY` 263, `UNSUPPORTED_DATA` 154, `DEFERRED_ASSET_CLASS` 50, `NEEDS_IMPLEMENTATION` 20 (multi-symbol),
`NEEDS_OPERATOR_POLICY` 14, `GOVERNANCE_CONTROL` 8, `NEEDS_FORMALIZATION` 7, `BENCHMARK_NOT_STRATEGY` 3, `DUPLICATE_OF_KNOWN` 1
(RDI-004 is Census-01 `S02_200`), `REJECTED_UNDERSPECIFIED` 1. Proposal kinds: 7 `STRATEGY_HYPOTHESIS`, 405 `MECHANISM_DIAGNOSTIC`.
**Zero ideas were admitted**: the catalogs are research questions, and the few rules named lack stated parameters. That is the honest
output, not a defect. Result hash `ff45ee3cdd46f569ae7d351d4a415b3698d1b17638d2d6a675d69083fd54a2c2` (grammar available).

## AI evidence

Live local probe on this machine: `ollama 0.40.0`, model `deepseek-coder:6.7b`, digest `ce298d984115…`; the golden extraction returned
`sma_trend_gate`, window 50 with a verified span (functional). Intake with a model was exercised in tests with injected providers
(valid, invalid, hostile, failing, non-conformant, budget-exhausted); a live end-to-end AI intake over the real catalogs was not run.

## Optimization (measured)

A 4-trial campaign: 43.8 s total, of which `finalize` 22.6 s (per-trial robustness/stress/placebo CLI+Python subprocesses), `trials` 5.5 s,
`backtest` 3.1 s, and about 1 s fixed process start per remaining stage. Per-trial parallel `finalize` is the only material gain and is
`DEFERRED` (shared SQLite registry writers + index ownership: risk exceeds benefit at current scale). No identity/reproducibility trade-off was made for speed.

## Deferred / blocked (not defects)

* Stateful and monthly templates, multi-symbol engines: `NEEDS_IMPLEMENTATION` by design (controlled workflow).
* GUI/daemon panel: `BLOCKED_OWNERSHIP`; the `factory_status_v1` export is the seam.
* Live web scouting and paid AI: implemented, policy-gated, not exercised.
* Prior-search denominator for pooled DSR/PBO remains the accepted `BLOCKED_UNSUPPORTED` (disclosed per strategy in the declaration).
* Short/long-short research remains `NEEDS_OPERATOR_POLICY` (no short/borrow authority in M1).

## Review (mqd-review-patch, read-only, by axis)

Contract: no BLOCKER/HIGH. MEDIUM: a retry reuses the frozen runner's per-trial directory, replacing the partial artifacts of an attempt that
never became terminal (terminal evidence is untouched; attempt-unique directories would be stricter but change the frozen layout).
INFORMATIONAL: `grammar_v1` names resolve in the Research CLI/scanner only (daemon registry untouched); the operator `release` command exists
but no scheduler/executor path can call it (static test); the Factory store is a Research-side SQLite file (no Postgres write path).
Quality: LOW unused imports and one redundant expression (fixed); mixed CRLF/LF working-copy noise from `core.autocrlf` (content LF).
Proof: MEDIUM (closed by correction R4) the real-engine E2E suite was skipped without a native binary; the strict native CI lane now proves it; LOW the mutation harness is local; a live AI intake over the real catalogs was not run.
Counts: contract 0 blocker / 0 high / 1 medium; quality 2 low; proof 1 medium / 2 low.

## Tooling

mqk_readonly / srclight: connected, not needed beyond direct reads (graft and native Git sufficed). graft: repo map and file APIs
(the tool estimated roughly 7.9M tokens avoided versus whole-file reads; an estimate, not a measurement). Context7/Firecrawl/Playwright: not needed (no third-party API ambiguity, no external source required, no GUI change).
rust-analyzer: not needed (compiler diagnostics sufficed). `cargo` runs were constrained (`-j 2`); no workspace-wide Rust run.
Full local workspace acceptance: NOT RUN — prohibited by laptop resource-safety rule; broad workspace proof delegated to GitHub CI.

## Independent-review correction (`V4-STRATEGY-FACTORY-INDEPENDENT-REVIEW-CORRECTION-01`)

Original Factory completion HEAD `b1c757e4708e803425724db17adf3facaa12cd49` (the correction start). The independent review (PARTIAL) raised R1-R5; each
is corrected on the same branch with a red-first test and mutants. Nothing was merged to `main`; pushes were fast-forwards to the development branch only.

| Item | Commit | Corrected semantics | Proof |
|---|---|---|---|
| R1 scheduler fail-open (HIGH) | `78885074` | The claim ownership boundary (`scheduler.run_one`) records every unexpected exception (before execution, in the executor, an invalid outcome) as an honest terminal `failed` with the exception type, written with the claim token. A claim lost to lease recovery is fenced (`ClaimLost`; the late result is discarded). If the store cannot record the result the job stays `running`, the pass ends `ERROR` and lists it in `unresolved_jobs`; lease expiry later records an `interrupted` attempt and re-queues the SAME job. A pass can no longer report `NO_ELIGIBLE_WORK` with an unresolved claim or a recorded scheduler error; `run` exits 4 on `ERROR`. No retry creates a job or a trial. | `test_strategy_factory_scheduler_faults.py` (11 tests: before claim, between claim and execution, in execution, at finalization transient/persistent, zombie worker, invalid outcome, cross-process hard crash with `os._exit`, in-process raise); 9/9 mutants (`faults`) |
| R2 promotion eligibility (HIGH) | `ca8a7f3e` | `contracts.promotion_view(grade)` is the single authority: `promotion_eligible` is false for every grade; readiness is `NOT_ELIGIBLE_SYNTHETIC` (synthetic) or `NOT_ESTABLISHED` (exposed development, anything unknown). Declaration, report and status all derive it from the grade; a stored flag is ignored. No new grade or policy exists. | `test_strategy_factory_authority_truth.py` (every supported grade, tampered stored flag, source scan); 7/7 mutants (`authority`) |
| R3 prior Factory search accounting | `c24e8dea`, `12c69847`, `c00dcf30` | Durable Factory history now feeds duplicate/adjacent recognition at intake and compile time and the declared `prior_search_disclosure`, using only the PREDECLARED strategy names of earlier campaigns (`store.prior_campaign_strategies` + `known_index.factory_prior_entries`); no attempt, result or verdict is read. A campaign that already exists is verified (spec hash, frozen declaration identity) and returned, never recomputed against newer history; a missing, corrupt or tampered frozen declaration is a `StoreError`. `create_campaign(expected_prior_campaigns=...)` refuses a campaign whose disclosed history is not the store history; the leftover declaration file of a refused attempt (no store row) is safely replaced on retry. | `test_strategy_factory_prior_search.py` (two sequential campaigns, restart, outcome independence, identity includes history, stale-history race and retry, damaged frozen declaration, intake path); 10/10 mutants (`history`; the PS-9 orphan-replace mutant was retired when that logic moved into the registry-published write, where mutant DC-4 covers it) |
| R4 CI native proof | `55993077`, `916e4482` | `.github/workflows/strategy-factory.yml` (triggers: pushes to `strategy-factory/**`, PRs to `main` touching Factory/engine paths, manual) builds `mqk-cli` with the pinned toolchain, runs the `grammar_v1` engine tests and the whole Factory suite with `MQK_FACTORY_REQUIRE_NATIVE=1` (a missing binary FAILS instead of skipping), and `scripts/guards/check_factory_native_lane.py` fails the job unless every load-bearing E2E test executed and passed. It is bounded: no workspace-wide Rust sweep (that remains `ci.yml`'s `rust` job). | GitHub run 38051841844 on `916e4482` (SUCCESS); `test_strategy_factory_native_lane.py`; 6/6 mutants (`lane`) |
| R5 status wording | `ca8a7f3e` | `status`/report carry `FACTORY_AUTHORITY`: scope `FACTORY_ACTIONS_ONLY`, `paper` and `live` = `NOT_TOUCHED_BY_FACTORY`, promotion `NOT_REQUESTED_BY_FACTORY`. The Factory does not read or assert the real Paper runtime state. | authority tests above |

### CI skip policy (exact)

The native lane tolerates ONLY these skips, each a proof that needs a machine-local resource a runner never has: E2E-12 (verified local Batch-03
bars), the live local-Ollama status test, and the two real-operator-catalog tests (catalog files). Every other skip, any failure or error, and
any absence of the 15 required tests (E2E-01..11 and the lane binary check) fails the lane. Locally, with those resources present, nothing skips.

### Second adversarial sweep over the corrected boundaries

Claim ownership and lease fencing (token per attempt; stale finish/heartbeat refused; recovery never touches earlier attempt rows), exception
finalization, trial/attempt identity (retries append attempts, never trials), promotion truth, history deduplication, stale-history refusal
and retry, atomic declaration replacement, CI required-versus-optional skips, cross-platform termination, status authority. Findings (all
fixed, tested, mutant-covered): the declaration file left by a refused stale-history compile blocked its own retry; a damaged frozen
declaration surfaced as a raw error on the idempotent compile path; the `run_until_idle` post-loop `ERROR` guard was unreachable (the loop already
stops on an `ERROR` pass) and was removed; the `SC-2` mutant fragment had gone stale after the R1 rewrite and was repaired. A test-isolation flaw (the
CLI test inherited `MQK_FACTORY_CLI`) and a timing-dependent assertion in the crash-recovery test were corrected.

### Correction evidence

Final local acceptance at the correction HEAD: `pytest tests -k strategy_factory` with the real native `mqk-cli` and `MQK_FACTORY_REQUIRE_NATIVE=1`:
**263 passed, 0 skipped** (guard: 0 problems). Mutation harness, all sets at the correction source: intake 20/20, ai 18/18, knowledge 8/8, store 9/9,
campaign/executor/scheduler 17/17, resume 3/3, impl 5/5, scout 11/11, authority 7/7, lane 6/6, history 10/10, faults 9/9, Rust `grammar_v1` 11/11
(the Rust sources are unchanged since `b1c757e4`; the engine tests also ran in CI). Full local workspace acceptance: NOT RUN - prohibited by laptop
resource-safety rule; broad workspace proof delegated to GitHub CI. The native lane (run 38051841844) is NOT a full workspace CI run.
The 120 `experiments/m1_native_trend_campaign` failures are identical on untouched `origin/main` and are unrelated.

## Declaration concurrency closure (`V4-STRATEGY-FACTORY-DECLARATION-CONCURRENCY-CLOSURE-01`)

Starting HEAD `692f3649d49334e640d4096111708e4b9f8c25cb`. An independent review reproduced an identity mismatch: `FactoryService.compile_campaign`
wrote `declaration.json` / `spec.json` BEFORE `FactoryStore.create_campaign` froze the identity, so two processes compiling one campaign id could
both see "not registered"; the later writer replaced the registered winner's file and then received the legitimate conflict refusal, leaving the
frozen row and its file inconsistent. The shared `.tmp` name made concurrent writers collide as well.

**Exact reproduction (red before the fix):** worker A and worker B (real processes, file handshakes, no sleeps) both stop at the moment of
registration; A registers, then B proceeds. On the unfixed code B's file write had already replaced A's; after the fix the registered row, the
file identity and `spec.json` are byte-for-byte A's and B receives `exists with a different predeclaration`.

**Corrected ownership boundary.** The registry write transaction is the single ordering point. `create_campaign(..., publish=...)` runs `publish`
(atomic write of `declaration.json` and `spec.json`) INSIDE the transaction, after every refusal check (existing row, stale prior history,
case alias) and the inserts, before the commit. Consequences, each proven by a test:

* a losing or refused compile never touches the winner's frozen files, and a stale-history loser leaves no files at all;
* identical concurrent compiles are idempotent (one population, one set of jobs); different campaign ids stay independent;
* an exception in `publish`, or a hard process death after the files are written and before the commit, rolls the registration back: the state
  is unregistered and the next compile simply replaces the leftover files (an unregistered orphan is never read, so a corrupt orphan cannot block it);
* a death right after the commit leaves a complete, verifiable registered campaign; recompiling returns it unchanged;
* a REGISTERED declaration that is corrupt, tampered or missing fails closed (`compile` and `release_gate`) and is never rewritten.

**Directly connected defects found by the caller/callee census and sweep (all fixed with red-first tests and mutants):**
campaign ids that differ only by case (one directory on Windows), end with `.`, or are reserved device names (`CON`, `NUL`, `COM1`, ...) are
refused; the status export and the report files used a fixed `.tmp` name / in-place writes and now use the shared unique-temp, fsync, atomic
`contracts.atomic_write_text`; `release_gate` raised a raw error on an unreadable declaration (now `StoreError`); the generated xlsx test
fixture embedded the wall-clock zip timestamp, which made an unrelated AI-normalization test fail intermittently across a second boundary.

**Evidence.** `test_strategy_factory_declaration_concurrency.py` (23 tests); mutant set `owner` 11/11 killed (publish not in the registry, publish
before the refusal checks, shared temp name, orphan kept, case alias, reserved names, trailing dot, no rollback, status/report in place, wall-clock fixture).
Full Factory suite with the real native `mqk-cli` and `MQK_FACTORY_REQUIRE_NATIVE=1`: **287 passed, 0 skipped**, lane guard 0 problems. All earlier
mutant sets were re-run against the changed sources (intake 20, ai 18, knowledge 8, store 9, campaign 17, resume 3, impl 5, scout 11,
authority 7, lane 6, history 10, faults 9, owner 11, Rust 11: all killed, byte-exact restores). Protected scope unchanged: no GUI file, Paper, Live, holdout, broker, migration or
trial-identity change; R1-R5 behaviour preserved. Full local workspace acceptance: NOT RUN - prohibited by laptop resource-safety rule; broad
workspace proof delegated to GitHub CI.
