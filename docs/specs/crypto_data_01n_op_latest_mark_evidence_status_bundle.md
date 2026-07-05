# CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED

Patch ID: `CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED`

This is operator-visibility and evidence-contract work. It is **not**
completed-bar ingestion, **not** OHLCV ingestion, **not** DB ingestion,
**not** DB mutation, **not** a DB migration, **not** a production
registry-v2 cutover, **not** a portfolio-ledger cutover, **not** portfolio
valuation, **not** risk enforcement, **not** order routing, **not** broker
integration, and **not** crypto trading enablement. It continues the crypto
data lane after
`CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED`,
closing three small adjacent slices together because they share the same
lane, the same safety profile, and the same files/modules:

- `CRYPTO-DATA-01N-LATEST-MARK-STORAGE-DECISION-01`
- `CRYPTO-DATA-01O-COINLORE-LATEST-MARK-EVIDENCE-CONTRACT-01`
- `CRYPTO-DATA-01P-LATEST-MARK-READONLY-STATUS-ROUTE-01`

Built at HEAD `1163f45c` (the commit `01J-K-L-M` closed at).

---

## 1. Executive Decision

**Use evidence-file-only latest-mark status.** No `latest_marks` DB table is
added. `md_bars` is not reused. The daemon exposes a new read-only status
route, `GET /api/v1/market-data/latest-marks/status`, that reads the latest
`mqk md coinlore-latest-mark --output-dir` evidence JSON from disk (the same
evidence directory `intraday-refresh/status` already reads, filtered to a
distinct filename prefix) and surfaces it with an honest `truth_state`. No
DB table, migration, or write path is introduced by this bundle.

---

## 2. Current Repo Facts (verified at HEAD `1163f45c`)

1. **Evidence JSON written today:** `mqk md coinlore-latest-mark
   --output-dir <dir>` (added by `01J-K-L-M`,
   `mqk-cli/src/commands/md.rs::md_coinlore_latest_mark`) wrote
   `<dir>/coinlore_latest_mark_<epoch_seconds>.json` with `schema_version`,
   `provider_id`, `network_call_made`, `db_write`, `md_bars_write`,
   `completed_bar_claim`, `requested_symbols`, and `marks`. It did **not**
   carry `produced_at_utc`, `mode`, `provider_enabled`, `registry_path`,
   `truth_state`, `stale_or_missing`, `all_passed`, `reason_code`, or
   `fail_reasons` — this bundle's `01O` slice adds those fields (§4 below).
2. **Route pattern for reading evidence files:** `GET
   /api/v1/market-data/intraday-refresh/status`
   (`mqk-daemon/src/routes/transport_quality.rs::intraday_refresh_status`)
   is the exact precedent: read `st.md_refresh_evidence_dir` with
   `std::fs::read_dir`, filter by filename prefix/suffix, alphabetically sort
   (filenames embed a sortable timestamp), read the latest match, parse JSON,
   check `schema_version`, and surface `truth_state` values of `"active"` /
   `"no_evidence"` / `"parse_error"` / `"backend_unavailable"`. This bundle's
   `01P` route follows the identical pattern.
3. **Evidence directory config:** `AppState.md_refresh_evidence_dir`
   (`mqk-daemon/src/state.rs`), sourced from `MQK_MD_REFRESH_EVIDENCE_DIR`,
   default `"exports/market_data"`. This is the same directory
   `Import-LocalCryptoMarks.ps1` and `Refresh-IntradayMarketData.ps1` already
   write evidence into (each with its own filename prefix).
4. **CoinLore enablement:** `config/providers/providers.json`'s `coinlore`
   entry remains `enabled: false`, `api_key_required: false`. The registry-v2
   `BTC/USD`/`ETH/USD` rows remain `enabled=false`,
   `paper_trading_enabled=false`, `live_trading_enabled=false`.
5. **No existing `latest_marks` DB table, migration, or query function**
   exists anywhere in `mqk-db`.

---

## 3. Options Considered

| Option | Verdict | Reason |
|---|---|---|
| Dedicated `latest_marks` DB table | Rejected for now | A DB write path is a categorically bigger step (migration, idempotent upsert, restart-safety proof, schema versioning per `db_rules.md`) than this bundle's stated scope. No consumer needs queryable historical latest-marks yet — the immediate need is *operator visibility into the most recent evidence file*, which the evidence-file approach already satisfies. |
| Reuse `md_bars` with a non-bar flag | Rejected | `md_bars` is a completed-bar table (`open`/`high`/`low`/`close`/`is_complete`/`end_ts` all NOT NULL per its schema and `RawBar`/`ProviderBar`'s Rust shape). A `LatestMark` has none of those fields honestly. Adding a "this row isn't really a bar" flag to a bar table is exactly the fabrication `CLAUDE.md` and the `01I`/`01J` lineage were built to avoid — every downstream `md_bars` consumer (backtest, portfolio economics, freshness gates) assumes every row is a real completed bar. |
| Evidence-file-only route (chosen) | **Selected** | Matches the exact precedent already proven safe and useful for `intraday-refresh/status`: zero DB dependency, zero migration, zero new write path, immediately queryable by an operator or the GUI later, and the honest `truth_state` model (`active`/`stale`/`no_evidence`/`parse_error`/`unsafe_evidence`) makes the absence of durable storage explicit rather than hidden. |
| No route / CLI-only | Rejected | Leaves the CLI's evidence artifact undiscoverable except by an operator who already knows the exact file path — no operator-visible status surface exists today for latest marks, unlike every other market-data evidence stream (`intraday-refresh/status`, `market-data/coverage`, `market-data/feed/status`). |

---

## 4. Decision and Rationale

Evidence-file-only status was chosen because:

- It requires no DB migration, no new write path, and no schema-versioning
  decision (`db_rules.md`'s `schema_version` requirement is trivially
  satisfied by the JSON envelope's own `schema_version` field, not a DB
  column).
- It reuses a pattern (`intraday_refresh_status`) already reviewed, tested,
  and running in production-shaped code — lower risk than inventing a new
  pattern for a small, evidence-only surface.
- It keeps the promotion path open: a `latest_marks` table can be added
  later as a distinct, separately-authorized patch if a real consumer
  (portfolio valuation, GUI dashboard, risk) needs queryable history: this
  bundle changes nothing that a future migration would need to undo.

---

## 5. Evidence Artifact Contract

Written by `mqk md coinlore-latest-mark --output-dir <dir>` to
`<dir>/coinlore_latest_mark_<epoch_seconds>.json` (unchanged file-path
pattern from `01M`; `01O` standardizes the JSON body). Required fields:

```
schema_version        "coinlore-latest-mark-v1"
producer              "mqk-cli md coinlore-latest-mark"
produced_at_utc        RFC3339 timestamp
provider               "coinlore"
mode                   "input_file" | "network_smoke"
network_call_made      bool
db_write               false (always)
md_bars_write          false (always)
completed_bar_claim    false (always)
provider_enabled       bool (from providers.json; operator-visibility only)
registry_path          string (the --registry path used)
symbols_requested      [string]
truth_state            "active" (the CLI only writes evidence after a successful parse)
stale_or_missing       false (freshness is a route-time concern, not a write-time one)
marks                  [LatestMark, ...] (canonical_symbol, provider_id, provider_symbol,
                        provider_coin_id, price_usd, volume24_usd,
                        as_of_client_request_ts, provider_ts, truth_state, kind)
all_passed             true (the CLI only writes evidence after a successful parse)
reason_code            "latest_mark_evidence_generated"
fail_reasons           [] (always empty; the CLI does not write evidence on a parse failure)
```

`provider_id` is also present at the top level for backward compatibility
with `01M`'s original evidence shape; `provider` is the `01O`-standardized
field name and is what the `01P` route reads.

No mark ever carries `open`/`high`/`low`/`close`/`is_complete`/`end_ts` —
`LatestMark`'s Rust type structurally cannot produce those fields (see
`mqk-md/src/latest_mark.rs`).

---

## 6. Route Contract

`GET /api/v1/market-data/latest-marks/status` (public, no auth — matches
every other read-only market-data status route).

Response fields: `canonical_route`, `truth_state`, `provider`,
`produced_at_utc`, `evidence_path`, `stale_or_missing_evidence`,
`max_evidence_age_secs`, `network_call_made`, `db_write`, `md_bars_write`,
`completed_bar_claim`, `provider_enabled`, `symbols_requested`, `marks`
(each: `canonical_symbol`, `provider_id`, `provider_symbol`,
`provider_coin_id`, `price_usd`, `volume24_usd`, `as_of_client_request_ts`,
`provider_ts`, `truth_state`, `kind`), `all_passed`, `reason_code`,
`fail_reasons`, `error`.

---

## 7. Staleness / Truth-State Model

| `truth_state` | Meaning |
|---|---|
| `active` | Latest evidence file parsed successfully, is safe, and is fresh (age ≤ `max_evidence_age_secs`). |
| `stale` | Latest evidence file parsed successfully and is safe, but its `produced_at_utc` is missing/unparseable or older than `max_evidence_age_secs`. |
| `no_evidence` | Evidence directory exists but contains no `coinlore_latest_mark_*.json` file. |
| `parse_error` | An evidence file was found but is not valid JSON, or its `schema_version` is not `"coinlore-latest-mark-v1"`. |
| `unsafe_evidence` | An evidence file parsed but claims `db_write`/`md_bars_write`/`completed_bar_claim=true`, or a mark carries a bar-like field (`open`/`high`/`low`/`close`/`is_complete`/`end_ts`). Never surfaced as `active` regardless of freshness — a fail-closed safety check independent of what the evidence claims about itself. |
| `backend_unavailable` | The evidence directory or the latest evidence file could not be read from disk. |

Max age defaults to 86 400 seconds (24 h), matching
`intraday_refresh_status`'s precedent; overridable via
`MQK_LATEST_MARK_EVIDENCE_MAX_AGE_SECS`.

---

## 8. Safety Boundaries

- The route never opens a DB connection (no `mqk_db` call anywhere in
  `latest_mark_status`).
- The route never calls CoinLore or any other provider/network endpoint.
- The route never starts the CLI, daemon runtime, or any scheduler.
- The route never writes to `outbox`/`inbox`/OMS/portfolio state.
- Evidence claiming an unsafe write/claim is refused (`unsafe_evidence`),
  never trusted at face value.
- `providers.json`'s `coinlore` entry remains `enabled: false`.

---

## 9. What This Does Not Change

- No file under `mqk-runtime`, `mqk-execution`, `mqk-broker-alpaca`,
  `mqk-broker-paper`, `mqk-risk`, `mqk-portfolio/src`, `mqk-db/src`, or
  `mqk-db/migrations` was touched.
- No DB migration. No DB mutation.
- `config/instruments/*.json` and `config/providers/providers.json` were not
  touched by this bundle (the CLI's `--provider-registry` flag only *reads*
  the existing file).
- `/api/v1/portfolio/live-weights` and `/api/v1/portfolio/economics/status`
  behavior is unchanged.
- No crypto/futures/options/forex trading is enabled.

---

## 10. Future Promotion Path

1. **This bundle:** evidence-file-only route (`01N`/`01O`/`01P`).
2. **Optional next:** a dedicated `latest_marks` DB table, only if a real
   consumer needs queryable history — a separately-authorized patch with its
   own migration, idempotency proof, and restart-safety proof per
   `db_rules.md`.
3. **Later:** a GUI operator surface consuming this route (or the future DB
   table).
4. **No trading enablement** at any point in this promotion path without
   separate, explicitly-authorized risk/execution patches.
