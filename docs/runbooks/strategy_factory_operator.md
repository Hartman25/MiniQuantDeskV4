# Strategy Factory — operator runbook

The Factory coordinates existing owners: catalogs -> typed ideas -> deterministic dedup/admission -> a frozen predeclaration -> the
stage-authorized batch runner (`ResearchResultStore`, the Rust native engine, the judge, scanner/review) -> a report. It grants no
Promotion, Paper or Live authority and never mints a stage authorization or consumes the reserved holdout. Everything below is
read-only or writes only the Factory's own state (`factory.sqlite3` and `campaigns/<id>/`) unless stated otherwise.

## 0. Setup

```bash
# from the repository root; the editable install of mqk-research may point at ANOTHER checkout, so pin the source tree
export PYTHONPATH=research-py/src
export MQK_FACTORY_ROOT=research-py/runs/strategy_factory      # git-ignored state directory (default)
export MQK_FACTORY_CLI=<path to mqk-cli.exe>                    # native binary (see below)
python -m mqk_research.strategy_factory --help
```

Native binary (only needed to detect `grammar_v1` and to run stages; a laptop-safe, constrained build):

```bash
cd core-rs && CARGO_TARGET_DIR=C:/tmp/mqk-target-factory cargo build -p mqk-cli -j 2
```

Without a binary the Factory still imports, formalizes, deduplicates and dispositions every idea; it reports `grammar_v1` as
unavailable and executes nothing.

## 1. Import catalogs

```bash
python -m mqk_research.strategy_factory import-catalog catalog.xlsx other.csv
```

* Accepted only through a profile whose header signature matches exactly. Eight built-in profiles cover the operator catalogs of
  2026-10-09. An unknown schema is refused (exit 2). For a new schema write a profile JSON
  (`strategy_factory_catalog_profile_v1`: `profile_id`, `catalog_family`, `file_format`, `sheets[{sheet, role: primary|subset|additional,
  id_column, entry_kind}]`, `field_map` over the canonical fields, `label_columns`, optional sources table) and pass `--profile file.json`.
* Every row of every declared entry sheet becomes an entry with its original cells verbatim; subset sheets must be subsets of the
  primary sheet; other sheets are kept as context only; formulas are never evaluated and any in an entry sheet are refused.
* Importing the same bytes twice is a no-op. The xlsx and csv copies of one catalog share a `catalog_family`; identical entries
  collapse into one idea (recorded as `DUPLICATE_SOURCE_COPY`), differing content for the same id is refused.

## 2. Intake: formalize, deduplicate, disposition

```bash
python -m mqk_research.strategy_factory intake [--ollama-model <model>] [--max-ai-calls N]
python -m mqk_research.strategy_factory ideas [--disposition NEEDS_FORMALIZATION]
```

Every entry gets exactly one disposition; the counts always sum to the submitted population.

| Disposition | Meaning | Next step |
|---|---|---|
| `ADMITTED_GRAMMAR` / `ADMITTED_NATIVE` | fully specified, not a duplicate, verified executable path exists | may be named by a campaign (`admitted_ideas` source) |
| `DUPLICATE_OF_KNOWN` | exact duplicate of a native engine, Census-01/02 coordinate or earlier idea | not re-run; a campaign may re-evaluate a native duplicate only with `allow_reevaluation_of_known: true` |
| `NEEDS_FORMALIZATION` | recognized rule, named parameters unstated (never defaulted) or prose not mapped to a template | record operator decisions (section 3) |
| `NEEDS_OPERATOR_POLICY` | direction/short/execution policy needed | operator decision |
| `NEEDS_IMPLEMENTATION` | recognized and well specified (or multi-symbol), no verified engine | `implementation request` (section 5) |
| `UNSUPPORTED_DATA` / `DEFERRED_ASSET_CLASS` | needs news/ML/fundamental/intraday/order-flow data, or futures/options/FX/crypto | none until MQD holds that data/asset support |
| `DIAGNOSTIC_NOT_STRATEGY` / `GOVERNANCE_CONTROL` / `BENCHMARK_NOT_STRATEGY` | not a candidate trial | route to factor diagnostics / testing controls |
| `REJECTED_UNDERSPECIFIED` | nothing computable can be recovered | none |

Relationship classes (`EXACT_DUPLICATE`, `PARAMETER_VARIANT`, `SEMANTIC_VARIANT`, `MIRROR`, `COMPOSITE_OF_EXISTING`,
`UNKNOWN_NEEDS_REVIEW`, ...) are derived from template signatures only; no result, rank or P&L is read. Lexical/embedding neighbours
are advisory.

## 3. Operator decisions (never silent defaults)

```bash
python -m mqk_research.strategy_factory decide parameter  d.json   # {"intake_id","param","value","decided_by","rationale","decision_ref"}
python -m mqk_research.strategy_factory decide field      d.json   # {"intake_id","field": direction|asset_class,"value",...}
python -m mqk_research.strategy_factory decide novelty    d.json   # {"intake_id","relationship",...} resolves UNKNOWN_NEEDS_REVIEW only
python -m mqk_research.strategy_factory intake                      # re-derives dispositions with the decisions applied
```

A decision can fill only a missing parameter (never override a source-stated one), must lie inside the template domain, is recorded
as `INFERRED_RULE` with `operator_decision:<ref>`, and is append-only.

## 4. Optional AI normalization

```bash
python -m mqk_research.strategy_factory ai probe --ollama-model deepseek-coder:6.7b
```

The model only proposes. A parameter becomes `EXPLICIT_SOURCE_RULE` only when its quoted span is verbatim in the source, contains the
value and carries a unit/indicator cue; every other model value is a suggestion and the parameter stays missing. A backend that fails
the golden extraction check is `BLOCKED_DEPENDENCY`, not functional. Provider, model, version, digest, prompt/response hashes and the
raw response are stored apart from every economic identity. Ollama must be loopback; a cloud provider needs an explicit
`cost_authorization_ref` and is available only through the library API (the CLI never enables a paid provider).

## 5. New executables (controlled workflow)

```bash
python -m mqk_research.strategy_factory implementation request <intake_id>   # inert request for a recognized, fully specified idea
python -m mqk_research.strategy_factory implementation check   <strategy_id> # card + Rust registration + tests + operator authorization
```

No generated code is ever admitted because it compiles. A native strategy may be named by a campaign only with a semantic card, a
Rust registration, deterministic tests and a committed `docs/research/admissions/<strategy_id>.json` authorization. The verified
`grammar_v1` engine (Research/Backtest-only; `sma_trend_gate`, `dual_sma_cross`, `abs_momentum_sessions`, `near_high_proximity`) needs
no new code: its name is its complete specification and the daemon/runtime registry does not know it.

## 6. Campaigns

Spec (`factory_campaign_spec_v1`): `campaign_id`, `evidence_grade` (`SYNTHETIC_DIAGNOSTIC` | `EXPOSED_DEVELOPMENT`),
`protocol_profile` (`m1_batch03_v1`, pinned by content hash), `predeclared_utc_date`, `population.sources`
(`native` | `grammar_grid` | `admitted_ideas`), `population.symbols`, `population.max_trials`, `data` (reuse of a verified bars directory
with SHA pins), `partition` (with a fixed `holdout_boundary`).

```bash
python -m mqk_research.strategy_factory campaign compile spec.json     # freezes the complete trial population + declaration identity
python -m mqk_research.strategy_factory campaign readiness <id>        # exact missing prerequisites (exit 3 if not ready)
python -m mqk_research.strategy_factory campaign release <id> --operator <you> --approval-ref <ref>   # OPERATOR: releases the gate only
python -m mqk_research.strategy_factory run --workers 2                # drains eligible work, ends with an explicit reason
python -m mqk_research.strategy_factory campaign show <id> | report <id>
python -m mqk_research.strategy_factory status [--export status.json] # read-only, never creates a store
```

Costs, sizing, benchmark and stress come only from the frozen profile; the Factory cannot choose economics. Synthetic data is refused a
market evidence grade and official data cannot hide as synthetic.

**Authorization is the operator's.** Every effectful stage needs a signed `m1_stage_authorization_v1` bound to the exact declaration
identity and the exact native binary hash, minted with your secret (`MQK_M1_STAGE_AUTH_KEY`, >= 32 chars, outside the repository)
via the accepted `stage_authorization.mint`, valid <= 7 days, acknowledging any blocking holdout incident. Place it at
`campaigns/<id>/stage_authorization.json` (or set `MQK_M1_STAGE_AUTHORIZATION`). Without it jobs end `blocked` with the precise reason.

## 7. Scheduling and unattended operation

`run --until-idle` (default) executes passes until nothing is eligible and exits 0 (no work / all done), 3 (work remains but a
prerequisite is missing), 4 (a stage failed, or the scheduler itself could not complete or record something: `ERROR`, with `errors` and `unresolved_jobs` in the pass result). Run it from Windows Task Scheduler or cron, for example hourly; a pass with no eligible
work executes nothing. Concurrency: `--workers N` jobs at once across independent campaigns; one stage at a time within a campaign.
Several `run` processes may share one store: claims are atomic and every job runs exactly once.

## 8. Recovery

* Worker killed: its lease expires; the next `run` records an `interrupted` attempt and re-queues the same stage with `--resume`.
  Orphaned `started` registry attempts are finalized `failed` with `interrupted_before_terminal_result`; completed trials are never
  evaluated again; a genuine failed attempt is terminal (no outcome-dependent retry); prior evidence is never rewritten.
* An unexpected exception in a stage is recorded as `failed` with its type; if the store cannot record an outcome the job keeps its lease, the pass
  ends `ERROR`, and lease expiry later re-queues the same job as `interrupted`. A late result from a recovered claim is discarded.
* `blocked`: supply the prerequisite and run again (blocked work is re-evaluated once per `run`).
* `failed`: investigate, then `retry <id> <stage> --reason "..."` (appends an attempt; never creates a new trial).
* A `fail-closed: ... reconcile manually` message means a durable artifact the registry points to is missing; nothing is invented.

## 9. Where results live

`campaigns/<id>/` holds `spec.json`, `declaration.json` (frozen identity), `registry/research.sqlite3` (trials, attempts, judge artifacts, holdout
ledger), `trials_index.json`, per-trial economic/backtest/robustness/stress/placebo artifacts, `scan/*/reviews`, `judge/judge.json`,
`batch_results.json`, `holdout_guard_{pre,post}.json` and `factory_report/report.{json,md}` (every section `PRESENT`, `EMPTY` or
`UNAVAILABLE` with a reason; artifact SHA-256s included). `factory.sqlite3` holds imports, ideas, decisions, AI provenance, queue and events.

## 10. Web scouting (operator-approved sources only)

```bash
python -m mqk_research.strategy_factory scout --policy approved_sources.json https://<approved host>/<approved path>
```

The default policy is empty, so nothing is fetched. A policy lists approved `domain`, `source_class`, `path_prefix`, rate limit and
size cap. Fetches are https-only, honour robots.txt, accept text only, and land byte-for-byte in `quarantine/` (never executed). Pages
enter the ordinary intake as untrusted ideas.

## 10b. Campaign history and compile

`compile` is idempotent: recompiling an existing campaign id verifies its spec and frozen declaration and returns it; a changed spec, or a missing
or tampered declaration, is refused. A new campaign discloses (and is checked against) every earlier Factory campaign by predeclared strategy name;
results never influence it. If another campaign is predeclared while a compile is in flight the compile is refused and must simply be re-run.

## 10c. CI evidence for the native path

The `Strategy Factory native lane` workflow builds `mqk-cli` and runs the Factory suite with the native binary REQUIRED; only the machine-local
optional skips named in `scripts/guards/check_factory_native_lane.py` are tolerated. Locally, set `MQK_FACTORY_CLI` to the built binary
(and `MQK_FACTORY_REQUIRE_NATIVE=1` to make an absent binary a failure).

## 11. What stays gated

Promotion, Paper and Live are not reachable from any Factory command (a test asserts it). Final-holdout windows are never read; a
holdout-reaching bars file fails `holdout_pre` before any attempt. The pending access incident `HOA-KISS-EXT032-01` blocks independence
claims and Promotion/Paper authorizations until the operator adjudicates it through the authenticated process.
