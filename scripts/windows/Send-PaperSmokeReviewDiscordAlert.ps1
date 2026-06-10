# =============================================================================
# DISCORD-EVIDENCE-REVIEW-ALERTS-01
# Send-PaperSmokeReviewDiscordAlert.ps1
#
# Optional, explicit-send Discord alert workflow for offline paper-smoke
# evidence review summaries produced by Review-PaperSmokeEvidence.ps1.
#
# This script reads ONE review_summary.json or review_summary.md file (or an
# evidence folder containing one), classifies it, and -- in normal mode only
# -- sends ONE sanitized Discord summary directly to $env:DISCORD_WEBHOOK_URL.
#
# Calls only:
#   POST $env:DISCORD_WEBHOOK_URL   (normal mode only; one sanitized summary;
#                                     direct webhook call, NOT via the daemon)
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperSmokeReviewDiscordAlert.ps1 -ReviewPath <path> -CheckOnly
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperSmokeReviewDiscordAlert.ps1 -ReviewPath <path> [-Title <text>]
#
# Parameters:
#   -ReviewPath  (required) Path to review_summary.json, review_summary.md,
#                or an evidence folder containing one of those files
#                (review_summary.json is preferred when both are present).
#   -CheckOnly   Verify, classify, and report only. Does NOT send a Discord alert.
#   -Title       Optional short title prefixed to the Discord message content.
#
# Supported review schema:
#   - review-v2 (Review-PaperSmokeEvidence.ps1 / PAPER-SMOKE-EVIDENCE-REVIEW-02),
#     read from review_summary.json (preferred) or review_summary.md (fallback).
#
# Known classification / VERDICT values (as produced by
# Review-PaperSmokeEvidence.ps1 -- this script never invents new values):
#   - NATURAL-TRADE-LIFECYCLE-CLOSED
#   - READINESS-CLOSED-NO-TRADE
#   - PARTIAL
#   - OPEN
#   - FALSE-CLOSED
#
# Hard rules enforced by this script:
#   - Operator-triggered only. Never invoked automatically by any pipeline.
#   - Does NOT start the daemon trading runtime, arm paper trading, submit
#     orders, call broker/Alpaca endpoints, touch OMS/runtime/strategy/
#     watchlist behavior, or write to the database.
#   - Does NOT change evidence classification logic or paper-readiness gates.
#   - Sends DIRECTLY to $env:DISCORD_WEBHOOK_URL -- the daemon is never
#     contacted by this script.
#   - Never prints the value of $env:DISCORD_WEBHOOK_URL.
#   - Sends the evidence FOLDER NAME and review FILE NAME only -- never the
#     full local path.
#   - Sends exactly one sanitized summary; never sends the raw review JSON or
#     Markdown contents.
#   - Refuses to send (fail-closed, exits non-zero, sends nothing) if:
#       * the review path does not exist, or no review_summary.json /
#         review_summary.md can be resolved from it,
#       * the review file is named review_summary.json but is not valid JSON,
#       * the JSON schema_version is not review-v2,
#       * a classification/VERDICT cannot be determined, or it is not one of
#         the known values listed above,
#       * the review file contains recommended_for_live=true,
#         approved_for_live=true, or eligible_for_live=true,
#       * the review file contains a Discord webhook URL,
#       * the review file contains an obvious secret/token,
#       * $env:DISCORD_WEBHOOK_URL is not configured.
#   - classification_reasons lists are capped to the first 8 entries.
# =============================================================================

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ReviewPath,

    [switch]$CheckOnly,

    [string]$Title
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
function Write-Step  { param([string]$M) Write-Host "[REVIEW-ALERT] $M" -ForegroundColor Cyan }
function Write-Ok    { param([string]$M) Write-Host "[REVIEW-ALERT] OK: $M" -ForegroundColor Green }
function Write-Warn  { param([string]$M) Write-Host "[REVIEW-ALERT] WARN: $M" -ForegroundColor Yellow }
function Write-Fail  { param([string]$M) Write-Host "[REVIEW-ALERT] FAIL: $M" -ForegroundColor Red }
function Write-Field { param([string]$K, $V, [string]$Color = 'White') Write-Host ("  {0,-32} {1}" -f "${K}:", $V) -ForegroundColor $Color }

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$MaxReasonItems = 8

$KnownClassifications = @(
    'NATURAL-TRADE-LIFECYCLE-CLOSED',
    'READINESS-CLOSED-NO-TRADE',
    'PARTIAL',
    'OPEN',
    'FALSE-CLOSED'
)

# Strict "real-looking" webhook pattern -- used by the script-guard / docs
# checks so placeholder URLs (e.g. .../webhooks/REPLACE_ME/alerts-daemon)
# remain allowed in tracked files.
$RealLookingDiscordWebhookPattern = 'https://(?:canary\.|ptb\.)?discord(?:app)?\.com/api/webhooks/\d{17,20}/[A-Za-z0-9_\-]{40,}'

# Loose pattern -- ANY discord webhook URL form (placeholder or real) inside
# a review file is refused. Review files should never contain webhook URLs.
$AnyDiscordWebhookUrlPattern = '(?i)discord(?:app)?\.com/api/webhooks/'

# Forged "live-ready" flags anywhere in the review text (JSON or Markdown).
$ForgedLiveFlagPattern = '(?i)"?(recommended_for_live|approved_for_live|eligible_for_live)"?\s*[:=]\s*true'

# Obvious embedded secrets / tokens.
$EnvVarSecretAssignmentPattern = '(?i)(ALPACA_API_SECRET_PAPER|ALPACA_API_KEY_PAPER|ALPACA_API_SECRET_LIVE|ALPACA_API_KEY_LIVE|ALPACA_API_SECRET|ALPACA_API_KEY|MQK_OPERATOR_TOKEN|DISCORD_WEBHOOK_URL|DISCORD_BOT_TOKEN)["\s]*[:=]\s*["'']?[A-Za-z0-9/_\-\.]{8,}'
$BearerTokenPattern = '(?i)Bearer\s+[A-Za-z0-9\-\._~\+/]{8,}'

Write-Host ''
Write-Host '=== Send-PaperSmokeReviewDiscordAlert.ps1 (DISCORD-EVIDENCE-REVIEW-ALERTS-01) ===' -ForegroundColor Cyan
Write-Host "    ReviewPath : $ReviewPath"
Write-Host "    Mode       : $(if ($CheckOnly) { 'CHECK-ONLY -- no Discord alert will be sent' } else { 'NORMAL -- sends one sanitized Discord summary alert' })"
Write-Host ''

Write-Step 'Safety confirmation'
Write-Host '  - This script does NOT start the daemon trading runtime.'
Write-Host '  - This script does NOT arm paper trading or submit/cancel/replace any order.'
Write-Host '  - This script does NOT call any broker or market-data endpoint.'
Write-Host '  - This script does NOT write to the database.'
Write-Host '  - This script does NOT change evidence classification logic or paper-readiness gates.'
Write-Host '  - This script sends directly to the configured Discord webhook (normal mode only) -- the daemon is never contacted.'
Write-Host '  - The Discord summary never includes the local evidence path, raw review contents, or any secret value.'

# ---------------------------------------------------------------------------
# Load .env.local (safe) -- only fills env vars not already set in the
# process environment. Values are never printed.
# ---------------------------------------------------------------------------
$envLocalPath = Join-Path $RepoRoot '.env.local'
$loadedCount = 0
if (Test-Path $envLocalPath) {
    foreach ($line in Get-Content -Path $envLocalPath) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        $trimmed = $line.Trim()
        if ($trimmed.StartsWith('#')) { continue }
        $idx = $trimmed.IndexOf('=')
        if ($idx -lt 1) { continue }
        $varName  = $trimmed.Substring(0, $idx).Trim()
        $varValue = $trimmed.Substring($idx + 1).Trim()
        if (($varValue.StartsWith('"') -and $varValue.EndsWith('"')) -or
            ($varValue.StartsWith("'") -and $varValue.EndsWith("'"))) {
            if ($varValue.Length -ge 2) { $varValue = $varValue.Substring(1, $varValue.Length - 2) }
        }
        if ([string]::IsNullOrWhiteSpace($varName)) { continue }
        $existing = [Environment]::GetEnvironmentVariable($varName, 'Process')
        if ([string]::IsNullOrWhiteSpace($existing)) {
            Set-Item -Path "Env:$varName" -Value $varValue
            $loadedCount++
        }
    }
    Write-Ok ".env.local found at $envLocalPath ($loadedCount env var(s) loaded; values not printed)"
} else {
    Write-Warn ".env.local not found at $envLocalPath (continuing with process environment only)"
}

$webhookConfigured = -not [string]::IsNullOrWhiteSpace($env:DISCORD_WEBHOOK_URL)

# ---------------------------------------------------------------------------
# Resolve review path -- accepts a direct file path (review_summary.json or
# review_summary.md) or a folder containing one (json preferred over md).
# Fail closed in both modes if nothing usable is found.
# ---------------------------------------------------------------------------
Write-Host ''
Write-Step 'Resolving review path'

if (-not (Test-Path $ReviewPath)) {
    Write-Fail "Review path not found: $ReviewPath. Refusing to proceed (fail-closed)."
    exit 1
}

$ReviewItem = Get-Item -Path $ReviewPath -ErrorAction Stop
$ResolvedReviewPath = $null

if ($ReviewItem.PSIsContainer) {
    $jsonCandidate = Join-Path $ReviewItem.FullName 'review_summary.json'
    $mdCandidate   = Join-Path $ReviewItem.FullName 'review_summary.md'
    if (Test-Path $jsonCandidate -PathType Leaf) {
        $ResolvedReviewPath = $jsonCandidate
    } elseif (Test-Path $mdCandidate -PathType Leaf) {
        $ResolvedReviewPath = $mdCandidate
    } else {
        Write-Fail "No review_summary.json or review_summary.md found in folder: $($ReviewItem.FullName). Refusing to proceed (fail-closed)."
        exit 1
    }
} else {
    $leafName = Split-Path -Leaf $ReviewItem.FullName
    if ($leafName -eq 'review_summary.json' -or $leafName -eq 'review_summary.md') {
        $ResolvedReviewPath = $ReviewItem.FullName
    } else {
        Write-Fail "ReviewPath must be review_summary.json, review_summary.md, or a folder containing one. Got: $leafName. Refusing to proceed (fail-closed)."
        exit 1
    }
}

$ReviewFileName = Split-Path -Leaf $ResolvedReviewPath
$ReviewFormat = if ($ReviewFileName -eq 'review_summary.json') { 'json' } else { 'markdown' }

$ReviewRawText = Get-Content -Path $ResolvedReviewPath -Raw
if ($ReviewRawText.Length -gt 0 -and $ReviewRawText[0] -eq [char]0xFEFF) {
    $ReviewRawText = $ReviewRawText.Substring(1)
}

Write-Ok "Review file resolved: $ReviewFileName (format: $ReviewFormat)"

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
function Get-JsonValue {
    param($Object, [string]$Name, $Default = $null)
    if ($null -eq $Object) { return $Default }
    $prop = $Object.PSObject.Properties[$Name]
    if ($null -eq $prop) { return $Default }
    if ($null -eq $prop.Value) { return $Default }
    return $prop.Value
}

function Get-LimitedList {
    param([array]$Items, [int]$MaxItems)
    $all = @($Items)
    $limited = if ($all.Count -gt $MaxItems) { @($all[0..($MaxItems - 1)]) } else { $all }
    [PSCustomObject]@{
        Items      = $limited
        Truncated  = ($all.Count -gt $MaxItems)
        TotalCount = $all.Count
    }
}

function Format-NullableValue {
    param($Value)
    if ($null -eq $Value) { return '(none)' }
    return "$Value"
}

function Get-MdFieldValue {
    param([string]$Text, [string]$FieldName)
    $pattern = '(?m)^- ' + [regex]::Escape($FieldName) + ':\s*(.*?)\s*$'
    $m = [regex]::Match($Text, $pattern)
    if ($m.Success) { return $m.Groups[1].Value }
    return $null
}

function ConvertTo-BoolIfPossible {
    param($Value)
    if ($null -eq $Value) { return $null }
    if ("$Value" -eq 'True') { return $true }
    if ("$Value" -eq 'False') { return $false }
    if ([string]::IsNullOrWhiteSpace("$Value")) { return $null }
    return $Value
}

# ---------------------------------------------------------------------------
# Parse + extract fields
# ---------------------------------------------------------------------------
Write-Host ''
Write-Step 'Parsing review file'

$SchemaVersion          = $null
$Classification         = $null
$ClassificationReasons  = @()
$FolderName             = $null
$LiveRoutingEnabled     = $null
$TradeLifecycleDetected = $null
$ReconcileStatus        = $null
$RuntimeStatus          = $null
$ArmState               = $null
$NextOperatorAction     = $null
$DiscordObservationStatus = $null
$GuiObservationStatus   = $null

if ($ReviewFormat -eq 'json') {
    try {
        $Review = $ReviewRawText | ConvertFrom-Json -ErrorAction Stop
    } catch {
        Write-Fail "Review file is not valid JSON: $ReviewFileName. Refusing to proceed (fail-closed)."
        exit 1
    }

    $SchemaVersion          = Get-JsonValue $Review 'schema_version' ''
    $Classification         = Get-JsonValue $Review 'classification' $null
    $ClassificationReasons  = @(Get-JsonValue $Review 'classification_reasons' @())
    $FolderName             = Get-JsonValue $Review 'folder_name' $null
    $LiveRoutingEnabled     = Get-JsonValue $Review 'live_routing_enabled' $null
    $TradeLifecycleDetected = Get-JsonValue $Review 'trade_lifecycle_detected' $null
    $ReconcileStatus        = Get-JsonValue $Review 'reconcile_status' $null
    $RuntimeStatus          = Get-JsonValue $Review 'runtime_status' $null
    $ArmState               = Get-JsonValue $Review 'arm_state' $null
    $NextOperatorAction     = Get-JsonValue $Review 'autonomous_next_operator_action' $null

    $LedgerClassifications = Get-JsonValue $Review 'ledger_classifications' $null
    if ($null -ne $LedgerClassifications) {
        $discordLedger = Get-JsonValue $LedgerClassifications 'discord_trade_lifecycle_real' $null
        if ($null -ne $discordLedger) {
            $DiscordObservationStatus = Get-JsonValue $discordLedger 'status' $null
        }
        $guiLedger = Get-JsonValue $LedgerClassifications 'gui_trade_lifecycle_visibility' $null
        if ($null -ne $guiLedger) {
            $GuiObservationStatus = Get-JsonValue $guiLedger 'status' $null
        }
    }

    Write-Ok "Review JSON parsed: $ReviewFileName"
} else {
    $verdictMatch = [regex]::Match($ReviewRawText, '(?m)^### VERDICT:\s*(\S+)[ \t]*\r?\n((?:- .+\r?\n?)*)')
    if ($verdictMatch.Success) {
        $Classification = $verdictMatch.Groups[1].Value
        $reasonLines = $verdictMatch.Groups[2].Value -split "\r?\n" | Where-Object { $_ -match '^- ' }
        $ClassificationReasons = @($reasonLines | ForEach-Object { $_.Substring(2).Trim() })
    }

    $FolderName             = Get-MdFieldValue $ReviewRawText 'Folder name'
    $LiveRoutingEnabled     = ConvertTo-BoolIfPossible (Get-MdFieldValue $ReviewRawText 'live_routing_enabled')
    $TradeLifecycleDetected = ConvertTo-BoolIfPossible (Get-MdFieldValue $ReviewRawText 'trade_lifecycle_detected')
    $ReconcileStatus        = Get-MdFieldValue $ReviewRawText 'reconcile_status'
    $RuntimeStatus          = Get-MdFieldValue $ReviewRawText 'runtime_status'
    $ArmState               = Get-MdFieldValue $ReviewRawText 'arm_state'
    $NextOperatorAction     = Get-MdFieldValue $ReviewRawText 'next_operator_action'

    Write-Ok "Review Markdown parsed: $ReviewFileName"
    Write-Warn 'ledger-specific observation statuses (discord/gui) are only available from review_summary.json; not extracted from Markdown.'
}

$ReasonsLimited = Get-LimitedList -Items $ClassificationReasons -MaxItems $MaxReasonItems

# ---------------------------------------------------------------------------
# Refusal checks -- evaluated against the raw review text. Order matters only
# for the reported reason; every check below is independently fail-closed.
# ---------------------------------------------------------------------------
$RefusalReason = $null

if ($ReviewRawText -match $ForgedLiveFlagPattern) {
    $RefusalReason = 'review file contains a forged live-readiness flag (recommended_for_live / approved_for_live / eligible_for_live = true)'
} elseif ($ReviewRawText -match $AnyDiscordWebhookUrlPattern) {
    $RefusalReason = 'review file appears to contain a Discord webhook URL'
} elseif (($ReviewRawText -match $EnvVarSecretAssignmentPattern) -or ($ReviewRawText -match $BearerTokenPattern)) {
    $RefusalReason = 'review file appears to contain an embedded secret or token'
} elseif ($ReviewFormat -eq 'json' -and $SchemaVersion -ne 'review-v2') {
    $shownVersion = if ([string]::IsNullOrWhiteSpace($SchemaVersion)) { '(missing)' } else { $SchemaVersion }
    $RefusalReason = "unsupported or missing schema_version ('$shownVersion'); supported: review-v2"
} elseif ([string]::IsNullOrWhiteSpace($Classification)) {
    $RefusalReason = 'could not determine a classification/VERDICT from the review file'
} elseif ($KnownClassifications -notcontains $Classification) {
    $RefusalReason = "unrecognized classification '$Classification'; known values: $($KnownClassifications -join ', ')"
}

# ---------------------------------------------------------------------------
# Classification labels
# ---------------------------------------------------------------------------
$ClassificationLabels = @{
    'NATURAL-TRADE-LIFECYCLE-CLOSED' = 'NATURAL TRADE LIFECYCLE: CLOSED'
    'READINESS-CLOSED-NO-TRADE'      = 'READINESS: CLOSED (NO TRADE)'
    'PARTIAL'                        = 'PARTIAL LIFECYCLE'
    'OPEN'                           = 'OPEN (BLOCKER PRESENT)'
    'FALSE-CLOSED'                   = 'FALSE-CLOSED (DO NOT TRUST)'
}
$ClassificationLabel = $ClassificationLabels[$Classification]
if ($null -eq $ClassificationLabel) { $ClassificationLabel = Format-NullableValue $Classification }

# ---------------------------------------------------------------------------
# Sanitized summary (printed in both modes)
# ---------------------------------------------------------------------------
Write-Host ''
Write-Step 'Sanitized review summary (no secret values, local path not shown)'
Write-Field 'review_filename'   $ReviewFileName
Write-Field 'review_format'     $ReviewFormat
Write-Field 'schema_version'    $(if ([string]::IsNullOrWhiteSpace($SchemaVersion)) { '(not present in this format)' } else { $SchemaVersion })
Write-Field 'evidence_folder_name' $(Format-NullableValue $FolderName)
Write-Field 'classification'    $(Format-NullableValue $Classification)
Write-Field 'classification_label' $ClassificationLabel
Write-Field 'reasons'           ('[' + (($ReasonsLimited.Items | ForEach-Object { Format-NullableValue $_ }) -join '; ') + ']')
Write-Field 'reasons_truncated' $ReasonsLimited.Truncated
Write-Field 'reasons_total_count' $ReasonsLimited.TotalCount
Write-Field 'live_routing_enabled' $(Format-NullableValue $LiveRoutingEnabled)
Write-Field 'trade_lifecycle_detected' $(Format-NullableValue $TradeLifecycleDetected)
Write-Field 'reconcile_status'  $(Format-NullableValue $ReconcileStatus)
Write-Field 'runtime_status'    $(Format-NullableValue $RuntimeStatus)
Write-Field 'arm_state'         $(Format-NullableValue $ArmState)
Write-Field 'next_operator_action' $(Format-NullableValue $NextOperatorAction)
Write-Field 'discord_observation_status' $(Format-NullableValue $DiscordObservationStatus)
Write-Field 'gui_observation_status' $(Format-NullableValue $GuiObservationStatus)

$Sendable = ($null -eq $RefusalReason) -and $webhookConfigured

Write-Host ''
Write-Step 'Send eligibility'
Write-Field 'discord_webhook_configured' $webhookConfigured $(if ($webhookConfigured) { 'Green' } else { 'Yellow' })
Write-Field 'refusal_reason' $(if ($null -eq $RefusalReason) { '(none)' } else { $RefusalReason }) $(if ($null -eq $RefusalReason) { 'Green' } else { 'Yellow' })
Write-Field 'sendable' $Sendable $(if ($Sendable) { 'Green' } else { 'Yellow' })

# ---------------------------------------------------------------------------
# CheckOnly: report and exit. No alert sent.
# ---------------------------------------------------------------------------
if ($CheckOnly) {
    Write-Host ''
    Write-Step 'CheckOnly mode -- no Discord alert sent, no webhook POST issued.'
    if (-not $webhookConfigured) {
        Write-Warn 'DISCORD_WEBHOOK_URL is not configured. The normal send would fail closed without it.'
    }
    if ($null -ne $RefusalReason) {
        Write-Warn "The normal send would refuse: $RefusalReason"
    }
    Write-Host ''
    exit 0
}

# ---------------------------------------------------------------------------
# Normal mode: fail closed before attempting delivery.
# ---------------------------------------------------------------------------
Write-Host ''
Write-Step 'Normal mode -- sending one sanitized Discord summary alert.'

if ($null -ne $RefusalReason) {
    Write-Fail "Refusing to send: $RefusalReason. (fail-closed)"
    exit 1
}

if (-not $webhookConfigured) {
    Write-Fail 'DISCORD_WEBHOOK_URL is not configured. Add it to .env.local. Refusing to proceed (fail-closed).'
    exit 1
}

# ---------------------------------------------------------------------------
# Build sanitized payload
# ---------------------------------------------------------------------------
$AlertGeneratedAtUtc = [DateTime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ssZ')

$contentLines = [System.Collections.Generic.List[string]]::new()
if (-not [string]::IsNullOrWhiteSpace($Title)) {
    $contentLines.Add($Title)
}
$contentLines.Add('[MiniQuantDesk] Paper smoke evidence review')
$contentLines.Add("evidence_folder: $(Format-NullableValue $FolderName)")
$contentLines.Add("review_file: $ReviewFileName")
$contentLines.Add("VERDICT: $Classification ($ClassificationLabel)")

if ($ReasonsLimited.TotalCount -gt 0) {
    $reasonsText = ($ReasonsLimited.Items -join '; ')
    if ($ReasonsLimited.Truncated) {
        $reasonsText += " (+$($ReasonsLimited.TotalCount - $MaxReasonItems) more)"
    }
    $contentLines.Add("reasons: $reasonsText")
}

$contentLines.Add("live_routing_enabled: $(Format-NullableValue $LiveRoutingEnabled) | trade_lifecycle_detected: $(Format-NullableValue $TradeLifecycleDetected) | reconcile_status: $(Format-NullableValue $ReconcileStatus)")
$contentLines.Add("runtime_status: $(Format-NullableValue $RuntimeStatus) | arm_state: $(Format-NullableValue $ArmState)")

if (-not [string]::IsNullOrWhiteSpace($NextOperatorAction)) {
    $contentLines.Add("next_operator_action: $NextOperatorAction")
}
if (-not [string]::IsNullOrWhiteSpace($DiscordObservationStatus)) {
    $contentLines.Add("discord_observation: $DiscordObservationStatus")
}
if (-not [string]::IsNullOrWhiteSpace($GuiObservationStatus)) {
    $contentLines.Add("gui_observation: $GuiObservationStatus")
}

$Content = $contentLines -join "`n"

$PayloadObject = [ordered]@{
    content                             = $Content
    alert_type                          = 'paper_smoke_evidence_review'
    review_format                       = $ReviewFormat
    schema_version                      = $SchemaVersion
    review_filename                     = $ReviewFileName
    evidence_folder_name                = $FolderName
    classification                      = $Classification
    classification_label                = $ClassificationLabel
    classification_reasons              = $ReasonsLimited.Items
    classification_reasons_truncated    = $ReasonsLimited.Truncated
    classification_reasons_total_count  = $ReasonsLimited.TotalCount
    live_routing_enabled                = $LiveRoutingEnabled
    trade_lifecycle_detected            = $TradeLifecycleDetected
    reconcile_status                    = $ReconcileStatus
    runtime_status                      = $RuntimeStatus
    arm_state                           = $ArmState
    next_operator_action                = $NextOperatorAction
    discord_observation_status          = $DiscordObservationStatus
    gui_observation_status              = $GuiObservationStatus
    alert_generated_at_utc              = $AlertGeneratedAtUtc
}
if (-not [string]::IsNullOrWhiteSpace($Title)) {
    $PayloadObject['title'] = $Title
}

$PayloadJson = $PayloadObject | ConvertTo-Json -Depth 6 -Compress

# ---------------------------------------------------------------------------
# Send -- one direct POST to the Discord webhook URL. Never log the URL.
# ---------------------------------------------------------------------------
try {
    $resp = Invoke-WebRequest -Uri $env:DISCORD_WEBHOOK_URL -Method Post `
        -ContentType 'application/json' -Body $PayloadJson `
        -TimeoutSec 10 -ErrorAction Stop
    $statusCode = [int]$resp.StatusCode
} catch {
    $statusCode = $null
    if ($_.Exception.PSObject.Properties['Response'] -and $_.Exception.Response) {
        try { $statusCode = [int]$_.Exception.Response.StatusCode } catch {}
    }
    if ($null -ne $statusCode) {
        Write-Fail "Discord webhook POST failed with HTTP status $statusCode."
    } else {
        Write-Fail "Discord webhook POST failed (network/transport error)."
    }
    exit 1
}

Write-Host ''
Write-Step 'Result (sanitized -- no webhook URL included)'
Write-Field 'http_status' $statusCode $(if ($statusCode -ge 200 -and $statusCode -lt 300) { 'Green' } else { 'Red' })
Write-Field 'classification' $Classification
Write-Field 'review_filename' $ReviewFileName

if ($statusCode -ge 200 -and $statusCode -lt 300) {
    Write-Ok 'Discord delivery succeeded. Check the configured Discord channel for the summary message.'
} else {
    Write-Warn "Discord webhook returned non-success status $statusCode."
    exit 1
}

Write-Host ''
exit 0
