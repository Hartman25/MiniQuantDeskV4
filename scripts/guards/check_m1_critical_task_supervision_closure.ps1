# =============================================================================
# M1-AUTONOMOUS-CRITICAL-TASK-SUPERVISION-WAVE-01: critical background-task
# supervision closure guard.
# =============================================================================
#
# Before this wave, two trading-critical mqk-daemon background tasks could
# die silently while the daemon kept looking healthier than it really was:
#
#   - the outer Alpaca paper WS transport task (main.rs retained its
#     JoinHandle but never awaited it)
#   - the per-run reconcile-tick task (spawn_reconcile_tick's JoinHandle was
#     fully discarded, and a fresh one was spawned on every run start with
#     no ownership of the previous run's task)
#
# This guard is a source-level regression check, not a behavioral test --
# `cargo test -p mqk-daemon --lib` (the m1a*/m1b*/d02* scenarios) is the
# actual proof; this guard exists so a future edit cannot quietly delete or
# rewire the supervision seams those tests depend on without this failing
# first, source-semantic rather than line-number-based.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_m1_critical_task_supervision_closure.ps1
#
# Exit codes: 0 = clean, 1 = violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$DaemonSrc = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src"

Write-Host "============================================================"
Write-Host " M1 Critical Task Supervision Closure Guard"
Write-Host " Repo root: $RepoRoot"
Write-Host "============================================================"

function Get-SrcFiles {
    Get-ChildItem -Path $DaemonSrc -Recurse -Filter "*.rs" |
        Where-Object { $_.FullName -notmatch '\\target\\' }
}

function Test-AnyFileContains {
    param([string]$Pattern)
    foreach ($File in (Get-SrcFiles)) {
        if (Select-String -Path $File.FullName -Pattern $Pattern -Quiet) {
            return $true
        }
    }
    return $false
}

function Test-FileContains {
    param([string]$RelativePath, [string]$Pattern)
    $FullPath = Join-Path $DaemonSrc $RelativePath
    if (-not (Test-Path $FullPath)) { return $false }
    return [bool](Select-String -Path $FullPath -Pattern $Pattern -Quiet)
}

$Violations = @()

# ---------------------------------------------------------------------------
# 1. main.rs owns/awaits the Alpaca WS JoinHandle instead of storing it
#    blindly (BRK-00R-05 outer-task watchdog).
# ---------------------------------------------------------------------------
if (-not (Test-FileContains "main.rs" 'supervise_alpaca_ws_terminal_task')) {
    $Violations += "main.rs no longer wires supervise_alpaca_ws_terminal_task onto the Alpaca WS JoinHandle"
}

# ---------------------------------------------------------------------------
# 2. Ordinary WS reconnect remains inside the existing transport loop --
#    tripwire that alpaca_ws_loop's own reconnect/backoff/GapDetected cycle
#    was not ripped out or replaced by an outer restart loop.
# ---------------------------------------------------------------------------
if (-not (Test-FileContains "state\alpaca_ws_transport.rs" 'fn alpaca_ws_loop')) {
    $Violations += "state/alpaca_ws_transport.rs: alpaca_ws_loop (the inner reconnect loop) not found"
}
if (-not (Test-FileContains "state\alpaca_ws_transport.rs" 'AlpacaWsContinuityState::GapDetected')) {
    $Violations += "state/alpaca_ws_transport.rs: ordinary-disconnect GapDetected marking appears to be missing"
}

# ---------------------------------------------------------------------------
# 3. Terminal WS failure drives fail-closed truth, reusing (not duplicating)
#    the existing continuity + session-truth seams.
# ---------------------------------------------------------------------------
if (-not (Test-AnyFileContains 'fn mark_alpaca_ws_task_exited')) {
    $Violations += "mark_alpaca_ws_task_exited not found -- terminal WS task-death projection is missing"
} else {
    if (-not (Test-FileContains "state.rs" 'update_ws_continuity\(AlpacaWsContinuityState::GapDetected')) {
        $Violations += "mark_alpaca_ws_task_exited no longer forces continuity to GapDetected via update_ws_continuity"
    }
    if (-not (Test-FileContains "state.rs" 'AutonomousSessionTruth::AlpacaWsTransportExited')) {
        $Violations += "AutonomousSessionTruth::AlpacaWsTransportExited truth variant not found"
    }
}

# ---------------------------------------------------------------------------
# 4. Reconcile task is explicitly run-owned, not left fully detached.
# ---------------------------------------------------------------------------
if (-not (Test-AnyFileContains 'fn install_reconcile_task_owner')) {
    $Violations += "install_reconcile_task_owner not found -- reconcile task ownership is missing"
}
if (-not (Test-FileContains "state\lifecycle.rs" 'install_reconcile_task_owner')) {
    $Violations += "state/lifecycle.rs's start_execution_runtime no longer calls install_reconcile_task_owner"
}
if (-not (Test-AnyFileContains 'struct ReconcileTaskOwnership')) {
    $Violations += "ReconcileTaskOwnership record type not found"
}

# ---------------------------------------------------------------------------
# 5. Reconcile task death cannot leave stale clean authority: the watchdog
#    must route through the existing publish_reconcile_failure disarm/halt
#    authority, not a parallel "reconcile healthy" boolean.
# ---------------------------------------------------------------------------
if (-not (Test-FileContains "state\loop_runner.rs" 'fn supervise_reconcile_terminal_task')) {
    $Violations += "supervise_reconcile_terminal_task not found in state/loop_runner.rs"
} elseif (-not (Test-FileContains "state\loop_runner.rs" 'publish_reconcile_failure')) {
    $Violations += "supervise_reconcile_terminal_task no longer routes through publish_reconcile_failure"
}

# ---------------------------------------------------------------------------
# 6. Clean shutdown/supersession is not failure: both watchdogs must check
#    JoinError::is_cancelled() before projecting any failure truth.
# ---------------------------------------------------------------------------
$CancelledCount = 0
foreach ($File in (Get-SrcFiles)) {
    $CancelledCount += (Select-String -Path $File.FullName -Pattern 'is_cancelled\(\)' -AllMatches).Matches.Count
}
if ($CancelledCount -lt 2) {
    $Violations += "expected at least 2 is_cancelled() guards (one per watchdog: WS + reconcile), found $CancelledCount"
}
if (-not (Test-FileContains "state\lifecycle.rs" 'reconcile_task_owner')) {
    $Violations += "state/lifecycle.rs's clear_local_runtime_for_run no longer aborts the reconcile task owner on stop/halt/shutdown"
}

# ---------------------------------------------------------------------------
# 7 / 8. Pre-existing supervision (completed-bar driver, session-controller
# watchdog) must remain present -- this wave must not have weakened either.
# ---------------------------------------------------------------------------
if (-not (Test-AnyFileContains 'CompletedBarDriverExited')) {
    $Violations += "CompletedBarDriverExited truth variant no longer found -- completed-bar driver supervision may have been weakened"
}
if (-not (Test-FileContains "main.rs" 'ControllerExited')) {
    $Violations += "main.rs no longer projects ControllerExited -- session-controller watchdog may have been weakened"
}

# ---------------------------------------------------------------------------
# 9. No alternate/duplicate supervision framework: this wave must extend the
# two existing per-task patterns, never introduce a generic actor/registry.
# ---------------------------------------------------------------------------
$BannedGenericNames = @('SupervisorManager', 'TaskSupervisor', 'TaskRegistry', 'GenericTaskActor')
foreach ($Name in $BannedGenericNames) {
    if (Test-AnyFileContains $Name) {
        $Violations += "found banned generic supervision-framework name '$Name' -- extend the existing per-task watchdogs instead"
    }
}

Write-Host ""
if ($Violations.Count -eq 0) {
    Write-Host " OK -- Alpaca WS terminal-task and reconcile-task supervision are wired," -ForegroundColor Green
    Write-Host "       route through existing fail-closed authority, and pre-existing" -ForegroundColor Green
    Write-Host "       completed-bar/session-controller supervision is unweakened." -ForegroundColor Green
    exit 0
} else {
    Write-Host " FAIL -- M1 critical task supervision contract violation(s):" -ForegroundColor Red
    $Violations | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}
