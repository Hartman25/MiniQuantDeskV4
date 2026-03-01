MiniQuantDeskV4
<p align="center"> <img src="assets/logo/miniquantdesk_banner_wide.png" alt="MiniQuantDesk" width="520"> </p> <p align="center"> <strong>Deterministic, Risk-First Capital Allocation Framework</strong><br/> Rust Core • Explicit Lifecycle • Engine Isolation • Scenario-Tested </p> <p align="center"> <img src="https://img.shields.io/badge/Rust-stable-orange?logo=rust" /> <img src="https://img.shields.io/badge/Mode-deterministic-purple" /> <img src="https://img.shields.io/badge/Focus-risk%20%26%20reliability-blue" /> <img src="https://img.shields.io/badge/Status-reliability%20hardening-lightgrey" /> </p>
<h2 align="center"><b>Overview</b></h2> <hr/>

MiniQuantDeskV4 is a structured quantitative trading system built around one principle:

Capital protection is a systems problem.

This repository is not a signal library or strategy toy.
It is an execution spine designed to enforce discipline and mechanical safety boundaries.

Built for:

Retail traders who want institutional structure

Developers building serious trading infrastructure

Systematic traders who care about deterministic behavior

Internal tooling stacks that require explicit invariants

The system is engineered under adversarial assumptions:

Market data can be stale or incomplete

Brokers can drift or return inconsistent state

Orders can partially fill

Systems can restart mid-execution

Humans can misconfigure workflows

Safety is enforced architecturally — not socially.

<h2 align="center"><b>Architecture</b></h2> <hr/> <p align="center"> <img src="assets/diagrams/architecture.svg" alt="MiniQuantDeskV4 Architecture" width="900" /> </p>

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

Core properties:

Deterministic event replay

Worst-case ambiguity modeling

Database-enforced lifecycle constraints

Engine-level capital isolation

Reconciliation gating before LIVE arming

<h2 align="center"><b>Core Characteristics</b></h2> <hr/>
Property	Description
Deterministic	Event-sourced backtesting and replay
Risk-First	Allocation limits enforced at execution boundary
Lifecycle Controlled	CREATED → ARMED → RUNNING → STOPPED
Engine-Isolated	Capital segregation per engine
DB-Enforced Safety	LIVE exclusivity + lifecycle constraints
Scenario-Tested	Adversarial cases (partials, stale feeds, drift, etc.)
<h2 align="center"><b>Repository Structure</b></h2> <hr/>
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

Rust forms the authoritative execution layer.
Python research emits deterministic artifacts consumed by the Rust spine.

<h2 align="center"><b>What Works Today</b></h2> <hr/>

<b>Market Data</b>

Canonical md_bars ingest

CSV + provider ingestion path

Data quality gate reporting

Gap detection + incomplete bar rejection

<b>Backtesting</b>

Deterministic event replay

Worst-case ambiguity modeling

Scenario-driven validation

<b>Risk & Integrity</b>

Allocation / exposure caps

PDT helper module

Stale feed disarm

Feed disagreement halt logic

Deadman-style kill paths

<b>Reconciliation</b>

Snapshot normalization adapter

Drift detection

Reconcile-before-arm gating (configurable)

<b>Control Plane</b>

CLI workflows

HTTP daemon for lifecycle + status

GUI console with status streaming (SSE)

<h2 align="center"><b>Reliability Hardening Status</b></h2> <hr/>

This project is under structured reliability hardening.

<b>Completed:</b>

Lifecycle enforcement

Engine isolation

Deterministic replay

Market data ingest + quality reporting

Control plane wiring

Scenario coverage across subsystems

<b>In Progress:</b>

Idempotent broker submission choke-point

OMS cancel/replace edge-case hardening

Sticky disarm across restart

Periodic reconcile tick with hard halt capability

“Scenario-tested” does not imply production-live safety.

<h2 align="center"><b>Security Model</b></h2> <hr/>

MiniQuantDeskV4 assumes:

The local environment may be misconfigured

External data feeds are untrusted

Broker APIs may return inconsistent or delayed state

Restarts may occur at unsafe boundaries

Security and safety are enforced through:

Deterministic execution paths (no hidden randomness)

Database-enforced lifecycle constraints

Explicit state transitions

Isolation between engines

Integrity + risk gates before execution

Reconciliation hooks before LIVE arming

This repository does not attempt to:

Provide hardened secret management

Implement network-level security controls

Protect against host-level compromise

Guarantee broker API correctness

Operational security is the responsibility of the deployment environment.

<h2 align="center"><b>System Guarantees & Non-Guarantees</b></h2> <hr/> <h3><b>What the System Guarantees (Within Scope)</b></h3>

Deterministic backtest replay given identical inputs

Explicit lifecycle state enforcement

Single LIVE run per engine (database constrained)

Capital allocation caps enforced at execution boundary

Scenario-driven validation of adversarial cases

<h3><b>What the System Does NOT Guarantee</b></h3>

Profitability

Broker correctness

Protection from infrastructure misconfiguration

Immunity to exchange-level anomalies

Automatic capital preservation without proper configuration

This framework reduces structural risk.
It does not eliminate market risk.

<h2 align="center"><b>Quick Start</b></h2> <hr/>

This is a systems project focused on reproducibility and safety invariants.

<h3>1. Clone</h3>
git clone <your-repo-url>
cd MiniQuantDeskV4
<h3>2. Requirements</h3>

Rust (stable toolchain)

Docker (recommended for Postgres)

<h3>3. Start Postgres (Example)</h3>
docker run --name mqk-postgres \
  -e POSTGRES_USER=mqk_user \
  -e POSTGRES_PASSWORD=mqk_pass \
  -e POSTGRES_DB=mqk \
  -p 5432:5432 \
  -d postgres:16
<h3>4. Build + Test</h3>
cd core-rs
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

All tests should pass before modifying behavior.

<h3>5. Optional: Control Plane</h3>

Daemon exposes lifecycle + status endpoints

GUI provides control console over daemon

See README_TECHNICAL.md for exact commands and configuration.

<h2 align="center"><b>Design Philosophy</b></h2> <hr/>

Returns are a strategy problem.
Blow-ups are a systems problem.

MiniQuantDeskV4 is engineered to address the second.

<h2 align="center"><b>Disclaimer</b></h2> <hr/>

This repository is an engineering framework for systematic capital allocation research.

It is not financial advice.
Do not deploy real capital without independent operational review, monitoring infrastructure, and governance controls.