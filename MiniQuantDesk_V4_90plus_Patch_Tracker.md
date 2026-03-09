# MiniQuantDesk V4 — 90+ Institutional Readiness Patch Tracker

## Rules
- One patch at a time. No bonus fixes.
- Return the full def or full section being changed.
- Preserve determinism. No randomness, no hidden IO, no new wall-clock reads unless already approved boundary code.
- Do not weaken tests to get green.
- Patch is not complete until compile + fmt + clippy + tests are green.

## Phase A — Mandatory patches

### A1 — P1-03 Cancel / Replace parity after partial fills **DONE**
**Priority:** Critical

**Likely files**
- `core-rs/crates/mqk-broker-paper/src/lib.rs`
- `core-rs/crates/mqk-execution/src/oms/state_machine.rs`
- `core-rs/crates/mqk-runtime/src/orchestrator.rs`
- targeted tests in broker paper / execution / runtime

**Implementation rules**
- Replace quantity is the new open leaves quantity, not a reset of cumulative fill history.
- Preserve already-filled quantity across replace.
- Reject replace below already-filled quantity.
- Reject replace on cancelled / filled / rejected orders.
- Cancel after partial fill must not erase prior fills.
- Late fills after cancel request must still apply correctly.
- Paper semantics must mirror intended live semantics.

**Acceptance gate**
- No path permits `filled_qty > total_qty`.
- Replace cannot erase prior fills.
- Partial fill + replace + fill preserves exact cumulative quantity.
- Scenario tests cover:
  - new -> partial fill -> replace -> fill
  - new -> partial fill -> cancel -> late fill -> cancel ack
  - new -> partial fill -> replace reject
  - new -> partial fill -> cancel reject

### A2 — Durable broker cursor / resume state **DONE**
**Priority:** Critical

**Likely files**
- `mqk-db` resume/checkpoint tables and functions
- live broker adapter
- runtime/orchestrator broker event ingestion path

**Implementation rules**
- Persist broker event resume checkpoint in DB.
- Resume only from durable checkpoint.
- Advance checkpoint only after inbox persistence succeeds.
- Treat cursor ambiguity as fail-closed.

**Acceptance gate**
- No restart creates an unknown broker event gap.
- Replay after restart is deterministic and idempotent.

### A3 — Broker error taxonomy + retry policy **DONE**
**Priority:** High

**Likely files**
- broker adapter(s)
- gateway/runtime dispatch path
- outbox status handling

**Implementation rules**
- Create typed broker error enum.
- Distinguish transient, ambiguous submit, reject, rate-limit, auth/session, transport.
- Bind outbox behavior to error class.
- Ambiguous submit never silently retries.

**Acceptance gate**
- Every broker failure lands in a named class.
- Hard rejects never retry.
- Retryable errors are bounded and logged.

### A4 — Ambiguous submit quarantine hardening **DONE**
**Priority:** High

**Likely files**
- outbox schema
- outbox DB helpers
- runtime dispatch state machine

**Implementation rules**
- Persist dispatch attempt metadata.
- Persist explicit ambiguous state.
- Require operator or reconcile proof before release.

**Acceptance gate**
- No ambiguous submit re-enters normal dispatch automatically.

### A5 — Live broker adapter completion
**Priority:** High

**Likely files**
- live broker crate
- event normalization layer
- contract tests

**Implementation rules**
- Implement submit/cancel/replace/fetch_events fully.
- Normalize all inbound lifecycle events into canonical `BrokerEvent`.
- Carry authoritative broker order IDs on ack/fill/cancel/replace.

**Acceptance gate**
- Live adapter is not scaffolded.
- Contract tests prove parity with OMS and paper semantics.

## Phase B — Depth patches that push into the 90s

### B1 — Reconcile auto-repair workflow
- Classify drift into auto-repairable, operator-only, halt-required.
- Persist repair audit trail.

### B2 — Structured risk decisions
- Replace bool allow/deny with typed reasoned decisions.
- Add config-driven risk limits and fail-closed tests.

### B3 — Leader lease / single-runtime enforcement
- DB-backed runtime lease with heartbeat and expiry.
- Split-brain tests required.

### B4 — Execution observability
- Structured lifecycle logs.
- Metrics for duplicates, halts, risk refusals, reconcile drift, submit latency.

### B5 — Adversarial scenario suite
- duplicate ack storm
- duplicate fill storm
- out-of-order ack/fill/cancel
- crash after DISPATCHING before broker response
- crash after inbox insert before mark applied
- reconnect replay
- split-brain runtime attempt

## Next
**A5** — Live broker adapter completion.
