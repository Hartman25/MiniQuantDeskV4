# =============================================================================
# PAPER-SMOKE-EVIDENCE-REVIEW-02 / EVIDENCE-CAPTURE-TRADE-FLOW-01
# Review-PaperSmokeEvidence.ps1
#
# Read-only evidence review tool for MiniQuantDesk V4 Paper+Alpaca smoke runs.
# Reads a captured evidence folder and classifies the run.
#
# Safety rules enforced by this script:
#   - Never calls live APIs or broker trading endpoints.
#   - Never invokes the paper trading smoke harness.
#   - Never writes to the database (read-only).
#   - Never prints secret values (API keys, tokens, webhook URLs).
#   - All reads are from the local evidence folder only.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Review-PaperSmokeEvidence.ps1 -Latest
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Review-PaperSmokeEvidence.ps1 -Latest -WriteSummary
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Review-PaperSmokeEvidence.ps1 -EvidencePath evidence\paper_smoke_20260603_100518_quick_market_close_alpaca_refresh_smoke
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Review-PaperSmokeEvidence.ps1 -EvidencePath <path> -WriteSummary -OutputJson
#
# Parameters:
#   -EvidencePath  Path to a specific evidence folder to review.
#   -Latest        Find and review the most recent evidence\paper_smoke_* folder.
#   -OutputJson    Print structured JSON summary to stdout.
#   -WriteSummary  Write review_summary.md (and review_summary.json) to the evidence folder.
#   -RepoRoot      Repo root override. Default: two levels up from this script.
#
# Classifications:
#   NATURAL-TRADE-LIFECYCLE-CLOSED -- full natural lifecycle: running, order submitted, ACK,
#                                     fill confirmed, inbox applied, reconcile clean
#   READINESS-CLOSED-NO-TRADE      -- running, bars loaded, no trade signal, reconcile clean, no fault
#   PARTIAL                        -- partial lifecycle; lifecycle incomplete without clear failure
#   OPEN                           -- active blocker: halt, kill switch, bars missing, reconcile dirty
#   FALSE-CLOSED                   -- live routing enabled, secrets in evidence, no proof, fake markers
# =============================================================================

[CmdletBinding()]
param(
    [string]$EvidencePath = '',
    [switch]$Latest,
    [switch]$OutputJson,
    [switch]$WriteSummary,
    [string]$RepoRoot = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

# ---------------------------------------------------------------------------
# Resolve repo root and evidence base
# ---------------------------------------------------------------------------
if (-not $RepoRoot) {
    $RepoRoot = Split-Path -Parent (Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Definition))
}

$EvidenceBase = Join-Path $RepoRoot 'evidence'

# ---------------------------------------------------------------------------
# Resolve evidence folder
# ---------------------------------------------------------------------------
if ($Latest -and $EvidencePath) {
    Write-Host 'ERROR: Specify -Latest OR -EvidencePath, not both.' -ForegroundColor Red
    exit 1
}

if (-not $Latest -and -not $EvidencePath) {
    Write-Host 'ERROR: Specify -Latest or -EvidencePath <path>.' -ForegroundColor Red
    Write-Host 'Example (recommended): powershell -ExecutionPolicy Bypass -File scripts\windows\Review-PaperSmokeEvidence.ps1 -Latest -WriteSummary'
    exit 1
}

if ($Latest) {
    $smokeFolders = @(Get-ChildItem -Path $EvidenceBase -Directory -ErrorAction SilentlyContinue |
        Where-Object { $_.Name -like 'paper_smoke_*' } |
        Sort-Object LastWriteTime -Descending)
    if ($smokeFolders.Count -eq 0) {
        Write-Host "ERROR: No paper_smoke_* folders found under $EvidenceBase" -ForegroundColor Red
        exit 1
    }
    $EvidencePath = $smokeFolders[0].FullName
    Write-Host "Using latest evidence folder: $EvidencePath"
} else {
    if (-not [System.IO.Path]::IsPathRooted($EvidencePath)) {
        $EvidencePath = Join-Path $RepoRoot $EvidencePath
    }
    if (-not (Test-Path $EvidencePath -PathType Container)) {
        Write-Host "ERROR: Evidence folder not found: $EvidencePath" -ForegroundColor Red
        exit 1
    }
}

$FolderName = Split-Path -Leaf $EvidencePath

# ---------------------------------------------------------------------------
# Secret scan helper  --  warn on likely secrets, never print values
#
# Test-SecretLeakLine: inspects one line for actual secret leakage.
# Returns a result object (PatternName/Reason/FilePath/LineNo) or $null.
# Never returns or prints the secret value itself.
# ---------------------------------------------------------------------------

# Values that are redacted placeholders -- do not flag these.
$RedactedPlaceholders = @('[REDACTED]', '<redacted>', '***', '')

function Test-SecretLeakLine {
    param([string]$Line, [string]$FilePath, [int]$LineNo)

    # Env-var style assignments: KEY=value where value is non-empty and non-redacted.
    # Each entry: Name and the regex that captures the value in group 1.
    $envPatterns = @(
        @{ Name = 'ALPACA_API_SECRET_PAPER'; Pattern = 'ALPACA_API_SECRET_PAPER\s*=\s*(.+)' },
        @{ Name = 'ALPACA_API_KEY_PAPER';    Pattern = 'ALPACA_API_KEY_PAPER\s*=\s*(.+)'    },
        @{ Name = 'ALPACA_API_SECRET';       Pattern = 'ALPACA_API_SECRET\s*=\s*(.+)'       },
        @{ Name = 'ALPACA_API_KEY';          Pattern = 'ALPACA_API_KEY\s*=\s*(.+)'          },
        @{ Name = 'MQK_OPERATOR_TOKEN';      Pattern = 'MQK_OPERATOR_TOKEN\s*=\s*(.+)'      },
        @{ Name = 'DISCORD_WEBHOOK_URL';     Pattern = 'DISCORD_WEBHOOK_URL\s*=\s*(.+)'     },
        @{ Name = 'DISCORD_BOT_TOKEN';       Pattern = 'DISCORD_BOT_TOKEN\s*=\s*(.+)'       }
    )

    foreach ($ep in $envPatterns) {
        if ($Line -match $ep.Pattern) {
            $val = $Matches[1].Trim()
            if ($val -ne '' -and $val -notin $script:RedactedPlaceholders) {
                return [PSCustomObject]@{
                    PatternName = $ep.Name
                    Reason      = 'env-var assignment with non-empty value'
                    FilePath    = $FilePath
                    LineNo      = $LineNo
                }
            }
        }
    }

    # Authorization: Bearer <token>  -- only flag when followed by a real token (>8 non-whitespace chars).
    if ($Line -match 'Authorization\s*:\s*Bearer\s+(\S+)') {
        $token = $Matches[1].Trim()
        if ($token.Length -gt 8 -and $token -notin $script:RedactedPlaceholders) {
            return [PSCustomObject]@{
                PatternName = 'Authorization-Bearer'
                Reason      = 'Authorization header with non-trivial Bearer token'
                FilePath    = $FilePath
                LineNo      = $LineNo
            }
        }
    } elseif ($Line -match 'Bearer\s+([A-Za-z0-9_\-\.]{20,})') {
        # Bare "Bearer <long-token>" outside of an Authorization header.
        $token = $Matches[1].Trim()
        if ($token -notin $script:RedactedPlaceholders) {
            return [PSCustomObject]@{
                PatternName = 'Bearer-long-token'
                Reason      = 'Bearer token (long value, >=20 chars)'
                FilePath    = $FilePath
                LineNo      = $LineNo
            }
        }
    }

    return $null
}

$SecretWarnings = [System.Collections.Generic.List[string]]::new()

function Invoke-SecretScan {
    param([string]$FilePath)
    $lines = Get-Content $FilePath -ErrorAction SilentlyContinue
    if (-not $lines) { return }
    $lineNo = 0
    foreach ($line in $lines) {
        $lineNo++
        $hit = Test-SecretLeakLine -Line $line -FilePath $FilePath -LineNo $lineNo
        if ($null -ne $hit) {
            # Report pattern, location, and reason -- never the value.
            $script:SecretWarnings.Add(
                "POSSIBLE SECRET: pattern=$($hit.PatternName)  reason=$($hit.Reason)  file=$(Split-Path -Leaf $hit.FilePath)  line=$($hit.LineNo)"
            )
        }
    }
}

# ---------------------------------------------------------------------------
# Load JSON snapshot helper
# ---------------------------------------------------------------------------
function Read-JsonSnapshot {
    param([string]$Path)
    if (-not (Test-Path $Path)) { return $null }
    try {
        $raw = Get-Content $Path -Raw -ErrorAction Stop
        Invoke-SecretScan -FilePath $Path
        return $raw | ConvertFrom-Json -ErrorAction Stop
    } catch {
        return $null
    }
}

# ---------------------------------------------------------------------------
# Load all API snapshots
# ---------------------------------------------------------------------------
$ApiDir = Join-Path $EvidencePath 'api'
$DbDir  = Join-Path $EvidencePath 'db'

$SystemStatus      = Read-JsonSnapshot (Join-Path $ApiDir 'system_status.json')
$Preflight         = Read-JsonSnapshot (Join-Path $ApiDir 'system_preflight.json')
$AutonomousReady   = Read-JsonSnapshot (Join-Path $ApiDir 'autonomous_readiness.json')
$OmsOverview       = Read-JsonSnapshot (Join-Path $ApiDir 'oms_overview.json')
$ReconcileStatus   = Read-JsonSnapshot (Join-Path $ApiDir 'reconcile_status.json')
$AlertsActive      = Read-JsonSnapshot (Join-Path $ApiDir 'alerts_active.json')
$EventsFeed        = Read-JsonSnapshot (Join-Path $ApiDir 'events_feed.json')
$RiskSummary       = Read-JsonSnapshot (Join-Path $ApiDir 'risk_summary.json')

# Trade-flow snapshots (EVIDENCE-CAPTURE-TRADE-FLOW-01)
$ExecutionFlow       = Read-JsonSnapshot (Join-Path $ApiDir 'execution_flow.json')
$FillQuality         = Read-JsonSnapshot (Join-Path $ApiDir 'fill_quality.json')
$ExecutionOrders     = Read-JsonSnapshot (Join-Path $ApiDir 'execution_orders.json')
$ExecutionOutbox     = Read-JsonSnapshot (Join-Path $ApiDir 'execution_outbox.json')
$PortfolioPositions  = Read-JsonSnapshot (Join-Path $ApiDir 'portfolio_positions.json')
$PortfolioFills      = Read-JsonSnapshot (Join-Path $ApiDir 'portfolio_fills.json')
$ReconcileMismatches = Read-JsonSnapshot (Join-Path $ApiDir 'reconcile_mismatches.json')

# Also scan all files in notes/ and db/ for secrets
foreach ($f in @(Get-ChildItem (Join-Path $EvidencePath 'notes') -File -ErrorAction SilentlyContinue)) {
    Invoke-SecretScan -FilePath $f.FullName
}
foreach ($f in @(Get-ChildItem $DbDir -File -ErrorAction SilentlyContinue)) {
    Invoke-SecretScan -FilePath $f.FullName
}

# ---------------------------------------------------------------------------
# Extract fields
# ---------------------------------------------------------------------------
function Get-Field {
    param($Obj, [string]$Field, $Default = $null)
    if ($null -eq $Obj) { return $Default }
    $val = $Obj.PSObject.Properties[$Field]
    if ($null -eq $val) { return $Default }
    return $val.Value
}

$run_id                  = Get-Field $OmsOverview 'run_id'
$daemon_mode             = Get-Field $SystemStatus 'daemon_mode'
$runtime_status          = Get-Field $SystemStatus 'runtime_status'
$strategy_armed          = Get-Field $SystemStatus 'strategy_armed'
$execution_armed         = Get-Field $SystemStatus 'execution_armed'
$kill_switch_active      = Get-Field $SystemStatus 'kill_switch_active'
$live_routing_enabled    = Get-Field $SystemStatus 'live_routing_enabled'
$alpaca_ws_continuity    = Get-Field $SystemStatus 'alpaca_ws_continuity'
$deadman_status          = Get-Field $SystemStatus 'deadman_status'
$integrity_halt_active   = Get-Field $SystemStatus 'integrity_halt_active'
$risk_halt_active        = Get-Field $SystemStatus 'risk_halt_active'
$fault_signals_raw       = Get-Field $SystemStatus 'fault_signals'
$reconcile_status_str    = Get-Field $SystemStatus 'reconcile_status'

$arm_state               = Get-Field $AutonomousReady 'arm_state'
$ws_continuity_ready     = Get-Field $AutonomousReady 'ws_continuity_ready'
$reconcile_ready         = Get-Field $AutonomousReady 'reconcile_ready'
$signal_ingestion_cfg    = Get-Field $AutonomousReady 'signal_ingestion_configured'
$last_bar_signal_qty     = Get-Field $AutonomousReady 'last_bar_signal_qty'
$bar_context_bars_loaded = Get-Field $AutonomousReady 'bar_context_bars_loaded'
$bar_context_source      = Get-Field $AutonomousReady 'bar_context_source'

$freshness_obj           = Get-Field $AutonomousReady 'market_data_freshness'
if ($null -eq $freshness_obj) { $freshness_obj = Get-Field $Preflight 'market_data_freshness' }
$latest_complete_bar_ts  = Get-Field $freshness_obj 'latest_complete_bar_ts'
$freshness_state         = Get-Field $freshness_obj 'freshness_state'
$completed_rows          = Get-Field $freshness_obj 'completed_rows'

$oms_runtime_status      = Get-Field $OmsOverview 'runtime_status'
$position_count          = Get-Field $OmsOverview 'position_count'
$open_order_count        = Get-Field $OmsOverview 'open_order_count'
$fill_count              = Get-Field $OmsOverview 'fill_count'
$exec_active_orders      = Get-Field $OmsOverview 'execution_active_orders'
$exec_pending_orders     = Get-Field $OmsOverview 'execution_pending_orders'
$reconcile_total_mis     = Get-Field $OmsOverview 'reconcile_total_mismatches'

$recon_mismatched_pos    = Get-Field $ReconcileStatus 'mismatched_positions'
$recon_mismatched_ord    = Get-Field $ReconcileStatus 'mismatched_orders'
$recon_mismatched_fills  = Get-Field $ReconcileStatus 'mismatched_fills'
$recon_unmatched_broker  = Get-Field $ReconcileStatus 'unmatched_broker_events'

$alert_count             = Get-Field $AlertsActive 'alert_count'
$alert_rows              = Get-Field $AlertsActive 'rows'

$live_routing_preflight  = Get-Field $Preflight 'live_routing_disabled'
$live_routing_confirmed_off = $null
if ($null -ne $live_routing_preflight) { $live_routing_confirmed_off = ($live_routing_preflight -eq $true) }

# Try to extract run_id from events_feed
if (-not $run_id -and $null -ne $EventsFeed) {
    $runningEvent = $EventsFeed.rows | Where-Object { $_.detail -eq 'RUNNING' } | Select-Object -First 1
    if ($runningEvent) { $run_id = $runningEvent.run_id }
}

# Determine if runtime reached running from events
$runtime_reached_running = $false
$runtime_halted          = $false
if ($null -ne $EventsFeed -and $null -ne $EventsFeed.rows) {
    foreach ($ev in $EventsFeed.rows) {
        if ($ev.kind -eq 'runtime_transition' -and $ev.detail -eq 'RUNNING')  { $runtime_reached_running = $true }
        if ($ev.kind -eq 'runtime_transition' -and $ev.detail -eq 'HALTED')   { $runtime_halted = $true }
    }
}

# Fault signal summary
$fault_signal_summaries = @()
if ($null -ne $fault_signals_raw) {
    foreach ($fs in $fault_signals_raw) {
        $fault_signal_summaries += "$($fs.severity): $($fs.summary)"
    }
}
if ($null -ne $alert_rows) {
    foreach ($ar in $alert_rows) {
        if ($ar.severity -eq 'critical') {
            $fault_signal_summaries += "alert.critical: $($ar.summary)"
        }
    }
}
$fault_signal_summaries = @($fault_signal_summaries | Select-Object -Unique)

# Outbox/inbox counts from OMS overview (best effort)
$outbox_submitted  = Get-Field $OmsOverview 'execution_active_orders'
$inbox_fill_count  = $fill_count

# Derive capture timestamp from folder name
$capture_ts = $null
if ($FolderName -match 'paper_smoke_(\d{8}_\d{6})') {
    $ts_raw = $Matches[1]
    $capture_ts = "$($ts_raw.Substring(0,4))-$($ts_raw.Substring(4,2))-$($ts_raw.Substring(6,2)) $($ts_raw.Substring(9,2)):$($ts_raw.Substring(11,2)):$($ts_raw.Substring(13,2)) UTC"
}

$api_files_present = @(Get-ChildItem $ApiDir -File -ErrorAction SilentlyContinue).Count -gt 0
$db_files_present  = @(Get-ChildItem $DbDir  -File -ErrorAction SilentlyContinue | Where-Object { $_.Name -ne 'unavailable.txt' }).Count -gt 0

# ---------------------------------------------------------------------------
# Trade-flow field extraction (EVIDENCE-CAPTURE-TRADE-FLOW-01)
# ---------------------------------------------------------------------------

# Fill detection: any single source is sufficient to confirm a fill occurred.
$fill_rows_fill_quality = 0
if ($null -ne $FillQuality -and $null -ne $FillQuality.rows) {
    $fill_rows_fill_quality = @($FillQuality.rows).Count
}

$fill_rows_portfolio_fills = 0
if ($null -ne $PortfolioFills -and $null -ne $PortfolioFills.rows) {
    $fill_rows_portfolio_fills = @($PortfolioFills.rows).Count
}

$fill_rows_exec_flow = 0
if ($null -ne $ExecutionFlow -and $null -ne $ExecutionFlow.rows) {
    $fill_rows_exec_flow = @($ExecutionFlow.rows | Where-Object {
        $null -ne $_.stage -and $_.stage -match 'fill'
    }).Count
}

$fill_detected = (
    ($null -ne $fill_count -and $fill_count -gt 0) -or
    ($fill_rows_fill_quality -gt 0) -or
    ($fill_rows_portfolio_fills -gt 0) -or
    ($fill_rows_exec_flow -gt 0)
)

# Order submission detection: any evidence that an order entered the system.
$order_rows_exec_orders = 0
if ($null -ne $ExecutionOrders -and $null -ne $ExecutionOrders.rows) {
    $order_rows_exec_orders = @($ExecutionOrders.rows).Count
}

$outbox_rows = 0
if ($null -ne $ExecutionOutbox -and $null -ne $ExecutionOutbox.rows) {
    $outbox_rows = @($ExecutionOutbox.rows).Count
}

$order_submitted = (
    ($null -ne $exec_active_orders   -and $exec_active_orders   -gt 0) -or
    ($null -ne $exec_pending_orders  -and $exec_pending_orders  -gt 0) -or
    ($null -ne $open_order_count     -and $open_order_count     -gt 0) -or
    ($order_rows_exec_orders -gt 0) -or
    ($outbox_rows -gt 0) -or
    $fill_detected   # if fill happened, order was definitely submitted
)

# ACK detection from execution flow stages.
$ack_detected = $false
if ($null -ne $ExecutionFlow -and $null -ne $ExecutionFlow.rows) {
    foreach ($row in $ExecutionFlow.rows) {
        if ($null -ne $row.stage -and $row.stage -match 'broker_sent|ack|broker_ack') {
            $ack_detected = $true
            break
        }
    }
}
# A fill implies ACK occurred at some point.
if (-not $ack_detected -and $fill_detected) { $ack_detected = $true }

# Inbox apply detection: fill event reached the execution flow (applied to portfolio).
$inbox_apply_detected = $false
if ($null -ne $ExecutionFlow -and $null -ne $ExecutionFlow.rows) {
    foreach ($row in $ExecutionFlow.rows) {
        if ($null -ne $row.stage -and $row.stage -match 'partial_fill|final_fill|broker_final_fill|broker_partial_fill') {
            $inbox_apply_detected = $true
            break
        }
    }
}
if (-not $inbox_apply_detected) { $inbox_apply_detected = $fill_detected }

# Position detection from portfolio/positions endpoint.
$position_rows_count = 0
if ($null -ne $PortfolioPositions -and $null -ne $PortfolioPositions.rows) {
    $position_rows_count = @($PortfolioPositions.rows).Count
}
$position_nonzero = ($position_rows_count -gt 0 -or ($null -ne $position_count -and $position_count -gt 0))
$position_flat    = (-not $position_nonzero)

# Current position qty (single symbol; null if multi-symbol or unavailable).
$current_position_qty = $null
if ($position_rows_count -eq 1 -and $null -ne $PortfolioPositions.rows[0].qty) {
    $current_position_qty = $PortfolioPositions.rows[0].qty
} elseif ($position_rows_count -eq 1 -and $null -ne $PortfolioPositions.rows[0].broker_qty) {
    $current_position_qty = $PortfolioPositions.rows[0].broker_qty
}

# Broker order map presence via DB snapshot file content.
$broker_order_map_present = $false
$bomFile = Join-Path $DbDir 'broker_order_map_recent.txt'
if (Test-Path $bomFile) {
    $bomContent = Get-Content $bomFile -Raw -ErrorAction SilentlyContinue
    if ($bomContent -and $bomContent.Length -gt 60 -and $bomContent -notmatch '^UNAVAILABLE') {
        $broker_order_map_present = $true
    }
}

# Reconcile mismatch count from the dedicated mismatches endpoint.
$reconcile_mismatch_endpoint_count = 0
if ($null -ne $ReconcileMismatches -and $null -ne $ReconcileMismatches.rows) {
    $reconcile_mismatch_endpoint_count = @($ReconcileMismatches.rows).Count
}

# Reconcile clean: status ok, no mismatches from any source.
$reconcile_clean = (
    ($reconcile_status_str -eq 'ok' -or $reconcile_ready -eq $true) -and
    ($null -eq $reconcile_total_mis -or $reconcile_total_mis -eq 0) -and
    ($reconcile_mismatch_endpoint_count -eq 0)
)

# Strategy decision fields (best effort from autonomous_readiness).
$signal_qty        = $last_bar_signal_qty
$target_qty        = $null
$strategy_decision = $null
if ($null -ne $AutonomousReady) {
    $strategy_decision = Get-Field $AutonomousReady 'last_bar_strategy_decision'
    if ($null -eq $strategy_decision) { $strategy_decision = Get-Field $AutonomousReady 'strategy_decision' }
    $target_qty = Get-Field $AutonomousReady 'last_bar_target_qty'
}

# No-order reason (populated when order_submitted is false).
$no_order_reason = $null
if (-not $order_submitted) {
    if ($null -ne $last_bar_signal_qty -and [string]"$last_bar_signal_qty" -eq '0') {
        $no_order_reason = "signal_qty=0 (strategy evaluated, no trade)"
    } elseif ($null -eq $last_bar_signal_qty) {
        $no_order_reason = "signal_qty=null (strategy not evaluated or no bars loaded)"
    } else {
        $no_order_reason = "signal_qty=$last_bar_signal_qty but no order detected in evidence"
    }
}

# Trade lifecycle composite: all critical legs confirmed.
$trade_lifecycle_detected = ($order_submitted -and $fill_detected -and $reconcile_clean)

# Read notes/final_verdict.txt for any manually-filled verdict
$manual_verdict = $null
$verdictFile = Join-Path $EvidencePath 'notes\final_verdict.txt'
if (Test-Path $verdictFile) {
    $vContent = Get-Content $verdictFile -Raw -ErrorAction SilentlyContinue
    if ($vContent -match 'SMOKE PASSED') { $manual_verdict = 'SMOKE PASSED' }
    elseif ($vContent -match 'SMOKE PARTIAL') { $manual_verdict = 'SMOKE PARTIAL' }
    elseif ($vContent -match 'SMOKE FAILED') { $manual_verdict = 'SMOKE FAILED' }
}

# ---------------------------------------------------------------------------
# Classification logic
# ---------------------------------------------------------------------------

# FALSE-CLOSED checks (highest priority)
$false_closed_reasons = [System.Collections.Generic.List[string]]::new()

if ($live_routing_enabled -eq $true) {
    $false_closed_reasons.Add('live_routing_enabled=true detected in system_status')
}
if ($null -ne $live_routing_confirmed_off -and $live_routing_confirmed_off -eq $false) {
    $false_closed_reasons.Add('preflight live_routing_disabled=false (live routing was NOT disabled at capture time)')
}
if (-not $api_files_present) {
    $false_closed_reasons.Add('No API snapshot files present  --  evidence missing')
}
# Placeholder/template check: final_verdict still shows uncompleted template markers
if ($null -ne (Get-Content $verdictFile -Raw -ErrorAction SilentlyContinue)) {
    $vc = Get-Content $verdictFile -Raw -ErrorAction SilentlyContinue
    if ($vc -and $vc -match 'SMOKE PASSED\s+--') {
        # Template line present but may not be filled  --  OK, do not flag
    }
}
if ($SecretWarnings.Count -gt 0) {
    $false_closed_reasons.Add("Possible secrets detected in evidence  --  review before sharing ($($SecretWarnings.Count) warning(s))")
}

$classification = $null
$classification_reasons = [System.Collections.Generic.List[string]]::new()

if ($false_closed_reasons.Count -gt 0) {
    $classification = 'FALSE-CLOSED'
    foreach ($r in $false_closed_reasons) { $classification_reasons.Add($r) }
}

# OPEN checks
if ($null -eq $classification) {
    $open_reasons = [System.Collections.Generic.List[string]]::new()

    if ($kill_switch_active -eq $true) { $open_reasons.Add('kill_switch_active=true') }
    if ($integrity_halt_active -eq $true) { $open_reasons.Add('integrity_halt_active=true') }
    if ($risk_halt_active -eq $true) { $open_reasons.Add('risk_halt_active=true') }
    if ($runtime_status -eq 'halted' -and -not $runtime_reached_running) {
        $open_reasons.Add('runtime_status=halted and runtime never reached running in event feed')
    }
    if ($freshness_state -and $freshness_state -ne 'ok') {
        $open_reasons.Add("market_data freshness_state=$freshness_state (not ok)")
    }
    if ($reconcile_status_str -eq 'dirty' -or ($null -ne $reconcile_total_mis -and $reconcile_total_mis -gt 0)) {
        $open_reasons.Add("reconcile dirty or mismatches > 0 (total_mismatches=$reconcile_total_mis)")
    }
    if (-not $api_files_present) {
        $open_reasons.Add('No API snapshot files  --  daemon was not reachable or evidence not captured')
    }

    if ($open_reasons.Count -gt 0) {
        $classification = 'OPEN'
        foreach ($r in $open_reasons) { $classification_reasons.Add($r) }
    }
}

# NATURAL-TRADE-LIFECYCLE-CLOSED
# Requires: runtime ran, fill confirmed from any evidence source, reconcile clean.
# Uses multi-source fill detection so a fill captured post-run is still classified correctly.
if ($null -eq $classification) {
    $lifecycle_ok = (
        $runtime_reached_running -eq $true -and
        ($live_routing_enabled -eq $false -or $null -eq $live_routing_enabled) -and
        $fill_detected -and
        $reconcile_clean -and
        ($null -eq $kill_switch_active -or $kill_switch_active -eq $false) -and
        ($null -eq $integrity_halt_active -or $integrity_halt_active -eq $false) -and
        $api_files_present
    )
    if ($lifecycle_ok) {
        $classification = 'NATURAL-TRADE-LIFECYCLE-CLOSED'
        $classification_reasons.Add("runtime reached running; fill_detected=$fill_detected")
        $classification_reasons.Add("fill evidence: fill_quality_rows=$fill_rows_fill_quality, portfolio_fill_rows=$fill_rows_portfolio_fills, exec_flow_fill_rows=$fill_rows_exec_flow, oms_fill_count=$fill_count")
        $classification_reasons.Add("order_submitted=$order_submitted, ack_detected=$ack_detected, inbox_apply_detected=$inbox_apply_detected")
        $classification_reasons.Add("position_nonzero=$position_nonzero, broker_order_map_present=$broker_order_map_present")
        $classification_reasons.Add("reconcile_clean=$reconcile_clean (status=$reconcile_status_str, total_mismatches=$reconcile_total_mis, mismatch_endpoint_rows=$reconcile_mismatch_endpoint_count)")
        $classification_reasons.Add('live_routing_enabled=false confirmed')
    }
}

# READINESS-CLOSED-NO-TRADE
# Requires: runtime ran, bars loaded, NO fills from any source, reconcile clean.
if ($null -eq $classification) {
    $readiness_ok = (
        $runtime_reached_running -eq $true -and
        ($live_routing_enabled -eq $false -or $null -eq $live_routing_enabled) -and
        ($null -ne $completed_rows -and $completed_rows -gt 0) -and
        (-not $fill_detected) -and
        (-not $order_submitted) -and
        $reconcile_clean -and
        ($null -eq $kill_switch_active -or $kill_switch_active -eq $false) -and
        ($null -eq $integrity_halt_active -or $integrity_halt_active -eq $false) -and
        $api_files_present
    )
    if ($readiness_ok) {
        $classification = 'READINESS-CLOSED-NO-TRADE'
        $classification_reasons.Add("runtime reached running, bars loaded (completed_rows=$completed_rows)")
        $noOrderReason = 'no signal or signal=0'
        if ($null -ne $last_bar_signal_qty) { $noOrderReason = "last_bar_signal_qty=$last_bar_signal_qty" }
        $classification_reasons.Add($noOrderReason)
        $classification_reasons.Add("fill_detected=False (fill_quality_rows=$fill_rows_fill_quality, portfolio_fill_rows=$fill_rows_portfolio_fills, exec_flow_fill_rows=$fill_rows_exec_flow, oms_fill_count=$fill_count)")
        $classification_reasons.Add("reconcile_clean=$reconcile_clean (status=$reconcile_status_str)")
    }
}

# PARTIAL  --  catch-all for anything that partially worked
if ($null -eq $classification) {
    $classification = 'PARTIAL'
    if ($runtime_reached_running) {
        $classification_reasons.Add('runtime reached running')
    } else {
        $classification_reasons.Add('runtime did NOT reach running in event feed')
    }
    if ($null -ne $completed_rows -and $completed_rows -gt 0) {
        $classification_reasons.Add("bars loaded (completed_rows=$completed_rows)")
    } else {
        $classification_reasons.Add('bars not confirmed loaded')
    }
    if ($null -eq $SystemStatus) { $classification_reasons.Add('system_status.json absent  --  evidence incomplete') }
    if ($runtime_halted) { $classification_reasons.Add('runtime halted at some point during session') }
    $classification_reasons.Add('lifecycle not fully proven; review notes/ and events_feed for details')
}

# ---------------------------------------------------------------------------
# Build human-readable summary lines
# ---------------------------------------------------------------------------
$summaryLines = [System.Collections.Generic.List[string]]::new()

$summaryLines.Add("# Paper Smoke Evidence Review")
$summaryLines.Add("# Tool: Review-PaperSmokeEvidence.ps1 (PAPER-SMOKE-EVIDENCE-REVIEW-02)")
$summaryLines.Add("# Reviewed: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') UTC")
$summaryLines.Add("")
$summaryLines.Add("## Evidence Folder")
$summaryLines.Add("- Path:             $EvidencePath")
$summaryLines.Add("- Folder name:      $FolderName")
if ($capture_ts) { $summaryLines.Add("- Capture time:     $capture_ts") }
$summaryLines.Add("- API files present: $api_files_present")
$summaryLines.Add("- DB files present:  $db_files_present")
$summaryLines.Add("")
$summaryLines.Add("## Classification")
$summaryLines.Add("### VERDICT: $classification")
foreach ($r in $classification_reasons) { $summaryLines.Add("- $r") }
$summaryLines.Add("")
$summaryLines.Add("## Runtime Fields")
$summaryLines.Add("- run_id:                  $run_id")
$summaryLines.Add("- daemon_mode:             $daemon_mode")
$summaryLines.Add("- runtime_status:          $runtime_status")
$summaryLines.Add("- runtime_reached_running: $runtime_reached_running")
$summaryLines.Add("- runtime_halted:          $runtime_halted")
$summaryLines.Add("- strategy_armed:          $strategy_armed")
$summaryLines.Add("- execution_armed:         $execution_armed")
$summaryLines.Add("- arm_state:               $arm_state")
$summaryLines.Add("- kill_switch_active:      $kill_switch_active")
$summaryLines.Add("- integrity_halt_active:   $integrity_halt_active")
$summaryLines.Add("- risk_halt_active:        $risk_halt_active")
$summaryLines.Add("- live_routing_enabled:    $live_routing_enabled")
$summaryLines.Add("- alpaca_ws_continuity:    $alpaca_ws_continuity")
$summaryLines.Add("- deadman_status:          $deadman_status")
$summaryLines.Add("")
$summaryLines.Add("## Market Data")
$summaryLines.Add("- latest_complete_bar_ts:  $latest_complete_bar_ts")
$summaryLines.Add("- completed_rows:          $completed_rows")
$summaryLines.Add("- freshness_state:         $freshness_state")
$summaryLines.Add("- bar_context_bars_loaded: $bar_context_bars_loaded")
$summaryLines.Add("- bar_context_source:      $bar_context_source")
$summaryLines.Add("")
$summaryLines.Add("## Signal / Order / Fill")
$summaryLines.Add("- last_bar_signal_qty:     $last_bar_signal_qty")
$summaryLines.Add("- signal_ingestion_cfg:    $signal_ingestion_cfg")
$summaryLines.Add("- fill_count (oms):        $fill_count")
$summaryLines.Add("- open_order_count:        $open_order_count")
$summaryLines.Add("- position_count:          $position_count")
$summaryLines.Add("- exec_active_orders:      $exec_active_orders")
$summaryLines.Add("- exec_pending_orders:     $exec_pending_orders")
$summaryLines.Add("")
$summaryLines.Add("## Trade Lifecycle (EVIDENCE-CAPTURE-TRADE-FLOW-01)")
$summaryLines.Add("- trade_lifecycle_detected:     $trade_lifecycle_detected")
$summaryLines.Add("- order_submitted:              $order_submitted")
$summaryLines.Add("- ack_detected:                 $ack_detected")
$summaryLines.Add("- fill_detected:                $fill_detected")
$summaryLines.Add("- inbox_apply_detected:         $inbox_apply_detected")
$summaryLines.Add("- position_nonzero:             $position_nonzero")
$summaryLines.Add("- position_flat:                $position_flat")
$summaryLines.Add("- current_position_qty:         $current_position_qty")
$summaryLines.Add("- broker_order_map_present:     $broker_order_map_present")
$summaryLines.Add("- reconcile_clean:              $reconcile_clean")
$summaryLines.Add("- signal_qty:                   $signal_qty")
$summaryLines.Add("- target_qty:                   $target_qty")
$summaryLines.Add("- strategy_decision:            $strategy_decision")
$summaryLines.Add("- no_order_reason:              $no_order_reason")
$summaryLines.Add("- fill_quality_rows:            $fill_rows_fill_quality")
$summaryLines.Add("- portfolio_fill_rows:          $fill_rows_portfolio_fills")
$summaryLines.Add("- exec_flow_fill_rows:          $fill_rows_exec_flow")
$summaryLines.Add("- outbox_rows:                  $outbox_rows")
$summaryLines.Add("- exec_order_rows:              $order_rows_exec_orders")
$summaryLines.Add("- mismatch_endpoint_rows:       $reconcile_mismatch_endpoint_count")
$summaryLines.Add("")
$summaryLines.Add("## Reconcile")
$summaryLines.Add("- reconcile_status:        $reconcile_status_str")
$summaryLines.Add("- total_mismatches:        $reconcile_total_mis")
$summaryLines.Add("- mismatch_endpoint_rows:  $reconcile_mismatch_endpoint_count")
$summaryLines.Add("- mismatched_positions:    $recon_mismatched_pos")
$summaryLines.Add("- mismatched_orders:       $recon_mismatched_ord")
$summaryLines.Add("- mismatched_fills:        $recon_mismatched_fills")
$summaryLines.Add("- unmatched_broker_events: $recon_unmatched_broker")
$summaryLines.Add("")
$summaryLines.Add("## Fault Signals")
if ($fault_signal_summaries.Count -gt 0) {
    foreach ($fs in $fault_signal_summaries) { $summaryLines.Add("- $fs") }
} else {
    $summaryLines.Add("- (none)")
}
$summaryLines.Add("")

if ($manual_verdict) {
    $summaryLines.Add("## Manual Verdict (from notes/final_verdict.txt)")
    $summaryLines.Add("- $manual_verdict")
    $summaryLines.Add("")
}

if ($SecretWarnings.Count -gt 0) {
    $summaryLines.Add("## SECRET SCAN WARNINGS")
    foreach ($w in $SecretWarnings) { $summaryLines.Add("- WARNING: $w") }
    $summaryLines.Add("- ACTION REQUIRED: Do not share this evidence bundle until secrets are removed.")
    $summaryLines.Add("")
}

$summaryLines.Add("## Classification Reference")
$summaryLines.Add("- NATURAL-TRADE-LIFECYCLE-CLOSED  Full natural lifecycle: running, order submitted, ACK, fill (any source), inbox applied, reconcile clean")
$summaryLines.Add("- READINESS-CLOSED-NO-TRADE       Running, bars loaded, no trade signal/order/fill, reconcile clean, no fault")
$summaryLines.Add("- PARTIAL                         Partial lifecycle; incomplete without clear failure (e.g. order but no fill, fill but reconcile missing)")
$summaryLines.Add("- OPEN                            Active blocker: halt, kill switch, bars missing, reconcile dirty, evidence missing")
$summaryLines.Add("- FALSE-CLOSED                    Live routing enabled, secrets in evidence, no proof files, fake markers")
$summaryLines.Add("")
$summaryLines.Add("## Next Steps")
$summaryLines.Add("- Send review_summary.md to ChatGPT or ledger session for classification update.")
$summaryLines.Add("- If OPEN: resolve blocker, re-run smoke, re-review.")
$summaryLines.Add("- If PARTIAL: check notes/smoke_lifecycle_checklist.txt and events_feed.json for details.")
$summaryLines.Add("- If FALSE-CLOSED: do not record as a passed smoke. Investigate live routing / evidence gap.")

# ---------------------------------------------------------------------------
# Print to console
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host '============================================================'
Write-Host "PAPER SMOKE EVIDENCE REVIEW  --  $FolderName"
Write-Host '============================================================'
foreach ($line in $summaryLines) { Write-Host $line }
Write-Host ''
Write-Host "VERDICT: $classification" -ForegroundColor $(
    switch ($classification) {
        'NATURAL-TRADE-LIFECYCLE-CLOSED' { 'Green' }
        'READINESS-CLOSED-NO-TRADE'      { 'Cyan' }
        'PARTIAL'                        { 'Yellow' }
        'OPEN'                           { 'Red' }
        'FALSE-CLOSED'                   { 'Magenta' }
        default                          { 'White' }
    }
)
Write-Host ''

if ($SecretWarnings.Count -gt 0) {
    Write-Host '*** SECRET SCAN WARNINGS ***' -ForegroundColor Magenta
    foreach ($w in $SecretWarnings) { Write-Host "  $w" -ForegroundColor Magenta }
    Write-Host ''
}

# ---------------------------------------------------------------------------
# Build JSON output
# ---------------------------------------------------------------------------
$jsonObj = [ordered]@{
    schema_version         = 'review-v2'
    reviewed_at            = (Get-Date -Format 'yyyy-MM-ddTHH:mm:ssZ')
    evidence_folder        = $EvidencePath
    folder_name            = $FolderName
    capture_ts             = $capture_ts
    classification         = $classification
    classification_reasons = @($classification_reasons)
    run_id                 = $run_id
    daemon_mode            = $daemon_mode
    runtime_status         = $runtime_status
    runtime_reached_running = $runtime_reached_running
    runtime_halted         = $runtime_halted
    arm_state              = $arm_state
    kill_switch_active     = $kill_switch_active
    integrity_halt_active  = $integrity_halt_active
    risk_halt_active       = $risk_halt_active
    live_routing_enabled   = $live_routing_enabled
    alpaca_ws_continuity   = $alpaca_ws_continuity
    deadman_status         = $deadman_status
    strategy_armed         = $strategy_armed
    execution_armed        = $execution_armed
    signal_ingestion_configured = $signal_ingestion_cfg
    reconcile_status       = $reconcile_status_str
    reconcile_total_mismatches = $reconcile_total_mis
    fill_count             = $fill_count
    open_order_count       = $open_order_count
    position_count         = $position_count
    latest_complete_bar_ts = $latest_complete_bar_ts
    completed_rows         = $completed_rows
    freshness_state        = $freshness_state
    bar_context_bars_loaded = $bar_context_bars_loaded
    bar_context_source     = $bar_context_source
    last_bar_signal_qty    = $last_bar_signal_qty
    fault_signals          = @($fault_signal_summaries)
    secret_scan_warnings   = @($SecretWarnings)
    api_files_present      = $api_files_present
    db_files_present       = $db_files_present
    manual_verdict_note         = $manual_verdict
    trade_lifecycle_detected    = $trade_lifecycle_detected
    order_submitted             = $order_submitted
    ack_detected                = $ack_detected
    fill_detected               = $fill_detected
    inbox_apply_detected        = $inbox_apply_detected
    position_nonzero            = $position_nonzero
    position_flat               = $position_flat
    current_position_qty        = $current_position_qty
    broker_order_map_present    = $broker_order_map_present
    reconcile_clean             = $reconcile_clean
    signal_qty                  = $signal_qty
    target_qty                  = $target_qty
    strategy_decision           = $strategy_decision
    no_order_reason             = $no_order_reason
    fill_quality_rows           = $fill_rows_fill_quality
    portfolio_fill_rows         = $fill_rows_portfolio_fills
    exec_flow_fill_rows         = $fill_rows_exec_flow
    outbox_rows                 = $outbox_rows
    exec_order_rows             = $order_rows_exec_orders
    mismatch_endpoint_rows      = $reconcile_mismatch_endpoint_count
}

$jsonOut = $jsonObj | ConvertTo-Json -Depth 5

if ($OutputJson) {
    Write-Host $jsonOut
}

# ---------------------------------------------------------------------------
# Write summary files
# ---------------------------------------------------------------------------
if ($WriteSummary) {
    $mdPath   = Join-Path $EvidencePath 'review_summary.md'
    $jsonPath = Join-Path $EvidencePath 'review_summary.json'

    $summaryLines | Out-File -FilePath $mdPath -Encoding utf8 -Force
    $jsonOut | Out-File -FilePath $jsonPath -Encoding utf8 -Force

    Write-Host "Summary written:"
    Write-Host "  MD:   $mdPath"
    Write-Host "  JSON: $jsonPath"
    Write-Host ''
}

exit 0
