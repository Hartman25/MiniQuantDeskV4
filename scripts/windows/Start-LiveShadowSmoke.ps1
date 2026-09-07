# =============================================================================
# Start-LiveShadowSmoke.ps1
# LIVE-TINY-CAPITAL-SMOKE-01 (engineering portion only -- see note below)
#
# Reusable LiveShadow evidence/orchestration wrapper. Targets LiveShadow
# ONLY -- never LiveCapital -- and, as of this script's authoring, never
# submits an order or mutates a live account, because the canonical
# launcher it delegates to (Start-MiniQuantDesk.ps1) documents its own
# `-Mode Live` path as read-only/report-only: "this patch NEVER starts a
# live daemon process, submits an order, or mutates a live account. Live
# mode only runs read-only / source-guard preflight checks." This script
# does not reimplement any broker/order/arm/halt logic of its own -- it
# only wraps that existing, already-safe canonical entrypoint with
# LiveShadow-specific framing and deterministic evidence capture.
#
# CONTROLLER NOTE (MQK-LEDGER-BURN-CONTROLLER-02 W2-E): the original ledger
# row asks for two separable things -- (1) build the reusable smoke/evidence
# orchestration, (2) perform a real operational smoke. This script closes
# ONLY (1). It is never invoked for a real smoke by this controller; the
# remaining real-operational-smoke requirement is tracked separately under
# DEFERRED_OPERATOR_VALIDATION. This script cannot enable LiveCapital and
# cannot, today, cause any real daemon start, broker call, or order --
# `Start-MiniQuantDesk.ps1 -Mode Live` (with or without -CheckOnly) is
# entirely read-only until a future patch adds real live-shadow daemon-
# start capability to that canonical launcher; this script will delegate to
# that capability once it exists rather than growing its own copy.
#
# Usage:
#   Start-LiveShadowSmoke.ps1                              (same as -CheckOnly)
#   Start-LiveShadowSmoke.ps1 -CheckOnly
#   Start-LiveShadowSmoke.ps1 -IAcknowledgeLiveShadowOnly   (full run -- still
#                                                             read-only today,
#                                                             see note above)
#
# Parameters:
#   -RepoRoot                   Repo root. Default: two levels up from this script.
#   -CheckOnly                  Read-only preflight only. Default when no other
#                                run switch is passed. Zero network/broker/order
#                                effects beyond the canonical launcher's own
#                                already-safe -Mode Live -CheckOnly reporting.
#   -IAcknowledgeLiveShadowOnly Explicit, named acknowledgement required to run
#                                the "full" (non-CheckOnly) path. Named for what
#                                it actually does today (LiveShadow evidence
#                                capture over a still-read-only report), not a
#                                generic "-Force"/"-Yes" flag, so an operator
#                                cannot pass it by habit without reading it.
#
# Exit codes: passthrough of Start-MiniQuantDesk.ps1's own exit code
#   (0=ready, 1=generic failure, 2=safety refusal, 5=LIVE blocked, ...).
#
# Hard rules enforced by this script:
#   - MQK_DAEMON_DEPLOYMENT_MODE is always set to 'live-shadow', never 'live'
#     -- there is exactly one assignment to this variable in the whole file.
#   - Never calls Invoke-WebRequest/Invoke-RestMethod/curl directly -- every
#     network-capable action is delegated to Start-MiniQuantDesk.ps1's own
#     process boundary, never duplicated here.
#   - Never prints ALPACA_API_KEY*, ALPACA_API_SECRET*, MQK_OPERATOR_TOKEN,
#     DB password, Discord webhook, or any other secret value.
#   - Evidence layout is deterministic: exports\live_shadow_smoke\evidence_<UTC
#     timestamp>\ containing readiness_report.log and manifest.json.
# =============================================================================

[CmdletBinding()]
param(
    [string]$RepoRoot = '',
    [switch]$CheckOnly,
    [switch]$IAcknowledgeLiveShadowOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Step    { param([string]$M) Write-Host "[LiveShadowSmoke] $M" -ForegroundColor Cyan }
function Write-Ok      { param([string]$M) Write-Host "[LiveShadowSmoke] OK: $M" -ForegroundColor Green }
function Write-Warn    { param([string]$M) Write-Host "[LiveShadowSmoke] WARN: $M" -ForegroundColor Yellow }
function Write-Fail    { param([string]$M) Write-Host "[LiveShadowSmoke] FAIL: $M" -ForegroundColor Red }
function Write-Section { param([string]$M) Write-Host ''; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Secret guard: never print these names' values (same convention as
# Start-PaperTradingSmoke.ps1's $SECRET_NAMES).
# ---------------------------------------------------------------------------
$SECRET_NAMES = @(
    'ALPACA_API_KEY_PAPER', 'ALPACA_API_SECRET_PAPER',
    'ALPACA_API_KEY_LIVE',  'ALPACA_API_SECRET_LIVE',
    'MQK_OPERATOR_TOKEN',   'DISCORD_WEBHOOK_URL',
    'POSTGRES_PASSWORD',    'DATABASE_URL', 'MQK_DATABASE_URL'
)

function Assert-NotSecret {
    param([string]$Name, [string]$Value)
    foreach ($s in $SECRET_NAMES) {
        if ($Name -eq $s -and -not [string]::IsNullOrWhiteSpace($Value)) {
            throw "BUG: script attempted to print secret env var '$Name'. Aborting."
        }
    }
}

# ---------------------------------------------------------------------------
# Resolve repo root
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

$launcherPath = Join-Path $RepoRoot 'scripts\windows\Start-MiniQuantDesk.ps1'
if (-not (Test-Path $launcherPath)) {
    Write-Fail "Canonical launcher not found at $launcherPath. This script only wraps it -- it does not reimplement live startup."
    exit 1
}

# ---------------------------------------------------------------------------
# Effective run mode: CheckOnly unless the full-run path is explicitly
# acknowledged. Passing neither switch defaults to the safe CheckOnly path
# -- this script never silently attempts a "full" run.
# ---------------------------------------------------------------------------
$effectiveCheckOnly = $CheckOnly.IsPresent -or (-not $IAcknowledgeLiveShadowOnly.IsPresent)
if (-not $effectiveCheckOnly) {
    Write-Warn "Full-run path acknowledged (-IAcknowledgeLiveShadowOnly). This still delegates to"
    Write-Warn "Start-MiniQuantDesk.ps1 -Mode Live, which is documented read-only/report-only today --"
    Write-Warn "see this script's header. No daemon is started, no broker is called, no order is submitted."
}

# ---------------------------------------------------------------------------
# Hard guard: force LiveShadow, never LiveCapital. This is the ONE and ONLY
# assignment to MQK_DAEMON_DEPLOYMENT_MODE in this file.
# ---------------------------------------------------------------------------
$env:MQK_DAEMON_DEPLOYMENT_MODE = 'live-shadow'
Write-Ok "MQK_DAEMON_DEPLOYMENT_MODE forced to 'live-shadow' (LiveCapital is never enabled by this script)."

# ---------------------------------------------------------------------------
# Deterministic evidence layout.
# ---------------------------------------------------------------------------
$evStamp = [DateTime]::UtcNow.ToString('yyyyMMdd_HHmmss')
$evDir = Join-Path $RepoRoot "exports\live_shadow_smoke\evidence_$evStamp"
New-Item -ItemType Directory -Force -Path $evDir | Out-Null
$reportLog = Join-Path $evDir 'readiness_report.log'
$manifestPath = Join-Path $evDir 'manifest.json'
Write-Ok "Evidence folder: $evDir"

# ---------------------------------------------------------------------------
# Delegate to the canonical launcher. Never construct HTTP requests here --
# every network-capable action lives entirely inside Start-MiniQuantDesk.ps1's
# own process.
# ---------------------------------------------------------------------------
Write-Section "Delegating to Start-MiniQuantDesk.ps1 -Mode Live$(if ($effectiveCheckOnly) { ' -CheckOnly' } else { '' })"

$launcherArgs = @('-Mode', 'Live')
if ($effectiveCheckOnly) { $launcherArgs += '-CheckOnly' }

$launcherExitCode = 0
try {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $launcherPath @launcherArgs 2>&1 |
        Tee-Object -FilePath $reportLog | Out-Host
    $launcherExitCode = $LASTEXITCODE
} catch {
    "LAUNCHER_INVOCATION_ERROR: $($_.Exception.Message)" | Add-Content -Path $reportLog
    Write-Fail "Failed to invoke canonical launcher: $($_.Exception.Message)"
    $launcherExitCode = 1
}

$manifest = [ordered]@{
    schema_version         = 'live-shadow-smoke-manifest-v1'
    checked_at_utc         = [DateTime]::UtcNow.ToString('o')
    deployment_mode_forced = 'live-shadow'
    check_only             = $effectiveCheckOnly
    canonical_launcher     = 'scripts\windows\Start-MiniQuantDesk.ps1'
    launcher_args          = $launcherArgs
    launcher_exit_code     = $launcherExitCode
    real_daemon_start_performed = $false
    real_broker_call_performed  = $false
    real_order_submitted        = $false
    note = 'Start-MiniQuantDesk.ps1 -Mode Live is documented read-only/report-only in this repo today (never starts a live daemon, never calls a broker, never submits an order), with or without -CheckOnly. This script adds LiveShadow framing and deterministic evidence capture only -- it performs no additional network or broker action of its own.'
}
$manifest | ConvertTo-Json -Depth 5 | Set-Content -Path $manifestPath -Encoding ASCII
Write-Ok "Manifest written: $manifestPath"

if ($launcherExitCode -eq 0) {
    Write-Ok "Canonical launcher reported readiness (exit 0)."
} else {
    Write-Warn "Canonical launcher exited $launcherExitCode -- see $reportLog for the reason. This is expected while LiveCapital readiness remains gated off."
}

exit $launcherExitCode
