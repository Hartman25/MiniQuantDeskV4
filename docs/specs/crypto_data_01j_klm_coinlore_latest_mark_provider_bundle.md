# CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED

Patch ID: `CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED`

This is provider/model/operator-readiness work. It is **not** completed-bar
ingestion, **not** OHLCV ingestion, **not** DB ingestion, **not** DB
mutation, **not** a production registry-v2 cutover, **not** a
portfolio-ledger cutover, **not** risk enforcement, **not** order routing,
**not** broker integration, **not** a DB migration, and **not** crypto
trading enablement. It continues the crypto data lane after
`CRYPTO-DATA-01H-LIVE-CRYPTO-PROVIDER-DECISION-01-COMBINED` and
`CRYPTO-DATA-01I-COINLORE-READONLY-NETWORK-VERIFY-01`, closing four small
adjacent slices together because they share the same lane, the same safety
profile, and the same files/modules:

- `CRYPTO-DATA-01J-LATEST-MARK-MODEL-01`
- `CRYPTO-DATA-01K-COINLORE-TICKER-PARSER-PROVIDER-01`
- `CRYPTO-DATA-01L-COINLORE-REGISTRY-ALIASES-01`
- `CRYPTO-DATA-01M-COINLORE-READONLY-CLI-SURFACE-01`

Built at HEAD `82d02a05` (the exact commit `01I` verified at).

---

## 1. Why This Bundle Exists

`CRYPTO-DATA-01I` made 2 bounded, keyless, read-only HTTP GETs to
`api.coinlore.net` and found CoinLore's verified endpoints
(`/api/tickers/`, `/api/ticker/?id=`) are **ticker/spot-only**: each returns
a single current `price_usd` and a rolling `volume24`, with no `open`,
`high`, `low`, and no per-ticker timestamp. `RawBar`/`ProviderBar` (the type
`MarketDataProvider::fetch_historical_bars`/`fetch_latest_closed_bar`
actually return) require exactly those bar fields. Populating them from
CoinLore's response would mean fabricating three OHLC fields (copying
`close`) and asserting a client-observed request time as a
provider-confirmed bar close — both forbidden by `01I`'s authorization and
`CLAUDE.md`'s no-fabricated-truth invariant.

`01I` §11 required `01J` to either introduce a distinct, honestly-labeled
latest-mark type, or make a further-authorized network call to check for a
real OHLCV endpoint. This bundle takes the first path: it adapts to a
ticker/latest-mark model. **No additional network call was made by this
bundle** — `01I`'s two verified requests remain the only network calls made
anywhere in this lineage.

---

## 2. 01J — Latest-Mark Model

New module: `core-rs/crates/mqk-md/src/latest_mark.rs`.

Public types:

- `LatestMark` — canonical symbol, provider id/symbol/coin id, `price_usd`
  (decimal string), optional `volume24_usd` (rolling window, explicitly
  labeled as such), `as_of_client_request_ts` (always present),
  `provider_ts: Option<i64>` (absent for CoinLore), `truth_state`
  (`ProviderTimestamped` | `ClientObservedOnly`), `kind` (`Ticker`).
- `LatestMarkTruthState`, `LatestMarkKind`, `LatestMarkProviderId`,
  `LatestMarkRequest`, `LatestMarkBatch`, `LatestMarkError` (`DisabledProvider`,
  `EmptyResponse`, `MalformedResponse`, `MissingConfiguredAsset`,
  `MismatchedIdentity`, `DuplicateIdentity`).

`LatestMark` has **no** `open`/`high`/`low`/`is_complete`/`end_ts` field and
**no** `From`/`Into` conversion to `RawBar`/`ProviderBar`. A test
(`lm03_serialized_shape_carries_no_bar_like_fields`) proves this at the wire
level: the serialized JSON shape contains none of those field names. This is
a structural, compile-time-plus-serialization guarantee, not a runtime flag
a caller could accidentally flip.

Not wired into `md_bars`, portfolio valuation, risk, runtime, or any order
path in this patch.

---

## 3. 01K — CoinLore Ticker Parser / Client

New module: `core-rs/crates/mqk-md/src/providers/coinlore.rs` (new
`providers` module in `mqk-md`, sibling to the existing `provider`/
`provider_registry` modules).

- `CoinloreAlias { canonical_symbol, coinlore_id, coinlore_symbol }`.
- `CoinloreTickerRaw` — the raw per-ticker shape verified by `01I`
  (`id`, `symbol`, `price_usd`, `volume24`; other CoinLore fields are
  present on the wire but not modeled since this patch does not use them).
- `parse_coinlore_ticker_response(body, aliases, as_of_client_request_ts)`
  — parses a bare JSON array into `LatestMark`s for exactly the configured
  aliases. Rejects (never fabricates or silently drops): an empty array
  (`EmptyResponse`), malformed JSON or a missing/empty `id`/`symbol`/
  `price_usd` (`MalformedResponse`), a non-decimal `price_usd` (validated
  via the crate's existing `normalize_price_str` — no float parsing
  introduced), duplicate `id` values (`DuplicateIdentity`), a response
  missing one of the configured aliases (`MissingConfiguredAsset`), and a
  response entry whose `symbol` does not match the configured alias for its
  `id` (`MismatchedIdentity`).
- `fetch_coinlore_ticker_body(coin_ids)` — performs exactly one HTTP GET to
  `https://api.coinlore.net/api/ticker/?id=<ids>`. This function does
  **not** check `providers.json`'s `enabled` flag or any environment
  variable itself; callers (the CLI) are responsible for gating it behind
  an explicit operator opt-in.
- `coinlore_aliases_from_registry_v2(registry)` — pure projection that
  extracts `CoinloreAlias` entries from a registry-v2 document for every
  instrument carrying both `provider_symbols.coinlore_id` and
  `provider_symbols.coinlore_symbol`.

`build_market_data_provider_from_config` (the bar-oriented factory in
`provider_registry.rs`) was **not** given a `"coinlore"` match arm. It was
already, and remains, unmodified: any `provider_id="coinlore"` passed to it
falls through to the existing `_ => Err(ProviderFactoryError::UnsupportedProvider)`
arm — a real, typed refusal, not a silent success. This satisfies `01I`
§11.2's requirement that CoinLore never be wired into the bar-oriented
contract while still allowing the latest-mark path to exist separately.

---

## 4. 01L — Registry Aliases

Only `config/instruments/instruments_v2.crypto_local_marks.example.json`
was touched. Both existing disabled rows gained two new
`provider_symbols` keys (no schema change — `provider_symbols` is already
a flat `BTreeMap<String, String>`):

- `BTC/USD`: `coinlore_id="90"`, `coinlore_symbol="BTC"`.
- `ETH/USD`: `coinlore_id="80"`, `coinlore_symbol="ETH"`.

Both values are exactly the CoinLore identity `01I` verified. `enabled`,
`paper_trading_enabled`, and `live_trading_enabled` remain `false` on both
rows; the existing `provider_symbols.local_csv` key is untouched.
`config/instruments/equities.json` was not touched. No production
registry-v2 file exists or was created.

---

## 5. 01M — Read-Only CLI Surface

New CLI command: `mqk md coinlore-latest-mark`
(`core-rs/crates/mqk-cli/src/commands/md.rs::md_coinlore_latest_mark`,
wired in `main.rs` as `MdCmd::CoinloreLatestMark`).

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- md coinlore-latest-mark `
  --registry .\config\instruments\instruments_v2.crypto_local_marks.example.json `
  --symbols BTC/USD,ETH/USD `
  --input-file .\path\to\coinlore_ticker_response.json
```

Behavior:

- Resolves the requested `--symbols` against the registry's CoinLore
  aliases; a symbol with no configured alias fails closed with a nonzero
  exit.
- Default mode reads `--input-file` (a local file containing a CoinLore
  `/api/ticker/?id=...` response body) — **zero network calls**.
- If `--input-file` is omitted, the command refuses to run
  (`MQK_ALLOW_COINLORE_NETWORK_SMOKE` is not set) unless the operator
  explicitly sets `MQK_ALLOW_COINLORE_NETWORK_SMOKE=1`, in which case it
  performs exactly one HTTP GET via `fetch_coinlore_ticker_body`.
- Never opens a DB connection (`mqk_db::connect_from_env` is never called
  anywhere in this function). Never writes `md_bars`. Never registers a
  scheduler.
- Prints `network_call_made=`, `db_write=false`, `md_bars_write=false`,
  `completed_bar_claim=false`, and one line per resolved `LatestMark`.
- Optionally writes a JSON evidence artifact to `--output-dir` (not staged;
  an operator artifact like the existing `exports/market_data/` outputs).

---

## 6. What This Bundle Did Not Change

No file under `core-rs/crates/mqk-daemon`, `mqk-runtime`, `mqk-execution`,
`mqk-broker-alpaca`, `mqk-broker-paper`, `mqk-risk`, `mqk-portfolio`, or
`mqk-db` was touched. No DB migration. No DB connection was opened by any
new code path. `config/instruments/equities.json` was not touched.
`scripts/windows/Import-LocalCryptoMarks.ps1` and
`scripts/windows/Register-LocalCryptoIngestTask.ps1` were not touched — the
three existing script validators (`01D`, `01E`, `01I`) all still pass
unmodified. `ingest-provider`/`sync-provider` remain hard-locked to
`"twelvedata"|"alpaca"`; this bundle adds no third value they accept.

---

## 7. Safety Confirmation

- No live or paper order was submitted.
- No daemon runtime was started.
- No DB connection was opened by any new code; no DB was mutated; no DB
  migration was added.
- No CoinLore ticker/latest-mark value was written to `md_bars`.
- No completed-bar/OHLCV claim was made anywhere: `LatestMark` carries no
  `open`/`high`/`low`/`is_complete`/`end_ts` field, and no conversion to
  `RawBar`/`ProviderBar` exists.
- No network call was made during default (fixture-based) validation. The
  only network-capable function this bundle adds
  (`fetch_coinlore_ticker_body`) is called from exactly one place (the CLI,
  gated behind `MQK_ALLOW_COINLORE_NETWORK_SMOKE`), and was not invoked
  during this patch's validation run.
- `providers.json`'s `coinlore` entry remains `enabled: false`.
- No crypto/futures/forex/options trading was enabled.
- No strategy, risk, runtime, broker, or order-path file was touched.

---

## 8. Remaining Gaps

- No live network crypto provider is implemented for completed-bar/OHLCV
  ingestion — CoinLore's verified endpoints remain ticker-only.
- No dedicated `latest_marks` storage/route exists; a `LatestMark` produced
  by this bundle's CLI is printed/written as an operator evidence artifact
  only, not persisted anywhere queryable. The next storage decision (a
  dedicated table/route, or continued no-DB evidence-only usage, or further
  provider verification for real OHLCV) remains open.
- No production registry-v2 cutover.
- No crypto session/calendar runtime enforcement, no crypto risk policy
  activation, no crypto broker/paper execution, no crypto strategy, no
  GUI/operator crypto surface.
- Local CSV import remains the only proven crypto `md_bars` ingest path.

This bundle does not imply crypto trading readiness, completed-bar provider
readiness, or live-mark storage readiness in any respect.
