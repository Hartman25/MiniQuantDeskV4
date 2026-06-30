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

The committed fixture for testing is:
`core-rs/crates/mqk-md/tests/fixtures/crypto_btcusd_1d_local.csv`

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

## Remaining Gaps (not addressed by this runbook)

- `provider_id` in `md_bars` rows will be `"unknown"` (the CLI's CSV path does
  not yet call `ingest_provider_bars_to_md_bars_with_provider_metadata`).
  This does not affect valuation — the mark-read path never queries `provider_id`.
- No `ETH/USD` fixture or registry-v2 entry exists yet.
- No Windows Scheduled Task registration (planned as CRYPTO-DATA-01E).
- No live network crypto provider is implemented or verified.
- This does not enable crypto trading, paper trading, or live trading.
