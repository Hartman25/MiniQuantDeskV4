# =============================================================================
# OPERATOR-RUNBOOK-STARTUP-HARDENING-01 — Script static-invariant tests
#
# Reads Start-PaperTradingSmoke.ps1 as text and asserts OPR01-OPR08.
# No daemon, no DB, no live calls. Pure text invariant checks.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_start_paper_trading_smoke.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot   = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$TargetScript = Join-Path $RepoRoot 'scripts\windows\Start-PaperTradingSmoke.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== Start-PaperTradingSmoke.ps1 static invariant tests ==="
Write-Host "    Target: $TargetScript"
Write-Host ""

# ---------------------------------------------------------------------------
# OPR01: Script exists at expected path
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    Pass 'OPR01' "Script exists at scripts\windows\Start-PaperTradingSmoke.ps1"
} else {
    Fail 'OPR01' "Script NOT found at: $TargetScript"
}

# Load script text for remaining checks
$scriptText = ''
if (Test-Path $TargetScript) {
    $scriptText = Get-Content -Path $TargetScript -Raw
}

# ---------------------------------------------------------------------------
# OPR02: Script contains required endpoint calls
# ---------------------------------------------------------------------------
$requiredEndpoints = @(
    '/v1/health',
    '/api/v1/system/status',
    '/api/v1/system/preflight',
    '/api/v1/autonomous/readiness',
    '/api/v1/alerts/active',
    '/api/v1/execution/orders',
    '/api/v1/reconcile/status',
    '/api/v1/ops/action',
    '/api/v1/ops/repair/adopt-broker-position-baseline'
)

foreach ($ep in $requiredEndpoints) {
    if ($scriptText -match [regex]::Escape($ep)) {
        Pass 'OPR02' "Contains endpoint: $ep"
    } else {
        Fail 'OPR02' "Missing endpoint: $ep"
    }
}

# ---------------------------------------------------------------------------
# OPR03: Script reasserts MQK_DATABASE_URL after loading .env.local
# ---------------------------------------------------------------------------
# The reassert must appear AFTER the .env.local load block.
# We verify both patterns are present and that the reassert pattern follows the load pattern.
$loadPattern     = [regex]::Escape('.env.local')
$reassertPattern = [regex]::Escape('$env:MQK_DATABASE_URL')

$loadIdx     = $scriptText.IndexOf('.env.local')
$reassertIdx = $scriptText.IndexOf('$env:MQK_DATABASE_URL')

if ($reassertIdx -gt $loadIdx -and $loadIdx -ge 0) {
    Pass 'OPR03' 'MQK_DATABASE_URL reasserted after .env.local load'
} else {
    Fail 'OPR03' "MQK_DATABASE_URL reassert (idx=$reassertIdx) must follow .env.local load (idx=$loadIdx)"
}

# ---------------------------------------------------------------------------
# OPR04: Script refuses / halts on live_routing_enabled=true
# ---------------------------------------------------------------------------
$hasLiveRoutingGuard = (
    $scriptText -match 'live_routing_enabled' -and
    ($scriptText -match "Refusing to continue" -or $scriptText -match 'exit 1') -and
    $scriptText -match 'TRUE-DANGER|live_routing_enabled.*true'
)
if ($hasLiveRoutingGuard) {
    Pass 'OPR04' 'Script guards against live_routing_enabled=true'
} else {
    Fail 'OPR04' 'Script must refuse/halt if live_routing_enabled=true'
}

# ---------------------------------------------------------------------------
# OPR05: Script does not print secret env values
# ---------------------------------------------------------------------------
$secretPatterns = @(
    '\$env:ALPACA_API_KEY_PAPER',
    '\$env:ALPACA_API_SECRET_PAPER',
    '\$env:ALPACA_API_KEY_LIVE',
    '\$env:ALPACA_API_SECRET_LIVE',
    '\$env:MQK_OPERATOR_TOKEN\b',
    '\$env:MQK_DISCORD_WEBHOOK',
    'Write-Host.*ALPACA_API_KEY',
    'Write-Host.*ALPACA_API_SECRET',
    'Write-Host.*MQK_OPERATOR_TOKEN.*\$'
)

$leaked = @()
foreach ($pat in $secretPatterns) {
    if ($scriptText -match $pat) {
        # Allow $env:MQK_OPERATOR_TOKEN in assignment/check context, not in Write-Host
        # More precisely: flag only if the match is on a Write-Host / Out-Host line
        # We do a line-by-line check for the noisy patterns
        foreach ($line in ($scriptText -split "`n")) {
            if ($line -match $pat -and $line -match 'Write-Host|echo|Out-Host') {
                $leaked += "$pat in: $($line.Trim())"
            }
        }
    }
}

if ($leaked.Count -eq 0) {
    Pass 'OPR05' 'No secret env values printed via Write-Host / Out-Host'
} else {
    foreach ($l in $leaked) { Fail 'OPR05' "Potential secret print: $l" }
}

# Also verify the script contains the Assert-NotSecret guard or equivalent redaction comment
if ($scriptText -match 'Assert-NotSecret|SECRET_NAMES|values not printed|value not printed|never print') {
    Pass 'OPR05b' 'Script contains explicit secret-guard pattern'
} else {
    Fail 'OPR05b' 'Script must contain explicit secret-guard pattern (SECRET_NAMES / "not printed")'
}

# ---------------------------------------------------------------------------
# OPR06: Script includes adopt baseline and durable arm verification
# ---------------------------------------------------------------------------
if ($scriptText -match 'adopt-broker-position-baseline' -and $scriptText -match 'ADOPT_BROKER_POSITION_BASELINE') {
    Pass 'OPR06' 'Script calls adopt-broker-position-baseline with confirm string'
} else {
    Fail 'OPR06' 'Script must call POST adopt-broker-position-baseline with confirm=ADOPT_BROKER_POSITION_BASELINE'
}

if ($scriptText -match 'arm-execution' -and ($scriptText -match 'arm_state.*armed|armed.*arm_state|ARMED')) {
    Pass 'OPR06b' 'Script calls arm-execution and verifies ARMED state'
} else {
    Fail 'OPR06b' 'Script must call arm-execution and verify ARMED state'
}

# ---------------------------------------------------------------------------
# OPR07: Script includes watcher fields
# ---------------------------------------------------------------------------
$requiredWatcherFields = @(
    'runtime_status',
    'alpaca_ws_continuity',
    'db_status',
    'reconcile_status',
    'deadman_status',
    'arm_state',
    'session_window_state',
    'bar_context_bars_loaded',
    'last_bar_signal_qty',
    'alerts/active',
    'execution/orders',
    'live_routing_enabled'
)

foreach ($field in $requiredWatcherFields) {
    if ($scriptText -match [regex]::Escape($field)) {
        Pass 'OPR07' "Watcher reads field: $field"
    } else {
        Fail 'OPR07' "Watcher missing field: $field"
    }
}

# ---------------------------------------------------------------------------
# OPR08: Script has paper-only defaults
# ---------------------------------------------------------------------------
$paperChecks = @(
    @{ Pattern = "PaperDbUrl.*5440";                    Desc = "PaperDbUrl default uses port 5440" },
    @{ Pattern = "'paper'";                             Desc = "deployment mode default 'paper'" },
    @{ Pattern = "'alpaca'";                            Desc = "adapter id default 'alpaca'" },
    @{ Pattern = "daemon_mode.*paper|paper.*daemon_mode"; Desc = "Guard checks daemon_mode=paper" },
    @{ Pattern = "DaemonPort.*8899|8899.*DaemonPort";   Desc = "Default DaemonPort=8899" }
)

foreach ($c in $paperChecks) {
    if ($scriptText -match $c.Pattern) {
        Pass 'OPR08' $c.Desc
    } else {
        Fail 'OPR08' "Missing: $($c.Desc)  (pattern: $($c.Pattern))"
    }
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL STATIC INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
