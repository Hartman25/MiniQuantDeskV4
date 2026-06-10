# =============================================================================
# DISCORD-TEST-ALERT-OFFLINE-01
# Test-DiscordAlert.ps1
#
# Safe operator workflow to verify Discord alert delivery configuration
# WITHOUT starting the daemon trading runtime, arming paper trading,
# submitting orders, or touching broker/Alpaca endpoints.
#
# Calls only:
#   GET  /v1/health                                            (reachability check, no auth)
#   POST /api/v1/ops/action  {"action_key":"test-discord-alert"}  (normal mode only; Bearer token required)
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Test-DiscordAlert.ps1 -CheckOnly
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Test-DiscordAlert.ps1
#
# Parameters:
#   -DaemonPort   Daemon HTTP port. Default: 8899.
#   -CheckOnly    Verify configuration only. Does NOT send a Discord alert.
#
# Hard rules enforced by this script:
#   - Calls only GET /v1/health and POST /api/v1/ops/action {"action_key":"test-discord-alert"}.
#   - Never arms paper trading, never starts the runtime, never submits orders,
#     never calls broker/Alpaca endpoints, never writes to the DB directly.
#   - -CheckOnly never sends a request to /api/v1/ops/action.
#   - Never prints the value of DISCORD_WEBHOOK_URL or MQK_OPERATOR_TOKEN --
#     only whether each appears configured (presence check only).
#   - Fails closed (normal mode only) if the daemon is unreachable or
#     MQK_OPERATOR_TOKEN is not configured.
# =============================================================================

[CmdletBinding()]
param(
    [int]   $DaemonPort = 8899,
    [switch]$CheckOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
function Write-Step  { param([string]$M) Write-Host "[DISCORD-TEST] $M" -ForegroundColor Cyan }
function Write-Ok    { param([string]$M) Write-Host "[DISCORD-TEST] OK: $M" -ForegroundColor Green }
function Write-Warn  { param([string]$M) Write-Host "[DISCORD-TEST] WARN: $M" -ForegroundColor Yellow }
function Write-Fail  { param([string]$M) Write-Host "[DISCORD-TEST] FAIL: $M" -ForegroundColor Red }
function Write-Field { param([string]$K, $V, [string]$Color = 'White') Write-Host ("  {0,-32} {1}" -f "${K}:", $V) -ForegroundColor $Color }

# ---------------------------------------------------------------------------
# Constants -- the ONE route this script is allowed to call for delivery.
# ---------------------------------------------------------------------------
$RepoRoot      = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$DaemonBaseUrl = "http://127.0.0.1:$DaemonPort"
$RoutePath     = '/api/v1/ops/action'
$ActionKey     = 'test-discord-alert'
$RequestBody   = '{"action_key":"test-discord-alert"}'

Write-Host ''
Write-Host '=== Test-DiscordAlert.ps1 (DISCORD-TEST-ALERT-OFFLINE-01) ===' -ForegroundColor Cyan
Write-Host "    Daemon : $DaemonBaseUrl"
Write-Host "    Route  : POST $RoutePath  $RequestBody"
Write-Host "    Mode   : $(if ($CheckOnly) { 'CHECK-ONLY -- no Discord alert will be sent' } else { 'NORMAL -- sends one [TEST] Discord alert via the daemon' })"
Write-Host ''

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

# ---------------------------------------------------------------------------
# Daemon reachability (GET /v1/health -- public, no auth, no mutation)
# ---------------------------------------------------------------------------
$daemonOnline = $false
try {
    $health = Invoke-RestMethod -Uri "$DaemonBaseUrl/v1/health" -Method Get -TimeoutSec 5 -ErrorAction Stop
    if ($health.ok -eq $true) { $daemonOnline = $true }
} catch {
    $daemonOnline = $false
}

# ---------------------------------------------------------------------------
# Configuration presence checks (presence only -- values never printed)
# ---------------------------------------------------------------------------
$webhookConfigured = -not [string]::IsNullOrWhiteSpace($env:DISCORD_WEBHOOK_URL)
$tokenConfigured   = -not [string]::IsNullOrWhiteSpace($env:MQK_OPERATOR_TOKEN)

Write-Host ''
Write-Step 'Configuration summary (no secret values shown)'
Write-Field 'daemon_reachable'           $daemonOnline            $(if ($daemonOnline) { 'Green' } else { 'Yellow' })
Write-Field 'route_to_call'              "POST $RoutePath"
Write-Field 'action_key'                 $ActionKey
Write-Field 'route_requires_auth'        'true (Authorization: Bearer $MQK_OPERATOR_TOKEN)'
Write-Field 'discord_webhook_configured' $webhookConfigured       $(if ($webhookConfigured) { 'Green' } else { 'Yellow' })
Write-Field 'operator_token_configured'  $tokenConfigured         $(if ($tokenConfigured) { 'Green' } else { 'Yellow' })

Write-Host ''
Write-Step 'Safety confirmation'
Write-Host '  - This script does NOT start the daemon trading runtime.'
Write-Host '  - This script does NOT arm paper trading.'
Write-Host '  - This script does NOT submit, cancel, or replace any order.'
Write-Host '  - This script does NOT call any broker or market-data endpoint.'
Write-Host '  - This script does NOT write to the database.'
Write-Host '  - The only mutating call (normal mode) is the route shown above,'
Write-Host '    which the daemon documents as not mutating trading/arm/integrity state.'

# ---------------------------------------------------------------------------
# CheckOnly: report and exit. No alert sent.
# ---------------------------------------------------------------------------
if ($CheckOnly) {
    Write-Host ''
    Write-Step 'CheckOnly mode -- no Discord alert sent, no POST issued.'
    if (-not $daemonOnline) {
        Write-Warn "Daemon is unreachable at $DaemonBaseUrl. Start the daemon before running the normal send."
    }
    if (-not $tokenConfigured) {
        Write-Warn 'MQK_OPERATOR_TOKEN is not configured. The normal send will fail closed without it.'
    }
    if (-not $webhookConfigured) {
        Write-Warn 'DISCORD_WEBHOOK_URL is not configured. The normal send will return disposition=noop_unconfigured (still accepted=true, no delivery attempted).'
    }
    Write-Host ''
    exit 0
}

# ---------------------------------------------------------------------------
# Normal mode: fail closed before attempting delivery.
# ---------------------------------------------------------------------------
Write-Host ''
Write-Step 'Normal mode -- sending one [TEST] Discord alert via the daemon route.'

if (-not $daemonOnline) {
    Write-Fail "Daemon unreachable at $DaemonBaseUrl/v1/health. Refusing to proceed (fail-closed)."
    exit 1
}

if (-not $tokenConfigured) {
    Write-Fail 'MQK_OPERATOR_TOKEN is not configured. Add it to .env.local. Refusing to proceed (fail-closed).'
    exit 1
}

try {
    $resp = Invoke-RestMethod -Uri "$DaemonBaseUrl$RoutePath" -Method Post `
        -Headers @{ Authorization = "Bearer $env:MQK_OPERATOR_TOKEN" } `
        -ContentType 'application/json' -Body $RequestBody `
        -TimeoutSec 10 -ErrorAction Stop
} catch {
    $statusCode = $null
    if ($_.Exception.PSObject.Properties['Response'] -and $_.Exception.Response) {
        try { $statusCode = [int]$_.Exception.Response.StatusCode } catch {}
    }
    if ($null -ne $statusCode) {
        Write-Fail "Request to $RoutePath failed with HTTP status $statusCode. Refusing further action (fail-closed)."
    } else {
        Write-Fail "Request to $RoutePath failed (network/transport error). Refusing further action (fail-closed)."
    }
    exit 1
}

Write-Host ''
Write-Step 'Result (sanitized -- no webhook URL or token included)'
Write-Field 'requested_action' $resp.requested_action
Write-Field 'accepted'         $resp.accepted    $(if ($resp.accepted -eq $true) { 'Green' } else { 'Red' })
Write-Field 'disposition'      $resp.disposition
Write-Field 'environment'      $resp.environment

if ($resp.PSObject.Properties['warnings'] -and $resp.warnings -and @($resp.warnings).Count -gt 0) {
    foreach ($w in @($resp.warnings)) {
        Write-Warn $w
    }
}

switch ($resp.disposition) {
    'delivery_attempted' {
        Write-Ok 'Discord delivery was attempted. Check the configured Discord channel for a [TEST] message.'
    }
    'noop_unconfigured' {
        Write-Warn 'Discord notifier is not configured (DISCORD_WEBHOOK_URL absent or empty). No alert was sent.'
    }
    'delivery_failed' {
        Write-Warn 'Discord delivery failed (best-effort). Check daemon logs for sanitized delivery diagnostics.'
    }
    default {
        Write-Warn "Unexpected disposition '$($resp.disposition)'."
    }
}

Write-Host ''
exit 0
