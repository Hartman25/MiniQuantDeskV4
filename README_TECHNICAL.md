MiniQuantDeskV4

MiniQuantDeskV4 is an architecture-first quantitative trading system designed to operate as a disciplined capital allocator with enforced integrity, deterministic replay, and strict risk boundaries.

This version represents a ground-up reliability-focused rebuild with enforced lifecycle management, engine isolation, reconciliation guarantees, and scenario-driven validation.

Current System Status (as of PATCH 14)

All workspace tests are GREEN.

Implemented and validated:

Integrity Layer (PATCH 08)

No lookahead enforcement

Incomplete bar rejection

Gap fail (tolerance = 0)

Stale feed disarm

Feed disagreement halt

Broker Reconciliation (PATCH 09)

LIVE mode requires reconciliation before arming

Drift detection halts

Unknown broker orders halt

Dirty reconcile prevents LIVE start

Strategy Framework (PATCH 10)

Strategy plugin trait + host

Single strategy + single timeframe guards

Shadow mode generates intents without execution

Deterministic Backtesting (PATCH 11)

Event-sourced backtest engine

Deterministic replay

Ambiguity worst-case enforced

Stress impact measurable

Scenario-based correctness validation

Engine Isolation (PATCH 13)

Allocation caps enforced per engine

No cross-engine position bleed

Isolation enforced at config + runtime + execution boundary

Dedicated tests for cap enforcement

Run Lifecycle Enforcement (PATCH 14)

Explicit lifecycle: ARMED → RUNNING → STOPPED

DB-level uniqueness: one active LIVE run per engine

STOPPED exits active set at database constraint level

Scenario test validates exclusivity

System Guarantees

The system enforces:

One active LIVE run per engine (DB constraint)

No execution without arming

No LIVE without reconciliation

Allocation caps at execution boundary

Deterministic backtest replay

Scenario-based correctness tests

Risk guardrails on stale data, drift, feed disagreement, reject storms, PDT, and forced halts

This is a risk-first architecture. Safety boundaries are enforced at multiple layers.

Project Structure (core-rs)
crates/
  mqk-integrity      → Market data integrity + guardrails
  mqk-reconcile      → Broker reconciliation + drift detection
  mqk-strategy       → Strategy plugin framework
  mqk-execution      → Intent → order execution engine
  mqk-risk           → Risk controls + kill switch logic
  mqk-backtest       → Deterministic event-sourced backtesting
  mqk-isolation      → Engine allocation isolation layer
  mqk-db             → Run lifecycle + migrations
  mqk-cli            → CLI entry point

Prerequisites

Rust (stable)

Docker

PostgreSQL 16 (via Docker recommended)

Database Setup

Start PostgreSQL (example using Docker):

docker run --name mqk-postgres `
  -e POSTGRES_USER=mqk `
  -e POSTGRES_PASSWORD=mqk `
  -e POSTGRES_DB=mqk_v4 `
  -p 5432:5432 `
  -d postgres:16


Create a dedicated test database (recommended):

docker exec -it mqk-postgres psql -U mqk -d postgres -c "CREATE DATABASE mqk_v4_test;"


Set environment variable (PowerShell):

$env:MQK_DATABASE_URL = "postgres://mqk:mqk@localhost:5432/mqk_v4_test"

Migrations

Run database migrations via CLI:

cargo run -p mqk-cli -- db migrate


This applies all migrations under:

crates/mqk-db/migrations/

Running Tests

Full workspace:

cargo test


DB-specific lifecycle test:

cargo test -p mqk-db --test scenario_run_lifecycle_enforced


Backtest scenarios:

cargo test -p mqk-backtest

Backtesting

Backtesting uses the event-sourced engine:

Deterministic replay

Worst-case ambiguity fill logic

Stress slippage measurable

To run backtest scenarios:

cargo test -p mqk-backtest

CLI Usage

Migrate database:

cargo run -p mqk-cli -- db migrate


Additional CLI commands depend on configured modes and engines.

Operational Safety Notes

Always use a fresh test database for migrations.

Never reuse a production DB after altering migration files.

LIVE requires reconciliation before arming.

STOPPED runs exit the LIVE-active uniqueness constraint.

Allocation caps are enforced at execution boundary, not only at strategy layer.

Development Workflow

Make change.

Add scenario test.

Ensure:

cargo test


is GREEN.

Commit with PATCH numbering.

Push.

Status

System is currently in reliability hardening phase.
Patch tracker defines remaining work.

See: patch_tracker_updated.md