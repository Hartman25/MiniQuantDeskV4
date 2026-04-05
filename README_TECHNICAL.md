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

Operationally, `MAIN` is the canonical engine. `EXP` is a research-side experimental sandbox and should not be treated as current readiness or operator truth unless explicitly promoted.

## **Prerequisites**

### **Core Workspace**
- Rust stable toolchain
- Docker

### **GUI**
- Node.js + npm

### **Windows-Specific**
- Git Bash is useful because the repo-native DB proof harness is a shell script
- PowerShell is fine for Rust, Docker, daemon, GUI, and the root proof runner
- Optional desktop bootstrap scripts now exist under `scripts/windows/`, but the primary documented path remains daemon + Vite GUI unless and until you validate the desktop shell on your machine

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

### **Canonical Local Proof Harness (full_repo_proof.ps1)**

`full_repo_proof.ps1` at repo root is the authoritative local proof runner.  
It runs all required proof lanes in sequence and produces a structured JSON summary.

```powershell
# Non-DB local proof (fmt + clippy + workspace tests + daemon/runtime/broker/GUI lanes + guards):
.\full_repo_proof.ps1 -ProofProfile local

# Low-memory Windows profile — reproduces the proven Windows posture:
#   CARGO_BUILD_JOBS=1, CARGO_INCREMENTAL=0, RUSTFLAGS=-C debuginfo=0
# (each set only if not already overridden); all test lanes use --test-threads=1:
.\full_repo_proof.ps1 -ProofProfile local -LowMemory

# Full DB-backed institutional proof (requires MQK_DATABASE_URL pointing at a live Postgres):
.\full_repo_proof.ps1 -ProofProfile full
```

The transcript is saved to `.proof/full_repo_proof_output.txt`. When `-LowMemory` is active
this harness prints the full active settings in the transcript header so the proof posture is unambiguous.

### **Repo-Native DB Proof Bootstrap (underlying shell harness)**

`scripts/db_proof_bootstrap.sh` is the DB proof shell script invoked by `full_repo_proof.ps1`
and by CI `db-proof`. You can run it directly if needed:

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
- market-data provider ingest and incremental sync DB behavior

This is the same proof lane CI uses in the `db-proof` job. Prefer running it via
`full_repo_proof.ps1 -ProofProfile full` so the full lane set runs together.

### **Local DB Helpers**
Also present in `scripts/`:
- `reset-mqk-testdb.ps1` — reset the local proof DB
- `psql-local.ps1` — interactive psql shortcut

**Deprecated scripts** (`test-all.ps1`, `test-db.ps1`, `ci_gate.ps1`) are stale wrappers
that do not cover the full canonical proof lane set. Each file contains a deprecation
warning pointing to `full_repo_proof.ps1`. Do not use them for operator validation.

## **Core Verification Commands**

All Rust commands below assume you are in `core-rs/`.

### **Formatting / Lint / Broad Test**
```powershell
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

### **GUI Contract Gate**
```powershell
cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate
cargo test -p mqk-daemon --test scenario_route_contract_rt01
```

### **GUI Local Truth Checks**
From `core-rs/mqk-gui/`:

```powershell
npm ci
npm run test
npm run build
```

### **Focused Execution / Runtime / Broker Checks**
```powershell
cargo test -p mqk-execution --features testkit
cargo test -p mqk-broker-paper
cargo test -p mqk-broker-alpaca
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

Valid mode+adapter combinations with `deployment_start_allowed: true`:

- `paper` mode + `alpaca` adapter — Alpaca paper endpoint, NYSE weekday/session calendar (AUTON-CALENDAR-01); canonical honest paper path
- `live-shadow` mode + `alpaca` adapter — Alpaca live endpoint, NYSE weekday calendar, no capital authority
- `live-capital` mode + `alpaca` adapter — Alpaca live endpoint, NYSE weekday calendar, real capital semantics; additional runtime gates (dev-token check, WS continuity proven) are enforced at start beyond the static readiness check

`paper` mode + `paper` adapter is **fail-closed** (`start_allowed: false`); the paper+paper combination is refused at the deployment-readiness gate (PT-TRUTH-01). It is not a valid start combination.

Typed support and `start_allowed: true` exist in source and are tested for all three combinations above.

**Operational trust for live-shadow and live-capital is still partial.** Typed support in source is not the same as operational trust. Runbooks, recovery proof, and shadow-to-live parity evidence are not yet strong enough for safe live claims. Do not treat typed support as proof of safe live operation.

### **Strongest current operational path: Paper + Alpaca autonomous paper**

The strongest daemon path in the current snapshot is the canonical **Paper + Alpaca** route.

That path now has:
- truthful readiness at `GET /api/v1/autonomous/readiness`
- truthful autonomous-paper fields on `GET /api/v1/system/preflight`
- NYSE-session-aware autonomous controller behavior
- WS continuity gating before start
- durable autonomous supervisor history in Postgres
- autonomous session rows surfaced in `GET /api/v1/events/feed`
- a one-day soak harness: `scripts/paper_soak_day.sh`
- an operator runbook: `docs/runbooks/autonomous_paper_ops.md`

That does **not** make it live-capital ready. It means paper/autonomous operator truth is materially stronger than before.

### **What the daemon unconditionally refuses**

- `backtest` mode: not supported in the daemon runtime; refuses start fail-closed regardless of adapter

### **What else fails closed**

- `live-shadow` or `live-capital` with the `paper` adapter: refused (paper adapter cannot provide real external broker truth for these modes)
- any mode with an unrecognised adapter ID: refused fail-closed

### **Important vocabulary mismatch**
- daemon deployment labels use `paper`, `live-shadow`, `live-capital`, and `backtest`
- `mqk run start` still uses the older config/run-row mode vocabulary: `BACKTEST | PAPER | LIVE`
- do not assume CLI `LIVE` maps one-to-one to daemon `live-shadow` versus `live-capital`; they describe different layers

### **Default bind posture**
- default bind: `127.0.0.1:8899`
- non-loopback bind requires explicit opt-in via environment configuration

### **Operator auth posture**
If `MQK_OPERATOR_TOKEN` is not configured, privileged routes fail closed.

### **Control-plane mode transitions**
Mode transitions are still **restart-based**, not hot-swapped.

Current operator truth:
- `change-system-mode` remains a guidance/compatibility path that returns 409
- canonical operator actions now include persisted restart-intent workflow via `/api/v1/ops/action`
- `request-mode-change` can persist a restart intent when the transition is admissible-with-restart
- `cancel-mode-transition` can cancel a pending durable restart intent
- the action catalog now exposes those truthful operator workflows rather than pretending hot mode changes are authoritative

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
- `core-rs/crates/mqk-db/migrations/` — the only authoritative migration source

There is exactly one migration authority. Any SQL file tracked under a path containing `/migrations/` but outside `core-rs/crates/mqk-db/migrations/` is rejected by `scripts/guards/check_migration_governance.sh` and the CI guard step.

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
extend coverage. An overlap window is subtracted from the latest stored bar's date to re-ingest
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
- The repo-native DB proof lane explicitly runs both `scenario_md_ingest_provider` and
  `scenario_md_sync_provider`, so incremental-sync DB semantics are part of promoted proof rather than
  hidden in ignored-only tests.

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

### **Deadman Check**
```powershell
cargo run -p mqk-cli -- run deadman-check --run-id "<RUN_ID>" --ttl-seconds 60
```

### **Deadman Enforce**
```powershell
cargo run -p mqk-cli -- run deadman-enforce --run-id "<RUN_ID>" --ttl-seconds 60
```

Other lifecycle helpers exist:
```powershell
cargo run -p mqk-cli -- run --help
```

## **Daemon**

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

You may also need:
```powershell
$env:MQK_SESSION_START_HH_MM = "14:30"
$env:MQK_SESSION_STOP_HH_MM = "21:00"
```

if you want to override the default NYSE regular-session autonomous window.

### **Useful daemon surfaces for the canonical paper path**
- `GET /api/v1/system/status`
- `GET /api/v1/system/preflight`
- `GET /api/v1/autonomous/readiness`
- `GET /api/v1/alerts/active`
- `GET /api/v1/events/feed`
- `GET /api/v1/ops/catalog`
- `POST /api/v1/ops/action`

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
The practical repo-native operator flow today is still:
- run daemon
- run Vite GUI
- point the GUI at the daemon

### **Optional Windows desktop bootstrap**
An optional Windows desktop bootstrap exists under:
- `scripts/windows/Launch-VeritasLedger.ps1`
- `scripts/windows/Install-VeritasLedgerDesktopShortcut.ps1`

Current intent:
- desktop launcher verifies canonical local daemon identity before GUI open
- observe/attach and trade-ready launcher modes both exist
- desktop privileged actions are canonical-only, not legacy-fallback

Treat this as an operator convenience path that still requires local Windows runtime validation on your machine. The browser GUI + daemon path remains the primary documented workflow.

## **One-Shot Local Launch (Two Shells)**

### **Shell 1 — Daemon**
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

- **Windows platform lane** (`windows-latest`, no DB) — CI-PLATFORM-01
  - `cargo fmt --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`
  - `cargo test --workspace -- --test-threads=1`
  - `CARGO_BUILD_JOBS=1` + `CARGO_INCREMENTAL=0` + `RUSTFLAGS=-C debuginfo=0` reproduces the proven local `-LowMemory` posture exactly
  - proves the Rust build is clean on the actual operator OS class
  - no DB lanes: Postgres service containers are not available on `windows-latest`

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

- the daemon/operator plane is materially stronger, but some deeper GUI detail surfaces remain intentionally deferred or unmounted rather than faked
- the daemon now has typed support for paper (Alpaca adapter only for start-authoritative paper execution), live-shadow (Alpaca adapter), and live-capital (Alpaca adapter); backtest is unconditionally refused; operational trust for live modes remains partial and is not yet strongly proven
- the backtest system is strong, but still being hardened toward promotion-grade provenance and lifecycle realism
- “scenario-tested” does **not** mean “safe for live capital by default”

## **Reference Docs**

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
- `MiniQuantDesk_V4_90plus_Patch_Tracker.md`
- `MiniQuantDeskV4_Foundation_Patch_Tracker.md`
