# =============================================================================
# BACKTEST-MULTIPLIER-MARGIN-01 -- Completion Audit / Closure Validator
# =============================================================================
# Pure docs/text validation of the BACKTEST-MULTIPLIER-MARGIN-01 backtest
# economics completion audit and (once written) closure decision. No network
# call, no provider/broker call, no DB connection, no daemon start, no
# cargo/npm build or test.
#
# Checks:
#   [1]  Completion audit spec exists.
#   [2]  Audit doc mentions every known backtest-economics sub-slice.
#   [3]  Audit doc mentions all seven entry-point/artifact surfaces.
#   [4]  Audit doc mentions margin metadata vs. margin enforcement truth.
#   [5]  Audit doc does not claim live trading uses this economics.
#   [6]  Audit doc does not claim margin is enforced.
#   [7]  Audit doc does not claim non-equity trading is enabled.
#   [8]  If present, closure decision doc carries an honest status label.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_backtest_multiplier_margin_01_completion.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec   = Join-Path $RepoRoot "docs\specs\backtest_multiplier_margin_01_completion_audit.md"
$PathClosureSpec = Join-Path $RepoRoot "docs\specs\backtest_multiplier_margin_01_closure_decision.md"

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
Write-Host " BACKTEST-MULTIPLIER-MARGIN-01 Completion Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] Completion audit spec exists ---"
$AuditContent = $null
if (Test-FileExists "Completion audit spec" $PathAuditSpec) {
    $AuditContent = Get-Content -Raw -Path $PathAuditSpec
}

Write-Host ""
Show-Info "--- [2] Audit doc mentions every known sub-slice ---"
$SubSlices = @(
    "BACKTEST-MULTIPLIER-MARGIN-01-COMBINED",
    "BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED",
    "BACKTEST-ECONOMICS-CONFIG-READY-01",
    "BACKTEST-REPORT-FIXTURE-READY-01-COMBINED",
    "BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED",
    "BACKTEST-ECONOMICS-CLI-ENTRY-01-COMBINED",
    "BACKTEST-ECONOMICS-DB-CLI-ENTRY-01-COMBINED",
    "BACKTEST-ECONOMICS-DAEMON-JOB-REQUEST-01-COMBINED",
    "BACKTEST-ECONOMICS-GUI-REGISTRY-01-COMBINED",
    "BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01-COMBINED",
    "INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED"
)
$MissingSubSlices = 0
foreach ($SubSlice in $SubSlices) {
    if ($null -eq $AuditContent -or -not $AuditContent.Contains($SubSlice)) {
        $MissingSubSlices++
        $Violations++
        Show-Red "  FAIL -- audit doc does not mention $SubSlice"
    }
}
if ($MissingSubSlices -eq 0) {
    Show-Green "  OK -- audit doc mentions all known sub-slices"
}

Write-Host ""
Show-Info "--- [3] Audit doc mentions all entry-point/artifact surfaces ---"
$Surfaces = @(
    "csv", "db", "csv-sweep", "daemon", "gui", "metrics.json", "manifest.json", "report.md"
)
$MissingSurfaces = 0
foreach ($Surface in $Surfaces) {
    if ($null -eq $AuditContent -or $AuditContent.IndexOf($Surface, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
        $MissingSurfaces++
        $Violations++
        Show-Red "  FAIL -- audit doc does not mention surface '$Surface'"
    }
}
if ($MissingSurfaces -eq 0) {
    Show-Green "  OK -- audit doc mentions all entry-point/artifact surfaces"
}

Write-Host ""
Show-Info "--- [4] Audit doc addresses margin metadata vs. enforcement ---"
Test-ContentContains "audit doc distinguishes margin metadata from margin enforcement" `
    $AuditContent "margin_enforced" | Out-Null

Write-Host ""
Show-Info "--- [5]-[7] No forbidden trading/enforcement/cutover claims ---"
$ContentLower = $null
if ($null -ne $AuditContent) { $ContentLower = $AuditContent.ToLowerInvariant() }

$ForbiddenPhrases = @(
    "live trading uses this economics",
    "margin is enforced",
    "margin enforcement is live",
    "crypto trading is enabled",
    "futures trading is enabled",
    "options trading is enabled",
    "forex trading is enabled",
    "non-equity trading is enabled",
    "trading is now enabled",
    "production cutover is complete"
)
$PhraseViolations = 0
foreach ($Phrase in $ForbiddenPhrases) {
    if ($null -ne $ContentLower -and $ContentLower.Contains($Phrase)) {
        $PhraseViolations++
        $Violations++
        Show-Red "  FAIL -- audit doc contains forbidden claim: '$Phrase'"
    }
}
if ($PhraseViolations -eq 0) {
    Show-Green "  OK -- no forbidden trading/enforcement/cutover claim found"
}

Write-Host ""
Show-Info "--- [8] Closure decision doc (if present) carries an honest status ---"
if (Test-Path $PathClosureSpec) {
    $ClosureContent = Get-Content -Raw -Path $PathClosureSpec
    $HonestLabels = @(
        "CLOSED_LOCAL / BACKTEST-COMPLETE",
        "PARTIAL / SAFE-GAPS-REMAIN",
        "PARTIAL / PRODUCTION-ACCOUNTING-BOUNDARY-OPEN"
    )
    $HasHonestLabel = $false
    foreach ($Label in $HonestLabels) {
        if ($ClosureContent.Contains($Label)) {
            $HasHonestLabel = $true
        }
    }
    if ($HasHonestLabel) {
        Show-Green "  OK -- closure decision doc carries a recognized honest status label"
    } else {
        $Violations++
        Show-Red "  FAIL -- closure decision doc exists but carries no recognized status label"
    }
} else {
    Show-Info "  SKIP -- closure decision doc not yet written (expected before Phase C)"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- BACKTEST-MULTIPLIER-MARGIN-01 completion evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
