# =============================================================================
# PAPER-SMOKE-CLEAN-SHUTDOWN-CAPTURE-01
# Stop-PaperTradingClean.ps1
#
# Orchestrated clean-shutdown sequence for MiniQuantDesk V4 Paper + Alpaca
# sessions. Applies equally to a normal autonomous operating day
# (docs/runbooks/autonomous_paper_ops.md section 10) and to a smoke /
# proof-harness run (docs/runbooks/operator_control_surface.md section 3) --
# the underlying authorized actions and evidence surfaces are identical.
# See docs/runbooks/operator_control_surface.md section 9 for the full
# narrative description of every phase below.
#
# Runs, strictly in this order:
#   1. Pre-stop evidence capture   (Capture-PaperSmokeEvidence.ps1 -Label <Label>_pre_stop)
#                                   -- captures paper-status, reconcile, positions,
#                                      orders, fills, alerts, watchlist status, and
#                                      admission-check WHILE the runtime is still live.
#   2. stop-system                 (POST /api/v1/ops/action, pre-existing authorized action_key)
#   3. Verify stopped              (GET  /api/v1/system/status, read-only)
#   4. disarm-execution            (POST /api/v1/ops/action, pre-existing authorized action_key)
#   5. Post-stop evidence capture  (Capture-PaperSmokeEvidence.ps1 -Label <Label>_post_stop)
#                                   -- re-captures the same surfaces to record the
#                                      final settled state after stop+disarm.
#   6. Manual next-steps summary   (daemon process kill / DB container -- intentionally
#                                   NOT automated; printed only, operator decides)
#
# Hard rules enforced by this script:
#   - Uses ONLY pre-existing authorized action_key values: stop-system, disarm-execution.
#     Introduces no new mutating route.
#   - Never submits, cancels, or replaces an order. Never calls a broker endpoint.
#   - Never issues SQL / never mutates the DB directly -- evidence capture is
#     delegated entirely to the existing read-only Capture-PaperSmokeEvidence.ps1.
#   - Never kills the daemon process and never stops the DB container -- those
#     remain explicit, manual operator decisions printed (not executed) in phase 6.
#   - Captures evidence BEFORE stopping (phase 1) and AGAIN after stop+disarm
#     (phase 5), so the final state of the session is provable, not asserted.
#   - Fails soft at every phase: if the daemon is unreachable, the failure is
#     recorded (matching Capture-PaperSmokeEvidence.ps1's "UNAVAILABLE: ..."
#     convention) and the script continues to the next phase rather than
#     aborting -- the goal is maximum honest evidence, never a fabricated
#     "clean" result.
#   - Never prints MQK_OPERATOR_TOKEN, ALPACA_API_KEY*, ALPACA_API_SECRET*,
#     DISCORD_WEBHOOK_URL, or DB password.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Stop-PaperTradingClean.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Stop-PaperTradingClean.ps1 -Label eod_shutdown
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Stop-PaperTradingClean.ps1 -SkipDisarm
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Stop-PaperTradingClean.ps1 -CaptureOnly
#
# Parameters:
#   -Label        Short label used to build the two evidence-folder labels
#                 (<Label>_pre_stop and <Label>_post_stop, passed straight
#                 through to Capture-PaperSmokeEvidence.ps1's -Label).
#                 Default: "eod_shutdown".
#   -RepoRoot     Repo root directory. Default: two levels up from this script.
#   -DaemonPort   Daemon HTTP port. Default: 8899.
#   -SkipDisarm   Skip phase 4 (disarm-execution). Use only when the operator
#                 intends to resume the same armed session shortly afterward.
#   -CaptureOnly  Run phases 1 and 5 only (pre/post evidence capture); skip
#                 stop-system and disarm-execution entirely. Useful for taking
#                 an evidence "bookend" pair around a manually-managed stop.
#
# Hard invariant: phase 1 (pre-stop capture) ALWAYS runs before any mutating
# action_key is issued, and phase 5 (post-stop capture) ALWAYS runs after --
# this ordering is the entire point of PAPER-SMOKE-CLEAN-SHUTDOWN-CAPTURE-01
# and must never be reordered or made conditional on phases 2-4 succeeding.
# =============================================================================

[CmdletBinding()]
param(
    [string]$Label      = 'eod_shutdown',
    [string]$RepoRoot   = '',
    [int]   $DaemonPort = 8899,
    [switch]$SkipDisarm,
    [switch]$CaptureOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
function Write-Step    { param([string]$M) Write-Host "[STOP-CLEAN] $M" -ForegroundColor Cyan }
function Write-Ok      { param([string]$M) Write-Host "[STOP-CLEAN] OK: $M" -ForegroundColor Green }
function Write-Warn    { param([string]$M) Write-Host "[STOP-CLEAN] WARN: $M" -ForegroundColor Yellow }
function Write-Unavail { param([string]$M) Write-Host "[STOP-CLEAN] UNAVAILABLE: $M" -ForegroundColor DarkGray }
function Write-Section { param([string]$M) Write-Host "" ; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Secret guard: never print these names' values (mirrors Start-PaperTradingSmoke.ps1)
# ---------------------------------------------------------------------------
$SECRET_NAMES = @(
    'ALPACA_API_KEY_PAPER', 'ALPACA_API_SECRET_PAPER',
    'ALPACA_API_KEY_LIVE',  'ALPACA_API_SECRET_LIVE',
    'MQK_OPERATOR_TOKEN',   'DISCORD_WEBHOOK_URL',
    'POSTGRES_PASSWORD',    'DATABASE_URL'
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

$DaemonBaseUrl  = "http://127.0.0.1:$DaemonPort"
$CaptureScript  = Join-Path $RepoRoot 'scripts\windows\Capture-PaperSmokeEvidence.ps1'
$PreStopLabel   = "${Label}_pre_stop"
$PostStopLabel  = "${Label}_post_stop"

if (-not (Test-Path $CaptureScript)) {
    Write-Warn "Capture-PaperSmokeEvidence.ps1 not found at $CaptureScript -- evidence phases will be SKIPPED."
}

# Authorized action_key values only -- introduces no new mutating route.
$ACTION_KEY_STOP    = 'stop-system'
$ACTION_KEY_DISARM  = 'disarm-execution'

# ---------------------------------------------------------------------------
# Operator token (presence only -- value is never printed)
# ---------------------------------------------------------------------------
$operatorToken = [Environment]::GetEnvironmentVariable('MQK_OPERATOR_TOKEN', 'Process')
if ([string]::IsNullOrWhiteSpace($operatorToken)) {
    $operatorToken = [Environment]::GetEnvironmentVariable('MQK_OPERATOR_TOKEN', 'User')
}
$haveToken = -not [string]::IsNullOrWhiteSpace($operatorToken)
if ($haveToken) {
    Write-Ok "MQK_OPERATOR_TOKEN is configured (value not printed)."
} else {
    Write-Warn "MQK_OPERATOR_TOKEN is not set -- stop-system/disarm-execution phases will be SKIPPED (auth required)."
}

# ---------------------------------------------------------------------------
# HTTP helpers (read-only GET, and POST restricted to the two authorized
# action_key values declared above -- no order routes, no broker routes)
# ---------------------------------------------------------------------------
function Invoke-DaemonGet {
    param([string]$Path)
    try {
        $resp = Invoke-WebRequest -Uri "$DaemonBaseUrl$Path" -Method GET `
            -TimeoutSec 5 -UseBasicParsing -ErrorAction Stop
        return [pscustomobject]@{ Ok = $true; Body = ($resp.Content | ConvertFrom-Json); Error = $null }
    } catch {
        return [pscustomobject]@{ Ok = $false; Body = $null; Error = $_.Exception.Message }
    }
}

function Invoke-DaemonOpsAction {
    param([string]$ActionKey)

    if ($ActionKey -ne $ACTION_KEY_STOP -and $ActionKey -ne $ACTION_KEY_DISARM) {
        throw "BUG: Stop-PaperTradingClean.ps1 only issues '$ACTION_KEY_STOP' or '$ACTION_KEY_DISARM'. Refusing '$ActionKey'."
    }
    if (-not $haveToken) {
        return [pscustomobject]@{ Ok = $false; StatusCode = $null; Body = $null; Error = 'MQK_OPERATOR_TOKEN not set' }
    }

    $bodyJson = (@{ action_key = $ActionKey } | ConvertTo-Json -Compress)
    try {
        $resp = Invoke-WebRequest -Uri "$DaemonBaseUrl/api/v1/ops/action" -Method POST `
            -ContentType 'application/json' -Body $bodyJson `
            -Headers @{ Authorization = "Bearer $operatorToken" } `
            -TimeoutSec 10 -UseBasicParsing -ErrorAction Stop
        return [pscustomobject]@{ Ok = $true; StatusCode = [int]$resp.StatusCode; Body = ($resp.Content | ConvertFrom-Json); Error = $null }
    } catch {
        $sc = $null
        $rawBody = $null
        if ($null -ne $_.Exception.Response) {
            try { $sc = [int]$_.Exception.Response.StatusCode } catch {}
            try {
                $stream = $_.Exception.Response.GetResponseStream()
                $reader = New-Object System.IO.StreamReader($stream)
                $rawBody = $reader.ReadToEnd()
                $reader.Dispose(); $stream.Dispose()
            } catch {}
        }
        $parsed = $null
        if ($rawBody) { try { $parsed = $rawBody | ConvertFrom-Json } catch {} }
        return [pscustomobject]@{ Ok = $false; StatusCode = $sc; Body = $parsed; Error = $_.Exception.Message }
    }
}

function Invoke-EvidenceCapture {
    param([string]$CaptureLabel, [string]$PhaseName)

    if (-not (Test-Path $CaptureScript)) {
        Write-Unavail "$PhaseName capture skipped -- Capture-PaperSmokeEvidence.ps1 not found."
        return $false
    }
    try {
        & powershell -ExecutionPolicy Bypass -File $CaptureScript -Label $CaptureLabel -RepoRoot $RepoRoot -DaemonPort $DaemonPort
        Write-Ok "$PhaseName capture complete (label: $CaptureLabel)."
        return $true
    } catch {
        Write-Unavail "$PhaseName capture failed: $($_.Exception.Message)"
        return $false
    }
}

# ===========================================================================
# Phase 1: Pre-stop evidence capture (ALWAYS first -- before any mutation)
# ===========================================================================
Write-Section "Phase 1/6: Pre-stop evidence capture (label: $PreStopLabel)"
Write-Step "Capturing paper-status, reconcile, positions, orders, fills, alerts,"
Write-Step "watchlist status, and admission-check WHILE the runtime is still live..."
$phase1Ok = Invoke-EvidenceCapture -CaptureLabel $PreStopLabel -PhaseName 'Pre-stop'

# ===========================================================================
# Phase 2: stop-system
# ===========================================================================
Write-Section "Phase 2/6: Stop execution runtime ($ACTION_KEY_STOP)"
$phase2Result = $null
if ($CaptureOnly) {
    Write-Step "-CaptureOnly set -- skipping $ACTION_KEY_STOP."
} else {
    $phase2Result = Invoke-DaemonOpsAction -ActionKey $ACTION_KEY_STOP
    if ($phase2Result.Ok) {
        Write-Ok "$ACTION_KEY_STOP -> HTTP $($phase2Result.StatusCode)"
    } else {
        Write-Unavail "$ACTION_KEY_STOP failed or daemon unreachable: $($phase2Result.Error)"
    }
}

# ===========================================================================
# Phase 3: Verify stopped (read-only)
# ===========================================================================
Write-Section "Phase 3/6: Verify stopped (GET /api/v1/system/status)"
$statusResult = Invoke-DaemonGet -Path '/api/v1/system/status'
if ($statusResult.Ok) {
    $runtimeStatus = $null
    try { $runtimeStatus = $statusResult.Body.runtime_status } catch {}
    if ($null -eq $runtimeStatus) { $runtimeStatus = '(field not present)' }
    Write-Ok "system/status reachable -- runtime_status: $runtimeStatus"
} else {
    Write-Unavail "system/status unreachable: $($statusResult.Error)"
}

# ===========================================================================
# Phase 4: disarm-execution
# ===========================================================================
Write-Section "Phase 4/6: Disarm execution ($ACTION_KEY_DISARM)"
$phase4Result = $null
if ($CaptureOnly) {
    Write-Step "-CaptureOnly set -- skipping $ACTION_KEY_DISARM."
} elseif ($SkipDisarm) {
    Write-Step "-SkipDisarm set -- leaving the run armed for a quick resume."
} else {
    $phase4Result = Invoke-DaemonOpsAction -ActionKey $ACTION_KEY_DISARM
    if ($phase4Result.Ok) {
        Write-Ok "$ACTION_KEY_DISARM -> HTTP $($phase4Result.StatusCode)"
    } else {
        Write-Unavail "$ACTION_KEY_DISARM failed or daemon unreachable: $($phase4Result.Error)"
    }
}

# ===========================================================================
# Phase 5: Post-stop evidence capture (ALWAYS runs -- records final state)
# ===========================================================================
Write-Section "Phase 5/6: Post-stop evidence capture (label: $PostStopLabel)"
Write-Step "Re-capturing the same surfaces to record the final settled state"
Write-Step "(positions/orders/reconcile after stop+disarm)..."
$phase5Ok = Invoke-EvidenceCapture -CaptureLabel $PostStopLabel -PhaseName 'Post-stop'

# ===========================================================================
# Phase 6: Manual next-steps summary (NOT automated -- operator decision)
# ===========================================================================
Write-Section "Phase 6/6: Manual next steps (not automated -- operator decides)"
Write-Host ""
Write-Host "  This script never kills the daemon process and never stops the" -ForegroundColor White
Write-Host "  Postgres container -- the container holds durable DB truth that" -ForegroundColor White
Write-Host "  the next session's restart-safety depends on. Confirm the capture" -ForegroundColor White
Write-Host "  folders above look correct, then (if desired):" -ForegroundColor White
Write-Host ""
Write-Host "    Stop-Process -Name 'mqk-daemon' -ErrorAction SilentlyContinue" -ForegroundColor Gray
Write-Host "    docker stop mqk-paper-postgres" -ForegroundColor Gray
Write-Host ""

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Section "Clean shutdown sequence complete"
Write-Host "    Pre-stop capture  : $(if ($phase1Ok) { 'captured' } else { 'UNAVAILABLE' }) (label: $PreStopLabel)"
if ($CaptureOnly) {
    Write-Host "    stop-system       : skipped (-CaptureOnly)"
    Write-Host "    disarm-execution  : skipped (-CaptureOnly)"
} else {
    Write-Host "    stop-system       : $(if ($phase2Result -and $phase2Result.Ok) { "HTTP $($phase2Result.StatusCode)" } else { 'UNAVAILABLE / not issued' })"
    if ($SkipDisarm) {
        Write-Host "    disarm-execution  : skipped (-SkipDisarm)"
    } else {
        Write-Host "    disarm-execution  : $(if ($phase4Result -and $phase4Result.Ok) { "HTTP $($phase4Result.StatusCode)" } else { 'UNAVAILABLE / not issued' })"
    }
}
Write-Host "    Post-stop capture : $(if ($phase5Ok) { 'captured' } else { 'UNAVAILABLE' }) (label: $PostStopLabel)"
Write-Host ""
Write-Host "Evidence folders are under: $RepoRoot\evidence\paper_smoke_<timestamp>_${PreStopLabel} / _${PostStopLabel}"
Write-Host ""
