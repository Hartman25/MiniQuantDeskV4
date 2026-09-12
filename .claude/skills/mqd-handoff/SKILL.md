---
name: mqd-handoff
description: Generate a compact, SHA-bound engineering handoff snapshot for the next session. Use when a mission/session ends and work needs to continue, or when asked for a status handoff.
---

# mqd-handoff

A snapshot at one commit, not a second status ledger. Derive every git fact
from the repo itself — do not invent or recall from memory.

## Required header

```
SNAPSHOT AT HEAD <sha>
NOT AUTHORITATIVE AFTER HEAD CHANGES
```

## Procedure

1. Derive: `git rev-parse --abbrev-ref HEAD`, `git rev-parse HEAD`,
   `git rev-parse origin/main`, `git status --porcelain` (worktree state).
2. Pull current milestone/patch/status from the current mission, current
   Git truth, and the canonical program/status authority
   (`MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md`) — reference it by
   name, do not copy its table into the handoff.
3. List accepted/frozen contracts and do-not-reopen items relevant to the
   active area by pointer (doc path or rule file under `.claude/rules/`),
   not by restating their text.
4. List completed patches from the current session only: patch ID, commit
   SHA, proof type, and acceptance status. Report acceptance status only
   when it is explicitly recorded by the current controller/operator or the
   canonical program authority — never infer it. Committed code, passing
   focused tests, or an implementation agent's own "done" claim are not
   acceptance; if no authoritative acceptance record exists, report
   `ACCEPTANCE PENDING` (or another plainly non-acceptance factual
   description) instead of assigning a status.
5. State the current deterministic defect (if any), known
   blockers/dependencies, and the exact next mission, one paragraph each.
6. State hard stops / authority limits still in force (e.g., no push, no
   Live, patch-specific scope freeze).

## Output

Required sections, in order:

```
AUTHORITATIVE REPOSITORY
BRANCH
HEAD
ORIGIN/MAIN
WORKTREE STATE

CURRENT MILESTONE
CURRENT PATCH
CURRENT STATUS

ACCEPTED/FROZEN CONTRACTS
DO NOT REOPEN ITEMS

COMPLETED PATCHES
(patch ID / commit SHA / proof type / acceptance status — this session
only; acceptance status is `ACCEPTANCE PENDING` unless explicitly recorded
by the current controller/operator or canonical program authority)

CURRENT DETERMINISTIC DEFECT

KNOWN BLOCKERS / DEPENDENCIES

EXACT NEXT MISSION

HARD STOPS / AUTHORITY LIMITS
```

By default, return this in conversation output. Do not create a Markdown
file unless the current mission explicitly asks for a persisted artifact.

## Hard stops

- This skill reports status; it does not invent or reinterpret status.
  Project acceptance/closure states remain owned by the current
  mission/controller and the canonical program authority.
- Never state or imply `CLOSED`, `INDEPENDENTLY ACCEPTED`,
  `PUSHED-VERIFIED`, `WAVE CLOSED`, or `MILESTONE CLOSED` for a patch unless
  that exact status is explicitly recorded by the current
  controller/operator or the canonical program authority — not merely
  because code is committed, focused tests pass, or an implementation agent
  says done.
- Never let this document become authoritative after HEAD changes; it is a
  snapshot, not a ledger.
- No secrets (CLAUDE.md §23).
- Do not invent readiness or copy large permanent status narratives —
  reference the canonical program authority/runbook/contract doc instead.
- Do not invoke another skill; hand back to the mission/controller.
