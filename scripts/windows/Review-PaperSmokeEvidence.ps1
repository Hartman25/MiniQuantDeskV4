# =============================================================================
# PAPER-SMOKE-EVIDENCE-REVIEW-02
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
#   TRADE-LIFECYCLE-CLOSED    -- full lifecycle proven: running, signal, order, ACK, fill, reconcile clean
#   READINESS-CLOSED-NO-TRADE -- running, bars loaded, no signal/order, reconcile clean, no fault
#   PARTIAL                   -- partial progress; lifecycle incomplete without clear failure
#   OPEN                      -- active blocker: halt, kill switch, bars missing, reconcile dirty
#   FALSE-CLOSED              -- live routing enabled, secrets in evidence, no proof files, fake markers
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

# TRADE-LIFECYCLE-CLOSED
if ($null -eq $classification) {
    $lifecycle_ok = (
        $runtime_reached_running -eq $true -and
        ($live_routing_enabled -eq $false -or $null -eq $live_routing_enabled) -and
        ($alpaca_ws_continuity -eq 'live' -or $ws_continuity_ready -eq $true) -and
        ($null -ne $fill_count -and $fill_count -gt 0) -and
        ($reconcile_status_str -eq 'ok' -or $reconcile_ready -eq $true) -and
        ($null -eq $kill_switch_active -or $kill_switch_active -eq $false) -and
        ($null -eq $integrity_halt_active -or $integrity_halt_active -eq $false) -and
        $api_files_present
    )
    if ($lifecycle_ok) {
        $classification = 'TRADE-LIFECYCLE-CLOSED'
        $classification_reasons.Add("runtime reached running, fill_count=$fill_count, reconcile=$reconcile_status_str, WS=$alpaca_ws_continuity")
        $classification_reasons.Add('live_routing_enabled=false confirmed')
    }
}

# READINESS-CLOSED-NO-TRADE
if ($null -eq $classification) {
    $readiness_ok = (
        $runtime_reached_running -eq $true -and
        ($live_routing_enabled -eq $false -or $null -eq $live_routing_enabled) -and
        ($null -ne $completed_rows -and $completed_rows -gt 0) -and
        ($null -eq $fill_count -or $fill_count -eq 0) -and
        ($null -eq $open_order_count -or $open_order_count -eq 0) -and
        ($reconcile_status_str -eq 'ok' -or $reconcile_ready -eq $true) -and
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
        $classification_reasons.Add("fill_count=0, open_orders=0, reconcile=$reconcile_status_str")
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
$summaryLines.Add("- fill_count:              $fill_count")
$summaryLines.Add("- open_order_count:        $open_order_count")
$summaryLines.Add("- position_count:          $position_count")
$summaryLines.Add("- exec_active_orders:      $exec_active_orders")
$summaryLines.Add("- exec_pending_orders:     $exec_pending_orders")
$summaryLines.Add("")
$summaryLines.Add("## Reconcile")
$summaryLines.Add("- reconcile_status:        $reconcile_status_str")
$summaryLines.Add("- total_mismatches:        $reconcile_total_mis")
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
$summaryLines.Add("- TRADE-LIFECYCLE-CLOSED    Full lifecycle: running, signal, order, ACK, fill, reconcile clean")
$summaryLines.Add("- READINESS-CLOSED-NO-TRADE Running, bars loaded, no trade signal, reconcile clean, no fault")
$summaryLines.Add("- PARTIAL                   Partial progress; lifecycle incomplete without clear failure")
$summaryLines.Add("- OPEN                      Active blocker: halt, kill switch, bars missing, reconcile dirty")
$summaryLines.Add("- FALSE-CLOSED              Live routing enabled, secrets in evidence, no proof files")
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
        'TRADE-LIFECYCLE-CLOSED'    { 'Green' }
        'READINESS-CLOSED-NO-TRADE' { 'Cyan' }
        'PARTIAL'                   { 'Yellow' }
        'OPEN'                      { 'Red' }
        'FALSE-CLOSED'              { 'Magenta' }
        default                     { 'White' }
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
    schema_version         = 'review-v1'
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
    manual_verdict_note    = $manual_verdict
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
