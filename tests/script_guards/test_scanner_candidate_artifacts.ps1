# =============================================================================
# SCANNER-CANDIDATE-01 static guard
#
# Proves:
#   G1. MAIN candidate module exists at expected path
#   G2. Module contains "scanner-candidate-v1"
#   G3. Module forces eligible_for_live to False
#   G4. Module does not reference forbidden broker/OMS strings
#   G5. Module does not import forbidden network/DB modules
#   G6. EXP candidate schema "exp-candidate-v1" remains present
#   G7. exports/ directory is not git-staged (no generated exports committed)
#
# Exit codes: 0 = all guards pass, 1 = one or more guards failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$CandidatesModule = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\candidates.py'
$ExpJournalModule = Join-Path $RepoRoot 'research-py\experiments\exp_engine\candidate_journal.py'

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
Write-Host 'SCANNER-CANDIDATE-01 static guard'
Write-Host '============================================================'

# G1: MAIN candidate module must exist
Write-Host ''
Write-Host '--- G1: MAIN candidate module exists ---'
if (Test-Path $CandidatesModule) {
    Pass "candidates.py found at $CandidatesModule"
} else {
    Fail "candidates.py not found at $CandidatesModule"
}

$CandidatesContent = ''
if (Test-Path $CandidatesModule) {
    $CandidatesContent = Get-Content $CandidatesModule -Raw
}

# G2: Module must contain "scanner-candidate-v1"
Write-Host ''
Write-Host '--- G2: Module contains "scanner-candidate-v1" ---'
if ($CandidatesContent -match [regex]::Escape('scanner-candidate-v1')) {
    Pass "scanner-candidate-v1 found in candidates.py"
} else {
    Fail "scanner-candidate-v1 NOT found in candidates.py"
}

# G3: Module must force eligible_for_live to False
Write-Host ''
Write-Host '--- G3: eligible_for_live forced False ---'
if ($CandidatesContent -match [regex]::Escape('"eligible_for_live": False')) {
    Pass "eligible_for_live: False found in candidates.py"
} else {
    Fail "eligible_for_live does not appear as False in candidates.py"
}

# G4: Module must not reference forbidden broker/OMS strings
$ForbiddenStrings = @(
    'BrokerGateway',
    'broker_adapter',
    'alpaca',
    'oms_outbox',
    'oms_inbox',
    '/v2/orders',
    'submit_order',
    'live_routing_enabled=True'
)

Write-Host ''
Write-Host '--- G4: No forbidden broker/OMS strings ---'
foreach ($forbidden in $ForbiddenStrings) {
    if ($CandidatesContent -match [regex]::Escape($forbidden)) {
        Fail "candidates.py references '$forbidden'"
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
    if ($CandidatesContent -match [regex]::Escape($imp)) {
        Fail "candidates.py contains '$imp'"
    } else {
        Pass "No '$imp' in candidates.py"
    }
}

# G6: EXP exp-candidate-v1 schema must remain present
Write-Host ''
Write-Host '--- G6: EXP exp-candidate-v1 schema present ---'
if (Test-Path $ExpJournalModule) {
    $ExpContent = Get-Content $ExpJournalModule -Raw
    if ($ExpContent -match [regex]::Escape('exp-candidate-v1')) {
        Pass "exp-candidate-v1 found in EXP candidate_journal.py"
    } else {
        Fail "exp-candidate-v1 NOT found in EXP candidate_journal.py"
    }
} else {
    Fail "EXP candidate_journal.py not found at $ExpJournalModule"
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
    Write-Host 'SCANNER-CANDIDATE-01 guard: ALL PASS' -ForegroundColor Green
    exit 0
} else {
    Write-Host "SCANNER-CANDIDATE-01 guard: $failCount FAILURE(S)" -ForegroundColor Red
    foreach ($f in $Failures) { Write-Host $f }
    exit 1
}
