# =============================================================================
# SCANNER-LIQUIDITY-01 static guard
#
# Proves:
#   G1. liquidity.py exists at expected path
#   G2. Module contains all stable rejection reason strings
#   G3. Module does not reference forbidden broker/OMS strings
#   G4. Module does not import forbidden network/DB modules
#   G5. Module uses build_scanner_candidate or ScannerCandidateWriter for rejection artifacts
#   G6. eligible_for_live is not set True
#   G7. No generated exports staged for commit
#
# Exit codes: 0 = all guards pass, 1 = one or more guards failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$LiqModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\liquidity.py'

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
Write-Host 'SCANNER-LIQUIDITY-01 static guard'
Write-Host '============================================================'

# G1: liquidity.py must exist
Write-Host ''
Write-Host '--- G1: liquidity.py exists ---'
if (Test-Path $LiqModule) {
    Pass "liquidity.py found at $LiqModule"
} else {
    Fail "liquidity.py not found at $LiqModule"
}

$LiqContent = ''
if (Test-Path $LiqModule) {
    $LiqContent = Get-Content $LiqModule -Raw
}

# G2: Module must contain all stable rejection reason strings
$RequiredReasons = @(
    'price_below_min',
    'adv_usd_below_min',
    'relative_volume_below_min',
    'spread_too_wide',
    'round_trip_cost_too_high',
    'slippage_too_high',
    'liquidity_score_below_min'
)

Write-Host ''
Write-Host '--- G2: All stable rejection reason strings present ---'
foreach ($reason in $RequiredReasons) {
    if ($LiqContent -match [regex]::Escape($reason)) {
        Pass "Rejection reason '$reason' found"
    } else {
        Fail "Rejection reason '$reason' NOT found in liquidity.py"
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
    if ($LiqContent -match [regex]::Escape($forbidden)) {
        Fail "liquidity.py references '$forbidden'"
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
    if ($LiqContent -match [regex]::Escape($imp)) {
        Fail "liquidity.py contains '$imp'"
    } else {
        Pass "No '$imp' in liquidity.py"
    }
}

# G5: Module must use build_scanner_candidate or ScannerCandidateWriter for rejection artifacts
Write-Host ''
Write-Host '--- G5: Uses scanner candidate writer integration ---'
if ($LiqContent -match [regex]::Escape('build_scanner_candidate')) {
    Pass "build_scanner_candidate referenced in liquidity.py"
} elseif ($LiqContent -match [regex]::Escape('ScannerCandidateWriter')) {
    Pass "ScannerCandidateWriter referenced in liquidity.py"
} else {
    Fail "liquidity.py must reference build_scanner_candidate or ScannerCandidateWriter"
}

# G6: eligible_for_live must not be set to True
Write-Host ''
Write-Host '--- G6: eligible_for_live not set True ---'
if ($LiqContent -match [regex]::Escape('eligible_for_live=True')) {
    Fail "liquidity.py sets eligible_for_live=True - not permitted"
} else {
    Pass "eligible_for_live=True not found in liquidity.py"
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
    Write-Host 'SCANNER-LIQUIDITY-01 guard: ALL PASS' -ForegroundColor Green
    exit 0
} else {
    Write-Host "SCANNER-LIQUIDITY-01 guard: $failCount FAILURE(S)" -ForegroundColor Red
    foreach ($f in $Failures) { Write-Host $f }
    exit 1
}
