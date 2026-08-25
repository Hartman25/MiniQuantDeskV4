# Safe Cleanup Plan — 2026-08-24

Mission: LEDGER-CLOSURE-CONSOLIDATION-01-CONTROLLER, Patch H.

This is a **plan only**. No worktree, branch, patch file, or run evidence is
removed by this controller. See
`docs/audits/2026-08-24_branch_worktree_consolidation_audit.md` for the
classification evidence behind each entry below, and the archive at
`C:\Users\Zacha\Desktop\MiniQuantDeskV4-wave-archive-20260824\ARCHIVE_MANIFEST.txt`
for the disposable artifacts already copied out (Patch G).

## PROTECTED_NEVER_CLEAN

- `smoke_logs/` (primary repo). Never delete/reset/clean, per CLAUDE.md §24
  and `.claude/rules/audit_repo_truth_rules.md`.
- `.env.local` (primary repo and any worktree).

## MUST_PRESERVE

Unfinished, in-progress, or unverified work — do not delete branches or
worktrees:

- `paper-soak-session-1-repair` branch + `MiniQuantDeskV4-soak-repair`
  worktree — `UNFINISHED_PRESERVE`, still needs
  `PAPER-SOAK-OUTBOX-ENQUEUE-RUN-STATE-FENCE-01`.
- `research-direct-rank-policy-01` branch + `MiniQuantDeskV4-direct-rank-policy`
  worktree — `UNFINISHED_PRESERVE`, predeclared SHORT wave 03 not yet run.
- `research-v2-factor-wave` branch + `MiniQuantDeskV4-research-v2` worktree
  — `UNKNOWN_NEEDS_PROOF`. Its "already in main" premise did not hold (13
  commits, none patch-equivalent to main); needs an explicit operator
  decision (merge, rebase-and-merge, or formally abandon) before any
  deletion is even considered.
- `gui-backtest-workbench` branch + `MiniQuantDeskV4-gui-backtest` worktree
  — `UNKNOWN_NEEDS_PROOF`, no independent-acceptance record found.
- `review/ai-ml-local-lab-foundation-01` (remote) / `ai/ml-local-lab-foundation-01`
  branch + `MiniQuantDeskV4-ai-lab` worktree — `UNKNOWN_NEEDS_PROOF`.
- Detached-HEAD worktrees `MiniQuantDeskV4-autofresh`, `-data`, `-integration`,
  `-mcp`, `-ops`, `-retry` — content not yet reconciled into main; per
  memory these correspond to prior unmerged repair sessions
  (`MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01*`,
  `MARKET-DATA-PROVIDER-PROVENANCE-01`, `OFFICIAL-DUAL-MODE-LAUNCHER-01`).
- `paper-soak-20260818` branch + `MiniQuantDeskV4-paper-soak` worktree, and
  `paper-soak-repair-20260819` branch + `MiniQuantDeskV4-paper-soak-repair`
  worktree — `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT` (see below); preserved
  until that audit is done, not deleted now.
- `codex/audit-last-two-patches-and-fix-stuck-state` (remote-only, no local
  worktree) — `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT`.
- `codex/apply-determinism-fixes-det01` branch + `.codex/worktrees/2915`
  worktree — `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT` (upstream remote gone).
- `research-alpha-gap-discovery-01` (non-clean) branch +
  `MiniQuantDeskV4-alpha-discovery` worktree — `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT`,
  likely superseded by `research-alpha-gap-discovery-01-clean` but not yet
  proven redundant.
- `claude/intelligent-bose-dfe00b`, `claude/trusting-perlman` — 1 unique
  commit each, not in main — `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT`.

## SAFE_AFTER_UNIQUE_COMMIT_AUDIT

Candidates above marked `STALE_REQUIRES_UNIQUE_COMMIT_AUDIT` become safe to
delete (branch + worktree) only after a future session:

1. Runs `git log <branch> --not main --not <all other retained branches>`
   (or per-branch `git cherry`) to prove zero unique, non-superseded commits
   remain, **or** explicitly promotes/merges the unique content first.
2. Confirms no open memory/ledger item still depends on the branch.

This applies to: `paper-soak-20260818`, `paper-soak-repair-20260819`,
`codex/audit-last-two-patches-and-fix-stuck-state`,
`codex/apply-determinism-fixes-det01`, `research-alpha-gap-discovery-01`
(non-clean), `claude/intelligent-bose-dfe00b`, `claude/trusting-perlman`.

## SAFE_AFTER_BRANCH_MERGED

- `research-alpha-gap-discovery-01-clean` branch +
  `MiniQuantDeskV4-alpha-discovery-clean` worktree — content is fully
  contained in `research-short-wave-02`, which is now merged into
  `ledger-closure-integration-01`. Safe to delete only *after*
  `ledger-closure-integration-01` (or its eventual merge into `main`) is
  itself accepted/pushed — not before, and not by this controller.
- `research-short-01-etf-trend` branch + `MiniQuantDeskV4-short-01-etf-trend`
  worktree — same condition (subsumed by `research-short-wave-02`).
- `pre-soak-resilience-wave-01` branch + `MiniQuantDeskV4-pre-soak-resilience`
  worktree, and `research-short-wave-02` branch +
  `MiniQuantDeskV4-short-wave-02` worktree — now merged into
  `ledger-closure-integration-01` via `--no-ff` merge commits. Safe to
  delete only after `ledger-closure-integration-01` is itself accepted and
  (if applicable) pushed/merged to `main` — not before.

## SAFE_TO_DELETE_NOW

- `research-factor-v2` branch (local, no worktree) — byte-identical to
  current `main` HEAD at inventory time (`edcda740`), zero unique commits.
  Note: because `main` has since advanced (Patches B/F added commits),
  re-verify `git merge-base --is-ancestor research-factor-v2 main` returns
  `YES` immediately before deleting, to confirm it is still a pure subset.
- Local `claude/*` branches already proven `ALREADY_IN_MAIN` (ancestors of
  `main`, 0 unique commits): `claude/agitated-lumiere-f7c208`,
  `claude/ai-lab-foundation-repair-bfca18`,
  `claude/bundle-5-runtime-allocation-744794`, `claude/busy-bardeen-9c0e9a`,
  `claude/miniquantdeskv4-ai-ml-lab-b56dae`,
  `claude/miniquantdeskv4-reconciliation-60a6e1`, `claude/modest-driscoll`,
  `claude/optimistic-bohr-96b041`, `claude/premarket-script-guard-repair-b23d9b`,
  `claude/wizardly-darwin-aee3db`. Their associated worktrees
  (`.claude/worktrees/agitated-lumiere-f7c208`,
  `.claude/worktrees/bundle-5-runtime-allocation-744794`,
  `.claude/worktrees/busy-bardeen-9c0e9a`,
  `.claude/worktrees/optimistic-bohr-96b041`,
  `.claude/worktrees/premarket-script-guard-repair-b23d9b`) may be removed
  alongside their branches. Re-verify ancestry immediately before deleting,
  same caveat as above.
- `codex/implement-migration-governance` branch + `.codex/worktrees/b992`
  worktree — proven ancestor of `main`, 0 unique commits. Same
  re-verify-before-delete caveat.
- Everything already copied into
  `C:\Users\Zacha\Desktop\MiniQuantDeskV4-wave-archive-20260824\` per the
  Patch G manifest (`.proof/`, `RESEARCH_CLOSEOUT_PATCH_A_REVIEW.patch`,
  the two `PAPER_SOAK_SESSION_1_REPAIR_*.txt` files, the alpha-discovery
  patch file, the three research `runs/` directories, and the stray
  `.playwright-mcp` snapshot) is disposable/reproducible and hash-verified
  in the archive — safe to delete from its original worktree location once
  the operator confirms the archive is retained. **Not deleted by this
  controller.**

## Explicitly out of scope for this plan

`ledger-closure-integration-01` itself (the new consolidation branch/worktree
created in Patch C) is **not** a cleanup candidate — it is the active work
product of this mission and remains until independently reviewed and
disposed of by the operator.
