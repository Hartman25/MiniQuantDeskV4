# ASSET-CORE-04L — Production Cutover Callsite Audit

**Patch:** `ASSET-CORE-04L-PRODUCTION-CUTOVER-CALLSITE-AUDIT-01`
(Phase A of `ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01-COMBINED`)

**Method:** direct source read plus cross-crate caller-map grep across
`core-rs/`, re-verified against current HEAD rather than trusted from any
prior session's memory or doc claim. No network call, no DB connection, no
daemon start, no provider/broker contact, no code changed.

---

## 1. Current HEAD

`87f49a19` (branch `main`, clean working tree at audit time; only
pre-existing untracked `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`
and `smoke_logs/` present, neither touched by this audit).

## 2. Current parent status of `ASSET-CORE-04`

```text
ASSET-CORE-04: PARTIAL / LIVE-ACCOUNTING-PRODUCTION-CONSUMPTION-OPEN
```

Per `docs/specs/asset_core_04_live_ledger_closure_decision.md` (commits
`b99e49b9` → `e21bbbaf` → `87f49a19`). This audit re-derives, rather than
assumes, that the underlying source facts behind that verdict still hold at
current HEAD.

---

## 3. Exact callsites in live accounting that would need to change

All of the following are the **unmodified, production-authoritative** live
accounting path. This audit changes none of them.

| File | Symbols | What would need to change for production cutover |
|---|---|---|
| `core-rs/crates/mqk-portfolio/src/types.rs` | `Fill { qty: i64, price_micros: i64, fee_micros: i64 }` (line 16), `Lot { qty_signed: i64, entry_price_micros: i64 }` (line 73), `PortfolioState { cash_micros: i64, realized_pnl_micros: i64, .. }` (line 142) | Quantity fields would need a fixed-point/fractional-capable representation (or a parallel type) before non-equity or fractional-equity positions could be recorded; no `currency` or `multiplier` field exists on any of these types today. |
| `core-rs/crates/mqk-portfolio/src/accounting.rs` | `apply_entry`, `apply_fill`, `buy_fifo`, `sell_fifo` (lines 23-80+) | `mul_qty_price_micros` (line 5) computes plain `qty * price_micros` in `i128`; would need an explicit multiplier term and, for non-USD instruments, a currency-conversion step before cash impact is computed. |
| `core-rs/crates/mqk-portfolio/src/metrics.rs` | `compute_exposure_micros`, `compute_unrealized_pnl_micros`, `compute_equity_micros` (lines 39-80+) | Same `qty * mark` convention (`mul_qty_price_micros_i128`, line 6); would need multiplier and currency terms to be NAV/exposure-correct for non-equity instruments. |
| `core-rs/crates/mqk-portfolio/src/valuation.rs` | `compute_portfolio_weights` (`PORTFOLIO-LIVE-WEIGHTS-01`) | Same implicit multiplier=1/single-currency assumption; fails closed (`"missing_marks"`/`"nav_unavailable"`) today rather than fabricating, which is the correct fail-closed behavior to preserve in any cutover. |

## 4. Exact callsites in runtime snapshots that would need to change

| File | Symbols | What would need to change |
|---|---|---|
| `core-rs/crates/mqk-runtime/src/observability.rs` | `PortfolioSnapshot { cash_micros: i64, realized_pnl_micros: i64, positions: Vec<PositionSnapshot> }`, `PositionSnapshot { net_qty: i64 }` (`net_qty: p.qty_signed()`, sourced directly from `PositionState`) | `net_qty: i64` would need to become fractional-capable (or gain a parallel fractional field) for any asset class whose positions are not whole-unit; currently a direct pass-through of `mqk-portfolio`'s whole-unit type. |
| `core-rs/crates/mqk-daemon/src/routes/portfolio.rs` | `PortfolioPositionRow { qty: i64, avg_price: f64, .. }` (`/api/v1/portfolio/positions`, `/orders/open`, `/fills`) | Same whole-unit assumption surfaced to the operator GUI/API; any cutover of the underlying model must keep this response shape truthful or version it. |

## 5. Exact callsites in risk enforcement that would need to change

| File | Symbols | Current state |
|---|---|---|
| `core-rs/crates/mqk-risk/src/*` | — | Zero references to any `*economics*` symbol anywhere in the crate (confirmed by grep; one unrelated substring false-positive for `RiskEngineUnavailable` in `scenario_risk_decisions_b2.rs`, not economics-related). Risk enforcement today reads only the whole-unit live accounting path (§3), never `InstrumentEconomics`/`PortfolioEconomicsSnapshot`. |
| `core-rs/crates/mqk-execution/src/asset_risk_policy.rs` | `AssetRiskPolicy`, `default_asset_risk_policies()`, `evaluate_asset_risk_for_order_intent_v2` (lines 20-283) | A **static**, model-only per-asset-class readiness table (`requires_margin_model`, `requires_contract_multiplier`, `requires_currency_conversion`, `requires_session_profile`). It does not read live portfolio state, NAV, or any economics-scaffold output — it only classifies asset-class readiness independent of any specific position. Module constants `ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED = false` and `ASSET_RISK_NON_EQUITY_ROUTING_ENABLED = false` (lines 54-55) are compile-time facts, not runtime defaults, and a production cutover would need to flip these deliberately and only after risk enforcement actually consumes live NAV/margin/multiplier data — not before. |

For a production cutover, `mqk-risk`'s gate-evaluation path would need a new
input (live NAV/margin/exposure computed with multiplier and currency
awareness) threaded through from wherever the cutover accounting model
lives, and `asset_risk_policy.rs`'s static table would need to become a
live, position-aware evaluation rather than a static per-class flag table.

## 6. Exact callsites in broker/order routing that must not change until accounting/risk are ready

| File | Symbols | Current state |
|---|---|---|
| `core-rs/crates/mqk-execution/src/gateway.rs` | broker-submit path | Zero references to `OrderIntentV2` or `QtyMicros` anywhere in the file (confirmed by grep at this HEAD). The only order representation the gateway ever submits is the legacy whole-unit `OrderIntent`/`OrderSpec`-equivalent type (`core-rs/crates/mqk-execution/src/types.rs` lines 56-60). This must not change until risk enforcement (§5) and live accounting (§3) both consume the new model — routing is downstream of both. |
| `core-rs/crates/mqk-execution/src/types.rs` | `OrderIntentV2 { qty: QtyMicros, .. }` (line 122, `qty` field line 127), `assert_equity_whole_units` (line 229) | `OrderIntentV2` is a fractional-capable model type that already exists and already guards equity against fractional quantity — but it is never constructed, validated, or submitted by the gateway (§above), so this capability is unreachable in production today. A cutover would route through this type only after §3-§5 are ready, not before. |
| `core-rs/crates/mqk-broker-alpaca/src/lib.rs` | broker adapter | Not audited for change in this bundle; per `broker_rules.md`, broker adapter behavior is out of scope for this design-only patch entirely. |

## 7. Exact DB tables/columns that currently encode whole-unit quantity assumptions

Every quantity-bearing column across all 44 migrations (`0001`-`0044`) is
`bigint`, confirmed by grep for `qty|quantity` across
`core-rs/crates/mqk-db/migrations/`:

| Migration | Table | Column(s) |
|---|---|---|
| `0026_risk_denial_events.sql` | risk denial events | `requested_qty bigint`, `limit_qty bigint` |
| `0028_fill_quality_telemetry.sql` | fill quality telemetry | `ordered_qty bigint not null`, `fill_qty bigint not null check (fill_qty > 0)` |
| `0035_oms_order_lifecycle_events.sql` | OMS order lifecycle events | `new_total_qty bigint` (replace-ack authoritative post-replace total) |
| `0043_strategy_signal_evaluations.sql` | strategy signal evaluations | `signal_qty bigint` |

No fixed-point-fraction, `numeric`, or `decimal` quantity column exists in
any migration. `oms_outbox`/`oms_inbox` and every fill/position-adjacent
table (per `db_rules.md`'s append-only migration discipline) would need a
new column (not a modification of an existing one) to carry fractional
quantity, since existing committed migrations must never be altered.

## 8. Whether a DB migration is needed before fractional live positions

**Yes.** Every quantity column that would need to represent a fractional
position (`oms_outbox`, `oms_inbox`, fill-quality, order-lifecycle, and any
future position/ledger-snapshot table) is `bigint` today. Per `db_rules.md`,
existing migrations are append-only and cannot be modified — a fractional
quantity representation requires a **new** migration (new column(s) or new
table(s)), not a change to any of `0001`-`0044`. This audit proposes no such
migration; it is Phase B/C's (design/plan) job to specify its shape, and no
migration may be added in this patch (`db_rules.md`, hard safety rules
above).

## 9. Whether currency conversion requires a new account-currency/source-of-FX-truth decision

**Yes.** No `currency` field exists anywhere in `accounting.rs`,
`metrics.rs`, or `valuation.rs` today. The one place a currency assumption
is made explicit is `GET /api/v1/portfolio/economics/status`'s
`PORTFOLIO_ECONOMICS_ACCOUNT_CURRENCY = "USD"` constant
(`core-rs/crates/mqk-daemon/src/routes/portfolio.rs`) — a read-only
diagnostic naming an already-implicit assumption, not a conversion. The
economics scaffold itself only ever *refuses* on currency mismatch
(`InstrumentEconomicsTruthState::CurrencyConversionUnsupported` in
`core-rs/crates/mqk-portfolio/src/instrument_economics.rs`) — no FX rate
source, no conversion function, and no account-currency concept exists
anywhere in the workspace. A production cutover would require an explicit
decision on: what the account currency is, where FX rates come from, and
what happens when a rate is stale or missing (must fail closed, per
`CLAUDE.md`'s core invariants).

## 10. Whether margin requires a new margin-model source-of-truth decision

**Yes.** No `margin` symbol exists anywhere in `mqk-portfolio`,
`mqk-execution`, or `mqk-risk` production source (confirmed by grep). The
only `margin`-related symbols in the entire workspace are
`AssetRiskPolicy.requires_margin_model` (a static, model-only per-asset-class
boolean flag, `core-rs/crates/mqk-execution/src/asset_risk_policy.rs`) and
`mqk-backtest`'s independent, backtest-only economics module
(`core-rs/crates/mqk-backtest/src/economics.rs`), which has no bearing on
live accounting. A production cutover needs an explicit decision on initial
margin, maintenance margin, and fail-closed behavior when margin data is
unavailable — none of this exists as a live-accounting concept today.

## 11. Whether contract multipliers can be introduced without enabling non-equity execution

**Yes, in principle, but not without care.** `InstrumentEconomics`
(`core-rs/crates/mqk-portfolio/src/instrument_economics.rs`) already models
a multiplier field and `value_position_economics` already applies it
correctly for the equity special case (multiplier=1), proven by
`scenario_asset_core_04_live_ledger_invariants.rs`'s `EQ-01`..`EQ-06` tests.
Introducing multiplier-awareness into live `accounting.rs`/`metrics.rs`
*could* be done as an equity-only, multiplier-always-1 no-op change in
principle — but doing so is still a live-accounting-math change
(`CLAUDE.md`: "preserve lifecycle correctness", `db_rules.md`/`execution_rules.md`
invariants) and is explicitly out of scope for this design-only bundle. Any
such change must land as its own reviewed, scenario-tested patch, and must
not be bundled with non-equity enablement — `asset_risk_policy.rs`'s
`ASSET_RISK_NON_EQUITY_ROUTING_ENABLED = false` must remain false
independent of any multiplier-support work in the accounting layer.

## 12. Safe/non-safe cutover classification

| Class | Examples | This bundle? |
|---|---|---|
| **Safe: docs/tests/status only** | This audit; `asset_core_04m_production_cutover_design.md`; `asset_core_04n_cutover_test_and_migration_plan.md`; `asset_core_04o_cutover_go_nogo_decision.md`; any future read-only status route composing already-existing truth | Yes — this is the entirety of this bundle's scope. |
| **Behavior-preserving internal model** | Adding a new fixed-point quantity type, a new DB migration/table, or a shadow-mode computation path that is computed but never read by any production decision | **Not performed in this bundle.** Would be a future patch's scope (see Phase B `04M`'s proposed patch sequence), and even then requires its own scenario-test proof per `audit_repo_truth_rules.md`. |
| **Behavior-changing production cutover** | Wiring `InstrumentEconomics`/`PortfolioEconomicsSnapshot` into `accounting.rs`/`metrics.rs`, `mqk-risk`'s gate evaluation, or `mqk-execution::gateway`'s submit path; flipping `ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED`/`ASSET_RISK_NON_EQUITY_ROUTING_ENABLED`; adding a live/paper order path for a new asset class | **Explicitly not performed, and not authorized, by this bundle.** Requires separate, explicit operator authorization per Phase D's go/no-go decision. |

## 13. Explicit non-goals

- No code cutover — no production `.rs` file's runtime behavior was changed
  by this audit.
- No DB migration was added or proposed as ready-to-apply.
- No live or paper order was submitted, and none will be as a result of
  this audit.
- No non-equity asset class was enabled — `AssetRiskPolicy` state for
  `crypto`/`future`/`option`/`forex` remains `Disabled`, `rates_fixed_income`
  remains `ResearchOnly`, unchanged by this audit.
- No risk enforcement wiring was added — `mqk-risk` still has zero
  references to any economics-scaffold symbol.

---

## Safety statement

No live or paper order was submitted. No broker, provider, or network call
was made. No DB connection was made and no migration was added. No
non-equity asset class was enabled. No config flag was changed. No
production `.rs` source file's behavior was modified — this document is a
read-only audit of already-committed source.
