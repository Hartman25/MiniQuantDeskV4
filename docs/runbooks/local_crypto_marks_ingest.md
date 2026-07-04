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

## Remaining Gaps

- No `ETH/USD` fixture or registry-v2 entry exists yet.
- No live network crypto provider is implemented or verified.
- This does not enable crypto trading, paper trading, or live trading.
