---
name: mqd-review-patch
description: Read-only review of a patch/diff along three separate axes — contract, quality, proof. Use only when the current mission/controller explicitly requests patch/wave review, or the current workflow has reached an explicitly defined acceptance boundary — not for routine/ordinary edits. Never edits, commits, or assigns acceptance status.
---

# mqd-review-patch

Read-only. This skill inspects; it never fixes, commits, pushes, or assigns
acceptance/closure status. Project acceptance/closure states remain owned by
the current mission/controller and canonical program authority.

## When to use

Use only when:

- the current mission/controller explicitly requests patch/wave review; or
- the current workflow has reached an explicitly defined acceptance
  boundary.

Do not auto-invoke this skill for every ordinary edit or diff.

Review the diff between the stated fixed point (baseline SHA, branch, or
`HEAD~N`) and `HEAD`, along three separate axes. Report them **separately**
— do not merge or re-rank across axes. A change can pass one axis and fail
another.

## 1. Contract review

Check the diff against: the requested invariant, the current mission's
stated scope, frozen contracts (CLAUDE.md §6), fail-closed behavior (§3),
broker/OMS authority (§4, `broker_rules.md`), execution/orchestrator
authority (`execution_rules.md`), DB write-path/migration discipline
(`db_rules.md`), GUI truth-state discipline (`gui_rules.md`), chronology,
provenance (§20), research trial/identity discipline (§19), Paper/Live
separation (§22), scope creep, and accidental reopening of accepted work.

## 2. Quality review

Check for: unnecessary complexity, duplicated logic, speculative
abstractions, shallow/pass-through modules, wrong seam placement, poor
locality, excessive historical/narrative comments (§11), unnecessary
parallel frameworks/helpers where an existing seam applies (§12), and
general maintainability.

## 3. Proof review

Apply the `mqd-test-proof` question set directly (or hand off to that skill
if the mission invokes it separately): RED-capable test, real production
seam, negative controls, mutation proof where useful, false-positive
fixture risk, silent skip, wrapper bypass, wrong evidence tier, an expected
value derived from the same implementation being tested, assertions
incapable of detecting recurrence.

## Severity

`BLOCKER`, `HIGH`, `MEDIUM`, `LOW`, `INFORMATIONAL`.

**A BLOCKER on the contract axis is never downgraded by green tests on the
proof axis.** A fail-closed gate turned optimistic-allow is a contract
BLOCKER regardless of how much passes.

## Output

Report all three axes, even if one is empty. For each finding: axis,
severity, file:line, what's wrong, why it matters. End with a one-line count
per axis — do not pick one overall winner across axes (that reranking is
exactly what the separation exists to prevent).

## Hard stops

- No edits, no commits, no pushes, no acceptance/closure status.
- May recommend a repair; may not implement it.
- Do not invoke `mqd-diagnose` or any other skill to fix what you found;
  hand the findings back to the mission/controller.
