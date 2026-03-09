# MiniQuantDesk V4 — AI Operator Context

This file provides **operational context for AI assistants** working on MiniQuantDesk V4.

Any AI session interacting with this repository should read this file first.

---

# Project Purpose

MiniQuantDesk V4 is a **deterministic algorithmic trading platform** designed to achieve **institutional-grade execution safety**.

Primary design goals:

• deterministic execution
• crash safety
• idempotent order flow
• broker reconciliation
• strict research/live parity
• deterministic backtesting
• operational fail-closed behavior
• auditability

The system is designed so that **real capital can be deployed safely**.

---

# Technology Stack

Core implementation:

Rust workspace

```
core-rs/
```

Database:

Postgres
SQLx migrations

Research layer:

Python tooling

GUI:

Rust daemon + web frontend

---

# Workspace Structure

Important crates:

```
mqk-db
Database schema and migrations

mqk-execution
Execution types and order intent generation

mqk-runtime
Orchestrator and dispatch loop

mqk-broker-paper
Deterministic paper broker adapter

mqk-reconcile
Broker reconciliation engine

mqk-risk
Risk enforcement

mqk-integrity
Invariant enforcement

mqk-backtest
Deterministic backtesting engine

mqk-strategy
Strategy logic

mqk-cli
Command line interface

mqk-testkit
Scenario tests

mqk-daemon
Service runtime

mqk-gui
Graphical interface
```

---

# Execution Pipeline

The trading pipeline follows this deterministic flow:

```
strategy
→ targets
→ order intents
→ outbox
→ runtime claim
→ broker submit
→ broker events
→ inbox
→ portfolio apply
```

Execution safety guarantees:

• outbox idempotency
• inbox deduplication
• deterministic event ordering
• crash recovery
• restart safety
• reconcile enforcement

---

# Critical Architecture Rules

These rules must **never be violated**.

---

## Determinism

Execution must be replayable from artifacts.

Do NOT introduce:

• randomness
• hidden IO
• uncontrolled system time

Time must be injected through deterministic interfaces.

---

## Runtime Boundary

Only `mqk-runtime` may:

• claim outbox rows
• submit broker orders
• advance execution state

No other crate may bypass this boundary.

---

## Paper / Live Parity

Paper broker must mimic live behavior:

• partial fills
• fill ordering
• cancel/replace behavior
• broker event ordering

---

## Fail-Closed Safety

System must halt/disarm if safety cannot be guaranteed.

Examples:

• reconcile drift
• invariant violations
• stale market data
• unsafe restart state

System must **never fail open**.

---

# Patch Implementation Process

MiniQuantDesk is being finalized using a **structured patch plan**.

Each patch must pass these gates:

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets --no-fail-fast
```

Patches must be:

• minimal
• deterministic
• architecture-preserving

---

# Completed Patches

The following patch groups are complete.

### Foundational Closure

FC-1
FC-2
FC-3

---

### Runtime Hardening

RT series patches implemented:

• runtime boundary enforcement
• outbox claim locking
• idempotent dispatch
• crash recovery protections

---

### Execution Safety

Includes:

• inbox dedupe
• outbox idempotency
• deterministic fill ordering
• crash matrix tests

---

### Patch 2 — Restart Quarantine

Patch 2 implemented restart safety.

If runtime restarts and finds outbox rows in:

```
DISPATCHING
SENT
```

without broker confirmation evidence, the system:

• quarantines the rows
• persists recovery reason
• transitions to HALT/DISARM
• refuses further dispatch

Tests:

```
scenario_restart_quarantines_dispatching_outbox
scenario_restart_quarantines_sent_outbox
```

---

# Current System State

Repository currently builds cleanly:

```
cargo fmt
cargo clippy
cargo test --workspace
```

Execution safety currently includes:

• outbox idempotency
• claim locking
• inbox dedupe
• crash restart quarantine

---

# Remaining Critical Work

Next patch:

## Patch 3 — Broker Order Map Durability

Goal:

Persist mapping:

```
client_order_id → broker_order_id
```

before acknowledging dispatch.

Why:

Without this mapping:

• restart cannot reconcile orders
• fills cannot be matched reliably
• cancel/replace safety is weakened

Required tests:

• broker order map survives restart
• crash during dispatch preserves mapping
• reconcile can match fills to correct order

---

# AI Collaboration Rules

AI assistants must:

• avoid architectural redesign
• preserve deterministic execution
• respect runtime boundaries
• add tests for crash scenarios
• ensure compile + clippy + tests pass

Large patches may generate **Claude prompts** for code generation.

---

# Operational Assumptions

Assume:

• real capital will run through the system
• broker events may be duplicated
• broker events may be delayed
• broker events may arrive out of order
• process may crash at any instruction
• DB transactions may partially commit

All code must survive these conditions.

---

# AI Session Bootstrapping

When starting a new AI session:

1. Load this file.
2. Analyze repository state.
3. Continue patch plan implementation.
4. Do not introduce nondeterminism.
