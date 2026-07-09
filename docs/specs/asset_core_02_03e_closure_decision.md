# ASSET-CORE-02-03E — Closure Decision (`ASSET-CORE-02` / `ASSET-CORE-03`)

**Patch group:** `ASSET-CORE-02-03-COMPLETION-SWEEP-01-COMBINED`
**Phase:** E / Commit 5 — `ASSET-CORE-02-03E-CLOSURE-AND-ROADMAP-RECONCILE-01`

## 1. Is `ASSET-CORE-02` closed for its own safe model scope?

**Yes.** `CLOSED_LOCAL / MODEL-COMPLETE`. Its own scope — a hardened,
validated, multi-asset order-intent model — is done:
`OrderIntentV2`/`ExecutionIntentV2` (foundation patch,
`ASSET-CORE-02-ORDER-INTENT-V2-FOUNDATION-01-COMBINED`) plus the one
remaining concrete gap this bundle identified and closed — bracket/OCO
representation (`ASSET-CORE-02B-ORDER-INTENT-V2-SAFE-GAP-CLOSURE-01`,
`BracketLegs`).

## 2. Is `ASSET-CORE-02` production-consumed?

**No.** Zero production callers of `OrderIntentV2`, `ExecutionIntentV2`,
`BracketLegs`, or `mqk_execution::asset_risk_policy` exist anywhere outside
`mqk-execution`'s own `src`/`tests` (confirmed by grep across `mqk-cli`,
`mqk-daemon`, `mqk-broker-*`, `mqk-runtime`). The live/paper dispatch path
still uses the separate V1 types through `BrokerSubmitRequest` /
`BrokerGateway::submit_with_context`.

## 3. Is `ASSET-CORE-03` closed for its own safe policy-boundary scope?

**Yes.** `CLOSED_LOCAL / POLICY-BOUNDARY-COMPLETE`. Its own scope — a
fail-closed, graduated per-asset-class policy model, plus operator
visibility into it — is done: `mqk_execution::asset_risk_policy` (foundation
patch, `ASSET-CORE-03-RISK-ROUTER-FOUNDATION-01-COMBINED`) plus the
operator-surface gap this bundle identified and closed
(`ASSET-CORE-03B-ASSET-RISK-POLICY-SAFE-GAP-CLOSURE-01`,
`GET /api/v1/system/asset-risk-policy`).

## 4. Is `ASSET-CORE-03` enforcing graduated live per-asset risk?

**No.** The policy model is descriptive/static only. The only two things
that actually enforce anything today are Gate 0 (signal admission,
equity-only string check) and the broker-submit routing guard
(equity-only `AssetClass` check) — both unchanged, both regression-proven
throughout this bundle. Nothing in `asset_risk_policy` or its new daemon
route blocks, sizes, or modifies any order.

## 5. What depends on `ASSET-CORE-04`?

True graduated live enforcement — actually blocking or sizing an order by
margin requirement, contract multiplier, or portfolio NAV at request time —
requires live portfolio/margin/NAV accounting wired into an enforcement
path (`mqk-risk` or the gateway). `ASSET-CORE-04A`-`04F` are `CLOSED_LOCAL`
foundation/bridge slices per the ledger, but have zero production callers
today; wiring them into `mqk-risk`'s actual enforcement path is a separate,
larger, boundary-crossing patch, not attempted here.

## 6. Did any live/paper trading behavior change?

**No.** Confirmed by regression: `scenario_asset_class_scope_b8` (Gate 0,
12/12), `scenario_asset_class_guard_multi_asset_routing_guard_01` (routing
guard, 8/8), `scenario_gui_daemon_contract_gate` (23/23),
`scenario_route_contract_rt01` (2/2) — all pass unchanged after every phase
of this bundle.

## 7. Were any non-equity classes enabled?

**No.** `asset_risk_policy`'s static truth is unchanged: equity and
ETF-as-equity remain `Enabled` (paper only, never live); crypto/future/
option/forex remain `Disabled`; rates/fixed-income remains `ResearchOnly`.
The new daemon route reports this truth verbatim; it does not change it.

## 8. Were Gate 0 and routing guard modified?

**No.** Neither `mqk-daemon/src/routes/strategy.rs`'s `validate_strategy_signal`
nor `mqk-execution/src/gateway.rs`'s `BrokerGateway::submit_with_context`
were touched by any phase of this bundle.

## 9. What tests prove equity behavior is unchanged?

- `scenario_order_intent_v2_foundation_01.rs` (11/11) — unchanged, still
  passing after Phase B's additive `bracket` field.
- `scenario_asset_core_02_order_intent_v2.rs` (new, 10/10) — proves equity
  intents without bracket legs are unaffected, and bracket legs never make
  any intent (equity or otherwise) routable.
- `scenario_asset_class_guard_multi_asset_routing_guard_01.rs` (8/8) —
  routing guard equity pass-through unchanged.
- `scenario_asset_class_scope_b8.rs` (12/12) — Gate 0 equity pass-through
  unchanged.
- `scenario_asset_core_03_asset_risk_policy_status.rs` (new, 7/7) — proves
  the new route reports equity as `enabled`/paper-only/never-live, and every
  non-equity class as `disabled`/`research_only`, never enabled or live.

## 10. What next partial-roadmap bundle is recommended?

`ASSET-CORE-04` is now the harder, higher-blast-radius blocker for any
further *enforcement* progress on `ASSET-CORE-03` (wiring live
portfolio/margin/NAV into an actual risk-enforcement path is a
production-behavior-changing decision, not a safe-closure sweep).
Recommended next: `ASSET-CORE-04-LIVE-LEDGER-BOUNDARY-AUDIT-AND-SAFE-GAP-CLOSURE-01-COMBINED`.

## Final verdict

```text
ASSET-CORE-02: CLOSED_LOCAL / MODEL-COMPLETE
ASSET-CORE-03: CLOSED_LOCAL / POLICY-BOUNDARY-COMPLETE
```

Both verdicts are scoped to each item's own stated model/policy-boundary
work. Production consumption of either remains explicitly separate, not
started, and not implied. No non-equity asset class is enabled for paper or
live trading. No trading behavior changed.
