# =============================================================================
# Script guard: test_discord_paper_readiness_alerts.ps1
# DISCORD-PAPER-READINESS-ALERTS-01
#
# Static assertions for the offline paper-readiness / strategy-fit Discord
# alert workflow.
# No daemon, no DB, no live calls, no .env.local reads, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent (Split-Path -Parent $ScriptDir)
$AlertScriptPath = Join-Path $RepoRoot 'scripts\windows\Send-PaperReadinessDiscordAlert.ps1'
$ControlSurfaceDocPath = Join-Path $RepoRoot 'docs\runbooks\operator_control_surface.md'
$OperatorWorkflowsDocPath = Join-Path $RepoRoot 'docs\runbooks\operator_workflows.md'
$PaperReadinessRunnerPath = Join-Path $RepoRoot 'research-py\src\mqk_research\scanner\paper_readiness_runner.py'

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
Write-Host '--- test_discord_paper_readiness_alerts.ps1 ---'

# ---------------------------------------------------------------------------
# Existence
# ---------------------------------------------------------------------------
Assert-True (Test-Path $AlertScriptPath) 'Send-PaperReadinessDiscordAlert.ps1 exists'
Assert-True (Test-Path $ControlSurfaceDocPath) 'operator_control_surface.md exists'
Assert-True (Test-Path $OperatorWorkflowsDocPath) 'operator_workflows.md exists'

$AlertScript = Read-TextFile $AlertScriptPath
$ControlSurfaceDoc = Read-TextFile $ControlSurfaceDocPath
$OperatorWorkflowsDoc = Read-TextFile $OperatorWorkflowsDocPath

# ---------------------------------------------------------------------------
# Parser correctness (DISCORD-PAPER-READINESS-ALERT-SCRIPT-PARSE-FIX-01)
#
# This script previously contained an invalid single-quoted regex literal
# (["\']?) which is not valid PowerShell escaping and caused a parser-error
# cascade across the whole file. Assert the file actually parses, that the
# invalid literal is gone, and that the corrected doubled-single-quote form
# is present.
# ---------------------------------------------------------------------------
Assert-False ($AlertScript -match '\["\\''\]\?') 'Send-PaperReadinessDiscordAlert.ps1 does not contain the invalid ["\'']? single-quoted regex literal'
Assert-True ($AlertScript -match '\["''''\]\?') 'Send-PaperReadinessDiscordAlert.ps1 uses the PowerShell-safe ["'''']? doubled-single-quote regex literal'

$ParseErrors = $null
$ParseTokens = $null
[System.Management.Automation.Language.Parser]::ParseFile($AlertScriptPath, [ref]$ParseTokens, [ref]$ParseErrors) | Out-Null
Assert-True ($ParseErrors.Count -eq 0) "Send-PaperReadinessDiscordAlert.ps1 parses with zero errors via [System.Management.Automation.Language.Parser]::ParseFile (found $($ParseErrors.Count))"

# ---------------------------------------------------------------------------
# Parameters: -ArtifactPath (mandatory), -CheckOnly, -Title
# ---------------------------------------------------------------------------
Assert-True (
    [regex]::IsMatch($AlertScript, '\[Parameter\(Mandatory\s*=\s*\$true\)\]\s*\r?\n\s*\[string\]\$ArtifactPath')
) 'Send-PaperReadinessDiscordAlert.ps1 declares -ArtifactPath as a mandatory string parameter'
Assert-True ($AlertScript -match '\[switch\]\$CheckOnly') 'Send-PaperReadinessDiscordAlert.ps1 supports -CheckOnly'
Assert-True ($AlertScript -match '\[string\]\$Title') 'Send-PaperReadinessDiscordAlert.ps1 supports -Title'

$CheckOnlyMatch = [regex]::Match($AlertScript, '(?m)^\s*if\s*\(\s*\$CheckOnly\s*\)')
Assert-True $CheckOnlyMatch.Success 'Send-PaperReadinessDiscordAlert.ps1 branches on $CheckOnly before sending an alert'

# ---------------------------------------------------------------------------
# Must not call other lifecycle / trading / smoke scripts, daemon routes, or
# broker/Alpaca endpoints. This script sends directly to the Discord webhook
# and never contacts the daemon at all.
# ---------------------------------------------------------------------------
Assert-False ($AlertScript -match 'Start-PaperTradingSmoke\.ps1') 'Send-PaperReadinessDiscordAlert.ps1 does not call Start-PaperTradingSmoke.ps1'
Assert-False ($AlertScript -match 'Run-AAPL5mMarketSmoke\.ps1') 'Send-PaperReadinessDiscordAlert.ps1 does not call Run-AAPL5mMarketSmoke.ps1'
Assert-False ($AlertScript -match 'Launch-VeritasLedger\.ps1\s+-ArmPaper') 'Send-PaperReadinessDiscordAlert.ps1 does not call Launch-VeritasLedger.ps1 -ArmPaper'
Assert-False ($AlertScript -match '(?i)arm-execution') 'Send-PaperReadinessDiscordAlert.ps1 does not reference arm-execution'
Assert-False ($AlertScript -match '(?i)start-system') 'Send-PaperReadinessDiscordAlert.ps1 does not reference start-system'
Assert-False ($AlertScript -match '(?i)flatten-paper-positions') 'Send-PaperReadinessDiscordAlert.ps1 does not reference flatten-paper-positions'
Assert-False ($AlertScript -match '/api/v1/strategy/signal') 'Send-PaperReadinessDiscordAlert.ps1 does not call /api/v1/strategy/signal'
Assert-False ($AlertScript -match '/api/v1/ops/action') 'Send-PaperReadinessDiscordAlert.ps1 does not call /api/v1/ops/action (direct webhook send only)'
Assert-False ($AlertScript -match '"action_key"') 'Send-PaperReadinessDiscordAlert.ps1 does not reference action_key (not a daemon route call)'
Assert-False ($AlertScript -match '/v1/health') 'Send-PaperReadinessDiscordAlert.ps1 does not call /v1/health (no daemon contact)'
Assert-False ($AlertScript -match '127\.0\.0\.1') 'Send-PaperReadinessDiscordAlert.ps1 does not reference a daemon base URL (127.0.0.1)'
Assert-False ($AlertScript -match '\$env:MQK_OPERATOR_TOKEN') 'Send-PaperReadinessDiscordAlert.ps1 does not read $env:MQK_OPERATOR_TOKEN (direct webhook send needs no operator auth)'
Assert-False ($AlertScript -match '(?i)alpaca\.(markets|com)') 'Send-PaperReadinessDiscordAlert.ps1 does not reference Alpaca broker base URLs'
Assert-False ($AlertScript -match "(?i)https?://[^\s`"']*(alpaca|broker)") 'Send-PaperReadinessDiscordAlert.ps1 does not call any broker/Alpaca URL'

# ---------------------------------------------------------------------------
# Must not write/mutate any file (no repo mutation, no DB writes)
# ---------------------------------------------------------------------------
Assert-False ($AlertScript -match '(?i)\b(Set-Content|Out-File|Add-Content|New-Item)\b') 'Send-PaperReadinessDiscordAlert.ps1 does not write any file'

# ---------------------------------------------------------------------------
# Must not print secret env var values
# ---------------------------------------------------------------------------
$SecretPrintPattern = '(?im)^\s*(Write-Host|Write-Output|Write-Warning|Write-Verbose|Write-Information|echo|printf)\b[^\r\n]*\$env:(DISCORD_WEBHOOK_URL|MQK_OPERATOR_TOKEN)\b'
Assert-False ($AlertScript -match $SecretPrintPattern) 'Send-PaperReadinessDiscordAlert.ps1 does not print $env:DISCORD_WEBHOOK_URL or $env:MQK_OPERATOR_TOKEN'

# ---------------------------------------------------------------------------
# Must not contain real-looking Discord webhook URLs
# ---------------------------------------------------------------------------
$RealLookingDiscordWebhookPattern = 'https://(?:canary\.|ptb\.)?discord(?:app)?\.com/api/webhooks/\d{17,20}/[A-Za-z0-9_\-]{40,}'
Assert-False ($AlertScript -match $RealLookingDiscordWebhookPattern) 'Send-PaperReadinessDiscordAlert.ps1 contains no real-looking Discord webhook URL'

# ---------------------------------------------------------------------------
# Normal mode sends exactly one POST directly to $env:DISCORD_WEBHOOK_URL
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match '(?i)\$env:DISCORD_WEBHOOK_URL') 'Send-PaperReadinessDiscordAlert.ps1 references $env:DISCORD_WEBHOOK_URL'
Assert-True (
    [regex]::IsMatch($AlertScript, '(?i)Invoke-(WebRequest|RestMethod)\s+-Uri\s+\$env:DISCORD_WEBHOOK_URL\s+-Method\s+Post')
) 'Send-PaperReadinessDiscordAlert.ps1 POSTs directly to $env:DISCORD_WEBHOOK_URL'

$InvokeMatches = [regex]::Matches($AlertScript, '(?i)Invoke-(WebRequest|RestMethod)')
Assert-True ($InvokeMatches.Count -eq 1) "Send-PaperReadinessDiscordAlert.ps1 makes exactly one HTTP call (found $($InvokeMatches.Count))"

if ($CheckOnlyMatch.Success -and $InvokeMatches.Count -ge 1) {
    Assert-True ($CheckOnlyMatch.Index -lt $InvokeMatches[0].Index) '-CheckOnly branch (and its exit) occurs before the webhook POST call'
}

# Raw artifact JSON / object must never be sent as the request body.
Assert-False ([regex]::IsMatch($AlertScript, '-Body\s+\$Artifact(RawText)?\b')) 'Send-PaperReadinessDiscordAlert.ps1 never sends the raw artifact JSON as the request body'

# Only the artifact filename is sent, never the local path.
Assert-True ($AlertScript -match 'Split-Path\s+-Leaf\s+\$ArtifactPath') 'Send-PaperReadinessDiscordAlert.ps1 derives an artifact filename via Split-Path -Leaf (never sends the local path)'

# ---------------------------------------------------------------------------
# Supported artifact schemas
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match 'paper-readiness-v1') 'Send-PaperReadinessDiscordAlert.ps1 supports paper-readiness-v1 artifacts'
Assert-True ($AlertScript -match 'strategy-fit-v1') 'Send-PaperReadinessDiscordAlert.ps1 supports strategy-fit-v1 artifacts'

# ---------------------------------------------------------------------------
# Refusal: forged live-readiness flags
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match 'recommended_for_live') 'Send-PaperReadinessDiscordAlert.ps1 checks for recommended_for_live'
Assert-True ($AlertScript -match 'approved_for_live') 'Send-PaperReadinessDiscordAlert.ps1 checks for approved_for_live'
Assert-True ($AlertScript -match 'eligible_for_live') 'Send-PaperReadinessDiscordAlert.ps1 checks for eligible_for_live'
Assert-True (
    $AlertScript -match '\(recommended_for_live\|approved_for_live\|eligible_for_live\)'
) 'Send-PaperReadinessDiscordAlert.ps1 refuses on a forged recommended_for_live/approved_for_live/eligible_for_live=true flag'

# ---------------------------------------------------------------------------
# Refusal: webhook URL or secret/token embedded in the artifact
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match '(?i)api/webhooks') 'Send-PaperReadinessDiscordAlert.ps1 refuses artifacts containing a Discord webhook URL'
Assert-True ($AlertScript -match 'EnvVarSecretAssignmentPattern') 'Send-PaperReadinessDiscordAlert.ps1 refuses artifacts containing an embedded env-var-style secret'
Assert-True ($AlertScript -match 'BearerTokenPattern') 'Send-PaperReadinessDiscordAlert.ps1 refuses artifacts containing an embedded Bearer token'

# ---------------------------------------------------------------------------
# Reasons / failure_reasons capped to first 8
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match '\$MaxReasonItems\s*=\s*8') 'Send-PaperReadinessDiscordAlert.ps1 caps reasons/failure_reasons lists to the first 8 entries'

# ---------------------------------------------------------------------------
# Fail-closed behavior present
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match '(?i)fail.?closed') 'Send-PaperReadinessDiscordAlert.ps1 documents fail-closed behavior'
Assert-True ($AlertScript -match 'DISCORD_WEBHOOK_URL') 'Send-PaperReadinessDiscordAlert.ps1 checks DISCORD_WEBHOOK_URL configuration'

# ---------------------------------------------------------------------------
# Operator-triggered only -- never auto-invoked by the paper-readiness pipeline
# ---------------------------------------------------------------------------
Assert-True (Test-Path $PaperReadinessRunnerPath) 'paper_readiness_runner.py exists'
$PaperReadinessRunnerSrc = Read-TextFile $PaperReadinessRunnerPath
Assert-False ($PaperReadinessRunnerSrc -match 'Send-PaperReadinessDiscordAlert') 'paper_readiness_runner.py does not auto-invoke Send-PaperReadinessDiscordAlert.ps1'
Assert-False ($PaperReadinessRunnerSrc -match 'DISCORD_WEBHOOK_URL') 'paper_readiness_runner.py does not reference DISCORD_WEBHOOK_URL (alert sending stays operator-triggered)'

# ---------------------------------------------------------------------------
# Docs updated with CheckOnly and normal commands + webhook safety warning
# ---------------------------------------------------------------------------
$CombinedDocs = $ControlSurfaceDoc + "`n" + $OperatorWorkflowsDoc

$ScriptRefLines = $CombinedDocs -split "`r?`n" | Where-Object { $_ -match 'Send-PaperReadinessDiscordAlert\.ps1' }
Assert-True (($ScriptRefLines | Where-Object { $_ -match '-ArtifactPath' }).Count -gt 0) 'Docs include -ArtifactPath usage for Send-PaperReadinessDiscordAlert.ps1'
Assert-True (($ScriptRefLines | Where-Object { $_ -match '-CheckOnly' }).Count -gt 0) 'Docs include the Send-PaperReadinessDiscordAlert.ps1 -CheckOnly command'
Assert-True (($ScriptRefLines | Where-Object { $_ -notmatch '-CheckOnly' }).Count -gt 0) 'Docs include the normal (non-CheckOnly) Send-PaperReadinessDiscordAlert.ps1 command'

Assert-True ($CombinedDocs -match '(?i)never paste|do not paste|must never be (committed|pasted)') 'Docs warn not to paste the Discord webhook into tracked files'
Assert-True ($CombinedDocs -match '(?i)\.env\.local') 'Docs reference .env.local as the home for DISCORD_WEBHOOK_URL'
Assert-True ($CombinedDocs -match '(?i)observability') 'Docs note that Discord is observability only'
Assert-True ($CombinedDocs -match '(?i)operator-triggered|operator triggered') 'Docs note the workflow is operator-triggered only'

# ---------------------------------------------------------------------------
# Combined docs/scripts must not contain real-looking Discord webhook URLs
# ---------------------------------------------------------------------------
Assert-False ($CombinedDocs -match $RealLookingDiscordWebhookPattern) 'Runbook docs contain no real-looking Discord webhook URL'

if ($Failures -eq 0) {
    Write-Host '  ALL ASSERTIONS PASSED (test_discord_paper_readiness_alerts)' -ForegroundColor Green
    exit 0
}

Write-Host "  $Failures ASSERTION(S) FAILED (test_discord_paper_readiness_alerts)" -ForegroundColor Red
exit 1
