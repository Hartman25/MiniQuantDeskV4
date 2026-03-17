# MiniQuantDeskV4 — Technical README

This is the hands-on setup and operator guide for MiniQuantDeskV4.

## **What This Document Is For**

This file is the practical companion to the top-level README.

Use it for:
- local setup
- verification commands
- DB proof execution
- daemon and GUI startup
- CLI usage
- current operational boundaries

Use the root README for the high-level system story.

## **Core Principles**

- **Deterministic inputs and outputs**
- **Explicit run lifecycle**
- **Integrity and risk gates before execution**
- **OMS-controlled order lifecycle**
- **Durable outbox / inbox truth**
- **Scenario-driven reliability validation**
- **Fail-closed operator posture where truth is missing**

## **Repository Structure**

- `core-rs/` — Rust workspace
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
    - `mqk-broker-alpaca` — live broker adapter under hardening
    - `mqk-daemon` — HTTP control plane
    - `mqk-runtime` — authoritative execution path
    - `mqk-testkit` — scenario-driven reliability harness
    - `mqk-md` — historical/provider market-data support
  - `mqk-gui/` — Vite/React operator console
- `research-py/` — optional Python research CLI
- `config/` — layered config set
- `scripts/` — repo-native helper and proof scripts
- `docs/` — specs, checklists, runbooks, audits

## **Prerequisites**

### **Core Workspace**
- Rust stable toolchain
- Docker

### **GUI**
- Node.js + npm

### **Windows-Specific**
- Git Bash is useful because the repo-native DB proof harness is a shell script
- PowerShell is fine for Rust, Docker, daemon, and GUI commands

## **Database and DB Proof Lane**

### **Recommended Local Proof DB**
Run a dedicated local proof container so your repo testing does not collide with an existing local Postgres on port `5432`.

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

### **Repo-Native DB Proof Harness**
From repo root:

```powershell
& "C:\Program Files\Git\bin\bash.exe" -lc 'export MQK_DATABASE_URL="postgres://mqk:mqk@127.0.0.1:55432/mqk_test"; export DATABASE_URL="$MQK_DATABASE_URL"; ./scripts/db_proof_bootstrap.sh 2>&1 | tee db-proof.log'
```

What this proves:
- migration manifest / replay safety
- inbox dedupe and apply-fence behavior
- outbox idempotency / claim / recovery behavior
- restart quarantine behavior
- runtime lease behavior
- deadman / runtime lifecycle behavior
- arm-preflight and DB constraint behavior

This is the same proof lane CI uses in the `db-proof` job.

### **Fallback Local DB Helpers**
Also present in `scripts/`:
- `reset-mqk-testdb.ps1`
- `psql-local.ps1`
- `test-db.ps1`

Those are useful helpers, but the authoritative proof harness is `db_proof_bootstrap.sh`.

## **Core Verification Commands**

All Rust commands below assume you are in `core-rs/`.

### **Formatting / Lint / Broad Test**
```powershell
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### **GUI Contract Gate**
```powershell
cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate
```

### **Focused Execution / Runtime / Paper Checks**
```powershell
cargo test -p mqk-execution --features testkit
cargo test -p mqk-broker-paper
cargo test -p mqk-runtime
cargo test -p mqk-testkit
```

### **Workspace Build**
```powershell
cargo build --workspace
```

## **Current Deployment Reality**

This matters because the technical README should not lie.

### **What the daemon supports today**
- paper deployment with the `paper` adapter

### **What the daemon currently refuses fail-closed**
- `backtest`
- `live-shadow`
- `live-capital`

The current daemon selection path treats those modes as unsupported/unproven in the present architecture and refuses startup.

### **Default bind posture**
- default bind: `127.0.0.1:8899`
- non-loopback bind requires explicit opt-in via environment configuration

### **Operator auth posture**
If `MQK_OPERATOR_TOKEN` is not configured, privileged routes fail closed.

## **CLI Entry Point**

The CLI binary is `mqk`.

From `core-rs/`:

```powershell
cargo run -p mqk-cli -- --help
```

## **CLI — Common Operations**

### **DB Status and Migrations**
```powershell
cargo run -p mqk-cli -- db status
cargo run -p mqk-cli -- db migrate
cargo run -p mqk-cli -- db migrate --yes
```

Migrations live under:
- `core-rs/crates/mqk-db/migrations/`
- `core-rs/migrations/` for top-level/runtime-related additions

### **Config Hash**
```powershell
cargo run -p mqk-cli -- config-hash config/defaults/base.yaml config/environments/windows-dev.yaml config/engines/main.yaml
```

### **Market Data — CSV Ingest**
```powershell
cargo run -p mqk-cli -- md ingest-csv --path "<PATH_TO_CSV>" --timeframe "1D" --source "csv"
```

### **Market Data — Provider Ingest**
```powershell
cargo run -p mqk-cli -- md ingest-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D" `
  --start "2000-01-01" `
  --end "2026-01-01"
```

### **Market Data — Incremental Sync (`sync-provider`)**

`sync-provider` detects the latest stored bar per symbol and fetches only the bars needed to
extend coverage.  An overlap window is subtracted from the latest stored bar's date to re-ingest
recent bars and handle late completions.

**First run — no bars exist yet (full backfill required):**
```powershell
cargo run -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D" `
  --full-start "2020-01-01"
```

**Subsequent runs — incremental (no `--full-start` needed once bars exist):**
```powershell
cargo run -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D"
```

**Override end date or overlap:**
```powershell
cargo run -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY" `
  --timeframe "1D" `
  --end "2026-03-01" `
  --overlap-days 10
```

**Overlap defaults:** 5 calendar days for `1D`, 2 days for `5m`, 1 day for `1m`.

**`--end` default:** today's date (wall clock, operator command only — no wall clock use in
deterministic src/ paths).

**Output format (per run):**
```
mode=sync-provider
ingest_id=<uuid>
source=twelvedata
timeframe=1D
symbol=SPY effective_start=2026-03-11
symbol=QQQ effective_start=2026-03-11
rows_read=N rows_ok=N rejected=0 inserted=N updated=N
artifact=../exports/md_ingest/<uuid>/data_quality.json
sql=select ingest_id, created_at, stats_json from md_quality_reports where ingest_id='<uuid>';
```

**Notes:**
- `sync-provider` and `ingest-provider` share the same ingest path (`ingest_provider_bars_to_md_bars`).
  `ingest-provider` behavior is not changed.
- Ingest ID is deterministic: re-running with identical inputs produces the same UUID and upserts
  existing rows rather than duplicating them.
- Research/backtest paths should read from `md_bars` via `fetch_md_bars` or `mqk backtest db`
  rather than calling providers directly.

## **Deterministic Backtests**

### **Backtest from CSV**
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

### **Backtest from Postgres `md_bars`**
```powershell
cargo run -p mqk-cli -- backtest db `
  --timeframe "1D" `
  --start-end-ts 946684800 `
  --end-end-ts 1704067200 `
  --symbols "SPY,QQQ"
```

Notes:
- `start_end_ts` and `end_end_ts` are epoch seconds over the bar `end_ts` range
- the backtest engine is deterministic, but the full promotion-grade backtest/provenance layer is still being hardened

## **Run Lifecycle**

Typical flow:

### **Create a Run**
```powershell
cargo run -p mqk-cli -- run start `
  --engine "MAIN" `
  --mode "PAPER" `
  --config "config/defaults/base.yaml" `
  --config "config/environments/windows-dev.yaml" `
  --config "config/engines/main.yaml"
```

### **Arm**
```powershell
cargo run -p mqk-cli -- run arm --run-id "<RUN_ID>"
```

### **Begin**
```powershell
cargo run -p mqk-cli -- run begin --run-id "<RUN_ID>"
```

### **Heartbeat**
```powershell
cargo run -p mqk-cli -- run heartbeat --run-id "<RUN_ID>"
```

### **Stop**
```powershell
cargo run -p mqk-cli -- run stop --run-id "<RUN_ID>"
```

### **Halt**
```powershell
cargo run -p mqk-cli -- run halt --run-id "<RUN_ID>" --reason "manual halt"
```

### **Status**
```powershell
cargo run -p mqk-cli -- run status --run-id "<RUN_ID>"
```

Other lifecycle helpers exist:
```powershell
cargo run -p mqk-cli -- run --help
```

## **Daemon**

Run from `core-rs/`:

```powershell
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
cargo run -p mqk-daemon
```

Default local URL:
- `http://127.0.0.1:8899`

You may also need:
```powershell
$env:MQK_OPERATOR_TOKEN = "<your-token>"
```

if you want privileged routes to succeed instead of failing closed.

## **GUI**

Run from `core-rs/mqk-gui/`:

```powershell
npm ci
npm run build
npm run dev
```

Default dev URL:
- `http://127.0.0.1:5173`

The GUI defaults to daemon URL:
- `http://127.0.0.1:8899`

### **Important**
The desktop/Tauri shell is not the primary documented path here.  
The practical repo-native operator flow today is:
- run daemon
- run Vite GUI
- point the GUI at the daemon

## **One-Shot Local Launch (Two Shells)**

### **Shell 1 — Daemon**
```powershell
cd C:\Users\<YOU>\Desktop\MiniQuantDeskV4\core-rs
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:55432/mqk_test"
cargo run -p mqk-daemon
```

### **Shell 2 — GUI**
```powershell
cd C:\Users\<YOU>\Desktop\MiniQuantDeskV4\core-rs\mqk-gui
npm ci
npm run dev
```

If you use `Start-Process`, keep the DB URL assignment quoted correctly inside the spawned command.

## **Python Research Layer (Optional)**

From `research-py/`:

```powershell
python -m venv .venv
.\.venv\Scripts\python.exe -m pip install -U pip
.\.venv\Scripts\python.exe -m pip install -e .
.\.venv\Scripts\python.exe -m mqk_research.cli --help
```

This layer is intended to emit deterministic artifacts that the Rust backtest/execution stack can consume.

## **CI Overview**

The current GitHub Actions pipeline includes:

- **GUI contract lane**
  - GUI build
  - daemon/GUI contract gate

- **Safety guards**
  - unsafe-pattern checks
  - migration-governance checks
  - ignored-proof hygiene checks

- **Rust lane**
  - `cargo fmt --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace`

- **DB proof lane**
  - repo-native Postgres proof harness

## **Development Discipline**

This repo is designed to be patched in small, test-backed units.

Recommended discipline:
1. change one invariant at a time
2. add or extend a scenario test that proves it
3. run targeted checks first
4. run broader checks after milestone patches
5. only commit once the patch and its directly affected surfaces are proven

## **Current Technical Caveats**

Be honest about these:

- the daemon/operator plane is improving, but not all GUI detail surfaces are fully authoritative yet
- the daemon currently fail-closes unsupported/unproven deployment modes
- the backtest system is strong, but still being hardened toward promotion-grade provenance and lifecycle realism
- “scenario-tested” does **not** mean “safe for live capital by default”

## **Reference Docs**

Useful repo docs:
- `docs/GUI_CONVERGENCE_CHECKLIST.md`
- `docs/ci/gui_daemon_contract_waivers.md`
- `docs/specs/`
- `docs/runbooks/`
- `MiniQuantDesk_V4_90plus_Patch_Tracker.md`
- `MiniQuantDeskV4_Foundation_Patch_Tracker.md`
