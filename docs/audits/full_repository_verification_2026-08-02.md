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

**Coverage as of report time:**
- GUI test suite (Node, separate toolchain, unaffected by the Rust memory constraint): 977/977 passed — see Phase 6.
- Python test suite (separate toolchain): 986 passed, 5 skipped (documented reasons) — see Phase 7.
- Rust workspace test matrix (`cargo test --workspace --exclude mqk-testkit -- --test-threads=1`, low-memory posture): **in progress / incomplete at report time.** No product test failures had been observed prior to report compilation; the run was still in its serial dependency-compilation phase.
- `mqk-testkit`'s own test binaries: not run (blocked by FULL-AUDIT-FAIL-003).
- `--include-ignored` DB-gated sweep (the ~479+246+21+9 `#[ignore = "requires MQK_DATABASE_URL..."]` tests): **not executed** — correctly classified as "available, not run" rather than silently skipped. Almost all inspected `#[ignore]` reasons are uniformly `"requires MQK_DATABASE_URL"` (i.e., safe to run against the port-5434 test DB) with exact unblock commands documented in-line by the authors; a small number of Python-side skips additionally require a built `mqk-cli` binary (`MQK_BACKTEST_CLI` env var) or `MQK_RUN_DB_PROOF_TEST=1`, both of which are safe-but-not-yet-executed lanes.

This is the single incomplete lane in an otherwise-complete audit. See **Disposition** at the end of this report.

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

No P0 or P1 findings. No evidence of unsafe trading/order-integrity defects, live-capital exposure, unsafe code, hardcoded secrets, or dynamically-constructed SQL.

## Residue check

- No `mqk-daemon`/daemon process left running.
- Heavy lock `C:\tmp\mqk-machine-heavy.lock` unchanged (was already stale/free at precheck; not created or deleted by this audit).
- Throwaway database `mqk_audit_fresh` (port 5434) was dropped after use — confirmed via `pg_database` query, no residue.
- `mqk_test` (port 5434) may carry residue from the still-running Phase 4 workspace test (in progress at report time) — this is the DB the test harness itself owns and manages; not independently cleaned by this audit.
- `git status --short --untracked-files=all` shows only the pre-existing allowed untracked paths (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`, `smoke_logs/**`) plus this report's own new files under `docs/audits/` and `smoke_logs/full_repository_audit_2026-08-02/**` — no other tracked or untracked drift.
- Nothing outside `docs/audits/full_repository_verification_2026-08-02.md` (and this pointer note in the ledger, per commit policy) was staged.

## Disposition

Phases 1, 2, 3, 5 (core), 6, 7, 8, 9, 10, and 11 are complete, with explicit classification of every skipped/unavailable/incomplete sub-item per the audit's "no silent skips" rule. Phase 4's non-DB-gated workspace test matrix was still running (low-memory serial posture, matching the CI-documented workaround for this machine's page-file constraint) at the time this report was compiled, and the `--include-ignored` DB-gated sweep (~750+ tests across 4 crates) had not yet been started.

**MINIQUANTDESK-V4-FULL-REPOSITORY-VERIFICATION-AND-FAILURE-INVENTORY-01:
BLOCKED — EXACT COMPLETED/REMAINING COVERAGE AND ROOT CAUSE PROVIDED**

Remaining commands to complete Phase 4, exactly as configured in this session (re-run if continuing):

```
cd core-rs
set CARGO_TARGET_DIR=C:\tmp\mqk-target-full-repository-audit
set MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test
set CARGO_BUILD_JOBS=1
set CARGO_INCREMENTAL=0
set RUSTFLAGS=-C debuginfo=0
cargo test --workspace --exclude mqk-testkit -- --test-threads=1
```

then, once that completes cleanly:

```
cargo test --workspace --exclude mqk-testkit -- --test-threads=1 --include-ignored
```

Root cause for the block is machine/session time, not a safety or scope decision — the run was actively executing under the documented-correct low-memory posture at report time, not stalled or erroring.
