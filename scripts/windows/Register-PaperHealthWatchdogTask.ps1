# =============================================================================
# OPS-AUTO-RESTART-LOCAL-01
# Register-PaperHealthWatchdogTask.ps1
#
# Registers (creates or reconciles) a permanent Windows Scheduled Task that
# runs the bounded local health/recovery supervisor
# (Watch-MiniQuantDeskPaperHealth.ps1) unattended, so a daemon crash that
# happens AFTER the existing pre-open task
# (MiniQuantDesk-Paper-Preopen-Startup, see Register-PaperStartupTask.ps1)
# has already completed successfully is still recovered the same trading
# day, instead of waiting for tomorrow's 02:00 pre-open trigger.
#
# SAFETY INVARIANT: the task action invokes ONLY:
#   Watch-MiniQuantDeskPaperHealth.ps1
# with no arguments. That script's own single authorized side effect is
# delegating to the official launcher
# (Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled) -- see that script's own
# header. This helper never calls the daemon, a broker, or any runtime HTTP
# action route itself, and never embeds a credential/token/API-key value in
# the task action.
#
# Two triggers are registered on the SAME task (so Task Scheduler's own
# MultipleInstances=IgnoreNew instance policy applies across both):
#   - AtStartup:  near-immediate health check after a reboot, rather than
#     waiting up to -IntervalMinutes for the next repetition tick.
#   - Daily repetition: fires at -DailyStart, then repeats every
#     -IntervalMinutes for -DurationHours, covering an ordinary trading
#     session so a mid-day crash is caught and recovered without waiting
#     for the next calendar day's pre-open task.
#
# The daily repetition window default (03:05) starts shortly after the
# existing 02:00 pre-open task's own 1-hour ExecutionTimeLimit (which ends by
# 03:00), so the two permanent tasks' own scheduled fire times never overlap
# by construction (defense in depth on top of
# Watch-MiniQuantDeskPaperHealth.ps1's own machine-wide recovery mutex,
# post-mutex health recheck, pre-open-task-running fence, and bounded
# backoff, which guard any residual overlap). This repo is operated in
# Hawaii local time, where US equity regular-market open occurs well before
# a 06:00 local start would cover -- 03:05 closes that early-session gap
# without cutting into the pre-open task's own execution window.
#
# TEMPORARY-SOAK COEXISTENCE: mirrors Register-PaperStartupTask.ps1 exactly
# -- a newly created permanent task is registered DISABLED by default; pass
# -Enable to explicitly activate it. An already-existing permanent task's
# current enabled/disabled state is preserved unless -Enable is passed.
#
# This helper does not remove or forcibly stop any task registration --
# that is intentionally out of scope and left to a separate, deliberate
# operator action.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PaperHealthWatchdogTask.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PaperHealthWatchdogTask.ps1 -Enable
#
# Parameters:
#   -RepoRoot         Absolute repo root. Default: resolved from this
#                      script's own location.
#   -TaskName         Permanent task name. Default:
#                      MiniQuantDesk-Paper-HealthWatchdog
#   -TaskPath         Task Scheduler folder. Default: \MiniQuantDesk\
#   -DailyStart       Local HH:mm the daily repetition window begins.
#                      Default: 03:05 (shortly after the 02:00 pre-open
#                      task's 1-hour ExecutionTimeLimit ends at 03:00)
#   -IntervalMinutes  Minutes between health checks within the window.
#                      Default: 15
#   -DurationHours    Hours the repetition window lasts from -DailyStart.
#                      Default: 16
#   -Enable           Explicitly activate the permanent task after its
#                      definition is reconciled.
#
# Exit codes: 0 = registered/reconciled and self-check passed, 1 = failure.
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [string] $RepoRoot         = '',
    [string] $TaskName         = 'MiniQuantDesk-Paper-HealthWatchdog',
    [string] $TaskPath         = '\MiniQuantDesk\',
    [string] $DailyStart       = '03:05',
    [ValidateRange(1, [int]::MaxValue)][int] $IntervalMinutes  = 15,
    [ValidateRange(1, [int]::MaxValue)][int] $DurationHours    = 16,
    [switch] $Enable
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$M) Write-Host "[PAPER-HEALTH-WATCHDOG-SCHED] $M"       -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[PAPER-HEALTH-WATCHDOG-SCHED] OK: $M"   -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[PAPER-HEALTH-WATCHDOG-SCHED] WARN: $M" -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[PAPER-HEALTH-WATCHDOG-SCHED] FAIL: $M" -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

$WatchdogPath = Join-Path $RepoRoot 'scripts\windows\Watch-MiniQuantDeskPaperHealth.ps1'
if (-not (Test-Path -LiteralPath $WatchdogPath)) {
    Write-Fail "Health watchdog script not found: $WatchdogPath"
    exit 1
}
Write-Ok "Health watchdog script found: $WatchdogPath"

$LauncherPath = Join-Path $RepoRoot 'scripts\windows\Start-MiniQuantDesk.ps1'
if (-not (Test-Path -LiteralPath $LauncherPath)) {
    Write-Fail "Official launcher not found: $LauncherPath"
    exit 1
}
Write-Ok "Official launcher found (delegated to by the watchdog script): $LauncherPath"

$WindowsPowerShellPath = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
if (-not (Test-Path -LiteralPath $WindowsPowerShellPath)) {
    $cmd = Get-Command 'powershell.exe' -ErrorAction SilentlyContinue
    if ($null -eq $cmd) {
        Write-Fail 'Could not resolve an absolute path to Windows PowerShell (powershell.exe).'
        exit 1
    }
    $WindowsPowerShellPath = $cmd.Source
}
Write-Ok "Windows PowerShell resolved: $WindowsPowerShellPath"

# ---------------------------------------------------------------------------
# Task action invokes ONLY the watchdog script, with no arguments -- the
# watchdog script itself hardcodes -Mode Paper -Scheduled for its one
# authorized launcher invocation.
# ---------------------------------------------------------------------------
$ExpectedArguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $WatchdogPath + '"'
Write-Step "Task action exe: $WindowsPowerShellPath"
Write-Step "Task action arg: $ExpectedArguments"

if (-not (Get-Module -Name ScheduledTasks -ListAvailable)) {
    Write-Fail 'ScheduledTasks module not available. Windows 8 / Server 2012+ required.'
    exit 1
}

$existingTask = Get-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath -ErrorAction SilentlyContinue
$taskExistedBefore = ($null -ne $existingTask)
$priorEnabledState = if ($taskExistedBefore) { ($existingTask.State -ne 'Disabled') } else { $null }

if ($Enable) {
    $desiredEnabled = $true
} elseif ($taskExistedBefore) {
    $desiredEnabled = $priorEnabledState
} else {
    $desiredEnabled = $false
}

# ---------------------------------------------------------------------------
# Triggers: AtStartup (near-immediate post-reboot check) + a daily window
# that repeats every -IntervalMinutes for -DurationHours. The Repetition
# properties are set on the trigger CIM object after construction (ISO 8601
# duration strings) -- New-ScheduledTaskTrigger's -RepetitionInterval /
# -RepetitionDuration parameters are only defined for the -Once parameter
# set, not -Daily, so this is the standard supported way to get a
# daily-recurring repeating trigger.
# ---------------------------------------------------------------------------
$startupTrigger = New-ScheduledTaskTrigger -AtStartup
$dailyTrigger = New-ScheduledTaskTrigger -Daily -At $DailyStart
$dailyTrigger.Repetition.Duration = "PT$($DurationHours)H"
$dailyTrigger.Repetition.Interval = "PT$($IntervalMinutes)M"

$action = New-ScheduledTaskAction -Execute $WindowsPowerShellPath -Argument $ExpectedArguments -WorkingDirectory $RepoRoot

if ($desiredEnabled) {
    $settings = New-ScheduledTaskSettingsSet `
        -MultipleInstances IgnoreNew `
        -RestartCount 2 `
        -RestartInterval (New-TimeSpan -Minutes 10) `
        -ExecutionTimeLimit (New-TimeSpan -Hours 1) `
        -StartWhenAvailable `
        -WakeToRun
} else {
    $settings = New-ScheduledTaskSettingsSet `
        -MultipleInstances IgnoreNew `
        -RestartCount 2 `
        -RestartInterval (New-TimeSpan -Minutes 10) `
        -ExecutionTimeLimit (New-TimeSpan -Hours 1) `
        -StartWhenAvailable `
        -WakeToRun `
        -Disable
}

$principal = New-ScheduledTaskPrincipal `
    -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) `
    -LogonType Interactive `
    -RunLevel Limited

Write-Sect 'Register/reconcile permanent Paper health-watchdog task'
Write-Step "Task name : $TaskName"
Write-Step "Task path : $TaskPath"
Write-Step "Triggers  : AtStartup + Daily at $DailyStart repeating every $IntervalMinutes min for $DurationHours h"

try {
    if ($taskExistedBefore) {
        Write-Step "Task exists at '$TaskPath'; reconciling definition (update path: action/trigger/settings/principal)..."
        Set-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath `
            -Action $action -Trigger @($startupTrigger, $dailyTrigger) -Settings $settings -Principal $principal | Out-Null
        Write-Ok "Task '$TaskName' definition reconciled."
    } else {
        Write-Step "Task does not exist at '$TaskPath'; creating (registration path)..."
        Register-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath `
            -Action $action -Trigger @($startupTrigger, $dailyTrigger) -Settings $settings -Principal $principal -Force | Out-Null
        Write-Ok "Task '$TaskName' created."
    }
} catch {
    Write-Fail "Failed to register/update task: $_"
    exit 1
}

if ($desiredEnabled) {
    try {
        Enable-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath | Out-Null
    } catch {
        Write-Fail "Failed to set activation state: $_"
        exit 1
    }
}

Write-Sect 'Post-registration self-check'
$readBack = Get-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath -ErrorAction SilentlyContinue
$selfCheckFailures = @()

if ($null -eq $readBack) {
    $selfCheckFailures += 'task_not_found_after_registration'
} else {
    if (@($readBack.Actions).Count -ne 1) {
        $selfCheckFailures += "expected_exactly_one_action_found_$(@($readBack.Actions).Count)"
    } else {
        $readAction = $readBack.Actions[0]
        if ($readAction.Execute -ne $WindowsPowerShellPath) {
            $selfCheckFailures += 'action_executable_mismatch'
        }
        if ($readAction.Arguments -ne $ExpectedArguments) {
            $selfCheckFailures += 'action_arguments_mismatch'
        }
        if ($readAction.WorkingDirectory.TrimEnd('\') -ne $RepoRoot) {
            $selfCheckFailures += 'working_directory_mismatch'
        }
    }
    if (@($readBack.Triggers).Count -ne 2) {
        $selfCheckFailures += "expected_exactly_two_triggers_found_$(@($readBack.Triggers).Count)"
    }
    $readBackEnabled = ($readBack.State -ne 'Disabled')
    if ($readBackEnabled -ne $desiredEnabled) {
        $selfCheckFailures += 'activation_state_mismatch'
    }
}

if ($selfCheckFailures.Count -gt 0) {
    Write-Fail "Post-registration self-check FAILED: $($selfCheckFailures -join '; ')"
    exit 1
}
Write-Ok 'Post-registration self-check passed: exactly one action, two triggers, executable/arguments/working directory match, activation state matches.'

Write-Sect 'Registration complete'
Write-Ok "Task '$TaskName' at '$TaskPath' will run at startup and daily $DailyStart-$([TimeSpan]::FromHours($DurationHours))."
Write-Ok "Activation state: $(if ($desiredEnabled) { 'ENABLED' } else { 'DISABLED' })"
if (-not $desiredEnabled) {
    Write-Warn 'Task registered DISABLED (temporary-soak coexistence). Pass -Enable to activate once accepted.'
}
Write-Ok 'Principal: Interactive logon, Limited run level -- requires the desktop user to remain signed in (session may be locked).'
Write-Ok "To verify: Get-ScheduledTask -TaskName '$TaskName' -TaskPath '$TaskPath'"
exit 0
