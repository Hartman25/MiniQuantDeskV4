# CRYPTO-DATA-01Z-AA — Kraken Incremental Sync DB Proof

Patch ID: `CRYPTO-DATA-01Z-AA-KRAKEN-INCREMENTAL-SYNC-DB-PROOF-BUNDLE-01-COMBINED`

Continues the crypto data lane after
`CRYPTO-DATA-01X-Y-KRAKEN-INGEST-PROVIDER-DB-PROOF-BUNDLE-01-COMBINED`
(fixture-first Kraken `md_bars` DB-write proof via `kraken-ohlc-ingest`).
This bundle adds a Kraken-specific incremental sync command, closing two
adjacent slices together:

- `CRYPTO-DATA-01Z-KRAKEN-INCREMENTAL-SYNC-CLI-01`
- `CRYPTO-DATA-01AA-KRAKEN-INCREMENTAL-SYNC-DB-PROOF-01`

This is **explicit operator sync proof only**. It is **not** recurring
ingestion, **not** Windows Scheduled Task registration, **not** daemon ingest
job wiring, **not** GUI, **not** runtime trading, **not** crypto risk, **not**
crypto paper execution, **not** strategy enablement, and **not** production
registry-v2 cutover.

---

## 1. CLI surface: `mqk md kraken-ohlc-sync`

A new, additive command
(`core-rs/crates/mqk-cli/src/commands/md.rs::md_kraken_ohlc_sync`), sharing
`kraken-ohlc-ingest`'s parse/alias-resolution/fail-closed-gate path exactly,
but adding a pre-write read of existing `md_bars` state before the write:

```text
mqk md kraken-ohlc-sync \
  --registry <registry-v2.json> \
  --symbol <BTC/USD|ETH/USD> \
  --timeframe 1D \
  [--input-file <kraken-ohlc-response.json>] \
  [--output-dir <dir>] \
  [--no-update-existing]
```

Behavior:

1. Only `--timeframe 1D` is supported; any other value is refused before any
   file/network/DB access.
2. Resolves the `KrakenAlias` for `--symbol` from the registry-v2 fixture,
   identical to `kraken-ohlc-ingest`.
3. **Fail-closed safety gate**, identical shape to `kraken-ohlc-ingest`'s but
   with its own named reason: `--input-file` absent and
   `MQK_ALLOW_KRAKEN_NETWORK_SMOKE` unset refuses with
   `kraken_sync_requires_input_file_or_network_opt_in` **before any DB
   connection is attempted**.
4. Parses the body via `parse_kraken_ohlc_response`; only
   `completed_provider_bars()` output is ever considered for a write — the
   forming row is never converted.
5. **Only after** the parse/gate above succeeds, connects to the DB
   (`mqk_db::connect_from_env()`) and calls the existing, unmodified
   `mqk_db::md::latest_stored_bar_end_ts(pool, symbol, timeframe)` helper to
   read the pre-sync high-water mark for this symbol/timeframe — this is the
   "inspect existing `md_bars` state" step, using a helper that already
   existed before this bundle (no `mqk-db` source change).
6. Classifies each completed candidate bar as **missing/new** (`end_ts` >
   the pre-sync high-water mark) or an **existing candidate** (`end_ts` <=
   the high-water mark). This is a presence check, not a per-row
   OHLCV-value comparison — see §2 for why, and why that limitation is
   acceptable here.
7. Applies the sync policy (§2) to decide which completed bars are actually
   sent to the write helper, then calls the same
   `ingest_provider_bars_to_md_bars_with_provider_metadata` helper
   `kraken-ohlc-ingest` uses (unchanged, provider-agnostic, no schema
   change), stamped with:
   - `provider_id = "kraken"`, `provider_source = "kraken"`
   - `provider_symbol = <Kraken's own result key>` (e.g. `"XXBTZUSD"`)
   - `ingest_mode = "provider_sync"` — deliberately distinct from
     `kraken-ohlc-ingest`'s `"provider_ingest"`, so DB rows and
     `md_quality_reports` history can be traced back to which command wrote
     them.
   - A separate deterministic `ingest_id` namespace
     (`mqk-md-ingest.kraken.sync.v1|...`) from `kraken-ohlc-ingest`'s
     (`mqk-md-ingest.kraken.v1|...`), so the two commands' quality-report
     rows never collide.
8. Prints operator-visible `key=value` lines and, with `--output-dir`, an
   evidence JSON (`kraken-ohlc-sync-v1`) — see §4.

`kraken-ohlc-ingest` (`01X-Y`) is **unchanged**: it remains a separate,
independently-invokable command with its own `"provider_ingest"` stamp.

---

## 2. Sync policy: honest limitation, not hidden

The underlying `ingest_provider_bars_to_md_bars_with_provider_metadata`
helper (`core-rs/crates/mqk-db/src/md.rs`, reused unchanged — this bundle
does not touch `mqk-db/src/*`) performs a plain `INSERT ... ON CONFLICT (symbol,
timeframe, end_ts) DO UPDATE` upsert. It has **no row-content-diff "skip if
unchanged"** capability — it always either inserts or unconditionally
updates a conflicting row, and reports only `rows_inserted`/`rows_updated`
via the `(xmax = 0)` trick, never a "was this row's content actually
different" signal.

Adding real content-diff skip semantics would require either:

- a new `mqk-db` read helper that fetches full row contents for comparison
  (a `mqk-db/src/*` change, explicitly out of this bundle's file scope), or
- giving `mqk-cli` a direct `sqlx` dependency to run ad-hoc comparison
  queries against the pool from `src/` (a `Cargo.toml` scope change beyond
  this bundle's enumerated file list).

Per the mission's own escape hatch for this exact situation, this bundle
does **not** force skip semantics into the DB helper. Instead it documents
and proves the current upsert semantics honestly, using only
`latest_stored_bar_end_ts` (an existing, unmodified helper) for a
**presence-based** classification:

- **Default policy** (`--no-update-existing` absent):
  `sync_policy=upsert_existing_matches_ingest_helper_default_no_content_diff_skip_detection`.
  Every completed bar is sent to the write helper — a previously-stored row
  is always updated (even if, coincidentally, to the same values), never
  skipped by value-equality. `rows_skipped_if_known=0` always, since nothing
  is withheld from the write helper under this policy.
- **Conservative policy** (`--no-update-existing` present):
  `sync_policy=skip_existing_end_ts_no_update_content_diff_not_detected`.
  Any completed bar whose `end_ts` was already <= the pre-sync high-water
  mark is never sent to the write helper at all — a real, provable-by-
  presence skip (not a value-equality skip: a row that changed upstream
  would also be skipped under this policy, since the check is presence-only).

Both policies are honest about what they do and do not detect. Neither
fabricates a "skipped because unchanged" claim the code cannot actually
prove.

---

## 3. DB-backed proof

`core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_sync_db_01zaa.rs`:

- `ksz01_no_input_file_and_no_network_opt_in_refuses_without_db_env` — plain
  `#[test]`, no `MQK_DATABASE_URL` needed or set; proves the fail-closed gate
  refuses **before** any DB connection, naming
  `kraken_sync_requires_input_file_or_network_opt_in`.
- `ksz02_kraken_fixture_sync_inserts_missing_updates_existing_and_cleans_up`
  — `#[ignore]`-gated on `MQK_DATABASE_URL`; run with:

  ```powershell
  $env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5434/mqk_test?sslmode=disable"
  cargo test -p mqk-cli --test scenario_cli_kraken_ohlc_sync_db_01zaa -- --include-ignored
  ```

  Proves, against the committed BTC/ETH Kraken fixtures:

  1. **Seed**: a single stale "earlier" completed BTC/USD row (`end_ts=1783123200`)
     is inserted directly (bypassing the CLI) with values that provably
     differ from the fixture (`close_micros=60000000000` /
     `volume=1000000000`, vs. the fixture's real earlier-row values
     `close_micros=62539000000` / `volume=98012345678` scaled) — a
     deliberately different stale placeholder.
  2. **First sync** (default policy): `latest_existing_end_ts_before` reads
     back the seeded row's `end_ts`; the newer fixture row is classified
     `bars_missing_new=1` (inserted), the seeded older row is classified
     `bars_existing_candidate=1` (updated in place — its stale value is
     overwritten with the fixture's real value, proving the "existing gets
     updated" half of the documented policy). `inserted=1`, `updated=1`.
  3. Forming row (`end_ts=1783296000`) is never written.
  4. Scaled volume DB readback matches the documented exact values after the
     update: BTC latest `131715941434`, BTC earlier (post-update, corrected
     from the stale seed) `98012345678`, ETH latest `1558801759742`.
  5. `end_ts = row.time + 86400` (`1783123200`, `1783209600`).
  6. **Idempotent re-run** (default policy): `latest_existing_end_ts_before`
     now reads the first run's high-water mark; both rows are classified
     existing (`bars_missing_new=0`); `inserted=0`, `updated=2` — matching
     `01X-Y`'s own documented idempotency semantics for the same underlying
     helper.
  7. **`--no-update-existing` proof**: a third run with the conservative flag
     classifies both rows as already-covered and sends nothing to the write
     helper: `rows_skipped_if_known=2`, `inserted=0`, `updated=0`,
     `md_bars_write=false`, and the stored row count is unchanged — a real,
     provable skip.
  8. ETH/USD proves the path is not hardcoded to one symbol: no prior rows,
     both bars classified `bars_missing_new=2`, `inserted=2`, `updated=0`.
  9. `oms_outbox` row count is identical before and after the whole test.
  10. Cleanup deletes only rows matching the exact `(symbol, timeframe='1D',
      end_ts)` keys **and** `provider_id='kraken'`, then asserts a
      post-cleanup count of `0` for both symbols.

No DB migration was needed. No `mqk-db/src/*` change was made.

---

## 4. Evidence contract

With `--output-dir`, writes
`exports/market_data/kraken_ohlc_sync_<epoch_seconds>.json`
(`schema_version="kraken-ohlc-sync-v1"`) carrying: `producer`,
`produced_at_utc`, `provider="kraken"`, `mode` (`"input_file"` |
`"network_smoke"`), `network_call_made`, `db_write` (always `true` — sync
always reaches the DB-connect step once the gate passes), `md_bars_write`
(`true` iff at least one bar was actually sent to the write helper),
`provider_id`, `provider_source`, `provider_symbol`,
`ingest_mode="provider_sync"`, `sync_policy`, `no_update_existing`,
`symbols_requested`, `bars_completed`, `bars_excluded_forming`,
`bars_considered_for_sync`, `bars_missing_new`, `bars_existing_candidate`,
`rows_inserted`, `rows_updated`, `rows_skipped_if_known`,
`latest_existing_end_ts_before`, `latest_completed_start_ts`,
`latest_completed_end_ts`, `volume_semantics`, `volume_scale`, `all_passed`,
`reason_code`, `fail_reasons`.

`bars_excluded_forming >= 1` for the committed fixtures. `volume_semantics`
preserves the `kraken_base_asset_volume_scaled_by_1e8_not_whole_coins_not_usd`
wording unchanged from `01U-V-W`/`01X-Y`.

---

## 5. `sync-provider` / `ingest-provider` — untouched

Neither generic command was changed. Both remain hard-locked to
`"twelvedata"|"alpaca"` exactly as `01X-Y` left them (see
`docs/specs/crypto_data_01x_y_kraken_ingest_provider_db_proof.md` §1 for the
full "why a new command instead" rationale, which applies identically here:
`sync-provider`'s incremental model assumes a live, date-range-queryable
provider, not a single fixed-shape Kraken fixture response). Kraken now has
two separate, explicit operator commands: `kraken-ohlc-ingest` (one-shot) and
`kraken-ohlc-sync` (incremental-aware, presence-based). No recurring
scheduler or daemon job wraps either.

---

## 6. Safety boundaries

- No live Kraken network call made by default or during validation (fixture
  files only; `MQK_ALLOW_KRAKEN_NETWORK_SMOKE` never set).
- `kraken` provider stays `enabled: false` in `config/providers/providers.json`
  (unchanged, not touched by this bundle).
- No DB migration, no schema change, no `mqk-db/src/*` change.
- No scheduler, no daemon ingest job, no GUI change.
- No broker/risk/execution/OMS/runtime/strategy code touched.
- No crypto/futures/options/forex trading enabled.
- `ingest-provider`/`sync-provider` source-name allowlists unchanged.

---

## 7. What remains open

- Content-diff ("skip only if truly unchanged") semantics remain
  unimplemented — an explicit, documented deferral (§2), not an oversight.
  Closing it would require a `mqk-db` read-helper addition or a direct
  `sqlx` dependency in `mqk-cli`, both out of this bundle's file scope.
- No recurring/scheduled Kraken sync of any kind.
- No GUI status surface for Kraken sync.
- No production registry-v2 cutover, no crypto session/calendar runtime
  enforcement, no crypto risk policy activation, no crypto broker/paper
  execution, no crypto strategy.

This bundle does not imply crypto trading readiness, recurring-ingestion
readiness, or production OHLCV-provider readiness in any respect.
