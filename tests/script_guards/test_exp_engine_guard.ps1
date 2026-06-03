# =============================================================================
# EXP-ENGINE-CORE-01 static guard
#
# Proves:
#   G1. MQK_EXPERIMENTAL_ENGINE_ENABLED is false (or absent) in .env.local.example
#   G2. MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED is false (or absent) in .env.local.example
#   G3. exp_engine source files do not reference oms_outbox
#   G4. exp_engine source files do not reference BrokerGateway
#   G5. exp_engine source files do not reference broker_adapter
#   G6. exp_engine source files do not reference Start-PaperTradingSmoke
#   G7. candidate_journal.py sets paper_order_id to None (never a non-null default)
#   G8. config.py does not set live_allowed=True as a default
#
# Exit codes: 0 = all guards pass, 1 = one or more guards failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$EnvExample = Join-Path $RepoRoot '.env.local.example'
$ExpEngineDir = Join-Path $RepoRoot 'research-py\experiments\exp_engine'

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
Write-Host 'EXP-ENGINE-CORE-01 static guard'
Write-Host '============================================================'

# G1: MQK_EXPERIMENTAL_ENGINE_ENABLED must be false or absent
Write-Host ''
Write-Host '--- G1: MQK_EXPERIMENTAL_ENGINE_ENABLED must be false or absent ---'
$enabledLine = Get-Content $EnvExample | Where-Object { $_ -match '^MQK_EXPERIMENTAL_ENGINE_ENABLED\s*=' }
if (-not $enabledLine) {
    Pass "MQK_EXPERIMENTAL_ENGINE_ENABLED not present (disabled by absence)"
} elseif ($enabledLine -match '=\s*false\s*$') {
    Pass "MQK_EXPERIMENTAL_ENGINE_ENABLED=false"
} else {
    Fail "MQK_EXPERIMENTAL_ENGINE_ENABLED is not false in .env.local.example: $enabledLine"
}

# G2: MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED must be false or absent
Write-Host ''
Write-Host '--- G2: MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED must be false or absent ---'
$liveAllowedLine = Get-Content $EnvExample | Where-Object { $_ -match '^MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED\s*=' }
if (-not $liveAllowedLine) {
    Pass "MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED not present (disabled by absence)"
} elseif ($liveAllowedLine -match '=\s*false\s*$') {
    Pass "MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED=false"
} else {
    Fail "MQK_EXPERIMENTAL_ENGINE_LIVE_ALLOWED is not false in .env.local.example: $liveAllowedLine"
}

# G3-G6: exp_engine source files (excluding test files) must not reference forbidden patterns
$ForbiddenPatterns = @{
    'G3' = 'oms_outbox'
    'G4' = 'BrokerGateway'
    'G5' = 'broker_adapter'
    'G6' = 'Start-PaperTradingSmoke'
}

$SourceFiles = Get-ChildItem -Path $ExpEngineDir -Recurse -Include '*.py' |
    Where-Object { $_.Name -notmatch '^test_' }

foreach ($key in $ForbiddenPatterns.Keys | Sort-Object) {
    $pattern = $ForbiddenPatterns[$key]
    Write-Host ''
    Write-Host "--- $key`: exp_engine source must not reference '$pattern' ---"
    $hits = $SourceFiles | Where-Object {
        (Get-Content $_.FullName -Raw) -match [regex]::Escape($pattern)
    }
    if ($hits) {
        foreach ($hit in $hits) {
            Fail "$($hit.Name) references '$pattern'"
        }
    } else {
        Pass "No exp_engine source file references '$pattern'"
    }
}

# G7: candidate_journal.py must set paper_order_id to None
Write-Host ''
Write-Host '--- G7: candidate_journal.py must set paper_order_id to None ---'
$journalPath = Join-Path $ExpEngineDir 'candidate_journal.py'
$journalContent = Get-Content $journalPath -Raw
if ($journalContent -match '"paper_order_id":\s*None') {
    Pass "paper_order_id is set to None in candidate_journal.py"
} else {
    Fail "paper_order_id does not appear as None in candidate_journal.py"
}

# G8: config.py must not set live_allowed=True as a default
Write-Host ''
Write-Host '--- G8: config.py must not default live_allowed to True ---'
$configPath = Join-Path $ExpEngineDir 'config.py'
$configContent = Get-Content $configPath -Raw
if ($configContent -match 'live_allowed\s*=\s*True') {
    Fail "config.py contains live_allowed=True; this must never be a default"
} else {
    Pass "config.py does not default live_allowed to True"
}

# Summary
Write-Host ''
Write-Host '============================================================'
$fileCount = $SourceFiles.Count
$failCount = $Failures.Count
if ($failCount -eq 0) {
    Write-Host "EXP-ENGINE guard: ALL PASS ($fileCount source files checked)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "EXP-ENGINE guard: $failCount FAILURE(S)" -ForegroundColor Red
    foreach ($f in $Failures) { Write-Host $f }
    exit 1
}
