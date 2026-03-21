# AI Working Rules for MiniQuantDesk V4

These rules apply to ChatGPT, Claude, Codex, and any other model used for non-trivial repo work.

## Core operating rules

1. Do not widen scope unless required by a direct dependency.
2. Do not confuse implementation with documentation.
3. Do not confuse mounted surfaces with authoritative backend truth.
4. Do not confuse empty data with authoritative empty truth.
5. Do not confuse test existence with fully wired behavior.
6. Do not confuse a green proof slice with total project closure.
7. Keep readiness, completion, viability, live-ops, and maintainability separate.
8. Do not treat `MAIN` and `EXP` as equivalent.
9. State unknowns instead of smoothing over them.
10. Do not claim closure without file and proof grounding.

## Project-specific hard rules

1. `MAIN` is the only canonical engine unless explicitly changed by operator decision.
2. `EXP` remains research-side, non-canonical, non-operator-facing, and non-readiness-bearing unless explicitly promoted.
3. EXP work must not widen MAIN proof burden.
4. One patch at a time is the default rule.
5. Do not patch unless explicitly asked.
6. When patching, return the full updated file or the full requested section only.
7. Do not fabricate backend truth or operator status.
8. Prefer fail-closed behavior where truth is unavailable.
9. If docs and code disagree, code/runtime/proof win.
10. If proof is missing, say it is missing.

## File-grounding rules

- Name the file path for important claims whenever possible.
- If the owning file was not inspected, mark the claim provisional.
- Treat trackers, runbooks, prompts, and summaries as secondary unless explicitly controlling.
- Treat DB-backed state and canonical scenario proof as higher authority than narrative summaries.

## Scope-control rules

- Start from the subsystem brief and patch packet.
- Prefer the smallest useful file bundle over broad repo exploration.
- If scope must widen, state exactly why and where.
- Avoid “just in case” edits.
- Do not turn narrow fixes into architecture rewrites.

## Proof and closure rules

- “Implemented” is not the same as “tested.”
- “Tested” is not the same as “proven in canonical proof flow.”
- “Passing locally” is not the same as “institutionally ready.”
- “Ready” is not the same as “complete.”
- “Complete” is not the same as “economically viable.”

## MAIN vs EXP rules

- MAIN owns current operator truth.
- MAIN owns current live-ops truth.
- MAIN owns current readiness truth.
- EXP may share platform primitives but must not share operational truth.
- EXP must stay out of daemon truth, GUI truth, canonical metrics surfaces, and readiness proof unless explicitly promoted.

## Deliverable rules by task type

### If auditing
Return:
- grounded findings
- what is proven
- what is partial
- what is missing
- what changed
- next best move

### If reviewing
Return:
- what is correct
- what is weak
- what is missing
- exact risk level
- whether to accept, revise, or reject

### If patching
Return only the requested deliverable format and stay inside scope.

## Operator safety rules

- Do not assume someone can “figure it out later.”
- Missing runbooks or missing proof are real gaps.
- Do not smooth over live-ops risk with architecture quality.
- Do not smooth over trading viability risk with platform cleanliness.

## Style rules

- Be direct.
- Separate proven from assumed.
- Correct wrong framing instead of following it.
- Do not add roadmap fluff.
- Do not hide uncertainty.
