# MiniQuantDesk V4 — Claude Operating Rules

## Identity

Institutional-style, deterministic trading and research platform.
Canonical engine: `MAIN`. Non-canonical / experimental: `EXP` (research only; not operational truth).

## Core invariants

- Determinism first. Non-deterministic behavior must be traced to a boundary, not tolerated silently.
- Fail-closed over fail-open. When truth is unavailable, deny or block — never optimistically pass.
- No synthetic broker lifecycle events. Broker is the source of truth for all order state.
- Inbox/outbox is authoritative flow. Do not fabricate, skip, or short-circuit it.
- Idempotency required. Every write path across restart or retry must be idempotent.
- Restart and crash safety required. The system must be safe to stop and resume at any point.

## Order and lifecycle discipline

- The durable chain is: outbox enqueue → broker submit → broker ack/fill → inbox → portfolio.
- Do not bypass any durable transition.
- Do not invent fill, ack, or cancel events not received from the broker.
- OMS state machine transitions must follow the canonical lifecycle — no shortcuts.

## Operator-truth discipline

- Operator surfaces must reflect real authority, not mounted-but-unproven stubs.
- If a truth source is unavailable, the surface must say so explicitly — not return empty as if authoritative.
- Distinguish unavailable, empty, and present. They are not the same.
- No fabricated truth. No optimistic defaults.

## Patch discipline

- One patch per turn. Do not bundle.
- Minimal scope only. Do not widen scope beyond the stated patch objective.
- Return full functions or sections when modifying logic — no partial edits.
- Do not touch files outside the patch's stated scope.
- Do not claim closure beyond the evidence.

## Proof discipline

- Preserve restart safety in every change.
- Preserve crash safety in every change.
- Preserve idempotency in every change.
- Preserve lifecycle correctness — do not weaken OMS or broker-event contracts.
- Scenario tests are the proof standard. Canonical proof matters more than optimistic implementation claims.
