# =============================================================================
# CRYPTO-DATA-01D-EXPLICIT-LOCAL-CRYPTO-INGEST-RUNNER-01
# Import-LocalCryptoMarks.ps1
#
# Explicit, default-off, operator-run local crypto CSV import runner for
# MiniQuantDesk V4.  Wraps the existing `mqk-cli md ingest-csv` command
# (proven by CRYPTO-DATA-01A/01B) behind fail-closed checks and evidence
# output so an operator can intentionally import a local BTC/USD (or other
# crypto) daily CSV into md_bars.
#
# No network calls.  No daemon start.  No order/broker/runtime path.
# No scheduler registration.  No crypto trading enablement.
#
# Usage:
#   # Check-only (default; read-only; never calls cargo):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Import-LocalCryptoMarks.ps1 `
#       -CheckOnly -CsvPath .\core-rs\crates\mqk-md\tests\fixtures\crypto_btcusd_1d_local.csv
#
#   # Import once (requires MQK_DATABASE_URL targeting paper DB at port 5440):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Import-LocalCryptoMarks.ps1 `
#       -Once -CsvPath .\path\to\btcusd_1d_local.csv
#
# Parameters:
#   -CsvPath                 Path to local crypto CSV file.  (Required)
#   -Timeframe               Bar timeframe.  Default: 1D
#   -Source                  Source label for md_quality_reports.  Default: local_crypto_manual
#   -OutputDir               Evidence output directory.  Default: .\exports\market_data
#   -MaxAgeHours             Maximum hours since latest completed bar before stale.  Default: 36
#   -AllowStaleForValidation Bypass staleness gate for read-only validation runs against
#                            committed historical fixtures.  Evidence records the override.
#   -CheckOnly               Read-only mode: CSV checks + evidence, no mutation.  Default.
#   -Once                    Import mode: CSV checks + DB guard + ingest + evidence.
#
# Hard rules:
#   - -CheckOnly never calls cargo, mqk-cli, provider scripts, daemon routes, or DB writes.
#   - -Once requires MQK_DATABASE_URL containing port 5440 and miniquantdesk_paper.
#   - No network calls, no daemon start, no order/broker/runtime behavior.
#   - Fails closed: missing/empty/stale CSV -> passed=false, nonzero exit.
#   - Evidence is always written with explicit all_passed/reason_code fields.
# =============================================================================

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $CsvPath,
    [string] $Timeframe               = '1D',
    [string] $Source                  = 'local_crypto_manual',
    [string] $OutputDir               = '',
    [int]    $MaxAgeHours             = 36,
    [switch] $AllowStaleForValidation,
    [switch] $CheckOnly,
    [switch] $Once
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
$PATCH_ID         = 'CRYPTO-DATA-01D-EXPLICIT-LOCAL-CRYPTO-INGEST-RUNNER-01-COMBINED'
$SCHEMA_VERSION   = 'local-crypto-import-v1'
$REQUIRED_COLUMNS = @('symbol','timeframe','end_ts','open','high','low','close','volume','is_complete')

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
function Write-Step { param([string]$M) Write-Host "[CRYPTO-IMPORT] $M"       -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[CRYPTO-IMPORT] OK: $M"   -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[CRYPTO-IMPORT] WARN: $M" -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[CRYPTO-IMPORT] FAIL: $M" -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Mode validation
# ---------------------------------------------------------------------------
if ($CheckOnly -and $Once) {
    Write-Fail '-CheckOnly and -Once are mutually exclusive. Provide exactly one.'
    exit 1
}

if (-not $CheckOnly -and -not $Once) {
    Write-Step 'No mode flag supplied. Defaulting to -CheckOnly (safe read-only mode).'
    $CheckOnly = $true
}

$effectiveMode = if ($Once.IsPresent) { 'once' } else { 'check_only' }
Write-Sect "Import-LocalCryptoMarks  mode=$effectiveMode"

# ---------------------------------------------------------------------------
# Resolve repo root and output directory
# ---------------------------------------------------------------------------
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $RepoRoot 'exports\market_data'
}

try { New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null } catch {}
Write-Step "Evidence output: $OutputDir"

# ---------------------------------------------------------------------------
# Resolve CSV path to absolute before any Push-Location changes cwd
# ---------------------------------------------------------------------------
$AbsCsvPath = [System.IO.Path]::GetFullPath(
    [System.IO.Path]::Combine((Get-Location).Path, $CsvPath)
)
Write-Step "CSV path (resolved): $AbsCsvPath"

# ---------------------------------------------------------------------------
# CSV checks  (run in both -CheckOnly and before -Once mutation)
# ---------------------------------------------------------------------------
$failReasons   = [System.Collections.Generic.List[string]]::new()
$csvExists     = $false
$headerPresent = $false
$columnsOk     = $false
$rowsSeen      = 0
$latestEndTs   = $null
$latestAgeSecs = $null
$isStale       = $false
$symbolsSeen   = @()

Write-Sect 'CSV checks'

# [C1] File exists
if (Test-Path -LiteralPath $AbsCsvPath) {
    $csvExists = $true
    Write-Ok "CSV file exists: $AbsCsvPath"
} else {
    Write-Fail "CSV file not found: $AbsCsvPath"
    $failReasons.Add("csv_file_not_found: $AbsCsvPath")
}

if ($csvExists) {
    $lines    = [System.IO.File]::ReadAllLines($AbsCsvPath)
    $nonEmpty = @($lines | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })

    # [C2] Header row present
    if ($nonEmpty.Count -ge 1) {
        $headerLine    = $nonEmpty[0]
        $headerPresent = $true
        Write-Ok "Header row present: $headerLine"
    } else {
        Write-Fail 'CSV is empty (no header row).'
        $failReasons.Add('csv_empty_no_header')
    }

    if ($headerPresent) {
        $headers = @($headerLine -split ',' | ForEach-Object { $_.Trim() })

        # [C3] Required columns
        $missing = @($REQUIRED_COLUMNS | Where-Object { $_ -notin $headers })
        if ($missing.Count -eq 0) {
            $columnsOk = $true
            Write-Ok "All required columns present: $($REQUIRED_COLUMNS -join ', ')"
        } else {
            Write-Fail "Missing required columns: $($missing -join ', ')"
            $failReasons.Add("missing_columns: $($missing -join ', ')")
        }

        # [C4] Data rows
        $dataLines = @($nonEmpty | Select-Object -Skip 1)
        $rowsSeen  = $dataLines.Count

        if ($rowsSeen -eq 0) {
            Write-Fail 'CSV has header but no data rows.'
            $failReasons.Add('no_data_rows')
        } else {
            Write-Ok "Data rows seen: $rowsSeen"
        }

        # [C5] Latest end_ts and staleness
        if ($columnsOk -and $rowsSeen -gt 0) {
            $endTsIdx  = [Array]::IndexOf($headers, 'end_ts')
            $symbolIdx = [Array]::IndexOf($headers, 'symbol')

            $maxEndTs = [long]0
            $hasEndTs = $false
            $localSymSet = [System.Collections.Generic.HashSet[string]]::new()

            foreach ($dl in $dataLines) {
                $cols = $dl -split ','
                if ($cols.Count -gt $endTsIdx) {
                    $tsRaw = $cols[$endTsIdx].Trim()
                    $tsVal = [long]0
                    if ([long]::TryParse($tsRaw, [ref]$tsVal)) {
                        if (-not $hasEndTs -or $tsVal -gt $maxEndTs) {
                            $maxEndTs = $tsVal
                            $hasEndTs = $true
                        }
                    }
                }
                if ($symbolIdx -ge 0 -and $cols.Count -gt $symbolIdx) {
                    $sym = $cols[$symbolIdx].Trim()
                    if (-not [string]::IsNullOrWhiteSpace($sym)) {
                        $null = $localSymSet.Add($sym)
                    }
                }
            }

            $symbolsSeen = @($localSymSet)
            Write-Step "Symbols seen in data: $($symbolsSeen -join ', ')"

            if ($hasEndTs) {
                $latestEndTs   = $maxEndTs
                $nowUnix       = [long][DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
                $latestAgeSecs = [long]($nowUnix - $maxEndTs)
                $maxAgeSecs    = [long]$MaxAgeHours * 3600
                $isStale       = ($latestAgeSecs -gt $maxAgeSecs)

                Write-Ok ("Latest completed bar end_ts: $latestEndTs" +
                          "  age: ${latestAgeSecs}s  threshold: ${maxAgeSecs}s  stale: $isStale")

                if ($isStale) {
                    if ($AllowStaleForValidation) {
                        Write-Warn ("Staleness gate: stale (${latestAgeSecs}s > ${maxAgeSecs}s). " +
                                   "-AllowStaleForValidation set -- gate bypassed for this validation run. " +
                                   "Evidence will record allow_stale_for_validation=true.")
                    } else {
                        Write-Fail ("Staleness gate: latest bar age ${latestAgeSecs}s exceeds " +
                                   "MaxAgeHours=${MaxAgeHours} (${maxAgeSecs}s). " +
                                   "Pass -AllowStaleForValidation to bypass for deterministic fixture testing.")
                        $failReasons.Add("latest_completed_bar_stale: age=${latestAgeSecs}s max=${maxAgeSecs}s MaxAgeHours=${MaxAgeHours}")
                    }
                } else {
                    Write-Ok "Staleness gate: OK (${latestAgeSecs}s <= ${maxAgeSecs}s)"
                }
            } else {
                Write-Warn 'Could not parse end_ts from any data row. Staleness check skipped.'
                $failReasons.Add('end_ts_unparseable')
            }
        }
    }
}

$csvChecksPassed = $csvExists -and $headerPresent -and $columnsOk -and ($rowsSeen -gt 0) -and
                  ($null -ne $latestEndTs) -and (-not $isStale -or $AllowStaleForValidation.IsPresent)

if ($csvChecksPassed) {
    Write-Ok 'CSV checks: PASS'
} else {
    Write-Fail 'CSV checks: FAIL'
}

$checksObj = [pscustomobject]@{
    csv_file_exists             = $csvExists
    header_present              = $headerPresent
    required_columns_present    = $columnsOk
    rows_seen                   = $rowsSeen
    symbols_seen                = $symbolsSeen
    latest_completed_bar_ts     = $latestEndTs
    latest_completed_bar_age_secs = $latestAgeSecs
    stale                       = $isStale
    allow_stale_for_validation  = $AllowStaleForValidation.IsPresent
    passed                      = $csvChecksPassed
}

# ---------------------------------------------------------------------------
# DB guard  (only in -Once mode)
# ---------------------------------------------------------------------------
$dbGuardChecked = $false
$dbGuardPassed  = $false
$dbGuardReason  = 'not_applicable_check_only'

$mutationAttempted = $false
$mutationCommand   = $null
$mutationExitCode  = $null
$mutationSucceeded = $false

if ($Once.IsPresent) {
    Write-Sect 'DB guard'
    $dbGuardChecked = $true

    $dbUrl = $env:MQK_DATABASE_URL
    if ([string]::IsNullOrWhiteSpace($dbUrl)) {
        Write-Fail 'MQK_DATABASE_URL is not set. Set it to the paper DB URL before running -Once.'
        Write-Fail 'Example: $env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable"'
        $dbGuardReason = 'database_url_missing'
        $failReasons.Add('database_url_missing')
    } elseif ($dbUrl -notmatch '5440') {
        Write-Fail 'MQK_DATABASE_URL does not contain port 5440 (paper DB marker). Refusing mutation.'
        $dbGuardReason = 'non_paper_database_target_refused'
        $failReasons.Add('non_paper_database_target_refused: port 5440 absent from MQK_DATABASE_URL')
    } elseif ($dbUrl -notmatch 'miniquantdesk_paper') {
        Write-Fail 'MQK_DATABASE_URL does not contain "miniquantdesk_paper". Refusing mutation.'
        $dbGuardReason = 'non_paper_database_target_refused'
        $failReasons.Add('non_paper_database_target_refused: miniquantdesk_paper absent from MQK_DATABASE_URL')
    } else {
        $dbGuardPassed = $true
        $dbGuardReason = 'paper_db_confirmed'
        Write-Ok 'DB guard passed: MQK_DATABASE_URL contains port 5440 and miniquantdesk_paper.'
    }
}

$dbGuardObj = [pscustomobject]@{
    checked     = $dbGuardChecked
    passed      = $dbGuardPassed
    reason_code = $dbGuardReason
}

# ---------------------------------------------------------------------------
# Mutation  (only in -Once, only if CSV checks and DB guard both pass)
# ---------------------------------------------------------------------------
if ($Once.IsPresent) {
    if ($csvChecksPassed -and $dbGuardPassed) {
        Write-Sect 'Ingest (mqk-cli md ingest-csv)'

        $coreRs    = Join-Path $RepoRoot 'core-rs'
        $cmdString = "cargo run -p mqk-cli --bin mqk-cli -- md ingest-csv --path `"$AbsCsvPath`" --timeframe $Timeframe --source $Source"
        Write-Step "Command: $cmdString"
        Write-Step "Working dir: $coreRs"

        $mutationAttempted = $true
        $mutationCommand   = $cmdString

        Push-Location $coreRs
        try {
            $local:ErrorActionPreference = 'Continue'
            $cliOutput = cargo run -p mqk-cli --bin mqk-cli -- md ingest-csv `
                --path $AbsCsvPath `
                --timeframe $Timeframe `
                --source $Source 2>&1
            $mutationExitCode = $LASTEXITCODE
            $cliOutput | Out-Host

            if ($mutationExitCode -eq 0) {
                $mutationSucceeded = $true
                Write-Ok "mqk-cli md ingest-csv completed (exit 0)."
            } else {
                Write-Fail "mqk-cli md ingest-csv failed (exit $mutationExitCode)."
                $failReasons.Add("cli_ingest_failed: exit $mutationExitCode")
            }
        } catch {
            Write-Fail "mqk-cli md ingest-csv threw: $_"
            $failReasons.Add("cli_ingest_exception: $_")
            if ($null -eq $mutationExitCode) { $mutationExitCode = -1 }
        } finally {
            Pop-Location
        }
    } else {
        Write-Sect 'Ingest skipped'
        if (-not $csvChecksPassed) { Write-Warn 'Skipping ingest: CSV checks did not pass.' }
        if (-not $dbGuardPassed)   { Write-Warn 'Skipping ingest: DB guard did not pass.' }
    }
}

$mutationObj = [pscustomobject]@{
    attempted  = $mutationAttempted
    command    = $mutationCommand
    exit_code  = $mutationExitCode
    succeeded  = $mutationSucceeded
}

# ---------------------------------------------------------------------------
# Overall pass/fail
# ---------------------------------------------------------------------------
$allPassed  = $false
$reasonCode = 'checks_passed'

if ($Once.IsPresent) {
    $allPassed = $csvChecksPassed -and $dbGuardPassed -and $mutationSucceeded
    if (-not $csvChecksPassed)  { $reasonCode = 'csv_checks_failed' }
    elseif (-not $dbGuardPassed) { $reasonCode = 'db_guard_failed' }
    elseif ($mutationAttempted -and -not $mutationSucceeded) { $reasonCode = 'cli_ingest_failed' }
    elseif ($allPassed) { $reasonCode = 'import_succeeded' }
} else {
    $allPassed = $csvChecksPassed
    if (-not $csvChecksPassed) { $reasonCode = 'csv_checks_failed' }
}

# ---------------------------------------------------------------------------
# Write evidence JSON and TXT
# ---------------------------------------------------------------------------
$stamp      = Get-Date -Format 'yyyyMMdd_HHmmss'
$evJsonFile = Join-Path $OutputDir "local_crypto_import_${stamp}.json"
$evTxtFile  = Join-Path $OutputDir "local_crypto_import_${stamp}.txt"

$evidence = [pscustomobject]@{
    schema_version             = $SCHEMA_VERSION
    patch_id                   = $PATCH_ID
    produced_at_utc            = (Get-Date).ToUniversalTime().ToString('o')
    mode                       = $effectiveMode
    csv_path                   = $AbsCsvPath
    timeframe                  = $Timeframe
    source                     = $Source
    max_age_hours              = $MaxAgeHours
    allow_stale_for_validation = $AllowStaleForValidation.IsPresent
    checks                     = $checksObj
    db_guard                   = $dbGuardObj
    mutation                   = $mutationObj
    all_passed                 = $allPassed
    reason_code                = $reasonCode
    fail_reasons               = @($failReasons)
}

try {
    $evidence | ConvertTo-Json -Depth 6 | Set-Content -Path $evJsonFile -Encoding ASCII
    Write-Ok "Evidence JSON: $evJsonFile"
} catch {
    Write-Warn "Could not write evidence JSON: $_"
}

$txtLines = @(
    "CRYPTO-DATA-01D Import-LocalCryptoMarks Evidence"
    "================================================="
    "patch_id             : $PATCH_ID"
    "produced_at_utc      : $((Get-Date).ToUniversalTime().ToString('o'))"
    "mode                 : $effectiveMode"
    "csv_path             : $AbsCsvPath"
    "timeframe            : $Timeframe"
    "source               : $Source"
    "max_age_hours        : $MaxAgeHours"
    "allow_stale          : $($AllowStaleForValidation.IsPresent)"
    "all_passed           : $allPassed"
    "reason_code          : $reasonCode"
    "fail_reasons         : $($failReasons -join '; ')"
    ""
    "checks:"
    "  csv_file_exists            : $csvExists"
    "  header_present             : $headerPresent"
    "  required_columns_present   : $columnsOk"
    "  rows_seen                  : $rowsSeen"
    "  symbols_seen               : $($symbolsSeen -join ', ')"
    "  latest_completed_bar_ts    : $latestEndTs"
    "  latest_completed_bar_age_s : $latestAgeSecs"
    "  stale                      : $isStale"
    "  passed                     : $csvChecksPassed"
    ""
    "db_guard:"
    "  checked     : $dbGuardChecked"
    "  passed      : $dbGuardPassed"
    "  reason_code : $dbGuardReason"
    ""
    "mutation:"
    "  attempted   : $mutationAttempted"
    "  command     : $mutationCommand"
    "  exit_code   : $mutationExitCode"
    "  succeeded   : $mutationSucceeded"
)

try {
    $txtLines | Set-Content -Path $evTxtFile -Encoding UTF8
    Write-Ok "Evidence TXT: $evTxtFile"
} catch {
    Write-Warn "Could not write evidence TXT: $_"
}

# ---------------------------------------------------------------------------
# Final verdict
# ---------------------------------------------------------------------------
Write-Sect 'Result'
if ($allPassed) {
    Write-Ok "Import-LocalCryptoMarks: PASSED  mode=$effectiveMode"
    exit 0
} else {
    Write-Fail "Import-LocalCryptoMarks: FAILED  mode=$effectiveMode  reason=$reasonCode"
    Write-Fail "Fail reasons: $($failReasons -join ' | ')"
    exit 1
}
