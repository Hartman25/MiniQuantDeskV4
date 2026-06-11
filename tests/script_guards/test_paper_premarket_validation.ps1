# =============================================================================
# CI-GUARD-CONSOLIDATION-01
# Script guard: test_paper_premarket_validation.ps1
#
# Static assertions against scripts\windows\Invoke-PaperPremarketValidation.ps1.
# No daemon, no DB, no live calls, no .env.local required. All checks are
# read-only source inspections.
#
# Guard assertions:
#   PPV01  Script exists at scripts\windows\Invoke-PaperPremarketValidation.ps1
#   PPV02  Script invokes tests\script_guards\run_all_script_guards.ps1 (NonInteractive)
#   PPV03  Script runs GUI tests (npm run test in core-rs\mqk-gui)
#   PPV04  Script runs GUI build (npm run build)
#   PPV05  Script does NOT invoke full_repo_proof.ps1
#   PPV06  Script does NOT invoke Run-AAPL5mMarketSmoke.ps1 (normal mode)
#   PPV07  Script does NOT invoke Start-PaperTradingSmoke.ps1 (normal mode)
#   PPV08  Script does NOT call an arm-execution action
#   PPV09  Script does NOT call clear-halted-run
#   PPV10  Script does NOT call flatten-paper-positions or any flatten action
#   PPV11  Script does NOT call any Alpaca/broker HTTP endpoint
#   PPV12  Script does NOT call a Discord webhook
#   PPV13  Script does NOT print or read .env.local contents
#   PPV14  Script has dirty tracked-file protection (-AllowDirty gate + hard stop)
#   PPV15  Script prints a final PASS/FAIL summary (FINAL: PASS / FINAL: FAIL)
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path
$Target    = Join-Path $RepoRoot 'scripts\windows\Invoke-PaperPremarketValidation.ps1'

$Passed = 0
$Failed = 0

function Assert-True {
    param([string]$Id, [string]$Description, [bool]$Condition)
    if ($Condition) {
        Write-Host "  PASS [$Id] $Description" -ForegroundColor Green
        $script:Passed++
    } else {
        Write-Host "  FAIL [$Id] $Description" -ForegroundColor Red
        $script:Failed++
    }
}

Write-Host ""
Write-Host "=== test_paper_premarket_validation.ps1 (CI-GUARD-CONSOLIDATION-01) ===" -ForegroundColor Cyan
Write-Host "    Target: $Target"
Write-Host ""

if (-not (Test-Path $Target)) {
    Write-Host "  FAIL [PPV01] Script not found: $Target" -ForegroundColor Red
    exit 1
}

$Content = Get-Content $Target -Raw

# PPV01: script exists (Test-Path above already gates this; record as a pass)
Assert-True 'PPV01' 'Script exists at scripts\windows\Invoke-PaperPremarketValidation.ps1' `
    (Test-Path $Target)

# PPV02: invokes run_all_script_guards.ps1 via NonInteractive PowerShell
Assert-True 'PPV02' 'Script invokes tests\script_guards\run_all_script_guards.ps1 via -NonInteractive PowerShell' `
    ($Content -match [regex]::Escape("'tests\script_guards\run_all_script_guards.ps1'") -and
     $Content -match [regex]::Escape("'-NonInteractive'") -and
     $Content -match [regex]::Escape("Name 'Script guards'"))

# PPV03: runs GUI tests (npm run test in core-rs\mqk-gui)
Assert-True 'PPV03' 'Script runs GUI tests via npm run test in core-rs\mqk-gui' `
    ($Content -match [regex]::Escape("Name 'GUI tests'") -and
     $Content -match [regex]::Escape("'core-rs\mqk-gui'") -and
     $Content -match [regex]::Escape("@('run', 'test')"))

# PPV04: runs GUI build (npm run build)
Assert-True 'PPV04' 'Script runs GUI build via npm run build' `
    ($Content -match [regex]::Escape("Name 'GUI build'") -and
     $Content -match [regex]::Escape("@('run', 'build')"))

# PPV05: does NOT invoke full_repo_proof.ps1
Assert-True 'PPV05' 'Script does NOT invoke full_repo_proof.ps1' `
    ($Content -notmatch '(?i)&\s*[''"]?[^\r\n]*full_repo_proof' -and
     $Content -notmatch '(?i)Invoke-CheckedCommand[^\r\n]*full_repo_proof' -and
     $Content -notmatch '(?i)\.\\full_repo_proof')

# PPV06: does NOT invoke Run-AAPL5mMarketSmoke.ps1 (normal mode)
Assert-True 'PPV06' 'Script does NOT invoke Run-AAPL5mMarketSmoke.ps1 (normal mode)' `
    ($Content -notmatch '&\s*[''"]?[^\r\n]*Run-AAPL5mMarketSmoke\.ps1')

# PPV07: does NOT invoke Start-PaperTradingSmoke.ps1 (normal mode)
Assert-True 'PPV07' 'Script does NOT invoke Start-PaperTradingSmoke.ps1 (normal mode)' `
    ($Content -notmatch '&\s*[''"]?[^\r\n]*Start-PaperTradingSmoke\.ps1')

# PPV08: does NOT call an arm-execution action
Assert-True 'PPV08' 'Script does NOT call an arm-execution action (-ArmPaper / ops/action arm)' `
    ($Content -notmatch '(?i)-ArmPaper' -and
     $Content -notmatch '(?i)/api/v1/ops/action' -and
     $Content -notmatch '(?i)arm-execution' -and
     $Content -notmatch "(?i)'action'\s*:\s*'arm'")

# PPV09: does NOT call clear-halted-run
Assert-True 'PPV09' 'Script does NOT call clear-halted-run' `
    ($Content -notmatch [regex]::Escape('clear-halted-run'))

# PPV10: does NOT call flatten-paper-positions or any flatten action
Assert-True 'PPV10' 'Script does NOT call flatten-paper-positions or any flatten action' `
    ($Content -notmatch '(?i)flatten-paper-positions' -and
     $Content -notmatch '(?i)Invoke-CheckedCommand[^\r\n]*[Ff]latten' -and
     $Content -notmatch '&\s*[''"]?[^\r\n]*[Ff]latten')

# PPV11: does NOT call any Alpaca/broker HTTP endpoint
Assert-True 'PPV11' 'Script does NOT call any Alpaca/broker HTTP endpoint' `
    ($Content -notmatch '(?i)alpaca\.markets' -and
     $Content -notmatch '(?i)paper-api\.alpaca' -and
     $Content -notmatch '/v2/orders' -and
     $Content -notmatch '/v2/account')

# PPV12: does NOT call a Discord webhook
Assert-True 'PPV12' 'Script does NOT call a Discord webhook' `
    ($Content -notmatch 'DISCORD_WEBHOOK_URL' -and
     $Content -notmatch '(?i)discord\.com/api/webhooks' -and
     $Content -notmatch '(?i)Send-DiscordAlert' -and
     $Content -notmatch '(?i)Invoke-CheckedCommand[^\r\n]*[Dd]iscord')

# PPV13: does NOT print or read .env.local contents
# (a plain-text mention of ".env.local" in a descriptive Write-Host banner
#  line is fine; reading the file's contents via Get-Content is not)
Assert-True 'PPV13' 'Script does NOT print or read .env.local contents' `
    ($Content -notmatch '(?i)Get-Content[^\r\n]*env\.local')

# PPV14: dirty tracked-file protection (-AllowDirty gate + hard stop)
Assert-True 'PPV14' 'Script gates on a dirty tracked working tree (-AllowDirty switch + git status --porcelain + hard stop)' `
    ($Content -match '\[switch\]\$AllowDirty' -and
     $Content -match [regex]::Escape('status --porcelain=v1') -and
     $Content -match [regex]::Escape('$hardStop = $true'))

# PPV15: final PASS/FAIL summary
Assert-True 'PPV15' 'Script prints a final PASS/FAIL summary (FINAL: PASS / FINAL: FAIL)' `
    ($Content -match [regex]::Escape('FINAL: PASS') -and
     $Content -match [regex]::Escape('FINAL: FAIL'))

# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $Passed" -ForegroundColor Green
Write-Host "  Failed: $Failed" -ForegroundColor $(if ($Failed -gt 0) { 'Red' } else { 'Green' })
Write-Host ""

if ($Failed -gt 0) {
    Write-Host "GUARD FAILED: $Failed assertion(s) failed." -ForegroundColor Red
    exit 1
} else {
    Write-Host "GUARD PASSED: all $Passed assertions passed." -ForegroundColor Green
    exit 0
}
