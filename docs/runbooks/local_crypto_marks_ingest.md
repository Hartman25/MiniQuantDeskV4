# Runbook: Local Crypto Marks Ingest

**Patch:** `CRYPTO-DATA-01D-EXPLICIT-LOCAL-CRYPTO-INGEST-RUNNER-01-COMBINED`

---

## Purpose

Import a local operator-supplied CSV file of crypto daily bars (e.g. BTC/USD 1D)
into `md_bars` so the ASSET-CORE-04 portfolio economics route can produce a mark
for a crypto position.

This is a manual, operator-run data-pipeline step — not a live provider, not a
recurring daemon job, not a trading action.

---

## What This Does

- Reads a local CSV file matching the `md_bars` CSV schema
  (`symbol,timeframe,end_ts,open,high,low,close,volume,is_complete`)
- Validates the file before any write: existence, header, required columns,
  data rows, latest bar timestamp
- In `-Once` mode: calls `mqk-cli md ingest-csv` (the existing, proven
  `CRYPTO-DATA-01A/01B` path) to upsert bars into `md_bars`
- Writes evidence JSON and TXT to `exports/market_data/`

## What This Does NOT Do

- Does **not** make any network call or call any market-data provider
- Does **not** start the daemon or runtime
- Does **not** touch the order path, OMS, outbox, broker, or risk engine
- Does **not** register a Windows Scheduled Task (deferred to CRYPTO-DATA-01E)
- Does **not** enable crypto trading, paper trading, or live trading
- Does **not** change strategy thresholds, risk policy, or admission gates
- Does **not** add a DB migration (existing `md_bars` schema stores BTC/USD)

---

## Required Environment Variable (for `-Once` only)

```
MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable
```

The script refuses to run in `-Once` mode unless `MQK_DATABASE_URL`:
- is non-empty
- contains port `5440`
- contains `miniquantdesk_paper`

**Set this in your shell before running `-Once`:**

```powershell
$env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable"
```

The script does **not** read `.env.local`. The env var must be set explicitly.

---

## CSV Schema

The input CSV must have this exact header (column order is mandatory):

```
symbol,timeframe,end_ts,open,high,low,close,volume,is_complete
```

- `symbol`: e.g. `BTC/USD` (the canonical symbol used throughout the system)
- `timeframe`: e.g. `1D`
- `end_ts`: Unix epoch seconds (integer), marks the close of the bar
- `open`, `high`, `low`, `close`: decimal price strings
- `volume`: integer
- `is_complete`: `true` or `false`

The committed fixtures for testing are:
`core-rs/crates/mqk-md/tests/fixtures/crypto_btcusd_1d_local.csv` and
`core-rs/crates/mqk-md/tests/fixtures/crypto_ethusd_1d_local.csv`
(`CRYPTO-DATA-01G-ETHUSD-LOCAL-CSV-MARKS-01-COMBINED` added the `ETH/USD`
fixture beside the existing `BTC/USD` one to prove this ingest path is not
hardcoded to a single symbol). Both fixtures use deterministic, committed
test data only — neither claims to be a current or live market price.

**Source file provenance is the operator's responsibility.** The script validates
the file format but not the accuracy, timeliness, or source of the price data.

---

## Commands

### Check-Only (read-only; default)

Validates the CSV file and reports coverage/staleness. No mutations. Safe to run
at any time.

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Import-LocalCryptoMarks.ps1 `
    -CheckOnly `
    -CsvPath .\path\to\btcusd_1d_local.csv `
    -Timeframe 1D `
    -Source local_crypto_manual
```

With the committed fixture (will report stale; safe to run):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Import-LocalCryptoMarks.ps1 `
    -CheckOnly `
    -CsvPath .\core-rs\crates\mqk-md\tests\fixtures\crypto_btcusd_1d_local.csv `
    -AllowStaleForValidation
```

### Import Once (mutation; requires paper DB env var)

Imports the CSV into `md_bars`. Requires `MQK_DATABASE_URL` to target the paper DB.

```powershell
$env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable"

powershell -ExecutionPolicy Bypass -File scripts\windows\Import-LocalCryptoMarks.ps1 `
    -Once `
    -CsvPath .\path\to\btcusd_1d_local.csv `
    -Timeframe 1D `
    -Source local_crypto_manual
```

---

## Parameters

| Parameter | Default | Description |
|---|---|---|
| `-CsvPath` | (required) | Path to local crypto CSV file |
| `-Timeframe` | `1D` | Bar timeframe string |
| `-Source` | `local_crypto_manual` | Source label for `md_quality_reports` |
| `-OutputDir` | `.\exports\market_data` | Evidence output directory |
| `-MaxAgeHours` | `36` | Latest bar age threshold for staleness gate |
| `-AllowStaleForValidation` | off | Bypass staleness gate for fixture testing |
| `-CheckOnly` | (default) | Read-only mode |
| `-Once` | off | Import mode (mutation) |

---

## Evidence Files

Every run writes two files to `exports/market_data/`:

```
exports/market_data/local_crypto_import_YYYYMMDD_HHmmss.json
exports/market_data/local_crypto_import_YYYYMMDD_HHmmss.txt
```

The JSON includes:
- `schema_version: "local-crypto-import-v1"`
- `mode`: `"check_only"` or `"once"`
- `checks`: file existence, header, columns, rows seen, latest bar timestamp,
  age in seconds, stale flag, passed flag
- `db_guard`: whether the guard was checked and the reason code
- `mutation`: whether cargo was called, the exact command, exit code, success
- `all_passed`: overall pass/fail
- `reason_code`: concise failure reason if `all_passed=false`
- `fail_reasons`: list of specific failure messages

The CLI itself also writes:
```
exports/md_ingest/<ingest_id>/data_quality.json
```

These evidence files are **not staged** — they are operator artifacts only.

---

## Interpreting States

| State | Meaning |
|---|---|
| `csv_file_not_found` | The path does not exist. Check `-CsvPath`. |
| `csv_empty_no_header` | File exists but is empty or blank. |
| `missing_columns` | Header row exists but required columns are absent. |
| `no_data_rows` | Header only, no data rows. |
| `end_ts_unparseable` | `end_ts` column values cannot be parsed as integers. |
| `latest_completed_bar_stale` | Latest bar older than `MaxAgeHours`. Update the CSV. |
| `database_url_missing` | `MQK_DATABASE_URL` not set. See env var section above. |
| `non_paper_database_target_refused` | URL does not target port 5440 or `miniquantdesk_paper`. |
| `cli_ingest_failed` | `mqk-cli md ingest-csv` returned nonzero. Check the CLI output above. |
| `import_succeeded` | All checks passed and ingest completed. |
| `checks_passed` | Check-only completed successfully. |

---

## Optional: Windows Scheduled Task (CRYPTO-DATA-01E)

**Patch:** `CRYPTO-DATA-01E-LOCAL-CRYPTO-INGEST-TASK-REGISTRATION-01-COMBINED`

`scripts/windows/Register-LocalCryptoIngestTask.ps1` provides an optional,
default-unregistered Windows Scheduled Task wrapper for the import runner.
The task is **not registered by default** — an operator must explicitly choose
to register it.

### What the task does

- Runs `Import-LocalCryptoMarks.ps1 -Once` daily at a configurable local time
  (default `07:30`).
- No daemon, no runtime, no provider, no broker, no order path is touched.
- The task does **not** set `MQK_DATABASE_URL`.  See MQK_DATABASE_URL note below.

### MQK_DATABASE_URL and the scheduled task

`MQK_DATABASE_URL` is **not** embedded in the task and is not read from
`.env.local`.  For the import to succeed, `MQK_DATABASE_URL` must be set as a
persistent Windows **user** or **system** environment variable before the task
runs.  Shell-session variables (`$env:VAR = ...`) are **not** inherited by
scheduled tasks.

If `MQK_DATABASE_URL` is absent or does not target port `5440` /
`miniquantdesk_paper`, the import runner **fails closed** — no partial write
occurs, evidence is written with `all_passed=false` and
`reason_code=database_url_missing`.

### Preview (no mutation)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Register-LocalCryptoIngestTask.ps1 `
    -CheckOnly `
    -CsvPath .\path\to\btcusd_1d_local.csv
```

With the committed fixture (safe to run at any time):

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Register-LocalCryptoIngestTask.ps1 `
    -CheckOnly `
    -CsvPath .\core-rs\crates\mqk-md\tests\fixtures\crypto_btcusd_1d_local.csv
```

### Register the task

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Register-LocalCryptoIngestTask.ps1 `
    -CsvPath .\path\to\btcusd_1d_local.csv `
    -At 07:30
```

`-CsvPath` is required for registration.  Do not point the task at the committed
test fixture for production use — supply a path to your regularly-updated CSV.

### Unregister the task

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Register-LocalCryptoIngestTask.ps1 `
    -Unregister
```

Unregister is safe and idempotent when the task does not exist.

### Evidence

Every run writes (overwrites) two files:

```
exports\market_data\local_crypto_ingest_task_registration.json
exports\market_data\local_crypto_ingest_task_registration.txt
```

The JSON includes `schema_version`, `mode`, `task_exists_before`,
`task_exists_after`, `registered`, `check_only`, `task_action`, `runner_path`,
`csv_path`, `all_passed`, `reason_code`, and a `safety` block with
`calls_import_runner_only=true`.

### Warnings

- The task calls **only** `Import-LocalCryptoMarks.ps1`.  It does not start
  the daemon, runtime, broker, or any provider script.
- The task does **not** set `MQK_DATABASE_URL`.  A persistent env var is required.
- **Crypto trading remains disabled.**  This task imports local data only.
- This is a **local-file import** only — not a live network provider.
- Each run applies the same staleness gate as the runner.  If the CSV has not
  been updated since the last run, the runner fails closed with
  `latest_completed_bar_stale`.

---

## Provider Metadata (CRYPTO-DATA-01F)

CSV-ingested rows now carry the `-Source` / `--source` label as truthful
`md_bars` provenance instead of `"unknown"`:

- `provider_id` = the `--source` value (e.g. `local_crypto_manual`), or
  `"unknown"` only if the label is blank/whitespace-only.
- `provider_source` = the same `--source` value.
- `ingest_mode` = `"csv_import"` — a mechanical fact of how the row was
  ingested, not a claim about the data's origin.
- `provider_symbol` is left unset: it is a single value applied to every row
  in one ingest batch, and a CSV file is not guaranteed to be single-symbol.

This is **provenance only**. It does not verify the accuracy, timeliness, or
source of the price data — that remains the operator's responsibility (see
"CSV Schema" above). It does not enable trading. It does not add a network
provider. The mark-read path used by `ASSET-CORE-04` still never queries
`provider_id` for valuation.

## ETH/USD Fixture (CRYPTO-DATA-01G)

A second disabled registry-v2 row and committed local CSV fixture exist for
`ETH/USD`, added beside the existing `BTC/USD` entry:

- Registry-v2 row: `config/instruments/instruments_v2.crypto_local_marks.example.json`
  (`ETH/USD`, `CryptoPair{base:"ETH",quote:"USD"}`, `enabled=false`,
  `paper_trading_enabled=false`, `live_trading_enabled=false`).
- CSV fixture: `core-rs/crates/mqk-md/tests/fixtures/crypto_ethusd_1d_local.csv`
  (3 deterministic daily bars, latest completed close `$3,200.00`).

This proves the local-CSV ingest path, the `CRYPTO-DATA-01F` provider-metadata
stamping, and the `ASSET-CORE-04A`/`04B`/`04C` model chain are generic over
symbol, not hardcoded to `BTC/USD`. `ETH/USD` remains disabled and
model-only, exactly like `BTC/USD` — this does not enable crypto trading.

## Live Provider Decision (CRYPTO-DATA-01H)

A live network crypto provider is still absent. `CRYPTO-DATA-01H` decided
only the *next lane* — it chose CoinLore as the first network-authorized
verification candidate (`CRYPTO-DATA-01I`, not yet built) and a CoinLore
provider adapter as the first implementation candidate after that
(`CRYPTO-DATA-01J`, not yet built). No network call was made, no provider
code was added, and no CLI/config file was changed by that decision. Local
CSV import (this runbook) remains the only proven crypto mark ingest path
until `01I`/`01J` close the network-provider gap. See
`docs/specs/crypto_data_01h_live_provider_decision.md` for the full
evaluation of all candidates.

## CoinLore Read-Only Network Verification (CRYPTO-DATA-01I)

`CRYPTO-DATA-01I` made the first explicitly-authorized, bounded, read-only
network verification of CoinLore (2 keyless HTTP GETs to
`api.coinlore.net`). Result: CoinLore reliably identifies BTC/ETH and
returns USD spot prices, but exposes **no OHLCV history and no per-ticker
timestamp** on the endpoints checked — decision `PARTIAL_TICKER_ONLY`. No
provider code was written, no CLI/config file was changed, no DB was
written or migrated, and no trading was enabled. Local CSV import (this
runbook) remains the only proven crypto mark ingest path until `01J` adapts
its scope to a ticker/latest-mark model (or makes a further-authorized call
to check for a real history endpoint) and actually builds the adapter. See
`docs/specs/crypto_data_01i_coinlore_network_verify.md` for full evidence.

## CoinLore Latest-Mark Provider Bundle (CRYPTO-DATA-01J-K-L-COINLORE-LATEST-MARK-PROVIDER-BUNDLE-01-COMBINED)

Adapting to `01I`'s ticker-only finding, this bundle added: an explicit
`LatestMark` model (`mqk_md::latest_mark`, no `open`/`high`/`low`/
`is_complete`/`end_ts` field and no conversion to `RawBar`/`ProviderBar`); a
CoinLore ticker parser (`mqk_md::providers::coinlore`) that rejects
malformed/empty/missing-asset/mismatched-identity responses rather than
fabricating data; CoinLore `provider_symbols.coinlore_id`/
`coinlore_symbol` aliases on both disabled `BTC/USD`/`ETH/USD` registry-v2
rows (`90`/`BTC` and `80`/`ETH`, matching `01I`'s verified IDs); and a
read-only `mqk md coinlore-latest-mark` CLI command that defaults to
parsing a local `--input-file` fixture (zero network calls) and only
attempts a live network call — at most one GET — when the operator
explicitly sets `MQK_ALLOW_COINLORE_NETWORK_SMOKE=1`. The command never
opens a DB connection and never writes `md_bars`. `providers.json`'s
`coinlore` entry remains `enabled: false`. See
`docs/specs/crypto_data_01j_klm_coinlore_latest_mark_provider_bundle.md`
for full detail.

## Latest-Mark Evidence Status Route (CRYPTO-DATA-01N-O-P-LATEST-MARK-EVIDENCE-STATUS-BUNDLE-01-COMBINED)

Continuing after `01J-K-L-M`, this bundle decided to keep latest-mark storage
evidence-file-only (no `latest_marks` DB table, no `md_bars` reuse — see
`docs/specs/crypto_data_01n_op_latest_mark_evidence_status_bundle.md` for the
full decision) and added a read-only daemon status route on top of it.

**Standardized evidence contract (`01O`):** `mqk md coinlore-latest-mark
--output-dir <dir>` writes `<dir>/coinlore_latest_mark_<epoch_seconds>.json`
carrying `schema_version`, `producer`, `produced_at_utc`, `provider`, `mode`
(`"input_file"` | `"network_smoke"`), `network_call_made`, `db_write`
(always `false`), `md_bars_write` (always `false`), `completed_bar_claim`
(always `false`), `provider_enabled` (read from `--provider-registry`,
default `config/providers/providers.json`, for operator visibility only —
never gates parsing or the network call), `registry_path`,
`symbols_requested`, `truth_state`, `stale_or_missing`, `marks` (each
carrying only `LatestMark`'s own fields — never `open`/`high`/`low`/`close`/
`is_complete`/`end_ts`), `all_passed`, `reason_code`, `fail_reasons`.

Generate a local fixture evidence file (zero network calls):

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- md coinlore-latest-mark `
  --registry .\config\instruments\instruments_v2.crypto_local_marks.example.json `
  --symbols BTC/USD,ETH/USD `
  --input-file .\path\to\coinlore_ticker_response.json `
  --output-dir .\exports\market_data
```

**Read-only status route (`01P`):** `GET
/api/v1/market-data/latest-marks/status` reads the latest
`coinlore_latest_mark_*.json` evidence file from the same evidence directory
`intraday-refresh/status` reads (`MQK_MD_REFRESH_EVIDENCE_DIR`, default
`exports/market_data`), filtered to its own filename prefix so the two
evidence streams never collide. It never opens a DB connection, never calls
CoinLore or any provider, and never mutates trading state — it is
read-only, evidence-file-backed.

`truth_state` values: `active` (fresh, safe evidence found), `stale`
(evidence found but older than `MQK_LATEST_MARK_EVIDENCE_MAX_AGE_SECS`,
default 86 400 s), `no_evidence` (no matching file), `parse_error`
(malformed JSON or wrong `schema_version`), `unsafe_evidence` (evidence
claims a `db_write`/`md_bars_write`/`completed_bar_claim`, or a mark carries
a bar-like field — never surfaced as `active`), `backend_unavailable`
(evidence directory/file unreadable).

**Reminder:** this route reports on ticker-only latest marks. It is **not**
`md_bars`, **not** OHLCV, and does not enable crypto trading.

## GUI Surface for Latest-Mark Evidence Status (CRYPTO-DATA-01Q-R-LATEST-MARK-GUI-SURFACE-BUNDLE-01-COMBINED)

The operator GUI (Ingest screen, `core-rs/mqk-gui/src/features/ingest/`) now
consumes `GET /api/v1/market-data/latest-marks/status` read-only and renders
a "Crypto latest marks" panel directly below the existing "Intraday refresh
status" panel.

- **Types** (`types.ts`): `LatestMarkStatusResponse`, `LatestMarkStatusMark`
  mirror the daemon's `LatestMarkStatusResponse`/`LatestMarkStatusRow`
  structs field-for-field.
- **API client** (`api.ts`): `fetchLatestMarkStatus` (GET only, no auth
  token, no CLI/provider call), `isLatestMarkStatusActive`,
  `latestMarkStatusTruthLabel` (words `unsafe_evidence` as a severe
  fail-closed condition, not a plain label), and a defense-in-depth
  `isLatestMarkEvidenceUnsafe` helper that also treats
  `db_write`/`md_bars_write`/`completed_bar_claim=true` as unsafe
  independent of the backend's own `truth_state` classification.
- **Panel** (`IngestScreen.tsx`): shows truth state, provider,
  `produced_at_utc`, evidence path, the `network_call_made`/`db_write`/
  `md_bars_write`/`completed_bar_claim`/`provider_enabled` flags, symbols
  requested, and one row per mark (`canonical_symbol`, `price_usd`,
  `volume24_usd`, `provider_symbol`, `provider_coin_id`,
  `as_of_client_request_ts`, `provider_ts` or "none", `kind`, `truth_state`).
  All six truth states (`active`, `stale`, `no_evidence`, `parse_error`,
  `unsafe_evidence`, `backend_unavailable`) render distinctly; `stale`/
  `no_evidence`/`backend_unavailable` show a stale/missing-evidence notice;
  `unsafe_evidence` (or the defense-in-depth check) renders a dedicated
  critical banner and suppresses the marks table even if the backend
  response also included data. The panel carries a fixed caption: "Ticker-only
  latest marks. Not OHLCV, not md_bars, not portfolio valuation, and not
  trading enablement." The only button is a local "Refresh" that re-GETs
  the same read-only route — no provider/network/CLI-triggering action.
- **Tests** (`__tests__/api.test.ts`): pure-function/shape tests for the new
  helpers and for the response shape under all six `truth_state` values,
  following this repo's established GUI test pattern. Note: this repo has no
  `.tsx` component-render test harness (no jsdom/testing-library dependency,
  zero existing `.test.tsx` files) — all GUI test coverage here and
  elsewhere in `mqk-gui` is pure-function/shape-level via `tsx --test`, not
  DOM rendering assertions. This bundle follows that existing convention
  rather than introducing new test infrastructure.

No backend/daemon route, API contract, CLI, DB, or trading-path code was
changed by this bundle. No CoinLore/provider/network call was added. No
`latest_marks` DB table or `md_bars` write was added.

## OHLCV Provider Decision + Verification (CRYPTO-DATA-01S-T-OHLCV-PROVIDER-DECISION-VERIFY-BUNDLE-01-COMBINED)

CoinLore (verified ticker-only, `01I`) cannot supply completed-bar/OHLCV
data. This bundle compared Kraken, Coinbase Exchange, Binance/Binance.US,
TwelveData crypto, and Alpaca crypto against current repo evidence, then
made 3 bounded, keyless, read-only GETs (1 docs fetch + BTC/USD + ETH/USD)
to confirm **Kraken's public `/0/public/OHLC` endpoint** as the selected
next completed-bar/OHLCV adapter lane: no credential required, real
`open`/`high`/`low`/`close`/`volume` fields for both symbols, and a
provider-supplied cursor (`result.last`) that lets a future adapter derive
`is_complete` honestly instead of guessing or fabricating it. Full
evidence, the candidate comparison table, and the exact completion-semantics
rule (`row.time <= result.last`, plus an `end_ts = row.time + interval_seconds`
correction since Kraken's `time` field is the bar's start, not its end) are
recorded in `docs/specs/crypto_data_01s_t_ohlcv_provider_decision_verify.md`.

**No adapter exists yet.** No provider code, factory arm, or CLI ingestion
path was added by this bundle. No DB write. No `md_bars` write. No crypto
trading. Local CSV import (this runbook) remains the only **proven** crypto
mark ingest path until a future, separately-authorized adapter patch
(recommended: `CRYPTO-DATA-01U-KRAKEN-OHLCV-PROVIDER-ADAPTER-LOCAL-INGEST-01`)
lands and its own DB-backed proof closes. Coinbase Exchange and Binance/
Binance.US were not ruled out but were not live-tested in this bundle;
Binance carries an unresolved, honestly-flagged geo-restriction risk.

## Kraken OHLCV Adapter, Parser, CLI Evidence (CRYPTO-DATA-01U-V-W-KRAKEN-OHLCV-ADAPTER-PARSER-CLI-BUNDLE-01-COMBINED)

Continuing after `01S-T`'s decision/verification, this bundle built the
first disabled-by-default, fixture-first Kraken OHLCV adapter lane:

- A Kraken `/0/public/OHLC` parser/model
  (`mqk_md::providers::kraken`) that rejects malformed/unsafe data and
  derives `is_complete` from `row.time <= result.last` (never a fabricated
  flag) and `end_ts = row.time + interval_seconds` (Kraken's `time` is the
  bar **start**).
- An explicit, tested volume-scaling convention: Kraken's fractional
  base-asset `volume` string is scaled by `1e8` into `ProviderBar.volume:
  i64` (e.g. `"1317.15941434"` -> `131715941434`) — not whole coins, not
  quote-currency (USD) volume. Overflow and >8 fractional digits are
  rejected, never truncated or wrapped.
- A `KrakenHistoricalProvider` implementing `HistoricalProvider`, wrapped by
  the existing adapter, plus a fourth `provider_registry.rs` factory arm
  (`"kraken"`) reached only when `providers.json`'s `kraken` entry is
  `enabled: true` — the committed registry keeps it `false`.
- Disabled `kraken_pair`/`kraken_result_key` registry-v2 aliases on the
  existing `BTC/USD`/`ETH/USD` fixture rows (still `enabled: false`,
  `paper_trading_enabled: false`, `live_trading_enabled: false`).
- A read-only `mqk md kraken-ohlc-dry-run` CLI command, fixture-first by
  default:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- md kraken-ohlc-dry-run `
  --registry .\config\instruments\instruments_v2.crypto_local_marks.example.json `
  --symbol BTC/USD `
  --timeframe 1D `
  --input-file .\core-rs\crates\mqk-md\tests\fixtures\kraken_ohlc_xbtusd_1d.json `
  --output-dir .\exports\market_data
```

A live network call is only attempted when no `--input-file` is given and
`MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` is explicitly set — this is not run by
default validation. See
`docs/specs/crypto_data_01u_v_w_kraken_ohlcv_adapter_parser_cli.md` for the
full decision record. At the time this bundle landed, a DB-backed ingest
proof was deliberately deferred (no ingest command wired `"kraken"` into
`ingest-provider`/`sync-provider` yet) — that gap is now closed by
`CRYPTO-DATA-01X-Y` below.

**No recurring ingestion, no scheduler, no daemon job, no GUI surface, and
no crypto trading enablement.** Local CSV import (this runbook) remains the
only DB-backed crypto `md_bars` ingest path proven **before** `01X-Y`.

## Kraken Ingest-Provider DB Proof (CRYPTO-DATA-01X-Y-KRAKEN-INGEST-PROVIDER-DB-PROOF-BUNDLE-01-COMBINED)

Continuing after `01U-V-W`'s parser/dry-run, this bundle wired the same
fixture-proven Kraken parser into the canonical `md_bars` DB write path via
a new, additive `mqk md kraken-ohlc-ingest` command — **not** a change to
`ingest-provider`/`sync-provider`, which remain hard-locked to
`"twelvedata"|"alpaca"` exactly as before (see
`docs/specs/crypto_data_01x_y_kraken_ingest_provider_db_proof.md` §1 for
the full "why a new command instead" rationale).

```powershell
$env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5434/mqk_test?sslmode=disable"

cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- md kraken-ohlc-ingest `
  --registry .\config\instruments\instruments_v2.crypto_local_marks.example.json `
  --symbol BTC/USD `
  --timeframe 1D `
  --input-file .\core-rs\crates\mqk-md\tests\fixtures\kraken_ohlc_xbtusd_1d.json `
  --output-dir .\exports\market_data
```

Same fail-closed gate as `kraken-ohlc-dry-run`: without `--input-file`, the
command refuses before any DB connection unless
`MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` is explicitly set. The forming
(not-yet-committed) Kraken row is never written to `md_bars`. Rows are
stamped with truthful `provider_id="kraken"`, `provider_source="kraken"`,
`provider_symbol=<Kraken's own result key, e.g. "XXBTZUSD">`,
`ingest_mode="provider_ingest"`. No DB migration was needed.

DB-backed proof:
`core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_ingest_db_01xy.rs`
proves completed-bar DB writes, forming-row exclusion, exact scaled-volume
readback, idempotent re-runs, zero `oms_outbox` side effects, and
zero-leftover cleanup (deleting only `provider_id='kraken'` rows at the
exact fixture keys — it can never touch pre-existing non-Kraken history).

**No recurring ingestion, no scheduler, no daemon job, no GUI surface, and
no crypto trading enablement.** `kraken-ohlc-ingest` is a single explicit
operator invocation, proven safe with fixture data only.

## Kraken Incremental Sync DB Proof (CRYPTO-DATA-01Z-AA-KRAKEN-INCREMENTAL-SYNC-DB-PROOF-BUNDLE-01-COMBINED)

Continuing after `01X-Y`'s one-shot ingest, this bundle added a
Kraken-specific incremental sync command that inspects existing `md_bars`
state before writing:

```powershell
$env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5434/mqk_test?sslmode=disable"

cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- md kraken-ohlc-sync `
  --registry .\config\instruments\instruments_v2.crypto_local_marks.example.json `
  --symbol BTC/USD `
  --timeframe 1D `
  --input-file .\core-rs\crates\mqk-md\tests\fixtures\kraken_ohlc_xbtusd_1d.json `
  --output-dir .\exports\market_data
```

Same fail-closed gate as `kraken-ohlc-ingest`, with its own named reason:
without `--input-file`, the command refuses before any DB connection unless
`MQK_ALLOW_KRAKEN_NETWORK_SMOKE=1` is explicitly set
(`kraken_sync_requires_input_file_or_network_opt_in`). Before writing, it
reads the pre-sync latest stored `end_ts` for the symbol/timeframe (via the
existing, unmodified `mqk_db::md::latest_stored_bar_end_ts` helper — no
`mqk-db` source change) and classifies each completed candidate bar as
missing/new or an existing candidate by `end_ts` presence. Rows are stamped
`ingest_mode="provider_sync"` (distinct from `kraken-ohlc-ingest`'s
`"provider_ingest"`) so DB history can be traced to which command wrote it.

`01Z-AA`'s "existing" classification was presence-only (by `end_ts` relative
to the pre-sync high-water mark), because the reused write helper had no
row-content-diff "skip if unchanged" capability — the default policy
upserted every completed bar unconditionally. That gap is now closed; see the
next section.

## Kraken Content-Diff Sync (CRYPTO-DATA-01AB-AC-KRAKEN-CONTENT-DIFF-SYNC-BUNDLE-01-COMBINED)

`kraken-ohlc-sync` now reads existing `md_bars` rows for the exact candidate
`end_ts` keys (`mqk_db::md::fetch_md_bars_for_provider_sync_keys`, a new
read-only helper) and compares OHLCV/`is_complete`/provider provenance
(`mqk_db::md::provider_bar_matches_existing`) before deciding to write. Every
completed (non-forming) bar is classified as:

- **missing/new** — no existing row for that `end_ts` — always written.
- **changed** — existing row present, content differs — written unless
  `--no-update-existing` is passed, in which case it is counted as
  `rows_changed_skipped_due_to_no_update_existing` and never written.
- **unchanged** — existing row present, content identical — never written,
  counted as `rows_skipped_unchanged`, regardless of the flag.

If no candidate bar requires a write, the upsert helper is not called at all
(`md_bars_write=false`, `rows_inserted=0`, `rows_updated=0`). `sync_policy`
is now the single fixed string
`content_diff_skip_unchanged_update_changed_insert_missing`. Evidence bumped
to `schema_version="kraken-ohlc-sync-v2"` with new `rows_changed` /
`rows_skipped_unchanged` / `rows_changed_skipped_due_to_no_update_existing`
fields.

DB-backed proof:
`core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_sync_db_01zaa.rs`
(updated for the new semantics) and the new
`core-rs/crates/mqk-cli/tests/scenario_cli_kraken_ohlc_content_diff_sync_db_01abac.rs`
prove missing-row insert, changed-row update, true unchanged-row skip (no
write-helper call), `--no-update-existing`'s changed-vs-unchanged
distinction with the stale value provably left in place, forming-row
exclusion, exact scaled-volume readback, true idempotency, zero `oms_outbox`
side effects, and zero-leftover cleanup for both BTC/USD and ETH/USD. See
`docs/specs/crypto_data_01ab_ac_kraken_content_diff_sync.md` for full detail.

`sync-provider`/`ingest-provider` remain untouched and TwelveData/
Alpaca-only. **No recurring ingestion, no scheduler, no daemon job, no GUI
surface, and no crypto trading enablement.**

## Kraken OHLC Evidence Status Route (CRYPTO-DATA-01AD-KRAKEN-SYNC-EVIDENCE-STATUS-ROUTE-01)

Read-only operator visibility for Kraken ingest/sync evidence, mirroring the
existing `latest-marks/status` pattern:

```text
GET /api/v1/market-data/kraken-ohlc/status
```

Reads the latest `kraken_ohlc_ingest_*.json` or `kraken_ohlc_sync_*.json`
evidence file (selected by the epoch-seconds timestamp embedded in the
filename, not alphabetical order) from the same evidence directory as
`latest-marks/status`/`intraday-refresh/status`. Never connects to a DB,
never calls Kraken, never runs the CLI, never triggers a sync, never
mutates trading state, never stages evidence. `truth_state`: `"active"`,
`"stale"`, `"no_evidence"`, `"parse_error"`, `"unsafe_evidence"`,
`"backend_unavailable"` — with fail-closed safety checks (wrong provider,
an unexplained network call, internally inconsistent write claims, missing
required provenance fields, execution-like fields) that can surface
`"unsafe_evidence"` even for content this route's own producer wrote
correctly. See `docs/specs/crypto_data_01ad_kraken_sync_evidence_status_route.md`
for full detail.

**This is data-ingestion visibility only.** It is not scheduling, not
strategy input, not broker execution, and not crypto trading enablement.

## Kraken OHLCV Sync Status Panel (CRYPTO-DATA-01AE-KRAKEN-SYNC-GUI-STATUS-SURFACE-01)

Read-only GUI panel ("Kraken OHLCV sync status") on the Ingest screen
consuming `GET /api/v1/market-data/kraken-ohlc/status`, mirroring the
existing "Crypto latest marks" panel. Displays `truth_state`, `provider`,
`latest_mode` (ingest/sync), `produced_at_utc`, `evidence_path`, staleness,
`network_call_made`/`db_write`/`md_bars_write`, provider provenance
(`provider_id`/`provider_source`/`provider_symbol`/`ingest_mode`),
`sync_policy`, symbols requested, bar/row counts including the `01AB-AC`
content-diff fields (`rows_changed`, `rows_skipped_unchanged`,
`rows_changed_skipped_due_to_no_update_existing`, `rows_inserted`,
`rows_updated`), latest end_ts fields, volume semantics/scale, and
`fail_reasons`. `unsafe_evidence` is rendered as a critical fail-closed
notice, never as usable data.

The panel carries a fixed warning:
*"Kraken OHLCV evidence is data-ingestion visibility only. It is not
scheduling, not strategy input by itself, not broker execution, and not
crypto trading enablement."*

The panel never triggers a Kraken sync, never runs the CLI, never calls a
provider, never starts a daemon job, and has no scheduler button. Read-only
in every respect — a "Refresh" button re-fetches the same GET route only.

Implemented in `core-rs/mqk-gui/src/features/ingest/{types.ts,api.ts,IngestScreen.tsx}`,
tested in `core-rs/mqk-gui/src/features/ingest/__tests__/api.test.ts`.

## Kraken Data Registry Cutover Decision (CRYPTO-REGISTRY-02-KRAKEN-DATA-REGISTRY-CUTOVER-DECISION-01)

Decision-only patch. Answers what "production registry-v2 cutover" means for
Kraken-sourced `BTC/USD`/`ETH/USD` data after the fixture/DB/sync/status/GUI
lane above closed: the registry-v2 schema structurally distinguishes data
tracking (`enabled`) from trading enablement (`paper_trading_enabled`/
`live_trading_enabled`), but `validate_registry_v2` fail-closed blocks
`enabled=true` for non-equity instruments without the test-only
`allow_enabled_non_equity_for_testing` flag — so no config flag is flipped by
this decision. `kraken.enabled` stays `false`; both crypto rows stay
`enabled=false`/`paper_trading_enabled=false`/`live_trading_enabled=false`,
unchanged. Full detail:
`docs/specs/crypto_registry_02_kraken_data_registry_cutover_decision.md`.

## Crypto Registry Readiness CLI (CRYPTO-REGISTRY-03-KRAKEN-DATA-ONLY-REGISTRY-READINESS-CLI-01)

Read-only operator readiness command:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- md crypto-registry-readiness `
  --registry .\config\instruments\instruments_v2.crypto_local_marks.example.json `
  --providers .\config\providers\providers.json `
  --provider kraken `
  --symbols BTC/USD,ETH/USD
```

Reads `--registry`/`--providers` (never mutated) and classifies whether the
current configs are ready for data-only Kraken OHLCV operations: provider
exists, `kraken.enabled=false` (an unexpected `enabled=true` fails closed as
`unsafe_provider_enabled`, per `CRYPTO-REGISTRY-02`'s decision), both
`BTC/USD`/`ETH/USD` rows are `asset_class=crypto` with complete
`kraken_pair`/`kraken_result_key` aliases, and `paper_trading_enabled`/
`live_trading_enabled` are both `false` (either being `true` fails closed as
`unsafe_trading_enabled`). The current disabled state classifies as
`data_readiness_state=data_ready_manual_only` — expected and correct, not a
failure. Never opens a DB connection, never calls a provider/network
endpoint, never writes `md_bars`, never registers a scheduler. `--output-dir`
writes a `crypto-registry-readiness-v1` JSON evidence artifact.

Implemented in `core-rs/crates/mqk-cli/src/commands/md.rs::md_crypto_registry_readiness`,
tested in `core-rs/crates/mqk-cli/tests/scenario_cli_crypto_registry_readiness_03.rs`.

## Crypto Registry Readiness Status Route + GUI Panel (CRYPTO-REGISTRY-04-KRAKEN-DATA-ONLY-REGISTRY-STATUS-SURFACE-01)

Read-only daemon route re-exposing the same classification as the
`CRYPTO-REGISTRY-03` CLI:

```
GET /api/v1/market-data/crypto-registry/readiness
```

Reads `MQK_INSTRUMENT_REGISTRY_V2_PATH` (falling back to the committed
`instruments_v2.crypto_local_marks.example.json` fixture when unset) and
`MQK_PROVIDER_REGISTRY_PATH`/its `config/providers/providers.json` default —
neither is ever mutated. Returns the same `truth_state` values as the CLI
(`active`, `missing_provider`, `missing_symbol`, `missing_alias`,
`unsafe_trading_enabled`, `unsafe_provider_enabled`, `parse_error`). No DB
connection, no provider/network call, no CLI subprocess, no scheduler.

A read-only "Crypto registry readiness" GUI panel on the Ingest screen
(next to the Kraken OHLCV sync status panel) displays: provider, data/
trading/scheduler readiness states, provider enabled flag,
`BTC/USD`/`ETH/USD` alias status, paper/live trading flags, and fail
reasons. Fixed warning: *"Registry readiness is data-pipeline visibility
only. It does not enable crypto trading, broker routing, strategy
execution, or scheduling."* No button mutates config or triggers a sync —
"Refresh" only re-issues the same read-only GET.

Implemented in `core-rs/crates/mqk-daemon/src/routes/transport_quality.rs::crypto_registry_readiness`,
`core-rs/crates/mqk-daemon/src/api_types.rs::CryptoRegistryReadinessResponse`,
`core-rs/mqk-gui/src/features/ingest/{types.ts,api.ts,IngestScreen.tsx}`.
Tested in `core-rs/crates/mqk-daemon/tests/scenario_crypto_registry_readiness_route_04.rs`
and `core-rs/mqk-gui/src/features/ingest/__tests__/api.test.ts`.

## Kraken Scheduler Rate-Limit Decision (CRYPTO-DATA-02A-KRAKEN-SCHEDULER-RATE-LIMIT-DECISION-01)

Decision-only patch, continuing after `CRYPTO-REGISTRY-04`. Verified Kraken's
public-endpoint rate-limit guidance via 2 bounded, keyless documentation-page
reads (no Kraken **API** call): Kraken's official support article states
public endpoints may be called at up to 1 request/second and remain within
limits, with OHLC/Trades specifically rate-limited by IP address **and**
currency pair (other public endpoints by IP only), and exceeding the limit
triggers a temporary throttle. Based on this, the decision records a
conservative, repo-local policy for a **future** scheduled Kraken sync — no
scheduler is registered by this patch: daily cadence, sequential-only pair
calls (never concurrent), at least 2 seconds between per-pair calls (double
the verified guideline), at most 2 OHLC calls per run (`BTC/USD` + `ETH/USD`),
bounded exponential-backoff retries (max 2, with jitter, fail-closed once
exhausted), and an explicit list of invariants a future task-registration
patch must satisfy first. `kraken.enabled` and both crypto rows' trading
flags remain unchanged (`false`). Full detail, exact quotes, and source URLs:
`docs/specs/crypto_data_02a_kraken_scheduler_rate_limit_decision.md`.

## Kraken Scheduler Readiness CLI (CRYPTO-DATA-02B-KRAKEN-SCHEDULER-READINESS-CLI-01)

Read-only operator readiness command proving whether a **future**, not-yet-
authorized Kraken scheduled sync is currently allowed by the
`CRYPTO-DATA-02A` policy, the current provider/registry config, and
(optionally) the latest Kraken OHLC evidence:

```powershell
cargo run --manifest-path .\core-rs\Cargo.toml -p mqk-cli --bin mqk-cli -- md kraken-scheduler-readiness `
  --policy .\docs\specs\crypto_data_02a_kraken_scheduler_rate_limit_decision.json `
  --registry .\config\instruments\instruments_v2.crypto_local_marks.example.json `
  --providers .\config\providers\providers.json `
  --symbols BTC/USD,ETH/USD
```

`active` (`scheduler_readiness_state=scheduler_ready_manual_registration_blocked`)
does **not** mean a scheduler is registered — it means every prerequisite
this command can check (policy contract validity, provider disabled,
registry aliases present, trading flags false, no evidence of an already-
registered scheduler) is satisfied for a future, separately authorized
scheduler-registration patch to be considered. Truth states: `active`,
`policy_missing`, `policy_invalid`, `registry_unsafe`, `provider_unsafe`,
`trading_flags_unsafe`, `scheduler_already_registered`, `evidence_unsafe`,
`parse_error`, `backend_unavailable`. Kraken OHLC evidence
(`--evidence-dir`) is optional and only fails closed
(`evidence_readiness_state=unsafe`/`stale`/`missing`) when
`--require-fresh-evidence` is explicitly passed; otherwise missing/unsafe
evidence is a warning only. Never opens a DB connection, never calls
Kraken or any provider/network endpoint, never mutates
`--policy`/`--registry`/`--providers`, never registers a scheduler, never
adds a daemon job.

Implemented in `core-rs/crates/mqk-cli/src/commands/md.rs::md_kraken_scheduler_readiness`,
tested in `core-rs/crates/mqk-cli/tests/scenario_cli_kraken_scheduler_readiness_02b.rs`.

## Remaining Gaps

- `sync-provider` (incremental backfill) still has no Kraken path — an
  explicit deferral, not an oversight (see
  `docs/specs/crypto_data_01x_y_kraken_ingest_provider_db_proof.md` §5). A
  Kraken-specific `kraken-ohlc-sync` command now exists instead (see above).
- No recurring/scheduled Kraken sync of any kind; no daemon ingest job.
- CoinLore's verified public endpoints are ticker/spot-only, not OHLCV; a
  `LatestMark` parser/model and a read-only evidence-file status route now
  exist for that ticker data, but no `latest_marks` DB table exists — the
  route is evidence-file-only, not backed by persisted/queryable storage.
- Read-only GUI surfaces for crypto latest marks and Kraken OHLCV sync
  status now both exist (see above); no GUI action can trigger a
  provider/network/CLI/scheduler call for either.
- No production registry-v2 cutover (registry-v2 still has zero production
  route callers for the default/legacy config); `CRYPTO-REGISTRY-02` records
  the decision not to flip `enabled` yet (see above).
- This does not enable crypto trading, paper trading, or live trading.
