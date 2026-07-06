# CRYPTO-DATA-01X-Y — Kraken Ingest-Provider DB Proof

Patch ID: `CRYPTO-DATA-01X-Y-KRAKEN-INGEST-PROVIDER-DB-PROOF-BUNDLE-01-COMBINED`

Continues the crypto data lane after
`CRYPTO-DATA-01U-V-W-KRAKEN-OHLCV-ADAPTER-PARSER-CLI-BUNDLE-01-COMBINED`
(Kraken parser + read-only `kraken-ohlc-dry-run` CLI evidence, no DB write).
This bundle wires the same fixture-proven Kraken parser into the canonical
`md_bars` DB write path, closing two adjacent slices together:

- `CRYPTO-DATA-01X-KRAKEN-INGEST-PROVIDER-CLI-GATE-01`
- `CRYPTO-DATA-01Y-KRAKEN-MD-BARS-DB-PROOF-01`

This is **not** recurring ingestion, **not** scheduler registration, **not**
GUI, **not** runtime trading, **not** crypto risk, **not** crypto paper
execution, **not** strategy enablement, and **not** production registry-v2
cutover.

---

## 1. CLI surface decision: a Kraken-specific command, not `ingest-provider`

The existing `mqk md ingest-provider` / `mqk md sync-provider` commands
(`core-rs/crates/mqk-cli/src/commands/md.rs`) were inspected first. Both:

- hard-reject any `--source` other than `"twelvedata"`/`"alpaca"`
  (`unsupported --source '{}'. supported: twelvedata, alpaca`);
- have **no fixture/`--input-file` mode at all** — they always call
  `HistoricalProvider::fetch_bars` over a `--start`/`--end` date-range,
  chunked into provider-specific windows (`CHUNK_DAYS_1D`, etc.) sized for
  TwelveData's ~5000-bar response cap;
- derive their deterministic `ingest_id` from `(source, timeframe, symbols,
  date range)` or `(source, timeframe, per-symbol effective-start, end)` —
  a shape that assumes a live, incrementally-polled provider, not a
  single fixed-content fixture response.

Kraken's `/0/public/OHLC` response is a single fixed-shape payload (2 rows
per fixture, no date-range request semantics) already fully modeled by
`KrakenPairOhlc`/`parse_kraken_ohlc_response`. Bolting a fixture mode onto
`ingest-provider` would require either widening its date-range/chunking
contract to accommodate a provider that doesn't use it, or adding
conditional branches that special-case Kraken inside otherwise
TwelveData/Alpaca-shaped code — both violate this patch's minimal-scope
rule and risk regressing the existing equities path.

**Decision: Option B.** A new, additive `mqk md kraken-ohlc-ingest` command
was added, mirroring `kraken-ohlc-dry-run`'s parse/alias-resolution path
exactly and adding only the `md_bars` DB write at the end.
`ingest-provider`/`sync-provider` are **untouched** — same source-name
allowlist, same date-range/chunking behavior, same TwelveData/Alpaca
credential loading. Zero regression risk to the existing equities path.

`sync-provider` was **not** extended or touched. Its incremental
"detect-latest-stored-bar, backfill from there" model doesn't map onto a
single fixed fixture response either, and nothing in this bundle's mission
requires it. Wiring Kraken into `sync-provider` (or into a recurring
incremental Kraken sync) remains explicitly open — see §5.

---

## 2. `mqk md kraken-ohlc-ingest`

```text
mqk md kraken-ohlc-ingest \
  --registry <registry-v2.json> \
  --symbol <BTC/USD|ETH/USD> \
  --timeframe 1D \
  [--input-file <kraken-ohlc-response.json>] \
  [--output-dir <dir>]
```

Behavior (`core-rs/crates/mqk-cli/src/commands/md.rs::md_kraken_ohlc_ingest`):

1. Only `--timeframe 1D` is supported (matches the adapter's only verified
   interval); any other value is refused before any file/network/DB access.
2. Resolves the `KrakenAlias` (`kraken_pair`) for `--symbol` from the
   registry-v2 fixture at `--registry` — the same
   `kraken_aliases_from_registry_v2` projection `kraken-ohlc-dry-run` uses.
3. **Fail-closed safety gate** (identical to `kraken-ohlc-dry-run`'s):
   - `--input-file` present -> reads the local file, zero network calls.
   - `--input-file` absent -> requires `MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1`;
     if unset, refuses with
     `kraken_requires_input_file_or_network_opt_in` in the error message
     **before any DB connection is attempted**.
4. Parses the body via `parse_kraken_ohlc_response`, then converts only
   `KrakenPairOhlc::completed_provider_bars()` — the forming
   (not-yet-committed) row is never converted, never sent to the DB write
   path.
5. Builds a deterministic `ingest_id` (`Uuid::new_v5`, namespace
   `mqk-md-ingest.kraken.v1|{symbol}|{timeframe}|{kraken_pair}`) — stable
   across re-runs of the same symbol/registry, mirroring
   `md_ingest_csv`/`md_ingest_provider`'s UUIDv5 convention (no random UUID
   in `src/`).
6. Calls `mqk_db::connect_from_env()` and
   `mqk_db::md::ingest_provider_bars_to_md_bars_with_provider_metadata`
   (the existing, provider-agnostic, unchanged helper — no schema change,
   no migration) with:
   - `provider_id = "kraken"`
   - `provider_source = "kraken"`
   - `provider_symbol = <the wire-level Kraken pair key from the parsed
     response>` (e.g. `"XXBTZUSD"`) — not the alias config's
     `kraken_pair` query string, so the value stamped on the row records
     exactly what Kraken's API returned as its own key, not what the
     operator queried with.
   - `ingest_mode = "provider_ingest"` — truthful, distinct from
     `"csv_import"`.
   - `provider_bar_id` / `provider_updated_at_utc` left `None` (not
     fabricated; Kraken's OHLC response carries neither).
7. Prints operator-visible `key=value` lines and, with `--output-dir`, an
   evidence JSON (`kraken-ohlc-ingest-v1`) carrying: `schema_version`,
   `producer`, `provider`, `mode`, `network_call_made`, `db_write`
   (`true`), `md_bars_write` (`true`), `provider_id`, `provider_source`,
   `provider_symbol`, `ingest_mode`, `symbols_requested`, `bars_completed`,
   `bars_excluded_forming`, `latest_completed_start_ts`,
   `latest_completed_end_ts`, `volume_semantics`, `volume_scale`,
   `rows_inserted`, `rows_updated`, `all_passed`, `reason_code`,
   `fail_reasons`.

`kraken-ohlc-dry-run` (`01W`) is unchanged: it remains the read-only,
always-`db_write=false`/`md_bars_write=false` evidence surface.

---

## 3. DB-backed proof

`core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_ingest_db_01xy.rs`
(a CLI-level scenario test — the Kraken-specific alias resolution,
fixture parsing, and provider-metadata assembly all live in
`mqk-cli/src/commands/md.rs`, so a CLI test proves the full path; no
separate `mqk-db`-level test was added, since the underlying
`ingest_provider_bars_to_md_bars_with_provider_metadata` helper is already
covered generically by `mqk-db/tests/scenario_md_ingest_provider.rs`).

Two tests:

- `ki01_no_input_file_and_no_network_opt_in_refuses_without_db_env` — plain
  `#[test]`, no `MQK_DATABASE_URL` needed or set; proves the fail-closed
  gate refuses **before** any DB connection is attempted.
- `ki02_kraken_fixture_ingest_writes_completed_bars_with_truthful_metadata_and_cleans_up`
  — `#[ignore]`-gated on `MQK_DATABASE_URL` (matches
  `scenario_md_ingest_provider.rs`'s convention); run with:

  ```powershell
  $env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5434/mqk_test?sslmode=disable"
  cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_ingest_db_01xy -- --include-ignored
  ```

  Proves, against the committed `kraken_ohlc_xbtusd_1d.json` /
  `kraken_ohlc_ethusd_1d.json` fixtures and the real canonical `BTC/USD` /
  `ETH/USD` symbols:

  1. `network_call_made=false`, `db_write=true`, `md_bars_write=true`.
  2. `provider_id=kraken`, `provider_source=kraken`,
     `provider_symbol=XXBTZUSD`/`XETHZUSD`, `ingest_mode=provider_ingest`.
  3. The forming row (`end_ts=1783296000`) is never written — `row_exists`
     against that exact key returns `false`.
  4. Scaled volume DB readback matches the documented exact values:
     BTC latest `131715941434`, BTC earlier `98012345678`, ETH latest
     `1558801759742`.
  5. `end_ts = row.time + 86400` (readback rows keyed at `1783123200` and
     `1783209600`, matching the fixture's `time` values `1783036800` and
     `1783123200`).
  6. Re-running the identical ingest is idempotent: `inserted=0`,
     `updated=2` on the second call, and the row count stays at exactly 2
     per symbol (no duplication).
  7. `oms_outbox` row count is identical before and after the whole test —
     market-data ingest never touches the order/outbox path.
  8. Cleanup deletes only rows matching the exact `(symbol, timeframe='1D',
     end_ts)` keys **and** `provider_id='kraken'`, then asserts a
     post-cleanup count of `0` for both symbols — this can never delete
     pre-existing non-Kraken history at the same canonical symbol/
     timeframe, satisfying the "preserve canonical symbol proof for
     `BTC/USD`/`ETH/USD` if safe cleanup is reliable" requirement without
     risking collateral damage to unrelated data.

No DB migration was needed: `md_bars.provider_id/provider_source/
provider_symbol/ingest_mode/provider_bar_id/provider_updated_at_utc`
already exist (added by an earlier patch), and
`ingest_provider_bars_to_md_bars_with_provider_metadata`'s `ON CONFLICT
(symbol, timeframe, end_ts) DO UPDATE` upsert is unchanged.

---

## 4. Safety boundaries

- No live Kraken network call made by default or during validation (only
  the committed fixture files were used; `MQK_ALLOW_KRAKEN_NETWORK_SMOKE`
  was never set).
- `kraken` provider stays `enabled: false` in the committed
  `config/providers/providers.json` (unchanged, not touched by this
  bundle).
- No DB migration, no schema change.
- No scheduler, no daemon ingest job, no GUI change.
- No broker/risk/execution/OMS/runtime/strategy code touched.
- No crypto/futures/options/forex trading enabled.
- `ingest-provider`/`sync-provider` source-name allowlists unchanged
  (`"twelvedata"|"alpaca"` only) — Kraken reaches `md_bars` exclusively
  through the new, explicit `kraken-ohlc-ingest` command.

---

## 5. What remains open

- `sync-provider` (incremental backfill) has no Kraken path — an explicit
  deferral (§1), not an oversight. A Kraken-specific incremental sync
  command (`kraken-ohlc-sync`) now exists instead, closed by
  `CRYPTO-DATA-01Z-AA-KRAKEN-INCREMENTAL-SYNC-DB-PROOF-BUNDLE-01-COMBINED` —
  see `docs/specs/crypto_data_01z_aa_kraken_incremental_sync_db_proof.md`.
  `sync-provider` itself remains untouched and TwelveData/Alpaca-only.
- No recurring/scheduled Kraken ingestion of any kind.
- No GUI status surface for Kraken ingest (mirroring `01N`-`01R`'s CoinLore
  pattern would be the natural next step if warranted).
- No production registry-v2 cutover, no crypto session/calendar runtime
  enforcement, no crypto risk policy activation, no crypto broker/paper
  execution, no crypto strategy.

This bundle does not imply crypto trading readiness, recurring-ingestion
readiness, or production OHLCV-provider readiness in any respect.
