---
name: mqd-diagnose
description: Deterministic diagnosis loop for hard MiniQuantDesk defects. Use when a bug, invariant violation, or regression needs isolation before a fix is authorized — not for routine debugging with an obvious cause.
---

# mqd-diagnose

Procedural diagnosis only. Authority for what may be fixed comes from the
current mission/controller, not this skill — see CLAUDE.md's project-local
skills section.

## Loop

1. **Reported defect.** State the exact symptom as reported, verbatim.
2. **Define the exact failure signal.** One sentence: what observation,
   precisely, means "still broken."
3. **Identify the required proof class** (below). Do not accept a weaker
   class as evidence for a stronger claim.
4. **Build one tight, red-capable loop** — the smallest command/test/script
   that can go red on this exact defect and green once fixed. Prefer, in
   order: a failing test at the real seam, a CLI/API invocation against a
   running component, a minimal harness, a differential/bisection loop.
   Never enable Live and never fabricate provider/broker/Paper activity to
   build the loop.
5. **Prove red.** Run the loop. Confirm it reproduces the reported symptom,
   not an adjacent one.
6. **Minimize the repro.** Cut inputs/state one at a time until every
   remaining element is load-bearing.
7. **Rank falsifiable hypotheses (3-5)** before testing any of them. Each
   must state a prediction: "if X is the cause, changing Y makes the bug
   disappear." Discard hypotheses that make no prediction.
8. **Run a discriminating probe** against the top hypothesis, one variable
   at a time. Prefer targeted inspection/logging at the boundary that
   distinguishes hypotheses over broad instrumentation.
9. **Add a regression test at the real production seam** — not a shallow
   helper that bypasses the wrapper the bug actually lived in. If no
   correct seam exists, that absence is itself a finding: report it.
10. **Apply the fix only if the current mission/controller already
    authorized implementation of this exact defect/patch.** Otherwise stop
    here and report:
    - CONFIRMED DEFECT
    - SMALLEST PROPOSED REPAIR
    - REQUIRED PROOF (which proof class, and what evidence would satisfy it)
11. **Prove green** at the regression seam, then **replay the original
    (un-minimized) failure loop** to confirm it no longer reproduces.
12. **Run adjacent regressions** at the narrowest scope covering the
    touched seam (CLAUDE.md §15 test pyramid).

## Proof classes

`CODE/TEST`, `DB-BACKED`, `PROVIDER/LIVE-READ-ONLY`, `PAPER OPERATIONAL`, `CI`.

A pass in one class does not prove another. A green `cargo test` does not
prove a DB-backed branch executed — see `mqd-test-proof` to validate that. A
CI-green run is CI proof, not Paper-operational proof.

## Hard stops

- Do not fix a defect the current mission has not authorized, even one found
  while diagnosing something else. Report it instead.
- Do not treat "test completion" as proof — a skipped or short-circuited
  branch is not evidence.
- Do not reproduce by enabling Live, submitting broker orders, or
  synthesizing fills/acks/cancels (CLAUDE.md §4, §22).
- Do not invoke another skill from inside this one; hand back to the
  mission/controller.
- Never assign `CLOSED`, `ACCEPTED`, `INDEPENDENTLY ACCEPTED`,
  `PUSHED-VERIFIED`, or `WAVE`/`MILESTONE CLOSED` — those are
  controller/operator states, not this skill's to grant.
- Redact secrets in every shown command/output (CLAUDE.md §23).

## Output when stopped short of a fix

```
CONFIRMED DEFECT: <one line>
SMALLEST PROPOSED REPAIR: <one line or short diff sketch>
REQUIRED PROOF: <proof class + what would satisfy it>
```
