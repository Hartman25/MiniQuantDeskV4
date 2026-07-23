# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F3-SUPERVISED-SOAK-EVIDENCE-PREPARATION
# -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the F3 supervised soak-evidence preparation
# tooling, built on top of the already-accepted F1 and implementation-
# complete F2. Pure text/source validation only -- no network call, no
# provider/broker call, no DB connection, no daemon start, no cargo/npm
# build or test.
#
# Checks:
#   [1]  The F2 guard exists and, when invoked, exits 0.
#   [2]  The spec, guard, capture script, validator script, template, and
#        checklist all exist and are nonempty.
#   [3]  The capture script is GET-only: contains "-Method Get" and contains
#        no Post/Put/Patch/Delete HTTP method reference.
#   [4]  No Alpaca or Discord URL/host string appears in the capture script.
#   [5]  No order-submission, start/stop/halt, arm/disarm, flatten, or
#        finalization route string appears in the capture script.
#   [6]  .env.local is never referenced in the capture script.
#   [7]  The capture script's fail-closed local-host check exists.
#   [8]  The validator script rejects a non-paper deployment_mode and a
#        live_routing_enabled: true value.
#   [9]  The validator script contains the secret-pattern scan.
#   [10] The validator script's null-count check exists and never coerces
#        null counts to a numeric value.
#   [11] .gitignore contains the smoke_logs/autonomous_paper_soak/ rule.
#   [12] No generated evidence file is staged or present in the current
#        working tree (staged and unstaged).
#   [13] No production Rust, migration, or GUI production file is touched.
#   [14] README.md / README_TECHNICAL.md / ledger record correct F3 status
#        and never overclaim Phase G, Bundle 3 closure, soak-started, or
#        live-capital-ready.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01f3_supervised_soak_evidence_preparation.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathF3Spec         = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01f3_supervised_soak_evidence_preparation.md"
$PathCaptureScript  = Join-Path $RepoRoot "scripts\soak\capture_autonomous_paper_session_evidence.ps1"
$PathValidatorScript = Join-Path $RepoRoot "scripts\soak\validate_autonomous_paper_session_evidence.ps1"
$PathTemplate       = Join-Path $RepoRoot "scripts\soak\templates\autonomous_paper_session_manifest.template.json"
$PathChecklist      = Join-Path $RepoRoot "scripts\soak\supervised_session_evidence_checklist.md"
$PathGitignore      = Join-Path $RepoRoot ".gitignore"
$PathReadme         = Join-Path $RepoRoot "README.md"
$PathReadmeTech     = Join-Path $RepoRoot "README_TECHNICAL.md"
$PathLedger         = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"

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

Write-Host "============================================================"
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F3-SUPERVISED-SOAK-EVIDENCE-PREPARATION Validator"
Write-Host "============================================================"

# -----------------------------------------------------------------------
# [1] F2 guard exists and exits 0.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [1] F2 guard exists and exits 0 ---"
$F2GuardPath = Join-Path $ScriptDir "validate_autonomous_daily_paper_operations_01f2_operator_runbook_correction.ps1"
if (Test-Path $F2GuardPath) {
    & powershell -ExecutionPolicy Bypass -File $F2GuardPath *> $null
    $exitCode = $LASTEXITCODE
    if ($exitCode -eq 0) {
        Show-Green "  OK -- F2 guard exits 0"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- F2 guard exited $exitCode"
    }
} else {
    $script:Violations++
    Show-Red "  FAIL -- F2 guard not found: $F2GuardPath"
}

# -----------------------------------------------------------------------
# [2] All deliverables exist and are nonempty.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [2] Spec, guard, capture script, validator, template, checklist all exist and are nonempty ---"
$CaptureContent = $null
if (Test-FileExists "capture_autonomous_paper_session_evidence.ps1" $PathCaptureScript) {
    $CaptureContent = Get-Content -Raw -Path $PathCaptureScript
    if ($CaptureContent.Length -gt 500) { Show-Green "  OK -- capture script is nonempty" } else { $script:Violations++; Show-Red "  FAIL -- capture script is suspiciously short" }
}
$ValidatorContent = $null
if (Test-FileExists "validate_autonomous_paper_session_evidence.ps1" $PathValidatorScript) {
    $ValidatorContent = Get-Content -Raw -Path $PathValidatorScript
    if ($ValidatorContent.Length -gt 500) { Show-Green "  OK -- validator script is nonempty" } else { $script:Violations++; Show-Red "  FAIL -- validator script is suspiciously short" }
}
$TemplateContent = $null
if (Test-FileExists "manifest template" $PathTemplate) {
    $TemplateContent = Get-Content -Raw -Path $PathTemplate
    try {
        $null = $TemplateContent | ConvertFrom-Json
        Show-Green "  OK -- manifest template is valid JSON"
    } catch {
        $script:Violations++
        Show-Red "  FAIL -- manifest template is not valid JSON"
    }
}
Test-FileExists "supervised_session_evidence_checklist.md" $PathChecklist | Out-Null
$F3SpecContent = $null
if (Test-FileExists "F3 spec doc" $PathF3Spec) {
    $F3SpecContent = Get-Content -Raw -Path $PathF3Spec
    if ($F3SpecContent.Length -gt 2000) { Show-Green "  OK -- F3 spec is nonempty" } else { $script:Violations++; Show-Red "  FAIL -- F3 spec is suspiciously short" }
}

# -----------------------------------------------------------------------
# [3] Capture script is GET-only.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [3] Capture script is GET-only ---"
Test-ContentContains "capture script uses -Method Get" $CaptureContent "-Method Get" | Out-Null
foreach ($Forbidden in @("-Method Post", "-Method Put", "-Method Patch", "-Method Delete", "-Method POST", "-Method PUT", "-Method PATCH", "-Method DELETE")) {
    Test-ContentDoesNotContain "capture script never uses '$Forbidden'" $CaptureContent $Forbidden | Out-Null
}

# -----------------------------------------------------------------------
# [4] No Alpaca or Discord URL/host.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [4] No Alpaca or Discord URL/host in the capture script ---"
foreach ($Forbidden in @("alpaca.markets", "alpaca.com", "discord.com", "discordapp.com", "hooks.slack.com")) {
    Test-ContentDoesNotContain "capture script never references '$Forbidden'" $CaptureContent $Forbidden | Out-Null
}

# -----------------------------------------------------------------------
# [5] No mutating/order/lifecycle route string.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5] No order-submission, start/stop/halt, arm/disarm, flatten, or finalization route ---"
foreach ($Forbidden in @(
    "/v1/run/start", "/v1/run/stop", "/v1/run/halt",
    "/api/v1/ops/action", "/v1/integrity/arm", "/v1/integrity/disarm",
    "flatten-paper-positions", "kill-switch", "clear-halted-run",
    "classify_and_finalize", "finalize_autonomous_daily_operation"
)) {
    Test-ContentDoesNotContain "capture script never references '$Forbidden'" $CaptureContent $Forbidden | Out-Null
}

# -----------------------------------------------------------------------
# [6] .env.local never referenced in executable code.
#
# The safety-comment block legitimately documents this prohibition using
# the literal filename ("Never reads or copies .env.local") -- that is
# prose, not a file access. This check strips comment-only lines (lines
# whose first non-whitespace character is '#') before scanning, so it
# proves no CODE line reads/joins/tests the path, without forbidding the
# script from naming the very file it promises never to touch.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [6] .env.local is never referenced in executable code (comments excluded) ---"
$CaptureCodeOnly = $null
if ($null -ne $CaptureContent) {
    $CaptureCodeOnly = ($CaptureContent -split "`r?`n" | Where-Object { $_.TrimStart() -notmatch '^#' }) -join "`n"
}
Test-ContentDoesNotContain "capture script's executable code never references .env.local" $CaptureCodeOnly ".env.local" | Out-Null

# -----------------------------------------------------------------------
# [7] Fail-closed local-host check exists.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [7] Fail-closed local-host check exists ---"
Test-ContentContains "capture script checks the daemon host against an allow-list" $CaptureContent "AllowedHosts" | Out-Null
Test-ContentContains "capture script refuses a non-local host" $CaptureContent "REFUSED" | Out-Null

# -----------------------------------------------------------------------
# [8] Validator rejects live mode and live routing.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [8] Validator rejects non-paper deployment_mode and live_routing_enabled: true ---"
Test-ContentContains "validator checks deployment_mode against paper" $ValidatorContent "-eq 'paper'" | Out-Null
Test-ContentContains "validator checks live_routing_enabled" $ValidatorContent "LiveRoutingEnabled" | Out-Null

# -----------------------------------------------------------------------
# [9] Validator contains the secret-pattern scan.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [9] Validator contains a secret-pattern scan ---"
Test-ContentContains "validator defines SecretPatterns" $ValidatorContent "SecretPatterns" | Out-Null
foreach ($Pattern in @("ALPACA_API_KEY", "MQK_OPERATOR_TOKEN", "MQK_DATABASE_URL", "Bearer ")) {
    Test-ContentContains "validator scans for '$Pattern'" $ValidatorContent $Pattern | Out-Null
}

# -----------------------------------------------------------------------
# [10] Validator's null-count check never coerces null to numeric.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [10] Validator preserves null counts, never coerces to a numeric value ---"
Test-ContentContains "validator checks strategy_evaluation_count/order_activity_count/fill_count" $ValidatorContent "strategy_evaluation_count" | Out-Null
Test-ContentDoesNotContain "validator never defaults a null count to 0" $ValidatorContent "?? 0" | Out-Null

# -----------------------------------------------------------------------
# [11] .gitignore contains the default output rule.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [11] .gitignore contains the smoke_logs/autonomous_paper_soak/ rule ---"
$GitignoreContent = if (Test-FileExists ".gitignore" $PathGitignore) { Get-Content -Raw -Path $PathGitignore } else { $null }
Test-ContentContains ".gitignore ignores smoke_logs/autonomous_paper_soak/" $GitignoreContent "smoke_logs/autonomous_paper_soak/" | Out-Null

# -----------------------------------------------------------------------
# [12] No generated evidence staged or present in the working tree.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [12] No generated evidence file is staged or present in the working tree ---"
$PriorErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
$UnstagedChanges = git -C $RepoRoot diff --name-only 2>$null
$StagedChanges = git -C $RepoRoot diff --name-only --cached 2>$null
$UntrackedFiles = git -C $RepoRoot ls-files --others --exclude-standard 2>$null
$ErrorActionPreference = $PriorErrorActionPreference

$UnstagedClean = @($UnstagedChanges) | Where-Object { $_ -ne "" } | Select-Object -Unique
$StagedClean = @($StagedChanges) | Where-Object { $_ -ne "" } | Select-Object -Unique
$UntrackedClean = @($UntrackedFiles) | Where-Object { $_ -ne "" } | Select-Object -Unique

$EvidencePaths = @($UnstagedClean) + @($StagedClean) + @($UntrackedClean) | Where-Object {
    $_ -like "smoke_logs/autonomous_paper_soak/*" -or $_ -like "*autonomous_paper_session_manifest.json"
} | Select-Object -Unique
if ($null -eq $EvidencePaths -or @($EvidencePaths).Count -eq 0) {
    Show-Green "  OK -- no generated evidence file staged, unstaged, or untracked"
} else {
    $script:Violations++
    Show-Red "  FAIL -- generated evidence file(s) present: $($EvidencePaths -join ', ')"
}

# -----------------------------------------------------------------------
# [13] No production Rust, migration, or GUI production file touched.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [13] No production Rust, migration, or GUI production file is touched ---"
function Test-NoProductionRustMigrationOrGui {
    param([string]$Label, [string[]]$Paths)
    $ProductionRust = $Paths | Where-Object { $_ -like "core-rs/*/src/*.rs" -and $_ -notlike "*/tests/*" }
    $Migrations = $Paths | Where-Object { $_ -like "core-rs/crates/mqk-db/migrations/*" }
    $Gui = $Paths | Where-Object { $_ -like "core-rs/mqk-gui/src/*" }

    if ($null -eq $ProductionRust -or @($ProductionRust).Count -eq 0) {
        Show-Green "  OK -- $Label -- no production Rust file"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label -- production Rust file(s): $($ProductionRust -join ', ')"
    }
    if ($null -eq $Migrations -or @($Migrations).Count -eq 0) {
        Show-Green "  OK -- $Label -- no migration file"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label -- migration file(s): $($Migrations -join ', ')"
    }
    if ($null -eq $Gui -or @($Gui).Count -eq 0) {
        Show-Green "  OK -- $Label -- no GUI production file"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label -- GUI production file(s): $($Gui -join ', ')"
    }
}
Test-NoProductionRustMigrationOrGui "unstaged working tree" $UnstagedClean
Test-NoProductionRustMigrationOrGui "staged working tree" $StagedClean

# -----------------------------------------------------------------------
# [14] README/ledger status and overclaim guard.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [14] README/ledger record correct F3 status and never overclaim ---"
$ReadmeContent = if (Test-FileExists "README.md" $PathReadme) { Get-Content -Raw -Path $PathReadme } else { $null }
$ReadmeTechContent = if (Test-FileExists "README_TECHNICAL.md" $PathReadmeTech) { Get-Content -Raw -Path $PathReadmeTech } else { $null }
$LedgerContent = if (Test-FileExists "Master patch ledger" $PathLedger) { Get-Content -Raw -Path $PathLedger } else { $null }

$ForbiddenClaims = @(
    "PHASE G: STARTED",
    "Phase G has started",
    "Phase G is complete",
    "BUNDLE 3: CLOSED",
    "Bundle 3 is now closed",
    "Bundle 3 has closed",
    "Bundle 3 is complete",
    "SOAK: STARTED",
    "soak has started",
    "unattended soak is underway",
    "LIVE CAPITAL: READY",
    "live capital is ready",
    "approved for live capital",
    "ready for live capital"
)
foreach ($Doc in @(
    @{Name = "README.md"; Content = $ReadmeContent},
    @{Name = "README_TECHNICAL.md"; Content = $ReadmeTechContent},
    @{Name = "Master patch ledger"; Content = $LedgerContent}
)) {
    foreach ($Phrase in $ForbiddenClaims) {
        Test-ContentDoesNotContain "$($Doc.Name) does not contain forbidden claim '$Phrase'" $Doc.Content $Phrase | Out-Null
    }
}
Test-ContentContains "README.md records F3 status" $ReadmeContent "F3" | Out-Null
Test-ContentContains "ledger records F3" $LedgerContent "F3" | Out-Null
Test-ContentDoesNotContain "ledger does not claim Phase F closed" $LedgerContent "PHASE F: CLOSED" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F3-SUPERVISED-SOAK-EVIDENCE-PREPARATION evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
