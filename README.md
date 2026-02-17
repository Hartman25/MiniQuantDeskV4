MiniQuantDeskV4

MiniQuantDeskV4 is a risk-first quantitative capital allocation framework designed for disciplined, deterministic, and operationally safe trading.

The system is built with enforced lifecycle controls, broker reconciliation requirements, deterministic backtesting, and strict capital isolation boundaries.

This is not a signal toy.
It is an architecture-first capital allocator.

Core Guarantees

The system enforces:

One active LIVE run per engine (database constraint)

Mandatory reconciliation before LIVE execution

Allocation caps at execution boundary

Cross-engine isolation (no capital bleed)

Deterministic event-sourced backtesting

Worst-case fill modeling under ambiguity

Automatic halts on drift, stale data, feed disagreement, reject storms, and threshold breaches

Safety is enforced at multiple independent layers.

System Maturity

Completed Phases:

Market integrity enforcement

Broker reconciliation layer

Strategy plugin framework

Deterministic backtest engine

Engine isolation layer

Run lifecycle enforcement (ARMED → RUNNING → STOPPED)

All workspace tests are currently green.

Architecture

The system is modular and workspace-based:

Integrity

Risk

Execution

Strategy

Isolation

Reconcile

Database lifecycle

Backtest engine

CLI

Each module has scenario-driven validation.

Operational Philosophy

MiniQuantDeskV4 assumes:

Markets are adversarial.

Data is unreliable.

Infrastructure fails.

Humans make mistakes.

The architecture is built to enforce constraints rather than assume discipline.

Current Status

The system is in reliability hardening mode.
Remaining work is tracked in patch_tracker_updated.md.

Technical Documentation

For full developer setup instructions and operational commands, see:

README_TECHNICAL.md