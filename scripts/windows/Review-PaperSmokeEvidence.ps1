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
$AutonomousPaperStatus = Read-JsonSnapshot (Join-Path $ApiDir 'autonomous_paper_status.json')
$OmsOverview       = Read-JsonSnapshot (Join-Path $ApiDir 'oms_overview.json')
$ReconcileStatus   = Read-JsonSnapshot (Join-Path $ApiDir 'reconcile_status.json')
$AlertsActive      = Read-JsonSnapshot (Join-Path $ApiDir 'alerts_active.json')
$EventsFeed        = Read-JsonSnapshot (Join-Path $ApiDir 'events_feed.json')
$RiskSummary       = Read-JsonSnapshot (Join-Path $ApiDir 'risk_summary.json')

# Trade-flow snapshots (EVIDENCE-CAPTURE-TRADE-FLOW-01)
$ExecutionFlow       = Read-JsonSnapshot (Join-Path $ApiDir 'execution_flow.json')
$FillQuality         = Read-JsonSnapshot (Join-Path $ApiDir 'fill_quality.json')
$ExecutionSummary    = Read-JsonSnapshot (Join-Path $ApiDir 'execution_summary.json')
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

function Test-QuantityZero {
    param($Qty)
    if ($null -eq $Qty) { return $false }
    try {
        return ([decimal]::Parse([string]$Qty, [System.Globalization.CultureInfo]::InvariantCulture) -eq [decimal]0)
    } catch {
        return $false
    }
}

function Test-QuantityNonZero {
    param($Qty)
    if ($null -eq $Qty) { return $false }
    try {
        return ([decimal]::Parse([string]$Qty, [System.Globalization.CultureInfo]::InvariantCulture) -ne [decimal]0)
    } catch {
        return $false
    }
}

function Test-RowFieldMatch {
    param($Row, [string[]]$Fields, [string]$Pattern)
    if ($null -eq $Row) { return $false }
    foreach ($field in $Fields) {
        $value = Get-Field $Row $field
        if ($null -ne $value -and [string]$value -match $Pattern) {
            return $true
        }
    }
    return $false
}

function Test-NegatedUnsafeLine {
    param([string]$Line)
    return ($Line -match '(?i)\b(no|not|without)\s+(\/v2\/orders|raw\s+alpaca|direct[_ -]?broker|fake[_ -]?fills?|fake[_ -]?bars?|signal[_ -]?injection|manual[_ -]?db[_ -]?mutation)')
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

# DB proof snapshots.
$oms_inbox_snapshot_present = $false
$oms_inbox_ack_present      = $false
$oms_inbox_apply_present    = $false
$omsInboxFile = Join-Path $DbDir 'oms_inbox_recent.txt'
if (Test-Path $omsInboxFile) {
    $omsInboxContent = Get-Content $omsInboxFile -Raw -ErrorAction SilentlyContinue
    if ($omsInboxContent -and $omsInboxContent -notmatch '^\s*(UNAVAILABLE|SKIPPED)') {
        $oms_inbox_snapshot_present = $true
        $oms_inbox_ack_present = ($omsInboxContent -match '(?i)\b(ack|broker_ack|order_ack|accepted|new)\b')
        $oms_inbox_apply_present = (
            $omsInboxContent -match '(?i)\b(fill|partial_fill|final_fill|broker_final_fill|broker_partial_fill)\b' -and
            $omsInboxContent -match '(?i)\b(applied\s*\|\s*t|applied\s*[:=]\s*(true|t|1)|\|\s*(true|t|1)\s*\|)'
        )
    }
}

# Baseline/long-position proof must come from explicit evidence, not a final fill.
$existing_long_position_detected = $false
$prior_position_baseline_present = $false
$baselineScanFiles = @(Get-ChildItem $EvidencePath -Recurse -File -ErrorAction SilentlyContinue |
    Where-Object { $_.Extension -in @('.json', '.txt', '.md', '.log') -and $_.Name -notin @('review_summary.md', 'review_summary.json') })
foreach ($f in $baselineScanFiles) {
    $lines = Get-Content $f.FullName -ErrorAction SilentlyContinue
    foreach ($line in $lines) {
        if ($line -match '(?i)\bexisting long position\b|\bprior long\b|\bstarting position\b.*\bAAPL\b.*\b(long|qty\s*[=:]\s*[1-9])') {
            $existing_long_position_detected = $true
        }
        if ($line -match '(?i)\bbroker position baseline adopted\b|\bprior position baseline\b|\bbaseline_position_count\s*[:=]\s*[1-9]') {
            $prior_position_baseline_present = $true
        }
    }
}
$existing_long_or_prior_baseline_present = ($existing_long_position_detected -or $prior_position_baseline_present)

# Fill detection: any real fill source is sufficient to confirm a fill occurred.
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
$has_fill = $fill_detected

# Order submission detection: only explicit normal-path evidence counts.
$order_rows_exec_orders = 0
if ($null -ne $ExecutionOrders -and $null -ne $ExecutionOrders.rows) {
    $order_rows_exec_orders = @($ExecutionOrders.rows).Count
}

$outbox_rows = 0
if ($null -ne $ExecutionOutbox -and $null -ne $ExecutionOutbox.rows) {
    $outbox_rows = @($ExecutionOutbox.rows).Count
}

$order_rows_exec_flow_submit = 0
if ($null -ne $ExecutionFlow -and $null -ne $ExecutionFlow.rows) {
    $order_rows_exec_flow_submit = @($ExecutionFlow.rows | Where-Object {
        Test-RowFieldMatch $_ @('stage', 'message', 'source_table') '(?i)\b(outbox_enqueued|outbox_claimed|outbox_dispatching|broker_sent|order_submitted|submitted|dispatching|oms_outbox)\b'
    }).Count
}

$order_submitted = (
    ($null -ne $exec_active_orders   -and $exec_active_orders   -gt 0) -or
    ($null -ne $exec_pending_orders  -and $exec_pending_orders  -gt 0) -or
    ($null -ne $open_order_count     -and $open_order_count     -gt 0) -or
    ($order_rows_exec_orders -gt 0) -or
    ($outbox_rows -gt 0) -or
    ($order_rows_exec_flow_submit -gt 0)
)
$has_order_submit = $order_submitted

$sell_order_detected = $false
foreach ($rowSet in @(
    @{ Rows = $(if ($null -ne $ExecutionOrders -and $null -ne $ExecutionOrders.rows) { $ExecutionOrders.rows } else { @() }) },
    @{ Rows = $(if ($null -ne $ExecutionOutbox -and $null -ne $ExecutionOutbox.rows) { $ExecutionOutbox.rows } else { @() }) },
    @{ Rows = $(if ($null -ne $ExecutionFlow -and $null -ne $ExecutionFlow.rows) { $ExecutionFlow.rows } else { @() }) }
)) {
    foreach ($row in @($rowSet.Rows)) {
        if (Test-RowFieldMatch $row @('side', 'order_side', 'action', 'intent', 'message', 'stage') '(?i)\b(sell|flatten|close)\b') {
            $sell_order_detected = $true
            break
        }
    }
    if ($sell_order_detected) { break }
}

# ACK detection from explicit ACK/accepted evidence only. A fill does not imply ACK proof.
$ack_detected = $false
$ack_rows_exec_flow = 0
if ($null -ne $ExecutionFlow -and $null -ne $ExecutionFlow.rows) {
    foreach ($row in $ExecutionFlow.rows) {
        if (Test-RowFieldMatch $row @('stage', 'message') '(?i)\b(ack|broker_ack|order_ack|accepted)\b') {
            $ack_detected = $true
            $ack_rows_exec_flow++
            break
        }
    }
}
if (-not $ack_detected -and $oms_inbox_ack_present) { $ack_detected = $true }
$has_order_ack = $ack_detected

# Inbox apply/durable application detection. A fill alone is not applied-state proof.
$inbox_apply_detected = $false
$inbox_apply_rows_exec_flow = 0
if ($null -ne $ExecutionFlow -and $null -ne $ExecutionFlow.rows) {
    foreach ($row in $ExecutionFlow.rows) {
        if (Test-RowFieldMatch $row @('stage', 'message') '(?i)\b(inbox_applied|fill_applied|applied_fill|oms_applied|portfolio_updated|position_updated|execution_state_applied)\b') {
            $inbox_apply_detected = $true
            $inbox_apply_rows_exec_flow++
            break
        }
    }
}
if (-not $inbox_apply_detected -and $oms_inbox_apply_present) { $inbox_apply_detected = $true }

$execution_summary_fill_applied = $false
if ($null -ne $ExecutionSummary) {
    foreach ($field in @('fills_applied', 'applied_fills', 'durable_fills_applied')) {
        $val = Get-Field $ExecutionSummary $field
        if ($val -eq $true -or (Test-QuantityNonZero $val)) {
            $execution_summary_fill_applied = $true
            break
        }
    }
}
if (-not $inbox_apply_detected -and $execution_summary_fill_applied) { $inbox_apply_detected = $true }
$has_inbox_apply = $inbox_apply_detected

# Position detection from portfolio/positions endpoint.
$portfolio_positions_snapshot_present = ($null -ne $PortfolioPositions -and $null -ne $PortfolioPositions.PSObject.Properties['rows'])
$portfolio_position_rows = @()
if ($portfolio_positions_snapshot_present -and $null -ne $PortfolioPositions.rows) {
    $portfolio_position_rows = @(@($PortfolioPositions.rows) | Where-Object { $null -ne $_ })
}
$position_rows_count = 0
if ($portfolio_positions_snapshot_present) {
    $position_rows_count = $portfolio_position_rows.Count
}

$current_position_qty = $null
$aapl_position_qty = $null
$position_qty_unknown = $false
$nonzero_position_rows = 0
if ($portfolio_positions_snapshot_present) {
    if ($position_rows_count -eq 0) {
        $current_position_qty = 0
        $aapl_position_qty = 0
    }
    foreach ($row in $portfolio_position_rows) {
        $qty = Get-Field $row 'qty'
        if ($null -eq $qty) { $qty = Get-Field $row 'broker_qty' }
        if ($null -eq $qty) { $qty = Get-Field $row 'quantity' }

        $symbol = Get-Field $row 'symbol'
        if ($null -ne $symbol -and [string]$symbol -eq 'AAPL') {
            $aapl_position_qty = $qty
            $current_position_qty = $qty
        } elseif ($position_rows_count -eq 1) {
            $current_position_qty = $qty
        }

        if ($null -eq $qty) {
            $position_qty_unknown = $true
        } elseif (Test-QuantityNonZero $qty) {
            $nonzero_position_rows++
        }
    }
}
$position_nonzero = ($nonzero_position_rows -gt 0)
$position_flat = ($portfolio_positions_snapshot_present -and -not $position_qty_unknown -and $nonzero_position_rows -eq 0)
$final_position_flat = $position_flat

# Broker order map presence via DB snapshot file content.
$broker_order_map_present = $false
$bomFile = Join-Path $DbDir 'broker_order_map_recent.txt'
if (Test-Path $bomFile) {
    $bomContent = Get-Content $bomFile -Raw -ErrorAction SilentlyContinue
    if ($bomContent -and
        $bomContent -notmatch '^\s*(UNAVAILABLE|SKIPPED)' -and
        $bomContent -notmatch '\(0 rows\)' -and
        $bomContent -match '(?i)(broker_order_id|broker order|[0-9a-f]{8}-[0-9a-f]{4}-)') {
        $broker_order_map_present = $true
    }
}

# Reconcile mismatch count from the dedicated mismatches endpoint.
$reconcile_mismatch_endpoint_count = 0
$reconcile_mismatch_rows = @()
if ($null -ne $ReconcileMismatches -and
    $null -ne $ReconcileMismatches.PSObject.Properties['rows'] -and
    $null -ne $ReconcileMismatches.rows) {
    $reconcile_mismatch_rows = @(@($ReconcileMismatches.rows) | Where-Object { $null -ne $_ })
    $reconcile_mismatch_endpoint_count = $reconcile_mismatch_rows.Count
}

# Reconcile clean: explicit clean/ok status, no mismatches from any source.
$reconcile_status_api_str = Get-Field $ReconcileStatus 'status'
if ($null -eq $reconcile_status_api_str) { $reconcile_status_api_str = Get-Field $ReconcileStatus 'reconcile_status' }
$reconcile_clean = (
    ($reconcile_status_str -eq 'ok' -or $reconcile_status_str -eq 'clean' -or
     $reconcile_status_api_str -eq 'ok' -or $reconcile_status_api_str -eq 'clean') -and
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

# Autonomous paper status fields (PAPER-SMOKE-AUTOMATION-BUNDLE-01).
$ps_status_present           = ($null -ne $AutonomousPaperStatus -and
                                $null -ne $AutonomousPaperStatus.PSObject.Properties['readiness_classification'])
$ps_readiness_classification = Get-Field $AutonomousPaperStatus 'readiness_classification'
$ps_next_operator_action     = Get-Field $AutonomousPaperStatus 'next_operator_action'
$ps_flatten_available        = Get-Field $AutonomousPaperStatus 'flatten_available'
$ps_flatten_blockers_raw     = Get-Field $AutonomousPaperStatus 'flatten_blockers'
$ps_current_position_qty     = Get-Field $AutonomousPaperStatus 'current_position_qty'
$ps_target_qty               = Get-Field $AutonomousPaperStatus 'target_qty'
$ps_computed_delta_qty       = Get-Field $AutonomousPaperStatus 'computed_delta_qty'
$ps_no_order_reason          = Get-Field $AutonomousPaperStatus 'no_order_reason'
$ps_flatten_blockers_str     = if ($null -ne $ps_flatten_blockers_raw) {
    ($ps_flatten_blockers_raw | ForEach-Object { "$_" }) -join '; '
} else { $null }

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

# Unsafe evidence markers: fail closed if evidence shows shortcuts or synthetic proof.
$unsafe_marker_flags = @{
    direct_broker_shortcut = $false
    fake_fill              = $false
    fake_bar               = $false
    signal_injection       = $false
    manual_db_mutation     = $false
}
$unsafe_marker_hits = [System.Collections.Generic.List[string]]::new()
$unsafeMarkerSpecs = @(
    @{ Name = 'direct_broker_shortcut'; Pattern = '(?i)\b(direct[_ -]?broker[_ -]?shortcut|raw[_ -]?alpaca|\/v2\/orders)\b' },
    @{ Name = 'fake_fill';              Pattern = '(?i)\b(fake[_ -]?fills?|synthetic[_ -]?fills?|simulated[_ -]?fills?)\b\s*(:|=|\b)\s*(true|yes|present|created|used)?' },
    @{ Name = 'fake_bar';               Pattern = '(?i)\b(fake[_ -]?bars?|synthetic[_ -]?bars?|simulated[_ -]?bars?)\b\s*(:|=|\b)\s*(true|yes|present|created|used)?' },
    @{ Name = 'signal_injection';       Pattern = '(?i)\b(signal[_ -]?injection\s*(:|=)?\s*(true|yes|present|used|admitted)|signal injection admitted|injected signal|\/api\/v1\/strategy\/signal)\b' },
    @{ Name = 'manual_db_mutation';     Pattern = '(?i)\b(manual[_ -]?db[_ -]?mutation\s*(:|=)?\s*(true|yes|present|used)|manual\s+(INSERT|UPDATE|DELETE|DROP)|psql.*-c.*(INSERT|UPDATE|DELETE|DROP))\b' }
)
$unsafeScanFiles = @(Get-ChildItem $EvidencePath -Recurse -File -ErrorAction SilentlyContinue |
    Where-Object { $_.Extension -in @('.json', '.txt', '.md', '.log', '.csv') -and $_.Name -notin @('review_summary.md', 'review_summary.json') })
foreach ($f in $unsafeScanFiles) {
    $relPath = $f.FullName
    if ($relPath.StartsWith($EvidencePath)) {
        $relPath = $relPath.Substring($EvidencePath.Length).TrimStart([char[]]@('\', '/'))
    }
    $lines = Get-Content $f.FullName -ErrorAction SilentlyContinue
    foreach ($line in $lines) {
        if (Test-NegatedUnsafeLine -Line $line) { continue }
        foreach ($spec in $unsafeMarkerSpecs) {
            if ($line -match $spec.Pattern) {
                $unsafe_marker_flags[$spec.Name] = $true
                $hit = "marker=$($spec.Name) file=$relPath"
                if (-not $unsafe_marker_hits.Contains($hit)) {
                    $unsafe_marker_hits.Add($hit)
                }
            }
        }
    }
}

$direct_broker_shortcut_detected = $unsafe_marker_flags['direct_broker_shortcut']
$fake_fill_detected              = $unsafe_marker_flags['fake_fill']
$fake_bar_detected               = $unsafe_marker_flags['fake_bar']
$signal_injection_detected       = $unsafe_marker_flags['signal_injection']
$manual_db_mutation_detected     = $unsafe_marker_flags['manual_db_mutation']

$no_direct_broker_shortcut = (-not $direct_broker_shortcut_detected)
$no_fake_fills             = (-not $fake_fill_detected)
$no_fake_bars              = (-not $fake_bar_detected)
$no_signal_injection       = (-not $signal_injection_detected)
$no_manual_db_mutation     = (-not $manual_db_mutation_detected)
$no_unsafe_markers         = ($unsafe_marker_hits.Count -eq 0)
$live_routing_disabled     = ($live_routing_enabled -eq $false -and ($null -eq $live_routing_confirmed_off -or $live_routing_confirmed_off -eq $true))

$lifecycle_missing_requirements = [System.Collections.Generic.List[string]]::new()
if (-not $api_files_present) { $lifecycle_missing_requirements.Add('api_files_present') }
if (-not $runtime_reached_running) { $lifecycle_missing_requirements.Add('runtime_reached_running=true') }
if (-not $existing_long_or_prior_baseline_present) { $lifecycle_missing_requirements.Add('existing_long_position_or_prior_baseline_present') }
if (-not $has_order_submit) { $lifecycle_missing_requirements.Add('has_order_submit') }
if (-not $sell_order_detected) { $lifecycle_missing_requirements.Add('sell_side_or_flatten_order_detected') }
if (-not $has_order_ack) { $lifecycle_missing_requirements.Add('has_order_ack') }
if (-not $has_fill) { $lifecycle_missing_requirements.Add('has_fill') }
if (-not $has_inbox_apply) { $lifecycle_missing_requirements.Add('has_inbox_apply_or_durable_applied_state') }
if (-not $portfolio_positions_snapshot_present) { $lifecycle_missing_requirements.Add('portfolio_positions_snapshot_present') }
if (-not $final_position_flat) { $lifecycle_missing_requirements.Add('final_position_flat') }
if (-not $broker_order_map_present) { $lifecycle_missing_requirements.Add('broker_order_map_present') }
if (-not $reconcile_clean) { $lifecycle_missing_requirements.Add('reconcile_clean') }
if (-not $live_routing_disabled) { $lifecycle_missing_requirements.Add('live_routing_enabled=false') }
if ($kill_switch_active -eq $true) { $lifecycle_missing_requirements.Add('kill_switch_active=false') }
if ($integrity_halt_active -eq $true) { $lifecycle_missing_requirements.Add('integrity_halt_active=false') }
if ($risk_halt_active -eq $true) { $lifecycle_missing_requirements.Add('risk_halt_active=false') }
if (-not $no_direct_broker_shortcut) { $lifecycle_missing_requirements.Add('no_direct_broker_shortcut_marker') }
if (-not $no_fake_fills) { $lifecycle_missing_requirements.Add('no_fake_fill_marker') }
if (-not $no_fake_bars) { $lifecycle_missing_requirements.Add('no_fake_bar_marker') }
if (-not $no_signal_injection) { $lifecycle_missing_requirements.Add('no_signal_injection_marker') }
if (-not $no_manual_db_mutation) { $lifecycle_missing_requirements.Add('no_manual_db_mutation_marker') }

# Trade lifecycle composite: all strict market-proof requirements confirmed.
$trade_lifecycle_detected = ($lifecycle_missing_requirements.Count -eq 0)

# Read notes/final_verdict.txt for any manually-CHECKED verdict.
# Only an explicitly-checked checkbox ("[x]" or "[X]") at the start of a
# line counts as an operator verdict -- the unfilled "[ ] SMOKE PASSED"
# template option must never be misread as a completed verdict
# (PAPER-SMOKE-EVIDENCE-VERDICT-HYGIENE-01).
$manual_verdict = $null
$verdictFile = Join-Path $EvidencePath 'notes\final_verdict.txt'
$verdictFileContent = $null
if (Test-Path $verdictFile) {
    $verdictFileContent = Get-Content $verdictFile -Raw -ErrorAction SilentlyContinue
    if ($verdictFileContent -match '(?im)^\s*#?\s*\[\s*[xX]\s*\]\s*SMOKE PASSED') { $manual_verdict = 'SMOKE PASSED' }
    elseif ($verdictFileContent -match '(?im)^\s*#?\s*\[\s*[xX]\s*\]\s*SMOKE PARTIAL') { $manual_verdict = 'SMOKE PARTIAL' }
    elseif ($verdictFileContent -match '(?im)^\s*#?\s*\[\s*[xX]\s*\]\s*SMOKE FAILED') { $manual_verdict = 'SMOKE FAILED' }
    elseif ($verdictFileContent -match '(?im)^\s*#?\s*\[\s*[xX]\s*\]\s*SMOKE NOT RUN') { $manual_verdict = 'SMOKE NOT RUN' }
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
if ($SecretWarnings.Count -gt 0) {
    $false_closed_reasons.Add("Possible secrets detected in evidence  --  review before sharing ($($SecretWarnings.Count) warning(s))")
}
if ($unsafe_marker_hits.Count -gt 0) {
    foreach ($hit in $unsafe_marker_hits) {
        $false_closed_reasons.Add("Unsafe evidence marker detected: $hit")
    }
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

    # Supplement: blocked autonomous paper status without trade lifecycle → OPEN.
    # Does not override trade lifecycle evidence; only adds a reason when lifecycle is not closed.
    if ($ps_status_present -and $ps_readiness_classification -eq 'blocked' -and -not $trade_lifecycle_detected) {
        $open_reasons.Add("autonomous_paper_status: readiness_classification=blocked  next_operator_action=$ps_next_operator_action")
        if ($ps_flatten_blockers_str) { $open_reasons.Add("  flatten_blockers: $ps_flatten_blockers_str") }
    }

    if ($open_reasons.Count -gt 0) {
        $classification = 'OPEN'
        foreach ($r in $open_reasons) { $classification_reasons.Add($r) }
    }
}

# NATURAL-TRADE-LIFECYCLE-CLOSED
# Requires explicit market-proof lifecycle evidence. Missing evidence fails closed.
if ($null -eq $classification) {
    $lifecycle_ok = ($lifecycle_missing_requirements.Count -eq 0)
    if ($lifecycle_ok) {
        $classification = 'NATURAL-TRADE-LIFECYCLE-CLOSED'
        $classification_reasons.Add("runtime reached running; strict lifecycle requirements satisfied")
        $classification_reasons.Add("fill evidence: fill_quality_rows=$fill_rows_fill_quality, portfolio_fill_rows=$fill_rows_portfolio_fills, exec_flow_fill_rows=$fill_rows_exec_flow, oms_fill_count=$fill_count")
        $classification_reasons.Add("has_order_submit=$has_order_submit, sell_order_detected=$sell_order_detected, has_order_ack=$has_order_ack, has_inbox_apply=$has_inbox_apply")
        $classification_reasons.Add("existing_long_or_prior_baseline_present=$existing_long_or_prior_baseline_present, final_position_flat=$final_position_flat, broker_order_map_present=$broker_order_map_present")
        $classification_reasons.Add("reconcile_clean=$reconcile_clean (status=$reconcile_status_str, total_mismatches=$reconcile_total_mis, mismatch_endpoint_rows=$reconcile_mismatch_endpoint_count)")
        $classification_reasons.Add('live_routing_enabled=false confirmed')
        $classification_reasons.Add('no unsafe evidence markers detected')
    }
}

# READINESS-CLOSED-NO-TRADE
# Requires: runtime ran, bars loaded, NO fills from any source, reconcile clean.
if ($null -eq $classification) {
    $readiness_ok = (
        $runtime_reached_running -eq $true -and
        $live_routing_disabled -and
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

# Supplement: note market_proof_pending regardless of classification (does not change classification).
if ($ps_status_present -and $ps_readiness_classification -eq 'market_proof_pending') {
    $classification_reasons.Add("autonomous_paper_status: market_proof_pending (market proof cycle not yet complete)")
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

if ($classification -ne 'NATURAL-TRADE-LIFECYCLE-CLOSED' -and $lifecycle_missing_requirements.Count -gt 0) {
    $classification_reasons.Add("lifecycle closure withheld; missing requirements: $($lifecycle_missing_requirements -join ', ')")
}

# ---------------------------------------------------------------------------
# Manual-verdict conflict check (PAPER-SMOKE-EVIDENCE-VERDICT-HYGIENE-01)
#
# review_summary.json/.md (this output) is the source of truth, never
# notes/final_verdict.txt. If an operator checked "[x] SMOKE PASSED" but the
# generated classification did not reach a closed state, surface a loud
# conflict warning rather than silently trusting the operator note.
# ---------------------------------------------------------------------------
$manual_verdict_conflict = (
    $manual_verdict -eq 'SMOKE PASSED' -and
    $classification -ne 'NATURAL-TRADE-LIFECYCLE-CLOSED' -and
    $classification -ne 'READINESS-CLOSED-NO-TRADE'
)

# ---------------------------------------------------------------------------
# Ledger-specific classification labels (PAPER-SMOKE-LEDGER-SPECIFIC-CLASSIFIER-01)
#
# These are EMITTED IN ADDITION to $classification above -- they never replace
# or weaken the strict NATURAL-TRADE-LIFECYCLE-CLOSED gate from commit 0b0bc93.
# Each ledger label below is its own independently-strict, fail-closed gate:
# every one of them requires the SAME underlying strict lifecycle proof
# ($trade_lifecycle_detected, i.e. $lifecycle_missing_requirements.Count -eq 0,
# or an equally strict parallel requirement set) PLUS additional ledger-specific
# evidence. Missing proof never produces a *-CLOSED label -- it withholds to
# MARKET-PROOF-PENDING / PARTIAL / OPEN, matching the overall evidence posture.
# ---------------------------------------------------------------------------

# Withheld status mirrors the overall evidence posture for this folder: an
# evidence pull that is itself OPEN/FALSE-CLOSED cannot produce a "pending"
# ledger item (that would overstate it); one in market_proof_pending stays
# market_proof_pending; anything else that is merely incomplete is PARTIAL.
$ledger_withheld_status = 'PARTIAL'
if ($classification -eq 'OPEN' -or $classification -eq 'FALSE-CLOSED') {
    $ledger_withheld_status = 'OPEN'
} elseif ($ps_status_present -and $ps_readiness_classification -eq 'market_proof_pending') {
    $ledger_withheld_status = 'MARKET-PROOF-PENDING'
}

function New-LedgerVerdict {
    param(
        [string]$ClosedLabel,
        [bool]$Closed,
        [string[]]$Missing
    )
    if ($Closed) {
        return [pscustomobject]@{ label = $ClosedLabel; status = $ClosedLabel; closed = $true;  missing_requirements = @() }
    }
    return [pscustomobject]@{ label = $ClosedLabel; status = $ledger_withheld_status; closed = $false; missing_requirements = @($Missing) }
}

# --- AAPL-NATURAL-SELL-LIFECYCLE-CLOSED -----------------------------------
# Strict NATURAL-TRADE-LIFECYCLE-CLOSED gate, narrowed to confirm the traded
# symbol was specifically AAPL (not just "some symbol"). Symbol confirmation
# requires an explicit AAPL row in trade evidence (orders/fills/flow), an
# AAPL row in the final positions snapshot, or the existing AAPL-baseline text
# match already used for $existing_long_position_detected.
$aapl_symbol_in_trade_evidence = $false
foreach ($rowSet in @(
    @(if ($null -ne $ExecutionOrders  -and $null -ne $ExecutionOrders.rows)  { $ExecutionOrders.rows }  else { @() }),
    @(if ($null -ne $PortfolioFills   -and $null -ne $PortfolioFills.rows)   { $PortfolioFills.rows }   else { @() }),
    @(if ($null -ne $ExecutionFlow    -and $null -ne $ExecutionFlow.rows)    { $ExecutionFlow.rows }    else { @() })
)) {
    foreach ($row in @($rowSet)) {
        $rowSymbol = Get-Field $row 'symbol'
        if ($null -ne $rowSymbol -and [string]$rowSymbol -eq 'AAPL') { $aapl_symbol_in_trade_evidence = $true; break }
    }
    if ($aapl_symbol_in_trade_evidence) { break }
}
$aapl_position_row_confirmed = $false
if ($portfolio_positions_snapshot_present -and $position_rows_count -gt 0) {
    $aapl_position_row_confirmed = (@($portfolio_position_rows | Where-Object {
        ([string](Get-Field $_ 'symbol')) -eq 'AAPL'
    }).Count -gt 0)
}
$aapl_symbol_confirmed = ($aapl_symbol_in_trade_evidence -or $aapl_position_row_confirmed -or $existing_long_position_detected)

$aapl_sell_missing = [System.Collections.Generic.List[string]]::new()
$aapl_sell_missing.AddRange([string[]]@($lifecycle_missing_requirements))
if (-not $aapl_symbol_confirmed) { $aapl_sell_missing.Add('aapl_symbol_confirmed_in_trade_evidence') }
$aapl_sell_proven = ($aapl_sell_missing.Count -eq 0)

# --- PAPER-SAFE-FLATTEN-ROUTE-MARKET-PROOF-CLOSED --------------------------
# Requires explicit flatten.requested evidence (events feed / active alerts /
# Discord observation note, all positively matched -- not inferred from a flat
# position alone), routed through the real broker path end to end, settling
# clean. A flat position with no flatten.requested evidence proves nothing
# about the *flatten route* specifically (it could be a natural sell).
$flatten_requested_evidence_detected = $false
foreach ($rowSet in @(
    @(if ($null -ne $EventsFeed   -and $null -ne $EventsFeed.rows)   { $EventsFeed.rows }   else { @() }),
    @(if ($null -ne $AlertsActive -and $null -ne $AlertsActive.rows) { $AlertsActive.rows } else { @() })
)) {
    foreach ($row in @($rowSet)) {
        if (Test-RowFieldMatch $row @('kind', 'detail', 'event_type', 'alert_type', 'message', 'summary', 'stage') '(?i)flatten[\.\-_ ]?requested') {
            $flatten_requested_evidence_detected = $true; break
        }
    }
    if ($flatten_requested_evidence_detected) { break }
}
if (-not $flatten_requested_evidence_detected) {
    $discordObsFileFlatten = Join-Path $EvidencePath 'notes\discord_observation.txt'
    if (Test-Path $discordObsFileFlatten) {
        $dcontentFlatten = Get-Content $discordObsFileFlatten -Raw -ErrorAction SilentlyContinue
        if ($dcontentFlatten -and
            $dcontentFlatten -match '(?i)flatten\.requested' -and
            $dcontentFlatten -match '(?i)\b(confirmed|observed|received|posted)\b' -and
            $dcontentFlatten -notmatch '(?i)\bnot\s+(yet\s+)?(observed|confirmed|received|posted)\b') {
            $flatten_requested_evidence_detected = $true
        }
    }
}

$safe_flatten_missing = [System.Collections.Generic.List[string]]::new()
if (-not $flatten_requested_evidence_detected) { $safe_flatten_missing.Add('flatten_requested_evidence_detected (events_feed/alerts_active/discord_observation)') }
if (-not $has_order_ack)           { $safe_flatten_missing.Add('has_order_ack') }
if (-not $has_fill)                { $safe_flatten_missing.Add('has_fill') }
if (-not $has_inbox_apply)         { $safe_flatten_missing.Add('has_inbox_apply_or_durable_applied_state') }
if (-not $broker_order_map_present){ $safe_flatten_missing.Add('broker_order_map_present') }
if (-not $final_position_flat)     { $safe_flatten_missing.Add('final_position_flat') }
if (-not $reconcile_clean)         { $safe_flatten_missing.Add('reconcile_clean') }
if (-not $live_routing_disabled)   { $safe_flatten_missing.Add('live_routing_enabled=false') }
if (-not $no_unsafe_markers)       { $safe_flatten_missing.Add('no_unsafe_evidence_markers') }
$safe_flatten_proven = ($safe_flatten_missing.Count -eq 0)

# --- PAPER-FULL-BUY-SELL-LIFECYCLE-CLOSED ----------------------------------
# A full round trip OBSERVED within this evidence window: a buy/long order
# AND a sell/close order, both submitted-acked-filled-applied, ending flat.
# This is intentionally NOT the same gate as NATURAL-TRADE-LIFECYCLE-CLOSED:
# that gate is satisfied by a PRE-EXISTING position being sold (no buy
# required). Proving the *full* buy->sell cycle requires the buy leg to be
# directly observed in this evidence, not inherited from a prior baseline.
$buy_order_detected = $false
foreach ($rowSet in @(
    @(if ($null -ne $ExecutionOrders -and $null -ne $ExecutionOrders.rows) { $ExecutionOrders.rows } else { @() }),
    @(if ($null -ne $ExecutionOutbox -and $null -ne $ExecutionOutbox.rows) { $ExecutionOutbox.rows } else { @() }),
    @(if ($null -ne $ExecutionFlow   -and $null -ne $ExecutionFlow.rows)   { $ExecutionFlow.rows }   else { @() })
)) {
    foreach ($row in @($rowSet)) {
        if (Test-RowFieldMatch $row @('side', 'order_side', 'action', 'intent', 'message', 'stage') '(?i)\b(buy|long)\b') {
            $buy_order_detected = $true; break
        }
    }
    if ($buy_order_detected) { break }
}

$full_cycle_missing_requirements = [System.Collections.Generic.List[string]]::new()
if (-not $api_files_present)          { $full_cycle_missing_requirements.Add('api_files_present') }
if (-not $runtime_reached_running)    { $full_cycle_missing_requirements.Add('runtime_reached_running=true') }
if (-not $buy_order_detected)         { $full_cycle_missing_requirements.Add('buy_or_long_order_detected_in_evidence') }
if (-not $sell_order_detected)        { $full_cycle_missing_requirements.Add('sell_side_or_flatten_order_detected') }
if (-not $has_order_submit)           { $full_cycle_missing_requirements.Add('has_order_submit') }
if (-not $has_order_ack)              { $full_cycle_missing_requirements.Add('has_order_ack') }
if (-not $has_fill)                   { $full_cycle_missing_requirements.Add('has_fill') }
if (-not $has_inbox_apply)            { $full_cycle_missing_requirements.Add('has_inbox_apply_or_durable_applied_state') }
if (-not $portfolio_positions_snapshot_present) { $full_cycle_missing_requirements.Add('portfolio_positions_snapshot_present') }
if (-not $final_position_flat)        { $full_cycle_missing_requirements.Add('final_position_flat') }
if (-not $broker_order_map_present)   { $full_cycle_missing_requirements.Add('broker_order_map_present') }
if (-not $reconcile_clean)            { $full_cycle_missing_requirements.Add('reconcile_clean') }
if (-not $live_routing_disabled)      { $full_cycle_missing_requirements.Add('live_routing_enabled=false') }
if ($kill_switch_active -eq $true)    { $full_cycle_missing_requirements.Add('kill_switch_active=false') }
if ($integrity_halt_active -eq $true) { $full_cycle_missing_requirements.Add('integrity_halt_active=false') }
if ($risk_halt_active -eq $true)      { $full_cycle_missing_requirements.Add('risk_halt_active=false') }
if (-not $no_unsafe_markers)          { $full_cycle_missing_requirements.Add('no_unsafe_evidence_markers') }
$full_buy_sell_cycle_proven = ($full_cycle_missing_requirements.Count -eq 0)

# --- GUI-TRADE-LIFECYCLE-VISIBILITY-CLOSED ---------------------------------
# Requires the strict trade lifecycle gate PLUS an explicit, affirmative
# operator note in notes/gui_observation.txt naming a screenshot AND
# confirming a filled order was visibly observed in the GUI. File presence
# alone is not proof -- the file is created as a template by the capture
# script; only explicit affirmative content (with no "not yet" / TODO /
# placeholder language) counts.
$gui_observation_confirmed = $false
$guiObsFile = Join-Path $EvidencePath 'notes\gui_observation.txt'
if (Test-Path $guiObsFile) {
    $guiContent = Get-Content $guiObsFile -Raw -ErrorAction SilentlyContinue
    if ($guiContent -and
        $guiContent -match '(?i)\bscreenshot\b' -and
        $guiContent -match '(?i)\b(filled order|order fill|fill)\b' -and
        $guiContent -match '(?i)\b(visible|confirmed|observed|verified)\b' -and
        $guiContent -notmatch '(?i)\bnot\s+(yet\s+)?(observed|confirmed|visible|captured|taken)\b' -and
        $guiContent -notmatch '(?i)\b(TODO|TBD|pending|placeholder|fill[_ -]?in|template)\b') {
        $gui_observation_confirmed = $true
    }
}
$gui_visibility_missing = [System.Collections.Generic.List[string]]::new()
if (-not $trade_lifecycle_detected)   { $gui_visibility_missing.Add('strict_trade_lifecycle_proven (see lifecycle_missing_requirements)') }
if (-not $gui_observation_confirmed)  { $gui_visibility_missing.Add('notes/gui_observation.txt explicit affirmative confirmation (screenshot + filled order + visible/confirmed, no placeholder language)') }
$gui_visibility_proven = ($gui_visibility_missing.Count -eq 0)

# --- DISCORD-TRADE-LIFECYCLE-REAL-CLOSED -----------------------------------
# Requires the strict trade lifecycle gate PLUS an explicit, affirmative
# operator note in notes/discord_observation.txt naming all 7 lifecycle stage
# keys (operator_control_surface.md section 8) and explicitly confirming all
# were observed -- not file presence alone, and not a partial subset.
$DISCORD_STAGE_KEYS_REQUIRED = @(
    'autonomous.run.start', 'signal.admitted', 'order.submitted',
    'order.acked', 'flatten.requested', 'reconcile.clean'
)
$discord_observation_confirmed = $false
$discordObsFile = Join-Path $EvidencePath 'notes\discord_observation.txt'
if (Test-Path $discordObsFile) {
    $discordContent = Get-Content $discordObsFile -Raw -ErrorAction SilentlyContinue
    if ($discordContent) {
        $allStagesPresent = $true
        foreach ($stageKey in $DISCORD_STAGE_KEYS_REQUIRED) {
            if ($discordContent -notmatch [regex]::Escape($stageKey)) { $allStagesPresent = $false }
        }
        $fillStagePresent = ($discordContent -match [regex]::Escape('fill.terminal') -or $discordContent -match [regex]::Escape('fill.partial'))
        $explicitAllConfirmed = (
            $discordContent -match '(?i)\ball\s*7\b.*\b(confirmed|observed|present|received)\b' -or
            $discordContent -match '(?i)\b(confirmed|observed|present|received)\b.*\ball\s*7\b' -or
            $discordContent -match '(?i)\ball\s+(seven|stage)s?\s+(messages?\s+)?(confirmed|observed|present|received)\b'
        )
        $negativeMarkers = ($discordContent -match '(?i)\b(not\s+(yet\s+)?(observed|confirmed|present|received)|missing|pending|TODO|TBD|placeholder)\b')
        if ($allStagesPresent -and $fillStagePresent -and $explicitAllConfirmed -and -not $negativeMarkers) {
            $discord_observation_confirmed = $true
        }
    }
}
$discord_lifecycle_missing = [System.Collections.Generic.List[string]]::new()
if (-not $trade_lifecycle_detected)        { $discord_lifecycle_missing.Add('strict_trade_lifecycle_proven (see lifecycle_missing_requirements)') }
if (-not $discord_observation_confirmed)   { $discord_lifecycle_missing.Add('notes/discord_observation.txt explicit affirmative confirmation of all 7 stage messages (no placeholder language)') }
$discord_lifecycle_proven = ($discord_lifecycle_missing.Count -eq 0)

# --- REPEATED-AUTONOMOUS-TRADE-CYCLE-CLOSED --------------------------------
# A single evidence-folder review cannot, by itself, observe two consecutive
# sessions -- that is inherently cross-session proof. This label therefore
# requires the strict trade lifecycle gate PLUS an explicit, affirmative,
# structured operator note in notes/repeated_cycle_confirmation.txt that
# names two distinct sessions and confirms each started AND stopped cleanly.
# Absent that artifact, this label is withheld -- it is expected to remain
# withheld for the overwhelming majority of evidence pulls, by design.
$repeated_cycle_confirmed = $false
$repeatedCycleFile = Join-Path $EvidencePath 'notes\repeated_cycle_confirmation.txt'
if (Test-Path $repeatedCycleFile) {
    $rcContent = Get-Content $repeatedCycleFile -Raw -ErrorAction SilentlyContinue
    if ($rcContent) {
        $sessionRefCount     = (@([regex]::Matches($rcContent, '(?i)\bsession\s*(#|number)?\s*[12]\b') |
                                  ForEach-Object { $_.Value.ToLower() -replace '\s+', ' ' } | Select-Object -Unique)).Count
        $startedCleanlyCount = (@([regex]::Matches($rcContent, '(?i)\bstart(ed)?\s+cleanly\b'))).Count
        $stoppedCleanlyCount = (@([regex]::Matches($rcContent, '(?i)\bstop(ped)?\s+cleanly\b'))).Count
        $explicitTwoSessions = (
            $rcContent -match '(?i)\b(two|2)\s+consecutive\s+sessions?\b.*\b(confirmed|proven|verified)\b' -or
            $rcContent -match '(?i)\b(confirmed|proven|verified)\b.*\b(two|2)\s+consecutive\s+sessions?\b'
        )
        $negativeMarkers = (
            $rcContent -match '(?i)\b(not\s+(yet\s+)?(observed|confirmed|proven)|missing|pending|TODO|TBD|placeholder|only\s+(one|1)\s+session)\b'
        )
        if ($sessionRefCount -ge 2 -and $startedCleanlyCount -ge 2 -and $stoppedCleanlyCount -ge 2 -and
            $explicitTwoSessions -and -not $negativeMarkers) {
            $repeated_cycle_confirmed = $true
        }
    }
}
$repeated_cycle_missing = [System.Collections.Generic.List[string]]::new()
if (-not $trade_lifecycle_detected)    { $repeated_cycle_missing.Add('strict_trade_lifecycle_proven (see lifecycle_missing_requirements)') }
if (-not $repeated_cycle_confirmed)    { $repeated_cycle_missing.Add('notes/repeated_cycle_confirmation.txt explicit confirmation naming two distinct sessions, each started AND stopped cleanly (cross-session proof; rarely satisfiable from a single evidence pull)') }
$repeated_cycle_proven = ($repeated_cycle_missing.Count -eq 0)

# --- Compose the ledger verdict table --------------------------------------
$ledger_verdicts = [ordered]@{
    aapl_natural_sell_lifecycle     = New-LedgerVerdict -ClosedLabel 'AAPL-NATURAL-SELL-LIFECYCLE-CLOSED'        -Closed $aapl_sell_proven           -Missing $aapl_sell_missing
    paper_safe_flatten_route        = New-LedgerVerdict -ClosedLabel 'PAPER-SAFE-FLATTEN-ROUTE-MARKET-PROOF-CLOSED' -Closed $safe_flatten_proven      -Missing $safe_flatten_missing
    paper_full_buy_sell_lifecycle   = New-LedgerVerdict -ClosedLabel 'PAPER-FULL-BUY-SELL-LIFECYCLE-CLOSED'      -Closed $full_buy_sell_cycle_proven  -Missing $full_cycle_missing_requirements
    gui_trade_lifecycle_visibility  = New-LedgerVerdict -ClosedLabel 'GUI-TRADE-LIFECYCLE-VISIBILITY-CLOSED'     -Closed $gui_visibility_proven       -Missing $gui_visibility_missing
    discord_trade_lifecycle_real    = New-LedgerVerdict -ClosedLabel 'DISCORD-TRADE-LIFECYCLE-REAL-CLOSED'       -Closed $discord_lifecycle_proven    -Missing $discord_lifecycle_missing
    repeated_autonomous_trade_cycle = New-LedgerVerdict -ClosedLabel 'REPEATED-AUTONOMOUS-TRADE-CYCLE-CLOSED'    -Closed $repeated_cycle_proven       -Missing $repeated_cycle_missing
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
$summaryLines.Add("## Source of Truth")
$summaryLines.Add("- This file (review_summary.md / review_summary.json) is the generated")
$summaryLines.Add("  evidence-review source of truth.")
$summaryLines.Add("- notes/final_verdict.txt is an operator-completed note/template only --")
$summaryLines.Add("  it is NOT authoritative and does not override the VERDICT below.")
$summaryLines.Add("- Do not treat a smoke as PASSED unless VERDICT is")
$summaryLines.Add("  NATURAL-TRADE-LIFECYCLE-CLOSED or READINESS-CLOSED-NO-TRADE.")
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
$summaryLines.Add("## Autonomous Paper Status (PAPER-SMOKE-AUTOMATION-BUNDLE-01)")
$summaryLines.Add("- autonomous_paper_status_present:  $ps_status_present")
$summaryLines.Add("- readiness_classification:         $ps_readiness_classification")
$summaryLines.Add("- next_operator_action:             $ps_next_operator_action")
$summaryLines.Add("- flatten_available:                $ps_flatten_available")
$summaryLines.Add("- flatten_blockers:                 $ps_flatten_blockers_str")
$summaryLines.Add("- current_position_qty:             $ps_current_position_qty")
$summaryLines.Add("- target_qty:                       $ps_target_qty")
$summaryLines.Add("- computed_delta_qty:               $ps_computed_delta_qty")
$summaryLines.Add("- no_order_reason:                  $ps_no_order_reason")
$summaryLines.Add("")
$summaryLines.Add("## Trade Lifecycle (EVIDENCE-CAPTURE-TRADE-FLOW-01)")
$summaryLines.Add("- trade_lifecycle_detected:     $trade_lifecycle_detected")
$summaryLines.Add("- has_order_submit:             $has_order_submit")
$summaryLines.Add("- sell_order_detected:          $sell_order_detected")
$summaryLines.Add("- has_order_ack:                $has_order_ack")
$summaryLines.Add("- has_fill:                     $has_fill")
$summaryLines.Add("- has_inbox_apply:              $has_inbox_apply")
$summaryLines.Add("- final_position_flat:          $final_position_flat")
$summaryLines.Add("- live_routing_disabled:        $live_routing_disabled")
$summaryLines.Add("- no_unsafe_markers:            $no_unsafe_markers")
$summaryLines.Add("- existing_long_detected:       $existing_long_position_detected")
$summaryLines.Add("- prior_baseline_present:       $prior_position_baseline_present")
$summaryLines.Add("- baseline_or_long_present:     $existing_long_or_prior_baseline_present")
$summaryLines.Add("- order_submitted:              $order_submitted")
$summaryLines.Add("- ack_detected:                 $ack_detected")
$summaryLines.Add("- fill_detected:                $fill_detected")
$summaryLines.Add("- inbox_apply_detected:         $inbox_apply_detected")
$summaryLines.Add("- oms_inbox_snapshot_present:   $oms_inbox_snapshot_present")
$summaryLines.Add("- oms_inbox_ack_present:        $oms_inbox_ack_present")
$summaryLines.Add("- oms_inbox_apply_present:      $oms_inbox_apply_present")
$summaryLines.Add("- position_nonzero:             $position_nonzero")
$summaryLines.Add("- position_flat:                $position_flat")
$summaryLines.Add("- positions_snapshot_present:   $portfolio_positions_snapshot_present")
$summaryLines.Add("- current_position_qty:         $current_position_qty")
$summaryLines.Add("- aapl_position_qty:            $aapl_position_qty")
$summaryLines.Add("- broker_order_map_present:     $broker_order_map_present")
$summaryLines.Add("- reconcile_clean:              $reconcile_clean")
$summaryLines.Add("- direct_broker_shortcut:       $direct_broker_shortcut_detected")
$summaryLines.Add("- fake_fill_marker:             $fake_fill_detected")
$summaryLines.Add("- fake_bar_marker:              $fake_bar_detected")
$summaryLines.Add("- signal_injection_marker:      $signal_injection_detected")
$summaryLines.Add("- manual_db_mutation_marker:    $manual_db_mutation_detected")
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
$summaryLines.Add("## Lifecycle Missing Requirements")
if ($lifecycle_missing_requirements.Count -gt 0) {
    foreach ($missing in $lifecycle_missing_requirements) { $summaryLines.Add("- $missing") }
} else {
    $summaryLines.Add("- (none)")
}
$summaryLines.Add("")
$summaryLines.Add("## Ledger-Specific Classifications (PAPER-SMOKE-LEDGER-SPECIFIC-CLASSIFIER-01)")
$summaryLines.Add("# Emitted IN ADDITION to VERDICT above. Each *-CLOSED label below requires its")
$summaryLines.Add("# own independently-strict, fail-closed proof -- never weakened from 0b0bc93.")
$summaryLines.Add("# Withheld items show status (MARKET-PROOF-PENDING / PARTIAL / OPEN) plus the")
$summaryLines.Add("# specific missing requirements that keep them from closing.")
foreach ($ledgerKey in $ledger_verdicts.Keys) {
    $lv = $ledger_verdicts[$ledgerKey]
    $summaryLines.Add("")
    $summaryLines.Add("### $($lv.label)")
    $summaryLines.Add("- status: $($lv.status)")
    if ($lv.closed) {
        $summaryLines.Add("- all strict requirements satisfied")
    } else {
        $summaryLines.Add("- missing requirements:")
        foreach ($m in $lv.missing_requirements) { $summaryLines.Add("  - $m") }
    }
}
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
    $summaryLines.Add("- NOTE: This is an operator note only. review_summary.json/.md (this")
    $summaryLines.Add("  document) remains the source of truth, not notes/final_verdict.txt.")
    if ($manual_verdict_conflict) {
        $summaryLines.Add("- WARNING: Operator checked SMOKE PASSED but VERDICT is $classification.")
        $summaryLines.Add("  Do NOT record this smoke as passed. Trust VERDICT above, not this note.")
    }
    $summaryLines.Add("")
}

if ($SecretWarnings.Count -gt 0) {
    $summaryLines.Add("## SECRET SCAN WARNINGS")
    foreach ($w in $SecretWarnings) { $summaryLines.Add("- WARNING: $w") }
    $summaryLines.Add("- ACTION REQUIRED: Do not share this evidence bundle until secrets are removed.")
    $summaryLines.Add("")
}

$summaryLines.Add("## Classification Reference")
$summaryLines.Add("- NATURAL-TRADE-LIFECYCLE-CLOSED  Strict lifecycle: prior long/baseline, sell submit, ACK, fill, apply, final flat, broker map, reconcile clean, live routing off, no unsafe markers")
$summaryLines.Add("- READINESS-CLOSED-NO-TRADE       Running, bars loaded, no trade signal/order/fill, reconcile clean, live routing off, no fault")
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

if ($manual_verdict_conflict) {
    Write-Host '*** MANUAL VERDICT CONFLICT ***' -ForegroundColor Magenta
    Write-Host "  notes/final_verdict.txt has [x] SMOKE PASSED checked, but VERDICT is $classification." -ForegroundColor Magenta
    Write-Host '  review_summary.json/.md (this output) is the source of truth -- do NOT record this smoke as passed.' -ForegroundColor Magenta
    Write-Host ''
}

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
    manual_verdict_conflict     = $manual_verdict_conflict
    trade_lifecycle_detected    = $trade_lifecycle_detected
    lifecycle_missing_requirements = @($lifecycle_missing_requirements)
    has_order_submit          = $has_order_submit
    sell_order_detected       = $sell_order_detected
    has_order_ack             = $has_order_ack
    has_fill                  = $has_fill
    has_inbox_apply           = $has_inbox_apply
    final_position_flat       = $final_position_flat
    live_routing_disabled     = $live_routing_disabled
    no_unsafe_markers         = $no_unsafe_markers
    existing_long_position_detected = $existing_long_position_detected
    prior_position_baseline_present = $prior_position_baseline_present
    existing_long_or_prior_baseline_present = $existing_long_or_prior_baseline_present
    order_submitted             = $order_submitted
    ack_detected                = $ack_detected
    fill_detected               = $fill_detected
    inbox_apply_detected        = $inbox_apply_detected
    oms_inbox_snapshot_present  = $oms_inbox_snapshot_present
    oms_inbox_ack_present       = $oms_inbox_ack_present
    oms_inbox_apply_present     = $oms_inbox_apply_present
    position_nonzero            = $position_nonzero
    position_flat               = $position_flat
    portfolio_positions_snapshot_present = $portfolio_positions_snapshot_present
    current_position_qty        = $current_position_qty
    aapl_position_qty           = $aapl_position_qty
    broker_order_map_present    = $broker_order_map_present
    reconcile_clean             = $reconcile_clean
    direct_broker_shortcut_detected = $direct_broker_shortcut_detected
    fake_fill_detected          = $fake_fill_detected
    fake_bar_detected           = $fake_bar_detected
    signal_injection_detected   = $signal_injection_detected
    manual_db_mutation_detected = $manual_db_mutation_detected
    unsafe_marker_hits          = @($unsafe_marker_hits)
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
    autonomous_paper_status_present  = $ps_status_present
    autonomous_readiness_classification = $ps_readiness_classification
    autonomous_next_operator_action  = $ps_next_operator_action
    autonomous_flatten_available     = $ps_flatten_available
    autonomous_flatten_blockers      = $ps_flatten_blockers_str
    autonomous_current_position_qty  = $ps_current_position_qty
    autonomous_target_qty            = $ps_target_qty
    autonomous_computed_delta_qty    = $ps_computed_delta_qty
    autonomous_no_order_reason       = $ps_no_order_reason
    ledger_classifications      = $ledger_verdicts
    ledger_closed_labels        = @($ledger_verdicts.Values | Where-Object { $_.closed } | ForEach-Object { $_.label })
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
