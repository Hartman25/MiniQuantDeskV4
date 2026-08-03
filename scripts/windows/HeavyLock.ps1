# =============================================================================
# HeavyLock.ps1
#
# HEAVY-LOCK-01 (atomic, FULL-AUDIT-FINAL-HERMETIC-CLOSURE-01 Part 4):
# exclusive lock so two heavy build/test sessions on this machine never run
# concurrently and compete for memory. Dot-sourced by full_repo_proof.ps1
# and by tests/script_guards/test_heavy_lock_atomic_01.ps1, so both the real
# caller and its test exercise the exact same implementation.
#
# Previous version was a check-then-write marker file (`Test-Path` followed
# by a separate `Set-Content`) -- a classic TOCTOU race: two processes could
# both observe "not present" before either wrote, and both would proceed
# believing they held the lock. This version instead holds a real OS-level
# exclusive file handle for the entire session, opened with
# `[System.IO.FileShare]::None` via a single atomic `[System.IO.File]::Open`
# call -- no separate existence-check step exists to race against.
#
# Stale-crash recovery relies on actual Windows OS behavior, not on this
# script's own `finally` block: when a process is terminated (gracefully or
# via TerminateProcess/Stop-Process -Force/kill), the OS unconditionally
# closes every handle that process held, including a FileShare.None handle
# on this lock file. So `[System.IO.FileMode]::OpenOrCreate` +
# `[System.IO.FileShare]::None` from a *new* process will succeed the moment
# the old, now-dead holder's handle is gone -- with no liveness heuristic,
# no PID-checking, and no reliance on that old process having run its own
# cleanup code. Do NOT assume a `finally` block runs after a force-killed
# process; it does not. `finally` in a caller's `Unlock-HeavyMachine` call
# only handles the *graceful* exception/normal-failure cleanup path -- the
# OS is what handles the crash path.
# =============================================================================

function Get-HeavyLockPath {
    return 'C:\tmp\mqk-machine-heavy.lock'
}

# Returned by Lock-HeavyMachine; passed back to Unlock-HeavyMachine so
# release can refuse to act on a lock it does not actually own (wrong-owner
# release refusal), in addition to the OS-level exclusivity above.
class HeavyMachineLockHandle {
    [string]$LockPath
    [string]$OwnerToken
    [System.IO.FileStream]$Stream
}

function Lock-HeavyMachine {
    param(
        [Parameter(Mandatory = $true)]
        [string]$LockPath
    )

    $lockDir = Split-Path -Parent $LockPath
    if (-not (Test-Path -LiteralPath $lockDir)) {
        New-Item -ItemType Directory -Force -Path $lockDir | Out-Null
    }

    $stream = $null
    try {
        # Atomic: no Test-Path before this. OpenOrCreate + FileShare.None
        # either (a) creates a fresh file and grabs the exclusive handle, or
        # (b) opens a stale file left by a now-dead process (whose handle the
        # OS already released) and grabs the exclusive handle, or (c) throws
        # immediately if another *live* process currently holds it.
        $stream = [System.IO.File]::Open(
            $LockPath,
            [System.IO.FileMode]::OpenOrCreate,
            [System.IO.FileAccess]::ReadWrite,
            [System.IO.FileShare]::None
        )
    } catch [System.IO.IOException] {
        throw ("EXITCODE=1;Heavy lock already held at {0} by another live process (deterministic: " +
               "the OS refused an exclusive handle). If you have confirmed via Task Manager / " +
               "Get-Process that no other build/test process is actually running, the previous " +
               "holder's OS handle would already have been released on process exit -- do not " +
               "delete this file manually; retry once the real holder finishes. Run with " +
               "-NoHeavyLock only if a caller already holds this lock itself." -f $LockPath)
    }

    $ownerToken = [guid]::NewGuid().ToString()
    $payload = "acquired_at_utc={0};pid={1};owner_token={2};script=full_repo_proof.ps1" -f `
        ([DateTimeOffset]::UtcNow.ToString('o')), $PID, $ownerToken
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($payload)
    $stream.SetLength(0)
    $stream.Write($bytes, 0, $bytes.Length)
    $stream.Flush()

    Write-Host "Heavy lock acquired (atomic, exclusive OS handle): $LockPath" -ForegroundColor Yellow

    $handle = [HeavyMachineLockHandle]::new()
    $handle.LockPath = $LockPath
    $handle.OwnerToken = $ownerToken
    $handle.Stream = $stream
    return $handle
}

function Unlock-HeavyMachine {
    param(
        [Parameter(Mandatory = $true)]
        [HeavyMachineLockHandle]$Handle
    )

    if ($null -eq $Handle -or $null -eq $Handle.Stream) {
        return
    }

    # Wrong-owner release refusal: re-read the file through the handle we
    # still hold and confirm it still carries the owner_token we wrote at
    # acquire time before releasing. (No other process could have written to
    # this file while we hold FileShare.None, so this is a defense-in-depth
    # assertion, not the primary safety mechanism -- the OS-level exclusive
    # handle is.)
    $Handle.Stream.Position = 0
    $reader = New-Object System.IO.StreamReader($Handle.Stream, [System.Text.Encoding]::UTF8, $false, 1024, $true)
    $currentContent = $reader.ReadToEnd()
    $reader.Dispose()
    if ($currentContent -notmatch [regex]::Escape("owner_token=$($Handle.OwnerToken)")) {
        throw ("EXITCODE=1;Unlock-HeavyMachine refused: the lock at {0} no longer carries this " +
               "handle's owner_token ({1}); refusing to release a lock this call does not own." -f `
               $Handle.LockPath, $Handle.OwnerToken)
    }

    $Handle.Stream.Close()
    $Handle.Stream.Dispose()
    Remove-Item -LiteralPath $Handle.LockPath -Force -ErrorAction SilentlyContinue
    Write-Host "Heavy lock released: $($Handle.LockPath)" -ForegroundColor Yellow
}
