# MiniQuantDeskV4 — Technical README

This is the **hands-on** setup and ops guide for MiniQuantDeskV4.

**Core ideas:**
- deterministic inputs/outputs
- explicit run lifecycle
- integrity/risk gates before any execution boundary
- OMS-controlled order lifecycle and idempotent broker event ingestion
- scenario-driven validation (treat brokers/data as adversarial)

---

## Repository Structure

- `core-rs/` — Rust workspace
  - `crates/` — subsystem crates
    - `mqk-db` — persistence, outbox/inbox, run lifecycle, broker mapping
    - `mqk-execution` — broker gateway, order router, OMS state machine
    - `mqk-broker-paper` — deterministic paper broker adapter
    - `mqk-broker-alpaca` — live broker adapter
    - `mqk-runtime` — execution orchestrator / authoritative tick path
    - `mqk-reconcile` — broker snapshot normalization and drift handling
    - `mqk-risk` — execution boundary risk controls
    - `mqk-testkit` — scenario-driven execution and reliability test helpers
  - `mqk-gui/` — GUI control console (Tauri/React)
- `research-py/` — optional Python research CLI that emits deterministic run artifacts
- `config/` — layered config: defaults, environments, engines, risk profiles, stress profiles
- `runtime/` — runtime scaffolding/config (if present for your environment)
- `tests/fixtures/` — fixtures used by scenario tests

---

## Prereqs

### Core (Rust workspace)
- Rust (stable toolchain)
- Docker (recommended for Postgres)

### GUI (optional)
- Node.js + npm
- Tauri prerequisites (platform-dependent)

### Research (optional)
- Python 3.11+ recommended
- `pip` (or `uv` if you choose)

---

## Database (Postgres 16)

### Start Postgres via Docker (example)

```powershell
docker run --name mqk-postgres `
  -e POSTGRES_USER=mqk `
  -e POSTGRES_PASSWORD=mqk `
  -e POSTGRES_DB=mqk_v4 `
  -p 5432:5432 `
  -d postgres:16

(Optional but recommended) create a dedicated test DB:

docker exec -it mqk-postgres psql -U mqk -d postgres -c "CREATE DATABASE mqk_v4_test;"

Set the connection string (PowerShell):

$env:MQK_DATABASE_URL = "postgres://mqk:mqk@localhost:5432/mqk_v4_test"

### DB-backed proof lane bootstrap (repo-native)

From repo root:

```bash
bash scripts/db_proof_bootstrap.sh
```

- This command is the repo-native DB proof harness used by CI's `db-proof` job.
- It fails closed when `MQK_DATABASE_URL` is missing or DB-backed proofs fail.
- It runs only the DB-backed proof subset (not the full workspace).

One-command local Postgres bootstrap + DB proof run:

```bash
bash scripts/db_proof_bootstrap.sh --start-postgres
```

- `--start-postgres` starts/reuses local Docker container `mqk-postgres-proof` and sets `MQK_DATABASE_URL=postgres://mqk:mqk@127.0.0.1:5432/mqk_test`.
- Local DB proofs remain environment-dependent on Docker availability and port `5432`.

Execution Boundary Guarantees

The execution path is intentionally constrained.

Order intent does not call broker adapters directly.

The authoritative path is:

outbox claim
  -> gateway submit
  -> broker response persistence
  -> inbox event ingestion
  -> OMS transition
  -> portfolio mutation

Operational guarantees within scope:

broker submission is routed through a single gateway choke-point

broker events are ingested through a durable inbox with deduplication

OMS lifecycle transitions are explicit and reject illegal state changes

cancel/replace after partial fills preserves already-filled quantity

restart replay safety is driven by durable inbox state, not in-memory OMS state

Non-goal of this document:

this file does not claim that live broker resume/cursor handling is complete unless explicitly stated elsewhere in the current hardening plan

Rust Workspace Commands

All commands below assume you are in core-rs/.

Format / Lint / Test
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

Focused execution hardening gate:

cargo test -p mqk-execution --features testkit
cargo test -p mqk-broker-paper
cargo test -p mqk-runtime
cargo test -p mqk-testkit
CLI Entry Point

The CLI binary is mqk (crate: mqk-cli).

Run it via cargo:

cargo run -p mqk-cli -- --help
CLI: Common Operations
DB status + migrations
# Status
cargo run -p mqk-cli -- db status

# Apply migrations
cargo run -p mqk-cli -- db migrate
# Guardrail: refuses when LIVE is ARMED/RUNNING unless you acknowledge:
cargo run -p mqk-cli -- db migrate --yes

Migrations live under:

core-rs/crates/mqk-db/migrations/

Market Data: Canonical md_bars
Ingest from CSV → md_bars
cargo run -p mqk-cli -- md ingest-csv --path "<PATH_TO_CSV>" --timeframe "1D" --source "csv"
Ingest from provider → md_bars (provider path scaffolding)
cargo run -p mqk-cli -- md ingest-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D" `
  --start "2000-01-01" `
  --end "2026-01-01"
Deterministic Backtests
Backtest from a bars CSV
cargo run -p mqk-cli -- backtest csv `
  --bars "<PATH_TO_BARS_CSV>" `
  --timeframe-secs 60 `
  --initial-cash-micros 100000000000 `
  --integrity-enabled true `
  --integrity-stale-threshold-ticks 120 `
  --integrity-gap-tolerance-bars 0

Optional deterministic artifact output:

cargo run -p mqk-cli -- backtest csv --bars "<PATH>" --out-dir "runs/backtests"
Backtest from Postgres md_bars
cargo run -p mqk-cli -- backtest db `
  --timeframe "1D" `
  --start-end-ts 946684800 `
  --end-end-ts 1704067200 `
  --symbols "SPY,QQQ"

Note: start_end_ts / end_end_ts are epoch seconds for the bar end_ts range.

Run Lifecycle (paper/live scaffolding)

The Rust core enforces an explicit lifecycle and is designed to refuse unsafe transitions.

Typical flow:

# Create run
cargo run -p mqk-cli -- run start --engine "MAIN" --mode "PAPER" --config "config/defaults/base.yaml" --config "config/environments/windows-dev.yaml" --config "config/engines/main.yaml"

# Arm run
cargo run -p mqk-cli -- run arm --run-id "<RUN_ID>"

# Begin
cargo run -p mqk-cli -- run begin --run-id "<RUN_ID>"

# Stop
cargo run -p mqk-cli -- run stop --run-id "<RUN_ID>"

Other run commands exist (status / halt / loop / heartbeat / deadman enforcement) — see:

cargo run -p mqk-cli -- run --help
Execution Notes (Current Hardening State)

What is already enforced in the execution layer:

broker gateway as the single submission choke-point

internal ↔ broker order identity mapping

idempotent broker event ingestion

OMS lifecycle correctness for fills, cancels, rejects, and replace flows

partial-fill-safe cancel/replace behavior in paper mode

restart replay safety for previously ingested inbox rows

What is still under active hardening:

durable broker event cursor / restart resume state

broker error taxonomy and retry policy

ambiguous submit quarantine

live broker adapter completion / contract proof

single-runtime / leader-lease enforcement

Treat the system as reliability-hardened infrastructure in progress, not as a completed live-capital platform.

Daemon (Control Plane)

Run the daemon:

cargo run -p mqk-daemon

The daemon exposes:

status endpoints

lifecycle endpoints

SSE status stream

Exact host/port are config-driven.

GUI (Control Console)

From core-rs/mqk-gui/:

npm install
npx tsc --noEmit
npm run build

Dev mode (requires daemon reachable):

npm run tauri dev
Python Research (Optional)

From research-py/:

python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -U pip
.\.venv\Scripts\python.exe -m pip install -e .

Run the research CLI:

.\.venv\Scripts\python.exe -m mqk_research.cli --help

This layer is intended to produce deterministic artifacts (manifests + CSV outputs) that the Rust backtest/execution layers can consume.

Dev Discipline

This repo is designed to be patched one scoped change at a time:

implement one invariant (small + surgical)

add/extend a scenario test that proves it

run: cargo fmt, cargo test --workspace, cargo clippy ... -D warnings

commit with a patch label + rationale

Roadmap / hardening plan:

MiniQuantDesk_V4_Master_Patch_Plan_v2.md

patch_tracker_updated.md

MiniQuantDesk_V4_90plus_Patch_Tracker.md