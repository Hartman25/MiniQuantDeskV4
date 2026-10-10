# M1 Remaining Gates — Current Closure Census

Date: 2026-10-09 (Pacific/Honolulu)
Controller: `V4-M1-ACCEPTED-BRANCHES-ALPHA-REUSE-AND-CLOSURE-CENSUS-01`
Status: `OPEN` — local integration only; not independently accepted; no push.

This is a finite current-state census, not a claim that green CI or local code proof makes Paper deployment ready. The canonical owners remain the existing Research registry, native strategy/backtest/evidence path, Promotion gate, and daemon `active_paper` authority. The Alpha Lab remains read-only archival reference.

## Integrated local baseline

| Fact | Evidence / status |
|---|---|
| Verified `origin/main` | `a49cae1f9a2849c769707d239a61e10cb0f39903` via read-only `git ls-remote` |
| Accepted four-way tip | `8996b99177238a83bde6984ab0efed715988b3a7`; local merge `5144aa769535d424f6381778e4c9f9fc60d16959` |
| Accepted EXT-032 tip | `6bac3369f7da3074f7e0bb69a83864d469b037ed`; local merge `722a04a426db73347a6c55839e4b61772a858a0b` |
| Local branch | `integration/m1-alpha-reuse-closure-01` in `C:\Users\Zacha\Desktop\MiniQuantDeskV4-M1-Integration` |
| Remote PR state | PRs #74 and #75 exist; neither was merged by this controller. Connector status lookup returned no individual checks, so the historical 6/6 claims are not re-asserted as independently verified here. |
| Local integration CI | Focused proof only; exact-head GitHub CI remains pending a later guarded publication. |

## M1-REMAINING-GATES

| # | Gate / owner / exact route | Status | Evidence | Precise next action | Code change? |
|---:|---|---|---|---|---|
| 1 | Accepted combined tree / Git + CI; integration branch and merge parents | `PROVEN` locally | Exact remote tips verified; two normal no-FF merge commits; no conflict; protected worktrees unchanged | Independent Git/diff review, then guarded publication and exact-head GitHub CI | No |
| 2 | Qualified US equity/ETF candidate; Research registry → native evidence → Promotion | `BLOCKED_OPERATOR` | No known qualified candidate; Batch 03 is 0/60 positive cost-aware alpha; EXT-032 is rejected archival evidence | Do not register a winner or rerun; operator must authorize a new prospective policy/campaign with a qualified population | No |
| 3 | Exposure/OOS and Final Holdout; holdout ledger + Promotion veto | `BLOCKED_OPERATOR` | Earlier windows are exposed; `HOA-KISS-EXT032-01` is `ACCESS_INCIDENT_PENDING_ADJUDICATION`; `[2026-03-01, ...)` remains reserved pending authenticated operator adjudication | Operator must adjudicate through the authenticated incident process; no simulated adjudication, HMAC generation, or holdout read | No |
| 4 | OD-1..OD-8 economic/operator policy choices; latest accepted ledger authority | `BLOCKED_OPERATOR` | Current authority leaves OD-1, OD-2, OD-4, OD-6, OD-7 and OD-8 unresolved; settled choices are not reopened | Obtain only the still-blocking decisions, then issue a frozen non-executable predeclaration | No |
| 5 | Provider/symbol/calendar bars for any new identity; loader/provenance/calendar owners | `BLOCKED_OPERATOR` | New identities require explicit provider/symbol/calendar scope; v2 calendar siblings do not grant v1 deployment authority | Declare a new prospective identity and prove its provider, symbol, session and corporate-action contract | Maybe, only if a declared identity requires it |
| 6 | Trial/attempt/evaluation identity, complete loser population, DSR/PBO and prior-search denominator; `ResearchResultStore` + ML judge | `BLOCKED` | Registry and attempt/slice authority exist; pooled DSR/PBO is not computable from the current accepted population without unsupported assumptions | Preserve `BLOCKED_UNSUPPORTED`; complete the whole-population denominator under an authorized campaign | No |
| 7 | Native causal execution, matched benchmark, OOS/stress, scanner/review, Promotion; Rust/native path + `mqk-promotion` | `PROVEN` for existing gates, not candidate readiness | Canonical owners and accepted negative evidence remain intact; `paper_candidate` is not `active_paper` | Run only a genuinely prospective candidate through every gate after rows 2–6 clear | No |
| 8 | Paper host read-only preflight; `docs/runbooks/m1_9_paper_deployment_preflight.md` | `UNAVAILABLE_NEEDS_PROOF` | Prior read-only report stated Paper DB schema 76 vs required 91; this controller did not access the real Paper DB | Obtain a separately authorized read-only schema verification; do not migrate, arm, clear HALTED, or enable Paper | No |
| 9 | Genuine Paper order/fill/position/reconcile lifecycle and genuine no-trade lifecycle; daemon/provider operational authority | `BLOCKED_OPERATOR` | No genuine candidate, active Paper identity, provider readiness, or operational validation was established; broker/provider calls were zero | After candidate and Paper preflight authorization, execute the separate real operational validation | No |
| 10 | Scheduler/host sleep/unattended start/held-state recovery/M1.10; daemon + `m1_10_finite_validation` | `BLOCKED_OPERATOR` | Countable 10 market sessions plus 5 consecutive clean sessions have not started; no count is fabricated; M1.10 must restart after final repair | Complete deployment identity proof, then observe the required real session sequence | No |

## Alpha Lab KEEP / ADAPT / RETIRE

| Alpha reference | Disposition | Canonical destination |
|---|---|---|
| `research-py/experiments/local_alpha_lab/lab.py`, `cli.py`, `run_local_alpha_lab.ps1` on `codex/local-alpha-lab` | `RETIRE` as active authority; preserve branch, ignored `runs/`, reports and hashes unchanged | None; no Alpha runner, schema or economic loop is copied |
| Alpha manifest/hash/provenance and read-only fence tests | `ADAPT` only where a canonical loader/provenance/calendar seam is missing | Existing `mqk_research.data` loaders, corporate-action provenance and session calendar |
| Alpha declaration hash, immutable population and result-independent identity checks | `ADAPT` only where missing | Existing `exp_distributed` registration and `ResearchResultStore` |
| Alpha lock/path/foreign-DB/crash/recovery/report/hash controls | `ADAPT` only where a canonical invariant is absent | Existing `ResearchResultStore`, runner and artifact snapshot authority |
| Corrected EXT-032 campaign and report SHA-256 `cae8ccdc763996134ad55a96b7967f17c0a11998039780420e44f74e68671770` | `KEEP` as immutable rejected archival evidence | `docs/research/M1_KISS_EXT032_CLOSEOUT.md` and incident ledger |
| Historical per-trade `net_pnl_usd` double-counting caveat | `KEEP` as `KNOWN_INCORRECT_REPORTING_FIELD`; no historical rewrite | Archival caveat only; canonical account-equity/benchmark alpha remains authoritative |

EXT-032 archival decision remains negative: SPY alpha `-$26,776.13`, DIA `-$25,975.92`, QQQ `-$42,646.00`, IWM `-$24,239.17`; four registered trials and four terminal attempts are not native Promotion evidence.

## Defect census and second sweep

- `runner.py::_finalize_candidate_attempts`: `FIXED+PROVEN` by the bounded patch in this integration tree. Aggregate attempt status/reason now use distinct names from per-slice status/reason. Two tests cover failed/then-succeeded and succeeded/then-failed order, with immutable slice status and failure count assertions. The test code is present, but the supplied runtime lacks `pytest` and `PyYAML`, so RED/GREEN execution was unavailable in this environment; syntax/test execution must be repeated in a provisioned research environment.
- Trial/attempt/slice registration, immutable slice snapshots, retries, identity, holdout guard, native economic bridge, scanner/review, Promotion and `active_paper` authority: `ALREADY_CORRECT+PROVEN` by the accepted branch evidence and adjacent source/tests; no duplicate owner was added here.
- Alpha Lab active runner, independent runs schema, and Python economic loop: `RETIRE` as an active path; no copy exists in the integration tree.
- Real Paper DB schema, provider readiness, genuine Paper lifecycle, and M1.10 market-session count: `BLOCKED` or `UNAVAILABLE_NEEDS_PROOF`; no provider, broker, Paper DB, migration, order, fill, or holdout call was made.
- Second adversarial sweep of changed paths and adjacent callers: no additional ordinary deterministic defect was found in this bounded review. The unresolved rows above are operator/economic/runtime proof gates, not silently closed code defects.

## Tooling and side effects

Inspected: repository-local `.agents`/`.codex` inventory (no MQD-specific skill surfaced), available Codex tool catalog (no `mqk_readonly`, Srclight, or Graft capability surfaced), bundled workspace dependency paths, Git, and the GitHub connector for PR existence and commit-status lookup. Used: native Git, focused `rg`/PowerShell reads, `apply_patch`, bundled-runtime discovery, and GitHub read-only calls. No provider, broker, real Paper DB, migration, order/fill, or remote write was performed. `smoke_logs/`, the main checkout, GUI worktree and Alpha Lab worktree were not modified.
