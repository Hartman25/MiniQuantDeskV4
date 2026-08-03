# =============================================================================
# test_heavy_lock_atomic_01.ps1
#
# FULL-AUDIT-FINAL-HERMETIC-CLOSURE-01 Part 4: proves the atomic heavy lock
# (scripts/windows/HeavyLock.ps1) actually behaves atomically, not just that
# it compiles. Four required scenarios:
#
#   1. Simultaneous acquisition deterministically fails.
#   2. Wrong-owner release is refused.
#   3. Normal (graceful) failure still releases the lock (finally path).
#   4. A killed holder's lock is recoverable by a new acquirer (OS-level
#      handle release on process termination, not a liveness heuristic).
#
# Uses a dedicated lock path under the OS temp dir so this test never
# touches C:\tmp\mqk-machine-heavy.lock (the real path other sessions may
# be holding).
#
# Exit codes: 0 = all four scenarios pass, 1 = any failed.
# =============================================================================

$ErrorActionPreference = 'Stop'
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')

. (Join-Path $RepoRoot 'scripts\windows\HeavyLock.ps1')

$TestLockPath = Join-Path $env:TEMP ("mqk-heavy-lock-test-{0}.lock" -f ([guid]::NewGuid().ToString('N')))

$failures = @()

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if (-not $Condition) {
        $script:failures += $Message
        Write-Host "  FAIL: $Message" -ForegroundColor Red
    } else {
        Write-Host "  PASS: $Message" -ForegroundColor Green
    }
}

Write-Host "============================================================"
Write-Host " Atomic heavy lock proof"
Write-Host " Test lock path: $TestLockPath"
Write-Host "============================================================"

# ---------------------------------------------------------------------------
# Scenario 1: simultaneous acquisition deterministically fails.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "--- Scenario 1: simultaneous acquisition ---"
if (Test-Path -LiteralPath $TestLockPath) { Remove-Item -LiteralPath $TestLockPath -Force }
$handle1 = Lock-HeavyMachine -LockPath $TestLockPath
$secondAcquireThrew = $false
try {
    Lock-HeavyMachine -LockPath $TestLockPath | Out-Null
} catch {
    $secondAcquireThrew = $true
}
Assert-True $secondAcquireThrew "a second concurrent acquisition of the same lock path must throw"
Unlock-HeavyMachine -Handle $handle1
Assert-True (-not (Test-Path -LiteralPath $TestLockPath)) "lock file must be gone after normal release"

# ---------------------------------------------------------------------------
# Scenario 2: wrong-owner release is refused.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "--- Scenario 2: wrong-owner release refusal ---"
if (Test-Path -LiteralPath $TestLockPath) { Remove-Item -LiteralPath $TestLockPath -Force }
$realHandle = Lock-HeavyMachine -LockPath $TestLockPath
$forgedHandle = [HeavyMachineLockHandle]::new()
$forgedHandle.LockPath = $realHandle.LockPath
$forgedHandle.OwnerToken = [guid]::NewGuid().ToString()  # wrong token
$forgedHandle.Stream = $realHandle.Stream                # same stream (only way to read it)
$wrongOwnerThrew = $false
try {
    Unlock-HeavyMachine -Handle $forgedHandle
} catch {
    $wrongOwnerThrew = $true
}
Assert-True $wrongOwnerThrew "release with a forged owner_token must be refused"
Assert-True (Test-Path -LiteralPath $TestLockPath) "lock file must still exist after a refused wrong-owner release"
# Clean up with the real handle.
Unlock-HeavyMachine -Handle $realHandle
Assert-True (-not (Test-Path -LiteralPath $TestLockPath)) "lock file must be gone after the real owner releases"

# ---------------------------------------------------------------------------
# Scenario 3: normal failure cleanup (finally path).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "--- Scenario 3: normal failure cleanup (finally releases) ---"
if (Test-Path -LiteralPath $TestLockPath) { Remove-Item -LiteralPath $TestLockPath -Force }
$laneThrew = $false
$handle3 = $null
try {
    $handle3 = Lock-HeavyMachine -LockPath $TestLockPath
    try {
        throw "simulated lane failure"
    } finally {
        Unlock-HeavyMachine -Handle $handle3
    }
} catch {
    $laneThrew = $true
}
Assert-True $laneThrew "the simulated lane exception must have propagated"
Assert-True (-not (Test-Path -LiteralPath $TestLockPath)) "finally must have released the lock despite the exception"

# ---------------------------------------------------------------------------
# Scenario 4: a killed holder's lock is recoverable (OS releases the handle).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "--- Scenario 4: stale recovery after a killed holder ---"
if (Test-Path -LiteralPath $TestLockPath) { Remove-Item -LiteralPath $TestLockPath -Force }

$childScript = @"
. '$($RepoRoot -replace "'", "''")\scripts\windows\HeavyLock.ps1'
`$h = Lock-HeavyMachine -LockPath '$($TestLockPath -replace "'", "''")'
Start-Sleep -Seconds 60
"@
$childScriptPath = Join-Path $env:TEMP ("mqk-heavy-lock-child-{0}.ps1" -f ([guid]::NewGuid().ToString('N')))
Set-Content -LiteralPath $childScriptPath -Value $childScript -Encoding UTF8

$selfHost = (Get-Process -Id $PID).Path
$child = Start-Process -FilePath $selfHost -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $childScriptPath) -PassThru -WindowStyle Hidden

# Wait for the child to actually acquire the lock (file exists and is non-empty).
$acquired = $false
for ($i = 0; $i -lt 50; $i++) {
    if ((Test-Path -LiteralPath $TestLockPath) -and ((Get-Item -LiteralPath $TestLockPath).Length -gt 0)) {
        $acquired = $true
        break
    }
    Start-Sleep -Milliseconds 100
}
Assert-True $acquired "child process must have acquired the lock before being killed"

# A live-holder acquisition attempt from this process must fail while the
# child is still alive (corroborates scenario 1 across process boundaries).
$liveHolderBlocks = $false
try {
    Lock-HeavyMachine -LockPath $TestLockPath | Out-Null
} catch {
    $liveHolderBlocks = $true
}
Assert-True $liveHolderBlocks "acquisition must fail while the child process is still alive and holding the lock"

# Force-kill the child -- no chance for its own finally/cleanup code to run.
Stop-Process -Id $child.Id -Force -ErrorAction SilentlyContinue
Start-Sleep -Milliseconds 500

# A new acquisition must now succeed: the OS released the dead process's
# exclusive handle, not because this test waited for any liveness check.
$recovered = $false
$handle4 = $null
try {
    $handle4 = Lock-HeavyMachine -LockPath $TestLockPath
    $recovered = $true
} catch {
    $recovered = $false
}
Assert-True $recovered "acquisition must succeed after the killed holder's process is gone (OS releases the handle)"
if ($handle4) {
    Unlock-HeavyMachine -Handle $handle4
}
Remove-Item -LiteralPath $childScriptPath -Force -ErrorAction SilentlyContinue

Write-Host ""
Write-Host "============================================================"
if ($failures.Count -eq 0) {
    Write-Host " ALL 4 ATOMIC LOCK SCENARIOS PASSED" -ForegroundColor Green
    exit 0
} else {
    Write-Host " FAILED: $($failures.Count) scenario assertion(s):" -ForegroundColor Red
    $failures | ForEach-Object { Write-Host "  - $_" -ForegroundColor Red }
    exit 1
}
