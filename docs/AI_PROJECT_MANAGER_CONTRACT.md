# MiniQuantDeskV4 — AI Project Manager Contract

Status: **AUTHORITATIVE PROCESS CONTRACT**

Purpose: keep ChatGPT, Claude, Codex, or any other implementation/review agent aligned across chat resets, context loss, long coding sessions, crashes, and handoffs.

This file governs **how work is performed**. Current repo code/tests/Git/DB/runtime evidence remain the ultimate source of truth.

---

## 1. Source of Truth

Priority order:

1. Current repo code, tests, Git state, DB/runtime proof, and real artifacts.
2. Current committed audits, ledgers, runbooks, and this contract.
3. Latest explicit operator instruction.
4. Prior project chats.
5. Memory/summaries only as support.

If an old chat, README, patch label, or agent report conflicts with current repo/runtime truth, **current truth wins**.

Do not reopen accepted/frozen work unless a new deterministic contradiction is found.

---

## 2. Primary Product Goal

The engineering priority is:

**credible US equity/ETF Research → Backtest → Promotion → Paper → controlled Live**

The system must actually be capable of trading through the normal production path.

Before adding broad infrastructure, cosmetic GUI work, new frameworks, multi-asset expansion, or strategy proliferation, ask:

> What is preventing the bot from trading correctly right now?

Once finite readiness gates are met, stop adding infrastructure and move to real alpha discovery and controlled Paper validation.

---

## 3. Efficiency Contract

Default operating model:

**ONE WRITER → ONE BLOCKER → ONE PATCH → ONE TARGETED PROOF → ONE COMMIT → RETURN TO PAPER**

### 3.1 One Writer

Exactly one coding session may modify MiniQuantDeskV4 at a time.

If another session is actively editing the same repo/files:

- the newer session stands down by default;
- read-only review may continue;
- writer authority changes only by explicit operator decision.

Do not run parallel AI coding sessions against the same production files.

### 3.2 Production RED Counts

A deterministic production/runtime/DB/artifact failure is valid **RED evidence**.

Do not manufacture an artificial failing test merely for ceremony.

Add a focused regression test when it materially helps prevent recurrence.

Require an explicit fail-before-fix test only when:
- the production evidence is ambiguous;
- the proposed patch could otherwise be a false positive;
- or the defect cannot be confidently localized from existing evidence.

### 3.3 No Redundant Builds

If a focused `cargo test` compiles the changed crate, do not run a standalone `cargo build` or `cargo check` first unless compilation itself is the uncertainty being investigated.

Do not run commands that prove the same thing twice unless there is a specific unresolved uncertainty.

### 3.4 Minimum Necessary Proof

For a narrow blocker repair, default proof budget is:

1. existing production RED evidence;
2. one focused regression/load-bearing test;
3. one negative control when materially useful;
4. `rustfmt` only for changed Rust files;
5. `git diff --check`.

Anything broader requires justification.

### 3.5 No Broad Suites During Blocker Repair

Do not run by default:

- `cargo test --workspace`
- full Rust workspace Clippy
- `full_repo_proof`
- full GUI suite
- Python research suite
- unrelated crates
- repeated broad repo scans

Run the full relevant subsystem suite only at a real acceptance boundary.

### 3.6 No Drive-By Cleanup

Do not:
- refactor adjacent code;
- rename unrelated symbols;
- fix unrelated warnings;
- reorganize files;
- rewrite working equity code for hypothetical future assets;
- update large documentation areas during a runtime repair.

One defect means one coherent behavior change.

### 3.7 Return to Paper Immediately

After the focused proof passes:

1. commit the patch locally;
2. recover/restart Paper only if required;
3. observe the normal production path;
4. identify the next real blocker, if any.

Do not add extra engineering work before seeing whether the repaired system actually proceeds.

---

## 4. Efficiency Exception Rule

If ChatGPT or an implementation agent wants to exceed the default proof/work budget, it must state **before doing so**:

```text
EXCEPTION REQUEST
Proposed extra command/work:
Unresolved uncertainty it answers:
Why existing evidence cannot answer it:
Expected cost:
Risk if skipped:
```

Without a specific justification, **do not do the extra work**.

"More confidence" by itself is not sufficient justification.

---

## 5. Patch / Commit Discipline

One patch = one coherent invariant/behavior change = one coherent commit.

For code patches:

- inspect only the load-bearing seam first;
- change the smallest correct surface;
- use exact staging;
- commit locally;
- **NO PUSH** until independent review accepts the patch.

Do not mix unrelated code and docs when separate commits are cleaner.

---

## 6. Git / Worktree Discipline

Default branch for active blocker repair: `main`, unless there is an explicit safety reason to isolate.

Default rule: **do not create another worktree**.

Never casually use:

- `git reset`
- `git stash`
- `git clean`
- force push
- forced worktree removal

Protect `smoke_logs/`.

Never add, delete, reset, clean, or otherwise modify `smoke_logs/` unless the operator explicitly authorizes it.

Do not expose or commit:
- `.env.local`
- broker credentials
- API keys
- tokens
- secrets
- secret-bearing artifacts

---

## 7. Testing Rules

Use targeted, load-bearing tests first.

Prefer tests that can disprove the patch, including negative controls where practical.

Do not confuse:

- code/test proof;
- DB-backed proof;
- provider/live-read-only proof;
- operational Paper proof;
- CI proof.

One does not automatically prove the others.

If a DB-backed test is blocked by a known environment problem, report that truthfully. Do not claim the DB-backed proof passed.

---

## 8. Trading Safety

Fail closed.

Never fabricate:

- orders;
- fills;
- positions;
- market activity;
- readiness;
- broker agreement;
- proof.

Never enable Live merely to test something.

Never submit Live orders.

Keep Research, Backtest, Paper, Live, and diagnostic modes clearly separated.

A safety-gate refusal is not automatically a bug. First determine whether it is truthful.

Do not weaken a safety gate merely to make the bot trade.

Fix false or avoidable trigger conditions instead.

---

## 9. Paper Unblock Classification

When Paper is blocked, classify the first load-bearing blocker as:

### A. Deterministic Code Defect

Examples:
- contradictory internal contracts;
- valid lifecycle evidence rejected by another subsystem;
- task dies incorrectly;
- wrong session boundary;
- canonical recovery cannot recover a safe state;
- state machine creates impossible/sticky state.

Action:
- smallest production repair;
- focused proof;
- commit;
- return to Paper.

### B. Stale / Recoverable Operational State

Examples:
- HALTED run after crash with clean reconcile and no unresolved OMS activity;
- supported orphaned-run recovery.

Action:
- use existing canonical operator recovery.
- Do not write code solely because a legitimate operator recovery was needed.

### C. Transient External / Provider Timing

Examples:
- just-closing Alpaca 5m bar not published yet;
- short `expected_latest_bar_missing` interval that self-clears.

Action:
- allow bounded self-heal.
- Do not widen thresholds blindly.

If the transient repeatedly causes pathological state churn, investigate the churn as a potential code defect.

### D. Truthful Safety Refusal

Examples:
- reconcile genuinely dirty;
- unresolved OMS/outbox/inbox activity;
- broker disagreement;
- unknown execution authority.

Action:
- do not bypass the gate.
- resolve the unsafe condition.

---

## 10. Research / Backtest Contracts

Preserve these established contracts unless an explicit mission changes them:

- `fwd_ret` is a classification label, not executable P&L.
- Ordinary execution remains causal; no fabricated same-bar fills.
- Final holdout remains reserved unless explicitly authorized.
- Hypothesis, trial, attempt, and evaluation slice are distinct.
- Retries must not manufacture extra independent trials.
- Winner-only experiment registration is unacceptable.
- Result values must not define trial/data/source identity.
- Semantic input changes should change relevant identity.
- Transport/layout artifacts must not manufacture research candidates.
- Promotion evidence must be genuinely OOS, cost-aware, and execution-aware.

Do not weaken conservative production/backtest behavior to improve Research results.

---

## 11. Long-Session Context Control

Do not allow a long session to repeatedly re-read or re-explain the entire project.

After each accepted blocker repair, reduce state to a compact checkpoint:

```text
CHECKPOINT
HEAD:
STATUS:
BLOCKER FIXED:
CURRENT PAPER STATE:
NEXT BLOCKER:
NEXT ACTION:
FILES/WORKTREES REQUIRING ATTENTION:
```

At a chat/session boundary, preserve durable truth only.

Git history records what happened.

This contract records how we work.

`CURRENT_MISSION.md` records where we are now.

Repo/tests/DB/runtime evidence records what is actually true.

---

## 12. New-Chat Startup Protocol

Before advising or issuing commands on MiniQuantDeskV4:

1. verify current GitHub/local `main` HEAD when available;
2. read this file;
3. read `docs/CURRENT_MISSION.md`;
4. inspect current repo/runtime truth relevant to the mission;
5. do not rely on old chat summaries when they conflict with current evidence;
6. lead with:

```text
VERIFIED HEAD:
CURRENT BLOCKER:
NEXT ACTION:
```

Do not broaden scope.

---

## 13. Acceptance Vocabulary

Use only:

- OPEN
- IN PROGRESS
- PARTIAL
- BLOCKED
- LOCALLY COMPLETE
- INDEPENDENTLY ACCEPTED
- PUSHED
- CI PENDING
- PUSHED-VERIFIED

Do not call work closed while a known deterministic defect remains.

Implementation-agent output is evidence, not acceptance.

---

## 14. Definition of Efficient

For MiniQuantDeskV4:

```text
EFFICIENCY =
minimum necessary discovery
+ minimum necessary change
+ minimum necessary proof
+ immediate return to the real objective
```

If an action does not materially improve correctness, safety, or decision quality, do not spend time or model usage on it.
