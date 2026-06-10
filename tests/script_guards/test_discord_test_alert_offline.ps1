# =============================================================================
# Script guard: test_discord_test_alert_offline.ps1
# DISCORD-TEST-ALERT-OFFLINE-01
#
# Static assertions for the safe operator Discord test-alert workflow.
# No daemon, no DB, no live calls, no .env.local reads, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent (Split-Path -Parent $ScriptDir)
$AlertScriptPath = Join-Path $RepoRoot 'scripts\windows\Test-DiscordAlert.ps1'
$ControlSurfaceDocPath = Join-Path $RepoRoot 'docs\runbooks\operator_control_surface.md'
$OperatorWorkflowsDocPath = Join-Path $RepoRoot 'docs\runbooks\operator_workflows.md'

$Failures = 0

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if ($Condition) {
        Write-Host "  PASS: $Message" -ForegroundColor Green
    } else {
        Write-Host "  FAIL: $Message" -ForegroundColor Red
        $script:Failures++
    }
}

function Assert-False {
    param([bool]$Condition, [string]$Message)
    Assert-True (-not $Condition) $Message
}

function Read-TextFile {
    param([string]$Path)
    if (-not (Test-Path $Path)) {
        return ''
    }
    return (Get-Content -Path $Path -Raw)
}

Write-Host ''
Write-Host '--- test_discord_test_alert_offline.ps1 ---'

# ---------------------------------------------------------------------------
# Existence
# ---------------------------------------------------------------------------
Assert-True (Test-Path $AlertScriptPath) 'Test-DiscordAlert.ps1 exists'
Assert-True (Test-Path $ControlSurfaceDocPath) 'operator_control_surface.md exists'
Assert-True (Test-Path $OperatorWorkflowsDocPath) 'operator_workflows.md exists'

$AlertScript = Read-TextFile $AlertScriptPath
$ControlSurfaceDoc = Read-TextFile $ControlSurfaceDocPath
$OperatorWorkflowsDoc = Read-TextFile $OperatorWorkflowsDocPath

# ---------------------------------------------------------------------------
# -CheckOnly support
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match '\-CheckOnly') 'Test-DiscordAlert.ps1 supports -CheckOnly'
Assert-True ($AlertScript -match '(?m)^\s*if\s*\(\s*\$CheckOnly\s*\)') 'Test-DiscordAlert.ps1 branches on $CheckOnly before sending an alert'

# ---------------------------------------------------------------------------
# Must not call other lifecycle / trading / smoke scripts or actions
# ---------------------------------------------------------------------------
Assert-False ($AlertScript -match 'Start-PaperTradingSmoke\.ps1') 'Test-DiscordAlert.ps1 does not call Start-PaperTradingSmoke.ps1'
Assert-False ($AlertScript -match 'Run-AAPL5mMarketSmoke\.ps1') 'Test-DiscordAlert.ps1 does not call Run-AAPL5mMarketSmoke.ps1'
Assert-False ($AlertScript -match 'Launch-VeritasLedger\.ps1\s+-ArmPaper') 'Test-DiscordAlert.ps1 does not call Launch-VeritasLedger.ps1 -ArmPaper'
Assert-False ($AlertScript -match '(?i)arm-execution') 'Test-DiscordAlert.ps1 does not reference arm-execution'
Assert-False ($AlertScript -match '(?i)start-system') 'Test-DiscordAlert.ps1 does not reference start-system'
Assert-False ($AlertScript -match '(?i)flatten-paper-positions') 'Test-DiscordAlert.ps1 does not reference flatten-paper-positions'
Assert-False ($AlertScript -match '/api/v1/strategy/signal') 'Test-DiscordAlert.ps1 does not call /api/v1/strategy/signal'
Assert-False ($AlertScript -match '(?i)alpaca\.(markets|com)') 'Test-DiscordAlert.ps1 does not reference Alpaca broker base URLs'
Assert-False ($AlertScript -match "(?i)https?://[^\s`"']*(alpaca|broker)") 'Test-DiscordAlert.ps1 does not call any broker/Alpaca URL'

# ---------------------------------------------------------------------------
# Must not print secret env var values
# ---------------------------------------------------------------------------
$SecretPrintPattern = '(?im)^\s*(Write-Host|Write-Output|Write-Warning|Write-Verbose|Write-Information|echo|printf)\b[^\r\n]*\$env:(DISCORD_WEBHOOK_URL|MQK_OPERATOR_TOKEN)\b'
Assert-False ($AlertScript -match $SecretPrintPattern) 'Test-DiscordAlert.ps1 does not print $env:DISCORD_WEBHOOK_URL or $env:MQK_OPERATOR_TOKEN'

# ---------------------------------------------------------------------------
# Must not contain real-looking Discord webhook URLs
# ---------------------------------------------------------------------------
$RealLookingDiscordWebhookPattern = 'https://(?:canary\.|ptb\.)?discord(?:app)?\.com/api/webhooks/\d{17,20}/[A-Za-z0-9_\-]{40,}'
Assert-False ($AlertScript -match $RealLookingDiscordWebhookPattern) 'Test-DiscordAlert.ps1 contains no real-looking Discord webhook URL'

# ---------------------------------------------------------------------------
# Normal send path calls only the intended test-alert route / action_key
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match "/api/v1/ops/action") 'Test-DiscordAlert.ps1 calls POST /api/v1/ops/action'
Assert-True ($AlertScript -match '"action_key"\s*:\s*"test-discord-alert"') 'Test-DiscordAlert.ps1 request body uses action_key=test-discord-alert'

$ActionKeyLiterals = [System.Collections.Generic.HashSet[string]]::new()
foreach ($m in [regex]::Matches($AlertScript, 'action_key[''"]?\s*[:=]\s*[''"]([A-Za-z0-9_\-]+)[''"]')) {
    [void]$ActionKeyLiterals.Add($m.Groups[1].Value)
}
Assert-True ($ActionKeyLiterals.Count -eq 1 -and $ActionKeyLiterals.Contains('test-discord-alert')) `
    "Test-DiscordAlert.ps1 references exactly one action_key literal (test-discord-alert); found: $($ActionKeyLiterals -join ', ')"

# ---------------------------------------------------------------------------
# Fail-closed behavior present
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match '(?i)fail.?closed') 'Test-DiscordAlert.ps1 documents fail-closed behavior'
Assert-True ($AlertScript -match 'MQK_OPERATOR_TOKEN') 'Test-DiscordAlert.ps1 checks MQK_OPERATOR_TOKEN configuration'
Assert-True ($AlertScript -match '/v1/health') 'Test-DiscordAlert.ps1 checks daemon reachability via /v1/health'

# ---------------------------------------------------------------------------
# Docs updated with CheckOnly and normal commands + webhook safety warning
# ---------------------------------------------------------------------------
$CombinedDocs = $ControlSurfaceDoc + "`n" + $OperatorWorkflowsDoc

Assert-True ($CombinedDocs -match 'Test-DiscordAlert\.ps1\s+-CheckOnly') 'Docs include the Test-DiscordAlert.ps1 -CheckOnly command'
Assert-True (
    [regex]::IsMatch($CombinedDocs, 'Test-DiscordAlert\.ps1(?!\s+-CheckOnly)(\s|$)')
) 'Docs include the normal (non-CheckOnly) Test-DiscordAlert.ps1 command'
Assert-True ($CombinedDocs -match '(?i)never paste|do not paste|must never be (committed|pasted)') 'Docs warn not to paste the Discord webhook into tracked files'
Assert-True ($CombinedDocs -match '(?i)\.env\.local') 'Docs reference .env.local as the home for DISCORD_WEBHOOK_URL'

# ---------------------------------------------------------------------------
# Combined docs/scripts must not contain real-looking Discord webhook URLs
# ---------------------------------------------------------------------------
Assert-False ($CombinedDocs -match $RealLookingDiscordWebhookPattern) 'Runbook docs contain no real-looking Discord webhook URL'

if ($Failures -eq 0) {
    Write-Host '  ALL ASSERTIONS PASSED (test_discord_test_alert_offline)' -ForegroundColor Green
    exit 0
}

Write-Host "  $Failures ASSERTION(S) FAILED (test_discord_test_alert_offline)" -ForegroundColor Red
exit 1
