# ASSET-CORE-01E — Registry V2 Rates/Fixed-Income Schema Gap Closure

Patch ID: `ASSET-CORE-01E-REGISTRY-V2-RATES-SCHEMA-GAP-01`

This is a schema/model-only patch. It is **not** provider execution, **not**
DB access, **not** a network call, **not** a production registry cutover,
**not** trading enablement of any kind, and **not** a change to
`mqk_schemas::AssetClass`, broker, risk, OMS, or runtime code.

---

## 1. Why this patch exists

A session was asked to "start `ASSET-CORE-01` — Unified Instrument Registry
v2" via a three-phase plan (schema audit doc → enum mapping → validator
CLI). Direct repo inspection at `HEAD` (`1d51a69a`) found all three
deliverables already built, committed, and marked `CLOSED_LOCAL` under the
same patch IDs the plan proposed to (re)create:

- **Enum mapping** (`ASSET-CORE-01A`, commit `7322b280`) —
  `mqk_md::provider::provider_asset_class_trading_class` /
  `provider_asset_class_instrument_kind`
  ([provider.rs](../../core-rs/crates/mqk-md/src/provider.rs)), exhaustive,
  no-wildcard, 9 tests (`pac_01`..`pac_09`).
- **Registry v2 schema + loader** (`ASSET-CORE-01B`) —
  `InstrumentRegistryV2`/`InstrumentDefinitionV2`/`ContractDefinitionV2`
  ([instrument_registry_v2.rs](../../core-rs/crates/mqk-md/src/instrument_registry_v2.rs)).
- **Validator CLI + status route** (`ASSET-CORE-01C`) — `mqk md
  registry-v2-status` (`mqk-cli`) and
  `GET /api/v1/system/instrument-registry-v2/status` (`mqk-daemon`).
- Further slices (`01D` GUI/status placement, `04A`-`04F` economics
  bridging, the full `CRYPTO-DATA-01A`..`03C` Kraken/CoinLore data lane)
  are also `CLOSED_LOCAL`/`PARTIAL` as of this `HEAD`.

Redoing any of that would either duplicate committed work or silently
contradict the ledger's own `CLOSED_LOCAL` status — against this repo's
audit-truth rule that the committed repo, not a prior prompt's framing, is
authoritative. This patch instead traced every "Recommended next slice"
thread in the ledger forward to `HEAD` and found each one now blocked by
this session's own hard constraints (live network calls, production
cutover, trading enablement — all explicitly out of scope here). Reading
the schema directly, rather than the ledger's narrative, surfaced one real
gap instead: **rates / fixed income was never modeled** anywhere in
`ContractDefinitionV2`, even though it is one of the seven asset categories
a "unified instrument registry" is expected to represent (equities, ETFs,
crypto spot, futures, options, forex, rates/fixed income).

---

## 2. Current true state of `ASSET-CORE-01` (consolidated)

| Sub-slice | Status | What it proves |
|---|---|---|
| `01A` enum mapping | `CLOSED_LOCAL` | `ProviderAssetClass` -> canonical trading-class string, exhaustive, ETF-aware |
| `01B` registry v2 schema/loader | `CLOSED_LOCAL` | `InstrumentRegistryV2` models equity/ETF/crypto/future/option/forex, fail-closed non-equity enablement |
| `01C` validator CLI + status route | `CLOSED_LOCAL` | read-only v1->v2 conversion/validation proof, no production consumer |
| `01D` GUI/status placement | `CLOSED_LOCAL` | operator-visible source config/validity, on Backtest Results screen (not a dedicated System screen) |
| `04A`-`04F` economics bridge | `CLOSED_LOCAL` | registry-v2 economics metadata provably reaches a daemon route end-to-end for a real crypto row |
| `CRYPTO-DATA-01A`..`03C` | `CLOSED_LOCAL` | local CSV marks, Kraken OHLCV adapter, scheduler readiness/task scripts, all data-lane, all manual/disabled |
| `01E` (this patch) | `CLOSED_LOCAL` | rates/fixed-income now representable in the v2 schema |

**Still genuinely open, but out of this session's scope:**

1. `InstrumentRegistryV2` is still never read by any trading/execution/
   risk/OMS/ingestion path — the v1 registry (`equities.json`) remains the
   sole source of trading truth. Closing this requires touching
   execution/runtime code, which every patch in this lineage (including
   this one) has been explicitly forbidden from doing.
2. No live/24-7 non-equity market-data provider has been network-verified
   beyond the read-only CoinLore/Kraken checks already done — closing this
   requires a live network call, out of scope here.
3. A production registry-v2 cutover decision remains explicitly unmade —
   by design; every prior patch in this lineage has deferred it.

None of these three are addressed by this patch; they are named here so a
future session does not need to re-derive them from a 4000+ line ledger.

---

## 3. The gap this patch closes

`ContractDefinitionV2` (`instrument_registry_v2.rs`) had a variant for
every asset class in `CANONICAL_ASSET_CLASSES_V2` except one:

| `asset_class` | Contract variant before this patch |
|---|---|
| `equity` | `Equity` |
| `equity` (+ `instrument_kind="etf"`) | `Etf` |
| `crypto` | `CryptoPair { base, quote }` |
| `future` | `Future { root, expiry, multiplier, tick_size_micros }` |
| `option` | `Option { underlying, expiry, strike_micros, right, multiplier }` |
| `forex` | `ForexPair { base, quote }` |
| *(rates/fixed income)* | **none — not representable at all** |

## 4. What was built

- `CANONICAL_ASSET_CLASSES_V2` gained `"rate"`.
- `ContractDefinitionV2::Rate { issuer, maturity, coupon_bps, face_value_micros }`
  — `coupon_bps` may be `0` (zero-coupon), `face_value_micros` must be
  positive, following the repo's existing micros convention
  (`strike_micros`, `tick_size_micros`).
- `validate_contract_v2` gained a `"rate"` match arm requiring
  `contract=Rate` with non-empty `issuer`/`maturity`, non-negative
  `coupon_bps`, and positive `face_value_micros` — mirroring the
  fail-closed shape validation every other derivative class already has.
- Tests: a `base_rate` fixture, a rate row added to the mixed-registry
  parse/validate test (`v2_01`), rate added to the "missing contract
  entirely" exhaustive test (`v2_07`), a new `v2_11b` field-violations test
  (empty issuer, empty maturity, negative coupon, non-positive face value)
  and `v2_11c` (zero-coupon validates cleanly), rate added to the disabled-
  backlog-fixtures test (`v2_17`) and to the economics-metadata absence/
  suggestion tests (`econ05`, `sug04`).
- `mqk-daemon`'s `contract_kind_label` (`routes/system.rs`, the pure,
  read-only label helper backing `ASSET-CORE-01C`'s `contract_kind_counts`)
  gained a `Rate { .. } => "rate"` arm — required for the crate to compile
  once the new enum variant existed; it is a display label only and does
  not gate, route, or affect any behavior.

**Deliberately not done:** no change to `mqk_schemas::AssetClass` or
`ContractSpec` (the live execution-path types); no change to
`mqk_md::provider::ProviderAssetClass` (no market-data provider serves
rates/bonds today, so no provider-capability tag was added); no
`Cargo.toml` edited anywhere; no config file added or changed; no DB
migration; no daemon/CLI behavior change beyond the one mechanical label
arm above; `"rate"` cannot become `enabled=true` in this schema without the
same `allow_enabled_non_equity_for_testing` test-only escape hatch every
other non-equity class already requires.

---

## 5. Validation results

`CARGO_TARGET_DIR=C:\tmp\mqk-target-asset-core-01-rate-schema`

- `cargo check -p mqk-md -p mqk-schemas` — clean.
- `cargo test -p mqk-md instrument_registry_v2` — 50/50 pass (49 unit +
  1 across scenario test files unaffected).
- `cargo test -p mqk-md` (full crate, all binaries) — 353 total pass
  (307 lib unit tests, including 2 new rate-specific tests plus the
  existing derivative-class tests extended to cover `rate`; 7 + 31 + 7
  across the crate's three scenario test files; 1 doc-test), zero
  failures, zero regressions.
- `cargo clippy -p mqk-md -p mqk-schemas --all-targets -- -D warnings` —
  clean.
- `cargo check -p mqk-daemon -p mqk-cli` (downstream dependents) — clean
  after adding the one required `contract_kind_label` match arm.
- `cargo test -p mqk-daemon --test scenario_instrument_registry_v2_status_asset_core_01c`
  — 13/13 pass (regression, unaffected by the new variant).
- `cargo clippy -p mqk-daemon --lib -- -D warnings` — clean.

---

## 6. Safety confirmation

- Zero network calls.
- Zero DB access or mutation.
- Zero config file changes.
- No non-equity asset class enabled; `"rate"` is fail-closed identically
  to `future`/`option`/`crypto`/`forex`.
- No broker/execution/risk/OMS/runtime code touched.
- No production registry cutover.

## 7. Recommended next slice

None required to keep `ASSET-CORE-01` schema-complete against its own
stated seven-category requirement — that requirement is now fully met.
The three items in §2 ("still genuinely open") are the honest ceiling:
each needs either a live network call, a production-cutover decision, or
an execution-path change, all explicitly out of scope for this lineage
until a separate operator decision authorizes crossing one of those
boundaries.
