MiniQuantDeskV4
<p align="center"> <strong>Risk-First Quantitative Capital Allocation Framework</strong> </p> <p align="center"> <img src="https://img.shields.io/badge/Rust-Stable-orange?logo=rust" /> <img src="https://img.shields.io/badge/Workspace-Tests%20Green-brightgreen" /> <img src="https://img.shields.io/badge/Architecture-Risk%20First-blue" /> <img src="https://img.shields.io/badge/Mode-Deterministic-purple" /> <img src="https://img.shields.io/badge/Status-Reliability%20Hardening-lightgrey" /> </p>

MiniQuantDeskV4 is an architecture-first quantitative trading system designed to operate as a disciplined capital allocator with enforced lifecycle controls, deterministic replay, and hard risk boundaries.

This system is engineered under the assumption that:

Markets are adversarial

Data feeds fail

Brokers drift

Infrastructure crashes

Humans misconfigure systems

Safety is enforced by design — not by policy.

Core Design Principles
1. Risk Before Return

Capital constraints are enforced at the execution boundary, not merely at the strategy layer.

2. Determinism Over Optimism

Backtesting is event-sourced and replayable. Ambiguous fills use worst-case logic.

3. Enforcement Over Convention

Critical invariants (e.g., single LIVE run per engine) are enforced at the database constraint layer.

4. Isolation by Design

Each engine operates with explicit capital allocation caps. No cross-engine position bleed is permitted.

5. Explicit Lifecycle Control

Execution state transitions are controlled:

ARMED → RUNNING → STOPPED


LIVE execution cannot bypass reconciliation or lifecycle gating.

Current System Capabilities
Integrity Layer

No lookahead bias

Incomplete bar rejection

Zero-tolerance gap detection

Stale feed disarm

Feed disagreement halt

Reconciliation Layer

LIVE requires broker reconciliation

Drift detection halts

Unknown broker order detection

Dirty reconciliation prevents LIVE start

Engine Isolation

Allocation caps enforced per engine

Isolation at config + runtime + execution boundary

Database-level LIVE exclusivity enforcement

Deterministic Backtesting

Event-sourced replay

Worst-case ambiguity modeling

Stress impact measurable

Scenario-driven validation

Run Lifecycle Enforcement

Single active LIVE run per engine (DB uniqueness constraint)

STOPPED runs exit active set

Explicit ARMED → RUNNING → STOPPED transitions

System Guarantees

MiniQuantDeskV4 enforces:

No execution without explicit arming

No LIVE without reconciliation

One active LIVE run per engine

Allocation caps at execution boundary

Automatic halts on:

Drift

Stale data

Feed disagreement

Reject storms

Threshold breaches

Deterministic replay for auditability

Safety mechanisms exist at multiple independent layers.

Architecture Overview
Integrity     → Market data validation & guardrails
Risk          → Capital limits, kill switch, thresholds
Execution     → Intent → order boundary enforcement
Strategy      → Plugin-based strategy host
Isolation     → Engine capital segregation
Reconcile     → Broker state validation
Database      → Run lifecycle + LIVE exclusivity
Backtest      → Deterministic event replay
CLI           → Operational control surface


Each layer is independently testable and validated through scenario-based tests.

Project Status

Workspace tests: Green

Lifecycle enforcement complete

Engine isolation enforced

Deterministic replay validated

Reliability hardening ongoing

Remaining work is tracked in:

patch_tracker_updated.md

Technical Documentation

For full setup instructions, database configuration, and operational commands:

➡ See README_TECHNICAL.md

Disclaimer

This repository is a research and engineering framework for systematic capital allocation.

It is not financial advice and is not intended for production capital deployment without additional operational review, monitoring infrastructure, and capital governance controls.