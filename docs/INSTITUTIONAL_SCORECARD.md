# MiniQuantDesk V4 — Institutional Scorecard

## Purpose

This document freezes the scoring rubric used for institutional-readiness audits so scores do not thrash across chats because one reviewer scores architecture ambition while another scores proof-clean repo state.

This scorecard is the locked audit rubric.

---

## Two Different Scores

Every audit must distinguish between:

### 1. Architecture / capability score
How strong the system design and intended semantics are.

### 2. Repo-state / proof-clean score
How strong the current repository state is, given actual proof results.

The **institutional readiness score** is the second one.

This prevents capability optimism from being mistaken for proof-clean readiness.

---

## Required Audit Profile Declaration

Every audit must explicitly declare which proof profile was used:

### A. Source-only audit
- code/docs inspected
- no local commands run
- no local proof evidence gathered

### B. Local non-DB proof audit
- local mechanical/proof commands run
- DB-backed mandatory lanes not run

### C. Full DB-backed institutional proof audit
- local mechanical/proof commands run
- DB-backed mandatory lanes run
- authoritative proof bundle completed for the current locked scope

If the audit does not declare its profile, it is incomplete.

---

## Candidate Workspace vs Committed Repo State

Every audit must explicitly state whether the score applies to:

### 1. Candidate workspace state
A local working tree that may include uncommitted changes.

### 2. Committed repo state
A specific checked-in commit identified by `git rev-parse HEAD`.

Rules:

- A dirty working tree may be used to validate a candidate state.
- **Official institutional-readiness scoring applies to the committed repo state unless the audit explicitly says it is scoring a candidate workspace state.**
- A candidate workspace proof result must never be presented as the score of the committed repo unless those exact changes are checked in.

---

## Authoritative Proof Bundle

For the current MiniQuantDesk V4 locked scope, the authoritative proof bundle is:

- `git status`
- `git rev-parse HEAD`
- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- workspace tests
- GUI typecheck/truth-test/build lanes
- targeted daemon/runtime/broker/market-data proof lanes
- safety guards
- the current canonical full proof runner: `full_repo_proof.ps1`

If a reviewer uses a narrower command set, the audit must explicitly say so and must not be represented as a full DB-backed institutional proof audit.

---

## Locked Overall Score Formula

Total score: **100**

### Weighted sections

- Core architecture / system spine — **15**
- Backtest / promotion / provenance — **12**
- Broker / adapter integration — **12**
- Daemon / API truth surfaces — **12**
- GUI / operator truthfulness — **10**
- Execution / portfolio semantics — **8**
- Risk / reconcile / runtime safety — **10**
- DB / migrations / durable truth — **10**
- Market data / ingest / backfill — **6**
- Test / proof posture — **5**

Total = **100**

---

## Section Scoring Rules

### Core architecture / system spine (15)
Score based on:
- crate boundaries
- fail-closed design
- typed truth semantics
- absence of obvious architectural contradictions

### Backtest / promotion / provenance (12)
Score based on:
- deterministic replay discipline
- promotion gate rigor
- provenance truthfulness
- conservative execution semantics

### Broker / adapter integration (12)
Score based on:
- adapter correctness
- inbound lifecycle proof
- identity/cursor truth
- external-broker operator semantics

### Daemon / API truth surfaces (12)
Score based on:
- mounted route truthfulness
- explicit unavailable/not-wired states
- no fake-zero/synthetic semantics
- contract test strength

### GUI / operator truthfulness (10)
Score based on:
- correct consumption of daemon contracts
- no silent fallback from canonical truth to placeholders
- honest panel state rendering
- compile/build/test posture

### Execution / portfolio semantics (8)
Score based on:
- row-level truthfulness
- absence of fabricated fields
- honest unavailable/null representation

### Risk / reconcile / runtime safety (10)
Score based on:
- fail-closed runtime semantics
- durable/current-state truth separation
- runtime lifecycle proof strength
- reconcile/risk honesty

### DB / migrations / durable truth (10)
Score based on:
- migration governance
- durable source correctness
- schema proof posture
- restart-safe truth surfaces

### Market data / ingest / backfill (6)
Score based on:
- provider ingest proof
- ordering/idempotency/quality proof
- retry/no-data truthfulness
- practical reliability of deep-history ingest

### Test / proof posture (5)
Score based on:
- local proof evidence
- promoted proof lanes
- safety guards
- DB-backed proof completion where required by the locked readiness definition

---

## Red-Line Score Caps

These caps prevent inflated scores when load-bearing failures still exist.

### Cap A — migration governance failure
If any migration-governance lane is red, including manifest mismatch:
- **maximum overall score: 79**

### Cap B — stale/broken schema-proof lane
If a DB schema-proof lane is red:
- **maximum overall score: 79**

### Cap C — failing DB-backed daemon lifecycle proof
If a DB-backed daemon/runtime lifecycle proof lane required by the locked readiness definition is red or unrun in a claimed full institutional audit:
- **maximum overall score: 79**

### Cap D — fake operator truth on mounted surfaces
If any mounted operator surface still presents fake/synthetic truth:
- **maximum overall score: 74**

### Cap E — source-only audit without local proof
If the audit is based only on source inspection with no local proof runs:
- **maximum overall score: 80**

### Cap F — proof bootstrap / promoted proof lane failure
If promoted proof bootstrap/guard lanes fail:
- **maximum overall score: 79**

### Cap G — mandatory DB-backed lane omitted in claimed institutional-ready verdict
If the reviewer claims institutional readiness while omitting a DB-backed lane that is mandatory under the readiness lock:
- **maximum overall score: 79**

These caps stack by taking the **lowest applicable cap**.

---

## Honest Non-Blocker Deductions

These do not create hard caps if handled honestly, but they still reduce weighted section scores:

- unmounted future panels
- explicitly `not_wired` surfaces
- thin-but-honest row models
- missing richer future features
- DB-only tests that are environment-gated **and not required by the locked readiness definition**
- lack of GUI runtime tests if compile/build is still green
- non-failing dependency warnings or future-compatibility warnings that do not presently break proof lanes or safety semantics

---

## Red-Line Interpretation Rule

Red-line caps apply to:

- mounted canonical surfaces
- mandatory proof lanes named in the readiness lock
- currently in-scope institutional requirements

Red-line caps do **not** apply to:

- unmounted future systems
- features explicitly declared unavailable/not wired
- hypothetical future operator surfaces
- ambition creep outside the locked scope

---

## Audit Procedure

Every audit should proceed in this order:

### Step 1 — gather proof evidence
Collect:
- proof profile used
- whether the score applies to candidate workspace state or committed repo state
- `git status`
- `git rev-parse HEAD`
- fmt/clippy output
- workspace test output
- targeted daemon/runtime/broker/md lanes
- DB-backed required lanes where applicable
- proof bootstrap / safety guard output

### Step 2 — identify red-line blockers
Check whether any red-line cap conditions apply.

### Step 3 — score each weighted section
Score each section using the rules above.

### Step 4 — apply cap
If any red-line cap applies, clamp overall score to the lowest applicable cap.

### Step 5 — report both truth and confidence
State:
- proof profile used
- commit hash audited
- clean or dirty tree
- whether the score applies to candidate workspace state or committed repo state
- what is proven locally
- what is source-only
- what is still red
- what is intentionally deferred but honest

---

## Required Audit Output Header

Every audit report must include, at minimum:

- audit profile: source-only / local non-DB / full DB-backed institutional
- scored target: candidate workspace state / committed repo state
- commit hash
- working tree status: clean / dirty
- authoritative proof bundle used: yes / no
- mandatory DB-backed lanes run: yes / no, with names

If this header is missing, the audit is incomplete.

---

## Locked Scoring Interpretation

This rubric must not be changed during ordinary audits.

It may be changed only when:

- the project scope changes materially, and
- this file is intentionally edited in-repo

Without that, future audits must use this scorecard as written.

This prevents score thrash from:
- changing reviewer standards
- mixing capability score with proof-clean score
- adding new ad hoc requirements after the fact
- blurring candidate workspace proof with committed repo-state proof
