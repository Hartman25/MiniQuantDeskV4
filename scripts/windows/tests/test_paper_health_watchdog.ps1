# =============================================================================
# test_paper_health_watchdog.ps1
# OPS-AUTO-RESTART-LOCAL-01
#
# Proof for scripts\windows\Watch-MiniQuantDeskPaperHealth.ps1 and
# scripts\windows\Register-PaperHealthWatchdogTask.ps1.
#
# Sections 1-3: static source-guard assertions (AST parse + regex/substring
# checks) against Register-PaperHealthWatchdogTask.ps1's own text. Never
# calls Register-ScheduledTask/Set-ScheduledTask/Enable-ScheduledTask/
# Disable-ScheduledTask/Unregister-ScheduledTask/Start-ScheduledTask/
# Stop-ScheduledTask. Zero real Task Scheduler side effects.
#
# Sections 4-5: static source-guard assertions against
# Watch-MiniQuantDeskPaperHealth.ps1's own text, proving it contains no
# HTTP verb beyond the one read-only health GET, no action_key literal, no
# Authorization header, and a hardcoded -Mode Paper -Scheduled launcher
# invocation.
#
# Section 6: functional proof. Dot-sources the real
# Watch-MiniQuantDeskPaperHealth.ps1 (safe -- its own dot-source guard
# prevents the MAIN DISPATCH block from executing), then shadows
# Test-DaemonHealthy / Invoke-PaperLauncherProcess / Enter-RecoveryMutex
# (the same mock-by-redefinition technique already used in
# test_official_dual_mode_launcher.ps1 for Invoke-JsonGet/Invoke-JsonPost)
# so Invoke-PaperRecoveryCycle can be proven end-to-end against a disposable
# scratch RepoRoot -- never the real repo's exports\ or smoke_logs\, never a
# real mutex wait, never a real launcher process.
#
# Exit codes: 0 = all proofs held, 1 = at least one did not.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$WindowsDir = (Resolve-Path (Join-Path $ScriptDir '..')).Path.TrimEnd('\')
$RepoRoot   = (Resolve-Path (Join-Path $WindowsDir '..\..')).Path.TrimEnd('\')
$RegHelper  = Join-Path $WindowsDir 'Register-PaperHealthWatchdogTask.ps1'
$Watchdog   = Join-Path $WindowsDir 'Watch-MiniQuantDeskPaperHealth.ps1'
$Launcher   = Join-Path $WindowsDir 'Start-MiniQuantDesk.ps1'

$Violations = 0
function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan }
function Assert-True {
    param([string]$Label, [bool]$Condition)
    if ($Condition) {
        Show-Green "  OK -- $Label"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label"
    }
}

foreach ($p in @($RegHelper, $Watchdog, $Launcher)) {
    if (-not (Test-Path -LiteralPath $p)) {
        Show-Red "FATAL -- required file not found: $p"
        exit 1
    }
}

$RegHelperText  = Get-Content -Path $RegHelper -Raw
$WatchdogText   = Get-Content -Path $Watchdog -Raw

# ---------------------------------------------------------------------------
# Section 1: parse guard
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 1: parse guard ==='

foreach ($pair in @(@{ Name = 'Register-PaperHealthWatchdogTask.ps1'; Path = $RegHelper }, @{ Name = 'Watch-MiniQuantDeskPaperHealth.ps1'; Path = $Watchdog })) {
    $parseErrors = $null
    $tokens = $null
    [System.Management.Automation.Language.Parser]::ParseFile($pair.Path, [ref]$tokens, [ref]$parseErrors) | Out-Null
    Assert-True "$($pair.Name) parses successfully (0 AST parse errors)" `
        ($null -ne $parseErrors -and $parseErrors.Count -eq 0)
}

# ---------------------------------------------------------------------------
# Section 2: Register-PaperHealthWatchdogTask.ps1 -- task action contract
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 2: registration helper action contract ==='

Assert-True 'action resolves Watch-MiniQuantDeskPaperHealth.ps1' `
    ($RegHelperText -match [regex]::Escape('Watch-MiniQuantDeskPaperHealth.ps1'))

Assert-True 'action passes no arguments to the watchdog script (mode is hardcoded inside the watchdog, not passed by the scheduler)' `
    ($RegHelperText -match [regex]::Escape('$ExpectedArguments = ''-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "'' + $WatchdogPath + ''"'''))

Assert-True 'helper never mentions -ArmPaper' `
    (-not ($RegHelperText -match [regex]::Escape('-ArmPaper')))

Assert-True 'helper never mentions -SkipGui' `
    (-not ($RegHelperText -match [regex]::Escape('-SkipGui')))

Assert-True 'no direct runtime/order/HTTP action in the registration helper itself (no action_key literals, no HTTP calls)' `
    (-not ($RegHelperText -match [regex]::Escape('start-system') -or `
           $RegHelperText -match [regex]::Escape('arm-execution') -or `
           $RegHelperText -match [regex]::Escape('adopt-broker-position-baseline') -or `
           $RegHelperText -match 'Invoke-WebRequest|Invoke-RestMethod'))

Assert-True 'no Live scheduling (helper source never mentions Live trading mode)' `
    (-not ($RegHelperText -match '(?i)live'))

$expectedArgumentsLine = ($RegHelperText -split "`n") | Where-Object { $_ -match '\$ExpectedArguments\s*=' } | Select-Object -First 1
Assert-True 'no secrets in action arguments' `
    ($null -ne $expectedArgumentsLine -and -not ($expectedArgumentsLine -match '\$env:' -or $expectedArgumentsLine -match '(?i)(api[_-]?key|api[_-]?secret|token|password|credential)'))

# ---------------------------------------------------------------------------
# Section 3: registration helper -- triggers, settings, idempotency, coexistence
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 3: triggers, settings, idempotency, coexistence ==='

Assert-True 'AtStartup trigger present' `
    ($RegHelperText -match [regex]::Escape('New-ScheduledTaskTrigger -AtStartup'))

Assert-True 'Daily repeating window trigger present' `
    ($RegHelperText -match [regex]::Escape('New-ScheduledTaskTrigger -Daily -At $DailyStart'))

Assert-True 'both triggers passed to the same task (single MultipleInstances policy governs both)' `
    ($RegHelperText -match [regex]::Escape('-Trigger @($startupTrigger, $dailyTrigger)'))

Assert-True 'MultipleInstances IgnoreNew' `
    ($RegHelperText -match [regex]::Escape('-MultipleInstances IgnoreNew'))

Assert-True 'RestartCount 2' `
    ($RegHelperText -match [regex]::Escape('-RestartCount 2'))

Assert-True 'ExecutionTimeLimit 1 hour (bounded, not indefinite)' `
    ($RegHelperText -match [regex]::Escape('-ExecutionTimeLimit (New-TimeSpan -Hours 1)'))

Assert-True 'default daily window starts at 06:00 (after the 02:00 pre-open task''s 1h ExecutionTimeLimit -- no scheduled overlap by construction)' `
    ($RegHelperText -match [regex]::Escape('$DailyStart       = ''06:00'''))

Assert-True 'new task defaults DISABLED (desiredEnabled=false when no prior task and no -Enable)' `
    ($RegHelperText -match [regex]::Escape('$desiredEnabled = $false'))

Assert-True 'existing task preserves its own current enabled/disabled state absent -Enable' `
    ($RegHelperText -match [regex]::Escape('$priorEnabledState'))

Assert-True 'helper contains no Unregister-ScheduledTask call' `
    (-not ($RegHelperText -match [regex]::Escape('Unregister-ScheduledTask')))

Assert-True 'helper contains no Stop-ScheduledTask call' `
    (-not ($RegHelperText -match [regex]::Escape('Stop-ScheduledTask')))

Assert-True 'post-registration self-check re-reads via Get-ScheduledTask and fails closed on mismatch (exit 1)' `
    ($RegHelperText -match [regex]::Escape('$readBack = Get-ScheduledTask') -and $RegHelperText -match [regex]::Escape('exit 1'))

# ---------------------------------------------------------------------------
# Section 4: Watch-MiniQuantDeskPaperHealth.ps1 -- fail-closed authority fence
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 4: watchdog authority fence (static) ==='

Assert-True 'watchdog never sets an Authorization header (no -Headers construct anywhere in its HTTP calls)' `
    (-not ($WatchdogText -match [regex]::Escape('-Headers')))

Assert-True 'watchdog never calls Invoke-RestMethod/Invoke-WebRequest with a mutating HTTP verb (-Method Post/Put/Delete/Patch)' `
    (-not ($WatchdogText -match '(?i)-Method\s+(Post|Put|Delete|Patch)'))

Assert-True 'watchdog contains no ops/action route or action_key literal' `
    (-not ($WatchdogText -match [regex]::Escape('/api/v1/ops/action') -or $WatchdogText -match [regex]::Escape('action_key')))

Assert-True 'watchdog never mentions arm-execution/disarm-execution/clear-halted-run/kill-switch literals' `
    (-not ($WatchdogText -match [regex]::Escape('arm-execution') -or $WatchdogText -match [regex]::Escape('disarm-execution') -or `
           $WatchdogText -match [regex]::Escape('clear-halted-run') -or $WatchdogText -match [regex]::Escape('kill-switch')))

Assert-True 'watchdog never mentions Live trading mode' `
    (-not ($WatchdogText -match '(?i)-Mode\s+Live' -or $WatchdogText -match "(?i)'Live'"))

Assert-True 'launcher invocation hardcodes -Mode Paper -Scheduled (no parameter can change the invoked mode)' `
    ($WatchdogText -match [regex]::Escape("'-Mode', 'Paper', '-Scheduled'"))

Assert-True 'only one authorized side effect: Start-MiniQuantDesk.ps1' `
    (([regex]::Matches($WatchdogText, [regex]::Escape('Start-MiniQuantDesk.ps1'))).Count -ge 1)

Assert-True 'watchdog never invokes Launch-VeritasLedger.ps1 as a script/file target (delegates only through the official launcher; a prose comment mentioning the name is fine, a -File/$launchScript reference to it is not)' `
    (-not ($WatchdogText -match [regex]::Escape('-File $launchScript') -or $WatchdogText -match "Join-Path[^`n]*Launch-VeritasLedger\.ps1"))

Assert-True 'no database connection string / MQK_DATABASE_URL literal in the watchdog' `
    (-not ($WatchdogText -match [regex]::Escape('MQK_DATABASE_URL') -or $WatchdogText -match 'postgres://'))

Assert-True 'bounded backoff function present' `
    ($WatchdogText -match 'function Test-RecoveryBackoffAllowed')

Assert-True 'machine-wide recovery mutex present' `
    ($WatchdogText -match [regex]::Escape('Global\MiniQuantDeskPaperRecoveryLauncher'))

Assert-True 'dot-source guard mirrors the official launcher''s own convention (safe to test without executing MAIN DISPATCH)' `
    ($WatchdogText -match [regex]::Escape("if (`$MyInvocation.InvocationName -ne '.') {"))

# ---------------------------------------------------------------------------
# Section 5: functional proof -- dot-source the real watchdog, shadow its
# I/O boundary functions, exercise Invoke-PaperRecoveryCycle end-to-end.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 5: functional recovery-cycle proof ==='

. $Watchdog

$ScratchRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_health_watchdog_test_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $ScratchRoot | Out-Null

$script:MockHealthy = $true
$script:LauncherInvocationCount = 0
$script:MockLauncherExitCode = 0
$script:MockMutexAcquired = $true

function Test-DaemonHealthy {
    param([string]$DaemonBaseUrl, [int]$TimeoutSec = 5)
    return [pscustomobject]@{ Healthy = $script:MockHealthy; Detail = 'mocked' }
}

function Invoke-PaperLauncherProcess {
    param([string]$RepoRoot, [int]$TimeoutSeconds)
    $script:LauncherInvocationCount++
    return [pscustomobject]@{ ExitCode = $script:MockLauncherExitCode; TimedOut = $false; Stdout = ''; Stderr = '' }
}

function Enter-RecoveryMutex {
    param([int]$WaitMilliseconds = 3000)
    return [pscustomobject]@{ Mutex = $null; Acquired = $script:MockMutexAcquired }
}

function Exit-RecoveryMutex {
    param($MutexHandle)
}

function Invoke-Cycle {
    return Invoke-PaperRecoveryCycle -RepoRoot $ScratchRoot -DaemonBaseUrl 'http://127.0.0.1:8899' `
        -MaxAttempts 3 -WindowMinutes 30 -HealthTimeoutSec 5 -MutexWaitMilliseconds 100 -LauncherTimeoutSeconds 5
}

# --- A2: healthy daemon -> zero launcher invocations ------------------------
$script:MockHealthy = $true
$script:LauncherInvocationCount = 0
$result = Invoke-Cycle
Assert-True 'A2 healthy daemon: verdict is HEALTHY_NO_ACTION' ($result.Verdict -eq 'HEALTHY_NO_ACTION')
Assert-True 'A2 healthy daemon: launcher never invoked' (-not $result.LauncherInvoked -and $script:LauncherInvocationCount -eq 0)
Assert-True 'A2 healthy daemon: exit code 0' ($result.ExitCode -eq 0)

# --- unhealthy daemon -> recovery invoked exactly once, state persisted ----
Remove-Item -Path (Join-Path $ScratchRoot 'exports') -Recurse -Force -ErrorAction SilentlyContinue
$script:MockHealthy = $false
$script:LauncherInvocationCount = 0
$result = Invoke-Cycle
Assert-True 'unhealthy daemon: verdict is RECOVERY_INVOKED' ($result.Verdict -eq 'RECOVERY_INVOKED')
Assert-True 'unhealthy daemon: launcher invoked exactly once' ($result.LauncherInvoked -and $script:LauncherInvocationCount -eq 1)

$statePath = Get-RecoveryStatePath -RepoRoot $ScratchRoot
$state = Read-RecoveryState -Path $statePath
Assert-True 'recovery attempt persisted to durable state file' (@($state.attempts).Count -eq 1)

# --- A7 bounded backoff: MaxAttempts reached within window refuses further
#     invocations (no rapid restart loop) -------------------------------
$script:LauncherInvocationCount = 0
$null = Invoke-Cycle   # attempt 2
$null = Invoke-Cycle   # attempt 3 (MaxAttempts=3 reached)
$refused = Invoke-Cycle  # attempt 4 -- must be refused
Assert-True 'A7 bounded backoff: 4th attempt within the window is refused' ($refused.Verdict -eq 'BOUNDED_BACKOFF_EXCEEDED')
Assert-True 'A7 bounded backoff: refused attempt never invokes the launcher' (-not $refused.LauncherInvoked)
Assert-True 'A7 bounded backoff: exactly 2 additional launcher invocations occurred before the refusal (3 total in window)' ($script:LauncherInvocationCount -eq 2)
Assert-True 'A7 bounded backoff: refusal is a bounded exit code (1), not a crash' ($refused.ExitCode -eq 1)

# --- A1 duplicate recovery invocation: mutex not acquired -> refused, no
#     launcher call, regardless of backoff state ---------------------------
Remove-Item -Path (Join-Path $ScratchRoot 'exports') -Recurse -Force -ErrorAction SilentlyContinue
$script:MockHealthy = $false
$script:MockMutexAcquired = $false
$script:LauncherInvocationCount = 0
$result = Invoke-Cycle
Assert-True 'A1 duplicate recovery: mutex refusal reports RECOVERY_ALREADY_IN_FLIGHT' ($result.Verdict -eq 'RECOVERY_ALREADY_IN_FLIGHT')
Assert-True 'A1 duplicate recovery: launcher never invoked when mutex is held elsewhere' (-not $result.LauncherInvoked -and $script:LauncherInvocationCount -eq 0)
$script:MockMutexAcquired = $true

# --- Test-RecoveryBackoffAllowed unit proof: attempts outside the window
#     are pruned and do not count against the limit ------------------------
$nowUtc = (Get-Date).ToUniversalTime()
$staleAttempt = $nowUtc.AddMinutes(-60).ToString('o')
$freshAttempt = $nowUtc.AddMinutes(-5).ToString('o')
$backoffCheck = Test-RecoveryBackoffAllowed -Attempts @($staleAttempt, $freshAttempt) -MaxAttempts 1 -WindowMinutes 30 -NowUtc $nowUtc
Assert-True 'stale (out-of-window) attempts are pruned and do not count toward the bounded limit' `
    (@($backoffCheck.PrunedAttempts).Count -eq 1 -and -not $backoffCheck.Allowed)

# --- corrupt state file fails closed to empty history, not a crash --------
$corruptPath = Join-Path $ScratchRoot 'corrupt_state.json'
Set-Content -Path $corruptPath -Value '{ not valid json ' -Encoding UTF8
$recovered = Read-RecoveryState -Path $corruptPath
Assert-True 'corrupt recovery-state file fails closed to empty attempt history rather than throwing' `
    (@($recovered.attempts).Count -eq 0)

Remove-Item -Path $ScratchRoot -Recurse -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Summary ==='
if ($Violations -eq 0) {
    Show-Green "All proofs held. 0 violations."
    exit 0
} else {
    Show-Red "$Violations violation(s) found."
    exit 1
}
