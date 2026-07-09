# ASSET-CORE-02-03A — Current Completion Audit (`ASSET-CORE-02` / `ASSET-CORE-03`)

**Patch group:** `ASSET-CORE-02-03-COMPLETION-SWEEP-01-COMBINED`
**Phase:** A / Commit 1 — `ASSET-CORE-02-03A-CURRENT-COMPLETION-AUDIT-01`
**HEAD at audit time:** `96813548` (docs: audit market-hours no-trade proof)

## 1. Purpose

This audit grounds the rest of the `ASSET-CORE-02-03` bundle in current repo
evidence only. It supersedes the stale summary rows in
`docs/audits/multi_asset_completion_audit.md` (lines ~140-141) and
`docs/specs/roadmap_completion_reconcile_01.md` (lines ~30-31), which still
read as if no foundation work had landed. It does not supersede those files'
own closure notes (`multi_asset_completion_audit.md` §36/§37), which already
correctly describe the two `-FOUNDATION-01-COMBINED` patches below.

## 2. Current status of `ASSET-CORE-02`

`CLOSED_LOCAL / PARTIAL` as of commit `ASSET-CORE-02-ORDER-INTENT-V2-FOUNDATION-01-COMBINED`
(ledger entry, `MiniQuantDesk_Master_Patch_Ledger_v2.md` line ~2863). Not
`OPEN` — a real, tested model foundation exists.

## 3. Current status of `ASSET-CORE-03`

`CLOSED_LOCAL / PARTIAL` as of commit `ASSET-CORE-03-RISK-ROUTER-FOUNDATION-01-COMBINED`
(ledger entry, line ~2897). Not `OPEN` — a real, tested per-asset-class policy
model exists, contradicting the stale claim in `roadmap_completion_reconcile_01.md`
line 31 ("zero graduated per-class policy behind them").

## 4. Where `OrderIntentV2` / `ExecutionIntentV2` live

- Defined in `core-rs/crates/mqk-execution/src/types.rs`:
  - `OrderIntentV2` (struct, lines ~118-222): keyed by `mqk_schemas::Instrument`,
    `QtyMicros`, canonical `OrderSide`/`OrderType`, builder methods
    (`with_instrument_id`, `with_contract`, `with_order_type`,
    `with_limit_price_micros`, `with_stop_price_micros`, `with_time_in_force`,
    `with_strategy_source`, `as_research_only`), plus `validate_model()` /
    `validate_model_with_caller_routing_request()`.
  - `IntentV2Contract` enum (Equity/CryptoSpot/Future/Option/ForexPair).
  - `IntentV2Routability` enum (`ResearchOnly`, `EquityRoutableCandidate`,
    `DisabledAssetClass`, `Invalid`) and `IntentV2Validation` struct.
  - `ExecutionIntentV2` (wraps `mqk_schemas::OrderSpec`, `market()` constructor,
    `validate_model()`).
  - `equity_instrument()` helper.
- Re-exported from `core-rs/crates/mqk-execution/src/lib.rs` (lines 35-38)
  under an explicit `RESEARCH-NON-EQ-01` doc comment stating these types are
  **not** wired into the canonical MAIN execution path.

## 5. Are these types compiled into any crate today?

Yes — `mqk-execution` compiles them unconditionally (not feature-gated).
`mqk-execution` is a normal workspace dependency of `mqk-daemon` and other
crates, so the *types* are reachable everywhere `mqk-execution` is a
dependency. Compiled-in is not the same as consumed; see next section.

## 6. Are they consumed by any production order/strategy/risk/broker/daemon/CLI/GUI path?

No production caller was found. Confirmed by direct grep across
`mqk-cli/src`, `mqk-daemon/src`, `mqk-broker-*/src`, `mqk-runtime/src`:
zero references to `OrderIntentV2`, `ExecutionIntentV2`, `IntentV2*`, or
`asset_risk_policy` outside `mqk-execution`'s own `src`/`tests`. The only
consumers are:
- `mqk-execution/tests/scenario_order_intent_v2_foundation_01.rs` (test-only)
- `mqk-execution/tests/scenario_asset_risk_router_foundation_01.rs` (test-only)

The live/paper dispatch path uses the separate, unrelated V1 types
(`mqk_execution::types::{OrderIntent, ExecutionIntent, ExecutionDecision}`)
through `BrokerSubmitRequest` / `BrokerGateway::submit_with_context`
(`gateway.rs`), which is where the real routing guard lives (§9 below).

## 7. What fields do `OrderIntentV2`/`ExecutionIntentV2` already model?

Instrument identity (`instrument_id`, `Instrument{symbol, asset_class, venue,
currency, contract}`), v2-local contract shape (`IntentV2Contract`), side,
`QtyMicros` (fractional-capable), order type (Market/Limit/Stop/StopLimit),
optional limit/stop price in micros, time-in-force, strategy/source metadata,
and a `research_only` marker. Validation enforces non-empty symbol/currency,
positive qty, non-empty time-in-force, required prices per order type, and
per-asset-class contract-shape matching (crypto pair, future root/expiry/
multiplier/tick, option underlying/expiry/strike/multiplier, forex pair).

## 8. What fields were missing for bracket/OCO or multi-leg orders (pre-Phase-B)?

Confirmed by grep (`bracket`, `\boco\b`, `take_profit`, `stop_loss`,
`multi.?leg`, case-insensitive) across `mqk-execution/src`,
`mqk-schemas/src`, `mqk-risk/src`: **zero matches**. No bracket, OCO, take-
profit, stop-loss, or multi-leg representation existed anywhere before this
bundle. This is the one concrete, closable `ASSET-CORE-02` model gap
identified by this audit — the candidate safe closure for Phase B of this
bundle (see §14 and the Phase E closure decision doc,
`asset_core_02_03e_closure_decision.md`, for the actual outcome).

## 9. What invariants do they validate today?

See §7. Additionally: a caller-supplied routing-request flag
(`validate_model_with_caller_routing_request`) is deliberately ignored,
proving that disabled non-equity asset classes cannot be promoted to
routable by caller intent alone.

## 10. Where exactly are Gate 0 and the broker-submit routing guard implemented?

- **Gate 0** (signal-admission asset-class check): `core-rs/crates/mqk-daemon/src/routes/strategy.rs`,
  function `validate_strategy_signal` (~line 1330-1340): rejects any
  `body.asset_class` other than `"equity"` before the signal reaches outbox
  enqueue. Regression-covered by `mqk-daemon/tests/scenario_asset_class_scope_b8.rs`.
- **Routing guard** (broker-submit asset-class check):
  `core-rs/crates/mqk-execution/src/gateway.rs`,
  `BrokerGateway::submit_with_context` (~line 369-377,
  `MULTI-ASSET-ROUTING-GUARD-01`): rejects any `req.asset_class !=
  AssetClass::Equity` before any broker adapter is invoked. Regression-covered
  by `mqk-execution/tests/scenario_asset_class_guard_multi_asset_routing_guard_01.rs`
  and `scenario_registry_v2_routing_guard_parity_01c.rs`.

Both gates are independent, both are equity-only, both are unchanged by this
bundle.

## 11. What asset classes do the current gates reject?

Any value other than `AssetClass::Equity` / the string `"equity"`: `Option`,
`Future`, `Crypto`, `Forex`. ETF is not a separate class — it is `Equity`
with `instrument_kind = "etf"` metadata (`mqk_md::instrument_registry`,
`ETF-REGISTRY-01`).

## 12. Is there a graduated per-asset risk policy today?

**Yes**, contrary to the stale `roadmap_completion_reconcile_01.md` line 31
claim. `core-rs/crates/mqk-execution/src/asset_risk_policy.rs` defines
`AssetRiskPolicy{asset_class, state, paper_trading_enabled,
live_trading_enabled, requires_margin_model, requires_contract_multiplier,
requires_session_profile, requires_currency_conversion, reason_code,
message}` with per-class static truth for equity (`Enabled`), ETF-as-equity
(`Enabled`), crypto/future/option/forex (`Disabled`), and
rates/fixed-income (`ResearchOnly`). `evaluate_asset_risk_for_order_intent_v2()`
maps `OrderIntentV2` validation + policy into an `AssetRiskRouteEvaluation`.
Static constants `ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED = false` and
`ASSET_RISK_NON_EQUITY_ROUTING_ENABLED = false` make explicit that this is
model-only. **What is genuinely missing** is an operator-facing surface for
this policy — it is not exposed via any daemon route or GUI panel today. The
existing `/api/v1/system/metadata` `asset_capability_matrix`
(`mqk-daemon/src/routes/system.rs::static_asset_capability_matrix`) is a
separate, simpler, hand-maintained scaffold (enabled/paper_ready/live_ready/
broker_adapter/notes only) that predates and does not read from
`asset_risk_policy.rs` — the two are not unified. This operator-surface gap
is the candidate safe closure for Phase C of this bundle (see §14 and the
Phase E closure decision doc for the actual outcome).

## 13. Does `ASSET-CORE-03` require `ASSET-CORE-04` for true live risk closure?

Yes for **graduated live enforcement** (actually blocking/sizing orders by
margin, multiplier, or NAV at runtime) — that requires live portfolio/margin/
NAV accounting, which is `ASSET-CORE-04`'s scope. `ASSET-CORE-04A/B/C/D/F` are
`CLOSED_LOCAL` foundation/bridge slices (per memory and ledger), but zero
production callers consume them yet (confirmed separately in
`ASSET-CORE-04F` closure). `ASSET-CORE-03`'s own scope — a fail-closed static
per-class policy boundary plus operator visibility — does **not** require
`ASSET-CORE-04`, and is closed for that scope in this bundle.

## 14. Safe gaps closed in this bundle

- **Phase B / `ASSET-CORE-02B`:** pure, unwired bracket/OCO model
  representation on `OrderIntentV2` (`BracketLegs`), validated and fail-closed
  (any bracket/OCO-bearing intent is reported non-routable regardless of
  asset class, since no execution path anywhere supports multi-leg
  submission). No broker/runtime/risk/OMS file touched.
- **Phase C / `ASSET-CORE-03B`:** read-only daemon route exposing
  `asset_risk_policy`'s per-class policy table, with explicit
  `production_enforcement_enabled: false` / `non_equity_routing_enabled:
  false` fields sourced directly from the existing static constants. No
  `mqk-risk`, broker, runtime, or OMS file touched; existing Gate 0/routing
  guard behavior is unchanged.

## 15. Gaps that require `ASSET-CORE-04`, live broker, or production cutover — remain open

- Any actual multi-leg/bracket **order submission** (requires broker adapter
  support, OMS lifecycle for child orders, and a scope-reviewed production
  wiring decision).
- Any graduated **live risk enforcement** that actually blocks/sizes orders
  by margin, contract multiplier, or NAV (requires `ASSET-CORE-04` live
  portfolio/margin accounting to be wired into `mqk-risk`'s enforcement path,
  which this bundle does not do).
- `InstrumentRegistryV2` becoming production trading truth
  (`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01`, unchanged, out of scope).
- Enabling any non-equity asset class for paper or live trading.

## 16. Explicit safety boundary

- No trading is enabled by this audit. No non-equity asset class is enabled
  for paper or live trading.
- No broker, provider, or network call was made to produce this audit.
- No risk, kill-switch, integrity, reconcile, broker, lease, arm, session,
  staleness, DB, or routing gate is weakened.
- No production `InstrumentRegistryV2` cutover occurs or is implied.
- Production consumption of `OrderIntentV2`/`ExecutionIntentV2`/
  `asset_risk_policy` remains explicitly separate and not started.
