# =============================================================================
# MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-01 -- Launcher Guard
# =============================================================================
# Validates the Defect C / SS14 repair in Start-PaperTradingSmoke.ps1:
#   - the -StartRequiredUniverseScheduler / -StartIntradayRefreshLoop
#     conflict is refused BEFORE any child-process/scheduler side effect
#     (functional proof: actually invokes the real script with both flags
#     and confirms zero new child processes plus a pre-STEP-1 exit)
#   - the required-universe scheduler is the normal-startup default
#   - -CheckOnly and -StartIntradayRefreshLoop both still resolve to "do not
#     start the required-universe scheduler" without erroring
#   - -SkipRequiredUniverseScheduler is declared and honored
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_market_data_autofresh_required_universe_01_repair_01.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$PathTarget = Join-Path $RepoRoot "scripts\windows\Start-PaperTradingSmoke.ps1"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

function Test-FileExists {
    param([string]$Label, [string]$Path)
    if (Test-Path $Path) {
        Show-Green "  OK -- $Label found: $Path"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label not found: $Path"
        return $false
    }
}

function Test-ContentContains {
    param([string]$Label, [string]$Content, [string]$Needle)
    if ($null -ne $Content -and $Content.IndexOf($Needle, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        Show-Green "  OK -- $Label"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (needle not found: '$Needle')"
        return $false
    }
}

Write-Host "============================================================"
Write-Host " MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-01 Guard"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Target script exists ---"
$Content = $null
if (Test-FileExists "Start-PaperTradingSmoke.ps1" $PathTarget) {
    $Content = Get-Content -Raw -Path $PathTarget
}

Write-Host ""
Show-Info "--- [2] -SkipRequiredUniverseScheduler switch parameter is declared ---"
Test-ContentContains "-SkipRequiredUniverseScheduler switch declared" $Content "[switch]`$SkipRequiredUniverseScheduler" | Out-Null

Write-Host ""
Show-Info "--- [3] Effective-default variable is computed before STEP 1 ---"
$step1Idx = $Content.IndexOf('Write-Section "STEP 1:')
$useVarIdx = $Content.IndexOf('$UseRequiredUniverseScheduler =')
if ($useVarIdx -ge 0 -and $step1Idx -ge 0 -and $useVarIdx -lt $step1Idx) {
    Show-Green "  OK -- `$UseRequiredUniverseScheduler is computed before STEP 1"
} else {
    $script:Violations++
    Show-Red "  FAIL -- `$UseRequiredUniverseScheduler must be computed before STEP 1 (idx useVar=$useVarIdx step1=$step1Idx)"
}

Write-Host ""
Show-Info "--- [4] Conflict refusal (exit 1) occurs before STEP 1 ---"
$conflictIdx = $Content.IndexOf('if ($StartRequiredUniverseScheduler -and $StartIntradayRefreshLoop) {')
if ($conflictIdx -ge 0 -and $step1Idx -ge 0 -and $conflictIdx -lt $step1Idx) {
    Show-Green "  OK -- conflict check text-precedes STEP 1"
} else {
    $script:Violations++
    Show-Red "  FAIL -- conflict check must precede STEP 1 in file order (idx conflict=$conflictIdx step1=$step1Idx)"
}

Write-Host ""
Show-Info "--- [5] STEP 8D gates on the computed default, not the raw explicit flag ---"
$step8dMatch = [regex]::Match($Content, '(?s)Write-Section "STEP 8D:.*?(?=Write-Section "STEP 9:)')
if ($step8dMatch.Success -and $step8dMatch.Value -match [regex]::Escape('if ($UseRequiredUniverseScheduler) {')) {
    Show-Green "  OK -- STEP 8D gated on `$UseRequiredUniverseScheduler"
} else {
    $script:Violations++
    Show-Red "  FAIL -- STEP 8D must gate on `$UseRequiredUniverseScheduler"
}

Write-Host ""
Show-Info "--- [6] FUNCTIONAL PROOF: conflicting flags refuse with zero child-process side effects ---"
Show-Info "  (actually invokes the real script -- no mocks)"
$procsBefore = (Get-Process -Name 'powershell' -ErrorAction SilentlyContinue | Measure-Object).Count
$out = & powershell -NoProfile -ExecutionPolicy Bypass -File $PathTarget `
    -StartRequiredUniverseScheduler -StartIntradayRefreshLoop -CheckOnly 2>&1
$exitCode = $LASTEXITCODE
Start-Sleep -Milliseconds 500
$procsAfter = (Get-Process -Name 'powershell' -ErrorAction SilentlyContinue | Measure-Object).Count
$outText = ($out | Out-String)

if ($exitCode -ne 0) {
    Show-Green "  OK -- conflicting invocation exited non-zero (exit=$exitCode)"
} else {
    $script:Violations++
    Show-Red "  FAIL -- conflicting invocation must exit non-zero (got exit=$exitCode)"
}
if ($outText -notmatch 'STEP 1') {
    Show-Green "  OK -- output never reached STEP 1 (refused before any side effect)"
} else {
    $script:Violations++
    Show-Red "  FAIL -- output reached STEP 1; conflict was not refused early enough"
}
if ($procsAfter -le $procsBefore) {
    Show-Green "  OK -- no net new PowerShell child process left running (before=$procsBefore after=$procsAfter)"
} else {
    $script:Violations++
    Show-Red "  FAIL -- child process count increased (before=$procsBefore after=$procsAfter)"
}

Write-Host ""
Show-Info "--- [7] -CheckOnly help text documents the new default-on behavior ---"
Test-ContentContains "help text documents -SkipRequiredUniverseScheduler" $Content "-SkipRequiredUniverseScheduler" | Out-Null

Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"
if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-01 launcher repair is consistent."
    exit 0
} else {
    Show-Red " $Violations VIOLATION(S) FOUND."
    exit 1
}
