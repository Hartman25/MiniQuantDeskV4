# MiniQuantDesk V4 — Claude Operating Contract

These instructions apply to all work in this repository unless the CURRENT
mission explicitly overrides a specific rule.

MiniQuantDesk is an institutional-style, deterministic trading and research
platform.

Canonical operational engine:

`MAIN`

Non-canonical / experimental work:

`EXP`

`EXP` is research-only and is never operational truth unless a future,
explicitly accepted promotion contract says otherwise.

---

# 1. Priority Order

Always optimize in this order:

1. Correctness
2. Preservation of accepted contracts
3. Fail-closed safety
4. Determinism
5. Reproducibility and auditability
6. Restart/crash/idempotency safety
7. Efficient context and tool use
8. Speed
9. Verbosity

Do not sacrifice correctness to save tokens.

Do not spend tokens, tool calls, test time, or context on actions that do not
materially increase confidence in the CURRENT patch.

The goal is to FINISH MiniQuantDesk credibly, not endlessly redesign it.

---

# 2. Repository Is Durable Project Memory

Treat committed repository content as the primary long-term project memory:

- code
- tests
- schemas
- Git history
- architecture documents
- subsystem contracts
- patch ledgers
- audit documents
- protocol documentation.

Do not expect months of conversation history to be restated.

At mission start:

1. read the current mission;
2. verify the requested Git baseline;
3. read the relevant authoritative subsystem document;
4. inspect only the files necessary for the current patch.

For Research/Backtest work, the current closeout/audit document should be read
when relevant, for example:

`docs/research/Research_Backtest_V1_Closeout_Audit.md`

Other subsystem-specific contracts should be used similarly when they exist.

If repository documentation already establishes an accepted architectural
decision, USE IT.

Do not rediscover the same decision through a repo-wide audit unless:

- the mission explicitly requires re-verification;
- the document may be stale;
- or actual code/tests contradict it.

Source code and tests are authoritative for what the software ACTUALLY does.

Accepted contract documentation is authoritative for intended/frozen design.

---

# 3. Core System Invariants

## Determinism

Determinism first.

Non-deterministic behavior must be traced to an explicit boundary, not silently
tolerated.

Prefer:

- stable sorting;
- explicit ordering;
- canonical serialization;
- fixed seeds;
- deterministic fixtures;
- content-addressed identities.

Any randomness affecting research evidence must have an explicit seed and
protocol identity.

Do not depend on unspecified filesystem, SQL, hash-map, API, or row ordering.

## Fail Closed

Fail closed over fail open.

When authoritative truth is unavailable:

deny
block
mark unavailable
mark unsupported
mark not_evaluable
or raise an explicit error.

Never optimistically pass.

Never fabricate evidence to obtain a COMPLETE verdict.

## Idempotency

Every durable write path that may retry or resume must be idempotent.

## Restart / Crash Safety

The system must remain safe to stop and resume at any point.

Every patch touching durable state must preserve:

- restart safety;
- crash safety;
- idempotency.

---

# 4. Broker / OMS Lifecycle Invariants

The broker is authoritative for broker lifecycle events.

Never synthesize:

- fill events;
- broker acknowledgements;
- broker cancellation confirmations;
- broker order-state transitions.

Canonical durable chain:

outbox enqueue
→ broker submit
→ broker ack/fill/cancel truth
→ inbox
→ portfolio/accounting.

Do not bypass durable transitions.

Do not fabricate, skip, or short-circuit inbox/outbox authority.

OMS state-machine transitions must follow the canonical lifecycle.

No shortcuts.

---

# 5. Operator-Truth Discipline

Operator surfaces must reflect real authority.

Do not present mounted or placeholder data as authoritative.

Distinguish:

UNAVAILABLE

EMPTY

PRESENT.

They are not interchangeable.

If the true source is unavailable:

say unavailable.

Do not return an empty collection and imply authoritative zero state.

No fabricated operator truth.

No optimistic defaults where authority matters.

---

# 6. Accepted Patches Are Frozen

Previously accepted/frozen patches and invariants are CLOSED.

Do not reopen them merely because:

- another implementation appears cleaner;
- you prefer another architecture;
- a modern library offers a different pattern;
- you would have implemented it differently.

Reopen a frozen contract only when NEW deterministic evidence demonstrates:

- a reproducible failing invariant;
- lookahead/leakage;
- incorrect economic chronology;
- unsafe fail-open behavior;
- incorrect statistical methodology;
- incorrect provenance/data semantics;
- contradiction between accepted contract and actual production path.

If the current mission defines this as a hard stop:

STOP.

Report the smallest exact contradiction.

Do not opportunistically redesign the subsystem.

---

# 7. Patch Discipline

A patch represents ONE coherent invariant.

Even when one Claude session contains several patches:

work on ONE PATCH AT A TIME.

Preferred lifecycle:

NARROW AUDIT
→ DEFINE EXACT INVARIANT
→ RED / NEGATIVE CONTROL WHEN APPROPRIATE
→ IMPLEMENT
→ FOCUSED TEST
→ COMPLETE DIFF REVIEW
→ DIRECT REGRESSION TESTS
→ REQUIRED ACCEPTANCE SUITE
→ COMMIT
→ COMPACT CHECKPOINT
→ NEXT PATCH.

Do not mix unrelated fixes into one commit.

Do not perform unrelated cleanup.

Do not reformat unrelated files.

Do not broaden scope merely because another issue was noticed.

If an unrelated defect is discovered:

record it.

Continue only if it does not invalidate the current patch.

If it invalidates current acceptance:

STOP according to mission rules.

---

# 8. Autonomous Multi-Patch Missions

A mission MAY contain multiple patches.

This does NOT authorize bundling.

For every patch:

complete
validate
self-review
commit

before beginning the next patch.

One coherent patch = one commit.

Never accumulate several independent patches into one uncommitted working tree.

After each committed patch, compress its working context to approximately:

PATCH:
COMMIT:
STATUS:
LOAD-BEARING PROOF:
NEW BLOCKER/DEPENDENCY:

Do not repeatedly restate the implementation history of completed patches.

At an explicitly defined wave checkpoint:

STOP for independent review.

Do not continue into the next wave unless authorized.

---

# 9. Minimal Scope

Implement the smallest change that satisfies the entire stated invariant.

Do not widen scope beyond the current patch objective.

Do not modify files outside the patch's legitimate dependency/test/document
scope.

If satisfying the invariant unexpectedly requires broad architecture outside
the declared mission:

STOP and report it rather than silently broadening scope.

---

# 10. Progressive Repository Discovery

Do NOT begin every mission by recursively reading the repository.

Use this escalation order.

## Level 1

Read:

- current mission;
- authoritative subsystem contract;
- baseline Git state.

## Level 2

Inspect exact files/modules named by the mission or contract.

## Level 3

Search exact:

- symbols;
- functions;
- methods;
- types;
- tests;
- callers/callees.

## Level 4

Inspect immediate dependencies and direct regression tests.

## Level 5

Expand to adjacent subsystem files only when evidence requires it.

## Level 6

Broad repository search only when narrower discovery fails.

Prefer:

symbol-aware search
targeted `rg`
specific file sections
specific functions/types

over:

huge directory listings
whole-repository dumps
reading giant files from start to finish.

Once a relevant file is understood, do not reread it unless:

- it changed;
- another section is actually required;
- or new evidence creates a contradiction.

---

# 11. Keep a Small Working Set

Maintain only the current engineering state in active context:

CURRENT HEAD

CURRENT PATCH

FROZEN INVARIANTS

RELEVANT FILES

CURRENT DEFECT

CURRENT LOAD-BEARING TEST

CURRENT BLOCKER.

Do not carry long descriptions of committed patches.

Completed work belongs in:

Git history
docs
audit contracts
compact checkpoints.

---

# 12. Context Is a Finite Engineering Resource

Do not fill context with:

- repeated mission text;
- repeated project history;
- repeated green test logs;
- repeated Git status output;
- unchanged file dumps;
- enormous MCP results;
- huge external-document excerpts;
- verbose command narration;
- several agents returning the same findings;
- speculative architecture essays before inspecting existing code.

Once a fact is established and remains unchanged:

reuse it.

Do not rediscover it.

---

# 13. Minimal Progress Narration

Do not narrate every obvious action.

Avoid:

"Now I will inspect file X."

"Now I will inspect file Y."

"Great, now I understand that."

"Next I will run the tests."

Use concise checkpoint updates when useful.

Examples:

"Scope verified. The defect is an unbound bars provenance manifest;
implementing the content-binding preflight."

"Focused tests pass. Running required regressions."

"Patch P7A committed at <sha>; beginning P7B."

Final evidence belongs primarily in the final report.

Do not expose private chain-of-thought.

Provide conclusions, evidence, and relevant decisions only.

---

# 14. Source Comments Must Stay Lean

Do not turn production source into a patch-history transcript.

Comments/docstrings should explain:

- WHY a non-obvious invariant exists;
- WHAT must remain true;
- WHY a surprising implementation choice is necessary;
- protocol/version identity where genuinely useful.

Avoid large comments containing:

- chat history;
- entire audit findings;
- detailed mission history;
- repeated patch chronology;
- descriptions of what ChatGPT or Claude previously requested.

Put long architectural/history explanation in:

`docs/`

not production source.

Tests should also avoid giant historical comments.

Keep comments useful for the next engineer, not as a transcript of the agent
session.

---

# 15. Reuse Existing Seams

Before creating new architecture, inspect the nearest accepted seam.

Prefer extending existing:

- types;
- registries;
- adapters;
- protocol objects;
- hashing helpers;
- artifact systems;
- error taxonomies;
- fixtures;
- CLI patterns.

Do not create a parallel framework when an accepted one already exists.

Do not introduce an abstraction solely because it is elegant.

A new abstraction should materially improve:

correctness
testability
auditability
or an explicitly required extension seam.

---

# 16. Avoid Unnecessary Code Growth

Prefer the smallest understandable implementation satisfying the invariant.

Do not create:

- duplicate helpers;
- redundant types;
- redundant wrappers;
- separate frameworks for the same concept.

Do not compress code into unreadable cleverness merely to save lines.

Correct and obvious is better than clever.

---

# 17. Test Quality Over Test Quantity

Tests are proof, not a scoreboard.

Before adding a test ask:

"If the old production defect returned, would this test actually fail?"

Prefer:

- parameterized tests;
- table-driven tests;
- adversarial fixtures;
- mutation tests;
- RED/GREEN regression tests

over dozens of nearly identical shallow tests.

Particularly important invariants include:

- lookahead;
- label leakage;
- holdout leakage;
- same-bar execution;
- wrong-symbol execution;
- optimistic execution;
- trial-count inflation;
- provenance bypass;
- corporate-action contamination;
- identity collisions;
- Python/Rust parity;
- fail-open behavior.

Do not inflate test count merely to make a report look impressive.

---

# 18. False-Positive Test Check

For tests proving:

identity
hashing
provenance
security
chronology
trial accounting
statistical behavior

inspect the fixture itself.

Explicitly ask:

"Could this test pass accidentally because both sides contain the same
missing/default/None/empty value?"

Check especially:

- hashes;
- file records;
- IDs;
- provider fields;
- timestamps;
- holdout states;
- trial counts;
- empty datasets;
- nonexistent fixture files.

Whenever practical, tests should exercise the actual production path rather
than a weaker synthetic approximation.

---

# 19. RED / GREEN Discipline

When fixing a deterministic defect and practical:

1. create/fix a regression test reproducing the defect;
2. prove it fails against old behavior;
3. implement the repair;
4. prove it passes.

Use mutation/old-behavior proof where especially valuable.

Do not leave temporary mutation/debug code in the final patch.

Do not perform dangerous repository state changes just to obtain RED.

---

# 20. Test Pyramid

During development:

## First

Run the smallest load-bearing test.

Prefer concise commands such as:

`pytest path/to/test.py::test_name -q --tb=short`

Use `-x` when useful during debugging.

## Second

Run the focused test file.

## Third

Run directly adjacent regression suites.

## Fourth

Run the subsystem/full acceptance suite ONCE at the required patch/wave
boundary.

Do not run the entire research suite after every minor edit.

Do not run the entire Rust workspace after Python-only changes.

Do not rerun an unchanged full green suite unless subsequent modifications
could affect it.

Report successful test output compactly:

command
pass count
relevant skips
unexpected warning/error.

Do not dump hundreds of lines of successful output.

---

# 21. Scenario Tests Are the Proof Standard

For operational/lifecycle behavior, scenario-level proof is preferred over
optimistic implementation claims.

Canonical proof matters more than "the code looks right."

Preserve scenario tests that prove:

- durable lifecycle;
- restart safety;
- crash safety;
- fail-closed behavior;
- execution chronology;
- reconciliation authority.

---

# 22. Subagent Discipline

Do not launch subagents merely because parallelism is available.

Subagents consume context and frequently duplicate discovery.

Good uses:

- large read-only audits with genuinely independent domains;
- one primary-paper verification while the main agent inspects implementation;
- independent adversarial review of a high-risk completed patch;
- several large truly independent modules.

Bad uses:

- five agents for a three-file patch;
- several agents reading the same files;
- repeated repository discovery;
- researching facts already established;
- rerunning the same tests.

For focused implementation:

prefer ONE primary agent maintaining the complete patch mental model.

Use a secondary reviewer only when risk justifies it.

---

# 23. MCP / Tool Evidence Hierarchy

Available tools may include:

- Srclight;
- Context7;
- Firecrawl;
- Playwright;
- other MCPs.

Tool availability does not mean tool usage is required.

Evidence priority:

1. current repository code/tests/schema;
2. local read-only data;
3. version-matched official documentation;
4. original research papers / primary provider docs;
5. secondary sources only when primary material is unavailable.

Use the least expensive authoritative route.

Do not query several tools for the same fact unless:

- the first source is insufficient;
- or contradiction checking is genuinely necessary.

---

# 24. Srclight / Indexed Code Search

Use symbol-aware/indexed search when useful for locating:

- definitions;
- references;
- callers;
- implementations.

Prefer indexed symbol search over dumping huge sections of the repository.

Once the implementation seam is established:

stop searching and inspect the relevant code.

---

# 25. Context7

Use Context7 only when implementation depends on uncertain/current or
version-specific third-party behavior.

Examples:

- Python library APIs;
- Rust crate APIs;
- framework configuration;
- CLI framework syntax;
- serialization behavior.

First inspect the actual dependency version where practical.

Do not use Context7 for:

- ordinary Python/Rust syntax;
- MiniQuantDesk behavior already established by code/tests;
- accepted MiniQuantDesk protocol semantics.

Once the required fact is established:

reuse it.

Do not repeatedly query it.

---

# 26. Firecrawl / External Research

Use Firecrawl only when current external factual information materially
affects correctness.

Prefer PRIMARY SOURCES:

- official provider documentation;
- original papers;
- official specifications.

Do not broadly crawl the web for repository questions.

Do not use blogs, Reddit, SEO articles, or summaries for load-bearing
statistical/provider contracts when primary material exists.

For provider work, verify only relevant facts such as:

- adjustment semantics;
- timestamps;
- pagination;
- corporate actions;
- revision/backfill behavior;
- completeness/finality;
- API field definitions.

Retrieve the minimum useful section.

Do not inject entire external documents into context.

When a provider contract is verified and versioned, preserve the durable fact
in repository documentation so future missions do not need to research it
again.

---

# 27. Playwright

Playwright is NOT the default test mechanism.

For backend work prefer:

- unit tests;
- integration tests;
- API tests;
- CLI tests.

Use Playwright only for real browser-facing behavior such as:

- GUI flows;
- artifact rendering;
- research job controls;
- promotion UI;
- browser-visible error states.

Do not use Playwright for backend-only patches.

Never use autonomous browser activity to:

- submit broker orders;
- enable Live;
- manipulate unrelated authenticated personal services.

Prefer local/test environments.

---

# 28. External Documentation Does Not Override Local Reality

External documentation answers:

"What does the external provider/library/method specify?"

Repository and actual stored data answer:

"What does MiniQuantDesk actually do?"

If they disagree:

REPORT THE CONTRADICTION.

Do not silently reinterpret one to fit the other.

For provider/data work triangulate only as needed:

official provider documentation
vs.
client/ingestion code
vs.
schema
vs.
actual read-only data.

---

# 29. Statistical Methodology

For load-bearing statistical methodology:

prefer original papers.

Do not use blogs for formulas when the original paper is available.

Once a methodology is independently verified and encoded into a frozen
protocol:

do not repeatedly re-fetch or re-derive the paper during unrelated work.

Reverify only when:

- methodology changes;
- a statistical defect is suspected;
- protocol version changes.

Never substitute:

attempt count
retry count
evaluation slice count
window count

for unique candidate trial count unless the accepted method specifically
requires it.

When prerequisites are insufficient:

return `not_evaluable` or equivalent.

Do not fabricate a statistic.

---

# 30. Research / Label / P&L Boundary

Research labels are not automatically executable P&L.

Never treat forward-return labels such as `fwd_ret` as actual strategy
economic returns unless an accepted protocol explicitly defines that behavior.

Economic results must follow the accepted causal execution contract.

Preserve:

signal timestamp
→ later executable target-symbol bar
→ execution
→ subsequent earned return.

Do not reintroduce same-bar lookahead.

---

# 31. Holdout Discipline

Final holdout data is reserved unless an explicit mission authorizes
consumption.

Do not inspect/evaluate the final holdout casually.

Never use it to:

- tune;
- debug;
- select parameters;
- compare candidates;
- repair models.

Respect the durable holdout-consumption ledger.

Once an exact holdout is consumed:

it is no longer fresh.

Do not pretend otherwise.

---

# 32. Experiment Accounting

Keep these concepts distinct:

HYPOTHESIS
= idea

TRIAL
= unique candidate specification

ATTEMPT
= invocation/retry

SLICE/JOB
= evaluation window/job.

Retries do not create new trials.

Evaluation windows do not create new trials.

Failed attempts remain durable evidence.

Do not register only winners.

Do not erase failed history.

Do not make result values determine candidate identity.

---

# 33. Data / Provenance Discipline

Never infer a strong data claim merely from:

- filename;
- column name;
- guessed provider;
- caller-supplied metadata string.

Where data identity matters, prefer:

canonical semantic identity
+
versioned provenance
+
physical artifact evidence.

Semantic dataset identity and physical artifact identity are different
concepts when appropriate.

Fail closed when:

- provider is unknown;
- adjustment semantics are unverified;
- manifest does not bind to actual content;
- point-in-time claims are unsupported;
- authoritative evidence is unavailable.

Never fabricate:

- corporate-action evidence;
- provider identity;
- historical constituent membership;
- adjustment behavior.

---

# 34. Authority Is Not the Same as Integrity

A hash proves integrity.

A hash does NOT prove authority.

A caller-created content-addressed object is not automatically an authoritative
external source.

For load-bearing external evidence, distinguish:

CONTENT INTEGRITY

from:

SOURCE AUTHORITY.

This applies especially to:

- corporate actions;
- provider provenance;
- historical universe membership;
- market-data authority;
- external economic/reference data.

Synthetic evidence is appropriate for tests/diagnostics.

Synthetic evidence must not silently authorize official research/promotion.

---

# 35. Multi-Asset Discipline

Current work may focus on US equities/ETFs while preserving clean future
extension seams for:

- Futures;
- Options;
- FX;
- Crypto.

Do not prematurely implement those asset classes.

Generalize now only when:

- current architecture is incorrectly coupled;
- current patch requires an asset-neutral boundary;
- or delaying the seam would create known substantial rework.

Otherwise:

document the future adapter boundary
and DEFER implementation.

The desired future architecture is:

shared research/statistical core
+
asset-specific data/execution/accounting adapters.

Do not duplicate the entire research framework per asset class.

---

# 36. Production / Paper / Live Safety

Do not touch runtime/Paper/Live systems unless the CURRENT mission explicitly
requires it.

Never fabricate trading activity.

Never enable Live trading.

Never submit real/live broker orders as routine validation.

Paper/broker interactions must be explicitly authorized by the mission.

Research/backtest work should remain isolated from runtime execution unless a
specific parity/promotion mission crosses that boundary.

---

# 37. Secrets

Never expose:

- API keys;
- tokens;
- cookies;
- broker credentials;
- database passwords;
- `.env`;
- `.env.local`;
- private auth headers.

Never place secrets in:

- source;
- tests;
- logs;
- artifacts;
- Git commits;
- MCP queries;
- Firecrawl;
- Context7;
- browser pages.

Use established environment/configuration conventions.

---

# 38. Git Baseline Discipline

At mission start when specified, verify:

- branch;
- HEAD;
- origin/main;
- working-tree residue.

If expected baseline differs:

STOP.

Do not begin implementation.

After baseline:

do not spam `git status`.

Use it when:

- establishing baseline;
- before staging;
- after commit;
- at final report;
- repository state becomes suspicious.

Never use:

`git reset`

`git stash`

`git clean`

unless explicitly authorized by the current mission.

---

# 39. Protect smoke_logs

`smoke_logs/` is protected operational residue.

Do not:

- add it;
- stage it;
- commit it;
- delete it;
- reset it;
- clean it;
- rewrite it

unless an explicit mission requires a specific operation there.

Ordinary expected status may include:

`?? smoke_logs/`

Do not treat that by itself as a defect.

---

# 40. Commit Discipline

One patch = one commit.

Stage exact intended files.

Avoid broad:

`git add .`

when targeted staging is practical.

Do not squash autonomous patch commits together.

Do not push unless the CURRENT mission explicitly authorizes pushing.

If the mission says:

DO NOT PUSH

then do not push.

---

# 41. Diff Discipline

Before commit, inspect the COMPLETE diff for the current patch.

Ask:

- only intended files?
- unrelated refactor?
- fail-closed behavior weakened?
- chronology changed accidentally?
- protocol identity altered unexpectedly?
- result-dependent identity introduced?
- temporary/debug code left?
- test genuinely load-bearing?
- huge unnecessary comments added?
- duplicate framework/helper created?

Use:

`git diff --check`

at appropriate acceptance boundaries.

Do not repeatedly dump giant identical diffs into context.

---

# 42. Keep Command Output Small

Prefer constrained command output.

For searches:

use exact patterns and restricted paths.

For pytest:

prefer `-q` and `--tb=short` where appropriate.

For Git:

prefer targeted:

diff
stat
log
status.

Do not dump thousands of lines into context if:

- a short error trace;
- a summary;
- or one relevant section

is sufficient.

---

# 43. Do Not Repeat Failed Approaches

When an approach fails:

record the reason once.

Change the hypothesis or method.

Do not repeatedly issue effectively identical searches/commands expecting a
different result.

If two attempts fail for the same underlying reason:

stop and reassess before spending more context.

---

# 44. Error Handling

Errors should communicate:

WHAT invariant failed

and enough identifying context to diagnose it.

Do not expose secrets.

Do not catch integrity failures simply to continue with:

defaults
empty results
fabricated success.

Do not silently downgrade failures.

---

# 45. Performance Work

Do not prematurely optimize.

Measure/profile first when performance is the mission.

During ordinary correctness patches:

avoid obviously wasteful behavior, but do not restructure correct systems for
speculative performance improvements.

Research scalability work is required only when actual intended V1 workloads
are blocked.

---

# 46. Completion Gates Are Finite

Subsystems have finite completion gates.

Once the documented completion gate is satisfied:

STOP ADDING INFRASTRUCTURE.

Do not invent requirements merely because:

- an institutional platform could have them;
- another academic paper mentions them;
- a future asset class might use them;
- more abstraction would be elegant.

Classify useful but non-required work as:

DEFERRED.

The objective is a credible finished system.

---

# 47. Tool-Call Decision Rule

Before any:

- broad repository search;
- large file read;
- MCP call;
- subagent launch;
- web crawl;
- full test suite;
- Playwright session

silently ask:

"Will this materially increase confidence in the correctness of the CURRENT
patch?"

If NO:

do not do it.

---

# 48. Feature Decision Rule

Before adding functionality outside the current mission ask:

"Is this required to satisfy the current documented subsystem completion
gate?"

If NO:

DEFER IT.

Do not broaden scope merely because something sounds useful.

---

# 49. Design Decision Rule

When several correct approaches exist, prefer the approach that:

1. preserves accepted architecture;
2. has fewer moving parts;
3. reuses existing seams;
4. is deterministic;
5. is easier to test;
6. fails closed;
7. creates durable auditable evidence;
8. minimizes new abstractions;
9. minimizes context/tool use;
10. preserves future extensibility;
11. reaches the finite completion gate sooner.

---

# 50. Patch Success Rule

A patch is NOT complete merely because tests are green.

Completion requires:

- intended invariant exists in the real production path;
- negative control is load-bearing where appropriate;
- no known deterministic contradiction remains inside patch scope;
- fixtures do not create false-positive proof;
- identity/provenance boundaries are genuine;
- no accepted contract was silently weakened;
- required regressions pass.

Tests are evidence.

Green tests alone are not proof.

---

# 51. Manual Patch Output for the User

Claude Code normally edits repository files directly.

Do NOT dump complete modified files/functions into the final report merely
because they changed.

However, if the USER must manually apply a patch:

provide the WHOLE affected:

- function;
- method;
- impl block;
- type;
- or coherent section.

Never give fragmented manual patch snippets.

When manually guiding the user:

one patch at a time unless explicitly requested otherwise.

---

# 52. Final Reports Are Evidence, Not Transcripts

Do not reproduce the mission prompt.

Do not narrate every tool call.

Do not paste entire successful logs.

Unless the mission specifies another format, final reports should contain only
the evidence needed for independent review:

VERDICT

STARTING HEAD

PATCH / INVARIANT

FILES CHANGED

NEGATIVE CONTROL / RED-GREEN PROOF

FOCUSED TEST RESULT

ACCEPTANCE SUITE RESULT

COMMIT SHA

DIFF CHECK

FINAL GIT STATUS

BLOCKERS

NEXT RECOMMENDED PATCH.

Keep reports concise but sufficient for independent verification.

---

# 53. Durable External Facts Belong in Documentation

When a mission verifies a durable external fact such as:

- provider adjustment semantics;
- provider corporate-action behavior;
- pagination contract;
- statistical methodology;
- protocol definition;

record the minimum necessary authoritative fact/source in the appropriate
repository documentation.

Do not copy long external research summaries into production source comments.

Future missions should consume the durable contract rather than research the
same fact again.

---

# 54. Autonomous Hard Stops

Unless explicitly overridden by the mission, STOP rather than broaden scope
when:

- accepted P&L chronology must unexpectedly change;
- holdout semantics must change;
- experiment/trial accounting must unexpectedly change;
- production Paper/Live/runtime scope becomes necessary unexpectedly;
- provider documentation contradicts stored data;
- required external authority/data source does not exist;
- a negative control does not detect the previous defect;
- tests fundamentally contradict the specification;
- unexplained tracked modifications appear;
- secrets would need to be exposed;
- broad architecture redesign becomes necessary;
- a frozen safety invariant is deterministically contradicted.

Report the smallest exact blocker.

---

# 55. Claude Usage / Context Efficiency Rule

Optimize the mission so most context and tokens are spent on:

- understanding the ACTIVE defect;
- implementing the ACTIVE invariant;
- proving the ACTIVE invariant.

Minimize usage spent on:

- narration;
- redundant discovery;
- repeated historical explanation;
- redundant agents;
- giant source comments;
- giant command output;
- repeated documentation retrieval;
- repeated full-suite runs;
- premature future design.

Do not cut verification that materially improves correctness.

Cut everything else first.