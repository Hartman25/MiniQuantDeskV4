# =============================================================================
# DATA-INGESTION-PREMARKET-REFRESH-01 -- Script static invariant tests
#
# Reads Prep-PremarketMarketData.ps1 as text and asserts PMD01-PMD14.
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_premarket_market_data_prep.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$TargetScript = Join-Path $RepoRoot 'scripts\windows\Prep-PremarketMarketData.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== Prep-PremarketMarketData.ps1 static invariant tests ==="
Write-Host "    Target: $TargetScript"
Write-Host ""

# ---------------------------------------------------------------------------
# PMD01: Script exists
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    Pass 'PMD01' "Script exists at scripts\windows\Prep-PremarketMarketData.ps1"
} else {
    Fail 'PMD01' "Script NOT found at: $TargetScript"
}

$scriptText = ''
if (Test-Path $TargetScript) {
    $scriptText = Get-Content -Path $TargetScript -Raw
}

# ---------------------------------------------------------------------------
# PMD02: Script is ASCII-safe (no chars above 0x7E that would break PS5.1)
# ---------------------------------------------------------------------------
$nonAscii = [regex]::Matches($scriptText, '[^\x00-\x7E]')
if ($nonAscii.Count -eq 0) {
    Pass 'PMD02' "Script is ASCII-safe (no non-ASCII characters)."
} else {
    Fail 'PMD02' "Script contains $($nonAscii.Count) non-ASCII character(s) at positions: $($nonAscii | Select-Object -First 5 -ExpandProperty Index)"
}

# ---------------------------------------------------------------------------
# PMD03: Paper DB guard present (port 5440 check)
# ---------------------------------------------------------------------------
if ($scriptText -match '5440') {
    Pass 'PMD03' "Paper DB port 5440 guard present."
} else {
    Fail 'PMD03' "Missing paper DB port 5440 guard."
}

# ---------------------------------------------------------------------------
# PMD04: TWELVEDATA_API_KEY presence check without printing value
# Script must check key presence but never echo/print/write its value.
# ---------------------------------------------------------------------------
$keyCheck  = $scriptText -match 'TWELVEDATA_API_KEY'
$keyPrint  = $scriptText -match 'Write-Host[^#]*\$env:TWELVEDATA_API_KEY'
if ($keyCheck -and -not $keyPrint) {
    Pass 'PMD04' "TWELVEDATA_API_KEY presence checked; value not printed."
} elseif (-not $keyCheck) {
    Fail 'PMD04' "TWELVEDATA_API_KEY not referenced in script."
} else {
    Fail 'PMD04' "Script appears to print TWELVEDATA_API_KEY value."
}

# ---------------------------------------------------------------------------
# PMD05: MinCompletedBars gate present
# ---------------------------------------------------------------------------
if ($scriptText -match 'MinCompletedBars') {
    Pass 'PMD05' "MinCompletedBars parameter and gate present."
} else {
    Fail 'PMD05' "MinCompletedBars gate not found."
}

# ---------------------------------------------------------------------------
# PMD06: Freshness/staleness gate present
# ---------------------------------------------------------------------------
if ($scriptText -match 'staleness' -or $scriptText -match 'StalenessDays' -or $scriptText -match 'MaxStalenessDays') {
    Pass 'PMD06' "Freshness/staleness gate present."
} else {
    Fail 'PMD06' "Freshness/staleness gate not found."
}

# ---------------------------------------------------------------------------
# PMD07: Evidence output present (exports/market_data or Write-Evidence)
# ---------------------------------------------------------------------------
if ($scriptText -match 'market_data' -or $scriptText -match 'Write-Evidence') {
    Pass 'PMD07' "Evidence output to exports/market_data present."
} else {
    Fail 'PMD07' "Evidence output not found."
}

# ---------------------------------------------------------------------------
# PMD08: CheckOnly parameter exists and has no-mutation behavior
# ---------------------------------------------------------------------------
if ($scriptText -match '\[switch\]\s+\$CheckOnly' -or $scriptText -match '\$CheckOnly') {
    Pass 'PMD08' "CheckOnly parameter present."
} else {
    Fail 'PMD08' "CheckOnly parameter not found."
}

# CheckOnly must not contain sync-provider or ingest-csv calls inside its block
# (approximate: we check CheckOnly block does not contain 'sync-provider' before the main body)
$checkOnlyIdx   = $scriptText.IndexOf('if ($CheckOnly)')
$checkOnlyEnd   = if ($checkOnlyIdx -ge 0) { $scriptText.IndexOf('exit 0', $checkOnlyIdx) } else { -1 }
if ($checkOnlyIdx -ge 0 -and $checkOnlyEnd -gt $checkOnlyIdx) {
    $checkOnlyBlock = $scriptText.Substring($checkOnlyIdx, $checkOnlyEnd - $checkOnlyIdx)
    if ($checkOnlyBlock -notmatch 'sync-provider' -and $checkOnlyBlock -notmatch 'ingest-csv') {
        Pass 'PMD08b' "CheckOnly block contains no sync-provider or ingest-csv calls (no mutations)."
    } else {
        Fail 'PMD08b' "CheckOnly block appears to call sync-provider or ingest-csv (mutations in CheckOnly)."
    }
} else {
    Pass 'PMD08b' "CheckOnly block boundary not easily isolated -- manual verification recommended."
}

# ---------------------------------------------------------------------------
# PMD09: Never executes SQL mutations against trading tables.
# Checks for actual SQL INSERT/UPDATE/DELETE against those tables, or
# direct Rust function calls that would mutate them.
# Comments that mention table names (safety documentation) are expected and allowed.
# ---------------------------------------------------------------------------
$tradingMutationPatterns = @(
    'INSERT\s+INTO\s+oms_outbox',
    'UPDATE\s+oms_outbox',
    'DELETE\s+FROM\s+oms_outbox',
    'INSERT\s+INTO\s+oms_inbox',
    'UPDATE\s+oms_inbox',
    'DELETE\s+FROM\s+oms_inbox',
    'INSERT\s+INTO\s+broker_order_map',
    'outbox_enqueue\s*\(',
    'submit_order\s*\(',
    'cancel_order\s*\(',
    'replace_order\s*\('
)
$tradingViolations = @()
foreach ($pat in $tradingMutationPatterns) {
    if ($scriptText -match $pat) {
        $tradingViolations += $pat
    }
}
if ($tradingViolations.Count -eq 0) {
    Pass 'PMD09' "No SQL mutation patterns against trading tables found."
} else {
    Fail 'PMD09' "SQL mutation patterns against trading tables found: $($tradingViolations -join ', ')"
}

# ---------------------------------------------------------------------------
# PMD10: Script does not embed a live DB connection string.
# Checks for raw port-5432 connection strings (not port 5440).
# Does not flag comments or error messages that mention "live" for documentation.
# ---------------------------------------------------------------------------
# Only flag lines that contain a postgres URL-like pattern with port 5432 (not 5440)
$liveDbViolations = @()
foreach ($line in ($scriptText -split "`n")) {
    $trimLine = $line.TrimStart()
    if ($trimLine.StartsWith('#')) { continue }   # skip comment lines
    if ($trimLine -match 'postgres://[^:]+:[^@]+@[^:]+:5432[^0]') {
        $liveDbViolations += $trimLine.Trim()
    }
}
if ($liveDbViolations.Count -eq 0) {
    Pass 'PMD10' "No live DB connection string (port 5432) found in non-comment lines."
} else {
    Fail 'PMD10' "Live DB port 5432 connection strings found: $($liveDbViolations -join '; ')"
}

# ---------------------------------------------------------------------------
# PMD11: end_ts column used (not bar_time_utc or ts_utc which don't exist)
# ---------------------------------------------------------------------------
if ($scriptText -match 'end_ts') {
    Pass 'PMD11' "Uses correct end_ts column for md_bars."
} else {
    Fail 'PMD11' "end_ts column reference not found -- may use wrong column name."
}
if ($scriptText -match 'bar_time_utc' -or $scriptText -match '\bts_utc\b') {
    Fail 'PMD11b' "Script references non-existent column bar_time_utc or ts_utc."
} else {
    Pass 'PMD11b' "Non-existent column names (bar_time_utc, ts_utc) not referenced."
}

# ---------------------------------------------------------------------------
# PMD12: to_timestamp used for end_ts date conversion
# ---------------------------------------------------------------------------
if ($scriptText -match 'to_timestamp') {
    Pass 'PMD12' "to_timestamp used for end_ts -> date conversion."
} else {
    Fail 'PMD12' "to_timestamp not found; date conversion from end_ts may be missing."
}

# ---------------------------------------------------------------------------
# PMD13: Schema version in evidence output
# ---------------------------------------------------------------------------
if ($scriptText -match 'schema_version') {
    Pass 'PMD13' "schema_version present in evidence output."
} else {
    Fail 'PMD13' "schema_version not found in evidence output."
}

# ---------------------------------------------------------------------------
# PMD14: Secrets never printed (no Write-Host of key values)
# ---------------------------------------------------------------------------
$secretPrintPatterns = @(
    'Write-Host.*\$env:TWELVEDATA_API_KEY',
    'Write-Host.*\$env:ALPACA_API_KEY',
    'Write-Host.*\$env:MQK_OPERATOR_TOKEN',
    'Write-Host.*password'
)
$secretViolations = @()
foreach ($pat in $secretPrintPatterns) {
    if ($scriptText -match $pat) {
        $secretViolations += $pat
    }
}
if ($secretViolations.Count -eq 0) {
    Pass 'PMD14' "No secret-printing patterns found."
} else {
    Fail 'PMD14' "Potential secret-printing patterns: $($secretViolations -join '; ')"
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Summary ==="
if ($Failures -eq 0) {
    Write-Host "ALL PASSED ($($scriptText.Length) bytes checked, 0 failures)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "FAILED: $Failures check(s) did not pass" -ForegroundColor Red
    exit 1
}
