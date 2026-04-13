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
  <img src="https://img.shields.io/badge/Status-paper%20path%20strong%20%7C%20live%20partial-yellow" />
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

The strongest current operational path is:

- **deployment mode:** `paper`
- **adapter:** `alpaca`
- **operator surface:** daemon + Vite GUI
- **proof posture:** full repo proof runner, DB-backed proof matrix, guard rails, GUI/daemon contract gate, and Windows low-memory parity

What that means in plain English:

- the canonical **Paper + Alpaca** path is the most credible route today
- paper+paper is not treated as an authoritative execution path
- backtest deployment through the daemon is intentionally refused fail-closed
- live-shadow and live-capital have typed support and start-gate work, but should still be treated as partially trusted modes rather than finished operational claims

## Architecture

<p align="center">
  <img src="assets/diagrams/architectureV2.svg" alt="MiniQuantDeskV4 architecture" width="960" />
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
- guard rails for unsafe patterns, ignored-proof hygiene, migration governance, and GUI/daemon contract drift

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
- shadow/live parity evidence is not yet fully surfaced and enforced end to end
- portfolio realism and capital-allocation realism still need further hardening
- some deeper GUI detail surfaces are intentionally deferred or unmounted rather than faked
- desktop bootstrap exists, but the primary documented operator path remains daemon + browser GUI

## Current daemon posture

- default bind is loopback-only: `127.0.0.1:8899`
- non-loopback bind requires explicit opt-in
- privileged routes fail closed until `MQK_OPERATOR_TOKEN` is configured
- `paper + alpaca` is the strongest operational path today
- `paper + paper` is refused as a start-authoritative daemon combination
- `backtest` deployment through the daemon is intentionally refused fail-closed
- `live-shadow` and `live-capital` remain partially trusted modes with additional gates and incomplete operational proof
- mode transitions are restart-based, not magical hot swaps

## Verification model

This repo does not rely on a single `cargo test` story.

### Authoritative local proof runner

- `full_repo_proof.ps1 -ProofProfile local` runs the non-DB local lane set
- `full_repo_proof.ps1 -ProofProfile full` runs the DB-backed institutional proof path
- `-LowMemory` reproduces the proven Windows low-memory posture for local proof execution

### Main proof and guard lanes

- **workspace lane** — `fmt`, `clippy`, and broad workspace tests
- **daemon proof lanes** — route truth, token auth, runtime lifecycle, fail-closed boot, and deadman behavior
- **broker lane** — Alpaca adapter contract and inbound lifecycle mapping proof
- **runtime lane** — lifecycle continuity and runtime proof surfaces
- **DB proof lane** — migrations, lifecycle constraints, outbox/inbox durability, restart quarantine, deadman, and broker-map enforcement
- **GUI contract lane** — GUI truth tests, GUI build, and daemon/GUI contract drift checks
- **guard lanes** — unsafe patterns, ignored-proof hygiene, migration governance, and related repo protections
- **Windows low-memory parity** — proof posture for the actual operator OS class

That DB-backed lane remains the load-bearing proof surface for the most important durability claims.

## Quick start

### 1. Clone

```powershell
git clone <your-repo-url>
cd MiniQuantDeskV4
```

### 2. Requirements

- Rust stable toolchain
- Docker
- Node.js + npm
- Git Bash on Windows if you want to run the shell proof harness directly

### 3. Start a local proof database

```powershell
docker run --name mqk-postgres-proof `
  -e POSTGRES_USER=mqk `
  -e POSTGRES_PASSWORD=mqk `
  -e POSTGRES_DB=mqk_test `
  -p 55432:5432 `
  -d postgres:16
```

### 4. Run the canonical proof path

```powershell
# Non-DB proof
.\full_repo_proof.ps1 -ProofProfile local

# Full DB-backed proof
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full
```

### 5. Run the daemon

```powershell
cd core-rs
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
$env:MQK_OPERATOR_TOKEN = "dev-local-operator-token"
$env:MQK_DAEMON_DEPLOYMENT_MODE = "paper"
$env:MQK_DAEMON_ADAPTER_ID = "alpaca"
$env:ALPACA_API_KEY_PAPER = "<your-paper-key>"
$env:ALPACA_API_SECRET_PAPER = "<your-paper-secret>"
cargo run -p mqk-daemon
```

### 6. Run the GUI

```powershell
cd core-rs\mqk-gui
npm ci
npm run dev
```

Open:

- GUI: `http://127.0.0.1:5173`
- Daemon: `http://127.0.0.1:8899`

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

## Read next

- `README_TECHNICAL.md` — practical setup, proof commands, daemon/GUI startup, and operator boundaries
- `docs/runbooks/autonomous_paper_ops.md` — canonical autonomous paper operations
- `docs/runbooks/operator_workflows.md` — operator control-plane workflows
- `docs/runbooks/live_shadow_operational_proof.md` — current live-shadow proof posture
- `docs/INSTITUTIONAL_READINESS_LOCK.md` — readiness lock and guardrail context
- `docs/INSTITUTIONAL_SCORECARD.md` — scorecard context

## Disclaimer

This repository is an engineering framework for systematic capital allocation research and operator-controlled execution. It is not investment advice and should not be treated as a promise of profitability or safe unattended live trading.
