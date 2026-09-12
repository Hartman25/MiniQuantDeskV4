---
name: mqd-test-proof
description: Validate whether test/proof evidence actually proves the invariant claimed, before it is trusted. Use when auditing a "PASS" or "green" claim for a DB-backed, provider, Paper, or CI-load-bearing scenario, or when a patch's proof needs a false-positive check.
---

# mqd-test-proof

Answers one question: does this evidence actually prove the invariant being
claimed? This skill assigns evidentiary verdicts only — it does not assign
project acceptance/closure states (`CLOSED`/`OPEN`/`PARKED` in
`audit_repo_truth_rules.md` are controller/operator states).

## Procedure

1. State the exact claim being made, in one sentence.
2. State the proof class it requires: `CODE/TEST`, `DB-BACKED`,
   `PROVIDER/LIVE-READ-ONLY`, `PAPER OPERATIONAL`, or `CI`.
3. Work through the questions below against the actual evidence (test code,
   fixture, run output) — not against a description of it.
4. Answer the mandatory falsifiability question.
5. Return a verdict.

## Questions

- Did the test exercise real production code, or a shallow helper/mock
  standing in for it?
- If this is a regression test, did the pre-fix implementation actually go
  RED on it? If historical RED cannot be reproduced, is there a valid
  mutation test or negative control instead?
- Could expected and observed both independently collapse to
  None/default/empty and still "match"?
- Could a missing file, hash, or dataset make this test pass for the wrong
  reason?
- Did the test silently skip (e.g., a DB/provider connection failed and the
  asserted branch never ran)?
- Is the expected value independently derived, or copied from the same
  implementation being tested?
- Did the test bypass the production wrapper the real code path runs
  through?
- Did chronology change in a way that could hide lookahead/leakage?
- Is provenance/identity genuinely exercised, or inferred from a
  filename/column/caller-supplied string (CLAUDE.md §20)?
- Is result data contaminating identity?
- Is authority (who may act) confused with integrity (a hash proving bytes
  weren't altered, not that the source is authorized)?
- Does a DB claim have DB proof? A provider claim provider evidence? A Paper
  claim actual Paper evidence? A CI claim actual CI evidence (not
  local-only)?

**Mandatory falsifiability question:** what observation would make this
proof fail even if the implementation agent expected it to pass? If no
credible answer exists, flag the proof as weak.

**Mandatory DB-specific rule:** a positive DB-backed proof requires a
positive execution signal — an observed DB row/effect, a test-specific
executed-path marker, or a real query/result tied to the asserted seam.
Absence of a "SKIP" string is not sufficient; a silently-skipped branch on
connection failure is the most common false-positive in this class.

## Verdicts

- `CONFIRMED` — evidence genuinely establishes the claim at the required
  proof class.
- `LIKELY` — evidence is consistent with the claim but has a named gap.
- `UNKNOWN-NEEDS-PROOF` — the required proof class has not yet been
  produced.
- `REJECTED-FALSE-PROOF` — the evidence looks like proof but doesn't
  establish the claim (name why: wrapper bypass, tautological fixture,
  silent skip, wrong tier, copied expected value, etc.).

## Hard stops

- Never assign `CLOSED`, `ACCEPTED`, `INDEPENDENTLY ACCEPTED`,
  `PUSHED-VERIFIED`, or `WAVE`/`MILESTONE CLOSED`. Those are
  controller/operator states.
- Do not repair the evidence or the code yourself; report the verdict and
  the gap.
- Do not invoke another skill; hand back to the mission/controller.
