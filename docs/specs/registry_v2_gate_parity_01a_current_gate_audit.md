# REGISTRY-V2-GATE-PARITY-01A — Current Gate Audit

Patch ID: `REGISTRY-V2-GATE-PARITY-01A-CURRENT-GATE-AUDIT-01`

Docs-only. Audits the current Gate 0 and broker-submit routing-guard
behavior against direct repo evidence, and defines the exact
`InstrumentRegistryV2::asset_class` parity contract that
`REGISTRY-V2-GATE-PARITY-01B`/`01C` will prove. No code changed by this
document.

---

## 1. HEAD and prerequisite commits

- HEAD at audit time: `f9032c2a` ("docs: close registry v2 translation
  layer").
- `REGISTRY-V2-TRANSLATION-01A`-`01D` (prerequisite #2, satisfied):
  `e77db7ce` (audit) → `6fefc3d1` (`RegistryV2SymbolTranslationIndex`) →
  `66b617f9` (CLI translation-check) → `f9032c2a` (closure).
- `BACKTEST-MULTIPLIER-MARGIN-01` (prerequisite #1, satisfied):
  `e3f2f77e` (audit) → `b870c823` (economics) → `f0ce4fff` (closure).
- `ASSET-CORE-01H` boundary decision: `6e6f69df`. `ASSET-CORE-01F`
  completion audit: `91cc08e1`.

## 2. Gate 0 current behavior

- **Where it lives:** `core-rs/crates/mqk-daemon/src/routes/strategy.rs`,
  function `validate_strategy_signal`, the "B8: Asset-class scope gate"
  block (~line 1326-1343).
- **What it accepts:** `StrategySignalRequest.asset_class` absent (equity
  implied, backward-compatible default), or present and, after
  `trim().to_ascii_lowercase()`, exactly equal to `"equity"`.
- **What it rejects:** any other value, case-insensitively — known
  non-equity classes (`"option"`, `"future"`, `"crypto"`, `"fx"`) and
  unknown/misspelled values (e.g. `"perpetual_swap"`) alike, appended as a
  blocker string and surfaced as HTTP 400 / `disposition: "rejected"`. This
  is an **allowlist of one value**, not a denylist of known non-equity
  types — `AS-07` in the test file below proves this explicitly.
- **Current tests proving it:**
  `core-rs/crates/mqk-daemon/tests/scenario_asset_class_scope_b8.rs` —
  `AS-01`..`AS-12`. Covers: absent passes, explicit `"equity"` passes,
  `"option"`/`"future"`/`"crypto"`/`"fx"` rejected, unknown value rejected,
  case-insensitivity (`"EQUITY"` passes, `"Option"` rejected), and the
  companion `GET /api/v1/system/status` `asset_class_scope: "equity_only"`
  operator-truth field (`AS-09`..`AS-11`).
- **Does Gate 0 read `InstrumentRegistryV2` today?** No. It reads only the
  request-body `asset_class` string field and compares it to the literal
  `"equity"`. It has no dependency on `mqk-md` or the v2 schema at all.

## 3. Broker-submit routing guard current behavior

- **Where it lives:**
  `core-rs/crates/mqk-execution/src/gateway.rs`,
  `BrokerGateway::enforce_gates` (called from `submit_with_context`),
  ~line 377-381: `if req.asset_class != AssetClass::Equity { return
  Err(SubmitError::Gate(GateRefusal::AssetClassDisabled { asset_class:
  req.asset_class })); }`. Tagged `MULTI-ASSET-ROUTING-GUARD-01`.
- **What it accepts:** `mqk_schemas::AssetClass::Equity` only.
- **What it rejects:** `AssetClass::Option`, `AssetClass::Future`,
  `AssetClass::Crypto`, `AssetClass::Forex` — rejected with
  `GateRefusal::AssetClassDisabled { asset_class }` **before** any
  `BrokerAdapter` method is invoked (proven by a panicking test double).
- **Current tests proving it:**
  `core-rs/crates/mqk-execution/tests/scenario_asset_class_guard_multi_asset_routing_guard_01.rs`
  — `G01`..`G08`. Covers: equity passes end-to-end through `EchoBroker`;
  crypto/future/option/forex each individually rejected with the exact
  `AssetClassDisabled` variant carrying the matching asset class; rejection
  is a typed `Err`, never a panic; a `PanicBroker` test double proves the
  broker adapter is never invoked for any of the four disabled classes.
- **Does the routing guard read `InstrumentRegistryV2` today?** No. It
  reads only `BrokerSubmitRequest.asset_class: mqk_schemas::AssetClass`,
  the same enum Gate 0's request type is adjacent to (`mqk-execution`
  depends on `mqk-schemas`; it has no dependency on `mqk-md`/`mqk-schemas`'s
  sibling `InstrumentRegistryV2` type).

## 4. Current asset-class vocabularies

| Type | Location | Values |
|---|---|---|
| `mqk_schemas::AssetClass` | `core-rs/crates/mqk-schemas/src/lib.rs` (~line 104-111) | Rust enum: `Equity`, `Option`, `Future`, `Crypto`, `Forex`. No `Rate` variant. ETF is deliberately not a variant — it is `Equity` + `instrument_kind = Some("etf")` metadata on the v1 `TrackedInstrument`/`Instrument` types. |
| `InstrumentRegistryV2.asset_class` | `core-rs/crates/mqk-md/src/instrument_registry_v2.rs`, `CANONICAL_ASSET_CLASSES_V2` (~line 60-61) | Plain lower-case singular `&str` constants: `"equity"`, `"option"`, `"future"`, `"crypto"`, `"forex"`, `"rate"`. One addition versus the schema enum: `"rate"` (fixed income), which `mqk_schemas::AssetClass` has no counterpart for. ETF is likewise not a separate `asset_class` value here either — same `instrument_kind = Some("etf")` convention, enforced by `validate_registry_v2`'s ETF sector/category check. |
| `ProviderAssetClass` (`mqk-md::provider`) | Separate concern — declares what a *market-data provider* can serve, not an execution-path gate. `provider_asset_class_trading_class` already maps it 1:1 onto the same singular vocabulary `mqk_schemas::AssetClass` uses. Not part of this patch's parity contract (no gate reads it). | |

`mqk_schemas::AssetClass` (5 variants) is therefore a strict subset of
`CANONICAL_ASSET_CLASSES_V2` (6 values) — every schema-enum variant has a
same-named v2 string counterpart; v2 additionally has `"rate"`, which has
no schema-enum counterpart and is today's only vocabulary gap between the
two types.

## 5. Alias behavior

- **`future` vs `futures`, `option` vs `options`:** `CANONICAL_ASSET_CLASSES_V2`
  (the authoritative v2 vocabulary enforced by `validate_registry_v2`) does
  **not** accept plural forms — only the exact strings `"future"`/`"option"`
  are valid; `"futures"`/`"options"` fail `validate_registry_v2` with
  `unknown asset_class`. A **separate, unrelated** type,
  `mqk-execution::asset_risk_policy::asset_risk_policy_for_asset_class`
  (an `ASSET-CORE-03` model-only foundation, not Gate 0 or the routing
  guard), does accept `"future"|"futures"` and `"option"|"options"` as
  aliases for its own static policy lookup — but that function is not one
  of the two gates this patch proves parity for, and does not gate, block,
  or route any order today (`ASSET_RISK_PRODUCTION_ENFORCEMENT_ENABLED =
  false`). Because the parity contract in this patch is specifically
  against `InstrumentRegistryV2::asset_class` (which a real registry
  document can only ever populate with a `CANONICAL_ASSET_CLASSES_V2`
  member, enforced fail-closed by `validate_registry_v2`), `01B`'s pure
  helper matches `CANONICAL_ASSET_CLASSES_V2`'s existing strictness and
  does **not** accept the plural aliases. A v2-registry-shaped value of
  `"futures"` or `"options"` is therefore classified `UnknownAssetClass`
  (fail-closed — never silently treated as its singular counterpart, and
  never defaulted to equity).
- **Case/whitespace:** Gate 0 already normalizes via
  `trim().to_ascii_lowercase()`; the routing guard has no string
  normalization (it compares a typed enum, not a string). `01B`'s helper
  will apply the same `trim().to_ascii_lowercase()` normalization Gate 0
  uses, so `" EQUITY "` and `"equity"` classify identically.

## 6. The parity contract

For every string `s`:

1. If `s` normalizes (trim + lowercase) to exactly `"equity"`, the v2
   helper's decision must agree with both existing gates' `Equity`-allowed
   outcome.
2. If `s` normalizes to any other member of `CANONICAL_ASSET_CLASSES_V2`
   (`"option"`, `"future"`, `"crypto"`, `"forex"`, `"rate"`), the v2
   helper's decision must agree with both existing gates' reject outcome —
   for the four values that have a `mqk_schemas::AssetClass` counterpart
   (`option`/`future`/`crypto`/`forex`), this is checked directly against
   the routing guard's `GateRefusal::AssetClassDisabled` and Gate 0's
   `"not supported"` blocker; for `"rate"` (no schema-enum counterpart),
   parity is checked against both gates' allowlist-of-one-value structure
   (anything not `"equity"` is rejected), which by construction already
   covers a class the schema enum cannot even represent.
3. If `s` is empty, unknown (e.g. `"stock"`, `"perpetual_swap"`), or `"etf"`
   (ETF is `instrument_kind`, not `asset_class` — see §4), the v2 helper
   fails closed (`Err`, never a silent reject-as-if-equity or
   accept-as-if-equity), matching Gate 0's `AS-07`-proved allowlist
   behavior and the routing guard's total-match-on-a-closed-enum behavior.

## 7. What this patch will prove

- A pure, fail-closed helper (`01B`) that classifies any
  `InstrumentRegistryV2.asset_class` string into the same
  allow-equity/reject-non-equity/fail-closed-unknown decision the two
  existing production gates already enforce.
- Regression tests (`01C`) proving that decision table matches Gate 0's
  and the routing guard's actual behavior for every
  `CANONICAL_ASSET_CLASSES_V2` value plus malformed/unknown input.
- That no previously-disabled asset class becomes reachable through this
  new helper or its tests.

## 8. What this patch will not do

- No production cutover — `InstrumentRegistryV2` is still read only by
  read-only diagnostic/status surfaces and this patch's own pure
  tests/helper.
- No runtime v2 consumption — Gate 0 and the routing guard are not
  modified; they continue to read `StrategySignalRequest.asset_class`
  (string) and `mqk_schemas::AssetClass` (enum) exactly as today.
- No DB migration, no DB access, no network call.
- No non-equity trading enablement of any kind (paper or live).
- No change to broker, risk, runtime, strategy, or portfolio behavior.
