<p align="center">
  <img src="assets/logo/miniquantdesk_banner_wide.png"
       alt="MiniQuantDesk"
       width="480">
</p>

<h1 align="center">MiniQuantDeskV4</h1>

<p align="center">
  <strong>Risk-First Quantitative Capital Allocation Framework</strong>
</p>

<p align="center">
  <img src="https://img.shields.io/badge/Rust-Stable-orange?logo=rust" />
  <img src="https://img.shields.io/badge/Workspace-Tests%20Green-brightgreen" />
  <img src="https://img.shields.io/badge/Architecture-Risk%20First-blue" />
  <img src="https://img.shields.io/badge/Mode-Deterministic-purple" />
  <img src="https://img.shields.io/badge/Status-Reliability%20Hardening-lightgrey" />
</p>

MiniQuantDeskV4 is an architecture-first quantitative trading system designed to operate as a disciplined capital allocator with **enforced lifecycle control**, **deterministic backtesting/replay**, and **hard safety boundaries**.

This system is engineered under the assumption that:

- Markets are adversarial
- Data feeds fail
- Brokers drift
- Infrastructure crashes
- Humans misconfigure systems

Safety is enforced by design — not by policy.

---

## Core Design Principles

1) **Risk Before Return**  
Capital constraints are enforced at the **execution boundary**, not merely at the strategy layer.

2) **Determinism Over Optimism**  
Backtesting is event-sourced and replayable. Ambiguous fills use **worst-case** logic.

3) **Enforcement Over Convention**  
Critical invariants (e.g., single LIVE run per engine) are enforced at the **database constraint layer**.

4) **Isolation by Design**  
Each engine operates with explicit allocation caps. No cross-engine position bleed is permitted.

5) **Explicit Lifecycle Control**  
Execution state transitions are controlled:

**ARMED → RUNNING → STOPPED**

LIVE execution cannot bypass reconciliation or lifecycle gating.

---

## Current System Capabilities (Current Repo State)

### Integrity Layer
- No lookahead bias enforcement (scenario-tested)
- Incomplete bar rejection
- Gap detection (zero-tolerance mode supported)
- Stale feed disarm
- Feed disagreement halt
- Deadman-style safety halts (scenario-tested)

### Reconciliation Layer
- LIVE can require broker reconciliation before arming
- Drift detection halts
- Unknown broker order detection
- Dirty reconciliation prevents LIVE start
- Snapshot normalization adapter exists (`mqk-reconcile/src/snapshot_adapter.rs`)

### Engine Isolation
- Allocation caps enforced per engine
- Isolation enforced at config + runtime boundary + execution boundary
- Database-level LIVE exclusivity enforcement (scenario-tested)

### Deterministic Backtesting + Market Data Ingest
- Event-sourced replay
- Worst-case ambiguity modeling
- Stress impact measurable
- Scenario-driven validation
- CSV ingestion → canonical `md_bars`
- Provider ingestion path → canonical `md_bars`
- Data Quality Gate reports for ingests

### Run Lifecycle Enforcement
- DB uniqueness: one active LIVE run per engine
- STOPPED runs exit active set
- Explicit lifecycle transitions are scenario-tested

### Control Plane Surfaces
- CLI commands for operational workflows
- Daemon API for status + lifecycle control endpoints
- GUI control-plane wiring (buttons + SSE updates)

### Risk Controls (Selected)
- Allocation/exposure caps (scenario-tested)
- PDT helper module (`mqk-risk/src/pdt.rs`) + tests
- Integrity/risk gate integration scenarios exist

---

## What’s *Not* “Production-Safe Live Trading” Yet

This repo is in **reliability hardening**. You should assume “paper-safe by tests” is not the same as “live-safe with capital” until the following are fully implemented and proven:

- A single **authoritative live run loop** that is the only path to broker submission
- Outbox-first idempotent broker submit discipline end-to-end
- Minimal OMS lifecycle state machine (cancel-reject, partials, late fills, replace semantics)
- Inbox apply proven idempotent (“insert → apply” atomic invariant)
- Sticky disarm/deadman across restarts
- Reconcile-before-arm and periodic reconcile tick that can hard-halt

The ordered hardening plan and definition-of-done for each patch is documented in:
- `MiniQuantDeskV4_Patch_By_Patch_Implementation_Plan.md`

---

## Architecture Overview

- **Integrity**     → Market data validation & guardrails
- **Risk**          → Capital limits, kill switch logic, PDT helpers
- **Execution**     → Intent → order boundary enforcement (+ order_router export)
- **Strategy**      → Plugin-based strategy host
- **Isolation**     → Engine capital segregation
- **Reconcile**     → Broker state validation + snapshot normalization adapter
- **Database**      → Run lifecycle + LIVE exclusivity + ingest persistence
- **Backtest**      → Deterministic event replay
- **Daemon**        → Control plane HTTP/SSE API
- **GUI**           → Control console over daemon endpoints
- **CLI**           → Operational tooling (migrate, backtest ops, etc.)

Each layer is independently testable and validated through scenario tests.

---

## Project Status

- Workspace tests: **Green**
- Lifecycle enforcement: **Done**
- Engine isolation: **Done**
- Deterministic replay: **Done**
- Market data ingest (CSV + provider path + DQ report): **Done**
- Control plane (daemon routes + GUI controls + SSE): **Done**
- Reliability hardening toward live-safety: **Ongoing**

---

## Technical Documentation

For setup instructions, database configuration, CLI commands, and development workflow:

➡ See `README_TECHNICAL.md`

---

## Disclaimer

This repository is a research and engineering framework for systematic capital allocation.

It is not financial advice and is not intended for production capital deployment without additional operational review, monitoring infrastructure, and capital governance controls.