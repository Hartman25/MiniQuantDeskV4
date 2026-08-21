# MiniQuantDesk V4 — Authoritative Master Completion Ledger

**Audit:** FULL-REPO-COMPLETION-AUDIT-01
**Audit date:** 2026-08-10
**Mode:** AUDIT + LEDGER ONLY — no application code, DB, or trading behavior was modified in this session.
**Branch:** `main`
**Starting HEAD:** `0a019b8bd80298ac0a04ba77fb080522122c37a8` ("fix: fence daemon supervisor safety halts")
**origin/main HEAD (matches local):** `0a019b8bd80298ac0a04ba77fb080522122c37a8` (state at this audit's 2026-08-10 start — see repo-truth refresh note immediately below for current state)

**Repo-truth refresh (`MASTER-LEDGER-REPO-TRUTH-REFRESH-02`, 2026-08-21):** Verified directly against Git, not inferred from prior ledger wording. `origin/main` HEAD is now `fd90f63a1529e740acb727845cc05ab59ea25def` ("docs: correct master ledger after wave 2 review"). `git merge-base --is-ancestor b80749bd origin/main` confirms **Wave 2 (P7A/P7B/LONG-SHORT/P7C, through commit `b80749bd`) is pushed** — every place below that previously read "not pushed" / "`origin/main` still equals `f8357ebc`" was stale and is corrected in place. Local `main` HEAD is `242cb7c31a69ddfaf25ad294e6066467addc935f` ("promotion: wire verified research evidence into production gate"), exactly one commit ahead of `origin/main` and **not pushed**. That commit is a real, non-docs Rust change that implements the `PROMOTION-WALKFORWARD-GATE-WIRING-01` production-wiring invariant. This was recorded at the time as `IMPLEMENTED_PENDING_INDEPENDENT_REVIEW`; independent review has since occurred (`MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01`, 2026-08-21, same day) and found material gaps — see the updated §5/§24 entries for the current status (`IN PROGRESS / PARTIAL — REPAIR REQUIRED`, not `READY`, not `CLOSED`, not `IMPLEMENTED_PENDING_INDEPENDENT_REVIEW`).

**Worktree:** primary working tree at `C:\Users\Zacha\Desktop\MiniQuantDeskV4` (several other worktrees/clones exist under `.claude/worktrees/`, `.codex/worktrees/`, and a sibling `MiniQuantDeskV4-ai-lab` dir — not inspected in this audit, out of scope).
**Repository dirty/untracked state at audit start:**
- Modified, uncommitted: `core-rs/crates/mqk-daemon/src/routes/control_plane.rs`, `core-rs/crates/mqk-daemon/src/state.rs`, `core-rs/crates/mqk-daemon/src/state/loop_runner.rs`, `core-rs/crates/mqk-daemon/tests/scenario_clear_halted_run_auton04.rs`, `scripts/test/ignored_test_inventory.csv` (net +447/-8 lines). This is a coherent, self-contained in-progress patch — see `PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01` below. It was **read as-is** (current repo truth) but **not committed, not modified, not run** by this audit.
- Untracked: `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` (this file — now tracked as the authoritative ledger), `smoke_logs/` (protected, untouched, generated artifacts).
- This is the **paper-soak baseline branch** (`main`), not a derived development branch. Per the Paper-Soak Protection Rule below, this audit treated all in-scope code as protected and made no trading-path changes.

This document is the whole-repository completion ledger for MiniQuantDeskV4. It supersedes `MiniQuantDesk_Master_Patch_Ledger_v2.md` (a 21k-line append-only session-prompt log, `~1.6MB`, kept as historical archive — not deleted) as the **current-status source of truth**. Future sessions should read *this* file first, locate the next eligible `READY` patch, implement exactly that one patch, and stop.

**Precedence (updated by `MASTER-LEDGER-CONSOLIDATION-01`, 2026-08-17):**

```text
CURRENT repo / code / tests / Git history / proof artifacts
        >
THIS master ledger (MiniQuantDesk_Master_Patch_Ledger_v2_updated.md)
        >
historical audits / closure docs / any other status/tracker document
```

Any other document in this repository — including `MiniQuantDesk_Master_Patch_Ledger_v2.md`, `docs/research/Research_Backtest_V1_Closeout_Audit.md`, `docs/specs/roadmap_completion_reconcile_01.md`, and every `docs/audits/*`/`docs/specs/*_closure_decision.md` file — may retain unique technical history, methodology, or accepted-evidence detail worth reading, but **none of them is authoritative for current remaining-work status**. Where any such document's stated status conflicts with this ledger, this ledger wins unless a session finds a deterministic contradiction in current repo truth (code/tests/Git) — in which case STOP and report the conflict per CLAUDE.md §6, rather than silently trusting either document.

---

## 0. Executive Summary

### Current Repository Verdict
The equity/ETF paper-trading core (orchestrator, OMS state machine, outbox/inbox, broker adapters, risk, portfolio, reconciliation, backtest engine, promotion gates, GUI truth-state discipline) is **evidence-provably complete and fail-closed** at HEAD. No RED (soak-blocking) source defect was found anywhere in the audited codebase. The repository's real remaining gaps cluster in three places: (1) live-capital readiness — deliberately and completely gated off pending a trust-chain proof that doesn't exist yet; (2) operational hardening around multi-symbol dispatch resilience, CLI/daemon control-plane parity, and Discord alert coverage; (3) one uncommitted-but-well-formed patch closing a narrow halt/deadman race that needs a harness run before it can be called closed.

### Current Paper Verdict
**PAPER_SOAK_GO** (`FINAL-CANONICAL-PRE-SOAK-VALIDATION-01`, 2026-08-10, HEAD `e44e3ddd`). The one previously-open item, `PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01`, is now CLOSED — its H08 test passed against a real local Postgres as part of a full canonical safe-ignored matrix run (733/733 tests green: H01-H08, daemon-supervisor halt fence, runtime halt fence CAS, stale-claim recovery, deadman, durable portfolio/P&L, fill/replay authority, outbox/pre-submit authority, risk/kill-switch/reconcile all proven with zero failures). All previously-tracked blockers (TradeActivity schema mismatch, partial-fill dedup, stale-claim recovery, terminal-fill replay parity) have corresponding committed fixes at HEAD, and this validation reproduced no new regression against any of them. No known accepted-list paper-soak code blocker remains.

### Current Live Verdict
**NOT READY, and cannot become ready without new work.** `LiveCapital` cold-start is hard-gated behind a trust-chain proof (`live_trust_complete`) that is **hardcoded `false`** in `research-py`'s TV-03 pipeline — this is by design, not a bug, and correctly enforced at both the advisory and cold-start-enforcement layers. Separately, live account truth is wrong today: `buying_power` is aliased to `cash` rather than pulled from Alpaca's real `buying_power`/`daytrading_buying_power` fields, which is economically dangerous for a margin account. No live-capital smoke-test tooling exists. A prior memory record claiming "daemon defaults to real Alpaca WS unless forced to paper" is **stale** — current default (`Paper`/`Paper`) is fail-closed and safe; this session is correcting that memory record.

### Current Research/Backtest Verdict
**WAVE_2 ACCEPTED_LOCALLY — PUSHED** (repo truth corrected by `MASTER-LEDGER-REPO-TRUTH-REFRESH-02`, 2026-08-21 — `b80749bd` is confirmed an ancestor of `origin/main`; see §24 for full detail). The DSR/PBO multiple-testing promotion-evidence gate (P7A execution pricing → P7B weight-to-share/discrete economics → P7C durable registry-anchored OOS evidence gate) is implemented, locally tested, independently reviewed and accepted, and **pushed to `origin/main`** at commit `b80749bd` (commits `81dcf621` P7B-REPAIR-03 and `b80749bd` P7C-REPAIR-04). Separately, an unpushed local commit (`242cb7c3`, one commit ahead of `origin/main`) implements the `PROMOTION-WALKFORWARD-GATE-WIRING-01` production-wiring invariant: it wires the accepted `verify_promotion_oos_evidence` mechanism into the real daemon promotion-transition route as an additional fail-closed gate. Focused unit tests for the new gate module pass (11/11), and the underlying `mqk-promotion` crate's full suite remains green and unmodified; the two DB-backed integration/scenario tests that exercise it end-to-end could not be run to completion this session (blocked by a pre-existing, unrelated local test-Postgres migration-checksum drift, not a defect in the patch) — see §5/§24. Status is corrected (2026-08-21, `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01`) to `IN PROGRESS / PARTIAL — REPAIR REQUIRED`: independent review of `242cb7c3` has already occurred and found material deterministic gaps (cross-candidate authority gap, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam — see §5 for full detail); it is local-only, unpushed, **not** independently accepted, **not** `CLOSED`. The immediate missing prerequisite is a new tracked item, `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` (§5), `OPEN`, not yet started. `docs/research/Research_Backtest_V1_Closeout_Audit.md` (dated 2026-08-15) predates this chain's later commits and is stale on this point — see the precedence note above. P9 (robustness gauntlet) remains `OPEN` — its Wave-2-push dependency is now satisfied, but P9 itself has not started. P10 (final acceptance) remains `OPEN`, blocked on P9 **and** on `PROMOTION-WALKFORWARD-GATE-WIRING-01` closing (production wiring — implemented locally, not yet pushed, repaired per independent review findings, or proven end-to-end via the DB-backed harness). `RESEARCH_BACKTEST_V1_COMPLETE` is **NOT MET**.

### Closest Subsystems to Completion
Core Execution/OMS (~97%), Database Layer (~97%), Reconciliation (~97%), Risk System (~95%), Paper Trading Lifecycle (~95%), Backtesting Engine (~95%), Test Infrastructure (~95%).

### Highest-Risk Incomplete Subsystems
Live Capital Trading (~40%, gated by design but genuinely far from proven), CLI/Daemon control-plane parity (~60%, no CLI path to arm/halt/clear the live daemon), Discord/Alerting coverage (~70%, multi-channel routing built but unused, no data-staleness/daily-summary pushes), Options/Futures/Forex (~5%, enum + risk-multiplier stub only, explicitly gated off).

### Active Patch Counts
READY: 33 · BLOCKED: 4 · DEFERRED: 8 · IMPLEMENTED_PENDING_REVIEW: 0 · CLOSED (this session): 0

*Counts above cover Lanes A-F (Paper/equity/GUI/live/multi-asset/maintainability) as recorded by the 2026-08-10 `FULL-REPO-COMPLETION-AUDIT-01` audit. `MASTER-LEDGER-CONSOLIDATION-01` (2026-08-17) had incorrectly reclassified `PROMOTION-WALKFORWARD-GATE-WIRING-01` as `CLOSED — SUPERSEDED`; `MASTER-LEDGER-TRUTH-REPAIR-01` (2026-08-17, same day) restored it to `READY`; `MASTER-LEDGER-REPO-TRUTH-REFRESH-02` (2026-08-21) corrected it to `IMPLEMENTED_PENDING_INDEPENDENT_REVIEW`; `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01` (2026-08-21, same day) further corrects it to `IN PROGRESS / PARTIAL — REPAIR REQUIRED` (see §5, §24) — independent review of the unpushed local commit implementing production wiring has since occurred and found material deterministic gaps (cross-candidate authority, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam), with a new prerequisite item `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` now tracked `OPEN`. Otherwise these counts were not re-verified this session. Research/Backtest (P7-P10, §24) and Operations Resilience (OPS-*, §25) are tracked separately and are NOT included in these counts: as of 2026-08-21, Research/Backtest has Wave 2 (P7A/P7B/P7C) `ACCEPTED_LOCALLY — PUSHED`, `PROMOTION-WALKFORWARD-GATE-WIRING-01` `IN PROGRESS / PARTIAL — REPAIR REQUIRED` (local-only, unpushed, independent review found gaps), `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` `OPEN` (new), and 2 items `OPEN` (P9, P10); OPS-* has 3 `OPEN` and 1 `DEFERRED`.*

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
| 14 | Strategy Research / Promotion | ~93% (corrected 2026-08-21, `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01`, see §24) — gate mechanics (NaN, tie-break, artifact-lock, stress-suite, provenance) fully proven fail-closed; DSR/PBO multiple-testing OOS-evidence gate implemented, registry-anchored (`mqk-promotion::verify_promotion_oos_evidence`), independently reviewed, accepted, and **pushed to `origin/main`** (Wave 2, `b80749bd`); production wiring exists in an unpushed local commit (`242cb7c3`, unit-tested) but independent review of that commit has since found material gaps (cross-candidate authority, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam) — robustness gauntlet (P9) and final acceptance (P10) remain open, and a new prerequisite (`PROMOTION-BACKTEST-EVIDENCE-SEAM-01`) is now tracked. | ACCEPTED_LOCALLY — PUSHED (gate mechanism) / IN PROGRESS / PARTIAL — REPAIR REQUIRED (production wiring, local-only, unpushed, independent review found gaps) / OPEN (`PROMOTION-BACKTEST-EVIDENCE-SEAM-01`, P9, P10) | 4 (`PROMOTION-WALKFORWARD-GATE-WIRING-01`, `PROMOTION-BACKTEST-EVIDENCE-SEAM-01`, P9, P10 — see §5, §24) | GREEN | B |
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
6. **Walk-forward promotion gate production wiring** (`PROMOTION-WALKFORWARD-GATE-WIRING-01`, corrected to `IN PROGRESS / PARTIAL — REPAIR REQUIRED` 2026-08-21, see §5/§24) — Wave 2 is pushed, and an unpushed local commit (`242cb7c3`) wires the accepted P7C DSR/PBO OOS-evidence mechanism (`verify_promotion_oos_evidence` / `VerifiedPromotionOosEvidence`) into the real daemon promotion route, but independent review of that commit has since found material gaps (cross-candidate authority, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam). No longer the fastest opportunity: the remaining step is repairing those gaps — starting with the new prerequisite `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` (§5, `OPEN`) — then pushing and proving end-to-end via the DB-backed scenario/closure-proof harness (blocked this session by an unrelated local test-DB migration-checksum drift).
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

#### MARKET-DATA-PROVIDER-PROVENANCE-01 — Fix provider-provenance defect in the normal market-data provider-sync path

**Status:** ACCEPTED_PENDING_INTEGRATION
**Priority:** P0
**Paper Impact:** RED
**Subsystem:** mqk-cli market-data ingest / mqk-daemon daily-data-readiness evaluator

**Current Source Truth:** Implemented in isolated worktree `C:\Users\Zacha\Desktop\MiniQuantDeskV4-data`, branch `fix-market-data-provider-provenance`, base `54082a448c84b6429713a429bfb9403da8822131` (`origin/main`). Not merged into the primary worktree/branch as of this writing.

**Problem (2026-08-11 PAPER incident):** `mqk-cli md ingest-provider` and `mqk-cli md sync-provider` called the metadata-less `mqk_db::md::ingest_provider_bars_to_md_bars` (defaults `provider_id="unknown"`) instead of the metadata-aware `..._with_provider_metadata` variant, even though the CLI already knows the actual selected provider (`source_lc`) at the call site. Every row written by the normal provider-sync CLI path landed with `provider_id="unknown"` regardless of `--source alpaca` or `--source twelvedata`, which the daily-data-readiness evaluator (`mqk-daemon/src/daily_data_readiness.rs`) treats as `REASON_PROVIDER_PROVENANCE_INVALID` — permanently blocking the market-data readiness gate for any symbol ingested this way. Separately, TwelveData was observed returning only stale prior-day intraday bars for AAPL/5m while Alpaca returned fresh same-day data, and the daemon's own `POST /api/v1/ingest/jobs mode=sync_provider` route (`routes/ingest.rs::run_real_provider_sync`) already wrote truthful `provider_id="alpaca"` via the same metadata-aware helper — proving the DB schema and provider layer were never the defect, only the CLI call sites.

**Why This Matters:** This directly blocked the daily-data-readiness/instrument-registry gate for the currently-approved paper trading universe (AAPL/5m via Alpaca, per `.env.local`: `MQK_STRATEGY_SYMBOL=AAPL`, `MQK_STRATEGY_MD_TIMEFRAME=5m`, `MQK_DAEMON_ADAPTER_ID=alpaca`, no watchlist override).

**Root Cause:** `md_ingest_provider`/`md_sync_provider` (`core-rs/crates/mqk-cli/src/commands/md.rs`) called `mqk_db::md::ingest_provider_bars_to_md_bars(pool, IngestProviderBarsArgs{..})` with no metadata argument, which internally defaults to `MdBarProviderMetadata::unknown()`. The already-known `source_lc` was never threaded into a metadata struct, unlike `ingest_csv_to_md_bars` (CRYPTO-DATA-01F precedent) and `md_kraken_ohlc_ingest`, which both already used the metadata-aware path correctly.

**Fix:** Both CLI commands now route through a new `ingest_provider_bars_with_truthful_provenance` helper that groups the fetched bars by symbol and issues one `ingest_provider_bars_to_md_bars_with_provider_metadata` call per symbol (mirroring `run_real_provider_sync`'s existing per-instrument pattern), stamping `provider_id`/`provider_source = source_lc` and `ingest_mode = "provider_ingest"`/`"provider_sync"` on every row. `provider_symbol` is populated only when genuinely known: a new `resolve_symbols_with_provider_symbol` (superset of the existing `resolve_symbols`) carries the registry's real `provider_symbol` through for `--symbols-from-registry`, and stays `None` for a raw `--symbols` list (never forged, per D10). The metadata-less `ingest_provider_bars_to_md_bars` helper itself is unchanged — it remains the honest "provider truly unknown" path for any caller that doesn't know the provider.

**Registry Decision:** `config/instruments/equities.json`'s `AAPL` entry changed from `provider="twelvedata"`, `timeframes=["1D"]` to `provider="alpaca"`, `timeframes=["1D","5m"]` — scoped to AAPL only (the sole symbol in the current approved paper universe), not a bulk equity-universe conversion. The primary worktree (`C:\Users\Zacha\Desktop\MiniQuantDeskV4`) independently carries an equivalent temporary same-day operational edit to the same file/field (uncommitted, made 2026-08-11 during the live incident response) — this ledger entry and that primary-worktree edit will need reconciliation when this patch is reviewed and merged; neither this session nor this patch touched the primary worktree's copy.

**Readiness Proof (`mqk-daemon/tests/scenario_daily_data_readiness_01.rs`, `ddr_62`/`ddr_63`):** Bars written through the exact production metadata-aware ingest call (never a raw DB `INSERT`/`UPDATE`) and read back through `mqk_db::md::fetch_bounded_bars_with_provenance`, then evaluated by the production `evaluate_bar_readiness` function. `ddr_62`: `source=alpaca` matching the expected provider yields zero provenance-invalidating blockers (`provenance_state`-equivalent = `ok`). `ddr_63`: `source=twelvedata` against an `alpaca`-expecting caller still blocks under `REASON_PROVIDER_ID_MISMATCH` (provenance validation not weakened).

**Dependencies:** NONE
**Unlocks:** `AUTONOMOUS-DAILY-OPERATOR-RETRY-01`, `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01`, `INSTRUMENT-UNIVERSE-REFRESH-01` (all OPEN, not started by this patch)
**In Scope:** `mqk-cli` provider-sync/ingest-provider call sites, `resolve_symbols` extension, targeted DB/CLI/readiness proof tests, the single-symbol AAPL registry decision.
**Out of Scope:** Autonomous retry/reset, full scheduler/freshness redesign, official launcher, risk/OMS/portfolio/broker/GUI/Discord/futures/options/crypto/live trading, bulk equity-universe provider conversion.
**Likely Files / Surfaces:** `core-rs/crates/mqk-cli/src/commands/md.rs`, `core-rs/crates/mqk-cli/Cargo.toml`, `core-rs/crates/mqk-daemon/tests/scenario_daily_data_readiness_01.rs`, `config/instruments/equities.json`.
**Required Implementation Rules:** Never infer provider identity from symbol/URL/API-key/DB-state/registry-guess; never forge `provider_symbol`; never change the metadata-less helper's `"unknown"` default semantics.
**Safety / Compatibility Requirements:** Provenance/freshness/continuity/registry validation must not be weakened (proved by `ddr_63`'s negative control).
**Required Negative Controls:** `ddr_63_provider_provenance_mismatch_still_blocks`.
**Required Positive Controls:** `ddr_62_provider_provenance_ok_when_ingested_with_truthful_metadata`; `dbp_01`/`dbp_02` (alpaca/twelvedata truthful `provider_id` round-trip); `dbp_03` (unmapped symbol never forges `provider_symbol`).
**Required Regression Tests:** `mqk-db --test scenario_md_ingest_provider` (13/13, unchanged); `mqk-daemon --test scenario_daily_data_readiness_01` (66/66, all prior DDR-01..61 unchanged); `mqk-cli --bin mqk-cli` unit tests (28/28, including pre-existing `resolve_symbols` RS-01..08).
**Required Validation:**
```powershell
$env:MQK_DATABASE_URL = "postgresql://postgres:postgres@127.0.0.1:5434/mqk_test"
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- --include-ignored
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-db --test scenario_md_ingest_provider -- --include-ignored
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_daily_data_readiness_01 -- --test-threads=1
bash scripts/guards/check_unsafe_patterns.sh
bash scripts/guards/check_workspace_dep_inheritance.sh
git diff --check
```
**Forbidden Validation / Side Effects:** No live/paper DB, no real provider network call, no manual DB provenance edits, no orders.
**Acceptance Criteria:**
1. `--source alpaca` and `--source twelvedata` durably persist their true `provider_id` (never `unknown`).
2. Readiness evaluator sees `provenance_state=ok` for correctly-provenanced rows and still blocks a genuine mismatch.
3. `provider_symbol` never forged for symbols with no registry mapping.
4. All listed regression suites remain green.
5. Primary and ops worktrees unmodified by this patch.
**Exact CLOSED End State:** Not yet CLOSED — `IMPLEMENTED_PENDING_REVIEW` until code-reviewed, the primary-worktree AAPL registry edit is reconciled, and the patch is merged.
**Expected Handoff:** Start HEAD `54082a44` (dev worktree base = `origin/main`); end HEAD = new commit SHA on `fix-market-data-provider-provenance`; not pushed, not merged.

---

#### MARKET-DATA-PROVIDER-PROVENANCE-01-REPAIR-01 — Operational repair of the AAPL/5m automatic provider-provenance path

**Status:** IMPLEMENTED_PENDING_REVIEW
**Priority:** P0
**Paper Impact:** RED
**Subsystem:** mqk-cli market-data ingest / instrument registry / Windows premarket+intraday scripts

**Current Source Truth:** Implemented in isolated worktree `C:\Users\Zacha\Desktop\MiniQuantDeskV4-data`, branch `fix-market-data-provider-provenance`, on top of `dae446b337b77245417a4cc982ff7fa22736b098`. Not merged.

**Problem (independent GitHub review of `dae446b3`):** The core provider-id fix was directionally correct but left the *normal automatic* Paper market-data path still unable to satisfy provenance:
- Raw `--symbols AAPL` (the mode `Refresh-IntradayMarketData.ps1` actually invokes) resolved to `provider_symbol=None`, which readiness treats as `REASON_PROVIDER_PROVENANCE_INVALID`, even after `provider_id` was fixed.
- `--symbols-from-registry` loaded ALL enabled equities with no provider/timeframe scoping, so `md sync-provider --source alpaca --symbols-from-registry` could select a `twelvedata`-only instrument and stamp it `provider_id=alpaca`.
- `Refresh-IntradayMarketData.ps1` defaulted `-Source` to `twelvedata` and `Start-PaperTradingSmoke.ps1`'s `-StartIntradayRefreshLoop` never passed `-Source`, so the scheduled AAPL/5m refresh would still hit TwelveData (the exact 2026-08-11 failure mode) despite the registry saying `alpaca`.
- `Prep-PremarketMarketData.ps1`'s provider-sync top-off stage unconditionally called `--source twelvedata` regardless of `-Timeframe`; `Start-PaperTradingSmoke.ps1` STEP 5B calls it with `-Timeframe $env:MQK_STRATEGY_MD_TIMEFRAME` (=`5m` per `.env.local`), so the default smoke path was actively writing `twelvedata`-labeled AAPL/5m rows into the readiness window before the later Alpaca top-off ran — `evaluate_bar_readiness` checks provenance on every bar in the window, not just the latest, so this silently broke provenance on every default smoke run.
- The AAPL registry entry claimed `timeframes=["1D","5m"]` under `provider=alpaca`, but `daily_data_readiness::resolve_daily_bar_timestamp_convention` treats `(alpaca, 1D)` as `Unverified` (no committed fixture/parser proof) — claiming it as authorized was untruthful.

**Fix:**
1. `resolve_symbols_with_provider_symbol`'s raw `--symbols` branch now returns `Some(symbol)` instead of `None` — verified true because `AlpacaHistoricalProvider`/`TwelveDataHistoricalProvider::fetch_bars` forward the raw symbol to the provider request unmodified (not a forged registry alias).
2. New `resolve_provider_scoped_registry_instruments`/`resolve_symbols_for_provider_operation` in `mqk-cli` mirror `mqk-daemon::routes::ingest::resolve_provider_scoped_equities`'s admission contract (enabled + asset_class=equity + provider==source + timeframe authorized) for `--symbols-from-registry`, duplicated narrowly rather than adding an `mqk-cli -> mqk-daemon` crate dependency.
3. `Refresh-IntradayMarketData.ps1`'s `-Source` now defaults to `''` and auto-derives per-symbol from the registry (same admission contract), failing closed (no guessing, no silent multi-provider pick) rather than defaulting to `twelvedata`. `Start-PaperTradingSmoke.ps1` was left untouched — not passing `-Source` now correctly inherits registry auto-derivation.
4. `Prep-PremarketMarketData.ps1`'s provider-sync top-off resolves each symbol's provider from the registry (scoped to that symbol/timeframe) and skips the stage (warn, non-fatal) rather than guessing when it doesn't resolve to exactly one instrument.
5. `config/instruments/equities.json` AAPL entry narrowed to `timeframes=["5m"]` — historical 1D rows are not deleted, only automatic provider/timeframe authorization is narrowed.

**Dependencies:** `MARKET-DATA-PROVIDER-PROVENANCE-01`
**Unlocks:** Nothing new — repairs the same operational gap `MARKET-DATA-PROVIDER-PROVENANCE-01` was meant to close.
**In Scope:** `mqk-cli` symbol-resolution helpers, `Refresh-IntradayMarketData.ps1`, `Prep-PremarketMarketData.ps1`, AAPL registry entry, targeted CLI/script tests.
**Out of Scope:** Autonomous retry (`AUTONOMOUS-DAILY-OPERATOR-RETRY-01`), full required-universe scheduler (`MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01`), live trading, risk/OMS/portfolio/broker/GUI/Discord/backtests/scheduler-task-definitions.
**Likely Files / Surfaces:** `core-rs/crates/mqk-cli/src/commands/md.rs`, `core-rs/crates/mqk-cli/src/main.rs` (doc-comment only), `config/instruments/equities.json`, `scripts/windows/Refresh-IntradayMarketData.ps1`, `scripts/windows/Prep-PremarketMarketData.ps1`, `tests/script_guards/test_intraday_market_data_refresh.ps1`.
**Required Implementation Rules:** Never guess a provider for a symbol/timeframe the registry doesn't cleanly authorize; never widen registry-scoping beyond the daemon's own admission contract; never claim `(alpaca, 1D)` is readiness-approved without committed proof.
**Safety / Compatibility Requirements:** Provenance/freshness/continuity/registry validation must not be weakened — proved by RS-SCOPE-03/04/05/07 negative controls (wrong-provider, wrong-timeframe, and no-match-fails-closed).
**Required Negative Controls:** `rs_scope_03_provider_scoping_excludes_wrong_provider_symbol`, `rs_scope_04_wrong_timeframe_is_excluded_not_silently_authorized`, `rs_scope_05_scoped_operation_fails_closed_on_no_match`, `rs_scope_07_canonical_registry_alpaca_1d_resolves_to_nothing`.
**Required Positive Controls:** `pp_01_raw_symbols_carry_request_symbol_provenance`, `rs_scope_02_provider_scoping_selects_only_matching_provider`, `rs_scope_06_canonical_registry_alpaca_5m_resolves_to_aapl_only`, `dbs_01_raw_symbols_mode_end_to_end_matches_windows_refresh_path`, `dbs_02_registry_scoped_mode_end_to_end_persists_truthful_provenance`.
**Required Regression Tests:** `mqk-cli --bin mqk-cli` (37/37, incl. 5 DB-gated `--include-ignored`); `mqk-db --test scenario_md_ingest_provider` (13/13 unchanged); `mqk-daemon --test scenario_daily_data_readiness_01` (66/66 unchanged); `tests\script_guards\test_intraday_market_data_refresh.ps1` (29/29); `tests\script_guards\test_premarket_market_data_prep.ps1` (16/16).
**Required Validation:**
```powershell
$env:MQK_DATABASE_URL = "postgresql://postgres:postgres@127.0.0.1:5434/mqk_test"
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- --include-ignored
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-db --test scenario_md_ingest_provider -- --include-ignored
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_daily_data_readiness_01 -- --include-ignored
powershell -ExecutionPolicy Bypass -File tests\script_guards\test_intraday_market_data_refresh.ps1
powershell -ExecutionPolicy Bypass -File tests\script_guards\test_premarket_market_data_prep.ps1
bash scripts/guards/check_unsafe_patterns.sh
bash scripts/guards/check_workspace_dep_inheritance.sh
git diff --check
```
**Forbidden Validation / Side Effects:** No live/paper DB writes from real provider network calls, no manual DB provenance edits, no orders, no runtime start.
**Acceptance Criteria:**
1. `md sync-provider --source alpaca --symbols AAPL --timeframe 5m` (the exact mode the Windows refresh script uses) persists `provider_id=alpaca`, `provider_source=alpaca`, `provider_symbol=AAPL`, `ingest_mode=provider_sync`.
2. `--symbols-from-registry` never selects an instrument configured for a different provider or an unauthorized timeframe.
3. The default `Start-PaperTradingSmoke.ps1 -StartIntradayRefreshLoop` path resolves to `alpaca` for AAPL/5m, never `twelvedata`.
4. `Prep-PremarketMarketData.ps1` never writes a wrong-provider row into a symbol's readiness window.
5. `(alpaca, 1D)` is not claimed as an authorized registry pairing without committed proof.
6. All listed regression suites remain green.
**Multi-Symbol Atomicity:** Unchanged from `MARKET-DATA-PROVIDER-PROVENANCE-01` — `ingest_provider_bars_with_truthful_provenance` still issues one DB call per symbol (not one atomic multi-symbol transaction), so a partial commit is possible across symbols within a single CLI invocation. Not addressed here (the current single-symbol AAPL/5m operational closure does not require it); tracked as `MARKET-DATA-CLI-MULTISYMBOL-ATOMICITY-01` if/when a real multi-symbol registry-scoped call needs the guarantee.
**Exact CLOSED End State:** Not yet CLOSED — `IMPLEMENTED_PENDING_REVIEW` until code-reviewed and merged.
**Expected Handoff:** Start HEAD `dae446b337b77245417a4cc982ff7fa22736b098`; end HEAD = new commit SHA on `fix-market-data-provider-provenance`; not pushed, not merged.

---

#### AUTONOMOUS-DAILY-OPERATOR-RETRY-01 — Safe operator recovery from manual_intervention_required after a preflight/readiness repair

**Status:** ACCEPTED_PENDING_INTEGRATION — independently reviewed against commit `035cabf0f43f64957f046aafc6e8136533c93939` (worktree `MiniQuantDeskV4-retry`, branch `fix-autonomous-daily-operator-retry`) during `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01`'s session (2026-08-11). Review confirmed `035cabf0` is reachable, its worktree/branch are exactly as recorded below, and it is used unmodified as the base commit for the new patch. Review did not re-run its own test suite in this session. **Not merged to `main`.**
**Priority:** P0
**Paper Impact:** YELLOW (new operator-authenticated route only; touches no order/execution/portfolio/broker/GUI path; reuses the existing durable operation state-machine's already-legal `manual_intervention_required -> preparing_data` edge)
**Subsystem:** mqk-daemon autonomous daily operation coordinator / operator control plane

**Current Source Truth:** Implemented in isolated worktree `C:\Users\Zacha\Desktop\MiniQuantDeskV4-retry`, branch `fix-autonomous-daily-operator-retry`, on top of `4bc78c7003257fca65d006d65aa660afe4b35a60` (`fix-market-data-provider-provenance`, `MARKET-DATA-PROVIDER-PROVENANCE-01`'s accepted base). Not merged.

**Problem (2026-08-11 incident, restated):** `market-data readiness failed` → daily operation entered `manual_intervention_required` → market data was repaired (`MARKET-DATA-PROVIDER-PROVENANCE-01`) → the durable operation remained `manual_intervention_required` forever, because ordinary coordinator ticks (`autonomous_daily_coordinator.rs::dispatch_by_state`) treat `STATE_MANUAL_INTERVENTION_REQUIRED` as sticky durable truth and only re-project `ManualInterventionRequired { newly_applied: false }` — no code path re-ran the readiness/start pipeline. No operator recovery route existed at all.

**Root Cause:** `dispatch_by_state`'s `STATE_MANUAL_INTERVENTION_REQUIRED` arm is read-only by design (correctly — manual states must not auto-clear); nothing else in the daemon ever issues the legal `manual_intervention_required -> preparing_data` DB transition (`mqk_db::is_legal_operation_transition`) that would let a repaired operation re-enter the coordinator pipeline.

**Fix:** New operator-authenticated route `POST /api/v1/autonomous/daily-operation/retry` (`core-rs/crates/mqk-daemon/src/routes/autonomous_daily_operator.rs`, registered in the existing `operator` router in `routes.rs` — same `token_auth_middleware` every other mutating operator route uses, zero new auth code). Given `{operation_id, expected_market_date?}`, it independently re-proves, in order, before any mutation: (1) `deployment_mode == "PAPER"` (refuses `not_authorized` otherwise); (2) the operation is currently `manual_intervention_required` (`not_manual` otherwise — deliberately never assumes a non-manual `preparing_data` row got there via a prior retry, since current state alone cannot prove that); (3) pristine pre-start class only, via the existing `check_operation_pristine` (`not_recoverable` on any runtime activity — `run_id`, `started_at_utc`, bar dispatch, or validated run lineage); (4) the session window has not closed; (5) a new closed-set `ManualRetryEligibility` classifier (`RecoverablePreflight` / `UnsafeRuntimeHistory` / `AdministrativeOrIdentityConflict` / `UnknownFailClosed`) accepts the operation's stored `state_reason_code` only if it is exact-membership in a static allow-list of `daily_data_readiness::REASON_*` constants plus `"assignment_missing"` — never substring/regex matching, and every halt/reconcile/arm/runtime-ownership reason fails closed; (6) the operation's identity (freshly re-derived via the same `derive_assignment_identity`/`derive_runtime_binding_identity`/`derive_autonomous_daily_operation_id` calls the coordinator itself uses) still matches today's canonical configuration (`not_recoverable` on drift); (7) a fresh, read-only re-run of the exact production `daily_data_readiness::evaluate_readiness_with_binding` reports `start_allowed` (`still_blocked` otherwise, no mutation). Only if all seven hold does it perform the canonical durable CAS transition (`mqk_db::transition_autonomous_daily_operation`, `manual_intervention_required -> preparing_data`, clearing `state_reason_code`/`state_blocker_signature`, preserving the original blocker event in the append-only `sys_autonomous_daily_operation_events` table) plus a best-effort `mqk_db::clear_retry_timing`. It never calls `start_execution_runtime`, `try_autonomous_arm_typed`, or any halt/kill-switch/reconcile-clearing path; `runtime_started`/`arm_modified`/`halt_changed`/`reconcile_changed` are hardcoded `false` and `orders_submitted` hardcoded `0` in every response branch. The existing `run_session_controller` loop (30s poll) picks up the `preparing_data` transition automatically and re-runs strict readiness -> `awaiting_open` -> typed arm -> canonical start with no shortcut.

**Dependencies:** `MARKET-DATA-PROVIDER-PROVENANCE-01` (base commit; provides the AAPL/5m/alpaca provenance fix this patch's proof scenario exercises)
**Unlocks:** NONE yet (operator-invoked only per §36 of the originating spec; automatic manual-state retry is explicitly deferred to a future patch)
**In Scope:** New route + handler module, new request/response API types, route registration, one new scenario test file.
**Out of Scope:** Automatic/scheduled retry of manual states (explicitly deferred), any change to `autonomous_retry_policy.rs`'s `ManualInterventionRequired`/`RetryableTransient` classification, strategy/OMS/portfolio/broker/GUI/launcher/Task-Scheduler code, live-capital support (route hard-refuses non-`PAPER` `deployment_mode`).
**Likely Files / Surfaces:** `core-rs/crates/mqk-daemon/src/routes/autonomous_daily_operator.rs` (new), `core-rs/crates/mqk-daemon/src/routes.rs`, `core-rs/crates/mqk-daemon/src/api_types.rs`, `core-rs/crates/mqk-daemon/tests/scenario_autonomous_daily_operator_retry_01.rs` (new).
**Required Implementation Rules:** No string/substring/regex authority over `state_reason_code` — exact static membership only; never call `start_execution_runtime`/`try_autonomous_arm_typed`/any halt-clearing path; never transition directly to `running`; CAS only via the existing `mqk_db::transition_autonomous_daily_operation` primitive (never a raw `UPDATE`).
**Safety / Compatibility Requirements:** Preserve arm-before-start (proved, not just asserted — see R10 below); preserve halt/kill-switch/reconcile authority (proved via R09); idempotent under repeated calls (R08 / T01's second-call check); race-safe under a stale CAS (R07, proved directly against the CAS primitive).
**Required Negative Controls:** R1 (still blocked → refused, `still_blocked`) · R2 (runtime history present → `not_recoverable`) · R3 (identity mismatch → `not_recoverable`) · R4 (session closed → `session_closed`) · R5 (wrong `operation_id` → `not_found`) · R6 (LIVE deployment → `not_authorized`, `403`) · R7 (stale CAS version → refused, never applied) · unsafe-runtime-history / administrative-identity-conflict / unknown-reason-class refusals · not-currently-manual → `not_manual`.
**Required Positive Controls:** T01 — full lifecycle: pristine operation → real coordinator CAS to `manual_intervention_required` with `REASON_MARKET_DATA_MISSING` → retry refused (`still_blocked`) while data is missing → bars repaired through the real metadata-aware ingest path (AAPL/5m/alpaca, mirroring `MARKET-DATA-PROVIDER-PROVENANCE-01`'s accepted lane) → retry accepted (`recovered`, `preparing_data`) → original blocker event preserved + recovery event recorded in `sys_autonomous_daily_operation_events` → coordinator `dispatch_by_state` progresses to `awaiting_open` → second retry call is a safe non-mutating no-op. R09 — halt/disarm state unchanged across a successful retry. R10 — a recovered, `awaiting_open` operation still cannot start without a successful typed arm (`attempt_canonical_start` under `DISARMED`/`halted` refuses; `start_attempt_count` stays `0`). Auth: missing/wrong/valid operator token proofs, mirroring `scenario_ctrl_auth_01.rs`'s established pattern.
**Required Regression Tests:** `scenario_autonomous_daily_session_coordinator_01` (35/35), `scenario_autonomous_daily_coordinator_policy_01` (8/8), `scenario_autonomous_daily_phase_d_integration_01` (49/49), `scenario_daily_data_readiness_01` (86/86), `scenario_daemon_routes` (66/66) — all green, zero regressions, run against the same DB-backed test suite this new patch's own tests use.
**Required Validation:**
```powershell
$env:MQK_DATABASE_URL = "postgresql://postgres:postgres@127.0.0.1:5434/mqk_test"
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_autonomous_daily_operator_retry_01 -- --include-ignored --test-threads=1
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_autonomous_daily_session_coordinator_01 --test scenario_autonomous_daily_coordinator_policy_01 --test scenario_autonomous_daily_phase_d_integration_01 --test scenario_daily_data_readiness_01 --test scenario_daemon_routes -- --include-ignored --test-threads=1
powershell -File scripts/guards/check_unsafe_patterns.ps1
git diff --check
```
**Forbidden Validation / Side Effects:** No live DB, no paper-soak production DB, no real Alpaca/broker call, no orders, no manual DB edits, no push, no merge.
**Acceptance Criteria:**
1. All 16 new scenario tests pass against a real local Postgres.
2. All five listed regression suites remain green (244/244 combined, 0 failures).
3. `check_unsafe_patterns.ps1` clean; `git diff --check` clean.
4. No file outside the four listed is modified; `main`, the `-ops`, and the `-data` worktrees remain untouched.
**Exact CLOSED End State:** Not yet CLOSED — `IMPLEMENTED_PENDING_REVIEW` until code-reviewed and merged.
**Expected Handoff:** Start HEAD `4bc78c70` (dev worktree base = `fix-market-data-provider-provenance`); end HEAD = new commit SHA on `fix-autonomous-daily-operator-retry`; not pushed, not merged.

#### MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01 — Automatic freshness maintenance for the required market-data universe

**Status:** ACCEPTED_PENDING_INTEGRATION
**Priority:** P0
**Paper Impact:** YELLOW (new market-data-only routes/scheduler; touches no order/execution/portfolio/broker/GUI path; reuses the existing latest-bar poll/ingest seam and the existing required-symbol resolver unchanged)
**Subsystem:** mqk-daemon market-data freshness controller / scheduler

**Problem:** The system previously had strict readiness gates and manual refresh tools, but lacked one authoritative controller that derived the complete required trading-data universe and maintained every requirement automatically before and throughout the trading session. An operator had to manually know which ticker(s) needed refreshing, which provider owned each ticker, which timeframe was required, whether historical bootstrap was missing, whether the latest completed bar was stale, and when to poll again.

**Current Source Truth:** Implemented in isolated worktree `C:\Users\Zacha\Desktop\MiniQuantDeskV4-autofresh`, branch `fix-market-data-autofresh-required-universe`, on top of `035cabf0f43f64957f046aafc6e8136533c93939` (`fix-autonomous-daily-operator-retry`, `AUTONOMOUS-DAILY-OPERATOR-RETRY-01`'s accepted base). Not merged.

**Fix:** New pure/business-logic module `core-rs/crates/mqk-daemon/src/state/required_market_data_autofresh.rs` that (1) resolves the required symbol/timeframe universe via the unchanged, pre-existing `market_data_freshness::required_symbols_with_source_from_env()` — the same resolver `GET /api/v1/market-data/ingest-plan` and the premarket readiness gate already use, so all three surfaces can never disagree; (2) resolves each requirement's provider through the validated instrument/provider registries (provider always comes from the instrument's own registered `provider` field — never hardcoded, never first-provider-wins); (3) groups resolved requirements into a typed `RequiredMarketDataRefreshPlan` by `(provider_id, timeframe)` so one provider call never mixes incompatible authority across symbols; (4) distinguishes typed registry/provenance blockers (`provider_registry_invalid`, `instrument_registry_invalid`, `provider_symbol_mismatch`, `unsupported_timeframe`, `provider_disabled`, `provider_capability_mismatch`, `provider_provenance_invalid`) — never retried by polling — from freshness blockers (`missing`/`insufficient`/`stale`, from the existing `evaluate_md_freshness_status`) which may trigger one bounded refresh attempt per cycle; (5) reuses the existing `state::market_data_latest_bar::{resolve_latest_bar_poll_target, poll_and_ingest_latest_closed_bar}` seam for the actual provider call and durable `md_bars` write — no second HTTP client, parser, or writer; (6) derives poll cadence and session-close cutoff from the existing `state::market_calendar::resolve_market_session_schedule` (DST/holiday/early-close aware, no HST/ET wall-clock hardcode); (7) never auto-repairs a bar whose stamped `provider_id` disagrees with the registry-resolved provider (wrong-provider negative control). New thin Axum routes in `core-rs/crates/mqk-daemon/src/routes/required_market_data.rs`: `GET /api/v1/market-data/required-universe/plan` (read-only dry-run, §46: zero provider calls, zero DB writes), `GET /api/v1/market-data/required-universe/status`, `POST /api/v1/market-data/required-universe/start` / `POST .../stop` (operator, requires auth) controlling a new process-local scheduler (`AppState::required_universe_scheduler`, not durable across restart — re-derived from `md_bars` + a fresh plan on every restart, matching the existing feed scheduler's own non-durability). All existing `market-data/feed/*` and `market-data/ingest-plan` routes are unchanged and continue to work; the required-universe surface is additive.
**Dependencies:** `MARKET-DATA-PROVIDER-PROVENANCE-01`, `AUTONOMOUS-DAILY-OPERATOR-RETRY-01` (base commit; this patch does not call its retry route)
**Unlocks:** `INSTRUMENT-UNIVERSE-REFRESH-01` (multi-symbol registry review), `MARKET-DATA-CLI-MULTISYMBOL-ATOMICITY-01`, `AUTONOMOUS-DATA-BLOCKER-AUTO-RECOVERY-01` (all OPEN, not started by this patch)
**In Scope:** New state module, new routes module + route registration, `AppState` scheduler-state field, focused scenario tests, `Prep-PremarketMarketData.ps1` / `Refresh-IntradayMarketData.ps1` / `Start-PaperTradingSmoke.ps1` updates to use the new required-universe authority, this ledger update.
**Out of Scope:** GUI changes; official-launcher branch/merge; Windows Task Scheduler registration; automatic invocation of `AUTONOMOUS-DAILY-OPERATOR-RETRY-01`'s retry route; live trading; strategy/OMS/portfolio/broker code; bulk instrument-registry review beyond the currently-approved universe (`INSTRUMENT-UNIVERSE-REFRESH-01`, still separate/OPEN).
**Likely Files / Surfaces:** `core-rs/crates/mqk-daemon/src/state/required_market_data_autofresh.rs` (new), `core-rs/crates/mqk-daemon/src/routes/required_market_data.rs` (new), `core-rs/crates/mqk-daemon/src/routes.rs`, `core-rs/crates/mqk-daemon/src/state.rs`, `core-rs/crates/mqk-daemon/tests/scenario_market_data_autofresh_required_universe_01.rs` (new), `core-rs/crates/mqk-daemon/tests/scenario_market_data_autofresh_plan_resolution_01.rs` (new), `scripts/windows/Prep-PremarketMarketData.ps1`, `scripts/windows/Refresh-IntradayMarketData.ps1`, `scripts/windows/Start-PaperTradingSmoke.ps1`.
**Required Implementation Rules:** No second required-symbol resolver; provider identity always read from the instrument registry's own `provider` field; registry/provenance blockers never retried by polling; overall readiness requires every required requirement ready (no partial green); zero order/execution/arm/halt/reconcile calls anywhere in this module (verified by grep — see closure evidence).
**Safety / Compatibility Requirements:** Existing `market-data/feed/*`, `market-data/ingest-plan`, and `market-data/readiness` routes unchanged; `md_bars` upsert idempotency unchanged (reused seam); no migration.
**Exact CLOSED End State:** Not yet CLOSED — `IMPLEMENTED_PENDING_REVIEW` until code-reviewed, its scenario tests run against a real local Postgres by a reviewer, and merged.
**Expected Handoff:** Start HEAD `035cabf0f43f64957f046aafc6e8136533c93939` (dev worktree base = `fix-autonomous-daily-operator-retry`); end HEAD = new commit SHA on `fix-market-data-autofresh-required-universe`; not pushed, not merged.

#### MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-01 — Align autofresh with strict readiness authority; fix scheduler start race, provider-call-counter truth, and launcher conflict ordering

**Status:** ACCEPTED_PENDING_INTEGRATION
**Priority:** P0
**Paper Impact:** YELLOW (repairs an already-YELLOW market-data-only surface; still touches no order/execution/portfolio/broker/GUI path)
**Subsystem:** mqk-daemon market-data freshness controller / scheduler; `Start-PaperTradingSmoke.ps1` launcher

**Problem:** An independent review of `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01` (commit `732f88951c1f1f01ea517dfcf42c119c44e1f104`) found four deterministic defects: (A) the controller decided missing/insufficient/stale/ok using the legacy `market_data_freshness::evaluate_md_freshness_status` (fixed 5-completed-bar minimum, wall-clock-age staleness) instead of the strict, session-anchored, strategy-history-aware readiness authority (`daily_data_readiness::evaluate_assignment`) that can ultimately block autonomous Paper trading — the two could disagree, most concretely on how many bars a requirement actually needs; (B) `start_required_universe_scheduler`'s stopped→running transition used two separate mutex acquisitions with a check-then-set gap, so two concurrent start calls could both observe `running=false` and both proceed; (C) `Start-PaperTradingSmoke.ps1`'s `-StartIntradayRefreshLoop`/`-StartRequiredUniverseScheduler` conflict check ran in STEP 8D, after STEP 8C had already started the legacy refresh-loop child process, so a conflicting invocation still left a side effect before refusing; (D) `RefreshAttemptOutcome`'s `Skipped` variant conflated "no provider call was made" with "a provider call was made and returned no usable data," so `provider_api_calls_made_this_cycle` could under-report actual invocations.

**Current Source Truth:** Implemented in isolated worktree `C:\Users\Zacha\Desktop\MiniQuantDeskV4-autofresh`, branch `fix-market-data-autofresh-required-universe`, directly on top of `732f88951c1f1f01ea517dfcf42c119c44e1f104` (the `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01` commit this repairs — same branch, not a new one). Not merged.

**Fix:** (A) `run_required_universe_cycle` now evaluates each resolved requirement's readiness via `daily_data_readiness::evaluate_assignment` — reused verbatim, never re-implemented — using the same "synthetic binding" pattern `dynamic_selection_plan_builder::evaluate_candidate` already establishes: a per-requirement ephemeral `PluginRegistry` plus an `EffectiveRuntimeBinding`/`SymbolStrategyAssignment` pair that trivially matches the one requirement under evaluation (strategy identity resolved once per cycle via `build_multi_symbol_runtime_config_from_env`, from the same watchlist/legacy-env inputs the required-symbol resolver already reads). `required_history_bars` now always comes from the assigned strategy's own `StrategyDataRequirements`, never a hardcoded minimum; the bounded historical-bootstrap lookback window scales with it. Only the exact bounded refreshable-reason set (`market_data_missing`, `insufficient_history`, `interior_gap`, `expected_latest_bar_missing`) may trigger a bounded provider attempt — every registry/binding/calendar/provenance blocker is surfaced and left alone. `expected_latest_bar_missing` repair now supplies `ExpectedLatestBarConstraint` (never `None`), so a provider that returns an older, lagging bar is recorded as lagging rather than silently accepted. Poll scheduling (`next_poll_time_for_groups`) is now session-anchored (`daily_data_readiness::intraday_grid_starts` + `effective_grace_seconds`) instead of `mqk_md::next_poll_time_ts`'s epoch-boundary cadence, so a preopen instant no longer wakes on every 5-minute UTC boundary and a previous session's already-durable tail correctly satisfies the current expectation. (B) `start_required_universe_scheduler`'s stopped→running check-and-set is now one atomic mutex acquisition with no `.await` inside the critical section; the immediate-cycle-finds-no-work case now settles the scheduler truthfully (`running=false`, `next_cycle_utc=None`, `task=None`) instead of leaving `running=true` with nothing scheduled. (C) The `-StartIntradayRefreshLoop`/`-StartRequiredUniverseScheduler` conflict check now runs immediately after argument parsing, before STEP 1 and any Docker/daemon/child-process side effect; the required-universe scheduler is now the default on a normal (non-`-CheckOnly`, non-`-StartIntradayRefreshLoop`) Paper startup, with a new `-SkipRequiredUniverseScheduler` opt-out. (D) `RefreshAttemptOutcome` is now `NoCall`/`CalledSuccess`/`CalledNoData`/`CalledError(String)`; the provider-call counter increments on every non-`NoCall` outcome, matching actual invocations. `registry_unavailable_report` now takes an explicit reason code so an instrument-registry load failure reports `instrument_registry_invalid` and a provider-registry load failure reports `provider_registry_invalid`, never both flattened into the latter.

**Dependencies:** `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01` (repairs it in place, same branch)
**Unlocks:** Nothing new; keeps the same unlock set as the parent patch.
**In Scope:** `core-rs/crates/mqk-daemon/src/state/required_market_data_autofresh.rs`, `core-rs/crates/mqk-daemon/tests/scenario_market_data_autofresh_required_universe_01.rs`, `scripts/windows/Start-PaperTradingSmoke.ps1`, `scripts/guards/validate_market_data_autofresh_required_universe_01_repair_01.ps1` (new), this ledger entry.
**Out of Scope:** Order/execution/risk/live changes; official-launcher branch; Windows Task Scheduler registration (the temporary Aug 11–14 soak task on protected `main` is explicitly untouched); reopening `MARKET-DATA-PROVIDER-PROVENANCE-01` or `AUTONOMOUS-DAILY-OPERATOR-RETRY-01`.
**Likely Files / Surfaces:** see In Scope.
**Required Implementation Rules:** No second, hand-rolled strict readiness evaluator — reuse `daily_data_readiness::evaluate_assignment` only; no fabricated fixed history minimum; registry/provenance/binding/calendar blockers never retried by polling; provider-call counter must equal actual invocations; the PowerShell conflict check must run with zero child-process/scheduler side effects.
**Safety / Compatibility Requirements:** Existing `required-universe/{plan,status,start,stop}` routes unchanged (no rename); public GET routes remain read-only; POST routes remain operator-authenticated; zero order/execution/arm/halt/reconcile calls (unchanged from parent); `AUTONOMOUS-DAILY-OPERATOR-RETRY-01`'s accepted behavior and base commit are untouched; the protected temporary soak task on `C:\Users\Zacha\Desktop\MiniQuantDeskV4` (main) is untouched by this branch.
**Exact CLOSED End State:** Not yet CLOSED — `IMPLEMENTED_PENDING_REVIEW` until code-reviewed, its scenario tests (including the new load-bearing tests this repair adds) run against a real local Postgres by a reviewer, and merged together with the parent patch.
**Expected Handoff:** Start HEAD `732f88951c1f1f01ea517dfcf42c119c44e1f104`; end HEAD = new commit SHA on `fix-market-data-autofresh-required-universe`; not pushed, not merged.

#### MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-02 — Finalize autofresh scheduler ownership: fail-closed launcher gate + stop/restart generation token

**Status:** ACCEPTED_PENDING_INTEGRATION
**Priority:** P0
**Paper Impact:** YELLOW (repairs an already-YELLOW market-data-only surface; still touches no order/execution/portfolio/broker/GUI path)
**Subsystem:** mqk-daemon market-data freshness controller / scheduler; `Start-PaperTradingSmoke.ps1` launcher

**Problem:** Two deterministic defects remained after REPAIR-01 (commit `fde6e227a289a17abf101a73fa0390bde9219612`): (A) `Start-PaperTradingSmoke.ps1` STEP 8D treated every required-universe scheduler establishment failure (POST failure, `overall_state=blocked`, or a `409` reused owner that turned out to be `dry_run=true`) as a `Write-Warn ... (non-fatal)` and continued toward reconcile/arm anyway — a normal Paper startup could reach arm without any real data-maintenance authority behind it, violating fail-closed. (B) `RequiredUniverseSchedulerRuntimeState` had no ownership/generation token: a stop immediately followed by a restart (ABA) let a still-in-flight cycle from the *stopped* generation, once its provider call finally returned, overwrite the *new* generation's `last_report`/`next_cycle_utc`/`cycle_count`, or install a second background task, because the code only checked `running`, which the new generation had already re-set to `true`.

**Current Source Truth:** Implemented in isolated worktree `C:\Users\Zacha\Desktop\MiniQuantDeskV4-autofresh`, branch `fix-market-data-autofresh-required-universe`, directly on top of `fde6e227a289a17abf101a73fa0390bde9219612` (the REPAIR-01 commit this repairs — same branch, not a new one). Not merged.

**Fix:** (A) Two new self-contained PowerShell functions in `Start-PaperTradingSmoke.ps1`, `Confirm-RequiredUniverseSchedulerOwnership` and `Start-OrVerifyRequiredUniverseScheduler`, replace the old inline try/warn STEP 8D body. A `200` response is verified via a follow-up `GET .../status` proving `running=true`/`dry_run=false`/a non-`blocked` report before being called established; a `409` is never itself proof — the same status verification runs for the reused-owner case, and a `dry_run=true` owner is refused with reason `REQUIRED_UNIVERSE_SCHEDULER_BLOCKED_DRY_RUN_OWNER`. `overall_state=blocked` (either from the `200` body or the verified status) surfaces every per-requirement blocker and refuses. `overall_state=not_applicable` (non-trading day / empty required universe) is accepted as a legitimate no-work result without requiring `running=true`. STEP 8D now calls `exit 1` before STEP 9/reconcile/arm whenever `Established=$false` — no more `(non-fatal)` wording on this path. Both functions are extracted (regex, not modified) by a new guard `scripts/guards/validate_market_data_autofresh_required_universe_01_repair_02.ps1`, which shadows `Invoke-DaemonGet`/`Invoke-DaemonPost` with mocked HTTP responses to functionally exercise every branch (POST failure, `200`+blocked, `409`+dry-run owner, `409`+verified reuse, `not_applicable`, plus unexpected-HTTP-status and 200-but-not-running coverage) with zero real daemon/network/DB/order side effects. (B) `RequiredUniverseSchedulerRuntimeState` gained `pub generation: u64`. `start_required_universe_scheduler` claims `old_generation.wrapping_add(1)` under the same atomic lock acquisition that flips `running`, explicitly preserved across the `..Default::default()` state reset (never left to reset to `0`). `run_and_record_cycle` now takes the caller's `generation` and only writes `last_cycle_utc`/`cycle_count`/`provider_api_calls_made`/`next_cycle_utc`/`last_report` when `scheduler.generation == generation` at write time — a superseded cycle still returns its actual computed report to its own caller, it just never reaches shared state. The post-immediate-cycle "should I spawn a background task" check and the post-`tokio::spawn` "should I install the task handle" check both now additionally require `scheduler.generation == generation` (not just `running`), so a superseded starter can never install a stale task. `required_universe_scheduler_loop` takes its owning `generation` and re-verifies `running && generation == my_generation` at loop-top, after its wait, and before recording its own cycle — an old loop returns immediately once superseded rather than continuing to poll or settle a newer generation as stopped.

**Dependencies:** `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-01` (repairs it in place, same branch)
**Unlocks:** Nothing new; keeps the same unlock set as the parent patch.
**In Scope:** `core-rs/crates/mqk-daemon/src/state/required_market_data_autofresh.rs`, `core-rs/crates/mqk-daemon/tests/scenario_market_data_autofresh_required_universe_01.rs`, `scripts/windows/Start-PaperTradingSmoke.ps1`, `scripts/guards/validate_market_data_autofresh_required_universe_01_repair_02.ps1` (new), this ledger entry.
**Out of Scope:** Order/execution/risk/live changes; official-launcher branch; Windows Task Scheduler registration; reopening strict-readiness, provider-grouping, historical-bootstrap, session-timing, provider-provenance, or operator-retry logic (REPAIR-01's own scope, left untouched); launcher integration beyond STEP 8D itself.
**Likely Files / Surfaces:** see In Scope.
**Required Implementation Rules:** STEP 8D must fail closed (`exit 1`) before STEP 9/reconcile/arm whenever real (`dry_run=false`) required-universe maintenance authority is not proven; a `200`/`409` HTTP response is never itself proof — the scheduler's own status route must be checked; a stale generation's cycle/task must never mutate or install itself into a newer generation's state, proven by a real barrier-controlled concurrency test, not a sequential-only proof.
**Safety / Compatibility Requirements:** Existing `required-universe/{plan,status,start,stop}` routes unchanged (no rename, no new fields on the wire besides the already-`pub` `generation` field on process-local runtime state, which is not serialized); zero order/execution/arm/halt/reconcile calls anywhere in this repair (verified by grep — see closure evidence); REPAIR-01's strict-readiness/session-anchoring behavior is unmodified.
**Exact CLOSED End State:** Not yet CLOSED — `IMPLEMENTED_PENDING_REVIEW` until code-reviewed, its scenario tests (including the new `stop_start_generation_race_old_cycle_cannot_overwrite_new_owner` load-bearing concurrency test) run against a real local Postgres by a reviewer, and merged together with the parent patch and REPAIR-01.
**Expected Handoff:** Start HEAD `fde6e227a289a17abf101a73fa0390bde9219612`; end HEAD = new commit SHA on `fix-market-data-autofresh-required-universe`; not pushed, not merged.
**Known Pre-Existing Issue (not fixed by this patch, out of scope):** Several DB-backed tests in `scenario_market_data_autofresh_required_universe_01.rs` (`aapl_5m_positive_proof_bootstraps_then_stays_ready` and others sharing the file's fixed `now_fixture()` helper) compare a real, wall-clock-stamped `ingested_at` DB column against that hardcoded timestamp plus a skew tolerance (`daily_data_readiness.rs`'s `REASON_PROVIDER_INGEST_TIME_FUTURE` check, `effective_future_skew_seconds = min(configured_future_skew_seconds, 60, timeframe.duration_secs())` — the configured default is 300s, but the *effective* ceiling for these tests' 5m timeframe is 60s) — once real wall-clock drifts more than that effective ~60s past the fixture's fixed value, they fail with `provider_ingest_time_future`. Confirmed reproducible identically on unmodified `fde6e227` (pre-dating this patch). A follow-up task was flagged separately; this repair's own new test (`stop_start_generation_race_old_cycle_cannot_overwrite_new_owner`) deliberately uses real `Utc::now()` instead of `now_fixture()` for exactly this reason and is not affected. **Resolved** by `MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01` immediately below (same branch, later commit).

#### MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01 — Stabilize autofresh test fixtures against wall-clock drift (test-only)

**Status:** ACCEPTED_PENDING_INTEGRATION
**Priority:** P2
**Paper Impact:** GREEN (test-only; zero production code touched)
**Subsystem:** `scenario_market_data_autofresh_required_universe_01.rs` test fixtures

**Problem:** The "Known Pre-Existing Issue" noted on `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-02` immediately above: several DB-backed tests compare a fixed, hardcoded `now_utc` (`now_fixture()`, or the preopen test's own hardcoded `2026-08-11`/`2026-08-12` instants) against `md_bars.ingested_at`, which the reused production ingest seam stamps from the real database server clock (`timestamptz not null default now()`, migration 0003). `MQK_DATA_READINESS_FUTURE_SKEW_SECS`'s *configured* default is 300s, but `daily_data_readiness.rs`'s `effective_future_skew_seconds` function actually enforces `min(configured_future_skew_seconds, 60, timeframe.duration_secs())` — for these tests' 5m timeframe under default config that is 60s, not 300s. Once real wall-clock drifts more than that effective ~60s tolerance past the fixed fixture value, `daily_data_readiness::evaluate_bar_readiness`'s unmodified `REASON_PROVIDER_INGEST_TIME_FUTURE` check correctly (and honestly) flags the row as future-dated relative to the test's stated evaluation instant — a false failure with no underlying defect. Confirmed reproducible identically on unmodified `aae1e3b8a7c96e9f283f4ed6589d07b69c058883` (REPAIR-02's own end state) before this fix.

**Current Source Truth:** Implemented in isolated worktree `C:\Users\Zacha\Desktop\MiniQuantDeskV4-autofresh`, branch `fix-market-data-autofresh-required-universe`, directly on top of `aae1e3b8a7c96e9f283f4ed6589d07b69c058883` (REPAIR-02's commit — same branch, not a new one). Not merged.

**Fix:** Test-only, in `scenario_market_data_autofresh_required_universe_01.rs`. Two new helpers: `stamp_recent_ingested_at_for_test` (re-stamps `md_bars.ingested_at` on every `ZZAUTOFR%` row written no earlier than a real-clock "not before" checkpoint, to a value consistent with the test's own fixed evaluation instant — a no-op when nothing was written) and `run_cycle_with_deterministic_ingest_stamp` (runs the real, unmodified `run_required_universe_cycle`, then applies the stamp to whatever it just wrote). Every test whose assertions depend on the ingest path being genuinely fresh now calls the wrapper instead of `run_required_universe_cycle` directly. The stamp cannot retroactively change a report object a call has already returned, so for any cycle that performs a live bootstrap/poll *and* re-evaluates readiness within that same call (the write and the read racing the same real DB-clock stamp), that cycle's own report still only asserts structural facts (which requirement, which provider, that the call was actually dispatched) — genuine `"ok"`/`"ready"` is proven by a subsequent cycle, reading the now-correctly-stamped row fresh (six tests gained one additional such follow-up cycle: `aapl_5m_positive_proof_bootstraps_then_stays_ready`, `multi_symbol_positive_proof_every_symbol_evaluated_and_refreshed`, `mixed_provider_proof_two_groups_partial_failure_does_not_block_the_other`, `one_stale_required_symbol_blocks_overall_readiness`, `strategy_history_requirement_above_five_bars_is_not_satisfied_by_five_bars`, `provider_api_call_counter_matches_actual_invocations`). The preopen test's own manual pre-seed insert is stamped directly, ahead of the cycle call, since that test never asserts anything about a same-call write. Every existing provider-call-count assertion (`historical_calls()`, `provider_api_calls_made`) is preserved exactly on the call that genuinely dispatched it — never inflated or moved.

**Dependencies:** `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-02`
**Unlocks:** Nothing new.
**In Scope:** `core-rs/crates/mqk-daemon/tests/scenario_market_data_autofresh_required_universe_01.rs`, this ledger entry.
**Out of Scope:** `daily_data_readiness.rs` (unmodified — `REASON_PROVIDER_INGEST_TIME_FUTURE` and its skew tolerance are untouched); `required_market_data_autofresh.rs` (unmodified — no scheduler/generation/ownership logic touched); `Start-PaperTradingSmoke.ps1`; any production ingest/write path; `MQK_DATA_READINESS_FUTURE_SKEW_SECS` production default (never increased/disabled).
**Required Implementation Rules:** No production code changes; no weakening of `REASON_PROVIDER_INGEST_TIME_FUTURE` or its tolerance; no skipped/ignored tests; no loosened assertions to hide a failure — every relaxed same-call assertion has an equivalent, genuine assertion added on a later, correctly-stamped cycle; every test's semantic intent (preopen stays preopen, mid-session stays mid-session, bootstrap-call-counting stays exact) is preserved.
**Safety / Compatibility Requirements:** Zero production code touched (verified by `git diff --stat`: one file changed, the scenario test file only); zero order/execution/arm/halt/reconcile paths anywhere in this patch.
**Exact CLOSED End State:** Not yet CLOSED — `IMPLEMENTED_PENDING_REVIEW` until code-reviewed and merged together with the parent patch and both repairs.
**Expected Handoff:** Start HEAD `aae1e3b8a7c96e9f283f4ed6589d07b69c058883`; end HEAD = new commit SHA on `fix-market-data-autofresh-required-universe`; not pushed, not merged.

#### AUTONOMOUS-DATA-BLOCKER-AUTO-RECOVERY-01 — Automatic retry of manual_intervention_required once autofresh repairs data (blocked/future)

**Status:** OPEN · **Priority:** P3 · **Paper Impact:** YELLOW · **Subsystem:** Autonomous daily operation / market-data freshness
**Problem:** `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01` may expose `operator_retry_required`-shaped truth when a required symbol is blocked, but deliberately never calls `AUTONOMOUS-DAILY-OPERATOR-RETRY-01`'s retry route automatically (out of scope, §33 of the originating spec). Whether/how to safely automate that composition is undecided and not started.
**Dependencies:** `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01`, `AUTONOMOUS-DAILY-OPERATOR-RETRY-01`

#### INSTRUMENT-UNIVERSE-REFRESH-01 — Bulk instrument-registry provider/timeframe review beyond AAPL (blocked/future)

**Status:** OPEN · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** Instrument registry
**Problem:** `MARKET-DATA-PROVIDER-PROVENANCE-01`'s registry decision was deliberately scoped to AAPL only (the current approved paper universe). Whether other seeded equities' `provider`/`timeframes` need the same review is undecided and not started. Also tracks the open architecture question (documented, not implemented, by `MARKET-DATA-PROVIDER-PROVENANCE-01`) of whether the registry's single `provider` field is sufficient long-term versus separate historical/intraday/streaming/execution provider concepts.
**Dependencies:** `MARKET-DATA-PROVIDER-PROVENANCE-01`

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

#### PROMOTION-WALKFORWARD-GATE-WIRING-01 — Wire the accepted OOS-evidence verifier into the production research → promotion path

**Status:** IN PROGRESS / PARTIAL — REPAIR REQUIRED — local-only, unpushed; independent review of commit `242cb7c3` has already occurred and found material deterministic gaps (corrected by `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01`, 2026-08-21, superseding the prior `IMPLEMENTED_PENDING_INDEPENDENT_REVIEW` characterization — review is no longer pending, it has occurred and did not accept the implementation) · **Priority:** P1 · **Paper Impact:** GREEN (promotion output is a report artifact; no portfolio/risk/execution/broker writes) · **Subsystem:** mqk-promotion / mqk-daemon

**Update (2026-08-21, `MASTER-LEDGER-REPO-TRUTH-REFRESH-02`):** Local `main` HEAD `242cb7c3` (one commit ahead of `origin/main`, not pushed) implements this entry's invariant. Diff inspection against parent `fd90f63a` confirms: (1) the new Gate 4c runs inside the exact same `transition_requires_evidence` branch as the existing Gate 4, in `strategy_promotion_transition` — the sole write path for promotion state (no other call site inserts a `strategy_promotion_transitions` row) — so there is no bypass/alternate route; (2) `research_registry_db_path`, `research_evidence_artifact_root`, and both DSR/PBO thresholds are read only from `AppState`/env (`MQK_RESEARCH_REGISTRY_DB`, `MQK_RESEARCH_EVIDENCE_ARTIFACT_ROOT`, `MQK_RESEARCH_MIN_DEFLATED_SHARPE_RATIO`, `MQK_RESEARCH_MAX_PROBABILITY_BACKTEST_OVERFITTING`) — no `StrategyPromotionTransitionRequest` field can select an alternate registry, root, or threshold; (3) caller-supplied `research_evidence_dir`/`research_judge_artifact_path` are canonicalized and root-bound (reusing `promotion_evidence_validation::{open_confined_regular_child, read_bounded_file_string}`), never trusted as bare claims; (4) missing config, missing/blank fields, an unregistered trial, a root-escaping path, or a mutated judge artifact all fail closed with a dedicated reason (`ResearchEvidenceGateOutcome::Rejected`). Focused validation actually run this session: `cargo test -p mqk-daemon --lib research_evidence_gate` — **11/11 passed** (acceptance, missing-registry/root/thresholds, unregistered trial, DSR-below/PBO-above rejection, evidence-dir/judge-path root-escape rejection, mutated-artifact authority-mismatch rejection, blank-trial-id); `cargo test -p mqk-promotion` (the underlying accepted P7C verifier, untouched by this commit) — **70/70 passed**, confirming no regression to the frozen mechanism; `git diff --check` on `fd90f63a..242cb7c3` — clean. **Not run to completion:** the two DB-backed integration tests that exercise this route end-to-end (`valid_research_evidence_without_scanner_evidence_is_rejected`, `valid_scanner_evidence_without_research_evidence_is_rejected` in `scenario_strategy_promotion_routes_01.rs`) and the new full-lifecycle `scenario_strategy_promotion_closure_proof_01f.rs` all failed to start with `migration 6 was previously applied but has been modified` against the local `mqk-test-postgres` container — a pre-existing local test-DB migration-checksum drift unrelated to this patch (migration `0006_arm_state.sql` is untouched by `242cb7c3` and has stable Git history predating it), not a defect in the patch itself. Per `.claude/rules/audit_repo_truth_rules.md`, scenario-test-file presence and unit-level passes are evidence, not independent acceptance, and were **not** sufficient for `CLOSED` at that time.

**Independent review finding (2026-08-21, `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01`):** An independent review (ChatGPT) of commit `242cb7c3` against the real production `strategy_promotion_transition` route has since occurred and found material deterministic gaps beyond the unit-level evidence recorded above. This corrects the entry's status from `IMPLEMENTED_PENDING_INDEPENDENT_REVIEW` (review not yet done) to `IN PROGRESS / PARTIAL — REPAIR REQUIRED` (review done, gaps found, repair outstanding). Findings:
1. **Cross-candidate authority gap** — the production transition can combine scanner/review evidence and independently-valid Research evidence without sufficient proof that both refer to the same semantic promotion candidate.
2. **Parallel / partial promotion policy** — the daemon performs Research verification plus DSR/PBO checks directly (this entry's Gate 4c) instead of routing the complete production promotion decision through canonical `mqk_promotion::evaluate_promotion`.
3. **Missing durable research lineage** — the Research evidence used to authorize the transition is not durably stored as promotion-transition authority.
4. **Missing canonical backtest evidence seam** — `evaluate_promotion` requires genuine canonical inputs (`BacktestReport`, `ArtifactLock`, `StressSuiteResult`); the current production promotion flow has no trustworthy candidate-bound seam resolving those objects. The immediate missing prerequisite this creates is tracked as a new entry, `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` (added immediately below this entry), status `OPEN`, not yet started.

None of this contradicts the unit-level evidence above (11/11 gate tests, 70/70 `mqk-promotion`, clean `git diff --check`) — that evidence remains true and unregressed. It establishes that unit-level correctness of Gate 4c in isolation is not the same as proof that the complete production promotion decision is correctly and exclusively routed through canonical authority for a single, unambiguous candidate. Per `.claude/rules/audit_repo_truth_rules.md`, this entry must not be marked `READY`, `LOCALLY COMPLETE`, `IMPLEMENTED_PENDING_INDEPENDENT_REVIEW`, `CLOSED`, `INDEPENDENTLY ACCEPTED`, or `PUSHED` while these gaps remain open.

**Correction note (2026-08-17):** `MASTER-LEDGER-CONSOLIDATION-01` (earlier the same day) incorrectly reclassified this entry `CLOSED — SUPERSEDED`, reasoning that the P7A→P7C research-promotion program (commits `3e2d926b`..`b80749bd` on `main`, see §24) delivered `PromotionInput.oos_evidence: Option<VerifiedPromotionOosEvidence>` (`core-rs/crates/mqk-promotion/src/types.rs`), populated only by `mqk_promotion::verify_promotion_oos_evidence` (`research_evidence.rs`), which hash-binds real Research artifacts to durable SQLite registry rows (`research_trials`/`research_attempts`/`research_judge_artifacts`) and fails closed on `None`, and treated that as fully superseding this entry's scope. That was wrong: P7C-REPAIR-04's own record states there is **no production call site** for `verify_promotion_oos_evidence` outside `mqk-promotion` tests, and review of the full Wave-2 patch chain confirms it never modified `mqk-daemon`. The production strategy-promotion daemon path still uses its older scanner/review-artifact validation surface. P7C implemented and hardened a stronger mechanism than this entry's original proposed one — it did **not** finish the production-wiring invariant this entry tracks. Restored to `READY`.

**Updated Current Source Truth (2026-08-17):**
- P7C's OOS evidence verifier (`verify_promotion_oos_evidence`) is implemented and independently accepted locally (Wave 2 — commits `81dcf621` P7B-REPAIR-03 and `b80749bd` P7C-REPAIR-04 — not yet pushed; see §24).
- `VerifiedPromotionOosEvidence` cannot be caller-constructed (hash-bound to durable Research registry rows).
- Research registry / attempt / judge authority (`research_trials`/`research_attempts`/`research_judge_artifacts`) is accepted.
- **But no production caller currently invokes `verify_promotion_oos_evidence`.**
- No trusted production Research registry DB path is currently wired into this promotion path.
- The daemon/operator promotion flow does not yet construct `PromotionInput.oos_evidence` from the accepted P7C verifier.

**Problem:** A strong, accepted OOS-evidence mechanism exists, but it is not enforced at the authoritative production promotion boundary — a strategy can still be promoted today without ever passing through `verify_promotion_oos_evidence`.
**Why This Matters:** This is the single largest correctness gap in the research→promotion pipeline; it directly affects the credibility of any strategy ever promoted. `RESEARCH_BACKTEST_V1_COMPLETE` cannot be met while it stays open (see §24).
**Dependencies:** Wave 2 (P7A/P7B/P7C, including `P7C-REPAIR-04`) `ACCEPTED_LOCALLY — PUSHED` — met (confirmed `b80749bd` is an ancestor of `origin/main` as of 2026-08-21). Remaining before this entry can be considered `CLOSED`: (1) push local `main` (including `242cb7c3`) to `origin/main`; (2) repair the four gaps found by independent review (cross-candidate authority, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam) — independent review of `242cb7c3` has now occurred (2026-08-21) and found these gaps, it is not merely pending; (3) `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` (new entry immediately below) `CLOSED` — the immediate missing prerequisite identified by that review; (4) a passing run of the DB-backed integration/closure-proof harness against a correctly-migrated Postgres instance (blocked this session by local test-DB drift, see update note above).
**Remaining mission (do not redesign P7C):**
```text
real Research artifacts
    -> trusted Research registry DB path
    -> verify_promotion_oos_evidence
    -> VerifiedPromotionOosEvidence
    -> PromotionInput.oos_evidence
    -> evaluate_promotion
    -> promotion decision
```
No caller-generated bypass. The Research registry path must come from trusted application/config state, not request/evidence JSON. Missing/unavailable/mismatched evidence fails closed.
**In Scope:** Construct `PromotionInput.oos_evidence` in the real daemon/operator promotion flow from a trusted, application/config-sourced Research registry path, calling `verify_promotion_oos_evidence`. **Out of Scope:** Redesigning P7C, changing `verify_promotion_oos_evidence`'s signature/verification logic, changing the Research registry schema.
**Likely Files / Surfaces:** `core-rs/crates/mqk-daemon/src/routes/strategy_promotions.rs` (or wherever `PromotionInput` is currently constructed for the production promotion flow), `core-rs/crates/mqk-promotion/src/research_evidence.rs`, `core-rs/crates/mqk-promotion/src/types.rs`.
**Required Implementation Rules:** No caller-generated bypass; the Research registry path must come from trusted application/config state, not request/evidence JSON; missing/unavailable/mismatched evidence fails closed (`PromotionInput.oos_evidence: None` blocks promotion exactly as it does today in `evaluator.rs`).
**Safety / Compatibility Requirements:** Must not change behavior for already-promoted strategies retroactively; must not weaken or bypass any P7A/P7B/P7C invariant (FROZEN per §24 — do not reopen the mechanism itself).
**Required Negative Controls:** A production promotion attempt with no trusted registry path resolvable, or with tampered/mismatched registry evidence, fails closed with a dedicated reason.
**Required Positive Controls:** A real Research artifact chain, written by the actual Python registry write path, flows through the daemon promotion route and produces a `PromotionInput.oos_evidence` that `evaluate_promotion` accepts.
**Required Regression Tests:** All existing `mqk-promotion` gate tests (`scenario_nan_metric_fails_promotion.rs`, `scenario_tie_break_correctness.rs`, `scenario_golden_artifact_hash_lock.rs`, `scenario_promotion_requires_partial_fill_stress.rs`, `scenario_promotion_oos_evidence_gate_p7c_repair_01.rs`) remain green.
**Required Validation:** `cargo test -p mqk-promotion`; `cargo test -p mqk-daemon` (promotion route scenarios).
**Forbidden Validation / Side Effects:** No real broker call, no live/paper DB write outside test fixtures.
**Acceptance Criteria:** 1) The real production promotion route constructs `PromotionInput.oos_evidence` via `verify_promotion_oos_evidence` from a trusted registry path. 2) A missing/unavailable/mismatched registry path fails closed with a dedicated reason. 3) All existing promotion gate tests (P7A-P7C) remain green. 4) A new negative-control test proves the production route itself fails closed on unwired/unavailable evidence — not just the library function in isolation.
**Exact CLOSED End State:** CLOSED when no production-path strategy promotion can proceed without a real, registry-verified `verify_promotion_oos_evidence` result, proven end-to-end through the actual daemon route, with all pre-existing promotion tests green.
**Acceptance History:** Implementation DONE locally (`242cb7c3`, unpushed) / Unit-level validation PASSED (11/11 gate tests, 70/70 `mqk-promotion`, `git diff --check` clean) / DB-backed integration & closure-proof harness PENDING (blocked by local test-DB migration drift, not yet re-attempted) / Independent review DONE — REPAIR REQUIRED (2026-08-21: cross-candidate authority gap, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam — see finding above) / Push to `origin/main` PENDING (now blocked on repair, not merely on review).

#### PROMOTION-BACKTEST-EVIDENCE-SEAM-01 — Canonical candidate-bound backtest evidence seam for `evaluate_promotion`

**Status:** OPEN (not started) · **Priority:** P1 · **Paper Impact:** GREEN (research/promotion evidence only; no execution/portfolio/broker path) · **Subsystem:** mqk-promotion / mqk-daemon

**Current Source Truth:** `evaluate_promotion` requires genuine canonical inputs — `BacktestReport`, `ArtifactLock`, `StressSuiteResult` — bound to a single, unambiguous promotion candidate. No current production seam resolves these objects for a specific candidate in a way proven bound to the same semantic candidate as any Research/OOS-evidence gate (Gate 4c, `242cb7c3`) or scanner/review evidence.
**Problem:** Without a canonical, candidate-bound backtest-evidence seam, the production promotion path can combine independently-valid pieces of evidence (scanner/review evidence, Research OOS evidence) without proof they refer to the same semantic candidate, and/or invoke Research verification plus DSR/PBO checks directly instead of routing the complete decision through canonical `mqk_promotion::evaluate_promotion`. Identified by independent review of `242cb7c3` (2026-08-21) — see `PROMOTION-WALKFORWARD-GATE-WIRING-01` above.
**Why This Matters:** This is the structural prerequisite for closing `PROMOTION-WALKFORWARD-GATE-WIRING-01` — without it, "production wiring" remains partial regardless of how much of the OOS-evidence mechanism is wired in.
**Dependencies:** NONE identified yet — newly identified by independent review (2026-08-21); not yet scoped/decomposed.
**In Scope:** Define and implement the canonical, candidate-bound seam resolving `BacktestReport`/`ArtifactLock`/`StressSuiteResult` for a specific promotion candidate, and route the complete production promotion decision through `evaluate_promotion` rather than partial/parallel checks. **Out of Scope:** Redesigning `evaluate_promotion` itself; redesigning the Research OOS-evidence mechanism (P7A-P7C, FROZEN, see §24).
**Exact CLOSED End State:** Not yet defined pending scoping — an honest, non-fabricated acceptance condition per this ledger's own convention (§15) for a newly-identified, not-yet-decomposed item.
**Acceptance History:** N/A — OPEN, not started.

**Original entry (historical, retained for context — its exact proposed field name, `walk_forward_evidence: Option<WalkForwardEvidence>`, was never implemented; superseded by the stronger `oos_evidence: Option<VerifiedPromotionOosEvidence>` mechanism, but the production-wiring gap it identified is real and is what the entry above now tracks):**
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

#### DOCS-TRACKER-RETIREMENT-01 — Finish retiring redundant historical tracker documents

**Status:** OPEN · **Priority:** P3 · **Paper Impact:** GREEN · **Subsystem:** Documentation / repository hygiene

**Context (added 2026-08-17, `MASTER-LEDGER-TRUTH-REPAIR-01`):** A prior `DOCS-TRACKER-CLEANUP-01` session safely deleted zero documents because real blockers were found — a correct fail-closed deletion decision, not a defect. That session's original cleanup objective remains partially open and is tracked here so it is not lost.

**Purpose:** Finish retiring redundant historical tracker documents once their remaining dependencies/content are safely migrated.

**Confirmed current blockers:**

1. `MiniQuantDesk_Master_Patch_Ledger_v2.md` — cannot currently be deleted because `scripts/guards/validate_autonomous_daily_paper_operations_01g_bundle_3_final_closure.ps1` reads that exact path and checks historical status content. Future retirement work must: inspect that guard's actual historical-proof requirement; move the durable proof to an appropriate retained technical/spec/evidence source, OR intentionally update the guard to the new authoritative ledger only if semantically truthful; prove the guard still fails on the intended negative controls; only then remove the hard path dependency and consider deleting the old ledger. Do NOT weaken or simply delete the guard to enable cleanup.
2. `ACTIVE_PATCH_LEDGER_20260425.md` — not deleted because full migration/deduplication of its backlog-derived content was not proven. Future retirement work must: inventory its unique actionable items; compare against current repo truth/master ledger; migrate only genuinely remaining items; preserve required technical history elsewhere if necessary; then delete if fully redundant.
3. `core-rs/mqk-gui/GUI_PATCH_TRACKER.md` — intentionally retained as a narrow GUI-specific detailed tracker. Its authority must remain scoped to GUI patch detail only; overall backlog/status remains master-ledger authoritative.

**In Scope:** The three retirement sub-tasks above, executed only once their stated blockers are genuinely cleared. **Out of Scope:** Weakening any guard; deleting any tracker before its blocker is proven cleared; performing the retirement work itself as part of this ledger-truth-repair patch (this entry only records the open item).
**Likely Files:** `MiniQuantDesk_Master_Patch_Ledger_v2.md`, `ACTIVE_PATCH_LEDGER_20260425.md`, `scripts/guards/validate_autonomous_daily_paper_operations_01g_bundle_3_final_closure.ps1`, `core-rs/mqk-gui/GUI_PATCH_TRACKER.md`.
**Required Validation:** The specific guard(s) touched must still fail on their intended negative controls after any change; `git diff --check`.
**Acceptance Criteria:** No unique actionable work lost; no guard weakened; no dangling references; old v2 ledger removed only after its hard dependency is eliminated; obsolete April ledger removed only after unique backlog migration is proven; this master ledger remains the sole repository-wide backlog authority.
**Exact CLOSED End State:** CLOSED when all three sub-blockers are cleared per their stated conditions and the corresponding documents are either migrated-and-deleted or explicitly re-scoped, with the acceptance criteria above proven.
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

**OFFICIAL-DUAL-MODE-LAUNCHER-01-REPAIR-03 (this update):** independent GitHub review of commit `9fadcbb899f7adb34d7334387d47ef11da384de8` found two remaining deterministic refresh-ownership defects; both repaired in this same worktree/branch, still not merged/accepted.
- **Defect 1 (process-identity fallback too weak):** REPAIR-02's `Test-RefreshOwnerProcessAlive` fell back to a process-name-only verdict (`ProcessName -match '^powershell'`) whenever `Get-CimInstance Win32_Process` failed, silently accepting any live PowerShell process — including one Windows later reused for an unrelated script under a stale owner PID — as a valid launcher-managed refresh owner. Replaced with `Get-RefreshOwnerProcessIdentity`, which returns one of four distinguishable, never-collapsed states: `dead` (PID no longer exists — safe to replace), `wrong_process` (live, but not PowerShell, or PowerShell with a verified non-matching command line — safe to replace the *record*, the unrelated process is never touched), `verified_refresh_owner` (live PowerShell with a CIM-confirmed `Refresh-IntradayMarketData.ps1` command line), and `identity_unavailable` (CIM/WMI query failed or returned no command line). `identity_unavailable` is never treated as reusable or as safe to replace — `Get-IntradayRefreshOwnerState` reports `Disposition='identity_unavailable', Reusable=$false` for it, and the caller (`Request-IntradayRefreshOwnership`) returns `IDENTITY_UNPROVEN` and refuses to start a replacement, fail-closing that launcher run (`REFRESH_OWNER_IDENTITY_UNPROVEN`) rather than risking either a duplicate loop or an unproven assumption about the ambiguous PID. No identity-check state ever calls `Stop-Process`/kills any process (unchanged invariant, re-verified for all new code).
- **Defect 2 (owner acquisition not atomic):** REPAIR-02's flow (`Get-IntradayRefreshOwnerState` → not reusable → `Start-Process` → `Set-IntradayRefreshOwnerRecord`) had a read/write race: two concurrent launcher invocations could both observe "not reusable" before either wrote a record, each starting its own refresh child. Fixed by `Request-IntradayRefreshOwnership`, now the single entry point for owner acquisition, which holds a deterministic named cross-process `System.Threading.Mutex` (`Local\MiniQuantDeskV4-Paper-IntradayRefreshOwner-<sha256(RepoRoot)[:16]>`; `Local\` chosen over `Global\` because this launcher only ever runs in the operator's own interactive/Task-Scheduler logon session, never across Terminal Services sessions or as a service, avoiding any `SeCreateGlobalPrivilege` permission risk) for the entire critical section: acquire (bounded `WaitOne($LockTimeoutMilliseconds)`, default 15000ms, abandoned-mutex-aware via a typed `catch [System.Threading.AbandonedMutexException]`) → **mandatory re-read** of the owner record (the pre-lock state is stale by the time the lock is granted) → validate → reuse or start a replacement → write the owner record → release in `finally` (`ReleaseMutex()` + `Dispose()`, executes even when the critical section throws — proven by a real cross-thread re-acquisition test, not a same-thread check, since Windows named mutexes are thread-affine/recursive and a same-thread re-check would pass trivially either way). Lock-acquisition timeout returns `LOCK_TIMEOUT`/`REFRESH_OWNER_LOCK_TIMEOUT` and starts no child. Item 8 (start-failure proof): after `Start-Process`, a bounded alive-check (`Start-Sleep -Milliseconds $StartAliveCheckMilliseconds`, default 700ms, then `Get-Process`) must pass before the owner record is written — a child that exits immediately produces `START_FAILED`, never a false-green record.
- **Item 9 (authoritative market_date):** `$marketDateLabel` was `Get-Date -Format 'yyyy-MM-dd'` (machine-local calendar date, timezone-dependent). `Get-AuthoritativeIntradayRefreshDuration` now also extracts and returns `market_date` from the same `GET /api/v1/market-data/readiness` response already used for `session_close_utc`/`calendar_coverage_state`, and fails closed (`Ok=$false`) if `market_date` is blank alongside the existing close-truth checks — making the official launcher's refresh-ownership scope timezone-independent.
- Startup order, arm contract, `-CheckOnly` read-only guarantee, and Live-mode behavior are all unchanged by REPAIR-03 (mission sections 10–14); `Request-IntradayRefreshOwnership` is never referenced by the `-CheckOnly` branch (re-verified by static guard).

**OFFICIAL-DUAL-MODE-LAUNCHER-01-REPAIR-04 (this update):** independent GitHub review of commit `59dc2540aef1b2156a10a60945545d6b7a135ba5` found one remaining durable-handoff gap; repaired in this same worktree/branch, still not merged/accepted.
- **Defect (owner write not durable / process identity still basename-only):** `Set-IntradayRefreshOwnerRecord` previously wrote the authoritative owner JSON directly with `Set-Content`; if that write threw after a refresh child had already started and passed its bounded alive-check, the child could remain alive while the owner record was absent/corrupt, and `Get-RefreshOwnerProcessIdentity` verified only the basename `Refresh-IntradayMarketData.ps1`, which is identical across every worktree of this repo (a process belonging to a different worktree/repo could be mistaken for this repo's owner). Fixed with four changes: (1) `Set-IntradayRefreshOwnerRecord` now serializes the complete record, writes it to a unique same-directory sibling temp file, then finalizes with a single atomic same-volume operation — `[System.IO.File]::Move` when the target is absent, `[System.IO.File]::Replace` (with an explicit same-directory backup path — passing `$null` for the backup argument throws `ArgumentException: The path is not of a legal form` under this box's Windows PowerShell 5.1 method-argument marshalling, so a real backup path is created and removed instead) when it already exists, so the target is only ever observed fully-absent, fully-previous, or fully-new, never partially written; any failure removes the abandoned temp file and re-throws. (2) `Get-RefreshOwnerProcessIdentity` now takes a mandatory `ExpectedScriptPath` and requires the full normalized `<RepoRoot>\scripts\windows\Refresh-IntradayMarketData.ps1` path (not just the basename) to appear in the actual `Win32_Process` command line, plus optional `ExpectedSymbols`/`ExpectedTimeframe` verified whenever those flags are present on the command line (`DurationSeconds` is deliberately never compared — a retry naturally computes a shorter remaining session window for what is still the same owner); the four disposition states (`dead`/`wrong_process`/`identity_unavailable`/`verified_refresh_owner`) are unchanged. A new `Test-RefreshCommandLineIdentity` helper backs both this function and the new orphan scanner, so the two never drift. (3) `Request-IntradayRefreshOwnership` now positively re-verifies the newly-created child's exact identity *before* ever writing the owner record, and wraps the owner-write call in try/catch; on either failure it calls a single new function, `Stop-NewlyCreatedRefreshChild` — the ONLY place in the file permitted to call `Stop-Process`, and only ever with the exact PID this invocation itself just created via `Start-Process` a few lines above (never a PID loaded from an owner record, never a reused/adopted/scope-mismatched/identity-unavailable PID, never any other process) — then bounded-polls (default 3000ms) for it to exit. A confirmed-exited cleanup returns `OWNER_PERSIST_FAILED` (identity failure returns `START_FAILED`); a child that cannot be confirmed terminated within the bound returns the stronger `OWNER_PERSIST_FAILED_CHILD_STILL_PRESENT` / `START_FAILED_CHILD_STILL_PRESENT` and prints an operator-visible PID warning via `Write-Fail`. Neither path is ever taken against a pre-existing or adopted process. (4) A new `Find-MatchingRefreshOwnerProcesses` orphan-recovery scan runs inside the ownership mutex, after the reusable/`identity_unavailable` checks and before any replacement child is started: it enumerates live `powershell.exe` processes for an exact-matching identity (full script path + symbol/timeframe scope) — zero matches falls through to starting a new child unchanged; exactly one match is adopted (owner record durably written for the orphan's existing pid, `Outcome='REUSED'`, no second child started, and an adoption-write failure never terminates the adopted pre-existing process); more than one match fails closed with `Outcome='MULTIPLE_MATCHES'`/`REFRESH_OWNER_MULTIPLE_MATCHES` without starting a third child or terminating either candidate; process-enumeration failure itself fails closed as `IDENTITY_UNPROVEN` rather than being treated as zero matches. The caller-side outcome dispatch in `Invoke-PaperStartup` gained explicit cases for all four new outcomes plus a `default` fail-closed branch so an unrecognized outcome can never silently fall through to launcher success.
- Startup order, arm contract, `-CheckOnly` read-only guarantee (still never references any REPAIR-04 machinery), and Live-mode behavior are all unchanged by REPAIR-04.

**Problem:** No single official entrypoint existed for starting MiniQuantDesk; operators had to know whether to run `Launch-VeritasLedger.ps1` or `Start-PaperTradingSmoke.ps1`, and no Live-mode surface existed at all. (REPAIR-01 closed the four original defects; REPAIR-02 closed two further pre-open/idempotency integration defects found by independent review of the REPAIR-01 commit; REPAIR-03 closed two further deterministic refresh-ownership defects — weak identity fallback and a non-atomic owner-acquisition race — found by independent review of the REPAIR-02 commit; REPAIR-04 closed the remaining durable-handoff gap — non-atomic owner write and basename-only process identity — found by independent review of the REPAIR-03 commit.)
**Dependencies:** NONE.
**In Scope:** Interactive Paper/Live menu; explicit `-Mode`/`-CheckOnly`/`-Scheduled`/`-ArmPaper` (legacy no-op) CLI surface; `-Scheduled` with no `-Mode` fails closed (`STARTUP_REFUSED`, exit 2); Paper full-run now owns DB/Docker/migration prerequisites, delegates daemon/GUI bootstrap to `Launch-VeritasLedger.ps1 -Mode Observe` (REPAIR-02 — no longer `TradeReady`, which was pre-open-circular), resolves the authoritative symbol universe via `GET /api/v1/market-data/ingest-plan` + `Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan`, runs the broker-baseline-adopt + reconcile hard gate, runs halt recovery (disarm→clear-halted-run) if needed, always arms and verifies `arm_state=="armed"`, then atomically starts or idempotently reuses (REPAIR-02 ownership tracking, REPAIR-03 atomic single-owner lock + four-state process-identity contract) an authoritative full-session-length `Refresh-IntradayMarketData.ps1` background loop with an authoritative `market_date` (REPAIR-03), and never calls the `start-system` action_key — runtime start authority stays with the autonomous session controller. Live mode unchanged by REPAIR-01/REPAIR-02/REPAIR-03 (out of scope): seven read-only/source-guard preflight checks that dynamically read `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` and `research-py/src/mqk_research/deployment/parity.py`; interactive non-CheckOnly Live requires a typed `LIVE` confirmation; Live never starts a process, calls a broker, or mutates a DB. **Out of Scope (explicitly not done):** Windows Task Scheduler registration (`PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01` stays BLOCKED); any Rust lifecycle change; any Live behavior expansion; any change to `live_trust_complete`, broker trust rules, live reconciliation, live risk, live execution, shadow parity, evidence signing, or live capital authorization; any change to `Launch-VeritasLedger.ps1`'s own `TradeReady` mode definition.
**Likely Files / Surfaces:** `scripts/windows/Start-MiniQuantDesk.ps1` (repaired four times: REPAIR-01, REPAIR-02, REPAIR-03, REPAIR-04), `scripts/windows/Launch-VeritasLedger.ps1` (unchanged since the original patch — REPAIR-01's narrow `-SkipGui` addition is its only delta, REPAIR-02/REPAIR-03/REPAIR-04 touched nothing here), `scripts/windows/tests/test_official_dual_mode_launcher.ps1` (repaired, +25 REPAIR-01 assertions, +27 REPAIR-02 assertions, +36 REPAIR-03 assertions, +38 REPAIR-04 assertions), this ledger.
**Required Implementation Rules:** One patch, minimal scope, no bundling with any Rust/Python change; built and committed only in the isolated `-ops` worktree; the protected paper-soak `main` worktree was never checked out to another branch, never had a new branch created inside it, and received zero commits from this session.
**Safety / Compatibility Requirements:** `-CheckOnly` never arms, clears halt, starts runtime, submits orders, mutates DB, runs migrations, starts/mutates Docker containers, launches broker activity, acquires the ownership lock, or creates an active refresh-ownership record (proven by guard-test Section 1/3/5/6/7 CheckOnly-scope checks + Section 2 real invocation + real `-CheckOnly` run in this session showing zero mutation and no ownership file created). Live mode never enables live routing, never sets `MQK_DAEMON_DEPLOYMENT_MODE`/`MQK_DAEMON_ADAPTER_ID` to a live value, and never prints `ALPACA_API_KEY_LIVE`/`ALPACA_API_SECRET_LIVE` values. `-Scheduled -Mode Live` fails closed (`unattended_live_start_not_authorized`, exit 6). Paper DB hard fence: `MQK_DATABASE_URL` is always reasserted to `127.0.0.1:5440/miniquantdesk_paper`, never `5432`/`5434`. Refresh-ownership/identity checks never call `Stop-Process`/kill any process ARBITRARILY (proven by static source guard scoped to exclude exactly one function + real unrelated/ambiguous-identity/multiple-orphan fixture PowerShell processes surviving every check in this session). As of REPAIR-04, exactly one narrowly-scoped exception exists: `Stop-NewlyCreatedRefreshChild` may terminate ONLY the exact PID the current `Request-IntradayRefreshOwnership` invocation itself just created via `Start-Process`, and only when that same invocation's own post-start identity verification or durable owner-record persistence fails — never a PID loaded from an owner record, never a reused/adopted PID, never a scope-mismatched/identity-unavailable PID, never an orphan-adoption candidate (adoption-write failures leave the adopted pre-existing process untouched by design). Proven via forced owner-persist-failure and post-start-identity-failure fixtures in this session, both confirming the created child is terminated while a co-located unrelated/pre-existing process, and both multiple-orphan candidates, survive untouched.
**Required Negative Controls:** `-Scheduled` with no `-Mode` → exit 2 (proven). `-Mode Live -Scheduled` → exit 6, no interactive prompt (proven). `-Mode Live -CheckOnly` → completes without hanging on stdin, reports BLOCKED with real ledger patch IDs (proven). Unavailable session-close truth (including a blank `market_date`, REPAIR-03) → `ExitDataReadiness` (3), never a 1800s fallback (proven via static guard; no live daemon available in this worktree to prove the dynamic branch end-to-end this session). Mismatched refresh-ownership scope (symbols/timeframe/market-date) → never silently reused (proven via real fixture-process functional test). Stale/dead refresh-owner PID → never reused (proven via real fixture-process functional test with an intentionally-invalid PID). REPAIR-03: an unrelated live PowerShell process → `wrong_process`, never reused, never killed (proven). A CIM/WMI-unprovable live PowerShell process → `identity_unavailable` → `IDENTITY_UNPROVEN`, launcher fails closed, no replacement started, existing record left unchanged (proven via `Get-CimInstance` function-shadow fixture). A held ownership lock → `LOCK_TIMEOUT` within the bounded timeout, no child started, no record written (proven via a background-job external holder on a separate thread). A malformed owner record forcing an uncaught exception inside the locked critical section → the mutex is still released in `finally`, verified from a separate thread/process, not a same-thread recursive-acquire false-positive (proven). REPAIR-04: same script basename under a *different* expected worktree/repo path → `wrong_process`, never `verified_refresh_owner` (proven). Correct script path but wrong symbol scope, or correct path but wrong timeframe scope → never `verified_refresh_owner` (proven via a real fixture process started with real `-Symbols`/`-Timeframe` command-line args, mirroring exactly how the launcher itself invokes the refresh child). A forced durable owner-write failure after a real child has started and passed identity verification → `OWNER_PERSIST_FAILED`, the created child is confirmed no longer alive afterward, no owner record is ever written, and a co-located unrelated pre-existing process survives untouched (proven via a `ConvertTo-Json` function-shadow fixture scoped so only the serialization step inside `Set-IntradayRefreshOwnerRecord` fails — the mandatory re-read, orphan scan, real `Start-Process`, and post-start identity check all still execute for real first). Two exact-matching orphan processes with no owner record → `MULTIPLE_MATCHES`/`REFRESH_OWNER_MULTIPLE_MATCHES`, no third child started, neither existing fixture process killed, no owner record written (proven).
**Required Positive Controls:** `-Mode Paper -CheckOnly` → delegates to and surfaces `Launch-VeritasLedger.ps1 -CheckOnly`'s real read-only report (proven; this dev worktree correctly reports a prerequisite-check failure because `.env.local` was never copied into it — expected, not a launcher defect; re-run after REPAIR-04 confirms zero mutation and no refresh-ownership file created, `mqk-paper-postgres` container was already running from prior work and untouched by this run). Matching-scope refresh-owner with a live, CIM-verified PID → reused, no second process started (proven via real fixture-process functional test). REPAIR-03: two concurrent real PowerShell processes racing the same owner scope against `Request-IntradayRefreshOwnership` → exactly one `STARTED` + one `REUSED`, exactly one live fixture refresh process, exactly one owner-record file, the `REUSED` caller's observed pid equals the `STARTED` caller's pid (proves the mandatory post-lock re-read) — proven via two real `Start-Job` background processes against a shared disposable fixture repo (unmodified and re-affirmed by REPAIR-04). REPAIR-04: a single exact-matching orphan process with no owner record → adopted (`Outcome='REUSED'`, `Pid`=the orphan's real pid, owner record durably written reflecting that pid), and exactly zero additional refresh children are started for that scope (proven via real fixture-process functional test).
**Required Regression Tests:** `scripts/windows/tests/test_official_dual_mode_launcher.ps1` — 161/161 green (123 REPAIR-01/REPAIR-02/REPAIR-03-era assertions, all retained — two static Stop-Process-absence guards were necessarily narrowed from "never anywhere in the ownership block" to "never anywhere in the ownership block outside the one new `Stop-NewlyCreatedRefreshChild` function", since REPAIR-04 introduces the file's first legitimate, narrowly-scoped process-termination path; direct `Get-RefreshOwnerProcessIdentity` calls and two fixture-script layouts were updated to supply the new mandatory `ExpectedScriptPath` parameter and to live at the real `scripts\windows\` sub-path the strengthened identity check now requires — plus 38 new REPAIR-04 assertions in Section 7 covering exact-path/scope identity (including same-basename-other-worktree rejection), atomic owner-write source guards, forced owner-persist-failure cleanup (child confirmed dead, no false-success record, unrelated process untouched), orphan adoption, and multiple-orphan fail-closed protection). `Launch-VeritasLedger.ps1` untouched by REPAIR-04 (unchanged since REPAIR-01).
**Required Validation:**
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\tests\test_official_dual_mode_launcher.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Start-MiniQuantDesk.ps1 -Mode Paper -CheckOnly
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\Start-MiniQuantDesk.ps1 -Mode Live -CheckOnly
```
**Forbidden Validation / Side Effects:** No real live order, no live runtime start, no live DB mutation, no push, no merge to `main`. No full (non-CheckOnly) Paper startup was run from this dev worktree this session (would contend with the protected paper-soak environment) — the arm-guarantee, DB-prerequisite, pre-open-Observe, refresh-ownership, process-identity, atomic-lock, atomic-owner-write, and orphan-recovery code paths are proven by static source-guard tests plus real-but-disposable fixture-process functional tests (temp repos, temp PowerShell sleep processes, real `System.Threading.Mutex`/`Start-Job` concurrency, function-shadow fixture seams for deterministic failure injection, never a real daemon/broker/trading runtime), not a live end-to-end dynamic run.
**Acceptance Criteria:** 1) `-Mode`/`-CheckOnly`/`-Scheduled`/`-ArmPaper` all behave exactly as specified, with `-ArmPaper` no longer required. 2) Live mode reports real, current ledger-sourced blockers, never a fabricated verdict; unchanged by REPAIR-01/REPAIR-02/REPAIR-03/REPAIR-04. 3) Paper mode never manually invokes `start-system`. 4) Official full Paper startup always establishes and verifies `arm_state=="armed"` before success. 5) Session refresh duration and `market_date` (REPAIR-03) are derived from authoritative NYSE-calendar truth, fails closed when either is unavailable. 6) Official launcher owns Docker/paper-DB/migration prerequisites. 7) Daemon bootstrap uses `Observe`, not `TradeReady`, so a pre-open `-Scheduled -Mode Paper` run can reach and pass its own arm stage without `session_in_window=true`. 8) The intraday refresh loop is idempotent and atomic: same-scope reuse of an exact-path-and-scope-verified live owner (REPAIR-04), exactly one replacement or orphan-adoption for a dead/wrong-process/mismatched/absent owner, fail-closed refusal (never reuse, replace, or blindly start) when identity or process enumeration cannot be proven, a single-owner cross-process lock with mandatory post-lock re-read, bounded/fail-closed lock timeout, release-in-`finally` even on exception, durable atomic owner-record persistence with narrowly-scoped created-child-only cleanup on failure (REPAIR-04), never a killed arbitrary/pre-existing/adopted process, no secrets in the ownership record, `-CheckOnly` never acquires the lock or creates an active ownership record. 9) Guard-test suite green (161/161). 10) Protected `main` worktree provably untouched.
**Exact CLOSED End State:** CLOSED when an operator has independently reviewed the diff (including this REPAIR-04 update) in the `-ops` worktree, confirmed the protected `main` baseline is unaffected, and either merges via an explicit separate decision or accepts the branch as the new operational default — none of which this patch itself performs.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING (REPAIR-01: PENDING / PENDING / PENDING / PENDING; REPAIR-02: PENDING / PENDING / PENDING / PENDING; REPAIR-03: PENDING / PENDING / PENDING / PENDING; REPAIR-04: PENDING / PENDING / PENDING / PENDING).

#### PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01 — Unify the official launcher's market-data authority onto the daemon required-universe scheduler

**Status:** IMPLEMENTED_PENDING_REVIEW · **Priority:** P1 · **Paper Impact:** GREEN (orchestration-script-only; removes a redundant PowerShell-owned refresh-child subsystem, zero Rust/Python trading code touched) · **Subsystem:** Ops tooling / operator launcher

**Current Source Truth:** Built in the isolated worktree `C:\Users\Zacha\Desktop\MiniQuantDeskV4-integration`, branch `integrate-paper-autofresh-launcher`, created from the accepted `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01` chain HEAD `f6e769728cfe6e4febd249c0c9db97d52a509d9d` with `origin/ops-official-launcher` (`OFFICIAL-DUAL-MODE-LAUNCHER-01` chain, parent `e4f9eb92ade32e2f7d7e5cc7c45a0c6dea18c8ba`) merged in via `git merge --no-ff`. Not merged to `main`.

**Problem:** `OFFICIAL-DUAL-MODE-LAUNCHER-01` (through REPAIR-04) and `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01` (through REPAIR-02 plus `MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01`) were both independently accepted-but-unmerged lines of work that each built a competing long-running market-data maintenance authority for official Paper startup: the launcher started and tracked its own background `Refresh-IntradayMarketData.ps1` child process via a PID/mutex/JSON-file ownership subsystem, while the daemon separately grew a process-local required-universe scheduler (`POST/GET /api/v1/market-data/required-universe/{start,status}`) that already owns required-symbol resolution, provider/timeframe admission, bounded historical bootstrap, latest-bar repair, and session-anchored cadence. Merging both branches as-is would have left two independent, potentially-conflicting market-data refresh authorities active during the same Paper session.

**Fix:** `scripts/windows/Start-MiniQuantDesk.ps1`'s official Paper startup (`-Mode Paper`, both interactive and `-Scheduled`) now uses the daemon's required-universe scheduler as its SOLE ongoing market-data authority. Two new self-contained functions, `Confirm-RequiredUniverseSchedulerOwnership` / `Start-OrVerifyRequiredUniverseScheduler`, are ported (not reinvented) from `Start-PaperTradingSmoke.ps1`'s already-accepted STEP 8D fail-closed start/verify contract (`MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-02`), adapted only to this launcher's own `Invoke-JsonGet`/`Invoke-JsonPost` HTTP helpers. A `200`/`409` response from `.../required-universe/start` is never itself treated as proof of maintenance authority — the scheduler's own status route is always (re-)checked for `running=true` AND `dry_run=false` AND a present, non-`blocked` report; a genuine `overall_state=not_applicable` (non-trading day / empty required universe) is accepted as legitimate no-work, never a failure. This establishment now runs BEFORE reconcile/halt-recovery/arm (previously the refresh-loop stage ran AFTER arm) — the launcher refuses with `ExitDataReadiness` (3) before any reconcile/halt-recovery/arm side effect whenever required-universe authority is not proven. The `Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan` pre-step was removed from the official launcher: the daemon required-universe scheduler's own immediate cycle already owns strict readiness evaluation, bounded historical bootstrap, latest expected-bar repair, provider mapping, and provider provenance for the current accepted config (AAPL/5m/alpaca, live Alpaca credentials present), making that PowerShell-side pre-step fully redundant for official startup; `GET /api/v1/market-data/ingest-plan` is retained but only for operator display/logging, never to build a provider universe or gate startup. The entire prior refresh-ownership subsystem — `Get-IntradayRefreshOwnerPath`, `Get-RefreshOwnerProcessIdentity`, `Get-IntradayRefreshOwnerState`, `Set-IntradayRefreshOwnerRecord`, `Request-IntradayRefreshOwnership`, `Get-IntradayRefreshOwnerLockName`, `Test-RefreshCommandLineIdentity`, `Find-MatchingRefreshOwnerProcesses`, `Stop-NewlyCreatedRefreshChild`, and `Get-AuthoritativeIntradayRefreshDuration` (the session-close-duration helper that sized the removed refresh loop) — was removed (confirmed via repo-wide grep: referenced nowhere else in the repo). `Refresh-IntradayMarketData.ps1` itself is untouched and remains available as a documented manual/compatibility operator tool (it already carries its own conflict guard against a running required-universe scheduler). `-CheckOnly` remains strictly read-only (only a `GET .../required-universe/status` was added for operator visibility, never the `POST .../start`); Live mode is completely unchanged (never reaches the Paper required-universe route). `scripts/windows/tests/test_official_dual_mode_launcher.ps1` was rewritten: the ownership-subsystem proof sections (formerly Sections 5-7) were removed along with the code they proved, and replaced with a new L1-L12 proof set (Section 5) covering: required-universe scheduler used and established before reconcile/halt-recovery/arm (L1); POST failure fails closed before reconcile/arm (L2); `overall_state=blocked` fails closed (L3); a `409` reused `dry_run=true` owner is refused (L4); a `409` reused verified non-dry owner continues (L5); `not_applicable`/non-trading-day is accepted no-work (L6); no `Refresh-IntradayMarketData.ps1` child is started by normal startup (L7); `-CheckOnly` starts neither the scheduler nor the refresh child (L8); `-Scheduled` uses the identical daemon-scheduler path as interactive (L9); a multi-symbol required-universe response is not collapsed to one symbol (L10); Live mode never reaches the route (L11); arm is still verified before success (L12) — using the same dot-source-then-function-shadow mock seam this file and `validate_market_data_autofresh_required_universe_01_repair_02.ps1` already use for `Invoke-JsonGet`/`Invoke-JsonPost`/`Get-CimInstance`/`ConvertTo-Json`, with zero real daemon/network/DB/order/runtime side effects. `docs/runbooks/operator_workflows.md` was updated narrowly to state that both `-Mode Paper` and `-Mode Paper -Scheduled` rely on the daemon required-universe scheduler and to remove the claim that the official launcher owns a `Refresh-IntradayMarketData.ps1` child process, while keeping `Refresh-IntradayMarketData.ps1` documented as a manual/compatibility utility.

**Dependencies:** `OFFICIAL-DUAL-MODE-LAUNCHER-01` (through REPAIR-04), `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01` (through REPAIR-02), `MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01` — all four accepted/pending-integration lines this patch unifies.
**Unlocks:** Nothing new; removes the duplicate-authority blocker that would otherwise complicate merging either parent chain.
**In Scope:** `scripts/windows/Start-MiniQuantDesk.ps1`, `scripts/windows/tests/test_official_dual_mode_launcher.ps1`, this ledger entry, narrow updates to `docs/runbooks/operator_workflows.md`.
**Out of Scope:** Any Rust/Python source change (none was needed); `Start-PaperTradingSmoke.ps1` (unchanged, already the accepted reference implementation this integration mirrors); `Refresh-IntradayMarketData.ps1` (unchanged, remains a manual/compatibility tool); `Prep-PremarketMarketData.ps1` itself (unchanged — only its call site inside the official launcher was removed; the script remains available for other callers); Live mode behavior; any risk/OMS/portfolio/reconcile/broker/halt/kill-switch semantics; `PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01` (still BLOCKED, untouched, out of scope for this patch).
**Required Implementation Rules:** The daemon required-universe scheduler is the launcher's sole ongoing market-data authority — no second PowerShell-owned refresh loop, no launcher-rebuilt provider universe, no hardcoded symbol/timeframe fallback; a `200`/`409` `.../start` response is never itself proof of authority — the status route is always (re-)checked; required-universe establishment must complete, fail-closed, strictly before reconcile/halt-recovery/arm.
**Safety / Compatibility Requirements:** `-CheckOnly` never starts the scheduler, calls the provider, writes `md_bars`, reconciles, clears halt, arms, starts runtime, or starts the refresh child (unchanged, re-verified). Live mode never starts a Paper daemon, never starts the Paper required-universe scheduler, never arms live, never alters live trust/readiness/confirmation gates (unchanged, re-verified). `AUTONOMOUS-DAILY-OPERATOR-RETRY-01`'s retry route is not automatically invoked by this launcher (unchanged, explicit/manual, out of scope). The daemon scheduler's status is process-local, not persisted (`limitation=process_local_only_not_persisted`) — a freshly started daemon always re-establishes the scheduler; this launcher never infers scheduler ownership from stale files/PIDs (the entire prior PID/mutex/JSON-file subsystem this depended on is removed). `MARKET-DATA-AUTOFRESH-TEST-TIME-DETERMINISM-01`'s documented facts are preserved verbatim and unmodified by this integration: configured future-skew default = 300s, effective ceiling = `min(configured, 60, timeframe)`, 5m effective default = 60s.
**Required Regression Tests:** `scripts/windows/tests/test_official_dual_mode_launcher.ps1` — 88/88 green (rewritten). `scripts/guards/validate_market_data_autofresh_required_universe_01_repair_01.ps1` and `_repair_02.ps1` — both green, unaffected (target `Start-PaperTradingSmoke.ps1`, not touched by this patch). `mqk-daemon --test scenario_market_data_autofresh_plan_resolution_01` (13/13), `scenario_daily_data_readiness_01` (66/66), `scenario_market_data_latest_bar_poll_01` + `scenario_market_data_latest_bar_scheduler_01` (17+6=23/23) — all green, unmodified by this patch (zero Rust source touched). `mqk-daemon --test scenario_market_data_autofresh_required_universe_01`: **result depends on who/where it is run, documented honestly rather than reconciled.** The implementing agent's complete tally in this worktree, on commit `f6e76972`: 0/8 passing runs — 3 full-suite invocations (`-- --test-threads=1[--include-ignored]`, no name filter) and 5 isolated single-test invocations (name filter and `--exact`), every single one failing identically at `stop_start_generation_race_old_cycle_cannot_overwrite_new_owner` with `A's provider call must start within 10s: Elapsed(())` at `scenario_market_data_autofresh_required_universe_01.rs:1798`. The reviewing coordinator's tally, same commit, separate session: 6/6 full-suite runs clean (18/18 each) plus 1/1 isolated `--exact` run reproducing the identical failure — i.e. the isolated-invocation result is consistent between both parties, but the full-suite result is not. Neither party has a confirmed mechanism for the full-suite discrepancy; a plausible but unverified theory is that background/sandboxed tool-execution contexts (as used by the implementing agent) may run under tighter resource constraints than an interactive session, making the test's hard-coded real-`Utc::now()` 10-second `tokio::time::timeout` budget (not the deterministic `now_fixture()`/barrier pattern used elsewhere in this file) tighter to clear. What both parties independently confirmed: (1) this patch made zero Rust source changes (`git diff --stat` shows only `Start-MiniQuantDesk.ps1` and its test file touched), so whatever is happening with this test's timing is categorically not caused by this integration; (2) the commit under test is identical (`f6e769728cfe6e4febd249c0c9db97d52a509d9d`) in both sessions. Follow-up hardening candidate (out of scope for this integration): replace the real-`Utc::now()` 10-second wall-clock timeout in `stop_start_generation_race_old_cycle_cannot_overwrite_new_owner` with a deterministic `tokio::sync::Notify`/barrier pattern (as the rest of this file already uses), or run it under `#[tokio::test(flavor = "multi_thread")]`, so its pass/fail no longer depends on wall-clock budget or execution-context load.
**Required Validation:**
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\tests\test_official_dual_mode_launcher.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\guards\validate_market_data_autofresh_required_universe_01_repair_01.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\guards\validate_market_data_autofresh_required_universe_01_repair_02.ps1
git diff --check
```
```
$env:MQK_DATABASE_URL = "postgresql://postgres:postgres@127.0.0.1:5434/mqk_test"
cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_market_data_autofresh_required_universe_01 --test scenario_market_data_autofresh_plan_resolution_01 --test scenario_daily_data_readiness_01 --test scenario_market_data_latest_bar_poll_01 --test scenario_market_data_latest_bar_scheduler_01 -- --test-threads=1 --include-ignored
cargo check --manifest-path .\core-rs\Cargo.toml -p mqk-daemon
```
**Forbidden Validation / Side Effects:** No real Paper runtime start, no real broker/order calls, no live anything, no mutation of the real paper Postgres (port 5440), no push, no merge to `main`, no touching any other worktree (`MiniQuantDeskV4`, `MiniQuantDeskV4-autofresh`, `MiniQuantDeskV4-ops`, `MiniQuantDeskV4-retry`, `MiniQuantDeskV4-data`), no touching `smoke_logs/`.
**Acceptance Criteria:** 1) Official Paper launcher uses the daemon required-universe scheduler as its sole ongoing market-data authority — proven by L1-L12. 2) `Prep-PremarketMarketData.ps1` is no longer invoked by the official launcher, with the daemon-side redundancy proof documented above. 3) No second PowerShell-owned refresh loop remains reachable from official Paper startup. 4) `-CheckOnly` and Live mode behavior are unchanged and still strictly read-only/non-Paper-authoritative respectively. 5) Both parent chains' full ledger history is preserved in this file (verified by grep before and after merge). 6) The `stop_start_generation_race_old_cycle_cannot_overwrite_new_owner` cross-session tally discrepancy is documented honestly (both parties' actual numbers, not a reconciled or asserted-but-unobserved result), confirmed unrelated to this patch by both parties independently.
**Exact CLOSED End State:** Not yet CLOSED — `IMPLEMENTED_PENDING_REVIEW` until code-reviewed and merged. The documented `stop_start_generation_race_old_cycle_cannot_overwrite_new_owner` execution-context timing discrepancy does not block this patch's own closure (zero Rust touched, confirmed independently by both the implementing agent and the reviewing coordinator) but should be tracked as a standalone test-hardening follow-up.
**Expected Handoff:** Start HEAD `f6e769728cfe6e4febd249c0c9db97d52a509d9d` merged with `e4f9eb92ade32e2f7d7e5cc7c45a0c6dea18c8ba`; end HEAD = new merge commit on `integrate-paper-autofresh-launcher`; not pushed, not merged to `main`.

#### PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01-REPAIR-01 — Fail closed on invalid required-universe state

**Status:** IMPLEMENTED_PENDING_REVIEW · **Priority:** P0 (final integration blocker) · **Paper Impact:** GREEN (orchestration-script-only, zero Rust/Python touched) · **Subsystem:** Ops tooling / operator launcher

**Current Source Truth:** Built in the same isolated worktree/branch as the parent patch, `C:\Users\Zacha\Desktop\MiniQuantDeskV4-integration`, branch `integrate-paper-autofresh-launcher`, starting from the pushed integration commit `1a9c4b8f150e728675d3aa996c4cef844da10c2e`. Not merged to `main`, not pushed.

**Problem:** The daemon's required-universe scheduler (`core-rs/crates/mqk-daemon/src/state/required_market_data_autofresh.rs`) legitimately returns `overall_state=not_applicable` for two distinct situations: a genuine non-trading day (`is_trading_day=false`) and an empty resolved required-symbol set, which can occur even on a trading day (a configuration defect). The official launcher's `Start-OrVerifyRequiredUniverseScheduler` and `Confirm-RequiredUniverseSchedulerOwnership` treated `overall_state=not_applicable` as always-successful no-work without checking `is_trading_day`, and treated any `overall_state` other than `blocked` as success (an "anything except blocked = ready" fallthrough). This meant a normal trading day with an empty required universe — or a future/malformed status report carrying an unrecognized `overall_state` — could incorrectly be accepted as established data-maintenance authority and continue toward reconcile/halt-recovery/arm instead of failing closed.

**Fix:** Introduced one shared helper, `Test-RequiredUniverseReportAcceptable`, in `scripts/windows/Start-MiniQuantDesk.ps1`, implementing the single closed-set interpretation of a required-universe report's `overall_state` used by both `Confirm-RequiredUniverseSchedulerOwnership` (409 reuse / post-start verification) and `Start-OrVerifyRequiredUniverseScheduler` (200 start response), so the two paths can no longer diverge. Only four outcomes are legitimate: `ready` → acceptable (active maintenance, existing ownership requirements `running=true`/`dry_run=false`/report-present still apply via `Confirm-RequiredUniverseSchedulerOwnership`); `blocked` → not acceptable (existing blocker-detail behavior preserved verbatim); `not_applicable` with `is_trading_day=false` → acceptable, legitimate no-work (`REQUIRED_UNIVERSE_NO_WORK_NOT_APPLICABLE`, unchanged from before); `not_applicable` with `is_trading_day=true` → NOT acceptable, fails closed with the new stable reason `REQUIRED_UNIVERSE_NOT_APPLICABLE_ON_TRADING_DAY` and a detail explaining that no authoritative required market-data universe exists for a trading day. Any other `overall_state` — unrecognized string, missing, null, or blank — fails closed with the new stable reason `REQUIRED_UNIVERSE_SCHEDULER_STATE_UNKNOWN`. Both functions were updated to call this one helper instead of each independently re-implementing (and, for `Confirm-RequiredUniverseSchedulerOwnership`, under-implementing) the state check; no other logic in either function changed. Two stale header/inline comments describing the old "non-trading day / empty required universe" combined wording were corrected narrowly (no behavior change) in `Start-MiniQuantDesk.ps1`'s usage-comment block and in `Invoke-PaperStartup`'s required-universe section comment. Zero daemon (`required_market_data_autofresh.rs`, `required_market_data.rs`, `daily_data_readiness.rs`) or other Rust/Python source touched — the daemon's `not_applicable` status modeling is legitimate; the defect was entirely in the launcher's interpretation of it.

**Dependencies:** `PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01` (parent, IMPLEMENTED_PENDING_REVIEW, unchanged by this repair beyond the two functions above).
**Unlocks:** Nothing new; removes a fail-open gap that would otherwise block accepting the parent integration.
**In Scope:** `scripts/windows/Start-MiniQuantDesk.ps1`, `scripts/windows/tests/test_official_dual_mode_launcher.ps1`, this ledger entry.
**Out of Scope:** Any Rust/Python source change (none needed); the parent patch's overall architecture (scheduler-as-sole-authority, establishment ordering, `-CheckOnly`/Live isolation) — unchanged and not redesigned by this repair; `PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01` (still BLOCKED, untouched).
**Required Implementation Rules:** The closed-set state contract (`ready`/`blocked`/`not_applicable`+trading-day) is the only legitimate interpretation of `overall_state` on both the 200-start and 409-reuse paths; no "anything except blocked = ready" fallthrough anywhere in either function.
**Safety / Compatibility Requirements:** Startup order unchanged (Paper DB prerequisites → daemon Paper+Alpaca identity → required-universe scheduler → reconcile → halt recovery → arm → verify armed → success); no runtime start added; `-CheckOnly` remains strictly read-only (scheduler POST/reconcile/arm/order calls all still zero); Live mode unchanged (Paper required-universe scheduler remains unreachable from the Live path); the removed legacy `Refresh-IntradayMarketData.ps1` child-ownership subsystem remains removed (not restored).
**Required Negative Controls:** L13 — 200 response, `overall_state=not_applicable`, `is_trading_day=true`, empty required universe → `Established=false`, `REQUIRED_UNIVERSE_NOT_APPLICABLE_ON_TRADING_DAY` (proves the verified defect is closed). L14 — 200 response, unrecognized `overall_state=mystery_state` → `Established=false`, `REQUIRED_UNIVERSE_SCHEDULER_STATE_UNKNOWN`. L15 — 409 reuse, `running=true`/`dry_run=false`, but the reused scheduler's own report carries `overall_state=mystery_state` → `Established=false`, `REQUIRED_UNIVERSE_SCHEDULER_STATE_UNKNOWN` (a running scheduler is not sufficient if its report state is unrecognized). L16 — report present but `overall_state` missing/null/blank → `Established=false`, `REQUIRED_UNIVERSE_SCHEDULER_STATE_UNKNOWN` (no `$null -ne 'blocked'` optimistic fallthrough). All four proven via the existing functional dot-source/function-shadow mock harness in `test_official_dual_mode_launcher.ps1` Section 5, zero real daemon/network/DB/order/runtime side effects.
**Required Positive Controls:** L6 re-affirmed unchanged — `overall_state=not_applicable`/`is_trading_day=false`/empty universe → `Established=true`, `REQUIRED_UNIVERSE_NO_WORK_NOT_APPLICABLE` (holiday/weekend startup truth not broken by this repair). Existing L3 (`blocked` → fail), L4 (409 dry-run owner → fail), L5 (409 valid non-dry owner → succeed) all re-verified green, unweakened.
**Required Regression Tests:** `scripts/windows/tests/test_official_dual_mode_launcher.ps1` — 93/93 green (88 prior assertions retained unchanged + 4 new REPAIR-01 negative controls L13-L16 + 1 explicit L6 re-affirmation assertion under its own label). `scripts/guards/check_unsafe_patterns.ps1` — all guards passed. No Rust test rerun required or performed (zero Rust source changed by this repair); the separate, already-documented `stop_start_generation_race_old_cycle_cannot_overwrite_new_owner` timing-sensitive Rust-test discrepancy from the parent ledger entry is not reopened here.
**Required Validation:**
```powershell
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\tests\test_official_dual_mode_launcher.ps1
powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\guards\check_unsafe_patterns.ps1
git diff --check
```
**Forbidden Validation / Side Effects:** No real Paper runtime start, no real broker/order calls, no live anything, no push, no merge to `main`, no touching `smoke_logs/` (left untracked and unmodified).
**Acceptance Criteria:** 1) `overall_state=not_applicable` on a trading day with an empty required universe fails closed (L13). 2) Any unrecognized/missing `overall_state` fails closed on both the 200 and 409 paths (L14/L15/L16). 3) The non-trading-day no-work case (L6) is unweakened. 4) The existing `blocked`/dry-run/valid-reuse controls (L3-L5) are unweakened. 5) Zero Rust/Python source changed. 6) Startup ordering, `-CheckOnly` read-only guarantee, and Live isolation are all unchanged. 7) Guard-test suite green.
**Exact CLOSED End State:** Not yet CLOSED — `IMPLEMENTED_PENDING_REVIEW` until code-reviewed. Parent `PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01` remains `IMPLEMENTED_PENDING_REVIEW` and is not marked accepted by this repair.
**Expected Handoff:** Start HEAD `1a9c4b8f150e728675d3aa996c4cef844da10c2e`; end HEAD = new commit on `integrate-paper-autofresh-launcher` titled `fix: fail closed on invalid required universe`; not pushed, not merged to `main`.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.

#### PAPER-SOAK-RUST-TIMING-TEST-HARDENING-01 — Reproduction attempt for the stop/start generation-race timing discrepancy

**Status:** OPEN (not reproduced; no Rust change made) · **Priority:** P2 · **Paper Impact:** GREEN (investigation-only, zero source touched) · **Subsystem:** `mqk-daemon` test suite

**Current Source Truth:** Investigated in the same worktree/branch as the integration patches above, `C:\Users\Zacha\Desktop\MiniQuantDeskV4-integration`, branch `integrate-paper-autofresh-launcher`, starting HEAD `3d2894d39a184b1740faa1f20694dcba5b498f78`. No commit produced beyond this ledger entry.

**Problem:** `PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01` (above) documented a cross-session tally discrepancy on `stop_start_generation_race_old_cycle_cannot_overwrite_new_owner` (`core-rs/crates/mqk-daemon/tests/scenario_market_data_autofresh_required_universe_01.rs:1744`): the implementing agent's background/sandboxed session saw 0/8 passing runs, every failure identical — `A's provider call must start within 10s: Elapsed(())` at line 1798 (the outer `tokio::time::timeout` wrapping `call_started.notified()`, not the deterministic `Notify`-based generation-ownership barrier itself, which was unaffected). The reviewing coordinator's interactive session saw only 1/7 failures on the same commit. This task was scoped to reproduce that discrepancy under a bounded matrix and, only if reproduced, apply a deterministic test-only hardening fix (no sleep/timeout increases, no retries, no `#[ignore]`).

**Reproduction matrix (this session, commit `3d2894d3`, `MQK_DATABASE_URL=postgresql://postgres:postgres@127.0.0.1:5434/mqk_test`):**
- Targeted, normal execution (`-p mqk-daemon stop_start_generation_race_old_cycle_cannot_overwrite_new_owner -- --nocapture`): 1/1 pass (0.24s).
- Targeted, single-thread (`--test-threads=1`): 1/1 pass (0.21s).
- 25x repeated targeted normal execution (bounded PowerShell/bash loop, stop on first failure): 25/25 pass, 0 skipped (confirmed via `test ... ok` line count, not just exit code).
- Narrow historical-context reproduction — the exact command the parent ledger entry's own `Required Validation` block specifies (`--test scenario_market_data_autofresh_required_universe_01 --test scenario_market_data_autofresh_plan_resolution_01 --test scenario_daily_data_readiness_01 --test scenario_market_data_latest_bar_poll_01 --test scenario_market_data_latest_bar_scheduler_01 -- --test-threads=1 --include-ignored`): 41/41 pass across all five files, including the target test.

One methodological note preserved for future investigators: the first attempt at each of these runs silently short-circuited (`skipped DB-backed proof because MQK_DATABASE_URL is not set` → reported as `ok` by the harness) because the ambient environment did not have `MQK_DATABASE_URL` set. That is not the timing discrepancy being investigated — it is a distinct, environment-dependent false-pass hazard specific to invoking this test file without first setting the DB URL, worth flagging separately but out of scope for this task (no source changed).

**No failure was reproduced in this session under any of the four conditions above**, including the specific narrow context the ledger recorded as previously 0/8-failing for the implementing agent. Per audit rules, no deterministic-barrier patch was written on the strength of a historical (unreproduced-here) timing suspicion alone. The original discrepancy is not disproven — only not observed in this session's execution context — so the test remains unmodified and the underlying resource-contention theory in the parent entry stands as the best available explanation.

**Dependencies:** None (standalone investigation, references `PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01`'s discrepancy record above).
**Unlocks:** Nothing; the standalone test-hardening follow-up flagged in `PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01`'s `Exact CLOSED End State` remains open.
**In Scope:** Reproduction only — no files changed except this ledger entry.
**Out of Scope:** Any Rust source or test change (none was warranted); scheduler activation; full canonical/integrated validation; `main`.
**Exact CLOSED End State:** Not CLOSED — `OPEN`. Reclassify to `PARKED` or re-attempt reproduction under harder resource contention (e.g. concurrent background compiles/tests) if this needs to be chased further; do not mark `CLOSED` on the strength of this session's clean tallies alone, since the original discrepancy was itself session-dependent.
**Expected Handoff:** Start HEAD `3d2894d39a184b1740faa1f20694dcba5b498f78`; no Rust changed; ledger-only commit expected on `integrate-paper-autofresh-launcher`; not pushed, not merged to `main`.

#### PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01 — Windows Task Scheduler registration for unattended Paper start

**Status:** IMPLEMENTED_PENDING_REVIEW · **Priority:** P2 · **Paper Impact:** GREEN (additive scheduling only) · **Subsystem:** Ops tooling
**Current Source Truth:** `scripts\windows\Register-PaperStartupTask.ps1` (new) registers/reconciles a permanent Windows Scheduled Task `MiniQuantDesk-Paper-Preopen-Startup` in the `\MiniQuantDesk\` folder whose single action invokes exactly `Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled` (no other launcher argument), Monday-Friday 02:00 local time by default, Interactive/Limited principal as the current Windows identity, `MultipleInstances=IgnoreNew`/`RestartCount=2`/`RestartInterval=10m`/`ExecutionTimeLimit=1h`/`StartWhenAvailable`/`WakeToRun`, working directory the canonical repo root. Idempotent create-or-update via `Set-ScheduledTask`/`Register-ScheduledTask`; no `Unregister-ScheduledTask`/`Stop-ScheduledTask` call exists in the helper. A post-registration self-check re-reads the task and fails closed if the action count, executable, arguments, working directory, or activation state do not match intent.
**Problem:** No Windows Scheduled Task existed that invokes `Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled` at the correct pre-open boundary.
**Dependencies:** `OFFICIAL-DUAL-MODE-LAUNCHER-01` CLOSED (satisfied — the `-Scheduled -Mode Paper` contract this patch registers against is stable).
**In Scope:** `Register-PaperStartupTask.ps1` and its non-mutating static-guard proof `scripts\windows\tests\test_paper_preopen_scheduler.ps1`; a narrow `docs\runbooks\operator_workflows.md` update (§10.1) documenting the new registration helper. **Out of Scope:** Any Live scheduling (blocked indefinitely behind the full `LIVE-*` critical path); any Rust/Python/GUI source change (none made); activating (`-Enable`) the permanent task; touching, disabling, or unregistering the existing temporary August soak task (`MiniQuantDesk-2026-08-PaperSoak-Startup`), which remains untouched and is still the authoritative unattended-start mechanism during its acceptance window.
**Safety Contract Implemented This Session:** the permanent task was registered but is left **DISABLED** by default (temporary-soak coexistence — two enabled tasks could otherwise invoke the official Paper launcher concurrently). No Live task exists or was created. `-Enable` was never passed. No push, no merge to `main`, no `smoke_logs/` modification.
**Targeted Proof (this session):** `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\tests\test_paper_preopen_scheduler.ps1` — non-mutating, 33/33 static source-guard assertions passed, exit 0. Followed by a real (non-`-Enable`) run of `Register-PaperStartupTask.ps1`, which created the permanent task, passed its own post-registration self-check, and left it `Disabled`; the temporary soak task was re-read afterward and confirmed byte-for-byte unchanged (same action, trigger, principal, `NextRunTime`/`LastRunTime`/`LastTaskResult`).
**Exact CLOSED End State:** Not yet CLOSED. CLOSED requires independent review of this session's diff plus real accepted-scheduler activation (`-Enable`) and at least one genuine unattended (locked-session, signed-in-user) run proven successful — neither has occurred yet.
**Acceptance History:** PENDING / PENDING / PENDING / PENDING.
**Independent Review Finding:** `3d45045a0e809e885b90083e886f659265c2f354` (`ops: register permanent paper preopen startup`): REVIEWED — REPAIR REQUIRED. A transient-enable race was found in the original create/update + activation-state section; see `PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01-REPAIR-01` immediately below.

#### PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01-REPAIR-01 — Register/reconcile the permanent task disabled atomically (transient-enable race repair)

**Status:** IMPLEMENTED_PENDING_REVIEW · **Priority:** P2 · **Paper Impact:** GREEN (additive scheduling only, same task as parent patch) · **Subsystem:** Ops tooling
**Current Source Truth:** `scripts\windows\Register-PaperStartupTask.ps1` now resolves `$existingTask`/`$taskExistedBefore`/`$priorEnabledState`/`$desiredEnabled` before constructing the `ScheduledTaskSettings` object, and builds that object with `-Disable` whenever `$desiredEnabled` is `$false`. `Register-ScheduledTask` (create path) and `Set-ScheduledTask` (reconcile path) both consume this already-correctly-activation-stated settings object. The post-registration/update call to flip activation state now only exists for the `$desiredEnabled=$true` case (`Enable-ScheduledTask`, retained as defense-in-depth); the prior unconditional post-registration `Disable-ScheduledTask` call for the false case has been removed entirely — the definition is already disabled at registration/update time, so there is nothing left to fall back on. The `ScheduledTasks` module-availability check was also moved to before the first `New-ScheduledTask*`/`Get-ScheduledTask` cmdlet use in this same section (previously it ran after `$settings`/`$principal` had already been constructed).
**Problem:** The original `PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01` implementation (`3d45045a`) built a single `ScheduledTaskSettings` object without `-Disable`, called `Register-ScheduledTask`/`Set-ScheduledTask` (which — per Task Scheduler's own default — leaves a brand-new task `Enabled`), and only afterward called `Disable-ScheduledTask` for the `$desiredEnabled=$false` case. For a brand-new task this left a real, non-hypothetical window in which the task existed registered and **Enabled**, with `StartWhenAvailable=true` and `WakeToRun=true` already in effect, before the separate `Disable-ScheduledTask` call landed. Because `StartWhenAvailable=true` means Task Scheduler can fire a missed/available run without an active trigger tick, this transient enabled window was not acceptable for a task whose default/desired state is DISABLED (temporary-soak coexistence).
**Fix:** Resolve `$desiredEnabled` before any settings object is built, and encode the disabled state directly into the `ScheduledTaskSettings` definition passed to `Register-ScheduledTask`/`Set-ScheduledTask`, so there is no register/update-then-disable window — the task is disabled (or enabled) atomically as part of the same definition that creates/reconciles it.
**Dependencies:** `PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01` (`IMPLEMENTED_PENDING_REVIEW`, not yet CLOSED) — this is a narrow repair of that patch's diff, on the same branch/worktree.
**In Scope:** `scripts\windows\Register-PaperStartupTask.ps1` (scheduler-construction / create-update / activation-state section only) and `scripts\windows\tests\test_paper_preopen_scheduler.ps1` (new Section 7 assertions plus this ledger entry). **Out of Scope:** The Rust timing-test hardening mentioned in the original review (explicitly deferred, not started); merging to `main`; activating (`-Enable`) the permanent task; touching, disabling, or unregistering the temporary August soak task (`MiniQuantDesk-2026-08-PaperSoak-Startup`); any Rust/Python/GUI/config change (none made).
**Safety Contract Implemented This Session:** Semantics preserved exactly: (1) new task + no `-Enable` → registered atomically DISABLED (no enabled-then-disabled window); (2) existing disabled task + no `-Enable` → remains disabled throughout reconciliation; (3) existing enabled task + no `-Enable` → remains enabled; (4) any task + explicit `-Enable` → ends enabled; (5) the temporary August soak task untouched; (6) no Live task; (7) no direct runtime/order/data-refresh authority added. `-Enable` was never passed this session. No push, no merge to `main`, no `smoke_logs/` modification.
**Targeted Proof (this session):** `powershell -NoProfile -ExecutionPolicy Bypass -File .\scripts\windows\tests\test_paper_preopen_scheduler.ps1` — non-mutating, all 49 static source-guard assertions passed (the original 37 unchanged, plus 12 new Section 7 assertions proving: `$desiredEnabled` resolved before settings construction; the disabled-path settings branch carries `-Disable` and the enabled-path branch does not; `Register-ScheduledTask`/`Set-ScheduledTask` both consume that same `$settings` object; no `Disable-ScheduledTask` call exists anywhere in the helper; `-Enable`/`Enable-ScheduledTask` still function and are gated correctly; existing-task state-preservation ordering; and the module-availability check now precedes the first `New-ScheduledTask*`/`Get-ScheduledTask` use), exit 0. Followed by a real (non-`-Enable`) run of `Register-PaperStartupTask.ps1` against the already-registered permanent task (pre-repair: `State=Disabled`, `LastRunTime=11/30/1999 12:00:00 AM` (never-run sentinel), `LastTaskResult=267011`, `NextRunTime=8/13/2026 2:00:00 AM`, `NumberOfMissedRuns=0`), which reconciled the task's definition via the `Set-ScheduledTask` update path, passed its own post-registration self-check, and left it `Disabled` (post-repair: identical `State=Disabled`, `LastRunTime`, `LastTaskResult`, `NextRunTime`, `NumberOfMissedRuns` — unchanged, confirming the task did not execute). `Export-ScheduledTask` XML confirmed `<Enabled>false</Enabled>` with action/executable/arguments/trigger/principal/retry-settings/working-directory all unchanged from the parent patch's contract. The temporary soak task was re-read before and after and its exported XML diffed byte-for-byte identical (0 lines changed); its `State=Ready`, `LastRunTime=8/11/2026 2:00:01 AM`, `LastTaskResult=0`, `NextRunTime=8/13/2026 2:00:00 AM`, `NumberOfMissedRuns=1` were unchanged throughout.
**Exact CLOSED End State:** Not yet CLOSED. CLOSED requires independent review of this repair's diff (this repair does not itself change `PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01`'s own CLOSED-blocking requirements — real accepted-scheduler activation and a genuine unattended run are still separately required there).
**Expected Handoff:** Start HEAD `3d45045a0e809e885b90083e886f659265c2f354`; end HEAD = new commit on `integrate-paper-autofresh-launcher` titled `fix: register paper startup task disabled atomically`; not pushed, not merged to `main`.
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
- **Backtesting / research pipeline:** `PROMOTION-WALKFORWARD-GATE-WIRING-01` is `IN PROGRESS / PARTIAL — REPAIR REQUIRED` (corrected 2026-08-21, `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01`, see §5/§24) — the gate mechanism itself is implemented and independently accepted locally (Wave 2, pushed); production wiring exists in an unpushed local commit but independent review found material gaps (cross-candidate authority, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam). Remaining critical path: `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` (new prerequisite, OPEN) → repair remaining gaps in `PROMOTION-WALKFORWARD-GATE-WIRING-01` → push to `origin/main` → `P9 (BKT-ROBUSTNESS-GAUNTLET-01)` → `P10 (RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01)` — see §26 for the full near-term roadmap.
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

**Research Pipeline Complete** means: Backtest Complete, plus: promotion gates fail closed on missing provenance, artifact-lock, and stress-suite evidence; and walk-forward/out-of-sample validation is enforced at the same authoritative gate, not left as an optional upstream step, in the real production path — not merely implemented and tested in isolation. **Current state (corrected 2026-08-21, `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01`, see §24): the OOS/DSR/PBO MECHANISM (`verify_promotion_oos_evidence` / `PromotionInput.oos_evidence`, fails closed on `None`) is implemented, independently accepted, and pushed (Wave 2 = `ACCEPTED_LOCALLY — PUSHED`). PRODUCTION WIRING now has a real caller (`242cb7c3`, local-only, unpushed), but independent review of that commit found material gaps — cross-candidate authority, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam (`PROMOTION-WALKFORWARD-GATE-WIRING-01`, status `IN PROGRESS / PARTIAL — REPAIR REQUIRED`, see §5). A new prerequisite, `PROMOTION-BACKTEST-EVIDENCE-SEAM-01`, plus push, repair, a robustness gauntlet (P9), and final acceptance composition (P10) all remain before this bar can be called met.**

**GUI/Operator Console Complete** means: every screen carrying snapshot data has an explicit `truth_state`; every live-data screen hard-blocks on unproven truth; every operator action route returns and *displays* a structured, actionable response including on failure; and no friendly defaults ever substitute for unproven state. **Current state: met** except the one 409-body-drop defect (`GUI-OPERATOR-ACTION-409-BODY-SURFACE-01`).

**Multi-Symbol Complete** means: concurrent (or deterministic sequential) per-symbol dispatch with failure isolation (one symbol's fault does not halt others); all five documented capital-protection caps are either enforced-by-default or loudly advisory when unset; and the watchlist-exceeds-cap case degrades gracefully rather than failing closed entirely. **Current state: dispatch is wired and live; failure isolation, cap defaults, and truncate-and-surface remain open** (`MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01`, `MULTI-SYMBOL-CAPS-PREFLIGHT-WARNING-01`, `MULTI-SYMBOL-CAP1-TRUNCATE-SURFACE-01`).

**Maintainability Complete** means: no production file so large it materially slows review (a soft target, not a hard line); no duplicated safety-critical logic (e.g., the deadman-alert duplication); CI guards prevent load-bearing tests from silently staying ignored and prevent test-only feature flags from shipping in release builds; and documentation living-docs (README) do not carry stale point-in-time snapshots. **Current state: mostly met**; `state.rs`/`lifecycle.rs` size and the README staleness are the two open items.

---

## 11. Repository-Wide Definition of Done — MiniQuantDesk V4 Full Completion Contract

**CORE V4 COMPLETE** — the equity/ETF paper-trading loop (data → strategy → risk → execution → broker → portfolio → reconcile → operator visibility) runs autonomously, deterministically, fail-closed, restart-safe, and idempotently, with full scenario-test proof and zero known RED defects. **This bar is effectively met today**, pending the one uncommitted fence patch.

**MULTI-ASSET COMPLETE** — Equity and ETF trade fully; Crypto, Options, Futures, and Forex each have a real instrument model, broker adapter, risk policy, execution path, portfolio/P&L support, calendar/session handling, and GUI support, each proven by scenario tests to the same standard as equities. **Not met** — Crypto is data-only; Options/Futures/Forex are enum-stub-only. This is explicitly long-lead, Lane E, post-soak.

**PRODUCT/UI COMPLETE** — every operator-facing screen in the GUI truthfully reflects backend state with no fabricated defaults, and the CLI has parity with the GUI/HTTP surface for at least the operator-safety action set (arm/disarm/halt/clear/status). **Nearly met** — GUI discipline is proven; CLI parity is the open item.

**RESEARCH PIPELINE COMPLETE** — research → backtest → evaluate → promote → deploy is fully proven end-to-end including walk-forward/out-of-sample enforcement at the authoritative gate. **Updated 2026-08-17 (see §24): the gate exists and is implemented, but is not independently accepted (Wave 2 pending review), and the robustness gauntlet (P9) + final acceptance composition (P10) remain open** — not met until both close.

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
        |         (PROMOTION-WALKFORWARD-GATE-WIRING-01 — IN PROGRESS / PARTIAL — REPAIR REQUIRED
        |          2026-08-21, see §5/§24; Wave 2 push dependency is satisfied, but independent
        |          review of the local production-wiring commit found material gaps — remaining
        |          work is PROMOTION-BACKTEST-EVIDENCE-SEAM-01, repair, push, and DB-harness proof)
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

**Historical — `MASTER-LEDGER-CONSOLIDATION-01`'s 2026-08-17 reclassification of item 3 below was itself incorrect and was corrected by `MASTER-LEDGER-TRUTH-REPAIR-01` (2026-08-17); `MASTER-LEDGER-REPO-TRUTH-REFRESH-02` (2026-08-21) then corrected it again to `IMPLEMENTED_PENDING_INDEPENDENT_REVIEW`. This entry is now a historical record of those superseded reclassifications, not current status — see §5 for the authoritative current entry (status updated 2026-08-21, `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01`, to `IN PROGRESS / PARTIAL — REPAIR REQUIRED`, not `READY`, not `IMPLEMENTED_PENDING_INDEPENDENT_REVIEW`):**

3. **`PROMOTION-WALKFORWARD-GATE-WIRING-01`** (originally `READY`, Lane B) — `MASTER-LEDGER-CONSOLIDATION-01` (2026-08-17) incorrectly marked this `CLOSED — SUPERSEDED`, reasoning that the P7A-P7C research-promotion program's accepted DSR/PBO registry-anchored OOS-evidence mechanism (`verify_promotion_oos_evidence`) fully achieved this patch's acceptance criteria. That mechanism exists and (as of 2026-08-21) is confirmed pushed to `origin/main`, but at the time P7C-REPAIR-04's own record confirmed it had **no production call site** — the daemon promotion route was never wired to it. `MASTER-LEDGER-TRUTH-REPAIR-01` (same day) corrected the status back to `READY`. A production-wiring commit (`242cb7c3`) now exists locally, and production call wiring does exist — but independent review of that commit (2026-08-21) found material gaps (cross-candidate authority, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam), so the old "no production caller exists" framing no longer applies and the entry is instead `IN PROGRESS / PARTIAL — REPAIR REQUIRED` (see §5) — still not `CLOSED`.

---

## 14. Next 10 Patches

| Order | Patch | Lane | Impact | Priority | Why Now | Depends On |
|---|---|---|---|---|---|---|
| 1 | `GUI-OPERATOR-ACTION-409-BODY-SURFACE-01` | B | GREEN | P1 | Real operator-safety defect, one file, no dependencies. | NONE |
| 2 | `CLI-DAEMON-CONTROL-PASSTHROUGH-01` | B | GREEN | P1 | Closes the incident-response CLI/HTTP parity gap; pure passthrough, low risk. | NONE |
| 3 | `PROMOTION-WALKFORWARD-GATE-WIRING-01` | B | GREEN | P1 | Corrected to `IN PROGRESS / PARTIAL — REPAIR REQUIRED` 2026-08-21 (see §5/§24) — Wave 2 is pushed (`b80749bd` confirmed ancestor of `origin/main`), and an unpushed local commit (`242cb7c3`) wires the accepted P7C OOS-evidence mechanism into the production promotion route (unit tests 11/11 pass), but independent review of that commit has since found material gaps (cross-candidate authority, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam). Remaining: `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` (new prerequisite), repair, push, DB-harness proof. Unblocks P9 only once CLOSED. | `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` (new, OPEN) |
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

## 18. `PAPER-SOAK-PROVIDER-SCOPED-INGEST-TEST-REPAIR-01`

**Status:** `IMPLEMENTED_PENDING_REVIEW`
**Branch:** `integrate-paper-autofresh-launcher` (worktree `MiniQuantDeskV4-integration`)
**Commit:** `66761a89c39a43cd62c279fa53a214cb0933da4b` — `test: align ingest expectations with provider-scoped registry`

**Context:** Final Gate G of the paper-launcher integration mission exposed six provider-sync test failures asserting `symbols_count == 88` where the actual value was `87`.

**Classification:** `STALE_TEST` — not a production defect.

**Root cause:** The canonical registry (`config/instruments/equities.json`) contains 88 enabled equities, but AAPL is intentionally scoped `provider=alpaca`, `timeframes=["5m"]` only (established by `MARKET-DATA-PROVIDER-PROVENANCE-01` / `-REPAIR-01`, see memory `project_market_data_provider_provenance_01.md`). The TwelveData/equity/1D provider-scoped universe therefore correctly contains 87 symbols (88 minus AAPL); the Alpaca/equity/5m provider-scoped universe correctly contains AAPL. The six failing tests were asserting the old whole-registry count against a resolver that has correctly been provider-scoped since the provenance repair landed — the tests were never updated to match.

**Production code:** `core-rs/crates/mqk-daemon/src/routes/ingest.rs::resolve_provider_scoped_equities` was reviewed and **not changed**. Its provider/timeframe-scoping behavior is correct and intentional.

**Repair:** Six stale provider-scoped test expectations (`pd_02`, `pd_10`, `pd_12`, `pd_13`, `db_01`, `db_02`) in `core-rs/crates/mqk-daemon/tests/scenario_ingest_jobs_data_ingest_daemon_01.rs` were repaired to derive their expected count from an independent registry-filter helper (`expected_registry_symbols_for_provider_timeframe`) rather than hardcoding `88`. Whole-registry `TE-*` expectations (`te_01` etc.) remain `88` — unchanged, since those correctly assert the full enabled-registry count, not a provider-scoped subset.

**New regression proof:** `canonical_registry_provider_scoping_excludes_alpaca_aapl_from_twelvedata_1d` (`PROV-SCOPE-01`) added — asserts registry enabled-count is 88, AAPL is `provider=alpaca` with `5m` and not `1D`, TwelveData/1D scoped set is 87 and excludes AAPL, Alpaca/5m scoped set is exactly `[AAPL]`. This exists specifically to catch any future regression back to whole-registry provider-sync behavior, which would silently destroy provider provenance.

**Validation (re-verified this session against committed HEAD, not carried over from prior claims):**
- `cargo test -p mqk-daemon --test scenario_ingest_jobs_data_ingest_daemon_01 -- --test-threads=1`: **65 passed / 0 failed / 0 ignored**.
- `cargo check -p mqk-daemon --tests`: **PASS** (only a pre-existing, unrelated `sqlx-postgres` future-incompatibility warning).
- Full daemon suite progressed further after this repair and exposed a distinct `E14a` halt-note failure (tracked separately below).
- No `main`/config/scheduler/Live/order-path changes. `smoke_logs/` untouched.

---

## 19. `PAPER-SOAK-WS-GAP-HALT-NOTE-TRUTH-REPAIR-01`

**Status:** `IMPLEMENTED_PENDING_REVIEW`
**Branch:** `integrate-paper-autofresh-launcher` (worktree `MiniQuantDeskV4-integration`)

**Context:** After the ingest repair above, the full `mqk-daemon` test binary progressed further and exposed `ptauto01b_e14a_gap_detected_halts_real_execution_loop` (in `scenario_paper_alpaca_proof_bundle_brk00r06.rs`) failing with actual exit note `"execution loop halted: Alpaca WS continuity gap detected (halt_outcome=None)"` against an old expectation of `"execution loop halted: Alpaca WS continuity gap detected"` (no suffix).

**Classification:** `STALE_TEST` — not a production defect.

**Root cause:** Commit `0a019b8b` (`fix: fence daemon supervisor safety halts`, `PRE-SOAK-DAEMON-SUPERVISOR-HALT-FENCE-CLOSURE-01`, already `CLOSED` per prior ledger record) deliberately added a `(halt_outcome={halt_outcome:?})` suffix to every supervisor safety-halt exit note in `core-rs/crates/mqk-daemon/src/state/loop_runner.rs`, including the PT-AUTO-01 WS-continuity-gap branch (`loop_runner.rs:559-563`). This is an intentional truthfulness/observability contract: the exit note must always surface whether the durable halt was `Halted`, `AlreadyHalted`, `Superseded`, `PersistenceFailure`, or (when no DB pool is present) `None` — never silently omitted. The `ptauto01b_e14a_*` test's harness helper `run_loop_one_tick_for_test` (`state.rs:2284-2359`) is documented in-source (`state.rs:2334-2336`) to use a `db = None` seam for all `new_for_test_with_*` AppState constructors, so `persist_execution_loop_safety_halt` is never invoked and `halt_outcome` is correctly `None` in this seam. The test's hardcoded expected string simply predated the `0a019b8b` observability change and was never updated.

**Production code:** **Not changed.** `loop_runner.rs`'s halt-outcome-suffix behavior is the accepted, already-closed fence contract — weakening or removing it to satisfy the old string would regress an accepted safety/observability invariant.

**Repair (test-only):** `core-rs/crates/mqk-daemon/tests/scenario_paper_alpaca_proof_bundle_brk00r06.rs`, `ptauto01b_e14a_gap_detected_halts_real_execution_loop` — updated the exact-match assertion to the new canonical string `"execution loop halted: Alpaca WS continuity gap detected (halt_outcome=None)"`, with an inline comment explaining why `None` is correct for this no-DB seam (not a loosened/prefix-only assertion — still an exact match on the full canonical PT-AUTO-01 halt reason plus its expected halt outcome for this seam). `ptauto01b_e14b_*` (Live continuity, PT-AUTO-01 must NOT fire) was inspected and required no change — its `assert_ne!` never matched the WS-gap string family before or after this repair.

**Safety invariants preserved (unchanged by this repair):**
- `GapDetected` still causes the real execution loop to self-halt via PT-AUTO-01.
- `integrity.disarmed` and `integrity.halted` both become `true` on this path.
- The loop still exits before reaching economic dispatch.
- No weaker safety behavior was introduced; no production code touched.

**Validation:**
- Targeted: `cargo test -p mqk-daemon --test scenario_paper_alpaca_proof_bundle_brk00r06 ptauto01b_e14a_gap_detected_halts_real_execution_loop -- --exact --nocapture`: **1 passed / 0 failed**.
- Full binary: `cargo test -p mqk-daemon --test scenario_paper_alpaca_proof_bundle_brk00r06 -- --test-threads=1 --nocapture`: **31 passed / 0 failed / 0 ignored**.
- `cargo check -p mqk-daemon --tests`: **PASS** (same pre-existing unrelated sqlx future-incompat warning only).
- `git diff --check`: clean.

**Commit:** test-only, separate from the ingest repair (not amended into it).

---

## 20. `PAPER-SOAK-CLIPPY-RETRY-TEST-LINT-REPAIR-01`

**Status:** `IMPLEMENTED_PENDING_REVIEW`
**Branch:** `integrate-paper-autofresh-launcher` (worktree `MiniQuantDeskV4-integration`)

**Context:** Gate I (`cargo clippy --workspace --all-targets -- -D warnings`) failed with exit 101 on two lints in `core-rs/crates/mqk-daemon/tests/scenario_autonomous_daily_operator_retry_01.rs`, blocking the workspace clippy gate. `cargo check --workspace` (no `-D warnings`) was already clean, and the full `mqk-daemon`/`mqk-cli` test suites had already passed — this defect was purely a clippy-strictness compile blocker, unrelated to any change made earlier in this mission.

**Classification:** `PRODUCT_DEFECT` (pre-existing, test-only, non-behavioral) — introduced whole in commit `035cabf0` (`fix: add safe autonomous daily retry`), not by this mission's earlier repairs.

**Findings and repairs:**
1. `dynamic_session_now()` (line ~182): `clippy::while_let_loop` — a `loop { match ... { pat => ..., _ => break } }` that clippy can prove is exactly a `while let` loop. Rewritten to `while let chrono::Weekday::Sat | chrono::Weekday::Sun = candidate_date.weekday() { candidate_date += ChronoDuration::days(1); }` — behaviorally identical, mechanical simplification only.
2. `real_transition()` (line ~285): `clippy::too_many_arguments` (9/7). The sibling test-fixture builder `seed_operation_row` in the same file (same original commit, same 9-parameter shape) already carries `#[allow(clippy::too_many_arguments)]` — `real_transition` was simply missed. Added the identical, narrow, function-scoped `#[allow(clippy::too_many_arguments)]` to match the file's own established precedent for named-parameter test-fixture helpers. This is not a blanket/crate-level allow; each of these test builder functions takes many named DB-fixture fields where a struct wrapper would not improve clarity over the existing call sites.

**Production code:** Not touched — both findings are in a test file only.

**Validation:**
- `cargo clippy -p mqk-daemon --all-targets -- -D warnings`: **PASS** (0 errors; same pre-existing unrelated sqlx future-incompat warning only).
- Targeted DB-backed proof (test module is `#[ignore]`-gated per its own doc, run with `--include-ignored` and the real DB per the mission's ignore-gated test rule): `cargo test -p mqk-daemon --test scenario_autonomous_daily_operator_retry_01 -- --test-threads=1 --include-ignored` against `postgresql://127.0.0.1:5434/mqk_test`: **16 passed / 0 failed / 0 ignored**.
- `cargo check -p mqk-daemon --tests`: **PASS**.
- `git diff --check`: clean.

**Commit:** test-only, narrow, separate from all prior repairs in this mission.

---

## 21. Mission Closure — `PAPER-SOAK-FINISH-LINE-RECOVERY-MERGE-CUTOVER-01`

**Mission date:** 2026-08-12
**Scope:** finish the paper-launcher integration, exhaust deterministic repo/test blockers one root cause at a time, fast-forward merge to `main`, cut the permanent scheduler over from the temporary August task, and clean up only proven-merged branches — without touching Live, without submitting orders, without manually starting the economic runtime.

**Focused repairs landed this mission (three separate commits, each a single root cause):**
1. `c45aa4c2` — `PAPER-SOAK-PROVIDER-SCOPED-INGEST-TEST-REPAIR-01` (§18): `STALE_TEST`, six provider-scoped ingest expectations updated from hardcoded `88` to a registry-derived count; production `resolve_provider_scoped_equities` unchanged. 65/65 passed.
2. `83e0707d` — `PAPER-SOAK-WS-GAP-HALT-NOTE-TRUTH-REPAIR-01` (§19): `STALE_TEST`, E14a's hardcoded exit-note string updated to match the `(halt_outcome=...)` suffix intentionally added by the already-closed `PRE-SOAK-DAEMON-SUPERVISOR-HALT-FENCE-CLOSURE-01` fence; production `loop_runner.rs` unchanged. 31/31 passed (full proof bundle).
3. `e63a3170` — `PAPER-SOAK-CLIPPY-RETRY-TEST-LINT-REPAIR-01` (§20): `PRODUCT_DEFECT` (pre-existing, test-only), two clippy lints in `scenario_autonomous_daily_operator_retry_01.rs` (`while_let_loop` mechanical rewrite; a missed `#[allow(clippy::too_many_arguments)]` matching the sibling fixture builder in the same file). 16/16 passed with `--include-ignored` against the real DB.

No additional deterministic blockers were found beyond these three — the full `mqk-daemon` and `mqk-cli` regressions were each green on the first real-DB run after the third repair.

**Final validation tallies (this session, against `postgresql://127.0.0.1:5434/mqk_test`):**
- `mqk-daemon` full suite: **3271 passed / 0 failed / 463 ignored**.
- `mqk-cli` full suite: **135 passed / 0 failed / 9 ignored**.
- `cargo check --workspace`: **PASS**.
- `cargo clippy --workspace --all-targets -- -D warnings`: **PASS** (0 errors after repair #3).
- `git diff --check`: clean at every commit boundary.

**Config conflict (surfaced and resolved by explicit operator decision, not assumed):** the main worktree's uncommitted `config/instruments/equities.json` carried a `TEMPORARY same-day override (2026-08-11)` that re-added AAPL `1D` alongside `5m`, past its own stated revert deadline. This conflicted with the integration branch's permanent, reasoned `MARKET-DATA-PROVIDER-PROVENANCE-01-REPAIR-01` decision (`alpaca`+`1D` is `DailyBarTimestampConvention::Unverified`). Per the mission's explicit STOP-on-any-diff instruction, this was surfaced to the operator rather than resolved unilaterally; the operator chose to keep the integration branch's `5m`-only permanent decision. Working copy backed up to `%TEMP%\MiniQuantDeskV4-premerge-20260812\equities.json` before `git restore --source=HEAD` was run.

**Merge:** `git merge --ff-only origin/integrate-paper-autofresh-launcher` from `main` — **fast-forward, no merge commit**. `54082a44` → `e63a3170`.

**CheckOnly proofs (main, post-merge, read-only):**
- Paper: prerequisites OK, daemon not started, no mutation.
- Live: **`LIVE START REFUSED`** (exit 5) — `broker configuration`, `account truth`, `reconciliation`, `risk`, `trust chain` all `BLOCKED`/`BLOCKED_NOT_IMPLEMENTED`, `live_trust_complete=FALSE`. This is the expected fail-closed gate, not a defect. No live broker orders enabled, no live runtime started, no live DB mutated.

**Push proof:** `integrate-paper-autofresh-launcher` pushed (`096f6826..e63a3170`) and verified equal to local before the merge; `main` pushed (`54082a44..e63a3170`) and verified equal to local (`origin/main` = `e63a3170`) after.

**Scheduler cutover:**
- Permanent task `\MiniQuantDesk\MiniQuantDesk-Paper-Preopen-Startup` rehomed from the integration worktree to `C:\Users\Zacha\Desktop\MiniQuantDeskV4` via `Register-PaperStartupTask.ps1`, registered `DISABLED` first; verified zero `MiniQuantDeskV4-integration` references anywhere in the exported task XML; action/working-directory/trigger (Mon–Fri 02:00 local)/settings (`IgnoreNew`, `RestartCount=2`, `RestartInterval=10m`, `ExecutionTimeLimit=1h`, `StartWhenAvailable=true`, `WakeToRun=true`)/principal (current user, Interactive, Limited) all confirmed correct.
- Temporary task `MiniQuantDesk-2026-08-PaperSoak-Startup` state recorded before cutover (`Ready`, last run `2026-08-11 02:00:01` result `0`, 1 missed run, already pointed at `main`, not deleted).
- Cutover: temporary task disabled (`Disable-ScheduledTask`) → permanent task enabled (`Register-PaperStartupTask.ps1 -Enable`) → verified exactly one of the two tasks (`Ready`) at a time throughout. Neither task was manually started.
- Post-cutover: permanent = `Ready`, `NextRunTime = 2026-08-13 02:00:00` (Thursday); temporary = `Disabled`, still registered (rollback path retained, not deleted).

**Branch cleanup:** ancestry proven (`git merge-base --is-ancestor`, local and remote) for all five candidates before any deletion: `fix-market-data-provider-provenance`, `fix-autonomous-daily-operator-retry`, `fix-market-data-autofresh-required-universe`, `ops-official-launcher`, `integrate-paper-autofresh-launcher`. Their five corresponding worktrees (`MiniQuantDeskV4-data`, `-retry`, `-autofresh`, `-ops`, `-integration`) were each confirmed clean (or only protected `smoke_logs/`), then detached to the final `main` SHA (`git switch --detach e63a3170`) rather than removed — `smoke_logs/` in `-ops` and `-integration` preserved and reconfirmed present after detach. Only then were the five branches deleted, `git branch -d` (safe/non-force) locally and `git push origin --delete` remotely, followed by `git fetch --prune`. Retained: `main`, `codex/audit-last-two-patches-and-fix-stuck-state`, `review/ai-ml-local-lab-foundation-01`, `review/bundle4-final-coherence`, `review/premarket-script-guard-truth-repair` — all confirmed still present after prune.

**Final state:**
- `main` = `origin/main` = `e63a31706954f21fa7b5ed48d018576e15bb39d0`.
- `git status --short` in the main worktree: `?? smoke_logs/` only.
- Exactly one Paper-startup scheduled task enabled (the permanent one).
- No Live runtime started, no Live routing exercised, no manual `Start-ScheduledTask`, no manual economic Paper runtime start, no orders submitted, no `branch -D`, no `git clean`, no `reset --hard`, no forced worktree removal, `smoke_logs/` not deleted anywhere, temporary task not deleted.

**Status distinctions (per this repo's honest-status vocabulary):**
- Code / merge / scheduler cutover: **CLOSED** — all proof above holds against committed HEAD.
- Unattended permanent-scheduler proof: **CLOSED, result = UNATTENDED_FAIL** — resolved below (§22): the real 2026-08-13 02:00 run fired and was killed by Task Scheduler's `ExecutionTimeLimit` after an ~8-hour hang; the configured retry never fired.
- Paper soak day result: **CLOSED, result = BLOCKED** — resolved below (§22): zero bar dispatch, zero strategy evaluation, zero orders/fills on 2026-08-13; not a valid `NO_SIGNAL` (no genuine strategy invocation occurred).

---

## 22. Mission — `PAPER-SOAK-FRIDAY-RECOVERY-LAUNCHER-HARDENING-AND-MONITOR-01`

**Mission date:** 2026-08-13 (Thursday)
**Scope:** honestly classify Thursday's unattended-scheduler failure, repair every proven root cause before Friday 2026-08-14 02:00 HST's permanent scheduled run, restore AAPL/5m market-data continuity through canonical Paper-only mechanisms, then monitor Friday's unattended startup and market session. One root cause per commit.

### Thursday 2026-08-13 failure — honest classification

- **Scheduler result: `UNATTENDED_FAIL`.** Permanent task `\MiniQuantDesk\MiniQuantDesk-Paper-Preopen-Startup` fired at `02:00:01` (`LastRunTime`), was terminated by Task Scheduler at `LastTaskResult=267014` (`SCHED_S_TASK_TERMINATED`, i.e. killed at the 1-hour `ExecutionTimeLimit`), and the configured `RestartCount=2`/`RestartInterval=10m` retry never fired (`NumberOfMissedRuns=0` — no second attempt was ever recorded). A second, manually-invoked official-launcher attempt separately hung for ~8 hours before intervention.
- **Soak result: `BLOCKED`.** `bar_tick_dispatch_count=0`, `strategy_evaluation_count=0`, zero new Paper orders/fills. The pre-existing AAPL x3 position remained reconciled; no Live activity occurred. This is not a valid `NO_SIGNAL` — no genuine strategy invocation occurred at all.
- **Contributing fact, not itself the root cause:** the daemon binary in use predated `MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01`'s retry functionality (built ~2026-08-11 02:06; that functionality landed later the same day) — `Ensure-DaemonBinary` had no way to detect this and reused the stale binary.

### Correction to the interior-gap diagnosis

Thursday's ~11:19 HST direct `required-universe/start` diagnostic ran **after** regular close + grace, so `provider_api_calls_made_this_cycle=0` at that check is expected controller behavior (§21's `SESSION_CLOSE_POLL_BUFFER_SECS` gate correctly refuses both the historical-bootstrap and latest-bar-poll refresh paths once `now > session_close_utc + 15min`), **not evidence that `interior_gap` is non-refreshable**. Verified directly in source (`required_market_data_autofresh.rs`): `REFRESHABLE_READINESS_REASONS` includes `market_data_missing`, `insufficient_history`, `interior_gap`, `expected_latest_bar_missing`; `needs_historical_bootstrap()` returns `true` for `interior_gap` identically to the other three. The controller was never rewritten or weakened on this mission — no defect was found in it.

### Root cause 1 — stale daemon binary accepted as current

`Ensure-DaemonBinary` (`scripts\windows\Launch-VeritasLedger.ps1`) previously reused any existing `mqk-daemon.exe` with zero proof it matched the current `core-rs` source. Fixed with a deterministic build-provenance sidecar: the `core-rs` git tree SHA (`git rev-parse HEAD:core-rs`, suffixed `-dirty` for an uncommitted tree) is written to `core-rs\target\release\mqk-daemon.build-tree.txt` after every successful build; reuse requires an exact match, and any failure to resolve git identity fails closed to a rebuild. `Launch-VeritasLedger.ps1`'s `MAIN DISPATCH` is now dot-source-guarded so the new functions are directly testable.
**Commit:** `d5de48b4` — `fix: verify daemon binary matches rust workspace`. **Proof:** 26/26 new functional assertions (`tests\script_guards\test_launch_veritas_ledger.ps1`, LVL18–26) against a disposable git fixture — matching identity reuses, missing/mismatched provenance rebuilds, a successful build writes provenance, `-ForceRebuild` always rebuilds.

### Root cause 2 — scheduled/headless bootstrap could hang indefinitely

Empirically reproduced: `Start-MiniQuantDesk.ps1` invoked `Launch-VeritasLedger.ps1` as `& powershell.exe @lvlArgs | Out-Host` with no `-NonInteractive`; that script's outer `catch` called `Read-Host` unconditionally on any failure. An isolated probe confirmed `Read-Host` blocks indefinitely without `-NonInteractive` (killed after 15s) and, given this script's own `$ErrorActionPreference='Stop'`, throws immediately and exits 1 with `-NonInteractive`. Fixed in two layers: (1) `Launch-VeritasLedger.ps1`'s outer catch now skips `Read-Host` and exits 1 immediately whenever `-SkipGui` is present (the headless/scheduled contract — `-Scheduled` always implies it); (2) `Start-MiniQuantDesk.ps1` replaces the raw pipeline with `Invoke-BoundedChildScript`, driving the child via `System.Diagnostics.ProcessStartInfo` directly (`Start-Process -PassThru` was found to have an unreliable `ExitCode` readback when combined with redirection — confirmed empirically, not assumed), always adding `-NonInteractive`, and enforcing an internal `-BootstrapTimeoutSeconds` (default 2400s) well under Task Scheduler's 1-hour `ExecutionTimeLimit` so a genuinely stuck child still returns a real nonzero exit for Task Scheduler's retry to act on. `Process.Kill(bool)` (kill entire tree) does not exist under Windows PowerShell 5.1 (the runtime Task Scheduler actually launches, confirmed via `$PSVersionTable`); the plain `Kill()` overload is used instead.
**Commit:** `20491a33` — `fix: make scheduled daemon bootstrap noninteractive and bounded`. **Proof:** isolated Read-Host/-NonInteractive probes; a bounded real run of the fixed launcher against a forced port-8899 failure completed in **5.3s, exit code 1**, no prompt — versus the prior unbounded hang. New static + functional proofs in `test_official_dual_mode_launcher.ps1` (`SCHEDULED-HEADLESS-BOOTSTRAP-01` section): bounded timeout kills only the wrapper (never the independently-started daemon), `-NonInteractive` converts a would-be `Read-Host` hang into a fast nonzero exit, fast success/failure exit codes propagate correctly.

### Prior-day operation isolation (coverage gap, not a defect)

Investigated whether Thursday's terminal `manual_intervention_required` (`reason=interior_gap`) record could block, gate, or leak into Friday's operation. Confirmed by design: the daily slot key `(market_date, deployment_mode, adapter_id)` is a DB unique constraint (migration `0048`), and `operation_id` is deterministically derived including `market_date`, so `create_or_recover_autonomous_daily_operation` for a new date always inserts an independent row. The one query that isn't `market_date`-scoped (`fetch_relevant_open_autonomous_daily_operation`) is only reachable via resolution-failure/nontrading-day fallbacks and structurally excludes a `manual_intervention_required` row with no `run_id`. No existing test drove this exact scenario end to end.
**Commit:** `5190a274` — `test: prove prior-day manual_intervention_required does not block next day`. **Proof:** new test walks day 1 through the real `preparing_data -> manual_intervention_required` path and leaves it terminal, then proves day 2's `create_or_recover` succeeds independently with its own `operation_id` and a fresh `awaiting_preopen` state, while day 1's history remains intact. 27/27 passed (full file).

### interior_gap bootstrap proof (coverage gap, not a defect)

`scenario_market_data_autofresh_required_universe_01.rs`'s only positive end-to-end proof exercised `market_data_missing` (empty DB), never `interior_gap` specifically, even though `needs_historical_bootstrap()` treats all three refreshable reasons identically in code.
**Commit:** `0cea94a4` — `test: prove interior_gap alone drives bounded historical bootstrap`. **Proof:** new test seeds a full 5-bar window, punches a hole in the middle bar directly against the DB (matching the shape of Thursday's real gap), confirms via a read-only `dry_run` cycle that the fixture genuinely produces `interior_gap` in the raw blockers list (co-occurring with `insufficient_history`, which `ddr_56_history_insufficient_continuity_state_never_ok` already documents as the realistic, expected pairing), then proves the controller performs exactly one more bounded historical provider call that repairs it and settles `ready`, with no further repair on subsequent cycles. 19/19 passed (full file, including the new test).

### Pre-Friday AAPL/5m historical repair — honest finding, no repair performed

Investigated the canonical Alpaca historical-ingest surfaces available in the current repo: `mqk-cli md sync-provider`/`ingest-provider` are both TwelveData-only (`--source <SOURCE> (only: twelvedata)`) — explicitly forbidden for AAPL by this mission. `Refresh-IntradayMarketData.ps1`, referenced in code comments as "untouched, remains available as a manual/compatibility operator tool," **no longer exists in the repo** (removed in an earlier cleanup; the comment is now stale). The daemon's `/api/v1/market-data/required-universe/start` route — the only real Alpaca historical-ingest path — is deliberately gated by `SESSION_CLOSE_POLL_BUFFER_SECS` (15 min): `attempt_bounded_historical_bootstrap` never fires once `now > session_close_utc + 15min`, confirmed empirically via a real dry-run against the live daemon (`provider_api_calls_made_this_cycle: 0`, `overall_state: "blocked"`, well past Thursday's 20:00 UTC close). Per the mission's own instruction for this situation, this sub-step was stopped rather than bypassing a deliberately-designed fail-closed gate with ad-hoc code. **Operator decision (confirmed): let Friday's real pre-open self-heal it live** — Phase 6A's own end-to-end proof (above) already demonstrates the controller correctly performs exactly one bounded historical bootstrap and repairs `interior_gap` when called within a valid session window, and this mission's Phases 3–4 now guarantee Friday's launcher will actually reach that call instead of hanging. **No bars were fabricated. TwelveData was not used for AAPL. The legacy 1D task was not re-enabled. No alternate market-data authority was introduced.**

Real dry-run evidence captured against the live daemon (2026-08-13T22:13 UTC): required universe resolves to exactly `AAPL/5m/alpaca` (`symbol_source=env_strategy_symbol`); readiness reports `blockers: ["interior_gap", "expected_latest_bar_missing"]`, `freshness_state: "interior_gap"`, `latest_completed_bar_ts: "2026-08-11T18:10:00+00:00"` — confirming the gap's exact shape and start point independently of the mission brief's own statement of it.

### Rehearsal (after-hours manual, not the unattended proof)

`Launch-VeritasLedger.ps1 -Mode Observe -SkipGui`: daemon started, verified `paper+alpaca` identity, `live_routing_enabled` false, wrapper returned in seconds, daemon remained alive headless, no runtime start, no orders — then stopped cleanly.

`Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled` (manual after-hours rehearsal): **returned in 9.13s, exit code 3 (`ExitDataReadiness`)** — DB prerequisites passed, `Launch-VeritasLedger.ps1` bootstrap returned (paper safety guard confirmed: `live_routing_enabled=false, daemon_mode=paper, adapter_id=alpaca`), the required-universe call executed and correctly reported `REQUIRED_UNIVERSE_SCHEDULER_BLOCKED` with the honest `interior_gap`/`expected_latest_bar_missing` reason, and the launcher correctly refused to proceed toward reconcile/arm. This is the mission's documented acceptable outcome B (fails closed quickly with a truthful reason) — no hang, no Live, no runtime start, no orders. Daemon left running headless per contract; stopped cleanly post-rehearsal.

### Validation tallies (this session)

- `git diff --check` (full mission diff from `main`): clean.
- `cargo check -p mqk-daemon`: **PASS**. `cargo clippy -p mqk-daemon --all-targets -- -D warnings`: **PASS** (0 errors; same pre-existing unrelated `sqlx-postgres` future-incompat warning only). Same for `mqk-db`.
- `tests\script_guards\test_launch_veritas_ledger.ps1`: **26/26 passed** (17 pre-existing + 9 new LVL18–26).
- `scripts\windows\tests\test_official_dual_mode_launcher.ps1`: **ALL PROOFS HELD (0 violations)** — includes the new `SCHEDULED-HEADLESS-BOOTSTRAP-01` static + functional proofs.
- `scripts\windows\tests\test_paper_preopen_scheduler.ps1`: **all proofs held, 0 violations**.
- `scenario_market_data_autofresh_required_universe_01` (real DB, `--include-ignored --test-threads=1`): **18/19 passed** — the 1 failure (`stop_start_generation_race_old_cycle_cannot_overwrite_new_owner`, a 10s-budget concurrency race) reproduces **identically against the unmodified pre-mission file** (confirmed via `git stash`), i.e. pre-existing host-timing flakiness, not a regression introduced by this mission.
- `scenario_daily_data_readiness_01`: **66/66 passed**. `scenario_autonomous_daily_operator_retry_01`: **16/16 passed**. `scenario_autonomous_daily_operation_store_01` (mqk-db): **27/27 passed**.

**Note on the local test DB:** `mqk-test-postgres` (port 5434) was found with `migration 6 was previously applied but has been modified` — a pre-existing environment-drift defect blocking all DB-backed tests, unrelated to this mission's code changes. Recreated fresh (disposable named-volume container, no bind mounts, no production/paper data involved) so current migrations apply cleanly; all tallies above are against the fresh container.

### Status at end of Phases 1–8

- Root causes 1 and 2: **CLOSED** — code committed, tests committed, tests passing (see commits above).
- Prior-day isolation and interior_gap bootstrap coverage gaps: **CLOSED** — tests committed and passing; no production defect found, none fixed.
- Pre-Friday AAPL/5m repair: **PARKED by explicit operator decision** — deferred to Friday's live pre-open self-heal, not performed manually.
- Merge to `main`, push, Friday host preparation, and Friday's actual unattended monitoring: **OPEN** — see below as this mission continues.

---

## 23. Mission — `PAPER-AUTONOMOUS-STARTUP-THREE-DEFECT-CLOSURE-01`

**Mission date:** 2026-08-14 (Friday, after-hours repair; regular session already closed)
**Scope:** review, and where needed narrowly repair, three candidate fix commits developed live during Friday's incident, closing three independent defects in the autonomous Paper startup/recovery path. Code/test proof only — no Live, no Paper orders, no scheduler/provider-authority changes, no manual state edits.

### Friday 2026-08-14 result — honest classification

**`BLOCKED_STRATEGY_NOT_INVOKED`. Does not count toward the 10–20 session soak.** The autonomous coordinator attempted canonical runtime start at approximately T+45s after regular-session open. No current-session 5-minute bar could physically exist yet (earliest possible completion T+300s); `daily_data_readiness` correctly fell back to the previous session's tail and reported ready, but the independent legacy `market_data_freshness` gate inside `start_execution_runtime` re-checked wall-clock bar age against a flat 900s threshold, saw the (necessarily prior-session) latest bar as stale, and refused with `runtime.start_refused.market_data_not_fresh` — a fault class `autonomous_retry_policy` durably classifies `ManualInterventionRequired`, parking the day. A second, independent gap then blocked the sanctioned recovery path: `POST /api/v1/autonomous/daily-operation/retry`'s recoverable-reason set did not recognize this legacy fault class at all (keyed only to newer `daily_data_readiness` reason codes). A third, related gap was found during recovery: the retry route's activity-safety check reused `autonomous_daily_coverage_authority::check_operation_pristine` — a stricter, differently-scoped predicate that treats a mere prestart bar *observation* (`bars_observed != 0`, left by the completed-bar driver's `PrepareDataOnly` mode) as disqualifying activity, which would have refused even a genuinely pristine-pre-start operation.

### Candidate-commit audit

Three commits already existed on `main` (developed live during the incident, HEAD `54daa588` = `origin/main` at mission start, working tree clean apart from untracked `smoke_logs/`). All three were independently reviewed against this mission's design requirements (full diff read, cross-checked against the actual state-machine/gate-ordering code they touch, not assumed correct from their own commit messages) and proven via real, DB-backed scenario tests run against the local `mqk-test-postgres` (`:5434`) container — **all retained unchanged**, no defect found in any of the three:

- **`0e6ea651`** — `market_data_freshness::is_awaiting_first_session_bar` + `state::lifecycle::readiness_blocked_only_by_pending_first_session_bar`: a narrow, timeframe-aware carve-out at the exact point `start_execution_runtime` would return `market_data_not_fresh` — only when every blocking symbol is blocked solely by `"stale"` (never `"missing"`/`"insufficient"`) and structurally cannot yet have a fresher bar (reusing `daily_data_readiness`'s own grace/timeframe helpers). Returns the new `runtime.start_refused.latest_completed_bar_pending` fault class instead, which `autonomous_retry_policy` classifies `WaitForCondition` via the coordinator's pre-existing `LatestCompletedBarPending` reason (not a new duplicate concept). Every other case is byte-for-byte unchanged.
- **`e109c9db`** — adds exactly one exact-match string, `"runtime.start_refused.market_data_not_fresh"`, to `RECOVERABLE_PREFLIGHT_REASON_CODES`. The sibling `latest_completed_bar_pending` fault class is deliberately excluded (it classifies `WaitForCondition`, never durably reaches `manual_intervention_required`, so including it would be unaudited, unused breadth). No prefix/substring matching; every other independent safety check in the route (pristine/activity history, session window, identity match, a fresh re-run of canonical `daily_data_readiness` before any mutation) remains unconditionally authoritative.
- **`54daa588`** — adds `check_prestart_retry_safety`, a separate, retry-route-owned predicate answering a different question than `check_operation_pristine` ("has genuine runtime/economic activity occurred?" vs. "may a coverage anchor be bound?"). Reuses the same two DB-backed activity queries (`count_autonomous_daily_bar_dispatch_claims`, `fetch_and_validate_autonomous_daily_operation_run_lineage`) and the same `run_id`/`started_at_utc`/`bars_dispatched`/`last_dispatched_bar_ts` field checks, but never inspects `bars_observed`/`last_completed_bar_ts`. `check_operation_pristine` itself is untouched — a same-session inline test proves it still reports `HasActivity` for the identical bars-observed-only fixture the new predicate proves `Safe`. Correctness of using `run_id.is_some()` as a sound proxy for "could strategy/dispatch evidence exist" was independently traced through source: `select_driver_mode_for_state` maps only `STATE_RUNNING` to `RunningDispatch` mode (the only mode that can create a dispatch claim, deposit a pending strategy bar, or invoke native strategy code), and `transition_autonomous_daily_operation_to_running` — the *only* function that can set `state = running` — always binds `run_id` in the same atomic CAS write.

**One incompleteness found and repaired** (narrowly-scoped follow-up, `54daa588` retained unmodified rather than rewritten): its two new test-only `Uuid::new_v4()` calls lacked the `// allow: test-only — isolated DB test fixture, never called from production paths` annotation this repo's `check_unsafe_patterns.sh` guard requires (established precedent already in `state/loop_runner.rs`, `mqk-db/src/runtime_lease.rs`, `mqk-runtime/src/orchestrator/tests.rs`). Neither call is reachable from any production path.
**Commit:** `41217092` — `fix: annotate defect-3 test-only Uuid::new_v4 calls for unsafe-pattern guard`. **Proof:** `check_unsafe_patterns.sh` passes clean after; the 5 affected inline tests (`prestart_retry_safety_tests`) re-run and still pass identically.

### Test proof (targeted, per defect)

- **Defect 1** (`AUTON-FIRST-BAR-FRESHNESS-WAIT-SEMANTICS-01`): `scenario_opening_bar_freshness_authority_repair_01.rs` — **1/1 passed** (real DB). Plus 6 pure-function tests in `market_data_freshness.rs` (`opening_bar_tests`) and 6 more in `state::lifecycle::opening_bar_freshness_authority_tests` (all in-crate `cargo test -p mqk-daemon --lib`, part of 809/809 passed).
- **Defect 2** (`AUTON-LEGACY-FRESHNESS-OPERATOR-RETRY-01`): `manual_retry_eligibility_tests` (5 tests, in-crate) plus the full `scenario_autonomous_daily_operator_retry_01.rs` suite — **18/18 passed** (real DB, `--include-ignored`), including `t_legacy_full_recovery_lifecycle_market_data_not_fresh` (the exact positive end-to-end incident-recovery proof, which also drives the real coordinator's `dispatch_by_state` after recovery and confirms it progresses toward `AwaitingOpen`/`PreparingData` on its own — no further operator action).
- **Defect 3** (`AUTON-PRESTART-OBSERVATION-RETRY-SAFETY-01`): `prestart_retry_safety_tests` (5 tests, in-crate, real DB) including `coverage_pristine_check_is_unaffected_and_still_reports_has_activity` (proves `check_operation_pristine` remains strict/unmodified) — all pass. Plus `t_prestart_bars_observed_only_retry_succeeds` in the same scenario file above (real HTTP retry route, real DB).
- **Full crate regression** (this mission touched only `market_data_freshness.rs`, `state/lifecycle.rs`, `state/autonomous_retry_policy.rs`, `routes/autonomous_daily_operator.rs`): `cargo test -p mqk-daemon --lib` **809/809 passed**; `scenario_autonomous_completed_bar_driver_01.rs` **56/56 passed**; `scenario_autonomous_completed_bar_task_01.rs` **49/49 passed**; `scenario_autonomous_daily_coordinator_policy_01.rs`, `scenario_daily_data_readiness_01/_api_01/_start_gate_01.rs`, `scenario_autonomous_daily_session_coordinator_01.rs`, `scenario_autonomous_daily_outcome_coordinator_integration_01.rs` **all passed** (see one exception below, unrelated).

### Combined proof — `AUTON-MONDAY-FIRST-BAR-SELF-HEAL-E2E`

New file, added only after all three defects passed individually: `scenario_auton_monday_first_bar_self_heal_e2e_01.rs`.
**Commit:** `7eb865ee` — `test: prove Monday-opening self-heal across all three defect patches`.

- `self_heal_01_t1_wait_then_t4_freshness_gate_clears_without_manual_intervention` — drives the real `start_execution_runtime` twice on one fixture with **no operator action in between**: at open+45s (only the prior session's tail exists) it refuses `latest_completed_bar_pending`/`WaitForCondition` with zero run/outbox rows; at open+301s (current bar now published) the market-data-freshness authority no longer refuses on any freshness ground — proving patch 1 is a genuine, self-resolving *wait*, not a permanent reclassification.
- `self_heal_02_t2_bar_due_but_missing_still_fails_closed` — the carve-out never covers a genuine gap: bar due, still missing, still refuses, still classifies `ManualInterventionRequired`, zero run rows.
- T3 (a `PrepareDataOnly`-shaped observation mid-wait must not poison retry safety) and T5–T7 (bounded dispatch exactly once, no duplicate dispatch, valid no-signal evaluation) are deliberately not re-proven in this new file — the identical claims are already proven, without duplication, by pre-existing/unmodified tests confirmed green this same session: `prestart_retry_safety_tests::bars_observed_only_is_safe`, `t_prestart_bars_observed_only_retry_succeeds`, `scenario_autonomous_completed_bar_driver_01.rs`'s `preopen_to_running_lifecycle_26_35_exactly_once_dispatch`, and `scenario_autonomous_completed_bar_task_01.rs`'s `m01_task_level_prepare_to_running_exactly_once`.

### Authority reconciliation

| Authority | Fact it owns | Before-first-bar behavior | After-bar-due behavior | Repair authority | Start authority |
|---|---|---|---|---|---|
| `daily_data_readiness` | canonical multi-symbol readiness against the expected bar grid (missing/insufficient/interior_gap/stale, grace/skew-aware) | falls back to the previous session's tail; reports ready | expects the new grid slot; blocks on missing/stale beyond grace | no (evaluator only) | no (feeds the coordinator's tick, not `start_execution_runtime` directly) |
| `market_data_freshness` (legacy, inside `start_execution_runtime`) | independent last-mile wall-clock bar-age re-check | (post-patch-1) recognizes the structural condition via `is_awaiting_first_session_bar`, reusing `daily_data_readiness`'s own grace/timeframe helpers; returns `latest_completed_bar_pending` (`WaitForCondition`) instead of `market_data_not_fresh` (`Manual`) | reverts to full enforcement unchanged: genuine stale/missing/insufficient still blocks `Manual` | no | yes — gates the final `start_execution_runtime` call |
| required-universe controller / `PrepareDataOnly` | actual provider ingest, bounded historical bootstrap, bar observation | nothing to bootstrap yet; polls once due | polls, ingests, observes exactly the expected bar | **yes** — the only genuine repair authority | no |
| `autonomous_retry_policy` | classifies a `RuntimeLifecycleError` into `WaitForCondition` vs `ManualInterventionRequired` | `latest_completed_bar_pending` → `WaitForCondition` (bounded automatic backoff) | genuine `market_data_not_fresh` → `ManualInterventionRequired` (durable park) | no (pure classifier) | no |
| `autonomous_daily_coordinator` | the durable operation state machine, ticks readiness + calls `start_execution_runtime` | stays in `start_retrying`, retries on bounded backoff | transitions to `manual_intervention_required` with the exact fault_class if still blocked | no (drives the loop the repair authority runs under) | yes — sole autonomous caller of `start_execution_runtime` |
| `POST /daily-operation/retry` | sanctioned narrow operator recovery from `manual_intervention_required` | never reachable — `WaitForCondition` never durably lands here by construction | recognizes the exact legacy reason (patch 2) + `check_prestart_retry_safety` (patch 3) + a fresh re-run of canonical readiness before any mutation | no (only re-admits into the normal pipeline) | **never** — explicitly forbidden from starting runtime, arming, clearing halt, or changing reconcile |

**No remaining contradiction for the first-bar timing condition.** Before this mission, `daily_data_readiness` (ready, via prior-session fallback) and legacy `market_data_freshness` (stale, via flat threshold) disagreed on the exact same structural fact with no reconciliation — Friday's actual incident. Patch 1 closes this by making the legacy gate consult the same calendar/timeframe/grace truth `daily_data_readiness` already uses for the identical narrow condition, so the *retry classification* now agrees the condition is transient and self-resolving; genuine staleness after the first bar is due still fails closed identically on both authorities. **Live-mode non-regression:** the carve-out is deployment-mode-agnostic by construction (no `Paper`/`Live` branch), but it only ever changes *how* an already-refused start is classified for autonomous-retry purposes — it never grants any mode additional access past `start_execution_runtime`'s independent, untouched deployment-mode/capital-policy/deployment-economics/arm gates, which are evaluated separately and unconditionally. Patch 2's operator-retry route independently requires `PAPER` (proven by the pre-existing, unmodified `r06_live_deployment_not_authorized`).

### One pre-existing, out-of-scope finding (not fixed, not blocking)

`scenario_market_data_autofresh_required_universe_01.rs::stop_start_generation_race_old_cycle_cannot_overwrite_new_owner` fails deterministically on this host (reproduced 3× in isolation, `--test-threads=1`, no concurrent load) with `A's provider call must start within 10s: Elapsed(())`. This is the **same test, same failure signature**, already discovered and documented as pre-existing host-timing flakiness in §22 (`git stash`-confirmed identical against unmodified code at the time). Confirmed again here: the test was introduced by `aae1e3b8`, long before this mission's three commits, in required-universe scheduler generation-ownership code none of `0e6ea651`/`e109c9db`/`54daa588`/`41217092`/`7eb865ee` touch. Left unfixed per this mission's explicit scope discipline (one defect → one patch; no broad refactor of surrounding autonomous infrastructure). Recommended as a separate, independent follow-up mission.

### Validation tallies

- `cargo check -p mqk-daemon`: **PASS**. `cargo clippy -p mqk-daemon --all-targets -- -D warnings`: **PASS** (0 errors; same pre-existing unrelated `sqlx-postgres` future-incompat note only).
- `git diff --check`: clean.
- `scripts/guards/check_unsafe_patterns.sh`: **PASS** (after `41217092`). `scripts/guards/check_ignored_load_bearing_proofs.sh`: **PASS**.
- All scenario/unit suites listed above: **passed**, with the one pre-existing unrelated exception documented above.

### Status

- Defect 1 (`AUTON-FIRST-BAR-FRESHNESS-WAIT-SEMANTICS-01`): **CLOSED** — `0e6ea651`, retained unchanged, individually proven.
- Defect 2 (`AUTON-LEGACY-FRESHNESS-OPERATOR-RETRY-01`): **CLOSED** — `e109c9db`, retained unchanged, individually proven.
- Defect 3 (`AUTON-PRESTART-OBSERVATION-RETRY-SAFETY-01`): **CLOSED** — `54daa588` retained + narrow guard-annotation follow-up `41217092`, individually proven.
- Combined `AUTON-MONDAY-FIRST-BAR-SELF-HEAL-E2E` proof: **CLOSED** — `7eb865ee`, passes, no remaining authority contradiction identified.
- Friday 2026-08-14 session: **`BLOCKED_STRATEGY_NOT_INVOKED`, does not count toward the 10–20 session soak.**
- Required-universe generation-race flake: **OPEN, out of scope** — pre-existing, unrelated, not fixed this mission.

---

## 24. Research / Backtest — Promotion Evidence Program (P7 → P10)

*Added by `MASTER-LEDGER-CONSOLIDATION-01`, 2026-08-17. This section is authoritative over `docs/research/Research_Backtest_V1_Closeout_Audit.md` for CURRENT status per the precedence note at the top of this document — that file (dated 2026-08-15) predates the later commits in this chain and has not been updated. It remains a valid historical/technical record of methodology and earlier closure evidence.*

### P7 chain — status as of HEAD `b80749bd` (confirmed pushed to `origin/main` — verified 2026-08-21, `MASTER-LEDGER-REPO-TRUTH-REFRESH-02`; local `main` has since advanced to unpushed `242cb7c3`, see below)

| Item | Status | Evidence |
|---|---|---|
| **P7A** — execution pricing / commission parity | **ACCEPTED, PUSHED** (ancestor of `origin/main`, no longer the tip — see 2026-08-21 refresh below) | Commits `3e2d926b`..`f8357ebc`; `f8357ebca81c3177a323393c749d06e2e17986e9` was `origin/main` HEAD at the time of the 2026-08-17 review — P7B/LONG-SHORT/P7C and subsequent docs commits have since been pushed on top of it (`origin/main` = `fd90f63a` as of 2026-08-21; `f8357ebc` remains an ancestor). `REQUIRED_EXECUTION_PRICING_PROTOCOL_ID = "rust_conservative_bar_range_v1"` enforced in `research_evidence.rs`. |
| **P7B** — weight-to-share / discrete economics parity | **ACCEPTED_LOCALLY, PUSHED** (confirmed 2026-08-21) | Commits `1e3cfe41`, `be1c6220`, `99e806e3`(long-short, see below), `221feb45`, `b079d6b5`, `81dcf621` (P7B-REPAIR-03, final reversal-arithmetic repair — independent review accepted the final prospective-gross reversal arithmetic per mission record). `REQUIRED_WEIGHT_TO_SHARE_PROTOCOL_ID = "weight_to_share_v1"` and `REQUIRED_DISCRETE_ECONOMICS_PROTOCOL_ID = "discrete_share_economic_path_v1"` both enforced. **FROZEN — do not reopen** absent a deterministic contradiction (CLAUDE.md §6). |
| **LONG-SHORT economic policy** | **ACCEPTED_LOCALLY, PUSHED** (confirmed 2026-08-21) | Commits `99e806e3` (versioned long/short economic policy), `b079d6b5` (legacy identity preservation). `mqk-promotion` is deliberately agnostic to long-only vs long/short (proven by `both_legacy_long_only_and_new_long_short_shapes_verify_identically`). **FROZEN — legacy identity compatibility, long/short threshold mapping, score terminology, and signed-share behavior must not be reopened** absent deterministic contradiction. |
| **P7C** — durable, registry-anchored OOS evidence gate | **ACCEPTED_LOCALLY, PUSHED** (corrected 2026-08-17, `MASTER-LEDGER-TRUTH-REPAIR-01`, after independent review; push confirmed 2026-08-21, `MASTER-LEDGER-REPO-TRUTH-REFRESH-02`) | Chain: `16b7445a` (REPAIR-01, require verified OOS evidence) → `19fc44d5` (REPAIR-02, verify OOS artifacts + statistical thresholds) → `b185d91b`/`cbcf9c10` (REPAIR-03, anchor to durable Research registry) → **`b80749bd` (REPAIR-04, stabilize cross-language judge authority)**. Each REPAIR superseded the previous within the same chain; only REPAIR-04 at `b80749bd` is current. |

**MECHANISM vs. PRODUCTION WIRING vs. `RESEARCH_BACKTEST_V1_COMPLETE` — do not conflate these:**
- **MECHANISM:** `verify_promotion_oos_evidence` / `VerifiedPromotionOosEvidence` — implemented, independently accepted, and **pushed to `origin/main`** (Wave 2, this table; `b80749bd` confirmed an ancestor of `origin/main` = `fd90f63a` as of 2026-08-21).
- **PRODUCTION WIRING:** **IN PROGRESS / PARTIAL — REPAIR REQUIRED** — local `main` HEAD `242cb7c3` (one commit ahead of `origin/main`, unpushed) constructs the equivalent gate (Gate 4c) in the real daemon promotion route; focused unit tests pass (11/11) and `mqk-promotion` is unregressed (70/70), but independent review of this commit has since occurred (2026-08-21) and found material gaps: a cross-candidate authority gap, a parallel/partial promotion policy (Research verification + DSR/PBO checks run directly instead of routing through canonical `mqk_promotion::evaluate_promotion`), missing durable research lineage, and a missing canonical backtest-evidence seam (`BacktestReport`/`ArtifactLock`/`StressSuiteResult` not resolved for a candidate-bound seam) — the immediate prerequisite for closing this gap is the new `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` entry (§5, `OPEN`). The DB-backed integration/closure-proof harness also still could not be run this session (local test-DB migration drift), and the commit remains unpushed. Tracked by `PROMOTION-WALKFORWARD-GATE-WIRING-01` (§5, status `IN PROGRESS / PARTIAL — REPAIR REQUIRED`).
- **`RESEARCH_BACKTEST_V1_COMPLETE`:** **NOT MET.** Wave 2 push is satisfied; still requires `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` CLOSED + `PROMOTION-WALKFORWARD-GATE-WIRING-01` CLOSED (push + gap repair + DB-harness proof) + P9 CLOSED + P10 CLOSED.

**P7C-REPAIR-04 summary (commit `b80749bd`) — implementation-agent evidence, focused-test counts:** fixed a genuine cross-language canonicalization defect — Python `json.dumps` and Rust `serde_json` are not guaranteed to format every float identically (`1e-06` vs `1e-6`), so the prior REPAIR-03 mechanism (Rust rehashing the supplied judge JSON and comparing to the Python-registered hash) could falsely reject a genuinely authoritative artifact. Fixed by durably persisting Python's exact canonical judge text (`canonical_judge_json` column, additive migration) alongside its hash, and having Rust verify per-row integrity against that stored text before doing a same-language (Rust-side) semantic comparison against the supplied artifact. 7 new Rust tests + 5 new Python tests added (exponent-format interoperability, semantic numeric mutation, registry-integrity tampering in both directions, missing canonical text, conflicting/identical re-registration). These focused counts (`cargo test -p mqk-promotion`: 70 passed / 0 failed; targeted `pytest` on 4 files: 93 passed / 0 failed) are implementation-agent evidence from the REPAIR-04 implementation session itself — see the canonical acceptance-boundary validation block below for the totals the independent review actually evaluated.

**Independent review & final acceptance-boundary validation (2026-08-17):** ChatGPT independently reviewed and accepted commits `81dcf621` (P7B-REPAIR-03) and `b80749bd` (P7C-REPAIR-04) by diff inspection — this was a review of the code, not an independent re-run of the test suite. The following are the implementation-agent's full test-suite totals from the completed controller validation report at HEAD `b80749bd`, recorded here as the canonical acceptance-boundary evidence (superseding the narrower focused counts above where they conflict):
- `mqk-promotion`: **101 passed / 0 failed**.
- full `research-py`: **1490 passed / 7 skipped / 0 failed** (+ 12 subtests passed).
- `mqk-backtest`: **265 passed / 0 failed**.
- `mqk-execution`: **108 passed / 0 failed**.

**Status distinction:** Wave 2 (P7B + LONG-SHORT + P7C) is `ACCEPTED_LOCALLY — PUSHED` (confirmed 2026-08-21, `MASTER-LEDGER-REPO-TRUTH-REFRESH-02`: `git merge-base --is-ancestor b80749bd origin/main` succeeds; `origin/main` = `fd90f63a`, a descendant of `b80749bd`). This corrects the prior claim that `origin/main` remained `f8357ebc` — that was stale as of this session.

**Supersedes (mechanism only, not the production-wiring invariant):** the original proposed mechanism in `PROMOTION-WALKFORWARD-GATE-WIRING-01` (Lane B, §5) — that entry's production-wiring gap is now implemented locally but unpushed, and a separate, later independent review of that specific commit (2026-08-21, distinct from this Wave-2 mechanism review) found further gaps; see its own current entry (status `IN PROGRESS / PARTIAL — REPAIR REQUIRED`) and §13.

### P9 — `BKT-ROBUSTNESS-GAUNTLET-01`

**Status:** OPEN (not started) · **Priority:** P1 · **Paper Impact:** GREEN (research-only, no execution/portfolio/broker path) · **Subsystem:** research-py / mqk-promotion
**Dependencies:** Wave 2 (P7C-REPAIR-04) is independently accepted and **confirmed pushed** to `origin/main` (2026-08-21) — this dependency is met. P9 has not started for any other reason; it does not additionally depend on `PROMOTION-WALKFORWARD-GATE-WIRING-01`.
**Required scope:**
- 2x / 3x cost stress
- execution-delay stress
- symbol leave-one-out
- month/year/regime concentration
- parameter-neighborhood execution
- DSR/PBO sensitivity
- shuffled/random-label placebo
- conservative P7A/P7B execution/capacity stress

**Hard stop:** if the placebo (shuffled/random-label) test appears statistically significant, do not tune the gauntlet to make it pass — that is exactly the failure mode this gauntlet exists to catch. Report the finding and stop.

### P10 — `RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01`

**Status:** OPEN (not started) · **Priority:** P1 · **Paper Impact:** GREEN · **Subsystem:** research-py / mqk-promotion / docs
**Dependencies:** `PROMOTION-WALKFORWARD-GATE-WIRING-01` `CLOSED` (production wiring proven) **and** P9 `CLOSED`.
**Purpose:** compose existing evidence (not re-derive it) into a final Research/Backtest completion record: Git SHA identity, environment/dependency identity, any genuinely still-missing Research CLI entrypoints, and the final `RESEARCH_BACKTEST_V1_COMPLETE` determination.
**Explicit constraint:** P10 does not create a parallel evidence/dossier/registry framework — it composes what P7/P9 already produced using existing seams (the Research SQLite registry, existing artifact hashing/provenance conventions).

### Dependency chain

```text
Wave 2 (P7A + P7B + P7C, commit b80749bd)
    |
    v
INDEPENDENT REVIEW  (DONE 2026-08-17 — 81dcf621, b80749bd ACCEPTED locally)
    |
    v
Wave 2 pushed to origin/main  (CONFIRMED DONE 2026-08-21 — b80749bd is an
                                ancestor of origin/main = fd90f63a)
    |
    v
PROMOTION-WALKFORWARD-GATE-WIRING-01  (production wiring — IMPLEMENTED locally
                                        at unpushed commit 242cb7c3; independent
                                        review of that commit (2026-08-21) found
                                        material gaps; status IN PROGRESS / PARTIAL
                                        — REPAIR REQUIRED, see §5)
    |
    v
PROMOTION-BACKTEST-EVIDENCE-SEAM-01  (new prerequisite identified by that
                                       review — OPEN, not started, see §5)
    |
    v
(repair remaining PROMOTION-WALKFORWARD-GATE-WIRING-01 gaps, push to
 origin/main, DB-backed harness proof — before CLOSED)
    |
    v
P9  BKT-ROBUSTNESS-GAUNTLET-01  (OPEN, not started — its Wave-2-push
                                  dependency is now satisfied)
    |
    v
P10  RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01
    |
    v
RESEARCH_BACKTEST_V1_COMPLETE
```

See §26 for how this chain connects to Operations Resilience and the eventual autonomous Paper soak.

### Post-V1 Research Capability Backlog — Vibe-Trading Comparative Audit

*Added by `RESEARCH-VIBE-GAP-BACKLOG-01`, 2026-08-21, docs-only.*

**Everything in this subsection is POST-V1 work.** Nothing here is `READY` while `RESEARCH_BACKTEST_V1_COMPLETE` is false, unless an entry's own dependency line states something even stronger. These entries must not preempt:
- `PROMOTION-WALKFORWARD-GATE-WIRING-01` (§5, §24)
- P9 `BKT-ROBUSTNESS-GAUNTLET-01` (§24)
- P10 `RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01` (§24)
- autonomous Paper operational validation (§26)

Existing frozen contracts remain authoritative over every entry below: `fwd_ret` (or any other prediction label) is a label, not executable P&L, unless an accepted protocol explicitly says otherwise; execution must remain causal; the final holdout remains reserved unless a mission explicitly authorizes consumption, and consumed holdout data is never fresh again; trial != attempt != evaluation slice, and retries/windows do not manufacture unique trials; result values never define trial identity; promotion evidence remains OOS/cost/execution-aware. None of these entries reopens or weakens any of them.

**Do not import or copy Vibe-Trading implementation code as part of any entry below.** The comparative audit that produced this backlog identified concepts/capabilities only — every entry is a from-scratch, asset-neutral, deterministic design against this repo's own contracts and seams.

#### 1. `RESEARCH-FACTOR-CONTRACT-AND-REGISTRY-01`
**Status:** DEFERRED — POST `RESEARCH_BACKTEST_V1_COMPLETE`
**Purpose:** Create an asset-neutral, deterministic factor research contract and registry. Identity must cover semantic formula/source/version, required inputs, warmup, timeframe/universe compatibility, parameters, implementation identity, and relevant data/provenance.
**Constraints:** Do NOT add a giant factor zoo in this patch. Do NOT treat result values as identity. Do NOT touch execution/Paper/Live.

#### 2. `RESEARCH-FACTOR-IC-IR-QUANTILE-BENCH-01`
**Status:** BLOCKED
**Dependencies:** `RESEARCH_BACKTEST_V1_COMPLETE`; `RESEARCH-FACTOR-CONTRACT-AND-REGISTRY-01`.
**Purpose:** Cross-sectional Spearman IC, IC mean/IR, positive-period ratio, horizon decay, quantile returns/equity, top-minus-bottom spread, coverage/missingness, and deterministic registered artifacts. Research evidence only; never a promotion bypass.

#### 3. `RESEARCH-FACTOR-NULL-CONTROLS-01`
**Status:** BLOCKED
**Dependencies:** `RESEARCH-FACTOR-IC-IR-QUANTILE-BENCH-01`.
**Purpose:** Deterministic within-date shuffled/null-factor falsification controls.
**Hard invariant:** random seeds/permutations/control repetitions are evaluation slices under the same hypothesis/trial context and MUST NOT manufacture independent trials.

#### 4. `RESEARCH-POINT-IN-TIME-UNIVERSE-01`
**Status:** DEFERRED / CONDITIONAL
**Dependencies:** `RESEARCH_BACKTEST_V1_COMPLETE`; `RESEARCH-FACTOR-CONTRACT-AND-REGISTRY-01`.
**Purpose:** Provide explicit point-in-time universe membership/provenance for broad historical cross-sectional research. A declared fixed universe remains legal and must stay explicitly identified as `fixed_declared_universe`. No fixed current constituent list may be represented as point-in-time history. Required BEFORE broad survivorship-sensitive factor claims, but must not block small fixed-universe research.

#### 5. `RESEARCH-FACTOR-FDR-01`
**Status:** BLOCKED
**Dependencies:** `RESEARCH-FACTOR-IC-IR-QUANTILE-BENCH-01`, plus an actual multi-hypothesis factor experiment requiring family-wise discovery analysis.
**Purpose:** Benjamini-Hochberg/FDR over registered factor hypotheses.
**Hard invariant:** FDR is additive diagnostics/discovery control and DOES NOT replace DSR/PBO or the existing promotion authority.

#### 6. `BKT-LIQUIDITY-IMPACT-CAPACITY-01`
**Status:** DEFERRED — POST `RESEARCH_BACKTEST_V1_COMPLETE`
**Purpose:** Optional Research/Backtest ADV participation limits, liquidity-dependent impact stress, unfilled/capacity evidence, and strategy capital/capacity curves.
**Hard invariant:** must not modify Paper/runtime/broker/live execution behavior. No generic impact formula may be treated as production calibration without real evidence. Activate before making meaningful strategy-scalability/capacity claims.

#### 7. `RESEARCH-FACTOR-EXPOSURE-ATTRIBUTION-01`
**Status:** BLOCKED
**Dependencies:** `RESEARCH-FACTOR-IC-IR-QUANTILE-BENCH-01`, and at least one real multi-symbol factor/strategy candidate worth diagnosing.
**Purpose:** Diagnose common market/style exposures such as size, value, momentum, volatility, and liquidity, and separate those exposures from residual strategy return. Diagnostic only; not a new promotion authority.

**Ideas intentionally not yet issued patch IDs:** point-in-time fundamental-data research; portfolio optimizers; event-study framework; Brinson/performance attribution; richer strategy-discovery UI; scheduled/agentic research loops. Each of these becomes a patch only when a concrete hypothesis/product need creates a deterministic requirement — speculative capability must not be turned into owed infrastructure.

---

## 25. Operations Resilience Backlog (`OPS-*`)

*Added by `MASTER-LEDGER-CONSOLIDATION-01`, 2026-08-17. These are new tracked items, not yet started, required before autonomous (unattended) Paper soak per the controlling mission — they do not block the currently-running supervised Paper soak.*

#### `OPS-AUTO-RESTART-LOCAL-01` — Safe automatic restart/recovery after local interruption

**Status:** OPEN · **Priority:** REQUIRED BEFORE AUTONOMOUS PAPER SOAK
**Purpose:** safe automatic restart/recovery after power outage, Windows reboot, daemon crash, Docker interruption, Postgres interruption, network outage, or provider interruption.
**Required conceptual sequence:** boot → dependencies ready → DB available → durable state restored → broker queried READ-ONLY → orders/positions reconciled → market data freshness validated → session/safety gates validated → execution authority acquired → Paper execution allowed.
**Required eventual invariants:** automatic startup; dependency-aware startup; idempotent startup; single local runtime authority; reconcile before trading; disagreement fails closed; bounded restart/backoff; durable recovery evidence; no duplicate jobs/evaluations/orders; Paper first; never auto-enable Live.
**Required future proofs:** normal reboot; abrupt shutdown; daemon crash; Docker unavailable then recovers; Postgres unavailable then recovers; network unavailable then recovers; provider unavailable then recovers; duplicate-start race; broker/local disagreement; existing-position recovery; pending/open-order recovery; clean no-signal restart; no duplicate economic action.

#### `OPS-OFFSITE-BACKUP-01` — Offsite backup of critical recoverable state

**Status:** OPEN
**Purpose:** loss of the local laptop/site must not destroy critical recoverable state. Eventually back up: accepted Git/source revision; safe non-secret configuration; Paper recovery DB backup; Research registry; promotion evidence; this master ledger; critical reconciliation state; required manifests/artifacts.
**Never store plaintext:** `.env.local`, broker keys, API secrets, tokens, credentials.
**Requires:** encryption, versioning, retention, hash/integrity verification, documented restoration, and an actual restoration test (not merely a documented procedure).

#### `OPS-CLOUD-FAILOVER-PAPER-01` — Cloud warm-standby failover for Paper

**Status:** OPEN
**Architecture direction:** LOCAL PRIMARY + CLOUD WARM STANDBY — explicitly **not** active-active.
**Hard invariant:** AT MOST ONE EXECUTION AUTHORITY, ever.
**Future requirements:** durable renewable leadership lease; fencing/generation token; stale-primary fencing; fail-closed network-partition behavior; broker read-only reconcile before takeover; position/order/account reconcile; code/config/protocol identity match; safe handback; no duplicate economic action.
**Negative controls (eventual):** local power loss; local network loss; cloud network loss; broker unavailable; authority store unavailable; local/cloud partition while both can reach broker; crash before broker ACK persistence; crash after broker ACK but before local persistence; open-position takeover; old local process returns after cloud takeover; stale generation attempts execution; simultaneous startup; version mismatch; DB disagreement.

#### `OPS-CLOUD-FAILOVER-LIVE-01` — Cloud warm-standby failover for Live

**Status:** DEFERRED
**Explicitly outside current scope.** Must not proceed until: Research/Backtest V1 is complete (§24); local restart is accepted (`OPS-AUTO-RESTART-LOCAL-01`); offsite recovery is accepted (`OPS-OFFSITE-BACKUP-01`); Paper failover is accepted (`OPS-CLOUD-FAILOVER-PAPER-01`); split-brain/fencing negative controls are accepted; autonomous Paper soak is accepted; and an explicit future Live authorization exists.
**Never enable Live merely to test failover.**

---

## 26. Near-Term Roadmap (Post Wave-2 → Autonomous Paper Soak)

*Added by `MASTER-LEDGER-CONSOLIDATION-01`, 2026-08-17; corrected by `MASTER-LEDGER-TRUTH-REPAIR-01`, 2026-08-17 (inserted the production-wiring step below, which the consolidation pass omitted); repo truth refreshed by `MASTER-LEDGER-REPO-TRUTH-REFRESH-02`, 2026-08-21 (Wave 2 push confirmed done; production wiring confirmed implemented locally, unpushed); further corrected by `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01`, 2026-08-21 (independent review of the production-wiring commit found material gaps; inserted the new `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` prerequisite step below). This chain is independent of — and must not preempt — the ongoing Lane A-F equity/Paper program (§5-§14), unless current repo truth shows a direct dependency. In particular, broad multi-asset work, cosmetic GUI work, unnecessary infrastructure, or strategy proliferation must not preempt this path. The Post-V1 Research Capability Backlog in §24 (added 2026-08-21, `RESEARCH-VIBE-GAP-BACKLOG-01`) becomes eligible only after `RESEARCH_BACKTEST_V1_COMPLETE` and is explicitly non-blocking here — it must not preempt this chain or the autonomous Paper path below.*

```text
CURRENT (as of 2026-08-21)
Wave 2 (P7A+P7B+P7C) independent review — DONE, ACCEPTED LOCALLY (81dcf621, b80749bd)
        |
        v
Wave 2 acceptance + push to origin/main  (CONFIRMED DONE 2026-08-21)
        |
        v
PROMOTION-WALKFORWARD-GATE-WIRING-01  (production wiring — IMPLEMENTED locally,
                                        unpushed commit 242cb7c3; independent
                                        review of that commit (2026-08-21) found
                                        material gaps; status IN PROGRESS / PARTIAL
                                        — REPAIR REQUIRED, see §5)
        |
        v
PROMOTION-BACKTEST-EVIDENCE-SEAM-01  (new prerequisite from that review —
                                       OPEN, not started, see §5)
        |
        v
(repair remaining gaps + push + DB-backed harness proof before CLOSED)
        |
        v
P9  BKT-ROBUSTNESS-GAUNTLET-01
        |
        v
P10  RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01
        |
        v
RESEARCH_BACKTEST_V1_COMPLETE
        |
        v
OPS-AUTO-RESTART-LOCAL-01
        |
        v
OPS-OFFSITE-BACKUP-01
        |
        v
OPS-CLOUD-FAILOVER-PAPER-01
        |
        v
autonomous Paper operational validation
        |
        v
10-20 session autonomous Paper soak
```

This chain is separate from, and does not gate, the currently-running **supervised** Paper soak (`PAPER_SOAK_GO`, §0/§1) — that soak continues under Lane A/B rules unaffected by Research/Backtest or OPS-* status.

---

## 27. Other Worktrees — Read-Only Inventory (not elsewhere referenced)

*Added by `MASTER-LEDGER-CONSOLIDATION-01`, 2026-08-17. Read-only per the controlling mission — no worktree listed here was modified, merged, or cherry-picked from. Worktrees `MiniQuantDeskV4-data`, `MiniQuantDeskV4-retry`, `MiniQuantDeskV4-ops`, `MiniQuantDeskV4-autofresh`, and `MiniQuantDeskV4-integration` are already documented inline in their respective Lane A/D patch entries above (§5) and are not repeated here.*

| Worktree | Branch | HEAD (at inventory time) | Note |
|---|---|---|---|
| `MiniQuantDeskV4-ai-lab` | `ai/ml-local-lab-foundation-01` | `11f3d571` | Not referenced anywhere else in this ledger. Represents local ML-lab foundation work (per commit message: "correct ai-lab closure proof and OpenHands partial truth"). Status/actionability not verified this session — before acting on it, a future session must re-verify against current `main`, not assume it is still needed merely because the worktree exists (per this ledger's own consolidation rule, §0 precedence). |
| `.codex/worktrees/2915` (branch `codex/apply-determinism-fixes-det01`) | — | `2b357fd8` | Stale session checkpoint (upstream branch reported `[gone]`); appears to be a determinism-fix branch, unmerged. |
| `.codex/worktrees/b992` (branch `codex/implement-migration-governance`) | — | `86688d7f` | Stale session checkpoint; appears to be a migration-governance hardening bundle, unmerged. |
| `.claude/worktrees/agitated-lumiere-f7c208` | `claude/agitated-lumiere-f7c208` | `31056e3d` | `[behind 784]` commits relative to current `main` — abandoned/stale checkpoint (halted-fill-replay repair, incomplete). |
| `.claude/worktrees/bundle-5-runtime-allocation-744794` | detached | `5355c579` | Appears to predate `AUTONOMOUS-DAILY-PAPER-OPERATIONS` Bundle 4/5 acceptance already reflected elsewhere in this ledger — likely superseded, not independently verified. |
| `.claude/worktrees/busy-bardeen-9c0e9a` | detached | `32bda6b7` | Durable-paper-portfolio-closure hardening checkpoint, unmerged, not independently verified. |
| `.claude/worktrees/optimistic-bohr-96b041` | `claude/optimistic-bohr-96b041` | `a76494c7` | `[behind 841]` commits — abandoned/stale checkpoint (Alpaca WS gap-recovery marking), superseded by current broker-lifecycle rules. |
| `.claude/worktrees/premarket-script-guard-repair-b23d9b` | detached | `5355c579` | Same HEAD as `bundle-5-runtime-allocation-744794` above — likely a duplicate/redundant clone, not a distinct patch. |

**Recommendation for a future session, if any of these are ever revisited:** do not assume any listed worktree's content is still needed or still correct merely because it exists — re-derive from current `main` and this ledger first (per §0 precedence), consistent with mission guidance that stale branches are inventoried, not trusted.

---

*End of MiniQuantDesk V4 Authoritative Master Completion Ledger — FULL-REPO-COMPLETION-AUDIT-01, updated by PAPER-AUTONOMOUS-STARTUP-THREE-DEFECT-CLOSURE-01, updated by MASTER-LEDGER-CONSOLIDATION-01 (2026-08-17).*
