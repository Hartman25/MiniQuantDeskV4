# Mapping Prompt Template — MiniQuantDesk V4

Use this before asking for a patch or deep analysis when the area is still fuzzy.

---

You are performing a narrow repo-mapping pass for MiniQuantDesk V4.

This is a mapping task only.
Do NOT patch code.
Do NOT rewrite files.
Do NOT give roadmap fluff.
Do NOT widen scope past the target area.
Do NOT assume docs, tests, or trackers equal real implementation.

## Repo-wide standing rules
- Treat `docs/INSTITUTIONAL_READINESS_LOCK.md` and `docs/INSTITUTIONAL_SCORECARD.md` as the controlling authority for readiness judgments when relevant.
- Keep readiness, completion, viability, live-ops, and maintainability separate.
- Keep `MAIN` and `EXP` separate.
- If the task is EXP-side, do not widen into MAIN operational truth.

## Objective
Map the minimal authoritative context needed for the target task.

## Target area
[fill in subsystem / patch area]

## Required output
Return only these sections:

### 1. Likely authority files
List the smallest set of files that appear to own the target behavior.
For each file include:
- path
- why it matters
- confidence level

### 2. Adjacent-but-secondary files
List supporting files that may matter but should not be treated as primary truth.

### 3. Dependency chain
Show the minimal relevant dependency path for this task.

### 4. Unknowns / ambiguity
List anything that cannot be safely concluded without more file inspection.

### 5. Proposed minimal inspection set
Give the smallest file bundle needed for the actual work.

### 6. Scope risks
List the most likely ways this task could drift or accidentally widen.

## Special instruction
If this target area is one of the known sink-file zones (`state.rs`, `routes.rs`, `orchestrator.rs`, DB layer, GUI system API layer), explicitly identify whether the real ownership may live in helpers/tests/contracts outside the sink file.
