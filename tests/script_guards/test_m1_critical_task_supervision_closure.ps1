# =============================================================================
# M1-CRITICAL-TASK-GUARD-CI-WIRING-01
#
# Thin CI wrapper for the M1 critical-task supervision source guard. Exists
# so the canonical Windows CI aggregator (run_all_script_guards.ps1) executes
# scripts\guards\check_m1_critical_task_supervision_closure.ps1 automatically
# on every run, instead of that guard supplying manual/local evidence only.
#
# Contains no daemon, DB, broker, network, or secret behavior -- it only
# locates the repo root and re-invokes the production source guard,
# propagating its exit code unchanged.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_m1_critical_task_supervision_closure.ps1
#
# Exit codes: 0 = guard passed, 1 = guard failed or was not found.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot   = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$GuardScript = Join-Path $RepoRoot 'scripts\guards\check_m1_critical_task_supervision_closure.ps1'

Write-Host ''
Write-Host '=== M1 critical task supervision closure guard (wrapper) ==='
Write-Host "    Guard: $GuardScript"
Write-Host ''

if (-not (Test-Path $GuardScript)) {
    Write-Host "FAIL: guard script not found at $GuardScript" -ForegroundColor Red
    exit 1
}

& (Get-Process -Id $PID).Path -ExecutionPolicy Bypass -NonInteractive -File $GuardScript
$guardExit = $LASTEXITCODE
if ($null -eq $guardExit) { $guardExit = 1 }

exit $guardExit
