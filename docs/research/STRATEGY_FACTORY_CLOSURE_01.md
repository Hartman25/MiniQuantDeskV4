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
| E2E-02 rejected candidate | all candidates honestly `rejected` (`negative_total_return`); no `paper_candidate`; no promotion artifact; Paper INACTIVE |
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
Proof: MEDIUM the real-engine E2E suite is skipped without a native binary, so CI does not re-prove it (a CI-visible Python/Rust domain
parity test and all unit/mutation-backed proofs do run); LOW the mutation harness is local; a live AI intake over the real catalogs was not run.
Counts: contract 0 blocker / 0 high / 1 medium; quality 2 low; proof 1 medium / 2 low.

## Tooling

mqk_readonly / srclight: connected, not needed beyond direct reads (graft and native Git sufficed). graft: repo map and file APIs
(the tool estimated roughly 7.9M tokens avoided versus whole-file reads; an estimate, not a measurement). Context7/Firecrawl/Playwright: not needed (no third-party API ambiguity, no external source required, no GUI change).
rust-analyzer: not needed (compiler diagnostics sufficed). `cargo` runs were constrained (`-j 2`); no workspace-wide Rust run.
Full local workspace acceptance: NOT RUN — prohibited by laptop resource-safety rule; broad workspace proof delegated to GitHub CI.
