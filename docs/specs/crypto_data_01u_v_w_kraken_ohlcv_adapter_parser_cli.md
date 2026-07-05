# CRYPTO-DATA-01U-V-W — Kraken OHLCV Adapter, Parser, CLI Evidence

Patch ID: `CRYPTO-DATA-01U-V-W-KRAKEN-OHLCV-ADAPTER-PARSER-CLI-BUNDLE-01-COMBINED`

This bundle builds the first disabled-by-default, fixture-first Kraken OHLCV
provider adapter lane for `BTC/USD` and `ETH/USD`, continuing the crypto
data lane after
`CRYPTO-DATA-01S-T-OHLCV-PROVIDER-DECISION-VERIFY-BUNDLE-01-COMBINED`
(Kraken selection + bounded live verification). It closes three small
adjacent slices together because they share the same provider lane, safety
profile, files/modules, and validation matrix:

- `CRYPTO-DATA-01U-KRAKEN-OHLCV-PROVIDER-ADAPTER-LOCAL-INGEST-01`
- `CRYPTO-DATA-01V-KRAKEN-OHLCV-VOLUME-SEMANTICS-01`
- `CRYPTO-DATA-01W-KRAKEN-OHLCV-CLI-EVIDENCE-01`

This is **not** recurring ingestion, **not** scheduling, **not** GUI,
**not** risk/execution/strategy, and **not** crypto trading enablement.

---

## 1. What this bundle builds

1. A Kraken OHLC parser/model (`core-rs/crates/mqk-md/src/providers/kraken.rs`)
   that parses the verified `/0/public/OHLC` response shape from committed
   fixtures first, rejects malformed/unsafe data, and derives completion
   from `row.time <= result.last` — never from a fabricated flag.
2. An explicit, tested, documented volume-scaling convention
   (`CRYPTO-DATA-01V`) so Kraken's fractional base-asset volume can be
   represented in `ProviderBar.volume: i64` without silent truncation or
   fabrication.
3. A `KrakenHistoricalProvider` implementing `mqk_md::HistoricalProvider`,
   wrapped by the existing `HistoricalProviderMarketDataAdapter`, and a
   fourth `provider_registry.rs` factory match arm (`"kraken"`) — gated
   disabled by default via `config/providers/providers.json`.
4. Disabled registry-v2 aliases (`kraken_pair` / `kraken_result_key`) on the
   existing `BTC/USD` / `ETH/USD` fixture rows.
5. A read-only `mqk md kraken-ohlc-dry-run` CLI evidence command
   (`CRYPTO-DATA-01W`), fixture-first by default, with a
   `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`-gated network opt-in mirroring
   `mqk md coinlore-latest-mark`'s design.

---

## 2. Volume semantics decision (CRYPTO-DATA-01V)

**Decision: Option A — scaled base-asset volume integer.**

Kraken's `volume` field is a decimal string in base-asset units (e.g. BTC,
ETH) — not whole coins and not quote-currency (USD) volume.
`RawBar.volume`/`ProviderBar.volume` is typed `i64`. This bundle scales the
decimal string by `KRAKEN_VOLUME_SCALE = 100_000_000` (1e8, "atomic-volume
units") before conversion:

| Raw Kraken volume | Scaled `i64` |
|---|---|
| `"1317.15941434"` (BTC, live-verified in `01S-T`) | `131715941434` |
| `"15588.01759742"` (ETH, live-verified in `01S-T`) | `1558801759742` |

This is documented in code (`kraken.rs` module doc comment,
`KRAKEN_VOLUME_SCALE`/`KRAKEN_VOLUME_SCALE_DECIMALS` constants) and in the
CLI evidence contract's `volume_semantics`/`volume_scale` fields as:
`"kraken_base_asset_volume_scaled_by_1e8_not_whole_coins_not_usd"`. It is
**not** whole-coin volume and **not** quote-currency (USD) volume, and no
strategy/risk/execution code reads this value in this patch.

Rejection behavior (`kraken_volume_to_scaled_i64`, `KV-01`..`KV-08` in
`kraken.rs`):

- More than 8 fractional digits -> `VolumeTooManyDecimals` (rejected, never
  truncated).
- Negative or non-decimal input -> `VolumeMalformed`.
- Integer overflow during `checked_mul`/`checked_add` -> `VolumeOverflow`
  (never wraps).

This convention is exact for both live-verified BTC and ETH samples (both
happened to carry exactly 8 fractional digits) and is tested against those
exact values, not approximated.

---

## 3. Completion semantics (unchanged from `01S-T`, now implemented)

- `is_complete = row.time <= result.last` — computed structurally from
  Kraken's own `result.last` cursor, never from a provider-supplied flag
  (Kraken sends none) and never inferred from wall-clock time.
- `end_ts = row.time + interval_seconds` — Kraken's `time` is the bar
  **start**, confirmed live by `01S-T`.
- Only `interval_seconds = 86_400` (`1D`) is supported; any other value is
  rejected (`UnsupportedInterval`), matching this bundle's scope.
- The forming/incomplete row is excluded from every
  `completed_provider_bars()` call and cannot be converted to a
  `ProviderBar` directly (`kraken_bar_to_provider_bar` refuses with
  `IncompleteBarConversionRefused` as a defense-in-depth guard).

---

## 4. Provider config / factory (disabled by default)

`config/providers/providers.json` gained a `kraken` entry:

- `enabled: false`
- `asset_classes: ["crypto"]`
- `api_key_required: false`, `credential_env_vars: []`
- `implementation_status: "ohlcv_adapter_fixture_proven_network_opt_in_only"`
- `verification_status: "fixture_parser_proven_live_network_not_exercised_by_this_patch"`

`provider_registry.rs::build_market_data_provider_from_config` gained a
fourth match arm for `"kraken"`, reached only when `enabled: true` (the
committed registry keeps it `false`, so the real registry never reaches
this arm — proven by `provider_registry::tests::kraken_factory_01_...` and
`scenario_kraken_ohlcv_provider_01uvw.rs::sc05_...`).

**Bug found and fixed in scope:** `HistoricalProviderMarketDataAdapter::new`
(`provider.rs`) unconditionally seeds `supported_asset_classes` with
`ProviderAssetClass::Equity` via
`MarketDataProviderCapabilities::historical_only` — harmless for the
existing Alpaca/TwelveData equity-only wrapped providers, but would have
made a Kraken-wrapped adapter falsely advertise `Equity` instead of
`Crypto`. Added `HistoricalProviderMarketDataAdapter::with_capabilities`
(mirroring `FakeMarketDataProvider`'s existing pattern) and used it in the
`"kraken"` factory arm to apply the config-derived capabilities (which
correctly include `ProviderAssetClass::Crypto`). This does not change
Alpaca/TwelveData behavior — neither arm was touched.

---

## 5. Registry-v2 aliases

Both `BTC/USD` and `ETH/USD` rows in
`config/instruments/instruments_v2.crypto_local_marks.example.json` gained:

```json
"kraken_pair": "XBTUSD",       // or "ETHUSD" for ETH/USD
"kraken_result_key": "XXBTZUSD" // or "XETHZUSD" for ETH/USD
```

Both rows remain `enabled: false`, `paper_trading_enabled: false`,
`live_trading_enabled: false` — unchanged. `kraken_aliases_from_registry_v2`
(mirroring `coinlore_aliases_from_registry_v2`) is a pure, read-only
projection; it does not validate or gate enablement.

---

## 6. CLI evidence (`CRYPTO-DATA-01W`)

`mqk md kraken-ohlc-dry-run --registry <path> --symbol <BTC/USD|ETH/USD>
--timeframe 1D [--input-file <path>] [--output-dir <dir>]`:

- Default (`--input-file`): zero network calls, zero DB connection, zero
  `md_bars` write.
- Network opt-in: only when no `--input-file` is given **and**
  `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` is set — performs at most one HTTP GET
  via `mqk_md::fetch_kraken_ohlc_body`.
- Only `--timeframe 1D` is supported; any other value is refused before any
  file/network access.
- Evidence JSON (`--output-dir`) carries: `schema_version`, `producer`,
  `produced_at_utc`, `provider`, `mode`, `network_call_made`, `db_write`
  (always `false`), `md_bars_write` (always `false`),
  `completed_bar_claim`, `forming_candle_excluded`, `volume_semantics`,
  `volume_scale`, `symbols_requested`, `bars_completed`,
  `bars_excluded_forming`, `latest_completed_start_ts`,
  `latest_completed_end_ts`, `all_passed`, `reason_code`, `fail_reasons`.

---

## 7. Fixtures

`core-rs/crates/mqk-md/tests/fixtures/kraken_ohlc_xbtusd_1d.json` and
`kraken_ohlc_ethusd_1d.json` each carry 3 rows: 1 synthetic filler committed
row (for multi-row proof, not independently live-verified) plus the exact
two rows (`1783123200` committed at `result.last`, `1783209600` forming)
recorded live by `CRYPTO-DATA-01S-T`'s `docs/specs/crypto_data_01s_t_ohlcv_provider_decision_verify.md`
§7. No fixture claims to be a current/live market price.

---

## 8. DB-backed ingest proof — not run in this bundle

Per this bundle's own closure standard, a DB-backed ingest test is
**optional** and only warranted once the volume representation is proven
safe (§2, done here). This bundle chose **not** to add one:

- The parser + `HistoricalProvider` adapter proof (httpmock, no live
  network) already exercises the full `ProviderBar` conversion path,
  including the documented volume scale.
- `ingest_provider_bars_to_md_bars_with_provider_metadata` (the reuse path
  identified by `01S-T` §2 item 7) is unchanged and provider-agnostic by
  construction — no schema change is needed for a future Kraken-sourced
  row.
- Adding a DB-backed test here would require standing up/asserting against
  the local paper Postgres and would not prove anything the pure
  parser/adapter tests do not already prove (no ingest command wires
  `"kraken"` into `ingest-provider`/`sync-provider` in this patch — those
  remain hard-locked to `"twelvedata"|"alpaca"`).

This is an explicit deferral, not an oversight: a future patch that wires
Kraken into `ingest-provider`/`sync-provider` (a DB-writing, operator-
authorized change) is the natural place for a DB-backed proof.

---

## 9. Safety boundaries

- No live Kraken network call made in default validation (only fixture
  files and `httpmock` mocks were used).
- No DB connection opened, no DB write, no DB migration.
- No `md_bars` write.
- `kraken` provider stays `enabled: false` in the committed registry.
- No scheduler, no daemon ingest job, no GUI change.
- No broker/risk/execution/OMS/runtime/strategy code touched.
- No crypto/futures/options/forex trading enabled.

---

## 10. What remains open

- No recurring/scheduled Kraken ingestion.
- No daemon ingest job wiring `"kraken"` into `ingest-provider`/`sync-provider`.
- No GUI status surface for Kraken OHLCV (mirroring `01N`-`01R`'s CoinLore
  pattern would be the natural next step if warranted).
- No DB-backed ingest proof (§8) — deferred to the patch that first wires
  Kraken into a DB-writing ingest path.
- Kraken's numeric public rate limit remains unestablished (`01S-T` §8,
  unchanged).
- Coinbase Exchange and Binance/Binance.US remain unverified candidates
  (unchanged from `01S-T`).
- No production registry-v2 cutover, no crypto session/calendar runtime
  enforcement, no crypto risk policy activation, no crypto broker/paper
  execution, no crypto strategy.

This bundle does not imply crypto trading readiness, recurring-ingestion
readiness, or production OHLCV-provider readiness in any respect.
