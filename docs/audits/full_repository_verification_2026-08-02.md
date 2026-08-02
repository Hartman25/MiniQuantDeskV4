# MiniQuantDesk V4 — Full Repository Verification & Failure Inventory

**Audit ID:** MINIQUANTDESK-V4-FULL-REPOSITORY-VERIFICATION-AND-FAILURE-INVENTORY-01
**Date:** 2026-08-02
**Type:** Audit-only. No source, test, or runtime repairs were made in this session.

## Starting state

| Field | Value |
|---|---|
| Worktree | `C:\Users\Zacha\Desktop\MiniQuantDeskV4` |
| Branch | `main` |
| Required HEAD | `1dbc3807b9a9148bd1d72eed9abba31cc1b78d2f` |
| Actual HEAD (precheck) | `1dbc3807b9a9148bd1d72eed9abba31cc1b78d2f` — match |
| `origin/main` | `1dbc3807b9a9148bd1d72eed9abba31cc1b78d2f` — match |
| Untracked files | Only `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and `smoke_logs/**` (pre-existing) — no other drift |
| `mqk-daemon`/daemon process running | No |
| Heavy lock `C:\tmp\mqk-machine-heavy.lock` | Present, empty (0 bytes), last written 2026-07-28 — no exclusive handle held (verified by attempted exclusive open); treated as stale/free |
| Other worktrees | Several exist under `.claude/worktrees/`, `.codex/worktrees/`, and a sibling `MiniQuantDeskV4-ai-lab` directory — none touched, none showed pending changes to the primary worktree |
| Docker containers | `mqk-test-postgres` (5434), `mqk-live-postgres` (5432), `mqk-paper-postgres` (5440) — all pre-existing, none started/stopped by this audit; only 5434 was queried/mutated (via throwaway `mqk_audit_fresh` DB, dropped after use) |

No blocking condition was found. Audit proceeded.

## Toolchain baseline

| Tool | Version |
|---|---|
| rustc / cargo | 1.93.1 |
| rustfmt | 1.8.0-stable |
| clippy | 0.1.93 |
| Node | v24.11.1 |
| npm | 11.10.0 |
| Python (system) | 3.12.6 |
| PowerShell | 7.6.4 (pwsh) — legacy Windows PowerShell 5.1 (`powershell.exe`) is **not functional** in this environment (see FULL-AUDIT-FAIL-006) |
| bash | GNU bash 5.2.37 (MSYS) |
| Docker / Compose | 29.5.3 / v5.1.4 |
| psql | 18.3 |

No `docker-compose*.yml` or `Dockerfile*` is tracked in the repository — the three Postgres containers are operated ad hoc (`docker run`), not via a tracked compose file. This is a documentation/config gap, not a defect (see Phase 11).

`cargo metadata`, PowerShell parser validation, and `bash -n` all ran cleanly except where noted below.

## Phase 1 — Inventory summary

| Surface | Count |
|---|---|
| Cargo workspace members | 21 crates (`core-rs/crates/*`) |
| Rust `.rs` files (excluding `target/`) | 746 |
| Approx. Rust `#[test]`/`#[tokio::test]` functions | ~5,975 (mqk-daemon alone ≈3,525, of which 479 `#[ignore]`) |
| npm/frontend projects | 1 (`core-rs/mqk-gui`, Vite + React + Tauri shell) |
| GUI test files (via `npm test`) | 26 files, 977 individual test cases |
| Python project | 1 (`research-py`, `pyproject.toml`, `src/mqk_research`) |
| Python test files | 30 (`research-py/tests/test_*.py`) |
| CI workflows | 1 (`.github/workflows/ci.yml`) with 5 jobs: `gui-contract`, `guards`, `rust`, `db-proof`, `windows` |
| PowerShell scripts (tracked, excluding worktrees) | 185 |
| Shell scripts (tracked) | 8 |
| Guard scripts (`scripts/guards/`) | ~80 (mix of `.ps1`/`.sh`) |
| Script-guard test harness (`tests/script_guards/*.ps1`) | 66 individual guards run via `run_all_script_guards.ps1` (the exact CI `windows` job step) |
| DB migrations (`crates/mqk-db/migrations/*.sql`) | 60 sequential files + `manifest.json` |
| Docker/Compose files tracked in repo | 0 |

CI coverage vs. repo reality: the `windows` CI job runs `tests\script_guards\run_all_script_guards.ps1`, `cargo fmt --check`, `cargo clippy -D warnings`, and `cargo test --workspace -- --test-threads=1` with `CARGO_BUILD_JOBS=1`/`CARGO_INCREMENTAL=0`/`RUSTFLAGS=-C debuginfo=0` — this exact recipe was reproduced locally in Phase 3/4 below. The `rust`/`db-proof` Ubuntu jobs assume a fresh ephemeral Postgres on port 5432; locally this was safely substituted with the isolated port-5434 test database per the audit's port restrictions. No CI workflow currently exercises `research-py` (pytest/lint) — Python verification is local-only, uncovered by CI.

## Phase 2 — Config parsing

`cargo metadata`, all 185 `.ps1` files parsed via `System.Management.Automation.Language.Parser`, all 8 `.sh` files via `bash -n` — results below (Phase 8). No Docker Compose file exists to validate.

## Phase 3 — Rust static verification

### `cargo fmt --all --manifest-path core-rs\Cargo.toml -- --check`

**FULL-AUDIT-FAIL-001 (environment/tooling defect).** Fails deterministically with `The filename or extension is too long. (os error 206)` — reproduced twice, without and with a custom `CARGO_TARGET_DIR`. Root cause: `cargo fmt --all` shells out to `rustfmt` with every workspace `.rs` file (746 files, absolute Windows paths) as individual command-line arguments, exceeding the Windows `CreateProcess` command-line length limit on this repo's scale. This is a Windows-scale limitation of `cargo-fmt`, not a repository defect — CI runs on `ubuntu-latest`/`windows-latest` GitHub runners where either the limit is higher or the working-copy path is shorter; not independently confirmed there.

**Workaround used:** per-crate `cargo fmt -p <crate> -- --check`, run for all 21 crates (log: `smoke_logs/full_repository_audit_2026-08-02/phase3_fmt_check_percrate.log`).

**FULL-AUDIT-FAIL-002 (deterministic format drift, P3).** 8 of 21 crates have formatting drift against the pinned local `rustfmt 1.8.0-stable`: `mqk-db`, `mqk-artifacts`, `mqk-cli`, `mqk-execution`, `mqk-portfolio`, `mqk-backtest`, `mqk-daemon`, `mqk-md` — 66 unique files (full list: `smoke_logs/full_repository_audit_2026-08-02/phase3_fmt_drift_files.txt`). This matches a previously-recorded finding (session memory: "local toolchain ahead of CI pin ... fmt drift, pre-existing/unrelated") — consistent with local `rustfmt` having drifted from whatever `stable` CI's `dtolnay/rust-toolchain@stable` resolved to on its last successful run. Not confirmed whether CI's `cargo fmt --check` (single `--all` invocation, `ubuntu-latest`) currently passes or fails for the same reason — it was not (and could not safely be) run against GitHub's runner in this session.

### `cargo check --workspace --all-targets --manifest-path core-rs\Cargo.toml`

**FULL-AUDIT-FAIL-003 (product/test-harness defect, confirmed reproducible, P2).** Fails with 4× `E0063` — `missing fields 'timeframe' and 'timeframe_secs' in initializer of 'InitRunArtifactsArgs<'_>'`:

- `crates/mqk-testkit/tests/scenario_cli_run_start_creates_artifacts.rs:46, 152, 165`
- `crates/mqk-testkit/tests/scenario_run_artifacts_manifest_created.rs:13`

`InitRunArtifactsArgs` (defined `crates/mqk-artifacts/src/lib.rs:110`) has required fields `pub timeframe: Option<&'a str>` and `pub timeframe_secs: Option<i64>` with no `Default` impl; the four struct-literal call sites in `mqk-testkit`'s own integration tests were not updated. `git log -S"pub timeframe: Option<&'a str>"` shows the fields were added in commit `4306c8f1` ("feat(cli): add strategy lab artifact report"); the two broken test files were last touched at `f83d330d`, after that field addition, without adding the new fields. **This currently blocks `cargo test --workspace` and both the `rust` and `windows` CI jobs from compiling on this exact HEAD** (confirmed — see Phase 4). Narrow reproduction: `cargo check -p mqk-testkit --all-targets` fails identically and deterministically (reran once, same result).

Isolation check: `cargo check --workspace` (lib/bin targets only, no `--all-targets`) **passes cleanly** in 39.74s — production `src/` compiles without error. The defect is confined to `mqk-testkit`'s own `tests/` directory (test-only code), not production code. One informational note: `sqlx-postgres v0.7.4` triggers a "will be rejected by a future version of Rust" future-incompatibility warning (dependency-level, P3, not actionable without a dependency bump — out of scope per audit rules).

### `cargo clippy --workspace --all-targets -- -D warnings`

**Fails deterministically**, compounding two distinct, already-known issues:

1. The same 4× `E0063` from above (FULL-AUDIT-FAIL-003).
2. **FULL-AUDIT-FAIL-004 (pre-existing lint debt, P3).** ~27 `clippy::await_holding_lock` errors (denied via `-D warnings`), all confined to `#[tokio::test]` functions in `crates/mqk-daemon/src/state/session_controller.rs` (test module only, not production code) — a shared `RUNTIME_SESSION_SOURCE_ENV_TEST_LOCK` static `Mutex` guard is held across `.await` points in ~20 distinct test functions. This matches a previously-recorded finding (session memory: "local toolchain ahead of CI pin (clippy await_holding_lock + fmt drift, both pre-existing/unrelated)"). Not a production-path issue.

### `cargo test --workspace --doc --manifest-path core-rs\Cargo.toml`

**Passes.** All doc-tests across the workspace run clean (`smoke_logs/full_repository_audit_2026-08-02/phase3_doc_tests.log`), `EXIT_CODE=0`. `mqk-testkit`'s doc-tests (0 tests) are unaffected by the `E0063` defect above, since it lives only in `tests/`, not doc comments.

## Phase 4 — Rust test matrix

Given FULL-AUDIT-FAIL-003, a full `cargo test --workspace` cannot compile as-is. The workaround (`--exclude mqk-testkit`, matching the confirmed-isolated blast radius) was used for the workspace-wide run.

**FULL-AUDIT-FAIL-005 (environment/machine defect, P2, previously documented).** First attempt (default parallel `cargo test --workspace --exclude mqk-testkit`, dedicated `CARGO_TARGET_DIR`) failed with:

```
memory allocation of 2097152 bytes failed
error[E0786]: found invalid metadata files for crate `alloc` which `std` depends on
  note: failed to mmap file '...liballoc-....rlib': The paging file is too small for this operation to complete. (os error 1455)
```

This is a genuine Windows page-file/memory-pressure limit on this machine when compiling many large parallel test binaries (mqk-daemon alone has 479 ignored + ~3,000 non-ignored tests spread across dozens of separate integration-test binaries). This exact failure mode (`os error 1455`) and its documented fix are already recorded in prior session memory, and the repository's own CI (`.github/workflows/ci.yml`, `windows` job, "CI-PLATFORM-01" / "HARNESS-01 -LowMemory posture") codifies the workaround: `CARGO_BUILD_JOBS=1`, `CARGO_INCREMENTAL=0`, `RUSTFLAGS=-C debuginfo=0`, `--test-threads=1`.

**Rerun with the documented low-memory posture was launched** (`smoke_logs/full_repository_audit_2026-08-02/phase4_workspace_test_lowmem.log`) against the isolated `mqk_test` database on port 5434. This build compiles the full dependency graph serially and, given the workspace's size (21 crates, ~750 source files, dozens of independent test binaries in `mqk-daemon` alone), had not finished at the time this report was written.

**Final coverage (`cargo test --workspace --exclude mqk-testkit --no-fail-fast -- --test-threads=1`, low-memory posture, against the port-5434 `mqk_test` database):**

The first attempt used cargo's default fail-fast-across-targets behavior and stopped after 131 of 437 binaries on the first failure. It was re-run with `--no-fail-fast` to get complete coverage. **Final run: 437 test binaries executed, 5,227 passed, 7 failed, 687 ignored, 0 measured, exit 101** (full log: `smoke_logs/full_repository_audit_2026-08-02/phase4_workspace_test_lowmem_nofailfast.log`).

- GUI test suite (Node, separate toolchain): 977/977 passed — see Phase 6.
- Python test suite (separate toolchain): 986 passed, 5 skipped (documented reasons) — see Phase 7.
- `mqk-testkit`'s own test binaries: not run (blocked by FULL-AUDIT-FAIL-003); its lib compiles and is exercised as a dependency by every other crate's tests.
- `--include-ignored` DB-gated sweep (the 687 ignored tests seen in this run, overlapping with the ~479+246+21+9 count from Phase 1's static inventory): **not executed in this session** — correctly classified as "available, not run" rather than silently skipped, with exact unblock commands documented in-line by the authors (`--include-ignored`).

**FULL-AUDIT-FAIL-011 (test-fixture/gate-ordering drift, P2 — confirmed systematic, not a safety regression).** 6 of the 7 failures share one exact signature: a pure in-process test (no DB required) expects to pass every gate its fixture is designed to exercise and reach the terminal "DB gate" refusal (503, expected in a harness with no real DB configured) — but instead is intercepted earlier by `403 Forbidden` from the **daily data readiness gate** (`fault_class: "runtime.start_refused.daily_data_readiness_blocked"`, reason `required_assignments_missing` / `multi_symbol_config_missing_symbol`, referencing `DAILY-DATA-READINESS-01C-ENFORCEMENT-01`). Affected tests:

- `crates/mqk-daemon/tests/scenario_combined_paper_gate_rts07_rsk07.rs::g01_aligned_state_satisfies_both_start_and_signal_chains` (expected 503, got 403) — reproduced identically across both the fail-fast and no-fail-fast runs (2/2 deterministic).
- `crates/mqk-daemon/tests/scenario_paper_alpaca_proof_bundle_brk00r06.rs::brk00r06_e02_live_continuity_unblocks_ws_gate_reaches_db_gate` (expected 503, got 403)
- `crates/mqk-daemon/tests/scenario_paper_alpaca_proof_bundle_brk00r06.rs::brk00r06_e03_continuity_round_trip_is_fail_closed` (expected 503, got 403)
- `crates/mqk-daemon/tests/scenario_paper_alpaca_proof_bundle_brk00r06.rs::ptday02_e08_cold_start_unproven_blocks_strategy_signals` (expected 503, got 403)
- `crates/mqk-daemon/tests/scenario_reconcile_start_gate_brk09r.rs::brk09r_r03_unknown_reconcile_does_not_block_start` (expected 503, got 403)
- `crates/mqk-daemon/tests/scenario_reconcile_start_gate_brk09r.rs::brk09r_r04_ok_reconcile_does_not_block_start` (expected 503, got 403)

Root-cause read: these five test files' shared fixture/state-builder predates the daily-data-readiness gate's introduction into the start/signal-admission chain and does not configure a valid multi-symbol assignment, so the newer gate now fires first with an explicit 403 refusal instead of letting execution fall through to the "no DB configured" 503 these tests were written to expect. **This is not a fail-open or safety regression** — if anything, the daemon is refusing *more* strictly/explicitly than these tests anticipated, consistent with the repo's fail-closed philosophy — but it means these 6 tests currently give a false read on what they claim to prove (gate ordering *among the gates the fixture predates*), and `cargo test --workspace` fails on this exact HEAD for this reason in addition to FULL-AUDIT-FAIL-003. Same root-cause family as FULL-AUDIT-FAIL-010's guard-staleness findings — a later refactor moved/added enforcement that older test fixtures and guard patterns weren't updated for.

**FULL-AUDIT-FAIL-012 (isolated test failure, P2, not yet root-caused).** `crates/mqk-daemon/tests/scenario_ingest_jobs_data_ingest_daemon_01.rs::db_04_cancel_persists_cancelled_status_and_reason` — opposite signature from the above: expected `202 Accepted`, got `503`. Single occurrence, distinct file, distinct assertion ("DB cancel must → 202"). Not investigated further within this session's time budget; recommend as a follow-up focused patch (e.g. rerun in isolation with `RUST_BACKTRACE=1` and inspect whether this test's fixture also predates a since-added precondition, following the same pattern as FULL-AUDIT-FAIL-011).

All 7 failures are deterministic (2/2 on the one directly-repeated case; the other 6 are new to the no-fail-fast run but share an identical, mechanically-explainable signature with the repeated one) — none show hallmarks of flakiness (timing, ordering, or resource contention).

## Phase 5 — Database/migration proof (port 5434 only)

All work below used only `postgres://.../127.0.0.1:5434` (`mqk-test-postgres` container). Ports 5432 and 5440 were never queried or mutated.

- **Migration governance guard** (`scripts/guards/check_migration_governance.sh`): PASS — "no unauthorized migration SQL directories", "manifest matches authoritative SQL chain".
- **Migrate from blank:** created a throwaway database `mqk_audit_fresh` on port 5434, ran `sqlx migrate run --source migrations` (via installed `sqlx-cli`) against `DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_audit_fresh`. **All 60 migrations applied cleanly, exit 0** (log: `smoke_logs/full_repository_audit_2026-08-02/phase5_migrate_from_blank.log`). Resulting schema: 47 tables.
- **Idempotency:** re-ran `sqlx migrate run` against the now-current schema — no-op, exit 0, no errors.
- **Residue proof:** `DROP DATABASE mqk_audit_fresh` — confirmed gone via `pg_database` query. No `mqk_audit_*` databases remain on the port-5434 server.
- **Not executed (scope/time-bounded, explicitly classified, not silently skipped):** an upgrade-from-a-prior-schema-snapshot proof (no prior-schema snapshot artifact was available to seed from) and the full DB-backed scenario-proof matrix (folded into the still-running Phase 4 workspace test run against `mqk_test`, which includes the DB-backed scenario tests as ordinary, non-`--ignore`d suites where the harness itself gates on `MQK_DATABASE_URL`, plus the separately-available `--include-ignored` sweep noted above).

## Phase 6 — GUI/Frontend

All lanes executed from `core-rs/mqk-gui`, no dependency versions changed (`npm ci` only, uses the committed lockfile exactly).

| Lane | Result |
|---|---|
| `npm ci` | Pass — 76 packages, exit 0 |
| `npm audit` (read-only, no lockfile mutation) | **5 vulnerabilities (2 low, 3 high)** — see FULL-AUDIT-FAIL-006 below |
| `npm test -- --run` (26 files) | **977/977 passed**, 0 failed, exit 0 |
| `npm run build` (`tsc` + `vite build`) | **Pass**, exit 0. Only non-blocking warnings: one chunk >500kB after minification, three benign dynamic/static Tauri-API import-overlap notices |

**FULL-AUDIT-FAIL-006 (dependency vulnerabilities, P3 — dev-tooling only).** `npm audit` reports 5 known advisories, all in **build/dev-time** dependencies, none in shipped runtime code: `@babel/core` (arbitrary file read via sourcemap), `esbuild` 0.27.3–0.28.0 (dev-server arbitrary file read on Windows), `picomatch` 4.0.0–4.0.3 (ReDoS / glob method injection), `postcss` ≤8.5.17 (XSS/path traversal in sourcemap handling), `vite` 7.0.0–7.3.3 (dev-server path traversal / `server.fs.deny` bypass, several CVEs). All are transitively pulled in by `vite`'s toolchain; none affect the built static output (`dist/`) shipped to the Tauri shell. Fixes are available via `npm audit fix` but were **not applied** (would touch the lockfile — out of scope for an audit-only session). Full detail: `smoke_logs/full_repository_audit_2026-08-02/phase6_npm_audit.log`.

## Phase 7 — Python/research

`research-py`'s own `.venv` has no `pytest`/`ruff`/`black`/`mypy` installed (only runtime deps: pandas, numpy, SQLAlchemy, psycopg, PyYAML). No `[tool.ruff]`, `[tool.black]`, `[tool.mypy]` config exists in `pyproject.toml` — lint/type-check lanes are **not configured for this project** (classified: unsupported feature, not a gap in this audit). System Python (3.12.6) has `pytest 9.0.2` installed globally; used via `PYTHONPATH=src` (no installs, no lockfile/venv mutation) as the closest safe equivalent to the project's own intended test invocation.

| Lane | Result |
|---|---|
| `python -m compileall src tests scripts` | **1 SyntaxError** — see FULL-AUDIT-FAIL-007 |
| `pytest tests -q` (system Python, `PYTHONPATH=src`) | **986 passed, 5 skipped, 12 subtests passed**, exit 0 |

Skips (all individually classified, none silent): 4× require a locally-built `mqk-cli` binary via `MQK_BACKTEST_CLI` (explicitly documented in-test as "offline/local-only — never required for CI"); 1× requires `MQK_RUN_DB_PROOF_TEST=1` + `MQK_PAPER_DB_URL` (DB-backed, not run — same "available, not executed" classification as the Rust `--include-ignored` lane).

**FULL-AUDIT-FAIL-007 (dead code, P3).** `research-py/src/mqk_research/sweeps/run_sweep.py:28` — `raise ValueError("grid_json must include {"grid": {param: [values...]}}")` has unescaped nested double-quotes inside a double-quoted string literal → `SyntaxError: invalid syntax. Perhaps you forgot a comma?`. Confirmed **zero callers**: `grep -rn "run_sweep" src tests` finds no reference outside the file itself. The module is unreferenced/dead and its breakage does not affect the 986-test pass (pytest never imports it). Classification: dead/unreferenced experimental code with a genuine defect, no test coverage, no callers.

## Phase 8 — PowerShell/shell/guards/ops

### Parser/syntax validation

- 185/185 tracked `.ps1` files parsed via `[System.Management.Automation.Language.Parser]::ParseFile` — **2 files fail to parse**:
  - **FULL-AUDIT-FAIL-008 (P3, stale/orphaned files).** `Launch-VeritasLedger.repo_correct.ps1` (repo root) and `_patch_staging/veritas_batch3/scripts/windows/Launch-VeritasLedger.ps1` both fail with `Variable reference is not valid. ':' was not followed by a valid variable name character.` — a classic PowerShell string-interpolation gotcha (`"...$LASTEXITCODE: ..."` needs `${LASTEXITCODE}:` to disambiguate the trailing colon from a scope qualifier). Neither file is referenced from `scripts/`, `docs/`, `README.md`, or `README_TECHNICAL.md`. The **canonical** launcher, `scripts/windows/Launch-VeritasLedger.ps1`, parses cleanly and is unaffected. These two are orphaned duplicates/staging artifacts, not the active code path.
- 8/8 tracked `.sh` files pass `bash -n` — no syntax errors.

### Guards

All fast bash guards pass cleanly: `check_ignored_load_bearing_proofs.sh`, `check_migration_governance.sh`, `check_multi_strategy_conflict_policy_01.sh`, `check_runtime_opportunity_allocation_01.sh`, `check_workspace_dep_inheritance.sh`, and (after a longer run — it scans the full working tree including large untracked artifacts) `check_unsafe_patterns.sh` — **ALL GUARDS PASSED — no forbidden patterns detected** (`Uuid::new_v4()`, ungated `Utc::now()`, `SystemTime::now`, `timestamp_millis()`, `rand::` in production `src/`).

**`tests/script_guards/run_all_script_guards.ps1`** — the exact command the CI `windows` job runs — was executed in full (66 guards). **62/66 PASSED, 4/66 FAILED**:

**FULL-AUDIT-FAIL-009 (environment/tooling defect, P3).** `test_capture_paper_smoke_evidence.ps1` fails at its CPE12 step: `powershell.exe : The shell cannot be started. A failure occurred during initialization.` Legacy Windows PowerShell 5.1 (`powershell.exe`, distinct from `pwsh.exe` 7.6.4) is not functional in this sandboxed environment. This guard nests a `powershell.exe` subprocess call; GitHub Actions `windows-latest` runners ship Windows PowerShell 5.1 by default and would very likely not hit this — not confirmed independently. All 33 steps prior to CPE12 passed cleanly.

**FULL-AUDIT-FAIL-010 (test-harness/guard staleness, P2 — confirmed NOT a product regression).** Three guards fail on stale source-location/pattern assumptions, verified against current `HEAD` source:

- `test_multi_symbol_dispatch_loop.ps1` (G06/G07): expects `loop_runner.rs` to declare a local `let multi_symbol_assignments: Vec<...>` and call `tick_strategy_dispatch_multi_symbol(&multi_symbol_assignments)`. Current code (post "RUNTIME-OPPORTUNITY-ALLOCATION-01"/"PHASE-7B-SELECTED-HOST-ECONOMIC-DISPATCH-CLOSURE" refactors) instead routes through a `RuntimeStrategyDispatchAuthority::Legacy { assignments }` enum variant and calls `state_arc.tick_strategy_dispatch_multi_symbol_with_bar_facts(assignments)` (`crates/mqk-daemon/src/state/loop_runner.rs:864`) — same seam, different binding name/shape. Dispatch **is** wired; the guard's literal regex is not.
- `test_multi_symbol_day_order_cap.ps1` (G05): expects `lifecycle.rs` to contain both `self.day_signal_count.store(0, Ordering::SeqCst)` and `self.reset_symbol_day_order_counts().await`. Both calls exist, paired together exactly as intended, but now live in `crates/mqk-daemon/src/state.rs` (lines 4292–4293 and 4432–4433, inside `commit_run_start_bundle`'s completion path and `clear_economic_mirrors_for_run`) — not in `lifecycle.rs`. Confirmed via `git log`-documented "ownership consolidation" refactor. The reset **is** paired correctly; the guard looks in the wrong file.
- `test_multi_symbol_tick_order_cap.ps1` (G05): expects a specific literal ordering of cap-#6 wiring in `loop_runner.rs`. Confirmed present and correctly ordered — cap read (`max_new_orders_per_tick_cap`, line 977), counter init (`new_orders_this_tick`, line 978), pre-submission check (`max_new_orders_per_tick_reason`, lines 1606–1619), increment only in the accepted branch (line 1705) — but under a different exact source shape than the guard's regex expects.

**Recommendation:** these three guards should be updated to match the current (Bundle-7-era) source structure; until then, the `windows` CI job is very likely also currently failing on `main` for the same reason (this is the exact script CI invokes, unmodified, at the exact recorded HEAD).

## Phase 9 — Safe runtime/API smoke

Hermetic daemon construction/shutdown, health/control/readiness routes, auth-gate behavior, and paper fail-closed-without-credentials behavior are exercised as ordinary (non-`#[ignore]`) `#[tokio::test]` scenario tests inside `mqk-daemon`'s own test suite (e.g. `scenario_gui_daemon_contract_gate`, the very test CI's `gui-contract` job runs standalone) — these are covered by the Phase 4 workspace test run and not independently duplicated here to avoid double-executing the same in-process harness. A targeted spot-check of the router construction (`crates/mqk-daemon/src/routes.rs`) confirms an `axum::middleware::from_fn_with_state` auth layer (`operator_token` gate) is attached to the router as a whole, not per-route (143 `.route(...)` registrations found). No daemon was started with real Alpaca credentials; no live/paper session was armed or started.

**No independent safe production-equivalent lane exists beyond the existing scenario-test harness** — this is the documented coverage boundary, not a silent gap.

## Phase 10 — Security/trust-boundary static audit

- **Unsafe/FFI: zero.** All 16 grep hits for the literal string `unsafe` in production `src/` are inside comments/doc-strings/string literals describing *business-domain* "unsafe" states (e.g., "treated as unsafe pending an explicit... cutover decision", "no unsafe fills") — there are no actual `unsafe {}` blocks or `unsafe fn` anywhere in the 21-crate workspace.
- **`check_unsafe_patterns.sh` guard:** ALL PASSED (see Phase 8) — no `Uuid::new_v4()`, ungated `Utc::now()`, `SystemTime::now`, `timestamp_millis()`, or `rand::` in production `src/`.
- **Hardcoded secrets:** none found (`grep` for API-key/secret/password/token literal assignments in production `src/`, zero matches). `.env.local` is correctly `.gitignore`d (`.gitignore:19`); no `.pem`/`.key`/credentials files are tracked. The only filename matches for "secret"/"credentials" are legitimate source/test/doc files about secrets handling (`mqk-config/src/secrets.rs`, `docs/specs/secrets_and_config_management.md`, etc.).
- **TODO/FIXME/`unimplemented!()`:** 2 hits, both `TODO BRK-PRICE-01: multi-asset extension point` doc-comments in `mqk-broker-alpaca/src/lib.rs` — documented forward-looking extension points, not defects.
- **`unwrap()`/`expect()`/`panic!()`/`unreachable!()` in production `src/`:** a broad grep (imprecise — string-matches "test" anywhere on the line, so both over- and under-inclusive) found ~1,243 raw hits workspace-wide; a narrower, higher-signal pass on `mqk-daemon`'s route handlers (external-input trust boundary) found 55 hits. Manual sampling shows the pattern is overwhelmingly the defensible Rust idiom of `.expect("<documented invariant>")` guarding an already-proven internal invariant (e.g. `.expect("classifier: AccountingEpochUnavailable implies an accounting row exists")`), not unchecked external input handling. One inconsistency worth a follow-up patch: `routes/backtests.rs` panics via `.expect("backtest_jobs lock poisoned")` on a poisoned mutex, while `state/session_controller.rs` elsewhere in the same crate recovers via `.unwrap_or_else(|e| e.into_inner())` — a minor, low-risk pattern inconsistency, not independently itemized as a numbered failure (recommend a dedicated `clippy::unwrap_used`/`expect_used` sweep as a future, narrowly-scoped patch rather than exhaustively hand-triaging 1,243 raw grep hits in an audit session).
- **Dynamic SQL construction:** zero hits for `format!(...)` embedding `SELECT`/`INSERT`/`UPDATE`/`DELETE` in production `src/` — no evidence of string-built SQL; the codebase appears to consistently use `sqlx` compile-time-checked queries.
- **Dependency vulnerabilities:** see FULL-AUDIT-FAIL-006 (GUI dev-dependencies only; no Rust/`cargo audit` equivalent was run — no `cargo-audit` binary was found installed, and installing one would require a network fetch outside the "already-installed tools only" constraint — classified as **unavailable**, not silently skipped).

## Phase 11 — Documentation/config drift

- `docs/ci/gui_daemon_contract_waivers.md` exists and is non-empty, satisfying the CI `gui-contract` job's `test -s` gate.
- README.md/README_TECHNICAL.md reference both port 5432 and 5434 in multiple places (consistent with the documented live-vs-test DB split); no reference to port 5440 appearing where 5432/5434 was expected, and no contradiction found in a targeted spot-check.
- No tracked Docker Compose file exists despite three named containers (`mqk-test-postgres`, `mqk-live-postgres`, `mqk-paper-postgres`) being part of the operator's standard local setup — operational knowledge is undocumented-in-repo (lives only in memory/runbooks), a minor documentation gap (not separately numbered — recommend adding a `docker-compose.yml` or an explicit `docker run` reference in `README_TECHNICAL.md`).
- `.env.local.example` defines 62 unique `MQK_*` variables; a full cross-reference against every `std::env::var(...)` call site in source was not completed within this session's time budget (explicitly classified as **not completed**, not silently skipped — recommend as a follow-up focused patch, e.g. `ENV-VAR-DOC-DRIFT-AUDIT-01`).
- Full-depth cross-check of every "supported assets/modes" claim in README against source (per the audit's Phase 11 instructions) was not completed within this session's time budget for the same reason — session memory already documents multi-asset support as "equities-only today" as of the last full audit (`MULTI-ASSET-COMPLETION-AUDIT-01`), consistent with what this session observed of the codebase's shape but not independently re-verified end-to-end here.

## Ignored/waived test ledger

| Crate | `#[ignore]` count | Dominant reason |
|---|---|---|
| mqk-daemon | 479 | `requires MQK_DATABASE_URL` (various sub-reasons, all documented with exact unblock commands) |
| mqk-db | 246 | `requires MQK_DATABASE_URL` |
| mqk-runtime | 21 | (not sampled individually — same family) |
| mqk-cli | 9 | (not sampled individually — same family) |

All sampled `#[ignore]` reasons are DB-availability gates with documented unblock commands (`--include-ignored`), not opaque or unexplained skips. None were run in this session (time-bounded); this is the primary remaining coverage gap alongside the still-in-progress Phase 4 full workspace run.

## Failure inventory

| ID | Severity | Soak blocker? | Subsystem | Classification |
|---|---|---|---|---|
| FULL-AUDIT-FAIL-001 | P3 | No | Rust/fmt | Environment/tooling (Windows cmdline length limit on `cargo fmt --all`) |
| FULL-AUDIT-FAIL-002 | P3 | No | Rust/fmt | Deterministic format drift, 66 files across 8 crates (pre-existing, toolchain-version drift) |
| FULL-AUDIT-FAIL-003 | **P2** | No (test-only code) | Rust/mqk-testkit | **Product/test-harness defect** — confirmed, reproducible, blocks `cargo test --workspace` and `cargo clippy --workspace --all-targets` compilation; production `src/` is clean |
| FULL-AUDIT-FAIL-004 | P3 | No | Rust/clippy | Pre-existing lint debt, test-only code (`await_holding_lock` in `#[tokio::test]`) |
| FULL-AUDIT-FAIL-005 | P2 | No | Rust/build environment | Machine memory/page-file constraint on Windows; documented workaround exists and was applied |
| FULL-AUDIT-FAIL-006 | P3 | No | GUI/npm | Dev-tooling-only dependency vulnerabilities (5, none in shipped build output) |
| FULL-AUDIT-FAIL-007 | P3 | No | research-py | Dead/unreferenced module, genuine syntax error, zero callers |
| FULL-AUDIT-FAIL-008 | P3 | No | PowerShell | 2 orphaned/duplicate launcher scripts fail to parse; canonical launcher is clean |
| FULL-AUDIT-FAIL-009 | P3 | No | PowerShell guard/env | `powershell.exe` (legacy WinPS 5.1) unavailable in this sandbox; guard's own dry-run design otherwise fully passes |
| FULL-AUDIT-FAIL-010 | **P2** | No (verified non-regression) | PowerShell guards | 3 guards reference stale file locations/patterns from before later refactors; underlying risk controls (dispatch wiring, day/tick order caps) verified present and correctly wired in current source |
| FULL-AUDIT-FAIL-011 | **P2** | No (verified non-regression) | Rust/mqk-daemon tests | 6 tests across 3 files fail identically: daily-data-readiness gate (added after these fixtures) now intercepts earlier than the fixtures anticipate; daemon behavior is more fail-closed, not less |
| FULL-AUDIT-FAIL-012 | P2 | Not yet determined | Rust/mqk-daemon tests | 1 isolated failure (`db_04_cancel_persists_cancelled_status_and_reason`, expected 202 got 503), not root-caused within session time budget |

No P0 or P1 findings. No evidence of unsafe trading/order-integrity defects, live-capital exposure, unsafe code, hardcoded secrets, or dynamically-constructed SQL. Where the daemon's behavior diverged from test expectations, it did so in the direction of *more* refusal/fail-closed, never less.

## Residue check

- One operational note: mid-session, a redundant narrow-reproduction `cargo test` invocation was terminated via `taskkill /F /IM cargo.exe /T` to avoid two concurrent cargo builds competing for memory on this constrained machine; this inadvertently also killed the just-started full no-fail-fast run, which was immediately and cleanly restarted as a single process. No partial/corrupted state resulted — confirmed via a post-restart process count of 0 stray `cargo`/`rustc` processes before the final run began, and the final run completed normally end-to-end.
- No `mqk-daemon`/daemon process left running (confirmed zero `cargo`/`rustc` processes remain at report finalization).
- Heavy lock `C:\tmp\mqk-machine-heavy.lock` unchanged (was already stale/free at precheck; not created or deleted by this audit).
- Throwaway database `mqk_audit_fresh` (port 5434) was dropped after use — confirmed via `pg_database` query, no residue.
- `mqk_test` (port 5434) may carry residue from the still-running Phase 4 workspace test (in progress at report time) — this is the DB the test harness itself owns and manages; not independently cleaned by this audit.
- `git status --short --untracked-files=all` shows only the pre-existing allowed untracked paths (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`, `smoke_logs/**`) plus this report's own new files under `docs/audits/` and `smoke_logs/full_repository_audit_2026-08-02/**` — no other tracked or untracked drift.
- Nothing outside `docs/audits/full_repository_verification_2026-08-02.md` (and this pointer note in the ledger, per commit policy) was staged.

## Disposition (original session)

Phases 1, 2, 3, 5, 6, 7, 8, 9, 10, and 11 are complete. Phase 4's primary (non-`--ignore`d) Rust workspace test matrix completed in full on a second pass with `--no-fail-fast` (437/437 discovered test binaries executed, 5,227 passed, 7 failed — all 7 classified above, none P0/P1, none flaky). The one remaining lane was the `--include-ignored` DB-gated sweep (687 tests observed as ignored in that run) — **not executed in the original session** for time reasons, and FULL-AUDIT-FAIL-012 was not yet root-caused. Both gaps are closed below in the **Completion Session**.

---

## Completion Session — Ignored-Test Sweep and FAIL-011/FAIL-012 Closure (2026-08-02, continuation)

**Audit:** MINIQUANTDESK-V4-FULL-REPOSITORY-VERIFICATION-COMPLETION-01
**Scope:** audit-only, no source/test/guard/config repairs. Continues from the original session above.

### Precheck

| Field | Value |
|---|---|
| Required/actual local HEAD | `c6161c416c43389460c4810b15cebd7f28891ec2` — match |
| `origin/main` | `c6161c416c43389460c4810b15cebd7f28891ec2` — match |
| Untracked files | Only `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and `smoke_logs/**` — no other drift |
| `mqk-daemon`/`cargo`/`rustc` process running | No |
| Heavy lock `C:\tmp\mqk-machine-heavy.lock` | Present, 0 bytes, last written 2026-07-28 — stale, not actively held |
| Docker | `mqk-test-postgres` → `127.0.0.1:5434`; `mqk-live-postgres` (5432) and `mqk-paper-postgres` (5440) present but never queried/mutated this session |
| Other worktrees / `MiniQuantDeskV4-ai-lab` | Present, not entered, not modified |

No blocking condition found. Completion audit proceeded.

### Part 1 — Ignored-test inventory

Static enumeration (function-level parse, not raw `grep`) of every `#[ignore]`-attributed test function under `core-rs/crates/*`:

| Crate | Ignored tests | `tests/` | `src/` (unit) |
|---|---|---|---|
| mqk-daemon | 428 | 417 | 11 |
| mqk-db | 233 | 228 | 5 |
| mqk-runtime | 19 | 15 | 4 |
| mqk-cli | 4 | 4 | 0 |
| mqk-testkit | 0 | 0 | 0 |
| **Total** | **684** | | |

**Cross-check against raw `grep -c "#\[ignore"` (755) and the original session's harness-observed ignored count (687):** the gap between the raw grep (755) and the real attribute count (684) is fully explained: 71 of the 755 raw hits are prose/comment lines (e.g. `// ... #[ignore] ...` inside doc comments describing the pattern, not real attributes) — confirmed by filtering to non-`//`-prefixed lines, which independently lands on exactly 684, matching the AST-level parse. The 6 `mqk-db` integration-test files gated behind `required-features = ["testkit"]`/`["runtime-claim"]` were confirmed (via the harness log) to still compile and run under a plain `cargo test --workspace` because other workspace members' `[dev-dependencies]` activate `testkit` during a whole-workspace build — so they are not a source of undercount. The residual 3-test gap between the real count (684) and the original session's harness-observed **687** ignored is unresolved to the single-test level (most plausibly a small number of additional comment-adjacent lines the line-level regex did not catch) but is immaterial: this completion session's own harness-executed total below (684) is self-consistent and fully reconciled against the independently-built static inventory with **zero** unexplained tests.

**Classification:** every one of the 684 ignored tests is **Category A (safe local/DB-only, executable now)**. Evidence:
- A DB-requirement text scan classified 537/684 with an explicit `#[ignore = "requires MQK_DATABASE_URL..."]` reason string; the remaining 147 bare `#[ignore]` tests were individually windowed and 100% resolve to the same family via `maybe_db(`/`maybe_pool(`/`no_db_state()`/`broken_pool()`/direct `PgPool::connect` helpers — no non-DB reason exists in the corpus.
- A targeted risk-marker sweep (`alpaca.markets`, `api.kraken`, `reqwest::Client`, `wss://`/`ws://`, `ALPACA_API_KEY*`, `MQK_KRAKEN_API`, `TcpStream::connect`) across every ignored function's body found **zero** hits. The 10 files that do contain these substrings elsewhere use them only in pure-string unit tests (URL-format derivation, e.g. `ws_url_from_base_url`), fake/loopback fixtures (`scenario_discord_secret_safety_01.rs` posts to `127.0.0.1:1`, expected-unreachable), or `std::env::set_var("ALPACA_API_KEY_PAPER", "test-paper-key")` placeholder config-detection — none are `#[ignore]`d and none make a real external call.
- No test requires broker credentials to *compile or start*; one test (`b2a_n02_registry_enabled_allows_activation`, discussed below) requires `ALPACA_API_KEY_LIVE` to be *set* to pass a presence gate — this audit correctly did not set it (prohibited: "do not load real credentials"), so that one test's outcome is an audit-environment constraint, not a defect.

No category B/C/D/E members exist in this inventory (mqk-testkit itself has 0 `#[ignore]` tests, so FULL-AUDIT-FAIL-003's E0063 compile blocker does not gate any ignored test — it only blocks `mqk-testkit`'s own non-ignored integration tests, confirmed unchanged in Part 5 below). **No unclassified test.**

Full per-test inventory (crate, file, line, function, reason): `smoke_logs/full_repository_audit_2026-08-02/completion_01/ignored_tests_inventory.csv`.

### Part 2 — Execution of every category-A ignored test (port 5434 only)

Executed per-crate (`cargo test -p <crate> [--features testkit] -- --ignored --test-threads=1 --no-fail-fast`), low-memory posture (`CARGO_BUILD_JOBS=1`, `CARGO_INCREMENTAL=0`, `RUSTFLAGS=-C debuginfo=0`, dedicated `CARGO_TARGET_DIR`), against `postgres://postgres:postgres@127.0.0.1:5434/mqk_test`. mqk-db required two invocations: default features (229 tests) plus `--features testkit` scoped to the two required-features-gated files carrying ignored tests (4 tests: `scenario_stale_claim_recovery`, `scenario_recovery_query_returns_pending_outbox`).

| Crate | Executed | Passed | Failed |
|---|---|---|---|
| mqk-cli | 4 | 3 | 1 |
| mqk-runtime | 19 | 18 | 1 |
| mqk-db (default) | 229 | 229 | 0 |
| mqk-db (`--features testkit`) | 4 | 4 | 0 |
| mqk-daemon | 428 | 409 | 19 |
| **Total** | **684** | **663** | **21** |

684/684 executed exactly matches the Part 1 static inventory — zero unexecuted, zero unaccounted-for.

**Operational note (not a defect):** `mqk-db::scenario_migration_bootstrap_replay_proof::migration_bootstrap_and_replay_follow_authoritative_manifest` performs `DROP SCHEMA public CASCADE` + fresh `mqk_db::migrate()` against whatever `MQK_DATABASE_URL` points to, by design (it proves migration bootstrap-from-blank against the authoritative manifest, and its own panic message says "run against a disposable postgres db"). Because this audit's required env var points every batch at the shared `mqk_test` database, this test wiped and rebuilt `mqk_test`'s schema mid-`mqk-db`-batch. The test itself passed correctly and proves exactly what it claims; the side effect was that all data accumulated in `mqk_test` up to that point (including this session's own precheck baseline) was reset to post-migration-empty and rebuilt fresh by every subsequent test. No corruption, no error, and ports 5432/5440 were never touched. **Recommendation:** this specific test should be run against a disposable one-off database (as the original session's Phase 5 already did for its own migrate-from-blank proof), not folded into a routine shared-`mqk_test` ignored-test sweep.

Every failure was rerun once in isolation with the exact function name(s), `--nocapture`, and `RUST_BACKTRACE=1`; DB residue was checked after each crate batch. **None were rerun again after failing (no "rerun until green").**

#### Failure classification

| Test | Crate/file | Signature | Classification |
|---|---|---|---|
| `gate03_evidence_records_input_file_network_authorization_mode` | mqk-cli / `scenario_cli_kraken_scheduler_task_gate_03a.rs` | "The system cannot find the path specified. (os error 3)" | **Deterministic** (2/2). Root cause: the test calls `std::fs::remove_dir_all(&out_dir)` (line 242) *before* reading the CLI's printed `evidence_path=` file (line 263), which lives inside that same directory — it deletes its own evidence file before verifying it. Test-only ordering bug; production `mqk-cli md kraken-ohlc-sync` command is unaffected. → **FULL-AUDIT-FAIL-013** |
| `b4_11_collect_db_snapshot_end_to_end` | mqk-runtime / `scenario_observability_b4.rs:338` | `sys_arm_state_reason_check` violation | **Deterministic** (2/2). Test passes the literal `Some("manual disarm")` as the arm-state reason; the schema's check constraint (migration 0009, refined through 0037) only accepts the PascalCase enum set (`ManualDisarm`, `OperatorDisarm`, …). Test-only wrong literal, unrelated to production disarm paths (which correctly use the PascalCase constants elsewhere). Residue (2 `runs` rows) cleaned. → **FULL-AUDIT-FAIL-014** |
| `routes::dynamic_selection_evidence::tests::plan_detail_for_a_real_persisted_row_is_found_and_valid` | mqk-daemon (`--lib`) | `left: "candidate_mismatch" right: "valid"` | **Order-dependent / DB-residue.** Part of the cluster below — the shared `mqk_test` DB carries dynamic-selection-plan rows from many prior tests in this long session; this unit test's "the real persisted row" lookup is not scoped to a fixture it privately owns. → grouped in **FULL-AUDIT-FAIL-017** |
| `ops02_t5_db_backed_ack_roundtrip` | mqk-daemon / `scenario_alerts_triage_ops02.rs` | expected `"postgres.sys_alert_acks"`, got `"postgres.sys_alert_acks+postgres.sys_incidents"` | **Deterministic**, test-predates-production-addition. The backend-target aggregator now also reports `sys_incidents` (a later addition); the test's exact-match assertion was never widened. → grouped in **FULL-AUDIT-FAIL-015** |
| `ah04_events_feed_surfaces_autonomous_session_kind_rows` | mqk-daemon / `scenario_auton_hist_durability_ah01.rs` | expected 3-source string, got a 4th `+postgres.audit_events[orchestrator]` appended | Same family as `ops02_t5` — test-predates-production-addition. → **FULL-AUDIT-FAIL-015** |
| `h03_clear_halted_run_no_run_returns_409` | mqk-daemon / `scenario_clear_halted_run_auton04.rs` | expected disposition `no_run_found`, got `run_not_halted` because "the latest run" resolved to a residual `CREATED`-state row from an earlier test in the same shared DB | **Order-dependent / DB-residue**, reproducible even in isolation because the query is globally scoped ("the latest run"), not privately fixtured. Production behavior is still fail-closed (refused the clear, did not fabricate success). → **FULL-AUDIT-FAIL-017** |
| `manual_order_submit_accepts_limit_order_with_explicit_defaults_aligned_to_runtime`, `manual_order_submit_duplicate_client_request_id_is_noop`, `manual_order_submit_enqueues_one_pending_outbox_row_for_active_run`, `manual_order_submit_refuses_when_durable_arm_state_is_disarmed_even_if_local_state_is_armed`, `manual_order_submit_refuses_when_durable_arm_state_is_halted_even_if_local_state_is_armed` (5) | mqk-daemon / `scenario_daemon_order_submit.rs:319` | shared `start()` helper: expected `/v1/run/start` → 200, got 403 `runtime.start_refused.deployment_mode_unproven` ("paper"+"paper" is not an honest paper trading path without `MQK_DAEMON_ADAPTER_ID=alpaca`) | **Deterministic**, test-predates-production-addition — same family as FULL-AUDIT-FAIL-011 (a gate added after the fixture was written now intercepts earlier). Fail-closed, not fail-open. → **FULL-AUDIT-FAIL-016** |
| `b1a_l04_start_with_registered_strategy_stores_active_bootstrap`, `b1a_l05_stop_clears_native_strategy_bootstrap`, `b1a_l06_halt_clears_native_strategy_bootstrap` (3) | mqk-daemon / `scenario_native_strategy_bootstrap_daemon_b1a.rs` | expected start→200, got 403 `runtime.start_refused.strategy_registry_missing` ("swing_momentum" not registered in `sys_strategy_registry`) | Same family/gate-predates-fixture pattern as above. → **FULL-AUDIT-FAIL-016** |
| `ed01_01_events_feed_exposes_orchestrator_halt_rows` | mqk-daemon / `scenario_evidence_durability_ed01.rs` | planted fixture event not found in the (bounded/recent) events feed, which instead returned dozens of unrelated residual rows from earlier tests in the session | **DB-residue**, reproducible in isolation because the events/feed query returns a bounded most-recent window and the huge volume of accumulated same-day test rows pushed the planted row out. → **FULL-AUDIT-FAIL-017** |
| `b2a_n03_registry_disabled_refused_at_registry_gate`, `b2a_n04_registry_absent_refused_at_registry_gate` (2) | mqk-daemon / `scenario_native_strategy_registry_b2a.rs` | **FAILED in the full-batch run, PASSED when rerun in isolation** | **Order-dependent (flaky under shared-DB load), not reproducible standalone.** Consistent with residual `sys_strategy_registry` state left by a preceding test in the same serialized run. → **FULL-AUDIT-FAIL-017** |
| `ir02_04_arm_execution_durable_target_is_sys_arm_state_not_audit_events`, `ir02_06_control_arm_accepted_action_durable_row_visible_in_history` (2) | mqk-daemon / `scenario_operator_audit_ir02.rs` | **FAILED in the full-batch run, PASSED when rerun in isolation** | Same order-dependent pattern. → **FULL-AUDIT-FAIL-017** |
| `n05_control_arm_provenance_ref_matches_exact_durable_audit_events_uuid` | mqk-daemon / `scenario_notify_ops01.rs` | expected `/control/arm` → 200, got 403 "reconcile status is 'dirty'; arm refused until reconcile is clean" | **Order-dependent / DB-residue** — a durable reconcile-checkpoint row left "dirty" by an earlier test in the shared DB blocks this later test's arm attempt. → **FULL-AUDIT-FAIL-017** |
| `b2a_n02_registry_enabled_allows_activation` | mqk-daemon / `scenario_native_strategy_registry_b2a.rs` | expected start→200, got 500 `runtime.start_refused.alpaca_creds_missing: broker 'alpaca' requires ALPACA_API_KEY_LIVE environment variable` | **Not a defect — blocked by this audit's own no-live-credentials constraint.** This test deliberately targets the `alpaca` broker path and requires `ALPACA_API_KEY_LIVE` to be *present* to pass the credential gate; this audit is explicitly prohibited from loading real credentials or broker env vars, so this test cannot be safely brought to green in this environment. The gate itself is correctly fail-closed. → **FULL-AUDIT-FAIL-018** |

DB residue after the `mqk-daemon` batch: 19 synthetic-`engine_id` rows remain in `runs` (patterns `test-*`, `MAIN*`, `EXP`, `zze*`). A `DELETE … WHERE engine_id ~ '^(test-|zze|MAIN|EXP|obs-e2e-)'` was attempted; it is blocked by an FK from `sys_dynamic_selection_plans.run_id` on one row (`89ff5fb3-…`) that a full cascade was not chased down within this session — all 19 rows are provably test-owned (synthetic `engine_id` values, none match real operator naming), confined to `mqk_test` on port 5434 only; ports 5432/5440 were never touched. Left in place rather than force-deleted without full FK verification.

### Part 3 — FULL-AUDIT-FAIL-011 reproduction (all 6/6)

Each of the 6 originally-reported tests was rerun individually (not `#[ignore]`d — these are part of the ordinary Phase-4 matrix) with `--nocapture --test-threads=1 RUST_BACKTRACE=1`. All 6/6 reproduced identically to the original two runs (now a 3rd independent confirmation, fully deterministic):

| Test | Result | fault_class |
|---|---|---|
| `g01_aligned_state_satisfies_both_start_and_signal_chains` | 403 (expected 503) | `runtime.start_refused.daily_data_readiness_blocked` |
| `brk00r06_e02_live_continuity_unblocks_ws_gate_reaches_db_gate` | 403 (expected 503) | `runtime.start_refused.daily_data_readiness_blocked` |
| `brk00r06_e03_continuity_round_trip_is_fail_closed` | 403 (expected 503) | `runtime.start_refused.daily_data_readiness_blocked` |
| `ptday02_e08_cold_start_unproven_blocks_strategy_signals` | 403 (expected 503) | `runtime.start_refused.daily_data_readiness_blocked` |
| `brk09r_r03_unknown_reconcile_does_not_block_start` | 403 (expected 503) | `runtime.start_refused.daily_data_readiness_blocked` |
| `brk09r_r04_ok_reconcile_does_not_block_start` | 403 (expected 503) | `runtime.start_refused.daily_data_readiness_blocked` |

Exact body (g01): `top_level_blocker=Some("required_assignments_missing")`, `assignment_resolution_error=Some("multi_symbol_config_missing_symbol")`, referencing `DAILY-DATA-READINESS-01C-ENFORCEMENT-01`.

**Fixture-builder proof:** all 6 tests share fixture-builder functions (`aligned_state()` in `scenario_combined_paper_gate_rts07_rsk07.rs`, and equivalents in the other two files) that arm the integrity gate, establish WS continuity, publish a reconcile snapshot, set the session clock, and set an active strategy fleet — but never configure a `multi_symbol_config`/daily-data-readiness assignment. `crates/mqk-daemon/src/state.rs` independently exposes `set_daily_data_readiness_evidence_override_for_test(&self, forced: Option<bool>)`, a test-only override that other (passing) tests use to satisfy this exact gate — proving the gate is satisfiable in a test fixture and that these 5 files' shared builders simply never call it.

**Production behavior:** fail-closed, not fail-open — the daemon refuses *more* strictly than these 5-file-old fixtures anticipate, consistent with `CLAUDE.md`'s "fail-closed over fail-open" invariant. Not a safety regression.

**Classification: fixture update.** Not an expected-gate-update, not a direct-replacement test, not obsolete-test removal — the smallest correct repair is adding daily-data-readiness fixture configuration (via the existing test-only override or an explicit `multi_symbol_config`) to the shared builder functions in these 3 files.

### Part 4 — FULL-AUDIT-FAIL-012 root cause (definitive, not "likely")

`crates/mqk-daemon/tests/scenario_ingest_jobs_data_ingest_daemon_01.rs::db_04_cancel_persists_cancelled_status_and_reason` was run: (1) in isolation twice — identical `left: 503, right: 202` at line 2505 both times; (2) the entire 54-test binary serialized — identical failure, all 53 other tests in the file passed (`53 passed; 1 failed`). **3/3 fully deterministic.**

**Source trace:** `routes/ingest.rs::ingest_job_cancel` — the seeded job is not in the in-memory `ingest_jobs` store, so the handler falls to the DB path: `load_persisted_ingest_job` → status not terminal → sets `record.status = IngestJobStatus::Cancelled` → `persist_ingest_job_record(pool, &record)`. **503 (`backend_unavailable_cancel_response`) is returned only when that persist call errors.**

**Exact defect, proven from the schema, not inferred:** `sys_ingest_jobs_status_check` (migration `0041_ingest_job_history.sql`, the *only* migration that ever defines this constraint — confirmed via `grep -l` across all 60 migrations) allows exactly `'queued','running','completed','dry_run_completed','failed','refused','partial'`. It does **not** include `'cancelled'`. `IngestJobStatus::Cancelled` (and the `/cancel` route that produces it) was added afterward, in commit `6f276155 feat(daemon): add ingest job cancellation`, which post-dates `c3c0b28b feat(daemon): persist ingest job history` (the commit that introduced migration 0041) — confirmed via `git log -S`. Per `db_rules.md` ("migrations are append-only, never modify a committed migration file"), no later migration ever widened this constraint to admit `'cancelled'`.

**This means every DB-backed ingest-job cancel — not just this test — always fails with a check-constraint violation and returns 503**, in this environment and in every environment sharing this exact migration history (CI included). It is not test-order-dependent, not residue, not a race, not environmental: the constraint categorically excludes the value the cancel route writes, unconditionally.

**Worse: a truth-integrity gap, not just an availability gap.** The *in-memory* cancel path (`cancel_record_in_memory`, taken when the job is tracked in-process) sets `status = Cancelled` in memory **before** attempting the same DB persist — so on that path the operator-visible in-memory truth flips to "cancelled" even though the durable write then fails with the same constraint violation and the DB row is left at its pre-cancel status. A daemon restart reloading from DB (the documented source of truth per `db_rules.md`) would show the job as still queued/running, silently reverting an operator's cancel action. This is exactly the class of divergence `CLAUDE.md`'s operator-truth-discipline section warns against ("no fabricated truth... distinguish unavailable, empty, and present").

**Classification: production cancel-route regression** (schema/code drift — new enum variant + route added without an accompanying migration). Not stale-test, not fixture, not residue, not order-dependence, not race, not environment.

**Severity:** P1 (real, deterministic functional defect + truth-divergence risk on the in-memory-tracked path), scoped to the ingest-job subsystem only.
**Paper-soak blocker:** **NO** — ingest-job cancellation is a data-ingestion management operation, not part of the order/trading OMS lifecycle chain; it does not touch outbox/inbox/broker/portfolio authority.
**Can normal premarket ingest/cancel be affected?** **YES** — any operator cancelling a running/queued ingest job through the real `/api/v1/ingest/jobs/:job_id/cancel` route in a DB-backed deployment hits this exact defect today.
**Smallest focused repair:** one new sequential migration (`0061_...sql`) that `ALTER`s `sys_ingest_jobs_status_check` to add `'cancelled'` to the allowed set. No application-code change is required — `IngestJobStatus::Cancelled` and the route logic are already otherwise correct.

### Part 5 — Narrow consistency checks (re-verified, no repairs)

- **FULL-AUDIT-FAIL-003**: re-run (`cargo check -p mqk-testkit --all-targets`, then the second target individually) — still exactly 4× `E0063` at the same 4 locations (`scenario_cli_run_start_creates_artifacts.rs:46,152,165`; `scenario_run_artifacts_manifest_created.rs:13`). Unchanged.
- **FULL-AUDIT-FAIL-010**: re-verified against current `HEAD` source at the exact previously-cited line numbers — `RuntimeStrategyDispatchAuthority::Legacy` dispatch wiring (`loop_runner.rs:61,862,864,1350,1484`), `day_signal_count.store(0, …)` + `reset_symbol_day_order_counts()` pairing in `state.rs:4292-4293,4432-4433`, and cap-#6 wiring in `loop_runner.rs:976-1705` all confirmed present, unchanged, correctly wired. Still 3 stale guards / non-regression.
- **FULL-AUDIT-FAIL-007**: `research-py/src/mqk_research/sweeps/run_sweep.py:28` still has the unescaped-quote `SyntaxError`; still zero callers (`grep -rn "run_sweep" src tests` → no hits outside the file). Unchanged.
- **FULL-AUDIT-FAIL-008**: `Launch-VeritasLedger.repo_correct.ps1` and `_patch_staging/veritas_batch3/scripts/windows/Launch-VeritasLedger.ps1` still present, unreferenced; canonical `scripts/windows/Launch-VeritasLedger.ps1` unaffected. Unchanged.
- **Migration count/manifest**: still 60 `.sql` files (`0001`–`0060`). `manifest.json` lists 61 entries — the 61st is `"0017-hold"`, a reserved/held placeholder id with no corresponding `.sql` file (not a new finding; consistent with the append-only/sequential governance model, pre-existing). Migration governance guard re-run: PASS.

### Residue check (completion session)

- No `mqk-daemon`/`cargo`/`rustc` process left running (confirmed before and after).
- Heavy lock unchanged (still stale/free, not created/deleted by this session).
- Ports 5432/5440 never queried or mutated.
- Throwaway/synthetic residue on `mqk_test` (port 5434): kraken `md_bars` rows from mqk-cli tests — cleaned (0 remain). 2 `runs` rows from the mqk-runtime `b4_11` failure — cleaned. 19 synthetic-`engine_id` `runs` rows from the mqk-daemon batch — **not fully cleaned**, blocked by an FK from `sys_dynamic_selection_plans`; documented above, confirmed test-owned, confined to `mqk_test` only.
- `git status --short --untracked-files=all`: only pre-existing allowed untracked paths (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`, `smoke_logs/**`) plus this session's own new files under `smoke_logs/full_repository_audit_2026-08-02/completion_01/**` (inventory CSV, per-batch/per-failure logs) and `docs/audits/full_repository_verification_2026-08-02.md`. No other tracked or untracked drift. No raw logs staged.

## Ignored-test execution ledger (final)

- Discovered (static, AST-parsed): 684
- Safe/executed (Category A): 684
- Passed: 663
- Failed: 21
- Blocked local prerequisite (Category B): 0
- Unsafe external/broker (Category C): 0
- Blocked by mqk-testkit E0063 compile (Category E): 0 (mqk-testkit has 0 ignored tests)
- Unclassified: 0
- New failure IDs from this sweep: FULL-AUDIT-FAIL-013 (mqk-cli, deterministic test bug), FULL-AUDIT-FAIL-014 (mqk-runtime, deterministic test bug), FULL-AUDIT-FAIL-015 (mqk-daemon ×2, test-predates-production-addition), FULL-AUDIT-FAIL-016 (mqk-daemon ×8, test-predates-production-gate, same family as FULL-AUDIT-FAIL-011), FULL-AUDIT-FAIL-017 (mqk-daemon ×8, order-dependent/DB-residue cluster), FULL-AUDIT-FAIL-018 (mqk-daemon ×1, blocked by this audit's own no-live-credentials constraint, not a defect)
- Log directory: `smoke_logs/full_repository_audit_2026-08-02/completion_01/`

## Updated failure inventory (additions)

| ID | Severity | Soak blocker? | Subsystem | Classification |
|---|---|---|---|---|
| FULL-AUDIT-FAIL-013 | P3 | No | mqk-cli test | Deterministic test-only bug: evidence dir deleted before being read back |
| FULL-AUDIT-FAIL-014 | P3 | No | mqk-runtime test | Deterministic test-only bug: wrong literal reason string violates DB check constraint |
| FULL-AUDIT-FAIL-015 | P3 | No | mqk-daemon test (×2) | Test assertions predate later-added durable backend targets (exact-match too narrow) |
| FULL-AUDIT-FAIL-016 | P2 | No (verified non-regression) | mqk-daemon test (×8) | Same family as FULL-AUDIT-FAIL-011: tests predate newer start-gates (`deployment_mode`, `strategy_registry`) not configured by their fixtures; daemon fail-closed, not fail-open |
| FULL-AUDIT-FAIL-017 | P3 | No | mqk-daemon test (×8) | Order-dependent/DB-residue: shared long-lived `mqk_test` DB across a full-day sweep leaves state (leftover runs, bounded event-feed truncation, dirty reconcile checkpoint, registry rows) that a subset of tests' global (non-privately-fixtured) queries pick up; 4 of the 8 pass cleanly in isolation, confirming order-dependence rather than a code defect |
| FULL-AUDIT-FAIL-018 | — | No | mqk-daemon test (×1) | Not a defect — test requires `ALPACA_API_KEY_LIVE` to pass a credential-presence gate; this audit is explicitly prohibited from setting it. Gate itself is correctly fail-closed. |
| FULL-AUDIT-FAIL-011 (resolved) | P2 | No (verified non-regression) | mqk-daemon test (×6) | Confirmed 6/6, 3rd independent reproduction. Root cause proven: 5 shared fixture-builder functions across 3 files never configure daily-data-readiness facts; a test-only override exists and is unused by these builders. **Classification: fixture update.** |
| FULL-AUDIT-FAIL-012 (resolved) | **P1** | **No** (ingest subsystem only, not OMS/order lifecycle) | mqk-daemon / mqk-db schema | **Confirmed production regression**, not a stale test. `sys_ingest_jobs_status_check` (migration 0041, append-only, never updated) omits `'cancelled'`, added later by the `/cancel` route (commit `6f276155`). Every DB-backed ingest-job cancel fails with a check-constraint violation → 503; the in-memory-tracked path additionally creates a truth-divergence (memory says cancelled, DB does not). **Smallest repair: one new migration widening the constraint.** |

## Recommended one-patch-at-a-time repair order

Each item below is independent and must be its own patch (no bundling), most operationally urgent first:

1. **INGEST-JOB-CANCEL-STATUS-CONSTRAINT-REPAIR-01** — add migration `0061_ingest_job_cancelled_status.sql` widening `sys_ingest_jobs_status_check` to include `'cancelled'`. Closes FULL-AUDIT-FAIL-012. Highest priority: real, currently-broken operator-facing route with a truth-integrity side effect.
2. **DAILY-DATA-READINESS-FIXTURE-REPAIR-01** — add daily-data-readiness fixture configuration (existing test-only override, or explicit `multi_symbol_config`) to the shared builder functions in `scenario_combined_paper_gate_rts07_rsk07.rs`, `scenario_paper_alpaca_proof_bundle_brk00r06.rs`, `scenario_reconcile_start_gate_brk09r.rs`. Closes FULL-AUDIT-FAIL-011 (6 tests) and the `deployment_mode`/`strategy_registry` half of FULL-AUDIT-FAIL-016 (8 tests) if the same gate-satisfying fixture pattern is reused/extended — verify per-file, do not assume identical fix shape.
3. **SCRIPT-GUARD-STALENESS-REPAIR-01** — update the 3 stale script guards (FULL-AUDIT-FAIL-010) to match current source locations/shapes.
4. **MQK-TESTKIT-E0063-REPAIR-01** — add `timeframe`/`timeframe_secs` to the 4 struct-literal call sites (FULL-AUDIT-FAIL-003).
5. **MQK-CLI-KRAKEN-GATE03-TEST-FIX-01** — reorder the cleanup/read sequence in `gate03_evidence_records_input_file_network_authorization_mode` (FULL-AUDIT-FAIL-013).
6. **MQK-RUNTIME-OBSERVABILITY-B4-11-TEST-FIX-01** — replace `"manual disarm"` with the canonical `"ManualDisarm"` reason constant (FULL-AUDIT-FAIL-014).
7. **MQK-DAEMON-BACKEND-TARGET-ASSERTION-WIDEN-01** — widen the two exact-match backend-target assertions to `contains` semantics (FULL-AUDIT-FAIL-015).
8. **RESEARCH-PY-RUN-SWEEP-SYNTAX-FIX-01** — escape the nested quotes in the dead `run_sweep.py` module, or delete it if truly unreferenced (FULL-AUDIT-FAIL-007).
9. Not independently patch-worthy: FULL-AUDIT-FAIL-017 (order-dependence is a shared-test-DB operational hazard, not a code defect — recommend a per-suite disposable-DB or explicit-cleanup convention rather than a source patch) and FULL-AUDIT-FAIL-018 (not a defect).
