# ASSET-CORE-04 — Current Live Ledger Boundary Audit

**Patch:** `ASSET-CORE-04A-CURRENT-LIVE-LEDGER-BOUNDARY-AUDIT-01`
(part of `ASSET-CORE-04-LIVE-LEDGER-SECTION-CLOSURE-01-COMBINED`)

**HEAD at audit time:** `1098bbb1` (branch `main`, clean working tree).

**Method:** grep across `core-rs/` for every live-accounting and
economics-scaffold symbol, direct reading of every file the grep surfaced
that is plausibly on the live path, and a caller map built by grepping
`mqk-execution`, `mqk-risk`, and `mqk-runtime` for the scaffold's own public
symbol names. No network call, no DB connection, no daemon start, no
provider/broker contact.

---

## 1. Status of `ASSET-CORE-04`

Per `MiniQuantDesk_Master_Patch_Ledger_v2.md` (`ASSET-CORE-02-03E-CLOSURE-AND-ROADMAP-RECONCILE-01`,
commit `1e1ce999`-era history): `ASSET-CORE-02` and `ASSET-CORE-03` are
`CLOSED_LOCAL` for their own model/policy-boundary scope, and that closure
doc states explicitly: *"True graduated live risk enforcement remains
blocked on `ASSET-CORE-04` live portfolio/margin/NAV accounting, not yet
wired into any enforcement path."* `ASSET-CORE-04` itself has six
already-closed sub-slices (`04A`, `04B`, `04C`, `04D`, `04E`, `04F` — see
§4 and §5) but no production-consumption slice. This audit is the seventh
sub-slice, auditing the boundary rather than moving it.

---

## 2. Exact live accounting path

All of the following are the **unmodified, production-authoritative** path.
Nothing in this audit or its sibling patches in this bundle touches any of
these files' behavior.

| Layer | File | Key symbols |
|---|---|---|
| Ledger types | `core-rs/crates/mqk-portfolio/src/types.rs` | `Fill { qty: i64, price_micros: i64, fee_micros: i64 }`, `Lot { qty_signed: i64, entry_price_micros: i64 }`, `PositionState`, `PortfolioState { cash_micros: i64, realized_pnl_micros: i64, ... }` |
| FIFO accounting | `core-rs/crates/mqk-portfolio/src/accounting.rs` | `apply_entry`, `apply_fill`, `buy_fifo`, `sell_fifo`, `recompute_from_ledger` — plain `qty * price_micros` in `i128`, no multiplier term, no currency field, no margin field anywhere in the module |
| Live metrics | `core-rs/crates/mqk-portfolio/src/metrics.rs` | `compute_equity_micros`, `compute_exposure_micros`, `compute_unrealized_pnl_micros`, `enforce_max_gross_exposure` — same `qty * mark` convention, no multiplier |
| Live weights/NAV | `core-rs/crates/mqk-portfolio/src/valuation.rs` (`PORTFOLIO-LIVE-WEIGHTS-01`) | `compute_portfolio_weights` — signed `i64` qty × mark, fails closed (`"missing_marks"` / `"nav_unavailable"`) rather than fabricating, but still implicitly multiplier=1 / single-currency |
| Runtime derivation | `core-rs/crates/mqk-runtime/src/observability.rs` | `PortfolioSnapshot { cash_micros: i64, realized_pnl_micros: i64, positions: Vec<PositionSnapshot> }`, `PositionSnapshot { net_qty: i64 }` — `net_qty: p.qty_signed()`, i.e. sourced directly from `PositionState` |
| Daemon broker-snapshot routes | `core-rs/crates/mqk-daemon/src/routes/portfolio.rs` (types in `api_types/portfolio_snapshot.rs`) | `PortfolioPositionRow { qty: i64, avg_price: f64, ... }` for `/api/v1/portfolio/positions`, `/orders/open`, `/fills` |
| DB tables (order/fill quantities) | `core-rs/crates/mqk-db/migrations/*.sql` | `oms_outbox`/`oms_inbox`/fill-quality/order-lifecycle tables all use `bigint` for quantity columns (`requested_qty`, `ordered_qty`, `fill_qty`, `new_total_qty`, `signal_qty`) — whole-unit, no fixed-point-fraction column exists |
| Order submission model | `core-rs/crates/mqk-execution/src/types.rs` (legacy, pre-V2) | `OrderIntent`/`OrderSpec`-equivalent legacy types at lines 15-31 use plain `qty: i64` — this is what the gateway actually submits |
| Broker submit gate | `core-rs/crates/mqk-execution/src/gateway.rs` | Zero references to `OrderIntentV2` or `QtyMicros` anywhere in the file (confirmed by grep) — the only order representation the gateway ever sees is the legacy whole-unit type |

**No file in this table imports, calls, or references anything from
`instrument_economics.rs`, `portfolio_economics.rs`, or the daemon's
`instrument_economics_bridge.rs` (§3).** Confirmed by grep for
`InstrumentEconomics|instrument_economics|PortfolioEconomics|portfolio_economics|value_position_economics|aggregate_portfolio_economics`
across `mqk-execution/src`, `mqk-risk`, and `mqk-runtime/src`: zero matches
in any of the three crates (one incidental substring hit in
`mqk-risk/tests/scenario_risk_decisions_b2.rs` for `RiskEngineUnavailable`,
unrelated to economics).

---

## 3. Exact economics scaffold path

| Slice | File | Key symbols | Callers |
|---|---|---|---|
| `ASSET-CORE-04A` | `core-rs/crates/mqk-portfolio/src/instrument_economics.rs` | `InstrumentEconomics`, `InstrumentEconomicsTruthState`, `PositionEconomicsInput`, `PositionEconomicsValue`, `value_position_economics` | `04B` bridge, `04C` aggregator, its own 27-case test file, `04D` route |
| `ASSET-CORE-04B` | `core-rs/crates/mqk-daemon/src/state/instrument_economics_bridge.rs` | `instrument_v2_to_economics`, `bridge_instrument_registry_v2_to_economics`, `InstrumentEconomicsBridgeResult { trading_enabled_by_bridge: false, .. }` | `04D` route only |
| `ASSET-CORE-04C` | `core-rs/crates/mqk-portfolio/src/portfolio_economics.rs` | `PortfolioEconomicsInput`, `PortfolioEconomicsSnapshot`, `aggregate_portfolio_economics` | `04D` route, its own test file |
| `ASSET-CORE-04D`/`04F` | `core-rs/crates/mqk-daemon/src/routes/portfolio.rs` (`portfolio_economics_status` + helpers, lines ~563-1280) | `GET /api/v1/portfolio/economics/status?registry_source=legacy\|v2` | Zero — see §5 |
| Tests | `core-rs/crates/mqk-portfolio/tests/scenario_portfolio_instrument_economics_asset_core_04a.rs`, `scenario_portfolio_economics_aggregation_asset_core_04c.rs`; `core-rs/crates/mqk-daemon/tests/scenario_instrument_economics_registry_bridge_asset_core_04b.rs`, `scenario_portfolio_economics_status_asset_core_04d.rs`, `scenario_portfolio_economics_v2_registry_seam_asset_core_04f.rs` | — | Pre-existing, already passing at HEAD |

Every one of these five source files carries an explicit module-doc
sentence to the effect of *"Nothing in this module is called by
`accounting.rs`, `metrics.rs`, or `valuation.rs`... zero production callers
anywhere in the workspace."* This audit independently verifies that claim
by caller-map grep (§2, §5) rather than trusting the comment.

---

## 4. Caller map: is the economics scaffold production-consumed?

| Consumer crate | Grep result |
|---|---|
| `mqk-execution` (gateway/submit/risk-decision) | Zero references to any `*economics*` symbol |
| `mqk-risk` | Zero references (one unrelated substring false-positive noted in §2) |
| `mqk-runtime` (orchestrator/observability/dispatch) | Zero references |
| `mqk-daemon` GUI-facing route wiring outside the `04D`/`04F` route itself | Zero — `routes.rs`/`state.rs` register the route but nothing else calls it internally |
| `core-rs/mqk-gui/src` (frontend) | Zero — grepped for `economics` (case-insensitive); the 9 matches are all backtest-economics (`ASSET-CORE-02`-era backtest UI) and `InstrumentRegistryV2SourcePanel`, none reference `/portfolio/economics/status` |

**Conclusion: the `ASSET-CORE-04A`-`04C`/`04B` economics scaffold has zero
production callers, and its one HTTP-reachable surface (`04D`/`04F`) has
zero *callers* of that surface** — it is reachable only by a human hitting
the URL directly or by its own test suite. The route's own response type
(`PortfolioEconomicsStatusResponse` in `mqk-daemon/src/api_types.rs`
lines 2188-2277) already self-documents this with four always-`false`
boolean fields: `trading_uses_portfolio_economics`,
`runtime_uses_portfolio_economics`, `risk_uses_portfolio_economics`,
`order_path_uses_portfolio_economics`, plus `model_only: true`.

---

## 5. Current live assumptions (all confirmed true at HEAD)

- **Whole-unit quantity.** `Fill.qty: i64`, `Lot.qty_signed: i64`,
  `PositionSnapshot.net_qty: i64`, every DB quantity column is `bigint`.
  `mqk-execution`'s V2 order model (`OrderIntentV2.qty: QtyMicros`,
  `mqk-schemas::QtyMicros`) *can* represent fractional quantity, and
  `OrderIntentV2::assert_equity_whole_units` exists precisely to guard
  equity against it — but `OrderIntentV2` is never constructed, validated,
  or submitted by the gateway (§2), so this capability is unreachable in
  production today.
- **Single currency.** No `currency` field exists anywhere in
  `accounting.rs`, `metrics.rs`, or `valuation.rs`. The `04D` status route
  hardcodes `PORTFOLIO_ECONOMICS_ACCOUNT_CURRENCY = "USD"` with a doc
  comment explicitly naming this as making an *already-implicit* live
  assumption explicit, not introducing a new one.
- **Multiplier implicitly 1.** `metrics::compute_equity_micros` and
  `compute_exposure_micros` compute `qty * mark` with no multiplier factor
  at all — there is no field to set to a non-1 value even if desired.
- **No margin model.** No `margin` symbol appears anywhere in
  `mqk-portfolio`, `mqk-execution`, or `mqk-risk` production source. The
  only `margin`-related symbols in the entire workspace are
  `AssetRiskPolicy.requires_margin_model` (a static, model-only boolean
  flag per asset class — see §6) and `mqk-backtest`'s independent,
  backtest-only economics module.
- **No FX/currency conversion.** The economics scaffold *models* this
  question (`InstrumentEconomicsTruthState::CurrencyConversionUnsupported`)
  but always refuses rather than converting — by construction, no code
  path anywhere performs currency conversion.
- **Equity-only routeability.** `mqk_execution::asset_risk_policy` (`ASSET-CORE-03B`,
  `core-rs/crates/mqk-execution/src/asset_risk_policy.rs`) statically marks
  only `equity`/`etf_as_equity` as `AssetRiskPolicyState::Enabled` with
  `paper_trading_enabled: true`; every other asset class
  (`crypto`, `future`, `option`, `forex`) is `Disabled`, and
  `rates_fixed_income` is `ResearchOnly` — all with
  `paper_trading_enabled: false, live_trading_enabled: false`. Module-level
  constants `ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED = false` and
  `ASSET_RISK_NON_EQUITY_ROUTING_ENABLED = false` make this a compile-time
  fact, not just a runtime default.

No live/paper order, broker call, or DB mutation was performed to confirm
any of the above — every fact in this section is confirmed by reading
source, not by running the system.

---

## 6. What `ASSET-CORE-04` blocks

- **`ASSET-CORE-03` live enforcement.** `asset_risk_policy.rs`'s own
  per-class `reason_code`/`message` fields name the exact blockers: futures
  and options are disabled pending "margin model, contract multiplier,
  expiry handling"; forex is disabled pending "currency conversion, pip/lot
  sizing, leverage"; crypto is disabled pending "pair registry, fractional
  quantity policy". `ASSET-CORE-04`'s live-portfolio-NAV/margin/multiplier
  accounting is the common prerequisite underlying every one of those
  reasons, which is why `ASSET-CORE-02-03E`'s closure doc names `04` as the
  next blocker (§1).
- **Futures/options/crypto/forex/rates risk and non-equity execution.**
  Same policy table: every non-equity class is `Disabled`/`ResearchOnly`
  with both `paper_trading_enabled` and `live_trading_enabled` false. This
  bundle does not change any of those flags.

---

## 7. Safe closure target for this bundle

- **Phase A (this document):** ground `ASSET-CORE-04`'s current state in
  current-repo evidence rather than prior session claims.
- **Phase B:** add the one cross-module regression proof that does **not**
  already exist in the test suite — a numeric equivalence test showing
  live `accounting`/`metrics` output and the `ASSET-CORE-04A` economics
  model's equity special case (`multiplier = 1x`, matching currency) agree
  exactly for the same fills, closing the gap between the scaffold's
  doc-comment claim ("multiplier=1 here reproduces their un-multiplied
  `qty * price` math exactly") and an actual assertion of it. See
  `core-rs/crates/mqk-portfolio/tests/scenario_asset_core_04_live_ledger_invariants.rs`.
- **Phase C/D:** see §8 — both skipped as duplicative of already-closed,
  already-live-truth-reporting surfaces.
- **Phase E:** an honest closure/reconcile doc naming the one remaining
  gap (production consumption) as explicitly out of this bundle's scope.

---

## 8. Unsafe / non-goal items (and why Phase C/D are skipped)

This bundle does **not**, and this audit found no safe gap that would
require it to:

- perform any production accounting cutover — live accounting stays
  exactly as described in §2, unmodified;
- enable any non-equity asset class for trading — `AssetRiskPolicy` state
  is unchanged;
- wire live risk enforcement to NAV/margin/multiplier economics — no
  caller is added anywhere in `mqk-risk` or `mqk-execution`'s submit path;
- submit any live or paper order, contact any broker/provider, or touch
  DB state — this audit is read-only, static analysis of committed source;
- add a DB migration — no observability-only proof requires one; every
  quantity/currency/multiplier fact needed is already derivable from
  existing tables and in-memory state.

**Phase C would be duplicative.** `PortfolioEconomicsStatusResponse`
(`04D`/`04F`, already committed) already carries the exact class of
honesty fields Phase C's mission describes wanting to add
(`model_only`, `trading_uses_portfolio_economics`,
`runtime_uses_portfolio_economics`, `risk_uses_portfolio_economics`,
`order_path_uses_portfolio_economics`, all fixed `false`/`true` as
appropriate) — see §4. Adding a second, differently-named set of the same
booleans on a new route would only create two sources of truth that could
drift from each other. Phase C is skipped for this reason; no files were
changed for Phase C.

**Phase D would be duplicative.** `GET /api/v1/system/asset-risk-policy/status`
(`ASSET-CORE-03B`, already committed,
`core-rs/crates/mqk-daemon/src/routes/system.rs:1432`) already reports,
per asset class and live from `mqk_execution::default_asset_risk_policies()`
(not a hardcoded string table), exactly the readiness-classifier shape
Phase D's mission describes (`requires_margin_model`,
`requires_contract_multiplier`, `requires_currency_conversion`,
`requires_session_profile`, plus workspace-level
`production_enforcement_enabled: false` /
`non_equity_routing_enabled: false`). A second
`AssetCore04RiskEnforcementReadiness` classifier would either duplicate
this route's live truth or (worse) hardcode roadmap-status strings — the
exact anti-pattern `ASSET-CORE-02-03E`'s closure doc already rejected for
the same reason. Phase D is skipped; no files were changed for Phase D.

---

## 9. Answers to the pre-flight questions

1. **Live portfolio accounting path today:** §2.
2. **Economics scaffold today:** §3.
3. **Does live accounting read `InstrumentEconomics`/registry-v2 economics?** No — §2, §4.
4. **Does `mqk-risk` read live NAV/margin/multiplier economics?** No — §4.
5. **Does broker-submit routing read live NAV/margin/multiplier economics?** No — §2, §4.
6. **Where are quantities whole-unit `i64` today?** §5 (first bullet + table in §2).
7. **Where are quantities fractional/fixed-point today, if anywhere?** Only as an unreachable model capability (`OrderIntentV2.qty: QtyMicros`, never submitted) and as descriptive-only scaffold metadata (`InstrumentEconomics.quantity_scale`, never read by `value_position_economics`) — §5.
8. **Can any current route submit fractional-quantity orders?** No — the gateway never references `OrderIntentV2`/`QtyMicros` (§2).
9. **Can any current DB table record fractional position quantity?** No — every relevant column is `bigint` (§2).
10. **Where is NAV computed today, and is multiplier/currency applied?** `metrics::compute_equity_micros` and `valuation::compute_portfolio_weights`; no multiplier or currency conversion is applied (§2, §5).
11. **What existing tests prove equity/FIFO behavior today?** `mqk-portfolio/tests/scenario_pnl_partial_fills_fifo.rs`, `scenario_conservation_invariants.rs`, `scenario_position_flatten_fifo.rs`, `scenario_fill_ordering_determinism.rs`, `scenario_rounding_boundaries_m4_1.rs`, `scenario_short_position_lifecycle_01.rs` — all pre-existing, unmodified by this bundle.
12. **What exact safe gap can be closed without changing production behavior?** The Phase B cross-module equivalence proof (§7) and this audit/closure documentation itself.
13. **What exact gap requires a future production-behavior-changing patch?** Wiring the `ASSET-CORE-04A`-`04D` economics scaffold as an actual input to live accounting, risk enforcement, or order routing — i.e. a real production cutover, explicitly out of scope for this bundle and every hard safety rule it operates under.

---

## Safety statement

No live or paper order was submitted. No broker, provider, or network call
was made. No non-equity asset class was enabled. No config flag was
changed. Production consumption of the `ASSET-CORE-04` economics scaffold
remains separate and not started, exactly as it was before this audit.
