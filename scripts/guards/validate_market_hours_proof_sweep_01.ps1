# =============================================================================
# MARKET-HOURS-PROOF-SWEEP-01 -- Runbook Validator
# =============================================================================
# Pure docs/text validation of the market-hours proof sweep runbook (and, once
# written, its lane closure docs). No network call, no provider/broker call,
# no DB connection, no daemon start, no cargo/npm build or test.
#
# Checks:
#   [1]  Runbook exists.
#   [2]  Runbook mentions AUTON-NO-TRADE-02.
#   [3]  Runbook mentions ASSET-CORE-05.
#   [4]  Runbook mentions MQK_RUNTIME_SESSION_SOURCE=v2_equity_active.
#   [5]  Runbook states no live orders.
#   [6]  Runbook states no forced paper orders.
#   [7]  Runbook states no strategy threshold changes.
#   [8]  Runbook states generated evidence must not be staged.
#   [9]  Runbook instructs schema-discovery-first (information_schema.columns)
#        rather than hardcoding a fixed column list, and does not claim any
#        column categorically does not exist.
#   [10] Lane closure docs (if present) carry an honest status label.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\guards\validate_market_hours_proof_sweep_01.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathRunbook = Join-Path $RepoRoot "docs\runbooks\market_hours_proof_sweep_01.md"

$PathClosureDocs = @(
    (Join-Path $RepoRoot "docs\specs\auton_no_trade_02b_market_hours_observation_summary.md"),
    (Join-Path $RepoRoot "docs\specs\auton_no_trade_02c_market_hours_closure_decision.md"),
    (Join-Path $RepoRoot "docs\specs\asset_core_05k_v2_equity_active_market_hours_proof.md"),
    (Join-Path $RepoRoot "docs\specs\market_hours_proof_sweep_01e_closure_decision.md")
)

# Categorical non-existence claims that would re-introduce the stale-schema
# bug this guard was corrected for (PAPER-SMOKE-FOLLOWUP-01B): the runbook
# must never assert a table lacks a lifecycle/stage timestamp column as a
# blanket fact, since migrations can add columns after the runbook was
# written. Schema-discovery-first (information_schema.columns) is the fix;
# this list catches the old wording pattern if it creeps back in.
$ForbiddenCategoricalClaims = @(
    "has no per-lifecycle-stage wall-clock timestamp columns",
    "has no separate per-outcome wall-clock timestamp column",
    "it has no per-lifecycle-stage",
    "no additional lifecycle timestamp columns exist",
    "no additional per-stage timestamp columns exist"
)

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
        Show-Green "  OK -- $Label (does not contain '$Needle')"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (forbidden term found: '$Needle')"
        return $false
    }
}

Write-Host "============================================================"
Write-Host " MARKET-HOURS-PROOF-SWEEP-01 Runbook Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Runbook exists ---"
$RunbookContent = $null
if (Test-FileExists "Market-hours proof sweep runbook" $PathRunbook) {
    $RunbookContent = Get-Content -Raw -Path $PathRunbook
}

Write-Host ""
Show-Info "--- [2] Runbook mentions AUTON-NO-TRADE-02 ---"
Test-ContentContains "runbook mentions AUTON-NO-TRADE-02" $RunbookContent "AUTON-NO-TRADE-02" | Out-Null

Write-Host ""
Show-Info "--- [3] Runbook mentions ASSET-CORE-05 ---"
Test-ContentContains "runbook mentions ASSET-CORE-05" $RunbookContent "ASSET-CORE-05" | Out-Null

Write-Host ""
Show-Info "--- [4] Runbook mentions MQK_RUNTIME_SESSION_SOURCE=v2_equity_active ---"
Test-ContentContains "runbook mentions MQK_RUNTIME_SESSION_SOURCE=v2_equity_active" $RunbookContent "MQK_RUNTIME_SESSION_SOURCE=v2_equity_active" | Out-Null

Write-Host ""
Show-Info "--- [5] Runbook states no live orders ---"
Test-ContentContains "runbook states no live orders" $RunbookContent "Do not submit live orders" | Out-Null

Write-Host ""
Show-Info "--- [6] Runbook states no forced paper orders ---"
Test-ContentContains "runbook states no forced paper orders" $RunbookContent "Do not force a paper order" | Out-Null

Write-Host ""
Show-Info "--- [7] Runbook states no strategy threshold changes ---"
Test-ContentContains "runbook states no strategy threshold changes" $RunbookContent "Do not change strategy thresholds" | Out-Null

Write-Host ""
Show-Info "--- [8] Runbook states generated evidence must not be staged ---"
Test-ContentContains "runbook states generated evidence must not be staged" $RunbookContent "Do not stage generated evidence" | Out-Null

Write-Host ""
Show-Info "--- [9a] Runbook instructs schema-discovery-first (information_schema.columns) ---"
Test-ContentContains "runbook instructs information_schema.columns discovery" $RunbookContent "information_schema.columns" | Out-Null

Write-Host ""
Show-Info "--- [9b] Runbook does not contain categorical column-non-existence claims ---"
foreach ($Claim in $ForbiddenCategoricalClaims) {
    Test-ContentDoesNotContain "runbook categorical-claim check" $RunbookContent $Claim | Out-Null
}

Write-Host ""
Show-Info "--- [10] Lane closure docs (if present) carry an honest status label ---"
$HonestLabels = @("CLOSED_LOCAL", "CLOSED", "PARTIAL", "OPEN", "BLOCKED")
foreach ($ClosurePath in $PathClosureDocs) {
    if (Test-Path $ClosurePath) {
        $ClosureContent = Get-Content -Raw -Path $ClosurePath
        $HasHonestLabel = $false
        foreach ($Label in $HonestLabels) {
            if ($ClosureContent.Contains($Label)) {
                $HasHonestLabel = $true
            }
        }
        if ($HasHonestLabel) {
            Show-Green "  OK -- $(Split-Path -Leaf $ClosurePath) carries a recognized honest status label"
        } else {
            $Violations++
            Show-Red "  FAIL -- $(Split-Path -Leaf $ClosurePath) exists but carries no recognized status label"
        }
    } else {
        Show-Info "  SKIP -- $(Split-Path -Leaf $ClosurePath) not yet written"
    }
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- MARKET-HOURS-PROOF-SWEEP-01 runbook evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
