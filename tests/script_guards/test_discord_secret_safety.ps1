# =============================================================================
# Script guard: test_discord_secret_safety.ps1
# DISCORD-SECRET-SAFETY-AUDIT-FIX-01
#
# Static assertions for Discord webhook secret safety.
# No daemon, no DB, no live calls, no .env.local reads, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent (Split-Path -Parent $ScriptDir)
$NotifyPath = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\notify.rs'
$SecretSafetyTestPath = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\tests\scenario_discord_secret_safety_01.rs'
$Non2xxTestPath = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\tests\scenario_discord_non2xx_delivery_01.rs'

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
Write-Host '--- test_discord_secret_safety.ps1 ---'

Assert-True (Test-Path $NotifyPath) "notify.rs exists"
Assert-True (Test-Path $SecretSafetyTestPath) "focused Discord secret-safety test exists"
Assert-True (Test-Path $Non2xxTestPath) "focused Discord non-2xx delivery classification test exists"

$NotifyContent = Read-TextFile $NotifyPath
$SecretSafetyTestContent = Read-TextFile $SecretSafetyTestPath
$Non2xxTestContent = Read-TextFile $Non2xxTestPath

Assert-True ($NotifyContent -match 'DiscordDeliveryErrorSummary') 'notify.rs contains a URL-free delivery error summary type'
Assert-True ($NotifyContent -match 'pub fn discord_delivery_error_summary') 'notify.rs contains a delivery error sanitizer helper'
Assert-True ($NotifyContent -match 'error_kind\s*=\s*summary\.kind') 'Discord failure logs use sanitized error_kind'
Assert-True ($NotifyContent -match 'is_timeout\s*=\s*summary\.is_timeout') 'Discord failure logs expose sanitized timeout category'
Assert-True ($NotifyContent -match 'is_connect\s*=\s*summary\.is_connect') 'Discord failure logs expose sanitized connect category'

# DISCORD-NON2XX-DELIVERY-CLASSIFICATION-01: non-2xx HTTP responses must be
# classified via a status-code-only summary, never via raw response/body/url.
Assert-True ($NotifyContent -match 'pub fn discord_status_class') 'notify.rs contains a sanitized HTTP status-class helper'
Assert-True ($NotifyContent -match 'DiscordDeliveryStatusSummary') 'notify.rs contains a URL-free non-2xx status summary type'
Assert-True ($NotifyContent -match '\.\s*status\s*\(\s*\)\s*\.\s*is_success\s*\(\s*\)') 'notify.rs checks response status for success (2xx) before treating delivery as successful'
Assert-True ($NotifyContent -match 'status_code\s*=\s*summary\.status_code') 'Discord non-2xx failure logs use sanitized status_code'
Assert-True ($NotifyContent -match 'status_class\s*=\s*summary\.status_class') 'Discord non-2xx failure logs use sanitized status_class'

Assert-False ($NotifyContent -match '%\s*err') 'notify.rs does not format raw reqwest errors with %err'
Assert-False ($NotifyContent -match '\berror\s*=\s*%\s*err') 'notify.rs does not log error = %err'
Assert-False ($NotifyContent -match 'warn!\(\s*%\s*err') 'notify.rs does not call warn!(%err...)'
Assert-False ($NotifyContent -match 'error!\(\s*%\s*err') 'notify.rs does not call error!(%err...)'
Assert-False ($NotifyContent -match 'tracing::warn!\(\s*%\s*err') 'notify.rs does not call tracing::warn!(%err...)'
Assert-False ($NotifyContent -match 'tracing::error!\(\s*%\s*err') 'notify.rs does not call tracing::error!(%err...)'
Assert-False ($NotifyContent -match '\berr\s*\.\s*to_string\s*\(') 'notify.rs does not stringify raw reqwest errors'
Assert-False ($NotifyContent -match '\berr\s*\.\s*url\s*\(') 'notify.rs does not read request URLs from reqwest errors'
Assert-False ($NotifyContent -match '\berr\s*\.\s*url_mut\s*\(') 'notify.rs does not mutate/read request URLs from reqwest errors'

# Discord response body/debug output must never be read or logged.
Assert-False ($NotifyContent -match '\.\s*text\s*\(\s*\)') 'notify.rs does not read the Discord response body via .text()'
Assert-False ($NotifyContent -match '\?\s*resp\b') 'notify.rs does not log the Discord response via tracing debug shorthand (?resp)'
Assert-False ($NotifyContent -match '\{\s*resp\s*:\?\s*\}') 'notify.rs does not format the Discord response with {:?}'
Assert-False ($NotifyContent -match '\{\s*req\s*:\?\s*\}') 'notify.rs does not format the Discord request with {:?}'

$ChangedSourceContents = @($NotifyContent, $SecretSafetyTestContent, $Non2xxTestContent)
foreach ($content in $ChangedSourceContents) {
    Assert-False ($content -match '\.env\.local') 'changed Discord source/tests do not read or reference .env.local'
    Assert-False ($content -match '(?i)\b(eligible_for_live|recommended_for_live|approved_for_live)\b\s*[:=]\s*true') `
        'changed Discord source/tests do not set live-readiness flags true'
}

Assert-True ($Non2xxTestContent -match '(?i)fake[-_]?webhook') 'non-2xx delivery test webhook fixtures are clearly fake/test-only'

$ScriptFiles = @(& git -C $RepoRoot ls-files -- '*.ps1' '*.psm1' '*.sh' '*.cmd' '*.bat')
$WebhookPrintHits = [System.Collections.Generic.List[string]]::new()
$WebhookPrintPattern = '(?im)^\s*(Write-Host|Write-Output|Write-Warning|Write-Verbose|Write-Information|echo|printf)\b[^\r\n]*(\$env:DISCORD_WEBHOOK_URL|\$\{?DISCORD_WEBHOOK_URL\}?)'

foreach ($rel in $ScriptFiles) {
    $path = Join-Path $RepoRoot $rel
    $content = Read-TextFile $path
    if ($content -match $WebhookPrintPattern) {
        $WebhookPrintHits.Add($rel)
    }
}

Assert-True ($WebhookPrintHits.Count -eq 0) 'scripts do not print DISCORD_WEBHOOK_URL environment values'

$TrackedTextFiles = @(& git -C $RepoRoot ls-files -- '*.rs' '*.ps1' '*.psm1' '*.sh' '*.cmd' '*.bat' '*.py' '*.md' '*.txt' '*.toml' '*.json' '*.yaml' '*.yml' '*.ts' '*.tsx' '*.js' '*.jsx')
$RealWebhookHits = [System.Collections.Generic.List[string]]::new()
$RealLookingDiscordWebhookPattern = 'https://(?:canary\.|ptb\.)?discord(?:app)?\.com/api/webhooks/\d{17,20}/[A-Za-z0-9_\-]{40,}'

foreach ($rel in $TrackedTextFiles) {
    $path = Join-Path $RepoRoot $rel
    if (-not (Test-Path $path)) {
        continue
    }

    $lines = @(Get-Content -Path $path -ErrorAction SilentlyContinue)
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match $RealLookingDiscordWebhookPattern) {
            $RealWebhookHits.Add("${rel}:$($i + 1)")
        }
    }
}

Assert-True ($RealWebhookHits.Count -eq 0) 'tracked text files contain no real-looking Discord webhook URLs'

if ($Failures -eq 0) {
    Write-Host '  ALL ASSERTIONS PASSED (test_discord_secret_safety)' -ForegroundColor Green
    exit 0
}

Write-Host "  $Failures ASSERTION(S) FAILED (test_discord_secret_safety)" -ForegroundColor Red
exit 1
