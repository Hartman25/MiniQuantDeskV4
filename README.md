# MiniQuantDeskV4

<p align="center">
  <img src="assets\logo\Veritas Ledger.png" alt="MiniQuantDesk" width="520">
</p>

<p align="center">
  <strong>Deterministic, Risk-First Execution and Capital Allocation Framework</strong><br/>
  Rust Core • Explicit Lifecycle • DB-Backed Safety • Scenario-Tested
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-stable-orange?logo=rust" />
  <img src="https://img.shields.io/badge/Mode-deterministic-purple" />
  <img src="https://img.shields.io/badge/Focus-risk%20%26%20reliability-blue" />
  <img src="https://img.shields.io/badge/CI-gui%20%2B%20rust%20%2B%20db--proof-success" />
</p>

## **Overview**

MiniQuantDeskV4 is a structured quantitative trading system built around one principle:

> **Capital protection is a systems problem.**

This repository is not a signal toy and not a broker-click wrapper.  
It is a deterministic execution spine designed to enforce discipline, explicit lifecycle control, durable state, and fail-closed behavior under adversarial assumptions.

It is built for:

- traders who want institutional structure instead of ad hoc scripts
- developers building serious trading infrastructure
- systematic workflows that need deterministic replay, bounded state transitions, and durable audit surfaces

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

**Core properties**

- deterministic event replay
- worst-case ambiguity modeling
- database-enforced lifecycle constraints
- explicit OMS order-state control
- engine-level capital isolation
- reconcile gating before arming sensitive modes
- loopback-by-default operator surface

## **Core Characteristics**

| Property | Description |
|---|---|
| **Deterministic** | Same inputs should produce the same replay, fills, and artifacts. |
| **Risk-First** | Integrity and risk gates sit in front of the execution boundary. |
| **Lifecycle Controlled** | Runs move through explicit status transitions instead of ad hoc process state. |
| **OMS-Governed** | Order lifecycle transitions are constrained by the OMS state machine. |
| **DB-Enforced Safety** | Durable outbox/inbox, run lifecycle, broker mapping, and lease/control truth live in Postgres. |
| **Scenario-Tested** | Reliability work is backed by adversarial scenario tests, not comments. |
| **Operator-Aware** | Daemon + GUI are being hardened as truth surfaces rather than decorative dashboards. |

## **Repository Structure**

```text
core-rs/
  crates/
    mqk-isolation
    mqk-schemas
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

### **Market Data**
- canonical `md_bars` ingest
- CSV and provider ingestion paths
- data quality reporting
- stale/gap/incomplete-bar handling in the data path

### **Backtesting**
- deterministic replay
- conservative fill modeling
- scenario-driven validation
- promotion-facing backtest infrastructure under active hardening

### **Execution Core**
- explicit OMS order state machine
- durable outbox submission flow
- durable inbox event ingestion
- idempotent broker-event handling
- broker/internal order identity mapping
- partial-fill-aware cancel/replace semantics

### **Risk, Integrity, and Reconcile**
- allocation/exposure boundary checks
- stale feed and disagreement controls
- deadman-style enforcement paths
- reconcile normalization and mismatch detection
- arming preflight tied to durable truth

### **Control Plane**
- CLI workflows for DB, market data, runs, and backtests
- HTTP daemon with control/status surfaces
- Vite/React GUI operator console
- GUI/daemon contract gate in CI

## **Current Operational Status**

This repo has real institutional bones, but it is **not** yet a fully live-capital-ready operator platform.

**What is strong right now**
- core DB-backed safety model
- OMS and durable execution-path structure
- repo-native DB proof lane
- daemon/GUI contract gating
- deterministic paper path and backtest infrastructure

**What is still intentionally fail-closed or under hardening**
- live-shadow and live-capital daemon/runtime posture are typed and gate-checked, but operational trust and end-to-end recovery proof remain partial
- daemon-level backtest deployment is intentionally refused fail-closed today
- complete operator-truth coverage across all GUI detail surfaces
- full live broker wiring proof through the daemon/runtime plane

**Important current daemon posture**
- default bind is loopback-only: `127.0.0.1:8899`
- non-loopback bind requires explicit opt-in
- privileged routes fail closed until `MQK_OPERATOR_TOKEN` is configured
- daemon now has typed support for paper (paper or Alpaca adapter), live-shadow (Alpaca adapter), and live-capital (Alpaca adapter); backtest is unconditionally refused fail-closed; live-shadow and live-capital have additional runtime gates enforced and operational trust for those modes is still partial — typed source support is not the same as proven safe live operation

## **Verification and CI**

The repo now has multiple verification lanes instead of one generic “cargo test and hope” story.

**CI lanes**
- **GUI contract gate** — GUI truth tests, GUI build, plus authoritative daemon contract test
- **Safety guards** — unsafe-pattern and migration-governance checks
- **Rust lane** — `fmt --check`, `clippy`, and broad workspace tests
- **DB proof lane** — repo-native Postgres-backed safety proof harness

**Local proof harness**
- `scripts/db_proof_bootstrap.sh`
- `scripts/db_proof_bootstrap.sh --start-postgres`

That DB lane is the load-bearing proof path for migrations, inbox/outbox durability, restart quarantine, lease/deadman, and arming constraints.

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

### **5. Build and test the Rust workspace**
```powershell
cd core-rs
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### **6. Run the daemon**
```powershell
cd core-rs
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
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

MiniQuantDeskV4 is engineered primarily to address the second.

## **Scope and Non-Goals**

**Within scope**
- deterministic backtest replay
- explicit lifecycle enforcement
- durable execution-path truth
- idempotent broker-event handling
- operator/control-plane hardening
- scenario-based reliability validation

**Not promised by this repo**
- profitability
- broker correctness
- exchange correctness
- host-level security
- secret-management hardening
- safe live deployment without operator review and additional controls

## **Disclaimer**

This repository is an engineering framework for systematic capital allocation research and operator-safe execution design.

It is **not** financial advice.

Do not deploy real capital without independent operational review, monitoring, governance controls, and a verified live-trading runbook.
