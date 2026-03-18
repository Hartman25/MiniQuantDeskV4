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

## Locked End State

MiniQuantDesk V4 counts as **institutional-ready** only when all of the following are true.

### 1. Mechanical cleanliness
All of these are green:

- `cargo fmt --all --check`
- `cargo clippy --workspace --all-targets -- -D warnings`
- GUI typecheck and build lanes green
- no dirty working tree required to pass proof lanes

### 2. Migration governance is clean
All migration-governance lanes are green:

- migration manifest matches migration files
- migration bootstrap / replay proof is green
- no missing SQL files in manifest
- no orphan manifest entries
- no stale schema-proof expectations against current authoritative schema

### 3. Workspace proof posture is clean
The repo’s promoted proof lanes are green, except for explicitly documented DB/integration lanes that remain intentionally gated by environment and are not failing semantically.

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

### 7. Risk and reconcile truth are complete enough
At minimum:

- durable risk-denial history is green
- reconcile mismatch durability/history is green or explicitly unavailable
- no reconcile/risk panel may overstate current-state truth as durable history

### 8. Market-data ingest proof is green
The historical provider ingest path must be proven clean enough that:

- ingest-provider DB-backed proof lanes are green
- chunking / ordering / idempotency / quality-report paths are green
- provider retry/no-data handling does not silently corrupt ingest truth

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

---

## What Does NOT Count as Readiness Regressions

The following do **not** reduce readiness score as red-line failures when they are handled honestly:

- unmounted future operator panels
- explicitly `not_wired` surfaces
- intentionally deferred subsystems not yet exposed as canonical truth
- source-only uncertainty when no local proof was provided
- richer future features not yet implemented

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

## Locked Interpretation Rule

When future audits occur, this file must be used as the stable end-state reference.

Audits must not invent new “definition of done” conditions unless:

- the repo scope explicitly changes, and
- this file is updated intentionally in-repo

Absent such a change, the goalposts are locked here.