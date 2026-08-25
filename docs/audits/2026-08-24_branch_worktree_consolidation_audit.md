# Branch / Worktree Consolidation Audit — 2026-08-24

Mission: LEDGER-CLOSURE-CONSOLIDATION-01-CONTROLLER, Patch B.

Baseline verified before this audit:

- `main` HEAD = `edcda740b2f05fbe8a2657f2301b8ea373efb4b6`
- `origin/main` = `edcda740b2f05fbe8a2657f2301b8ea373efb4b6` (exact match)
- Working tree: no tracked drift; only untracked `smoke_logs/` residue and
  `RESEARCH_CLOSEOUT_PATCH_A_REVIEW.patch` present (both pre-existing,
  untouched by this controller).

Classification vocabulary used below: `MERGE_ACCEPTED`, `ALREADY_IN_MAIN`,
`UNFINISHED_PRESERVE`, `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT`,
`SAFE_DELETE_AFTER_FINAL_INTEGRATION`, `UNKNOWN_NEEDS_PROOF`.

## 1. Mission-named branches (independently verified)

### `pre-soak-resilience-wave-01` — 66e3b3f1
- Worktree: `MiniQuantDeskV4-pre-soak-resilience`, tracked-clean.
- `merge-base(main, branch)` = main HEAD exactly; 14 commits ahead of main, 0
  behind. `origin/pre-soak-resilience-wave-01` present and matches tip.
- **Classification: `MERGE_ACCEPTED`** — confirmed as expected. Target of
  Patch D.

### `research-short-wave-02` — f0113f68
- Worktree: `MiniQuantDeskV4-short-wave-02`. Tracked-clean; 91 untracked
  lines are experiment run artifacts under
  `research-py/experiments/short_wave_02/runs/run_01/` (data outputs, not
  tracked source).
- 17 commits ahead of main, 0 behind. `origin/research-short-wave-02`
  present and matches tip.
- **Classification: `MERGE_ACCEPTED`** — confirmed as expected. Target of
  Patch E.

### `research-alpha-gap-discovery-01-clean` — 28497968
- Worktree: `MiniQuantDeskV4-alpha-discovery-clean`. 1 untracked file
  (`0001-research-fail-closed-on-ineffective-placebo.patch`), no tracked
  drift.
- **Verified**: `git merge-base --is-ancestor research-alpha-gap-discovery-01-clean research-short-wave-02` → **YES**.
- **Classification: `MERGE_ACCEPTED` (subsumed by `research-short-wave-02`
  — do not merge separately, per mission instruction)**.

### `research-short-01-etf-trend` — e31a4914
- Worktree: `MiniQuantDeskV4-short-01-etf-trend`. Tracked-clean; 32
  untracked lines are experiment run artifacts under
  `research-py/experiments/short_01_etf_trend/runs/run_01/`.
- **Verified**: `git merge-base --is-ancestor research-short-01-etf-trend research-short-wave-02` → **YES**.
- **Classification: `MERGE_ACCEPTED` (subsumed by `research-short-wave-02`
  — do not merge separately)**.

### `research-v2-factor-wave` — 0d6bf957
- Worktree: `MiniQuantDeskV4-research-v2`, tracked-clean.
- Mission's stated expectation was "already represented in main." **This
  does NOT hold**: `git merge-base --is-ancestor research-v2-factor-wave main`
  → NO. 13 commits ahead of main, 0 behind, merge-base = main HEAD.
- `git cherry main research-v2-factor-wave` marked **all 13 commits `+`**
  (not patch-equivalent to anything on main) — i.e. none of this content
  exists on main under a different hash (no evidence of a squash/rebase
  merge either).
- **Classification: `UNKNOWN_NEEDS_PROOF`** — contradicts the mission's
  starting assumption. This branch's factor-registry/FDR/exposure-attribution
  work (13 commits, e.g. `7e843f9a` canonical factor contract through
  `0d6bf957` rank/neutralization invariants) is NOT in main and was NOT
  merged by this controller. Needs an explicit operator decision before any
  future merge or deletion.
- Separately noted: a **different**, similarly-named local branch
  `research-factor-v2` exists at tip `edcda740` — byte-identical to current
  main HEAD (0 ahead / 0 behind). This looks like a stray/duplicate branch
  pointer (possibly a mis-typed clone of `research-v2-factor-wave` that was
  never advanced). It carries zero unique commits. Not touched in this
  controller; flagged for Patch H.

### `paper-soak-session-1-repair` — 4125d364
- Worktree: `MiniQuantDeskV4-soak-repair`. **No tracked modifications** —
  only 2 untracked evidence files (`PAPER_SOAK_SESSION_1_REPAIR_FINAL_TEST_PROOF.txt`,
  `PAPER_SOAK_SESSION_1_REPAIR_MQK_DAEMON_RETRY.txt`). The mission's concern
  about a controller interrupted mid-Patch-A leaving uncommitted tracked
  state does **not** materialize — worktree is clean.
- 14 commits ahead of main, 0 behind. Local-only, no remote tracking branch.
- Per memory (`project_paper_soak_session_watch_01_stale_operation_defect.md`):
  still requires `PAPER-SOAK-OUTBOX-ENQUEUE-RUN-STATE-FENCE-01` before it can
  be considered complete.
- **Classification: `UNFINISHED_PRESERVE`** — confirmed as expected.

### `research-direct-rank-policy-01` — 46aa8ecb
- Worktree: `MiniQuantDeskV4-direct-rank-policy`, fully tracked- and
  untracked-clean.
- 22 commits ahead of main, 0 behind. Local-only, no remote tracking branch.
- Latest commit: "research: predeclare broad direct-rank SHORT wave 03" —
  a predeclaration commit, consistent with in-progress/unfinished research.
- **Classification: `UNFINISHED_PRESERVE`** — confirmed as expected.

### `gui-backtest-workbench` — 15aef54b
- Worktree: `MiniQuantDeskV4-gui-backtest`, clean. 9 commits ahead of main,
  0 behind. `origin/gui-backtest-workbench` present and matches tip.
- No ledger/memory record found marking this independently accepted.
- **Classification: `UNKNOWN_NEEDS_PROOF`** — do not merge or delete
  without unique-commit proof, per mission instruction.

### `codex/audit-last-two-patches-and-fix-stuck-state` — 3079a10e (remote-only)
- No local branch, no local worktree.
- 1 commit ahead of main ("Fix daemon cancel proof test shutdown hang"),
  1124 behind main — very stale relative to current main.
- **Classification: `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT`** — do not merge
  or delete without unique-commit proof, per mission instruction.

### `review/ai-ml-local-lab-foundation-01` — 11f3d571 (remote)
- Tip is byte-identical to local branch `ai/ml-local-lab-foundation-01`
  (worktree `MiniQuantDeskV4-ai-lab`, tracked-clean). 8 commits ahead of
  main, 294 behind.
- **Classification: `UNKNOWN_NEEDS_PROOF`** — do not merge or delete
  without unique-commit proof, per mission instruction. Content is fully
  captured by the local `ai/ml-local-lab-foundation-01` branch, so the two
  should be treated as one unit going forward.

## 2. Additional branches/worktrees discovered during inventory

Not named in the mission but present in the repo; included for completeness
per Patch B's "every local and remote branch" instruction. None are merged,
deleted, or modified by this controller.

| Branch | Tip | vs main | Classification | Note |
|---|---|---|---|---|
| `codex/implement-migration-governance` | 86688d7f | ancestor of main (0 ahead / 1204 behind) | `ALREADY_IN_MAIN` | fully subsumed |
| `codex/apply-determinism-fixes-det01` | 2b357fd8 | 1 ahead of its own base, 1204 behind main | `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT` | remote branch shows `gone`; 1 unique commit "Remove wall-clock timestamp" |
| `research-alpha-gap-discovery-01` (non-clean) | ef482240 | 1 ahead of main | `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT` | worktree `MiniQuantDeskV4-alpha-discovery`, 41 status lines untriaged; likely superseded by `-clean` variant above |
| `paper-soak-20260818` | 5b7879c7 | 9 ahead / 36 behind (old base `fd90f63a`) | `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT` | worktree `MiniQuantDeskV4-paper-soak`; tracked "modifications" to 19 migration files are line-ending-only artifacts (empty `git diff`/`--raw` content) — not real edits; likely superseded by `paper-soak-repair-20260819` / `paper-soak-session-1-repair` |
| `paper-soak-repair-20260819` | d50b1bbd | 12 ahead / 36 behind (old base `fd90f63a`) | `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT` | worktree `MiniQuantDeskV4-paper-soak-repair`, clean; per memory this fix was already cherry-picked into `paper-soak-session-1-repair` — verify no unique remainder before any deletion |
| `research-factor-v2` | edcda740 | identical to main | `SAFE_DELETE_AFTER_FINAL_INTEGRATION` | zero unique commits; see note under `research-v2-factor-wave` above |
| `claude/agitated-lumiere-f7c208`, `claude/ai-lab-foundation-repair-bfca18`, `claude/bundle-5-runtime-allocation-744794`, `claude/busy-bardeen-9c0e9a`, `claude/miniquantdeskv4-ai-ml-lab-b56dae`, `claude/miniquantdeskv4-reconciliation-60a6e1`, `claude/modest-driscoll`, `claude/optimistic-bohr-96b041`, `claude/premarket-script-guard-repair-b23d9b`, `claude/wizardly-darwin-aee3db` | various | all ancestors of main (0 ahead) | `ALREADY_IN_MAIN` | fully subsumed; safe cleanup candidates |
| `claude/intelligent-bose-dfe00b` | bfa183bc | 1 ahead / 891 behind | `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT` | 1 unique commit not in main |
| `claude/trusting-perlman` | bb858900 | 1 ahead / 975 behind | `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT` | 1 unique commit not in main |

Detached-HEAD worktrees pointing at commit `e63a31706954f21fa7b5ed48d018576e15bb39d0`
(`MiniQuantDeskV4-autofresh`, `-data`, `-integration`, `-ops`, `-retry`) and
at `38f074a9ca29f57de48e90366a4448e71a3f4db6` (`-mcp`) have no branch
attached. `-integration` and `-ops` carry untracked residue (19 and 50
status lines respectively, no tracked drift); the rest are clean. These
correspond to prior single-purpose repair worktrees documented in memory
(`MARKET-DATA-AUTOFRESH-*`, `MARKET-DATA-PROVIDER-PROVENANCE-01`,
`OFFICIAL-DUAL-MODE-LAUNCHER-01`) — all already noted there as not merged.
**Classification: `MUST_PRESERVE`** (detached, no branch to lose, but
worktree content not yet reconciled into main).

## 3. Summary for Patch C–E

Only two branches meet `MERGE_ACCEPTED` with an independent tip to merge in
this controller:

1. `pre-soak-resilience-wave-01` @ `66e3b3f1` (Patch D)
2. `research-short-wave-02` @ `f0113f68` (Patch E, already contains
   `research-alpha-gap-discovery-01-clean` and `research-short-01-etf-trend`)

`research-v2-factor-wave` is explicitly **not** merged in this controller —
its "already in main" premise did not hold and it requires a separate
operator decision.

## 4. Patch F corrections — 2026-08-24 (LEDGER-CLOSURE-CONSOLIDATION-REPAIR-02)

This controller (a later mission on the same date) merged the two branches
the prior audit above left as `UNKNOWN_NEEDS_PROOF`, resolved the resilience
validation ambiguity, and closed the `EMPTY_INTERRUPTED_WORKTREE` /
`STALE_REQUIRES_UNIQUE_COMMIT_AUDIT` items named in its mission. Superseding
corrections only — Section 1–3 above stand as the historical record of what
was true at that earlier point in time.

### `research-v2-factor-wave` — 0d6bf957
- **Corrected classification: `MERGED_ACCEPTED`.** Verified
  `origin/research-v2-factor-wave` == `0d6bf957d47311708feadf8a1c870abd1ef6fa89`
  exactly; merge-base with integration HEAD = `origin/main`. Merged with
  `git merge --no-ff` (merge commit `254902a3`), zero conflicts. Focused
  suite (`test_factor_contract_identity/diagnostics/exposure/fdr/
  null_controls/registry/universe.py` + `test_exp_distributed.py`, the
  direct test for the modified `exp_distributed/storage.py`): **146 passed**.

### `gui-backtest-workbench` — 15aef54b
- **Corrected classification: `MERGED_ACCEPTED`.** Verified
  `origin/gui-backtest-workbench` == `15aef54b43b93436f563e7eb980cb2e3455398df`
  exactly; merge-base with integration HEAD (post Factor-V2 merge) =
  `origin/main`. Merged with `git merge --no-ff` (merge commit `0df36eb0`),
  zero conflicts. `npm ci` + focused test run (`backtests/__tests__/
  {parsers,api,pathHelpers}.test.ts`): **300 passed**. `tsc --noEmit`: clean.
  `cargo check -p mqk-gui` (Tauri Rust seam, `lib.rs` was touched by this
  branch): clean.

### `pre-soak-resilience-wave-01` resilience-test ambiguity — resolved
- Prior controller's report of `test_offsite_b2_workflow.ps1` "11 failures"
  attributed to missing real B2 credentials is **not reproduced**. Ran the
  full transcript under Windows PowerShell 5.1 against both the original
  worktree (`MiniQuantDeskV4-pre-soak-resilience`) and the integration
  worktree (`ledger-closure-integration-01`, post-merge): **identical
  result on both — "All proofs held. 0 violations." exit 0.**
  Environment at time of this run: `restic 0.19.1` installed,
  `docker 29.5.3` available, `mqk-test-postgres` container already up (no
  disposable dependency needed to be started).
- `HISTORICAL_11_FAILURE_CAUSE=UNKNOWN_NEEDS_PROOF`.
  `CURRENT_SOURCE_VALIDATION=PASS`. `CURRENT_INTEGRATION_VALIDATION=PASS`.
  `CURRENT_INTEGRATION_REGRESSION=NO_EVIDENCE`. The test's own contract
  (Section 3 docstring) confirms the local-restic end-to-end proof is the
  functional acceptance gate, and real-B2 credentials are a separate,
  explicitly-deferred operational proof — so the prior attribution of the 11
  historical failures to missing real B2 credentials was incorrect, because
  the committed test does not require real B2 credentials for its local
  functional proof. However, the exact historical cause of the 11 prior
  failures cannot be reconstructed from the evidence retained here: the
  earlier failing transcript was not preserved in the independent review
  bundle, and non-reproduction under current prerequisites (`restic 0.19.1`,
  `docker 29.5.3`, `mqk-test-postgres` already running) does not prove which
  prerequisite, if any, caused the historical failures. No current
  integration regression found; no repair performed on the accepted branch.

### `research-direct-rank-policy-01` — contradiction resolved
- A later interactive inventory in this mission's chain claimed this
  worktree was "completely clean" at `f0113f68` (== `research-short-wave-02`
  tip, i.e. empty/interrupted). **That claim is false.** Re-verified
  directly: `HEAD` = `46aa8ecbe5a23ab503cb26f6492dcccab6c49979`, branch
  `research-direct-rank-policy-01`, working tree clean (no untracked files),
  **5 unique committed commits** on top of `f0113f68`
  (`93a6e76a`..`46aa8ecb`: dynamic cross-sectional rank policy, trial-identity
  binding, broad-universe snapshot identity, and a predeclared SHORT wave 03),
  **2984 lines of tracked diff** across 10 files.
- **Classification: `UNFINISHED_PRESERVE`** (unchanged from the prior
  audit's Section 1 entry — the interim "clean" claim was incorrect and is
  superseded by this direct verification).

### AI/ML lab — `ai/ml-local-lab-foundation-01` / `review/ai-ml-local-lab-foundation-01`
- **Corrected classification: `ARCHIVE_DONOR_DO_NOT_MERGE`** (was
  `UNKNOWN_NEEDS_PROOF`). Per explicit prior operator decision (mission
  instruction, not re-derived here): historical donor/reference material
  only, not to be merged into current development. Not deleted — future
  cleanup should bundle/archive before removing refs.

### `codex/audit-last-two-patches-and-fix-stuck-state` — 3079a10e
- **Corrected classification: `SUPERSEDED_SAFE_TO_ARCHIVE_DELETE`** (was
  `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT`). Its single unique commit ("Fix
  daemon cancel proof test shutdown hang") changes
  `core-rs/crates/mqk-daemon/src/state.rs`'s `inject_running_loop_for_test`
  from an unconditional 24h sleep to a `tokio::select!` that also races a
  `stop_rx.changed()` signal. Diffed against integration HEAD
  (`core-rs/crates/mqk-daemon/src/state.rs:5205-5218`): **the identical
  `tokio::select!`-with-`stop_rx.changed()` shape is already present**, and
  integration has since evolved further (an added
  `leadership_release_outcome: None` field on both arms that the codex
  branch predates). The shutdown-hang behavior this commit fixed is fully
  superseded by current integration semantics; no unique behavior remains
  to review.

### Detached-HEAD worktrees — re-classified individually
The prior audit grouped all six under one `MUST_PRESERVE` line. Re-verified
each independently against both `origin/main` and integration HEAD
(`0df36eb0`):

| Worktree | HEAD | Tracked | Untracked | Ancestor of origin/main | Ancestor of integration HEAD | Classification |
|---|---|---|---|---|---|---|
| `-autofresh` | `e63a3170` | clean | none | YES | YES | `SAFE_TO_REMOVE_AFTER_CONSOLIDATION_ACCEPTANCE` |
| `-data` | `e63a3170` | clean | none | YES | YES | `SAFE_TO_REMOVE_AFTER_CONSOLIDATION_ACCEPTANCE` |
| `-mcp` | `38f074a9` | clean | none | YES | YES | `SAFE_TO_REMOVE_AFTER_CONSOLIDATION_ACCEPTANCE` |
| `-retry` | `e63a3170` | clean | none | YES | YES | `SAFE_TO_REMOVE_AFTER_CONSOLIDATION_ACCEPTANCE` |
| `-integration` | `e63a3170` | clean | `.playwright-mcp/`, `smoke_logs/` | YES | YES | `PRESERVED_WORKTREES` — untracked residue needs review/archival before removal |
| `-ops` | `e63a3170` | clean | `smoke_logs/` | YES | YES | `PRESERVED_WORKTREES` — untracked residue needs review/archival before removal |

### `research-factor-v2` (local branch + `MiniQuantDeskV4-research-factor` worktree)
- Re-verified per the prior audit's caveat: `HEAD` = `edcda740` still
  exactly equals `origin/main` (main has not advanced past this since the
  prior audit — Patch A of this mission restored primary `main` to
  `origin/main` exactly). Zero unique commits.
- **Classification: `SAFE_TO_REMOVE_AFTER_CONSOLIDATION_ACCEPTANCE`.**
