# =============================================================================
# PAPER-SMOKE-EVIDENCE-01
# Capture-PaperSmokeEvidence.ps1
#
# Read-only evidence capture for MiniQuantDesk V4 Paper + Alpaca smoke runs.
# Creates a durable evidence folder under evidence/paper_smoke_<YYYYMMDD_HHMMSS>/
#
# Safety rules enforced by this script:
#   - Never prints ALPACA_API_KEY*, ALPACA_API_SECRET*, MQK_OPERATOR_TOKEN,
#     DB password, Discord webhook, or TwelveData key.
#   - Never mutates the DB (SELECT-only via psql).
#   - Never starts, stops, or modifies a trading run.
#   - Never calls external broker endpoints.
#   - All daemon calls are localhost read-only GETs.
#   - Handles offline daemon, offline DB, and missing tools gracefully.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label pre_smoke
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label post_smoke -DaemonPort 8899
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Capture-PaperSmokeEvidence.ps1 -Label dry_run_offline
#
# Parameters:
#   -Label        Short label appended to folder name (e.g. pre_smoke, post_smoke).
#                 Default: "capture".
#   -RepoRoot     Repo root. Default: two levels up from this script.
#   -DaemonPort   Daemon HTTP port. Default: 8899.
#   -SkipDb       Skip DB snapshots even if MQK_DATABASE_URL is set.
#   -SkipDaemon   Skip daemon API snapshots even if daemon appears reachable.
#
# Output:
#   evidence/paper_smoke_<YYYYMMDD_HHMMSS>_<Label>/
#     summary.md          -- human-readable evidence checklist
#     git_state.txt       -- git status + log
#     proof_tail.txt      -- last 100 lines of .proof\full_repo_proof_output.txt
#     api/                -- JSON snapshots from localhost daemon (if reachable)
#       system_status.json
#       system_preflight.json
#       autonomous_readiness.json
#       alerts_active.json
#       events_feed.json
#       oms_overview.json
#       risk_summary.json
#       reconcile_status.json
#     db/                 -- SELECT-only DB snapshots (if DB reachable)
#       runs_recent.txt
#       oms_outbox_recent.txt
#       oms_inbox_recent.txt
#       broker_order_map_recent.txt
#       fill_quality_recent.txt
#     notes/              -- placeholder files for operator notes
#       discord_observation.txt
#       gui_observation.txt
#       smoke_lifecycle_checklist.txt
#       final_verdict.txt
# =============================================================================

[CmdletBinding()]
param(
    [string]$Label      = 'capture',
    [string]$RepoRoot   = '',
    [int]   $DaemonPort = 8899,
    [switch]$SkipDb,
    [switch]$SkipDaemon
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Resolve repo root
# ---------------------------------------------------------------------------
if ($RepoRoot -eq '') {
    $ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
    $RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
}

# ---------------------------------------------------------------------------
# Create evidence folder
# ---------------------------------------------------------------------------
$Timestamp  = (Get-Date -Format 'yyyyMMdd_HHmmss')
$FolderName = "paper_smoke_${Timestamp}_${Label}"
$EvidenceDir = Join-Path $RepoRoot "evidence\$FolderName"

New-Item -ItemType Directory -Force -Path $EvidenceDir | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $EvidenceDir 'api')   | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $EvidenceDir 'db')    | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $EvidenceDir 'notes') | Out-Null

$DaemonBase = "http://127.0.0.1:${DaemonPort}"

Write-Host ""
Write-Host "=== Capture-PaperSmokeEvidence.ps1 (PAPER-SMOKE-EVIDENCE-01) ===" -ForegroundColor Cyan
Write-Host "    Label     : $Label"
Write-Host "    Folder    : $EvidenceDir"
Write-Host "    Daemon    : $DaemonBase"
Write-Host "    Time (UTC): $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') UTC"
Write-Host ""

# ---------------------------------------------------------------------------
# Helper: safe invoke-restmethod (returns $null on any error)
# ---------------------------------------------------------------------------
function Invoke-DaemonGet {
    param([string]$Path)
    try {
        $resp = Invoke-RestMethod -Uri "${DaemonBase}${Path}" `
            -Method Get -TimeoutSec 5 -ErrorAction Stop
        return $resp
    } catch {
        return $null
    }
}

function Save-DaemonJson {
    param([string]$Path, [string]$FileName)
    $data = Invoke-DaemonGet -Path $Path
    $dest = Join-Path $EvidenceDir "api\$FileName"
    if ($null -ne $data) {
        $data | ConvertTo-Json -Depth 20 | Set-Content -Path $dest -Encoding UTF8
        Write-Host "  [API OK ] $Path --> api\$FileName" -ForegroundColor Green
        return $true
    } else {
        "UNAVAILABLE: daemon did not respond to GET $Path" | Set-Content -Path $dest -Encoding UTF8
        Write-Host "  [API --] $Path unavailable" -ForegroundColor Yellow
        return $false
    }
}

# ---------------------------------------------------------------------------
# 1. Git state
# ---------------------------------------------------------------------------
Write-Host "-- 1. Git state --"
$gitState = @()
$gitState += "=== git status ==="
try   { $gitState += (& git -C $RepoRoot status --short --untracked-files=all 2>&1) }
catch { $gitState += "ERROR: git status failed: $_" }
$gitState += ""
$gitState += "=== git log (last 10) ==="
try   { $gitState += (& git -C $RepoRoot log --oneline -10 2>&1) }
catch { $gitState += "ERROR: git log failed: $_" }
$gitState += ""
$gitState += "=== git commit (HEAD) ==="
try   { $gitState += (& git -C $RepoRoot rev-parse HEAD 2>&1) }
catch { $gitState += "ERROR: git rev-parse failed: $_" }
$gitState | Set-Content -Path (Join-Path $EvidenceDir 'git_state.txt') -Encoding UTF8
Write-Host "  [OK] git_state.txt"

# ---------------------------------------------------------------------------
# 2. Proof tail
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- 2. Proof output tail --"
$ProofFile = Join-Path $RepoRoot '.proof\full_repo_proof_output.txt'
$proofDest = Join-Path $EvidenceDir 'proof_tail.txt'
if (Test-Path $ProofFile) {
    Get-Content -Path $ProofFile -Tail 100 | Set-Content -Path $proofDest -Encoding UTF8
    Write-Host "  [OK] proof_tail.txt (last 100 lines)"
} else {
    "UNAVAILABLE: .proof\full_repo_proof_output.txt not found" | Set-Content -Path $proofDest -Encoding UTF8
    Write-Host "  [--] proof_tail.txt not found" -ForegroundColor Yellow
}

# ---------------------------------------------------------------------------
# 3. Daemon API snapshots
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- 3. Daemon API snapshots --"
$daemonReachable = $false
if (-not $SkipDaemon) {
    $healthCheck = Invoke-DaemonGet -Path '/v1/health'
    if ($null -ne $healthCheck) {
        $daemonReachable = $true
        Write-Host "  [OK] daemon reachable at $DaemonBase" -ForegroundColor Green
    } else {
        Write-Host "  [--] daemon not reachable -- API snapshots will be marked UNAVAILABLE" -ForegroundColor Yellow
    }

    $null = Save-DaemonJson '/api/v1/system/status'        'system_status.json'
    $null = Save-DaemonJson '/api/v1/system/preflight'     'system_preflight.json'
    $null = Save-DaemonJson '/api/v1/autonomous/readiness' 'autonomous_readiness.json'
    $null = Save-DaemonJson '/api/v1/alerts/active'        'alerts_active.json'
    $null = Save-DaemonJson '/api/v1/events/feed'          'events_feed.json'
    $null = Save-DaemonJson '/api/v1/oms/overview'         'oms_overview.json'
    $null = Save-DaemonJson '/api/v1/risk/summary'         'risk_summary.json'
    $null = Save-DaemonJson '/api/v1/reconcile/status'     'reconcile_status.json'
} else {
    "SKIPPED: -SkipDaemon flag was set" | Set-Content -Path (Join-Path $EvidenceDir 'api\skipped.txt') -Encoding UTF8
    Write-Host "  [--] daemon snapshots skipped (-SkipDaemon)" -ForegroundColor Yellow
}

# ---------------------------------------------------------------------------
# 4. DB snapshots (SELECT-only via psql)
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- 4. DB snapshots (SELECT-only) --"
$dbUrl = $env:MQK_DATABASE_URL
if ($SkipDb) {
    "SKIPPED: -SkipDb flag was set" | Set-Content -Path (Join-Path $EvidenceDir 'db\skipped.txt') -Encoding UTF8
    Write-Host "  [--] DB snapshots skipped (-SkipDb)" -ForegroundColor Yellow
} elseif ([string]::IsNullOrEmpty($dbUrl)) {
    "UNAVAILABLE: MQK_DATABASE_URL not set in environment" | Set-Content -Path (Join-Path $EvidenceDir 'db\unavailable.txt') -Encoding UTF8
    Write-Host "  [--] MQK_DATABASE_URL not set -- DB snapshots skipped" -ForegroundColor Yellow
} else {
    # Never print the URL (may contain password). Just report presence.
    Write-Host "  [OK] MQK_DATABASE_URL is set (not printed)" -ForegroundColor Green

    function Save-DbQuery {
        param([string]$FileName, [string]$Query)
        $dest = Join-Path $EvidenceDir "db\$FileName"
        try {
            $result = & psql $dbUrl -c $Query --no-password -X 2>&1
            $result | Set-Content -Path $dest -Encoding UTF8
            Write-Host "  [DB OK] $FileName" -ForegroundColor Green
        } catch {
            "UNAVAILABLE: psql error: $_" | Set-Content -Path $dest -Encoding UTF8
            Write-Host "  [DB --] $FileName -- psql error" -ForegroundColor Yellow
        }
    }

    Save-DbQuery 'runs_recent.txt' `
        "SELECT id, run_state, started_at_utc, stopped_at_utc, created_at FROM runs ORDER BY created_at DESC LIMIT 10;"
    Save-DbQuery 'oms_outbox_recent.txt' `
        "SELECT id, order_id, symbol, side, status, submitted_at, created_at FROM oms_outbox ORDER BY created_at DESC LIMIT 20;"
    Save-DbQuery 'oms_inbox_recent.txt' `
        "SELECT id, order_id, event_type, applied, created_at FROM oms_inbox ORDER BY created_at DESC LIMIT 20;"
    Save-DbQuery 'broker_order_map_recent.txt' `
        "SELECT order_id, broker_order_id, created_at FROM broker_order_map ORDER BY created_at DESC LIMIT 20;"
    Save-DbQuery 'fill_quality_recent.txt' `
        "SELECT id, order_id, symbol, fill_qty, fill_price_micros, slippage_micros, created_at FROM fill_quality_telemetry ORDER BY created_at DESC LIMIT 20;"
}

# ---------------------------------------------------------------------------
# 5. Operator note placeholders
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- 5. Operator note placeholders --"

$discordNote = @"
# Discord Observation Notes
# Fill in after the smoke run.
#
# Signal produced:           YES / NO
# Order submitted:           YES / NO
# Alpaca paper ACK received: YES / NO
# Partial fill observed:     YES / NO
# Final fill observed:       YES / NO
# OMS terminal state:        YES / NO
# Portfolio updated:         YES / NO
# Reconcile clean:           YES / NO
# Discord trade alert fired: YES / NO
#
# Notes:
"@
$discordNote | Set-Content -Path (Join-Path $EvidenceDir 'notes\discord_observation.txt') -Encoding UTF8

$guiNote = @"
# GUI Observation Notes
# Fill in after the smoke run.
#
# GUI launched:              YES / NO
# Runtime status = running:  YES / NO
# WS continuity = live:      YES / NO
# OMS order appeared:        YES / NO
# OMS fill appeared:         YES / NO
# GUI matched backend:       YES / NO
# Screenshot path:           (add path here)
#
# Notes:
"@
$guiNote | Set-Content -Path (Join-Path $EvidenceDir 'notes\gui_observation.txt') -Encoding UTF8

$lifecycleChecklist = @"
# Smoke Lifecycle Checklist
# Mark each YES / NO / N/A during or after the smoke.
#
# PRE-SMOKE
# [ ] Daemon started and healthy (/v1/health = ok)
# [ ] WS continuity = live (autonomous/readiness ws_continuity)
# [ ] overall_ready = true
# [ ] reconcile_ready = true
# [ ] arm_state = armed or arm_pending
# [ ] signal_ingestion_configured = true
# [ ] No active alerts (alerts/active fault_signals empty)
# [ ] Start-PaperTradingSmoke.ps1 -CheckOnly passed
#
# SMOKE COMMAND (fill in exact command used):
#   Command: powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 ...
#   WatchSeconds:
#   MQK_STRATEGY_SYMBOL:
#   MQK_STRATEGY_IDS:
#   Start time (UTC):
#   End time (UTC):
#
# LIFECYCLE
# [ ] Runtime started (runtime_status = running)
# [ ] Signal POST accepted (intent_placed = true)
# [ ] Outbox row written
# [ ] Broker submit succeeded (broker_order_map row)
# [ ] ACK received from Alpaca (oms_inbox ack event)
# [ ] Partial fill received (if applicable)
# [ ] Final fill received
# [ ] OMS terminal state (filled)
# [ ] Portfolio state updated
# [ ] Reconcile clean after fill
# [ ] Discord alert fired
# [ ] GUI showed filled order
# [ ] Runtime stopped cleanly
# [ ] No halt triggered
#
# FINAL VERDICT:
#   [ ] SMOKE PASSED -- full lifecycle end-to-end, reconcile clean, Discord alert, GUI matched
#   [ ] SMOKE PARTIAL -- lifecycle partial (describe below)
#   [ ] SMOKE FAILED -- describe below
#
# Notes:
"@
$lifecycleChecklist | Set-Content -Path (Join-Path $EvidenceDir 'notes\smoke_lifecycle_checklist.txt') -Encoding UTF8

$verdictNote = @"
# Final Verdict
# Complete after reviewing all evidence.
#
# Date (UTC):
# Smoke label:    $Label
# Git commit:
# Proof state:
#
# Verdict:
#   SMOKE PASSED       -- full lifecycle proven, reconcile clean, evidence durable
#   SMOKE PARTIAL      -- partial lifecycle (describe)
#   SMOKE FAILED       -- failed (describe)
#   SMOKE NOT RUN      -- daemon/session not reached
#
# Blockers for next smoke (if any):
#
# Next action:
"@
$verdictNote | Set-Content -Path (Join-Path $EvidenceDir 'notes\final_verdict.txt') -Encoding UTF8

Write-Host "  [OK] discord_observation.txt"
Write-Host "  [OK] gui_observation.txt"
Write-Host "  [OK] smoke_lifecycle_checklist.txt"
Write-Host "  [OK] final_verdict.txt"

# ---------------------------------------------------------------------------
# 6. Summary doc
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- 6. Summary --"

$gitHead = 'unknown'
try { $gitHead = (& git -C $RepoRoot rev-parse HEAD 2>&1)[0] } catch {}
$gitDirty = $false
try { $gitDirty = ((& git -C $RepoRoot status --porcelain 2>&1) -ne '') } catch {}

$summary = @"
# Paper Smoke Evidence Pack
# Generated: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') UTC
# Label:     $Label
# Folder:    $EvidenceDir

## Git State
- Commit   : $gitHead
- Dirty    : $gitDirty

## Proof
- File     : .proof\full_repo_proof_output.txt
- Captured : proof_tail.txt (last 100 lines)

## Daemon API Snapshots
- Daemon reachable: $daemonReachable
- Files in api/    : (see api/ subfolder)

## DB Snapshots
- MQK_DATABASE_URL set: $(-not [string]::IsNullOrEmpty($dbUrl))
- Files in db/         : (see db/ subfolder)

## Operator Notes (fill in manually)
- notes/discord_observation.txt
- notes/gui_observation.txt
- notes/smoke_lifecycle_checklist.txt
- notes/final_verdict.txt

## Required Evidence Fields (fill after smoke)
- Signal produced       : (see lifecycle checklist)
- Order submitted       : (see lifecycle checklist)
- ACK received          : (see lifecycle checklist)
- Fill received         : (see lifecycle checklist)
- OMS terminal          : (see lifecycle checklist)
- Reconcile clean       : (see lifecycle checklist)
- Discord alert         : (see discord_observation.txt)
- GUI matched backend   : (see gui_observation.txt)

## Final Verdict
- See notes/final_verdict.txt
"@
$summary | Set-Content -Path (Join-Path $EvidenceDir 'summary.md') -Encoding UTF8
Write-Host "  [OK] summary.md"

# ---------------------------------------------------------------------------
# Done
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Evidence pack complete ===" -ForegroundColor Cyan
Write-Host "    Folder: $EvidenceDir" -ForegroundColor Cyan
Write-Host ""
Write-Host "Next steps:"
Write-Host "  1. Before smoke: run Start-PaperTradingSmoke.ps1 -CheckOnly"
Write-Host "  2. Run smoke:    Start-PaperTradingSmoke.ps1 [-WatchSeconds 420]"
Write-Host "  3. After smoke:  run this script again with -Label post_smoke"
Write-Host "  4. Fill in:      notes/smoke_lifecycle_checklist.txt"
Write-Host "  5. Fill in:      notes/discord_observation.txt"
Write-Host "  6. Fill in:      notes/gui_observation.txt"
Write-Host "  7. Fill in:      notes/final_verdict.txt"
Write-Host ""
