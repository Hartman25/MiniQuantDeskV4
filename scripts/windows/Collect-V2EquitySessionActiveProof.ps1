# =============================================================================
# ASSET-CORE-05F-V2-EQUITY-ACTIVE-PROOF-RUNBOOK-COLLECTOR-01-COMBINED
# Collect-V2EquitySessionActiveProof.ps1
#
# Read-only evidence collector for the MQK_RUNTIME_SESSION_SOURCE=v2_equity_active
# market-hours proof (ASSET-CORE-05). Calls only GET routes on the local daemon,
# optionally reads (never writes) a few DB tables via `docker exec ... psql`, and
# writes a timestamped evidence file under smoke_logs/. Never mutates anything.
#
# Calls (all GET; read-only telemetry routes do not require an operator token):
#   GET /api/v1/system/status
#   GET /api/v1/system/preflight
#   GET /api/v1/autonomous/readiness
#   GET /api/v1/market-data/intraday-refresh/status   (best-effort; older builds may lack this route)
#   GET /api/v1/alerts/active
#   GET /api/v1/execution/summary
#   GET /api/v1/execution/flow
#
# Optional, only with -IncludeDb (read-only `select` statements via `docker exec psql`):
#   select run_id, started_at_utc, armed_at_utc, running_at_utc, stopped_at_utc,
#          halted_at_utc, status, last_heartbeat_utc from runs order by started_at_utc desc limit 5;
#   select * from sys_arm_state;
#   select ts_utc, detail, run_id from sys_autonomous_session_events order by ts_utc desc limit 20;
#
# This script NEVER calls:
#   any POST/PUT/DELETE/PATCH route, /api/v1/ops/action, any strategy/signal route,
#   any order/broker/provider route or script, daemon start/stop, arm/disarm,
#   clear-halted-run, or anything that writes to the database or to .env.local.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\Collect-V2EquitySessionActiveProof.ps1
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\Collect-V2EquitySessionActiveProof.ps1 -IncludeDb
#   powershell -NoProfile -ExecutionPolicy Bypass -File scripts\windows\Collect-V2EquitySessionActiveProof.ps1 -AllowOutsideMarketWindow -SkipIntradayRefreshCheck
#
# Exit codes:
#   0 = daemon reachable, evidence collected, safe_to_continue_to_manual_paper_proof=true
#   2 = daemon reachable, evidence collected, safe_to_continue_to_manual_paper_proof=false (see blockers)
#   1 = daemon unreachable, or the collector itself failed before evidence could be written
#
# See docs/runbooks/v2_equity_session_active_market_hours_proof.md for the full
# operator workflow this script supports.
# =============================================================================

[CmdletBinding()]
param(
    [string]$BaseUrl = "http://127.0.0.1:8899",
    [string]$OutDir = "smoke_logs",
    [switch]$IncludeDb,
    [switch]$SkipIntradayRefreshCheck,
    [switch]$AllowOutsideMarketWindow,
    [int]$Depth = 30,
    [string]$DbContainer = "mqk-paper-postgres",
    [string]$DbName = "miniquantdesk_paper",
    [string]$DbUser = "postgres"
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

function Write-Header   { param([string]$M) Write-Host "" ; Write-Host "=== $M ===" -ForegroundColor Magenta }
function Write-Field    { param([string]$K, $V, [string]$Color = 'White') Write-Host ("  {0,-40} {1}" -f "${K}:", $V) -ForegroundColor $Color }
function Write-Blocker  { param([string]$M) Write-Host "  BLOCKER: $M" -ForegroundColor Red }
function Write-Ok2      { param([string]$M) Write-Host "  OK: $M" -ForegroundColor Green }
function Write-Unavail  { param([string]$M) Write-Host "  UNAVAILABLE: $M" -ForegroundColor DarkGray }

function Get-Prop {
    param($Obj, [string]$Path)
    if ($null -eq $Obj) { return $null }
    $cur = $Obj
    foreach ($seg in ($Path -split '\.')) {
        if ($null -eq $cur) { return $null }
        $prop = $cur.PSObject.Properties[$seg]
        if ($null -eq $prop) { return $null }
        $cur = $prop.Value
    }
    return $cur
}

function Test-True {
    param($v)
    return ($v -eq $true)
}

function Invoke-DaemonGet {
    param([string]$Path)
    try {
        return Invoke-RestMethod -Uri "${BaseUrl}${Path}" -Method Get -TimeoutSec 8 -ErrorAction Stop
    } catch {
        return $null
    }
}

function Invoke-ReadOnlyDbQuery {
    param([string]$Sql, [string]$Label)
    try {
        $output = docker exec $DbContainer psql -U $DbUser -d $DbName -c $Sql 2>&1 | Out-String
        return [ordered]@{ label = $Label; sql = $Sql; output = $output.Trim(); error = $null }
    } catch {
        return [ordered]@{ label = $Label; sql = $Sql; output = $null; error = $_.Exception.Message }
    }
}

# ---------------------------------------------------------------------------
# Repo root / output directory resolution
# ---------------------------------------------------------------------------
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$ResolvedOutDir = if ([System.IO.Path]::IsPathRooted($OutDir)) { $OutDir } else { Join-Path $RepoRoot $OutDir }

Write-Host ""
Write-Host "=== Collect-V2EquitySessionActiveProof.ps1 (ASSET-CORE-05F) ===" -ForegroundColor Cyan
Write-Host "    Daemon : $BaseUrl"
Write-Host "    Time   : $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') local / $((Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')) UTC"
Write-Host "    Mode   : READ-ONLY  |  No mutations  |  No arm/start/order/broker calls"
Write-Host ""

# ---------------------------------------------------------------------------
# 1. Daemon reachability (hard gate -- fails clearly if daemon is not running)
# ---------------------------------------------------------------------------
$SystemStatus = Invoke-DaemonGet '/api/v1/system/status'
$DaemonReachable = ($null -ne $SystemStatus)

if (-not $DaemonReachable) {
    Write-Host ""
    Write-Host "ERROR: daemon is not reachable at $BaseUrl/api/v1/system/status" -ForegroundColor Red
    Write-Host "       This collector does not start the daemon. Start it manually first," -ForegroundColor Red
    Write-Host "       then re-run this script. See docs/runbooks/v2_equity_session_active_market_hours_proof.md" -ForegroundColor Red

    if (-not (Test-Path $ResolvedOutDir)) { New-Item -ItemType Directory -Path $ResolvedOutDir -Force | Out-Null }
    $timestamp = Get-Date -Format 'yyyyMMdd_HHmmss'
    $evidence = [ordered]@{
        schema_version   = 'v2-equity-active-session-proof-v1'
        produced_at_utc  = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        base_url         = $BaseUrl
        daemon_reachable = $false
        verdict          = [ordered]@{
            safe_to_continue_to_manual_paper_proof = $false
            blockers                                = @('daemon_unreachable')
        }
    }
    $jsonPath = Join-Path $ResolvedOutDir "v2_equity_active_session_proof_$timestamp.json"
    $evidence | ConvertTo-Json -Depth $Depth | Set-Content -Path $jsonPath -Encoding utf8
    Write-Host ""
    Write-Host "Evidence written: $jsonPath" -ForegroundColor DarkGray
    exit 1
}

Write-Ok2 "Daemon reachable at $BaseUrl"

# ---------------------------------------------------------------------------
# 2. Collect remaining read-only surfaces (fail-soft per route)
# ---------------------------------------------------------------------------
$Preflight           = Invoke-DaemonGet '/api/v1/system/preflight'
$AutonomousReadiness = Invoke-DaemonGet '/api/v1/autonomous/readiness'
$IntradayRefresh     = Invoke-DaemonGet '/api/v1/market-data/intraday-refresh/status'
$AlertsActive        = Invoke-DaemonGet '/api/v1/alerts/active'
$ExecutionSummary    = Invoke-DaemonGet '/api/v1/execution/summary'
$ExecutionFlow       = Invoke-DaemonGet '/api/v1/execution/flow'

# ---------------------------------------------------------------------------
# 3. System status fields
# ---------------------------------------------------------------------------
Write-Header "SYSTEM STATUS"
$daemonMode           = Get-Prop $SystemStatus 'daemon_mode'
$adapterId            = Get-Prop $SystemStatus 'adapter_id'
$liveRoutingEnabled   = Get-Prop $SystemStatus 'live_routing_enabled'
$rss                  = Get-Prop $SystemStatus 'runtime_session_source'
$sessionSourceMode    = Get-Prop $rss 'session_source_mode'
$productionCutover    = Get-Prop $rss 'production_cutover_enabled'
$activeSourceUsed     = Get-Prop $rss 'active_source_used'
$runtimeUsesSessionV2 = Get-Prop $rss 'runtime_uses_session_v2'
$tradingUsesSessionV2 = Get-Prop $rss 'trading_uses_session_v2'
$legacySessionState   = Get-Prop $rss 'legacy_session_state'
$candidateV2State     = Get-Prop $rss 'candidate_v2_session_state'
$candidateV2Parity    = Get-Prop $rss 'candidate_v2_parity_state'
$fallbackReason       = Get-Prop $rss 'fallback_reason'
$activationRefusal    = Get-Prop $rss 'activation_refusal_reason'

Write-Field 'daemon_mode'                 $daemonMode           $(if ($daemonMode -eq 'paper') { 'Green' } else { 'Red' })
Write-Field 'adapter_id'                  $adapterId            $(if ($adapterId -eq 'alpaca') { 'Green' } else { 'Yellow' })
Write-Field 'live_routing_enabled'        $liveRoutingEnabled   $(if ($liveRoutingEnabled -eq $false) { 'Green' } else { 'Red' })
Write-Field 'session_source_mode'         $sessionSourceMode    $(if ($sessionSourceMode -eq 'v2_equity_active') { 'Cyan' } else { 'Yellow' })
Write-Field 'production_cutover_enabled'  $productionCutover
Write-Field 'active_source_used'          $activeSourceUsed     $(if (Test-True $activeSourceUsed) { 'Green' } else { 'Yellow' })
Write-Field 'runtime_uses_session_v2'     $runtimeUsesSessionV2 $(if (Test-True $runtimeUsesSessionV2) { 'Green' } else { 'Yellow' })
Write-Field 'trading_uses_session_v2'     $tradingUsesSessionV2 $(if (Test-True $tradingUsesSessionV2) { 'Green' } else { 'Yellow' })
Write-Field 'legacy_session_state'        $legacySessionState
Write-Field 'candidate_v2_session_state'  $candidateV2State
Write-Field 'candidate_v2_parity_state'   $candidateV2Parity
Write-Field 'fallback_reason'             $fallbackReason
Write-Field 'activation_refusal_reason'   $activationRefusal

if ($liveRoutingEnabled -eq $true) {
    Write-Host ""
    Write-Host "  !!! DANGER: live_routing_enabled=true -- this must never be true on this path !!!" -ForegroundColor Red
}

# ---------------------------------------------------------------------------
# 4. System preflight fields
# ---------------------------------------------------------------------------
Write-Header "SYSTEM PREFLIGHT"
$deploymentStartAllowed   = Get-Prop $Preflight 'deployment_start_allowed'
$preflightSessionInWindow = Get-Prop $Preflight 'session_in_window'
$autonomousArmState       = Get-Prop $Preflight 'autonomous_arm_state'
$autonomousBlockersPre    = @(Get-Prop $Preflight 'autonomous_blockers')
$preflightBlockers        = @(Get-Prop $Preflight 'blockers')

Write-Field 'deployment_start_allowed'      $deploymentStartAllowed
Write-Field 'session_in_window (preflight)' $preflightSessionInWindow
Write-Field 'autonomous_arm_state'          $autonomousArmState
if ($autonomousBlockersPre.Count -gt 0) {
    Write-Field 'autonomous_blockers' ''
    foreach ($b in $autonomousBlockersPre) { Write-Host "    - $b" -ForegroundColor Yellow }
}
if ($preflightBlockers.Count -gt 0) {
    Write-Field 'blockers' ''
    foreach ($b in $preflightBlockers) { Write-Host "    - $b" -ForegroundColor Yellow }
}

# ---------------------------------------------------------------------------
# 5. Autonomous readiness fields
# ---------------------------------------------------------------------------
Write-Header "AUTONOMOUS READINESS"
$readinessTruthState = Get-Prop $AutonomousReadiness 'truth_state'
$sessionInWindow     = Get-Prop $AutonomousReadiness 'session_in_window'
$sessionWindowState  = Get-Prop $AutonomousReadiness 'session_window_state'
$runtimeStartAllowed = Get-Prop $AutonomousReadiness 'runtime_start_allowed'
$overallReady        = Get-Prop $AutonomousReadiness 'overall_ready'
$armState            = Get-Prop $AutonomousReadiness 'arm_state'
$armReady            = Get-Prop $AutonomousReadiness 'arm_ready'
$wsContinuity        = Get-Prop $AutonomousReadiness 'ws_continuity'
$reconcileStatus     = Get-Prop $AutonomousReadiness 'reconcile_status'
$readinessBlockers   = @(Get-Prop $AutonomousReadiness 'blockers')

Write-Field 'truth_state'           $readinessTruthState $(if ($readinessTruthState -eq 'active') { 'Green' } else { 'Yellow' })
Write-Field 'session_in_window'     $sessionInWindow
Write-Field 'session_window_state'  $sessionWindowState
Write-Field 'runtime_start_allowed' $runtimeStartAllowed
Write-Field 'overall_ready'         $overallReady
Write-Field 'arm_state'             $armState
Write-Field 'arm_ready'             $armReady
Write-Field 'ws_continuity'         $wsContinuity    $(if ($wsContinuity -eq 'live') { 'Green' } else { 'Yellow' })
Write-Field 'reconcile_status'      $reconcileStatus $(if ($reconcileStatus -eq 'ok') { 'Green' } else { 'Yellow' })
if ($readinessBlockers.Count -gt 0) {
    Write-Field 'blockers' ''
    foreach ($b in $readinessBlockers) { Write-Host "    - $b" -ForegroundColor Yellow }
}

# ---------------------------------------------------------------------------
# 6. Intraday refresh status
# ---------------------------------------------------------------------------
Write-Header "INTRADAY REFRESH STATUS"
$intradayRouteAvailable = ($null -ne $IntradayRefresh)
$intradayTruthState     = Get-Prop $IntradayRefresh 'truth_state'
$intradayAllPassed      = Get-Prop $IntradayRefresh 'all_passed'
$intradayStale          = Get-Prop $IntradayRefresh 'stale_or_missing_evidence'
$intradayProducedAt     = Get-Prop $IntradayRefresh 'produced_at_utc'

if ($intradayRouteAvailable) {
    Write-Field 'truth_state'               $intradayTruthState $(if ($intradayTruthState -eq 'active') { 'Green' } else { 'Yellow' })
    Write-Field 'all_passed'                $intradayAllPassed  $(if ($intradayAllPassed -eq $true) { 'Green' } else { 'Red' })
    Write-Field 'stale_or_missing_evidence' $intradayStale      $(if ($intradayStale -eq $false) { 'Green' } else { 'Red' })
    Write-Field 'produced_at_utc'           $intradayProducedAt
} else {
    Write-Unavail "/api/v1/market-data/intraday-refresh/status (route not available on this build, or daemon error)"
}
$intradayRefreshPassed = (Test-True $intradayAllPassed) -and ($intradayStale -eq $false)

# ---------------------------------------------------------------------------
# 7. Alerts / execution summary / execution flow (raw evidence only)
# ---------------------------------------------------------------------------
Write-Header "ALERTS ACTIVE"
if ($null -ne $AlertsActive) {
    $alertCount = Get-Prop $AlertsActive 'alert_count'
    Write-Field 'alert_count' $alertCount $(if ($alertCount -eq 0) { 'Green' } else { 'Yellow' })
} else {
    Write-Unavail "/api/v1/alerts/active"
}

Write-Header "EXECUTION SUMMARY / FLOW"
if ($null -ne $ExecutionSummary) { Write-Ok2 "/api/v1/execution/summary reachable" } else { Write-Unavail "/api/v1/execution/summary" }
if ($null -ne $ExecutionFlow)    { Write-Ok2 "/api/v1/execution/flow reachable" }    else { Write-Unavail "/api/v1/execution/flow" }

# ---------------------------------------------------------------------------
# 8. Verdict computation
# ---------------------------------------------------------------------------
$blockers = New-Object System.Collections.Generic.List[string]

$paperModeConfirmed = ($daemonMode -eq 'paper') -and ($adapterId -eq 'alpaca')
if (-not $paperModeConfirmed) {
    $blockers.Add("daemon_mode='$daemonMode' adapter_id='$adapterId' -- expected paper+alpaca (the canonical autonomous path)")
}

$liveRoutingDisabled = ($liveRoutingEnabled -eq $false)
if ($liveRoutingEnabled -eq $true) {
    $blockers.Add("live_routing_enabled=true -- THIS MUST NEVER BE TRUE; stop and investigate before any further action")
} elseif ($null -eq $liveRoutingEnabled) {
    $blockers.Add("live_routing_enabled is null/missing from /api/v1/system/status -- cannot prove live routing is disabled")
}

$v2ActiveConfigured = ($sessionSourceMode -eq 'v2_equity_active')
if (-not $v2ActiveConfigured) {
    $blockers.Add("session_source_mode='$sessionSourceMode' -- daemon was not started with MQK_RUNTIME_SESSION_SOURCE=v2_equity_active")
}

$v2ActiveSourceUsed = (Test-True $activeSourceUsed)
if ($v2ActiveConfigured -and -not $v2ActiveSourceUsed) {
    $blockers.Add("active_source_used=$activeSourceUsed -- v2 candidate was not accepted as authoritative (fallback_reason='$fallbackReason', activation_refusal_reason='$activationRefusal')")
}

$runtimeUsesV2Bool = (Test-True $runtimeUsesSessionV2)
$tradingUsesV2Bool = (Test-True $tradingUsesSessionV2)
if ($v2ActiveConfigured -and -not $runtimeUsesV2Bool) { $blockers.Add("runtime_uses_session_v2=$runtimeUsesSessionV2") }
if ($v2ActiveConfigured -and -not $tradingUsesV2Bool) { $blockers.Add("trading_uses_session_v2=$tradingUsesSessionV2") }

if ($readinessTruthState -ne 'active') {
    $blockers.Add("autonomous/readiness truth_state='$readinessTruthState' -- not the canonical Paper+Alpaca path; session_in_window/overall_ready are not authoritative")
}
if ($null -ne $runtimeStartAllowed -and $runtimeStartAllowed -ne $true) {
    $blockers.Add("runtime_start_allowed=$runtimeStartAllowed -- a run is already active or start would otherwise be refused")
}

if (-not $AllowOutsideMarketWindow) {
    if ($sessionInWindow -ne $true) {
        $blockers.Add("session_in_window=$sessionInWindow (session_window_state='$sessionWindowState') -- not currently inside the trading session window; pass -AllowOutsideMarketWindow for off-market dry validation")
    }
}

if (-not $SkipIntradayRefreshCheck) {
    if (-not $intradayRouteAvailable) {
        $blockers.Add("intraday-refresh/status route unavailable -- cannot confirm fresh intraday bars; pass -SkipIntradayRefreshCheck only if freshness was independently confirmed")
    } elseif (-not $intradayRefreshPassed) {
        $blockers.Add("intraday refresh all_passed=$intradayAllPassed stale_or_missing_evidence=$intradayStale -- run Refresh-IntradayMarketData.ps1 before proceeding")
    }
}

$safeToContinue = ($blockers.Count -eq 0)

Write-Header "VERDICT"
Write-Field 'daemon_reachable'              $DaemonReachable    'Green'
Write-Field 'paper_mode_confirmed'          $paperModeConfirmed $(if ($paperModeConfirmed) { 'Green' } else { 'Red' })
Write-Field 'live_routing_disabled'         $liveRoutingDisabled $(if ($liveRoutingDisabled) { 'Green' } else { 'Red' })
Write-Field 'v2_active_configured'         $v2ActiveConfigured  $(if ($v2ActiveConfigured) { 'Cyan' } else { 'Yellow' })
Write-Field 'v2_active_source_used'        $v2ActiveSourceUsed  $(if ($v2ActiveSourceUsed) { 'Green' } else { 'Yellow' })
Write-Field 'runtime_uses_session_v2'      $runtimeUsesV2Bool   $(if ($runtimeUsesV2Bool) { 'Green' } else { 'Yellow' })
Write-Field 'trading_uses_session_v2'      $tradingUsesV2Bool   $(if ($tradingUsesV2Bool) { 'Green' } else { 'Yellow' })
Write-Field 'session_in_window'            $sessionInWindow
Write-Field 'session_window_state'         $sessionWindowState
Write-Field 'runtime_start_allowed'        $runtimeStartAllowed
Write-Field 'overall_ready'                $overallReady
Write-Field 'intraday_refresh_passed'      $intradayRefreshPassed $(if ($intradayRefreshPassed) { 'Green' } else { 'Yellow' })
Write-Host ""
if ($blockers.Count -gt 0) {
    Write-Host "  BLOCKERS:" -ForegroundColor Red
    foreach ($b in $blockers) { Write-Blocker $b }
} else {
    Write-Ok2 "No blockers."
}
Write-Host ""
Write-Field 'safe_to_continue_to_manual_paper_proof' $safeToContinue $(if ($safeToContinue) { 'Green' } else { 'Red' })

# ---------------------------------------------------------------------------
# 9. Optional read-only DB evidence (-IncludeDb)
# ---------------------------------------------------------------------------
$DbEvidence = $null
if ($IncludeDb) {
    Write-Header "DATABASE (read-only, -IncludeDb)"
    $latestRuns    = Invoke-ReadOnlyDbQuery -Label 'latest_runs' -Sql "select run_id, started_at_utc, armed_at_utc, running_at_utc, stopped_at_utc, halted_at_utc, status, last_heartbeat_utc from runs order by started_at_utc desc limit 5;"
    $dbArmState    = Invoke-ReadOnlyDbQuery -Label 'sys_arm_state' -Sql "select * from sys_arm_state;"
    $sessionEvents = Invoke-ReadOnlyDbQuery -Label 'latest_autonomous_session_events' -Sql "select ts_utc, detail, run_id from sys_autonomous_session_events order by ts_utc desc limit 20;"

    $DbEvidence = [ordered]@{
        container = $DbContainer
        database  = $DbName
        queries   = @($latestRuns, $dbArmState, $sessionEvents)
    }

    foreach ($q in $DbEvidence.queries) {
        if ($q.error) {
            Write-Unavail "$($q.label): $($q.error)"
        } else {
            Write-Ok2 "$($q.label) collected"
        }
    }
} else {
    Write-Header "DATABASE"
    Write-Host "  Skipped (-IncludeDb not passed)" -ForegroundColor DarkGray
}

# ---------------------------------------------------------------------------
# 10. Write evidence files
# ---------------------------------------------------------------------------
if (-not (Test-Path $ResolvedOutDir)) { New-Item -ItemType Directory -Path $ResolvedOutDir -Force | Out-Null }
$timestamp = Get-Date -Format 'yyyyMMdd_HHmmss'
$jsonPath  = Join-Path $ResolvedOutDir "v2_equity_active_session_proof_$timestamp.json"
$txtPath   = Join-Path $ResolvedOutDir "v2_equity_active_session_proof_$timestamp.txt"

$evidence = [ordered]@{
    schema_version        = 'v2-equity-active-session-proof-v1'
    produced_at_utc        = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    base_url               = $BaseUrl
    system_status           = $SystemStatus
    system_preflight        = $Preflight
    autonomous_readiness    = $AutonomousReadiness
    intraday_refresh_status = $IntradayRefresh
    alerts_active           = $AlertsActive
    execution_summary       = $ExecutionSummary
    execution_flow          = $ExecutionFlow
    db                      = $DbEvidence
    verdict = [ordered]@{
        daemon_reachable                        = $DaemonReachable
        paper_mode_confirmed                    = $paperModeConfirmed
        live_routing_disabled                   = $liveRoutingDisabled
        v2_active_configured                    = $v2ActiveConfigured
        v2_active_source_used                   = $v2ActiveSourceUsed
        runtime_uses_session_v2                 = $runtimeUsesV2Bool
        trading_uses_session_v2                 = $tradingUsesV2Bool
        session_in_window                       = $sessionInWindow
        session_window_state                    = $sessionWindowState
        runtime_start_allowed                    = $runtimeStartAllowed
        overall_ready                            = $overallReady
        intraday_refresh_passed                  = $intradayRefreshPassed
        safe_to_continue_to_manual_paper_proof   = $safeToContinue
        blockers                                 = @($blockers)
    }
}

$evidence | ConvertTo-Json -Depth $Depth | Set-Content -Path $jsonPath -Encoding utf8

$summaryLines = @(
    "V2 Equity Active Session Proof -- $($evidence.produced_at_utc)"
    "Base URL: $BaseUrl"
    "daemon_reachable=$DaemonReachable paper_mode_confirmed=$paperModeConfirmed live_routing_disabled=$liveRoutingDisabled"
    "v2_active_configured=$v2ActiveConfigured v2_active_source_used=$v2ActiveSourceUsed runtime_uses_session_v2=$runtimeUsesV2Bool trading_uses_session_v2=$tradingUsesV2Bool"
    "session_in_window=$sessionInWindow session_window_state=$sessionWindowState runtime_start_allowed=$runtimeStartAllowed overall_ready=$overallReady"
    "intraday_refresh_passed=$intradayRefreshPassed"
    "safe_to_continue_to_manual_paper_proof=$safeToContinue"
    "Blockers:"
)
if ($blockers.Count -eq 0) {
    $summaryLines += "  (none)"
} else {
    foreach ($b in $blockers) { $summaryLines += "  - $b" }
}
$summaryLines -join [Environment]::NewLine | Set-Content -Path $txtPath -Encoding utf8

Write-Host ""
Write-Host "Evidence written:" -ForegroundColor DarkGray
Write-Host "  $jsonPath" -ForegroundColor DarkGray
Write-Host "  $txtPath" -ForegroundColor DarkGray
Write-Host ""

if ($safeToContinue) {
    Write-Host "RESULT: PASS -- safe_to_continue_to_manual_paper_proof=true" -ForegroundColor Green
    exit 0
} else {
    Write-Host "RESULT: BLOCKED -- safe_to_continue_to_manual_paper_proof=false (see blockers above)" -ForegroundColor Red
    exit 2
}
