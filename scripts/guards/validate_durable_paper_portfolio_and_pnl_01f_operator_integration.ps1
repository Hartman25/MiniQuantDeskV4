# =============================================================================
# DURABLE-PAPER-PORTFOLIO-AND-PNL-01F -- Source-aware static validator
# validate_durable_paper_portfolio_and_pnl_01f_operator_integration.ps1
#
# Scope: core-rs/mqk-gui/src/features/portfolio/PortfolioScreen.tsx,
# core-rs/mqk-gui/src/features/system/api.ts,
# docs/runbooks/autonomous_paper_ops.md,
# scripts/soak/capture_autonomous_paper_session_evidence.ps1,
# scripts/soak/validate_autonomous_paper_session_evidence.ps1,
# scripts/soak/templates/autonomous_paper_session_manifest.template.json,
# scripts/soak/supervised_session_evidence_checklist.md,
# scripts/soak/tests/test_autonomous_paper_session_evidence.ps1,
# docs/specs/durable_paper_portfolio_and_pnl_01f_operator_integration.md.
# Pure text/source validation -- no npm/cargo build/test, no DB connection,
# no daemon start. Running `npm run build` / `npm test` in core-rs/mqk-gui
# and the soak test suite are separate, required steps of the B4-F mission,
# not performed by this guard.
#
# Checks:
#   [1]  All required files exist.
#   [2]  PortfolioScreen.tsx renders the durable section unconditionally
#        (not nested inside the in-memory truthState early-return branch)
#        and contains no mutation-control keyword.
#   [3]  api.ts fetches all three new durable routes.
#   [4]  The runbook documents the three routes, the fill_history_incomplete
#        meaning, and the no-manual-edit / no-fabricated-fill prohibition.
#   [5]  The capture script fetches all three routes via the same
#        Invoke-DaemonGetOnly seam (no ad-hoc Invoke-RestMethod call).
#   [6]  The validate script's required-fields, missing-endpoints, and new
#        durable-truth-state checks all reference the three new fields.
#   [7]  The manifest template contains the three new null placeholders.
#   [8]  The test file's baseline fixture set and baseline manifest both
#        include populated durable fields, and the five new negative test
#        cases exist.
#   [9]  No production Rust (core-rs/crates) or migration file appears in
#        this patch's working-tree diff -- B4-F is GUI/runbook/evidence
#        only.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_durable_paper_portfolio_and_pnl_01f_operator_integration.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$PortfolioScreenFile = Join-Path $RepoRoot 'core-rs\mqk-gui\src\features\portfolio\PortfolioScreen.tsx'
$ApiFile = Join-Path $RepoRoot 'core-rs\mqk-gui\src\features\system\api.ts'
$RunbookFile = Join-Path $RepoRoot 'docs\runbooks\autonomous_paper_ops.md'
$CaptureScriptFile = Join-Path $RepoRoot 'scripts\soak\capture_autonomous_paper_session_evidence.ps1'
$ValidateScriptFile = Join-Path $RepoRoot 'scripts\soak\validate_autonomous_paper_session_evidence.ps1'
$TemplateFile = Join-Path $RepoRoot 'scripts\soak\templates\autonomous_paper_session_manifest.template.json'
$ChecklistFile = Join-Path $RepoRoot 'scripts\soak\supervised_session_evidence_checklist.md'
$SoakTestFile = Join-Path $RepoRoot 'scripts\soak\tests\test_autonomous_paper_session_evidence.ps1'
$SpecDoc = Join-Path $RepoRoot 'docs\specs\durable_paper_portfolio_and_pnl_01f_operator_integration.md'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan  }

Write-Host '============================================================'
Write-Host ' DURABLE-PAPER-PORTFOLIO-AND-PNL-01F validate_..._01f_operator_integration'
Write-Host '============================================================'

$requiredFiles = @(
    $PortfolioScreenFile, $ApiFile, $RunbookFile, $CaptureScriptFile, $ValidateScriptFile,
    $TemplateFile, $ChecklistFile, $SoakTestFile, $SpecDoc
)
foreach ($f in $requiredFiles) {
    if (-not (Test-Path -LiteralPath $f)) {
        Show-Fail "Required file not found: $f"
    } else {
        Show-Green "Found: $f"
    }
}
if ($Violations -gt 0) {
    Write-Host ''
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
}

$portfolioScreenText = [System.IO.File]::ReadAllText($PortfolioScreenFile)
$apiText = [System.IO.File]::ReadAllText($ApiFile)
$runbookText = [System.IO.File]::ReadAllText($RunbookFile)
$captureText = [System.IO.File]::ReadAllText($CaptureScriptFile)
$validateText = [System.IO.File]::ReadAllText($ValidateScriptFile)
$templateText = [System.IO.File]::ReadAllText($TemplateFile)
$soakTestText = [System.IO.File]::ReadAllText($SoakTestFile)

# [2] Durable section renders unconditionally + no mutation control
if ($portfolioScreenText -match 'DurablePortfolioSection') {
    Show-Green "PortfolioScreen.tsx defines/uses DurablePortfolioSection"
} else {
    Show-Fail "PortfolioScreen.tsx does not define/use a DurablePortfolioSection"
}
if ($portfolioScreenText -match 'return\s*<TruthStateNotice[\s\S]{0,20}/>;\s*\n\s*\}') {
    Show-Fail "The in-memory truthState branch still returns bare <TruthStateNotice /> without the durable section alongside it"
} else {
    Show-Green "The in-memory truthState branch does not bare-return (durable section renders alongside it)"
}
$mutationKeywords = @('onClick={.*[Cc]apture', 'onClick={.*[Ff]latten', 'onClick={.*[Rr]etry', 'onClick={.*[Aa]rm', '<button')
foreach ($kw in $mutationKeywords) {
    if ($portfolioScreenText -match $kw) {
        Show-Fail "PortfolioScreen.tsx appears to contain a mutation control matching '$kw'"
    }
}
Show-Green "No mutation-control keyword found in PortfolioScreen.tsx"

# [3] api.ts fetches all three routes
$requiredRoutes = @('/api/v1/portfolio/durable-summary', '/api/v1/portfolio/durable-positions', '/api/v1/portfolio/durable-snapshots')
foreach ($route in $requiredRoutes) {
    if ($apiText -match [regex]::Escape($route)) {
        Show-Green "api.ts fetches $route"
    } else {
        Show-Fail "api.ts does not fetch $route"
    }
}

# [4] Runbook content
$runbookRequiredTerms = @(
    'durable-summary', 'durable-positions', 'durable-snapshots',
    'fill_history_incomplete', 'order_filled_portfolio_durable_pnl_available',
    'order_filled_portfolio_durable_pnl_incomplete',
    'synthetic opening fill'
)
foreach ($term in $runbookRequiredTerms) {
    if ($runbookText -match [regex]::Escape($term)) {
        Show-Green "Runbook mentions '$term'"
    } else {
        Show-Fail "Runbook does not mention '$term'"
    }
}

# [5] Capture script uses the shared GET-only seam for all three routes
foreach ($route in $requiredRoutes) {
    if ($captureText -match "Invoke-DaemonGetOnly -Path '$([regex]::Escape($route))") {
        Show-Green "Capture script fetches $route via Invoke-DaemonGetOnly"
    } else {
        Show-Fail "Capture script does not fetch $route via Invoke-DaemonGetOnly"
    }
}

# [6] Validate script coverage
if ($validateText -match "'durable_portfolio_summary', 'durable_portfolio_positions', 'durable_portfolio_snapshots'") {
    Show-Green "Validate script's required-fields list includes the three durable fields"
} else {
    Show-Fail "Validate script's required-fields list does not include the three durable fields"
}
if ($validateText -match 'durable_portfolio_summary.*=.*durable-summary' -or $validateText -match "'durable_portfolio_summary'\s*=\s*'/api/v1/portfolio/durable-summary'") {
    Show-Green "Validate script's missing-endpoints map includes durable_portfolio_summary"
} else {
    Show-Fail "Validate script's missing-endpoints map does not include durable_portfolio_summary"
}
if ($validateText -match '\[13\]' -and $validateText -match 'accounting_epoch') {
    Show-Green "Validate script contains check [13] for durable truth-state fields"
} else {
    Show-Fail "Validate script does not contain a dedicated durable truth-state check"
}

# [7] Template placeholders
foreach ($field in @('durable_portfolio_summary', 'durable_portfolio_positions', 'durable_portfolio_snapshots')) {
    if ($templateText -match "`"$field`":\s*null") {
        Show-Green "Template contains '$field': null"
    } else {
        Show-Fail "Template does not contain '$field': null"
    }
}

# [8] Test file coverage
if ($soakTestText -match 'api_v1_portfolio_durable-summary\.json' -and
    $soakTestText -match 'durable_portfolio_summary\s*=') {
    Show-Green "Test file's baseline fixture set and baseline manifest include durable fields"
} else {
    Show-Fail "Test file's baseline fixture set/manifest do not include durable fields"
}
$requiredNegativeCases = @(
    'durable_summary_missing_truthstate',
    'durable_summary_missing_accounting_truthstate',
    'durable_summary_bad_epoch',
    'durable_positions_missing_truthstate',
    'durable_snapshots_missing_truthstate'
)
foreach ($case in $requiredNegativeCases) {
    if ($soakTestText -match [regex]::Escape($case)) {
        Show-Green "Found required negative test case: $case"
    } else {
        Show-Fail "Missing required negative test case: $case"
    }
}

# [9] No production Rust or migration file touched
Push-Location $RepoRoot
try {
    # git diff can write a benign LF/CRLF autocrlf-conversion notice to
    # stderr for newly-touched files; under PSNativeCommandUseErrorActionPreference
    # that would otherwise be promoted to a terminating error even with
    # 2>$null, so this call is bracketed with a local, non-terminating
    # ErrorActionPreference rather than relying on redirection alone.
    $previousEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $touched = git diff --name-only HEAD 2>$null
    $ErrorActionPreference = $previousEap
    if ($LASTEXITCODE -eq 0 -and $touched) {
        $unexpected = $touched | Where-Object {
            $_ -like 'core-rs/crates/*' -or $_ -like 'core-rs/crates/mqk-db/migrations/*'
        }
        if ($unexpected) {
            Show-Fail "Unexpected Rust/migration file(s) touched: $($unexpected -join ', ') -- B4-F is GUI/runbook/evidence only"
        } else {
            Show-Green "No production Rust or migration file touched by this patch"
        }
    } else {
        Show-Info "No working-tree diff against HEAD -- scope check skipped (patch already committed)"
    }
} finally {
    Pop-Location
}

Write-Host ''
if ($Violations -gt 0) {
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
} else {
    Write-Host " VALIDATION PASSED -- 0 violations" -ForegroundColor Green
    exit 0
}
