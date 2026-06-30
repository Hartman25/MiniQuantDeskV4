# =============================================================================
# CRYPTO-DATA-01D -- Import-LocalCryptoMarks.ps1 Validator
# validate_import_local_crypto_marks.ps1
#
# Validates Import-LocalCryptoMarks.ps1 without calling cargo, mutating any
# DB, or making any network, provider, or broker call.
#
# Checks:
#   [1] PowerShell parser: Import-LocalCryptoMarks.ps1 parses with zero errors.
#   [2] -CheckOnly with default MaxAgeHours against committed BTC/USD fixture
#       must fail (fixture is stale) and produce no mutation output.
#   [3] -CheckOnly with -AllowStaleForValidation must pass local-file checks
#       and produce no mutation output.
#   [4] -Once with no MQK_DATABASE_URL must refuse before any mutation.
#   [5] Evidence JSON is written and contains all required keys.
#   [6] Import-LocalCryptoMarks.ps1 text does not reference forbidden
#       network/provider/order/runtime patterns.
#
# No cargo.  No DB connection.  No network call.  No daemon start.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_import_local_crypto_marks.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$ImportScript = Join-Path $RepoRoot 'scripts\windows\Import-LocalCryptoMarks.ps1'
$FixturePath  = Join-Path $RepoRoot 'core-rs\crates\mqk-md\tests\fixtures\crypto_btcusd_1d_local.csv'
$TempDir      = Join-Path ([System.IO.Path]::GetTempPath()) 'mqk_validate_import_crypto'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red    }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green  }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan   }
function Show-Warn  { param([string]$M) Write-Host "  WARN -- $M" -ForegroundColor Yellow }

Write-Host '============================================================'
Write-Host ' CRYPTO-DATA-01D validate_import_local_crypto_marks'
Write-Host " Import script : $ImportScript"
Write-Host " Fixture       : $FixturePath"
Write-Host ' No cargo, no DB, no network calls.'
Write-Host '============================================================'

try { New-Item -ItemType Directory -Force -Path $TempDir | Out-Null } catch {}

# =============================================================================
# [1] PowerShell parser: zero errors
# =============================================================================
Write-Host ''
Show-Info '--- [1] PowerShell parser: zero errors ---'

if (-not (Test-Path -LiteralPath $ImportScript)) {
    $Violations++
    Show-Fail "Import-LocalCryptoMarks.ps1 not found at $ImportScript"
} else {
    $parseErrors = $null
    $parseTokens = $null
    [System.Management.Automation.Language.Parser]::ParseFile(
        $ImportScript,
        [ref]$parseTokens,
        [ref]$parseErrors
    ) | Out-Null

    if ($null -ne $parseErrors -and $parseErrors.Count -gt 0) {
        $Violations++
        Show-Fail "Parse errors found: $($parseErrors.Count)"
        foreach ($pe in $parseErrors) { Write-Host "    $pe" -ForegroundColor Red }
    } else {
        Show-Green 'Import-LocalCryptoMarks.ps1 parses with zero errors'
    }
}

# =============================================================================
# Helper: run the import script in a subprocess, capture output + exit code
# =============================================================================
function Invoke-ImportScript {
    param([string[]]$Arguments)
    $prevUrl  = $env:MQK_DATABASE_URL
    $env:MQK_DATABASE_URL = ''
    try {
        $local:ErrorActionPreference = 'Continue'
        $out = & powershell -NoProfile -ExecutionPolicy Bypass `
            -File $ImportScript @Arguments 2>&1
        $ec = $LASTEXITCODE
    } catch {
        $out = @("EXCEPTION: $_")
        $ec  = -99
    } finally {
        if ($null -ne $prevUrl -and $prevUrl -ne '') {
            $env:MQK_DATABASE_URL = $prevUrl
        } else {
            try { Remove-Item Env:MQK_DATABASE_URL -ErrorAction SilentlyContinue } catch {}
        }
    }
    return [pscustomobject]@{ Output = $out; ExitCode = $ec }
}

# =============================================================================
# [2] -CheckOnly default staleness: expect FAIL (stale fixture), no mutation
# =============================================================================
Write-Host ''
Show-Info '--- [2] -CheckOnly (stale fixture, default MaxAgeHours): expect FAIL ---'

if (-not (Test-Path -LiteralPath $FixturePath)) {
    $Violations++
    Show-Fail "BTC/USD fixture not found at $FixturePath"
} else {
    $r2 = Invoke-ImportScript @(
        '-CheckOnly',
        '-CsvPath', $FixturePath,
        '-Timeframe', '1D',
        '-Source', 'local_crypto_manual',
        '-OutputDir', $TempDir
    )
    $r2.Output | ForEach-Object { Write-Host "    $_" }

    if ($r2.ExitCode -ne 0) {
        Show-Green "-CheckOnly (stale) exited nonzero ($($r2.ExitCode)) -- correct (stale detected)"
    } else {
        $Violations++
        Show-Fail "-CheckOnly (stale) exited 0 but should fail on stale data"
    }

    $mutationInOutput = @($r2.Output | Where-Object { $_ -match 'cargo|md_ingest_ok|ingest-csv called' }).Count
    if ($mutationInOutput -gt 0) {
        $Violations++
        Show-Fail "-CheckOnly (stale): mutation-related output detected"
    } else {
        Show-Green "-CheckOnly (stale): no mutation output detected (correct)"
    }
}

# =============================================================================
# [3] -CheckOnly -AllowStaleForValidation: expect PASS (stale bypassed)
# =============================================================================
Write-Host ''
Show-Info '--- [3] -CheckOnly -AllowStaleForValidation: expect PASS ---'

if (Test-Path -LiteralPath $FixturePath) {
    $r3 = Invoke-ImportScript @(
        '-CheckOnly',
        '-CsvPath', $FixturePath,
        '-Timeframe', '1D',
        '-Source', 'local_crypto_manual',
        '-OutputDir', $TempDir,
        '-AllowStaleForValidation'
    )
    $r3.Output | ForEach-Object { Write-Host "    $_" }

    if ($r3.ExitCode -eq 0) {
        Show-Green "-CheckOnly -AllowStaleForValidation exited 0 (checks passed)"
    } else {
        $Violations++
        Show-Fail "-CheckOnly -AllowStaleForValidation exited $($r3.ExitCode) but should pass"
    }

    $mutationInOutput3 = @($r3.Output | Where-Object { $_ -match 'cargo|md_ingest_ok|ingest-csv called' }).Count
    if ($mutationInOutput3 -gt 0) {
        $Violations++
        Show-Fail "-CheckOnly -AllowStaleForValidation: mutation-related output detected"
    } else {
        Show-Green "-CheckOnly -AllowStaleForValidation: no mutation output (correct)"
    }
}

# =============================================================================
# [4] -Once with no MQK_DATABASE_URL: must refuse before mutation
# =============================================================================
Write-Host ''
Show-Info '--- [4] -Once with no MQK_DATABASE_URL: expect FAIL (DB guard) before mutation ---'

if (Test-Path -LiteralPath $FixturePath) {
    $r4 = Invoke-ImportScript @(
        '-Once',
        '-CsvPath', $FixturePath,
        '-Timeframe', '1D',
        '-Source', 'local_crypto_manual',
        '-OutputDir', $TempDir,
        '-AllowStaleForValidation'
    )
    $r4.Output | ForEach-Object { Write-Host "    $_" }

    if ($r4.ExitCode -ne 0) {
        Show-Green "-Once (no DB URL) exited nonzero ($($r4.ExitCode)) -- DB guard correct"
    } else {
        $Violations++
        Show-Fail "-Once (no DB URL) exited 0 but should refuse"
    }

    $cargoInOutput = @($r4.Output | Where-Object { $_ -match 'cargo|md_ingest_ok' }).Count
    if ($cargoInOutput -gt 0) {
        $Violations++
        Show-Fail "-Once (no DB URL): cargo-related output detected -- mutation attempted before guard"
    } else {
        Show-Green "-Once (no DB URL): cargo not called (correct)"
    }

    $guardInOutput = @($r4.Output | Where-Object { $_ -match 'database_url_missing|MQK_DATABASE_URL' }).Count
    if ($guardInOutput -gt 0) {
        Show-Green "-Once (no DB URL): DB guard output present (correct)"
    } else {
        Show-Warn "-Once (no DB URL): expected DB guard message not found in output"
    }
}

# =============================================================================
# [5] Evidence JSON: required keys present
# =============================================================================
Write-Host ''
Show-Info '--- [5] Evidence JSON: required keys present ---'

$evFiles = @(Get-ChildItem -Path $TempDir -Filter 'local_crypto_import_*.json' -ErrorAction SilentlyContinue |
             Sort-Object LastWriteTime)

if ($evFiles.Count -eq 0) {
    $Violations++
    Show-Fail "No evidence JSON found in $TempDir"
} else {
    $evJson = $evFiles[-1]
    Show-Info "Evidence JSON: $($evJson.FullName)"

    try {
        $ev = Get-Content -Raw -Path $evJson.FullName | ConvertFrom-Json

        $topKeys = @(
            'schema_version','patch_id','produced_at_utc','mode',
            'csv_path','timeframe','source','max_age_hours',
            'allow_stale_for_validation','checks','db_guard','mutation',
            'all_passed','reason_code','fail_reasons'
        )
        $checksKeys = @(
            'csv_file_exists','header_present','required_columns_present',
            'rows_seen','latest_completed_bar_ts','latest_completed_bar_age_secs',
            'stale','passed'
        )
        $mutationKeys = @('attempted','command','exit_code','succeeded')
        $dbKeys       = @('checked','passed','reason_code')

        $missingTop = @($topKeys | Where-Object { $_ -notin ($ev.PSObject.Properties.Name) })
        if ($missingTop.Count -gt 0) {
            $Violations++
            Show-Fail "Evidence JSON missing top-level keys: $($missingTop -join ', ')"
        } else {
            Show-Green "All $($topKeys.Count) required top-level keys present"
        }

        $missingCk = @($checksKeys | Where-Object { $_ -notin ($ev.checks.PSObject.Properties.Name) })
        if ($missingCk.Count -gt 0) {
            $Violations++
            Show-Fail "Evidence JSON checks missing keys: $($missingCk -join ', ')"
        } else {
            Show-Green "All $($checksKeys.Count) required checks keys present"
        }

        $missingMut = @($mutationKeys | Where-Object { $_ -notin ($ev.mutation.PSObject.Properties.Name) })
        if ($missingMut.Count -gt 0) {
            $Violations++
            Show-Fail "Evidence JSON mutation missing keys: $($missingMut -join ', ')"
        } else {
            Show-Green "All $($mutationKeys.Count) required mutation keys present"
        }

        $missingDb = @($dbKeys | Where-Object { $_ -notin ($ev.db_guard.PSObject.Properties.Name) })
        if ($missingDb.Count -gt 0) {
            $Violations++
            Show-Fail "Evidence JSON db_guard missing keys: $($missingDb -join ', ')"
        } else {
            Show-Green "All $($dbKeys.Count) required db_guard keys present"
        }

        if ($ev.schema_version -eq 'local-crypto-import-v1') {
            Show-Green "schema_version='local-crypto-import-v1' (correct)"
        } else {
            $Violations++
            Show-Fail "schema_version='$($ev.schema_version)' (expected 'local-crypto-import-v1')"
        }

        if ($ev.patch_id -match 'CRYPTO-DATA-01D') {
            Show-Green "patch_id contains 'CRYPTO-DATA-01D' (correct)"
        } else {
            $Violations++
            Show-Fail "patch_id='$($ev.patch_id)' does not reference CRYPTO-DATA-01D"
        }
    } catch {
        $Violations++
        Show-Fail "Evidence JSON parse failed: $_"
    }
}

# =============================================================================
# [6] Forbidden patterns absent from Import-LocalCryptoMarks.ps1
# =============================================================================
Write-Host ''
Show-Info '--- [6] Forbidden patterns absent from Import-LocalCryptoMarks.ps1 ---'

$forbidden = [ordered]@{
    'Invoke-RestMethod'          = 'REST/HTTP call to daemon or external endpoint'
    '/api/v1/ops/action'         = 'operator action route'
    '/api/v1/strategy/signal'    = 'strategy signal route'
    'Refresh-IntradayMarketData' = 'equity intraday refresh script invocation'
    'Start-PaperTradingSmoke'    = 'paper trading smoke script invocation'
    'sync-provider'              = 'provider sync CLI command'
    'ingest-provider'            = 'provider ingest CLI command'
}

if (Test-Path -LiteralPath $ImportScript) {
    $scriptText = [System.IO.File]::ReadAllText($ImportScript)

    foreach ($pat in $forbidden.Keys) {
        if ($scriptText -match [regex]::Escape($pat)) {
            $Violations++
            Show-Fail "Forbidden pattern found: '$pat' ($($forbidden[$pat]))"
        } else {
            Show-Green "Forbidden pattern absent: '$pat'"
        }
    }
}

# =============================================================================
# Cleanup and summary
# =============================================================================
try { Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue } catch {}

Write-Host ''
Write-Host '============================================================'
Write-Host ' Summary'
Write-Host '============================================================'

if ($Violations -eq 0) {
    Write-Host ' ALL CHECKS PASSED' -ForegroundColor Green
    exit 0
} else {
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
}
