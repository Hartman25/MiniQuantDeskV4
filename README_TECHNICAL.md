# MiniQuantDeskV4 — Technical README

This is the hands-on setup, proof, and operator guide for MiniQuantDeskV4.

## What this document is for

Use this file for:

- local setup
- proof and verification commands
- DB proof execution
- daemon and GUI startup
- CLI usage
- current deployment boundaries
- operator workflow reality

Use the root `README.md` for the high-level system story.

## Current proved posture

The repo snapshot used for this README refresh reflects a clean committed state and a completed full DB-backed proof run through `full_repo_proof.ps1 -ProofProfile full -LowMemory`.

That matters because this technical README is meant to describe the strongest currently proved path, not an aspirational one.

The strongest current operational route is still:

- `paper` deployment mode
- `alpaca` adapter
- daemon + Vite GUI operator path
- DB-backed proof and guard lanes as the load-bearing validation standard

This is a materially stronger operator posture than early scaffold state, but it is still **not** a safe-live-capital blanket claim.

## Core principles

- **Deterministic inputs and outputs**
- **Explicit run lifecycle**
- **Integrity and risk gates before execution**
- **OMS-controlled order lifecycle**
- **Durable outbox / inbox truth**
- **Scenario-driven reliability validation**
- **Fail-closed operator posture where truth is missing**

## Repository structure

- `core-rs/` — authoritative Rust workspace
  - `crates/`
    - `mqk-config` — layered config loading and config-hash support
    - `mqk-db` — persistence, outbox/inbox, run lifecycle, broker mapping, proof-backed DB contracts
    - `mqk-audit` — audit and structured event support
    - `mqk-artifacts` — run artifact initialization and report writing
    - `mqk-cli` — CLI entrypoint
    - `mqk-execution` — broker gateway, order router, OMS state machine
    - `mqk-portfolio` — fill application and position/accounting behavior
    - `mqk-risk` — execution-boundary risk controls
    - `mqk-integrity` — stale/gap/disagreement controls
    - `mqk-reconcile` — broker snapshot normalization and mismatch handling
    - `mqk-strategy` — strategy interface layer
    - `mqk-backtest` — deterministic backtest engine
    - `mqk-promotion` — promotion/evaluation layer
    - `mqk-broker-paper` — deterministic paper broker adapter
    - `mqk-broker-alpaca` — Alpaca adapter under hardening
    - `mqk-daemon` — HTTP control plane
    - `mqk-runtime` — authoritative execution path
    - `mqk-testkit` — scenario-driven reliability harness
    - `mqk-md` — historical/provider market-data support
    - `mqk-isolation` — cross-engine isolation and anti-state-bleed support
    - `mqk-schemas` — shared schema contracts
  - `mqk-gui/` — Vite/React operator console
- `research-py/` — optional Python research CLI
- `config/` — layered config sets
- `scripts/` — repo-native helper and proof scripts
- `docs/` — specs, checklists, runbooks, audits
- `assets/` — branding and diagrams

Operationally, `MAIN` is the canonical engine.
`EXP` is a research-side experimental sandbox and should not be treated as readiness truth unless explicitly promoted.

## Prerequisites

### Core workspace

- Rust stable toolchain
- Docker

### GUI

- Node.js + npm

### Windows-specific

- Git Bash is useful because the repo-native DB proof harness is a shell script
- PowerShell is fine for Rust, Docker, daemon, GUI, and the root proof runner
- optional desktop bootstrap scripts exist under `scripts/windows/`, but the primary documented path remains daemon + browser GUI unless you have validated the desktop shell locally

## Database and proof model

### Recommended local proof DB

Run a dedicated local proof container so repo testing does not collide with another local Postgres on port `5432`.

```powershell
docker run --name mqk-postgres-proof `
  -e POSTGRES_USER=mqk `
  -e POSTGRES_PASSWORD=mqk `
  -e POSTGRES_DB=mqk_test `
  -p 55432:5432 `
  -d postgres:16
```

Sanity-check it:

```powershell
docker exec mqk-postgres-proof pg_isready -U mqk -d mqk_test
docker exec mqk-postgres-proof psql -U mqk -d mqk_test -c "select current_user, current_database();"
```

### Canonical local proof harness

`full_repo_proof.ps1` at repo root is the authoritative local proof runner.
It runs the required lanes in sequence and writes a structured summary to `.proof/full_repo_proof_output.txt`.

```powershell
# Non-DB local proof
.\full_repo_proof.ps1 -ProofProfile local

# Low-memory Windows posture
.\full_repo_proof.ps1 -ProofProfile local -LowMemory

# Full DB-backed institutional proof
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full

# Full DB-backed proof using the proven Windows low-memory profile
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
.\full_repo_proof.ps1 -ProofProfile full -LowMemory
```

When `-LowMemory` is active, the harness sets or preserves the proven Windows posture:

- `CARGO_BUILD_JOBS=1`
- `CARGO_INCREMENTAL=0`
- `RUSTFLAGS=-C debuginfo=0`
- all test lanes run with `--test-threads=1`

Use that profile on Windows hosts where linker or codegen parallelism causes OOM pressure.

### Repo-native DB proof bootstrap

`scripts/db_proof_bootstrap.sh` is the underlying DB proof harness invoked by `full_repo_proof.ps1` and by CI `db-proof`.

```powershell
& "C:\Program Files\Git\bin\bash.exe" -lc 'export MQK_DATABASE_URL="postgres://mqk:mqk@127.0.0.1:55432/mqk_test"; export DATABASE_URL="$MQK_DATABASE_URL"; ./scripts/db_proof_bootstrap.sh 2>&1 | tee db-proof.log'
```

What this proves:

- migration manifest and replay safety
- inbox dedupe and apply-fence behavior
- outbox idempotency, claim, and recovery behavior
- restart quarantine behavior
- runtime lease behavior
- deadman and runtime lifecycle behavior
- arm-preflight and DB constraint behavior
- market-data provider ingest and incremental sync DB behavior

Prefer running it through `full_repo_proof.ps1 -ProofProfile full` so the full lane set stays bundled.

### Local DB helpers

Also present in `scripts/`:

- `reset-mqk-testdb.ps1` — reset the local proof DB
- `psql-local.ps1` — interactive psql shortcut

Deprecated wrappers such as `test-all.ps1`, `test-db.ps1`, and `ci_gate.ps1` should not be used for operator validation. The canonical local proof entrypoint is `full_repo_proof.ps1`.

## Core verification commands

All Rust commands below assume you are in `core-rs/`.

### Formatting, lint, and broad tests

```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### GUI contract gate

```powershell
cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate
cargo test -p mqk-daemon --test scenario_route_contract_rt01
```

### GUI local truth checks

From `core-rs/mqk-gui/`:

```powershell
npm ci
npm run test
npm run build
```

### Focused execution, runtime, and broker checks

```powershell
cargo test -p mqk-execution --features testkit
cargo test -p mqk-broker-paper
cargo test -p mqk-broker-alpaca
cargo test -p mqk-runtime
cargo test -p mqk-testkit
```

### Workspace build

```powershell
cargo build --workspace
```

## Current deployment reality

This section is intentionally blunt.

### Valid daemon combinations today

Valid mode + adapter combinations with `deployment_start_allowed: true`:

- `paper` mode + `alpaca` adapter — canonical honest paper path
- `live-shadow` mode + `alpaca` adapter — typed support, no capital authority
- `live-capital` mode + `alpaca` adapter — typed support with additional runtime gates

### Fail-closed combinations

- `paper` mode + `paper` adapter — refused; not a valid start-authoritative daemon combination
- `live-shadow` or `live-capital` with `paper` adapter — refused
- any unrecognized adapter ID — refused
- `backtest` deployment in daemon runtime — unconditionally refused

### Strongest current operational path

The strongest current daemon path is canonical **Paper + Alpaca autonomous paper**.

That path has:

- truthful readiness at `GET /api/v1/autonomous/readiness`
- truthful autonomous-paper fields on `GET /api/v1/system/preflight`
- NYSE-session-aware autonomous controller behavior
- WS continuity gating before start
- durable autonomous supervisor history in Postgres
- autonomous session rows surfaced in `GET /api/v1/events/feed`
- a one-day soak harness: `scripts/paper_soak_day.sh`
- an operator runbook: `docs/runbooks/autonomous_paper_ops.md`

That does **not** make it live-capital ready.
It means paper/autonomous operator truth is materially stronger than before.

### Important vocabulary mismatch

- daemon deployment labels use `paper`, `live-shadow`, `live-capital`, and `backtest`
- `mqk run start` still uses the older run/config vocabulary: `BACKTEST | PAPER | LIVE`
- do not assume CLI `LIVE` maps one-to-one to daemon `live-shadow` versus `live-capital`

### Default bind posture

- default bind: `127.0.0.1:8899`
- non-loopback bind requires explicit opt-in through environment configuration

### Operator auth posture

If `MQK_OPERATOR_TOKEN` is not configured, privileged routes fail closed.

### Control-plane mode transitions

Mode transitions are restart-based, not hot-swapped.

Current truthful operator workflow:

- `change-system-mode` remains a guidance/compatibility path that returns `409`
- canonical operator actions now include persisted restart-intent workflow through `/api/v1/ops/action`
- `request-mode-change` can persist a restart intent when the transition is admissible-with-restart
- `cancel-mode-transition` can cancel a pending durable restart intent
- the action catalog exposes those truthful restart workflows instead of pretending hot mode changes are authoritative

## CLI entry point

The CLI binary is `mqk`.

From `core-rs/`:

```powershell
cargo run -p mqk-cli -- --help
```

## CLI common operations

### DB status and migrations

```powershell
cargo run -p mqk-cli -- db status
cargo run -p mqk-cli -- db migrate
cargo run -p mqk-cli -- db migrate --yes
```

Authoritative migration source:

- `core-rs/crates/mqk-db/migrations/`

Any tracked SQL file under another `/migrations/` path is rejected by migration governance guards.

### Config hash

```powershell
cargo run -p mqk-cli -- config-hash config/defaults/base.yaml config/environments/windows-dev.yaml config/engines/main.yaml
```

### Market data — CSV ingest

```powershell
cargo run -p mqk-cli -- md ingest-csv --path "<PATH_TO_CSV>" --timeframe "1D" --source "csv"
```

### Market data — provider ingest

```powershell
cargo run -p mqk-cli -- md ingest-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D" `
  --start "2000-01-01" `
  --end "2026-01-01"
```

### Market data — incremental sync

First run, when no bars exist yet:

```powershell
cargo run -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D" `
  --full-start "2020-01-01"
```

Subsequent incremental runs:

```powershell
cargo run -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D"
```

Override end date or overlap:

```powershell
cargo run -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY" `
  --timeframe "1D" `
  --end "2026-03-01" `
  --overlap-days 10
```

Notes:

- default overlap is 5 calendar days for `1D`, 2 days for `5m`, and 1 day for `1m`
- `--end` defaults to today for this operator-facing command
- `sync-provider` and `ingest-provider` share the same ingest path
- ingest ID is deterministic for identical inputs
- research and backtest paths should read from `md_bars` rather than calling providers directly

## Deterministic backtests

### Backtest from CSV

```powershell
cargo run -p mqk-cli -- backtest csv `
  --bars "<PATH_TO_BARS_CSV>" `
  --timeframe-secs 60 `
  --initial-cash-micros 100000000000 `
  --integrity-enabled true `
  --integrity-stale-threshold-ticks 120 `
  --integrity-gap-tolerance-bars 0
```

Optional artifact output:

```powershell
cargo run -p mqk-cli -- backtest csv `
  --bars "<PATH_TO_BARS_CSV>" `
  --out-dir "runs/backtests"
```

### Backtest from Postgres `md_bars`

```powershell
cargo run -p mqk-cli -- backtest db `
  --timeframe "1D" `
  --start-end-ts 946684800 `
  --end-end-ts 1704067200 `
  --symbols "SPY,QQQ"
```

Notes:

- `start_end_ts` and `end_end_ts` are epoch seconds over the `end_ts` bar range
- the backtest engine is deterministic, but promotion-grade provenance and realism are still being hardened

## Run lifecycle

Typical flow:

### Create a run

```powershell
cargo run -p mqk-cli -- run start `
  --engine "MAIN" `
  --mode "PAPER" `
  --config "config/defaults/base.yaml" `
  --config "config/environments/windows-dev.yaml" `
  --config "config/engines/main.yaml"
```

### Arm

```powershell
cargo run -p mqk-cli -- run arm --run-id "<RUN_ID>"
```

### Begin

```powershell
cargo run -p mqk-cli -- run begin --run-id "<RUN_ID>"
```

### Heartbeat

```powershell
cargo run -p mqk-cli -- run heartbeat --run-id "<RUN_ID>"
```

### Stop

```powershell
cargo run -p mqk-cli -- run stop --run-id "<RUN_ID>"
```

### Halt

```powershell
cargo run -p mqk-cli -- run halt --run-id "<RUN_ID>" --reason "manual halt"
```

### Status

```powershell
cargo run -p mqk-cli -- run status --run-id "<RUN_ID>"
```

### Deadman check

```powershell
cargo run -p mqk-cli -- run deadman-check --run-id "<RUN_ID>" --ttl-seconds 60
```

### Deadman enforce

```powershell
cargo run -p mqk-cli -- run deadman-enforce --run-id "<RUN_ID>" --ttl-seconds 60
```

Other helpers exist:

```powershell
cargo run -p mqk-cli -- run --help
```

## Daemon

Run from `core-rs/`:

```powershell
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
$env:MQK_OPERATOR_TOKEN = "dev-local-operator-token"
$env:MQK_DAEMON_DEPLOYMENT_MODE = "paper"
$env:MQK_DAEMON_ADAPTER_ID = "alpaca"
$env:ALPACA_API_KEY_PAPER = "<your-paper-key>"
$env:ALPACA_API_SECRET_PAPER = "<your-paper-secret>"
cargo run -p mqk-daemon
```

Default local URL:

- `http://127.0.0.1:8899`

Optional session override variables:

```powershell
$env:MQK_SESSION_START_HH_MM = "14:30"
$env:MQK_SESSION_STOP_HH_MM = "21:00"
```

Use those only if you explicitly want to override the default NYSE regular-session autonomous window.

### Useful daemon surfaces for the canonical paper path

- `GET /api/v1/system/status`
- `GET /api/v1/system/preflight`
- `GET /api/v1/autonomous/readiness`
- `GET /api/v1/alerts/active`
- `GET /api/v1/events/feed`
- `GET /api/v1/ops/catalog`
- `POST /api/v1/ops/action`

## GUI

Run from `core-rs/mqk-gui/`:

```powershell
npm ci
npm run build
npm run dev
```

Default dev URL:

- `http://127.0.0.1:5173`

Default daemon URL:

- `http://127.0.0.1:8899`

### Practical operator path

The practical repo-native operator flow today is still:

- run daemon
- run Vite GUI
- point the GUI at the daemon

### Optional Windows desktop bootstrap

An optional Windows desktop bootstrap exists under:

- `scripts/windows/Launch-VeritasLedger.ps1`
- `scripts/windows/Install-VeritasLedgerDesktopShortcut.ps1`

Intent of that path:

- desktop launcher verifies canonical local daemon identity before GUI open
- observe/attach and trade-ready launcher modes both exist
- desktop privileged actions are canonical-only, not legacy-fallback

Treat it as an operator convenience path that still requires local Windows validation on your machine.
The browser GUI + daemon path remains the primary documented workflow.

## One-shot local launch (two shells)

### Shell 1 — daemon

```powershell
cd C:\Users\<YOU>\Desktop\MiniQuantDeskV4\core-rs
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
$env:MQK_OPERATOR_TOKEN = "dev-local-operator-token"
$env:MQK_DAEMON_DEPLOYMENT_MODE = "paper"
$env:MQK_DAEMON_ADAPTER_ID = "alpaca"
$env:ALPACA_API_KEY_PAPER = "<your-paper-key>"
$env:ALPACA_API_SECRET_PAPER = "<your-paper-secret>"
cargo run -p mqk-daemon
```

### Shell 2 — GUI

```powershell
cd C:\Users\<YOU>\Desktop\MiniQuantDeskV4\core-rs\mqk-gui
npm ci
npm run dev
```

If you use `Start-Process`, keep the DB URL assignment quoted correctly inside the spawned command.

## Python research layer (optional)

From `research-py/`:

```powershell
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -U pip
.\.venv\Scripts\python.exe -m pip install -e .
.\.venv\Scripts\python.exe -m mqk_research.cli --help
```

This layer is intended to emit deterministic artifacts that the Rust stack can consume.

## CI overview

Current GitHub Actions coverage includes:

- **GUI contract lane** (`ubuntu-latest`)
  - GUI truth tests
  - GUI build
  - daemon/GUI contract gate

- **Safety guards** (`ubuntu-latest`)
  - unsafe-pattern checks
  - migration-governance checks
  - ignored-proof hygiene checks

- **Rust lane** (`ubuntu-latest`, with Postgres service)
  - `cargo fmt --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`

- **DB proof lane** (`ubuntu-latest`, with Postgres service)
  - repo-native Postgres proof harness (`scripts/db_proof_bootstrap.sh`)

- **Windows platform lane** (`windows-latest`, no DB)
  - `cargo fmt --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace -- --test-threads=1`
  - `CARGO_BUILD_JOBS=1` + `CARGO_INCREMENTAL=0` + `RUSTFLAGS=-C debuginfo=0` reproduces the proven local `-LowMemory` posture

## Development discipline

This repo should be patched in small, test-backed units.

Recommended discipline:

1. change one invariant at a time
2. add or extend the scenario test that proves it
3. run targeted checks first
4. run broader checks after milestone patches
5. only commit once the patch and the directly affected surfaces are proven

## Current technical caveats

Be honest about these:

- the daemon/operator plane is materially stronger, but some deeper GUI detail surfaces remain intentionally deferred or unmounted rather than faked
- the daemon has typed support for paper, live-shadow, and live-capital on Alpaca, but typed support is not the same thing as safe live operation
- the backtest system is strong, but still being hardened toward promotion-grade provenance and lifecycle realism
- shadow/live parity evidence is not yet strong enough for a safe unattended live claim
- scenario-tested does **not** mean safe for live capital by default

## Reference docs

Useful repo docs:

- `docs/GUI_CONVERGENCE_CHECKLIST.md`
- `docs/ci/gui_daemon_contract_waivers.md`
- `docs/ci/dependency_governance.md`
- `docs/runbooks/operator_workflows.md`
- `docs/runbooks/autonomous_paper_ops.md`
- `docs/runbooks/live_shadow_operational_proof.md`
- `docs/runbooks/common_failure_modes.md`
- `docs/specs/`
- `docs/runbooks/`
- `docs/INSTITUTIONAL_READINESS_LOCK.md`
- `docs/INSTITUTIONAL_SCORECARD.md`
- `MiniQuantDesk_V4_90plus_Patch_Tracker.md`
- `MiniQuantDeskV4_Foundation_Patch_Tracker.md`
