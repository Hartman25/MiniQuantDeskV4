# MiniQuantDeskV4 — Technical README

This is the hands-on setup, proof, and operator guide for MiniQuantDeskV4.

## What this document is for

Use this file for:

- local setup
- env-file workflow
- proof and verification commands
- DB proof execution
- daemon and GUI startup
- CLI usage
- current deployment boundaries
- operator workflow reality

Use the root `README.md` for the high-level system story.

## Current proved posture

**Repository snapshot used for this update (2026-07-20):** local `main` at
`3591064a805efc82b3f6468e1de0fe06ea028471`
(`docs: require coverage authority before bar processing`), plus independent
ChatGPT/operator acceptance of D4 and its evaluation-lineage repair together
— **Phase D is accepted complete in full** — plus independent acceptance of
the four-times-corrected
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-DURABLE-OUTCOME-AUTHORITY-AND-EVIDENCE-CONTRACT
(Phase E1: the read-only architecture audit producing the binding
durable-outcome/no-trade contract for Phase E) — **E1 is accepted complete**
— plus the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-COVERAGE-ANCHOR-AND-RUN-LINEAGE-FOUNDATION
patch on top of it: the first Phase E runtime code. E2A implements the
durable, operation-scoped `autonomous_daily_coverage_bound` evidence event
(typed model, canonical construction, write/re-read/replay/conflict
contract), the coordinator's ensure-authority seam, the completed-bar
adapter's mandatory per-tick authority/mid-day-drift gate, and a raw
full-run-lineage read/validate helper — plus, on top of that, the
AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E2A-AUTHORITY-ENVELOPE-GATE-ORDERING-AND-CONCURRENCY-CLOSURE
repair: a complete durable event-envelope validator (id, event_type, source,
`run_id IS NULL`, `resume_source IS NULL`), a duplicate-JSON-key-rejecting
typed parser, the adapter's authority gate reordered strictly before any
assignment/identity resolution so a missing anchor stays a quiet no-op even
under a locally malformed environment, and a live `tokio::join!`-driven
coordinator/adapter concurrency proof — **E2A repair implementation
complete, awaiting ChatGPT and operator acceptance.** No outcome classifier
and no finalization behavior exist yet; that remains E2B.

The strongest current operational route is:

- `paper` deployment mode
- `alpaca` adapter
- long-only, single-symbol US equity/ETF lane
- daemon + Vite GUI operator path
- DB-backed targeted scenarios and repository guards as the load-bearing development proof
- `full_repo_proof.ps1` as the final locked-snapshot proof runner

The historical 2026-06-01 full DB-backed low-memory proof passed 18/18 lanes, but the repository has
advanced materially since that snapshot; this README does not represent that historical 18/18
transcript as a fresh full-repo proof of the current commit.

### Active Bundle 3 boundary

`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED` remains open.

Current local `main` contains, accepted (D1–D4, Phase D accepted complete in full):

- durable daily-operation identity, state/version CAS, and append-only transitions
- canonical session boundaries and nontrading-day reconciliation
- typed start, recovery, and stop retries
- exact completed-bar observation and durable dispatch claims
- production `main.rs` cutover from the legacy blind ticker to the supervised completed-bar task
  (legacy ticker retained in source for compatibility tests only, never spawned in production)
- retained supervisor ownership and shutdown wait behavior
- full restart-budget exhaustion proof (bounded to 3 restarts / 4 worker generations)
- durable operation degradation when the task permanently fails
- sticky operator failure truth (survives session-controller's own Running-style projections)
- complete typed classification of non-recoverable driver/setup outcomes
- task-level PrepareDataOnly → RunningDispatch → exactly-once proof
- closure of a confirmed completed-bar dispatch-ownership race: the completed-bar driver's durable
  claim previously deposited into the same shared, account-wide `pending_strategy_bar_input`
  mailbox the ordinary execution loop drains every tick, then immediately re-took it — a concurrent
  execution-loop tick could steal the deposited bar first, causing the claim to be recorded failed
  despite a real evaluation having occurred. The claim now dispatches directly through
  `AppState::dispatch_native_strategy_for_symbol_with_bar` (the same canonical exact-input dispatch
  implementation, just called with the claim's own bar value) and never touches the mailbox;
  execution-loop and manual-signal-route dispatch are unchanged. A deterministic concurrency proof
  (both interleaving orderings) and one integrated scenario test driving a synthetic Paper+Alpaca
  day through preopen, canonical start, running dispatch, runtime interruption/recovery, session
  close, and shutdown together accompany this fix.
- a shared deterministic identity helper (`AppState::derive_strategy_signal_evaluation_id`) used by
  both the signal-evaluation journal writer and the completed-bar claim path — never a second,
  independently-derived algorithm; the claim path durably confirms the exact
  `strategy_signal_evaluations` row before completing a claim; the completion write's `Result<bool>`
  is captured and matched explicitly, routing `Ok(false)`/`Err` through one authoritative re-read;
  the full-day lifecycle test's preopen phase resolves through real production readiness truth with
  zero manual unstick; a supervised-task proof under an injected clock

Accepted (Phase E1, four-times-corrected):

- the read-only architecture audit
  (`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-DURABLE-OUTCOME-AUTHORITY-AND-EVIDENCE-CONTRACT`,
  `docs/specs/autonomous_daily_paper_operations_01e_outcome_truth_contract.md`) producing the
  binding contract for Phase E's durable daily outcome/no-trade classification: which durable store
  is outcome authority (`sys_autonomous_daily_operations`, already Phase-B-built and unused by any
  production writer today), exactly when an operation becomes finalization-eligible
  (`stopped_at_utc IS NOT NULL` plus no locally-owned runtime/completed-bar-task activity remaining
  — `postclose_finalize_utc` is a stop-retry escalation deadline, not a finalization gate), the
  activity and no-trade evidence hierarchies, an `unknown_insufficient_evidence` representation that
  reuses the existing nonterminal `evidence_degraded` state (no migration required), evidence-
  conflict precedence, a restart/idempotency contract reusing the existing CAS transition machinery,
  a bounded reason-code matrix, a read-only API contract for two net-new
  `GET /api/v1/autonomous/daily-operation[s]` routes, a notification contract, the corrected
  operation-scoped `autonomous_daily_coverage_bound` coverage-anchor seam, the raw full-run-lineage
  read/validate contract, and the **E2A/E2B** implementation decomposition, across four correction
  passes that closed source-proven defects found by fresh, targeted re-reads of the driver/coordinator
  source
- **no Phase E runtime code was written by E1 itself** — E1 is documentation/guard-only; the classifier,
  coordinator wiring, API routes, and durable coverage-anchor/run-lineage foundation remained
  E2A/E2B/E3/E4's job

Implemented on the local `main` worktree (Phase E2A, plus its
AUTHORITY-ENVELOPE-GATE-ORDERING-AND-CONCURRENCY-CLOSURE repair), implementation complete but awaiting
independent ChatGPT/operator acceptance:

- the typed, schema-versioned `CoverageBoundDetail` payload
  (`core-rs/crates/mqk-daemon/src/state/autonomous_daily_coverage_authority.rs`), a duplicate-key-
  rejecting typed-wire-struct parser (`CoverageBoundDetailWire`, `#[serde(deny_unknown_fields)]` —
  decodes directly into a typed struct rather than a `serde_json::Value` object map, so serde's own
  derived `visit_map` rejects a literal duplicate field the moment it is seen a second time; rejects
  missing/wrong-type/unknown-field/unknown-schema-version payloads), and `#[derive(PartialEq)]`-based
  semantic equality over every immutable field — the payload deliberately excludes `bound_at_utc`; the
  bind instant is the event row's own `ts_utc` column, metadata only
- `validate_coverage_authority_envelope`: the complete durable event-envelope validator every authority
  read now uses — verifies the row's exact deterministic `id`, `event_type ==
  autonomous_daily_coverage_bound`, `source == mqk-daemon.autonomous_daily_coordinator`, `run_id IS
  NULL`, and `resume_source IS NULL`, never merely the id and a matching JSON detail payload
- `construct_coverage_bound_detail`, a pure, side-effect-free constructor reusing only
  `daily_data_readiness::expected_intraday_end_ts_window`/`intraday_grid_starts` — no second
  calendar, timeframe, grace, or completed-bar algorithm. The first dispatchable bar is the final
  element of the expected window evaluated at `effective_operation_open_utc` (may spill into the
  previous session at an ordinary open); the final dispatchable bar is the last current-session grid
  identity whose expectation instant is strictly greater than the first bar's own and strictly less
  than `effective_operation_close_utc` (a close-boundary bar is excluded), or the first bar itself
  when none qualifies
- `check_coverage_authority_envelope` (Stage A: exact-id fetch, complete envelope validation,
  duplicate-safe parse, payload `operation_id` verification — requires no assignment/runtime-binding/
  policy resolution of any kind) composed with `write_and_confirm_coverage_authority` /
  `check_coverage_authority` (Stage B: semantic comparison against the one already-loaded authority
  value, never re-read or re-parsed through a second algorithm): the exact write/re-read/idempotent-
  replay/conflict contract over the existing `sys_autonomous_session_events` store (`ON CONFLICT (id)
  DO NOTHING`, id = `autonomous_daily_coverage_bound:{operation_id}`) — a write error is never trusted
  without a confirming authoritative re-read through the same envelope validator, and a row with any
  single tampered envelope field (`event_type`/`source`/`run_id`/`resume_source`) or a tampered
  `detail.operation_id` is rejected without ever overwriting the original row
- the coordinator's `ensure_coverage_authority` seam
  (`state/autonomous_daily_coordinator.rs`), run immediately after `create_or_recover` and before any
  state-handler dispatch, for both newly created and recovered operations: a pristine operation (zero
  `run_id`/`started_at_utc`/bars/claims/running-lineage, via `check_operation_pristine`) may bind a
  fresh anchor; any operation with prior activity and no anchor fails closed to
  `coverage_authority_missing_after_activity` (`running` degrades to `evidence_degraded`; other
  nonterminal states degrade to `manual_intervention_required`), reusing the existing D1
  blocker-signature mechanism — never a retroactively fabricated anchor. Close priority is preserved:
  at or after `effective_operation_close_utc`, canonical close/stop reconciliation always takes
  precedence over a fresh coverage blocker
- the completed-bar production adapter's mandatory per-tick authority gate
  (`state/autonomous_completed_bar_task.rs`), corrected to a two-stage order: after operation fetch and
  the cheap state-only mode short-circuit, Stage A (`check_coverage_authority_envelope`) runs strictly
  *before* any assignment/runtime-binding resolution is even attempted — a missing, unreadable, or
  envelope-invalid authority returns `CoverageAuthorityUnavailable` with zero lifecycle mutation and
  zero attempt to resolve local assignment/runtime configuration at all, so this stays true even when
  that local environment/configuration is itself malformed. Only once Stage A proves a real,
  correctly-shaped authority exists does the adapter resolve its current policy, construct the fresh
  payload, and semantically compare it (Stage B) against the one authority value Stage A already
  loaded — strictly before `load_driver_instruments` or any provider/registry object is built. The
  `CoverageAuthorityUnavailable { operation_id, reason_code }` outcome variant carries the four closed
  reason codes (`coverage_authority_not_bound` / `_unreadable` / `_invalid` / `_conflict`); a missing
  anchor is a quiet, no-mutation no-op, while every other case refuses the driver without the adapter
  itself mutating lifecycle state — durable fail-closed projection remains coordinator-owned. Once
  Stage A has proven the authority present and valid, the existing source-aligned
  `IdentityUnresolved`/blocker behavior for assignment/runtime/policy failures is preserved unchanged
- mid-day coverage-policy drift: the adapter's `resolve_current_coverage_policy_inputs` resolves
  `timeframe_secs`/`required_history_bars`/`effective_grace_seconds` from the assignment's own
  configured timeframe and the strategy registry's data requirements on every tick (deliberately
  ignoring `EffectiveRuntimeBinding::effective_runtime_timeframe_secs` for this purpose, so this
  resolver never preempts the driver's own separate `runtime_strategy_timeframe_mismatch` readiness
  blocker with a competing check); any field disagreeing with the bound anchor is
  `coverage_authority_conflict`, proven for both `PrepareDataOnly`- and `RunningDispatch`-eligible
  states
- `mqk_db::fetch_autonomous_daily_operation_running_transitions_raw` /
  `validate_autonomous_daily_operation_run_lineage` /
  `fetch_and_validate_autonomous_daily_operation_run_lineage`: a raw, unbounded `(transition_seq,
  run_id)` read (`to_state = 'running'`, never `SELECT DISTINCT`, never the general-purpose 100-row
  API list cap) plus Rust-side strict-monotonicity/uniqueness/current-run-match validation, proven
  against a real two-run recovery cycle and a 150-row fixture
- `mqk_db::fetch_autonomous_session_event_by_id` (exact primary-key read) and
  `mqk_db::count_autonomous_daily_bar_dispatch_claims` — the two narrow `mqk-db` read helpers this
  foundation needed; no migration
- a live, deterministic coordinator/adapter concurrency proof
  (`AutonomousCoverageAuthorityPreBindTestHook`, `state/autonomous_daily_coordinator.rs`, mirroring the
  existing D4.4 `AutonomousCompletedBarPostClaimTestHook` pattern): a `tokio::sync::Notify`-based
  rendezvous pauses the real coordinator tick immediately after `create_or_recover` commits the
  operation row and before `ensure_coverage_authority` begins; production never installs the hook (one
  uncontended async mutex lock per tick). The scenario test drives the real coordinator tick and the
  real production adapter tick concurrently via `tokio::join!`, proving the adapter observes
  `coverage_authority_not_bound` with zero lifecycle mutation, zero claims, and zero bar observations
  while the coordinator is paused, then proves a normal eligible tick proceeds once released
- `tests/scenario_autonomous_daily_coverage_anchor_and_run_lineage_01.rs` (41 tests): construction
  bounds (ordinary-open spillover, close-boundary exclusion, no-later-bar-qualifies), serialize/parse
  round-trip and tamper cases including duplicate-JSON-key rejection, semantic-equality field
  sensitivity, pure run-lineage validation, the durable write/replay/conflict contract including five
  independent envelope-field tamper cases, the coordinator's pristine-bind and prior-activity
  fail-closed paths (plus close priority), the adapter's corrected two-stage authority gate including a
  deterministic zero-side-effect proof for a newly-visible not-yet-anchored operation, a
  proceeds-once-bound proof, and the live `tokio::join!` concurrency proof, mid-day drift for both
  eligible modes, and the DB-backed run-lineage read/validate helper
- **no outcome classifier and no finalization behavior were written** — `outcome`/`finalized_at_utc`
  remain unwritten by any production code path; no new API route; no GUI change; no migration; no
  `is_legal_operation_transition` graph change

After D4 and its evaluation-lineage repair (Phase D, accepted complete in full) and the
four-times-corrected Phase E1 contract (accepted complete) are both accepted, Bundle 3 still requires
Phase E2A's (and its authority-envelope/gate-ordering/concurrency closure repair's) independent
acceptance, then Phase E2B (strict classifier and finalization CAS, built on E2A's authorities), E3
coordinator integration, E4 read-only API, E5 integrated proof and closure, Phase F GUI/runbook/soak
preparation, and Phase G final closure.

### Operational meaning

Completion of Bundle 3 is the boundary for beginning a **supervised autonomous paper soak**.
It is not a live-capital authorization and it is not the end of the operational roadmap.

The intended post-Bundle-3 sequence is:

1. run final targeted and full locked-snapshot proof
2. start with operator-watched Paper + Alpaca sessions
3. collect roughly 10–20 clean autonomous sessions
4. close real-fill, reconcile, Discord, restart, and repeated-cycle evidence
5. complete Bundle 4 durable paper portfolio and P&L truth before treating accounting as restart-safe

This is a materially stronger operator posture than early scaffold state, but it is still **not**
a safe-live-capital blanket claim.

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

## Local env-file workflow

The repo ships `.env.local.example` as the canonical local starting point.
It states that `.env.local` is loaded automatically by both `mqk-cli` and `mqk-daemon`.
That is true **when the file is in the current working directory used to launch them**.

### Practical rule

- launch from the **repo root** if you want a repo-root `.env.local` to auto-load
- if you launch from `core-rs/`, place a copy at `core-rs/.env.local` or export the needed env vars manually

This matters because many older command examples start with `cd core-rs`, while the snapshot keeps `.env.local.example` at repo root.

### Recommended local pattern

1. Copy the template:

```powershell
Copy-Item .env.local.example .env.local
```

2. Fill in the values you actually use.

3. For daemon and CLI runs that should auto-load repo-root `.env.local`, use repo-root launches such as:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- --help
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon
```

### What the local env file usually owns

At minimum, local runtime work normally needs:

- `MQK_DATABASE_URL`
- `MQK_OPERATOR_TOKEN`
- `MQK_DAEMON_DEPLOYMENT_MODE`
- `MQK_DAEMON_ADAPTER_ID`
- `ALPACA_API_KEY_PAPER`
- `ALPACA_API_SECRET_PAPER`

Optional but common entries include session-window overrides, Discord webhooks, and artifact/capital policy paths.

## Proof DB vs runtime DB

This repo now has a clearer local DB split than older docs suggested.
Do not collapse these into one mental model.

### Runtime/operator DB

Use a **runtime DB** for actual daemon, GUI, and autonomous paper work.
The template `.env.local.example` currently uses this runtime default:

```text
MQK_DATABASE_URL=postgres://postgres:postgres@localhost:5432/mqk_dev
```

If you keep that default, a compatible local Postgres looks like this:

```powershell
docker run --name mqk-postgres-dev `
  -e POSTGRES_USER=postgres `
  -e POSTGRES_PASSWORD=postgres `
  -e POSTGRES_DB=mqk_dev `
  -p 5432:5432 `
  -d postgres:16
```

You can use a different runtime DB layout.
What matters is that your daemon and CLI point to the URL you actually configured.

### Proof DB

Use a **separate disposable proof DB** for proof work.
The recommended isolated manual example binds to `55432` specifically to avoid collisions with a normal runtime DB on `5432`.

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

### DB proof bootstrap default

`scripts/db_proof_bootstrap.sh --start-postgres` has its own default local Docker path.
It starts or reuses a Postgres 16 container on **5432** and defaults to:

```text
postgres://mqk:mqk@127.0.0.1:5432/mqk_test
```

That is fine for quick proof work, but it is a different path from the isolated manual `55432` example above.

### Reality-test DB path

The snapshot also includes a committed autonomous paper reality-test PowerShell script at repo root:

- `autonomous_reality_test_paper.ps1.ps1`

That script intentionally uses its **own isolated Docker default path**:

- container: `mqk-reality-postgres`
- host port: `5440`
- DB user/password: `mqk` / `mqk`
- DB name: `mqk_v4`

That separation is deliberate.
Treat reality-test DB state as a different lane from both everyday runtime ops and proof DB work.

### Verify ports before trusting any default above

The ports above (`5432`, `55432`, `5440`) are *defaults*, not guarantees. On a machine that already
has long-running containers for other purposes — e.g. a persistent live-trading or paper-trading
Postgres container — one or more of those ports may already be bound to something you must not touch.
Before starting a new container or pointing `MQK_DATABASE_URL` at one of these defaults, run
`docker ps` and check the `PORTS` column for what is *actually* listening, not just what a doc or
script assumes. If a default port is already taken by something other than a disposable proof/test
container, pick a free port explicitly (check with `docker ps` first — don't just reuse a port a
container on this machine has used before) rather than colliding — and double-check with
`docker exec <container> psql -U <user> -c "select 1"` that the container you think you are talking
to is the one actually answering on that port; a stale host-side port forward (observed once on this
repo, recreating a container on a host port it had previously used) can otherwise make a correct
password look like authentication failure from outside the container, even though the same password
works fine via `docker exec` or Docker's internal network. If that happens, recreating the same
container on a *different* host port is the fastest fix — cheaper than re-debugging credentials.

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

### Canonical local proof harness

`full_repo_proof.ps1` at repo root is the authoritative local proof runner.
It runs the required lanes in sequence and writes a structured summary to `.proof/full_repo_proof_output.txt`.

```powershell
# Non-DB local proof
.\full_repo_proof.ps1 -ProofProfile local

# Low-memory Windows posture
.\full_repo_proof.ps1 -ProofProfile local -LowMemory

# Full DB-backed institutional proof against the isolated manual proof DB
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

From repo root on Windows:

```powershell
& "C:\Program Files\Git\bin\bash.exe" -lc './scripts/db_proof_bootstrap.sh'
```

Or, to let the script start its own default `5432` proof DB container:

```powershell
& "C:\Program Files\Git\bin\bash.exe" -lc './scripts/db_proof_bootstrap.sh --start-postgres'
```

Or, to point it at the isolated manual proof DB on `55432`:

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

Deprecated wrappers such as `test-all.ps1`, `test-db.ps1`, and `ci_gate.ps1` should not be used for operator validation.
The canonical local proof entrypoint is `full_repo_proof.ps1`.

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

Valid mode + adapter combinations with `deployment_start_allowed: true` include:

- `paper` mode + `alpaca` adapter — canonical honest paper path
- `live-shadow` mode + `alpaca` adapter — typed support, no current capital authority
- `live-capital` mode + `alpaca` adapter — typed support with additional gates, not operationally authorized here

### Fail-closed combinations

- `paper` mode + `paper` adapter — refused; not a valid start-authoritative daemon combination
- `live-shadow` or `live-capital` with `paper` adapter — refused
- any unrecognized adapter ID — refused
- `backtest` deployment in daemon runtime — unconditionally refused

### Strongest current operational path

The strongest current daemon path is canonical **Paper + Alpaca**.

Its current source-grounded capabilities include:

- durable daily-operation state in Postgres
- canonical NYSE-session planning and persisted operation boundaries
- strict daily-data-readiness gating before runtime start
- strategy-promotion gating before paper order-producing logic
- WS continuity and reconcile truth gates
- bounded typed start/recovery/stop behavior
- completed-bar-driven production task instead of the legacy blind timer
- durable observation and exactly-once dispatch-claim infrastructure, dispatched through the same
  exact-input strategy-dispatch seam the execution loop uses (D4)
- existing readiness, preflight, events, and alert surfaces

The path is still in **pre-soak hardening** because Bundle 3 is open. Phase D (D1–D4) is accepted
complete in full; the Phase E1 contract audit (four-times-corrected) is **accepted complete**; Phase
E2A (durable coverage-anchor/run-lineage evidence foundation), plus its authority-envelope/gate-
ordering/concurrency closure repair, is implementation complete, awaiting acceptance. Do not label the
current `main` head as a finished autonomous-paper MVP until Phase E's
remaining runtime implementation (E2B–E5) and the later F/G phases are independently accepted.

### What is expected after Bundle 3

After Bundle 3 closes, the daemon should be able to remain running across supported NYSE sessions
and manage the daily Paper + Alpaca lifecycle without a person manually starting and stopping each
run.

The first deployment posture should still be:

```text
autonomous PAPER
+ operator watched
+ evidence captured
+ live capital disabled
```

Use the first sessions as a controlled soak. The project plan is roughly 10–20 clean sessions before
broader rollout. Bundle 4 then adds trustworthy durable paper cash, positions, lots, cost basis, and
daily realized/unrealized P&L across restarts.

### Completed-bar task configuration

Current D3 source adds:

```text
MQK_AUTONOMOUS_COMPLETED_BAR_TICK_SECS
```

Behavior:

- absent or blank: 15-second default
- allowed range: 1–300 seconds
- invalid, zero, negative, or out-of-range: task startup fails closed
- Tokio missed ticks are skipped rather than replayed in a burst

Provider network calls remain separately authorized by both:

```text
MQK_AUTONOMOUS_DATA_REFRESH_ENABLED=true
MQK_ALLOW_PROVIDER_API_CALLS=true
```

The completed-bar task may still use an exact trusted local bar when provider calls are disabled.
Do not enable provider calls merely to make startup succeed.

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
- canonical operator actions include persisted restart-intent workflow through `/api/v1/ops/action`
- `request-mode-change` can persist a restart intent when the transition is admissible-with-restart
- `cancel-mode-transition` can cancel a pending durable restart intent
- the action catalog exposes restart workflows instead of pretending hot mode changes are authoritative

## CLI entry point

The CLI binary is `mqk`.

From repo root:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- --help
```

## CLI common operations

### DB status and migrations

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- db status
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- db migrate
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- db migrate --yes
```

Authoritative migration source:

- `core-rs/crates/mqk-db/migrations/`

Any tracked SQL file under another `/migrations/` path is rejected by migration governance guards.

### Config hash

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- config-hash config/defaults/base.yaml config/environments/windows-dev.yaml config/engines/main.yaml
```

### Market data — CSV ingest

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- md ingest-csv --path "<PATH_TO_CSV>" --timeframe "1D" --source "csv"
```

### Market data — provider ingest

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- md ingest-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D" `
  --start "2000-01-01" `
  --end "2026-01-01"
```

### Market data — incremental sync

First run, when no bars exist yet:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D" `
  --full-start "2020-01-01"
```

Subsequent incremental runs:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- md sync-provider `
  --source "twelvedata" `
  --symbols "SPY,QQQ" `
  --timeframe "1D"
```

Override end date or overlap:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- md sync-provider `
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
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- backtest csv `
  --bars "<PATH_TO_BARS_CSV>" `
  --timeframe-secs 60 `
  --initial-cash-micros 100000000000 `
  --integrity-enabled true `
  --integrity-stale-threshold-ticks 120 `
  --integrity-gap-tolerance-bars 0
```

Cash fields are integer micros. For a $100,000 backtest, enter `100000000000`; `100000` means $0.10, not $100,000. Using `100000` can make otherwise valid AAPL orders reject for insufficient cash. This applies to the GUI backtest form as well as CLI `--initial-cash-micros`.

Optional artifact output:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- backtest csv `
  --bars "<PATH_TO_BARS_CSV>" `
  --out-dir "runs/backtests"
```

### Backtest from Postgres `md_bars`

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- backtest db `
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
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run start `
  --engine "MAIN" `
  --mode "PAPER" `
  --config "config/defaults/base.yaml" `
  --config "config/environments/windows-dev.yaml" `
  --config "config/engines/main.yaml"
```

### Arm

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run arm --run-id "<RUN_ID>"
```

### Begin

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run begin --run-id "<RUN_ID>"
```

### Heartbeat

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run heartbeat --run-id "<RUN_ID>"
```

### Stop

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run stop --run-id "<RUN_ID>"
```

### Halt

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run halt --run-id "<RUN_ID>" --reason "manual halt"
```

### Status

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run status --run-id "<RUN_ID>"
```

### Deadman check

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run deadman-check --run-id "<RUN_ID>" --ttl-seconds 60
```

### Deadman enforce

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run deadman-enforce --run-id "<RUN_ID>" --ttl-seconds 60
```

Other helpers exist:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli -- run --help
```

## Daemon

### Preferred local daemon launch

From repo root, with repo-root `.env.local` already configured:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon
```

Default local URL:

- `http://127.0.0.1:8899`

### Manual override example

If you prefer to launch from `core-rs/` instead, export env vars manually or keep a `core-rs/.env.local` copy.

```powershell
cd core-rs
$env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5432/mqk_dev"
$env:MQK_OPERATOR_TOKEN = "dev-local-operator-token"
$env:MQK_DAEMON_DEPLOYMENT_MODE = "paper"
$env:MQK_DAEMON_ADAPTER_ID = "alpaca"
$env:ALPACA_API_KEY_PAPER = "<your-paper-key>"
$env:ALPACA_API_SECRET_PAPER = "<your-paper-secret>"
cargo run -p mqk-daemon
```

Optional autonomous-operation variables:

```powershell
# Completed-bar worker cadence. Default 15; valid range 1-300 seconds.
$env:MQK_AUTONOMOUS_COMPLETED_BAR_TICK_SECS = "15"

# Only set both to true when real provider latest-bar calls are intentionally authorized.
$env:MQK_AUTONOMOUS_DATA_REFRESH_ENABLED = "true"
$env:MQK_ALLOW_PROVIDER_API_CALLS = "true"
```

Optional session override variables:

```powershell
$env:MQK_SESSION_START_HH_MM = "14:30"
$env:MQK_SESSION_STOP_HH_MM = "21:00"
```

Use session overrides only when you explicitly intend to replace the default NYSE regular-session
authority for a controlled test. Provider authorization is not required when the exact canonical
bar is already available locally.

### Useful daemon surfaces for the canonical paper path

- `GET /api/v1/system/status`
- `GET /api/v1/system/preflight`
- `GET /api/v1/autonomous/readiness`
- `GET /api/v1/alerts/active`
- `GET /api/v1/events/feed`
- `GET /api/v1/ops/catalog`
- `POST /api/v1/ops/action`

### Paper smoke review caveat

Current `scripts/windows/Review-PaperSmokeEvidence.ps1` derives `runtime_halted=true` by scanning captured `events_feed.json` rows for any `runtime_transition/HALTED` event. That check is not filtered by the current `run_id` or evidence window, so older HALTED events in the captured feed can set the flag. Treat `runtime_halted=true` as a review caveat and verify run_id/window context before using it as the current smoke verdict.

## GUI

Run from `core-rs/mqk-gui/`:

```powershell
npm ci
npm run build
npm run dev
```

Default dev URL:

- `http://127.0.0.1:1420`

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
- the launcher imports local env hints from repo-root and `core-rs` env files when present

Treat it as an operator convenience path that still requires local Windows validation on your machine.
The browser GUI + daemon path remains the primary documented workflow.

## One-shot local launch (two shells)

### Shell 1 — daemon

From repo root:

```powershell
cd C:\Users\<YOU>\Desktop\MiniQuantDeskV4
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-daemon
```

### Shell 2 — GUI

```powershell
cd C:\Users\<YOU>\Desktop\MiniQuantDeskV4\core-rs\mqk-gui
npm ci
npm run dev
```

If you use `Start-Process`, keep the DB URL assignment quoted correctly inside the spawned command.

## Autonomous paper reality test

The repo includes a committed PowerShell reality-test harness at repo root:

- `autonomous_reality_test_paper.ps1.ps1`

Its job is different from normal proof or normal operator startup.
It unpacks a snapshot, provisions its own Docker Postgres container, launches the daemon, checks readiness, optionally injects a crash, and validates recovery behavior.

Default reality-test DB settings in the committed script:

- container: `mqk-reality-postgres`
- host port: `5440`
- DB name: `mqk_v4`

The script also looks for `.env.local` under both repo root and `core-rs/`.

Treat this as a dedicated reality-test lane, not your everyday operator startup path.

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
  - workspace dependency inheritance guard

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

- Bundle 3 is not closed; Phase D (D1–D4, integrated lifecycle proof, dispatch-ownership race closure, and the evaluation-lineage repair) is accepted complete in full; the Phase E1 contract audit (the binding durable outcome/no-trade contract, four-times-corrected) is **accepted complete**; Phase E2A (durable coverage-anchor/run-lineage evidence foundation), plus its authority-envelope/gate-ordering/concurrency closure repair, is implementation complete but awaiting independent ChatGPT/operator acceptance, and no outcome classifier or finalization behavior exists yet
- the current main branch should not begin an unattended soak until Phase E's remaining runtime implementation (E2B strict classifier/finalization, then E3–E5, per the accepted E1 contract and built on E2A's authorities) and the later Bundle 3 phases (F/G) are accepted; controlled, operator-supervised autonomous Paper + Alpaca operation is the current Bundle 3 target, not unattended soak
- Bundle 4 durable paper cash/positions/lots/cost basis/P&L truth is still open
- real paper fill, reconcile-after-fill, Discord lifecycle, restart, and repeated-session evidence remain incomplete
- the daemon/operator plane is materially stronger, but some deeper GUI detail surfaces remain intentionally deferred or unmounted rather than faked
- the daemon has typed support for paper, live-shadow, and live-capital on Alpaca, but typed support is not the same thing as safe live operation
- the backtest system is strong, but still being hardened toward promotion-grade provenance and lifecycle realism
- shadow/live parity evidence is not yet strong enough for a safe unattended live claim
- scenario-tested does **not** mean profitable, broker-proof, or safe for live capital

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
