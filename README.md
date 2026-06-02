<p align="center">
  <img src="assets/logo/Veritas Ledger.png" alt="Veritas Ledger" width="520">
</p>

<p align="center">
  <strong>Deterministic, risk-first execution and capital allocation framework</strong><br/>
  Rust core • explicit lifecycle • DB-backed safety • scenario-tested proof lanes
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-stable-orange?logo=rust" />
  <img src="https://img.shields.io/badge/Execution-deterministic-purple" />
  <img src="https://img.shields.io/badge/Proof-DB--backed-blue" />
  <img src="https://img.shields.io/badge/Status-supervised%20paper%20candidate%20%7C%20live%20not%20ready-orange" />
</p>

## Overview

Veritas Ledger is a structured quantitative trading platform built around one principle:

> **Capital protection is a systems problem.**

This repo is not a signal toy and not a broker-click wrapper.
It is a deterministic execution spine designed to enforce explicit lifecycle control, durable state, fail-closed behavior, restart discipline, and truthful operator surfaces under hostile assumptions.

It is built for:

- traders who want institutional structure instead of ad hoc scripts
- developers building serious trading infrastructure
- systematic workflows that need deterministic replay, bounded state transitions, and durable auditability

The system is engineered assuming that:

- market data can be stale, missing, or internally inconsistent
- broker events can drift, duplicate, gap, or arrive out of order
- orders can partially fill at the worst possible boundary
- processes can restart during submit, ack, or fill windows
- humans can misconfigure the control plane

Safety is enforced architecturally, not socially.

## What the repo is today

MiniQuantDeskV4 has real institutional bones and a materially stronger proof posture than scaffold-stage trading repos.

The strongest current operational route is:

- **deployment mode:** `paper`
- **adapter:** `alpaca`
- **operator surface:** daemon + Vite GUI
- **proof posture:** full repo proof runner, DB-backed proof matrix, guard rails, GUI/daemon contract gate, and Windows low-memory parity

What that means in plain English:

- the canonical **Paper + Alpaca** path is the most credible route today
- paper+paper is not treated as an authoritative execution path
- backtest deployment through the daemon is intentionally refused fail-closed
- live-shadow and live-capital have typed support and start-gate work, but should still be treated as partially trusted modes rather than finished operational claims

### Current readiness boundary

Use these labels precisely:

| Mode | Current posture | Meaning |
|---|---|---|
| **Supervised Paper + Alpaca** | Candidate / operator-watched | Credible current path after a clean proof run, valid env, live Alpaca paper auth, and operator supervision. |
| **Autonomous Paper + Alpaca** | Mechanically ready — live-paper evidence pending | Long-only single-symbol path is mechanically proven. Remaining blockers are operational evidence: live-paper lifecycle, reconcile after real fill, and repeated autonomous cycle proof. |
| **Live / live-capital** | Not ready | Typed support and gates exist, but this repo should not be treated as safe for unattended live trading yet. |

**Proof status (2026-06-01):** `full_repo_proof.ps1 -ProofProfile full -LowMemory` passed 18/18 lanes. The repo is proof-clean for current scope. Proof-clean is not the same as live-ready. Proof-clean is not the same as safe-capital-deployment.

The proof harness can prove the current locked repo scope. It does **not** mean the system is profitable, broker-proof, or live-ready.

## Architecture

<p align="center">
  <img src="assets/diagrams/architecture.svg" alt="MiniQuantDeskV4 architecture" width="960" />
</p>

### High-level flow

Market data / broker snapshots / research artifacts  
→ canonical ingest + quality gates  
→ deterministic backtest / replay / promotion evidence  
→ integrity + risk gates  
→ execution boundary  
→ durable outbox / broker / durable inbox / OMS  
→ portfolio + reconcile  
→ operator control plane (CLI / daemon / GUI)

### Load-bearing subsystems

| Layer | Purpose |
|---|---|
| **Market data ingest** | Canonical `md_bars` ingest, provider/CSV support, and quality reporting. |
| **Backtest / replay** | Deterministic replay with conservative semantics and promotion-oriented evidence paths. |
| **DB + lifecycle enforcement** | Durable run state, outbox/inbox truth, broker mapping, and lifecycle constraints. |
| **Integrity + risk gates** | Stale feed, gap, disagreement, limits, halt, and risk-cap enforcement before execution. |
| **Execution boundary** | Intent-to-order constraint enforcement, OMS transitions, cancel/replace discipline. |
| **Reconcile** | Snapshot normalization, drift detection, and start/arm gating tied to durable truth. |
| **Control plane** | CLI, HTTP daemon, GUI, audit/event surfaces, and restart-intent operator workflows. |

## Core characteristics

| Property | Description |
|---|---|
| **Deterministic** | Same inputs should produce the same replay, artifacts, and constrained execution decisions. |
| **Risk-first** | Integrity and risk gates sit in front of the execution boundary, not behind it. |
| **Lifecycle-controlled** | Runs move through explicit status transitions instead of ad hoc process state. |
| **OMS-governed** | Order lifecycle transitions are constrained by an explicit state machine. |
| **DB-enforced where it matters** | Durable outbox/inbox, lifecycle, broker identity mapping, cursor state, and operator truth are persisted where the readiness bar requires it. |
| **Scenario-tested** | Reliability work is backed by adversarial scenario tests and proof lanes, not comments or happy-path demos. |
| **Fail-closed** | Missing authority, invalid mode/adapter combinations, and unsafe control-plane actions are refused rather than guessed. |
| **Operator-honest** | Daemon and GUI are being hardened as truth surfaces, not decorative dashboards. |

## Repository structure

```text
core-rs/
  crates/
    mqk-config
    mqk-db
    mqk-audit
    mqk-artifacts
    mqk-cli
    mqk-testkit
    mqk-execution
    mqk-portfolio
    mqk-risk
    mqk-integrity
    mqk-reconcile
    mqk-strategy
    mqk-backtest
    mqk-promotion
    mqk-broker-paper
    mqk-broker-alpaca
    mqk-daemon
    mqk-runtime
    mqk-md
    mqk-isolation
    mqk-schemas

  mqk-gui/

research-py/
config/
scripts/
docs/
assets/
```

Rust is the authoritative execution and control layer.
Python research is optional and is intended to emit deterministic artifacts that the Rust spine can consume.

Operationally, `MAIN` is the canonical engine.
`EXP` is a research-side experimental sandbox and is not part of readiness or operator-truth claims unless explicitly promoted.

## What is strong right now

### Core platform

- deterministic Rust workspace with explicit execution boundaries
- DB-backed lifecycle and execution-path safety model
- authoritative local proof runner: `full_repo_proof.ps1`
- repo-native DB proof harness and mandatory DB matrix
- scenario-driven reliability validation across runtime, execution, DB, broker, and daemon surfaces
- guard rails for unsafe patterns, ignored-proof hygiene, migration governance, workspace dependency inheritance, and GUI/daemon contract drift

### Market data

- canonical `md_bars` ingest
- CSV and provider ingestion paths
- incremental provider sync support
- data-quality reporting artifacts
- stale / gap / incomplete-bar handling in the integrity path

### Backtesting and promotion

- deterministic replay
- conservative ambiguity handling
- promotion-facing infrastructure and artifact checks
- parity and provenance work is materially stronger than earlier scaffolds

### Execution core

- explicit OMS order state machine
- durable outbox-first submission flow
- durable inbox event ingestion
- idempotent broker-event handling
- broker/internal order identity mapping
- partial-fill-aware cancel / replace handling
- restart and crash-window proof coverage

### Risk, integrity, and reconcile

- allocation / exposure boundary checks
- stale feed and disagreement controls
- deadman-style enforcement paths
- reconcile normalization and mismatch detection
- arming preflight tied to durable truth
- autonomous paper gating tied to session truth and WS continuity for the canonical Paper + Alpaca route

### Control plane

- CLI workflows for DB, market data, runs, and backtests
- HTTP daemon with readiness, preflight, control, audit, and event surfaces
- canonical Paper + Alpaca autonomous paper path with truthful readiness, session control, WS continuity gating, and durable autonomous-session history
- persisted restart-intent workflow for admissible mode changes
- Vite/React GUI operator console with a CI-enforced daemon contract gate
- optional Windows desktop bootstrap scripts for a stricter desktop operator path

## What is still partial

Be honest about the open edges.

- research → deployability → runtime artifact closure is not fully complete
- live-shadow and live-capital typed support are not the same thing as proven safe live operation
- Alpaca WebSocket gap recovery is still not a complete lifecycle replay story for every non-fill event
- Alpaca REST fill recovery must remain treated carefully until pagination and high-volume recovery are proven end to end
- bounded broker retry budgets and operator repair workflows still need hardening before unattended operation
- shadow/live parity evidence is not yet fully surfaced and enforced end to end
- portfolio realism and capital-allocation realism still need further hardening
- some deeper GUI detail surfaces are intentionally deferred or unmounted rather than faked
- desktop bootstrap exists, but the primary documented operator path remains daemon + browser GUI

## Open live-paper proof items

The long-only single-symbol Paper + Alpaca MVP is mechanically ready. The following items are required before the MVP is fully closed:

| Item | Status |
|---|---|
| PAPER-TRADE-LIFECYCLE-01 | Open — market-hours paper smoke with real fills |
| RECONCILE-AFTER-REAL-FILL-01 | Open — reconcile pass after a real paper fill |
| DISCORD-TRADE-LIFECYCLE-REAL-01 | Open — Discord notification evidence from a real cycle |
| REPEATED-AUTONOMOUS-TRADE-CYCLE-01 | Open — multiple autonomous cycles without operator intervention |
| PAPER-SMOKE-EVIDENCE-REVIEW-01 | Open — operator review of captured smoke evidence |

### Evidence capture workflow

Evidence is captured using `scripts/windows/Capture-PaperSmokeEvidence.ps1` before and after a paper smoke session. The full workflow is documented in `docs/runbooks/paper_smoke_evidence_pack.md`. Evidence folders are stored under `evidence/` in timestamped run folders and are gitignored by default.

Live trading remains locked until repeated clean paper evidence and live-capital readiness gates are satisfied.

## Local setup model

The repo now has a more explicit local Docker/DB split than older docs suggested.

### Runtime/operator DB

For real local daemon, GUI, and autonomous paper work, use a **runtime DB** that matches your local env configuration.

The repo ships `.env.local.example` as the starting point for this workflow.
It defines a default runtime URL of:

```text
MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5432/mqk_dev
```

Many local workflows keep separate runtime, proof, and reality-test DBs.
That separation is healthy.

### Proof DB

For proof work, use a **disposable proof DB** instead of reusing your runtime DB.
The isolated example below binds Postgres to `55432` specifically to avoid collisions with a normal local runtime DB on `5432`.

### Env-file workflow

`mqk-cli` and `mqk-daemon` will auto-load `.env.local` from the **current working directory**.

Practical implication:

- if you launch from the repo root, a repo-root `.env.local` is picked up automatically
- if you launch from `core-rs/`, place a copy at `core-rs/.env.local` or export the env vars in your shell

The Windows desktop launcher and the autonomous reality-test script already look for both repo-root and `core-rs` env files.

## Verification model

This repo does not rely on a single `cargo test` story.

Command verification note for this README refresh:

- `full_repo_proof.ps1` exists at repo root and accepts `-ProofProfile local`, `-ProofProfile full`, `-ProofProfile exploratory`, and optional `-LowMemory`.
- `core-rs/Cargo.toml` is the current workspace manifest.
- `mqk-daemon` is a workspace package with a binary named `mqk-daemon`.
- `core-rs/mqk-gui/package.json` includes `dev`, `test`, and `build` scripts.
- `core-rs/mqk-gui/vite.config.ts` pins the browser dev server to port `1420`, not Vite's usual `5173`.


### Authoritative local proof runner

- `full_repo_proof.ps1 -ProofProfile local` runs the non-DB local lane set
- `full_repo_proof.ps1 -ProofProfile full` runs the DB-backed proof path and requires `MQK_DATABASE_URL`
- `-LowMemory` can be added to any proof profile and reproduces the proven Windows low-memory posture

### Main proof and guard lanes

- **workspace lane** — `fmt`, `clippy`, and broad workspace tests
- **daemon proof lanes** — route truth, token auth, runtime lifecycle, fail-closed boot, and deadman behavior
- **broker lane** — Alpaca adapter contract and inbound lifecycle mapping proof
- **runtime lane** — lifecycle continuity and runtime proof surfaces
- **DB proof lane** — migrations, lifecycle constraints, outbox/inbox durability, restart quarantine, deadman, and broker-map enforcement
- **GUI contract lane** — GUI truth tests, GUI build, and daemon/GUI contract drift checks
- **guard lanes** — unsafe patterns, ignored-proof hygiene, migration governance, and workspace dependency inheritance
- **Windows low-memory parity** — proof posture for the actual operator OS class

That DB-backed lane remains the load-bearing proof surface for the most important durability claims.

### CI vs operator-class local proof boundary (CI-DB-01)

CI runs five jobs on every push:

- **gui-contract** — GUI truth tests, build gate, daemon/GUI contract gate (ubuntu)
- **guards** — safety pattern guards (ubuntu)
- **rust** — fmt + clippy + workspace tests with ephemeral Postgres (ubuntu)
- **db-proof** — DB proof bootstrap and targeted DB-backed safety proof lanes (ubuntu)
- **windows** — fmt + clippy + workspace tests on windows-latest; no Postgres available on GitHub Actions Windows runners, so **DB-backed lanes do not run in CI Windows**

The Windows CI job proves the build is correct on the operator OS class. It does NOT run the full operator-class DB proof. DB-backed proof in CI is run on ubuntu only.

The full operator-class proof — Windows platform + DB-backed lanes together — requires a local run:

```powershell
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full
```

Release and readiness claims require a clean transcript from this local full proof run (or an equivalent operator-class DB proof). CI passing alone does not substitute for it.

## Quick start

### 1. Clone

```powershell
git clone <your-repo-url>
cd MiniQuantDeskV4
```

### 2. Create your local env file

```powershell
Copy-Item .env.local.example .env.local
```

Fill in the values you actually use for local runtime work.
At minimum, that usually means:

- `MQK_DATABASE_URL`
- `MQK_OPERATOR_TOKEN`
- `MQK_DAEMON_DEPLOYMENT_MODE`
- `MQK_DAEMON_ADAPTER_ID`
- `ALPACA_API_KEY_PAPER`
- `ALPACA_API_SECRET_PAPER`

### 3. Start a local runtime DB

Match this to your `.env.local`.
If you keep the example runtime DB URL from `.env.local.example`, a compatible local Postgres looks like this:

```powershell
docker run --name mqk-postgres-dev `
  --restart unless-stopped `
  -e POSTGRES_USER=postgres `
  -e POSTGRES_PASSWORD=postgres `
  -e POSTGRES_DB=mqk_dev `
  -p 5432:5432 `
  -d postgres:16

# If the container already exists, use this instead:
# docker start mqk-postgres-dev

docker exec mqk-postgres-dev pg_isready -U postgres -d mqk_dev
```

### 4. Start a separate local proof DB

```powershell
docker run --name mqk-postgres-proof `
  --restart unless-stopped `
  -e POSTGRES_USER=mqk `
  -e POSTGRES_PASSWORD=mqk `
  -e POSTGRES_DB=mqk_test `
  -p 55432:5432 `
  -d postgres:16

# If the container already exists, use this instead:
# docker start mqk-postgres-proof

docker exec mqk-postgres-proof pg_isready -U mqk -d mqk_test
```

### 5. Run the canonical proof path

```powershell
# Non-DB proof
.\full_repo_proof.ps1 -ProofProfile local

# Full DB-backed proof against the isolated proof DB
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full

# Same full proof using the Windows low-memory posture
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full -LowMemory
```

### 6. Run the daemon from repo root

Running from repo root lets `mqk-daemon` auto-load repo-root `.env.local`.

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --bin mqk-daemon
```

### 7. Run the GUI

```powershell
cd core-rs\mqk-gui
npm ci
npm run dev
```

Open:

- GUI: `http://127.0.0.1:1420`
- Daemon: `http://127.0.0.1:8899`

The GUI defaults to the daemon URL `http://127.0.0.1:8899`. You can override it with `VITE_MQK_DAEMON_URL` or through the GUI's saved daemon URL setting.

## Design philosophy

> **Returns are a strategy problem. Blow-ups are a systems problem.**

Veritas Ledger is engineered primarily to address the second.

## Scope and non-goals

### Within scope

- deterministic backtest replay
- explicit lifecycle enforcement
- durable execution-path truth
- idempotent broker-event handling
- operator/control-plane hardening
- scenario-based reliability validation

### Not promised by this repo

- profitability
- broker correctness
- exchange correctness
- host-level security
- fully hardened secret management
- safe unattended live deployment without stronger parity evidence, deeper runbooks, and additional controls

## Roadmap

Items beyond the current long-only single-symbol MVP scope, in rough priority order:

- full data ingestion expansion (additional providers, bar types, tick data)
- trading methods expansion beyond long-only single-symbol
- multi-symbol universe support
- multi-asset expansion
- trade journal and forensic review surface
- regime attribution and strategy decay detection
- GUI reskin and multi-monitor operator polish
- live-capital readiness lock (gated on repeated clean paper evidence and all live-capital gates)

## Read next

- `README_TECHNICAL.md` — practical setup, proof commands, daemon/GUI startup, and operator boundaries
- `docs/runbooks/autonomous_paper_ops.md` — canonical autonomous paper operations
- `docs/runbooks/operator_workflows.md` — operator control-plane workflows
- `docs/runbooks/live_shadow_operational_proof.md` — current live-shadow proof posture
- `docs/INSTITUTIONAL_READINESS_LOCK.md` — readiness lock and guardrail context
- `docs/INSTITUTIONAL_SCORECARD.md` — scorecard context

## Snapshot and secret hygiene

Never include a real `.env.local`, API keys, operator tokens, Discord webhooks, or broker secrets in repo snapshots, support zips, or AI handoff bundles. `.env.local.example` is safe to share because it contains names/placeholders only; `.env.local` is not safe to share.

If a support snapshot ever included real credentials, rotate them before running broker-connected sessions again.

## Disclaimer

This repository is an engineering framework for systematic capital allocation research and operator-controlled execution. It is not investment advice and should not be treated as a promise of profitability or safe unattended live trading.
