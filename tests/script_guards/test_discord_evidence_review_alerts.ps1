# =============================================================================
# Script guard: test_discord_evidence_review_alerts.ps1
# DISCORD-EVIDENCE-REVIEW-ALERTS-01
#
# Static assertions for the offline paper-smoke evidence review Discord alert
# workflow.
# No daemon, no DB, no live calls, no .env.local reads, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent (Split-Path -Parent $ScriptDir)
$AlertScriptPath = Join-Path $RepoRoot 'scripts\windows\Send-PaperSmokeReviewDiscordAlert.ps1'
$ControlSurfaceDocPath = Join-Path $RepoRoot 'docs\runbooks\operator_control_surface.md'
$OperatorWorkflowsDocPath = Join-Path $RepoRoot 'docs\runbooks\operator_workflows.md'
$ReviewRunnerPath = Join-Path $RepoRoot 'scripts\windows\Review-PaperSmokeEvidence.ps1'

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
Write-Host '--- test_discord_evidence_review_alerts.ps1 ---'

# ---------------------------------------------------------------------------
# Existence
# ---------------------------------------------------------------------------
Assert-True (Test-Path $AlertScriptPath) 'Send-PaperSmokeReviewDiscordAlert.ps1 exists'
Assert-True (Test-Path $ControlSurfaceDocPath) 'operator_control_surface.md exists'
Assert-True (Test-Path $OperatorWorkflowsDocPath) 'operator_workflows.md exists'
Assert-True (Test-Path $ReviewRunnerPath) 'Review-PaperSmokeEvidence.ps1 exists'

$AlertScript = Read-TextFile $AlertScriptPath
$ControlSurfaceDoc = Read-TextFile $ControlSurfaceDocPath
$OperatorWorkflowsDoc = Read-TextFile $OperatorWorkflowsDocPath
$ReviewRunnerSrc = Read-TextFile $ReviewRunnerPath

# ---------------------------------------------------------------------------
# Parameters: -ReviewPath (mandatory), -CheckOnly, -Title
# ---------------------------------------------------------------------------
Assert-True (
    [regex]::IsMatch($AlertScript, '\[Parameter\(Mandatory\s*=\s*\$true\)\]\s*\r?\n\s*\[string\]\$ReviewPath')
) 'Send-PaperSmokeReviewDiscordAlert.ps1 declares -ReviewPath as a mandatory string parameter'
Assert-True ($AlertScript -match '\[switch\]\$CheckOnly') 'Send-PaperSmokeReviewDiscordAlert.ps1 supports -CheckOnly'
Assert-True ($AlertScript -match '\[string\]\$Title') 'Send-PaperSmokeReviewDiscordAlert.ps1 supports -Title'

$CheckOnlyMatch = [regex]::Match($AlertScript, '(?m)^\s*if\s*\(\s*\$CheckOnly\s*\)')
Assert-True $CheckOnlyMatch.Success 'Send-PaperSmokeReviewDiscordAlert.ps1 branches on $CheckOnly before sending an alert'

# ---------------------------------------------------------------------------
# Must not call other lifecycle / trading / smoke scripts, daemon routes, or
# broker/Alpaca endpoints. This script sends directly to the Discord webhook
# and never contacts the daemon at all.
# ---------------------------------------------------------------------------
Assert-False ($AlertScript -match 'Start-PaperTradingSmoke\.ps1') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not call Start-PaperTradingSmoke.ps1'
Assert-False ($AlertScript -match 'Run-AAPL5mMarketSmoke\.ps1') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not call Run-AAPL5mMarketSmoke.ps1'
Assert-False ($AlertScript -match 'Launch-VeritasLedger\.ps1\s+-ArmPaper') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not call Launch-VeritasLedger.ps1 -ArmPaper'
Assert-False ($AlertScript -match '(?i)arm-execution') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not reference arm-execution'
Assert-False ($AlertScript -match '(?i)start-system') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not reference start-system'
Assert-False ($AlertScript -match '(?i)flatten-paper-positions') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not reference flatten-paper-positions'
Assert-False ($AlertScript -match '/api/v1/strategy/signal') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not call /api/v1/strategy/signal'
Assert-False ($AlertScript -match '/api/v1/ops/action') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not call /api/v1/ops/action (direct webhook send only)'
Assert-False ($AlertScript -match '"action_key"') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not reference action_key (not a daemon route call)'
Assert-False ($AlertScript -match '/v1/health') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not call /v1/health (no daemon contact)'
Assert-False ($AlertScript -match '127\.0\.0\.1') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not reference a daemon base URL (127.0.0.1)'
Assert-False ($AlertScript -match '\$env:MQK_OPERATOR_TOKEN') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not read $env:MQK_OPERATOR_TOKEN (direct webhook send needs no operator auth)'
Assert-False ($AlertScript -match '(?i)alpaca\.(markets|com)') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not reference Alpaca broker base URLs'
Assert-False ($AlertScript -match "(?i)https?://[^\s`"']*(alpaca|broker)") 'Send-PaperSmokeReviewDiscordAlert.ps1 does not call any broker/Alpaca URL'

# ---------------------------------------------------------------------------
# Must never read or reference notes/final_verdict.txt -- it is an
# operator-completed note/template only, not the source of truth
# (PAPER-SMOKE-EVIDENCE-VERDICT-HYGIENE-01). review_summary.json/.md remain
# the only inputs to this alert.
# ---------------------------------------------------------------------------
Assert-False ($AlertScript -match '(?i)final_verdict') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not reference notes/final_verdict.txt'

# ---------------------------------------------------------------------------
# Must not write/mutate any file (no repo mutation, no DB writes) and must not
# change evidence classification logic.
# ---------------------------------------------------------------------------
Assert-False ($AlertScript -match '(?i)\b(Set-Content|Out-File|Add-Content|New-Item)\b') 'Send-PaperSmokeReviewDiscordAlert.ps1 does not write any file'

# ---------------------------------------------------------------------------
# Must not print secret env var values
# ---------------------------------------------------------------------------
$SecretPrintPattern = '(?im)^\s*(Write-Host|Write-Output|Write-Warning|Write-Verbose|Write-Information|echo|printf)\b[^\r\n]*\$env:(DISCORD_WEBHOOK_URL|MQK_OPERATOR_TOKEN)\b'
Assert-False ($AlertScript -match $SecretPrintPattern) 'Send-PaperSmokeReviewDiscordAlert.ps1 does not print $env:DISCORD_WEBHOOK_URL or $env:MQK_OPERATOR_TOKEN'

# ---------------------------------------------------------------------------
# Must not contain real-looking Discord webhook URLs
# ---------------------------------------------------------------------------
$RealLookingDiscordWebhookPattern = 'https://(?:canary\.|ptb\.)?discord(?:app)?\.com/api/webhooks/\d{17,20}/[A-Za-z0-9_\-]{40,}'
Assert-False ($AlertScript -match $RealLookingDiscordWebhookPattern) 'Send-PaperSmokeReviewDiscordAlert.ps1 contains no real-looking Discord webhook URL'

# ---------------------------------------------------------------------------
# Normal mode sends exactly one POST directly to $env:DISCORD_WEBHOOK_URL
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match '(?i)\$env:DISCORD_WEBHOOK_URL') 'Send-PaperSmokeReviewDiscordAlert.ps1 references $env:DISCORD_WEBHOOK_URL'
Assert-True (
    [regex]::IsMatch($AlertScript, '(?i)Invoke-(WebRequest|RestMethod)\s+-Uri\s+\$env:DISCORD_WEBHOOK_URL\s+-Method\s+Post')
) 'Send-PaperSmokeReviewDiscordAlert.ps1 POSTs directly to $env:DISCORD_WEBHOOK_URL'

$InvokeMatches = [regex]::Matches($AlertScript, '(?i)Invoke-(WebRequest|RestMethod)')
Assert-True ($InvokeMatches.Count -eq 1) "Send-PaperSmokeReviewDiscordAlert.ps1 makes exactly one HTTP call (found $($InvokeMatches.Count))"

if ($CheckOnlyMatch.Success -and $InvokeMatches.Count -ge 1) {
    Assert-True ($CheckOnlyMatch.Index -lt $InvokeMatches[0].Index) '-CheckOnly branch (and its exit) occurs before the webhook POST call'
}

# Raw review JSON / Markdown must never be sent as the request body.
Assert-False ([regex]::IsMatch($AlertScript, '-Body\s+\$Review(RawText)?\b')) 'Send-PaperSmokeReviewDiscordAlert.ps1 never sends the raw review JSON/Markdown as the request body'

# Only the review filename + evidence folder name are sent, never the local path.
Assert-True ($AlertScript -match 'Split-Path\s+-Leaf\s+\$ResolvedReviewPath') 'Send-PaperSmokeReviewDiscordAlert.ps1 derives a review filename via Split-Path -Leaf (never sends the local path)'

# ---------------------------------------------------------------------------
# Supported review path forms / schema
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match 'review_summary\.json') 'Send-PaperSmokeReviewDiscordAlert.ps1 supports review_summary.json'
Assert-True ($AlertScript -match 'review_summary\.md') 'Send-PaperSmokeReviewDiscordAlert.ps1 supports review_summary.md'
Assert-True ($AlertScript -match 'review-v2') 'Send-PaperSmokeReviewDiscordAlert.ps1 supports the review-v2 schema'

# ---------------------------------------------------------------------------
# Known classification / VERDICT values (must match Review-PaperSmokeEvidence.ps1
# -- this script must never invent new classification values)
# ---------------------------------------------------------------------------
$KnownClassifications = @(
    'NATURAL-TRADE-LIFECYCLE-CLOSED',
    'READINESS-CLOSED-NO-TRADE',
    'PARTIAL',
    'OPEN',
    'FALSE-CLOSED'
)
foreach ($cls in $KnownClassifications) {
    Assert-True ($AlertScript -match [regex]::Escape($cls)) "Send-PaperSmokeReviewDiscordAlert.ps1 references known classification $cls"
    Assert-True ($ReviewRunnerSrc -match [regex]::Escape($cls)) "Review-PaperSmokeEvidence.ps1 produces known classification $cls"
}

# ---------------------------------------------------------------------------
# Refusal: forged live-readiness flags
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match 'recommended_for_live') 'Send-PaperSmokeReviewDiscordAlert.ps1 checks for recommended_for_live'
Assert-True ($AlertScript -match 'approved_for_live') 'Send-PaperSmokeReviewDiscordAlert.ps1 checks for approved_for_live'
Assert-True ($AlertScript -match 'eligible_for_live') 'Send-PaperSmokeReviewDiscordAlert.ps1 checks for eligible_for_live'
Assert-True (
    $AlertScript -match '\(recommended_for_live\|approved_for_live\|eligible_for_live\)'
) 'Send-PaperSmokeReviewDiscordAlert.ps1 refuses on a forged recommended_for_live/approved_for_live/eligible_for_live=true flag'

# ---------------------------------------------------------------------------
# Refusal: webhook URL or secret/token embedded in the review file
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match '(?i)api/webhooks') 'Send-PaperSmokeReviewDiscordAlert.ps1 refuses review files containing a Discord webhook URL'
Assert-True ($AlertScript -match 'EnvVarSecretAssignmentPattern') 'Send-PaperSmokeReviewDiscordAlert.ps1 refuses review files containing an embedded env-var-style secret'
Assert-True ($AlertScript -match 'BearerTokenPattern') 'Send-PaperSmokeReviewDiscordAlert.ps1 refuses review files containing an embedded Bearer token'

# ---------------------------------------------------------------------------
# Reasons capped to first 8
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match '\$MaxReasonItems\s*=\s*8') 'Send-PaperSmokeReviewDiscordAlert.ps1 caps classification_reasons lists to the first 8 entries'

# ---------------------------------------------------------------------------
# Fail-closed behavior present
# ---------------------------------------------------------------------------
Assert-True ($AlertScript -match '(?i)fail.?closed') 'Send-PaperSmokeReviewDiscordAlert.ps1 documents fail-closed behavior'
Assert-True ($AlertScript -match 'DISCORD_WEBHOOK_URL') 'Send-PaperSmokeReviewDiscordAlert.ps1 checks DISCORD_WEBHOOK_URL configuration'

# ---------------------------------------------------------------------------
# Operator-triggered only -- never auto-invoked by the evidence review tool
# ---------------------------------------------------------------------------
Assert-False ($ReviewRunnerSrc -match 'Send-PaperSmokeReviewDiscordAlert') 'Review-PaperSmokeEvidence.ps1 does not auto-invoke Send-PaperSmokeReviewDiscordAlert.ps1'

# ---------------------------------------------------------------------------
# Docs updated with -ReviewPath, -CheckOnly and normal commands + webhook
# safety warning
# ---------------------------------------------------------------------------
$CombinedDocs = $ControlSurfaceDoc + "`n" + $OperatorWorkflowsDoc

$ScriptRefLines = $CombinedDocs -split "`r?`n" | Where-Object { $_ -match 'Send-PaperSmokeReviewDiscordAlert\.ps1' }
Assert-True (($ScriptRefLines | Where-Object { $_ -match '-ReviewPath' }).Count -gt 0) 'Docs include -ReviewPath usage for Send-PaperSmokeReviewDiscordAlert.ps1'
Assert-True (($ScriptRefLines | Where-Object { $_ -match '-CheckOnly' }).Count -gt 0) 'Docs include the Send-PaperSmokeReviewDiscordAlert.ps1 -CheckOnly command'
Assert-True (($ScriptRefLines | Where-Object { $_ -notmatch '-CheckOnly' }).Count -gt 0) 'Docs include the normal (non-CheckOnly) Send-PaperSmokeReviewDiscordAlert.ps1 command'

Assert-True ($CombinedDocs -match '(?i)never paste|do not paste|must never be (committed|pasted)') 'Docs warn not to paste the Discord webhook into tracked files'
Assert-True ($CombinedDocs -match '(?i)\.env\.local') 'Docs reference .env.local as the home for DISCORD_WEBHOOK_URL'
Assert-True ($CombinedDocs -match '(?i)observability') 'Docs note that Discord is observability only'
Assert-True ($CombinedDocs -match '(?i)operator-triggered|operator triggered') 'Docs note the workflow is operator-triggered only'

# ---------------------------------------------------------------------------
# Combined docs/scripts must not contain real-looking Discord webhook URLs
# ---------------------------------------------------------------------------
Assert-False ($CombinedDocs -match $RealLookingDiscordWebhookPattern) 'Runbook docs contain no real-looking Discord webhook URL'

if ($Failures -eq 0) {
    Write-Host '  ALL ASSERTIONS PASSED (test_discord_evidence_review_alerts)' -ForegroundColor Green
    exit 0
}

Write-Host "  $Failures ASSERTION(S) FAILED (test_discord_evidence_review_alerts)" -ForegroundColor Red
exit 1
