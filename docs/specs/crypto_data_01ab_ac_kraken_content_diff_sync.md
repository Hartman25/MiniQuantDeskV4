# CRYPTO-DATA-01AB-AC — Kraken Content-Diff Sync

Patch ID: `CRYPTO-DATA-01AB-AC-KRAKEN-CONTENT-DIFF-SYNC-BUNDLE-01-COMBINED`

Continues the crypto data lane after
`CRYPTO-DATA-01Z-AA-KRAKEN-INCREMENTAL-SYNC-DB-PROOF-BUNDLE-01-COMBINED`
(presence-based Kraken incremental sync via `kraken-ohlc-sync`). Closes two
adjacent slices together:

- `CRYPTO-DATA-01AB-KRAKEN-MD-BARS-CONTENT-DIFF-HELPER-01`
- `CRYPTO-DATA-01AC-KRAKEN-SYNC-CONTENT-DIFF-PROOF-01`

This is **explicit operator sync quality proof only**. It is **not**
recurring ingestion, **not** Windows Scheduled Task registration, **not**
daemon ingest job wiring, **not** GUI, **not** runtime trading, **not**
crypto risk, **not** crypto paper execution, **not** strategy enablement, and
**not** production registry-v2 cutover.

---

## 1. What 01Z-AA left open

01Z-AA honestly documented (its own §2/§7) that `kraken-ohlc-sync`'s
"existing" classification was by `end_ts` presence relative to a pre-sync
high-water mark only — because the underlying write helper
(`ingest_provider_bars_to_md_bars_with_provider_metadata`) always performs an
unconditional `INSERT ... ON CONFLICT DO UPDATE`. It could not distinguish
"existing and unchanged" from "existing and changed": the default policy
upserted every completed bar regardless of whether its content had actually
changed.

## 2. New DB read helper — closes 01AB

`core-rs/crates/mqk-db/src/md.rs` adds:

```rust
pub struct ExistingMdBarForProviderSync {
    pub symbol: String,
    pub timeframe: String,
    pub end_ts: i64,
    pub open_micros: i64,
    pub high_micros: i64,
    pub low_micros: i64,
    pub close_micros: i64,
    pub volume: i64,
    pub is_complete: bool,
    pub provider_id: Option<String>,
    pub provider_source: Option<String>,
    pub provider_symbol: Option<String>,
    pub ingest_mode: Option<String>,
}

pub async fn fetch_md_bars_for_provider_sync_keys(
    pool: &PgPool,
    symbol: &str,
    timeframe: &str,
    end_ts_keys: &[i64],
) -> Result<BTreeMap<i64, ExistingMdBarForProviderSync>>
```

- Read-only. No migration, no schema change.
- Queries exactly `(symbol, timeframe, end_ts = any(keys))` — never a broader
  scan, never a different symbol/timeframe, never a broker/order/OMS table.
- Returns rows keyed by `end_ts` (a map, not a `Vec`) for O(1) lookup against
  the caller's candidate list — one query per sync invocation regardless of
  how many candidate bars there are.
- Returns an empty map without issuing a query when `end_ts_keys` is empty.

Alongside it, a pure comparison helper:

```rust
pub fn provider_bar_matches_existing(
    candidate: &ProviderBar,
    metadata: &MdBarProviderMetadata,
    existing: &ExistingMdBarForProviderSync,
) -> Result<bool>
```

Compares `open_micros`/`high_micros`/`low_micros`/`close_micros` (parsed from
the candidate's decimal strings via the same `price_to_micros` the write path
uses), `volume`, `is_complete`, and provider provenance (`provider_id`,
`provider_source`, `provider_symbol`, `ingest_mode`). Deliberately does
**not** compare `provider_bar_id` / `provider_updated_at_utc`: Kraken never
supplies either, and every write path here always leaves them `None`, so
comparing them would only ever compare `None == None` and add no signal —
documented, not silently dropped.

Both are generic over any provider using the existing `ProviderBar` /
`MdBarProviderMetadata` shapes — no Kraken-specific logic lives in `mqk-db`.
No other change was made to `mqk-db/src/md.rs`'s existing functions.

## 3. Updated `kraken-ohlc-sync` classification — closes 01AC

`core-rs/crates/mqk-cli/src/commands/md.rs::md_kraken_ohlc_sync` now reads
existing rows for the exact candidate `end_ts` keys before deciding what to
write, and classifies each completed (non-forming) bar as:

- **missing/new** (`bars_missing_new`): no existing row for that `end_ts` —
  always sent to the write helper.
- **changed** (`rows_changed`): existing row present but
  `provider_bar_matches_existing` returns `false` — sent to the write helper
  unless `--no-update-existing` is passed, in which case it is counted as
  `rows_changed_skipped_due_to_no_update_existing` and never sent.
- **unchanged** (`rows_skipped_unchanged`): existing row present and content
  matches exactly — **never** sent to the write helper, regardless of the
  `--no-update-existing` flag.

`bars_existing_candidate = rows_changed + rows_skipped_unchanged`. If no
candidate bar requires a write, the upsert helper (and its
`md_quality_reports` persistence) is **not called at all** —
`md_bars_write=false`, `rows_inserted=0`, `rows_updated=0`.

`sync_policy` is now a single fixed string:
`content_diff_skip_unchanged_update_changed_insert_missing` (the old
`--no-update-existing`-dependent `sync_policy` values from 01Z-AA are
retired — the policy string no longer varies by flag; the flag only changes
whether *changed* rows get written).

`--no-update-existing` semantics changed: it no longer skips by `end_ts`
presence alone. It still refuses to write **any** existing row, but the
evidence now distinguishes *why* a row was withheld:
`rows_skipped_unchanged` (would have been skipped regardless of the flag) vs.
`rows_changed_skipped_due_to_no_update_existing` (withheld only because of
the flag — the row is stale and stays stale in DB).

`latest_existing_end_ts_before` (the pre-sync high-water mark via
`latest_stored_bar_end_ts`, unchanged helper) is retained as a descriptive
evidence field only; it is no longer used for classification.

## 4. Evidence contract: `kraken-ohlc-sync-v2`

`KRAKEN_OHLC_SYNC_EVIDENCE_SCHEMA_VERSION` bumped from `kraken-ohlc-sync-v1`
to `kraken-ohlc-sync-v2` (materially different shape: new
`rows_changed`/`rows_skipped_unchanged`/
`rows_changed_skipped_due_to_no_update_existing` fields, changed
`sync_policy` semantics). Evidence written with `--output-dir` to
`exports/market_data/kraken_ohlc_sync_<epoch_seconds>.json` carries:
`schema_version`, `producer`, `produced_at_utc`, `provider`, `mode`,
`network_call_made`, `db_write` (always `true` once the fail-closed gate
passes and existing rows are read), `md_bars_write`, `provider_id`,
`provider_source`, `provider_symbol`, `ingest_mode`, `sync_policy`,
`no_update_existing`, `symbols_requested`, `bars_completed`,
`bars_excluded_forming`, `bars_considered_for_sync`, `bars_missing_new`,
`bars_existing_candidate`, `rows_changed`, `rows_skipped_unchanged`,
`rows_changed_skipped_due_to_no_update_existing`, `rows_inserted`,
`rows_updated`, `rows_skipped_if_known`, `latest_existing_end_ts_before`,
`latest_completed_start_ts`, `latest_completed_end_ts`, `volume_semantics`,
`volume_scale`, `all_passed`, `reason_code`, `fail_reasons`.

`rows_skipped_if_known` is retained: total completed bars not sent to the
write helper for *any* reason (`= rows_skipped_unchanged` under default
policy; `= rows_skipped_unchanged + rows_changed_skipped_due_to_no_update_existing`
under `--no-update-existing`).

## 5. DB-backed proof

Two test files now cover `kraken-ohlc-sync`:

- `core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_sync_db_01zaa.rs`
  (updated) — retains the fail-closed proof and the original insert/update
  narrative, with assertions updated to the new content-diff numbers
  (idempotent re-run is now `rows_skipped_unchanged=2`/`updated=0`, not
  `updated=2`; `--no-update-existing` now re-dirties the row first to prove
  a genuine changed-vs-unchanged distinction).
- `core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_content_diff_sync_db_01abac.rs`
  (new) — the full missing/changed/unchanged/no-update-existing matrix for
  both BTC/USD and ETH/USD:
  1. Fail-closed gate unchanged (`kraken_sync_requires_input_file_or_network_opt_in`).
  2. BTC/USD: seed one stale existing row (different close/volume/
     provider_symbol), leave the other row missing. Default sync:
     `bars_missing_new=1`, `rows_changed=1`, `rows_skipped_unchanged=0`,
     `inserted=1`, `updated=1`. Forming row never written. Metadata
     corrected to `provider_id=kraken`, `provider_source=kraken`,
     `provider_symbol=XXBTZUSD`, `ingest_mode=provider_sync`. Volume
     readback exact: latest `131715941434`, earlier `98012345678`.
  3. Re-run (true idempotency): `rows_skipped_unchanged=2`, `inserted=0`,
     `updated=0`, `md_bars_write=false`, row count unchanged at 2.
  4. Re-dirty the earlier row, then `--no-update-existing`: `rows_changed=1`,
     `rows_changed_skipped_due_to_no_update_existing=1`,
     `rows_skipped_unchanged=1` (the other row), `md_bars_write=false`, and
     the DB value **stays stale** (proven by readback).
  5. A subsequent default-policy run corrects the still-stale row (proves the
     flag is opt-in per invocation, not a persistent state change).
  6. ETH/USD: both rows missing on first sync (`inserted=2`), then re-run
     proves `rows_skipped_unchanged=2`/`md_bars_write=false`. Volume readback
     exact: `1558801759742`.
  7. `end_ts = row.time + 86400` proven via fixture constants.
  8. `oms_outbox` row count identical before/after.
  9. Cleanup deletes only exact `(symbol, timeframe='1D', end_ts)` keys
     **and** `provider_id='kraken'`; asserts `0` leftover rows for both
     symbols.

No DB migration. No schema change.

## 6. `sync-provider` / `ingest-provider` — untouched

Neither generic command was changed; both remain hard-locked to
`"twelvedata"|"alpaca"`. Kraken still has two separate, explicit operator
commands: `kraken-ohlc-ingest` (one-shot) and `kraken-ohlc-sync` (now
content-diff-aware). No recurring scheduler or daemon job wraps either.

## 7. Safety boundaries

- No live Kraken network call made by default or during validation (fixture
  files only; `MQK_ALLOW_KRAKEN_NETWORK_SMOKE` never set).
- `kraken` provider stays `enabled: false` in `config/providers/providers.json`
  (unchanged, not touched by this bundle).
- No DB migration, no schema change.
- No scheduler, no daemon ingest job, no GUI change (that is deferred to
  `CRYPTO-DATA-01AD-KRAKEN-SYNC-EVIDENCE-STATUS-ROUTE-01` /
  `CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01`, tracked separately).
- No broker/risk/execution/OMS/runtime/strategy code touched.
- No crypto/futures/options/forex trading enabled.
- `ingest-provider`/`sync-provider` source-name allowlists unchanged.

## 8. What remains open

- No recurring/scheduled Kraken sync of any kind.
- No GUI status surface for Kraken sync (tracked as 01AD/01AE).
- No production registry-v2 cutover, no crypto session/calendar runtime
  enforcement, no crypto risk policy activation, no crypto broker/paper
  execution, no crypto strategy.

This bundle does not imply crypto trading readiness, recurring-ingestion
readiness, or production OHLCV-provider readiness in any respect.
