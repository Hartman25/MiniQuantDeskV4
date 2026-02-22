# MiniQuantDeskV4 — Technical README

MiniQuantDeskV4 is a reliability-focused trading system built around:
- deterministic backtesting + replay,
- strict integrity/risk boundaries,
- reconciliation gates for LIVE,
- DB-enforced run lifecycle rules,
- and scenario-driven validation.

This file covers setup, DB, tests, and operational commands.

---

## Current System Status (Current Repo State)

Workspace tests are GREEN.

Implemented and validated (high level):

### Integrity (scenario-tested)
- no-lookahead enforcement
- incomplete bar rejection
- gap detection (zero-tolerance supported)
- stale feed disarm
- feed disagreement halt
- deadman-style halt scenarios

### Reconciliation (scenario-tested)
- LIVE can require reconcile before arming
- drift detection halts
- unknown broker orders halt
- dirty reconcile blocks LIVE start
- snapshot normalization adapter exists (`mqk-reconcile/src/snapshot_adapter.rs`)

### Strategy Framework
- plugin trait + host shape
- guardrails for single-strategy/single-timeframe assumptions (where configured)
- shadow mode support exists in the architecture

### Execution Boundary
- intent → order routing boundary exists
- order_router is exported and test-covered

### Risk
- allocation/exposure constraints tested
- PDT helper module exists (`mqk-risk/src/pdt.rs`) with unit tests

### Deterministic Backtesting
- event-sourced engine + deterministic replay
- worst-case ambiguity fill logic
- stress impact measurable
- scenario-driven validation

### Market Data Ingest
- CSV ingest → canonical `md_bars` in Postgres
- provider ingest → canonical `md_bars` in Postgres
- Data Quality Gate reports produced for ingests
- idempotency behaviors tested at ingest level

### Run Lifecycle Enforcement
- explicit lifecycle: ARMED → RUNNING → STOPPED
- DB uniqueness: one active LIVE run per engine
- STOPPED exits active set
- scenario tests validate exclusivity and lifecycle rules

### Control Plane
- daemon API provides status + lifecycle endpoints and SSE status stream
- GUI can act as a control console over daemon endpoints

---

## Repository Structure (core-rs)


core-rs/
crates/
mqk-artifacts → run artifacts + reports
mqk-audit → audit log/hash-chain primitives
mqk-backtest → deterministic event-sourced backtesting
mqk-cli → CLI entry point + operational commands
mqk-config → layered config + deterministic hashing + unused-key guards
mqk-daemon → HTTP/SSE control plane
mqk-db → schema/migrations + run lifecycle + ingest persistence
mqk-execution → intent → order boundary + order_router
mqk-integrity → market data integrity + guardrails
mqk-isolation → engine allocation isolation layer
mqk-md → market data ingest/provider/normalization/quality
mqk-portfolio → portfolio accounting + metrics (in progress in roadmap)
mqk-promotion → promotion gate evaluator (thresholds + tie-breaks)
mqk-reconcile → reconcile + drift detection + snapshot_adapter
mqk-risk → risk controls + PDT helpers
mqk-strategy → strategy plugin framework
mqk-gui/ → GUI control console (React/Tauri)


---

## Prerequisites

- Rust (stable)
- Docker
- PostgreSQL 16 (Docker recommended)
- Node.js (for GUI)

---

## Database Setup

Start PostgreSQL (example Docker):

```powershell
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

cd core-rs
cargo run -p mqk-cli -- db migrate

Migrations live under:

core-rs/crates/mqk-db/migrations/
Running Tests

Full workspace:

cd core-rs
cargo test --workspace

Targeted crates:

cargo test -p mqk-db
cargo test -p mqk-backtest
cargo test -p mqk-integrity
cargo test -p mqk-reconcile
cargo test -p mqk-risk
cargo test -p mqk-execution
cargo test -p mqk-md

Clippy (warnings as errors):

cargo clippy --workspace --all-targets -- -D warnings

Format:

cargo fmt
Running the Daemon (Control Plane)

From core-rs/:

cargo run -p mqk-daemon

Expected capabilities:

status endpoint (includes integrity armed flag)

lifecycle endpoints (start/stop/halt)

integrity endpoints (arm/disarm)

SSE status stream

(Exact host/port are set by daemon config; GUI assumes the daemon is reachable and streaming status.)

Running the GUI (Control Console)

From core-rs/mqk-gui/:

npm install
npx tsc --noEmit
npm run build

Dev mode (requires daemon reachable):

npm run tauri dev

GUI behavior (current):

connects to daemon SSE stream for live status

polls status as fallback

provides control buttons for start/stop/halt and arm/disarm

shows an event log buffer

Operational Safety Notes (Reality Check)

This repo is in reliability hardening.

Before any real-capital deployment, you must complete the live-safety roadmap:

single authoritative run loop and choke-point submission path

outbox-first idempotent broker submit discipline

minimal OMS lifecycle state machine

inbox apply idempotency proven (“insert → apply” atomic invariant)

sticky disarm/deadman across restarts

reconcile-before-arm plus periodic reconcile tick that hard-halts

The ordered plan and definition-of-done lives in:

MiniQuantDeskV4_Patch_By_Patch_Implementation_Plan.md

Development Workflow

Make one scoped change (one patch item).

Add/extend scenario test proving the invariant.

Ensure:

cargo fmt

cargo test --workspace

cargo clippy --workspace --all-targets -- -D warnings

Commit with patch label and short rationale.

Status

System is currently in reliability hardening phase.
Patch tracker / implementation plan defines remaining work.


---

If you want, paste your **current repo root tree** (or just `ls` of `core-r