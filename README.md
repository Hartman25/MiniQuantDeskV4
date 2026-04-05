<p align="center">
  <img src="assets\logo\Veritas Ledger.png" alt="Veritas Ledger" width="520">
</p>

<p align="center">
  <strong>Deterministic, Risk-First Execution and Capital Allocation Framework</strong><br/>
  Rust Core • Explicit Lifecycle • DB-Backed Safety • Scenario-Tested
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-stable-orange?logo=rust" />
  <img src="https://img.shields.io/badge/Mode-deterministic-purple" />
  <img src="https://img.shields.io/badge/Focus-risk%20%26%20reliability-blue" />
  <img src="https://img.shields.io/badge/Status-paper%20path%20strong%20%7C%20live%20partial-yellow" />
</p>

## **Overview**

Veritas Ledger is a structured quantitative trading platform built around one principle:

> **Capital protection is a systems problem.**

This repository is not a signal toy and not a broker-click wrapper.  
It is a deterministic execution spine designed to enforce explicit lifecycle control, durable state, fail-closed behavior, and truthful operator surfaces under adversarial assumptions.

It is built for:

- traders who want institutional structure instead of ad hoc scripts
- developers building serious trading infrastructure
- systematic workflows that need deterministic replay, bounded state transitions, and durable auditability

The system is engineered under hostile assumptions:

- market data can be stale, missing, or internally inconsistent
- brokers can drift, duplicate, or delay events
- orders can partially fill or arrive out of order
- processes can restart at the worst possible boundary
- humans can misconfigure the control plane

Safety is enforced architecturally, not socially.

## **Architecture**

<p align="center">
  <img src="assets/diagrams/architecture.svg" alt="MiniQuantDeskV4 Architecture" width="900" />
</p>

**High-level flow**

Market Data / Research Artifacts  
↓  
Canonical Market Data + Quality Gates  
↓  
Deterministic Backtest / Promotion Path  
↓  
Integrity + Risk Gates  
↓  
Execution Boundary  
↓  
Outbox / Broker / Inbox / OMS  
↓  
Portfolio Mutation + Reconcile  
↓  
Control Plane (CLI / Daemon / GUI)

## **Core Characteristics**

| Property | Description |
|---|---|
| **Deterministic** | Same inputs should produce the same replay, fills, and artifacts. |
| **Risk-First** | Integrity and risk gates sit in front of the execution boundary. |
| **Lifecycle Controlled** | Runs move through explicit status transitions instead of ad hoc process state. |
| **OMS-Governed** | Order lifecycle transitions are constrained by the OMS state machine. |
| **DB-Enforced Safety** | Durable outbox/inbox, run lifecycle, broker mapping, and lease/control truth live in Postgres where the readiness standard requires it. |
| **Scenario-Tested** | Reliability work is backed by adversarial scenario tests, not comments. |
| **Operator-Aware** | Daemon + GUI are being hardened as truth surfaces rather than decorative dashboards. |

## **Repository Structure**

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

  mqk-gui/

research-py/
config/
scripts/
docs/
```

Rust is the authoritative execution and control layer.  
Python research is optional and is intended to emit deterministic artifacts that the Rust spine can consume.

Operationally, `MAIN` is the canonical engine. `EXP` exists as a research-side experimental sandbox and is not part of current readiness or operator-truth claims unless explicitly promoted.

## **What Works Today**

### **Core platform**
- deterministic Rust workspace with explicit execution boundaries
- DB-backed lifecycle and execution-path safety model
- repo-native DB proof harness
- scenario-driven reliability validation
- fail-closed operator posture where authority is missing

### **Market data**
- canonical `md_bars` ingest
- CSV and provider ingestion paths
- data quality reporting
- stale / gap / incomplete-bar handling in the data path

### **Backtesting and promotion**
- deterministic replay
- conservative fill modeling
- promotion-facing infrastructure exists
- research-to-runtime artifact closure is improving, but is not fully complete yet

### **Execution core**
- explicit OMS order state machine
- durable outbox submission flow
- durable inbox event ingestion
- idempotent broker-event handling
- broker/internal order identity mapping
- partial-fill-aware cancel / replace semantics

### **Risk, integrity, and reconcile**
- allocation / exposure boundary checks
- stale feed and disagreement controls
- deadman-style enforcement paths
- reconcile normalization and mismatch detection
- arming preflight tied to durable truth
- session-aware autonomous paper gating for the canonical Paper + Alpaca path

### **Control plane**
- CLI workflows for DB, market data, runs, and backtests
- HTTP daemon with control, readiness, status, and audit/event surfaces
- canonical Paper + Alpaca autonomous paper path with truthful readiness, session control, WS continuity gating, and durable supervisor-history surfacing
- persisted restart-intent control workflow for mode changes (no hot-switch fiction)
- Vite/React GUI operator console with GUI/daemon contract gate in CI
- optional Windows desktop bootstrap scripts exist, but browser GUI + daemon remains the primary documented path

## **Current Operational Status**

This repo has real institutional bones, but it is **not** yet a fully live-capital-ready operator platform.

**What is strong right now**
- core DB-backed safety model
- OMS and durable execution-path structure
- repo-native DB proof lane plus authoritative `full_repo_proof.ps1` local runner
- truthful daemon / GUI contract gating, including route-contract drift checks
- canonical Paper + Alpaca autonomous paper path with readiness truth, session-window truth, WS continuity gating, durable autonomous-session history, and a one-day soak harness
- restart / recovery posture is materially stronger than early scaffold state
- Windows proof posture is materially stronger via low-memory proof mode and Windows CI parity

**What is still partial or under hardening**
- research → deployability → runtime artifact chain is not fully closed
- shadow / live parity evidence is not fully surfaced and enforced
- live-shadow and live-capital typed support is not the same as proven safe live operation
- portfolio and capital-allocation realism remain open work
- some deeper GUI detail surfaces remain intentionally deferred or unmounted rather than faked

**Important current daemon posture**
- default bind is loopback-only: `127.0.0.1:8899`
- non-loopback bind requires explicit opt-in
- privileged routes fail closed until `MQK_OPERATOR_TOKEN` is configured
- backtest deployment through the daemon is intentionally refused fail-closed
- the most credible operational path today is **Paper + Alpaca**, not paper+paper
- live-shadow and live-capital have additional runtime gates and should still be treated as partially trusted modes, not finished operational claims

## **Verification and CI**

The repo uses multiple verification lanes instead of one generic “cargo test and hope” story.

**Authoritative local proof runner**
- `full_repo_proof.ps1` is the canonical local proof entry point
- `-ProofProfile local` runs the non-DB local lane set
- `-ProofProfile full` runs the DB-backed institutional proof path
- `-LowMemory` reproduces the proven Windows low-memory posture for local proof execution

**CI lanes**
- **GUI contract gate** — GUI truth tests, GUI build, plus authoritative daemon contract tests
- **Safety guards** — unsafe-pattern, migration-governance, and ignored-proof hygiene checks
- **Rust lane** — `fmt --check`, `clippy`, and broad workspace tests
- **DB proof lane** — repo-native Postgres-backed safety proof harness
- **Windows lane** — low-memory Windows build/test parity for the real operator OS class

That DB lane remains the load-bearing proof path for migrations, inbox/outbox durability, restart quarantine, lease/deadman, and arming constraints.

## **Quick Start**

### **1. Clone**
```powershell
git clone <your-repo-url>
cd MiniQuantDeskV4
```

### **2. Requirements**
- Rust stable toolchain
- Docker
- Node.js + npm (for the GUI)
- Git Bash on Windows if you want to run the repo-native shell proof harness directly

### **3. Start a local proof database**
```powershell
docker run --name mqk-postgres-proof `
  -e POSTGRES_USER=mqk `
  -e POSTGRES_PASSWORD=mqk `
  -e POSTGRES_DB=mqk_test `
  -p 55432:5432 `
  -d postgres:16
```

### **4. Run the DB proof lane**
```powershell
& "C:\Program Files\Git\bin\bash.exe" -lc 'export MQK_DATABASE_URL="postgres://mqk:mqk@127.0.0.1:55432/mqk_test"; export DATABASE_URL="$MQK_DATABASE_URL"; ./scripts/db_proof_bootstrap.sh'
```

### **5. Run the authoritative local proof runner**
```powershell
# Non-DB local proof
.\full_repo_proof.ps1 -ProofProfile local

# Full DB-backed proof
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full
```

### **6. Run the daemon**
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

### **7. Run the GUI**
```powershell
cd core-rs\mqk-gui
npm ci
npm run dev
```

Open:
- GUI: `http://127.0.0.1:5173`
- Daemon: `http://127.0.0.1:8899`

## **Design Philosophy**

> **Returns are a strategy problem. Blow-ups are a systems problem.**

Veritas Ledger is engineered primarily to address the second.

## **Scope and Non-Goals**

**Within scope**
- deterministic backtest replay
- explicit lifecycle enforcement
- durable execution-path truth
- idempotent broker-event handling
- operator / control-plane hardening
- scenario-based reliability validation

**Not promised by this repo**
- profitability
- broker correctness
- exchange correctness
- host-level security
- secret-management hardening
- safe live deployment without operator review, stronger parity evidence, and additional controls

## **Disclaimer**

This repository is an engineering framework for systematic capital allocation research and operator-controlled execution. It is not investment advice and should not be treated as a promise of profitability or safe unattended live trading.
