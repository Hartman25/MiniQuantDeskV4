# RUNTIME-OPPORTUNITY-ALLOCATION-01 — Phase H: Operator Visibility

Status: implemented and tested.

## Routes (all GET-only)

- `GET /api/v1/portfolio/allocation/status[?run_id=]`
- `GET /api/v1/portfolio/allocation/plans[?run_id=&limit=]`
- `GET /api/v1/portfolio/allocation/plans/:plan_id`

[core-rs/crates/mqk-daemon/src/routes/portfolio_allocation.rs](../../core-rs/crates/mqk-daemon/src/routes/portfolio_allocation.rs)

`repeated_gets_perform_zero_writes` proves 15 repeated GET calls across all
three routes leave the plans-table row count for a seeded run unchanged.

## Closed `truth_state` vocabulary

`active | invalid_configuration | db_unavailable | query_failed | not_found`
— `query_failed` (a DB error occurred) is always distinct from `not_found`
(the query succeeded and genuinely found nothing), mirroring
`routes/durable_portfolio.rs`'s existing convention exactly (same
`RunResolution` enum, same `resolve_run` helper, reused not reimplemented).

`null` always means unavailable/unproven; a real `0` is only ever the
literal number. `status`'s `latest_plan_id`/`latest_plan_created_at_utc`/
`latest_plan_candidate_count`/`latest_plan_allowed_count` are `null` exactly
when no plan exists for the resolved run — never a fabricated zero.

## Malformed-input handling

- Invalid `run_id`/`plan_id` (not a parseable UUID) → `400 Bad Request` with
  a fixed, bounded message that **never echoes the raw caller-supplied
  value** (`invalid_plan_id_is_bounded_and_does_not_echo_input`,
  `invalid_run_id_query_param_is_bounded`).
- Run/plan-format validation happens **before** the DB-availability check
  (matching `durable_portfolio.rs`'s convention) — a malformed
  `run_id` is rejected even when no DB is configured at all.
- Non-GET methods on any of the three routes → `405 Method Not Allowed`
  (`mutation_methods_are_rejected`).

## GUI contract

[core-rs/mqk-gui/src/features/system/runtimeOpportunityAllocation.ts](../../core-rs/mqk-gui/src/features/system/runtimeOpportunityAllocation.ts)
is the single seam deciding whether an HTTP 200 body is trusted as its
claimed `truth_state`, mirroring `durablePortfolio.ts`'s hardening pattern.
Malformed-success fail-closed cases proven by test: unrecognized
`truth_state`; `approved_for_live: true` (hard invariant, never
overridable); `mode_effective`/`runtime_influence` disagreement; NaN/Infinity
numeric fields; `active` claimed with a `null` plan; a non-`active`
`truth_state` smuggling plan/candidate rows; an unrecognized `disposition`
value; a plan row itself claiming `approved_for_live: true`.

`RuntimeOpportunityAllocationPanel.tsx` (mounted on the Settings screen,
gated by the existing `systemStatusScreenIncludes` registry) contains **no**
mode-switch control and **no** mutation control of any kind — it only ever
calls the three GET endpoints above. `runtime_influence` is labeled
distinctly for `none` / `shadow` (explicitly captioned "evidence only — no
trading effect") / `paper_enforced` (captioned "clamping/refusing buys"), and
`approved_for_live` is always rendered as `false`, with the literal string
"(should never happen)" appended defensively should a future regression ever
produce `true` — the panel never implies live readiness under any state.

`npm run build` (tsc + vite) passes clean; 16 new parser tests pass (922/922
total GUI suite). Live in-browser verification of the mounted panel was not
performed this session: the app hard-gates every screen behind a live daemon
HTTP connection, and starting the real daemon in this worktree is explicitly
forbidden by the task's soak-isolation rules. Correctness rests on the
TypeScript build succeeding and the parser's fail-closed test coverage, not
on a rendered screenshot.
