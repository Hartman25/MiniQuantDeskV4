# Patch Packet Template — MiniQuantDesk V4

Use this for real patch work. Keep it narrow.

---

## Patch ID
- **Patch ID:**
- **Short title:**

## Patch type
Choose one:
- narrow bug fix
- truth-contract patch
- fixture fix
- proof-depth patch
- test promotion
- wiring closure
- refactor with unchanged semantics
- docs-only adjacent clarification
- research-side isolated EXP patch

## Controlling authority
List only the docs/tests/contracts that actually control this patch.

- 
- 
- 

## Primary goal
State the exact thing that must become true after this patch.

## Non-goals
State what this patch is not allowed to become.

- 
- 
- 

## Strict scope

### Files to inspect first
- 
- 
- 
- 

### Files allowed only if strictly required
- 
- 
- 

### Files out of scope
- 
- 
- 

## Current behavior / defect
Describe the current weakness only with grounded facts.

## Required outcome
Be explicit.

Examples:
- a mounted route remains mounted but becomes fail-closed and semantically honest
- a failing fixture inserts a valid enum under the current DB constraint
- the GUI distinguishes authoritative empty from unavailable truth
- an EXP research patch stays entirely outside MAIN truth and proof burden

### Your outcome
- 

## Constraints
- do not widen scope
- do not redesign adjacent architecture
- do not patch unrelated docs
- preserve existing semantics unless the patch explicitly changes them
- one patch at a time
- full updated file(s) or full requested section only

## Required proof / validation
List the exact command(s) that must be run.

- 
- 
- 

## Success criteria
A patch is not closed unless all of these are satisfied.

- 
- 
- 

## Known risks
- 
- 
- 

## Deliverable format
Choose exactly one and say it explicitly.

- full updated files only
- full updated sections only
- review findings only
- audit findings only

## MAIN vs EXP check
Fill this out if relevant.

- **Does this touch MAIN canonical truth?**
- **Does this touch EXP only?**
- **Does it widen proof burden?**
- **Does it risk operator ambiguity?**
- **If yes, stop or narrow scope.**
