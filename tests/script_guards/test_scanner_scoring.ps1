# =============================================================================
# SCANNER-SCORE-01 static guard
#
# Proves:
#   G1.  scoring.py exists at expected path
#   G2.  Module contains ScoringConfig, ScoringInputs, ScoringResult
#   G3.  Module contains build_scored_scanner_candidate
#   G4.  Module uses build_scanner_candidate
#   G5.  Module preserves eligible_for_live=False (not set True)
#   G6.  Module does not reference forbidden broker/OMS strings
#   G7.  Module does not import forbidden network/DB modules
#   G8.  No generated exports staged for commit
#
# Exit codes: 0 = all guards pass, 1 = one or more guards failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$ScoringModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\scoring.py'

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
Write-Host 'SCANNER-SCORE-01 static guard'
Write-Host '============================================================'

# G1: scoring.py must exist
Write-Host ''
Write-Host '--- G1: scoring.py exists ---'
if (Test-Path $ScoringModule) {
    Pass "scoring.py found at $ScoringModule"
} else {
    Fail "scoring.py not found at $ScoringModule"
}

$Content = ''
if (Test-Path $ScoringModule) {
    $Content = Get-Content $ScoringModule -Raw
}

# G2: Required types present
$RequiredTypes = @('ScoringConfig', 'ScoringInputs', 'ScoringResult')
Write-Host ''
Write-Host '--- G2: Required types present ---'
foreach ($t in $RequiredTypes) {
    if ($Content -match [regex]::Escape($t)) {
        Pass "$t found"
    } else {
        Fail "$t NOT found in scoring.py"
    }
}

# G3: build_scored_scanner_candidate present
Write-Host ''
Write-Host '--- G3: build_scored_scanner_candidate present ---'
if ($Content -match [regex]::Escape('build_scored_scanner_candidate')) {
    Pass "build_scored_scanner_candidate found"
} else {
    Fail "build_scored_scanner_candidate NOT found in scoring.py"
}

# G4: Uses build_scanner_candidate
Write-Host ''
Write-Host '--- G4: Uses build_scanner_candidate ---'
if ($Content -match [regex]::Escape('build_scanner_candidate')) {
    Pass "build_scanner_candidate referenced"
} else {
    Fail "scoring.py does not reference build_scanner_candidate"
}

# G5: eligible_for_live=True must NOT appear
Write-Host ''
Write-Host '--- G5: eligible_for_live=True not present ---'
if ($Content -match [regex]::Escape('eligible_for_live=True')) {
    Fail "scoring.py sets eligible_for_live=True - not permitted"
} else {
    Pass "eligible_for_live=True not found in scoring.py"
}

# G6: No forbidden broker/OMS strings
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
Write-Host '--- G6: No forbidden broker/OMS strings ---'
foreach ($forbidden in $ForbiddenStrings) {
    if ($Content -match [regex]::Escape($forbidden)) {
        Fail "scoring.py references '$forbidden'"
    } else {
        Pass "No reference to '$forbidden'"
    }
}

# G7: No forbidden network/DB imports
$ForbiddenImports = @(
    'import requests',
    'import urllib',
    'import http.client',
    'import aiohttp',
    'import psycopg',
    'import sqlalchemy'
)
Write-Host ''
Write-Host '--- G7: No forbidden network/DB imports ---'
foreach ($imp in $ForbiddenImports) {
    if ($Content -match [regex]::Escape($imp)) {
        Fail "scoring.py contains '$imp'"
    } else {
        Pass "No '$imp' in scoring.py"
    }
}

# G8: No generated exports staged for commit
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
    Write-Host 'SCANNER-SCORE-01 guard: ALL PASS' -ForegroundColor Green
    exit 0
} else {
    Write-Host "SCANNER-SCORE-01 guard: $failCount FAILURE(S)" -ForegroundColor Red
    foreach ($f in $Failures) { Write-Host $f }
    exit 1
}
