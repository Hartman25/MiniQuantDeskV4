# MiniQuantDesk V4 — Claude Operating Contract

These rules apply to all work in this repository unless the CURRENT mission
explicitly overrides a specific rule.

MiniQuantDesk is a deterministic trading and research platform.

Canonical operational engine: `MAIN`.
Experimental/non-canonical work: `EXP`.
`EXP` is research-only and is not operational truth unless explicitly promoted.

## 1. Priority

Optimize in this order:

1. Correctness
2. Preserve accepted contracts
3. Fail-closed safety
4. Determinism
5. Reproducibility/auditability
6. Restart/crash/idempotency safety
7. Efficient context/tool use
8. Speed
9. Verbosity

Do not sacrifice correctness to save tokens.

Do not spend tokens, tool calls, or test time on work that does not materially
increase confidence in the CURRENT patch.

The goal is to FINISH the system credibly, not endlessly redesign it.

## 2. Repository = durable memory

Use committed repository content as long-term project memory:

- code/tests/schema
- Git history
- subsystem contracts
- audits/ledgers
- protocol documentation.

At mission start:

1. read the mission;
2. verify the requested Git baseline;
3. read the relevant authoritative subsystem document;
4. inspect only the code needed for the active patch.

For Research/Backtest work, use when relevant:

`docs/research/Research_Backtest_V1_Closeout_Audit.md`

Code/tests are authoritative for actual behavior.
Accepted contract docs are authoritative for frozen design intent.

Do not re-audit settled architecture unless current evidence contradicts it.

## 3. Core safety invariants

### Determinism

Prefer:

- stable ordering
- canonical serialization
- fixed seeds
- deterministic fixtures
- content-addressed identity.

Do not depend on unspecified filesystem, SQL, API, hash-map, or row ordering.

### Fail closed

When authoritative truth is unavailable:

- block
- reject
- mark unavailable/unsupported/not_evaluable
- or raise an explicit error.

Never fabricate evidence or optimistic success.

### Durable safety

Every durable write path that may retry/resume must preserve:

- idempotency
- restart safety
- crash safety.

## 4. Broker / OMS invariants

Broker truth is authoritative for broker lifecycle events.

Never synthesize:

- fills
- acknowledgements
- broker cancellation confirmations
- broker order-state transitions.

Canonical flow:

outbox enqueue
→ broker submit
→ broker truth
→ inbox
→ portfolio/accounting.

Do not bypass durable lifecycle transitions.

## 5. Operator truth

Distinguish:

- UNAVAILABLE
- EMPTY
- PRESENT.

Do not represent unavailable authority as an authoritative empty result.

No fabricated truth or optimistic defaults where authority matters.

## 6. Frozen contracts

Accepted patches/invariants are CLOSED.

Do not reopen them because another design looks cleaner.

Reopen only for NEW deterministic evidence such as:

- reproducible invariant failure
- lookahead/leakage
- incorrect P&L chronology
- unsafe fail-open behavior
- incorrect statistical methodology
- incorrect provenance/data semantics
- contradiction between frozen contract and production path.

If that contradicts the current mission:

STOP and report the smallest exact defect.

## 7. One patch at a time

A patch = ONE coherent invariant.

A Claude session MAY contain multiple patches when the mission explicitly
defines an autonomous wave, but execute them sequentially:

NARROW INSPECTION
→ DEFINE INVARIANT
→ RED/NEGATIVE CONTROL WHEN APPROPRIATE
→ IMPLEMENT
→ FOCUSED TEST
→ COMPLETE DIFF REVIEW
→ DIRECT REGRESSIONS
→ REQUIRED ACCEPTANCE SUITE
→ COMMIT
→ COMPACT CHECKPOINT
→ NEXT PATCH.

One patch = one commit.

Never mix unrelated fixes into one commit.

Do not perform unrelated cleanup/refactoring.

If an unrelated defect appears, record it. Repair it only if the mission
authorizes it.

At a defined wave checkpoint, STOP for independent review.

## 8. Progressive discovery

Do not recursively inspect the repository by default.

Use this order:

1. mission + authoritative contract
2. named files/modules
3. exact symbol/function/type search
4. direct callers/callees/tests
5. adjacent subsystem only if necessary
6. broad repo search only if narrower discovery fails.

Prefer targeted symbol/index search or restricted `rg` over large file/repo
dumps.

Do not repeatedly reread unchanged files.

## 9. Keep active context small

Active working state should contain roughly:

- CURRENT HEAD
- CURRENT PATCH
- FROZEN INVARIANTS
- RELEVANT FILES
- CURRENT DEFECT
- LOAD-BEARING TEST
- CURRENT BLOCKER.

After committing a patch, compress it to:

PATCH:
COMMIT:
STATUS:
PROOF:
BLOCKER/DEPENDENCY:

Do not carry long histories of already-committed patches.

## 10. Minimal narration

Do not narrate every obvious tool call or thought.

Good:

"Defect confirmed: provenance manifest is not bound to loaded bars."

"Focused tests pass; running required regressions."

"Patch P7A committed; beginning P7B."

Final reports should contain evidence, not a transcript.

Do not expose private chain-of-thought.

## 11. Keep source comments lean

Do not put chat history, patch-history essays, audit transcripts, or mission
narratives into production comments/docstrings.

Comments should explain only:

- a non-obvious invariant
- why a surprising choice is necessary
- what must remain true
- relevant protocol/version when useful.

Long history belongs in `docs/`.

## 12. Reuse existing seams

Before adding architecture, inspect existing:

- types
- adapters
- registries
- protocols
- hashing helpers
- error types
- artifact systems
- CLI conventions
- test fixtures.

Prefer extending accepted seams over creating parallel frameworks.

Use the smallest clear implementation satisfying the full invariant.

## 13. Test quality > test count

Ask:

"If the old bug returned, would this test actually fail?"

Prefer:

- parameterized/table-driven tests
- adversarial fixtures
- mutation proof
- pre-fix RED / post-fix GREEN

over many shallow duplicates.

Especially use strong negative controls for:

- lookahead
- holdout leakage
- execution timing
- trial inflation
- provenance bypass
- corporate-action contamination
- identity mistakes
- Python/Rust parity
- fail-open behavior.

## 14. False-positive fixture check

For identity/provenance/chronology/statistical tests, explicitly check whether
the test could pass because both sides contain the same:

- None
- default
- empty value
- nonexistent file
- missing hash
- empty dataset.

Prefer exercising the real production path.

## 15. Test pyramid

While developing:

1. run the single load-bearing test
2. run its test file
3. run adjacent regressions
4. run subsystem/full suite once at the acceptance boundary.

Prefer concise output:

`pytest ... -q --tb=short`

Use `-x` during debugging when useful.

Do not run the entire Rust workspace for Python-only changes.

Do not rerun an unchanged full green suite without reason.

## 16. Subagents

Use subagents only when work is genuinely independent and substantial.

Good:

- broad multi-domain audit
- primary-paper research independent of repo inspection
- independent review of a high-risk patch.

Bad:

- multiple agents reading the same files
- focused three-file repairs
- duplicate searches/tests.

Prefer one primary implementation agent for focused patches.

## 17. Tool / MCP routing

Evidence priority:

1. repository code/tests/schema
2. local read-only data
3. version-matched official docs
4. primary papers/provider docs
5. secondary sources only when needed.

Use the least expensive authoritative tool.

### Srclight / indexed search

Use for definitions, references, callers/callees.
Stop searching once the relevant seam is known.

### Context7

Use only for uncertain current/version-specific third-party API/library
behavior.

Do not use for ordinary syntax or behavior already established locally.

### Firecrawl

Use only when current external facts materially affect correctness.

Prefer official provider docs and original papers.

Retrieve only the relevant sections.

Do not broadly crawl the web.

### Playwright

Use only for actual browser-facing behavior.

Do not use for backend-only work where unit/API/CLI tests suffice.

Never use it to enable Live trading or submit broker orders.

## 18. External docs vs local reality

External docs answer what a provider/library SHOULD mean.

Repository code/data answer what MiniQuantDesk ACTUALLY does.

If they conflict:

STOP or report the contradiction.

Do not silently force them to agree.

## 19. Statistical research

Use primary papers for load-bearing methodology.

Once a method is verified and frozen, do not re-research it during unrelated
work.

Keep distinct:

HYPOTHESIS = idea
TRIAL = unique candidate
ATTEMPT = invocation/retry
SLICE/JOB = evaluation window/job.

Retries/windows do not inflate unique trials.

`fwd_ret` or another prediction label is not executable P&L unless an accepted
protocol explicitly says otherwise.

Final holdout remains untouched unless a mission explicitly authorizes
consumption.

Consumed holdout data is never fresh again.

## 20. Data provenance

Never infer strong data truth from a filename, column name, or caller-provided
string.

Prefer:

canonical semantic identity
+
versioned provenance
+
physical artifact evidence.

A hash proves integrity, not authority.

Synthetic evidence is valid for tests/diagnostics but must not silently
authorize official research.

Fail closed on unknown provider/adjustment/provenance/authority.

## 21. Multi-asset discipline

Current work may target US equities/ETFs while preserving future seams for:

- futures
- options
- FX
- crypto.

Do not implement future asset classes prematurely.

Desired architecture:

shared research/statistical core
+
asset-specific data/execution/accounting adapters.

Generalize now only when current work genuinely requires the seam or deferring
it would cause known major rework.

## 22. Paper / Live safety

Do not modify or operate Paper/Live/runtime systems unless explicitly required
by the mission.

Never fabricate activity.

Never enable Live.

Never submit broker orders as routine validation.

## 23. Secrets

Never expose:

- API keys
- tokens
- cookies
- broker credentials
- DB passwords
- `.env`
- `.env.local`
- private auth headers.

Never place secrets in source, tests, artifacts, logs, commits, or MCP calls.

## 24. Git discipline

At mission start when requested, verify:

- branch
- HEAD
- origin/main
- working tree.

If baseline differs, STOP.

Do not repeatedly run `git status` after every edit.

Never use:

- `git reset`
- `git stash`
- `git clean`

unless explicitly authorized.

`smoke_logs/` is protected residue.

Never stage/delete/reset/clean it.

Use targeted staging; avoid `git add .`.

One patch = one commit.

Do not push unless the CURRENT mission explicitly authorizes it.

## 25. Diff discipline

Before committing, review the complete patch diff.

Check for:

- unrelated files/refactors
- weakened fail-closed behavior
- accidental chronology changes
- unintended protocol/identity changes
- result-dependent identity
- temporary/debug code
- false-positive tests
- duplicate helpers/frameworks
- oversized comments.

Run `git diff --check` at acceptance boundaries.

## 26. Keep tool output small

Restrict searches to relevant paths/patterns.

Use concise test output.

Do not dump giant unchanged files, logs, or diffs into context when a focused
section or summary is sufficient.

If an approach fails twice for the same reason, stop and reassess instead of
repeating it.

## 27. Completion is finite

A patch is not COMPLETE merely because tests are green.

Completion requires:

- real production path satisfies the invariant
- load-bearing negative proof where appropriate
- no known deterministic contradiction remains in scope
- fixtures genuinely prove the behavior
- accepted contracts are preserved
- required regressions pass.

Once a subsystem's documented completion gate is met:

STOP ADDING INFRASTRUCTURE.

Mark non-required improvements DEFERRED.

## 28. Manual code patches

Claude Code normally edits files directly.

Do not dump modified functions into the final report unnecessarily.

If the user must manually apply a code patch, provide the WHOLE affected
function/method/type/impl/coherent section.

Only one manual patch at a time unless explicitly requested otherwise.

## 29. Final report

Do not reproduce the mission prompt or tool transcript.

Unless the mission specifies otherwise, report:

VERDICT
STARTING HEAD
PATCH / INVARIANT
FILES CHANGED
NEGATIVE CONTROL / RED-GREEN PROOF
FOCUSED TESTS
ACCEPTANCE SUITE
COMMIT SHA
DIFF CHECK
FINAL STATUS
BLOCKERS
NEXT PATCH.

Keep it concise but independently reviewable.

## 30. Default decision rule

Before a large search, MCP call, subagent launch, full suite, or broad file
read ask:

"Will this materially increase confidence in the CURRENT patch?"

If no, skip it.

Before adding a feature ask:

"Is this required by the current documented completion gate?"

If no, defer it.

When several correct approaches exist, prefer the one that:

1. preserves accepted architecture
2. has fewer moving parts
3. reuses existing seams
4. is deterministic
5. is easier to test
6. fails closed
7. produces auditable evidence
8. uses less context/tooling
9. preserves future extensibility
10. reaches completion sooner.