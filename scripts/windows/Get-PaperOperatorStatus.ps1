# =============================================================================
# OPERATOR-CONTROL-SURFACE-BUNDLE-01
# Get-PaperOperatorStatus.ps1
#
# Read-only compact status summary for Paper+Alpaca autonomous paper sessions.
# No mutations. No broker calls. No secrets printed. Fails soft if daemon offline.
#
# Calls:
#   GET /api/v1/autonomous/paper-status
#   GET /api/v1/system/status
#   GET /api/v1/reconcile/status
#   GET /api/v1/portfolio/positions
#   GET /api/v1/portfolio/orders/open
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Get-PaperOperatorStatus.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Get-PaperOperatorStatus.ps1 -DaemonPort 8899
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Get-PaperOperatorStatus.ps1 -Json
#
# Parameters:
#   -DaemonPort   Daemon HTTP port. Default: 8899.
#   -Json         Also print structured JSON summary to stdout.
#
# Hard rules enforced by this script:
#   - No POST, PUT, DELETE, or PATCH calls.
#   - No /v2/orders or any Alpaca broker endpoint.
#   - No DB mutations.
#   - Never prints MQK_OPERATOR_TOKEN, ALPACA_API_KEY*, ALPACA_API_SECRET*,
#     DISCORD_WEBHOOK_URL, or DB password.
#   - Fails soft on each endpoint: offline daemon prints UNAVAILABLE, not exception.
# =============================================================================

[CmdletBinding()]
param(
    [int]   $DaemonPort = 8899,
    [switch]$Json
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$DaemonBase = "http://127.0.0.1:$DaemonPort"

function Write-Header { param([string]$M) Write-Host "" ; Write-Host "=== $M ===" -ForegroundColor Magenta }
function Write-Field  { param([string]$K, $V, [string]$Color = 'White') Write-Host ("  {0,-32} {1}" -f "${K}:", $V) -ForegroundColor $Color }
function Write-Warn   { param([string]$M) Write-Host "  WARN: $M" -ForegroundColor Yellow }
function Write-Ok     { param([string]$M) Write-Host "  OK: $M" -ForegroundColor Green }
function Write-Unavail{ param([string]$M) Write-Host "  UNAVAILABLE: $M" -ForegroundColor DarkGray }

function Invoke-DaemonGet {
    param([string]$Path)
    try {
        return Invoke-RestMethod -Uri "${DaemonBase}${Path}" `
            -Method Get -TimeoutSec 5 -ErrorAction Stop
    } catch {
        return $null
    }
}

function Get-FieldSafe {
    param($Obj, [string]$Field, $Default = 'null')
    if ($null -eq $Obj) { return $Default }
    $prop = $Obj.PSObject.Properties[$Field]
    if ($null -eq $prop) { return $Default }
    $v = $prop.Value
    if ($null -eq $v) { return 'null' }
    return $v
}

# ---------------------------------------------------------------------------
# Collect all data (fail-soft per endpoint)
# ---------------------------------------------------------------------------

Write-Host ""
Write-Host "=== Get-PaperOperatorStatus.ps1 (OPERATOR-CONTROL-SURFACE-BUNDLE-01) ===" -ForegroundColor Cyan
Write-Host "    Daemon : $DaemonBase"
Write-Host "    Time   : $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') UTC"
Write-Host "    Mode   : READ-ONLY  |  No mutations  |  No broker calls"
Write-Host ""

$PaperStatus  = Invoke-DaemonGet '/api/v1/autonomous/paper-status'
$SystemStatus = Invoke-DaemonGet '/api/v1/system/status'
$Reconcile    = Invoke-DaemonGet '/api/v1/reconcile/status'
$Positions    = Invoke-DaemonGet '/api/v1/portfolio/positions'
$OpenOrders   = Invoke-DaemonGet '/api/v1/portfolio/orders/open'

$DaemonOnline = ($null -ne $SystemStatus)

# ---------------------------------------------------------------------------
# 1. Autonomous paper status
# ---------------------------------------------------------------------------
Write-Header "AUTONOMOUS PAPER STATUS"
if ($null -ne $PaperStatus) {
    $readiness     = Get-FieldSafe $PaperStatus 'readiness_classification'
    $next_action   = Get-FieldSafe $PaperStatus 'next_operator_action'
    $live_routing  = Get-FieldSafe $PaperStatus 'live_routing_enabled'
    $recon_st      = Get-FieldSafe $PaperStatus 'reconcile_status'
    $ws_cont       = Get-FieldSafe $PaperStatus 'ws_continuity'
    $flatten_avl   = Get-FieldSafe $PaperStatus 'flatten_available'
    $cur_qty       = Get-FieldSafe $PaperStatus 'current_position_qty'
    $tgt_qty       = Get-FieldSafe $PaperStatus 'target_qty'
    $delta_qty     = Get-FieldSafe $PaperStatus 'computed_delta_qty'
    $no_ord_reason = Get-FieldSafe $PaperStatus 'no_order_reason'

    $readinessColor = switch ($readiness) {
        'ready_for_market_smoke' { 'Green' }
        'market_proof_pending'   { 'Cyan' }
        'blocked'                { 'Red' }
        default                  { 'Yellow' }
    }
    Write-Field 'readiness_classification' $readiness $readinessColor
    Write-Field 'next_operator_action'     $next_action
    Write-Field 'live_routing_enabled'     $live_routing $(if ($live_routing -eq 'True') { 'Red' } else { 'Green' })
    Write-Field 'reconcile_status'         $recon_st    $(if ($recon_st -eq 'ok') { 'Green' } elseif ($recon_st -eq 'dirty') { 'Red' } else { 'Yellow' })
    Write-Field 'ws_continuity'            $ws_cont     $(if ($ws_cont -eq 'live') { 'Green' } elseif ($ws_cont -eq 'gap_detected') { 'Red' } else { 'Yellow' })
    Write-Field 'flatten_available'        $flatten_avl $(if ($flatten_avl -eq 'True') { 'Green' } else { 'Yellow' })
    Write-Field 'current_position_qty'     $cur_qty
    Write-Field 'target_qty'              $tgt_qty
    Write-Field 'computed_delta_qty'      $delta_qty
    Write-Field 'no_order_reason'         $no_ord_reason

    # Flatten blockers (if any)
    $blockers_raw = $PaperStatus.PSObject.Properties['flatten_blockers']
    if ($null -ne $blockers_raw -and $null -ne $blockers_raw.Value -and @($blockers_raw.Value).Count -gt 0) {
        Write-Field 'flatten_blockers' ''
        foreach ($b in @($blockers_raw.Value)) {
            Write-Host ("    - {0}" -f $b) -ForegroundColor Yellow
        }
    }

    # Safety alert
    if ($live_routing -eq 'True') {
        Write-Host ""
        Write-Host "  !!! DANGER: live_routing_enabled=true — this should never occur on paper path !!!" -ForegroundColor Red
    }
    if ($readiness -eq 'blocked') {
        Write-Warn "System is BLOCKED. Resolve flatten_blockers / next_operator_action before starting."
    }
} else {
    Write-Unavail "/api/v1/autonomous/paper-status"
}

# ---------------------------------------------------------------------------
# 2. System status
# ---------------------------------------------------------------------------
Write-Header "SYSTEM STATUS"
if ($null -ne $SystemStatus) {
    $rt_status     = Get-FieldSafe $SystemStatus 'runtime_status'
    $arm_state     = Get-FieldSafe $SystemStatus 'arm_state'
    $daemon_mode   = Get-FieldSafe $SystemStatus 'daemon_mode'
    $adapter_id    = Get-FieldSafe $SystemStatus 'adapter_id'
    $db_status     = Get-FieldSafe $SystemStatus 'db_status'
    $ws_cont_sys   = Get-FieldSafe $SystemStatus 'alpaca_ws_continuity'
    $deadman_st    = Get-FieldSafe $SystemStatus 'deadman_status'
    $kill_switch   = Get-FieldSafe $SystemStatus 'kill_switch_active'
    $live_rt_sys   = Get-FieldSafe $SystemStatus 'live_routing_enabled'
    $integrity_h   = Get-FieldSafe $SystemStatus 'integrity_halt_active'

    Write-Field 'runtime_status'        $rt_status   $(if ($rt_status -eq 'running') { 'Green' } elseif ($rt_status -eq 'halted') { 'Red' } else { 'Yellow' })
    Write-Field 'arm_state'             $arm_state   $(if ($arm_state -eq 'armed') { 'Green' } elseif ($arm_state -eq 'halted') { 'Red' } else { 'Yellow' })
    Write-Field 'daemon_mode'           $daemon_mode $(if ($daemon_mode -eq 'paper') { 'Green' } else { 'Yellow' })
    Write-Field 'adapter_id'            $adapter_id
    Write-Field 'db_status'             $db_status   $(if ($db_status -eq 'ok') { 'Green' } elseif ($db_status -eq 'unavailable') { 'Red' } else { 'Yellow' })
    Write-Field 'alpaca_ws_continuity'  $ws_cont_sys $(if ($ws_cont_sys -eq 'live') { 'Green' } elseif ($ws_cont_sys -eq 'gap_detected') { 'Red' } else { 'Yellow' })
    Write-Field 'deadman_status'        $deadman_st  $(if ($deadman_st -eq 'healthy') { 'Green' } elseif ($deadman_st -eq 'expired') { 'Red' } else { 'Yellow' })
    Write-Field 'kill_switch_active'    $kill_switch $(if ($kill_switch -eq 'True') { 'Red' } else { 'Green' })
    Write-Field 'live_routing_enabled'  $live_rt_sys $(if ($live_rt_sys -eq 'True') { 'Red' } else { 'Green' })
    Write-Field 'integrity_halt_active' $integrity_h $(if ($integrity_h -eq 'True') { 'Red' } else { 'Green' })
} else {
    Write-Unavail "/api/v1/system/status — daemon offline or not yet started"
    Write-Host "  To start daemon: use Start-PaperTradingSmoke.ps1 or Launch-VeritasLedger.ps1" -ForegroundColor DarkGray
}

# ---------------------------------------------------------------------------
# 3. Reconcile status
# ---------------------------------------------------------------------------
Write-Header "RECONCILE STATUS"
if ($null -ne $Reconcile) {
    $rec_st       = Get-FieldSafe $Reconcile 'status'
    $rec_truth    = Get-FieldSafe $Reconcile 'truth_state'
    $rec_mis_pos  = Get-FieldSafe $Reconcile 'mismatched_positions' 0
    $rec_mis_ord  = Get-FieldSafe $Reconcile 'mismatched_orders' 0
    $rec_mis_fill = Get-FieldSafe $Reconcile 'mismatched_fills' 0

    Write-Field 'status'                $rec_st    $(if ($rec_st -eq 'ok') { 'Green' } elseif ($rec_st -eq 'dirty') { 'Red' } else { 'Yellow' })
    Write-Field 'truth_state'           $rec_truth
    Write-Field 'mismatched_positions'  $rec_mis_pos  $(if ([string]$rec_mis_pos -ne '0' -and $rec_mis_pos -ne 'null') { 'Red' } else { 'Green' })
    Write-Field 'mismatched_orders'     $rec_mis_ord  $(if ([string]$rec_mis_ord -ne '0' -and $rec_mis_ord -ne 'null') { 'Red' } else { 'Green' })
    Write-Field 'mismatched_fills'      $rec_mis_fill $(if ([string]$rec_mis_fill -ne '0' -and $rec_mis_fill -ne 'null') { 'Red' } else { 'Green' })

    if ($rec_st -eq 'dirty') {
        Write-Warn "Reconcile dirty — review GET /api/v1/reconcile/mismatches before arming."
    }
} else {
    Write-Unavail "/api/v1/reconcile/status"
}

# ---------------------------------------------------------------------------
# 4. Portfolio positions
# ---------------------------------------------------------------------------
Write-Header "PORTFOLIO POSITIONS"
if ($null -ne $Positions) {
    $pos_truth = Get-FieldSafe $Positions 'truth_state'
    Write-Field 'truth_state' $pos_truth
    $pos_rows = $null
    if ($null -ne $Positions.PSObject.Properties['rows']) { $pos_rows = @($Positions.rows) }
    if ($null -ne $Positions.PSObject.Properties['positions']) { $pos_rows = @($Positions.positions) }
    if ($null -ne $pos_rows -and $pos_rows.Count -gt 0) {
        Write-Field 'position_count' $pos_rows.Count 'Yellow'
        foreach ($p in $pos_rows) {
            $sym = if ($null -ne $p.PSObject.Properties['symbol'])     { $p.symbol }     else { '?' }
            $qty = if ($null -ne $p.PSObject.Properties['qty'])        { $p.qty }        `
                   elseif ($null -ne $p.PSObject.Properties['broker_qty']) { $p.broker_qty } `
                   else { '?' }
            Write-Host ("    {0,-8} qty={1}" -f $sym, $qty) -ForegroundColor Yellow
        }
    } else {
        Write-Ok "No open positions (flat)"
    }
} else {
    Write-Unavail "/api/v1/portfolio/positions"
}

# ---------------------------------------------------------------------------
# 5. Open orders
# ---------------------------------------------------------------------------
Write-Header "OPEN ORDERS"
if ($null -ne $OpenOrders) {
    $ord_truth = Get-FieldSafe $OpenOrders 'truth_state'
    Write-Field 'truth_state' $ord_truth
    $ord_rows = $null
    if ($null -ne $OpenOrders.PSObject.Properties['rows'])   { $ord_rows = @($OpenOrders.rows)   }
    if ($null -ne $OpenOrders.PSObject.Properties['orders']) { $ord_rows = @($OpenOrders.orders) }
    if ($null -ne $ord_rows -and $ord_rows.Count -gt 0) {
        Write-Field 'open_order_count' $ord_rows.Count 'Yellow'
        foreach ($o in $ord_rows) {
            $sym  = if ($null -ne $o.PSObject.Properties['symbol']) { $o.symbol } else { '?' }
            $side = if ($null -ne $o.PSObject.Properties['side'])   { $o.side }   else { '?' }
            $qty  = if ($null -ne $o.PSObject.Properties['qty'])    { $o.qty }    else { '?' }
            $st   = if ($null -ne $o.PSObject.Properties['status']) { $o.status } else { '?' }
            Write-Host ("    {0,-8} {1,-5} qty={2,-5} status={3}" -f $sym, $side, $qty, $st) -ForegroundColor Yellow
        }
    } else {
        Write-Ok "No open orders"
    }
} else {
    Write-Unavail "/api/v1/portfolio/orders/open"
}

# ---------------------------------------------------------------------------
# 6. Quick-action reference
# ---------------------------------------------------------------------------
Write-Header "QUICK ACTION REFERENCE (read-only reminder)"
Write-Host "  Arm:          POST /api/v1/ops/action  {action_key: 'arm-execution'}            (Bearer token required)"
Write-Host "  Disarm:       POST /api/v1/ops/action  {action_key: 'disarm-execution'}"
Write-Host "  Clear halt:   POST /api/v1/ops/action  {action_key: 'clear-halted-run'}"
Write-Host "  Stop:         POST /api/v1/ops/action  {action_key: 'stop-system'}"
Write-Host "  Halt:         POST /api/v1/ops/action  {action_key: 'kill-switch'}"
Write-Host "  Flatten AAPL: POST /api/v1/ops/action  {action_key: 'flatten-paper-positions'}"
Write-Host "  Capture:      powershell -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label <label>"
Write-Host "  Review:       powershell -File scripts\windows\Review-PaperSmokeEvidence.ps1 -Latest -WriteSummary"
Write-Host ""
Write-Host "  See: docs\runbooks\operator_control_surface.md  for full action matrix" -ForegroundColor DarkGray

# ---------------------------------------------------------------------------
# 7. Daemon offline message
# ---------------------------------------------------------------------------
if (-not $DaemonOnline) {
    Write-Host ""
    Write-Host "  Daemon is OFFLINE (port $DaemonPort not responding)." -ForegroundColor DarkGray
    Write-Host "  To start: powershell -File scripts\windows\Start-PaperTradingSmoke.ps1 -CheckOnly" -ForegroundColor DarkGray
    Write-Host "  Or:       powershell -File scripts\windows\Launch-VeritasLedger.ps1 -Mode Observe" -ForegroundColor DarkGray
}

# ---------------------------------------------------------------------------
# 8. JSON output (optional)
# ---------------------------------------------------------------------------
if ($Json) {
    Write-Header "JSON SUMMARY"
    $summary = [ordered]@{
        schema_version          = 'ops-status-v1'
        captured_at             = (Get-Date -Format 'yyyy-MM-ddTHH:mm:ssZ')
        daemon_online           = $DaemonOnline
        autonomous_paper_status = if ($null -ne $PaperStatus)  { $PaperStatus  } else { $null }
        system_status           = if ($null -ne $SystemStatus) { $SystemStatus } else { $null }
        reconcile_status        = if ($null -ne $Reconcile)    { $Reconcile    } else { $null }
        portfolio_positions     = if ($null -ne $Positions)    { $Positions    } else { $null }
        portfolio_open_orders   = if ($null -ne $OpenOrders)   { $OpenOrders   } else { $null }
    }
    $summary | ConvertTo-Json -Depth 10
}

Write-Host ""
