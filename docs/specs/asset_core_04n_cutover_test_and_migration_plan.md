# ASSET-CORE-04N — Cutover Test Plan and Migration Plan

**Patch:** `ASSET-CORE-04N-PRODUCTION-CUTOVER-TEST-PLAN-AND-MIGRATION-PLAN-01`
(Phase C of `ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01-COMBINED`)

**Builds on:** `docs/specs/asset_core_04l_production_cutover_callsite_audit.md`
(Phase A) and `docs/specs/asset_core_04m_production_cutover_design.md`
(Phase B). This document is a plan only — it adds no migration, runs no
test, and authorizes no cutover.

---

## 1. DB migration decision

- **Exact tables likely affected:** `oms_outbox`, `oms_inbox`, the
  fill-quality telemetry table (`0028_fill_quality_telemetry.sql`), the OMS
  order-lifecycle events table (`0035_oms_order_lifecycle_events.sql`), and
  any future position/ledger-snapshot table derived from `PortfolioState`.
  Per `db_rules.md`'s append-only rule, **none of `0001`-`0044` may be
  modified**; every one of these needs a new migration, not an edit.
- **New columns vs. new tables:** prefer new **columns** on existing
  quantity-bearing tables (e.g. a nullable
  `qty_micros_fraction bigint` or equivalent alongside the existing
  `bigint` whole-unit column) for tables that are conceptually
  per-order/per-fill, since the row identity doesn't change. Prefer a new
  **table** for anything that is a new concept entirely — e.g. an
  `instrument_economics_snapshot` table capturing multiplier/currency/margin
  metadata per position at valuation time, since this has no natural home
  in the existing whole-unit tables and mixing concerns there would
  violate `db_rules.md`'s schema-versioning discipline.
- **How to preserve existing equity rows:** any new column must be
  nullable with no default that implies a value the row doesn't actually
  have (`db_rules.md`: "no `DEFAULT now()` or `DEFAULT gen_random_uuid()`
  in schema — callers must supply time and id" — the same discipline
  applies to a fractional-quantity default: it must not silently default
  to `0` or `1` for pre-existing equity rows). Existing equity rows should
  simply have the new column `NULL`, meaning "whole-unit, no fraction" —
  an explicit absence, not a fabricated value.
- **How to backfill:** no backfill needed for the whole-unit legacy rows,
  since `NULL` in the new column already means "this row is whole-unit,
  read the existing `bigint` column as authoritative" — no ambiguity, no
  computed backfill value that could be wrong.
- **Rollback plan:** the new column/table can be ignored by rolling the
  config guard (`asset_core_04m_production_cutover_design.md` §10) back to
  `false`; the old code path never reads the new column, so no data
  migration is needed to roll back, only a config flip. Per `db_rules.md`,
  no migration is ever rolled back destructively — the new column/table
  simply goes unread again.

## 2. Quantity-precision migration

- **Whole-share equity compatibility:** the existing `bigint` quantity
  columns remain the source of truth for equity; a cutover must not
  require rewriting any existing equity row.
- **Fractional crypto compatibility:** a new fixed-point column (using the
  existing `mqk_schemas::QtyMicros` scale, per `asset_core_04m_production_cutover_design.md`
  §2, so there is exactly one fixed-point convention in the workspace, not
  two) is required before any crypto position could be recorded with
  sub-whole-unit precision.
- **Contract quantity compatibility:** futures/options contract quantity
  is whole-unit-like (you hold an integer number of contracts) but needs a
  separate `multiplier` value (from `InstrumentEconomics`, not a new DB
  column duplicating it) to convert contract count to notional exposure —
  no new quantity column is needed for contracts themselves, only the
  already-planned multiplier/currency metadata (§1's new table).

## 3. Margin/multiplier/currency fields

- **Where stored:** multiplier and currency are **instrument-level**
  metadata (belongs with `InstrumentEconomics`/registry-v2, not duplicated
  per-row in `oms_outbox`/`oms_inbox`). Margin, if ever computed, is
  **position-level** and time-varying, so it belongs in the new
  `instrument_economics_snapshot`-style table (§1), not as a static
  instrument property.
- **Source of truth:** `InstrumentEconomics` (via the `ASSET-CORE-04B`
  registry-v2 bridge) for multiplier/currency; margin has **no existing
  source of truth in the workspace** (confirmed by Phase A's audit §10) —
  a margin model must be designed and decided as its own future patch
  before any margin column/table is added, not invented ad hoc in this
  migration.
- **Missing-data behavior:** must fail closed — a position whose
  instrument has no resolvable `InstrumentEconomics` row must not be
  valued with an assumed multiplier/currency; the existing
  `InstrumentEconomicsTruthState` refusal states already model this
  correctly and must be preserved, not bypassed, by whatever code reads
  the new columns/table.

## 4. Tests required before any production cutover

- **Live equity accounting parity:** extend
  `scenario_asset_core_04_live_ledger_invariants.rs`'s existing
  `EQ-01`..`EQ-06` approach to prove the new economics-aware valuation
  layer (§ design doc §1) agrees exactly with unmodified
  `accounting.rs`/`metrics.rs` output for every existing equity fixture —
  zero drift tolerance.
- **Equity regression suite:** the full existing suite
  (`scenario_pnl_partial_fills_fifo.rs`, `scenario_conservation_invariants.rs`,
  `scenario_position_flatten_fifo.rs`, `scenario_fill_ordering_determinism.rs`,
  `scenario_rounding_boundaries_m4_1.rs`, `scenario_short_position_lifecycle_01.rs`)
  must pass unmodified — any change to these tests to make a cutover pass
  is itself a signal the cutover broke equity behavior and must not ship.
- **Fractional quantity disabled until explicitly enabled:** a test
  proving that submitting a fractional-quantity order is rejected
  (`assert_equity_whole_units`-style validation, or its non-equity
  equivalent from the design doc §9) whenever the new config guard (§10 of
  the design doc) is `false` — the default.
- **Multiplier disabled until explicitly enabled:** same pattern — a test
  proving multiplier application is a no-op (effectively `1`) whenever the
  config guard is off, and only applies a non-1 value when explicitly
  enabled for a specific asset class.
- **Missing economics fail-closed:** a test constructing a position whose
  instrument has no `InstrumentEconomics` row and asserting the valuation
  layer refuses rather than defaulting.
- **Stale/missing mark fail-closed:** a test proving the new valuation
  layer inherits `compute_portfolio_weights`'s existing
  `"missing_marks"`/`"nav_unavailable"` fail-closed behavior rather than
  reimplementing (and potentially weakening) it.
- **Margin unavailable fail-closed:** a test proving any asset class with
  `requires_margin_model: true` and no live margin computation is refused
  at the risk layer, not silently allowed with a zero margin requirement.
- **No broker/order side effects in shadow mode:** a test proving that
  running the new valuation/risk computation path in shadow mode makes
  zero calls into `mqk-execution::gateway` or any broker adapter — the
  shadow computation must be provably read-only.

## 5. Scenario tests required before paper cutover

- **Read-only shadow-mode comparison:** a scenario test running both the
  old and new valuation paths side by side against a realistic multi-fill,
  multi-symbol fixture and asserting equity-shaped positions agree exactly
  (extending, not replacing, the existing invariants test).
- **Paper-only equity unchanged:** a scenario test proving that with the
  cutover config guard enabled for paper mode, existing equity paper-order
  behavior (from `PAPER-SMOKE-*` and `AUTON-NO-TRADE-*` lineages) is
  bit-for-bit unchanged.
- **Paper-only non-equity still disabled:** a scenario test proving that
  even with the cutover guard enabled, `ASSET_RISK_NON_EQUITY_ROUTING_ENABLED`
  remaining `false` still blocks any non-equity order from reaching the
  gateway — the cutover guard and the non-equity routing guard are
  independent and both must be true before non-equity paper trading could
  ever be attempted.
- **Risk/readiness/status visibility:** a scenario test proving
  `GET /api/v1/system/asset-risk-policy/status` and
  `GET /api/v1/portfolio/economics/status` both continue to report
  accurate, non-fabricated `truth_state`/`model_only`/`*_uses_*` fields
  throughout every stage of rollout (shadow → paper-only) — these routes'
  honesty fields must never say `true` before the corresponding code path
  actually reads live data.

## 6. Required operator evidence

- **Daemon status:** `GET /api/v1/system/status` (or equivalent) showing
  the new config guard's current value explicitly, not implied.
- **Risk status:** `GET /api/v1/system/asset-risk-policy/status` showing
  per-asset-class readiness, unchanged in shape from today.
- **Portfolio status:** `GET /api/v1/portfolio/economics/status` showing
  `model_only`/`trading_uses_portfolio_economics`/etc. flipped to their
  true values only once the corresponding code path is actually wired —
  never flipped preemptively.
- **DB rows:** direct read of the new column/table showing `NULL`/absent
  for all pre-existing equity rows, confirming no backfill corrupted
  history.
- **No live routing:** explicit confirmation (log line, status field, or
  test assertion) that `live_trading_enabled` remains `false` for every
  asset class throughout the entire rollout sequence up to and including
  paper-only cutover.

## 7. Required rollback

- **Config off:** flipping the new cutover guard back to `false` must be
  the entire rollback action — no data migration required to roll back, by
  construction (§1).
- **Route status shows disabled:** `GET /api/v1/portfolio/economics/status`
  and `GET /api/v1/system/asset-risk-policy/status` must both reflect the
  rolled-back state accurately and immediately (no caching that would show
  stale "enabled" truth).
- **Old equity accounting path still works:** the pre-cutover
  `accounting.rs`/`metrics.rs`/`valuation.rs` path must never be deleted or
  bypassed by the cutover — it remains the ledger-of-record permanently,
  per the design doc §1's "additive, not replacing" principle.
- **No DB rollback that destroys data:** rollback is a config change only;
  no migration is ever reverted in a way that drops the new column/table
  or any data written to it — per `db_rules.md`'s migration idempotency
  rule, forward-only migrations are the standard, and "rollback" here means
  "stop reading the new data," not "delete the new schema."

## 8. Explicit Go/No-Go checklist

Before any future patch may flip the cutover config guard to `true` for
even shadow mode, all of the following must be true and evidenced:

- [ ] §4's full pure/DB/scenario test list exists and passes in CI.
- [ ] §1's migration has landed as its own reviewed, additive-only patch,
      with the round-trip proof from §4.
- [ ] §5's paper-only scenario tests pass, including the non-equity-still-
      disabled proof.
- [ ] §6's operator evidence has been captured and reviewed by a human
      operator, not just asserted by an automated report.
- [ ] An explicit operator authorization phrase (see
      `asset_core_04o_cutover_go_nogo_decision.md` §10) has been given for
      the *specific* slice being cutover (shadow mode, then paper-only, then
      any later live consideration are each separately authorized — one
      authorization does not cover all three).
- [ ] No hard safety rule from this bundle or any `.claude/rules/*.md` file
      is violated by the specific patch being proposed.

---

## Safety confirmation

No live routing was enabled. No broker adapter change was made or
proposed as ready. No DB migration was added by this document — it plans
one for a future, separately-authorized patch. No live or paper order was
submitted. No config flag was changed.
