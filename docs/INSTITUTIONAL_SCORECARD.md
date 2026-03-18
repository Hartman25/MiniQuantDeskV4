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
How strong the current checked-in repository state is, given actual proof results.

The **institutional readiness score** is the second one.

This prevents capability optimism from being mistaken for proof-clean readiness.

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
- DB-backed proof completion where required

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
If a DB-backed daemon/runtime lifecycle proof lane is red:
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

These caps stack by taking the **lowest applicable cap**.

---

## Honest Non-Blocker Deductions

These do not create hard caps if handled honestly, but they still reduce weighted section scores:

- unmounted future panels
- explicitly `not_wired` surfaces
- thin-but-honest row models
- missing richer future features
- DB-only tests intentionally gated by env when not failing semantically
- lack of GUI runtime tests if compile/build is still green

---

## Audit Procedure

Every audit should proceed in this order:

### Step 1 — gather proof evidence
Collect:
- `git status`
- `git rev-parse HEAD`
- fmt/clippy output
- workspace test output
- targeted daemon/runtime/broker/md lanes
- DB-backed ignored test lanes where applicable
- proof bootstrap / safety guard output

### Step 2 — identify red-line blockers
Check whether any red-line cap conditions apply.

### Step 3 — score each weighted section
Score each section using the rules above.

### Step 4 — apply cap
If any red-line cap applies, clamp overall score to the lowest applicable cap.

### Step 5 — report both truth and confidence
State:
- what is proven locally
- what is source-only
- what is still red
- what is intentionally deferred but honest

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