# ASSET-CORE-01F — Instrument Registry V2 Completion Audit

Patch ID: `ASSET-CORE-01F-INSTRUMENT-REGISTRY-V2-COMPLETION-AUDIT-01`

Audit-only. No production code, config, DB, or trading-path change. No
network call, no provider/broker call, no DB connection, no daemon start.

---

## 1. HEAD and relevant `ASSET-CORE-01` commits

Audited at `HEAD = e2680ac1` ("md: close registry v2 rates/fixed-income
schema gap"), branch `main`, tracked working tree clean at audit start.

| Sub-slice | Commit | Message |
|---|---|---|
| `ASSET-CORE-01A` | `7322b280` | data: reconcile asset class metadata seams |
| `ASSET-CORE-01B` | `696b56fe` | data: add instrument registry v2 schema |
| `ASSET-CORE-01C` | `83f8a89e` | data: expose registry v2 status |
| `ASSET-CORE-01D` (combined, GUI placement) | `e951b0ef` | daemon: expose registry v2 source status and surface it in the GUI |
| `ASSET-CORE-01E` | `e2680ac1` | md: close registry v2 rates/fixed-income schema gap |
| `ASSET-CORE-04A`-`04F` (economics bridge) | `1c80ee70`..`6535db8e` | portfolio/daemon economics bridge lineage (see ledger §19) |

---

## 2. Current sub-slice table

| Sub-slice | Status | What it proves |
|---|---|---|
| `01A` enum mapping | `CLOSED_LOCAL` | `mqk_md::provider::provider_asset_class_trading_class` / `provider_asset_class_instrument_kind` ([provider.rs](../../core-rs/crates/mqk-md/src/provider.rs)) give an exhaustive, no-wildcard, tested (`pac_01`..`pac_09`) mapping from `ProviderAssetClass` to canonical trading-class/instrument-kind strings. Does not touch `mqk_schemas::AssetClass` or add a `Cargo.toml` dependency edge (deliberate — see ledger §19 `ASSET-CORE-01A` finding 4). |
| `01B` schema/loader | `CLOSED_LOCAL` | `InstrumentRegistryV2`/`InstrumentDefinitionV2`/`ContractDefinitionV2` + `load_instrument_registry_v2`/`convert_v1_registry_to_v2`/`validate_registry_v2` ([instrument_registry_v2.rs](../../core-rs/crates/mqk-md/src/instrument_registry_v2.rs)). Additive sibling of the v1 `TrackedInstrument` registry, not a replacement. |
| `01C` validator CLI/status route | `CLOSED_LOCAL` | `mqk md registry-v2-status` (`mqk-cli/src/commands/md.rs::md_registry_v2_status`) and `GET /api/v1/system/instrument-registry-v2/status` (`mqk-daemon/src/routes/system.rs::system_instrument_registry_v2_status`) both load the v1 registry, convert to v2 in memory, validate, and report — read-only, no production consumer. |
| `01D` GUI/operator status | `CLOSED_LOCAL` | `GET /api/v1/system/instrument-registry-v2-source/status` (separate v2-source path, `MQK_INSTRUMENT_REGISTRY_V2_PATH`) plus the static asset-capability matrix are both rendered on the operator System/Status GUI surface (moved off Backtest Results by the `01D`-combined follow-up, ledger §34). |
| `01E` rates/fixed-income schema | `CLOSED_LOCAL` | `CANONICAL_ASSET_CLASSES_V2` gained `"rate"`; `ContractDefinitionV2::Rate{issuer, maturity, coupon_bps, face_value_micros}` added with fail-closed validation, closing the last missing category from the seven-category requirement. |

---

## 3. Exact current schema coverage

`CANONICAL_ASSET_CLASSES_V2` (`instrument_registry_v2.rs:60-61`):
`["equity", "option", "future", "crypto", "forex", "rate"]`, plus ETF
represented via `instrument_kind = Some("etf")` on an `equity`-class row.

| Category | `ContractDefinitionV2` variant | Present |
|---|---|---|
| Equity | `Equity` | Yes |
| ETF | `Etf` (equity + `instrument_kind="etf"`) | Yes |
| Crypto spot | `CryptoPair { base, quote }` | Yes |
| Future | `Future { root, expiry, multiplier, tick_size_micros }` | Yes |
| Option | `Option { underlying, expiry, strike_micros, right, multiplier }` | Yes |
| Forex | `ForexPair { base, quote }` | Yes |
| Rate / fixed income | `Rate { issuer, maturity, coupon_bps, face_value_micros }` | Yes (added by `01E`) |

All seven categories a "unified instrument registry" is expected to
represent are now modeled. `validate_contract_v2` has a fail-closed match
arm for every one of the six non-implied-equity variants (`instrument_registry_v2.rs:354`+),
and `enabled=true` on any non-equity row is rejected unless
`allow_enabled_non_equity_for_testing=true` (test/fixture escape hatch
only — `instrument_registry_v2.rs:115-124, 330-333`).

---

## 4. Exact current validator/operator surfaces

- **CLI:** `mqk md registry-v2-status` (`mqk-cli/src/commands/md.rs::md_registry_v2_status`).
- **Daemon routes:**
  - `GET /api/v1/system/instrument-registry-v2/status` — v1→v2 conversion/validation diagnostic (`ASSET-CORE-01C`).
  - `GET /api/v1/system/instrument-registry-v2-source/status` — separate v2-source (`MQK_INSTRUMENT_REGISTRY_V2_PATH`) configuration/validity status (`ASSET-CORE-01D`).
  - `GET /api/v1/system/instrument-economics/status` — registry-v2 economics bridge status (`ASSET-CORE-04D`).
- **GUI:** registry-v2 source status and the static asset-capability matrix are both rendered on the operator System/Status screen (moved there from Backtest Results by the `01D`-combined follow-up patch).

---

## 5. Production-consumption truth

`InstrumentRegistryV2` is **not** consumed by any trading, execution, risk,
OMS, or ingestion path. Every route/CLI surface above loads the v1 registry
(`config/instruments/equities.json`), converts to v2 **in memory**, and
reports the result — nothing writes v2 back into a production file, and no
runtime/execution/risk/broker code path reads `InstrumentRegistryV2` at
all. Confirmed by direct grep of `core-rs/crates/mqk-runtime`,
`mqk-execution`, `mqk-risk`, `mqk-broker-alpaca` for `instrument_registry_v2`
— zero matches outside `mqk-md`/`mqk-daemon`/`mqk-cli` read-only surfaces.

`config/instruments/equities.json` (v1) remains the sole production
trading-instrument source. No production registry-v2 file exists; the only
registry-v2 JSON on disk are disabled, non-equity example/fixture files
(`config/instruments/instruments_v2.*.example.json`).

---

## 6. Remaining gaps, classified

| Gap | Classification |
|---|---|
| No trading/execution/risk/OMS/ingestion path reads `InstrumentRegistryV2` | `production_consumption_gap` |
| No live non-equity market-data provider network-verified beyond existing read-only CoinLore/Kraken checks | `production_consumption_gap` (requires a live network call, out of this session's scope) |
| No production registry-v2 cutover decision made | `production_consumption_gap` |
| Futures/options/forex/rates have zero real instrument data, risk model, broker adapter, or strategy | `future_asset_class_gap` — belongs to `FUTURES-*`/`OPTIONS-*`/`FX-*`/`RATES-*` roadmap phases, not `ASSET-CORE-01`'s own scope |
| Crypto lane (`CRYPTO-DATA-01*`/`CRYPTO-REGISTRY-01`) partial completion (data ingestion, no risk/exec/strategy) | `not_ASSET_CORE_01` — tracked under its own `CRYPTO-*` patch IDs |

No `foundation_gap` (schema/model/docs/validator gap) remains: all seven
asset categories are modeled, validated, tested, and operator-visible.

---

## 7. Closure decision

**`CLOSED_LOCAL / FOUNDATION-COMPLETE`.**

`ASSET-CORE-01`'s stated scope — a unified instrument registry v2
foundation (schema, loader, provider-asset-class mapping, validator,
operator status surface, docs, test coverage) covering equities, ETFs,
crypto spot, futures, options, forex, and rates/fixed income — is fully
met as of `01A`-`01E`. Production consumption (trading/execution/risk/OMS/
ingestion paths reading `InstrumentRegistryV2` as truth) was never part of
this patch's own scope in any prior slice's stated mission, and remains a
distinct, explicitly-deferred boundary (see
[`asset_core_01h_instrument_registry_v2_consumption_boundary_decision.md`](asset_core_01h_instrument_registry_v2_consumption_boundary_decision.md)).

- No config flags were changed by this audit.
- No trading was enabled by this audit.

---

## 8. Recommended next patch

`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` — a decision-only patch (no
code) that names what must be true before any production path is switched
to read `InstrumentRegistryV2`. See
[`asset_core_01h_instrument_registry_v2_consumption_boundary_decision.md`](asset_core_01h_instrument_registry_v2_consumption_boundary_decision.md)
for the detailed boundary definition.

Alternative, independent next patches (do not require crossing the
registry-v2 production-consumption boundary): `ASSET-CORE-05`
(market-calendar/session generalization — already `PARTIAL`, closest to
done) or `BACKTEST-MULTIPLIER-MARGIN-01` (backtest P&L multiplier support,
a hard prerequisite for any futures/options backtest result to be
trustworthy).
