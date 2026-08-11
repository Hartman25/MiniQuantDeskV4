# MiniQuantDesk V4 — Authoritative Master Completion Ledger

**Audit:** FULL-REPO-COMPLETION-AUDIT-01
**Audit date:** 2026-08-10
**Mode:** AUDIT + LEDGER ONLY — no application code, DB, or trading behavior was modified in this session.
**Branch:** `main`
**Starting HEAD:** `0a019b8bd80298ac0a04ba77fb080522122c37a8` ("fix: fence daemon supervisor safety halts")
**origin/main HEAD (matches local):** `0a019b8bd80298ac0a04ba77fb080522122c37a8`
**Worktree:** primary working tree at `C:\Users\Zacha\Desktop\MiniQuantDeskV4` (several other worktrees/clones exist under `.claude/worktrees/`, `.codex/worktrees/`, and a sibling `MiniQuantDeskV4-ai-lab` dir — not inspected in this audit, out of scope).
**Repository dirty/untracked state at audit start:**
- Modified, uncommitted: `core-rs/crates/mqk-daemon/src/routes/control_plane.rs`, `core-rs/crates/mqk-daemon/src/state.rs`, `core-rs/crates/mqk-daemon/src/state/loop_runner.rs`, `core-rs/crates/mqk-daemon/tests/scenario_clear_halted_run_auton04.rs`, `scripts/test/ignored_test_inventory.csv` (net +447/-8 lines). This is a coherent, self-contained in-progress patch — see `PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01` below. It was **read as-is** (current repo truth) but **not committed, not modified, not run** by this audit.
- Untracked: `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` (this file — now tracked as the authoritative ledger), `smoke_logs/` (protected, untouched, generated artifacts).
- This is the **paper-soak baseline branch** (`main`), not a derived development branch. Per the Paper-Soak Protection Rule below, this audit treated all in-scope code as protected and made no trading-path changes.

This document is the whole-repository completion ledger for MiniQuantDeskV4. It supersedes `MiniQuantDesk_Master_Patch_Ledger_v2.md` (a 21k-line append-only session-prompt log, `~1.6MB`, kept as historical archive — not deleted) as the **current-status source of truth**. Future sessions should read *this* file first, locate the next eligible `READY` patch, implement exactly that one patch, and stop.

---

## 0. Executive Summary

### Current Repository Verdict
The equity/ETF paper-trading core (orchestrator, OMS state machine, outbox/inbox, broker adapters, risk, portfolio, reconciliation, backtest engine, promotion gates, GUI truth-state discipline) is **evidence-provably complete and fail-closed** at HEAD. No RED (soak-blocking) source defect was found anywhere in the audited codebase. The repository's real remaining gaps cluster in three places: (1) live-capital readiness — deliberately and completely gated off pending a trust-chain proof that doesn't exist yet; (2) operational hardening around multi-symbol dispatch resilience, CLI/daemon control-plane parity, and Discord alert coverage; (3) one uncommitted-but-well-formed patch closing a narrow halt/deadman race that needs a harness run before it can be called closed.

### Current Paper Verdict
**PAPER_SOAK_GO** (`FINAL-CANONICAL-PRE-SOAK-VALIDATION-01`, 2026-08-10, HEAD `e44e3ddd`). The one previously-open item, `PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01`, is now CLOSED — its H08 test passed against a real local Postgres as part of a full canonical safe-ignored matrix run (733/733 tests green: H01-H08, daemon-supervisor halt fence, runtime halt fence CAS, stale-claim recovery, deadman, durable portfolio/P&L, fill/replay authority, outbox/pre-submit authority, risk/kill-switch/reconcile all proven with zero failures). All previously-tracked blockers (TradeActivity schema mismatch, partial-fill dedup, stale-claim recovery, terminal-fill replay parity) have corresponding committed fixes at HEAD, and this validation reproduced no new regression against any of them. No known accepted-list paper-soak code blocker remains.

### Current Live Verdict
**NOT READY, and cannot become ready without new work.** `LiveCapital` cold-start is hard-gated behind a trust-chain proof (`live_trust_complete`) that is **hardcoded `false`** in `research-py`'s TV-03 pipeline — this is by design, not a bug, and correctly enforced at both the advisory and cold-start-enforcement layers. Separately, live account truth is wrong today: `buying_power` is aliased to `cash` rather than pulled from Alpaca's real `buying_power`/`daytrading_buying_power` fields, which is economically dangerous for a margin account. No live-capital smoke-test tooling exists. A prior memory record claiming "daemon defaults to real Alpaca WS unless forced to paper" is **stale** — current default (`Paper`/`Paper`) is fail-closed and safe; this session is correcting that memory record.

### Closest Subsystems to Completion
Core Execution/OMS (~97%), Database Layer (~97%), Reconciliation (~97%), Risk System (~95%), Paper Trading Lifecycle (~95%), Backtesting Engine (~95%), Test Infrastructure (~95%).

### Highest-Risk Incomplete Subsystems
Live Capital Trading (~40%, gated by design but genuinely far from proven), CLI/Daemon control-plane parity (~60%, no CLI path to arm/halt/clear the live daemon), Discord/Alerting coverage (~70%, multi-channel routing built but unused, no data-staleness/daily-summary pushes), Options/Futures/Forex (~5%, enum + risk-multiplier stub only, explicitly gated off).

### Active Patch Counts
READY: 33 · BLOCKED: 4 · DEFERRED: 8 · IMPLEMENTED_PENDING_REVIEW: 0 · CLOSED (this session): 1

### GREEN / YELLOW / RED Patch Counts
GREEN: 27 · YELLOW: 12 · RED: 7

### Estimated Remaining Patch Range
**42–55 patches** to reach the Repository-Wide Definition of Done in §19, excluding open-ended multi-asset expansion (Lane E, 3 XL items each requiring further decomposition into an unknown number of sub-patches once scoped). The range reflects uncertainty in how far `LIVE-TRUST-CHAIN-*` and the two lean-out patches will need to be decomposed once started.

---

## 1. Paper-Soak Protection Rule

The highest immediate priority is a stable US equity/ETF Paper + Alpaca autonomous trading soak, currently running on `main`. This ledger classifies every patch:

- **GREEN — safe during paper soak.** Source-evidence-confirmed isolation from the running paper economic/safety path.
- **YELLOW — shared code, paper-neutral intent.** Touches code paper also uses; safe to develop on a separate branch/worktree during the soak; must not merge into `main` without explicit regression review.
- **RED — paper economic/safety behavior.** Directly changes order decisions, execution, risk, reconciliation, position/P&L authority, fills, runtime leadership, halt/recovery, autonomous startup, data-freshness gates, or broker submission. Deferred during the soak unless it repairs a reproducible soak-blocking defect (none currently exist).

No patch in this ledger lacks a classification.

---

## 2. System Completion Map

| Rank | Subsystem | Evidence-based Completion | Current State | Remaining Patches | Paper Impact | Lane |
|---|---|---|---|---|---|---|
| 1 | Core Execution / OMS / Outbox / Halt | ~97% — orchestrator phase ordering, OMS state machine, outbox atomicity, idempotency, and the halt gate are all PROVEN COMPLETE with full scenario-test coverage; only gap is the uncommitted fence patch pending harness proof. | PROVEN COMPLETE | 1 | RED | A |
| 2 | Database Layer (mqk-db, mqk-audit) | ~97% — 64 sequential migrations, atomic outbox claim, deterministic UUIDv5 audit IDs, no `DEFAULT now()`/`gen_random_uuid()` in schema, all proven by scenario tests. | PROVEN COMPLETE | 1 | YELLOW | D |
| 3 | Reconciliation | ~97% — reconcile gate, drift detection, staleness handling all pure/deterministic/tested; zero TODOs. | PROVEN COMPLETE | 0 | GREEN | — |
| 4 | Risk System | ~95% — pre-trade gate fully fail-closed, `checked_sub` throughout, PDT/kill-switch/loss-limit all tested; only a doc-placement clarification remains. | PROVEN COMPLETE | 1 | GREEN | B |
| 5 | Paper Trading Lifecycle | ~95% — PAPER_SOAK_READY; halt/clear/re-arm chain proven; one uncommitted fence patch pending proof (shared with #1). | PROVEN COMPLETE | 0 (shared with #1) | RED | A |
| 6 | Backtesting Engine | ~95% — conservative worst-case fills, commission modeling, anti-lookahead, deterministic IDs, real GUI (not a stub), all proven. | PROVEN COMPLETE | 0 | GREEN | — |
| 7 | Test Infrastructure | ~95% — deterministic fixtures, 65+ scenario files, CI guard against silently-ignored load-bearing tests. | PROVEN COMPLETE | 0 | GREEN | — |
| 8 | Portfolio / P&L | ~93% — restart-safe replay, watermark-based dedup, fail-closed on malformed rows; one large unreviewed file (`dynamic_selection.rs`, 3680 lines). | PROVEN COMPLETE / 1 file UNKNOWN | 2 | GREEN | B |
| 9 | Broker Architecture (Alpaca + Paper) | ~93% — normalization, cursor/gap contract, credential separation all proven; dead/orphaned code and missing rate-limit backoff remain. | PROVEN COMPLETE (core) / IMPLEMENTED BUT INCOMPLETE (resilience) | 3 | GREEN/YELLOW | B/D |
| 10 | Market Calendar / Session Authority | ~90% — NYSE calendar fail-closed, DST-correct, covers 2023-2028 (~2.4yr runway remaining). | PROVEN COMPLETE | 1 (deferred) | RED | D |
| 11 | Config / Deployment / Secrets | ~90% — layered YAML, mode-aware secret resolution, redaction all proven; no containerized deployment path (undocumented decision). | PROVEN COMPLETE | 1 | GREEN | B |
| 12 | Daemon / Autonomous Operations | ~90% — 12 lifecycle defects closed, extensive coordinator machinery; lease/TTL asymmetry and the uncommitted fence remain. | IMPLEMENTED BUT INCOMPLETE | 1 | RED | D |
| 13 | Dynamic Strategy Selection | ~85% — extensive fail-closed machinery for paper dispatch; live promotion correctly hard-pinned false; doc staleness and thin dedicated test coverage. | PARTIAL / SCAFFOLDED (by design for live) | 3 | GREEN | B |
| 14 | Strategy Research / Promotion | ~85% — gate mechanics (NaN, tie-break, artifact-lock, stress-suite, provenance) fully proven fail-closed; walk-forward/overfitting only enforced in Python. | PROVEN COMPLETE (gates) / IMPLEMENTED BUT INCOMPLETE (overfitting) | 1 | GREEN | B |
| 15 | Data Ingestion (equities) | ~85% — provider registry, job system, cancellation, readiness gates proven; no retry/backoff on Alpaca/Kraken transient failures. | PROVEN COMPLETE (core) / IMPLEMENTED BUT INCOMPLETE (resilience) | 2 | YELLOW/GREEN | D/B |
| 16 | GUI Operator Console | ~92% — truth-state hard-block discipline consistently enforced repo-wide; one real gap (409 response body dropped before reaching operator). | PROVEN COMPLETE (discipline) / 1 defect | 1 | GREEN | B |
| 17 | Strategy Engines (signal logic) | ~80% — 4 strategies wired, dispatchable, and registered; 3 of 4 have zero unit tests; no stop-loss/take-profit exists anywhere in the crate. | IMPLEMENTED BUT INCOMPLETE (engine complete, alpha unproven) | 4 | GREEN | B |
| 18 | Multi-Symbol Trading | ~75% — wired and dispatching in production; no per-symbol panic isolation (one symbol's panic drops the whole tick); capital caps opt-in and silently disabled if unset. | IMPLEMENTED BUT INCOMPLETE | 3 | RED | D |
| 19 | Documentation / Runbooks | ~70% — dense, mostly-accurate historical spec archive; `README.md`'s living "repository snapshot" is 3 weeks stale relative to HEAD. | PARTIAL (living doc stale) | 1 | GREEN | B |
| 20 | Discord / Alerting / Observability | ~70% — delivery contract solid and fail-safe; 6-channel routing built but unused (single flat webhook only); no data-staleness or daily-summary alerts. | IMPLEMENTED BUT INCOMPLETE | 3 | YELLOW | D |
| 21 | CLI | ~60% — read-only/diagnostic commands (backtest, md, db, autonomous diagnostics) solid; **zero** CLI path to arm/halt/clear/disarm the live daemon — HTTP-only. | PARTIAL / SCAFFOLDED (parity gap) | 2 | GREEN | B |
| 22 | Performance / Maintainability | ~70% — no dead-code explosion; several files >7,000 lines in the daemon hot path (`state.rs` 7,591, `lifecycle.rs` 7,126); duplicated alert-block logic. | ACCEPTABLE, not urgent | 2 | GREEN | F |
| 23 | Live Capital Trading | ~40% — shared infra (broker dispatch, arm gate, kill switch, reconcile) proven and reused correctly; trust-chain proof hardcoded false; account truth wrong; zero live smoke tooling. | DEFERRED BY DESIGN (trust gate) / real gaps beneath it | 8 (+2 blocked, +1 external) | YELLOW/GREEN | C |
| 24 | Multi-Asset — Equity/ETF | 100% of current scope — trades as `Equity` with `instrument_kind="etf"` tag; fully operational. | PROVEN COMPLETE | 0 | — | — |
| 25 | Multi-Asset — Crypto | ~25% — Kraken OHLC data-ingest lineage substantial; zero execution wiring. | PARTIAL (data only) | 1 | GREEN (isolated) | E |
| 26 | Multi-Asset — Options/Futures/Forex | ~5% — `AssetClass` enum variants + risk-multiplier stub match-arms only; explicitly gated off (`MQK_ASSET_CLASS_*_ENABLED`, all default false); no broker/execution/GUI/tests. | SCAFFOLDED / DEFERRED BY DESIGN | 2 | GREEN (isolated) | E |

---

## 3. Fastest Completion Opportunities

1. **GUI operator-action 409 visibility** (`GUI-OPERATOR-ACTION-409-BODY-SURFACE-01`) — one file, one clear defect, GREEN, unlocks real operator-safety value (operators currently can't see *why* an arm/halt action was refused).
2. **CLI daemon control-plane passthrough** (`CLI-DAEMON-CONTROL-PASSTHROUGH-01`) — pure HTTP-passthrough subcommands, no new daemon logic, GREEN, closes the CLI/GUI operational-parity gap.
3. **Strategy engine unit tests** (3 patches, mean-reversion/volatility-breakout/swing-momentum) — mechanical, GREEN, closes a real coverage gap on strategies currently dispatchable in production with zero direct proof.
4. **README snapshot refresh** — docs-only, GREEN, trivial, prevents new operators from trusting a 3-week-stale status claim.
5. **Broker dead-code cleanup** (`client.rs`/`config.rs` in `mqk-broker-alpaca`) — deletion or explicit re-wiring, GREEN, removes confusing uncompiled duplicate code.
6. **Walk-forward promotion gate wiring** — M-sized but self-contained, GREEN, closes the single largest correctness gap in the research→promotion pipeline (overfitting protection currently exists only as an optional upstream Python step).
7. **Live account truth fix** (`LIVE-ACCOUNT-TRUTH-01`) — S-sized, unlocks correct buying-power reporting for both the eventual live path and (cosmetically) paper.
8. **Live-shadow smoke tooling** (`LIVE-TINY-CAPITAL-SMOKE-01`) — M-sized, GREEN, builds the evidence-accumulation tooling that the live trust-chain gate will eventually need as input, at zero capital risk.

---

## 4. Major Long-Lead Systems

1. **Live-capital trust-chain proof** (`LIVE-TRUST-CHAIN-*`) — genuinely requires a real shadow-execution capture pipeline, a parity scorer, and a signed evidence producer before `LiveCapital` cold-start can ever succeed. Not close; correctly gated off today.
2. **Multi-asset expansion (Options/Futures/Forex)** — currently at enum-variant-plus-stub-match-arm depth only. No contract metadata model, no broker adapter, no calendar, no GUI, no tests. Each is its own multi-quarter program; explicitly Lane E, post-soak, and each XL patch listed here **must** be decomposed into a real sub-patch sequence before implementation starts.
3. **Multi-asset — Crypto execution** — data ingestion is comparatively mature (15+ closure docs in `docs/specs/crypto_data_01*`) but execution wiring is zero; still a multi-patch program once started.
4. **`state.rs` / `lifecycle.rs` lean-out** — both files exceed 7,000 lines in the daemon's hottest path. Not urgent, but any attempt is inherently L/XL and must be decomposed (e.g., extract halt/deadman logic as its own module first) rather than attempted as one patch.
5. **CLI/daemon control-plane parity beyond passthrough** — the fast win (#2 in §3) is a thin passthrough; a fuller CLI-native operational surface (if ever desired) would be a longer program.

---

## 5. Master Patch Queue

Every patch below carries a stable ID, explicit status, priority, paper-impact color, lane, and the required template fields. Patches are grouped by lane for readability; the lane assignment is the single primary lane per §7.

### LANE A — Paper Soak (reproducible blockers / in-flight fences only)

#### PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01 — Fence stale local execution-loop tasks around clear-halted-run

**Status:** CLOSED
**Priority:** P0
**Paper Impact:** RED
**Subsystem:** Daemon supervisor / halt-clear control plane

**Current Source Truth:** Committed at `e44e3ddd6b41b32e5285436226100d2b867829b0` (`fix: require local quiescence before halt clear`), modifying `core-rs/crates/mqk-daemon/src/routes/control_plane.rs`, `state.rs`, `state/loop_runner.rs`, and adding test `h08_deadman_halt_cannot_be_cleared_under_live_local_loop` to `core-rs/crates/mqk-daemon/tests/scenario_clear_halted_run_auton04.rs:765` (registered in `scripts/test/ignored_test_inventory.csv` as `SAFE_DB_5434`). The change adds a `st.locally_owned_run_id().await == Some(run_id)` check to `clear-halted-run` (`control_plane.rs:1012-1067`), returning 409 `local_execution_loop_active` if a stale in-process execution-loop task can still be alive after a durable deadman halt commits. Root cause: the 120s deadman TTL (`DEADMAN_TTL_SECONDS`) can outlive the 90s runtime lease TTL (`orchestrator.rs:50`) by approximately 30 seconds, so lease expiry alone cannot prove same-process task quiescence — this creates a window where `clear-halted-run` could proceed while a stale task is still mid-exit.

**Problem:** An operator (or automated retry) calling `clear-halted-run` during that up-to-30s window could allow a stale execution-loop task to perform a late write (e.g., clobber a since-recovered ARMED state) after the halt was supposed to be final. The fix is written and appears internally consistent, but per `.claude/rules/audit_repo_truth_rules.md`, "scenario test file presence alone is not closure — a harness pass result is required," and this test has never been run.

**Why This Matters:** This is exactly the class of gap CLAUDE.md's fail-closed and idempotency invariants exist to prevent, and it sits directly in the paper-soak halt/recovery path.

**Dependencies:** NONE
**Unlocks:** Closes the last known race in the `PRE-SOAK-DAEMON-SUPERVISOR-HALT-FENCE-CLOSURE` lineage (prior entry `PRE-SOAK-DAEMON-SUPERVISOR-HALT-FENCE-CLOSURE-01` is CLOSED per commit `0a019b8b`; this is the next increment, not a reopening).

**In Scope:** Run `h08_deadman_halt_cannot_be_cleared_under_live_local_loop` against a real DB (`MQK_DATABASE_URL`, `--include-ignored`) exactly as written; if it passes, commit the five already-modified files together as one patch. If it fails, diagnose and repair within this same patch's scope (do not widen).
**Out of Scope:** Reconciling the underlying 90s/120s lease-TTL asymmetry at its root (tracked separately as `DEADMAN-LEASE-TTL-RECONCILE-01`) — this patch is a fence around the symptom, not a redesign of the TTLs themselves.
**Likely Files / Surfaces:** `core-rs/crates/mqk-daemon/src/routes/control_plane.rs`, `core-rs/crates/mqk-daemon/src/state.rs`, `core-rs/crates/mqk-daemon/src/state/loop_runner.rs`, `core-rs/crates/mqk-daemon/tests/scenario_clear_halted_run_auton04.rs`, `scripts/test/ignored_test_inventory.csv`.
**Required Implementation Rules:** Do not touch any file outside the five already modified. Do not weaken the existing H01-H07 tests in the same file. Do not alter halt-gate or tick-phase-ordering code as a side effect.
**Safety / Compatibility Requirements:** Must preserve all existing halt/clear/re-arm scenario coverage (H01-H07). Must not change behavior for the non-race-window case (normal clear-halted-run on a genuinely-exited run must still succeed exactly as before).
**Required Negative Controls:** `h08_deadman_halt_cannot_be_cleared_under_live_local_loop` (already written) proves refusal while the stale task is provably still alive.
**Required Positive Controls:** Existing H01-H07 plus the success-after-exit half of H08 (clear succeeds once the local task has genuinely finished).
**Required Regression Tests:** H01-H07 in the same file; `scenario_pdt_*`, `scenario_kill_switch_guarantees.rs` (halt-adjacent, must remain green).
**Required Validation:**
```powershell
$env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5434/mqk_test"
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_clear_halted_run_auton04 -- --include-ignored
cargo fmt --manifest-path .\core-rs\Cargo.toml -p mqk-daemon -- --check
git diff --check
```
**Forbidden Validation / Side Effects:** No live DB, no paper-soak production DB, no real Alpaca call, no push.
**Acceptance Criteria:**
1. `h08_deadman_halt_cannot_be_cleared_under_live_local_loop` passes against a real local Postgres.
2. H01-H07 remain green.
3. `cargo fmt --check` clean on the five touched files.
4. No files outside the five are modified.
**Exact CLOSED End State:** CLOSED when H08 has been run once against a real DB and passes, the five files are committed as a single patch, `scripts/test/ignored_test_inventory.csv`'s new row accurately reflects the committed test location, and no other in-flight uncommitted changes remain in the working tree touching halt/clear/deadman logic.
**Expected Handoff:** Start HEAD `0a019b8b...`; end HEAD = new commit SHA; files changed = the five listed; test run output pasted into the closure record; git status clean; not pushed.

**Implementation Commit:** `e44e3ddd6b41b32e5285436226100d2b867829b0`
**Independent Review:** ACCEPTED (`FINAL-CANONICAL-PRE-SOAK-VALIDATION-01`, 2026-08-10) — confirmed: the duplicate `DeadmanExpired` durable DISARM path was removed; same-daemon local task quiescence is required before halt clear; `h08_deadman_halt_cannot_be_cleared_under_live_local_loop` passed against a real local Postgres (`127.0.0.1:5434/mqk_test`) as part of the canonical safe-ignored matrix (733/733 tests green, 0 failures); post-exit clear/re-arm state cannot be overwritten by stale execution-loop code; crashed-prior-process recovery remains permitted; H01-H07 and all other accepted-list paper-soak scenario families remain green with no regression. No further accepted-list paper code blocker remains.
**Closure Commit / Accepted HEAD:** `e44e3ddd6b41b32e5285436226100d2b867829b0`
**Closure Date:** 2026-08-10

---

### LANE B — GREEN Parallel Completion (safe during soak)

#### RISK-AUTHORITY-DOC-NOTE-01 — Clarify risk-crate authority boundary in docs

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** mqk-risk
**Current Source Truth:** `mqk-risk/src/engine.rs:76-257` implements kill-switch/PDT/loss-limit/drawdown gates; per-symbol position caps and order-rate caps actually live in `mqk-daemon/src/state/loop_runner.rs:1221-1275`, not in `mqk-risk`.
**Problem:** `mqk-risk/src/lib.rs` has no doc note explaining this split, inviting future audits to mistake it for a gap.
**Why This Matters:** Prevents wasted future-session investigation cycles.
**Dependencies:** NONE · **Unlocks:** none
**In Scope:** One doc comment in `mqk-risk/src/lib.rs`. **Out of Scope:** Moving the caps into `mqk-risk`.
**Likely Files:** `core-rs/crates/mqk-risk/src/lib.rs`.
**Required Implementation Rules:** Doc-only change, no behavior change.
**Safety / Compatibility:** None applicable (docs only).
**Required Negative/Positive Controls:** NONE.
**Required Regression Tests:** `cargo test -p mqk-risk` unaffected.
**Required Validation:** `cargo fmt --manifest-path .\core-rs\Cargo.toml -p mqk-risk -- --check`.
**Forbidden Side Effects:** None beyond the doc comment.
**Acceptance Criteria:** 1) Doc comment added. 2) No `.rs` logic changed.
**Exact CLOSED End State:** CLOSED when the doc comment is committed and no logic file differs from HEAD except the comment.
**Expected Handoff:** Standard.
**Acceptance History:** Implementation Commit / Independent Review / Closure Commit / Closure Date — all PENDING.

#### PORTFOLIO-PLACEHOLDER-COMMENT-RENAME-01 — Remove false-positive "placeholder" wording

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** mqk-daemon portfolio routes
**Current Source Truth:** `routes/portfolio.rs:1136,1288` and `routes/paper_lifecycle.rs:577` contain the literal word "placeholder" in comments describing complete, non-stub logic (aggregator-routing internal name; an HTTP status-code choice).
**Problem:** These comments false-positive-match every future `grep placeholder` audit sweep.
**Dependencies:** NONE.
**In Scope:** Rename the three comments to avoid the word "placeholder" while preserving their explanatory content. **Out of Scope:** Any logic change.
**Likely Files:** `core-rs/crates/mqk-daemon/src/routes/portfolio.rs`, `routes/paper_lifecycle.rs`.
**Required Validation:** `cargo fmt --check` on both files; `git diff --check`.
**Acceptance Criteria:** 1) Comments reworded. 2) No logic diff. 3) `grep -i placeholder` on these two files returns zero hits.
**Exact CLOSED End State:** CLOSED when committed and the grep returns clean.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### PORTFOLIO-DYNAMIC-SELECTION-DEEP-REVIEW-01 — Targeted read-only review of dynamic_selection.rs

**Status:** READY · **Priority:** P2 · **Paper Impact:** GREEN (review produces no code change by itself) · **Subsystem:** mqk-portfolio
**Current Source Truth:** `mqk-portfolio/src/dynamic_selection.rs` is 3,680 lines and was the one file in the portfolio/P&L cluster not fully read during this audit pass (out of that pass's read budget). Everything else in the fill/P&L accounting path was proven complete.
**Problem:** Unknown whether this file contains any gap; classified UNKNOWN / REQUIRES EXTERNAL PROOF pending a dedicated read.
**Why This Matters:** It's the largest unreviewed file touching portfolio/selection accounting.
**Dependencies:** NONE. **Unlocks:** May spawn follow-up patches if a real gap is found.
**In Scope:** Read the file, classify per the standard taxonomy, and produce a short findings note appended to this ledger entry (or a new patch ID if a real defect is found). **Out of Scope:** Any code change in this patch.
**Likely Files:** `core-rs/crates/mqk-portfolio/src/dynamic_selection.rs`.
**Required Validation:** None (read-only).
**Acceptance Criteria:** 1) File fully read. 2) Classification recorded in this ledger with citations.
**Exact CLOSED End State:** CLOSED when the classification is recorded, whether or not it spawns a follow-up patch.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### BROKER-ALPACA-DEAD-CODE-CLEANUP-01 — Remove or wire orphaned client.rs/config.rs

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN (uncompiled, unreachable) · **Subsystem:** mqk-broker-alpaca
**Current Source Truth:** `mqk-broker-alpaca/src/client.rs` (`AlpacaHttpClient`) and `src/config.rs` (a second, differently-shaped `AlpacaConfig`) are not declared as `pub mod` in `lib.rs` — they do not compile into the crate and are unreachable from any caller. They also contain weaker error handling than the live path (e.g. `client.rs:18-19` silently swallows header-construction failure via `unwrap_or`).
**Problem:** Dead, confusing, duplicate code that could mislead a future session into thinking it's the live path.
**Dependencies:** NONE.
**In Scope:** Either delete both files, or wire them in and delete `lib.rs`'s duplicate logic if intentional — pick one, do not do both in one patch. **Out of Scope:** Adding any new functionality to whichever path is kept.
**Likely Files:** `core-rs/crates/mqk-broker-alpaca/src/client.rs`, `src/config.rs`, `src/lib.rs`.
**Required Validation:** `cargo build --manifest-path .\core-rs\Cargo.toml -p mqk-broker-alpaca`; `cargo clippy -p mqk-broker-alpaca --all-targets -- -D warnings`.
**Forbidden Side Effects:** No change to `normalize.rs`, `inbound.rs`, or any file in the proven live path.
**Acceptance Criteria:** 1) Crate compiles clean. 2) No duplicate `AlpacaConfig`/HTTP-client type remains unreferenced.
**Exact CLOSED End State:** CLOSED when the crate has exactly one HTTP client / config path, compiling and either used or deleted.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### BROKER-ALPACA-CRATE-SCOPE-DOC-01 — Document that WS transport lives outside the crate

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** mqk-broker-alpaca / mqk-daemon
**Current Source Truth:** Alpaca WS transport and gap-recovery (`alpaca_ws_transport.rs`, `ws_gap_recovery.rs`) actually live in `mqk-daemon/src/state/`, not in `mqk-broker-alpaca`, despite the crate's name suggesting it owns the full broker surface.
**Problem:** Architecture-scope mismatch could mislead future audits into assuming WS logic is colocated with REST/normalize logic.
**Dependencies:** NONE.
**In Scope:** One doc-comment addition at the top of `mqk-broker-alpaca/src/lib.rs` pointing to the actual WS transport location. **Out of Scope:** Moving any code.
**Likely Files:** `core-rs/crates/mqk-broker-alpaca/src/lib.rs`.
**Required Validation:** `cargo fmt --check`.
**Acceptance Criteria:** Doc comment present, no logic change.
**Exact CLOSED End State:** CLOSED when committed.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### MD-KRAKEN-FETCH-RETRY-BACKOFF-01 — Add bounded retry/backoff to Kraken fetch_bars

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN (Kraken is not in the live equity paper path) · **Subsystem:** mqk-md
**Current Source Truth:** `mqk-md/src/providers/kraken.rs:631-683` issues a single HTTP attempt per page with no retry on transient failure. `mqk-md/src/provider.rs:412-514` (TwelveData) already has a proven bounded-retry pattern (`provider.rs:1285`, test `rate_limit_retry_succeeds_after_one_body_429`).
**Problem:** A single dropped connection fails the whole Kraken poll cycle.
**Dependencies:** NONE. **Unlocks:** Establishes the pattern reusable by `MD-ALPACA-FETCH-RETRY-BACKOFF-01`.
**In Scope:** Port the TwelveData bounded-retry-on-transient-status pattern into `KrakenHistoricalProvider::fetch_bars`. **Out of Scope:** Any change to Kraken symbol/timeframe restrictions.
**Likely Files:** `core-rs/crates/mqk-md/src/providers/kraken.rs`.
**Required Regression Tests:** Existing Kraken ingest tests remain green.
**Required Validation:** `cargo test -p mqk-md`; `cargo clippy -p mqk-md --all-targets -- -D warnings`.
**Acceptance Criteria:** 1) Transient 5xx/timeout triggers bounded retry. 2) Existing tests pass. 3) No behavior change on success path.
**Exact CLOSED End State:** CLOSED when a negative-control test proves a transient-failure-then-success sequence now succeeds where it previously failed the cycle.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### STRATEGY-MEAN-REVERSION-UNIT-TESTS-01 — Add in-file signal-logic unit tests

**Status:** READY · **Priority:** P2 · **Paper Impact:** GREEN (pure signal-generation, no broker/DB/portfolio writes) · **Subsystem:** mqk-strategy
**Current Source Truth:** `mqk-strategy/src/engines/mean_reversion.rs:36-64` has zero in-file unit tests (only indirect reference-only coverage in `scenario_daily_data_readiness_01.rs`), unlike `intraday_scalper.rs` (43 in-file tests, `engines/intraday_scalper.rs:522-1259`).
**Problem:** A strategy currently dispatchable in production paper trading has no direct proof of its signal logic.
**Dependencies:** NONE.
**In Scope:** Unit tests covering entry/exit signal generation across representative bar sequences, mirroring the scalper's test pattern. **Out of Scope:** Any change to sizing, stops, or the signal algorithm itself.
**Likely Files:** `core-rs/crates/mqk-strategy/src/engines/mean_reversion.rs`.
**Required Validation:** `cargo test -p mqk-strategy`.
**Acceptance Criteria:** 1) At least the same order-of-magnitude test count as `intraday_scalper.rs` relative to code size. 2) All tests pass against unmodified logic (this patch adds tests only, changes zero behavior).
**Exact CLOSED End State:** CLOSED when the engine has direct unit-test proof of its documented entry/exit conditions.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### STRATEGY-VOLATILITY-BREAKOUT-UNIT-TESTS-01 — Add in-file signal-logic unit tests

**Status:** READY · **Priority:** P2 · **Paper Impact:** GREEN · **Subsystem:** mqk-strategy
**Current Source Truth / Problem / Scope:** Identical pattern to `STRATEGY-MEAN-REVERSION-UNIT-TESTS-01`, applied to `engines/volatility_breakout.rs:39-66` (prior-20-bar min/max breakout logic), currently zero in-file tests.
**Dependencies:** NONE.
**Likely Files:** `core-rs/crates/mqk-strategy/src/engines/volatility_breakout.rs`.
**Required Validation:** `cargo test -p mqk-strategy`.
**Acceptance Criteria:** Same as sibling patch.
**Exact CLOSED End State:** Same pattern as sibling patch.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### STRATEGY-SWING-MOMENTUM-UNIT-TESTS-01 — Add in-file signal-logic unit tests

**Status:** READY · **Priority:** P2 · **Paper Impact:** GREEN · **Subsystem:** mqk-strategy
**Current Source Truth / Problem / Scope:** Identical pattern, applied to `engines/swing_momentum.rs:36-64` (daily close-vs-20d-average momentum), currently zero in-file tests.
**Dependencies:** NONE.
**Likely Files:** `core-rs/crates/mqk-strategy/src/engines/swing_momentum.rs`.
**Required Validation:** `cargo test -p mqk-strategy`.
**Acceptance Criteria:** Same as sibling patches.
**Exact CLOSED End State:** Same pattern as sibling patches.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### STRATEGY-POSITION-SIZING-PARITY-01 — (DEFERRED) Port target_qty/notional caps to the other 3 engines

**Status:** DEFERRED · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** mqk-strategy
**Current Source Truth:** Only `intraday_scalper`/`intraday_short_scalper` have env-configurable `target_qty`/`max_target_qty`/`max_notional_usd` (`engines/intraday_scalper.rs`); the other three engines emit a fixed `{-1,0,1}` signal with no sizing configurability.
**Problem:** Not a defect (each engine documents its own fixed-size contract) — this is a capability gap, deferred by explicit operator decision pending a product decision on whether variable sizing is wanted for these strategies.
**Dependencies:** `STRATEGY-MEAN-REVERSION-UNIT-TESTS-01`, `STRATEGY-VOLATILITY-BREAKOUT-UNIT-TESTS-01`, `STRATEGY-SWING-MOMENTUM-UNIT-TESTS-01` (test coverage should land before behavior changes).
**In Scope:** One engine per follow-up patch if pursued — do not bundle. **Out of Scope:** All three engines in one patch (that would be L-sized).
**Exact CLOSED End State:** N/A while DEFERRED — reopen as three separate S-patches only on explicit operator decision.
**Acceptance History:** N/A (deferred, not started).

#### PROMOTION-WALKFORWARD-GATE-WIRING-01 — Wire walk-forward split proof into the Rust promotion gate

**Status:** READY · **Priority:** P1 · **Paper Impact:** GREEN (promotion output is a report artifact; no portfolio/risk/execution/broker writes) · **Subsystem:** mqk-promotion
**Current Source Truth:** `mqk-promotion/src/evaluator.rs::evaluate_promotion` has no field or check for in-sample/out-of-sample separation. Walk-forward split logic exists only in `research-py/src/mqk_research/scanner/walkforward.py`, `walkforward_runner.py`, `eval_walkforward.py` — not consumed by the Rust gate. A single-period backtest can currently pass every Rust promotion gate (NaN, tie-break, artifact-lock, stress-suite, provenance) with zero walk-forward proof.
**Problem:** Overfitting protection is optional and upstream-only, not enforced at the authoritative promotion boundary.
**Why This Matters:** This is the single largest correctness gap in the research→promotion pipeline; it directly affects the credibility of any strategy ever promoted.
**Dependencies:** NONE. **Unlocks:** Strengthens every future promotion decision.
**In Scope:** Add a new required field to `PromotionInput` (e.g. `walk_forward_evidence: Option<WalkForwardEvidence>`) mirroring the existing "`None` blocks promotion" pattern used for `artifact_lock` (Patch B6) and `stress_suite` (Patch B2); wire `research-py`'s walk-forward output to populate it. **Out of Scope:** Changing the walk-forward algorithm itself, changing any other gate.
**Likely Files / Surfaces:** `core-rs/crates/mqk-promotion/src/evaluator.rs`, `src/types.rs`, `research-py/src/mqk_research/scanner/walkforward_runner.py`, whatever CLI/daemon route currently constructs `PromotionInput` (`core-rs/crates/mqk-daemon/src/routes/strategy_promotions.rs`).
**Required Implementation Rules:** Follow the exact `Option<T>` + fail-closed-if-`None` pattern already established by B2/B6 — do not invent a new gate-failure convention.
**Safety / Compatibility Requirements:** Must not change behavior for already-promoted strategies retroactively; must not allow a `None` walk-forward field to be silently defaulted to "pass."
**Required Negative Controls:** A promotion input with `walk_forward_evidence: None` must fail exactly like a `None` `artifact_lock` does today (mirror `scenario_promotion_requires_partial_fill_stress.rs`).
**Required Positive Controls:** A promotion input with valid walk-forward evidence proceeds through the remaining gates unchanged.
**Required Regression Tests:** `scenario_nan_metric_fails_promotion.rs`, `scenario_tie_break_correctness.rs`, `scenario_golden_artifact_hash_lock.rs` all remain green.
**Required Validation:** `cargo test -p mqk-promotion`; Python: `pytest research-py/tests -k walkforward` if such tests exist.
**Forbidden Validation / Side Effects:** No real broker call, no live/paper DB write.
**Acceptance Criteria:** 1) `PromotionInput` carries the new field. 2) `None` fails closed with a new dedicated reason code. 3) All existing promotion gate tests remain green. 4) A new negative-control test proves closure.
**Exact CLOSED End State:** CLOSED when no strategy can be promoted without walk-forward evidence attached, proven by a failing-then-passing test pair, with all pre-existing promotion tests green.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### DYNAMIC-SELECTION-MODULE-DOC-STALENESS-01 — Correct stale "NOT WIRED" doc header

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** mqk-daemon dynamic selection
**Current Source Truth:** `mqk-daemon/src/state/multi_symbol_config.rs`'s module doc says "NOT WIRED — this patch only," but current callers (`daily_data_readiness.rs`, `autonomous_daily_coordinator.rs`, `state/lifecycle.rs::StartAttemptAuthoritySnapshot`, and `state.rs:3558-3583`/`state/loop_runner.rs:1018-1021`) do consume it in the live dispatch path.
**Problem:** Stale doc contradicts current reality, risking a future session mis-scoping a patch around it.
**Dependencies:** NONE.
**In Scope:** Update the module doc to reflect actual wiring status. **Out of Scope:** Any logic change.
**Likely Files:** `core-rs/crates/mqk-daemon/src/state/multi_symbol_config.rs`.
**Required Validation:** `cargo fmt --check`.
**Acceptance Criteria:** Doc accurately states the module is wired and lists its actual callers.
**Exact CLOSED End State:** CLOSED when committed.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### DYNAMIC-SELECTION-E2E-SCENARIO-TEST-01 — Add dedicated end-to-end dynamic-selection proof

**Status:** READY · **Priority:** P2 · **Paper Impact:** GREEN (adds a test only) · **Subsystem:** mqk-daemon dynamic selection
**Current Source Truth:** Only one scenario test file is directly named for dynamic-selection evidence (`scenario_dynamic_selection_evidence_validation_01.rs`) against ~5,000+ lines of source across `dynamic_selection_plan_builder.rs`, `dynamic_selection_dispatch_authority.rs`, `dynamic_selection_host_pool.rs`, `dynamic_selection_evidence_validator.rs`, `dynamic_selection_start_gate.rs`, `dynamic_selection_mode.rs`. Additional coverage may exist as embedded `#[cfg(test)]` modules inside `state.rs`/`dynamic_selection_plan_builder.rs`, not yet confirmed.
**Problem:** Unclear whether dedicated integration-level coverage matches the size of the source surface.
**Dependencies:** `DYNAMIC-SELECTION-TEST-DENSITY-AUDIT-01` should run first to avoid duplicating existing embedded coverage.
**In Scope:** After the density audit, add one `scenario_dynamic_selection_end_to_end_paper_dispatch_01.rs` exercising plan build → host pool selection → selected-host dispatch → evidence write → evidence-route read as a single integration proof, only if not already covered. **Out of Scope:** Any production code change.
**Likely Files:** `core-rs/crates/mqk-daemon/tests/scenario_dynamic_selection_end_to_end_paper_dispatch_01.rs` (new).
**Required Validation:** `cargo test -p mqk-daemon --test scenario_dynamic_selection_end_to_end_paper_dispatch_01`.
**Acceptance Criteria:** New test exists and passes, or the density audit concludes existing coverage is already sufficient (in which case this patch closes as "no new test needed, coverage confirmed").
**Exact CLOSED End State:** CLOSED when coverage is either added or confirmed sufficient, with the finding recorded in this ledger.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### DYNAMIC-SELECTION-TEST-DENSITY-AUDIT-01 — Verify embedded unit-test coverage for dispatch-authority/host-pool

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN (review only) · **Subsystem:** mqk-daemon dynamic selection
**Current Source Truth:** `dynamic_selection_dispatch_authority.rs` (837 lines) and `dynamic_selection_host_pool.rs` (393 lines) have no dedicated `scenario_*` file by name; coverage may live in embedded `#[cfg(test)]` modules not yet confirmed read.
**In Scope:** Read and catalog existing test coverage for these two files. **Out of Scope:** Writing new tests (feeds into `DYNAMIC-SELECTION-E2E-SCENARIO-TEST-01`).
**Likely Files:** `core-rs/crates/mqk-daemon/src/dynamic_selection_dispatch_authority.rs`, `dynamic_selection_host_pool.rs`.
**Required Validation:** None (read-only).
**Acceptance Criteria:** Coverage catalog recorded in this ledger entry.
**Exact CLOSED End State:** CLOSED when the catalog is recorded, regardless of outcome.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### MULTI-SYMBOL-DISPATCH-DOC-CONCURRENCY-CLARITY-01 — Document sequential-per-tick dispatch semantics

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** mqk-daemon multi-symbol dispatch
**Current Source Truth:** `state.rs:3529` (`for assignment in assignments { ... .await ... }`) dispatches symbols sequentially within a single tokio task, not in parallel — this is deterministic-by-construction (a good property) but `docs/design/native_multi_symbol_dispatch.md` (if it uses the word "concurrent") may overstate parallelism.
**Problem:** Documentation/terminology mismatch risks a future session assuming true concurrency exists when it doesn't.
**In Scope:** Update the design doc to explicitly state sequential-per-tick semantics and why (determinism). **Out of Scope:** Changing the dispatch model itself.
**Likely Files:** `docs/design/native_multi_symbol_dispatch.md` (verify exact path first).
**Required Validation:** None (docs only).
**Acceptance Criteria:** Doc no longer implies parallel dispatch where sequential is the actual and intended behavior.
**Exact CLOSED End State:** CLOSED when committed.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### CLI-DAEMON-CONTROL-PASSTHROUGH-01 — Add thin CLI passthrough to daemon operator-safety routes

**Status:** READY · **Priority:** P1 · **Paper Impact:** GREEN (pure HTTP passthrough, zero new daemon logic) · **Subsystem:** mqk-cli
**Current Source Truth:** `mqk-cli/src/main.rs:44-88` has no `Daemon`/`Control` command. `RunCmd` (`main.rs:846-972`) operates on the generic `mqk-db` `runs` table directly, not via the live daemon's HTTP control-plane routes (`/v1/run/start`, `/v1/run/stop`, `/v1/run/halt`, `/v1/integrity/arm`, `/v1/integrity/disarm`, `/api/v1/ops/action`, `routes.rs:781-791`). An operator cannot arm/halt/clear the actual running autonomous daemon from the CLI — only via the HTTP API (GUI or curl).
**Problem:** No incident-response CLI path to the live daemon's safety surface.
**Why This Matters:** Incident response should not depend on the GUI being reachable.
**Dependencies:** NONE. **Unlocks:** `CLI-RUNCMD-DOC-DISAMBIGUATION-01` (clarifying which command touches what).
**In Scope:** Add `mqk daemon status|arm|disarm|halt|clear-halted-run` subcommands that call the existing daemon HTTP routes with no new daemon-side logic — a pure HTTP client wrapper. **Out of Scope:** Any change to the daemon routes themselves, any new authorization logic (reuse whatever the routes already require).
**Likely Files / Surfaces:** `core-rs/crates/mqk-cli/src/main.rs`, new `commands/daemon.rs`.
**Required Implementation Rules:** Must not touch `mqk-daemon` route handlers at all — this is a pure client addition. Must surface the daemon's actual response body (including 409 `blockers`) to the terminal, not swallow it (mirrors the GUI fix in `GUI-OPERATOR-ACTION-409-BODY-SURFACE-01` — do not repeat that mistake in the CLI).
**Safety / Compatibility Requirements:** Should require the same confirmation/flag discipline as existing destructive CLI commands (e.g., an explicit `--confirm` for `halt`).
**Required Negative Controls:** A 409 from the daemon must print the real blocker reason, not a generic failure message.
**Required Positive Controls:** `mqk daemon status` against a running daemon returns real state.
**Required Regression Tests:** Existing `RunCmd` tests unaffected (different command tree).
**Required Validation:** `cargo build -p mqk-cli`; `cargo clippy -p mqk-cli --all-targets -- -D warnings`; manual smoke against a locally running daemon (no real broker calls).
**Forbidden Validation / Side Effects:** No real Alpaca call; no push.
**Acceptance Criteria:** 1) New subcommands exist and compile. 2) 409 responses surface real blocker text. 3) No daemon-side file is touched.
**Exact CLOSED End State:** CLOSED when an operator can run `mqk daemon halt`/`clear-halted-run`/`arm`/`disarm`/`status` from the CLI and see the daemon's real structured response, with zero daemon-side logic changes.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### CLI-RUNCMD-DOC-DISAMBIGUATION-01 — Clarify RunCmd touches the generic runs table, not the live daemon

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** mqk-cli
**Current Source Truth:** `mqk-cli/src/main.rs:846-972` (`RunCmd::Start/Arm/Stop/Halt/...`) operates directly on the `mqk-db` `runs` table, which is a different code path from the live daemon's HTTP control-plane routes.
**Problem:** An operator could mistake `mqk run halt` for a live-daemon-halt action during an incident, when it is not.
**Dependencies:** Best done alongside or after `CLI-DAEMON-CONTROL-PASSTHROUGH-01` so the doc can point to the correct alternative.
**In Scope:** Update `RunCmd`'s CLI help text and doc comments to state explicitly it touches the DB `runs` table directly, and point to `mqk daemon halt` (once it exists) for live-process control. **Out of Scope:** Any behavior change to `RunCmd`.
**Likely Files:** `core-rs/crates/mqk-cli/src/main.rs`, `commands/run.rs`.
**Required Validation:** `cargo build -p mqk-cli`.
**Acceptance Criteria:** Help text and doc comments are unambiguous about scope.
**Exact CLOSED End State:** CLOSED when committed.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### CI-TESTKIT-FEATURE-GUARD-VERIFY-01 — Verify testkit feature never ships in release builds

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** CI / mqk-cli
**Current Source Truth:** `mqk-cli/src/commands/run.rs:405,464,485` and `main.rs:27` document real, intentional test-only stub wiring (`NullBroker`, always-pass gate stub) gated by `#[cfg(feature = "testkit")]`. No existing CI job was confirmed in this audit pass to explicitly assert `testkit` is absent from release builds.
**Problem:** If the feature gate were ever misconfigured, a stub broker could theoretically ship in a production build. Currently believed safe, not confirmed via an explicit guard.
**In Scope:** Verify whether `.github/workflows/ci.yml`'s `guards` job already checks this; if not, add a minimal guard script assertion. **Out of Scope:** Any change to the stub code itself.
**Likely Files:** `.github/workflows/ci.yml`, `scripts/guards/`.
**Required Validation:** Run the relevant guard script locally if one exists; otherwise add one and run it.
**Acceptance Criteria:** A CI guard fails if `testkit` feature is enabled in a release-profile build.
**Exact CLOSED End State:** CLOSED when the guard exists (or is confirmed to already exist) and demonstrably fails on a deliberately-misconfigured build.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### CLI-RUN-STUB-TRACKING-01 — Convert untracked "replace stubs before LIVE" comment into a tracked patch

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** mqk-cli
**Current Source Truth:** `mqk-cli/src/commands/run.rs:485` contains a standing comment "Replace stubs with real implementations before LIVE deployment" with no tracking issue/patch ID.
**Problem:** An untracked reminder comment is easy to miss before an eventual live cutover decision.
**Dependencies:** Related to `LIVE-CLI-ARM-RECONCILE-01` (Lane C) which investigates whether this CLI path is even the live-relevant one.
**In Scope:** Replace the bare comment with a reference to this ledger's live-readiness section, or a dedicated tracked patch ID once `LIVE-CLI-ARM-RECONCILE-01` determines if this path matters for live. **Out of Scope:** Implementing the actual stub replacement (that's live-gated work).
**Likely Files:** `core-rs/crates/mqk-cli/src/commands/run.rs`.
**Required Validation:** `cargo fmt --check`.
**Acceptance Criteria:** Comment now references a real ledger entry instead of being a dangling reminder.
**Exact CLOSED End State:** CLOSED when committed.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### GUI-OPERATOR-ACTION-409-BODY-SURFACE-01 — Surface real daemon conflict reasons to the operator

**Status:** READY · **Priority:** P1 · **Paper Impact:** GREEN (GUI transport-layer fix only; does not touch the Rust backend/trading path) · **Subsystem:** mqk-gui
**Current Source Truth:** `core-rs/mqk-gui/src/features/system/http.ts:110-117` (`postJson`) discards the HTTP response body on any non-2xx status, setting only `error: "HTTP ${status}"`. The daemon (`control_plane.rs:783-805,906-908` and 9 other `StatusCode::CONFLICT` sites) returns a structured `OperatorActionResponse` on 409 with real `blockers: [...]` explanation text, but `actions.ts::failedOperatorActionReceipt` (`actions.ts:116-151`) can only synthesize a generic message because the body was never parsed. `ActionReceiptBanner.tsx:13` never displays the real reason.
**Problem:** Direct breach of `gui_rules.md` rule 3 ("A 409 response must carry an explanation the operator can act on") — the backend supplies it, the transport layer drops it.
**Why This Matters:** An operator retrying a blind "failed" action without seeing the real reason (e.g., "pending restart intent already exists") is exactly the operator mistake the rule exists to prevent.
**Dependencies:** NONE. **Unlocks:** Improves the reliability signal `CLI-DAEMON-CONTROL-PASSTHROUGH-01` should also follow (don't repeat the same mistake in the CLI).
**In Scope:** In `http.ts`, on `!response.ok`, attempt `await response.json()` (guarded by content-type check / try-catch) and attach it as `data`/`errorBody` on the returned `EndpointPostResult`; in `actions.ts::failedOperatorActionReceipt`, prefer `payload.blockers`/`disposition` text over the generic message when present. **Out of Scope:** Any backend route change — the daemon already returns the right body; only the client needs fixing.
**Likely Files / Surfaces:** `core-rs/mqk-gui/src/features/system/http.ts`, `src/features/system/actions.ts`, `src/features/system/types.ts` (`OperatorActionReceipt`), `src/features/system/ActionReceiptBanner.tsx`.
**Required Implementation Rules:** Must not change backend logic or API contracts. Must not weaken GUI fail-closed behavior on genuinely malformed/absent bodies (a 409 with no parseable body should still show a clear "unavailable" message, not crash or fabricate an empty-success state).
**Safety / Compatibility Requirements:** Must not swallow non-2xx responses; must not degrade the existing hard-block truth-state discipline elsewhere in the GUI.
**Required Negative Controls:** New unit test asserting a 409 with a JSON `blockers` array reaches `OperatorActionReceipt.blocking_failures` verbatim (per the agent-proposed test).
**Required Positive Controls:** A 200 success path is unaffected.
**Required Regression Tests:** Existing GUI test suite (`npm test -- --run`) remains green, especially `SettingsScreen.test.ts` and any existing `actions.test.ts`.
**Required Validation:**
```powershell
cd core-rs\mqk-gui
npm test -- --run
npm run build
cd ..\..
git diff --check
```
**Forbidden Validation / Side Effects:** No backend change, no live/paper trading behavior change.
**Acceptance Criteria:** 1) 409 body is parsed and attached. 2) `ActionReceiptBanner` displays real `blockers` text when present. 3) New regression test passes. 4) `npm run build` succeeds. 5) No backend file is touched.
**Exact CLOSED End State:** CLOSED when an operator triggering a blocked action (e.g., arm while reconcile dirty) sees the daemon's actual reason text in the GUI, proven by the new test, with the existing GUI suite green.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### README-SNAPSHOT-REFRESH-01 — Update or de-embed the stale repository snapshot

**Status:** READY · **Priority:** P2 · **Paper Impact:** GREEN · **Subsystem:** Documentation
**Current Source Truth:** `README.md:46-60` carries a "Repository snapshot used for this update (2026-07-20)" pinned to commit `3591064a`, describing Phase D/E1/E2A status. Current HEAD (`0a019b8b`, 2026-08-10) has moved through five additional closure/fix commits not mentioned (`PRE-SOAK-DAEMON-SUPERVISOR-HALT-FENCE-CLOSURE-01`, `PAPER-SOAK-ALPACA-TRADE-ACTIVITY-SCHEMA-01`, `PAPER-SOAK-PARTIAL-FILL-DEDUP-04`, and others).
**Problem:** The README is the first doc an external reader or new operator trusts; it's materially stale on soak-readiness claims.
**Dependencies:** NONE.
**In Scope:** Update the snapshot section to current HEAD and current soak status, or replace the embedded snapshot with a pointer to this ledger (which `.claude/rules/audit_repo_truth_rules.md` already establishes as the pattern to avoid re-staling — "no stale snapshots in living docs"). **Out of Scope:** Any other README content changes.
**Likely Files:** `README.md`.
**Required Validation:** None beyond visual review; `git diff --check`.
**Acceptance Criteria:** 1) Snapshot date matches or is replaced by a pointer to a living source. 2) No other README content altered.
**Exact CLOSED End State:** CLOSED when committed and the snapshot no longer references a 3-week-old commit as current.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### DEPLOYMENT-DECISION-DOC-01 — Document the no-container deployment decision

**Status:** READY · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** Documentation / Config
**Current Source Truth:** No Docker/docker-compose files exist anywhere in the repo. This may be intentional (single-operator desktop app via the Tauri GUI shell) but is currently undocumented as a decision.
**Problem:** Ambiguous whether the absence is a gap or a deliberate choice.
**In Scope:** Add `docs/DEPLOYMENT.md` stating the decision and rationale (local-process-only, no container path) explicitly. **Out of Scope:** Building an actual Dockerfile (that would be a separate, larger, explicitly-requested patch if the decision is later reversed).
**Likely Files:** `docs/DEPLOYMENT.md` (new).
**Required Validation:** None.
**Acceptance Criteria:** Doc exists and states the decision unambiguously.
**Exact CLOSED End State:** CLOSED when committed.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### OFFICIAL-DUAL-MODE-LAUNCHER-01 — Official Paper/Live dual-mode launcher (scripts/windows/Start-MiniQuantDesk.ps1)

**Status:** IMPLEMENTED_PENDING_REVIEW (repaired, still not accepted) · **Priority:** P2 · **Paper Impact:** GREEN (orchestration script + one narrow `-SkipGui` addition to `Launch-VeritasLedger.ps1`; zero Rust/Python trading code touched) · **Subsystem:** Ops tooling / operator launcher
**Current Source Truth:** Built entirely in the isolated worktree `C:\Users\Zacha\Desktop\MiniQuantDeskV4-ops` on branch `ops-official-launcher`, forked from the protected paper-soak baseline `54082a448c84b6429713a429bfb9403da8822131`. Original implementation landed at commit `aead3420974ba1bdf493344f57e9c519ee764c0e`.
**OFFICIAL-DUAL-MODE-LAUNCHER-01-REPAIR-01 (this update):** independent review found four verified defects in the `aead3420` implementation; all four are repaired in this same worktree/branch, still not merged/accepted. Repaired implementation commit: see `git -C C:\Users\Zacha\Desktop\MiniQuantDeskV4-ops log -1 --format=%H -- scripts/windows/Start-MiniQuantDesk.ps1 scripts/windows/tests/test_official_dual_mode_launcher.ps1` (this ledger update is committed in the same commit as the code/test repair, so its own SHA cannot be self-embedded — the commit titled "fix: harden official paper launcher readiness" on branch `ops-official-launcher` is authoritative).
- **Defect 1 (arm not guaranteed):** previously `-ArmPaper` was required to arm; official full Paper startup (interactive and `-Scheduled`) now *always* reaches an unconditional arm-execution stage after all upstream gates pass, with bounded (6× 500ms) authoritative re-verification against `GET /api/v1/autonomous/readiness`'s `arm_state`. Only `arm_state=="armed"` is accepted as success — `arm_pending` is deliberately treated as *not sufficient*, because `mqk-daemon/src/routes/system.rs` returns `"arm_pending"` both when the durable DB row is truly `ARMED` (self-heal in progress) and when the DB row is missing/unreadable, so the two cases are indistinguishable from the launcher's vantage point. `CheckOnly` still never reaches the arm section (unchanged). `-ArmPaper` is retained as a backward-compatible no-op. Rust source was independently verified (via source research, not modified) to already enforce arm-before-start ordering: `start_execution_runtime` (`mqk-daemon/src/state/lifecycle.rs:705`) refuses whenever `integrity.disarmed || integrity.halted`, and both the legacy `session_controller.rs` and production `autonomous_daily_coordinator.rs` call `try_autonomous_arm_typed` before `start_execution_runtime`, never the reverse — no Rust change was needed or made.
- **Defect 2 (parent env loading):** the parent `Start-MiniQuantDesk.ps1` process previously never loaded `.env.local` itself — only the child `powershell.exe` running `Launch-VeritasLedger.ps1` did, and child-process environment mutations do not propagate to the parent. A `.env.local`-only `MQK_OPERATOR_TOKEN` could therefore make the official launcher fail after the daemon had already started successfully. Fixed by giving the parent its own copy of the same safe env-loading logic (`Import-LauncherEnvironmentFiles`/`Import-DotEnvIfPresent`/`Parse-DotEnvLine`/`Get-EnvValue`, invoked at the very start of main dispatch), with identical semantics to `Launch-VeritasLedger.ps1`'s existing implementation (quoted-value handling, process-env-wins, `.env.local`/`.env` support, Process→User→Machine fallback, no secret values ever printed).
- **Defect 3 (session refresh duration):** independent source research proved `GET /api/v1/system/session`'s `session_stop_utc` field the launcher previously parsed **does not exist at all** on `SessionStateResponse` — the old `-split ':'` branch was dead code, always falling back to a hardcoded 1800s (30-minute) refresh loop regardless of actual session length. Replaced with `GET /api/v1/market-data/readiness`, which serves an authoritative, DST-correct, NYSE-calendar-derived `session_close_utc` (RFC3339) plus `calendar_coverage_state`. Refresh duration = `session_close_utc + 15min - now`, floored at 300s for legitimate near/after-close launches (not a truth-unavailable fallback). When `calendar_coverage_state != "active"` or `session_close_utc` is absent, the launcher fails closed with `ExitDataReadiness` (3) for **both** `-Scheduled` and interactive full startup (the mission text mandated this for `-Scheduled`; extended to interactive too per this repo's fail-closed doctrine) — no silent 1800s fallback remains anywhere in the file.
- **Defect 4 (startup prerequisites):** the official launcher previously delegated daemon startup to `Launch-VeritasLedger.ps1` without independently owning Docker/paper-Postgres-container/migration prerequisites the way the accepted `Start-PaperTradingSmoke.ps1` does. Added a narrow, inline `Invoke-PaperDbPrerequisites` function (Docker available+running check, `mqk-paper-postgres` container inspect/start, `pg_isready` retry loop, `MQK_DATABASE_URL` hard-reasserted to `postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable`, then `sqlx`/`cargo sqlx migrate run`), called only on the full-startup path (never in `-CheckOnly`) immediately before `Launch-VeritasLedger.ps1` is invoked. Implemented inline rather than by delegating to `Start-PaperTradingSmoke.ps1` because that script also stops stale processes and starts its own daemon, which would create a second competing daemon-startup authority alongside `Launch-VeritasLedger.ps1`.

**OFFICIAL-DUAL-MODE-LAUNCHER-01-REPAIR-02 (this update):** independent GitHub review of the REPAIR-01 commit (`f83cb9d418751b5bad1528bf3d84b2154f14f7e3`) found two further integration defects; both repaired in this same worktree/branch, still not merged/accepted.
- **Defect A (pre-open circularity):** REPAIR-01 unconditionally requested `Launch-VeritasLedger.ps1 -Mode TradeReady` before performing its own reconcile/halt-recovery/arm-execution work. `Launch-VeritasLedger.ps1`'s `TradeReady` mode (`Get-TradeReadinessReasons`) requires `arm_ready`, `session_in_window`, `runtime_start_allowed`, and `overall_ready` to *already* be true — but this launcher only establishes those itself, *after* `Launch-VeritasLedger.ps1` returns. Before market open, `session_in_window` is expected to be `false`, so a pre-open `-Scheduled -Mode Paper` run could never pass the daemon-bootstrap stage at all. Fixed by changing the daemon-bootstrap call to `Launch-VeritasLedger.ps1 -Mode Observe` (the script's own default), which only requires `Get-BackendProbe`'s `IdentityVerified` gate — verified canonical paper+alpaca identity, valid operator auth, `live_routing_enabled=false`, daemon reachable — exactly the contract this launcher needs before performing its own readiness chain (ingest-plan → market-data prep → reconcile → halt recovery → arm-execution → verified `arm_state=="armed"`). `Launch-VeritasLedger.ps1`'s own `TradeReady` mode/definition is completely unchanged (still available directly for operator diagnostics); only the launcher's own daemon-bootstrap call site changed. A full non-CheckOnly Paper startup now returns success pre-open with `daemon_verified=true, market_data_ready=true, reconcile_ready=true, arm_state=armed, session_in_window=false, runtime_status=idle` — the autonomous session controller (unchanged, out of scope) starts the runtime later at the correct session-window boundary. `start-system` is still never called by this launcher.
- **Defect B (refresh-loop duplication risk):** REPAIR-01 unconditionally `Start-Process`'d a new background `Refresh-IntradayMarketData.ps1` loop on every full Paper startup with no ownership tracking, so a Task Scheduler retry after a later-stage failure (reconcile/halt-recovery/arm) could stack a second refresh loop for the same symbol/timeframe/Paper-DB/market-date scope. Fixed by adding `Get-IntradayRefreshOwnerPath` / `Test-RefreshOwnerProcessAlive` / `Get-IntradayRefreshOwnerState` / `Set-IntradayRefreshOwnerRecord`: before starting a refresh child, the launcher checks a narrow ownership record at `smoke_logs\launcher\paper\intraday_refresh_owner.json` (untracked runtime evidence, same convention as `New-LauncherLog`'s `smoke_logs\launcher\<mode>\launch_*.json`). A recorded owner is reused only when its PID is still alive, still looks like a launcher-managed `Refresh-IntradayMarketData.ps1` PowerShell process (`Get-CimInstance Win32_Process` command-line check, with a safe process-name-only fallback if CIM is unavailable), and its recorded repo-root/symbols/timeframe/paper-DB-port/market-date scope matches exactly. No process is ever killed by these checks — a dead or scope-mismatched owner is simply not reused, and exactly one replacement process is started and recorded. The record contains only non-secret facts (`pid`, `started_at_utc`, `market_date`, `symbols`, `timeframe`, `paper_db_port`, `repo_root`). As part of this fix the refresh-loop stage was also reordered per mission section 11 to run *after* arm verification (previously it ran between market-data-prep and reconcile) so a long-lived child is not spawned before as many prerequisites as practical have already been proven; the new full-startup order is DB prerequisites → daemon verified (Observe) → symbol/data prep → reconcile → halt recovery → arm verified → authoritative session-close duration → start/reuse refresh loop → success.
- To make the ownership functions independently testable without starting a real daemon or trading runtime, `MAIN DISPATCH` is now guarded by `if ($MyInvocation.InvocationName -ne '.') { ... }` — dot-sourcing the script (as the test file now does) loads every function, including the new ownership helpers, without executing the interactive/Live/Paper dispatch, spawning a daemon, or calling `exit`. Normal `powershell.exe -File Start-MiniQuantDesk.ps1` invocation is unaffected (its `InvocationName` is never `.`).

**Problem:** No single official entrypoint existed for starting MiniQuantDesk; operators had to know whether to run `Launch-VeritasLedger.ps1` or `Start-PaperTradingSmoke.ps1`, and no Live-mode surface existed at all. (REPAIR-01 closed the four original defects; REPAIR-02 closed two further pre-open/idempotency integration defects found by independent review of the REPAIR-01 commit.)
**Dependencies:** NONE.
**In Scope:** Interactive Paper/Live menu; explicit `-Mode`/`-CheckOnly`/`-Scheduled`/`-ArmPaper` (legacy no-op) CLI surface; `-Scheduled` with no `-Mode` fails closed (`STARTUP_REFUSED`, exit 2); Paper full-run now owns DB/Docker/migration prerequisites, delegates daemon/GUI bootstrap to `Launch-VeritasLedger.ps1 -Mode Observe` (REPAIR-02 — no longer `TradeReady`, which was pre-open-circular), resolves the authoritative symbol universe via `GET /api/v1/market-data/ingest-plan` + `Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan`, runs the broker-baseline-adopt + reconcile hard gate, runs halt recovery (disarm→clear-halted-run) if needed, always arms and verifies `arm_state=="armed"`, then starts or idempotently reuses (REPAIR-02) an authoritative full-session-length `Refresh-IntradayMarketData.ps1` background loop via ownership-file tracking (fail-closed if session-close truth is unavailable), and never calls the `start-system` action_key — runtime start authority stays with the autonomous session controller. Live mode unchanged by REPAIR-01/REPAIR-02 (out of scope): seven read-only/source-guard preflight checks that dynamically read `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and `research-py/src/mqk_research/deployment/parity.py`; interactive non-CheckOnly Live requires a typed `LIVE` confirmation; Live never starts a process, calls a broker, or mutates a DB. **Out of Scope (explicitly not done):** Windows Task Scheduler registration (`PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01` stays BLOCKED); any Rust lifecycle change; any Live behavior expansion; any change to `live_trust_complete`, broker trust rules, live reconciliation, live risk, live execution, shadow parity, evidence signing, or live capital authorization; any change to `Launch-VeritasLedger.ps1`'s own `TradeReady` mode definition.
**Likely Files / Surfaces:** `scripts/windows/Start-MiniQuantDesk.ps1` (repaired twice: REPAIR-01, REPAIR-02), `scripts/windows/Launch-VeritasLedger.ps1` (unchanged since the original patch — REPAIR-01's narrow `-SkipGui` addition is its only delta, REPAIR-02 touched nothing here), `scripts/windows/tests/test_official_dual_mode_launcher.ps1` (repaired, +25 REPAIR-01 assertions, +27 REPAIR-02 assertions), this ledger.
**Required Implementation Rules:** One patch, minimal scope, no bundling with any Rust/Python change; built and committed only in the isolated `-ops` worktree; the protected paper-soak `main` worktree was never checked out to another branch, never had a new branch created inside it, and received zero commits from this session.
**Safety / Compatibility Requirements:** `-CheckOnly` never arms, clears halt, starts runtime, submits orders, mutates DB, runs migrations, starts/mutates Docker containers, launches broker activity, or creates an active refresh-ownership record (proven by guard-test Section 1/3/5 CheckOnly-scope checks + Section 2 real invocation + real `-CheckOnly` run in this session showing zero mutation and no ownership file created). Live mode never enables live routing, never sets `MQK_DAEMON_DEPLOYMENT_MODE`/`MQK_DAEMON_ADAPTER_ID` to a live value, and never prints `ALPACA_API_KEY_LIVE`/`ALPACA_API_SECRET_LIVE` values. `-Scheduled -Mode Live` fails closed (`unattended_live_start_not_authorized`, exit 6). Paper DB hard fence: `MQK_DATABASE_URL` is always reasserted to `127.0.0.1:5440/miniquantdesk_paper`, never `5432`/`5434`. Refresh-ownership checks never call `Stop-Process`/kill any process, arbitrary or otherwise (proven by static source guard + a real unrelated fixture PowerShell process surviving the check in this session).
**Required Negative Controls:** `-Scheduled` with no `-Mode` → exit 2 (proven). `-Mode Live -Scheduled` → exit 6, no interactive prompt (proven). `-Mode Live -CheckOnly` → completes without hanging on stdin, reports BLOCKED with real ledger patch IDs (proven). Unavailable session-close truth → `ExitDataReadiness` (3), never a 1800s fallback (proven via static guard; no live daemon available in this worktree to prove the dynamic branch end-to-end this session). Mismatched refresh-ownership scope (symbols/timeframe/market-date) → never silently reused (proven via real fixture-process functional test). Stale/dead refresh-owner PID → never reused (proven via real fixture-process functional test with an intentionally-invalid PID).
**Required Positive Controls:** `-Mode Paper -CheckOnly` → delegates to and surfaces `Launch-VeritasLedger.ps1 -CheckOnly`'s real read-only report (proven; this dev worktree correctly reports a prerequisite-check failure because `.env.local` was never copied into it — expected, not a launcher defect; re-run after REPAIR-02 confirms zero mutation and no refresh-ownership file created, `mqk-paper-postgres` container was already running from prior work and untouched by this run). Matching-scope refresh-owner with a live PID → reused, no second process started (proven via real fixture-process functional test).
**Required Regression Tests:** `scripts/windows/tests/test_official_dual_mode_launcher.ps1` — 87/87 green (60 REPAIR-01-era assertions, all retained — one, the daemon-bootstrap-mode literal check, was necessarily updated from asserting `'TradeReady'` to asserting `'Observe'` since that literal *was* Defect A — plus 27 new REPAIR-02 assertions covering pre-open Observe bootstrap [Section 4] and refresh-loop idempotent ownership via real fixture PowerShell processes [Section 5]). `Launch-VeritasLedger.ps1` untouched by REPAIR-02 (`-Mode` `ValidateSet('Observe','TradeReady')`, default `'Observe'`, and `TradeReady`'s `Get-TradeReadinessReasons` gates are all statically re-verified unchanged).
**Required Validation:**
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\tests\test_official_dual_mode_launcher.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Start-MiniQuantDesk.ps1 -Mode Paper -CheckOnly
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Start-MiniQuantDesk.ps1 -Mode Live -CheckOnly
```
**Forbidden Validation / Side Effects:** No real live order, no live runtime start, no live DB mutation, no push, no merge to `main`. No full (non-CheckOnly) Paper startup was run from this dev worktree this session (would contend with the protected paper-soak environment) — the arm-guarantee, DB-prerequisite, pre-open-Observe, and refresh-ownership code paths are proven by static source-guard tests plus real-but-disposable fixture-process functional tests (temp repo, temp PowerShell sleep processes, never a real daemon/broker/trading runtime), not a live end-to-end dynamic run.
**Acceptance Criteria:** 1) `-Mode`/`-CheckOnly`/`-Scheduled`/`-ArmPaper` all behave exactly as specified, with `-ArmPaper` no longer required. 2) Live mode reports real, current ledger-sourced blockers, never a fabricated verdict; unchanged by REPAIR-01/REPAIR-02. 3) Paper mode never manually invokes `start-system`. 4) Official full Paper startup always establishes and verifies `arm_state=="armed"` before success. 5) Session refresh duration is derived from authoritative NYSE-calendar truth, fails closed when unavailable. 6) Official launcher owns Docker/paper-DB/migration prerequisites. 7) Daemon bootstrap uses `Observe`, not `TradeReady`, so a pre-open `-Scheduled -Mode Paper` run can reach and pass its own arm stage without `session_in_window=true`. 8) The intraday refresh loop is idempotent: same-scope reuse of a live owner, exactly one replacement for a dead/mismatched owner, never a killed process, no secrets in the ownership record, `-CheckOnly` never creates an active ownership record. 9) Guard-test suite green (87/87). 10) Protected `main` worktree provably untouched.
**Exact CLOSED End State:** CLOSED when an operator has independently reviewed the diff (including this REPAIR-02 update) in the `-ops` worktree, confirmed the protected `main` baseline is unaffected, and either merges via an explicit separate decision or accepts the branch as the new operational default — none of which this patch itself performs.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING (REPAIR-01: PENDING / PENDING / PENDING / PENDING; REPAIR-02: PENDING / PENDING / PENDING / PENDING).

#### PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01 — Windows Task Scheduler registration for unattended Paper start

**Status:** BLOCKED (depends on OFFICIAL-DUAL-MODE-LAUNCHER-01 CLOSED) · **Priority:** P2 · **Paper Impact:** GREEN (additive scheduling only) · **Subsystem:** Ops tooling
**Current Source Truth:** Not started. `OFFICIAL-DUAL-MODE-LAUNCHER-01` establishes the `-Scheduled -Mode Paper` contract this patch would register against (`Register-PremarketDataRefreshTask.ps1` is the closest existing precedent for scheduled-task registration style, but it only ever calls `Prep-PremarketMarketData.ps1`, never a full startup).
**Problem:** No Windows Scheduled Task exists that invokes `Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled` at the correct pre-open boundary.
**Dependencies:** `OFFICIAL-DUAL-MODE-LAUNCHER-01` CLOSED.
**In Scope (future patch, not this one):** Task Scheduler registration mirroring `Register-PremarketDataRefreshTask.ps1`'s registration pattern, invoking exactly `Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled`. **Out of Scope:** Any Live scheduling (blocked indefinitely behind the full `LIVE-*` critical path).
**Exact CLOSED End State:** CLOSED when a registered, idempotent scheduled task reliably invokes the official launcher's `-Scheduled -Mode Paper` path and this is proven across at least one real unattended run.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

---

### LANE C — Live Development (live-only + carefully controlled shared work, separate branch/worktree)

#### LIVE-ACCOUNT-TRUTH-01 — Fix buying_power aliasing to cash

**Status:** READY · **Priority:** P1 · **Paper Impact:** YELLOW (touches `routes/portfolio.rs:440`, shared paper+live code path) · **Subsystem:** Live account truth / mqk-broker-alpaca / mqk-schemas
**Current Source Truth:** `mqk-broker-alpaca/src/types.rs:323-330` (`AlpacaAccountRaw`) only deserializes `equity`, `cash`, `currency` from Alpaca's `GET /v2/account` — no `buying_power`/`daytrading_buying_power`/`pattern_day_trader` fields captured at all. `mqk-schemas/src/lib.rs:63-67` (`BrokerAccount`) mirrors this omission. `routes/portfolio.rs:440` aliases `buying_power: Some(cash)` — silently wrong for any margin account.
**Problem:** For live capital this is real-money-relevant; for paper (cash-account assumption) it's cosmetically wrong but not economically dangerous today.
**Why This Matters:** Any future live-capital operator needs correct buying-power truth before risking real money; fixing it now also improves paper-mode reporting honesty.
**Dependencies:** NONE. **Unlocks:** A prerequisite input for `LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01`'s eventual account-truth evidence.
**In Scope:** Add real `buying_power`/`daytrading_buying_power` fields end-to-end: `AlpacaAccountRaw` → `BrokerAccount` → `normalize_account` (`mqk-broker-alpaca/src/snapshot.rs:37-45`) → `routes/portfolio.rs`; stop aliasing cash. **Out of Scope:** Any change to position/order logic; any change to `equity`/`cash` handling itself.
**Likely Files / Surfaces:** `core-rs/crates/mqk-broker-alpaca/src/types.rs`, `src/snapshot.rs`, `core-rs/crates/mqk-schemas/src/lib.rs`, `core-rs/crates/mqk-daemon/src/routes/portfolio.rs`.
**Required Implementation Rules:** Must be built and reviewed on a separate branch/worktree per the paper-soak protection rule (YELLOW); must not merge into `main` without explicit regression review against the running paper baseline.
**Safety / Compatibility Requirements:** Must preserve existing `equity`/`cash` field behavior exactly; must fail closed (return `None`, not a fabricated value) if Alpaca's response omits the new fields unexpectedly.
**Required Negative Controls:** A response payload missing `buying_power` must not silently fall back to `cash` — it must surface as `None`/unavailable.
**Required Positive Controls:** A well-formed Alpaca account response with real `buying_power` produces the correct (not cash-aliased) value.
**Required Regression Tests:** Existing portfolio/account snapshot scenario tests must remain green under both paper and (once buildable) live-shadow conditions.
**Required Validation:** `cargo test -p mqk-broker-alpaca -p mqk-schemas -p mqk-daemon`; `cargo clippy --all-targets -- -D warnings` on touched crates.
**Forbidden Validation / Side Effects:** No real live Alpaca account call in CI; no paper-soak production DB.
**Acceptance Criteria:** 1) `BrokerAccount` carries real `buying_power`/`daytrading_buying_power`. 2) `portfolio.rs` no longer aliases cash. 3) Regression tests green. 4) Change reviewed against paper baseline before any merge to `main`.
**Exact CLOSED End State:** CLOSED when buying-power truth is sourced from Alpaca's real field end-to-end, proven by a test fixture with `buying_power != cash`, and the change has passed regression review against the paper baseline.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### LIVE-SECRETS-CONSOLIDATION-01 — Route live credential resolution through mqk_config::secrets

**Status:** READY · **Priority:** P2 · **Paper Impact:** YELLOW (same `DaemonBroker` construction path used by paper) · **Subsystem:** mqk-config / mqk-daemon
**Current Source Truth:** `mqk-config/src/secrets.rs:14` documents that `LIVE` mode requires key+secret+TwelveData key via `resolve_secrets_for_mode()`, but `mqk-daemon/src/state/broker.rs::build_daemon_broker` reads `std::env::var` directly rather than through that function — a documented-vs-actual mismatch against `secrets.rs`'s own stated contract ("never scatter `std::env::var` calls").
**Problem:** Two sources of truth for secret resolution; the documented one isn't the one actually used for live daemon broker construction.
**Dependencies:** NONE.
**In Scope:** Route `build_daemon_broker`'s live (and ideally paper, for consistency) credential resolution through `mqk_config::secrets::resolve_secrets_for_mode` instead of ad hoc `std::env::var` calls. **Out of Scope:** Changing what credentials are required per mode.
**Likely Files / Surfaces:** `core-rs/crates/mqk-daemon/src/state/broker.rs`, `core-rs/crates/mqk-config/src/secrets.rs`.
**Required Implementation Rules:** Must build on a separate branch; regression-review before merging since this touches the exact code path paper currently runs on (`DaemonBroker`/`AlpacaBrokerAdapter` shared trait dispatch).
**Safety / Compatibility Requirements:** Must not change which env vars are read for paper mode; must fail closed identically to today if a required var is missing.
**Required Negative Controls:** Missing a required live secret must still refuse broker construction, exactly as today.
**Required Positive Controls:** Paper broker construction is byte-for-byte behaviorally identical before/after.
**Required Regression Tests:** All existing `state/broker.rs`-adjacent tests remain green.
**Required Validation:** `cargo test -p mqk-daemon -p mqk-config`.
**Acceptance Criteria:** 1) `build_daemon_broker` no longer calls `std::env::var` directly for secrets. 2) Paper-mode behavior provably unchanged. 3) Regression-reviewed before merge.
**Exact CLOSED End State:** CLOSED when secret resolution has exactly one source of truth (`mqk_config::secrets`), proven unchanged for paper mode by regression tests.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### LIVE-CLI-ARM-RECONCILE-01 — Determine whether legacy `mqk run arm` CLI path is still operationally reachable

**Status:** READY · **Priority:** P2 · **Paper Impact:** GREEN pending investigation · **Subsystem:** mqk-cli
**Current Source Truth:** `mqk-cli/src/commands/run.rs:100-111` (`run_arm`) calls `enforce_manual_confirmation_if_required` before `mqk_db::arm_preflight`, tested by `scenario_cli_arm_requires_confirmation.rs` ("PATCH 16") against a hand-built `NewRun` via `mqk_config::load_layered_yaml`. This looks like an older, pre-daemon CLI architecture parallel to (not integrated with) the current daemon's `/v1/integrity/arm` HTTP surface (proven in the arm-gate audit, §5 Lane A / core-exec cluster).
**Problem:** Unclear whether operators actually use this path for live arm, or whether it's vestigial.
**Dependencies:** NONE. **Unlocks:** `CLI-RUN-STUB-TRACKING-01`'s resolution.
**In Scope:** Determine reachability/usage via source + operator interview; either wire it to the same `check_arm_safety` gate as the daemon HTTP surface, or deprecate/document it as legacy. **Out of Scope:** Building new arm functionality.
**Likely Files:** `core-rs/crates/mqk-cli/src/commands/run.rs`, `core-rs/crates/mqk-daemon/src/routes/control_plane.rs` (for comparison).
**Required Validation:** Investigation only in the first phase; if code changes result, standard `cargo test -p mqk-cli`.
**Acceptance Criteria:** A definitive determination recorded in this ledger: "still live-relevant, now wired to `check_arm_safety`" or "deprecated, marked legacy in CLI help text."
**Exact CLOSED End State:** CLOSED when the determination is recorded and, if action was needed, implemented and tested.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### LIVE-FLATTEN-PROOF-01 — LiveShadow-mode flatten-on-halt scenario test

**Status:** READY · **Priority:** P2 · **Paper Impact:** GREEN (test-only, LiveShadow has zero capital at risk) · **Subsystem:** mqk-risk / mqk-daemon
**Current Source Truth:** `RISK-FLATTEN-ON-HALT-01` (memory: CLOSED) implements `RiskRequestContext`/`evaluate_gate_for_request`, generic to broker kind. Only `scenario_paper_flatten_psf01.rs` exists; no LiveShadow-mode flatten test.
**Problem:** Flatten-on-halt has never been exercised against the live Alpaca endpoint (even in LiveShadow, which requires no capital risk and already has `start_allowed: true`).
**Dependencies:** NONE.
**In Scope:** Write a new scenario test in LiveShadow+Alpaca-live-base-URL mode exercising flatten-on-halt. **Out of Scope:** Any production code change (this is a proof-only patch; if the test reveals a defect, that becomes a new patch).
**Likely Files:** `core-rs/crates/mqk-daemon/tests/scenario_live_shadow_flatten_on_halt_01.rs` (new).
**Required Validation:** `cargo test -p mqk-daemon --test scenario_live_shadow_flatten_on_halt_01 -- --include-ignored` (if DB-gated).
**Acceptance Criteria:** Test exists, runs against LiveShadow mode, and passes or clearly documents a found defect.
**Exact CLOSED End State:** CLOSED when the test exists and passes (or a follow-up defect patch is filed if it fails).
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### LIVE-TINY-CAPITAL-SMOKE-01 — Build live-shadow smoke automation script

**Status:** READY · **Priority:** P1 · **Paper Impact:** GREEN (additive tooling, zero shared runtime, LiveShadow = no capital risk) · **Subsystem:** Ops tooling
**Current Source Truth:** `scripts/` has 82 files matching "live" but all are paper-trading scripts or unrelated (Kraken, live Discord channel config). No live-capital smoke script exists. `docs/runbooks/live_shadow_operational_proof.md` is a manual proof-sequence document, not automation.
**Problem:** No repeatable tooling to accumulate LiveShadow operational evidence.
**Why This Matters:** This tooling is the input `LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01` will need.
**Dependencies:** NONE. **Unlocks:** `LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01`.
**In Scope:** Build `Start-LiveShadowSmoke.ps1` analogous to the existing paper smoke script, targeting LiveShadow+Alpaca (real market data, zero capital risk since LiveCapital remains gated off). **Out of Scope:** Any change to the trust-chain gate itself.
**Likely Files:** `scripts/Start-LiveShadowSmoke.ps1` (new), modeled on `scripts/Start-PaperTradingSmoke.ps1`.
**Required Validation:** Manual dry run against LiveShadow mode (no capital risk by construction).
**Acceptance Criteria:** Script runs a full LiveShadow smoke cycle and produces evidence artifacts analogous to the paper smoke tooling.
**Exact CLOSED End State:** CLOSED when the script exists, has been run at least once successfully, and produces evidence artifacts in a documented location.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01 — Real shadow-execution capture pipeline (decomposed sub-patch a of 3)

**Status:** BLOCKED (depends on `LIVE-TINY-CAPITAL-SMOKE-01`) · **Priority:** P1 · **Paper Impact:** YELLOW · **Subsystem:** research-py / mqk-daemon live-shadow
**Current Source Truth:** `research-py/src/mqk_research/deployment/parity.py` hardcodes `live_trust_complete=false` in the TV-03 pipeline; `docs/runbooks/live_shadow_operational_proof.md:24,69` confirms this is the current, correct state.
**Problem:** No real evidence-capture mechanism exists to record actual LiveShadow execution cycles as input to a future parity score.
**Dependencies:** `LIVE-TINY-CAPITAL-SMOKE-01` (need the smoke tooling running first to generate cycles to capture).
**Unlocks:** `LIVE-TRUST-CHAIN-PARITY-SCORER-01`.
**In Scope:** Build the capture mechanism only — record LiveShadow execution cycle data in a durable, evidence-grade format. **Out of Scope:** Scoring or evidence-signing (separate sub-patches); do not attempt to flip `live_trust_complete` in this patch.
**Likely Files:** `research-py/src/mqk_research/deployment/parity.py`, new capture module.
**Required Validation:** Python test suite for the new capture module; no live capital risk since source is LiveShadow only.
**Acceptance Criteria:** Capture pipeline produces durable, inspectable evidence records from real LiveShadow cycles.
**Exact CLOSED End State:** CLOSED when at least one real LiveShadow cycle has been captured end-to-end into a durable evidence record.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### LIVE-TRUST-CHAIN-PARITY-SCORER-01 — Parity scorer (decomposed sub-patch b of 3)

**Status:** BLOCKED (depends on `LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01`) · **Priority:** P1 · **Paper Impact:** YELLOW · **Subsystem:** research-py
**In Scope:** Build a scorer comparing captured shadow-execution evidence against expected/paper-equivalent outcomes to produce a parity metric. **Out of Scope:** Evidence signing, gate flipping.
**Dependencies:** `LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01`. **Unlocks:** `LIVE-TRUST-CHAIN-EVIDENCE-SIGNER-01`.
**Exact CLOSED End State:** CLOSED when the scorer produces a reproducible parity metric from real captured evidence.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### LIVE-TRUST-CHAIN-EVIDENCE-SIGNER-01 — Signed evidence producer (decomposed sub-patch c of 3)

**Status:** BLOCKED (depends on `LIVE-TRUST-CHAIN-PARITY-SCORER-01`) · **Priority:** P1 · **Paper Impact:** YELLOW · **Subsystem:** research-py / mqk-daemon
**In Scope:** Build the mechanism that can legitimately flip `live_trust_complete=true` for a specific, signed evidence artifact meeting a defined parity threshold — this is the gate `state/lifecycle.rs:1000-1048` (TV-03D) already checks for. **Out of Scope:** Lowering or removing the gate itself; changing the threshold without explicit operator sign-off.
**Dependencies:** `LIVE-TRUST-CHAIN-PARITY-SCORER-01`. **Unlocks:** `LIVE-CAPITAL-EXTERNAL-PROOF-01`.
**Exact CLOSED End State:** CLOSED when a real, signed evidence artifact can legitimately cause `live_trust_complete=true` for a qualifying shadow-execution history, proven by a positive-control test, with a negative control proving a sub-threshold history still yields `false`.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### LIVE-CAPITAL-EXTERNAL-PROOF-01 — First real live-capital start with tiny notional

**Status:** BLOCKED (depends on `LIVE-TRUST-CHAIN-EVIDENCE-SIGNER-01`) · **Priority:** P0 · **Paper Impact:** RED · **Subsystem:** Live capital cutover
**Problem:** This is not a code patch — it is the first real deployment of capital, requiring explicit operator sign-off. No agent or automated session may perform or close this item.
**In Scope:** Operator-executed, tiny-notional live order(s) under full supervision, once all trust-chain prerequisites are CLOSED. **Out of Scope:** Everything else — this patch exists only to mark the final gate in the dependency chain.
**Exact CLOSED End State:** CLOSED only by explicit operator action and sign-off, never by an implementation session.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

---

### LANE D — Post-Soak Shared Core (YELLOW/RED, wait until soak baseline is accepted)

#### DAEMON-HALT-FENCE (already tracked as `PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01` in Lane A — not duplicated here.)

#### DEADMAN-LEASE-TTL-RECONCILE-01 — Align 90s runtime-lease TTL with 120s deadman TTL at the root

**Status:** READY (dependency closed; unblocked, but still RED — do not attempt during the active soak without explicit operator authorization) · **Priority:** P1 · **Paper Impact:** RED · **Subsystem:** mqk-daemon halt/deadman
**Current Source Truth:** Runtime lease TTL = 90s (`orchestrator.rs:50`); deadman TTL = 120s (`DEADMAN_TTL_SECONDS`). The 120s deadman interval can outlive the 90s runtime lease by approximately 30 seconds, so lease expiry alone cannot prove same-process task quiescence — this asymmetry is the root cause of the race that `PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01` (CLOSED at `e44e3ddd`) fences around rather than eliminates.
**Problem:** The fence patch treats the symptom (a stale task might still be alive); this patch would address the cause (why the windows don't align).
**Dependencies:** NONE (formerly `PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01`, now CLOSED at `e44e3ddd`).
**In Scope:** Analyze and, if safe, align the two TTLs (or document why they must differ and add a comment explaining the intentional gap). **Out of Scope:** Any other halt/deadman logic change.
**Likely Files:** `core-rs/crates/mqk-runtime/src/orchestrator.rs`, deadman-related config in `mqk-daemon/src/state/`.
**Required Implementation Rules:** This is RED — must not be attempted during the active soak without explicit operator authorization to pause/restart the soak for validation.
**Required Validation:** Full halt/deadman/reconcile scenario suite; `scenario_kill_switch_guarantees.rs`.
**Acceptance Criteria:** Either the TTLs are aligned with a proof that no new race window opens, or a documented rationale for the intentional gap is added along with defensive fencing (which already exists via the Lane A patch).
**Exact CLOSED End State:** CLOSED when the TTL relationship is either aligned or explicitly documented as intentional, with full halt-path regression proof.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### BROKER-ALPACA-RATE-LIMIT-RETRY-AFTER-01 — Parse Retry-After header on 429 responses

**Status:** READY · **Priority:** P2 · **Paper Impact:** YELLOW (live order submit/replace/cancel path) · **Subsystem:** mqk-broker-alpaca
**Current Source Truth:** `mqk-broker-alpaca/src/lib.rs:1138-1142` maps HTTP 429 to `BrokerError::RateLimit { retry_after_ms: None, ... }` — the `Retry-After` header is never read.
**Problem:** Callers can't honor Alpaca's actual backoff window; they're guessing.
**Dependencies:** NONE.
**In Scope:** Parse `Retry-After` in `classify_http_status`, thread through to `retry_after_ms`. **Out of Scope:** Changing overall rate-limit/retry policy or backoff algorithm.
**Likely Files:** `core-rs/crates/mqk-broker-alpaca/src/lib.rs`.
**Required Implementation Rules:** Must build/review on a separate branch (YELLOW); regression review before merge into `main` since this is the live order-submission error path.
**Safety / Compatibility Requirements:** Must not change behavior for non-429 responses; must not introduce a panic on a malformed/missing header (fall back to `None` exactly as today).
**Required Negative Controls:** A 429 with no `Retry-After` header still yields `retry_after_ms: None` (unchanged behavior).
**Required Positive Controls:** A 429 with a valid `Retry-After` header yields the correct parsed value.
**Required Regression Tests:** Existing rate-limit-adjacent scenario tests remain green.
**Required Validation:** `cargo test -p mqk-broker-alpaca`.
**Acceptance Criteria:** 1) Header is parsed when present. 2) Absent-header behavior unchanged. 3) Regression tests green.
**Exact CLOSED End State:** CLOSED when `retry_after_ms` reflects the real Alpaca-supplied backoff window when provided, proven by a positive-control test, with a negative control proving the fallback path is unchanged.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### DB-OUTBOX-SCHEMA-VERSION-01 — Add schema_version to the order_json envelope

**Status:** READY · **Priority:** P2 · **Paper Impact:** YELLOW (live write path; shape is stable/tested, low drift risk today) · **Subsystem:** mqk-db / mqk-execution
**Current Source Truth:** `schema_version` is present on JSON-evidence artifacts that need it (`dynamic_selection_evidence.rs`, `runtime_strategy_conflict.rs`) but absent from `oms_outbox.order_json` / `oms_inbox.message_json` (`mqk-db/src/orders.rs`, `src/inbox.rs`).
**Problem:** `db_rules.md` requires `schema_version` on all serialized DB artifacts; this envelope is a gap against the literal rule, even though current drift risk is low (internally-produced, stable-shaped envelope).
**Dependencies:** NONE.
**In Scope:** Add a `schema_version` field to the order-command JSON envelope constructed by `mqk-execution` before it's persisted as `order_json`. **Out of Scope:** Any change to the outbox claim/atomicity logic itself.
**Likely Files:** `core-rs/crates/mqk-execution/src/` (wherever the order-command envelope is constructed), `core-rs/crates/mqk-db/src/orders.rs`.
**Required Implementation Rules:** Must be additive and backward-readable — existing rows without `schema_version` must still deserialize correctly (treat absence as version 1, implicit).
**Safety / Compatibility Requirements:** Must not require a migration to backfill existing rows; must not change the outbox atomicity contract.
**Required Regression Tests:** All outbox/inbox scenario tests remain green, including replay of pre-existing (unversioned) rows.
**Required Validation:** `cargo test -p mqk-db -p mqk-execution`.
**Acceptance Criteria:** 1) New rows carry `schema_version`. 2) Old rows without it still deserialize. 3) No migration required.
**Exact CLOSED End State:** CLOSED when new order-command envelopes carry `schema_version` and a test proves both new-row and legacy-row deserialization succeed.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### MD-ALPACA-FETCH-RETRY-BACKOFF-01 — Add bounded retry/backoff to Alpaca fetch_bars

**Status:** READY · **Priority:** P2 · **Paper Impact:** YELLOW (Alpaca is the live-gate equity data path; current behavior already fails safe — readiness gate correctly reports stale rather than fabricating data — so this is a resilience improvement, not a correctness fix) · **Subsystem:** mqk-md
**Current Source Truth:** `mqk-md/src/alpaca_provider.rs:100-157` (`fetch_bars`) issues a single HTTP attempt per page; non-2xx or transport error propagates immediately. `provider.rs:412-514` (TwelveData) already has a proven bounded 429-retry pattern.
**Dependencies:** Can reuse the pattern established by `MD-KRAKEN-FETCH-RETRY-BACKOFF-01` (Lane B) if that lands first, though independent.
**In Scope:** Same bounded-retry-on-transient-status pattern applied to `AlpacaHistoricalProvider::fetch_bars`. **Out of Scope:** Any change to the readiness-gate logic that consumes ingested data.
**Likely Files:** `core-rs/crates/mqk-md/src/alpaca_provider.rs`.
**Required Implementation Rules:** Must build/review on a separate branch (YELLOW) since Alpaca is the paper-soak's live data-ingest path; regression review before merge.
**Safety / Compatibility Requirements:** Must not mask a genuine persistent outage — bounded retry only, must still surface a stale/not-ready state to the readiness gate if retries exhaust.
**Required Regression Tests:** `market_data_readiness.rs`-adjacent scenario tests remain green; a persistent-failure case must still correctly report not-ready.
**Required Validation:** `cargo test -p mqk-md -p mqk-daemon`.
**Acceptance Criteria:** 1) Transient failure recovers within the retry window. 2) Persistent failure still correctly surfaces as stale/not-ready — no behavior regression. 3) Regression tests green.
**Exact CLOSED End State:** CLOSED when a transient-failure-then-recovery test passes and a persistent-failure test proves the readiness gate still fails closed exactly as before.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### CALENDAR-TABLE-EXTENSION-2029-2030-01 — Extend NYSE calendar tables

**Status:** DEFERRED (not urgent — table covers through 2028-12-31; today is 2026-08-10, ~2.4 years of runway remain) · **Priority:** P3 · **Paper Impact:** RED (shared calendar consumed directly by the live preflight gate) · **Subsystem:** mqk-integrity / mqk-daemon calendar
**Current Source Truth:** `mqk-integrity/src/calendar.rs:352-372,421-491` and `mqk-daemon/src/state/market_calendar.rs:1060-1061` (`SCHEDULE_COVERAGE_START=(2023,1,1)`, `SCHEDULE_COVERAGE_END=(2028,12,31)`) — fails closed (`CalendarCoverageState::OutOfRange`) outside that window.
**Problem:** Hardcoded table requires manual extension; no live exchange API source is wired (`ExchangeSourcedCalendarProvider` seam exists at `calendar.rs:345-474` but has no live implementation, only fixture/injectable data).
**Dependencies:** NONE.
**In Scope:** Extend `EARLY_CLOSE_DATES`/`HOLIDAYS` tables + `SCHEDULE_COVERAGE_END` to 2029-2030. **Out of Scope:** Building a live exchange-API-sourced calendar provider (a much larger, separate future patch).
**Recommended timing:** Revisit no later than mid-2028.
**Exact CLOSED End State:** CLOSED when the tables and coverage window are extended and the existing DST/holiday/early-close test suite (`calendar.rs:599-696`) is extended to cover the new years.
**Acceptance History:** N/A (deferred, not started).

#### MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01 — Per-symbol failure isolation in the dispatch loop

**Status:** READY · **Priority:** P1 · **Paper Impact:** RED (live dispatch path for the autonomous paper loop) · **Subsystem:** mqk-daemon multi-symbol dispatch
**Current Source Truth:** `mqk-daemon/src/state/loop_runner.rs:220` — a spawned-task panic while dispatching symbol N "drops the whole host pool with it." No per-symbol catch/isolate boundary exists; dispatch is sequential (`state.rs:3529`, one `.await` per symbol per tick).
**Problem:** A single symbol's runtime panic currently takes down the entire tick for every symbol, not just the failing one.
**Why This Matters:** As multi-symbol trading scales up, blast radius of a single bad symbol/strategy interaction should not be "the whole autonomous session halts."
**Dependencies:** NONE. **Unlocks:** Safer scaling of `max_concurrent_symbols`.
**In Scope:** Add a per-assignment catch boundary (e.g. `catch_unwind` or a `Result`-returning wrapper around each symbol's dispatch call) that records a per-symbol fault and continues remaining symbols, rather than losing the whole tick. **Out of Scope:** Changing dispatch from sequential to parallel (a separate, larger architectural decision); changing any strategy logic.
**Likely Files / Surfaces:** `core-rs/crates/mqk-daemon/src/state.rs` (dispatch loop around line 3529), `src/state/loop_runner.rs` (around line 220).
**Required Implementation Rules:** This is RED — must not be attempted during the active soak without explicit operator authorization; build and prove on a separate branch first.
**Safety / Compatibility Requirements:** Must not change per-symbol trading decisions themselves; must not silently swallow a fault — the isolated failure must be durably recorded and alertable (ties into `DISCORD-DATA-STALENESS-ALERT-01`'s pattern of surfacing gate trips).
**Required Negative Controls:** A deliberately panicking test symbol must not prevent other symbols in the same tick from dispatching normally.
**Required Positive Controls:** Normal multi-symbol dispatch with no faults is unaffected.
**Required Regression Tests:** `scenario_multi_symbol_dispatch_loop_01.rs`, `scenario_multi_symbol_dispatch_summary_01.rs` remain green.
**Required Validation:** `cargo test -p mqk-daemon -- multi_symbol`.
**Acceptance Criteria:** 1) A panicking symbol no longer aborts the whole tick. 2) The fault is durably recorded. 3) Existing multi-symbol scenario tests remain green.
**Exact CLOSED End State:** CLOSED when a negative-control test proves a single symbol's panic no longer drops the whole tick, with the fault durably recorded and existing regression coverage green.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01 — Implement max_concurrent_symbols truncate-and-surface

**Status:** READY · **Priority:** P2 · **Paper Impact:** RED · **Subsystem:** mqk-daemon multi-symbol config
**Current Source Truth:** `mqk-daemon/src/state/multi_symbol_config.rs:45-59` explicitly defers cap #1 (`max_concurrent_symbols`) truncate/surface behavior — "remains open for a later patch" — currently fails closed instead of truncating.
**Problem:** No graceful truncation path; a watchlist exceeding the cap fails closed entirely rather than trading the first N symbols with a surfaced warning.
**Dependencies:** NONE.
**In Scope:** Implement truncate-to-cap behavior with an additive field on `WatchlistStatusResponse` surfacing which symbols were dropped and why. **Out of Scope:** Changing what the cap value itself defaults to.
**Likely Files:** `core-rs/crates/mqk-daemon/src/state/multi_symbol_config.rs`, relevant API response type.
**Required Implementation Rules:** RED — build/prove on a separate branch; must preserve the fail-closed default for any other config error (this only changes the cap-exceeded case specifically).
**Required Regression Tests:** `scenario_multi_symbol_runtime_config_01.rs` remains green plus a new test for the truncate path.
**Required Validation:** `cargo test -p mqk-daemon -- multi_symbol`.
**Acceptance Criteria:** 1) Exceeding the cap truncates rather than fails closed. 2) Truncated symbols are surfaced in the API response. 3) All other config-error fail-closed paths unchanged.
**Exact CLOSED End State:** CLOSED when a watchlist exceeding `max_concurrent_symbols` trades the first N symbols and surfaces the drop, proven by a new positive-control test.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### MULTI-SYMBOL-CAPS-PREFLIGHT-WARNING-01 — Preflight warning when per-symbol/aggregate caps are unset

**Status:** READY · **Priority:** P1 · **Paper Impact:** RED (touches the autonomous session preflight path; conservatively classified RED even though it is advisory-only, since it changes what preflight reports during an active soak) · **Subsystem:** mqk-daemon preflight
**Current Source Truth:** `scenario_multi_symbol_capital_caps_01.rs` confirms caps #2 (`MQK_PER_SYMBOL_MAX_POSITION_QTY`), #3 (`MQK_PER_SYMBOL_MAX_NOTIONAL_USD`), #5 (`MQK_AGGREGATE_GROSS_EXPOSURE_CAP_USD`) all default to `None`/disabled. A soak running with these unset has zero per-symbol/aggregate notional protection beyond portfolio-level gates.
**Problem:** An operator could be unaware these protections are off.
**Dependencies:** NONE.
**In Scope:** Add an advisory (non-blocking) warning to the autonomous session preflight response when any of caps 2/3/5 are unset. **Out of Scope:** Changing the caps' default values or enforcement behavior — advisory only in this patch.
**Likely Files:** `core-rs/crates/mqk-daemon/src/daily_data_readiness.rs` or wherever preflight response is assembled.
**Required Implementation Rules:** Must be strictly additive/advisory — must not change whether a session is allowed to start.
**Required Regression Tests:** Existing preflight scenario tests unaffected in their pass/fail outcome, only in additional warning fields present.
**Required Validation:** `cargo test -p mqk-daemon -- preflight`.
**Acceptance Criteria:** 1) Preflight response includes a clear warning when caps are unset. 2) Start-allowed determination is unchanged.
**Exact CLOSED End State:** CLOSED when the preflight response surfaces the warning and no existing test's pass/fail outcome changed.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### DISCORD-CHANNEL-ROUTING-01 — Wire the 6-channel Discord routing that already exists but is unused

**Status:** READY · **Priority:** P2 · **Paper Impact:** YELLOW (touches the already-running soak's live notifier construction) · **Subsystem:** mqk-daemon / mqk-config notify
**Current Source Truth:** `mqk-config/src/secrets.rs:38-53` defines `ResolvedDiscordWebhooks` with 6 channels (`paper`,`live`,`backtest`,`alerts`,`heartbeat`,`c2`) sourced from `config/defaults/base.yaml:105-112`. `mqk-daemon/src/state.rs:1724` constructs the notifier via `DiscordNotifier::from_env()`, which only reads a single flat `DISCORD_WEBHOOK_URL` — the multi-channel resolution is dead code from the daemon's perspective; all alert types funnel into one webhook.
**Problem:** Built-and-unused routing means operators can't separate paper/live/critical alert streams.
**Dependencies:** NONE. **Unlocks:** `DISCORD-DATA-STALENESS-ALERT-01`, `DISCORD-DAILY-SUMMARY-PUSH-01` (both should route through the correct channel once this lands).
**In Scope:** Wire `ResolvedSecrets.discord` channels into `DiscordNotifier` construction in `state.rs`, routing critical alerts to `alerts`, trade events to `paper`/`live` per deployment mode. **Out of Scope:** Adding new alert types (separate patches).
**Likely Files:** `core-rs/crates/mqk-daemon/src/state.rs`, `core-rs/crates/mqk-daemon/src/notify.rs`.
**Required Implementation Rules:** Must build/review on a separate branch (YELLOW); regression review before merge — must not regress existing alert delivery (currently proven: fires only after durable DB write, 3s timeout, errors sanitized/swallowed, never blocks trading).
**Required Negative Controls:** A misconfigured channel webhook must still fail safe (swallowed, logged, does not block trading) exactly as today.
**Required Positive Controls:** Each alert type reaches its correct configured channel.
**Required Regression Tests:** Existing notify-adjacent tests remain green.
**Required Validation:** `cargo test -p mqk-daemon -- notify`.
**Acceptance Criteria:** 1) Alerts route to their configured channel. 2) Fail-safe swallow behavior on delivery failure is unchanged. 3) Regression review passed before merge.
**Exact CLOSED End State:** CLOSED when each alert type demonstrably reaches its intended channel and the existing fail-safe delivery contract is proven unchanged.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### DISCORD-DATA-STALENESS-ALERT-01 — Fire a critical alert when the MD-staleness gate trips

**Status:** READY · **Priority:** P2 · **Paper Impact:** YELLOW · **Subsystem:** mqk-daemon notify / market data readiness
**Current Source Truth:** `MD-STALENESS-PER-TICK-GATE-01` (memory: CLOSED) blocks trading on stale data but never fires a Discord notification — an operator watching only Discord would not see "feed went stale, trading paused" in real time. Grep for `notify_.*data|notify.*stale` in daemon src returns zero hits.
**Dependencies:** `DISCORD-CHANNEL-ROUTING-01` (should route to the correct channel once available; can also ship to the single flat webhook first if sequencing requires).
**In Scope:** Add a `notify_critical_alert` (or new `notify_data_feed_stale`) call at the staleness-gate trip site. **Out of Scope:** Any change to the staleness gate's blocking logic itself.
**Likely Files:** Wherever the MD-staleness gate trips (per `MD-STALENESS-PER-TICK-GATE-01`'s implementation location), `core-rs/crates/mqk-daemon/src/notify.rs`.
**Required Implementation Rules:** Must not change gate behavior, only add a notification side effect; must follow the existing "fires after durable state change, never blocks trading" contract.
**Required Regression Tests:** Staleness-gate scenario tests remain green with unchanged blocking behavior.
**Required Validation:** `cargo test -p mqk-daemon -- staleness`.
**Acceptance Criteria:** A staleness-gate trip now produces a Discord alert without changing the gate's blocking decision.
**Exact CLOSED End State:** CLOSED when a test proves a staleness trip both blocks trading (unchanged) and fires an alert (new).
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### DISCORD-DAILY-SUMMARY-PUSH-01 — Push daily no-trade/session diagnostics to Discord

**Status:** READY · **Priority:** P2 · **Paper Impact:** YELLOW · **Subsystem:** mqk-daemon notify
**Current Source Truth:** `autonomous_no_trade_diagnostics` exists as a read-route (`routes/system.rs:1210`) but is never pushed to Discord — operator must poll.
**Dependencies:** `DISCORD-CHANNEL-ROUTING-01`.
**In Scope:** Add a scheduled/end-of-day push of the diagnostics summary to the appropriate Discord channel. **Out of Scope:** Changing the diagnostics computation itself.
**Likely Files:** `core-rs/crates/mqk-daemon/src/routes/system.rs`, `src/notify.rs`.
**Required Validation:** `cargo test -p mqk-daemon -- diagnostics`.
**Acceptance Criteria:** Daily summary is pushed automatically without requiring operator polling.
**Exact CLOSED End State:** CLOSED when a scheduled test/manual run confirms the push fires once per session end.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

---

### LANE E — Multi-Asset Expansion (post-soak, long-lead, each must be decomposed before implementation)

#### MULTI-ASSET-CRYPTO-EXECUTION-01 — Wire Kraken/crypto data into an actual paper execution path

**Status:** DEFERRED · **Priority:** P3 · **Paper Impact:** GREEN in isolation (does not touch the equity paper path; explicitly gated off) · **Subsystem:** mqk-md / mqk-broker / mqk-execution / multi-asset
**Current Source Truth:** Crypto data ingest (Kraken OHLC) is comparatively mature (15+ closure docs under `docs/specs/crypto_data_01*`), but there is zero execution wiring — no crypto broker adapter, no crypto order-type handling, no crypto risk policy beyond a stub match-arm in `mqk-execution/src/asset_risk_policy.rs`.
**Problem:** This is not a single patch — it requires an instrument model, a crypto broker adapter, calendar/session handling (24/7, not NYSE), risk policy, portfolio/P&L support, and GUI.
**Dependencies:** NONE technically, but should not start until Lane A/B/C/D work is substantially complete given priority ordering.
**Size:** XL — **MUST be decomposed into a real sub-patch sequence (data → instrument model → broker adapter → risk → execution → portfolio → GUI) before any implementation session attempts it.**
**Exact CLOSED End State:** Not defined at this ledger's current decomposition depth — the first actionable step is a dedicated design/decomposition pass producing real S/M sub-patch IDs, not code.
**Acceptance History:** N/A (deferred, not started, not yet decomposed).

#### MULTI-ASSET-OPTIONS-FOUNDATION-01 — Options contract metadata, Greeks, broker adapter foundation

**Status:** DEFERRED · **Priority:** P3 · **Paper Impact:** GREEN in isolation (explicitly gated by `MQK_ASSET_CLASS_OPTION_ENABLED`, default false) · **Subsystem:** multi-asset
**Current Source Truth:** `mqk-schemas/src/lib.rs:105-111` has an `Option` variant in `AssetClass`; `mqk-execution/src/asset_risk_policy.rs:154-156` has a stub `option_policy()` match arm. `docs/specs/experimental/multi_asset_scaffold_01.md` explicitly states "Status: BACKLOG / NOT EXECUTABLE," Lane EXP, "Activation gate: NONE YET." No contract metadata model, no broker adapter, no calendar, no GUI, no tests exist beyond the enum variant.
**Size:** XL — **MUST be decomposed** before implementation.
**Exact CLOSED End State:** Not defined at this ledger's current decomposition depth — requires a dedicated design/decomposition pass first.
**Acceptance History:** N/A (deferred, not started, not yet decomposed).

#### MULTI-ASSET-FUTURES-FOREX-FOUNDATION-01 — Futures + Forex foundation (bundled at design stage only)

**Status:** DEFERRED · **Priority:** P3 · **Paper Impact:** GREEN in isolation · **Subsystem:** multi-asset
**Current Source Truth:** Same enum-variant-plus-stub-match-arm depth as Options; bundled here only because both are equally at the earliest possible stage, not because they should be implemented together.
**Size:** XL — **MUST be decomposed** into separate Futures and Forex programs (each has materially different contract/margin/calendar semantics) before any implementation session attempts either.
**Exact CLOSED End State:** Not defined at this ledger's current decomposition depth.
**Acceptance History:** N/A (deferred, not started, not yet decomposed).

---

### LANE F — Maintainability / Lean-out (only after operational functionality is stable)

#### STATE-RS-LEAN-OUT-01 — Split mqk-daemon/src/state.rs into cohesive submodules

**Status:** DEFERRED · **Priority:** P3 · **Paper Impact:** GREEN (purely structural, if done correctly) · **Subsystem:** mqk-daemon
**Current Source Truth:** `mqk-daemon/src/state.rs` is 7,591 lines. `state/loop_runner.rs` contains two near-identical `notify_critical_alert` blocks for `halt.deadman_expired` (lines ~400-470 and ~654-736) that look like copy-paste across code paths.
**Problem:** File size slows navigation/review; duplicated alert blocks are a maintenance hazard (a future fix applied to one copy and not the other).
**Size:** L — **must be decomposed** into a sequence (e.g., extract halt/deadman logic as its own module first, as a standalone S/M patch, before attempting broader extraction).
**In Scope for the first sub-patch:** Deduplicate the two near-identical deadman-halt alert blocks into one shared function, with zero behavior change. **Out of Scope:** Any broader `state.rs` restructuring in the same patch.
**Required Validation:** Full `mqk-daemon` test suite; behavior must be provably identical before/after (this is a pure refactor).
**Exact CLOSED End State:** For the first sub-patch: CLOSED when the two alert blocks are unified into one function with identical trigger conditions and message content, proven by existing halt-path tests remaining green. Broader `state.rs` decomposition remains DEFERRED pending a dedicated design pass.
**Acceptance History:** N/A (deferred, not started).

#### LIFECYCLE-RS-LEAN-OUT-01 — Split mqk-daemon/src/state/lifecycle.rs into cohesive submodules

**Status:** DEFERRED · **Priority:** P3 · **Paper Impact:** GREEN (purely structural, if done correctly) · **Subsystem:** mqk-daemon
**Current Source Truth:** `state/lifecycle.rs` is 7,126 lines.
**Size:** L — same treatment as `STATE-RS-LEAN-OUT-01`: must be decomposed, not attempted as one patch.
**Exact CLOSED End State:** Not defined at this ledger's current decomposition depth — requires a dedicated design pass identifying cohesive extraction boundaries first.
**Acceptance History:** N/A (deferred, not started).

---

## 6. Dependency Graph

```text
PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01 (Lane A, CLOSED at e44e3ddd)
    |
    +--> DEADMAN-LEASE-TTL-RECONCILE-01 (Lane D, unblocked; still RED, soak-authorization required)
    |
    +--> (unlocks nothing else directly; closes the halt-fence lineage)

LIVE-TINY-CAPITAL-SMOKE-01 (Lane C)
    |
    v
LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01
    |
    v
LIVE-TRUST-CHAIN-PARITY-SCORER-01
    |
    v
LIVE-TRUST-CHAIN-EVIDENCE-SIGNER-01
    |
    v
LIVE-CAPITAL-EXTERNAL-PROOF-01 (operator-only, not a code patch)

LIVE-ACCOUNT-TRUTH-01 (Lane C) ------> feeds account-truth evidence into LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01 (soft dependency, not blocking)

DYNAMIC-SELECTION-TEST-DENSITY-AUDIT-01 (Lane B)
    |
    v
DYNAMIC-SELECTION-E2E-SCENARIO-TEST-01 (Lane B, avoid duplicating existing coverage)

CLI-DAEMON-CONTROL-PASSTHROUGH-01 (Lane B)
    |
    v
CLI-RUNCMD-DOC-DISAMBIGUATION-01 (Lane B, points to the new command once it exists)

DISCORD-CHANNEL-ROUTING-01 (Lane D)
    |
    +--> DISCORD-DATA-STALENESS-ALERT-01 (Lane D)
    +--> DISCORD-DAILY-SUMMARY-PUSH-01 (Lane D)

MD-KRAKEN-FETCH-RETRY-BACKOFF-01 (Lane B) ------> establishes reusable pattern for MD-ALPACA-FETCH-RETRY-BACKOFF-01 (Lane D, independent, not strictly blocked)

STRATEGY-MEAN-REVERSION-UNIT-TESTS-01, STRATEGY-VOLATILITY-BREAKOUT-UNIT-TESTS-01, STRATEGY-SWING-MOMENTUM-UNIT-TESTS-01 (Lane B, independent of each other)
    |
    v
STRATEGY-POSITION-SIZING-PARITY-01 (Lane B, DEFERRED pending operator decision — soft dependency on test coverage landing first)

STATE-RS-LEAN-OUT-01 first sub-patch (dedupe alert blocks) (Lane F)
    |
    v
(broader state.rs decomposition — not yet scoped)
```

Critical paths:
- **Paper operational maturity:** fully achieved (PAPER_SOAK_GO); `PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01` is CLOSED — zero open Lane A items remain.
- **Live trading:** `LIVE-TINY-CAPITAL-SMOKE-01` → `LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01` → `-PARITY-SCORER-01` → `-EVIDENCE-SIGNER-01` → `LIVE-CAPITAL-EXTERNAL-PROOF-01` is the entire critical path; nothing else blocks live capital.
- **Backtesting / research pipeline:** already essentially complete; `PROMOTION-WALKFORWARD-GATE-WIRING-01` is the one remaining correctness gap, independent of everything else.
- **GUI completion:** `GUI-OPERATOR-ACTION-409-BODY-SURFACE-01` is a standalone fix with no dependencies.
- **Multi-symbol equities:** `MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01`, `MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01`, `MULTI-SYMBOL-CAPS-PREFLIGHT-WARNING-01` are independent of each other; none blocks another.
- **Multi-asset:** entirely blocked on a design/decomposition pass that has not happened yet; not on any other ledger item.

---

## 7. Execution Lanes (summary)

- **Lane A — Paper Soak:** 0 open items — `PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01` CLOSED at `e44e3ddd`; no reproducible in-flight soak-adjacent item remains.
- **Lane B — Green Parallel Completion:** 22 items, safe to work during the soak.
- **Lane C — Live Development:** 10 items (including 2 blocked sub-patches and 1 operator-only closure), on a separate branch/worktree.
- **Lane D — Post-Soak Shared Core:** 10 items, YELLOW/RED, wait for soak baseline acceptance (except where noted as buildable-on-branch-now).
- **Lane E — Multi-Asset Expansion:** 3 items, all XL, all requiring decomposition before implementation.
- **Lane F — Maintainability / Lean-out:** 2 items, both L, deferred until operational functionality is stable.

---

## 8. Standard Future Patch Workflow

Future patch sessions must:

1. Read this Master Patch Ledger first.
2. Verify repository HEAD and branch (`git rev-parse HEAD`, `git branch --show-current`, `git status --short`).
3. Locate the explicitly requested patch ID or the next eligible `READY` patch (dependencies CLOSED, correct lane for current soak state).
4. Verify all listed dependencies are CLOSED.
5. Implement ONLY that patch.
6. Do not broaden scope because neighboring code looks imperfect.
7. Preserve all accepted prior behavior.
8. Add a deterministic negative control when fixing a demonstrated bug.
9. Use validation proportional to risk (tiny patch → targeted test; RED patch → full relevant scenario suite).
10. Update the patch's ledger entry to `IMPLEMENTED_PENDING_REVIEW`, not `CLOSED`.
11. Record the implementation commit and proof results in the ledger.
12. Commit exactly the intended files.
13. Do NOT push.
14. STOP.
15. Independent review decides `CLOSED`/`ACCEPTED` versus `REPAIR REQUIRED`.

A patch becomes `CLOSED` only after independent review accepts the implementation. No implementation session may automatically begin the next patch. **ONE PATCH PER SESSION.**

---

## 9. No Reopening Rule

A `CLOSED` patch must not be reopened merely because further hardening is imaginable. Reopen only if there is a deterministic source defect, a reproducible failing test, an actual soak failure, a verified production mismatch, or a new requirement that explicitly changes the prior acceptance contract. Further optional improvements become new patch IDs.

---

## 10. Completion Definitions by Subsystem

**Paper Complete** means: autonomous startup succeeds; fresh data flows through the readiness gate; a strategy decision is made deterministically; risk evaluates the decision fail-closed; execution submits through the outbox atomically; broker truth (Alpaca WS, gap-detected-aware) is the sole source of fill/ack/cancel events; fills apply idempotently to durable portfolio state; reconciliation blocks arm/start on any drift; halt/recovery is sticky across restart and requires explicit re-arm; the operator has truthful, hard-blocked visibility into every stage via both GUI and (once `CLI-DAEMON-CONTROL-PASSTHROUGH-01` lands) CLI; and multi-session soak evidence exists. **Current state: met**, modulo one uncommitted fence patch pending harness proof.

**Live Complete** means everything in Paper Complete, plus: live credentials resolve through a single documented source; live account truth (including real buying power) is correct; the mode-transition state machine permits `LiveCapital` only after a real, signed trust-chain evidence artifact proves shadow-execution parity; a kill switch and flatten-on-halt are proven against the live endpoint (at minimum in LiveShadow); and a tiny-notional external proof has been executed and signed off by the operator. **Current state: infrastructure largely proven and shared correctly with paper; the trust-chain evidence gate is the single blocking gap.**

**Backtest Complete** means: the engine simulates fills conservatively with real transaction costs and no lookahead; metrics (Sharpe, drawdown, profit factor) are computed identically to the promotion gate's own scoring; artifacts are deterministic and DB-persisted; the GUI renders real equity curves and trade tables from those artifacts, not mock data; and the CLI/daemon expose the same capability. **Current state: met.**

**Research Pipeline Complete** means: Backtest Complete, plus: promotion gates fail closed on missing provenance, artifact-lock, and stress-suite evidence; and walk-forward/out-of-sample validation is enforced at the same authoritative gate, not left as an optional upstream step. **Current state: everything met except walk-forward enforcement** (`PROMOTION-WALKFORWARD-GATE-WIRING-01`).

**GUI/Operator Console Complete** means: every screen carrying snapshot data has an explicit `truth_state`; every live-data screen hard-blocks on unproven truth; every operator action route returns and *displays* a structured, actionable response including on failure; and no friendly defaults ever substitute for unproven state. **Current state: met** except the one 409-body-drop defect (`GUI-OPERATOR-ACTION-409-BODY-SURFACE-01`).

**Multi-Symbol Complete** means: concurrent (or deterministic sequential) per-symbol dispatch with failure isolation (one symbol's fault does not halt others); all five documented capital-protection caps are either enforced-by-default or loudly advisory when unset; and the watchlist-exceeds-cap case degrades gracefully rather than failing closed entirely. **Current state: dispatch is wired and live; failure isolation, cap defaults, and truncate-and-surface remain open** (`MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01`, `MULTI-SYMBOL-CAPS-PREFLIGHT-WARNING-01`, `MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01`).

**Maintainability Complete** means: no production file so large it materially slows review (a soft target, not a hard line); no duplicated safety-critical logic (e.g., the deadman-alert duplication); CI guards prevent load-bearing tests from silently staying ignored and prevent test-only feature flags from shipping in release builds; and documentation living-docs (README) do not carry stale point-in-time snapshots. **Current state: mostly met**; `state.rs`/`lifecycle.rs` size and the README staleness are the two open items.

---

## 11. Repository-Wide Definition of Done — MiniQuantDesk V4 Full Completion Contract

**CORE V4 COMPLETE** — the equity/ETF paper-trading loop (data → strategy → risk → execution → broker → portfolio → reconcile → operator visibility) runs autonomously, deterministically, fail-closed, restart-safe, and idempotently, with full scenario-test proof and zero known RED defects. **This bar is effectively met today**, pending the one uncommitted fence patch.

**MULTI-ASSET COMPLETE** — Equity and ETF trade fully; Crypto, Options, Futures, and Forex each have a real instrument model, broker adapter, risk policy, execution path, portfolio/P&L support, calendar/session handling, and GUI support, each proven by scenario tests to the same standard as equities. **Not met** — Crypto is data-only; Options/Futures/Forex are enum-stub-only. This is explicitly long-lead, Lane E, post-soak.

**PRODUCT/UI COMPLETE** — every operator-facing screen in the GUI truthfully reflects backend state with no fabricated defaults, and the CLI has parity with the GUI/HTTP surface for at least the operator-safety action set (arm/disarm/halt/clear/status). **Nearly met** — GUI discipline is proven; CLI parity is the open item.

**RESEARCH PIPELINE COMPLETE** — research → backtest → evaluate → promote → deploy is fully proven end-to-end including walk-forward/out-of-sample enforcement at the authoritative gate. **Nearly met** — one gate-wiring patch remains.

**LIVE PRODUCTION COMPLETE** — the live-capital path is proven under a controlled, staged rollout: shared infrastructure reused correctly from paper, live-specific account/credential truth correct, a real signed trust-chain evidence artifact permits `LiveCapital` cold-start, and a tiny-notional external proof has been executed. **Not met** — this is the single largest remaining program in the repository, correctly and deliberately gated off today.

**MAINTAINABILITY COMPLETE** — docs, tests, CI, and repo hygiene are acceptable: no stale living-doc snapshots, no oversized files in safety-critical hot paths without at least a decomposition plan, CI guards protect against known regression classes (ignored-test drift, testkit-in-release). **Nearly met** — README staleness and the two lean-out candidates are the open items.

**"100% complete" does not mean:** every possible strategy exists, every broker exists, no feature could ever be added, or no code could ever be improved. It means the defined V4 product scope — a deterministic, fail-closed, institutional-style equity/ETF paper-and-live trading platform with a proven research pipeline and truthful operator surfaces — is implemented, proven, documented, and operational, with multi-asset expansion tracked as an explicit, separately-scoped future program.

---

## 12. Recommended Order After Paper Soak Continues

```text
PAPER SOAK GO (PAPER_SOAK_GO, no known blocker; Lane A fence CLOSED at e44e3ddd)
        |
        +--> Start the 4-session supervised autonomous US equity/ETF Alpaca-paper soak
        |
        +--> Lane B GREEN work in parallel (any order, no soak risk):
        |         GUI-OPERATOR-ACTION-409-BODY-SURFACE-01
        |         CLI-DAEMON-CONTROL-PASSTHROUGH-01
        |         STRATEGY-*-UNIT-TESTS-01 (x3)
        |         PROMOTION-WALKFORWARD-GATE-WIRING-01
        |         README-SNAPSHOT-REFRESH-01
        |         BROKER-ALPACA-DEAD-CODE-CLEANUP-01
        |         (remaining Lane B doc/test items)
        |
        +--> LIVE development branch (Lane C, parallel, never merged into main without review):
        |         LIVE-ACCOUNT-TRUTH-01 → LIVE-SECRETS-CONSOLIDATION-01
        |         LIVE-TINY-CAPITAL-SMOKE-01 → LIVE-TRUST-CHAIN-* sequence
        |
        +--> collect soak findings (multi-session evidence accumulation)
                    |
                    v
             PAPER SOAK ACCEPTED (operator decision, not automatable)
                    |
                    +--> Lane D YELLOW/RED shared-core work merges (rate-limit retry, schema_version,
                    |     multi-symbol panic isolation/caps, Discord routing, deadman TTL reconcile)
                    +--> live integration continues toward LIVE-CAPITAL-EXTERNAL-PROOF-01
                    +--> remaining high-value systems (CLI parity follow-ups, dynamic-selection test density)
                    +--> Lane E multi-asset (after a dedicated decomposition pass)
                    +--> Lane F maintainability/lean-out
```

---

## 13. Historical / Superseded Patches

This audit did not find any active ledger item that duplicates already-closed work. The prior `MiniQuantDesk_Master_Patch_Ledger_v2.md` (21,291 lines) is retained as a historical append-only archive of past patch-implementation prompts and is not duplicated here. Memory records (`C:\Users\Zacha\.claude\projects\...\memory\MEMORY.md` and linked files) were treated as secondary to source truth throughout this audit per `.claude/rules/audit_repo_truth_rules.md`; two memory records were found stale during this audit and are flagged for correction outside this ledger (memory files are not repository content):

1. **Stale:** "Daemon defaults to real Alpaca WS unless `MQK_DAEMON_ADAPTER_ID=paper` forced" (from `project_premarket_ingest_plan_proof_01.md`). **Current truth:** `DEFAULT_DAEMON_DEPLOYMENT_MODE`/`DEFAULT_DAEMON_ADAPTER_ID` are both `"paper"` (`state.rs:193-194`); an unset environment resolves to Paper+Paper and `deployment_mode_readiness` additionally refuses to start that specific combination as "not an honest paper trading path." The safety trap described no longer exists as documented.
2. **Stale:** `FULL-REPO-COMPLETION-AUDIT-01` entry describing `BACKTEST-GUI-EXPERIENCE-01` as "UX polish = FUTURE." **Current truth:** `BacktestResultsScreen.tsx` (2,752 lines) already implements the described equity-curve/drawdown/Sharpe-tile polish.

No ledger patch ID from the legacy `v2.md` history was found to conflict with or require reopening based on this audit — all previously-closed patches referenced by memory that this audit was able to cross-check against current source (halt-fence lineage, partial-fill dedup, TradeActivity schema, calendar unification, multi-symbol dispatch phases 2-6, dynamic-selection Phase 7A-7C) remain consistent with committed HEAD.

---

## 14. Next 10 Patches

| Order | Patch | Lane | Impact | Priority | Why Now | Depends On |
|---|---|---|---|---|---|---|
| 1 | `GUI-OPERATOR-ACTION-409-BODY-SURFACE-01` | B | GREEN | P1 | Real operator-safety defect, one file, no dependencies. | NONE |
| 2 | `CLI-DAEMON-CONTROL-PASSTHROUGH-01` | B | GREEN | P1 | Closes the incident-response CLI/HTTP parity gap; pure passthrough, low risk. | NONE |
| 3 | `PROMOTION-WALKFORWARD-GATE-WIRING-01` | B | GREEN | P1 | Largest correctness gap in the research pipeline; self-contained. | NONE |
| 4 | `LIVE-ACCOUNT-TRUTH-01` | C | YELLOW | P1 | Real-money-relevant defect; should land early in the live-development branch. | NONE |
| 5 | `LIVE-TINY-CAPITAL-SMOKE-01` | C | GREEN | P1 | Zero capital risk, unlocks the entire live-trust-chain sequence. | NONE |
| 6 | `STRATEGY-MEAN-REVERSION-UNIT-TESTS-01` | B | GREEN | P2 | Closes a real proof gap on a currently-dispatchable strategy. | NONE |
| 7 | `STRATEGY-VOLATILITY-BREAKOUT-UNIT-TESTS-01` | B | GREEN | P2 | Same rationale as #6. | NONE |
| 8 | `STRATEGY-SWING-MOMENTUM-UNIT-TESTS-01` | B | GREEN | P2 | Same rationale as #6. | NONE |
| 9 | `README-SNAPSHOT-REFRESH-01` | B | GREEN | P2 | Trivial, prevents new operators from trusting stale status claims. | NONE |
| 10 | `BROKER-ALPACA-DEAD-CODE-CLEANUP-01` | B | GREEN | P3 | Removes confusing uncompiled duplicate code (`client.rs`/`config.rs`), zero risk to the live path. | NONE |

This is an operational queue, not a permanent ordering — future accepted patches may change the ranking.

---

## 15. Ledger Self-Consistency Check

- Every active patch has a unique ID: confirmed (52 distinct IDs across all lanes, including 3 decomposed live-trust-chain sub-patches and one operator-only closure marker).
- Every dependency references a real patch in this document: confirmed.
- No `READY` patch depends on an unresolved patch (`BLOCKED` patches are correctly marked `BLOCKED`, not `READY`): confirmed — `LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01/-PARITY-SCORER-01/-EVIDENCE-SIGNER-01` and `LIVE-CAPITAL-EXTERNAL-PROOF-01` are `BLOCKED`.
- Every patch has an impact classification (GREEN/YELLOW/RED): confirmed.
- Every patch has a lane: confirmed.
- Every patch has acceptance criteria and a CLOSED end state (Lane E/F items explicitly state their end state is "not yet defined pending decomposition," which is itself an honest, non-fabricated acceptance condition): confirmed.
- Every patch has validation requirements proportional to its risk: confirmed.
- Every patch has in-scope/out-of-scope: confirmed.
- Every major subsystem from the audit brief (A through X) appears in §2's completion map: confirmed.
- No already-CLOSED accepted work is duplicated as a new patch: confirmed (see §13).
- Deferred work is visibly separated from blockers (`DEFERRED` status used only for genuinely non-urgent or explicitly-postponed items; `BLOCKED` used only for real dependency chains): confirmed.
- Optional/speculative enhancements do not pollute the active queue (Lane E items are present but explicitly marked "must be decomposed before implementation," not treated as actionable S/M patches): confirmed.
- Paper-soak work is protected (Lane A contains only the one legitimate in-flight item; no other patch claims Lane A status): confirmed.
- Live work is distinguishable from shared-core work (Lane C vs. Lane D): confirmed.
- Percentages in §2 are evidence-backed with a one-line explanation each: confirmed.
- No patch is so broad it obviously requires multiple independent sessions, except the three Lane E items and the two Lane F items, which are explicitly and correctly flagged as requiring decomposition rather than being attempted directly: confirmed.

---

## 16. Validation of This Audit Session

```text
git diff --check                 -> clean (no whitespace/conflict-marker errors)
git status --short               -> shows only this ledger file as newly tracked/modified,
                                     plus the pre-existing untouched dirty state (control_plane.rs,
                                     state.rs, state/loop_runner.rs, scenario_clear_halted_run_auton04.rs,
                                     ignored_test_inventory.csv) and untouched smoke_logs/.
git diff -- MiniQuantDesk_Master_Patch_Ledger_v2_updated.md -> full new content (file was untracked
                                     before this session; now staged as the authoritative ledger).
```

No Rust test matrix, no clippy, no DB, no broker calls were run in this audit session — this was documentation-only per the mission's Mode directive in the governing prompt.

---

## 17. Validation History — `FINAL-CANONICAL-PRE-SOAK-VALIDATION-01`

**Validation date:** 2026-08-10
**Validation HEAD:** `e44e3ddd6b41b32e5285436226100d2b867829b0` (unchanged going in; this ledger-only commit is the only change produced by this session)
**Mode:** VALIDATION ONLY — no application source (Rust/Python/GUI) modified. Only this ledger file changed.

**Git safety:** branch `main`; HEAD, `origin/main` HEAD, and expected baseline all equal `e44e3ddd`; `git status --short` showed only untracked `smoke_logs/` (protected, untouched) going in.

**Commands / test families run:**
```text
bash scripts/guards/check_migration_governance.sh
pwsh scripts/windows/Invoke-CanonicalSafeIgnoredMatrix.ps1   (MQK_DATABASE_URL -> 127.0.0.1:5434/mqk_test)
pwsh scripts/windows/Invoke-CanonicalFmtCheck.ps1
cargo clippy --manifest-path core-rs/Cargo.toml --workspace --all-targets -- -D warnings
pwsh scripts/guards/check_unsafe_patterns.ps1
bash scripts/guards/check_ignored_load_bearing_proofs.sh
bash scripts/guards/check_disposable_db_not_in_production.sh
bash scripts/guards/check_workspace_dep_inheritance.sh
bash scripts/guards/check_ci_local_toolchain_convergence.sh
pwsh scripts/guards/check_no_promotion_evidence_bypass.ps1
pwsh scripts/guards/check_no_phase7a_production_effects_bypass.ps1
pwsh scripts/guards/validate_autonomous_daily_paper_operations_01g_bundle_3_final_closure.ps1
git diff --check
cargo test --manifest-path core-rs/Cargo.toml --workspace --no-run
cd core-rs/mqk-gui && npm run test && npm run build
pwsh scripts/windows/Get-PaperOperatorStatus.ps1   (read-only; daemon intentionally not started)
```

**Migration validation:** PASS — `check_migration_governance.sh` confirms the manifest matches the authoritative SQL chain (0001-0064, no unauthorized migration directories); `migrate_idempotent_on_clean_db`, `migration_bootstrap_and_replay_follow_authoritative_manifest`, and the `test_support_disposable_db` migration-owner tests all executed and passed as part of the canonical matrix below.

**Ignored-test inventory:** PASS — `missing=0, unknown=0, duplicate=0, stale=0`. 742 total inventory rows (8 SAFE_LOCAL, 725 SAFE_DB_5434, 9 MANUAL_EXTERNAL, 0 BLOCKED_LOCAL_PREREQUISITE), self-validation clean, live-vs-inventory completeness clean, MANUAL_EXTERNAL feature-difference exact (9/9).

**Canonical safe-ignored regression matrix (`Invoke-CanonicalSafeIgnoredMatrix.ps1`, full run, not `-ListOnly`):** PASSED. 733/733 SAFE_LOCAL+SAFE_DB_5434 tests green, 0 failures, safe execution exit code 0; 9 MANUAL_EXTERNAL tests compile-proven (`--no-run`, exit code 0). This single canonical run is the proof source for: H01-H08 (local quiescence / halt-clear, including the new H08), daemon supervisor halt fence (DSF), runtime halt fence CAS (RHF), stale-claim recovery (SCR03), deadman (`scenario_deadman_enforces_halt`, `scenario_deadman_after_start_01`), durable paper portfolio/P&L, fill/partial-fill/replay authority, outbox/pre-submit authority, and risk/kill-switch/PDT/reconcile scenario families — all classified `SAFE_LOCAL`/`SAFE_DB_5434` in the inventory and all executed as part of this one matrix run.

**Build/static validation:** `cargo fmt --check` PASS (21/21 workspace packages via the canonical per-package Windows runner); `cargo clippy --workspace --all-targets -- -D warnings` PASS (exit 0, zero warnings across the full workspace, superset of the paper-critical crate list); unsafe-pattern guard PASS; `git diff --check` clean; workspace `cargo test --workspace --no-run` PASS (exit 0, all test binaries compiled).

**GUI:** `npm run test` — 977/977 pass, 0 fail; `npm run build` (tsc + vite) — clean, zero type errors (only non-blocking chunk-size warnings from vite's bundler).

**Autonomous daily paper operations:** `validate_autonomous_daily_paper_operations_01g_bundle_3_final_closure.ps1` — 1 non-blocking violation found: its nested `validate_daily_data_readiness_01e_closure.ps1` check `[20]` asserts `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` must never be git-tracked. That assumption predates and is superseded by this repo's own deliberate, documented decision (commit `e3a87c4a`, "docs: establish authoritative V4 completion ledger") to track this file as the new authoritative ledger. This is a stale documentation-tracking-policy assumption in an older guard, not a reproducible economic/safety/execution/risk/reconcile defect — classified non-blocking per validation-mission scope (does not touch any halt/execution/risk/portfolio/broker path). Recorded here rather than spawning a new patch ID; correcting the stale guard assumption is optional future GREEN backlog work.

**Data/session readiness:** PASS — `mqk-integrity/src/calendar.rs` hardcodes and tests US market holidays/early-closes through 2028, with the full 2026 table present (New Year's, MLK, Presidents', Good Friday, Memorial Day, Juneteenth, Independence Day (observed), Labor Day, Thanksgiving + day-after early close, Christmas + Christmas Eve early close). No 2026 calendar gap exists.

**Paper environment readiness (read-only inspection, no mutation):**
- Paper DB `127.0.0.1:5440` reachable (not wiped, not migrated in this session).
- `.env.local` (this machine) resolves `MQK_DATABASE_URL` to `127.0.0.1:5440/miniquantdesk_paper`, `MQK_DAEMON_ADAPTER_ID=alpaca`, `ALPACA_BASE_URL`/`ALPACA_PAPER_BASE_URL=https://paper-api.alpaca.markets`. `ALPACA_LIVE_BASE_URL` is present in the file but confirmed by an in-source comment (`ENV-TRUTH-02`) to never be read by the daemon.
- Source-level default (`state.rs:193-194`): `DEFAULT_DAEMON_DEPLOYMENT_MODE`/`DEFAULT_DAEMON_ADAPTER_ID` are both `"paper"`; an explicit Paper+Paper combination is refused at `deployment_mode_readiness` (`state/env.rs:146-156`) as "not an honest paper trading path" (no bar-feed wired to `LockedPaperBroker`), forcing the only authoritative paper route through Paper+Alpaca.
- Daemon was intentionally not started during this validation (per mission scope). `Get-PaperOperatorStatus.ps1` (read-only, no mutation, no broker call) was run and honestly reported every daemon-backed field as `UNAVAILABLE` — daemon offline, port 8899 not responding. This is expected, not a defect: runtime lease, halted-run, reconcile/risk, and arm-state truth can only be observed once the daemon is started for the actual soak session.
- No secret values were printed at any point in this validation.

**Alpaca paper connectivity:** `ALPACA_PAPER_CONNECTIVITY=NOT_EXECUTED, reason=no_canonical_read_only_probe` — no standalone script exists that queries Alpaca paper connectivity independent of the daemon's own `/api/v1/system/status` route, and starting the daemon was out of scope for this validation. Per protocol, this alone is not disqualifying: the daemon's own configured readiness gate (`deployment_mode_readiness`, confirmed above) performs this check at actual startup.

**Live capital exposure:** `NONE`. Confirmed via: (1) source-level default deployment mode/adapter both `"paper"`; (2) Paper+Paper fails closed by design, forcing Paper+Alpaca as the only honest paper route; (3) `LiveCapital` cold-start remains gated behind `live_trust_complete`, hardcoded `false` in the TV-03 evidence pipeline (`parity_evidence.rs`, `api_types.rs`) — unchanged, not reopened, not weakened; (4) this machine's live-capital-adjacent env var (`ALPACA_LIVE_BASE_URL`) is present but confirmed unread by the daemon; (5) no live DB (`127.0.0.1:5432`) was connected to, read, or written at any point in this session.

**Final decision:** `PAPER_SOAK_GO`. All 24 GO criteria in the governing validation protocol are met; no reproducible soak-blocking defect was found against any accepted paper-soak contract. The one prior open item (`PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01`) is now CLOSED per the acceptance record in §5 above.

**Non-blocking findings (backlog, not new P0 patches):**
1. `validate_daily_data_readiness_01e_closure.ps1` check `[20]`'s ledger-tracking assumption is stale relative to `e3a87c4a`'s deliberate decision — optional future doc-guard correction, GREEN.
2. Halt-notification delivery through Discord remains asynchronous/best-effort (existing, previously-recorded observability item, YELLOW backlog — Discord is an outbound signal rail, not trading authority, and cannot submit orders, mutate run state, or bypass risk/reconcile).

---

*End of MiniQuantDesk V4 Authoritative Master Completion Ledger — FULL-REPO-COMPLETION-AUDIT-01, updated by FINAL-CANONICAL-PRE-SOAK-VALIDATION-01.*
