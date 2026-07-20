# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4 -- Phase D Closure Validator
# =============================================================================
# Source-aware static validator for the D4 patch
# (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4-INTEGRATED-LIFECYCLE-PROOF-AND-
# PHASE-D-CLOSURE). No network call, no provider/broker call, no DB
# connection, no daemon start, no cargo/npm build or test -- pure text/source
# validation only.
#
# Checks:
#   [1]  main.rs does not spawn the legacy blind-timer autonomous bar ticker.
#   [2]  main.rs spawns the supervised completed-bar driver task exactly once.
#   [3]  main.rs's graceful-shutdown closure awaits
#        cancel_and_wait_completed_bar_task_for_shutdown() strictly before
#        stop_for_shutdown().
#   [4]  The completed-bar driver's claim-and-dispatch function does not use
#        the unsafe shared deposit-then-consume mailbox sequence (D4.2): its
#        body must not call deposit_strategy_bar_input, and must call the
#        exact-input dispatch seam
#        (dispatch_native_strategy_for_symbol_with_bar) directly.
#   [5]  The Phase D closure document exists and documents the confirmed race
#        and its fix.
#   [6]  The required Phase D integration scenario test file exists and
#        contains the full-day lifecycle test.
#   [7]  The master patch ledger does not mark Bundle 3 (or Phase D) closed.
#   [8]  README.md / README_TECHNICAL.md do not claim Bundle 3 is complete or
#        soak-ready.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01d_phase_d_closure.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathMainRs      = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\main.rs"
$PathDriverRs    = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\autonomous_completed_bar_driver.rs"
$PathClosureDoc  = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01d_phase_d_closure.md"
$PathIntegration = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_autonomous_daily_phase_d_integration_01.rs"
$PathLedger      = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"
$PathReadme      = Join-Path $RepoRoot "README.md"
$PathReadmeTech  = Join-Path $RepoRoot "README_TECHNICAL.md"

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

function Test-ContentDoesNotContain {
    param([string]$Label, [string]$Content, [string]$Needle)
    if ($null -eq $Content -or $Content.IndexOf($Needle, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
        Show-Green "  OK -- $Label"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (forbidden needle found: '$Needle')"
        return $false
    }
}

function Test-ContentContainsAll {
    param([string]$Label, [string]$Content, [string[]]$Needles)
    $missing = @()
    foreach ($Needle in $Needles) {
        if ($null -eq $Content -or $Content.IndexOf($Needle, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
            $missing += $Needle
        }
    }
    if ($missing.Count -eq 0) {
        Show-Green "  OK -- $Label"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (missing needle(s): $($missing -join ', '))"
        return $false
    }
}

function Count-Occurrences {
    param([string]$Content, [string]$Needle)
    if ($null -eq $Content) { return 0 }
    $count = 0
    $idx = 0
    while ($true) {
        $found = $Content.IndexOf($Needle, $idx, [System.StringComparison]::OrdinalIgnoreCase)
        if ($found -lt 0) { break }
        $count++
        $idx = $found + $Needle.Length
    }
    return $count
}

function Get-ContentBetween {
    param([string]$Content, [string]$StartNeedle, [string]$EndNeedle)
    if ($null -eq $Content) { return $null }
    $startIdx = $Content.IndexOf($StartNeedle, [System.StringComparison]::OrdinalIgnoreCase)
    if ($startIdx -lt 0) { return $null }
    $endIdx = $Content.IndexOf($EndNeedle, $startIdx + $StartNeedle.Length, [System.StringComparison]::OrdinalIgnoreCase)
    if ($endIdx -lt 0) { $endIdx = $Content.Length }
    return $Content.Substring($startIdx, $endIdx - $startIdx)
}

Write-Host "============================================================"
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4 Phase D Closure Validator"
Write-Host "============================================================"

$MainContent = $null
if (Test-FileExists "main.rs" $PathMainRs) {
    $MainContent = Get-Content -Raw -Path $PathMainRs
}

Write-Host ""
Show-Info "--- [1] main.rs does not spawn the legacy blind-timer autonomous bar ticker ---"
Test-ContentDoesNotContain "main.rs does not call spawn_autonomous_bar_ticker(" $MainContent "spawn_autonomous_bar_ticker(" | Out-Null

Write-Host ""
Show-Info "--- [2] main.rs spawns the supervised completed-bar driver task exactly once ---"
$SpawnCount = Count-Occurrences -Content $MainContent -Needle "spawn_autonomous_completed_bar_driver_task("
if ($SpawnCount -eq 1) {
    Show-Green "  OK -- main.rs calls spawn_autonomous_completed_bar_driver_task( exactly once"
} else {
    $script:Violations++
    Show-Red "  FAIL -- main.rs calls spawn_autonomous_completed_bar_driver_task( $SpawnCount time(s), expected exactly 1"
}

Write-Host ""
Show-Info "--- [3] Shutdown awaits completed-bar-task cancellation strictly before runtime shutdown ---"
$ShutdownBody = Get-ContentBetween -Content $MainContent `
    -StartNeedle "with_graceful_shutdown(async move {" `
    -EndNeedle "})"
if ($null -eq $ShutdownBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate main.rs's graceful-shutdown closure body to scope this check"
} else {
    $cancelIdx = $ShutdownBody.IndexOf("cancel_and_wait_completed_bar_task_for_shutdown", [System.StringComparison]::OrdinalIgnoreCase)
    $stopIdx   = $ShutdownBody.IndexOf("stop_for_shutdown", [System.StringComparison]::OrdinalIgnoreCase)
    if ($cancelIdx -lt 0) {
        $script:Violations++
        Show-Red "  FAIL -- shutdown closure does not call cancel_and_wait_completed_bar_task_for_shutdown"
    } elseif ($stopIdx -lt 0) {
        $script:Violations++
        Show-Red "  FAIL -- shutdown closure does not call stop_for_shutdown"
    } elseif ($cancelIdx -lt $stopIdx) {
        Show-Green "  OK -- cancel_and_wait_completed_bar_task_for_shutdown is called strictly before stop_for_shutdown"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- stop_for_shutdown is called before (or without) cancel_and_wait_completed_bar_task_for_shutdown"
    }
}

Write-Host ""
Show-Info "--- [4] Completed-bar driver does not use the unsafe shared deposit-then-consume sequence (D4.2) ---"
$DriverContent = $null
if (Test-FileExists "autonomous_completed_bar_driver.rs" $PathDriverRs) {
    $DriverContent = Get-Content -Raw -Path $PathDriverRs
}
$ClaimFnBody = Get-ContentBetween -Content $DriverContent `
    -StartNeedle "async fn claim_and_dispatch_observed_bar(" `
    -EndNeedle "`n// ---"
if ($null -eq $ClaimFnBody) {
    $script:Violations++
    Show-Red "  FAIL -- could not locate claim_and_dispatch_observed_bar's body to scope this check"
} else {
    # Matched against the call form (with the opening paren) so an
    # explanatory doc comment referencing the old function name by its bare
    # identifier (no trailing paren) does not false-positive this check.
    Test-ContentDoesNotContain "claim_and_dispatch_observed_bar does not call deposit_strategy_bar_input(" $ClaimFnBody "deposit_strategy_bar_input(" | Out-Null
    Test-ContentDoesNotContain "claim_and_dispatch_observed_bar does not call tick_strategy_dispatch_for_symbol(" $ClaimFnBody "tick_strategy_dispatch_for_symbol(" | Out-Null
    Test-ContentContains "claim_and_dispatch_observed_bar calls the exact-input dispatch seam directly" $ClaimFnBody "dispatch_native_strategy_for_symbol_with_bar" | Out-Null
}

Write-Host ""
Show-Info "--- [5] Phase D closure document exists and documents the race/fix ---"
$ClosureContent = $null
if (Test-FileExists "Phase D closure document" $PathClosureDoc) {
    $ClosureContent = Get-Content -Raw -Path $PathClosureDoc
}
Test-ContentContainsAll "closure doc documents the confirmed dispatch-ownership race and its fix" $ClosureContent @(
    "pending_strategy_bar_input",
    "dispatch_native_strategy_for_symbol_with_bar",
    "claim_and_dispatch_observed_bar"
) | Out-Null
Test-ContentContains "closure doc states D4 is awaiting acceptance, not closing Bundle 3" $ClosureContent "awaiting ChatGPT and operator acceptance" | Out-Null

Write-Host ""
Show-Info "--- [6] Required Phase D integration scenario test exists ---"
$IntegrationContent = $null
if (Test-FileExists "Phase D integration scenario test" $PathIntegration) {
    $IntegrationContent = Get-Content -Raw -Path $PathIntegration
}
Test-ContentContainsAll "integration test covers the full-day lifecycle and concurrency proofs" $IntegrationContent @(
    "fn phase_d_full_day_lifecycle",
    "fn phase_d_concurrency_forward_ordering_execution_loop_cannot_steal_claimed_bar",
    "fn phase_d_concurrency_reverse_ordering_execution_loop_first_is_also_safe"
) | Out-Null

Write-Host ""
Show-Info "--- [7] Ledger does not mark Bundle 3 or Phase D closed ---"
$LedgerContent = $null
if (Test-FileExists "Master patch ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Path $PathLedger
}
Test-ContentDoesNotContain "ledger does not contain 'Bundle 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED): CLOSED'" $LedgerContent "Bundle 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED): CLOSED" | Out-Null
Test-ContentDoesNotContain "ledger does not contain 'Phase D: CLOSED'" $LedgerContent "Phase D: CLOSED" | Out-Null
Test-ContentContains "ledger documents D4 as awaiting acceptance" $LedgerContent "AWAITING CHATGPT AND OPERATOR ACCEPTANCE" | Out-Null

Write-Host ""
Show-Info "--- [8] README.md / README_TECHNICAL.md do not overclaim Bundle 3 completion ---"
$ReadmeContent = $null
if (Test-Path $PathReadme) { $ReadmeContent = Get-Content -Raw -Path $PathReadme }
$ReadmeTechContent = $null
if (Test-Path $PathReadmeTech) { $ReadmeTechContent = Get-Content -Raw -Path $PathReadmeTech }

$ForbiddenReadmePhrases = @(
    "bundle 3 complete",
    "bundle 3: complete",
    "bundle 3 is complete",
    "bundle 3 closed",
    "soak-ready",
    "soak ready",
    "autonomous trading is approved for unattended operation"
)
foreach ($DocPair in @(@{Name="README.md"; Content=$ReadmeContent}, @{Name="README_TECHNICAL.md"; Content=$ReadmeTechContent})) {
    $DocName = $DocPair.Name
    $DocContentLower = $null
    if ($null -ne $DocPair.Content) { $DocContentLower = $DocPair.Content.ToLowerInvariant() }
    foreach ($Phrase in $ForbiddenReadmePhrases) {
        if ($null -ne $DocContentLower -and $DocContentLower.Contains($Phrase)) {
            $script:Violations++
            Show-Red "  FAIL -- $DocName contains forbidden overclaim: '$Phrase'"
        }
    }
}
if ($null -ne $ReadmeContent -or $null -ne $ReadmeTechContent) {
    Show-Green "  OK -- no forbidden Bundle-3/soak-ready overclaim found in README.md / README_TECHNICAL.md"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D4 Phase D closure evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
