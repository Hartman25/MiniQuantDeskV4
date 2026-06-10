# =============================================================================
# DISCORD-PAPER-READINESS-ALERTS-01
# Send-PaperReadinessDiscordAlert.ps1
#
# Optional, explicit-send Discord alert workflow for offline
# paper-readiness / strategy-fit artifact JSON results.
#
# This script reads ONE artifact JSON file produced by the offline research
# pipeline (paper_readiness_runner.py / backtest_gates.py), classifies it,
# and -- in normal mode only -- sends ONE sanitized Discord summary directly
# to $env:DISCORD_WEBHOOK_URL.
#
# Calls only:
#   POST $env:DISCORD_WEBHOOK_URL   (normal mode only; one sanitized summary;
#                                     direct webhook call, NOT via the daemon)
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperReadinessDiscordAlert.ps1 -ArtifactPath <path> -CheckOnly
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Send-PaperReadinessDiscordAlert.ps1 -ArtifactPath <path> [-Title <text>]
#
# Parameters:
#   -ArtifactPath  (required) Path to a paper-readiness-v1 or strategy-fit-v1 JSON artifact.
#   -CheckOnly     Verify, classify, and report only. Does NOT send a Discord alert.
#   -Title         Optional short title prefixed to the Discord message content.
#
# Supported artifact schemas:
#   - paper-readiness-v1 (research-py paper_readiness_runner.py)
#   - strategy-fit-v1    (research-py backtest_gates.py / backtest_runner.py)
#
# Hard rules enforced by this script:
#   - Operator-triggered only. Never invoked automatically by any pipeline.
#   - Does NOT start the daemon trading runtime, arm paper trading, submit
#     orders, call broker/Alpaca endpoints, touch OMS/runtime/strategy/
#     watchlist behavior, or write to the database.
#   - Sends DIRECTLY to $env:DISCORD_WEBHOOK_URL -- the daemon is never
#     contacted by this script.
#   - Never prints the value of $env:DISCORD_WEBHOOK_URL.
#   - Sends the artifact FILE NAME only -- never the full local path.
#   - Sends exactly one sanitized summary; never sends raw artifact JSON.
#   - Refuses to send (fail-closed, exits non-zero, sends nothing) if:
#       * the artifact file does not exist or is not valid JSON,
#       * schema_version is not paper-readiness-v1 or strategy-fit-v1,
#       * the artifact contains recommended_for_live=true,
#         approved_for_live=true, or eligible_for_live=true,
#       * the artifact contains a Discord webhook URL,
#       * the artifact contains an obvious secret/token,
#       * $env:DISCORD_WEBHOOK_URL is not configured.
#   - reasons / failure_reasons lists are capped to the first 8 entries.
# =============================================================================

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ArtifactPath,

    [switch]$CheckOnly,

    [string]$Title
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
function Write-Step  { param([string]$M) Write-Host "[READINESS-ALERT] $M" -ForegroundColor Cyan }
function Write-Ok    { param([string]$M) Write-Host "[READINESS-ALERT] OK: $M" -ForegroundColor Green }
function Write-Warn  { param([string]$M) Write-Host "[READINESS-ALERT] WARN: $M" -ForegroundColor Yellow }
function Write-Fail  { param([string]$M) Write-Host "[READINESS-ALERT] FAIL: $M" -ForegroundColor Red }
function Write-Field { param([string]$K, $V, [string]$Color = 'White') Write-Host ("  {0,-32} {1}" -f "${K}:", $V) -ForegroundColor $Color }

# ---------------------------------------------------------------------------
# Constants
# ---------------------------------------------------------------------------
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$MaxReasonItems = 8

# Strict "real-looking" webhook pattern -- used by the script-guard / docs
# checks so placeholder URLs (e.g. .../webhooks/REPLACE_ME/alerts-daemon)
# remain allowed in tracked files.
$RealLookingDiscordWebhookPattern = 'https://(?:canary\.|ptb\.)?discord(?:app)?\.com/api/webhooks/\d{17,20}/[A-Za-z0-9_\-]{40,}'

# Loose pattern -- ANY discord webhook URL form (placeholder or real) inside
# an artifact is refused. Artifacts should never contain webhook URLs at all.
$AnyDiscordWebhookUrlPattern = '(?i)discord(?:app)?\.com/api/webhooks/'

# Forged "live-ready" flags anywhere in the artifact JSON text.
$ForgedLiveFlagPattern = '"(recommended_for_live|approved_for_live|eligible_for_live)"\s*:\s*true'

# Obvious embedded secrets / tokens.
$EnvVarSecretAssignmentPattern = '(?i)(ALPACA_API_SECRET_PAPER|ALPACA_API_KEY_PAPER|ALPACA_API_SECRET_LIVE|ALPACA_API_KEY_LIVE|ALPACA_API_SECRET|ALPACA_API_KEY|MQK_OPERATOR_TOKEN|DISCORD_WEBHOOK_URL|DISCORD_BOT_TOKEN)["\s]*[:=]\s*["'']?[A-Za-z0-9/_\-\.]{8,}'
$BearerTokenPattern = '(?i)Bearer\s+[A-Za-z0-9\-\._~\+/]{8,}'

Write-Host ''
Write-Host '=== Send-PaperReadinessDiscordAlert.ps1 (DISCORD-PAPER-READINESS-ALERTS-01) ===' -ForegroundColor Cyan
Write-Host "    Artifact : $ArtifactPath"
Write-Host "    Mode     : $(if ($CheckOnly) { 'CHECK-ONLY -- no Discord alert will be sent' } else { 'NORMAL -- sends one sanitized Discord summary alert' })"
Write-Host ''

Write-Step 'Safety confirmation'
Write-Host '  - This script does NOT start the daemon trading runtime.'
Write-Host '  - This script does NOT arm paper trading or submit/cancel/replace any order.'
Write-Host '  - This script does NOT call any broker or market-data endpoint.'
Write-Host '  - This script does NOT write to the database.'
Write-Host '  - This script sends directly to the configured Discord webhook (normal mode only) -- the daemon is never contacted.'
Write-Host '  - The Discord summary never includes the artifact local path, raw JSON, or any secret value.'

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
# Artifact existence + JSON parse (fail closed in both modes)
# ---------------------------------------------------------------------------
Write-Host ''
Write-Step 'Loading artifact'

if (-not (Test-Path $ArtifactPath -PathType Leaf)) {
    Write-Fail "Artifact not found: $ArtifactPath. Refusing to proceed (fail-closed)."
    exit 1
}

$ArtifactFileName = Split-Path -Leaf $ArtifactPath
$ArtifactRawText  = Get-Content -Path $ArtifactPath -Raw

try {
    $Artifact = $ArtifactRawText | ConvertFrom-Json -ErrorAction Stop
} catch {
    Write-Fail "Artifact is not valid JSON: $ArtifactFileName. Refusing to proceed (fail-closed)."
    exit 1
}

Write-Ok "Artifact parsed: $ArtifactFileName"

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

# ---------------------------------------------------------------------------
# Classification
# ---------------------------------------------------------------------------
$SchemaVersion = Get-JsonValue $Artifact 'schema_version' ''

$ArtifactType = 'unknown'
if ($SchemaVersion -eq 'paper-readiness-v1') {
    $ArtifactType = 'paper-readiness'
} elseif ($SchemaVersion -eq 'strategy-fit-v1') {
    $ArtifactType = 'strategy-fit'
}

# ---------------------------------------------------------------------------
# Refusal checks -- evaluated against the raw artifact text for ALL types,
# including 'unknown'. Order matters only for the reported reason; every
# check below is independently fail-closed.
# ---------------------------------------------------------------------------
$RefusalReason = $null

if ($ArtifactRawText -match $ForgedLiveFlagPattern) {
    $RefusalReason = 'artifact contains a forged live-readiness flag (recommended_for_live / approved_for_live / eligible_for_live = true)'
} elseif ($ArtifactRawText -match $AnyDiscordWebhookUrlPattern) {
    $RefusalReason = 'artifact appears to contain a Discord webhook URL'
} elseif (($ArtifactRawText -match $EnvVarSecretAssignmentPattern) -or ($ArtifactRawText -match $BearerTokenPattern)) {
    $RefusalReason = 'artifact appears to contain an embedded secret or token'
} elseif ($ArtifactType -eq 'unknown') {
    $shownVersion = if ([string]::IsNullOrWhiteSpace($SchemaVersion)) { '(missing)' } else { $SchemaVersion }
    $RefusalReason = "unsupported or missing schema_version ('$shownVersion'); supported: paper-readiness-v1, strategy-fit-v1"
}

# ---------------------------------------------------------------------------
# Per-type field extraction + category derivation
# ---------------------------------------------------------------------------
$Category = 'unknown'
$Status   = Get-JsonValue $Artifact 'status' ''
$Notes    = Get-JsonValue $Artifact 'notes' $null

$PayloadFields = [ordered]@{}

if ($ArtifactType -eq 'paper-readiness') {
    $Reasons             = @(Get-JsonValue $Artifact 'reasons' @())
    $TopSymbol           = Get-JsonValue $Artifact 'top_symbol' $null
    $SymbolInputsStatus  = Get-JsonValue $Artifact 'symbol_inputs_status' $null
    $RiskSimPassed       = Get-JsonValue $Artifact 'risk_simulation_passed' $null
    $PremarketRevalPassed = Get-JsonValue $Artifact 'premarket_revalidation_passed' $null
    $PromotionPassed     = Get-JsonValue $Artifact 'promotion_passed' $null
    $ApprovedForAutoPaper = Get-JsonValue $Artifact 'approved_for_autonomous_paper' $false
    $PaperHandoffRequested = Get-JsonValue $Artifact 'paper_handoff_requested' $false

    switch ($Status) {
        'ready_for_operator_review' { $Category = 'paper_readiness_ready_for_operator_review' }
        'ready_for_paper_handoff'   { $Category = 'paper_readiness_ready_for_paper_handoff' }
        default                     { $Category = 'paper_readiness_blocked' }
    }

    $ReasonsLimited = Get-LimitedList -Items $Reasons -MaxItems $MaxReasonItems

    $PayloadFields['top_symbol'] = $TopSymbol
    $PayloadFields['symbol_inputs_status'] = $SymbolInputsStatus
    $PayloadFields['risk_simulation_passed'] = $RiskSimPassed
    $PayloadFields['premarket_revalidation_passed'] = $PremarketRevalPassed
    $PayloadFields['promotion_passed'] = $PromotionPassed
    $PayloadFields['approved_for_autonomous_paper'] = $ApprovedForAutoPaper
    $PayloadFields['paper_handoff_requested'] = $PaperHandoffRequested
    $PayloadFields['reasons'] = $ReasonsLimited.Items
    $PayloadFields['reasons_truncated'] = $ReasonsLimited.Truncated
    $PayloadFields['reasons_total_count'] = $ReasonsLimited.TotalCount

} elseif ($ArtifactType -eq 'strategy-fit') {
    $FailureReasons     = @(Get-JsonValue $Artifact 'failure_reasons' @())
    $Symbol             = Get-JsonValue $Artifact 'symbol' $null
    $StrategyId         = Get-JsonValue $Artifact 'strategy_id' $null
    $Timeframe          = Get-JsonValue $Artifact 'timeframe' $null
    $RecommendedForPaper = Get-JsonValue $Artifact 'recommended_for_paper' $false
    $Sharpe             = Get-JsonValue $Artifact 'sharpe' $null
    $ProfitFactor       = Get-JsonValue $Artifact 'profit_factor' $null
    $ExpectancyBps      = Get-JsonValue $Artifact 'expectancy_bps' $null
    $Trades             = Get-JsonValue $Artifact 'trades' $null

    switch ($Status) {
        'blocked_no_backtest_interface' { $Category = 'strategy_fit_generation_failed' }
        'complete' {
            if ($RecommendedForPaper -eq $true) {
                $Category = 'strategy_fit_recommended_for_paper'
            } else {
                $Category = 'strategy_fit_rejected_by_gates'
            }
        }
        default { $Category = 'strategy_fit_unknown_status' }
    }

    $FailureReasonsLimited = Get-LimitedList -Items $FailureReasons -MaxItems $MaxReasonItems

    $PayloadFields['symbol'] = $Symbol
    $PayloadFields['strategy_id'] = $StrategyId
    $PayloadFields['timeframe'] = $Timeframe
    $PayloadFields['recommended_for_paper'] = $RecommendedForPaper
    $PayloadFields['sharpe'] = $Sharpe
    $PayloadFields['profit_factor'] = $ProfitFactor
    $PayloadFields['expectancy_bps'] = $ExpectancyBps
    $PayloadFields['trades'] = $Trades
    $PayloadFields['failure_reasons'] = $FailureReasonsLimited.Items
    $PayloadFields['failure_reasons_truncated'] = $FailureReasonsLimited.Truncated
    $PayloadFields['failure_reasons_total_count'] = $FailureReasonsLimited.TotalCount
}

$CategoryLabels = @{
    'paper_readiness_ready_for_operator_review' = 'PAPER READINESS: READY FOR OPERATOR REVIEW'
    'paper_readiness_ready_for_paper_handoff'   = 'PAPER READINESS: READY FOR PAPER HANDOFF (artifact-only)'
    'paper_readiness_blocked'                   = 'PAPER READINESS: BLOCKED'
    'strategy_fit_recommended_for_paper'        = 'STRATEGY FIT: RECOMMENDED FOR PAPER'
    'strategy_fit_rejected_by_gates'            = 'STRATEGY FIT: REJECTED BY GATES'
    'strategy_fit_generation_failed'            = 'STRATEGY FIT: ARTIFACT GENERATION FAILED'
    'strategy_fit_unknown_status'               = 'STRATEGY FIT: UNKNOWN STATUS'
    'unknown'                                   = 'UNKNOWN ARTIFACT'
}
$CategoryLabel = $CategoryLabels[$Category]
if ($null -eq $CategoryLabel) { $CategoryLabel = $Category }

# ---------------------------------------------------------------------------
# Sanitized summary (printed in both modes)
# ---------------------------------------------------------------------------
Write-Host ''
Write-Step 'Sanitized artifact summary (no secret values, local path not shown)'
Write-Field 'artifact_filename' $ArtifactFileName
Write-Field 'schema_version'    $(if ([string]::IsNullOrWhiteSpace($SchemaVersion)) { '(missing)' } else { $SchemaVersion })
Write-Field 'artifact_type'     $ArtifactType
Write-Field 'status'            $(Format-NullableValue $Status)
Write-Field 'category'          $Category
Write-Field 'category_label'    $CategoryLabel

foreach ($key in $PayloadFields.Keys) {
    $val = $PayloadFields[$key]
    if ($val -is [array]) {
        Write-Field $key ('[' + (($val | ForEach-Object { Format-NullableValue $_ }) -join '; ') + ']')
    } else {
        Write-Field $key (Format-NullableValue $val)
    }
}

if (-not [string]::IsNullOrWhiteSpace($Notes)) {
    $notesPreview = $Notes
    if ($notesPreview.Length -gt 200) { $notesPreview = $notesPreview.Substring(0, 200) + '...' }
    Write-Field 'notes_preview' $notesPreview
}

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
$contentLines.Add("[mqk-research] $CategoryLabel")
$contentLines.Add("artifact: $ArtifactFileName")
$contentLines.Add("status: $(Format-NullableValue $Status)")

switch ($ArtifactType) {
    'paper-readiness' {
        $contentLines.Add("top_symbol: $(Format-NullableValue $PayloadFields['top_symbol'])")
        if ($PayloadFields['reasons_total_count'] -gt 0) {
            $reasonsText = ($PayloadFields['reasons'] -join '; ')
            if ($PayloadFields['reasons_truncated']) {
                $reasonsText += " (+$($PayloadFields['reasons_total_count'] - $MaxReasonItems) more)"
            }
            $contentLines.Add("reasons: $reasonsText")
        }
    }
    'strategy-fit' {
        $contentLines.Add("symbol: $(Format-NullableValue $PayloadFields['symbol']) | strategy: $(Format-NullableValue $PayloadFields['strategy_id']) | timeframe: $(Format-NullableValue $PayloadFields['timeframe'])")
        if ($PayloadFields['failure_reasons_total_count'] -gt 0) {
            $failText = ($PayloadFields['failure_reasons'] -join '; ')
            if ($PayloadFields['failure_reasons_truncated']) {
                $failText += " (+$($PayloadFields['failure_reasons_total_count'] - $MaxReasonItems) more)"
            }
            $contentLines.Add("failure_reasons: $failText")
        }
    }
}

$Content = $contentLines -join "`n"

$PayloadObject = [ordered]@{
    content                 = $Content
    alert_type              = 'paper_readiness_artifact'
    artifact_type           = $ArtifactType
    schema_version          = $SchemaVersion
    artifact_filename       = $ArtifactFileName
    category                = $Category
    status                  = $Status
    alert_generated_at_utc  = $AlertGeneratedAtUtc
}
if (-not [string]::IsNullOrWhiteSpace($Title)) {
    $PayloadObject['title'] = $Title
}
foreach ($key in $PayloadFields.Keys) {
    $PayloadObject[$key] = $PayloadFields[$key]
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
Write-Field 'category'    $Category
Write-Field 'artifact_filename' $ArtifactFileName

if ($statusCode -ge 200 -and $statusCode -lt 300) {
    Write-Ok 'Discord delivery succeeded. Check the configured Discord channel for the summary message.'
} else {
    Write-Warn "Discord webhook returned non-success status $statusCode."
    exit 1
}

Write-Host ''
exit 0
