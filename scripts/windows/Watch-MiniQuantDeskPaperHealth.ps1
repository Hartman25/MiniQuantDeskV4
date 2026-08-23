# =============================================================================
# OPS-AUTO-RESTART-LOCAL-01
# Watch-MiniQuantDeskPaperHealth.ps1
#
# Small, bounded local health/recovery supervisor for the official Paper
# launcher. Closes the local-interruption gap that exists AFTER the initial
# scheduled pre-open task (Register-PaperStartupTask.ps1) has already exited
# 0 -- e.g. the daemon process dies hours later, mid-session, with no
# Task Scheduler retry left to fire because that task already completed
# successfully.
#
# SAFETY INVARIANT: this script has exactly one authorized side effect --
# invoking the existing official launcher:
#     Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled
# It never calls a daemon HTTP action route, never sends an Authorization
# header, never touches a broker, never mutates a DB row, and never runs
# any command other than a bounded read-only health GET plus (conditionally)
# the one launcher invocation above. All arm/disarm/halt-clear/reconcile
# decisions remain entirely inside the official launcher's own authority
# (Start-MiniQuantDesk.ps1 -> Invoke-PaperStartup), which this script never
# duplicates or bypasses.
#
# Recovery is bounded on two axes:
#   - MUTEX: a machine-wide named mutex ensures at most one recovery
#     invocation of the official launcher can be in flight from this
#     supervisor at any moment, regardless of how many overlapping
#     Task Scheduler firings occur (defense-in-depth on top of the
#     Task Scheduler task's own MultipleInstances=IgnoreNew).
#   - BACKOFF: a durable local JSON state file records recovery attempt
#     timestamps. No more than -MaxAttempts recovery invocations are made
#     within any trailing -WindowMinutes window; once exceeded, this script
#     reports BOUNDED_BACKOFF_EXCEEDED and refuses to invoke the launcher
#     again until the window rolls forward. This prevents a persistently
#     failing dependency (Docker/Postgres/network/provider) from producing
#     a rapid restart loop.
#
# A healthy daemon (GET /v1/health reachable, service=mqk-daemon) is left
# completely alone -- no launcher invocation, no state-file write.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Watch-MiniQuantDeskPaperHealth.ps1
#
# Exit codes:
#   0 = healthy (no action) OR recovery invocation completed (any exit code;
#       see recovery log for the launcher's own truthful result)
#   1 = recovery was needed but the backoff/mutex guard refused to invoke
#       the launcher (bounded refusal, not a crash)
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [string] $RepoRoot            = '',
    [string] $DaemonBaseUrl       = 'http://127.0.0.1:8899',
    [int]    $MaxAttempts         = 3,
    [int]    $WindowMinutes       = 30,
    [int]    $HealthTimeoutSec    = 5,
    [int]    $MutexWaitMilliseconds = 3000,
    [int]    $LauncherTimeoutSeconds = 2400
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$M) Write-Host "[PAPER-HEALTH-WATCHDOG] $M"       -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[PAPER-HEALTH-WATCHDOG] OK: $M"   -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[PAPER-HEALTH-WATCHDOG] WARN: $M" -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[PAPER-HEALTH-WATCHDOG] FAIL: $M" -ForegroundColor Red    }

# ---------------------------------------------------------------------------
# Read-only daemon health probe. No Authorization header -- /v1/health is the
# same unauthenticated route Start-MiniQuantDesk.ps1/Launch-VeritasLedger.ps1
# already use for reachability checks. Overridable (shadowable) for tests.
# ---------------------------------------------------------------------------
function Test-DaemonHealthy {
    param([Parameter(Mandatory = $true)][string]$DaemonBaseUrl, [int]$TimeoutSec = 5)
    try {
        $resp = Invoke-WebRequest -Uri ($DaemonBaseUrl.TrimEnd('/') + '/v1/health') -Method Get `
            -TimeoutSec $TimeoutSec -UseBasicParsing -ErrorAction Stop
        $json = $null
        try { $json = $resp.Content | ConvertFrom-Json } catch {}
        $healthy = ($null -ne $json) -and ($json.PSObject.Properties['service']) -and ($json.service -eq 'mqk-daemon')
        return [pscustomobject]@{ Healthy = [bool]$healthy; Detail = "HTTP $([int]$resp.StatusCode)" }
    } catch {
        return [pscustomobject]@{ Healthy = $false; Detail = $_.Exception.Message }
    }
}

function Get-RecoveryStatePath {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)
    $dir = Join-Path $RepoRoot 'exports\ops\recovery'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    return (Join-Path $dir 'paper_recovery_state.json')
}

function Get-RecoveryLogPath {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)
    $dir = Join-Path $RepoRoot 'exports\ops\recovery'
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $stamp = Get-Date -Format 'yyyyMMdd_HHmmss'
    return (Join-Path $dir "recovery_$stamp.json")
}

function Read-RecoveryState {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        return [pscustomobject]@{ schema_version = 'paper-recovery-state-v1'; attempts = @() }
    }
    try {
        $raw = Get-Content -Path $Path -Raw
        if ([string]::IsNullOrWhiteSpace($raw)) {
            return [pscustomobject]@{ schema_version = 'paper-recovery-state-v1'; attempts = @() }
        }
        $parsed = $raw | ConvertFrom-Json
        $attempts = @()
        if ($null -ne $parsed.PSObject.Properties['attempts']) { $attempts = @($parsed.attempts) }
        return [pscustomobject]@{ schema_version = 'paper-recovery-state-v1'; attempts = $attempts }
    } catch {
        # Fail closed on a corrupt state file: treat as empty history rather
        # than crashing, but never silently invent a permissive state beyond
        # "no known prior attempts".
        return [pscustomobject]@{ schema_version = 'paper-recovery-state-v1'; attempts = @() }
    }
}

function Save-RecoveryState {
    param([Parameter(Mandatory = $true)][string]$Path, [Parameter(Mandatory = $true)]$State)
    ($State | ConvertTo-Json -Depth 4) | Set-Content -Path $Path -Encoding UTF8
}

# ---------------------------------------------------------------------------
# Bounded backoff: prune attempts older than WindowMinutes, then decide
# whether a NEW attempt is allowed. Never mutates the passed-in state --
# caller decides whether/when to persist the appended attempt.
# ---------------------------------------------------------------------------
function Test-RecoveryBackoffAllowed {
    param(
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][array]$Attempts,
        [Parameter(Mandatory = $true)][int]$MaxAttempts,
        [Parameter(Mandatory = $true)][int]$WindowMinutes,
        [Parameter(Mandatory = $true)][datetime]$NowUtc
    )
    $cutoff = $NowUtc.AddMinutes(-1 * $WindowMinutes)
    $pruned = @($Attempts | Where-Object {
        try { [datetime]::Parse([string]$_, $null, [System.Globalization.DateTimeStyles]::RoundtripKind) -gt $cutoff } catch { $false }
    })
    $allowed = ($pruned.Count -lt $MaxAttempts)
    return [pscustomobject]@{
        Allowed        = $allowed
        PrunedAttempts = $pruned
        Reason         = if ($allowed) { 'WITHIN_BOUNDED_WINDOW' } else { 'BOUNDED_BACKOFF_EXCEEDED' }
    }
}

# ---------------------------------------------------------------------------
# Machine-wide named mutex: at most one recovery invocation of the official
# launcher may be in flight from this supervisor at a time. Overridable
# (shadowable) for tests so functional proofs never take a real named mutex.
# ---------------------------------------------------------------------------
function Enter-RecoveryMutex {
    param([int]$WaitMilliseconds = 3000)
    $createdNew = $false
    $mutex = New-Object System.Threading.Mutex($false, 'Global\MiniQuantDeskPaperRecoveryLauncher', [ref]$createdNew)
    $acquired = $false
    try {
        $acquired = $mutex.WaitOne($WaitMilliseconds)
    } catch [System.Threading.AbandonedMutexException] {
        # A prior holder terminated without releasing -- ownership transfers
        # to us; treat as acquired rather than refusing recovery forever.
        $acquired = $true
    }
    return [pscustomobject]@{ Mutex = $mutex; Acquired = $acquired }
}

function Exit-RecoveryMutex {
    param($MutexHandle)
    if ($null -eq $MutexHandle) { return }
    if ($MutexHandle.Acquired) {
        try { $MutexHandle.Mutex.ReleaseMutex() | Out-Null } catch {}
    }
    try { $MutexHandle.Mutex.Dispose() } catch {}
}

# ---------------------------------------------------------------------------
# The ONE authorized side effect. Hardcodes -Mode Paper -Scheduled -- no
# parameter of this script can change the invoked mode. Overridable
# (shadowable) for tests so functional proofs never actually launch a daemon.
# ---------------------------------------------------------------------------
function Invoke-PaperLauncherProcess {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][int]$TimeoutSeconds
    )
    $launcherPath = Join-Path $RepoRoot 'scripts\windows\Start-MiniQuantDesk.ps1'
    $fullArgs = @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $launcherPath, '-Mode', 'Paper', '-Scheduled')
    $quotedArgs = $fullArgs | ForEach-Object { '"' + ($_ -replace '"', '""') + '"' }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = 'powershell.exe'
    $psi.Arguments = ($quotedArgs -join ' ')
    $psi.WorkingDirectory = $RepoRoot
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $psi
    [void]$process.Start()
    $stdout = $process.StandardOutput.ReadToEnd()
    $stderr = $process.StandardError.ReadToEnd()
    $finished = $process.WaitForExit($TimeoutSeconds * 1000)
    if (-not $finished) {
        try { $process.Kill() } catch {}
        return [pscustomobject]@{ ExitCode = 1; TimedOut = $true; Stdout = $stdout; Stderr = $stderr }
    }
    return [pscustomobject]@{ ExitCode = $process.ExitCode; TimedOut = $false; Stdout = $stdout; Stderr = $stderr }
}

# ---------------------------------------------------------------------------
# Main recovery cycle -- the single entry point exercised by both the real
# dispatch below and by functional tests (which shadow Test-DaemonHealthy,
# Invoke-PaperLauncherProcess, and Enter-RecoveryMutex before calling this).
# ---------------------------------------------------------------------------
function Invoke-PaperRecoveryCycle {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$DaemonBaseUrl,
        [Parameter(Mandatory = $true)][int]$MaxAttempts,
        [Parameter(Mandatory = $true)][int]$WindowMinutes,
        [Parameter(Mandatory = $true)][int]$HealthTimeoutSec,
        [Parameter(Mandatory = $true)][int]$MutexWaitMilliseconds,
        [Parameter(Mandatory = $true)][int]$LauncherTimeoutSeconds
    )

    $health = Test-DaemonHealthy -DaemonBaseUrl $DaemonBaseUrl -TimeoutSec $HealthTimeoutSec
    if ($health.Healthy) {
        return [pscustomobject]@{ Verdict = 'HEALTHY_NO_ACTION'; Detail = $health.Detail; LauncherInvoked = $false; ExitCode = 0 }
    }

    $mutexHandle = Enter-RecoveryMutex -WaitMilliseconds $MutexWaitMilliseconds
    try {
        if (-not $mutexHandle.Acquired) {
            return [pscustomobject]@{ Verdict = 'RECOVERY_ALREADY_IN_FLIGHT'; Detail = 'another recovery invocation holds the mutex'; LauncherInvoked = $false; ExitCode = 1 }
        }

        $statePath = Get-RecoveryStatePath -RepoRoot $RepoRoot
        $state = Read-RecoveryState -Path $statePath
        $nowUtc = (Get-Date).ToUniversalTime()
        $backoff = Test-RecoveryBackoffAllowed -Attempts $state.attempts -MaxAttempts $MaxAttempts -WindowMinutes $WindowMinutes -NowUtc $nowUtc

        if (-not $backoff.Allowed) {
            # Persist the pruned (non-expired) attempts even on refusal, so
            # the window continues to roll forward correctly on next run.
            Save-RecoveryState -Path $statePath -State ([pscustomobject]@{ schema_version = 'paper-recovery-state-v1'; attempts = $backoff.PrunedAttempts })
            return [pscustomobject]@{ Verdict = $backoff.Reason; Detail = "attempts_in_window=$($backoff.PrunedAttempts.Count) max=$MaxAttempts window_minutes=$WindowMinutes"; LauncherInvoked = $false; ExitCode = 1 }
        }

        $newAttempts = @($backoff.PrunedAttempts) + @($nowUtc.ToString('o'))
        Save-RecoveryState -Path $statePath -State ([pscustomobject]@{ schema_version = 'paper-recovery-state-v1'; attempts = $newAttempts })

        $result = Invoke-PaperLauncherProcess -RepoRoot $RepoRoot -TimeoutSeconds $LauncherTimeoutSeconds

        $logPath = Get-RecoveryLogPath -RepoRoot $RepoRoot
        $logEntry = [pscustomobject]@{
            schema_version    = 'paper-recovery-log-v1'
            timestamp         = $nowUtc.ToString('o')
            trigger_reason    = $health.Detail
            launcher_exit_code = $result.ExitCode
            launcher_timed_out = $result.TimedOut
            attempts_in_window = $newAttempts.Count
        }
        ($logEntry | ConvertTo-Json -Depth 4) | Set-Content -Path $logPath -Encoding UTF8

        return [pscustomobject]@{ Verdict = 'RECOVERY_INVOKED'; Detail = "launcher_exit_code=$($result.ExitCode) timed_out=$($result.TimedOut)"; LauncherInvoked = $true; ExitCode = $result.ExitCode }
    } finally {
        Exit-RecoveryMutex -MutexHandle $mutexHandle
    }
}

# =============================================================================
# MAIN DISPATCH -- guarded so tests can dot-source this file (loading every
# function above) without executing a real health check, mutex wait, or
# launcher invocation. Mirrors the same dot-source guard convention used by
# Start-MiniQuantDesk.ps1 / Launch-VeritasLedger.ps1.
# =============================================================================
if ($MyInvocation.InvocationName -ne '.') {
    if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
        $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
    }
    $RepoRoot = $RepoRoot.TrimEnd('\')

    $cycle = Invoke-PaperRecoveryCycle -RepoRoot $RepoRoot -DaemonBaseUrl $DaemonBaseUrl `
        -MaxAttempts $MaxAttempts -WindowMinutes $WindowMinutes -HealthTimeoutSec $HealthTimeoutSec `
        -MutexWaitMilliseconds $MutexWaitMilliseconds -LauncherTimeoutSeconds $LauncherTimeoutSeconds

    switch ($cycle.Verdict) {
        'HEALTHY_NO_ACTION'          { Write-Ok "Daemon healthy ($($cycle.Detail)); no recovery action taken." }
        'RECOVERY_ALREADY_IN_FLIGHT' { Write-Warn "Recovery mutex held by another instance; skipping this run ($($cycle.Detail))." }
        'BOUNDED_BACKOFF_EXCEEDED'   { Write-Fail "Bounded recovery backoff exceeded; refusing to invoke the launcher again this window ($($cycle.Detail))." }
        'RECOVERY_INVOKED'           { Write-Step "Recovery invoked the official launcher ($($cycle.Detail))." }
        default                      { Write-Warn "Unrecognized recovery verdict: $($cycle.Verdict)" }
    }

    exit $cycle.ExitCode
}
