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
2. Pull current milestone/patch/status from the current mission and the
   canonical ledger (`MiniQuantDesk_Master_Patch_Ledger_v2.md`) — reference
   it by name, do not copy its table into the handoff.
3. List accepted/frozen contracts and do-not-reopen items relevant to the
   active area by pointer (doc path or rule file under `.claude/rules/`),
   not by restating their text.
4. List completed patches from the current session only: patch ID, commit
   SHA, status (`CLOSED`/`OPEN`/`PARKED` per `audit_repo_truth_rules.md`),
   proof type, acceptance status. An implementation agent's own "done"
   claim is not acceptance — say so explicitly if acceptance is still
   pending.
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
(patch ID / commit SHA / status / proof type / acceptance status — this
session only)

CURRENT DETERMINISTIC DEFECT

KNOWN BLOCKERS / DEPENDENCIES

EXACT NEXT MISSION

HARD STOPS / AUTHORITY LIMITS
```

By default, return this in conversation output. Do not create a Markdown
file unless the current mission explicitly asks for a persisted artifact.

## Hard stops

- Never state or imply `INDEPENDENTLY ACCEPTED`, `PUSHED-VERIFIED`,
  `WAVE CLOSED`, or `MILESTONE CLOSED` — those are controller/operator
  states, not derivable from a handoff.
- Never let this document become authoritative after HEAD changes; it is a
  snapshot, not a ledger.
- No secrets (CLAUDE.md §23).
- Do not invent readiness or copy large permanent status narratives —
  reference the canonical ledger/runbook/contract doc instead.
- Do not invoke another skill; hand back to the mission/controller.
