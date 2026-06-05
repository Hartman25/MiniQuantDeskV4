# =============================================================================
# SCANNER-SELECTOR-01 static guard
#
# Proves:
#   G1.  selector.py exists at expected path
#   G2.  Module contains watchlist-v1 schema version
#   G3.  Module contains ranked-candidates-v1 schema version
#   G4.  Module preserves approved_for_live=False (not set True)
#   G5.  Module preserves approved_for_autonomous_paper=False (not set True)
#   G6.  Module forces max_symbols_to_trade to 1
#   G7.  Module forces max_concurrent_positions to 1
#   G8.  Module does not reference forbidden broker/OMS strings
#   G9.  Module does not import forbidden network/DB modules
#   G10. Module does not write into config/watchlists
#   G11. No generated exports staged for commit
#
# Exit codes: 0 = all guards pass, 1 = one or more guards failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$SelectorModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\selector.py'

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
Write-Host 'SCANNER-SELECTOR-01 static guard'
Write-Host '============================================================'

# G1: selector.py must exist
Write-Host ''
Write-Host '--- G1: selector.py exists ---'
if (Test-Path $SelectorModule) {
    Pass "selector.py found at $SelectorModule"
} else {
    Fail "selector.py not found at $SelectorModule"
}

$Content = ''
if (Test-Path $SelectorModule) {
    $Content = Get-Content $SelectorModule -Raw
}

# G2: watchlist-v1 present
Write-Host ''
Write-Host '--- G2: watchlist-v1 schema version present ---'
if ($Content -match [regex]::Escape('watchlist-v1')) {
    Pass "watchlist-v1 found"
} else {
    Fail "watchlist-v1 NOT found in selector.py"
}

# G3: ranked-candidates-v1 present
Write-Host ''
Write-Host '--- G3: ranked-candidates-v1 schema version present ---'
if ($Content -match [regex]::Escape('ranked-candidates-v1')) {
    Pass "ranked-candidates-v1 found"
} else {
    Fail "ranked-candidates-v1 NOT found in selector.py"
}

# G4: approved_for_live=True must NOT appear
Write-Host ''
Write-Host '--- G4: approved_for_live=True not present ---'
if ($Content -match [regex]::Escape('approved_for_live=True')) {
    Fail "selector.py sets approved_for_live=True - not permitted"
} else {
    Pass "approved_for_live=True not found in selector.py"
}

# G5: approved_for_autonomous_paper=True must NOT appear
Write-Host ''
Write-Host '--- G5: approved_for_autonomous_paper=True not present ---'
if ($Content -match [regex]::Escape('approved_for_autonomous_paper=True')) {
    Fail "selector.py sets approved_for_autonomous_paper=True - not permitted in this patch"
} else {
    Pass "approved_for_autonomous_paper=True not found in selector.py"
}

# G6: max_symbols_to_trade forced to 1
Write-Host ''
Write-Host '--- G6: max_symbols_to_trade forced to 1 ---'
if ($Content -match [regex]::Escape('max_symbols_to_trade=1')) {
    Pass "max_symbols_to_trade=1 found"
} else {
    Fail "max_symbols_to_trade=1 NOT found in selector.py"
}

# G7: max_concurrent_positions forced to 1
Write-Host ''
Write-Host '--- G7: max_concurrent_positions forced to 1 ---'
if ($Content -match [regex]::Escape('max_concurrent_positions=1')) {
    Pass "max_concurrent_positions=1 found"
} else {
    Fail "max_concurrent_positions=1 NOT found in selector.py"
}

# G8: No forbidden broker/OMS strings
$ForbiddenStrings = @(
    '/v2/orders',
    'submit_order',
    'live_routing_enabled=true',
    'oms_outbox',
    'oms_inbox',
    'BrokerGateway',
    'broker_adapter'
)
Write-Host ''
Write-Host '--- G8: No forbidden broker/OMS strings ---'
foreach ($forbidden in $ForbiddenStrings) {
    if ($Content -match [regex]::Escape($forbidden)) {
        Fail "selector.py references '$forbidden'"
    } else {
        Pass "No reference to '$forbidden'"
    }
}

# G9: No forbidden network/DB imports
$ForbiddenImports = @(
    'import requests',
    'import urllib',
    'import http.client',
    'import aiohttp',
    'import psycopg',
    'import sqlalchemy'
)
Write-Host ''
Write-Host '--- G9: No forbidden network/DB imports ---'
foreach ($imp in $ForbiddenImports) {
    if ($Content -match [regex]::Escape($imp)) {
        Fail "selector.py contains '$imp'"
    } else {
        Pass "No '$imp' in selector.py"
    }
}

# G10: Does not write into config/watchlists
Write-Host ''
Write-Host '--- G10: Does not write into config/watchlists ---'
if ($Content -match [regex]::Escape('config/watchlists')) {
    Fail "selector.py references config/watchlists - writes must go to exports/ only"
} else {
    Pass "No config/watchlists reference in selector.py"
}

# G11: No generated exports staged for commit
Write-Host ''
Write-Host '--- G11: No generated exports staged for commit ---'
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
    Write-Host 'SCANNER-SELECTOR-01 guard: ALL PASS' -ForegroundColor Green
    exit 0
} else {
    Write-Host "SCANNER-SELECTOR-01 guard: $failCount FAILURE(S)" -ForegroundColor Red
    foreach ($f in $Failures) { Write-Host $f }
    exit 1
}
