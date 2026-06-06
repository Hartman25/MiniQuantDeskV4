# =============================================================================
# AAPL-5M-MARKET-SMOKE-RUNNER-01
# Run-AAPL5mMarketSmoke.ps1
#
# Single operator command for tomorrow's AAPL/5m Paper+Alpaca market-hours smoke.
# Orchestrates: bar refresh -> check-only gate -> env set -> smoke -> evidence.
#
# Usage:
#   # Check prerequisites only (no mutations, no smoke):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Run-AAPL5mMarketSmoke.ps1 -CheckOnly
#
#   # Full smoke (30-minute watch window):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Run-AAPL5mMarketSmoke.ps1 -WatchSeconds 1800
#
#   # Skip bar refresh (bars already fresh):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Run-AAPL5mMarketSmoke.ps1 -WatchSeconds 1800 -SkipRefresh
#
# Parameters:
#   -WatchSeconds               Watcher loop duration passed to Start-PaperTradingSmoke. Default: 1800.
#   -Symbol                     Symbol to refresh and smoke. Default: AAPL.
#   -Timeframe                  Bar timeframe. Default: 5m.
#   -RefreshSource              Market-data provider for bar refresh: twelvedata | alpaca. Default: alpaca.
#   -MinCompletedBars           Minimum completed bars required for bar gate. Default: 30.
#   -MaxStalenessMinutes        Maximum bar staleness in minutes before warning. Default: 15.
#   -TargetQty                  MQK_STRATEGY_TARGET_QTY env var value. Default: 3.
#   -MaxTargetQty               MQK_STRATEGY_MAX_TARGET_QTY env var value. Default: 3.
#   -MaxPositionNotionalUsd     MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD env var value. Default: 1000.
#   -EvidenceLabel              Label appended to evidence folder. Default: aapl5m_market_smoke.
#   -SkipRefresh                Skip bar refresh step. Use when bars are already fresh.
#   -CheckOnly                  Run check-only gates only. No smoke, no env mutation.
#
# Hard rules:
#   - Paper+Alpaca only. No live routing.
#   - No signal injection outside the normal smoke path.
#   - No manual broker orders.
#   - No fake bars (refresh calls Refresh-IntradayMarketData.ps1 only).
#   - No DB schema changes.
#   - No .env.local commit.
#   - Never prints MQK_OPERATOR_TOKEN, ALPACA_API_KEY*, ALPACA_API_SECRET*,
#     DISCORD_WEBHOOK_URL, TWELVEDATA_API_KEY, or DB credentials.
# =============================================================================

#Requires -Version 5.1
[CmdletBinding()]
param(
    [int]    $WatchSeconds             = 1800,
    [string] $Symbol                   = 'AAPL',
    [string] $Timeframe                = '5m',
    [ValidateSet('twelvedata', 'alpaca')]
    [string] $RefreshSource            = 'alpaca',
    [int]    $MinCompletedBars         = 30,
    [int]    $MaxStalenessMinutes      = 15,
    [int]    $TargetQty                = 3,
    [int]    $MaxTargetQty             = 3,
    [int]    $MaxPositionNotionalUsd   = 1000,
    [string] $EvidenceLabel            = 'aapl5m_market_smoke',
    [int]    $DaemonPort               = 8899,
    [switch] $SkipRefresh,
    [switch] $CheckOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Helpers
# ---------------------------------------------------------------------------
function Write-Step { param([string]$M) Write-Host "[SMOKE-RUNNER] $M"          -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[SMOKE-RUNNER] OK: $M"      -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[SMOKE-RUNNER] WARN: $M"    -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[SMOKE-RUNNER] FAIL: $M"    -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

# PAPER-SMOKE-AUTOMATION-BUNDLE-01: pre-smoke paper status check (fail-soft).
# Queries /api/v1/autonomous/paper-status if a daemon is already running.
# Prints compact triage summary and warns on blocked (informational before daemon restart).
function Invoke-PaperStatusPrecheck {
    param([int]$Port = 8899)
    $base = "http://127.0.0.1:$Port"
    $data = $null
    try {
        $data = Invoke-RestMethod -Uri "$base/api/v1/autonomous/paper-status" `
            -Method Get -TimeoutSec 4 -ErrorAction Stop
    } catch {
        Write-Warn "autonomous/paper-status not reachable on port $Port (daemon not yet started -- expected)."
        return
    }
    $readiness_classification = if ($null -ne $data.PSObject.Properties['readiness_classification']) { $data.readiness_classification } else { 'unknown' }
    $next_operator_action     = if ($null -ne $data.PSObject.Properties['next_operator_action'])     { $data.next_operator_action }     else { 'unknown' }
    $live_routing_enabled     = if ($null -ne $data.PSObject.Properties['live_routing_enabled'])     { $data.live_routing_enabled }     else { 'unknown' }
    $reconcile_status         = if ($null -ne $data.PSObject.Properties['reconcile_status'])         { $data.reconcile_status }         else { 'unknown' }
    $ws_continuity            = if ($null -ne $data.PSObject.Properties['ws_continuity'])            { $data.ws_continuity }            else { 'unknown' }
    $flatten_available        = if ($null -ne $data.PSObject.Properties['flatten_available'])        { $data.flatten_available }        else { 'unknown' }
    $current_position_qty     = if ($null -ne $data.PSObject.Properties['current_position_qty'])     { $data.current_position_qty }     else { 'null' }
    $target_qty               = if ($null -ne $data.PSObject.Properties['target_qty'])               { $data.target_qty }               else { 'null' }
    $computed_delta_qty       = if ($null -ne $data.PSObject.Properties['computed_delta_qty'])       { $data.computed_delta_qty }       else { 'null' }
    $no_order_reason          = if ($null -ne $data.PSObject.Properties['no_order_reason'])          { $data.no_order_reason }          else { 'null' }

    Write-Step "readiness_classification : $readiness_classification"
    Write-Step "next_operator_action     : $next_operator_action"
    Write-Step "live_routing_enabled     : $live_routing_enabled"
    Write-Step "reconcile_status         : $reconcile_status"
    Write-Step "ws_continuity            : $ws_continuity"
    Write-Step "flatten_available        : $flatten_available"
    Write-Step "current_position_qty     : $current_position_qty"
    Write-Step "target_qty               : $target_qty"
    Write-Step "computed_delta_qty       : $computed_delta_qty"
    Write-Step "no_order_reason          : $no_order_reason"

    # Warn on blocked (daemon will be restarted by Start-PaperTradingSmoke so this is informational).
    if ($readiness_classification -eq 'blocked') {
        Write-Warn "Existing daemon reports readiness_classification=blocked. Start-PaperTradingSmoke will restart and re-establish state."
        Write-Warn "If this is unexpected, resolve before proceeding. next_operator_action: $next_operator_action"
    } elseif ($readiness_classification -eq 'market_proof_pending') {
        Write-Ok "Existing daemon readiness_classification=market_proof_pending."
    } elseif ($readiness_classification -eq 'ready_for_market_smoke') {
        Write-Ok "Existing daemon readiness_classification=ready_for_market_smoke."
    }
}

# ---------------------------------------------------------------------------
# 1. Repo root
# ---------------------------------------------------------------------------
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path.TrimEnd('\')
Set-Location $RepoRoot
Write-Step "Repo root: $RepoRoot"

# ---------------------------------------------------------------------------
# 2. Timestamped log folder
# ---------------------------------------------------------------------------
$Stamp   = Get-Date -Format 'yyyyMMdd_HHmmss'
$LogDir  = Join-Path $RepoRoot "logs\aapl5m_market_smoke\$Stamp"
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
Write-Step "Log dir: $LogDir"

# ---------------------------------------------------------------------------
# 3. Start transcript
# ---------------------------------------------------------------------------
$TranscriptPath = Join-Path $LogDir 'transcript.log'
Start-Transcript -Path $TranscriptPath -Append | Out-Null
Write-Step "Transcript: $TranscriptPath"

# ---------------------------------------------------------------------------
# 4. Git state (HEAD + status; never print secrets)
# ---------------------------------------------------------------------------
Write-Sect "Git state"
try {
    $head = & git -C $RepoRoot rev-parse HEAD 2>&1
    Write-Step "HEAD: $head"
    $status = & git -C $RepoRoot status --short --untracked-files=no 2>&1
    if ($status) {
        Write-Warn "Dirty tracked files:"
        $status | ForEach-Object { Write-Warn "  $_" }
    } else {
        Write-Ok "Working tree clean (tracked files)."
    }
} catch {
    Write-Warn "git state unavailable: $_"
}

# ---------------------------------------------------------------------------
# Subscript paths
# ---------------------------------------------------------------------------
$ScriptsDir        = Join-Path $RepoRoot 'scripts\windows'
$RefreshScript     = Join-Path $ScriptsDir 'Refresh-IntradayMarketData.ps1'
$SmokeScript       = Join-Path $ScriptsDir 'Start-PaperTradingSmoke.ps1'
$EvidenceScript    = Join-Path $ScriptsDir 'Capture-PaperSmokeEvidence.ps1'
$LauncherScript    = Join-Path $ScriptsDir 'Launch-VeritasLedger.ps1'

foreach ($s in @($RefreshScript, $SmokeScript, $EvidenceScript)) {
    if (-not (Test-Path $s)) {
        Write-Fail "Required script not found: $s"
        Stop-Transcript | Out-Null
        exit 1
    }
}

# ---------------------------------------------------------------------------
# CHECK-ONLY mode
# ---------------------------------------------------------------------------
if ($CheckOnly) {
    Write-Sect "CHECK-ONLY mode"

    # Bar readiness check
    Write-Sect "Step C1: bar readiness check (CheckOnly)"
    & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $RefreshScript `
        -Source $RefreshSource `
        -Symbols $Symbol `
        -Timeframe $Timeframe `
        -MinCompletedBars $MinCompletedBars `
        -MaxStalenessMinutes $MaxStalenessMinutes `
        -CheckOnly
    $exitRefresh = $LASTEXITCODE
    if ($exitRefresh -ne 0) {
        Write-Warn "Bar check exited $exitRefresh (non-fatal in CheckOnly)."
    } else {
        Write-Ok "Bar check passed."
    }

    # Set MQK_STRATEGY_SYMBOL so smoke script's CheckOnly bar count reflects the selected symbol
    $env:MQK_STRATEGY_SYMBOL = $Symbol

    # Smoke prerequisite check
    Write-Sect "Step C2: smoke prerequisite check (CheckOnly)"
    & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $SmokeScript -CheckOnly
    $exitSmoke = $LASTEXITCODE
    if ($exitSmoke -ne 0) {
        Write-Warn "Smoke check exited $exitSmoke."
    } else {
        Write-Ok "Smoke check passed."
    }

    # Launcher market-data check (optional — skip if script missing)
    if (Test-Path $LauncherScript) {
        Write-Sect "Step C3: launcher market-data check"
        & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $LauncherScript `
            -Mode Observe -CheckMarketData
        $exitLauncher = $LASTEXITCODE
        if ($exitLauncher -ne 0) {
            Write-Warn "Launcher market-data check exited $exitLauncher."
        } else {
            Write-Ok "Launcher market-data check passed."
        }
    } else {
        Write-Warn "Launch-VeritasLedger.ps1 not found -- skipping launcher check."
    }

    # Result marker
    $markerDir = Join-Path $RepoRoot 'logs\aapl5m_market_smoke'
    $markerPath = Join-Path $markerDir 'latest_result.txt'
    @"
check_only
timestamp: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') UTC
log_dir:   $LogDir
transcript: $TranscriptPath
bar_check_exit:   $exitRefresh
smoke_check_exit: $exitSmoke
"@ | Set-Content -Path $markerPath -Encoding UTF8
    Write-Ok "Result marker: $markerPath"

    Write-Sect "CHECK-ONLY complete"
    Stop-Transcript | Out-Null
    exit 0
}

# ---------------------------------------------------------------------------
# NORMAL (full smoke) mode
# ---------------------------------------------------------------------------
Write-Sect "AAPL/5m Paper+Alpaca market smoke"
Write-Step "WatchSeconds=$WatchSeconds  Symbol=$Symbol  Timeframe=$Timeframe"
Write-Step "RefreshSource=$RefreshSource  EvidenceLabel=$EvidenceLabel"

$smokeStarted = $false
$smokeExit    = 0
$evidenceExit = 0

try {

    # -----------------------------------------------------------------------
    # Step 1: Bar refresh
    # -----------------------------------------------------------------------
    Write-Sect "Step 1: bar refresh"
    if ($SkipRefresh) {
        Write-Step "SkipRefresh is set -- skipping bar refresh."
    } else {
        & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $RefreshScript `
            -Source $RefreshSource `
            -Symbols $Symbol `
            -Timeframe $Timeframe `
            -MinCompletedBars $MinCompletedBars `
            -MaxStalenessMinutes $MaxStalenessMinutes `
            -Once
        $exitRefreshFull = $LASTEXITCODE
        if ($exitRefreshFull -ne 0) {
            Write-Fail "Bar refresh exited $exitRefreshFull."
            Stop-Transcript | Out-Null
            exit $exitRefreshFull
        }
        Write-Ok "Bar refresh complete."
    }

    # -----------------------------------------------------------------------
    # Step 2: Smoke prerequisite check
    # -----------------------------------------------------------------------
    Write-Sect "Step 2: smoke prerequisite check (CheckOnly)"
    & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $SmokeScript -CheckOnly
    $exitPrecheck = $LASTEXITCODE
    if ($exitPrecheck -ne 0) {
        Write-Fail "Smoke prerequisite check failed (exit $exitPrecheck). Aborting."
        Stop-Transcript | Out-Null
        exit $exitPrecheck
    }
    Write-Ok "Smoke prerequisite check passed."

    # -----------------------------------------------------------------------
    # Step 3: Set strategy env vars (conservative, paper-safe)
    # -----------------------------------------------------------------------
    Write-Sect "Step 3: set strategy env vars"
    $env:MQK_STRATEGY_SYMBOL                    = $Symbol
    $env:MQK_STRATEGY_MD_TIMEFRAME              = $Timeframe
    $env:MQK_STRATEGY_TARGET_QTY               = "$TargetQty"
    $env:MQK_STRATEGY_MAX_TARGET_QTY           = "$MaxTargetQty"
    $env:MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD = "$MaxPositionNotionalUsd"
    Write-Ok "MQK_STRATEGY_SYMBOL=$Symbol"
    Write-Ok "MQK_STRATEGY_MD_TIMEFRAME=$Timeframe"
    Write-Ok "MQK_STRATEGY_TARGET_QTY=$TargetQty  MQK_STRATEGY_MAX_TARGET_QTY=$MaxTargetQty"
    Write-Ok "MQK_STRATEGY_MAX_POSITION_NOTIONAL_USD=$MaxPositionNotionalUsd"

    # -----------------------------------------------------------------------
    # Step 3b: Pre-smoke autonomous paper status check (PAPER-SMOKE-AUTOMATION-BUNDLE-01)
    # Informational: queries existing daemon if running before Start-PaperTradingSmoke
    # restarts it. Fail-soft -- daemon likely not yet running at this point.
    # -----------------------------------------------------------------------
    Write-Sect "Step 3b: pre-smoke autonomous paper status check (fail-soft)"
    Invoke-PaperStatusPrecheck -Port $DaemonPort

    # -----------------------------------------------------------------------
    # Step 4: Run smoke
    # -----------------------------------------------------------------------
    Write-Sect "Step 4: run smoke (WatchSeconds=$WatchSeconds)"
    $smokeStarted = $true
    & powershell.exe -ExecutionPolicy Bypass -File $SmokeScript -WatchSeconds $WatchSeconds
    $smokeExit = $LASTEXITCODE
    if ($smokeExit -ne 0) {
        Write-Warn "Smoke exited $smokeExit -- will still capture evidence."
    } else {
        Write-Ok "Smoke exited cleanly."
    }

} catch {
    Write-Fail "Runner caught exception: $_"
    $smokeExit = 1
} finally {

    # -----------------------------------------------------------------------
    # Step 5: Capture evidence (always attempt if smoke started)
    # -----------------------------------------------------------------------
    if ($smokeStarted) {
        Write-Sect "Step 5: capture evidence (label=$EvidenceLabel)"
        try {
            & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $EvidenceScript `
                -Label $EvidenceLabel
            $evidenceExit = $LASTEXITCODE
            if ($evidenceExit -ne 0) {
                Write-Warn "Evidence capture exited $evidenceExit."
            } else {
                Write-Ok "Evidence captured."
            }
        } catch {
            Write-Warn "Evidence capture threw exception: $_"
            $evidenceExit = 1
        }
    }

    # -----------------------------------------------------------------------
    # Step 6: Write result marker
    # -----------------------------------------------------------------------
    Write-Sect "Step 6: write result marker"
    $markerDir = Join-Path $RepoRoot 'logs\aapl5m_market_smoke'
    New-Item -ItemType Directory -Force -Path $markerDir | Out-Null
    $markerPath = Join-Path $markerDir 'latest_result.txt'
    $verdict = if ($smokeExit -eq 0) { 'SMOKE_CLEAN' } else { "SMOKE_NONZERO_EXIT_$smokeExit" }
    @"
$verdict
timestamp:      $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') UTC
symbol:         $Symbol
timeframe:      $Timeframe
watch_seconds:  $WatchSeconds
smoke_exit:     $smokeExit
evidence_exit:  $evidenceExit
log_dir:        $LogDir
transcript:     $TranscriptPath
evidence_label: $EvidenceLabel
"@ | Set-Content -Path $markerPath -Encoding UTF8
    Write-Ok "Result marker: $markerPath"
    Write-Step "Evidence: evidence\paper_smoke_*_${EvidenceLabel}\"

    Stop-Transcript | Out-Null
}

exit $smokeExit
