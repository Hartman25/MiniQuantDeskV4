# =============================================================================
# MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-02 -- Startup Fail-Closed Guard
# =============================================================================
# Validates the Defect A repair in Start-PaperTradingSmoke.ps1: normal Paper
# startup (STEP 8D) must fail closed -- refuse to proceed toward
# reconcile/arm -- unless real (non-dry-run) required-universe data-
# maintenance authority is proven, and must not treat a 200/409 response
# alone as proof.
#
# FUNCTIONAL PROOF, not just static text matching: this guard extracts the
# real `Confirm-RequiredUniverseSchedulerOwnership` / `Start-OrVerifyRequired
# UniverseScheduler` function bodies out of the real Start-PaperTradingSmoke.ps1
# (via regex -- both are self-contained, top-level functions whose own
# closing brace is unindented, so extraction is unambiguous), loads them
# into this guard's own scope, then shadows `Invoke-DaemonGet`/
# `Invoke-DaemonPost` with mocked HTTP responses to exercise every fail-
# closed branch (§16 CASE A-E, plus extra coverage) -- zero real daemon,
# zero network, zero DB, zero order/runtime side effects.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_market_data_autofresh_required_universe_01_repair_02.ps1
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

Write-Host "============================================================"
Write-Host " MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-02 Guard"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Target script exists ---"
if (-not (Test-Path $PathTarget)) {
    Show-Red "  FAIL -- Start-PaperTradingSmoke.ps1 not found: $PathTarget"
    exit 1
}
Show-Green "  OK -- Start-PaperTradingSmoke.ps1 found: $PathTarget"
$Content = Get-Content -Raw -Path $PathTarget

# ---------------------------------------------------------------------------
# [2] Extract the two ownership-establishment functions out of the real
# script (function extraction/shadowing test seam -- no real daemon/network).
# ---------------------------------------------------------------------------
Write-Host ""
Show-Info "--- [2] Extract Confirm-RequiredUniverseSchedulerOwnership / Start-OrVerifyRequiredUniverseScheduler ---"

function Get-FunctionSource {
    param([string]$Content, [string]$FunctionName)
    $match = [regex]::Match($Content, "(?ms)^function $([regex]::Escape($FunctionName)) \{.*?^\}")
    if (-not $match.Success) { return $null }
    return $match.Value
}

$confirmSrc = Get-FunctionSource -Content $Content -FunctionName 'Confirm-RequiredUniverseSchedulerOwnership'
$startSrc   = Get-FunctionSource -Content $Content -FunctionName 'Start-OrVerifyRequiredUniverseScheduler'

if ($null -eq $confirmSrc) {
    $script:Violations++
    Show-Red "  FAIL -- could not extract function Confirm-RequiredUniverseSchedulerOwnership from target script"
} else {
    Show-Green "  OK -- extracted Confirm-RequiredUniverseSchedulerOwnership ($($confirmSrc.Length) chars)"
}
if ($null -eq $startSrc) {
    $script:Violations++
    Show-Red "  FAIL -- could not extract function Start-OrVerifyRequiredUniverseScheduler from target script"
} else {
    Show-Green "  OK -- extracted Start-OrVerifyRequiredUniverseScheduler ($($startSrc.Length) chars)"
}

if ($null -eq $confirmSrc -or $null -eq $startSrc) {
    Show-Red "  Cannot continue functional proof without both functions -- aborting."
    Write-Host ""
    Write-Host "============================================================"
    Show-Red " $Violations VIOLATION(S) FOUND."
    exit 1
}

# Load the extracted, real function bodies into this guard's own scope.
Invoke-Expression $confirmSrc
Invoke-Expression $startSrc

# ---------------------------------------------------------------------------
# Mocked HTTP seam -- shadows the real Invoke-DaemonGet/Invoke-DaemonPost so
# the extracted functions (which call them by name) resolve to these mocks
# instead of making any real HTTP call.
# ---------------------------------------------------------------------------
$script:MockPostResult = $null
$script:MockPostThrows = $false
$script:MockGetResult  = $null
$script:MockGetThrows  = $false

function Invoke-DaemonPost {
    param([string]$Path, [hashtable]$Body)
    if ($script:MockPostThrows) { throw "mocked required-universe/start request failure (no real HTTP call made)" }
    return $script:MockPostResult
}

function Invoke-DaemonGet {
    param([string]$Path, [switch]$AuthRequired)
    if ($script:MockGetThrows) { throw "mocked required-universe/status request failure (no real HTTP call made)" }
    return $script:MockGetResult
}

function Reset-Mocks {
    $script:MockPostResult = $null
    $script:MockPostThrows = $false
    $script:MockGetResult  = $null
    $script:MockGetThrows  = $false
}

function New-FakeReport {
    param(
        [string]$OverallState = 'ready',
        [bool]$IsTradingDay = $true,
        [string]$MarketDate = '2026-08-12',
        [array]$Requirements = @()
    )
    [pscustomobject]@{
        market_date    = $MarketDate
        overall_state  = $OverallState
        is_trading_day = $IsTradingDay
        requirements   = $Requirements
        groups         = @()
    }
}

$blockedRequirement = [pscustomobject]@{
    symbol          = 'ZZFAKEBLOCKED'
    timeframe       = '5m'
    freshness_state = 'instrument_registry_invalid'
    blockers        = @("instrument '{ZZFAKEBLOCKED}' is disabled in the instrument registry")
}

function Assert-Case {
    param(
        [string]$CaseLabel,
        [pscustomobject]$Result,
        [bool]$ExpectedEstablished,
        [string]$ExpectedReason = $null
    )
    if ($Result.Established -ne $ExpectedEstablished) {
        $script:Violations++
        Show-Red "  FAIL -- ${CaseLabel}: expected Established=$ExpectedEstablished, got $($Result.Established) (Reason=$($Result.Reason) Detail=$($Result.Detail))"
        return
    }
    if ($ExpectedReason -and $Result.Reason -ne $ExpectedReason) {
        $script:Violations++
        Show-Red "  FAIL -- ${CaseLabel}: expected Reason=$ExpectedReason, got $($Result.Reason)"
        return
    }
    Show-Green "  OK -- ${CaseLabel}: Established=$($Result.Established) Reason=$($Result.Reason)"
}

# ---------------------------------------------------------------------------
# [3] CASE A: normal startup scheduler POST fails -> must not establish
#     authority (STEP 8D's caller must then refuse before reconcile/arm).
# ---------------------------------------------------------------------------
Write-Host ""
Show-Info "--- [3] CASE A: required-universe/start POST request fails ---"
Reset-Mocks
$script:MockPostThrows = $true
$r = Start-OrVerifyRequiredUniverseScheduler -DryRun $false
Assert-Case "CASE A" $r $false 'REQUIRED_UNIVERSE_SCHEDULER_START_REQUEST_FAILED'

# ---------------------------------------------------------------------------
# [4] CASE B: POST returns 200 but report overall_state=blocked -> must fail
#     before reconcile/arm.
# ---------------------------------------------------------------------------
Write-Host ""
Show-Info "--- [4] CASE B: required-universe/start returns 200 with overall_state=blocked ---"
Reset-Mocks
$script:MockPostResult = [pscustomobject]@{
    StatusCode = 200
    Body       = [pscustomobject]@{ report = (New-FakeReport -OverallState 'blocked' -Requirements @($blockedRequirement)) }
}
$r = Start-OrVerifyRequiredUniverseScheduler -DryRun $false
Assert-Case "CASE B" $r $false 'REQUIRED_UNIVERSE_SCHEDULER_BLOCKED'
if ($r.Detail -notmatch 'ZZFAKEBLOCKED') {
    $script:Violations++
    Show-Red "  FAIL -- CASE B: blocked detail must surface the per-requirement blocker (got: $($r.Detail))"
} else {
    Show-Green "  OK -- CASE B: blocked detail surfaces the per-requirement blocker"
}

# ---------------------------------------------------------------------------
# [5] CASE C: POST returns 409 and status running=true dry_run=true -> fail
#     (a dry-run owner is not maintenance authority).
# ---------------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5] CASE C: 409 already_running, existing owner is dry_run=true ---"
Reset-Mocks
$script:MockPostResult = [pscustomobject]@{
    StatusCode = 409
    Body       = [pscustomobject]@{ error = 'required-universe scheduler is already running' }
}
$script:MockGetResult = [pscustomobject]@{
    running = $true
    dry_run = $true
    report  = (New-FakeReport -OverallState 'ready')
}
$r = Start-OrVerifyRequiredUniverseScheduler -DryRun $false
Assert-Case "CASE C" $r $false 'REQUIRED_UNIVERSE_SCHEDULER_BLOCKED_DRY_RUN_OWNER'

# ---------------------------------------------------------------------------
# [6] CASE D: POST returns 409 and status running=true dry_run=false with a
#     valid current report -> verified reuse, continue.
# ---------------------------------------------------------------------------
Write-Host ""
Show-Info "--- [6] CASE D: 409 already_running, existing owner is a genuine non-dry-run scheduler ---"
Reset-Mocks
$script:MockPostResult = [pscustomobject]@{
    StatusCode = 409
    Body       = [pscustomobject]@{ error = 'required-universe scheduler is already running' }
}
$script:MockGetResult = [pscustomobject]@{
    running = $true
    dry_run = $false
    report  = (New-FakeReport -OverallState 'ready')
}
$r = Start-OrVerifyRequiredUniverseScheduler -DryRun $false
Assert-Case "CASE D" $r $true 'REQUIRED_UNIVERSE_SCHEDULER_VERIFIED_REUSE'

# ---------------------------------------------------------------------------
# [7] CASE E: non-trading day report overall_state=not_applicable -> valid
#     no-work result, never a false failure solely because running=false.
# ---------------------------------------------------------------------------
Write-Host ""
Show-Info "--- [7] CASE E: non-trading day, overall_state=not_applicable ---"
Reset-Mocks
$script:MockPostResult = [pscustomobject]@{
    StatusCode = 200
    Body       = [pscustomobject]@{ report = (New-FakeReport -OverallState 'not_applicable' -IsTradingDay $false) }
}
$r = Start-OrVerifyRequiredUniverseScheduler -DryRun $false
Assert-Case "CASE E" $r $true 'REQUIRED_UNIVERSE_NO_WORK_NOT_APPLICABLE'

# ---------------------------------------------------------------------------
# [8] Extra coverage beyond the mandatory A-E set.
# ---------------------------------------------------------------------------
Write-Host ""
Show-Info "--- [8] Extra coverage: new-active success, unexpected HTTP status, 200-but-not-running ---"

# 200 + overall_state=ready, and the follow-up status GET proves real
# ownership (running=true, dry_run=false) -- the success path of §3A.
Reset-Mocks
$script:MockPostResult = [pscustomobject]@{
    StatusCode = 200
    Body       = [pscustomobject]@{ report = (New-FakeReport -OverallState 'ready') }
}
$script:MockGetResult = [pscustomobject]@{
    running = $true
    dry_run = $false
    report  = (New-FakeReport -OverallState 'ready')
}
$r = Start-OrVerifyRequiredUniverseScheduler -DryRun $false
Assert-Case "EXTRA new-active success" $r $true 'REQUIRED_UNIVERSE_SCHEDULER_NEW_ACTIVE'

# Neither 200 nor 409 -> fail closed, never a "non-fatal" warning.
Reset-Mocks
$script:MockPostResult = [pscustomobject]@{
    StatusCode = 500
    Body       = [pscustomobject]@{ error = 'internal error' }
}
$r = Start-OrVerifyRequiredUniverseScheduler -DryRun $false
Assert-Case "EXTRA unexpected HTTP 500" $r $false 'REQUIRED_UNIVERSE_SCHEDULER_START_HTTP_500'

# 200 + ready, but the follow-up status GET says running=false (e.g. it
# crashed/settled immediately) -- a 200 response is not proof by itself.
Reset-Mocks
$script:MockPostResult = [pscustomobject]@{
    StatusCode = 200
    Body       = [pscustomobject]@{ report = (New-FakeReport -OverallState 'ready') }
}
$script:MockGetResult = [pscustomobject]@{
    running = $false
    dry_run = $false
    report  = (New-FakeReport -OverallState 'ready')
}
$r = Start-OrVerifyRequiredUniverseScheduler -DryRun $false
Assert-Case "EXTRA 200-but-not-running" $r $false 'REQUIRED_UNIVERSE_SCHEDULER_NOT_RUNNING'

# ---------------------------------------------------------------------------
# [9] STEP 8D gates on the new fail-closed helper and exits 1 when the
# helper reports Established=$false (static structural proof, complementing
# the functional proofs above).
# ---------------------------------------------------------------------------
Write-Host ""
Show-Info "--- [9] STEP 8D fails closed (exit 1) when ownership was not established ---"
$step8dMatch = [regex]::Match($Content, '(?s)Write-Section "STEP 8D:.*?(?=Write-Section "STEP 9:)')
if ($step8dMatch.Success -and
    $step8dMatch.Value -match [regex]::Escape('Start-OrVerifyRequiredUniverseScheduler') -and
    $step8dMatch.Value -match [regex]::Escape('if (-not $ruResult.Established)') -and
    $step8dMatch.Value -match [regex]::Escape('exit 1')) {
    Show-Green "  OK -- STEP 8D calls Start-OrVerifyRequiredUniverseScheduler and exits 1 when not Established"
} else {
    $script:Violations++
    Show-Red "  FAIL -- STEP 8D must call Start-OrVerifyRequiredUniverseScheduler and exit 1 when not Established"
}
if ($step8dMatch.Success -and $step8dMatch.Value -match [regex]::Escape('(non-fatal)')) {
    $script:Violations++
    Show-Red "  FAIL -- STEP 8D must not contain any '(non-fatal)' warning wording on the default path anymore"
} else {
    Show-Green "  OK -- STEP 8D no longer treats scheduler-establishment failure as non-fatal"
}

Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"
if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-02 startup fail-closed repair is consistent."
    exit 0
} else {
    Show-Red " $Violations VIOLATION(S) FOUND."
    exit 1
}
