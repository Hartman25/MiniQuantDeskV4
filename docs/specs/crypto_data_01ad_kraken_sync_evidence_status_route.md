# CRYPTO-DATA-01AD — Kraken Sync Evidence Status Route

Patch ID: `CRYPTO-DATA-01AD-KRAKEN-SYNC-EVIDENCE-STATUS-ROUTE-01`

Continues the crypto data lane after
`CRYPTO-DATA-01AB-AC-KRAKEN-CONTENT-DIFF-SYNC-BUNDLE-01-COMBINED` (content-diff-aware
`kraken-ohlc-sync`). Adds read-only operator visibility for Kraken OHLC
ingest/sync evidence, mirroring the existing
`GET /api/v1/market-data/latest-marks/status` pattern
(`CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED`).

This is **data/operator visibility only**. It does not enable crypto
trading, does not add a scheduler, does not start daemon runtime, does not
submit orders, and does not make Kraken production-active.

---

## 1. Route

```text
GET /api/v1/market-data/kraken-ohlc/status
```

Public (no operator auth), matching the sibling `latest-marks/status` and
`intraday-refresh/status` routes.

Implemented in `core-rs/crates/mqk-daemon/src/routes/transport_quality.rs::kraken_ohlc_status`,
registered in `core-rs/crates/mqk-daemon/src/routes.rs`. Response type
`KrakenOhlcStatusResponse` in `core-rs/crates/mqk-daemon/src/api_types.rs`.

## 2. Evidence source

Reads the same evidence directory as `latest-marks/status`/`intraday-refresh/status`
(`st.md_refresh_evidence_dir`, env var `MQK_MD_REFRESH_EVIDENCE_DIR`, default
`exports/market_data`), filtered to two distinct filename prefixes:

- `kraken_ohlc_ingest_<epoch>.json` (written by `mqk md kraken-ohlc-ingest --output-dir`)
- `kraken_ohlc_sync_<epoch>.json` (written by `mqk md kraken-ohlc-sync --output-dir`)

The latest file is selected by the **epoch-seconds timestamp embedded in the
filename**, not by alphabetical filename order. This matters because
`"kraken_ohlc_ingest_..."` sorts alphabetically before
`"kraken_ohlc_sync_..."` regardless of which file is actually newer — a
naive alphabetical-last selection (as `latest-marks/status` uses, correctly,
for its single-prefix case) would misorder a newer ingest file behind an
older sync file. `latest_mode` in the response (`"ingest"`/`"sync"`) records
which command produced the selected file.

The route never connects to a DB, never calls Kraken or any provider, never
runs the CLI, never triggers a sync/ingest, never mutates trading state, and
never writes/stages evidence — it only reads whatever evidence files already
exist on disk.

## 3. Accepted schema versions

- `kraken-ohlc-ingest-v1` (unchanged since `CRYPTO-DATA-01X-Y`)
- `kraken-ohlc-sync-v2` (current, since `CRYPTO-DATA-01AB-AC`)

Any other `schema_version` (including the retired `kraken-ohlc-sync-v1`)
is surfaced as `truth_state="parse_error"`, naming the unsupported version —
this route does not attempt to interpret an evidence shape it was not built
against.

## 4. `truth_state` values

- `"active"` — latest evidence file parsed successfully, passed every
  safety check (§5), and is fresh (`produced_at_utc` within
  `max_evidence_age_secs`).
- `"stale"` — evidence parsed and passed safety checks but is older than
  `max_evidence_age_secs`, or carries no `produced_at_utc`.
- `"no_evidence"` — no `kraken_ohlc_ingest_*.json`/`kraken_ohlc_sync_*.json`
  file found in the evidence directory.
- `"parse_error"` — evidence file found but JSON is malformed or has an
  unsupported `schema_version`.
- `"unsafe_evidence"` — evidence fails a fail-closed safety check (§5).
  Never surfaced as `"active"` regardless of freshness.
- `"backend_unavailable"` — evidence directory or file could not be read.

## 5. Fail-closed safety checks (`kraken_ohlc_unsafe_reason`)

Independent of the CLI's own invariants — this route does not trust the
producer, it verifies. Evidence is `"unsafe_evidence"` if any of:

- `provider` is not `"kraken"`.
- `network_call_made=true` without the evidence's own `mode` field naming
  the CLI's explicit operator opt-in path (`mode="network_smoke"`, the only
  mode the CLI ever pairs with a real network call).
- `completed_bar_claim=true` is present (Kraken ingest/sync evidence never
  sets this field; its presence would imply an unverified completed-bar claim).
- `forming_candle_excluded=false` is present.
- `db_write=false` while `rows_inserted`/`rows_updated` report values > 0
  (an internally inconsistent claim).
- `md_bars_write=true` while `bars_excluded_forming=0` (inconsistent with
  the committed Kraken fixtures, which always carry exactly one forming
  row — a heuristic tied to the current fixture-only validation posture).
- Any of `volume_semantics`, `provider_id`, `provider_source`, `ingest_mode`
  is missing.
- Any top-level field implying trading/broker/order execution is present
  (`order_id`, `broker_order_id`, `fill_price`, `position_id`,
  `account_id`, `side`, `limit_price`, `stop_price`).

## 6. Response fields

`canonical_route`, `truth_state`, `provider`, `latest_mode`
(`"ingest"`/`"sync"`), `latest_schema_version`, `produced_at_utc`,
`evidence_path`, `stale_or_missing_evidence`, `max_evidence_age_secs`,
`network_call_made`, `db_write`, `md_bars_write`, `provider_id`,
`provider_source`, `provider_symbol`, `ingest_mode`, `sync_policy`
(sync-only), `no_update_existing` (sync-only), `symbols_requested`,
`bars_completed`, `bars_excluded_forming`, `bars_considered_for_sync`
(sync-only), `bars_missing_new` (sync-only), `bars_existing_candidate`
(sync-only), `rows_changed` (sync-only), `rows_skipped_unchanged`
(sync-only), `rows_changed_skipped_due_to_no_update_existing` (sync-only),
`rows_inserted`, `rows_updated`, `rows_skipped_if_known` (sync-only),
`latest_existing_end_ts_before` (sync-only), `latest_completed_start_ts`,
`latest_completed_end_ts`, `volume_semantics`, `volume_scale`,
`all_passed`, `reason_code`, `fail_reasons`, `error`.

Fields absent from the selected evidence file's own schema (e.g.
`sync_policy` when `latest_mode="ingest"`) are `None`/`null`, never
fabricated with a default.

## 7. Staleness

Default max evidence age: 24 hours (`86_400` seconds), matching
`latest-marks/status`. Overridable via
`MQK_KRAKEN_OHLC_EVIDENCE_MAX_AGE_SECS`. Stale evidence is never `"active"`.

## 8. DB-backed / test proof

`core-rs/crates/mqk-daemon/tests/scenario_kraken_ohlc_status_route_01ad.rs`
(11 tests, no DB required — the route itself never opens one):

- Missing evidence dir → `backend_unavailable`.
- Empty evidence dir → `no_evidence`.
- Valid fresh sync evidence → `active`, all sync-specific fields parsed.
- Valid fresh ingest evidence → `active`, `latest_mode="ingest"`,
  sync-only fields `null`.
- Old `produced_at_utc` → `stale`.
- Malformed JSON → `parse_error`, no panic.
- Wrong `schema_version` → `parse_error`.
- `forming_candle_excluded=false` → `unsafe_evidence`.
- Wrong `provider` → `unsafe_evidence`.
- A higher-epoch ingest file is selected over a lower-epoch sync file
  (proves epoch-based selection, not alphabetical filename order).
- Route returns `active` with no DB pool configured at all (the `AppState`
  test constructor never wires one; the handler contains no `mqk_db` call).

## 9. Safety boundaries

- No live Kraken network call (route never calls Kraken).
- No DB connection opened by this route.
- No CLI execution triggered.
- No scheduler, no daemon ingest job added.
- No GUI change (deferred to `CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01`).
- No broker/risk/execution/OMS/runtime/strategy code touched.
- No crypto trading enabled.

## 10. What remains open

- No GUI surface consuming this route yet (tracked as `01AE`).
- No recurring/scheduled Kraken sync of any kind.
- No production registry-v2 cutover, no crypto session/calendar runtime
  enforcement, no crypto risk policy activation, no crypto broker/paper
  execution, no crypto strategy.

This patch does not imply crypto trading readiness in any respect.
