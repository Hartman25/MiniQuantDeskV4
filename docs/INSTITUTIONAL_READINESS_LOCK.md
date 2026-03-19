# MiniQuantDesk V4 — Institutional Readiness Lock

## Purpose

This document freezes the repository’s **defined end state** for institutional-readiness audits so the target does not drift between chats, reviewers, or audit passes.

This is not a roadmap.
This is not a wish list.
This is the locked definition of what must be true for the repo to count as institutionally ready under the current MiniQuantDesk V4 scope.

---

## Core Principle

A high score is not awarded for:

- patch count
- architecture ambition
- tracker language
- “likely closed” claims
- green non-authoritative test subsets
- mounted routes that still carry fake semantics
- GUI panels that render despite unproven truth

A high score is awarded only when:

- the repository state is mechanically clean
- proof lanes are green
- mounted operator surfaces are truthful
- durable history is either truly durable or explicitly unavailable
- no major audit blocker remains red

---

## Scope of This Lock

This lock governs **current MiniQuantDesk V4 institutional readiness** for the currently mounted and auditable system.

It covers:

- current mechanical cleanliness requirements
- migration governance
- daemon/runtime truth surfaces
- GUI/operator truthfulness for mounted canonical surfaces
- DB-backed runtime/control truth
- risk/reconcile history truth where exposed
- broker/adapter proof lanes in current scope
- market-data ingest/sync proof in current scope
- promoted and mandatory DB-backed proof lanes in current scope

---

## Out of Scope for This Lock

The following are not institutional-readiness blockers unless they are later mounted as canonical truth or added intentionally to this file:

- unmounted future operator panels
- richer research/alpha/discovery tooling
- future strategy breadth
- future cloud/distributed deployment ambitions
- future broker abstractions not currently in canonical scope
- future analytics or observability surfaces not yet mounted as authoritative truth
- quality-of-life feature requests that do not change mounted truth semantics

These may matter to future project scope, but they do not move the readiness goalposts for the current lock.

---

## Candidate Workspace vs Committed Repo State

This lock applies officially to the **committed repository state**.

A dirty working tree may still be used to validate a candidate state, but:

- candidate-workspace proof is not the same thing as committed repo readiness
- a reviewer must explicitly label candidate-workspace proof as such
- institutional-ready status is official only for the checked-in commit unless clearly labeled otherwise

---

## Locked End State

MiniQuantDesk V4 counts as **institutional-ready** only when all of the following are true.

### 1. Mechanical cleanliness
All of these are green:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- GUI typecheck and build lanes green
- no dirty working tree required to pass proof lanes for the committed state being scored

### 2. Migration governance is clean
All migration-governance lanes are green:

- migration manifest matches migration files
- migration bootstrap / replay proof is green
- no missing SQL files in manifest
- no orphan manifest entries
- no stale schema-proof expectations against current authoritative schema

### 3. Workspace proof posture is clean
The repo’s promoted proof lanes are green.

Environment-gated DB/integration lanes are allowed to be omitted **only if they are not mandatory under this lock**.

This includes:

- promoted daemon truth lanes
- promoted broker/adapter proof lanes
- promoted DB proof lanes
- promoted MD ingest/sync proof lanes
- safety guard lanes

### 4. Mounted operator surfaces are truthful
No mounted daemon/GUI surface may:

- return fake-zero semantics
- return synthetic rows as if they were real fleet/runtime truth
- render placeholder-derived truth as canonical
- claim durable history when the source is only in-memory
- fabricate row fields that are not derivable from authoritative sources

Allowed states are:

- active and truthful
- durable and truthful
- active_session_only / ephemeral, clearly labeled
- unavailable / no_snapshot / not_wired, clearly labeled

### 5. Durable history surfaces are honest
Any operator-facing history surface must be one of:

- durably persisted and restart-safe
- explicitly marked as current-session only / ephemeral
- explicitly unavailable / not wired

No history surface may silently degrade from durable truth to process-local memory without the contract saying so.

### 6. DB-backed runtime/control truth is green
DB-backed daemon/runtime lifecycle proof lanes must be green.

This includes:

- hostile restart / poisoned local cache truth
- durable halt truth
- deadman / heartbeat lifecycle truth
- durable running/ownership truth

These are mandatory institutional-readiness lanes, not optional environment extras.

### 7. Risk and reconcile truth are complete enough
At minimum:

- durable risk-denial history is green
- reconcile mismatch durability/history is green or explicitly unavailable
- no reconcile/risk panel may overstate current-state truth as durable history

If a mounted surface claims durable risk/reconcile history, the corresponding DB-backed proof lane is mandatory.

### 8. Market-data ingest proof is green
The historical provider ingest path must be proven clean enough that:

- ingest-provider DB-backed proof lanes are green
- sync-provider DB-backed proof lanes are green
- chunking / ordering / idempotency / quality-report paths are green
- provider retry/no-data handling does not silently corrupt ingest truth

These are mandatory institutional-readiness lanes for the current scope.

### 9. Safety guards are green
All promoted safety guard lanes must be green, including:

- no forbidden nondeterministic patterns in protected scopes
- no forbidden UUID/time/random usage in production paths without explicit allowance
- no forbidden migration defaults / inline SQL now() violations in protected scopes

### 10. No hidden red blockers remain
A repo is not institutional-ready if any major load-bearing red lane remains open, even if most of the rest of the repository is green.

Examples:
- migration manifest failure
- stale schema-proof lane failure
- failing DB-backed daemon lifecycle proof
- mounted operator surface with fake truth
- proof bootstrap failure

### 11. Authoritative proof bundle has been run for a full institutional verdict
A full institutional-ready verdict requires the current authoritative proof bundle for this scope.

For the current lock, that includes the canonical full proof runner and its required DB-backed lanes.

A narrower command subset may still validate a candidate state or partial audit, but it does not qualify as a full institutional-ready verdict unless explicitly equivalent.

---

## What Does NOT Count as Readiness Regressions

The following do **not** reduce readiness score as red-line failures when they are handled honestly:

- unmounted future operator panels
- explicitly `not_wired` surfaces
- intentionally deferred subsystems not yet exposed as canonical truth
- source-only uncertainty when no local proof was provided
- richer future features not yet implemented
- non-failing dependency warnings or future-compatibility warnings that do not presently break proof lanes or create a demonstrated safety issue

These may still reduce weighted scores, but they are not red-line blockers if they are explicitly honest.

---

## Audit Evidence Hierarchy

When scoring, evidence must be ranked in this order:

1. local proof runs and green command outputs
2. failing local proof runs
3. authoritative test code and route logic in the repo
4. docs and trackers
5. patch claims / narrative summaries

Docs and trackers never override code or test truth.

---

## Audit Declaration Requirements

Every institutional-readiness audit must explicitly state:

- whether it is source-only, local non-DB, or full DB-backed institutional proof
- whether it scores a candidate workspace state or committed repo state
- commit hash audited
- clean or dirty working tree
- which mandatory DB-backed lanes were actually run
- whether the canonical full proof bundle was used

If these are not stated, the audit is incomplete.

---

## Locked Interpretation Rule

When future audits occur, this file must be used as the stable end-state reference.

Audits must not invent new “definition of done” conditions unless:

- the repo scope explicitly changes, and
- this file is updated intentionally in-repo

Absent such a change, the goalposts are locked here.
