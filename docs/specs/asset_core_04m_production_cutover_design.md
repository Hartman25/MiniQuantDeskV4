# ASSET-CORE-04M — Production Cutover Design Spec

**Patch:** `ASSET-CORE-04M-PRODUCTION-CUTOVER-DESIGN-SPEC-01`
(Phase B of `ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01-COMBINED`)

**Builds on:** `docs/specs/asset_core_04l_production_cutover_callsite_audit.md`
(Phase A) for all current-state facts cited below. This document is
design/decision-only — it specifies target architecture and a proposed
future patch sequence; it changes no production behavior, adds no DB
migration, and authorizes no cutover by itself (see Phase D's go/no-go
decision for the explicit non-authorization).

---

## 1. Target architecture for live accounting after cutover

Today, live accounting is a single, whole-unit, single-currency,
multiplier-implicit-1, no-margin path:
`mqk-portfolio::{accounting.rs, metrics.rs, valuation.rs}` operating on
`PortfolioState`/`PositionState`/`Fill`/`Lot` (`i64` quantities), with
`mqk-runtime::observability` and `mqk-daemon`'s portfolio routes as
read-only derived views (`asset_core_04l_production_cutover_callsite_audit.md`
§3-§4).

Target architecture keeps this path **structurally intact for equity**, and
introduces a **parallel, explicitly-typed economics-aware valuation layer**
that:

- Continues to use `Fill`/`Lot`/`PortfolioState` as the ledger-of-record for
  cash and whole-unit-equivalent quantity (this is the append-only,
  crash-safe, restart-safe ledger `CLAUDE.md`'s invariants protect — it
  should not be replaced, only extended).
- Adds a new, additive `InstrumentEconomicsContext` resolution step between
  "raw fill/position" and "valued position" — reusing the existing
  `ASSET-CORE-04A` `InstrumentEconomics`/`value_position_economics`
  (`core-rs/crates/mqk-portfolio/src/instrument_economics.rs`) as the
  starting model, since it already correctly special-cases equity
  (multiplier=1, same-currency) and already fails closed on unsupported
  currency/margin cases.
- Keeps `mqk-execution::gateway`'s broker-submit path reading only the
  legacy whole-unit order type until risk enforcement (§8) and accounting
  (this section) are both cutover-ready — order routing is the last layer
  to change, not the first (per the callsite audit §6).

No existing production symbol's signature or behavior changes in this
design; the target is additive until an explicit, later, behavior-changing
patch flips a specific call site over.

## 2. Required quantity representation

- **Equity whole-share path:** `i64` remains. Every existing scenario test
  (`scenario_pnl_partial_fills_fifo.rs`, `scenario_conservation_invariants.rs`,
  etc., named in the Phase A boundary audit) already proves this path;
  changing its type would be a needless, high-blast-radius risk for zero
  behavioral gain, since equity is whole-share by definition.
- **New fixed-point quantity type needed:** yes, for any fractional
  (crypto) or contract-multiplier (futures/options) position. The
  workspace already has a candidate: `mqk_schemas::QtyMicros`, used today
  only by `OrderIntentV2.qty` (`core-rs/crates/mqk-execution/src/types.rs`
  line 127) and never constructed or read by the gateway or by
  `mqk-portfolio`. The target design reuses `QtyMicros` rather than
  inventing a second fixed-point type — introducing a second type would
  create exactly the two-sources-of-truth risk `ASSET-CORE-04`'s own prior
  closure decisions (Phase C/D skip rationale) were written to avoid.
- **DB schema change needed:** yes — see §3 of the sibling test/migration
  plan doc (`asset_core_04n_cutover_test_and_migration_plan.md`). No
  existing `bigint` column should be altered in place (violates
  `db_rules.md`'s append-only migration rule); a new column or new table
  is required.

## 3. Required economics source of truth

- **`InstrumentEconomics`** (`ASSET-CORE-04A`) remains the model-level
  source of truth for multiplier/currency/quantity-scale metadata per
  instrument.
- **Registry-v2 economics** (`ASSET-CORE-04B`'s
  `instrument_economics_bridge.rs`) remains the bridge from
  `mqk-md::instrument_registry_v2` definitions to `InstrumentEconomics` —
  this bridge is already proven model-only/zero-enablement and should stay
  the single translation point rather than duplicating bridge logic
  elsewhere.
- **Fallback behavior when missing:** must fail closed, per `CLAUDE.md`'s
  "fail-closed over fail-open" invariant and matching the existing
  `InstrumentEconomicsTruthState` enum's already-defined refusal states
  (e.g. `CurrencyConversionUnsupported`). A cutover must never silently
  default a missing multiplier to `1` or a missing currency to `"USD"` for
  a non-equity instrument — only equity/ETF, where `1`/`"USD"` are already
  true by construction, may use those as defaults.

## 4. Contract multiplier design

- **Where loaded:** from `InstrumentEconomics` (resolved via the
  registry-v2 bridge, §3), not hardcoded per asset class in the accounting
  layer.
- **Where applied:** at the valuation boundary only (a new
  economics-aware `value_position` step that wraps, but does not replace,
  `metrics::compute_equity_micros`/`compute_exposure_micros`'s existing
  `qty * mark` computation) — never inside `accounting.rs`'s FIFO
  cash/lot bookkeeping, which must stay multiplier-free and
  currency-free for the equity ledger-of-record.
- **How equity stays multiplier=1:** `equity_policy()`/`etf_as_equity_policy()`
  in `core-rs/crates/mqk-execution/src/asset_risk_policy.rs` already declare
  `requires_contract_multiplier: false` for both; the bridge
  (`ASSET-CORE-04B`) already resolves equity instruments to
  `InstrumentEconomics` with multiplier `1` by construction. The cutover
  design must preserve this — multiplier is read from data, never assumed
  non-1 without an explicit non-equity `InstrumentEconomics` row.

## 5. Margin design

- **Initial margin / maintenance margin:** out of scope for the first
  cutover slice (equity has none; `requires_margin_model: true` is
  currently only set for `future`/`option`/`forex`/`rates_fixed_income` in
  `asset_risk_policy.rs`). A margin model must be designed and
  scenario-tested as its own patch before any of those asset classes may
  route.
- **No-margin spot asset handling:** crypto spot is `requires_margin_model:
  false` today (`crypto_policy()`) — a crypto cutover slice does not block
  on margin, only on fractional quantity (§2) and currency conversion (§6).
- **Default fail-closed behavior:** any asset class with
  `requires_margin_model: true` and no live margin computation available
  must be refused at the risk layer (§8), not silently allowed through with
  a zero/placeholder margin requirement.

## 6. Currency conversion design

- **Account currency:** must be an explicit, single, configured value (USD
  today, matching the existing `PORTFOLIO_ECONOMICS_ACCOUNT_CURRENCY`
  diagnostic constant in `mqk-daemon/src/routes/portfolio.rs`) — not
  inferred per-instrument.
- **Instrument currency:** sourced from `InstrumentEconomics`, same as
  multiplier (§4).
- **FX source:** does not exist anywhere in the workspace today (confirmed
  by the Phase A audit, §9) and must be an explicit new decision — not
  assumed to be Alpaca, not assumed to be free, not assumed to be
  real-time. This decision is deliberately deferred to whichever future
  patch actually needs non-USD instruments (crypto/forex are USD-quoted in
  the common case, which may avoid this dependency for the first cutover
  slice entirely).
- **Unsupported currency pair behavior:** must refuse
  (`InstrumentEconomicsTruthState::CurrencyConversionUnsupported` already
  models this refusal) — never silently pass through unconverted.

## 7. NAV design

- **Live NAV:** the existing `aggregate_portfolio_economics`
  (`ASSET-CORE-04C`, `core-rs/crates/mqk-portfolio/src/portfolio_economics.rs`)
  already computes multi-asset NAV correctly for the equity special case
  (proven by `scenario_asset_core_04_live_ledger_invariants.rs`) and is the
  correct target function to promote to a production caller, not a
  from-scratch rewrite.
- **Per-asset-class exposure:** already modeled by
  `PortfolioEconomicsSnapshot`'s asset-class/currency exposure breakdown
  (`ASSET-CORE-04C`).
- **Missing marks:** must fail closed exactly as `compute_portfolio_weights`
  already does today (`"missing_marks"`/`"nav_unavailable"`,
  `PORTFOLIO-LIVE-WEIGHTS-01`) — this fail-closed behavior must be
  preserved, not weakened, by any cutover.
- **Stale marks:** must be rejected using the existing
  `MD-STALENESS-PER-TICK-GATE-01` staleness concept rather than inventing a
  second staleness check.
- **Multi-currency marks:** blocked on §6's FX decision; must fail closed
  until that decision is made and implemented.

## 8. Risk-enforcement design

- **Which risk checks should consume the new live accounting output:**
  none, in this design-only patch. When a future behavior-changing patch
  wires this in, the target is `mqk-risk`'s gate-evaluation path gaining a
  new optional input (live multiplier/currency/margin-aware NAV and
  exposure) — additive to, not a replacement of, the existing whole-unit
  gates.
- **Which checks remain unchanged:** every existing equity gate
  (`enforce_max_gross_exposure`, staleness, halt, etc.) — none of these are
  touched by this design.
- **Fail-closed conditions:** any risk check reading the new economics
  input must refuse (not silently fall back to the old whole-unit number)
  if the economics input is unavailable, matching `CLAUDE.md`'s fail-closed
  invariant. `asset_risk_policy.rs`'s existing static per-class
  `requires_margin_model`/`requires_contract_multiplier`/
  `requires_currency_conversion` flags (already live via
  `GET /api/v1/system/asset-risk-policy/status`) are the authoritative
  per-class readiness gate for whether a class's risk check may even
  attempt to consume the new input yet.

## 9. Order-routing design

- **What must be validated before an order reaches `BrokerGateway`:** for
  any non-equity order, `OrderIntentV2::assert_equity_whole_units`-style
  validation (already present for equity) must have an equivalent
  non-equity validation that confirms quantity, multiplier, and currency
  all resolved successfully from `InstrumentEconomics` before the intent
  is ever constructed for submission — not validated only at the broker
  boundary.
  `mqk_execution::asset_risk_policy::evaluate_asset_risk_for_order_intent_v2`
  already provides the routability classification
  (`AllowedEquity`/`DisabledAssetClass`/`ResearchOnly`/`Unsupported`/`Invalid`)
  this validation would compose with.
- **What stays unchanged until a later broker-specific patch:**
  `mqk-execution::gateway`'s actual submit call and everything in
  `mqk-broker-alpaca` — per `broker_rules.md`, broker adapter behavior is
  out of scope for the accounting/risk cutover design entirely and must be
  its own patch, gated on `broker_rules.md`'s own inbound-lane and
  no-synthetic-lifecycle invariants.

## 10. Rollout design

- **Default-off config guard:** any cutover code path must be gated behind
  a new, explicitly-named, default-`false` flag (mirroring
  `ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED`/`ASSET_RISK_NON_EQUITY_ROUTING_ENABLED`'s
  existing pattern) — never behind an existing flag being silently
  repurposed.
- **Read-only shadow mode:** the new economics-aware valuation must be
  computable and comparable against live equity accounting (exactly as
  `scenario_asset_core_04_live_ledger_invariants.rs` already proves
  offline, in tests) *in production*, logged/exposed via a status route,
  before any enforcement path reads it.
- **Paper-only mode:** after shadow-mode parity is proven live, a
  paper-only enablement (broker/routing unchanged, only internal
  accounting/risk reads the new path) precedes any live-money path.
- **Production cutover gate:** requires an explicit operator authorization
  phrase (see Phase D's go/no-go decision for the exact phrase) — this
  design patch does not authorize it.
- **Rollback path:** flipping the config guard back to `false` must fully
  restore the pre-cutover code path with no data loss — the ledger
  (`PortfolioState`/DB tables) must remain valid and readable by the old
  code path at all times, which is why §2 requires additive schema changes
  only, never in-place column changes.

## 11. Testing strategy

- **Pure unit tests:** economics valuation math (multiplier application,
  currency-refusal paths) — extending the existing
  `mqk-portfolio/tests/scenario_portfolio_instrument_economics_asset_core_04a.rs`
  style rather than a new framework.
- **DB migration tests:** any new migration must have a round-trip test
  (write via new column/table, read back, confirm old whole-unit columns
  untouched) before it can be considered mergeable.
- **Scenario tests:** cross-module equivalence tests in the style of
  `scenario_asset_core_04_live_ledger_invariants.rs`, extended to
  non-equity fixtures once a real non-equity mark source exists
  (`ASSET-CORE-04E`'s decision, already closed).
- **No-network proof:** every test above must run with zero network calls,
  matching every existing `ASSET-CORE-04*` test file's own constraint.
- **Paper-only proof:** before any paper cutover, a scenario test must
  prove the shadow-mode economics path produces a value without the
  broker/gateway path being touched (no order submitted).
- **Live forbidden:** no test in this lineage may submit a live order —
  this must remain a hard, checked invariant in CI, not just a convention.

## 12. Proposed future patch sequence

1. **DB/schema design patch** — new migration(s) adding fractional-quantity
   and currency/multiplier columns/tables, additive only, no existing
   column altered. Design-only or schema-only; no production code reads it
   yet.
2. **Fixed-point quantity model patch** — promote `QtyMicros` from
   `OrderIntentV2`-only to a shared, tested quantity type usable by a new
   ledger-adjacent type; still zero production callers in the live
   accounting path.
3. **Live accounting shadow-mode patch** — wire the new economics-aware
   valuation as a read-only, default-off, parallel computation alongside
   existing `accounting.rs`/`metrics.rs`, exposed via a status route for
   comparison; no risk or order-routing change.
4. **Risk shadow-mode patch** — `mqk-risk` computes (but does not act on)
   the new economics-aware NAV/exposure/margin input, comparing against
   existing gate decisions.
5. **Paper-only cutover patch** — risk enforcement begins consuming the new
   input for paper orders only, broker/gateway path unchanged, explicit
   operator authorization required per Phase D.
6. **Broker-specific non-equity patch** — a separate, later patch (per
   `broker_rules.md`) to add non-equity order submission capability to a
   specific broker adapter, gated on all of the above being proven in
   paper mode first.

## 13. Explicitly forbidden until later

- Non-equity live trading.
- Live routing (any asset class).
- Broker adapter changes (`mqk-broker-alpaca` or any future adapter).
- Weakening any existing risk gate, staleness check, or fail-closed
  behavior in the name of "supporting" the new economics path.
- Silent fallback to a wrong (assumed) multiplier or currency for any
  instrument the economics model cannot resolve — must refuse, not guess.

---

## Safety statement

No live or paper order was submitted. No broker, provider, or network call
was made. No DB migration was added. No config flag was changed. No
production `.rs` source file's behavior was modified — this document
specifies a target design and a proposed future patch sequence only; every
future patch it names requires its own separate authorization and
scenario-test proof before it may be started.
