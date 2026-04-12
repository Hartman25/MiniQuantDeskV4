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

mqk-broker-alpaca
Live Alpaca broker adapter (submit/cancel/replace/fetch-events via REST; WS inbound parse)

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

The following patch groups are listed here as an abbreviated early-stage history.
The full verified-closed patch list (with scenario-file proof references) is in
`docs/ai_workflow/MASTER_COMMAND_BRIEF.md` under "Closed — committed-state code/test
proof verified" and the "Additionally closed" table.  Do not treat the list below
as exhaustive — it covers only the foundational and broker series.

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
• crash matrix tests (I9 series)
• broker order map durability (EB-4: FK constraint + migration 0013)

---

### Alpaca External Broker (AP series — AP-01 through AP-09)

Deployment policy and broker implementation separation:

• AP-01/02: typed DeploymentMode + BrokerKind split
• AP-03: snapshot normalization (N1-N6 pure)
• AP-04: broker_snapshot_source emitted on every /system/status
• AP-05: alpaca_ws_continuity daemon-owned state machine (ColdStartUnproven/Live/GapDetected)
• AP-06/07: paper+alpaca and live-shadow+alpaca deployment modes proven
• AP-08: live-capital+alpaca deployment mode proven (capital token gate + WS continuity check)
• AP-09: operator-truth semantics — execution/reconcile gated on external WS continuity;
  GUI SystemStatus typed for broker_snapshot_source + alpaca_ws_continuity

Proof coverage: 75+ pure in-memory Alpaca adapter tests. Daemon route integration tests and
GUI/daemon contract gate tests have grown substantially beyond the initial counts — see
`docs/ai_workflow/MASTER_COMMAND_BRIEF.md` for the current verified-closed list.
See scripts/db_proof_bootstrap.sh for the external broker proof lane.

---

### GUI Hardening (H-series + PC-series)

All 19 operator-facing screens hard-block on truthState !== null.
Action catalog daemon-backed. Dead mode-change paths removed.
Legacy fallback authority propagates through panelSources. 46/46 GUI truth tests pass.

---

### Subsequent series (abbreviated — see MASTER_COMMAND_BRIEF for full list)

Major series proven in committed repo after the foundational batch above:

- **CC-01 through CC-06**: operator control surfaces, alerts/events, OMS/metrics routes
- **TV-01 through TV-EXEC-01B**: artifact promotion chain, deployability gate, parity evidence, capital policy, fill quality telemetry
- **LO-01 through LO-03G**: operator runbooks, stressed recovery matrix, live shadow proof
- **B1A through B8**: native strategy bootstrap, registry, fleet control, asset-class scope
- **AUTON-01, AC-01, AUTON-TRUTH-01, AH-01**: autonomous paper session controller, calendar truth, history durability
- **PTA-01/02, OPS-08/09**: canonical paper path, paper survivability, supervision proofs
- **C1 through C4, LT-01**: live trust series (live_trust_complete=False is current honest state)
- **BRK-00R-04/05/06/07R/08R/09R**: WS transport, cursor durability, gap recovery, reconcile start gate

Full list with scenario-file proof references: `docs/ai_workflow/MASTER_COMMAND_BRIEF.md`.

---

# Current System State

Repository builds cleanly:

```
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
```

Execution safety includes:

• outbox idempotency
• claim locking
• inbox dedupe
• crash restart quarantine
• broker order map durability
• external broker WS continuity truth gates

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
