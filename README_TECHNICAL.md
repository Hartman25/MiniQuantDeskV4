# MiniQuantDeskV4 — Technical README

This is the **hands-on** setup and ops guide for MiniQuantDeskV4.

**Core ideas:**
- deterministic inputs/outputs
- explicit run lifecycle
- integrity/risk gates before any execution boundary
- scenario-driven validation (treat brokers/data as adversarial)

---

## Repository Structure

- `core-rs/` — Rust workspace
  - `crates/` — subsystem crates
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
```

(Optional but recommended) create a dedicated test DB:

```powershell
docker exec -it mqk-postgres psql -U mqk -d postgres -c "CREATE DATABASE mqk_v4_test;"
```

Set the connection string (PowerShell):

```powershell
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@localhost:5432/mqk_v4_test"
```

---

## Rust Workspace Commands

All commands below assume you are in `core-rs/`.

### Format / Lint / Test

```powershell
cargo fmt
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### CLI Entry Point

The CLI binary is `mqk` (crate: `mqk-cli`).

Run it via cargo:

```powershell
cargo run -p mqk-cli -- --help
```

---

## CLI: Common Operations

### DB status + migrations

```powershell
# Status
cargo run -p mqk-cli -- db status

# Apply migrations
cargo run -p mqk-cli -- db migrate
# Guardrail: refuses when LIVE is ARMED/RUNNING unless you acknowledge:
cargo run -p mqk-cli -- db migrate --yes
```

Migrations live under:

- `core-rs/crates/mqk-db/migrations/`

---

## Market Data: Canonical `md_bars`

### Ingest from CSV → `md_bars`

```powershell
cargo run -p mqk-cli -- md ingest-csv --path "<PATH_TO_CSV>" --timeframe "1D" --source "csv"
```

### Ingest from provider → `md_bars` (provider path scaffolding)

```powershell
cargo run -p mqk-cli -- md ingest-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D" `
  --start "2000-01-01" `
  --end "2026-01-01"
```

---

## Deterministic Backtests

### Backtest from a bars CSV

```powershell
cargo run -p mqk-cli -- backtest csv `
  --bars "<PATH_TO_BARS_CSV>" `
  --timeframe-secs 60 `
  --initial-cash-micros 100000000000 `
  --integrity-enabled true `
  --integrity-stale-threshold-ticks 120 `
  --integrity-gap-tolerance-bars 0
```

Optional deterministic artifact output:

```powershell
cargo run -p mqk-cli -- backtest csv --bars "<PATH>" --out-dir "runs/backtests"
```

### Backtest from Postgres `md_bars`

```powershell
cargo run -p mqk-cli -- backtest db `
  --timeframe "1D" `
  --start-end-ts 946684800 `
  --end-end-ts 1704067200 `
  --symbols "SPY,QQQ"
```

> Note: `start_end_ts` / `end_end_ts` are epoch seconds for the **bar end_ts** range.

---

## Run Lifecycle (paper/live scaffolding)

The Rust core enforces an explicit lifecycle and is designed to refuse unsafe transitions.

Typical flow:

```powershell
# Create run
cargo run -p mqk-cli -- run start --engine "MAIN" --mode "PAPER" --config "config/defaults/base.yaml" --config "config/environments/windows-dev.yaml" --config "config/engines/main.yaml"

# Arm run
cargo run -p mqk-cli -- run arm --run-id "<RUN_ID>"

# Begin
cargo run -p mqk-cli -- run begin --run-id "<RUN_ID>"

# Stop
cargo run -p mqk-cli -- run stop --run-id "<RUN_ID>"
```

Other run commands exist (status / halt / loop / heartbeat / deadman enforcement) — see:

```powershell
cargo run -p mqk-cli -- run --help
```

---

## Daemon (Control Plane)

Run the daemon:

```powershell
cargo run -p mqk-daemon
```

The daemon exposes:
- status endpoints
- lifecycle endpoints
- SSE status stream

Exact host/port are config-driven.

---

## GUI (Control Console)

From `core-rs/mqk-gui/`:

```powershell
npm install
npx tsc --noEmit
npm run build
```

Dev mode (requires daemon reachable):

```powershell
npm run tauri dev
```

---

## Python Research (Optional)

From `research-py/`:

```powershell
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -U pip
.\.venv\Scripts\python.exe -m pip install -e .
```

Run the research CLI:

```powershell
.\.venv\Scripts\python.exe -m mqk_research.cli --help
```

This layer is intended to produce deterministic artifacts (manifests + CSV outputs) that the Rust backtest/execution layers can consume.

---

## Dev Discipline

This repo is designed to be patched **one scoped change at a time**:

1) implement one invariant (small + surgical)
2) add/extend a scenario test that proves it
3) run: `cargo fmt`, `cargo test --workspace`, `cargo clippy ... -D warnings`
4) commit with a patch label + rationale

Roadmap / hardening plan:
- `MiniQuantDesk_V4_Master_Patch_Plan_v2.md`
- `patch_tracker_updated.md`
