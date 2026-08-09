# PAPER-SOAK-READINESS-AND-ZERO-FAILURE-CLOSURE-01 — Closure Report — 2026-08-09

## 1. Verdict

```text
READY_TO_BEGIN_FORMAL_PAPER_SOAK
```

All 9 mandatory patches are closed with independent investigation, a failing regression test proving each bug before its fix, negative-control verification, and an independent diff review. The full validation matrix (git integrity, workspace `fmt`/`clippy -D warnings`, complete test suite, canonical safe-ignored-test matrix, script guards, migration governance, GUI build/typecheck/tests, Python tests) passes clean at final HEAD. This report does not authorize or claim the soak itself has begun — only that the repository is ready for a human operator to begin one.

## 2. Starting state

- Baseline SHA: `0dd72bc4` (docs-only commit closing [`AUDIT-02` / `end_to_end_intent_behavior_conformance_2026-08-04.md`](end_to_end_intent_behavior_conformance_2026-08-04.md), classification `NONCONFORMANT`).
- The 9-patch sequence was derived from that audit's numbered findings (A2-FIND-001 through A2-FIND-013), each independently re-verified against current HEAD before implementation, never trusted from the audit doc alone.
- Working tree at mission start: two protected untracked paths (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`, `smoke_logs/`), neither read, edited, staged, or deleted at any point in this mission.
- Database policy honored throughout: only the disposable test Postgres at `127.0.0.1:5434`/`mqk_test` was used for any DB-backed proof. Ports 5432 (live) and 5440 (paper) were never touched.

## 3. Commit table

| # | Commit | Patch ID | Kind |
|---|---|---|---|
| 1 | `69fb5edf` | PAPER-SOAK-PARTIAL-FILL-DEDUP-01 | fix |
| 2 | `1d834dcf` | PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01 | fix |
| 3 | `556a5c1e` | PAPER-SOAK-DAILY-TERMINAL-FINALIZATION-01 | test-only (no code change authorized — see §4.3) |
| 4 | `e4e399ad` | PAPER-SOAK-STALE-CLAIM-RECOVERY-01 | fix |
| 5 | `c87c511d` | RUN-LIFECYCLE-CAS-01 | fix |
| 6 | `28a8f0f8` | STRATEGY-DECISION-IDEMPOTENCY-01 | fix |
| 6b | `3f1218df` | (fmt drift attributable to #4, kept separate per one-patch-per-commit discipline) | style |
| 7 | `8ce19818` | OPERATOR-RISK-UNKNOWN-TRUTH-01 | fix |
| 8 | `828f2121` | LIVE-CAPITAL-PARITY-COMPLETE-GATE-01 | fix |
| 8b | `0f85853b` | (fmt drift attributable to #8) | style |
| 9 | `cbedfca8` | REPLACE-CAPABILITY-TRUTH-01 | fix |
| — | `988a9d01` | Validation-matrix remediation: `check_unsafe_patterns` guard violations (attributable to #4 and #1, never run per-patch) | fix |
| — | `960ac159` | Validation-matrix remediation: canonical ignored-test inventory classification for new tests added by #3/#4/#7 | test |
| — | `eef9c063` | Validation-matrix remediation: `Invoke-CanonicalSafeIgnoredMatrix.ps1` PowerShell stream-ordering bug, unrelated to any of the 9 patches (pre-existing, found by running the script for the first time in this mission) | fix |

14 commits total: 9 patch commits (one of which is test-only by design), 2 formatting-drift commits kept separate per one-patch-per-commit discipline, and 3 validation-matrix-remediation commits for gaps the matrix itself surfaced.

## 4. Per-patch proof summary

Full investigation detail, exact code excerpts, and test names for each patch are recorded in session memory (`project_paper_soak_readiness_zero_failure_closure_01` and linked files) and in each commit's own message. Summary:

### 4.1 `69fb5edf` — PAPER-SOAK-PARTIAL-FILL-DEDUP-01 (CATASTROPHIC)
Confirmed A2-FIND-003: real Alpaca WS `trade_updates` never carry `broker_fill_id`, REST `activity_to_trade_update` always sets one — the same partial fill delivered via both lanes got two different dedupe identities and was applied twice. Migration 0040 fixed this exact pattern for terminal fills and explicitly excluded partial fills. Fixed via a transactional, advisory-lock-guarded economic-match window (`mqk-db::inbox::inbox_insert_partial_fill_deduped`). Found and fixed a second bug during review: two recovery call sites hardcoded `event_ts_ms=0`, which would have broken the new window.

### 4.2 `1d834dcf` — PAPER-SOAK-INBOUND-DRAIN-OWNERSHIP-01 (HIGH)
Confirmed A2-FIND-005: `stop_execution_runtime` cleared local ownership unconditionally with no broker-order-resolution check, silently dropping late WS frames with zero logging and allowing false-clean finalization. Fixed via `runs.stop_requested_at_utc` (additive column) plus a pre-check refusing stop while any broker-reachable order remains unresolved.

### 4.3 `556a5c1e` — PAPER-SOAK-DAILY-TERMINAL-FINALIZATION-01 (test-only, no code change)
The mission brief's framing was a false premise: `classify_autonomous_daily_outcome`'s handling of ACKed-but-unresolved orders is the governing spec's deliberate design (`docs/specs/autonomous_daily_paper_operations_01e_outcome_truth_contract.md` §5 tier 2), not a bug. The real gap was finalization *eligibility*, already closed as an emergent property of patch #2. Verified end-to-end with a new regression test, not a production change — rewriting the classifier as the brief literally requested would have contradicted an already-audited, currently-governing contract.

### 4.4 `e4e399ad` — PAPER-SOAK-STALE-CLAIM-RECOVERY-01 (HIGH)
Confirmed A2-FIND-002: `outbox_reset_stale_claims` (crash-recovery reaper) existed, was unit-tested, had zero production call sites. Wired into `build_execution_orchestrator`, the single construction seam every run start and restart-resume calls unconditionally. Added `run_id` scoping (was system-wide across every run).

### 4.5 `c87c511d` — RUN-LIFECYCLE-CAS-01 (HIGH)
`arm_run`/`begin_run`/`stop_run`/`clear_halted_run`/`heartbeat_run` all did check-then-act with no `status` guard on their `UPDATE` — a race could silently overwrite `HALTED` back to `STOPPED`, breaking `HALTED`'s required sticky/fail-closed semantics. Fixed with compare-and-set on every transition (reusing `status` itself as the CAS token); `halt_run` deliberately left unconditional as the override that must win.

### 4.6 `28a8f0f8` — STRATEGY-DECISION-IDEMPOTENCY-01 (CATASTROPHIC)
Confirmed A2-FIND-001: `decision_id` was seeded with live wall-clock `now_micros`, read fresh every 1-second loop tick. The loop legitimately re-evaluates the same still-current completed bar many times before a new bar closes; every re-evaluation minted a fresh `decision_id`, defeating the outbox's `ON CONFLICT DO NOTHING` dedup — a real duplicate-live-order path. Fixed by seeding from `bar_end_ts` (durable, bar-anchored evidence) instead of wall-clock, matching the already-proven pattern in `runtime_opportunity_allocation::compute_cycle_id`.

### 4.7 `8ce19818` — OPERATOR-RISK-UNKNOWN-TRUTH-01 (HIGH)
Confirmed A2-FIND-006: `/api/v1/risk/summary` collapsed a genuine DB query error into a confirmed `kill_switch_active: false` via `.ok().flatten()`. Independent blast-radius investigation confirmed this was isolated to this one route (the other four `kill_switch_active` sites already fail loud or carry their own `truth_state`). Added `truth_state` to the response; `kill_switch_active` now fail-closed `true` whenever truth isn't confirmed by a successful DB read. Extended the same fail-closed default through the GUI's fetch-failure fallbacks and stopped `RiskScreen.tsx` from coloring the kill switch green on unconfirmed truth.

### 4.8 `828f2121` — LIVE-CAPITAL-PARITY-COMPLETE-GATE-01 (HIGH policy-integrity)
Confirmed A2-FIND-007, but investigation found something more nuanced than a simple bug: `LIVE-TRUST-01`'s own regression-locked tests explicitly assert `ParityEvidenceOutcome::is_start_safe()` must NOT block on `live_trust_complete=false` (a deliberate decoupling so LiveShadow can run with parity evidence present but trust incomplete) — while `evaluate_mode_transition` simultaneously already advertises that upward transitions into LiveCapital are fail-closed for exactly that reason, but only at the advisory `mode-change-guidance` route, never at actual cold start. Closed the gap with a new LiveCapital-only TV-03D gate at the real start path, leaving TV-03C's LiveShadow-safe contract untouched. Since the real TV-03 pipeline can never currently produce `live_trust_complete=true`, this is a pure safety tightening — no currently-supported path was broken. Two existing test files (`scenario_capital_policy_tv04f.rs`, `scenario_live_capital_lo03de.rs`) needed updating because they relied on the same permissiveness being closed, as the direct consequence of the fix rather than a regression.

### 4.9 `cbedfca8` — REPLACE-CAPABILITY-TRUTH-01 (dormant)
Confirmed A2-FIND-013 cleanly: `AlpacaBrokerAdapter::replace_order` discarded Alpaca's PATCH response and echoed back the stale pre-replace broker order id, while Alpaca's replace endpoint actually creates a new broker order with a new id. Confirmed replace is exhaustively unreachable from any production dispatch path today (outbox only recognizes submit/cancel; zero non-test callers of `BrokerGateway::replace`). Fixed the identity-discard bug via a pure, directly-unit-testable extraction function, without attempting the much larger, separate initiative of wiring replace into production dispatch.

## 5. Validation matrix results

| Check | Result |
|---|---|
| Git integrity (`git fsck`, working-tree cleanliness) | Clean (dangling loose objects only, no corruption; only the two pre-existing protected untracked paths present) |
| Workspace `cargo fmt --check` (21 packages) | Clean |
| Workspace `cargo clippy --workspace --all-targets -- -D warnings` | Clean |
| Workspace `cargo test --workspace --lib` | Clean — ~1775 tests, 0 failed (mqk-daemon 762/0/11 matches pre-mission baseline exactly) |
| Workspace `cargo test --tests` (all 21 packages, mqk-daemon's 237 files chunked to avoid a linker-memory limit on this machine) | Clean — 0 failed across every chunk and every other package |
| Migration governance (`check_migration_governance.sh`) | Clean (no migrations touched by this mission) |
| Script-guard aggregator (`run_all_script_guards.ps1`, ~57 guards) | Clean (after fixing 2 real `check_unsafe_patterns` violations — see §6) |
| Canonical safe-ignored-test matrix (`Invoke-CanonicalSafeIgnoredMatrix.ps1`) | Clean (after fixing 1 pre-existing script bug — see §7) |
| GUI production build + typecheck | Clean |
| GUI test suite | Clean — 977/0 |
| Python test suite | Clean — 988 passed, 5 skipped, 0 failed |

## 6. Gaps the validation matrix surfaced beyond the 9 patches

`check_unsafe_patterns.ps1`/`.sh` was never run as part of any individual patch's own verification loop during this mission — only at final validation. It caught two real, if minor, issues (commit `988a9d01`):

1. A test fixture in `hermetic_positive_proofs.rs` (introduced by patch #4) used `Uuid::new_v4()`, which the guard forbids anywhere in `src/`, including `#[cfg(test)]` blocks. Replaced with a deterministic `Uuid::new_v5`, matching this codebase's established identity-generation convention.
2. Two `timestamp_millis()` calls in `repair.rs`/`ws_gap_recovery.rs` (introduced by patch #1) were false positives — both parse a broker-supplied REST activity timestamp, not a wall-clock read — annotated with `// allow: broker-sourced timestamp, not wall-clock`.

Recommendation for future missions: run `check_unsafe_patterns` as part of every patch's own verification loop, not just at final validation.

## 7. A pre-existing tooling bug found and fixed, unrelated to any of the 9 patches

`Invoke-CanonicalSafeIgnoredMatrix.ps1`'s Step 1c initially failed by reporting 20 real tests as unclassified under `scenario_daily_data_readiness_01` — a file those tests do not exist in (confirmed: they live in `scenario_daemon_runtime_lifecycle.rs`, and `scenario_daily_data_readiness_01.rs` has zero diff against the mission's baseline SHA). Root-caused empirically: the script correlates cargo's `--all-targets -- --ignored --list` output by matching each stderr "Running ..." header to the stdout test names that follow, and PowerShell's `2>&1` *variable* capture does not reliably preserve chronological order between the two streams for an external process — most exposed on a cold build (the `manual-external` feature's first-ever compile in a session). Fixed (commit `eef9c063`) by redirecting both streams to a file via `*>` (a genuine OS-level merged handle) instead, confirmed via direct A/B comparison and a full clean matrix rerun. The script's own 30 pre-existing unit tests still pass unmodified. This was investigated and fixed at the user's explicit direction after being flagged as an out-of-scope discovery.

## 8. Ignored-test accounting

- Total canonical inventory: 702 rows (696 pre-mission + 6 added for tests this mission's own patches introduced).
- `SAFE_LOCAL`: 8. `SAFE_DB_5434`: 685 (both executed by Step 2 of the canonical matrix — 693 total, 0 failures). `MANUAL_EXTERNAL`: 9 (excluded from execution, compile-proven only — these require real Alpaca credentials and are expected to fail without them, by design). `BLOCKED_LOCAL_PREREQUISITE`: 0.
- The 6 new rows: patch #3's `d11_stop_retry_defers_finalization_while_order_unresolved_then_stops_once_resolved`; patch #7's `risk_summary_confirmed_db_state_reports_active_truth`; patch #4's 4 stale-claim-recovery DB round-trip tests. All correctly classified `SAFE_DB_5434`.

## 9. Database and external-impact report

- No production database (port 5432) or paper database (port 5440) was ever connected to, queried, or modified by this mission.
- All DB-backed proof — every patch's own tests, and the full validation matrix's Step 2 execution of 693 tests — ran exclusively against the disposable test Postgres at `127.0.0.1:5434`/`mqk_test`.
- No real broker (Alpaca or otherwise) was ever called. No live or paper order routing occurred. No real credentials were loaded.
- One piece of environment hygiene performed on the disposable test DB only: the shared singleton `runtime_leader_lease` row (`id=1`) was cleared (`DELETE FROM runtime_leader_lease WHERE id = 1`) several times between ad-hoc test invocations in this same session, when a stale not-yet-expired lease from an earlier invocation caused unrelated tests to fail with a lease-conflict error. This is routine, expected hygiene on a disposable test database, not a production action — documented in session memory for future sessions.
- ~88GB of stale, already-obsolete `C:\tmp\mqk-target-*` build-cache directories from prior, already-closed sessions were deleted mid-mission after disk space (99% full) caused build corruption; this followed the same narrow, previously-established precedent for this specific class of scratch directory (documented in session memory).

## 10. Remaining items / explicitly out of scope

- The `README_TECHNICAL.md`/other documentation drift noted by the originating audit (§5 of `end_to_end_intent_behavior_conformance_2026-08-04.md`) was not addressed — out of this mission's 9-patch scope.
- Fully wiring order replace into production dispatch (as opposed to fixing the dormant identity-discard bug, per §4.9) remains a separate, materially larger initiative — deliberately not attempted.
- No live daemon process, real Alpaca WS session, or actual market-hours trading activity was exercised by this mission. The soak itself has not begun.

## 11. Final git state

- HEAD: `eef9c063`.
- Branch `main`, 14 commits ahead of `origin/main` at report time — exactly the 14 commits in §3, all from this mission. Nothing pushed by this mission; push was never requested or performed.
- Working tree: clean except the same two pre-existing protected untracked paths present at mission start (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`, `smoke_logs/`), neither touched.

## 12. Next authorized action

This report authorizes a human operator to review the above and decide whether to begin a formal, supervised 10-20 session Paper+Alpaca soak. It does not itself start, schedule, or authorize any daemon start, broker connection, or trading activity. Per this mission's own vocabulary: the correct status is `READY_TO_BEGIN_FORMAL_PAPER_SOAK`, never `CLOSED` for the soak itself, since the soak is a separate, operator-initiated activity outside this mission's scope.

## 13. Proposed patch-ledger entry

`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md` is untracked and protected; per this mission's own rules it was not modified directly. Proposed entry for the operator to incorporate at their discretion:

```text
PAPER-SOAK-READINESS-AND-ZERO-FAILURE-CLOSURE-01 — CLOSED_LOCAL
  69fb5edf 1d834dcf 556a5c1e e4e399ad c87c511d 28a8f0f8 3f1218df
  8ce19818 828f2121 0f85853b cbedfca8 988a9d01 960ac159 eef9c063
  9/9 mandatory patches closed; full validation matrix green.
  See docs/audits/paper_soak_readiness_zero_failure_closure_2026-08-09.md.
```
