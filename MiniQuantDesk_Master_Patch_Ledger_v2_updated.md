# MiniQuantDesk V4 — Authoritative Master Completion Ledger

**Audit:** FULL-REPO-COMPLETION-AUDIT-01
**Audit date:** 2026-08-10
**Mode:** AUDIT + LEDGER ONLY — no application code, DB, or trading behavior was modified in this session.
**Branch:** `main`
**Starting HEAD:** `0a019b8bd80298ac0a04ba77fb080522122c37a8` ("fix: fence daemon supervisor safety halts")
**origin/main HEAD (matches local):** `0a019b8bd80298ac0a04ba77fb080522122c37a8` (state at this audit's 2026-08-10 start — see repo-truth refresh note immediately below for current state)

**Repo-truth refresh (`MASTER-LEDGER-CURRENT-TRUTH-CLOSURE-01`, 2026-08-30):** Verified directly against Git. `main` local HEAD and `origin/main` HEAD are now identical at `70ed507acfe02ef860b8378b9e5eddb25a36065d` ("fix: distinguish stress transforms in semantic identity") — both pushed and verified via `git rev-parse HEAD` / `git rev-parse origin/main`. Between the prior refresh below and this one, `origin/main` advanced through the `RESEARCH-BACKTEST-V1-FINAL-INTEGRATION-AND-ACCEPTANCE-01` closure (Executive Summary above, commit `12490668`) and then a further 14-commit strategy-identity / promotion-binding wave (`4ef6b643`..`70ed507a`, all 2026-08-29, all pushed ancestors of this HEAD) — see the new **Current Strategy-Identity / Promotion-Binding Wave Verdict** paragraph below and §5's `MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01` entry, both added by this refresh. No application code, DB, or trading behavior was modified by this refresh — audit/docs only.

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
**Corrected 2026-08-30 (`MASTER-AUDIT-TRUTH-CORRECTION-01`) — see "Current Paper/Risk Truth Correction" below.** The paragraph immediately below (unchanged, retained for history) predates the `FULL-SYSTEM-COMPLETION-SITUATIONAL-AUDIT-01` (2026-08-30) runtime-risk findings and overstates current truth in one respect: its "No RED (soak-blocking) source defect was found anywhere in the audited codebase" claim is now superseded — the live risk gate's runtime-state/config authority (frozen equity/day/reject-window, an unreachable kill-switch input, an ordinarily-disabled max-drawdown control) is a real, deterministic gap in exactly the RED-classified risk/safety category this ledger's own Paper-Soak Protection Rule (§1) exists to catch. The rest of this paragraph's characterization of engineering depth remains accurate.

*Original (2026-08-10/29, retained for history):* The equity/ETF paper-trading core (orchestrator, OMS state machine, outbox/inbox, broker adapters, risk, portfolio, reconciliation, backtest engine, promotion gates, GUI truth-state discipline) is **evidence-provably complete and fail-closed** at HEAD. No RED (soak-blocking) source defect was found anywhere in the audited codebase. The repository's real remaining gaps cluster in three places: (1) live-capital readiness — deliberately and completely gated off pending a trust-chain proof that doesn't exist yet; (2) operational hardening around multi-symbol dispatch resilience, CLI/daemon control-plane parity, and Discord alert coverage; (3) one uncommitted-but-well-formed patch closing a narrow halt/deadman race that needs a harness run before it can be called closed.

### Current Paper Verdict
**SUPERSEDED 2026-08-30 (`MASTER-AUDIT-TRUTH-CORRECTION-01`) — see "Current Paper/Risk Truth Correction" below for the current authoritative verdict.** The `PAPER_SOAK_GO` verdict immediately below (retained for history — it accurately reports what the 2026-08-10 validation session concluded at that time, against that HEAD) is **no longer current authoritative status**: it predates the 2026-08-30 runtime-risk findings and did not evaluate whether the account-level risk gate's runtime-state authority was wired. Current status is **BLOCKED FOR COUNTABLE PAPER SOAK / FORWARD VALIDATION**.

*Original (2026-08-10, retained for history):* **PAPER_SOAK_GO** (`FINAL-CANONICAL-PRE-SOAK-VALIDATION-01`, 2026-08-10, HEAD `e44e3ddd`). The one previously-open item, `PRE-SOAK-DAEMON-LOCAL-QUIESCENCE-AND-DEADMAN-SIDE-EFFECT-FENCE-01`, is now CLOSED — its H08 test passed against a real local Postgres as part of a full canonical safe-ignored matrix run (733/733 tests green: H01-H08, daemon-supervisor halt fence, runtime halt fence CAS, stale-claim recovery, deadman, durable portfolio/P&L, fill/replay authority, outbox/pre-submit authority, risk/kill-switch/reconcile all proven with zero failures). All previously-tracked blockers (TradeActivity schema mismatch, partial-fill dedup, stale-claim recovery, terminal-fill replay parity) have corresponding committed fixes at HEAD, and this validation reproduced no new regression against any of them. No known accepted-list paper-soak code blocker remains.

### Current Live Verdict
**NOT READY, and cannot become ready without new work.** `LiveCapital` cold-start is hard-gated behind a trust-chain proof (`live_trust_complete`) that is **hardcoded `false`** in `research-py`'s TV-03 pipeline — this is by design, not a bug, and correctly enforced at both the advisory and cold-start-enforcement layers. Separately, live account truth is wrong today: `buying_power` is aliased to `cash` rather than pulled from Alpaca's real `buying_power`/`daytrading_buying_power` fields, which is economically dangerous for a margin account. No live-capital smoke-test tooling exists. A prior memory record claiming "daemon defaults to real Alpaca WS unless forced to paper" is **stale** — current default (`Paper`/`Paper`) is fail-closed and safe; this session is correcting that memory record.

### Current Research/Backtest Verdict
**`RESEARCH_BACKTEST_V1_COMPLETE` — CLOSED — INDEPENDENTLY ACCEPTED** (2026-08-28, `RESEARCH-BACKTEST-V1-FINAL-INTEGRATION-AND-ACCEPTANCE-01`; supersedes the paragraph immediately below, `FINAL-P9-AUTHORITY-BINDING-REPAIR-01`, commit `06417bdc`). ChatGPT independently reviewed the full 24-commit Research/Promotion V1 closure range (`fbddeb3d`..`06417bdc`) plus its evidence bundle (`RESEARCH_PROMOTION_V1_INDEPENDENT_REVIEW_01.zip`, all 25 manifest-listed files hash-verified) and found `CANONICAL_PROMOTION_DECISION`, `BACKTEST_EVIDENCE_SEAM`, `CROSS_CANDIDATE_AUTHORITY`, `OOS_RESEARCH_AUTHORITY`, `DURABLE_PROMOTION_LINEAGE` (structurally atomic), `STRESS_SUITE_AUTHORITY`, `ROBUSTNESS_GAUNTLET`, `DSR_PBO_SENSITIVITY`, `GENUINE_SHUFFLED_PLACEBO`, and `P7A_P7B_REPLAY` all SOUND, with one remaining deterministic proof defect: P10's own acceptance had claimed a route-level Postgres lineage readback that did not actually exist (the shared `mqk_test` DB also carried unrelated historical migration-checksum drift, so DB proof was honestly classified BLOCKED, not fabricated). That gap was closed by test-only commit `12490668e57f0ab2a900bb0e4b045619e4a904be` (`test: prove promotion http route persists exact evidence lineage`) — a real daemon HTTP promotion route exercised end to end against a fresh isolated disposable Postgres (`mqk_promotion_lineage_review_20260828_55375fb1`), with a raw Postgres readback proving the exact Research trial identity, economic-evaluation identity, judge-artifact hash, backtest-run identity, stress/robustness protocol + artifact hashes, promotion-policy fingerprint, and scanner/review evidence-root binding actually judged, plus a mutation-style RED control (a wrong Research lineage identity is proven to cause failure). Repair bundle `RESEARCH_PROMOTION_LINEAGE_HTTP_PROOF_REPAIR_01.zip` (12 manifest-listed files) was independently hash-verified and accepted. Results on the fresh isolated DB: closure proof 1/1, `mqk-daemon` promotion routes 33/33, `mqk-db` promotion registry/lineage 33/33, P10 acceptance suite 5/5 — zero production files changed by the repair (test-only: exactly `core-rs/crates/mqk-daemon/tests/scenario_strategy_promotion_closure_proof_01f.rs` and `core-rs/crates/mqk-promotion/tests/scenario_research_backtest_promotion_v1_acceptance_01.rs`). `12490668` was fast-forward-integrated onto `ledger-closure-integration-01` at the identical SHA (`RESEARCH-BACKTEST-V1-FINAL-INTEGRATION-AND-ACCEPTANCE-01`, 2026-08-28); no test rerun was required for that integration step, since the fast-forward produced the byte-identical, already-reviewed commit — verified via `git diff --check` (clean) and `git diff --name-status` (only the same two test files). **This means engineering closure — Research -> Backtest -> OOS/robustness evidence -> canonical promotion evaluation -> production HTTP promotion boundary -> durable Postgres evidence lineage — is independently accepted end to end.** It does NOT mean `PROVEN_ALPHA`, promotion-readiness for an arbitrary new strategy, final-holdout consumption, `SHORT-WAVE-03` execution, Paper forward validation, or Live readiness — each of those remains a separate, unestablished stage. Does not activate the Post-V1 Research Capability Backlog (§24) and does not alter Paper/Live status. Underlying implementation commits `e56f94fb` and `06417bdc` were already pushed ancestors of `origin/main` before this acceptance (per the immediately-superseded paragraph below); `12490668` itself remains local-only on `ledger-closure-integration-01`, not pushed.

**Prior (2026-08-22, `FINAL-P9-AUTHORITY-BINDING-REPAIR-01`) — now superseded by the correction immediately above:** `RESEARCH_BACKTEST_V1_COMPLETE` — LOCALLY COMPLETE, PENDING INDEPENDENT REVIEW (commit `06417bdc`; supersedes the paragraph immediately below, `FINAL-P10-FIXTURE-REALISM-01`). Closes three remaining P9 evidence-authority bindings an independent review of `RESEARCH_BACKTEST_V1_FINAL_PRODUCTION_4.mbox` found still open, with no change to any frozen algorithm, robustness threshold, execution logic, or Research trial identity. (1) `dsr_pbo_sensitivity` now requires a caller-supplied `--judge-artifact-sha256` naming the exact P7C-authorized `research_judge_artifacts` row, resolves ITS registered `(experiment_id, hypothesis_id)` scope (never `trial_id`'s own `hypothesis_id`), and reuses that exact scope for every block-count rerun; `evaluate_promotion` now requires this authoritative judge identity to equal the P7C-verified judge identity — closing a real gap where a whole-experiment-scoped P7C judge could be silently narrowed to a single-hypothesis comparison population while claiming to vary "only" the block count. (2) `dsr_pbo_sensitivity` is a REQUIRED promotion-grade P9 scenario: every `not_evaluable` outcome (previously sometimes mapped to `applicable: false` for a structurally-too-small comparison population) now maps to `applicable: true, passed: false` unconditionally — it can no longer vanish from a promotion-grade P9 artifact. (3) `genuine_shuffled_placebo`'s `research_trial_id`, `baseline_economic_eval_id`, and `protocol_id` are now extracted from its structured evidence and checked against the P7C-verified trial/economic-result identity and the one accepted placebo protocol (`genuine_shuffled_placebo_v1`) — previously unchecked entirely. (4) `p7a_p7b_economic_replay_stress` evidence completeness is now checked against every required structured field (exact `protocol_id`, both economic-result identities and their content hashes, the three input-file hashes, bars-provenance/pricing identity, stress-spec identity, and the actual stressed pass/fail metric), not merely `baseline_economic_eval_id` alone. Verified: 9/9 `dsr_pbo_sensitivity.rs` unit tests, 11/11 `test_dsr_pbo_sensitivity_cli.py`, 18/18 `scenario_robustness_gauntlet_artifact_01.rs`, 13/13 `scenario_promotion_requires_robustness_evidence_01.rs` (including 7 new Section 1/3/4 negative controls), 5/5 `scenario_research_backtest_promotion_v1_acceptance_01.rs`, full `mqk-promotion`/`mqk-backtest`/`mqk-artifacts` suites green, full workspace `cargo check --workspace --tests` clean, and — against the same real disposable Postgres (`mqk-test-postgres`, port 5434) prior sessions used — `scenario_strategy_promotion_routes_01.rs` 33/33 pass, `scenario_strategy_promotion_closure_proof_01f.rs` 1/1 pass, both `--include-ignored --test-threads=1`. **This is still a local, self-assessed completion** — independent review of `06417bdc` has not yet occurred, and it has not been pushed to `origin/main` (which remains at `fbddeb3d`). Does not activate the Post-V1 Research Capability Backlog (§24) and does not alter Paper/Live status.

**Prior (2026-08-22, `FINAL-P10-FIXTURE-REALISM-01`) — now superseded by the correction immediately above:** `RESEARCH_BACKTEST_V1_COMPLETE` — LOCALLY COMPLETE, PENDING INDEPENDENT REVIEW (commit `e19602ff`; supersedes the paragraph immediately below, `FINAL-RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01`). Closes the exact, honestly-named blocker the prior session left open: the `mqk-daemon` DB/HTTP integration suite's own test-fixture debt, with zero production code changes. (1) Every positive-path test across both integration files now builds Research evidence via `write_real_research_evidence_via_production_pipeline` instead of the lightweight hand-built fixture, which could never satisfy the mandatory `p7a_p7b_economic_replay_stress`/`genuine_shuffled_placebo` scenarios' requirement for genuine `inputs`; `closure_proof_01f.rs` gained its own copy of that helper (duplicated for the file's existing "no cross-crate test visibility" reason) and its dead hand-built fixture was removed. (2) `smooth_uptrend_bars` (shared by both files) was replaced with one deterministic 240-bar/8-month sequence built from three legs — calm uptrend, wide-intrabar-range uptrend, and a decline leg the strategy genuinely shorts and profits from — verified directly against the real `detect_market_regime` classifier and `run_robustness_gauntlet` to produce 3 genuinely distinct, non-concentrated regime buckets (`month_year_regime_concentration` now genuinely passes, not merely inapplicable), with every other P9/stress scenario continuing to clear with real, unforced margin. (3) `closure_proof_01f.rs`'s own `write_real_backtest_evidence` had a separate, pre-existing fixture bug (its P7A/P7B stress call passed `max_target_qty=None`, failing the earlier `FINAL-P7A-P7B-REPLAY-AUTHORITY-01` genuine-tightening requirement) — fixed to `Some(1000)`, matching the routes file's already-correct call. Verified against the same real disposable Postgres (`mqk-test-postgres`, port 5434) the prior session used: `scenario_strategy_promotion_routes_01.rs` 33/33 pass, `scenario_strategy_promotion_closure_proof_01f.rs` 1/1 pass, both `--include-ignored --test-threads=1`. **This is still a local, self-assessed completion** — independent review of `e19602ff` has not yet occurred, and it has not been pushed to `origin/main` (which remains at `fbddeb3d`). Does not activate the Post-V1 Research Capability Backlog (§24) and does not alter Paper/Live status.

**Prior (2026-08-22, `FINAL-RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01`) — now superseded by the correction immediately above:** `RESEARCH_BACKTEST_V1_COMPLETE` — NOT MET (updated 2026-08-22, `FINAL-RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01`, commits `dba91f44`, `975e952a`, `ede3a0b6`; supersedes the paragraph immediately below, `RESEARCH-BACKTEST-V1-FINAL-P7A-P7B-REPLAY-CLOSURE-01` / `930d60c1`). This mission hardened P7A/P7B replay authority and P9 semantics against a stricter, explicitly promotion-grade invariant, then ran the real HTTP/Postgres acceptance chain (the first session to do so — see below) and found a real, currently-open gap.

- **Patch 1, `FINAL-P7A-P7B-REPLAY-AUTHORITY-01` (`dba91f44`): CLOSED.** Closes six remaining gaps in `p7a_p7b_economic_replay_stress` against the mission's promotion-grade invariant: (A) "mandatory means mandatory" — a `not_evaluable` CLI response now maps to `applicable: true, passed: false`, never `applicable: false` (this required scenario can no longer vacuously satisfy P9 completeness by being silently excluded); (B) exact `economic_eval_id` binding — the CLI now requires a caller-supplied `--economic-eval-id` and resolves the EXACT succeeded attempt whose durable registry `result_id` equals it, never "the latest successful attempt"; (C) durable-authority content-hash authentication — recomputes `economic_walk_forward.json`'s content hash from disk and requires it equal the durable registry `result_id`, so a mutated artifact can never become replay authority merely by being internally self-consistent; (E) exact spec reconstruction round-trips byte-for-byte against the recorded protocol identity before any replay; (F) genuine P7A/P7B adversity validation (stress must be strictly worse in at least one P7A dimension and strictly tighter in at least one real P7B capacity cap; no-op/loosened stress is rejected); (G) durable typed replay evidence (`RobustnessScenarioOutcome.evidence`) plus a new `evaluate_promotion` gate requiring the P7A/P7B replay's bound `economic_eval_id` to equal the verified P7C/OOS evidence's own. 23/23 new/updated `research-py` tests pass, 10 new subprocess-free Rust dispatch tests, zero regressions across `mqk-backtest`/`mqk-artifacts`/`mqk-promotion` (139 tests) or the full `research-py` suite.
- **Patch 2, `FINAL-P9-ROBUSTNESS-SEMANTICS-01` (`975e952a`, `ede3a0b6`): CLOSED.** Bumps `ROBUSTNESS_GAUNTLET_PROTOCOL_VERSION` to `bkt_robustness_gauntlet_v2` (a `v1` artifact is rejected by promotion exactly like any other stale protocol) and closes: DSR/PBO sensitivity now requires >= 2 DISTINCT `block_counts` (a single-value or duplicate-only grid trivially reports zero sensitivity and now fails closed pre-spawn); a genuine shuffled/null-control placebo (new `genuine_shuffled_placebo` scenario, replacing `placebo_temporal_offset` as required placebo evidence) that deterministically permutes the trial's frozen OOS `ml_score` stream within each fold and re-runs the real economic evaluator against it; month + year + regime concentration (`month_year_regime_concentration`, renamed from `month_and_regime_concentration`) as three independent bucketed checks, regime buckets built by classifying each calendar month's own bars via the existing `detect_market_regime`; and edge-collapse semantics (a shared `clears_economic_edge` check) failing `execution_delay_stress`, `symbol_leave_one_out`, `parameter_neighborhood_execution`, and `cost_stress_2x`/`cost_stress_3x` whenever a genuinely profitable baseline collapses to net non-positive return under stress. Full `mqk-backtest`/`mqk-artifacts`/`mqk-promotion` suites green (including new, empirically-tuned real-fixture negative controls for every new invariant) and the full `research-py` suite green (1528 passed, 7 skipped, 12 subtests).
- **Patch 3 acceptance run — real disposable Postgres, not merely "no MQK_DATABASE_URL":** unlike every prior wave recorded in this ledger, this session confirmed `mqk-test-postgres` (Docker, port 5434) genuinely running and reachable, and ran the DB-backed suites against it for real. `mqk-promotion`'s real P10 acceptance chain (`scenario_research_backtest_promotion_v1_acceptance_01.rs`, all of `p10a`-`p10e`) passes in full: real `BacktestEngine` -> real artifacts -> real stress suite -> complete v2 P9 gauntlet (all 9 required scenarios) -> real Research SQLite registry verified via the frozen P7C mechanism -> canonical `evaluate_promotion`, including the new `economic_eval_id` binding gate. `mqk-daemon`'s two DB/HTTP integration files were then run with `--include-ignored` against that same real Postgres: `scenario_strategy_promotion_routes_01.rs` 26/33 passed (7 failed), `scenario_strategy_promotion_closure_proof_01f.rs` 0/1 (failed). Every failure traces to one of two known, non-code-defect causes -- in each failing case the promotion route correctly REJECTED under-qualifying evidence, proving the new gates work as designed:
  1. **Lightweight-fixture consequence (6 of 7 routes-file failures, and the closure_proof file's one test)** — exactly the effect disclosed in the `dba91f44`/`975e952a` commit messages: candidates built from this file's own hand-registered Research-evidence fixture (no real `inputs` recorded in `economic_walk_forward.json`) now correctly fail the mandatory `p7a_p7b_economic_replay_stress`/`genuine_shuffled_placebo` scenarios instead of being silently excluded via `applicable: false`.
  2. **Shared "healthy" fixture is genuinely regime-concentrated (the routes-file's `real_research_production_trial_used_for_both_p7c_and_p9_passes`, and a contributing cause of the closure_proof failure)** — `smooth_uptrend_bars` (a smooth monotonic exponential uptrend, shared by 18 daemon tests including this one, which DOES use the real production Research pipeline) puts 99.18% of its equity gain in a single `bull_trend` regime bucket. This is an ACCURATE finding, not a bug: independently proven correct via `mqk-backtest`'s own `rg01m` unit test (a hand-built regime-diverse fixture correctly detects and fails the same way) and via the fully-green `mqk-promotion` P10 chain above. Three attempts to recalibrate `smooth_uptrend_bars` against the real `swing_momentum` strategy's own signal logic (varying trend/sideways segment lengths and daily rates) each still produced >=98.5% single-regime concentration and did not converge in this session; per CLAUDE.md's "if an approach fails twice for the same reason, stop and reassess" guidance, further tuning was not attempted.
- **Exact blocker:** the `mqk-daemon` HTTP/Postgres integration suite has a real, currently-open, TEST-FIXTURE-ONLY gap (no production code defect) — repairing it requires (a) migrating the affected lightweight-fixture tests to `write_real_research_evidence_via_production_pipeline` (already proven correct by this same file's own passing production-pipeline tests) and (b) recalibrating or replacing `smooth_uptrend_bars` against `swing_momentum`'s real entry/exit logic so its shared "healthy" candidate is not itself regime-concentrated. This touches on the order of 24 test functions across two large integration files — sized beyond what this session's resource-constrained, no-full-suite mandate permits to also attempt safely. `RESEARCH_BACKTEST_V1_COMPLETE` therefore remains **NOT MET**, even though every mechanism-level patch this mission required is CLOSED at the code+test level, and even though this session achieved strictly more real verification (an actual live-Postgres run) than any prior wave recorded below.

**Prior (2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-P7A-P7B-REPLAY-CLOSURE-01`) — now superseded by the correction immediately above:** `RESEARCH_BACKTEST_V1_COMPLETE` — LOCALLY COMPLETE, PENDING INDEPENDENT REVIEW (commit `930d60c1`; see §24's closing summary below for full per-entry/per-commit detail). This corrects and supersedes the paragraph immediately below (`RESEARCH-BACKTEST-V1-FINAL-AUTHORITY-REPAIR-01`, 2026-08-22), which had reported two open gaps: (1) genuine P7A/P7B execution/capacity stress replay, HARD STOPPED on a premise that the durable Research registry retained no usable per-row replay input; and (2) real Research-production-path artifact CONTENT (not merely registry rows) for the DB E2E fixtures. Re-inspection this session found gap (1)'s premise incomplete: `run_registered_economic_walkforward_eval` already durably records, in `economic_walk_forward.json`'s own `inputs` section, content-addressed `file_record()`s (`{path, bytes, sha256}`) for `bars_csv`/`oos_predictions_csv`/`walk_forward_eval` — everything needed to re-verify and replay the trial's FROZEN OOS predictions through the real, accepted `run_economic_walkforward` under an explicit conservative P7A/P7B stress configuration, with no new bars database, no training replay, and no new replay framework. `P7A-P7B-ECONOMIC-REPLAY-STRESS-01` (commit `930d60c1`) implements exactly this: a new `p7a_p7b_economic_replay_stress` P9 scenario wired into `REQUIRED_ROBUSTNESS_SCENARIO_NAMES`, its own `evaluate_promotion` trial-binding gate (mirroring `dsr_pbo_sensitivity`'s, proven to fail closed independently of it), 13 `research-py` negative-control tests (tampered/missing/mismatched inputs, non-official baseline P7A/P7B, unknown trial, no-new-trial-registered, holdout-still-reserved, durable-evidence-field proof), 2 dedicated `mqk-promotion` trial-binding negative controls isolating the new gate, and a mutation-proof (temporarily disabling the SHA-256 input check let an equal-byte-length OOS tamper pass silently and undetected; restoring the check caught it). Gap (2) was independently confirmed already closed by the prior `REAL-RESEARCH-TO-PROMOTION-E2E-01` wave (`real_research_promotion_e2e_cli`, a real Python subprocess, not hand-written JSON) — this session additionally strengthened `scenario_strategy_promotion_routes_01.rs`'s `real_research_production_trial_used_for_both_p7c_and_p9_passes` to assert the NEW P7A/P7B scenario itself was genuinely evaluated (`applicable: true, passed: true`), not merely deferred/inapplicable. Both DB-backed integration test files pass against real disposable Postgres with the new gate active: `scenario_strategy_promotion_routes_01.rs` 30/30, `scenario_strategy_promotion_closure_proof_01f.rs` 1/1. **This is still a local, self-assessed completion** — independent (ChatGPT) review of `930d60c1` has not yet occurred, and it has not been pushed to `origin/main` (which remains at the confirmed-pushed `fbddeb3d`). Does not activate the Post-V1 Research Capability Backlog (§24) and does not alter Paper/Live status.

**Prior (2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-AUTHORITY-REPAIR-01`) — now superseded by the correction immediately above:** This corrects and supersedes the paragraph immediately below (`RESEARCH-BACKTEST-V1-FINAL-PRODUCTION-CLOSURE-CONTROLLER-01`, 2026-08-22 earlier the same day), which had reached `LOCALLY COMPLETE`. A further independent (ChatGPT) review of that closure's own local HEAD (`54eee812`) found four more confirmed deterministic defects — most notably that the closure's own positive DB E2E proof used a DIFFERENT Research trial for P9 DSR/PBO sensitivity evidence than for P7C/OOS evidence, silently, without detecting it. This repair wave (commits `db436a44`, `265fd63f`, `80eb8a5a`, `679f9499`, on top of `54eee812`, none pushed) closes two of the four findings completely (trial-binding gate + negative/positive controls; extended evidence lineage + legacy-duplicate-backfill fix, both proven against real disposable Postgres) and reports two as honest partial closures with a named, non-fabricated remaining gap (genuine P7A/P7B execution/capacity stress replay — the durable Research registry does not retain the per-row input data needed; and real Research-production-path artifact CONTENT generation, as opposed to real registry ROWS, for the DB E2E fixtures). Per this ledger's own rule, `RESEARCH_BACKTEST_V1_COMPLETE` cannot be marked `LOCALLY COMPLETE` while either gap remains open. Does not activate the Post-V1 Research Capability Backlog (§24) and does not alter Paper/Live status.

**Prior (2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-PRODUCTION-CLOSURE-CONTROLLER-01`) — now superseded by the correction immediately above:** This corrects and supersedes the paragraph immediately above (`RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01`, 2026-08-21) via two further local waves, neither pushed: (1) `RESEARCH-BACKTEST-V1-FINAL-REPAIR-WAVE-01` (commits `7f8b0cdb`..`86c61557`, on top of the confirmed-pushed baseline `fbddeb3d`) repaired the protocol-authority and P9-completeness gaps a later independent (ChatGPT) review of `fbddeb3d` found — see the "Independent review finding" correction in §24, which this supersedes; (2) this same 2026-08-22 controller's Patch 5A/5B/6 (`4f300a78`, `37649200`, `307bac81`) closed the one gap that review's own correction explicitly left open for P10: the real HTTP `POST /api/v1/strategy/promotions/transition` route now runs against a real disposable `mqk-test-postgres` instance, with real (non-fabricated) Research/Backtest/P9 evidence end to end, and a read-back proof that the persisted lineage identifies the exact evidence judged (`mqk-daemon`'s `scenario_strategy_promotion_routes_01.rs` / `scenario_strategy_promotion_closure_proof_01f.rs`). Full `mqk-artifacts`/`mqk-backtest`/`mqk-promotion` acceptance suites, the two named `mqk-daemon` DB-integration test files, and the load-bearing `research-py` frozen-contract tests (holdout reservation/consumption, trial-vs-attempt-vs-slice, winner-only-registration, result-value-independent identity) are all green. **This is still a local, self-assessed completion** — independent (ChatGPT) review of this wave has not yet occurred, and none of `7f8b0cdb`..`307bac81` has been pushed to `origin/main` (which remains at the confirmed-pushed `fbddeb3d`). Does not activate the Post-V1 Research Capability Backlog (§24) and does not alter Paper/Live status.

### Current Strategy-Identity / Promotion-Binding Wave Verdict
**PUSHED-VERIFIED** (`MASTER-LEDGER-CURRENT-TRUTH-CLOSURE-01`, 2026-08-30). A 14-commit wave (`4ef6b643`..`70ed507a`, all dated 2026-08-29, all ancestors of the current pushed `origin/main` HEAD `70ed507a`) closed a class of defects where a strategy's *semantic* configuration (not merely its `strategy_name`/opaque config id) could diverge from the identity a promotion, backtest run, or stress/robustness evidence artifact claimed to be bound to. None of this wave is yet reflected elsewhere in this ledger — this paragraph and the corresponding §5 entry below are the correction. Capabilities now accepted as current production truth:
- **Strategy semantic fingerprinting** (`feat: add strategy semantic identity seam`, `4ef6b643`) — a canonical, versioned `semantic_fingerprint()` on `Strategy`/`StrategyHost`, distinct from the pre-existing opaque `config_id`.
- **Promotion → strategy config-identity binding** (`f81b418c`, `e30af1ed`) — promotion continuity transitions and Paper dispatch both now verify the promoted fingerprint against the currently-resolved one before treating a candidate as continuous/eligible.
- **Verified promotion continuity** (`6c63388c` "require verified promotion config continuity") — a `verified_v1`-status promotion can no longer continue on unverified/legacy fingerprint pairs.
- **Legacy NULL/unverified config identity fail-closed** — rows predating semantic-identity support are refused, never silently treated as matching (see `strategy_config_identity.rs`'s negative-control matrix, added in `3d22e2b5`).
- **External already-computed signal semantic provenance fail-closed** (`0f29c266` "fail closed on external semantic provenance").
- **Dynamic-selection config eligibility binding** (`e1487971` "bind dynamic selection to promoted config").
- **Selected-host frozen fingerprint / TOCTOU cross-check** (`5869700e` "snapshot strategy identity before evaluation").
- **Semantic-aware Backtest `run_id`** (`b0431da1`) — `BacktestReport::run_id` now folds `Strategy::semantic_fingerprint()` into a new versioned (`v5`) derivation; historical `v2`/`v3`/`v4` artifacts remain readable, not rewritten.
- **Stress/robustness factory semantic verification** (`cd45ca20`) — every fresh strategy instance `run_backtest_stress_suite`/`run_robustness_gauntlet` constructs is checked against the baseline candidate's `strategy_semantic_fingerprint` before use; a mismatch fails that scenario closed rather than passing silently.
- **Stress-transform effective semantic identity fixed** (`70ed507a`, this HEAD) — splits *underlying candidate identity* (verified pre-wrap) from *effective wrapped-strategy identity* (`DelayedStrategy`'s own fingerprint, encoding wrapper version + inner fingerprint + `delay_bars`), closing a same-`run_id` collision between an execution-delay/placebo run and its baseline.
- **Config-identity verification centralized** (`3d22e2b5`) — the promotion-continuity (C1), runtime-dispatch (C2), and dynamic-selection-eligibility (R4) call sites that had each independently reimplemented the same verified-fingerprint-equality predicate now share one canonical `config_identity_is_verified` function; `mqk-portfolio`'s dependency-free selection gate independently re-validates the caller-claimed boolean against the fingerprint pair rather than trusting it.
- **Migration `0067_dynamic_selection_plan_candidates_config_identity.sql`** exists as a committed migration artifact at this HEAD. Per L1 discipline: this does **not** mean it has been applied to any running Paper database — see "Paper DB migration currency" in §28/near-term items; applying it (or confirming it is already applied) remains an operational step, not a code-review one.

This wave also closed **`MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01`** (§5), a previously-`READY` item, via a separate same-day commit (`060966be`, an ancestor of `4ef6b643` on this same ordered range) — see the §5 entry for the corrected acceptance description (the accepted mechanism is host quarantine, not unconditional per-symbol continuation).

### Current Paper/Risk Truth Correction (2026-08-30, `MASTER-AUDIT-TRUTH-CORRECTION-01`)

This correction supersedes the "Current Paper Verdict" and "Current Repository Verdict" paragraphs above (both retained, unedited, for history) and the percentage/status claims for "Risk System" and "Paper Trading Lifecycle" in the "Closest Subsystems to Completion" line immediately below and in §2's System Completion Map. It is driven by `FULL-SYSTEM-COMPLETION-SITUATIONAL-AUDIT-01`'s (2026-08-30) independently-spot-verified Risk domain finding — full detail and code citations in `docs/audits/2026-08-30_full_system_completion_situational_audit.md` (Domain: Risk, §D, §G) — and does not change any other domain's verdict in this ledger.

- **Research/Backtest/Promotion engineering closure remains accepted** — unaffected by this correction; see the `RESEARCH_BACKTEST_V1_COMPLETE` verdict above.
- **Equity/ETF Paper mechanics are substantially complete** — order lifecycle, portfolio accounting, reconciliation, autonomous-operation scheduling, and GUI truth-state discipline are all independently confirmed sound at the code level (situational audit §C).
- **`mqk_risk::evaluate()` is real, production-reachable risk-engine logic** — it is wired as the middle of the three-gate order-submission pipeline (`IntegrityGate` → `RiskGate` → `ReconcileGate`) via `RuntimeRiskGate`, constructed per-run in `mqk-daemon/src/state/orchestrator_build.rs`. Every order submission is gated by it. This is genuine production wiring, not a stub.
- **But the runtime state/config *authority* feeding that engine is partly static/inert, not fully wired.** `RuntimeRiskGate` binds one `RiskInput` once, at orchestrator construction time, and reuses it for the life of the run except for per-request overrides. Concretely: current equity is frozen at run-start (daily-loss-limit and max-drawdown checks cannot fire against a real intraday loss or drawdown); `day_id` and `reject_window_id` are equally frozen (day-rollover and reject-window rollover cannot occur); `RiskState::record_reject()` has zero production call sites (reject-storm detection cannot trigger); `PdtContext::ok()` is hardcoded (the PDT guard cannot deny); `kill_switch` is hardcoded `None` with no production call site ever constructing a non-`None` value (the kill-switch branch of `evaluate()` is unreachable); and the daemon's ordinary risk-config seam (`load_risk_env`/`effective_run_config_for_risk`) supplies only initial equity and daily-loss-limit, never max-drawdown, so max-drawdown is disabled by default on the ordinary daemon-created-run path, not merely on some unusual hand-authored config.
- **Do not claim Paper safety readiness while the account-level risk gate is partly static/inert.** The mechanics of order routing, accounting, and reconciliation being sound does not substitute for the account-level kill switches CLAUDE.md's priority ordering exists to protect actually being able to fire.
- **Required current verdict: countable autonomous Paper soak is BLOCKED** pending a runtime-risk dynamic state/config authority repair (recommended mission: `RUNTIME-RISK-DYNAMIC-STATE-AUTHORITY-01` — see the situational audit §M for the full decomposition) and required operational DB proof (migration `0067` confirmation against the running Paper DB). Sequencing: the risk-gate repair must close **before** any fresh soak session is run — a fresh soak session does not substitute for, or precede, that repair; it follows it. See the situational audit §D/§I/§K for the exact ordered sequence and the countable-session criteria.
- **Halt/auto-flatten decision is unchanged** (`docs/specs/halt_without_auto_flatten_decision.md`) — halting the runtime still never automatically submits a flatten order; that document's own wording is separately corrected in this same commit to avoid overstating `RiskAction::FlattenAndHalt` verdict reachability given the frozen inputs above.
- **This correction does not itself repair the defect** — it is a docs-only truth correction (`MASTER-AUDIT-TRUTH-CORRECTION-01`). No application code, DB, broker, or trading behavior was modified to produce it.

### Closest Subsystems to Completion
*The percentages below are the 2026-08-10 `FULL-REPO-COMPLETION-AUDIT-01` baseline, retained for history. "Risk System" and "Paper Trading Lifecycle" are superseded by the correction immediately above — do not read either figure as current. See §2's Current Status Map (2026-08-30) for current categorical status.*

Core Execution/OMS (~97%), Database Layer (~97%), Reconciliation (~97%), Risk System (~95%), Paper Trading Lifecycle (~95%), Backtesting Engine (~95%), Test Infrastructure (~95%).

### Highest-Risk Incomplete Subsystems
Live Capital Trading (~40%, gated by design but genuinely far from proven), CLI/Daemon control-plane parity (~60%, no CLI path to arm/halt/clear the live daemon), Discord/Alerting coverage (~70%, multi-channel routing built but unused, no data-staleness/daily-summary pushes), Options/Futures/Forex (~5%, enum + risk-multiplier stub only, explicitly gated off).

### Active Patch Counts
READY: 33 · BLOCKED: 4 · DEFERRED: 8 · IMPLEMENTED_PENDING_REVIEW: 0 · CLOSED (this session): 0

*Counts above cover Lanes A-F (Paper/equity/GUI/live/multi-asset/maintainability) as recorded by the 2026-08-10 `FULL-REPO-COMPLETION-AUDIT-01` audit. `MASTER-LEDGER-CONSOLIDATION-01` (2026-08-17) had incorrectly reclassified `PROMOTION-WALKFORWARD-GATE-WIRING-01` as `CLOSED — SUPERSEDED`; `MASTER-LEDGER-TRUTH-REPAIR-01` (2026-08-17, same day) restored it to `READY`; `MASTER-LEDGER-REPO-TRUTH-REFRESH-02` (2026-08-21) corrected it to `IMPLEMENTED_PENDING_INDEPENDENT_REVIEW`; `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01` (2026-08-21, same day) further corrects it to `IN PROGRESS / PARTIAL — REPAIR REQUIRED` (see §5, §24) — independent review of the unpushed local commit implementing production wiring has since occurred and found material deterministic gaps (cross-candidate authority, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam), with a new prerequisite item `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` now tracked `OPEN`. Otherwise these counts were not re-verified this session. Research/Backtest (P7-P10, §24) and Operations Resilience (OPS-*, §25) are tracked separately and are NOT included in these counts: as of 2026-08-21, Research/Backtest has Wave 2 (P7A/P7B/P7C) `ACCEPTED_LOCALLY — PUSHED`, `PROMOTION-WALKFORWARD-GATE-WIRING-01` `IN PROGRESS / PARTIAL — REPAIR REQUIRED` (local-only, unpushed, independent review found gaps), `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` `OPEN` (new), and 2 items `OPEN` (P9, P10); OPS-* has 3 `OPEN` and 1 `DEFERRED`. **Superseded same-day by `RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01` (2026-08-21):** `BKT-PROMOTION-ARTIFACT-AUTHORITY-01`, `PROMOTION-STRESS-SUITE-AUTHORITY-01` (both new), `PROMOTION-BACKTEST-EVIDENCE-SEAM-01`, `PROMOTION-WALKFORWARD-GATE-WIRING-01`, P9, and P10 are all now `CLOSED_LOCAL — PENDING INDEPENDENT REVIEW` (commits `08a292cd`..`41c19cc7`) — see the Executive Summary verdict above and §5/§24 for full detail; still none of Research/Backtest has been pushed or independently reviewed.*

### GREEN / YELLOW / RED Patch Counts
GREEN: 27 · YELLOW: 12 · RED: 7

### Estimated Remaining Patch Range
**SUPERSEDED (`MASTER-LEDGER-CURRENT-TRUTH-CLOSURE-01`, 2026-08-30).** The prior "42–55 patches" estimate below is from the 2026-08-10 `FULL-REPO-COMPLETION-AUDIT-01` baseline and cannot be reproduced against current `main` (`70ed507a`) without re-deriving it — carrying it forward unchanged would imply false precision against a denominator this session did not recompute. Do not treat the number below as current. The finite, categorized (not patch-counted) critical-path gates to Equity/ETF Paper V1, and the separate Full-V4 backlog, are tracked in the current situational audit instead — see `docs/audits/2026-08-30_full_system_completion_situational_audit.md` (`FULL-SYSTEM-COMPLETION-SITUATIONAL-AUDIT-01`).

*Prior estimate, retained for history only, not current:* ~~**42–55 patches** to reach the Repository-Wide Definition of Done in §19, excluding open-ended multi-asset expansion (Lane E, 3 XL items each requiring further decomposition into an unknown number of sub-patches once scoped). The range reflects uncertainty in how far `LIVE-TRUST-CHAIN-*` and the two lean-out patches will need to be decomposed once started.~~

---

## 1. Paper-Soak Protection Rule

The highest immediate priority is a stable US equity/ETF Paper + Alpaca autonomous trading soak, currently running on `main`. This ledger classifies every patch:

- **GREEN — safe during paper soak.** Source-evidence-confirmed isolation from the running paper economic/safety path.
- **YELLOW — shared code, paper-neutral intent.** Touches code paper also uses; safe to develop on a separate branch/worktree during the soak; must not merge into `main` without explicit regression review.
- **RED — paper economic/safety behavior.** Directly changes order decisions, execution, risk, reconciliation, position/P&L authority, fills, runtime leadership, halt/recovery, autonomous startup, data-freshness gates, or broker submission. Deferred during the soak unless it repairs a reproducible soak-blocking defect (none currently exist).

No patch in this ledger lacks a classification.

---

## 2. System Completion Map

**HISTORICAL — 2026-08-10 `FULL-REPO-COMPLETION-AUDIT-01` baseline, retained for history and per-patch detail only. Not current authority.** The percentages, "PROVEN COMPLETE" labels, and remaining-patch counts below reflect that audit's own denominator at its own HEAD and have not been recomputed since; several rows (notably Risk System, Paper Trading Lifecycle) are directly superseded by the 2026-08-30 correction above. Do not cite this table as current status. See **§2a. Current Status Map (2026-08-30)** immediately after the table for current authoritative, categorical status per the situational audit.

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
| 18 | Multi-Symbol Trading | ~85% — wired and dispatching in production; panic isolation CLOSED 2026-08-29 (`060966be`, corrected 2026-08-30 — mechanism is shared-host quarantine on first panic, not unconditional per-symbol continuation; see §5 `MULTI-SYMBOL-DISPATCH-PANIC-ISOLATION-01`); capital caps still opt-in and silently disabled if unset. | IMPLEMENTED BUT INCOMPLETE | 3 | RED | D |
| 19 | Documentation / Runbooks | ~70% — dense, mostly-accurate historical spec archive; `README.md`'s living "repository snapshot" is 3 weeks stale relative to HEAD. | PARTIAL (living doc stale) | 1 | GREEN | B |
| 20 | Discord / Alerting / Observability | ~70% — delivery contract solid and fail-safe; 6-channel routing built but unused (single flat webhook only); no data-staleness or daily-summary alerts. | IMPLEMENTED BUT INCOMPLETE | 3 | YELLOW | D |
| 21 | CLI | ~60% — read-only/diagnostic commands (backtest, md, db, autonomous diagnostics) solid; **zero** CLI path to arm/halt/clear/disarm the live daemon — HTTP-only. | PARTIAL / SCAFFOLDED (parity gap) | 2 | GREEN | B |
| 22 | Performance / Maintainability | ~70% — no dead-code explosion; several files >7,000 lines in the daemon hot path (`state.rs` 7,591, `lifecycle.rs` 7,126); duplicated alert-block logic. | ACCEPTABLE, not urgent | 2 | GREEN | F |
| 23 | Live Capital Trading | ~40% — shared infra (broker dispatch, arm gate, kill switch, reconcile) proven and reused correctly; trust-chain proof hardcoded false; account truth wrong; zero live smoke tooling. | DEFERRED BY DESIGN (trust gate) / real gaps beneath it | 8 (+2 blocked, +1 external) | YELLOW/GREEN | C |
| 24 | Multi-Asset — Equity/ETF | 100% of current scope — trades as `Equity` with `instrument_kind="etf"` tag; fully operational. | PROVEN COMPLETE | 0 | — | — |
| 25 | Multi-Asset — Crypto | ~25% — Kraken OHLC data-ingest lineage substantial; zero execution wiring. | PARTIAL (data only) | 1 | GREEN (isolated) | E |
| 26 | Multi-Asset — Options/Futures/Forex | ~5% — `AssetClass` enum variants + risk-multiplier stub match-arms only; explicitly gated off (`MQK_ASSET_CLASS_*_ENABLED`, all default false); no broker/execution/GUI/tests. | SCAFFOLDED / DEFERRED BY DESIGN | 2 | GREEN (isolated) | E |

### 2a. Current Status Map (2026-08-30)

*Categorical statuses per `docs/audits/2026-08-30_full_system_completion_situational_audit.md` — no completion percentages are asserted here; none were recomputed against current HEAD and inventing fresh ones would imply false precision. This map covers the domains that audit inspected; it does not replace the row-level historical detail in the table above.*

| Domain | Status | Note |
|---|---|---|
| Research / Backtest / Promotion | CONFIRMED — engineering-closed, independently accepted | Unaffected by this correction. |
| Strategy Framework | CONFIRMED, with nuances | Concurrent long+short dispatch not yet multiplexed; `max_concurrent_symbols`-in-production status UNKNOWN-NEEDS-PROOF. |
| Data | CONFIRMED, one real gap | Corporate-action/event-risk: no earnings-calendar feed, no pre-event flattening gate (operator-declared static blackout only). |
| Execution / OMS | CONFIRMED | Atomic outbox claim, restart-safe idempotency, structural Paper/Live isolation. |
| **Risk** | **CONFIRMED wired, BLOCKED on runtime-state authority** | `mqk_risk::evaluate()` is genuinely production-reachable, but equity/day/reject-window/kill-switch inputs are frozen at run-start and max-drawdown is ordinarily disabled — see the correction above and situational audit §G. **This is the current NOW-lane engineering blocker.** |
| Portfolio / Accounting | CONFIRMED | Durable FIFO lot accounting, inbox-only dedup-guarded writes. |
| Reconciliation / Recovery | CONFIRMED | One shared clean-reconcile definition; local backup/restore round-trip proven; real-B2 offsite round-trip still outstanding. |
| Autonomous Paper Operations | CONFIRMED, soak-validity caveat | Stale-operation fix is independently-accepted CLOSED but has zero confirmed-VALID sessions since; see §K of the situational audit for countable-session criteria. |
| Daemon / Control Plane | CONFIRMED | Single armed/halted source of truth, mode isolation from one source. |
| GUI | CONFIRMED | `truth_state` hard-block discipline holds, stricter than documented minimum. |
| Multi-Asset (equity/ETF) | CONFIRMED — full current scope | Other asset classes remain data/economics-layer scaffolding at most, by design. |
| Live | CONFIRMED NOT READY, hard-gated by design | `live_trust_complete` hardcoded false; `buying_power` aliased to `cash` is a separate, real, Live-gated defect. |
| Git / CI / Repository Governance | CONFIRMED — CI real; `main` has no branch protection or ruleset | Re-confirmed 2026-08-30 via `gh api` (protection: 404; rulesets: empty). Governance track, does not gate Paper/Live runtime safety — see situational audit §D. |
| Testing | CONFIRMED | 520 test files, disciplined `#[ignore]` tracking, DB-backed CI by default, genuine adversarial controls. |

**Current Paper readiness (this map's bottom line):** BLOCKED FOR COUNTABLE PAPER SOAK / FORWARD VALIDATION — see the correction above and situational audit §I/§K.

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

**Status:** CLOSED_LOCAL — PENDING INDEPENDENT REVIEW (updated 2026-08-21, `RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01` / `PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE`, commit `f8e9edf4`). Repairs all four gaps the 2026-08-21 independent review of `242cb7c3` found (see that finding, preserved below): (1) cross-candidate authority — both `backtest_evidence_gate::evaluate_backtest_evidence_gate` and the extended `research_evidence_gate::evaluate_research_evidence_gate` now independently cross-check their resolved evidence's own strategy identity against the promotion candidate's `strategy_id`, proven against real end-to-end fixtures (not mocks) in both gates' own test suites; (2) parallel/partial promotion policy — `evaluate_research_evidence_gate` no longer compares DSR/PBO itself; it returns the raw `VerifiedPromotionOosEvidence`, and the route constructs a real `PromotionInput` (report + artifact_lock + stress_suite + oos_evidence, all from the resolved `BacktestEvidenceBundle`) and calls the canonical `evaluate_promotion`, which alone decides every gate together; (3) missing durable Research lineage — migration 0065 adds nullable `research_trial_id`/`research_economic_eval_id`/`research_deflated_sharpe_ratio`/`research_probability_backtest_overfitting`/`backtest_run_id` to `sys_strategy_promotion_transitions`, written by a new best-effort `record_promotion_transition_lineage_v2` immediately after a successful insert; (4) missing canonical backtest-evidence seam — closed by `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` below (`e56f94fb`) and wired into the route via the new `backtest_evidence_gate` module. Full `mqk-artifacts`/`mqk-backtest`/`mqk-promotion` suites green; `mqk-daemon`'s evidence-gate modules green (`--test-threads=1`; a pre-existing, unrelated env-var-race flakiness in `research_evidence_gate`'s own test module was found and flagged separately, not fixed as part of this patch); full `mqk-daemon --lib` has zero new failures (793 passed; 31 pre-existing `MQK_DATABASE_URL`-required failures + 15 ignored, unrelated to promotions). **Known limitation, stated honestly:** the two DB-backed integration tests that exercise the real HTTP route end-to-end (`scenario_strategy_promotion_routes_01.rs`, `scenario_strategy_promotion_closure_proof_01f.rs`) were not run this session — no `MQK_DATABASE_URL` configured on this box, and this same pre-existing local test-DB migration-checksum drift (see the 2026-08-21 update note below) independently blocks them regardless of this patch. Both files compile against the new function signatures but their JSON fixtures do not yet supply `backtest_run_id`, so neither would currently pass a real DB run for an evidence-requiring transition — follow-up work. **Not pushed. Not independently reviewed.** Preserved below: the full history through the 2026-08-21 independent-review finding this closure repairs. · **Priority:** P1 · **Paper Impact:** GREEN (promotion output is a report artifact; no portfolio/risk/execution/broker writes) · **Subsystem:** mqk-promotion / mqk-daemon
**2026-08-22 update:** a LATER independent (ChatGPT) review of pushed baseline `fbddeb3d` found this entry's own protocol-authority gap (stress-protocol identity silently dropped by the evidence-seam bridge) and its "known limitation" above (no DB-backed route proof); both are now closed — see §24's `RESEARCH-BACKTEST-V1-FINAL-REPAIR-WAVE-01` / `RESEARCH-BACKTEST-V1-FINAL-PRODUCTION-CLOSURE-CONTROLLER-01` closing summary for the full, current-truth resolution and commit list. Still not pushed, still not independently reviewed.

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
**Acceptance History:** Implementation DONE locally (`242cb7c3`, unpushed) / Unit-level validation PASSED (11/11 gate tests, 70/70 `mqk-promotion`, `git diff --check` clean) / DB-backed integration & closure-proof harness PENDING (blocked by local test-DB migration drift, not yet re-attempted) / Independent review DONE — REPAIR REQUIRED (2026-08-21: cross-candidate authority gap, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam — see finding above) / **REPAIR DONE locally (2026-08-21, `f8e9edf4`): all four gaps closed, see Status line above** / DB-backed integration harness still PENDING (no `MQK_DATABASE_URL` this session; same pre-existing drift) / Push to `origin/main` PENDING / Independent review of the repair itself PENDING.

#### BKT-PROMOTION-ARTIFACT-AUTHORITY-01 — Canonical durable `BacktestReport` + audit-chained completion evidence

**Status:** CLOSED_LOCAL — PENDING INDEPENDENT REVIEW (2026-08-21, `RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01`, commit `08a292cd`) · **Priority:** P1 · **Paper Impact:** GREEN (backtest artifact writer only; no execution/portfolio/broker path) · **Subsystem:** mqk-artifacts / mqk-backtest / mqk-promotion
**Current Source Truth (before this patch):** `BacktestReport` had no durable, schema-versioned round-trip — `metrics.json`/`orders.csv`/`fills.csv`/`equity_curve.csv` are derived, lossy views. Backtest `audit.jsonl` was left empty by every run, so `mqk_promotion::lock_artifact_from_str` (Patch B6) unconditionally rejected every real backtest artifact (`AuditEmpty`).
**Resolution:** `mqk-artifacts::backtest_report_artifact` writes a schema-versioned, lossless `backtest_report.json` (a plain DTO mirror of `BacktestReport`/`BacktestOrder`/`BacktestFill`/etc. — never the engine/portfolio types directly, avoiding wire coupling to their internals) and cross-validates it against `manifest.json` (`run_id`/`strategy_name`/config identity/`execution_model_id`) on load, failing closed on any mismatch, malformed content, missing file, or unsupported schema version — never reconstructed from `metrics.json`/CSVs, never optimistically upgrading a pre-existing artifact. `write_backtest_report` now appends a single, hash-chained `backtest_run_completed` completion audit event (idempotent across retries) carrying the canonical report's own SHA-256 and the run's `initial_cash_micros` (added by the later `PROMOTION-WALKFORWARD-GATE-WIRING-01-REPAIR-CLOSURE` patch, since `PromotionInput.initial_equity_micros` needs a source `BacktestReport` itself does not carry).
**Exact CLOSED End State:** Every new backtest run produces a `backtest_report.json` + a real (non-empty, hash-chained) `audit.jsonl` that `lock_artifact_from_str` accepts — met locally, proven by 12 tests including a real engine round-trip, single-field mismatch rejection (run_id/strategy_name/config_id/execution_model_id, one field mutated per test), and real emitted manifest+audit accepted by `lock_artifact_from_str`.
**Acceptance History:** Implementation DONE locally (`08a292cd`) / 12/12 focused tests PASSED / Full `mqk-artifacts` suite green / Independent review PENDING / Push PENDING.

#### PROMOTION-STRESS-SUITE-AUTHORITY-01 — Real, deterministic Backtest stress-scenario authority for `evaluate_promotion`

**Status:** CLOSED_LOCAL — PENDING INDEPENDENT REVIEW (2026-08-21, `RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01`, commit `8bed1b6c`) · **Priority:** P1 · **Paper Impact:** GREEN (research/promotion evidence only) · **Subsystem:** mqk-backtest / mqk-artifacts
**2026-08-22 update:** a later independent review of pushed `fbddeb3d` found the stress-scenario mechanism here was real, but protocol-identity was not enforced at the loader/evaluator boundary. Repaired by `PROMOTION-STRESS-AUTHORITY-REPAIR-01` (`7f8b0cdb`, §24); re-closed as of that commit, still not pushed.
**Current Source Truth (before this patch):** `mqk_promotion::StressSuiteResult` had no production caller — `StressSuiteResult::pass(n)`/`fail(...)` were test-only constructors with no real execution behind them anywhere in the workspace.
**Investigation finding:** the original Patch B2 doc comment named "adversarial partial-fill + cancel/replace" stress, but the engine has no partial-fill model (every order fills fully or not at all) and no cancel/replace order lifecycle — building either would require rewriting the accepted `BKT-FUTURE-EXECUTION-01` causal-execution invariant, explicitly forbidden by this wave's own hard-stop list. Real adversarial evidence was built instead from execution knobs the engine genuinely supports: `cost_stress_2x`/`cost_stress_3x` (`StressProfile`/`CommissionModel` scaled — also the first two scenarios P9 needed, so P9 reuses rather than duplicates this) and `conservative_risk_limits` (re-run under `BacktestConfig::conservative_defaults()`'s own accepted 2%/18% daily-loss/max-drawdown ratios, exercising the real halt/flatten path).
**Resolution:** `mqk-backtest::stress_suite::run_backtest_stress_suite` (pure computation) + `mqk-artifacts::stress_suite_artifact` (durable `stress_suite.json`, candidate-bound like `backtest_report.json`, and integrity-proven via its own hash-chained `stress_suite_completed` audit event carrying the artifact's SHA-256 — tampering the file without also defeating the audit chain is detected).
**Exact CLOSED End State:** A real stress suite execution produces a durable, candidate-bound, tamper-evident result loadable by `load_canonical_stress_suite` — met locally, proven by 11 tests including a genuinely fragile candidate (~35% realized loss via real fills) producing a real failed scenario and a structural proof that no production source fabricates a `StressSuiteRunOutput` via struct literal outside `stress_suite.rs` itself.
**Acceptance History:** Implementation DONE locally (`8bed1b6c`) / 11/11 focused tests PASSED / Full `mqk-backtest`/`mqk-artifacts`/`mqk-promotion` suites green / Not yet wired into any CLI/daemon route (deferred to `PROMOTION-BACKTEST-EVIDENCE-SEAM-01`/`PROMOTION-WALKFORWARD-GATE-WIRING-01`, both also closed in this wave) / Independent review PENDING / Push PENDING.

#### PROMOTION-BACKTEST-EVIDENCE-SEAM-01 — Canonical candidate-bound backtest evidence seam for `evaluate_promotion`

**Status:** CLOSED_LOCAL — PENDING INDEPENDENT REVIEW (2026-08-21, `RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01`, commit `e56f94fb`) · **Priority:** P1 · **Paper Impact:** GREEN (research/promotion evidence only; no execution/portfolio/broker path) · **Subsystem:** mqk-promotion / mqk-daemon
**2026-08-22 update:** a later independent review of pushed `fbddeb3d` found this seam resolved `BacktestReport`/`ArtifactLock` correctly but silently dropped stress-protocol identity in its `StressSuiteResult` bridge. Repaired by `PROMOTION-STRESS-AUTHORITY-REPAIR-01` (`7f8b0cdb`, §24); re-closed as of that commit, still not pushed.

**Current Source Truth:** `evaluate_promotion` requires genuine canonical inputs — `BacktestReport`, `ArtifactLock`, `StressSuiteResult` — bound to a single, unambiguous promotion candidate. No current production seam resolves these objects for a specific candidate in a way proven bound to the same semantic candidate as any Research/OOS-evidence gate (Gate 4c, `242cb7c3`) or scanner/review evidence.
**Problem:** Without a canonical, candidate-bound backtest-evidence seam, the production promotion path can combine independently-valid pieces of evidence (scanner/review evidence, Research OOS evidence) without proof they refer to the same semantic candidate, and/or invoke Research verification plus DSR/PBO checks directly instead of routing the complete decision through canonical `mqk_promotion::evaluate_promotion`. Identified by independent review of `242cb7c3` (2026-08-21) — see `PROMOTION-WALKFORWARD-GATE-WIRING-01` above.
**Why This Matters:** This is the structural prerequisite for closing `PROMOTION-WALKFORWARD-GATE-WIRING-01` — without it, "production wiring" remains partial regardless of how much of the OOS-evidence mechanism is wired in.
**Dependencies:** `BKT-PROMOTION-ARTIFACT-AUTHORITY-01` (§5, `CLOSED_LOCAL`, `08a292cd`) and `PROMOTION-STRESS-SUITE-AUTHORITY-01` (§5, `CLOSED_LOCAL`, `8bed1b6c`) — both closed in the same wave, immediately before this entry.
**Resolution (`e56f94fb`, `mqk_promotion::resolve_backtest_evidence`):** the caller supplies only a candidate's identity (an artifact root + `run_id`, the existing `BacktestReport::run_id` deterministic identity — never a result/metric value), never the evidence itself. The resolver performs one exact `artifact_root.join(run_id.to_string())` join (the same convention `init_run_artifacts` already writes every run to) rather than searching or picking "latest," making ambiguous duplicate evidence structurally impossible. It independently recomputes `backtest_report.json`'s SHA-256 against the hash-chained completion audit event (a real integrity gap `BKT-PROMOTION-ARTIFACT-AUTHORITY-01` left open — that patch recorded the hash but never verified it), never judges promotion eligibility itself (a genuinely failed real stress suite still resolves successfully — only structural evidence problems are errors), and is proven with 11 tests including a genuine cross-candidate directory swap and a real symlink-based root-escape rejection.
**In Scope:** Define and implement the canonical, candidate-bound seam resolving `BacktestReport`/`ArtifactLock`/`StressSuiteResult` for a specific promotion candidate, and route the complete production promotion decision through `evaluate_promotion` rather than partial/parallel checks. **Out of Scope:** Redesigning `evaluate_promotion` itself; redesigning the Research OOS-evidence mechanism (P7A-P7C, FROZEN, see §24).
**Exact CLOSED End State:** A caller supplying only `(artifact_root, run_id)` receives a fully resolved, candidate-bound, tamper-evident `BacktestReport`/`ArtifactLock`/`StressSuiteResult` bundle or a fail-closed structural error — met locally; production route wiring is `PROMOTION-WALKFORWARD-GATE-WIRING-01` (above), also closed in this same wave.
**Acceptance History:** Implementation DONE locally (`e56f94fb`) / 11/11 focused tests PASSED / Full `mqk-artifacts`/`mqk-backtest`/`mqk-promotion` suites green / Independent review PENDING / Push PENDING.

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

**Status:** CLOSED — PUSHED-VERIFIED (corrected `MASTER-LEDGER-CURRENT-TRUTH-CLOSURE-01`, 2026-08-30; was `READY`) · **Priority:** P1 · **Paper Impact:** RED (live dispatch path for the autonomous paper loop) · **Subsystem:** mqk-daemon multi-symbol dispatch
**Prior Source Truth (now stale, kept for history):** `mqk-daemon/src/state/loop_runner.rs:220` — a spawned-task panic while dispatching symbol N "drops the whole host pool with it." No per-symbol catch/isolate boundary exists; dispatch is sequential (`state.rs:3529`, one `.await` per symbol per tick).
**Problem (as originally scoped):** A single symbol's runtime panic currently takes down the entire tick for every symbol, not just the failing one.
**Closing commit:** `060966be` ("fix: isolate multi-symbol strategy dispatch faults", 2026-08-29, pushed ancestor of current HEAD `70ed507a`).
**Actual accepted mechanism (corrects the original acceptance criteria's "siblings dispatch normally" framing):** `AppState::invoke_native_strategy_host_on_bar` (`mqk-daemon/src/state.rs:3490-3522`) wraps each symbol's `Strategy::on_bar` call in `catch_unwind`. Because Tier A holds exactly one mutable `StrategyHost` shared across every symbol in the tick and every future tick, and `on_bar` takes `&mut self` (so a panic mid-callback cannot be proven not to have corrupted shared state), the production behavior is **host quarantine, not unconditional per-symbol continuation**: a caught panic immediately sets the shared bootstrap to `Failed`, permanently for the rest of the run. Symbols already dispatched *before* the panic in that tick keep their results; the panicking symbol and every symbol dispatched *after* it — later in the same tick, and in every subsequent tick — get no result (ordinary fail-closed `None`, indistinguishable from any other `Dormant`/`Failed` bootstrap, never a fabricated decision) until the run is restarted and re-bootstrapped. This is a real improvement over the pre-fix behavior (previously ALL results for the tick, including symbols already dispatched, were lost when the panic unwound the whole tick) but is narrower than "every sibling this tick is unaffected." Proven by `core-rs/crates/mqk-daemon/tests/scenario_multi_symbol_dispatch_loop_01.rs`'s `a1_r1_real_middle_symbol_panic_quarantines_host_for_remaining_siblings` and `a1_r2_real_first_symbol_panic_blocks_all_siblings` — both inject a real panic inside `Strategy::on_bar` via a real `StrategyHost`, not a pre-dispatch test hook.
**Separate backend, unchanged semantics:** The `DynamicPaperEnforced` selected-host backend already treated an ordinary `on_bar` `Err` as a whole-tick structural fault (zero decisions, halt) before this patch. A panic there is now converted to a `HostOnBarPanicked` fault and contained identically to that pre-existing `Err` path — i.e. still whole-tick-halt, not per-symbol/quarantine — since each binding owns a separate host object, so the shared-mutable-state corruption risk this patch addresses does not apply there.
**Fault durability:** the fault is recorded via the existing signal-evaluation journal (no fabricated decision, no silent swallow), satisfying the original alertability requirement.
**Required Regression Tests:** `scenario_multi_symbol_dispatch_loop_01.rs` — green at HEAD (includes the A1-R1..R3 real-panic negative controls above).
**Acceptance History:** CLOSED 2026-08-29 (`060966be`) · Ledger-corrected 2026-08-30.

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
- **PRODUCTION WIRING:** **CLOSED_LOCAL — PENDING INDEPENDENT REVIEW** (updated 2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-PRODUCTION-CLOSURE-CONTROLLER-01`; see `PROMOTION-WALKFORWARD-GATE-WIRING-01`, §5, for the full resolution history). Local `main` HEAD `242cb7c3`'s Gate 4c found four gaps under independent review (cross-candidate authority, parallel/partial promotion policy, missing durable research lineage, missing canonical backtest-evidence seam), repaired at `f8e9edf4`; a later independent review of pushed `fbddeb3d` found a further protocol-authority gap, repaired at `7f8b0cdb`. The DB-backed integration/closure-proof harness now runs for real against a disposable `mqk-test-postgres` instance (`PRODUCTION-PROMOTION-DB-E2E-01`, `37649200`) — nothing in this chain has been pushed.
- **`RESEARCH_BACKTEST_V1_COMPLETE`:** **LOCALLY COMPLETE — PENDING INDEPENDENT REVIEW.** `BKT-PROMOTION-ARTIFACT-AUTHORITY-01` (`08a292cd`), `PROMOTION-STRESS-SUITE-AUTHORITY-01` (`8bed1b6c`), `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` (`e56f94fb`), `PROMOTION-WALKFORWARD-GATE-WIRING-01` (`f8e9edf4`), P9 (`c66fe32d`), and P10 (`41c19cc7`) are all `CLOSED_LOCAL` as of this same 2026-08-21 wave (`RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01`). This is a local, self-assessed completion, NOT an independent-review acceptance and NOT pushed — see §24's closing summary below for the full commit list, what was and was not verifiable in this environment (no `MQK_DATABASE_URL`), and the explicit distinction from "independently accepted."

**P7C-REPAIR-04 summary (commit `b80749bd`) — implementation-agent evidence, focused-test counts:** fixed a genuine cross-language canonicalization defect — Python `json.dumps` and Rust `serde_json` are not guaranteed to format every float identically (`1e-06` vs `1e-6`), so the prior REPAIR-03 mechanism (Rust rehashing the supplied judge JSON and comparing to the Python-registered hash) could falsely reject a genuinely authoritative artifact. Fixed by durably persisting Python's exact canonical judge text (`canonical_judge_json` column, additive migration) alongside its hash, and having Rust verify per-row integrity against that stored text before doing a same-language (Rust-side) semantic comparison against the supplied artifact. 7 new Rust tests + 5 new Python tests added (exponent-format interoperability, semantic numeric mutation, registry-integrity tampering in both directions, missing canonical text, conflicting/identical re-registration). These focused counts (`cargo test -p mqk-promotion`: 70 passed / 0 failed; targeted `pytest` on 4 files: 93 passed / 0 failed) are implementation-agent evidence from the REPAIR-04 implementation session itself — see the canonical acceptance-boundary validation block below for the totals the independent review actually evaluated.

**Independent review & final acceptance-boundary validation (2026-08-17):** ChatGPT independently reviewed and accepted commits `81dcf621` (P7B-REPAIR-03) and `b80749bd` (P7C-REPAIR-04) by diff inspection — this was a review of the code, not an independent re-run of the test suite. The following are the implementation-agent's full test-suite totals from the completed controller validation report at HEAD `b80749bd`, recorded here as the canonical acceptance-boundary evidence (superseding the narrower focused counts above where they conflict):
- `mqk-promotion`: **101 passed / 0 failed**.
- full `research-py`: **1490 passed / 7 skipped / 0 failed** (+ 12 subtests passed).
- `mqk-backtest`: **265 passed / 0 failed**.
- `mqk-execution`: **108 passed / 0 failed**.

**Status distinction:** Wave 2 (P7B + LONG-SHORT + P7C) is `ACCEPTED_LOCALLY — PUSHED` (confirmed 2026-08-21, `MASTER-LEDGER-REPO-TRUTH-REFRESH-02`: `git merge-base --is-ancestor b80749bd origin/main` succeeds; `origin/main` = `fd90f63a`, a descendant of `b80749bd`). This corrects the prior claim that `origin/main` remained `f8357ebc` — that was stale as of this session.

**Supersedes (mechanism only, not the production-wiring invariant):** the original proposed mechanism in `PROMOTION-WALKFORWARD-GATE-WIRING-01` (Lane B, §5) — that entry's production-wiring gap is now implemented locally but unpushed, and a separate, later independent review of that specific commit (2026-08-21, distinct from this Wave-2 mechanism review) found further gaps; see its own current entry (status `IN PROGRESS / PARTIAL — REPAIR REQUIRED`) and §13.

### P9 — `BKT-ROBUSTNESS-GAUNTLET-01`

**Status:** PARTIAL (2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-AUTHORITY-REPAIR-01`, see update below) · **Priority:** P1 · **Paper Impact:** GREEN (research-only, no execution/portfolio/broker path) · **Subsystem:** mqk-backtest / mqk-artifacts (moved from the originally-listed research-py/mqk-promotion — see resolution note)
**2026-08-22 update:** a later independent review of pushed `fbddeb3d` found this entry's two `DEFERRED` required scenarios (DSR/PBO sensitivity, conservative execution/capacity stress) made the gauntlet genuinely incomplete for promotion-grade use. Both are now implemented for real (`26fe57cb`) and wired into `evaluate_promotion` (`86c61557`, §24); re-closed as of `86c61557`, still not pushed.
**2026-08-22 update 2 (`RESEARCH-BACKTEST-V1-FINAL-AUTHORITY-REPAIR-01`):** a further independent review found `26fe57cb`'s "conservative execution/capacity stress" is a Rust-only halved-config re-run that never exercises P7A/P7B code at all — mislabeled, not genuine P7A/P7B evidence — and that `dsr_pbo_sensitivity_scenario`'s DSR/PBO acceptance ceilings and block-count grid were hidden `pub const`s with no accepted policy source. `265fd63f` closes the hidden-policy-constant defect (both now required, fail-closed-validated parameters/CLI flags, no default). The genuine-P7A/P7B-replay defect is a confirmed **HARD STOP**: the durable Research registry does not retain the per-row bar/signal input authority `_simulate_fold_execution` would need to replay under stressed parameters for an already-completed trial (see §24 for the full investigation). `conservative_capacity_stress` remains in place, unmodified, as real (if differently-scoped) Rust-side evidence — it must not be conflated with P7A/P7B parity evidence. Separately, `db436a44` (`PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01`) now binds this scenario's `dsr_pbo_sensitivity` result to the exact Research `trial_id` it was computed against, durably. **Status: PARTIAL**, not `CLOSED_LOCAL` — see §24.
**Dependencies:** Wave 2 (P7C-REPAIR-04) is independently accepted and **confirmed pushed** to `origin/main` (2026-08-21) — this dependency is met. `PROMOTION-STRESS-SUITE-AUTHORITY-01` (§5, `CLOSED_LOCAL`) supplies the `cost_stress_2x`/`cost_stress_3x` machinery this patch reuses rather than duplicates.
**Required scope (8 items):**
- 2x / 3x cost stress — **IMPLEMENTED**, reused from `PROMOTION-STRESS-SUITE-AUTHORITY-01`, not duplicated.
- execution-delay stress — **IMPLEMENTED**: a strategy-layer `DelayedStrategy` decorator (buffers/re-emits decisions N bars late), deliberately not built by touching the engine's `BKT-FUTURE-EXECUTION-01` eligibility rule.
- symbol leave-one-out — **IMPLEMENTED** for multi-symbol candidates; single-symbol candidates report `applicable: false` honestly.
- month/year/regime concentration — **IMPLEMENTED** (month/year via equity-curve analysis; regime CONTEXT reused from `detect_market_regime`, reported for audit — full regime-bucketed concentration, beyond a single whole-run classification, was judged out of proportionate scope and not attempted).
- parameter-neighborhood execution — **IMPLEMENTED**, reuses the existing `run_sweep` machinery as-is.
- shuffled/random-label placebo — **IMPLEMENTED as a deterministic temporal-offset placebo** (same `DelayedStrategy` wrapper, ~half the run length) — this project avoids RNG everywhere, so a literal random shuffle was judged the wrong primitive; per the hard stop below, a placebo that performs comparably to the real signal is reported as a finding, never tuned away.
- DSR/PBO sensitivity — **DEFERRED, not fabricated.** Requires re-running `mqk_research.ml.multiple_testing_judge` (Python) under perturbation; this Rust wave only ever verifies an already-computed judge artifact (P7C), it has no judge implementation to re-run.
- conservative P7A/P7B execution/capacity stress — **DEFERRED, not fabricated.** P7A/P7B are themselves still `OPEN` (never implemented in any wave) — a stress scenario conditioned on their own machinery cannot be built before they exist.

**Hard stop:** if the placebo (shuffled/random-label) test appears statistically significant, do not tune the gauntlet to make it pass — that is exactly the failure mode this gauntlet exists to catch. Report the finding and stop. **Honored:** the placebo scenario's pass criterion (`placebo_final_equity < baseline_final_equity`) is unconditional; no threshold was tuned to force a pass, and the 2 deferred items above are reported as deferred, not silently marked passing.
**Exact CLOSED End State:** the 6 implementable-in-Rust items produce real, deterministic, durable, candidate-bound, tamper-evident evidence; the 2 genuinely-blocked items are honestly recorded as deferred, never fabricated — met locally, proven by 12 tests (6 core-logic, 6 artifact-authority) including a candidate whose entire result depends on one symbol failing leave-one-out for real (not mocked) and a real sustained trend beating its own temporal-offset placebo.
**Acceptance History:** Implementation DONE locally (`c66fe32d`) / 12/12 focused tests PASSED / Full `mqk-backtest`/`mqk-artifacts`/`mqk-promotion` suites green / Not yet wired into `evaluate_promotion` (a fourth evidence type beyond the three it currently consumes; wiring it in, if ever needed, is future work, not redesigned here) / Independent review PENDING / Push PENDING.

### P10 — `RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01`

**Status:** PARTIAL (2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-AUTHORITY-REPAIR-01`, see update below) · **Priority:** P1 · **Paper Impact:** GREEN · **Subsystem:** mqk-promotion (moved from the originally-listed research-py/mqk-promotion/docs — see resolution note)
**2026-08-22 update:** a later independent review of pushed `fbddeb3d` found this entry's acceptance proof depended on the stress-evidence seam and P9 completeness above (both repaired, `86c61557`), and did not itself exercise the real daemon HTTP promotion route against Postgres. That gap is now closed by `PRODUCTION-PROMOTION-DB-E2E-01` (`37649200`) in `mqk-daemon`'s own test suite; this file's own scope note and `P10-RESEARCH-BACKTEST-FINAL-ACCEPTANCE-REPAIR-01` (`307bac81`, superseding this entry's `RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01` id) now name that proof explicitly rather than duplicating it. Re-closed as of `307bac81` (§24); still not pushed.
**2026-08-22 update 2 (`RESEARCH-BACKTEST-V1-FINAL-AUTHORITY-REPAIR-01`):** the `37649200` DB E2E proof this entry relies on used a DIFFERENT Research trial for P9 sensitivity than for P7C evidence in its own fixtures, undetected — the exact confirmed defect `PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01` closes (`db436a44`). Fixing the gate correctly turned 6 of those DB E2E tests red until the fixtures themselves were repaired to share one real trial/registry (`679f9499`), which also replaced their hand-rolled SQLite registry rows with real `ResearchResultStore` calls and added a real HTTP/Postgres/Python negative control for this exact scenario. Artifact CONTENT (economic/judge JSON) in those fixtures remains schema-accurate hand-constructed content, not a real economic-walkforward run's actual output — REAL-RESEARCH-TO-PROMOTION-E2E-01's "do not hand-write canonical Research result JSON" is satisfied for registry rows, not yet for artifact content. **Status: PARTIAL**, not `CLOSED_LOCAL` — see §24.
**Dependencies:** `PROMOTION-WALKFORWARD-GATE-WIRING-01` `CLOSED_LOCAL` (production wiring proven) **and** P9 `CLOSED_LOCAL` — both met, this same wave.
**Purpose:** compose existing evidence (not re-derive it) into a final Research/Backtest completion record: Git SHA identity, environment/dependency identity, any genuinely still-missing Research CLI entrypoints, and the final `RESEARCH_BACKTEST_V1_COMPLETE` determination.
**Explicit constraint:** P10 does not create a parallel evidence/dossier/registry framework — it composes what P7/P9 already produced using existing seams (the Research SQLite registry, existing artifact hashing/provenance conventions).
**Resolution:** one comprehensive Rust integration test (`mqk-promotion/tests/scenario_research_backtest_promotion_v1_acceptance_01.rs`) proves the load-bearing chain: real `BacktestEngine` run → canonical artifacts (`BKT-PROMOTION-ARTIFACT-AUTHORITY-01`) → real stress suite (`PROMOTION-STRESS-SUITE-AUTHORITY-01`) → real robustness gauntlet (P9) → canonical evidence resolution (`PROMOTION-BACKTEST-EVIDENCE-SEAM-01`) → a real, schema-accurate Research SQLite registry verified via the FROZEN P7C mechanism → canonical `evaluate_promotion` — PASSES for a legitimate candidate; a cross-candidate `strategy_id` pairing is genuinely distinguishable (the exact check the production route independently enforces); and genuinely valid Research evidence does NOT compensate for a real failed stress suite (the "AND of all gates" property, proven against the full real chain, not a synthetic report). **Stated honestly, not proven here:** the actual HTTP transition route against a live Postgres (already unit-tested per-gate by `PROMOTION-WALKFORWARD-GATE-WIRING-01`'s own commit, with its DB-integration gap already documented there); "winner-only registration forbidden" and "final holdout reserved" (research-py/Python invariants, already covered by that codebase's own accepted P6-CLOSURE/P6B test suites — not re-derived from a Rust test, which cannot genuinely test Python code).

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
BKT-PROMOTION-ARTIFACT-AUTHORITY-01  (CLOSED_LOCAL 08a292cd, 2026-08-21)
    |
    v
PROMOTION-STRESS-SUITE-AUTHORITY-01  (CLOSED_LOCAL 8bed1b6c, 2026-08-21)
    |
    v
PROMOTION-BACKTEST-EVIDENCE-SEAM-01  (CLOSED_LOCAL e56f94fb, 2026-08-21)
    |
    v
PROMOTION-WALKFORWARD-GATE-WIRING-01  (production wiring — all 4 independent-
                                        review gaps repaired; CLOSED_LOCAL
                                        f8e9edf4, 2026-08-21; push + DB-backed
                                        harness proof still pending)
    |
    v
P9  BKT-ROBUSTNESS-GAUNTLET-01  (CLOSED_LOCAL c66fe32d, 2026-08-21)
    |
    v
P10  RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01  (CLOSED_LOCAL 41c19cc7, 2026-08-21)
    |
    v
RESEARCH_BACKTEST_V1_COMPLETE  — LOCALLY COMPLETE, PENDING INDEPENDENT REVIEW
                                  AND PUSH (see §24 closing summary below)
```

See §26 for how this chain connects to Operations Resilience and the eventual autonomous Paper soak.

**2026-08-21 closure summary (`RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01`, 6 commits, `08a292cd`..`41c19cc7`, all on local `main`, none pushed):** every entry in the chain above is now `CLOSED_LOCAL — PENDING INDEPENDENT REVIEW`. Full `mqk-artifacts`/`mqk-backtest`/`mqk-promotion` acceptance suites green throughout; full workspace builds and test-compiles clean at every checkpoint; `mqk-daemon --lib` unregressed (793 passed both before and after this wave; the same 31 failures are pre-existing `MQK_DATABASE_URL`-only, unrelated to promotions, and 15 ignored). **What this wave could NOT verify, stated honestly:** the real HTTP `POST /api/v1/strategy/promotions/transition` route against a live Postgres instance — no `MQK_DATABASE_URL` was configured in this environment, and a pre-existing local test-DB migration-checksum drift (documented since the original `242cb7c3` review) independently blocks the two DB-backed integration test files regardless of this wave's changes. This is a REAL, currently-open verification gap, not merely a formality — a future session with real Postgres access must run `scenario_strategy_promotion_routes_01.rs` and `scenario_strategy_promotion_closure_proof_01f.rs` (after adding `backtest_run_id` to their JSON fixtures, since neither currently supplies one) before `RESEARCH_BACKTEST_V1_COMPLETE` can be upgraded past "locally complete." Nothing in this wave was pushed to `origin/main`; per this ledger's own repo-truth rules, `CLOSED_LOCAL` is not equivalent to `CLOSED`, `ACCEPTED_LOCALLY — PUSHED`, or independently accepted.

**Independent review finding (2026-08-21, `RESEARCH-BACKTEST-V1-FINAL-REPAIR-WAVE-01`) — corrects the 2026-08-21 closure summary above:** `origin/main` is confirmed **PUSHED-VERIFIED at `fbddeb3d`** (the closure-wave commits `08a292cd`..`41c19cc7` plus the docs-only closure record `fbddeb3d` are all ancestors of `origin/main`, per repo inspection this session). An independent (ChatGPT) review of that pushed state found a confirmed semantic/protocol-authority gap that the 2026-08-21 closure summary did not catch: `PROMOTION-STRESS-SUITE-AUTHORITY-01`'s durable artifact loader (`load_canonical_stress_suite`) verified schema/hash/candidate identity but never checked `StressSuiteArtifact.protocol_version` against the actual required production stress protocol, and `mqk_promotion::StressSuiteResult` (the type `evaluate_promotion` consumes) carried no protocol identity field at all — `resolve_backtest_evidence`'s bridge (`PROMOTION-BACKTEST-EVIDENCE-SEAM-01`) dropped protocol identity when converting artifact-level evidence to promotion-level evidence. In effect, any structurally-valid stress artifact (regardless of which protocol produced it) could become accepted promotion evidence. This is being repaired this session (`PROMOTION-STRESS-AUTHORITY-REPAIR-01`, see §5) with new commits layered on top of `fbddeb3d` — **not yet independently reviewed, not pushed.**

This finding corrects the following statuses (all as of the pushed `fbddeb3d` state, before this session's repair commits):
- `PROMOTION-STRESS-SUITE-AUTHORITY-01` (§5): **PARTIAL — REPAIR REQUIRED** (was `CLOSED_LOCAL — PENDING INDEPENDENT REVIEW`). The stress-scenario mechanism itself (cost_stress_2x/3x, conservative_risk_limits) is real and correctly implemented; what was missing is protocol-identity enforcement at the loader/evaluator boundary.
- `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` (§5): **PARTIAL** (was `CLOSED_LOCAL — PENDING INDEPENDENT REVIEW`) — the seam resolved `BacktestReport`/`ArtifactLock` correctly but silently dropped stress-protocol identity in its `StressSuiteResult` bridge.
- `PROMOTION-WALKFORWARD-GATE-WIRING-01` (§5): **PARTIAL — REPAIR REQUIRED** (was `CLOSED_LOCAL — PENDING INDEPENDENT REVIEW`) — the production route routes through this same unweakened evidence bridge, so it inherits the gap above.
- P9 `BKT-ROBUSTNESS-GAUNTLET-01` (above): **PARTIAL — required gauntlet incomplete** (was `CLOSED_LOCAL — PENDING INDEPENDENT REVIEW`) — DSR/PBO sensitivity and conservative P7A/P7B execution/capacity stress remain `DEFERRED, not fabricated` per this entry's own recorded scope; a promotion-grade P9 artifact cannot report success while a required slice is deferred without an accepted fail-closed reason, and neither deferral currently has one.
- P10 `RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01` (above): **PARTIAL** (was `CLOSED_LOCAL — PENDING INDEPENDENT REVIEW`) — its acceptance proof depends on the stress-evidence seam and P9 completeness above, and does not itself exercise the real daemon HTTP promotion route against Postgres.
- **`RESEARCH_BACKTEST_V1_COMPLETE`: NOT MET** (was `LOCALLY COMPLETE — PENDING INDEPENDENT REVIEW`).

The Post-V1 Research Capability Backlog immediately below remains correctly gated (`READY` while `RESEARCH_BACKTEST_V1_COMPLETE` is false) and is unaffected — non-blocking, as already stated in its own header.

**Repair wave closure (2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-REPAIR-WAVE-01`, commits `7f8b0cdb`..`86c61557` on top of pushed baseline `fbddeb3d`, none pushed) — repairs the protocol-authority/P9-completeness gaps the finding above found:**
- `7f8b0cdb` (`PROMOTION-STRESS-AUTHORITY-REPAIR-01`): `mqk_promotion::StressSuiteResult` now carries the stress protocol identity through `resolve_backtest_evidence`'s bridge, and the route/`evaluate_promotion` path enforces it against the exact required production protocol — closes the gap in `PROMOTION-STRESS-SUITE-AUTHORITY-01` / `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` / `PROMOTION-WALKFORWARD-GATE-WIRING-01` (§5) the finding above identified.
- `9483e9dc`: promotion evidence lineage is now written durably, atomically with the transition itself (single transaction), not as a best-effort follow-up write.
- `26fe57cb`: P9 `BKT-ROBUSTNESS-GAUNTLET-01`'s two previously-`DEFERRED` required scenarios (DSR/PBO sensitivity, conservative execution/capacity stress) are now implemented for real, closing the "required gauntlet incomplete" gap the finding above found.
- `86c61557`: P9 robustness evidence is wired into the canonical `evaluate_promotion` decision itself, not merely computed and left unconsumed.

Updates the finding's per-entry corrections above: `PROMOTION-STRESS-SUITE-AUTHORITY-01`, `PROMOTION-BACKTEST-EVIDENCE-SEAM-01`, `PROMOTION-WALKFORWARD-GATE-WIRING-01`, and P9 `BKT-ROBUSTNESS-GAUNTLET-01` (all §5/above) are `CLOSED_LOCAL — PENDING INDEPENDENT REVIEW` again as of `86c61557`, not `PARTIAL`. P10 (above) remained `PARTIAL` after this wave — its own acceptance proof still did not exercise the real HTTP route against Postgres, exactly as the finding's P10 correction stated.

**Final production closure (2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-PRODUCTION-CLOSURE-CONTROLLER-01`, commits `4f300a78`, `37649200`, `307bac81`, none pushed) — closes the one gap the repair wave above left open:**
- `4f300a78` (`BKT-PROMOTION-EVIDENCE-PRODUCTION-FINALIZER-01`): the smallest real production workflow generating the complete promotion-grade evidence set (manifest/audit/`backtest_report.json`/`stress_suite.json`/`robustness_gauntlet.json`) for an actual candidate, plus a sanctioned production DSR/PBO-sensitivity finalization path (`mqk backtest finalize-robustness-sensitivity`) that identifies the real Research trial, executes the real cross-language sensitivity evaluation, candidate-binds it, and atomically finalizes the durable artifact — never hand-built fixtures, never a second Research trial manufactured from sensitivity repetitions.
- `37649200` (`PRODUCTION-PROMOTION-DB-E2E-01`): proves one genuine candidate flowing real Research registry/trial → real OOS/economic evidence → real Backtest → the `4f300a78` production finalizer → exact stress protocol → complete P9 → production HTTP transition (`POST /api/v1/strategy/promotions/transition` via `tower::oneshot`, no mock) → canonical `evaluate_promotion` → a real disposable `mqk-test-postgres` instance → atomic evidence lineage, read back and confirmed to identify the exact evidence judged. 11 negative controls (cross-candidate strategy mismatch, missing/incomplete/failed P9, missing stress evidence, artifact tamper, DSR-below/PBO-above threshold, missing `backtest_run_id`, missing artifact-root config, mismatched-lineage retry) all fail closed with no row committed. Also fixed a genuine idempotency gap these controls surfaced: `backtest_run_id` was missing from the deterministic `transition_id` seed, so a retry differing only in `backtest_run_id` was silently accepted as a duplicate of the original request, never re-validating the retry's own claimed evidence (RED/GREEN mutation-proven). Bypass proof: `insert_strategy_promotion_transition_serialized` has exactly one production caller, mounted at exactly one route.
- `307bac81` (`P10-RESEARCH-BACKTEST-FINAL-ACCEPTANCE-REPAIR-01`): retires P10's own honestly-flagged HTTP/Postgres gap by naming where it is now genuinely closed (`37649200`, above) rather than duplicating a second integration harness in `mqk-promotion`; re-verifies the load-bearing `research-py` frozen-contract tests (holdout reservation/consumption, trial-vs-attempt-vs-slice, winner-only-registration, result-value-independent identity) still pass.

`PROMOTION-STRESS-SUITE-AUTHORITY-01`, `PROMOTION-BACKTEST-EVIDENCE-SEAM-01`, `PROMOTION-WALKFORWARD-GATE-WIRING-01`, P9 `BKT-ROBUSTNESS-GAUNTLET-01`, and P10 (all §5/above) are all `CLOSED_LOCAL — PENDING INDEPENDENT REVIEW` as of `307bac81`. **`RESEARCH_BACKTEST_V1_COMPLETE`: LOCALLY COMPLETE — PENDING INDEPENDENT REVIEW** (was `NOT MET`) — see the Current Research/Backtest Verdict at the top of this document for the full statement. Independent (ChatGPT) review of this wave has not occurred; nothing in `7f8b0cdb`..`307bac81` has been pushed — `origin/main` remains at the confirmed-pushed `fbddeb3d`. This closure does not activate the Post-V1 Research Capability Backlog immediately below (still correctly gated `READY`-only, non-blocking) and does not alter Paper/Live status.

**Independent (ChatGPT) review finding, second occurrence (2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-AUTHORITY-REPAIR-01`) — corrects the `307bac81` closure above:** a further independent review of local HEAD `54eee812` (the `307bac81` closure plus its own docs-only ledger record) found four more confirmed deterministic defects, none caught by the `307bac81` closure's own test suite:

1. **Trial-binding gap (`PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01`):** the production finalizer verified only `Research trial strategy_id == BacktestReport.strategy_name` — never that the SAME Research trial produced both the P7C/OOS evidence and the P9 DSR/PBO sensitivity evidence. Two distinct, individually valid trials sharing one `strategy_id` could each supply one half of the required evidence. The `307bac81` wave's own positive DB E2E fixtures (`scenario_strategy_promotion_routes_01.rs`) demonstrated this gap directly: P7C evidence came from `trial_{strategy_id}` in `research_registry.sqlite3`, P9 sensitivity came from a separately-registered `trial_for_{strategy_id}` in a different file, `dsr_registry.sqlite3`.
2. **Fabricated P9 P7A/P7B stress:** `26fe57cb`'s "conservative execution/capacity stress" is a Rust-only re-run of halved exposure/risk-limit config — it never exercises the accepted P7A (`RESEARCH-EXECUTION-PRICING-PARITY-01`) execution-pricing-parity code or P7B (`RESEARCH-WEIGHT-TO-SHARE-PARITY-01`) weight-to-share code at all, despite the commit message implying it satisfied the ledger's own "conservative P7A/P7B execution/capacity stress" required-scope item (line 2513 below).
3. **Hidden DSR/PBO sensitivity policy constants:** `dsr_pbo_sensitivity_scenario` hardcoded a CSCV block-count default (`[8,10,12]`) and DSR/PBO acceptance ceilings (`0.25`/`0.25`) as `pub const`s with no accepted Research/promotion-policy source establishing those numbers.
4. **Thin evidence lineage / legacy-duplicate backfill gap:** migration 0065's durable lineage recorded only identity pointers + judged DSR/PBO, not which judge artifact, stress/robustness protocol, or artifact-content-hash was actually judged, nor a promotion-policy fingerprint — a later transition could not prove the EXACT evidence a historical decision rested on. Separately, `write_evidence_lineage_in_tx`'s "existing lineage is NULL" branch could not distinguish "this row was just inserted" from "this is a duplicate of a historical row that never had lineage," so a duplicate request supplying valid lineage could silently backfill a historical decision that never actually had that evidence.

This finding corrects the following statuses (all as of `54eee812`, before this session's repair patches):
- `PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01` (new): **OPEN — CONFIRMED DEFECT.**
- P9 `BKT-ROBUSTNESS-GAUNTLET-01` (above): **PARTIAL** (was `CLOSED_LOCAL — PENDING INDEPENDENT REVIEW`) — the mechanism is real for 6 of 8 scenarios and DSR/PBO sensitivity's cross-language wiring is real, but the "conservative P7A/P7B execution/capacity stress" item is not genuinely P7A/P7B evidence, and its acceptance thresholds were hidden policy.
- `PROMOTION-EVIDENCE-LINEAGE-V2` / `9483e9dc` (above): **PARTIAL** (was implicitly folded into the `CLOSED_LOCAL` chain) — atomicity was and remains real; lineage completeness and the legacy-duplicate rule were not.
- P10 / `RESEARCH_BACKTEST_V1_COMPLETE`: **NOT MET** (was `LOCALLY COMPLETE — PENDING INDEPENDENT REVIEW`) — the `307bac81` wave's own positive proof exercised the confirmed trial-binding gap without detecting it.

**Repair wave (2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-AUTHORITY-REPAIR-01`, commits `db436a44`, `265fd63f`, `80eb8a5a`, `679f9499` on top of local HEAD `54eee812`, none pushed) — addresses the four findings above. Two are genuinely closed; two are honest partial closures with a named, non-fabricated blocker, per this mission's own explicit "HARD STOP and report the specific missing durable input; do not fabricate it" instruction:**

- **Patch A — `db436a44` (`PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01`): CLOSED.** Persists the exact Research `trial_id` the P9 `dsr_pbo_sensitivity` scenario was computed against (`RobustnessScenarioOutcome::research_trial_id`, carried through the durable artifact and its finalization audit event), exposes it via `RobustnessGauntletArtifact::dsr_pbo_sensitivity_research_trial_id()` / `RobustnessEvidence::dsr_pbo_sensitivity_research_trial_id`, and adds a new `evaluate_promotion` gate requiring it to equal the verified P7C/OOS trial_id — missing or mismatched binding fails closed. Also closed a related finalize-idempotency gap (two finalizations with identical scenario content but different trial_ids are now correctly treated as conflicting, not a silent replay). Negative/positive controls (`p10d`/`p10e` in `mqk-promotion`'s `scenario_research_backtest_promotion_v1_acceptance_01.rs`) prove two real, independently-registered trials under the SAME strategy_id can never both contribute evidence unless they are the literal same trial. `mqk-promotion`/`mqk-backtest`/`mqk-artifacts` suites green.
- **Patch B — `265fd63f` (`P9-P7A-P7B-REAL-STRESS-01`): PARTIAL — HARD STOP on the P7A/P7B replay half, CLOSED on the policy half.** Removed the hidden `DSR_MAX_SENSITIVITY_RANGE`/`PBO_MAX_SENSITIVITY_RANGE`/`DEFAULT_BLOCK_COUNTS` constants; `dsr_max_sensitivity_range`/`pbo_max_sensitivity_range` are now required parameters (fail-closed validated) and required CLI flags with no default, so an operator must supply an explicit value every invocation — closes finding 3. Finding 2 (genuine P7A/P7B execution/capacity stress) is a confirmed **HARD STOP**: targeted investigation of `research-py`'s `economic_walkforward.py`/`exp_distributed/storage.py` found the durable Research registry does not retain the per-row bar OHLC or per-row signal-weight data `_simulate_fold_execution` would need to replay a fold under stressed P7A/P7B parameters for an already-completed trial — only aggregate/derived outputs (`economic_daily_returns.csv`) and hash+path pointers (never content) are durably kept. Per this mission's explicit instruction, this is reported, not fabricated; the existing `conservative_capacity_stress` Rust-only scenario is left in place unmodified (removing it would silently drop P9 evidence coverage) but must not be conflated with genuine P7A/P7B parity evidence.
- **Patch C — `80eb8a5a` (`PROMOTION-EVIDENCE-LINEAGE-V3`): CLOSED.** Migration 0066 adds `research_judge_artifact_sha256`, `stress_protocol_version`, `stress_artifact_sha256`, `robustness_protocol_version`, `finalized_robustness_artifact_sha256`, `promotion_policy_fingerprint` — every hash reused from an existing accepted audit hash already verified elsewhere in the chain (`VerifiedResearchAuthority::judge_artifact_sha256`, and new `StressSuiteArtifact`/`RobustnessGauntletArtifact::content_sha256()` methods that bit-for-bit reproduce the `stress_suite_sha256`/`robustness_gauntlet_sha256` audit hashes their loaders already verify) except `promotion_policy_fingerprint` (genuinely new — `PromotionConfig` was never durably hashed before; follows the existing `evidence_fingerprint_v2` canonical-bytes-then-SHA-256 pattern). `write_evidence_lineage_in_tx` split into `write_evidence_lineage_for_fresh_insert_in_tx` (always writes; row was just created this same transaction) and `verify_evidence_lineage_for_duplicate_in_tx` (never writes; a historical NULL-lineage row now fails closed with `Err` instead of being backfilled) — closes the legacy-duplicate-backfill gap. Required negative control (`lineage_atomicity_duplicate_of_null_lineage_row_refuses_backfill`) proven against real disposable Postgres (port 5434); full `mqk-db` strategy-promotion-registry suite (31/31) green.
- **Patch D — `679f9499` (`REAL-RESEARCH-TO-PROMOTION-E2E-01`): PARTIAL.** Patch A's new gate correctly turned 6 previously-green DB E2E tests red (`400` instead of `200`) by exposing the exact confirmed trial-binding gap in their own fixtures — fixed by threading one real `research_trial_id`/registry through both `write_research_evidence_fixture` (P7C) and `write_real_backtest_evidence` (P9), which in turn surfaced a second real defect: the hand-rolled `rusqlite` registry schema (`trial_id`/`experiment_id`/`hypothesis_id`/`strategy_id` columns only) is missing `protocol_id` and other columns the REAL `ResearchResultStore` schema has, so the real `dsr_pbo_sensitivity_cli.py` subprocess failed once both P7C and P9 pointed at one shared registry. Replaced the hand-rolled schema with real `ResearchResultStore.register_hypothesis`/`register_trial`/`begin_attempt`/`finalize_attempt`/`register_judge_artifact` calls (real Python subprocess) — registry rows are now genuinely produced by production code, never hand-inserted to satisfy a verifier. Added the required real HTTP/Postgres/Python negative control (`same_strategy_different_research_trial_for_p9_vs_p7c_is_rejected`): two independently-registered real trials under the SAME strategy_id, one supplying P7C, the other P9 — rejected with "Research trial binding mismatch". All 29 tests across both DB E2E files (`scenario_strategy_promotion_routes_01.rs`, `scenario_strategy_promotion_closure_proof_01f.rs`) pass against real disposable Postgres and real Python. **Remaining gap, stated honestly:** the economic/judge artifact JSON *content* (`economic_walk_forward.json`, `economic_daily_returns.csv`, the judge JSON) is still schema-accurate hand-constructed content, not the literal output of a real `run_registered_economic_walkforward_eval` + `build_multiple_testing_judge` run — the mission's "do not hand-write canonical Research result JSON" instruction is satisfied for registry ROWS, not yet for artifact FILE CONTENT. A follow-up patch could close this (feasibility confirmed: `research-py/tests/test_multiple_testing_judge.py::test_real_pipeline_integration` already exercises the real end-to-end chain in ~9s from a small synthetic dataset, with no external infra beyond local Python + SQLite + a CSV) but was not attempted this session.

**`RESEARCH_BACKTEST_V1_COMPLETE`: NOT MET / PARTIAL** (was `LOCALLY COMPLETE — PENDING INDEPENDENT REVIEW`) — per this ledger's own rule and this mission's explicit instruction, only mark `LOCALLY COMPLETE` when every confirmed defect is actually closed; Patch B and Patch D each have a real, honestly-reported remaining gap. Independent (ChatGPT) review of `db436a44`..`679f9499` has not occurred; nothing in this repair wave has been pushed — `origin/main` remains at the confirmed-pushed `fbddeb3d`. Does not activate the Post-V1 Research Capability Backlog immediately below (still correctly gated `READY`-only, non-blocking) and does not alter Paper/Live status.

**Closure wave (2026-08-22, `RESEARCH-BACKTEST-V1-FINAL-P7A-P7B-REPLAY-CLOSURE-01`, commit `930d60c1` on top of local HEAD `a185aa00`, not pushed) — closes both gaps Patch B and Patch D above left open, per the mission's own correction of the prior HARD STOP:**

**Correction of Patch B's premise:** the prior HARD STOP asserted the durable Research registry retains no per-row data a P7A/P7B replay would need. Direct inspection of `economic_walkforward.py` (lines ~1855-1878) found this incomplete: `run_registered_economic_walkforward_eval`'s output already carries an `inputs` section — `bars_csv`/`oos_predictions_csv`/`walk_forward_eval`, each a `file_record()` (`{path, bytes, sha256}`) — durably recorded for every successful attempt and retrievable via `ResearchResultStore.list_attempts(...)[-1]["artifact_paths_json"]["economic_walk_forward"]`. No new bars database, no archived features/targets, no model retraining, and no new replay framework were needed; only a thin wrapper around the already-accepted `run_economic_walkforward` entry point.

- **Patch G-REPAIR — `930d60c1` (`P7A-P7B-ECONOMIC-REPLAY-STRESS-01`): CLOSED.** New `research-py/src/mqk_research/ml/p7a_p7b_economic_replay_stress_cli.py`: resolves trial T's successful attempt's `economic_walk_forward.json`, re-verifies every recorded input's path/byte-count/sha256 (fail closed on any mismatch, never refetch-and-assume-identical), reconstructs the exact baseline `EconomicWalkForwardSpec`, requires it used the official P7A (`rust_conservative_bar_range_v1`) execution-pricing model and official P7B (`weight_to_share_v1`) protocol (fails closed / `not_evaluable` otherwise, never fabricated evidence for a non-qualifying baseline), builds a stressed spec overriding only the accepted P7A/P7B stress knobs (execution-pricing slippage/volatility, weight-to-share capacity caps — no fake liquidity/ADV impact, that remains `BKT-LIQUIDITY-IMPACT-CAPACITY-01`, post-V1), and re-runs the real `run_economic_walkforward` against the SAME verified bars/OOS/walk-forward-eval files into a fresh output directory — the trial's model output is FROZEN, never retrained, never re-read from features/targets. New Rust `mqk-backtest::p7a_p7b_economic_replay_stress_scenario` (mirrors `dsr_pbo_sensitivity_scenario`'s pattern exactly, fails closed on a non-finite/out-of-range required `max_drawdown_ceiling` with no hidden default) wired into `REQUIRED_ROBUSTNESS_SCENARIO_NAMES` (now 8 scenarios); `RobustnessGauntletArtifact`'s sensitivity-merge finalization generalized to accept either deferred scenario by name; new `p7a_p7b_economic_replay_stress_research_trial_id` accessor/field threaded through `mqk-artifacts` → `mqk-promotion::RobustnessEvidence` → a new `evaluate_promotion` gate requiring it to equal the verified P7C/OOS trial_id, independently of (and proven not to piggyback on) the existing `dsr_pbo_sensitivity` binding gate. 13 `research-py` tests (`tests/test_p7a_p7b_economic_replay_stress.py`) prove: genuine positive replay-and-pass; no new trial registered; stressed artifact still reports holdout `reserved_not_evaluated`; bars/OOS/walk-forward-eval tamper (byte-count and same-length content mutation) each fail closed; a recorded input file deleted (not merely mutated) fails closed via a distinct code path; a tampered `bars_provenance` block (bars file itself untouched) fails closed via the downstream `require_bars_pricing_provenance` content check; an unknown `trial_id` fails closed; a non-official baseline P7A or P7B is `not_evaluable`; two independently-registered trials each produce their own non-interchangeable stress result; and every mission-required durable-evidence field (trial_id, baseline economic-eval id, bars/OOS/walk-forward SHA-256, bars-provenance hash, stress-spec identity, stressed-artifact SHA-256) is present and self-consistent. 2 new `mqk-promotion` tests isolate the new trial-binding gate from the pre-existing `dsr_pbo_sensitivity` one (mismatch/missing-binding fails even when `dsr_pbo_sensitivity` correctly matches). **Mutation-proofed:** temporarily disabling the CLI's SHA-256 input check let a same-byte-length OOS-predictions tamper pass silently as `status: evaluated, passed: true` (RED — proves the check is load-bearing, not redundant with the independent bars-provenance content check); restoring it caught the tamper again (GREEN). Full `mqk-artifacts`/`mqk-backtest`/`mqk-promotion` suites green; both DB-backed `mqk-daemon` integration files re-run against real disposable Postgres with the new gate active — `scenario_strategy_promotion_routes_01.rs` 30/30 (including the strengthened `real_research_production_trial_used_for_both_p7c_and_p9_passes`, which now asserts the new scenario was genuinely `applicable: true, passed: true`, not fast-pathed as inapplicable), `scenario_strategy_promotion_closure_proof_01f.rs` 1/1.

**Confirmation Patch D's artifact-content gap was already closed:** `scenario_strategy_promotion_routes_01.rs`'s real-production-pipeline tests already invoke `mqk_research.ml.real_research_promotion_e2e_cli` as a real Python subprocess (not hand-written canonical JSON) for both trials' economic/judge artifact content — confirmed by direct inspection this session, no new code needed for this half.

**`RESEARCH_BACKTEST_V1_COMPLETE`: LOCALLY COMPLETE — PENDING INDEPENDENT REVIEW** (was `NOT MET / PARTIAL`) — both gaps the `RESEARCH-BACKTEST-V1-FINAL-AUTHORITY-REPAIR-01` finding left open are now closed with committed code and passing tests. Independent (ChatGPT) review of `930d60c1` has not occurred; nothing in this closure wave has been pushed — `origin/main` remains at the confirmed-pushed `fbddeb3d`. Does not activate the Post-V1 Research Capability Backlog immediately below (still correctly gated `READY`-only, non-blocking) and does not alter Paper/Live status.

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

*Added by `MASTER-LEDGER-CONSOLIDATION-01`, 2026-08-17; corrected by `MASTER-LEDGER-TRUTH-REPAIR-01`, 2026-08-17 (inserted the production-wiring step below, which the consolidation pass omitted); repo truth refreshed by `MASTER-LEDGER-REPO-TRUTH-REFRESH-02`, 2026-08-21 (Wave 2 push confirmed done; production wiring confirmed implemented locally, unpushed); further corrected by `MASTER-LEDGER-PROMOTION-REVIEW-TRUTH-REPAIR-01`, 2026-08-21 (independent review of the production-wiring commit found material gaps; inserted the new `PROMOTION-BACKTEST-EVIDENCE-SEAM-01` prerequisite step below); closed locally by `RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01`, 2026-08-21 (all steps through `RESEARCH_BACKTEST_V1_COMPLETE` now `CLOSED_LOCAL`); reopened by a later independent (ChatGPT) review of pushed baseline `fbddeb3d` (protocol-authority + P9-completeness gaps, see §24); repaired and re-closed by `RESEARCH-BACKTEST-V1-FINAL-REPAIR-WAVE-01` and `RESEARCH-BACKTEST-V1-FINAL-PRODUCTION-CLOSURE-CONTROLLER-01`, 2026-08-22 (the DB-backed route verification gap is now closed too — see §5/§24 for the full per-entry resolution). This chain is independent of — and must not preempt — the ongoing Lane A-F equity/Paper program (§5-§14), unless current repo truth shows a direct dependency. In particular, broad multi-asset work, cosmetic GUI work, unnecessary infrastructure, or strategy proliferation must not preempt this path. The Post-V1 Research Capability Backlog in §24 (added 2026-08-21, `RESEARCH-VIBE-GAP-BACKLOG-01`) becomes eligible only after `RESEARCH_BACKTEST_V1_COMPLETE` and is explicitly non-blocking here — it must not preempt this chain or the autonomous Paper path below.*

```text
CURRENT (as of 2026-08-22)
Wave 2 (P7A+P7B+P7C) independent review — DONE, ACCEPTED LOCALLY (81dcf621, b80749bd)
        |
        v
Wave 2 acceptance + push to origin/main  (CONFIRMED DONE 2026-08-21; origin/main
                                           remains at fbddeb3d, the last pushed SHA)
        |
        v
BKT-PROMOTION-ARTIFACT-AUTHORITY-01 -> PROMOTION-STRESS-SUITE-AUTHORITY-01 ->
PROMOTION-BACKTEST-EVIDENCE-SEAM-01 -> PROMOTION-WALKFORWARD-GATE-WIRING-01
        (production wiring; independent review of pushed fbddeb3d found a
         protocol-authority + P9-completeness gap; repaired by
         RESEARCH-BACKTEST-V1-FINAL-REPAIR-WAVE-01, CLOSED_LOCAL again as of
         86c61557, 2026-08-22; still unpushed, see §5/§24)
        |
        v
P9  BKT-ROBUSTNESS-GAUNTLET-01  (CLOSED_LOCAL 86c61557 -- both previously-
                                  deferred required scenarios now real)
        |
        v
P10  RESEARCH-BACKTEST-FINAL-ACCEPTANCE-REPAIR-01  (CLOSED_LOCAL 307bac81 --
      now backed by a real HTTP-route + live-Postgres E2E proof,
      PRODUCTION-PROMOTION-DB-E2E-01, 37649200, see §5/§24 -- BUT this
      wave's own positive proof used a DIFFERENT Research trial for P9
      than for P7C, undetected; see the RESEARCH-BACKTEST-V1-FINAL-
      AUTHORITY-REPAIR-01 finding below)
        |
        v
PROMOTION-RESEARCH-BACKTEST-TRIAL-BINDING-01 (CLOSED db436a44) +
PROMOTION-EVIDENCE-LINEAGE-V3 (CLOSED 80eb8a5a) +
REAL-RESEARCH-TO-PROMOTION-E2E-01 fixture/registry fix (679f9499, PARTIAL) +
P9-P7A-P7B-REAL-STRESS-01 policy fix (265fd63f, PARTIAL -- P7A/P7B replay
      itself HARD STOPPED, missing durable input, see §24)
        |
        v
RESEARCH_BACKTEST_V1_COMPLETE  — was NOT MET / PARTIAL (2026-08-22,
      RESEARCH-BACKTEST-V1-FINAL-AUTHORITY-REPAIR-01) -- two named,
      non-fabricated gaps
        |
        v
P7A-P7B-ECONOMIC-REPLAY-STRESS-01 (CLOSED 930d60c1) -- real replay of
      frozen OOS predictions through official P7A/P7B under an explicit
      stress config, closing the P7A/P7B replay gap; artifact-content
      gap confirmed already closed by prior REAL-RESEARCH-TO-PROMOTION-
      E2E-01 work; see §24 closing summary
        |
        v
RESEARCH_BACKTEST_V1_COMPLETE  — LOCALLY COMPLETE, PENDING INDEPENDENT
      REVIEW (2026-08-22, RESEARCH-BACKTEST-V1-FINAL-P7A-P7B-REPLAY-
      CLOSURE-01, 930d60c1; not pushed -- origin/main remains fbddeb3d)
        |
        v
FINAL-P7A-P7B-REPLAY-AUTHORITY-01 (CLOSED dba91f44) +
FINAL-P9-ROBUSTNESS-SEMANTICS-01 (CLOSED 975e952a, ede3a0b6) -- mandatory-
      means-mandatory, exact economic_eval_id binding + durable content-hash
      authentication, genuine P7A/P7B adversity validation, DSR distinct
      grid, genuine shuffled placebo, month+year+regime concentration,
      edge-collapse semantics, protocol bump to bkt_robustness_gauntlet_v2;
      see §0 Current Research/Backtest Verdict for full detail
        |
        v
Patch 3 acceptance run against a REAL disposable Postgres (mqk-test-postgres,
      port 5434, confirmed live and used directly -- the first session to do
      so): mqk-promotion real P10 chain full green; mqk-daemon DB/HTTP
      suites 26/33 + 0/1 -- every failure traced to one of two known,
      non-code-defect causes (lightweight Research-evidence fixture now
      correctly fails mandatory scenarios; shared smooth_uptrend_bars
      fixture is genuinely regime-concentrated), neither repaired this
      session (test-fixture debt, not production defect); see §0 for detail
        |
        v
RESEARCH_BACKTEST_V1_COMPLETE  — NOT MET (2026-08-22,
      FINAL-RESEARCH-BACKTEST-V1-CLOSURE-CONTROLLER-01) -- exact blocker:
      mqk-daemon's own DB/HTTP integration-test fixtures (smooth_uptrend_bars,
      the lightweight hand-registered Research-evidence fixture) need
      recalibration/migration against the new, correct P9 semantics; not a
      production code defect
        |
        v
FINAL-P10-FIXTURE-REALISM-01 (e19602ff) -- fixture-only closeout: migrated
      6 daemon-routes-file tests + closure_proof_01f.rs's own test to
      write_real_research_evidence_via_production_pipeline; replaced
      smooth_uptrend_bars (both files) with a genuinely multi-regime,
      non-concentrated bars fixture verified against the real regime
      classifier; fixed closure_proof_01f.rs's own pre-existing P7B
      max_target_qty=None fixture bug. Zero production code changes.
        |
        v
RESEARCH_BACKTEST_V1_COMPLETE  — LOCALLY COMPLETE, PENDING INDEPENDENT
      REVIEW (2026-08-22, FINAL-P10-FIXTURE-REALISM-01, e19602ff) --
      scenario_strategy_promotion_routes_01.rs 33/33 pass,
      scenario_strategy_promotion_closure_proof_01f.rs 1/1 pass, both
      against real disposable Postgres (mqk-test-postgres, port 5434);
      not pushed -- origin/main remains fbddeb3d
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

This chain is separate from, and does not gate, Research/Backtest or OPS-* status — those remain independent tracks either way. **Corrected 2026-08-30 (`MASTER-AUDIT-TRUTH-CORRECTION-01`):** the parenthetical this sentence originally cited (`PAPER_SOAK_GO`, §0/§1) is superseded — see "Current Paper/Risk Truth Correction" in §0. Current Paper status is BLOCKED FOR COUNTABLE PAPER SOAK / FORWARD VALIDATION pending the runtime-risk dynamic-state-authority repair; no supervised Paper soak session should be treated as countable until that repair and its post-repair validation session (situational audit §D/§I) are complete.

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

## 28. Post-Soak Blockers Recorded During Ledger-Closure Consolidation

*Added by `LEDGER-CLOSURE-CONSOLIDATION-01-CONTROLLER`, 2026-08-24. Recorded from
preserved 2026-08-24 forensic evidence per the controlling mission. **Not
fixed in this session** — recording only, per mission scope. See
`docs/audits/2026-08-24_branch_worktree_consolidation_audit.md` for the
accompanying branch/worktree inventory from the same controller.*

### DATA-READINESS-BAR-COVERAGE-AUTHORITY-01 — STATUS=CLOSED — INDEPENDENTLY ACCEPTED

*Investigated and repaired by `PAPER-BACKEND-LEDGER-CLOSURE-WAVE-01-
CONTROLLER`, 2026-08-25, on temporary worktree `AppData/Local/Temp/
MiniQuantDeskV4-paper-backend-wave-01`, branch `ledger-paper-backend-
wave-01`, base `dc398721` (identical to the primary repo's accepted HEAD at
wave start). Independent review of that controller's own first production
commit (`a511ab4c`) found one scope regression, corrected below by
`PAPER-BACKEND-LEDGER-WAVE-01-INDEPENDENT-REVIEW-REPAIR-01`. Per
`audit_repo_truth_rules.md`, this remained `LOCALLY COMPLETE — PENDING
INDEPENDENT REVIEW`, not `CLOSED`, until an independent reviewer accepted
the repair below — that acceptance is now recorded; see §33.*

Observed truth (2026-08-24 forensic evidence, unchanged from the original
recording below): 284 complete AAPL 5m Alpaca bars existed in storage;
`bars_observed=0`, `bars_dispatched=0`, `strategy_evaluation_count=0`; bar
coverage authority reported `latest_completed_bar_pending` /
`unknown_incomplete_bar_coverage` despite complete bars on disk.

**Root cause, precisely stated (not bar-coverage/eligibility/provenance —
the readiness gate itself worked correctly on 2026-08-24, confirmed by
`market_open/phase3_running_transition.json`'s clean `preflight_blocked ->
running` self-recovery once real bars caught up):**
`fetch_relevant_open_autonomous_daily_operation` (`core-rs/crates/mqk-db/
src/autonomous_daily_operation.rs`) failed closed every tick with
`"4 equally authoritative active operations found"` because three prior-day
`sys_autonomous_daily_operations` rows (2026-08-17 `evidence_degraded`,
2026-08-18 `stopping`, 2026-08-19 `running`) had never reached a terminal
state and stayed unconditionally relevant alongside 2026-08-24's own fresh
row — so the completed-bar driver task could never determine which
operation owned lifecycle authority, and every bar tick failed before a
single bar could be credited (`diagnostics/CONFIRMED_BLOCKER_strategy_
invocation_stale_operations.json`).

Two of the three conflicting states (`stopping`, `evidence_degraded`, plus
`stop_retrying`) were already given an evidence-gated release clause by the
already-integrated `paper-soak-session-1-repair` chain (§29) and the
`AUTONOMOUS-DAILY-STOPPING-EVIDENCE-DEGRADED-OSCILLATION-01` close-priority
fix (above) — proven by 27/27 `relevant_open_lookup_*` tests passing clean
against `dc398721` at the start of this wave, before any new production
change. The third state, `running` (and its sibling `recovery_retrying`,
reached via the identical code path), had **no release clause at all** —
unconditionally relevant forever, identical to the pre-repair shape of the
other three states.

**This was not merely a historical 2026-08-24 artifact — it was live and
reproducible in the current Paper DB at wave start**, verified read-only
(zero mutation) against `mqk-paper-postgres`: operation `9cf72cd5`
(`market_date=2026-08-19`, `state=running`, `run_id=254e89c3`) had its bound
run durably `status='STOPPED'` with zero unacked `oms_outbox` rows, zero
unapplied `oms_inbox` rows, and a clean `sys_reconcile_status_state` since
`2026-08-24T12:03:46Z` — i.e. proven safe to release by the exact evidence
standard already coded for the other three states — yet remained
unconditionally relevant, one tick away from reproducing the identical
`"N equally authoritative operations"` fail-closed block on the very next
Paper session (2026-08-24's own operation, `40e97f92`, is independently and
correctly still blocking on a genuine `HALTED` run needing operator
attention — a second, unrelated, correctly-fail-closed row that would have
collided with `9cf72cd5` the moment both were evaluated together).

**Production patch**: extended the existing run-level safe-terminal evidence
proof (`runs.status = 'STOPPED'`, zero unacked outbox, zero unapplied inbox,
clean global reconcile) to gate `running`/`recovery_retrying`'s previously-
unconditional relevance, and to the REPAIR-2 bound-run-without-stop-evidence
fallback clause (which independently kept the same row relevant regardless
of state). `controller_degraded` is unchanged and remains unconditionally
relevant (a control-plane/process problem with no run-level evidence to
gate on).

**RED/GREEN proof**: `relevant_open_lookup_stale_running_row_with_safely_
terminal_run_is_released` fails against the pre-patch query with the exact
production error string (`"2 equally authoritative active operations
found"`), reproducing the live defect deterministically; GREEN after the
fix. Negative controls added: `relevant_open_lookup_running_row_with_
active_run_still_blocks` (a genuinely still-active run must never release)
and `relevant_open_lookup_running_row_with_stopped_run_but_unacked_outbox_
still_blocks` (a stopped run with unresolved economic evidence must never
release). All 59/59 `scenario_autonomous_daily_operation_lifecycle_01`
tests pass (disposable `mqk_test`, port 5434, single-threaded to avoid the
shared `sys_reconcile_status_state` singleton race across parallel tests in
the same file — a pre-existing test-isolation property of that table, not a
production defect).

**Read-only re-verification against the live Paper DB after the fix**
(query logic manually re-evaluated against current row state, zero
mutation): with `running`/`recovery_retrying` evidence-gated, `9cf72cd5`
(2026-08-19) now correctly releases; `6aaa0349` (2026-08-18, `stopping`) and
`6e4606e8` (2026-08-17, `evidence_degraded`) already released cleanly
before this patch; `40e97f92` (2026-08-24) correctly remains the sole
relevant row, `HALTED`, requiring operator manual intervention — not a code
defect. **Operational follow-up for the operator (not a code patch):**
`40e97f92`'s `HALTED` run still needs an explicit operator decision before
the next Paper session's coordinator can create a fresh day's operation;
this is intentional fail-closed behavior per `broker_rules.md`/`CLAUDE.md`
§4, not something this wave is authorized to clear.

**Closure bar met**: durable valid complete bar -> readiness eligibility ->
runtime observation -> dispatch -> strategy evaluation, proven by the
pre-existing, unmodified `mqk-daemon`
`m01_task_level_prepare_to_running_exactly_once` end-to-end test (real
seeded `md_bars`, mock Alpaca server, real coordinator/driver/task code,
zero shadow implementation): bar observed exactly once, dispatched exactly
once, strategy evaluated exactly once, idempotent repeat, zero side
effects. Negative controls `readiness_22_30_not_ready_blocks_regardless_
of_reason` and `observation_34_db_evidence_failure_prevents_dispatch`
(pre-existing, unmodified) confirm an invalid/not-ready bar still fails
closed. All pass unchanged against this wave's fix.

Focused tests: `scenario_autonomous_daily_operation_lifecycle_01` (59/59),
`scenario_autonomous_completed_bar_task_01::m01_task_level_prepare_to_
running_exactly_once` and `::f01_no_relevant_operation_creates_nothing_
zero_db_writes_to_operations_table`, `scenario_autonomous_completed_bar_
driver_01::readiness_22_30_not_ready_blocks_regardless_of_reason` and
`::observation_34_db_evidence_failure_prevents_dispatch`. All green.
`cargo check -p mqk-db` and `cargo check -p mqk-daemon` both clean, zero
warnings. `git diff --check` clean.

Production commit: `a511ab4c` ("fix: release crash-orphaned running/
recovery_retrying operations by run evidence") — original implementation,
independently reviewed below.

#### Independent review finding and repair — `PAPER-BACKEND-LEDGER-WAVE-01-INDEPENDENT-REVIEW-REPAIR-01`

Independent review found `a511ab4c`'s run-evidence-gated carve-out had
accidentally over-broadened REPAIR-2's generic bound-run-without-stop-
evidence fallback clause (`run_id is not null and stopped_at_utc is null`)
to *every* lifecycle state reaching it, not just the `running`/
`recovery_retrying` pair the patch was scoped to. Concretely: a
`manual_intervention_required` row whose bound `run.status` happened to be
durably `'STOPPED'` (a distinct authority from
`operation.stopped_at_utc is not null` — REPAIR-2's actual durable-stop-
evidence contract) could silently lose relevance despite the operation
itself never recording durable stop completion. `run.status = 'STOPPED'`
and `operation.stopped_at_utc is not null` must never be conflated.

**Repair** (`core-rs/crates/mqk-db/src/autonomous_daily_operation.rs`,
`fetch_relevant_open_autonomous_daily_operation`): the fallback clause now
reads `run_id is not null and stopped_at_utc is null and (state not in
(running, recovery_retrying) or not RUN_SAFELY_TERMINAL)` — the
run-evidence carve-out applies only to the two states it was designed for;
every other state keeps the exact pre-`a511ab4c` REPAIR-2 guarantee
unconditionally.

**RED/GREEN proof**: `relevant_open_lookup_manual_intervention_row_with_
safely_terminal_run_but_no_operation_stop_evidence_remains_relevant`
(new) fails against `a511ab4c` (`left: None, right:
Some(operation_id)`), reproducing the over-broadening exactly; passes
after the repair. A second-relevant-row control in the same test proves
the lookup still fails closed as ambiguous when two rows are genuinely
relevant simultaneously. Three new tests
(`relevant_open_lookup_stale_recovery_retrying_row_with_safely_terminal_
run_is_released`, `relevant_open_lookup_recovery_retrying_row_with_
active_run_still_blocks`, `relevant_open_lookup_recovery_retrying_row_
with_stopped_run_but_unacked_outbox_still_blocks`) close a coverage gap
`a511ab4c` itself left: it directly proved its new evidence-gated clause
only for `running`, never for its sibling `recovery_retrying`, even though
both share the identical query clause. All 63/63
`scenario_autonomous_daily_operation_lifecycle_01` tests pass.

**Integrated data-path closure proof** (new,
`core-rs/crates/mqk-daemon/tests/scenario_autonomous_daily_phase_d_
integration_01.rs`): two tests combine a crash-orphaned prior-day `running`
operation (bound run durably `STOPPED`, zero unresolved outbox/inbox
evidence, clean reconcile, outside its own window — seeded directly via
`mqk_db::insert_run`/`arm_run`/`begin_run`/`stop_run` and the same
transition-CAS chain the mqk-db fixtures use) with the real, unmodified
production chain `phase_d_full_day_lifecycle` already exercises
(`tick_autonomous_daily_coordinator` -> `tick_autonomous_completed_bar_
driver_from_state`), for the exact same `(deployment_mode="PAPER",
adapter_id="alpaca")` slot family the production lookup filters on
(reuses this file's existing synthetic-symbol/mock-Alpaca-server fixture
rather than a real network call, per this file's own "no real provider,
broker, or network call" module contract — the invariant under proof,
`fetch_relevant_open_autonomous_daily_operation`'s adapter/deployment-mode
scoping, never reads symbol identity at all):
- `phase_d_integrated_stale_running_row_releases_and_bar_chain_completes`:
  the stale row does not create authority ambiguity; today's operation is
  selected; `bars_observed` increments exactly once and `bars_dispatched`
  increments exactly once for the dispatched bar; exactly one strategy
  evaluation is recorded; a repeated tick is idempotent (zero duplication
  of any of the three counters).
- `phase_d_integrated_stale_running_row_with_active_run_fails_closed`
  (integrated negative control): mutating only the stale row so its bound
  run stays genuinely active (never calling `stop_run`) reproduces the
  original defect's exact symptom — the coordinator still reaches
  `running` for today's operation, but the completed-bar driver's
  authority lookup fails closed with an ambiguity error at both the
  preopen and running-dispatch stages; zero bars are dispatched
  (`sum(bars_dispatched)` across the adapter slot stays `0`) and zero
  strategy evaluations occur, proving the positive proof above is actually
  exercising the new evidence-gated release rather than a fixture that
  always succeeds.

All 10/10 `scenario_autonomous_daily_phase_d_integration_01` tests pass
(including the pre-existing, unmodified `phase_d_full_day_lifecycle`), and
all 49/49 `scenario_autonomous_completed_bar_task_01` +
`scenario_autonomous_completed_bar_driver_01` tests pass unchanged.
`cargo check -p mqk-db` and `cargo check -p mqk-daemon` both clean, zero
warnings. `git diff --check` clean.

Production/test commit: `5346f90a2233b8cdf8ed1ff5f82f2aea974421d4` ("fix:
scope stale running release to active states"), on
`ledger-paper-backend-wave-01`, NOT merged into
`ledger-closure-integration-01`, NOT pushed — pending independent review.

### AUTONOMOUS-DAILY-STOPPING-EVIDENCE-DEGRADED-OSCILLATION-01 — STATUS=CLOSED

*Closed by `AUTONOMOUS-DAILY-STOPPED-EVIDENCE-DEGRADED-CLOSE-PRIORITY-
UNIFICATION-01-CONTROLLER`, 2026-08-24 (third pass, same day), on temporary
worktree `AppData/Local/Temp/MiniQuantDeskV4-oscillation-unification`,
branch `ledger-oscillation-close-priority-01`, base
`4248bdb4a499460de136864d07972a5c8cd2a059`. Supersedes the
`LIKELY_SAME_FAMILY_BUT_NOT_PROVEN` / `OPEN` classification below this
heading previously recorded by `LEDGER-CLOSURE-PAPER-REPAIR-INTEGRATION-01-
CONTROLLER`.*

Root cause, precisely stated: not a single dispatcher bug, but a **family of
independently-implemented close-priority gates that each duplicated the same
`now_utc >= effective_operation_close_utc` check without a shared exemption
predicate**. `dispatch_by_state`'s D2.17 gate was repaired first (prior
session) and proven against the exact 2026-08-24 forensic shape, but three
sibling gates in the same file dispatch into the same
`handle_session_close` target via independently-written close-priority
checks, and none of the three inherited D2.17's exemption merely by having
it exist elsewhere in the file.

**Phase 0 route inventory** (complete audit of every
`now_utc >= *_close_utc` check in `autonomous_daily_coordinator.rs` that can
route toward `handle_session_close`):

1. `dispatch_by_state` (line ~1938) — exempted by the prior session's fix;
   unchanged by this patch except extracting its inline boolean into the new
   shared `evidence_degraded_runtime_stop_already_recorded` helper (identical
   behavior, confirmed by the full regression suite below).
2. `reconcile_existing_operation_against_relevant_lookup` (line ~676) —
   reached via `resolve_or_degrade_on_resolution_failure` (current-tick
   calendar/assignment/registry/runtime-context resolution failure) and
   `resolve_or_reconcile_on_nontrading_day` (materially distinct caller: a
   weekend/holiday calendar result, no resolution failure involved). Was
   **not exempted** — `evidence_degraded` absent from its own close-priority
   allow-list.
3. `apply_coverage_blocker` (line ~1049) — reached via `ensure_coverage_
   authority`, which runs on **every** coordinator tick, strictly before
   `dispatch_by_state`. Was **not exempted**.
4. `handle_identity_conflict` (line ~1175) — reached from `create_or_
   recover`'s `IdentityConflict` arm whenever the freshly-computed
   `assignment_identity`/`runtime_binding_identity`/`session_plan_identity`
   disagrees with the existing row occupying the same `(market_date,
   deployment_mode, adapter_id)` slot (e.g. an operator changes the
   resolvable strategy symbol while a stopped `evidence_degraded` operation
   from earlier in the day still occupies the slot). Was **not exempted**.
   **This route was not named by the original 2026-08-24 incident report or
   by the prior session's audit** — it surfaced only from this session's
   exhaustive Phase 0 search, confirming the mission's caution against
   assuming exactly three gates.

**Reachability proof** (all four DB-backed against disposable `mqk_test`,
driving the real `tick_autonomous_daily_coordinator` top-level production
entry point — never a private helper called directly):

- Route 2 (resolution-failure caller): `t5_resolution_failure_route_never_
  reenters_stopping`. Fixture's bound run is `HALTED` (not `STOPPED`) —
  reproducing the exact 2026-08-24 run-status shape — because `fetch_
  relevant_open_autonomous_daily_operation`'s own SQL only treats a stopped
  `evidence_degraded` operation as fully resolved (and excludes it from the
  "relevant" lookup) when its bound run is `STOPPED`; `HALTED` is not
  exempted, so the operation is found "relevant" and routed into this gate
  exactly as the incident's operation was. `REACHABLE_DEFECT` confirmed by
  RED proof below.
- Route 2 (nontrading-day caller): `t6_nontrading_day_route_never_reenters_
  stopping`, `now_utc` on a real weekend date, no env/config change needed.
  `REACHABLE_DEFECT` confirmed.
- Route 3 (coverage-authority failure): `t7_coverage_authority_failure_
  route_never_reenters_stopping`. The fixture never binds a coverage anchor
  (built via raw lifecycle transitions, not a real `running` start), so a
  real tick's own `ensure_coverage_authority` genuinely finds `NotBound`, and
  `check_operation_pristine` genuinely reports `HasActivity` (`run_id`/
  `started_at_utc` are set) — no fabricated conflict was needed.
  `REACHABLE_DEFECT` confirmed.
- Route 4 (identity conflict): `t8_identity_conflict_route_never_reenters_
  stopping`. Changing the resolvable assignment symbol after the fixture is
  seeded produces a real `IdentityConflict` on every subsequent tick.
  `REACHABLE_DEFECT` confirmed.

**Production patch**: extracted the existing D2.17 predicate into a shared
`evidence_degraded_runtime_stop_already_recorded(operation) -> bool`
(`state == evidence_degraded && stopped_at_utc.is_some()`, nothing more —
never treated as proof of clean outbox/inbox/reconcile) and added the same
exemption to routes 2, 3, and 4's close-priority checks. Once each gate's
close-priority check is bypassed, that function's own pre-existing
`evidence_degraded` arm/self-loop (route 2's dedicated
`handle_outcome_finalization` arm; routes 3 and 4's existing `evidence_
degraded => evidence_degraded` self-loop target) already handles the shape
correctly — no new finalization path was invented, exactly as the mission
required.

**RED/GREEN mutation proof**: routes 2, 3, and 4's three new exemption
conditions were temporarily reverted (each `&& !evidence_degraded_runtime_
stop_already_recorded(...)` removed) with a source comment marking the
bypass; `t5`–`t8` all RED-failed reproducing the exact pre-patch transition
(`outcome == RuntimeStopped`, `after.state == "stopping"`) while `t1`–`t4`
(the unaffected `dispatch_by_state` route) continued to pass — proving the
failures were specific to the reverted gates, not incidental. The bypass was
then fully reverted via `git checkout` + re-applying the saved patch diff
(`git diff` against the committed patch confirmed byte-identical), and
`t1`–`t8` all GREEN-confirmed passing again.

**Convergence proof**: `t1` and `t5`–`t8` each drive 5 repeated top-level
ticks 30s apart past `postclose_finalize_utc`; `state_version` is asserted
non-decreasing every tick and stable (unchanged) from the third tick onward
for every route — no route re-enters an alternating `stopping <->
evidence_degraded` loop.

**Negative controls preserved** (unchanged, all still passing):
- `t2` — an unacked outbox row on the fixture's run still fails closed
  (this patch does not weaken the existing outbox/inbox/reconcile safety
  checks; it only changes which path reaches `evidence_degraded`'s own arm).
- `t3` — no route may schedule/attempt a fresh start inside `[effective_
  operation_close_utc, postclose_finalize_utc]` (REPAIR-01, unaffected).
- `t4` — a mid-run `evidence_degraded` row (`stopped_at_utc` still `None`)
  is never exempted by any of the four gates; still routes into `handle_
  session_close` and fails closed on the still-active run exactly as before.
- `scenario_autonomous_daily_session_coordinator_01::j04` —
  `handle_identity_conflict`'s close-priority gate still correctly reaches
  canonical stop for a genuinely `running` (not evidence_degraded) identity
  conflict at persisted close; the new exemption does not weaken this case.
- `scenario_autonomous_daily_session_coordinator_01::l01`/`p01`/`p03` — the
  resolution-failure and nontrading-day routes still correctly degrade/close
  `running`/`stopping` operations at close; unaffected by the new exemption.

**Focused tests** (disposable `mqk_test`, port 5434, reset via `DROP
DATABASE` + `CREATE DATABASE` to clear a stale sqlx migration-checksum error
before running — see `scripts/reset-mqk-testdb.ps1` for the documented
equivalent):
`scenario_autonomous_daily_stopping_evidence_degraded_oscillation_01` (8/8),
`scenario_autonomous_daily_evidence_degraded_recovery_01` (13/13),
`scenario_autonomous_daily_stale_evidence_degraded_finalization_01` (4/4),
`scenario_autonomous_daily_controller_degraded_recovery_01` (8/8),
`scenario_autonomous_daily_session_coordinator_01` (46/46, 3 filtered). All
green. `cargo check -p mqk-daemon` clean; `git diff --check` clean.

Production commit: `f37cd8c4` ("fix: unify stopped degraded close
priority"), on `ledger-oscillation-close-priority-01`, NOT merged into
`ledger-closure-integration-01`, NOT pushed — pending independent review.

Still related but not proven identical to the previously-closed
`PAPER-SOAK-SESSION-WATCH-01` stale-operation defect (see memory
`project_paper_soak_session_watch_01_stale_operation_defect.md` in the
operator's persistent notes) — do not assume identity between the two
without independent proof.

### DAEMON-EXIT-20260824 — STATUS=UNKNOWN_NEEDS_PROOF

*Investigated (not closed) by `PAPER-BACKEND-LEDGER-CLOSURE-WAVE-01-
CONTROLLER`, 2026-08-25. Adds one new, precisely-timestamped forensic
finding below; does not change the status, per the mission's own closure
bar (exact cause or a deterministic, reproduced source path required —
neither is met).*

Observed: the daemon process (`mqk-daemon.exe`, PID 40184) disappeared
without this closeout mission (or its immediate predecessor) issuing a
kill; confirmed absent by direct process enumeration immediately before any
termination action would have been attempted (`patch-review-proof/
PAPER_SOAK_2026-08-24_CLOSEOUT_REVIEW/14_processes_post_shutdown.txt`).

**New finding (LIKELY, not CONFIRMED): the host machine entered Windows
Modern Standby (sleep) for the exact window immediately preceding the
daemon's disappearance.** Windows `Microsoft-Windows-Power-Troubleshooter`
(Application event log) and `Microsoft-Windows-Kernel-General` (system time
change) both record: Sleep Time `2026-08-24T22:50:09.036075200Z`, Wake Time
`2026-08-25T03:12:24.388810800Z` (a 4h22m gap). Cross-referencing this
against `sys_autonomous_daily_operation_events` for operation `40e97f92`
(read-only, `mqk-paper-postgres`) shows the daemon's own stopping/
evidence_degraded oscillation loop has a transition-timestamp gap of
**exactly the same window**: transition #262 at `2026-08-24 22:50:24.989Z`,
then transition #263 at `2026-08-25 03:12:35.006Z` (11 seconds after OS
wake) — i.e. the daemon's tick loop was itself suspended for the full sleep
duration and resumed normally immediately on wake. The daemon then
continued ticking for almost exactly two more minutes (through transition
#511 at `2026-08-25 03:14:34.317Z`) before going silent; no further DB
writes, and the process was independently confirmed gone by the time of the
closeout mission's process check shortly afterward.

This is a precise temporal correlation, not a source-level proof: no daemon
stdout/stderr log or crash dump survives from the actual exit window (the
`exports\launcher\daemon_20260824_020338.stdout.log` file referenced by
`diagnostics/CONFIRMED_BLOCKER_strategy_invocation_stale_operations.json`
is not present in `smoke_logs/` or the forensic archive — likely rotated or
never retained past the consolidation cleanup pass); no Windows Application
Error / Windows Error Reporting event for `mqk-daemon.exe` exists in the
Application log for 2026-08-24 (checked, zero matches), which argues
against a hard OS-level fault (segfault/access violation) and toward either
a clean self-exit shortly after resume (e.g. an unhandled error from a
DB/WS connection that failed to survive the suspend/resume transition) or
an OS-level termination that does not itself produce an Application Error
record (e.g. Modern Standby connectivity-standby process management). No
production patch is justified without the exact mechanism — implementing
one now would be guessing, which the controlling mission explicitly
forbids. **No causal link to the stopping/evidence_degraded oscillation
above is asserted** beyond the fact that oscillation was the loop actively
ticking through the sleep/wake boundary; the two remain separate open items.

**Recommended next steps for a future session (not performed by this
wave):** (a) confirm whether Windows Event Viewer / WER retains anything
past its default retention for this window on this machine; (b) consider an
operational mitigation (disable sleep/Modern Standby on this host during
active Paper sessions) independent of any code change; (c) if a future
unattended exit recurs, capture the daemon's stdout/stderr log and any WER
crash dump into the forensic archive immediately, before any consolidation/
cleanup pass can prune it.

**Source-path audit — completed by `PAPER-BACKEND-LEDGER-WAVE-01-
INDEPENDENT-REVIEW-REPAIR-01` (PATCH G), 2026-08-25.** The exhaustive
source-level audit the prior wave explicitly left unfinished is now
complete: every candidate exit path the controlling mission named
(`process::exit`, panic propagation from long-lived supervisor tasks,
`JoinHandle` error propagation, `main`-returning fatal errors, shutdown
signal/`select` branches, task supervisor exhaustion, launcher `Stop-
Process`/`taskkill`/ownership cleanup, scheduled-task `ExecutionTimeLimit`/
timeout/end-boundary behavior) was searched in `core-rs/**/*.rs` and
`scripts/windows/*.ps1` and classified against the preserved forensic
evidence above. Summary: `process::exit`/`process::abort` calls,
background-task panic propagation, and `JoinHandle` propagation into
`main()` are all `DISPROVEN` by source (zero occurrences; the workspace
uses the Rust-default `panic = "unwind"`, so a background-task panic
becomes a `JoinError`, never a process exit, confirmed by the existing
`k01`/`k03`/`k05` completed-bar-task supervisor tests). The launcher/
watchdog scripts are `DISPROVEN` as a cause — `Start-DaemonIfNeeded`
(`Launch-VeritasLedger.ps1`) only ever reuses or refuses against an
existing identity-verified daemon, never kills one; every `Stop-Process`/
`Kill()` call in `scripts/windows/*.ps1` targets only a process the same
script invocation itself just spawned. Two mechanisms remain genuinely
`POSSIBLE_BUT_UNPROVEN`, consistent with every piece of preserved
evidence but with no independent proof tying either to this specific
timestamp: (1) `main()` (`mqk-daemon/src/main.rs`) returning `Err` from
its one live `axum::serve(...).context("server crashed")?` call exits the
process silently via the Rust runtime's own `Termination` handling — no
panic, no WER entry, matching the evidence exactly; (2) `main.rs` only
registers `tokio::signal::ctrl_c()` (`CTRL_C_EVENT`), never `ctrl_close`/
`ctrl_logoff`/`ctrl_shutdown` — an unhandled Windows session/console-close
event would silently bypass graceful shutdown entirely. One mechanism
initially suspected as a strong candidate — the `MiniQuantDesk-Paper-
Preopen-Startup` / health-watchdog scheduled tasks' `ExecutionTimeLimit
= 1 hour` with `WakeToRun` (a previously-proven-dangerous pattern on this
exact system, per `Start-MiniQuantDesk.ps1`'s own 2026-08-13 incident
comment) — is classified `NOT_APPLICABLE to this specific window`: both
tasks fire at fixed early-morning **local** clock times, and converting
the preserved UTC sleep/wake evidence to this repository's own git-author
timezone convention (`-1000`, Hawaii Standard Time) places the actual
sleep/wake event in the early afternoon/evening local time, roughly ten
hours away from either task's execution window. Full findings, evidence
citations, and the complete classification table are in this wave's
review bundle (`07_daemon_exit_complete_source_audit.md`). No deterministic
causal proof was found for any candidate; **no production or script code
was changed by this audit**, per the controlling mission's explicit
prohibition on patching an unproven root cause. `DAEMON-EXIT-20260824`
remains `UNKNOWN_NEEDS_PROOF`.

---

## 29. Paper Soak Repair Chain Integration

*Added by `LEDGER-CLOSURE-PAPER-REPAIR-INTEGRATION-01-CONTROLLER`, 2026-08-24
(same day, second pass after §28's consolidation).*

`paper-soak-session-1-repair` = INDEPENDENTLY ACCEPTED — PUSHED — INTEGRATED
LOCALLY into `ledger-closure-integration-01` (not pushed; awaiting
independent review).

- Repair tip: `8dd9ba3ff1f9f0082db370bb5a6e66930ef2fb7b` ("fix: fence outbox
  enqueue on running run").
- Pre-merge spine HEAD: `828af6ed09303bb5ed7f51585a0a2f9676eef414`.
- Merge commit: `4d7aafca4a6a0a3c39e8b704fe2979eab5f2125e` (`--no-ff`, two
  parents, clean — zero conflicts; merge-base with the repair branch was
  `edcda740b2f05fbe8a2657f2301b8ea373efb4b6`, i.e. both branches shared the
  same frozen `main` ancestor).
- Files touched: confined to `core-rs/crates/mqk-db/` and
  `core-rs/crates/mqk-daemon/` (decision/execution/strategy routes,
  control_plane, autonomous_daily_coordinator, runs/orders/inbox/
  reconcile_state) plus new/extended scenario test files in both crates. No
  Research/GUI/resilience file was touched.
- Paper repair known deterministic defects: NONE found this session.

Combined acceptance boundary (disposable `mqk_test` DB on port 5434, reset
via `scripts/reset-mqk-testdb.ps1` to clear a stale sqlx migration-checksum
error before running): `cargo check -p mqk-db` and `cargo check -p
mqk-daemon` both clean; every scenario/prover test file named in the
controlling mission passed with zero failures, plus the direct
decision/execution/strategy/pre-event-flatten regressions
(`scenario_execution_flow_flow01`, `scenario_internal_strategy_decision`,
`scenario_runtime_strategy_conflict_api_01`, `scenario_pre_event_flatten_01`,
`scenario_strategy_decision_idempotency_01`). See §28's updated
`AUTONOMOUS-DAILY-STOPPING-EVIDENCE-DEGRADED-OSCILLATION-01` entry above —
originally recorded here as `LIKELY_SAME_FAMILY_BUT_NOT_PROVEN` / `OPEN`,
subsequently `CLOSED` by `AUTONOMOUS-DAILY-STOPPED-EVIDENCE-DEGRADED-CLOSE-
PRIORITY-UNIFICATION-01-CONTROLLER` the same day (third pass).
`DATA-READINESS-BAR-COVERAGE-AUTHORITY-01` and `DAEMON-EXIT-20260824` were
unchanged by that later session and remained `OPEN` / `UNKNOWN_NEEDS_PROOF`
respectively at that time; see §30 below (`PAPER-BACKEND-LEDGER-CLOSURE-
WAVE-01-CONTROLLER`, 2026-08-25) for the subsequent closure of the former
and continued investigation of the latter.

Review bundle:
`Documents/MiniQuantDeskV4-Archive/2026-08-24/paper-repair-integration-review/PAPER_SOAK_REPAIR_INTEGRATION_REVIEW.zip`.
Not pushed. Temp repair worktree
(`AppData/Local/Temp/MiniQuantDeskV4-paper-repair-final`) preserved for
independent review.

---

## 30. Paper Backend Ledger Closure Wave (Data Readiness + Daemon Exit)

*Added by `PAPER-BACKEND-LEDGER-CLOSURE-WAVE-01-CONTROLLER`, 2026-08-25, on
temporary worktree `AppData/Local/Temp/MiniQuantDeskV4-paper-backend-wave-01`,
branch `ledger-paper-backend-wave-01`, base `dc398721` (the primary repo's
accepted `ledger-closure-integration-01` HEAD, verified matching
`origin/ledger-closure-integration-01` and unchanged throughout this wave).*

- `DATA-READINESS-BAR-COVERAGE-AUTHORITY-01`: `OPEN` -> `LOCALLY COMPLETE
  — PENDING INDEPENDENT REVIEW`. Root cause found and repaired
  (crash-orphaned `running`/`recovery_retrying` operations had no
  evidence-gated release path, unlike the three sibling states already
  fixed by the integrated paper-repair and oscillation-closure chains);
  see §28's updated entry above for the full proof. One production commit
  (`a511ab4c`) — see §31 below for the independent-review repair that
  followed on the same branch.
- `DAEMON-EXIT-20260824`: remains `UNKNOWN_NEEDS_PROOF`. One new forensic
  finding added (a precise Windows Modern Standby sleep/wake window
  matching the daemon's tick-loop transition gap almost to the second,
  immediately preceding its disappearance); no production patch, per the
  controlling mission's explicit prohibition on patching an ambiguous root
  cause. See §28's updated entry above; see §31 below for the completed
  source-path audit that followed on the same branch.
- No other Paper/backend ledger lines were found provably stale enough to
  correct in this same commit; the wave's scope was confined to the two
  named investigations per the controlling mission.

Both investigations were conducted read-only against the live
`mqk-paper-postgres` database (zero mutation — verified via `git status`-
equivalent DB state comparison before/after) and read-only against Windows
Event Viewer. No Paper session was started, no order was submitted, no
Live routing was touched, and `smoke_logs/`/`.env.local` were not modified.

Review bundle: `Documents/MiniQuantDeskV4-Archive/2026-08-24/
paper-backend-ledger-wave-01-review/
PAPER_BACKEND_LEDGER_CLOSURE_WAVE_01_REVIEW.zip`. Not pushed. Temp wave
worktree (`AppData/Local/Temp/MiniQuantDeskV4-paper-backend-wave-01`)
preserved for independent review.

---

## 31. Paper Backend Ledger Closure Wave — Independent Review Repair

*Added by `PAPER-BACKEND-LEDGER-WAVE-01-INDEPENDENT-REVIEW-REPAIR-01`,
2026-08-25, same worktree/branch as §30
(`AppData/Local/Temp/MiniQuantDeskV4-paper-backend-wave-01`,
`ledger-paper-backend-wave-01`), continuing from §30's HEAD
(`42ae95b4`). Repairs the `PARTIAL — REPAIR REQUIRED` finding an
independent review returned against §30's own commits, without rebasing,
amending, or squashing either of them.*

- `DATA-READINESS-BAR-COVERAGE-AUTHORITY-01`: `LOCALLY COMPLETE — PENDING
  INDEPENDENT REVIEW` (unchanged label; the underlying fix is now
  independently-review-repaired). Independent review found `a511ab4c`'s
  run-evidence carve-out had over-broadened REPAIR-2's generic fallback
  clause to every state instead of only `running`/`recovery_retrying`;
  corrected by production/test commit `5346f90a2233b8cdf8ed1ff5f82f2aea
  974421d4` (PATCH F). Full finding, RED/GREEN proof, coverage-gap
  closure, and the new integrated stale-row -> bar -> observation ->
  dispatch -> evaluation proof are recorded in §28's updated entry above.
  Still `PENDING INDEPENDENT REVIEW` — this wave does not self-accept.
- `DAEMON-EXIT-20260824`: remains `UNKNOWN_NEEDS_PROOF`. PATCH G completed
  the exhaustive source-path audit §30 explicitly left unfinished (every
  category the controlling mission named, searched and classified against
  the preserved forensic evidence); no deterministic causal proof was
  found for any candidate, so no production or script code was changed.
  Full classification table in §28's updated entry above and in the review
  bundle's `07_daemon_exit_complete_source_audit.md`.
- No other ledger line was touched by this repair; scope was confined to
  the two items the independent review named.

Zero DB mutation, zero Paper session started, zero order submitted, zero
Live routing touched; `smoke_logs/`/`.env.local` not modified;
`git reset`/`git stash`/`git clean`/force-push not used; not pushed.

Review bundle: `PAPER_BACKEND_LEDGER_CLOSURE_WAVE_01_REPAIR_REVIEW.zip`.
Temp wave worktree preserved for independent review.

---

## 32. Paper Backend Wave Integration Test Closure Repair

*Added by `PAPER-BACKEND-WAVE-01-INTEGRATION-TEST-CLOSURE-REPAIR-01`,
2026-08-25, primary repo `C:\Users\Zacha\Desktop\MiniQuantDeskV4`, branch
`ledger-closure-integration-01`, starting HEAD `020d98e9` (the merge §30/§31
integrated: `merge: integrate accepted paper backend ledger wave`, parents
`dc398721` + `9cb49a56`).*

Resolves the one acceptance failure inherited into `020d98e9`:
`scenario_autonomous_daily_outcome_coordinator_integration_01::
ci_11_12_evidence_degraded_warning_dedup`, expected `evidence_degraded`,
actual `manual_intervention_required`. Reproduced identically at pre-wave
`dc398721` and post-wave `020d98e9` — confirmed **not** introduced by
`PAPER-BACKEND-WAVE-01` (§30/§31 above).

**Root cause (STALE_TEST_FIXTURE, confirmed by isolated execution against a
disposable `mqk_test`, not by static reading alone):** the failure is on the
test's *second* (replay) tick, not the first — the first tick's
`evidence_degraded` / `unknown_incomplete_bar_coverage` classification
already passed. `dispatch_by_state`'s `STATE_EVIDENCE_DEGRADED` arm
(`autonomous_daily_coordinator.rs`) routes a stopped operation through
`attempt_evidence_degraded_recovery` before ever reaching E2B's
finalization/replay path. That gate is scoped to exactly the reason code
this fixture uses (`unknown_incomplete_bar_coverage` — the one closed
`unknown_*` reason accepted as recovery-eligible by `cd3a5bab`, "paper:
recover evidence degraded operation after transient blocker clears",
2026-08-18, i.e. `AUTONOMOUS-DAILY-STOPPING-EVIDENCE-DEGRADED-
OSCILLATION-01`'s lineage — an ancestor of the designated pre-wave baseline
`4248bdb4`, unmodified since). Because the fixture's replay tick fired
before `effective_operation_close_utc`, and never seeded a
`sys_reconcile_status` row, the recovery attempt fails closed to
`evidence_degraded_recovery_reconcile_dirty` / `manual_intervention_
required` instead of exercising the dedup path the test targets. This is a
different mechanism than the mission's initial E2A-coverage-authority-
ordering hypothesis, which was checked and does **not** apply here (the
existing `e02_prior_activity_running_missing_authority_reaches_evidence_
degraded` negative control in `scenario_autonomous_daily_coverage_anchor_
and_run_lineage_01.rs` already proves E2A's `NotBound`+`HasActivity`
ordering correctly, unaffected, re-run clean).

`F37_CAUSALITY=DISPROVEN`: the recovery-attempt gate predates `f37cd8c4`
("fix: unify stopped degraded close priority") by six commits and is an
ancestor of the pre-f37 baseline `4248bdb4`; no commit between `cd3a5bab`
and `020d98e9` touched the reason-code gate (`git log -S` on the gate's
literal, verified empty for every intervening commit touching this file).

**Fix (test-only, one file):** move `ci_11_12`'s replay tick to
`plan.effective_operation_close_utc + 1s`, so it lands past the recovery
gate's own close boundary and exercises E2B's finalization/replay path —
matching the test's original dedup intent without reopening or amending the
accepted `cd3a5bab` recovery contract. Commit `dc9655fa`, `test: repair
evidence degraded warning fixture`. No production file changed.

Acceptance, disposable `mqk_test` (port 5434), `--test-threads=1`:
`scenario_autonomous_daily_outcome_coordinator_integration_01` 15/15 green;
`scenario_autonomous_daily_operation_lifecycle_01` (mqk-db) 63/63,
`scenario_autonomous_daily_phase_d_integration_01` 10/10,
`scenario_autonomous_completed_bar_driver_01` 56/56,
`scenario_autonomous_completed_bar_task_01` 2/2,
`scenario_autonomous_daily_coordinator_policy_01` 35/35,
`scenario_autonomous_daily_session_coordinator_01` 46/46 — zero failures.
`cargo check -p mqk-db` / `-p mqk-daemon` clean; `git diff --check` clean.

**`DATA-READINESS-BAR-COVERAGE-AUTHORITY-01` status is unchanged by this
repair** — still `LOCALLY COMPLETE — PENDING INDEPENDENT REVIEW` (§28/§30/
§31); this inherited `ci_11_12` fixture defect was never part of that
patch's 2026-08-24 root cause and is not evidence for or against it.
`AUTONOMOUS-DAILY-STOPPING-EVIDENCE-DEGRADED-OSCILLATION-01` remains
`CLOSED` (§28). `DAEMON-EXIT-20260824` remains `UNKNOWN_NEEDS_PROOF` (§28/
§31) — untouched by this repair.

Zero DB mutation beyond the disposable `mqk_test` database used for the
required test runs; zero Paper session started; zero order submitted; zero
Live routing touched; `smoke_logs/`/`.env.local` not modified; `git reset`/
`git stash`/`git clean`/rebase/force-push not used; not pushed.

Review bundle:
`Documents/MiniQuantDeskV4-Archive/2026-08-24/paper-backend-wave-01-
integration-review/PAPER_BACKEND_WAVE_01_INTEGRATION_REPAIR_REVIEW.zip`.

---

## 33. Independent Acceptance of the Paper Backend Wave

*Added by `PAPER-BACKEND-WAVE-01-INDEPENDENT-ACCEPTANCE-FINALIZER-01`,
2026-08-25, primary repo `C:\Users\Zacha\Desktop\MiniQuantDeskV4`, branch
`ledger-closure-integration-01`, pre-finalizer HEAD `2e0b560d5e7a90beffe
3e44a852f33ca9aba2d91` (§32's tip). Docs-only — records that the wave's
outstanding independent-review gate has now been cleared externally by
ChatGPT. No production, test, or script file is touched by this entry.*

Final authoritative status, superseding every prior "PENDING INDEPENDENT
REVIEW" occurrence for this patch (§28 header updated above; §30/§31/§32's
own narrative text is left unmodified as an accurate record of what was
true at each of those points):

- `AUTONOMOUS-DAILY-STOPPING-EVIDENCE-DEGRADED-OSCILLATION-01` = `CLOSED`
  (unchanged; §28).
- `DATA-READINESS-BAR-COVERAGE-AUTHORITY-01` = `CLOSED — INDEPENDENTLY
  ACCEPTED`.
- `DAEMON-EXIT-20260824` = `UNKNOWN_NEEDS_PROOF` (unchanged; §28/§31/§32).
  Independent acceptance of the wave does **not** establish a daemon-exit
  root cause — none is claimed here.

**Acceptance chain of custody:**

- Paper/backend wave source tip: `9cb49a56f36cc0dbc225c1c29b9729de4e1c0e6c`
  (`docs: repair paper backend wave review truth`).
- Integration merge: `020d98e945b2923640b622f8f8221531d94f84a4` (`merge:
  integrate accepted paper backend ledger wave`).
- Data-readiness implementation: `a511ab4c` (`fix: release crash-orphaned
  running/recovery_retrying operations by run evidence`).
- Independent-review scope repair: `5346f90a2233b8cdf8ed1ff5f82f2aea
  974421d4` (`fix: scope stale running release to active states`).
- Independent-review test-fixture repair: `dc9655fa68c5c428b0c4c8416ea120a
  328d3b28c` (`test: repair evidence degraded warning fixture` — §32;
  `ci_11_12_evidence_degraded_warning_dedup` was an inherited
  `STALE_TEST_FIXTURE`, unrelated to and not part of the 2026-08-24
  data-readiness root cause).
- Integrated acceptance boundary, independently reviewed: 227/227 affected
  tests green; `cargo check -p mqk-db` clean; `cargo check -p mqk-daemon`
  clean; `git diff --check` clean (§32).

No Paper soak is claimed to have passed by this entry — none was run as
part of this acceptance. `smoke_logs/`/`.env.local` not modified; `git
reset`/`git stash`/`git clean`/rebase/force-push not used; not pushed.

---

## 34. Independent Acceptance of Research/Backtest V1

*Added by `RESEARCH-BACKTEST-V1-FINAL-INTEGRATION-AND-ACCEPTANCE-01`,
2026-08-28, primary repo `C:\Users\Zacha\Desktop\MiniQuantDeskV4`, branch
`ledger-closure-integration-01`, starting HEAD `484d93f3c153d22ff196b523
f77844dfba67b750` (§33's tip, `docs: correct research promotion push
truth`). Records that the Research/Promotion V1 closure chain's
outstanding independent-review gate — the one deterministic proof defect
`RESEARCH_BACKTEST_V1_COMPLETE`'s own `06417bdc` closure (Executive
Summary, above) had not yet cleared — has now been cleared externally by
ChatGPT. Fast-forward integration only; no production, test, or script
file beyond the two named below is touched by this entry.*

**Final authoritative status, superseding the `06417bdc` "LOCALLY
COMPLETE — PENDING INDEPENDENT REVIEW" label recorded in the Executive
Summary above (§24's own internal narrative text — §5, the closure/repair
wave paragraphs, and the `1F`/`1L`/`1L-1`/`1L-2` addenda in
`docs/research/Research_Backtest_V1_Closeout_Audit.md` — is left
unmodified as an accurate record of what was true at each of those
points):**

- `RESEARCH_BACKTEST_V1_COMPLETE` = `CLOSED — INDEPENDENTLY ACCEPTED`.
- `PROMOTION-BACKTEST-EVIDENCE-SEAM-01`, `PROMOTION-WALKFORWARD-GATE-
  WIRING-01`, P9 `BKT-ROBUSTNESS-GAUNTLET-01`, and P10
  `RESEARCH-BACKTEST-FINAL-ACCEPTANCE-01` (all §5/§24) = `CLOSED —
  INDEPENDENTLY ACCEPTED` as components of the same accepted chain.
- `DIRECT_RANK` (`RESEARCH-DIRECT-RANK-AND-LEDGER-CLOSURE-WAVE-01`,
  §1L-2 of the closeout audit) is **unchanged** by this entry — it was
  already `CLOSED — INDEPENDENTLY ACCEPTED` on its own, separate review,
  and this acceptance does not re-review or extend that wave.

**Acceptance chain of custody:**

- Historical closure tip: `06417bdcdc73ce2e0e9a0247cb1656d9af211c4c`
  (`FINAL-P9-AUTHORITY-BINDING-REPAIR-01`) — the 24-commit
  Research/Promotion closure range independently reviewed is
  `fbddeb3dba3066bc4f658a576d8393be127d9d62`..`06417bdc`.
- Independent-review finding: production/promotion chain SOUND end to
  end; one confirmed proof gap — P10 claimed a route-level Postgres V3
  lineage readback that did not actually exist (shared `mqk_test` DB also
  carried unrelated historical migration-checksum drift, so DB proof was
  truthfully `BLOCKED`, not fabricated).
- Final proof / repair commit:
  `12490668e57f0ab2a900bb0e4b045619e4a904be` (`test: prove promotion http
  route persists exact evidence lineage`) — real daemon HTTP promotion
  route, real Postgres readback, exact Research/backtest/stress/
  robustness/policy identity binding, mutation-style RED control.
  Fresh isolated disposable Postgres results: closure proof 1/1, daemon
  promotion routes 33/33, `mqk-db` promotion registry/lineage 33/33, P10
  acceptance 5/5. Production files changed: none.
- Integration acceptance base: `12490668` fast-forward-merged onto
  `ledger-closure-integration-01` (`git merge --ff-only`, starting HEAD
  `484d93f3` == merge-base with `origin/research-promotion-lineage-proof-
  repair-01`), resulting HEAD `12490668e57f0ab2a900bb0e4b045619e4a904be`.
  `git diff --check 484d93f3..HEAD` clean; `git diff --name-status
  484d93f3..HEAD` touched exactly
  `core-rs/crates/mqk-daemon/tests/scenario_strategy_promotion_closure_
  proof_01f.rs` and `core-rs/crates/mqk-promotion/tests/scenario_
  research_backtest_promotion_v1_acceptance_01.rs`. No test rerun was
  performed for the fast-forward itself — the integrated commit is
  byte-identical to the independently reviewed and tested commit, and a
  fast-forward with no production/test divergence cannot alter test
  semantics.

**What this does not establish:** `PROVEN_ALPHA`; promotion-readiness for
an arbitrary new strategy; final-holdout consumption (final holdout
remains reserved, not consumed); `SHORT-WAVE-03` execution (unexecuted);
Paper forward validation (separate operational/economic evidence stage,
not established by this entry); Live readiness.

Zero DB mutation beyond the reviewer's own disposable Postgres instance;
zero Paper session started; zero order submitted; zero Live routing
touched; `smoke_logs/`/`.env.local` not modified; `git reset`/`git
stash`/`git clean`/rebase/force-push not used; not pushed.

Review bundle:
`Documents/MiniQuantDeskV4-Archive/2026-08-28/research-backtest-v1-final-
acceptance-integration-01/
RESEARCH_BACKTEST_V1_FINAL_ACCEPTANCE_INTEGRATION_01.zip`.

---

*End of MiniQuantDesk V4 Authoritative Master Completion Ledger — FULL-REPO-COMPLETION-AUDIT-01, updated by PAPER-AUTONOMOUS-STARTUP-THREE-DEFECT-CLOSURE-01, updated by MASTER-LEDGER-CONSOLIDATION-01 (2026-08-17), updated by LEDGER-CLOSURE-CONSOLIDATION-01-CONTROLLER (2026-08-24), updated by LEDGER-CLOSURE-PAPER-REPAIR-INTEGRATION-01-CONTROLLER (2026-08-24), updated by PAPER-BACKEND-LEDGER-CLOSURE-WAVE-01-CONTROLLER (2026-08-25), updated by PAPER-BACKEND-LEDGER-WAVE-01-INDEPENDENT-REVIEW-REPAIR-01 (2026-08-25), updated by PAPER-BACKEND-WAVE-01-INTEGRATION-TEST-CLOSURE-REPAIR-01 (2026-08-25), updated by PAPER-BACKEND-WAVE-01-INDEPENDENT-ACCEPTANCE-FINALIZER-01 (2026-08-25).*

---

# DEFERRED DESIGN IDEAS

Non-required architecture ideas surfaced during `W06-A-P9-REPLAY-SOURCE-AUTHORITY-REPAIR-WAVE-02` (2026-09-05). Preserved for future reference only.

**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.** Every entry below carries this same status individually; do not treat any entry as scheduled, prioritized, or authorized work. Do not alter active patch status/count based on this section.

### Persist per-fold trained model state
A future artifact version may durably store coefficients/intercept/standardization state for stronger long-term replay.
Deferred because deterministic refit is sufficient for current Wave06.
Revisit if library/version drift or repeated replay needs justify it.
**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**

### Generic ResearchReplayBundle protocol
Generalize authenticated Research replay beyond LIQ/VOL.
Deferred until multiple real candidate families require it.
**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**

### Generic Research -> Backtest candidate adapter
Standard interface for non-builtin Research candidates through Backtest.
Deferred until repeated real usage proves the abstraction.
**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**

### Batch-level cross-sectional decision source
Explicit Backtest batch-decision interface for future strategies that cannot safely use the replay Strategy seam.
Deferred because current replay strategy should remain narrower.
**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**

### Canonical read-only evidence authority CLI
One machine-readable surface exposing verified Research + Backtest + P9 authority.
Deferred until current Wave06 authority path is complete.
**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**

### Point-in-time research universe snapshots
Durable historical constituent/delistings/corporate-action universe data.
Deferred until operational readiness is finished and fresh broad-universe alpha confirmation begins.
**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**

### Replay compatibility/version audit
Read-only check that HEAD can still reproduce old accepted Research runs.
Deferred until durable replay artifacts exist across multiple versions.
**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**

### Research/Paper equivalence proof harness
Compare promoted Research decisions, canonical Backtest targets, and Paper shadow decisions on the same completed bars.
Deferred until a strategy actually earns Paper entry.
**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**

### Generic candidate-family API
Common feature/model/rank/replay/falsification interface for future research families.
Deferred until repeated successful families justify abstraction.
**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**

### Automatic temporary-artifact ownership/retention metadata
Tag generated artifacts as temporary/review/durable/protected.
Deferred because explicit per-wave cleanup is currently safer/simpler.
**NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**

### genuine_shuffled_placebo / cross-sectional-percentile-rank tie incompatibility (investigation, not a design idea)
A genuine, reproducible, deterministic finding (not a design idea to build, but recorded here since it surfaced from this wave and needs a dedicated future investigation): `genuine_shuffled_placebo_cli.py`'s fold-wide score shuffle is structurally incompatible with any cross-sectional-percentile-rank feature transform (LIQ-01/VOL-01's own feature family) whenever a walk-forward fold spans more than one decision date, per `_resolve_rank_direction_for_frame`'s exact boundary-tie fail-closed check. See the dedicated follow-up task spawned from this wave (`W06-A-P9-REPLAY-SOURCE-AUTHORITY-REPAIR-WAVE-02`, Patch R3) for full analysis.

**RESOLVED LOCALLY by commit `2e1aa7d9` (`W06-A-P9-GENUINE-SHUFFLED-PLACEBO-CROSS-SECTIONAL-REPAIR-01`), timestamp-scoping fix.** For `cross_sectional_rank_long_only_v1`/`cross_sectional_rank_long_short_v1` candidates, `genuine_shuffled_placebo_cli._shuffle_oos_predictions` now permutes frozen OOS `ml_score` within each `(fold, decision_ts)` cross-section instead of fold-wide, preserving every real decision frame's exact score multiset so the shuffle can never manufacture a boundary tie absent from the original frame. Non-rank policies (`long_only_v1`, `long_short_threshold_v1`) keep the legacy fold-wide shuffle unchanged. Verified on the real production path via mqk-cli's `r3_5_full_canonical_completion_synthetic_e2e_proof` (ignored E2E test): `genuine_shuffled_placebo` now genuinely reports `applicable: true, passed: true` for the LIQ-01-shaped rank fixture that previously exposed this finding.

**A SECOND, SCORE-LEVEL identity defect in the SAME repair was found by independent review**: `2e1aa7d9`'s own nontriviality check compared permuted ROW INDICES against the original indices, not the resulting ml_score ASSIGNMENT. With duplicate score values away from the selection boundary, a nonidentity row permutation can swap only equal-valued rows and leave the entire score vector unchanged -- silently turning the placebo into the original signal for that cross-section (deterministic repro: `trial_93`, `scores=[0.9, 0.7, 0.5, 0.5]`, row permutation `[0, 1, 3, 2]` swaps only the two equal `0.5` rows). **RESOLVED LOCALLY by commit `18d989530a8d4f247b04859f2ef0e6eeee1a72b3` (`W06-GENUINE-PLACEBO-SCORE-NONTRIVIALITY-REPAIR-01`). STATUS: RESOLVED LOCALLY — PENDING INDEPENDENT REVIEW.** The identity check now compares the candidate SCORE ASSIGNMENT directly; on an unchanged assignment it falls back to a deterministic nonzero cyclic rotation of the group's own score vector (never numeric noise, never crossing a fold/decision_ts boundary). Cross-sections with fewer than 2 rows or a single distinct score value are not meaningfully permutable and are now tracked explicitly (`groups_seen`, `groups_meaningfully_permutable`, `groups_score_changed`, `identity_groups_corrected`); if the entire rank placebo evaluation contains zero meaningfully permutable groups, the production wrapper (`_run_shuffled_placebo`) now fails closed with `status=not_evaluable` instead of reporting a placebo it never meaningfully ran. `PLACEBO_PROTOCOL_ID` (`genuine_shuffled_placebo_v1`) and non-rank fold-wide shuffle semantics are unchanged. 27 focused tests pass (`test_genuine_shuffled_placebo.py`, `test_genuine_shuffled_placebo_rank.py`), including a direct regression against the exact `trial_93` fixture above. **NOT PART OF ACTIVE 43-PATCH COUNT** -- this resolution note, like the finding it resolves, remains outside the active 43-patch count.

**SEPARATE, STILL-OPEN, OUT-OF-SCOPE FINDING surfaced while verifying the above**: the same R3.5 synthetic fixture's five stress-family robustness scenarios (`execution_delay_stress`, `symbol_leave_one_out`, `parameter_neighborhood_execution`, `placebo_temporal_offset`, `conservative_capacity_stress`) fail against an 18% max-drawdown ceiling. A `W06-R3-POSITIVE-SYNTHETIC-FIXTURE-CLOSURE-01` attempt (same wave, post-`18d98953`) diagnosed this: the fixture's original `_build_bars` generator embedded no genuine economic edge (independent per-symbol noise, uncorrelated with the ranked feature), so a deliberately positive replacement generator (deterministic per-symbol drift keyed to the same rank index the candidate trades on, mirroring `test_genuine_shuffled_placebo.py`'s own `_build_edge_bars` pattern) was built and iterated through several designs (bounded/delayed-onset compounding, a price floor, both `long_only`- and `long_short`-primary variants) to rule out weak-fixture-economics as the cause. **HARD-STOPPED, per this wave's own mission instruction, on an independently-established, deterministic PRODUCTION defect, not a fixture defect**: across every fixture variant tried, `mqk-cli`'s real replay-backtest execution path (`r3_5_full_canonical_completion_synthetic_e2e_proof`'s `run_research_replay_backtest` -> `BacktestEngine`) repeatedly, deterministically double- (up to 6x-) fills a single logical retried MARKET order instead of treating the retry idempotently: in one captured baseline run, 231 of 369 filled order_ids received 2-6 `FILLED` fills each (485 total fills against those 231 orders) before later retries of the SAME `order_id` were correctly `REJECTED` -- e.g. `order_id d4f43279-d591-5ba1-8f8f-72b8ab8ef5eb` (`SYM15 SELL 141` at one bar) was resubmitted 15 times: 2 `FILLED`, 13 `REJECTED`. Net effect: even with a strong, deterministic, monotonic price edge in the underlying bars (confirmed directly -- e.g. one traded symbol's close rose from ~$326 to ~$9,758 over the affected window), the position's OWN realized cash flow across the run was net NEGATIVE (two symbols: -$11,659.77 and -$5,382.58, both ending flat/fully closed, commissions negligible at ~$84 total) purely from the over-fill inflating sell-side (or buy-side) volume beyond the intended target delta -- an idempotency violation of exactly the kind [[execution_rules]] requires proof against ("a fill applied twice must be idempotent"). This reproduced identically in character (though not in exact numbers) across every bars-generator variant tried, ruling out fixture design as the cause. **No fixture or test change was committed for this attempt** -- the exploratory `research-py/tests/support/build_r3_e2e_fixture.py` / `core-rs/crates/mqk-cli/src/commands/research_replay.rs` edits were reverted (working tree matches `18d989530a8d4f247b04859f2ef0e6eeee1a72b3`) rather than committed, per this wave's explicit "do not fix the defect inside this patch" instruction. **STATUS: OPEN — NEEDS PROOF.** Blocker: the replay-backtest order-fill idempotency defect above must be independently reproduced and repaired (a real, dedicated, narrowly-scoped patch -- likely in `mqk-backtest`'s order/fill retry path) before any positive-fixture R3.5 proof can be attempted again. **NOT AN APPROVED PATCH. NOT PART OF ACTIVE 43-PATCH COUNT. NOT AUTHORIZED FOR IMPLEMENTATION.**
