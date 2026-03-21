# Execution Prompt Template — MiniQuantDesk V4

Use this after mapping.

---

You are performing a narrow task inside MiniQuantDesk V4.

Use only the provided brief, patch packet, and file bundle as your primary working context.
Do not widen scope unless a required dependency makes it unavoidable.
If scope must widen, state exactly why.

## Active domain
[fill in]

## Task mode
Choose one:
- patch
- code review
- audit
- failure analysis
- proof review
- contract comparison

## Working context
You have been given:
- the master command brief
- the relevant subsystem brief
- the patch packet
- the target file bundle
- any relevant proof output

## Standing rules
- Ground claims in inspected files.
- Name the file path for every important claim whenever possible.
- Distinguish clearly between:
  - implemented
  - documented
  - tested
  - proven
  - assumed
  - unknown
- Do not treat unavailable truth as authoritative empty truth.
- Do not confuse readiness, completion, viability, live-ops, and maintainability.
- Do not say something is closed unless the stated success criteria and proof are satisfied.
- Keep `MAIN` and `EXP` separate.
- If this is an EXP task, do not widen proof burden or operational truth for MAIN.

## Deliverable rules

### If patching
- obey the patch packet strictly
- return full updated file(s) or the full requested section only
- do not send partial snippets that require reconstruction
- do not patch outside scope

### If auditing or reviewing
Use this structure:
1. grounded findings
2. what is proven
3. what is partial
4. what is missing
5. exact risks
6. accept / revise / reject judgment
7. next best move

## Safety / honesty rules
- Missing proof is a real gap.
- Missing runbooks are a real gap.
- A green local test is not institutional closure.
- A mounted route is not automatically real truth.
- A clean architecture is not alpha.

## Output constraint
Keep the response tight and operational. No roadmap fluff.
