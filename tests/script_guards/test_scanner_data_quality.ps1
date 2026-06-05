# =============================================================================
# SCANNER-DQ-01 static guard
#
# Proves:
#   G1. data_quality.py exists at expected path
#   G2. Module contains all stable rejection reason strings
#   G3. Module does not reference forbidden broker/OMS strings
#   G4. Module does not import forbidden network/DB modules
#   G5. Module uses build_scanner_candidate or ScannerCandidateWriter for rejection artifacts
#   G6. eligible_for_live=False enforced (not True)
#   G7. No generated exports staged for commit
#
# Exit codes: 0 = all guards pass, 1 = one or more guards failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$DqModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\data_quality.py'

$Failures = [System.Collections.Generic.List[string]]::new()

function Fail([string]$msg) {
    $Failures.Add("  FAIL: $msg")
    Write-Host "  FAIL: $msg" -ForegroundColor Red
}

function Pass([string]$msg) {
    Write-Host "  PASS: $msg" -ForegroundColor Green
}

Write-Host ''
Write-Host '============================================================'
Write-Host 'SCANNER-DQ-01 static guard'
Write-Host '============================================================'

# G1: data_quality.py must exist
Write-Host ''
Write-Host '--- G1: data_quality.py exists ---'
if (Test-Path $DqModule) {
    Pass "data_quality.py found at $DqModule"
} else {
    Fail "data_quality.py not found at $DqModule"
}

$DqContent = ''
if (Test-Path $DqModule) {
    $DqContent = Get-Content $DqModule -Raw
}

# G2: Module must contain all stable rejection reason strings
$RequiredReasons = @(
    'no_bars',
    'insufficient_completed_bars',
    'latest_bar_stale',
    'duplicate_bar_timestamp',
    'too_many_zero_volume_bars',
    'zero_price_bar',
    'inverted_ohlc',
    'close_outside_ohlc_range',
    'missing_bar_gap',
    'latest_bar_incomplete'
)

Write-Host ''
Write-Host '--- G2: All stable rejection reason strings present ---'
foreach ($reason in $RequiredReasons) {
    if ($DqContent -match [regex]::Escape($reason)) {
        Pass "Rejection reason '$reason' found"
    } else {
        Fail "Rejection reason '$reason' NOT found in data_quality.py"
    }
}

# G3: Module must not reference forbidden broker/OMS strings
$ForbiddenStrings = @(
    'BrokerGateway',
    'broker_adapter',
    'oms_outbox',
    'oms_inbox',
    '/v2/orders',
    'submit_order',
    'live_routing_enabled=True'
)

Write-Host ''
Write-Host '--- G3: No forbidden broker/OMS strings ---'
foreach ($forbidden in $ForbiddenStrings) {
    if ($DqContent -match [regex]::Escape($forbidden)) {
        Fail "data_quality.py references '$forbidden'"
    } else {
        Pass "No reference to '$forbidden'"
    }
}

# G4: Module must not import forbidden network/DB modules
$ForbiddenImports = @(
    'import requests',
    'import urllib',
    'import http.client',
    'import aiohttp',
    'import psycopg',
    'import sqlalchemy'
)

Write-Host ''
Write-Host '--- G4: No forbidden network/DB imports ---'
foreach ($imp in $ForbiddenImports) {
    if ($DqContent -match [regex]::Escape($imp)) {
        Fail "data_quality.py contains '$imp'"
    } else {
        Pass "No '$imp' in data_quality.py"
    }
}

# G5: Module must use build_scanner_candidate or ScannerCandidateWriter for rejection artifacts
Write-Host ''
Write-Host '--- G5: Uses scanner candidate writer integration ---'
if ($DqContent -match [regex]::Escape('build_scanner_candidate')) {
    Pass "build_scanner_candidate referenced in data_quality.py"
} elseif ($DqContent -match [regex]::Escape('ScannerCandidateWriter')) {
    Pass "ScannerCandidateWriter referenced in data_quality.py"
} else {
    Fail "data_quality.py must reference build_scanner_candidate or ScannerCandidateWriter"
}

# G6: eligible_for_live must not be set to True
Write-Host ''
Write-Host '--- G6: eligible_for_live not set True ---'
if ($DqContent -match [regex]::Escape('eligible_for_live=True')) {
    Fail "data_quality.py sets eligible_for_live=True - not permitted"
} else {
    Pass "eligible_for_live=True not found in data_quality.py"
}

# G7: exports/ directory must not appear in git staged files
Write-Host ''
Write-Host '--- G7: No generated exports staged for commit ---'
Push-Location $RepoRoot
try {
    $staged = & git diff --cached --name-only 2>$null
    $exportHits = $staged | Where-Object { $_ -match '^exports/' }
    if ($exportHits) {
        foreach ($hit in $exportHits) {
            Fail "Staged export file detected: $hit"
        }
    } else {
        Pass "No exports/ files staged for commit"
    }
} finally {
    Pop-Location
}

# Summary
Write-Host ''
Write-Host '============================================================'
$failCount = $Failures.Count
if ($failCount -eq 0) {
    Write-Host 'SCANNER-DQ-01 guard: ALL PASS' -ForegroundColor Green
    exit 0
} else {
    Write-Host "SCANNER-DQ-01 guard: $failCount FAILURE(S)" -ForegroundColor Red
    foreach ($f in $Failures) { Write-Host $f }
    exit 1
}
