# =============================================================================
# DAILY-DATA-READINESS-01E-CLOSURE-01 -- Phase E Closure Integrity Validator
# =============================================================================
# Pure static/text/git validation of Bundle 2's closure evidence. No network
# call, no provider/broker call, no DB connection, no daemon/runtime/scheduler
# start, no cargo/npm build or test invocation. This is a closure-integrity
# check, not a substitute for the focused test commands documented in
# docs/specs/daily_data_readiness_01e_closure_decision.md.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#     -File scripts\guards\validate_daily_data_readiness_01e_closure.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathPhaseAContract = Join-Path $RepoRoot "docs\specs\daily_data_readiness_01a_current_truth_and_contract.md"
$PathPhaseAGuard     = Join-Path $RepoRoot "scripts\guards\validate_daily_data_readiness_01a_audit.ps1"
$PathClosureDoc      = Join-Path $RepoRoot "docs\specs\daily_data_readiness_01e_closure_decision.md"
$PathLedger          = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"
$PathLedgerUpdated   = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2_updated.md"

$PathRoutesRs             = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes.rs"
$PathReadinessRs          = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\daily_data_readiness.rs"
$PathGuiTypes             = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\ingest\types.ts"
$PathGuiApi               = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\ingest\api.ts"
$PathGuiPanel             = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\ingest\DailyDataReadinessPanel.tsx"
$PathGuiSourceTest        = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\ingest\__tests__\dailyDataReadinessScreenSource.test.ts"
$PathGuiPackageJson       = Join-Path $RepoRoot "core-rs\mqk-gui\package.json"

$RequiredTestFiles = @(
    "core-rs\crates\mqk-daemon\tests\scenario_daily_data_readiness_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_daily_data_readiness_api_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_daily_data_readiness_start_gate_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_ingest_jobs_data_ingest_daemon_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_market_data_latest_bar_poll_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_ingest_plan_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_data_freshness_readiness_gate_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_premarket_data_readiness_gate_01.rs",
    "core-rs\crates\mqk-daemon\tests\scenario_intraday_md_freshness_autonomous_01.rs"
)

$RequiredBundleCommits = @(
    "242de234", "64306c39", "0920de5d", "395e5ee6", "579b422e", "09f0a919",
    "304a4fd0", "574f5cc6", "f5fc1e80", "23425d4a", "53ad2454", "81932229",
    "500bb174", "45e3c44d"
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
        Show-Green "  OK -- $Label"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (forbidden needle found: '$Needle')"
        return $false
    }
}

function Test-RegexDoesNotMatch {
    param([string]$Label, [string]$Content, [string]$Pattern)
    if ($null -eq $Content -or -not ([regex]::IsMatch($Content, $Pattern))) {
        Show-Green "  OK -- $Label"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (forbidden pattern matched: '$Pattern')"
        return $false
    }
}

Write-Host "============================================================"
Write-Host " DAILY-DATA-READINESS-01E-CLOSURE-01 Validator"
Write-Host "============================================================"

# -----------------------------------------------------------------------
# [1] Phase A contract and Phase A guard exist
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [1] Phase A contract and Phase A guard exist ---"
Test-FileExists "Phase A contract" $PathPhaseAContract | Out-Null
Test-FileExists "Phase A guard" $PathPhaseAGuard | Out-Null

# -----------------------------------------------------------------------
# [2] Phase E closure document exists
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [2] Phase E closure document exists ---"
$ClosureContent = $null
if (Test-FileExists "Phase E closure decision" $PathClosureDoc) {
    $ClosureContent = Get-Content -Raw -Path $PathClosureDoc
}

# -----------------------------------------------------------------------
# [3] Canonical ledger contains the Bundle 2 entry
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [3] Canonical ledger contains the Bundle 2 entry ---"
$LedgerContent = $null
if (Test-FileExists "Canonical ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Path $PathLedger
}
Test-ContentContains "ledger contains Bundle 2 section header" $LedgerContent "## DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED" | Out-Null

# -----------------------------------------------------------------------
# [4] Closure document and ledger use the same final disposition
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [4] Closure document and ledger agree on final disposition ---"
$DispositionPattern = "CLOSED_LOCAL|PARTIAL|BLOCKED|FALSE_CLOSED"
$ClosureDisposition = $null
$LedgerDisposition = $null
if ($null -ne $ClosureContent) {
    $m = [regex]::Match($ClosureContent, "FINAL DISPOSITION:\s*($DispositionPattern)")
    if ($m.Success) { $ClosureDisposition = $m.Groups[1].Value }
}
if ($null -ne $LedgerContent) {
    $m = [regex]::Match($LedgerContent, "DAILY-DATA-READINESS-AND-FRESHNESS-01-COMBINED:\s*($DispositionPattern)")
    if ($m.Success) { $LedgerDisposition = $m.Groups[1].Value }
}
if ($null -ne $ClosureDisposition -and $ClosureDisposition -eq $LedgerDisposition) {
    Show-Green "  OK -- closure doc and ledger both report disposition '$ClosureDisposition'"
} else {
    $script:Violations++
    Show-Red "  FAIL -- disposition mismatch or missing (closure='$ClosureDisposition', ledger='$LedgerDisposition')"
}

# -----------------------------------------------------------------------
# [5] Required Bundle 2 commits are ancestors of HEAD
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5] Required Bundle 2 commits are ancestors of HEAD ---"
Push-Location $RepoRoot
try {
    foreach ($c in $RequiredBundleCommits) {
        & git merge-base --is-ancestor $c HEAD 2>$null
        if ($LASTEXITCODE -eq 0) {
            Show-Green "  OK -- $c is an ancestor of HEAD"
        } else {
            $script:Violations++
            Show-Red "  FAIL -- $c is NOT an ancestor of HEAD"
        }
    }
} finally {
    Pop-Location
}

# -----------------------------------------------------------------------
# [6] Dedicated route exists
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [6] Dedicated readiness route exists ---"
$RoutesContent = $null
if (Test-FileExists "routes.rs" $PathRoutesRs) {
    $RoutesContent = Get-Content -Raw -Path $PathRoutesRs
}
Test-ContentContains "routes.rs registers /api/v1/market-data/readiness" $RoutesContent "/api/v1/market-data/readiness" | Out-Null

# -----------------------------------------------------------------------
# [7]/[8] Binding scopes exist
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [7]/[8] Binding scopes exist ---"
$ReadinessContent = $null
if (Test-FileExists "daily_data_readiness.rs" $PathReadinessRs) {
    $ReadinessContent = Get-Content -Raw -Path $PathReadinessRs
}
Test-ContentContains "configuration_preview binding scope exists" $ReadinessContent "configuration_preview" | Out-Null
Test-ContentContains "start_attempt_binding binding scope exists" $ReadinessContent "start_attempt_binding" | Out-Null

# -----------------------------------------------------------------------
# [9] Durable event types exist
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [9] Durable event types exist ---"
Test-ContentContains "daily_data_readiness_evaluated event type" $ReadinessContent "daily_data_readiness_evaluated" | Out-Null
Test-ContentContains "daily_data_readiness_run_linked event type" $ReadinessContent "daily_data_readiness_run_linked" | Out-Null

# -----------------------------------------------------------------------
# [10] Stable persistence-failure reasons exist
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [10] Stable persistence-failure reasons exist ---"
Test-ContentContains "readiness_evidence_persist_failed reason exists" $ReadinessContent "readiness_evidence_persist_failed" | Out-Null
Test-ContentContains "readiness_run_link_persist_failed reason exists" $ReadinessContent "readiness_run_link_persist_failed" | Out-Null

# -----------------------------------------------------------------------
# [11] Provider mismatch and timestamp-convention reason codes exist
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [11] Provider mismatch / timestamp-convention reason codes exist ---"
Test-ContentContains "provider_id_mismatch reason exists" $ReadinessContent "provider_id_mismatch" | Out-Null
Test-ContentContains "provider_symbol_mismatch reason exists" $ReadinessContent "provider_symbol_mismatch" | Out-Null
Test-ContentContains "provider_timestamp_convention_unverified reason exists" $ReadinessContent "provider_timestamp_convention_unverified" | Out-Null

# -----------------------------------------------------------------------
# [12] Required focused test files exist
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [12] Required focused test files exist ---"
foreach ($rel in $RequiredTestFiles) {
    Test-FileExists $rel (Join-Path $RepoRoot $rel) | Out-Null
}

# -----------------------------------------------------------------------
# [13] GUI panel and GET fetch exist
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [13] GUI panel and GET fetch exist ---"
$GuiPanelContent = $null
if (Test-FileExists "DailyDataReadinessPanel.tsx" $PathGuiPanel) {
    $GuiPanelContent = Get-Content -Raw -Path $PathGuiPanel
}
$GuiApiContent = $null
if (Test-FileExists "ingest api.ts" $PathGuiApi) {
    $GuiApiContent = Get-Content -Raw -Path $PathGuiApi
}
Test-ContentContains "api.ts declares fetchDailyDataReadiness" $GuiApiContent "fetchDailyDataReadiness" | Out-Null

# -----------------------------------------------------------------------
# [14] GUI readiness panel does not import or call postJson
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [14] GUI readiness panel does not import or call postJson ---"
Test-ContentDoesNotContain "panel does not reference postJson" $GuiPanelContent "postJson" | Out-Null

# -----------------------------------------------------------------------
# [15] GUI source safety test exists and is registered
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [15] GUI source safety test exists and is registered ---"
Test-FileExists "dailyDataReadinessScreenSource.test.ts" $PathGuiSourceTest | Out-Null
$GuiPackageJsonContent = $null
if (Test-FileExists "mqk-gui package.json" $PathGuiPackageJson) {
    $GuiPackageJsonContent = Get-Content -Raw -Path $PathGuiPackageJson
}
Test-ContentContains "package.json test script registers dailyDataReadinessScreenSource.test.ts" $GuiPackageJsonContent "dailyDataReadinessScreenSource.test.ts" | Out-Null

# -----------------------------------------------------------------------
# [16] start_allowed normalized type supports boolean | null
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [16] start_allowed normalized type supports boolean | null ---"
$GuiTypesContent = $null
if (Test-FileExists "ingest types.ts" $PathGuiTypes) {
    $GuiTypesContent = Get-Content -Raw -Path $PathGuiTypes
}
Test-ContentContains "start_allowed typed boolean | null" $GuiTypesContent "start_allowed: boolean | null" | Out-Null

# -----------------------------------------------------------------------
# [17] Daily-readiness numeric types support number | null
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [17] Daily-readiness numeric types support number | null ---"
Test-ContentContains "loaded_completed_bars typed number | null" $GuiTypesContent "loaded_completed_bars: number | null" | Out-Null
Test-ContentContains "effective_grace_seconds typed number | null" $GuiTypesContent "effective_grace_seconds: number | null" | Out-Null

# -----------------------------------------------------------------------
# [18] The readiness normalizer never fabricates zero for missing/malformed
#      numeric evidence, within its own scoped section of api.ts.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [18] Readiness normalizer never fabricates zero for missing numeric evidence ---"
if ($null -ne $GuiApiContent) {
    $startMarker = "// DAILY-DATA-READINESS-01D-GUI-01: Daily data readiness (read-only,"
    $endMarker = "export function formatReadinessNumber"
    $startIdx = $GuiApiContent.IndexOf($startMarker)
    $endIdx = $GuiApiContent.IndexOf($endMarker)
    if ($startIdx -ge 0 -and $endIdx -gt $startIdx) {
        $scoped = $GuiApiContent.Substring($startIdx, $endIdx - $startIdx)
        Test-RegexDoesNotMatch "no 'normalizeNullableNumber(...) ?? 0' in the readiness normalizer section" $scoped 'normalizeNullableNumber\([^)]*\)\s*\?\?\s*0' | Out-Null
    } else {
        $script:Violations++
        Show-Red "  FAIL -- could not locate the daily-data-readiness scoped section in api.ts (markers not found)"
    }
} else {
    $script:Violations++
    Show-Red "  FAIL -- api.ts content unavailable for scoped normalizer check"
}

# -----------------------------------------------------------------------
# [19] No closure file or ledger text claims real provider/broker/order/
#      market-hours/paper-soak proof. Uses the bounded field:value safety
#      block both documents are required to carry, not free-text grep,
#      so the closure doc's own "does not mean" negation list never
#      collides with this check.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [19] No fabricated live/provider/broker/order/market-hours/soak claim ---"
$SafetyFields = @(
    "PROVIDER CALLS",
    "BROKER CALLS",
    "NETWORK CALLS",
    "REAL DAEMON STARTED",
    "REAL RUNTIME STARTED",
    "SCHEDULER STARTED",
    "EXECUTION ARMED",
    "PAPER ORDERS",
    "LIVE ORDERS",
    "PAPER DB TOUCHED"
)
foreach ($doc in @(@{Name="closure doc"; Content=$ClosureContent}, @{Name="ledger"; Content=$LedgerContent})) {
    if ($null -eq $doc.Content) { continue }
    foreach ($field in $SafetyFields) {
        $pattern = [regex]::Escape($field) + ":\s*yes"
        if ([regex]::IsMatch($doc.Content, $pattern, [System.Text.RegularExpressions.RegexOptions]::IgnoreCase)) {
            $script:Violations++
            Show-Red "  FAIL -- $($doc.Name) claims '${field}: yes'"
        }
    }
}
Show-Green "  OK -- no safety field in closure doc or ledger claims 'yes' for a live/provider/broker/order proof"

# -----------------------------------------------------------------------
# [20] MiniQuantDesk_Master_Patch_Ledger_v2_updated.md is not tracked or
#      staged by this patch.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [20] Generated ledger-updated copy is not tracked or staged ---"
Push-Location $RepoRoot
try {
    $tracked = & git ls-files -- "MiniQuantDesk_Master_Patch_Ledger_v2_updated.md" 2>$null
    if ([string]::IsNullOrWhiteSpace($tracked)) {
        Show-Green "  OK -- MiniQuantDesk_Master_Patch_Ledger_v2_updated.md is not tracked by git"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- MiniQuantDesk_Master_Patch_Ledger_v2_updated.md is tracked by git"
    }
    $staged = & git diff --cached --name-only 2>$null | Select-String -SimpleMatch "MiniQuantDesk_Master_Patch_Ledger_v2_updated.md"
    if ($null -eq $staged) {
        Show-Green "  OK -- MiniQuantDesk_Master_Patch_Ledger_v2_updated.md is not staged"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- MiniQuantDesk_Master_Patch_Ledger_v2_updated.md is staged"
    }
} finally {
    Pop-Location
}

# -----------------------------------------------------------------------
# Re-run Phase A guard -- Phase E must not close over a regressed Phase A.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- Re-running Phase A guard (must still pass) ---"
& powershell -NoProfile -ExecutionPolicy Bypass -File $PathPhaseAGuard | Out-Null
if ($LASTEXITCODE -eq 0) {
    Show-Green "  OK -- Phase A guard still passes"
} else {
    $script:Violations++
    Show-Red "  FAIL -- Phase A guard no longer passes"
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- DAILY-DATA-READINESS-01E closure evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
