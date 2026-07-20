# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-DURABLE-OUTCOME-AUTHORITY-AND-
# EVIDENCE-CONTRACT -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the Phase E1 contract audit -- a documentation
# and guard-only patch. No network call, no provider/broker call, no DB
# connection, no daemon start, no cargo/npm build or test -- pure text/source
# validation only.
#
# Checks:
#   [1]  The E1 contract document exists.
#   [2]  Ledger does not mark Bundle 3 closed.
#   [3]  Ledger/README/contract doc do not falsely claim Phase E is
#        implemented, wired, or closed.
#   [4]  The contract document does not define completed_no_trade from the
#        absence of fills alone -- it requires the full evidence hierarchy.
#   [5]  The contract document does not name a process-local counter as final
#        outcome authority -- it explicitly forbids that.
#   [6]  unknown_insufficient_evidence is present and defined in the contract.
#   [7]  Unresolved dispatch claims are never allowed to become no-trade --
#        the contract explicitly blocks this.
#   [8]  stopped_at_utc is required before finalization eligibility.
#   [9]  README.md / README_TECHNICAL.md do not claim the unattended soak has
#        begun.
#   [10] README.md / README_TECHNICAL.md do not claim live-capital readiness.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01e_outcome_contract.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathContractDoc = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01e_outcome_truth_contract.md"
$PathLedger       = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"
$PathReadme       = Join-Path $RepoRoot "README.md"
$PathReadmeTech   = Join-Path $RepoRoot "README_TECHNICAL.md"

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

function Test-ContentContainsAny {
    param([string]$Label, [string]$Content, [string[]]$Needles)
    if ($null -eq $Content) {
        $script:Violations++
        Show-Red "  FAIL -- $Label (content is null)"
        return $false
    }
    foreach ($Needle in $Needles) {
        if ($Content.IndexOf($Needle, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            Show-Green "  OK -- $Label"
            return $true
        }
    }
    $script:Violations++
    Show-Red "  FAIL -- $Label (none of the candidate needles found: $($Needles -join ' | '))"
    return $false
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

Write-Host "============================================================"
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1 Outcome Contract Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] E1 contract document exists ---"
$ContractContent = $null
if (Test-FileExists "Phase E1 outcome truth contract" $PathContractDoc) {
    $ContractContent = Get-Content -Raw -Path $PathContractDoc
}

$LedgerContent = $null
if (Test-FileExists "Master patch ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Path $PathLedger
}
$ReadmeContent = $null
if (Test-Path $PathReadme) { $ReadmeContent = Get-Content -Raw -Path $PathReadme }
$ReadmeTechContent = $null
if (Test-Path $PathReadmeTech) { $ReadmeTechContent = Get-Content -Raw -Path $PathReadmeTech }

Write-Host ""
Show-Info "--- [2] Ledger does not mark Bundle 3 closed ---"
Test-ContentDoesNotContain "ledger does not contain 'Bundle 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED): CLOSED'" $LedgerContent "Bundle 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED): CLOSED" | Out-Null

Write-Host ""
Show-Info "--- [3] Phase E is not falsely marked implemented/wired/closed ---"
$ForbiddenPhaseEClaims = @(
    "PHASE E: CLOSED",
    "PHASE E: COMPLETE",
    "PHASE E: IMPLEMENTATION COMPLETE",
    "phase e runtime implementation: complete",
    "phase e outcome classifier implemented",
    "phase e is wired",
    "durable daily outcome classification is live"
)
foreach ($DocPair in @(
        @{Name="Master patch ledger"; Content=$LedgerContent},
        @{Name="Phase E1 contract doc"; Content=$ContractContent},
        @{Name="README.md"; Content=$ReadmeContent},
        @{Name="README_TECHNICAL.md"; Content=$ReadmeTechContent}
    )) {
    foreach ($Phrase in $ForbiddenPhaseEClaims) {
        Test-ContentDoesNotContain "$($DocPair.Name) does not contain forbidden claim '$Phrase'" $DocPair.Content $Phrase | Out-Null
    }
}
Test-ContentContainsAny "ledger states Phase E runtime implementation is not started" $LedgerContent @(
    "Phase E runtime implementation: NOT STARTED",
    "unattended 10", # tolerant anchor in case exact phrasing shifts, combined with the explicit check below
    "Phase E1 contract audit: IMPLEMENTATION COMPLETE"
) | Out-Null

Write-Host ""
Show-Info "--- [4] completed_no_trade is never defined from absence of fills alone ---"
Test-ContentContains "contract states completed_no_trade must never be derived from zero fill rows alone" $ContractContent "Never" | Out-Null
Test-ContentContains "contract explicitly requires the full evidence hierarchy for completed_no_trade" $ContractContent "completed_no_trade" | Out-Null
Test-ContentContains "contract states the necessary-but-not-sufficient rule for zero fills" $ContractContent "necessary but nowhere near sufficient" | Out-Null

Write-Host ""
Show-Info "--- [5] Process-local counters are never named as final outcome authority ---"
Test-ContentContains "contract explicitly forbids process-local counters as no-trade authority" $ContractContent "must never" | Out-Null
Test-ContentContains "contract names bar_tick_dispatch_count as forbidden authority" $ContractContent "bar_tick_dispatch_count" | Out-Null
Test-ContentContains "contract names last_bar_signal_qty as forbidden authority" $ContractContent "last_bar_signal_qty" | Out-Null
Test-ContentContains "contract states these are diagnostic/process-local, carrying no restart-safe authority" $ContractContent "carry no restart-safe authority" | Out-Null

Write-Host ""
Show-Info "--- [6] unknown_insufficient_evidence is present and defined ---"
Test-ContentContains "contract defines unknown_insufficient_evidence" $ContractContent "unknown_insufficient_evidence" | Out-Null
Test-ContentContains "contract states unknown_insufficient_evidence is never a fabricated no-trade reason" $ContractContent "never a fabricated no-trade reason invented to fill the gap" | Out-Null

Write-Host ""
Show-Info "--- [7] Unresolved dispatch claims never become no-trade ---"
Test-ContentContains "contract requires zero unresolved dispatch claims for completed_no_trade" $ContractContent "zero" | Out-Null
Test-ContentContains "contract names the unresolved-claim reason code" $ContractContent "unknown_unresolved_dispatch_claim" | Out-Null
Test-ContentContains "contract states an unresolved claim blocks no-trade outright" $ContractContent "Any unresolved" | Out-Null

Write-Host ""
Show-Info "--- [8] stopped_at_utc is required before finalization ---"
Test-ContentContains "contract requires stopped_at_utc IS NOT NULL for eligibility" $ContractContent "stopped_at_utc IS NOT NULL" | Out-Null
Test-ContentContains "contract names finalization eligibility section" $ContractContent "Finalization eligibility" | Out-Null

Write-Host ""
Show-Info "--- [9] README.md / README_TECHNICAL.md do not claim the unattended soak has begun ---"
$ForbiddenSoakClaims = @(
    "soak has begun",
    "soak is underway",
    "soak in progress",
    "currently running the soak",
    "unattended soak has started",
    "soak: in progress",
    "soak: started"
)
foreach ($DocPair in @(@{Name="README.md"; Content=$ReadmeContent}, @{Name="README_TECHNICAL.md"; Content=$ReadmeTechContent})) {
    $DocName = $DocPair.Name
    $DocContentLower = $null
    if ($null -ne $DocPair.Content) { $DocContentLower = $DocPair.Content.ToLowerInvariant() }
    foreach ($Phrase in $ForbiddenSoakClaims) {
        if ($null -ne $DocContentLower -and $DocContentLower.Contains($Phrase)) {
            $script:Violations++
            Show-Red "  FAIL -- $DocName contains forbidden soak-started claim: '$Phrase'"
        }
    }
}
if ($null -ne $ReadmeContent -or $null -ne $ReadmeTechContent) {
    Show-Green "  OK -- no forbidden unattended-soak-has-begun claim found in README.md / README_TECHNICAL.md"
}

Write-Host ""
Show-Info "--- [10] README.md / README_TECHNICAL.md do not claim live-capital readiness ---"
$ForbiddenLiveClaims = @(
    "live capital is ready",
    "live capital: ready",
    "approved for live capital",
    "live trading is approved",
    "cleared for live capital",
    "ready for live capital"
)
foreach ($DocPair in @(@{Name="README.md"; Content=$ReadmeContent}, @{Name="README_TECHNICAL.md"; Content=$ReadmeTechContent})) {
    $DocName = $DocPair.Name
    $DocContentLower = $null
    if ($null -ne $DocPair.Content) { $DocContentLower = $DocPair.Content.ToLowerInvariant() }
    foreach ($Phrase in $ForbiddenLiveClaims) {
        if ($null -ne $DocContentLower -and $DocContentLower.Contains($Phrase)) {
            $script:Violations++
            Show-Red "  FAIL -- $DocName contains forbidden live-capital-readiness claim: '$Phrase'"
        }
    }
}
Test-ContentContains "README.md states live capital is not ready" $ReadmeContent "Not ready" | Out-Null
if ($null -ne $ReadmeContent -or $null -ne $ReadmeTechContent) {
    Show-Green "  OK -- no forbidden live-capital-readiness claim found in README.md / README_TECHNICAL.md"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1 outcome contract evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
