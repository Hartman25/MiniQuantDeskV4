# =============================================================================
# CRYPTO-DATA-03D -- Kraken Scheduler Section Closure Audit Validator
# =============================================================================
# Pure docs/text validation of the CRYPTO-DATA-02A..03C Kraken scheduler
# section's closure evidence. No network call, no provider/broker call, no
# DB connection, no daemon start, no cargo/npm build or test, no scheduled
# task registration/unregistration/start.
#
# Checks:
#   [1]  02A rate-limit decision spec exists.
#   [2]  03B task-scripts spec exists.
#   [3]  03D closure audit spec (this section's own output) exists.
#   [4]  Runbook mentions all six section patch IDs (02A/02B/02C/03A/03B/03C).
#   [5]  Runbook states the task is not registered by any patch.
#   [6]  Runbook mentions the task-status route path.
#   [7]  03B spec states check-only is the default.
#   [8]  03B spec states -Register is explicit/separate.
#   [9]  03B spec states .env.local is never read.
#   [10] Route code references the task-registration evidence filename.
#   [11] GUI code contains the "Kraken scheduled task status" panel title.
#   [12] GUI code contains the fixed no-mutation warning text.
#   [13] No audited doc claims crypto trading is enabled.
#   [14] No audited doc claims the task was registered during validation.
#   [15] Ledger and audit both still record parent roadmap items as PARTIAL
#        (or the closure audit's own honest-PARTIAL wording) for this section.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_crypto_data_03d_kraken_scheduler_section_closure.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$Path02aSpec     = Join-Path $RepoRoot "docs\specs\crypto_data_02a_kraken_scheduler_rate_limit_decision.md"
$Path03bSpec     = Join-Path $RepoRoot "docs\specs\crypto_data_03b_kraken_scheduler_task_scripts.md"
$Path03dSpec     = Join-Path $RepoRoot "docs\specs\crypto_data_03d_kraken_scheduler_section_closure_audit.md"
$PathRunbook     = Join-Path $RepoRoot "docs\runbooks\local_crypto_marks_ingest.md"
$PathLedger      = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"
$PathAudit       = Join-Path $RepoRoot "docs\audits\multi_asset_completion_audit.md"
$PathRouteFile   = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\transport_quality.rs"
$PathGuiApi      = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\ingest\api.ts"
$PathGuiScreen   = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\ingest\IngestScreen.tsx"

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
Write-Host " CRYPTO-DATA-03D Kraken Scheduler Section Closure Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1]-[3] Section spec docs exist ---"
Test-FileExists "02A rate-limit decision spec" $Path02aSpec | Out-Null
Test-FileExists "03B task-scripts spec" $Path03bSpec | Out-Null
Test-FileExists "03D closure audit spec" $Path03dSpec | Out-Null

Write-Host ""
Show-Info "--- [4]-[6] Runbook coverage ---"
$RunbookContent = $null
if (Test-FileExists "Runbook" $PathRunbook) {
    $RunbookContent = Get-Content -Raw -Path $PathRunbook
}

$SectionPatchIds = @(
    "CRYPTO-DATA-02A", "CRYPTO-DATA-02B", "CRYPTO-DATA-02C",
    "CRYPTO-DATA-03A", "CRYPTO-DATA-03B", "CRYPTO-DATA-03C"
)
$MissingPatchIds = 0
foreach ($PatchId in $SectionPatchIds) {
    if ($null -eq $RunbookContent -or -not $RunbookContent.Contains($PatchId)) {
        $MissingPatchIds++
        $Violations++
        Show-Red "  FAIL -- runbook does not mention $PatchId"
    }
}
if ($MissingPatchIds -eq 0) {
    Show-Green "  OK -- runbook mentions all six section patch IDs"
}

Test-ContentContains "runbook states task registration requires an explicit operator invocation" `
    $RunbookContent "requires an explicit, separate operator invocation" | Out-Null

Test-ContentContains "runbook mentions the task-status route" `
    $RunbookContent "/api/v1/market-data/kraken-scheduler/task-status" | Out-Null

Write-Host ""
Show-Info "--- [7]-[9] 03B spec safety contract wording ---"
$Path03bContent = $null
if (Test-Path $Path03bSpec) {
    $Path03bContent = Get-Content -Raw -Path $Path03bSpec
}
Test-ContentContains "03B spec states check-only is the default mode" `
    $Path03bContent 'defaults to `-CheckOnly`' | Out-Null
Test-ContentContains "03B spec states -Register is explicit and separate" `
    $Path03bContent '`-Register` and `-Unregister` are both' | Out-Null
Test-ContentContains "03B spec states .env.local is never read" `
    $Path03bContent '.env.local` is never read by either script' | Out-Null

Write-Host ""
Show-Info "--- [10] Route code references task-registration evidence file ---"
$RouteContent = $null
if (Test-FileExists "Route implementation file" $PathRouteFile) {
    $RouteContent = Get-Content -Raw -Path $PathRouteFile
}
Test-ContentContains "route code references kraken_ohlc_task_registration.json" `
    $RouteContent "kraken_ohlc_task_registration.json" | Out-Null

Write-Host ""
Show-Info "--- [11]-[12] GUI panel presence and no-mutation warning ---"
$GuiScreenContent = $null
if (Test-FileExists "GUI Ingest screen" $PathGuiScreen) {
    $GuiScreenContent = Get-Content -Raw -Path $PathGuiScreen
}
Test-ContentContains "GUI contains 'Kraken scheduled task status' panel title" `
    $GuiScreenContent "Kraken scheduled task status" | Out-Null

$GuiApiContent = $null
if (Test-FileExists "GUI api.ts" $PathGuiApi) {
    $GuiApiContent = Get-Content -Raw -Path $PathGuiApi
}
Test-ContentContains "GUI carries the fixed no-mutation warning text" `
    $GuiApiContent "This panel cannot register, unregister, start a task, call Kraken, write market data, or enable crypto trading." | Out-Null

Write-Host ""
Show-Info "--- [13]-[14] No stale trading-enabled / task-registered claims ---"

$DocsToScan = @(
    @{ Label = "runbook"; Path = $PathRunbook },
    @{ Label = "02A spec"; Path = $Path02aSpec },
    @{ Label = "03B spec"; Path = $Path03bSpec },
    @{ Label = "03D spec"; Path = $Path03dSpec },
    @{ Label = "ledger"; Path = $PathLedger },
    @{ Label = "audit"; Path = $PathAudit }
)

$TradingEnabledPhrases = @(
    "crypto trading is enabled",
    "crypto trading is now enabled",
    "ready for crypto trading",
    "approved for crypto trading",
    "crypto trading enabled in this patch"
)
$TaskRegisteredPhrases = @(
    "task has been registered by this patch",
    "task is registered by this patch",
    "scheduler has been registered",
    "task was successfully registered"
)

$TradingClaimViolations = 0
$TaskClaimViolations = 0
foreach ($Doc in $DocsToScan) {
    if (-not (Test-Path $Doc.Path)) { continue }
    $Content = (Get-Content -Raw -Path $Doc.Path).ToLowerInvariant()
    foreach ($Phrase in $TradingEnabledPhrases) {
        if ($Content.Contains($Phrase.ToLowerInvariant())) {
            $TradingClaimViolations++
            $Violations++
            Show-Red "  FAIL -- $($Doc.Label) contains forbidden trading-enabled claim: '$Phrase'"
        }
    }
    foreach ($Phrase in $TaskRegisteredPhrases) {
        if ($Content.Contains($Phrase.ToLowerInvariant())) {
            $TaskClaimViolations++
            $Violations++
            Show-Red "  FAIL -- $($Doc.Label) contains forbidden task-registered claim: '$Phrase'"
        }
    }
}
if ($TradingClaimViolations -eq 0) {
    Show-Green "  OK -- no audited doc claims crypto trading is enabled"
}
if ($TaskClaimViolations -eq 0) {
    Show-Green "  OK -- no audited doc claims the task was registered during validation"
}

Write-Host ""
Show-Info "--- [15] Ledger/audit record parent roadmap as PARTIAL for this section ---"
$LedgerContent = $null
if (Test-Path $PathLedger) { $LedgerContent = Get-Content -Raw -Path $PathLedger }
$AuditContent = $null
if (Test-Path $PathAudit) { $AuditContent = Get-Content -Raw -Path $PathAudit }

Test-ContentContains "ledger records honest-PARTIAL wording for this section" `
    $LedgerContent 'remain PARTIAL, not `CLOSED`' | Out-Null
Test-ContentContains "audit records honest-PARTIAL wording for this section" `
    $AuditContent 'remain `PARTIAL`, not `CLOSED`' | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- Kraken scheduler section closure evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
