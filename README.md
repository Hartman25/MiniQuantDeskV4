MiniQuantDeskV4
<p align="center"> <img src="assets/logo/miniquantdesk_banner_wide.png" alt="MiniQuantDesk" width="520"> </p> <p align="center"> <strong>Deterministic, Risk-First Capital Allocation Framework</strong><br/> Rust Core • Explicit Lifecycle • Engine Isolation • Scenario-Tested </p> <p align="center"> <img src="https://img.shields.io/badge/Rust-stable-orange?logo=rust" /> <img src="https://img.shields.io/badge/Mode-deterministic-purple" /> <img src="https://img.shields.io/badge/Focus-risk%20%26%20reliability-blue" /> <img src="https://img.shields.io/badge/Status-reliability%20hardening-lightgrey" /> </p>
Overview

MiniQuantDeskV4 is a structured quantitative trading system built around one principle:

Capital protection is a systems problem.

This repository is not a strategy collection.
It is not a signal toy.
It is an execution spine designed to enforce discipline.

Built for:

Retail traders who want institutional structure

Developers building serious trading infrastructure

Systematic traders who care about deterministic behavior

Internal tooling stacks that require explicit invariants

The system is engineered under adversarial assumptions:

Market data can be incomplete or stale

Brokers can drift or return inconsistent states

Orders can partially fill

Infrastructure can restart mid-execution

Humans can misconfigure systems

Safety is enforced by architecture — not convention.

Architecture
<p align="center"> <img src="assets/diagrams/architecture.svg" alt="MiniQuantDeskV4 Architecture" width="900" /> </p>

High-level flow:

Market Data / Research Artifacts
            ↓
Market Data Ingest + Quality Gates
            ↓
Deterministic Backtest Engine
            ↓
Integrity + Risk Gates
            ↓
Execution Boundary
            ↓
Lifecycle + DB Enforcement
            ↓
Control Plane (CLI / Daemon / GUI)

Key properties:

Deterministic event replay

Worst-case ambiguity modeling

Database-enforced lifecycle constraints

Engine-level capital isolation

Reconciliation gating before LIVE arming

Core Characteristics
Property	Description
Deterministic	Event-sourced backtesting and replay
Risk-First	Allocation limits enforced at execution boundary
Lifecycle Controlled	CREATED → ARMED → RUNNING → STOPPED
Engine-Isolated	Capital segregation per engine
DB-Enforced Safety	LIVE exclusivity + lifecycle constraints
Scenario-Tested	Adversarial cases (partials, stale feeds, drift, etc.)
Repository Structure
core-rs/
  crates/
    mqk-db          Run lifecycle + persistence
    mqk-md          Market data ingest + quality gates
    mqk-integrity   Feed validation + safety halts
    mqk-risk        Capital limits + PDT helpers
    mqk-execution   Intent → order boundary
    mqk-backtest    Deterministic event replay
    mqk-reconcile   Broker snapshot normalization
    mqk-promotion   Strategy promotion gates
    mqk-isolation   Engine capital segregation
    mqk-strategy    Strategy host framework
    mqk-audit       Audit logging
    mqk-testkit     Scenario-driven test utilities

  mqk-gui/          GUI control console (daemon-backed)

research-py/        Optional Python research artifact emitter

The Rust workspace forms the authoritative execution layer.
The Python research layer emits deterministic artifacts that feed the Rust engine.

What Works Today
Market Data

Canonical md_bars ingest

CSV + provider ingestion path

Data quality gate reporting

Gap detection + incomplete bar rejection

Backtesting

Deterministic event replay

Worst-case ambiguity modeling

Scenario-driven validation

Risk & Integrity

Allocation / exposure caps

PDT helper module

Stale feed disarm

Feed disagreement halt logic

Deadman-style kill paths

Reconciliation

Snapshot normalization adapter

Drift detection

Reconcile-before-arm gating (configurable)

Control Plane

CLI workflows

HTTP daemon for lifecycle + status

GUI console with status streaming (SSE)

Reliability Hardening Status

This project is in structured reliability hardening.

Completed:

Lifecycle enforcement

Engine isolation

Deterministic replay

Market data ingest + quality reporting

Control plane wiring

Scenario coverage across subsystems

In Progress:

End-to-end idempotent broker submission discipline

OMS cancel/replace edge-case hardening

Sticky disarm across restart

Periodic reconcile tick with hard halt capability

“Scenario-tested” does not equal “production-live safe.”
Live deployment requires additional operational review and governance.

Quick Start (Developer)

This is a systems project. The objective is reproducible behavior and safety invariants — not instant live deployment.

1. Clone
git clone <your-repo-url>
cd MiniQuantDeskV4
2. Install Requirements

Rust (stable toolchain)

Docker (recommended for local Postgres)

3. Start Postgres (example)
docker run --name mqk-postgres \
  -e POSTGRES_USER=mqk_user \
  -e POSTGRES_PASSWORD=mqk_pass \
  -e POSTGRES_DB=mqk \
  -p 5432:5432 \
  -d postgres:16

Adjust credentials as needed.

4. Build + Test
cd core-rs
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

All workspace tests should pass before modifying behavior.

5. Run Control Plane (Optional)

The daemon exposes lifecycle + status endpoints.
The GUI operates as a control console over the daemon.

See README_TECHNICAL.md for exact commands and configuration.

6. Optional: Research Layer

The research-py/ system emits deterministic artifacts (runs, manifests, CSV outputs) consumable by the Rust spine.

Refer to README_TECHNICAL.md for setup instructions.

Design Philosophy

Returns are a strategy problem.
Blow-ups are a systems problem.

MiniQuantDeskV4 focuses on the second.

If a safety invariant cannot be mechanically enforced, it is considered incomplete.

Disclaimer

This repository is an engineering framework for systematic capital allocation research.

It is not financial advice.
Do not deploy real capital without independent operational review, monitoring infrastructure, and governance controls.