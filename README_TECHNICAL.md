# MiniQuantDeskV4 (Veritas Ledger) — Technical README

*Veritas Ledger* is the presentation name of MiniQuantDeskV4; this is one repository and one codebase.

## 1. Purpose of this file

Technical orientation for a serious reader: how the repo is organized, how truth flows through it, what the
current sizing and safety contracts are, how proof is done, and how to set up and operate the local tools.

It is a summary, not a ledger or a patch history. Current program status is in
[`MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md`](MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md) and
[`docs/CURRENT_MISSION.md`](docs/CURRENT_MISSION.md); patch history lives in git and in `docs/specs/`,
`docs/research/` and `docs/audits/`. Where this file and those sources disagree, they win, and committed code and
tests win over all documents. Use the root [`README.md`](README.md) for the short project overview.

## 2. Current technical posture

| Topic | State |
|---|---|
| Current target | **M1 — US Equity/ETF Paper Production** (open) |
| Accepted baseline | `origin/main` `92d337b67670b2858aa24c00b2768a2ca04cc212` |
| Native Sizing V1 | **PUSHED-VERIFIED** — CI #621, run 37145565796, SUCCESS, 6/6 jobs, exact pushed head |
| Research Batch 01 | **BATCH_REJECTED** (3 hypotheses × 5 symbols, 0 `paper_candidate`) |
| Research Batch 02 | **NOT STARTED — next** (frozen directions only: turn-of-month, Halloween / November–April, 50-day range breakout with 10-day hold) |
| Promoted candidate | **None** |
| Paper deployment | **Inactive** — no deployed strategy; pending a valid promotable candidate |
| Final holdout | **RESERVED / UNCONSUMED** |
| Live | **DISABLED / NOT READY** |

Open M1 gates: candidate discovery and the promotion path; M1.9 (verification of actual deployed Paper state);
M1.10 (ten countable autonomous Paper sessions, five of them consecutive and clean). Historical genuine Paper
trade-lifecycle and no-trade-lifecycle evidence remains accepted but does not imply a currently deployed strategy.

M2–M10 may have meaningful code or foundation work (multi-symbol dispatch, asset-neutral quantity types, registry v2,
Live trust-chain scaffolding), but none is operationally complete or the current target.

## 3. Technical architecture overview

```mermaid
flowchart TB
  subgraph Inputs
    MD[Market data and provider ingest<br/>md_bars]
    RP[research-py<br/>hypotheses and trials]
  end
  subgraph Core[Rust core]
    BT[Backtest and native strategies]
    PR[Scanner review and Promotion gate]
    RT[Runtime and execution<br/>risk, integrity, OMS]
    PF[Portfolio and reconcile]
  end
  DB[(Postgres<br/>runs, outbox, inbox, evidence)]
  BR[Broker boundary<br/>Alpaca Paper adapter]
  OP[Operator plane<br/>mqk CLI, mqk-daemon, mqk-gui]
  MD --> BT
  RP --> PR
  BT --> PR --> RT
  RT --> BR --> PF
  RT --- DB
  PF --- DB
  PR --- DB
  DB --> OP
```

- **Rust core (`core-rs/`)** — authoritative for execution, accounting, promotion and control.
- **Python research (`research-py/`)** — research only, no execution authority; emits deterministic artifacts the Rust stack verifies.
- **Postgres** — durable run state, outbox/inbox, OMS and portfolio state, promotion and evidence records. After a restart the DB is authoritative over any in-memory state.
- **Daemon (`mqk-daemon`)** — HTTP control plane and session/runtime owner; default bind `127.0.0.1:8899`; privileged routes fail closed without `MQK_OPERATOR_TOKEN`.
- **GUI (`core-rs/mqk-gui`)** — Vite/React operator console; every data-bearing daemon response carries an explicit `truth_state` and the GUI hard-blocks on non-active states rather than rendering friendly defaults.
- **Broker/provider boundaries** — broker adapters (`mqk-broker-paper`, `mqk-broker-alpaca`, `mqk-broker-ibkr`) and market-data providers sit behind typed interfaces. Broker truth is authoritative for broker lifecycle events; data provenance is semantic and versioned, never inferred from a file name.

`MAIN` is the canonical engine. `EXP` is experimental and is not operational truth unless explicitly promoted.

## 4. M1 path: Research → Backtest → Promotion → Paper

```mermaid
flowchart LR
  H[Predeclared hypothesis<br/>and trial registration] --> BT[Causal Backtest<br/>cost and execution aware]
  BT --> SC[Scanner and review<br/>Benchmark V2]
  SC --> PG[Promotion gate<br/>bound evidence]
  PG --> PD[Paper deployment]
  PD --> VAL[Finite autonomous<br/>Paper validation]
```

The diagram shows the designed path, not the current position. No candidate has yet cleared scanner review, so
nothing has reached the Promotion gate; the later stages exist as engineering but are not exercised for any
strategy.

Where identity binds, and what fails closed:

- **Strategy identity** — a native engine id plus a semantic fingerprint. Promotion requires the Research evidence, the Backtest report and the deployed engine to agree on it; a Research-only strategy with no native engine cannot become a deployable identity.
- **Data identity** — the exact bars used (provider, adjustment, provenance) are bound to the evidence. Unknown provider, adjustment or provenance authority is refused. A hash proves integrity, not authority.
- **Benchmark identity** — scanner/review alpha is measured against an explicit, versioned benchmark policy. For exact-target native strategies this is Benchmark V2 (`capital_matched_exact_target_buy_hold_v1`); with Native Sizing V1 it is `capital_fraction_matched_passive_buy_hold_v1`. Cross-substitution between policies, and missing or mismatched review rows, are refused.
- **Sizing identity** — the sizing policy and its parameters are part of the strategy's semantic fingerprint (see §5).
- **Execution identity** — Promotion requires the review row's candidate config and run to match the canonical Backtest evidence, including execution model and integrity settings.
- **Trial discipline** — hypothesis = idea, trial = unique candidate, attempt = invocation, slice = window. Trials are registered before any result; retries and windows do not inflate unique-trial counts. The final holdout is not touched by campaign work.

Specifications and results: `docs/research/` (including `Research_Backtest_V1_Closeout_Audit.md`,
`BENCHMARK_V2_REVIEW_AUTHORITY.md`, `BENCHMARK_V2_PROMOTION_ECONOMIC_BINDING.md`, `M1_BATCH01_CORRECTED_RESULT.md`).

## 5. Current sizing contract

`fixed_initial_capital_fraction_v1` (**FixedInitialCapitalFractionV1**) is accepted and pushed-verified. Full
contract: [`docs/research/NATIVE_SIZING_V1_CONTRACT.md`](docs/research/NATIVE_SIZING_V1_CONTRACT.md).

- Selected only explicitly; the default policy remains the historical `fixed_quantity_v1`, and every historical config id, run id and fingerprint is unchanged.
- `allocation_fraction_bps` is an explicit integer in `1..=10000`. No default, no floats; missing, zero, oversized or malformed values are refused.
- Capital basis is the immutable **initial** allocated strategy capital — never current equity, P&L or a broker balance.
- Entry quantity: `floor(floor(initial_capital × bps / 10000) / reference_price)` whole shares, where the reference price is the close of the fully completed bar that emitted the entry. A budget that cannot buy the minimum quantity is refused; there is no one-share fallback.
- Quantity is fixed for the position lifetime. Existing caps can only reduce or refuse.
- One shared pure resolver is used by Backtest, scanner and the Paper contract.
- The Paper/runtime seam is restart-recoverable (durable held-sizing state) and refuses any contract mismatch. **Paper is not activated** under this policy; activation needs its own authorized mission.

## 6. Execution and safety invariants

- **Causal execution** — a signal computed on a completed bar executes no earlier than the next bar; no same-bar fills; the prediction label is never treated as executable P&L.
- **Deterministic** — stable ordering, canonical serialization, fixed seeds, content-addressed identity, deterministic (UUIDv5) audit ids; no dependence on unspecified row or hash-map order.
- **Fail closed** — unavailable authority results in a block, a refusal, `unavailable`/`not_evaluable`, or an explicit error; never optimistic success.
- **Lifecycle** — outbox enqueue → broker submit → broker truth → inbox → portfolio/accounting. Fills, acknowledgements and cancel confirmations are never synthesized. Orchestrator phase order is load-bearing, and the halt gate precedes any dispatch.
- **Restart, crash and idempotency** — every durable write path that can retry or resume is idempotent; claim tokens do not survive a crash; a fill applied twice is a no-op.
- **Operator truth** — `UNAVAILABLE`, `EMPTY` and `PRESENT` are distinct; `no_db` and `backend_unavailable` are surfaced, never shown as empty.
- **Truthful promotion and evidence** — missing, stale or mismatched evidence blocks promotion; evidence from a superseded protocol cannot authorize a later one.

The binding rules are in [`CLAUDE.md`](CLAUDE.md) and `.claude/rules/`.

## 7. Testing and proof posture

- **Focused local proof** — the load-bearing test, its file, adjacent regressions, and affected-crate tests. Prefer pre-fix RED / post-fix GREEN, negative controls and mutation proof; check that a test cannot pass only because both sides hold the same default, empty value or missing artifact.
- **CI is the broad proof.** GitHub CI runs six jobs: GUI contract (truth tests, build, daemon/GUI contract gate), Python research (pytest), Safety Guards, Rust (fmt + clippy + tests with Postgres), DB Proof Lane (Postgres), and Windows platform (no-DB lanes). Windows CI has no Postgres, so DB-backed lanes run on Ubuntu only.
- **Laptop restriction.** Workspace-wide Rust runs (`cargo test --workspace`, `--all`, full `--all-targets` builds, and workspace clippy sweeps) have destabilized the maintainer's machine and are **not routine local practice**; they run in CI unless explicitly authorized. Local work uses focused tests, `-p <crate>` runs, and constrained parallelism (e.g. `-j 2`).
- `full_repo_proof.ps1` is the repo's bundled local proof runner (`-ProofProfile local|full|exploratory`, optional `-LowMemory`). Its profiles include broad workspace lanes, so treat it as an explicit, authorized run rather than a default step. It writes `.proof/full_repo_proof_output.txt`.
- DB-backed tests need a disposable proof database (§9). `scripts/db_proof_bootstrap.sh` is the underlying DB proof harness used by CI's DB lane.
- Python research tests: `pytest` from `research-py/`. GUI: `npm run test` and `npm run build` from `core-rs/mqk-gui/`.

Examples of focused proof:

```powershell
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-strategy
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_gui_daemon_contract_gate
```

## 8. Repo layout and entry points

| Path | Role |
|---|---|
| `core-rs/Cargo.toml` | Rust workspace manifest |
| `core-rs/crates/mqk-config` | Layered config loading and config hash |
| `core-rs/crates/mqk-db` | Persistence, migrations (`crates/mqk-db/migrations/`, append-only), outbox/inbox, run lifecycle |
| `core-rs/crates/mqk-execution`, `mqk-risk`, `mqk-integrity`, `mqk-reconcile`, `mqk-portfolio` | Order routing and OMS, risk gates, stale/gap controls, broker reconcile, accounting |
| `core-rs/crates/mqk-strategy`, `mqk-backtest`, `mqk-promotion` | Strategy interface and sizing, deterministic Backtest, promotion evaluation |
| `core-rs/crates/mqk-md`, `mqk-artifacts`, `mqk-audit`, `mqk-schemas` | Market data, run artifacts, audit events, shared schemas |
| `core-rs/crates/mqk-runtime`, `mqk-daemon`, `mqk-cli` | Execution path, HTTP control plane, `mqk` CLI |
| `core-rs/crates/mqk-broker-paper`, `mqk-broker-alpaca`, `mqk-broker-ibkr` | Broker adapters |
| `core-rs/crates/mqk-testkit`, `mqk-isolation` | Scenario harness, cross-engine isolation |
| `core-rs/mqk-gui/` | Operator console |
| `research-py/` | Python research CLI (`mqk_research`) |
| `config/` | Layered config sets |
| `scripts/guards/`, `scripts/windows/`, `scripts/soak/` | Guards, Windows launch and evidence helpers, soak evidence tooling |
| `docs/` | Specs, runbooks, research records, audits |

Operational entry points:

- CLI binary `mqk`: `cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- --help`
- Daemon: `cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon` (from repo root, so a repo-root `.env.local` auto-loads)
- GUI: `cd core-rs\mqk-gui; npm ci; npm run dev` (port `1420`; daemon at `http://127.0.0.1:8899`, overridable via `VITE_MQK_DAEMON_URL`)
- Windows launcher: `scripts/windows/Launch-VeritasLedger.ps1` (optional operator convenience)

## 9. Local setup

Prerequisites: Rust stable (pinned by `core-rs/rust-toolchain.toml`), Docker, Node.js + npm, Git Bash (the DB proof
harness is a shell script), PowerShell. Python 3 for `research-py/`.

**Env file.** Copy `.env.local.example` to `.env.local`. `mqk-cli` and `mqk-daemon` auto-load `.env.local` from the
*current working directory*, so launch from repo root, or place a copy in `core-rs/`. Typical keys:
`MQK_DATABASE_URL`, `MQK_OPERATOR_TOKEN`, `MQK_DAEMON_DEPLOYMENT_MODE`, `MQK_DAEMON_ADAPTER_ID`,
`ALPACA_API_KEY_PAPER`, `ALPACA_API_SECRET_PAPER`. Never commit or share a real `.env.local`.

**Keep databases separate.** Runtime/operator work, disposable proof work, the isolated `cargo test` database, the
reality-test lane and the operating Paper database are different lanes with different ports. Do not collapse them.
The canonical topology is in `docs/runbooks/autonomous_paper_ops.md` (§0 and §0b). Before trusting any default
port, run `docker ps` and confirm what is actually listening; a stale host port-forward can make a correct password
look like an authentication failure (`docker exec <container> psql ...` distinguishes the two).

Disposable proof DB example (binds `55432` to avoid colliding with a runtime DB on `5432`):

```powershell
docker run --name mqk-postgres-proof `
  -e POSTGRES_USER=mqk -e POSTGRES_PASSWORD=mqk -e POSTGRES_DB=mqk_test `
  -p 55432:5432 -d postgres:16
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
```

Runtime DB example from `.env.local.example`: `postgres://postgres:postgres@localhost:5432/mqk_dev`. Real Paper
operations use a separate operating database; the launcher scripts reassert it (see the runbook above).

**Migrations** (`core-rs/crates/mqk-db/migrations/`, append-only, never edited once committed):

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- db status
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- db migrate
```

## 10. Common CLI operations

Market data (research and backtest read `md_bars`, not providers directly):

```powershell
mqk md ingest-csv --path "<CSV>" --timeframe "1D" --source "csv"
mqk md ingest-provider --source "twelvedata" --symbols "SPY,QQQ" --timeframe "1D" --start "2000-01-01" --end "2026-01-01"
mqk md sync-provider --source "twelvedata" --symbols "SPY,QQQ" --timeframe "1D"        # incremental
```

(`mqk` stands for `cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --`.) Ingest ids are deterministic for
identical inputs. Default sync overlap is 5 days for `1D`, 2 for `5m`, 1 for `1m`.

Deterministic Backtest:

```powershell
mqk backtest csv --bars "<BARS_CSV>" --timeframe-secs 60 --initial-cash-micros 100000000000 `
  --integrity-enabled true --integrity-stale-threshold-ticks 120 --integrity-gap-tolerance-bars 0
mqk backtest db --timeframe "1D" --start-end-ts 946684800 --end-end-ts 1704067200 --symbols "SPY,QQQ"
```

Cash fields are integer micros: `$100,000` is `100000000000`; `100000` is `$0.10`. This applies to the GUI backtest
form too. Native Sizing V1 is selected explicitly through the Backtest and `scan-strategies` options; routes that
cannot carry the sizing bridge refuse a sizing selection. See `docs/runbooks/backtest_workflow.md` and the sizing
contract for the exact flags.

Run lifecycle: `mqk run start | arm | begin | heartbeat | stop | halt | status | deadman-check | deadman-enforce`
(`mqk run --help`). The CLI uses `BACKTEST | PAPER | LIVE`; daemon deployment labels are `paper`, `live-shadow`,
`live-capital`, `backtest`, and they do not map one-to-one.

## 11. Daemon, GUI and deployment combinations

Valid start-authoritative daemon combinations: `paper` + `alpaca` (the canonical Paper path), and typed
`live-shadow` / `live-capital` + `alpaca`, which are **not operationally authorized**. Refused: `paper` + `paper`
adapter, any Live mode with the `paper` adapter, unrecognized adapter ids, and `backtest` in the daemon runtime.
Mode transitions are restart-based (persisted restart intent via `/api/v1/ops/action`), never hot-swapped.

With no strategy deployed, a Paper start is refused rather than run with a default. Useful read-only daemon
surfaces: `GET /api/v1/system/status`, `/system/preflight`, `/autonomous/readiness`, `/autonomous/daily-operation`,
`/alerts/active`, `/events/feed`, `/ops/catalog`.

Autonomous Paper operations, evidence capture and recovery procedures are in
`docs/runbooks/autonomous_paper_ops.md`, `docs/runbooks/paper_smoke_evidence_pack.md` and
`docs/runbooks/operator_workflows.md`. Provider network calls need both `MQK_AUTONOMOUS_DATA_REFRESH_ENABLED=true`
and `MQK_ALLOW_PROVIDER_API_CALLS=true`; do not enable them merely to make a start succeed.

Python research setup: from `research-py/`, `python -m venv .venv`, `pip install -e .`, `python -m mqk_research.cli --help`.

## 12. Current boundaries and non-claims

- No active deployed Paper strategy and no promoted candidate. Paper deployment is inactive pending a valid promotable candidate.
- Research Batch 02 has not started; Batch 01 is rejected.
- The final holdout is reserved and unconsumed.
- Live is disabled and not ready. Typed support for `live-shadow` and `live-capital` is not evidence of safe live operation.
- M1 is not complete: candidate discovery/promotion, M1.9 and M1.10 are open.
- M2–M10 are not the current target and are not claimed operationally complete.
- Scenario-tested does not mean profitable, broker-proof, or safe for live capital.
- Some GUI detail surfaces are intentionally unmounted or marked unavailable rather than faked.

## 13. Where deeper truth lives

- Program plan and ledger: `MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md`
- Current state: `docs/CURRENT_MISSION.md`; code-completion manifest: `docs/V4_CODE_COMPLETION_MANIFEST.md`
- Specs: `docs/specs/`; research records: `docs/research/`; audits: `docs/audits/`
- Runbooks: `docs/runbooks/` (`autonomous_paper_ops.md`, `operator_workflows.md`, `common_failure_modes.md`, `db_migration_safety.md`, `live_shadow_operational_proof.md`)
- CI and governance: `docs/ci/`, `.github/workflows/ci.yml`, `scripts/guards/`
- Operating contract for AI-assisted work: `CLAUDE.md`, `.claude/rules/`
- Readiness context: `docs/INSTITUTIONAL_READINESS_LOCK.md`, `docs/INSTITUTIONAL_SCORECARD.md`
