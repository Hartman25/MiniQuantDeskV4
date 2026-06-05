# =============================================================================
# SCANNER-REGIME-01 static guard
#
# Proves:
#   G1. regime.py exists at expected path
#   G2. Module contains all required regime label strings
#   G3. Module contains stable rejection reason strings
#   G4. Module does not reference forbidden broker/OMS strings
#   G5. Module does not import forbidden network/DB modules
#   G6. Module uses build_scanner_candidate or ScannerCandidateWriter for rejection artifacts
#   G7. eligible_for_live is not set True
#   G8. No generated exports staged for commit
#
# Exit codes: 0 = all guards pass, 1 = one or more guards failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$RegimeModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\regime.py'

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
Write-Host 'SCANNER-REGIME-01 static guard'
Write-Host '============================================================'

# G1: regime.py must exist
Write-Host ''
Write-Host '--- G1: regime.py exists ---'
if (Test-Path $RegimeModule) {
    Pass "regime.py found at $RegimeModule"
} else {
    Fail "regime.py not found at $RegimeModule"
}

$RegimeContent = ''
if (Test-Path $RegimeModule) {
    $RegimeContent = Get-Content $RegimeModule -Raw
}

# G2: Module must contain all required regime label strings
$RequiredLabels = @(
    'trending_up',
    'trending_down',
    'range_bound',
    'high_volatility',
    'low_volatility',
    'gapping',
    'unclassified'
)

Write-Host ''
Write-Host '--- G2: All required regime label strings present ---'
foreach ($label in $RequiredLabels) {
    if ($RegimeContent -match [regex]::Escape($label)) {
        Pass "Regime label '$label' found"
    } else {
        Fail "Regime label '$label' NOT found in regime.py"
    }
}

# G3: Module must contain stable rejection reason strings
$RequiredReasons = @(
    'regime_unclassified',
    'regime_score_below_min'
)

Write-Host ''
Write-Host '--- G3: Stable rejection reason strings present ---'
foreach ($reason in $RequiredReasons) {
    if ($RegimeContent -match [regex]::Escape($reason)) {
        Pass "Rejection reason '$reason' found"
    } else {
        Fail "Rejection reason '$reason' NOT found in regime.py"
    }
}

# G4: Module must not reference forbidden broker/OMS strings
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
Write-Host '--- G4: No forbidden broker/OMS strings ---'
foreach ($forbidden in $ForbiddenStrings) {
    if ($RegimeContent -match [regex]::Escape($forbidden)) {
        Fail "regime.py references '$forbidden'"
    } else {
        Pass "No reference to '$forbidden'"
    }
}

# G5: Module must not import forbidden network/DB modules
$ForbiddenImports = @(
    'import requests',
    'import urllib',
    'import http.client',
    'import aiohttp',
    'import psycopg',
    'import sqlalchemy'
)

Write-Host ''
Write-Host '--- G5: No forbidden network/DB imports ---'
foreach ($imp in $ForbiddenImports) {
    if ($RegimeContent -match [regex]::Escape($imp)) {
        Fail "regime.py contains '$imp'"
    } else {
        Pass "No '$imp' in regime.py"
    }
}

# G6: Module must use build_scanner_candidate or ScannerCandidateWriter for rejection artifacts
Write-Host ''
Write-Host '--- G6: Uses scanner candidate writer integration ---'
if ($RegimeContent -match [regex]::Escape('build_scanner_candidate')) {
    Pass "build_scanner_candidate referenced in regime.py"
} elseif ($RegimeContent -match [regex]::Escape('ScannerCandidateWriter')) {
    Pass "ScannerCandidateWriter referenced in regime.py"
} else {
    Fail "regime.py must reference build_scanner_candidate or ScannerCandidateWriter"
}

# G7: eligible_for_live must not be set to True
Write-Host ''
Write-Host '--- G7: eligible_for_live not set True ---'
if ($RegimeContent -match [regex]::Escape('eligible_for_live=True')) {
    Fail "regime.py sets eligible_for_live=True - not permitted"
} else {
    Pass "eligible_for_live=True not found in regime.py"
}

# G8: exports/ directory must not appear in git staged files
Write-Host ''
Write-Host '--- G8: No generated exports staged for commit ---'
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
    Write-Host 'SCANNER-REGIME-01 guard: ALL PASS' -ForegroundColor Green
    exit 0
} else {
    Write-Host "SCANNER-REGIME-01 guard: $failCount FAILURE(S)" -ForegroundColor Red
    foreach ($f in $Failures) { Write-Host $f }
    exit 1
}
