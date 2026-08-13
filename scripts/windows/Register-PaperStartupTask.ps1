# =============================================================================
# PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01
# Register-PaperStartupTask.ps1
#
# Registers (creates or reconciles) a permanent Windows Scheduled Task that
# invokes the official Paper launcher unattended ahead of the accepted
# pre-open operational boundary, using the launcher's own accepted
# scheduled-invocation contract (OFFICIAL-DUAL-MODE-LAUNCHER-01).
#
# SAFETY INVARIANT: the task action invokes ONLY:
#   Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled
# No other launcher argument is added. No other script is invoked as a
# scheduling authority. No credential/token/API-key value is embedded in the
# task action. This helper never calls the daemon, a broker, or any runtime
# HTTP action route itself -- it only registers/reconciles a task definition
# whose action delegates entirely to the official launcher.
#
# TEMPORARY-SOAK COEXISTENCE: this helper never names, inspects, or mutates
# the existing temporary August soak task. A newly created permanent task is
# registered DISABLED by default -- pass -Enable to explicitly activate it.
# An already-existing permanent task's current enabled/disabled state is
# preserved unless -Enable is passed, so re-running this helper to reconcile
# drift never silently flips activation state.
#
# This helper does not remove or forcibly stop any task registration --
# that is intentionally out of scope and left to a separate, deliberate
# operator action.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PaperStartupTask.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PaperStartupTask.ps1 -Enable
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PaperStartupTask.ps1 -StartTime 02:30
#
# Parameters:
#   -RepoRoot    Absolute repo root. Default: resolved from this script's own
#                location (two levels up from scripts\windows\).
#   -TaskName    Permanent task name. Default: MiniQuantDesk-Paper-Preopen-Startup
#   -TaskPath    Task Scheduler folder. Default: \MiniQuantDesk\
#   -StartTime   Local HH:mm trigger time, Monday-Friday. Default: 02:00
#   -Enable      Explicitly activate the permanent task after its definition
#                is reconciled. Without this switch, a brand-new task is left
#                DISABLED and an existing task's current enabled/disabled
#                state is preserved as-is.
#
# Exit codes: 0 = registered/reconciled and self-check passed, 1 = failure
# (prerequisite missing, registration failed, or post-registration
# self-check did not match the intended definition).
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [string] $RepoRoot  = '',
    [string] $TaskName  = 'MiniQuantDesk-Paper-Preopen-Startup',
    [string] $TaskPath  = '\MiniQuantDesk\',
    [string] $StartTime = '02:00',
    [switch] $Enable
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$M) Write-Host "[PAPER-PREOPEN-SCHED] $M"       -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[PAPER-PREOPEN-SCHED] OK: $M"   -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[PAPER-PREOPEN-SCHED] WARN: $M" -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[PAPER-PREOPEN-SCHED] FAIL: $M" -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Resolve repo root canonically from this script's own location unless an
# explicit override is supplied.
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

# ---------------------------------------------------------------------------
# Verify the official launcher exists before registering anything.
# ---------------------------------------------------------------------------
$LauncherPath = Join-Path $RepoRoot 'scripts\windows\Start-MiniQuantDesk.ps1'
if (-not (Test-Path -LiteralPath $LauncherPath)) {
    Write-Fail "Official launcher not found: $LauncherPath"
    Write-Fail 'Ensure Start-MiniQuantDesk.ps1 is present before registering this task.'
    exit 1
}
Write-Ok "Official launcher found: $LauncherPath"

# ---------------------------------------------------------------------------
# Resolve an absolute path to Windows PowerShell -- not a PATH-relative
# lookup, and never System32 used only implicitly as a working directory.
# ---------------------------------------------------------------------------
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
# Build the task action. The PowerShell host uses normal host flags; the
# launcher itself receives ONLY -Mode Paper -Scheduled and no other
# argument.
# ---------------------------------------------------------------------------
$ExpectedArguments = '-NoProfile -NonInteractive -ExecutionPolicy Bypass -File "' + $LauncherPath + '" -Mode Paper -Scheduled'
Write-Step "Task action exe: $WindowsPowerShellPath"
Write-Step "Task action arg: $ExpectedArguments"

# ---------------------------------------------------------------------------
# Trigger: Monday-Friday at $StartTime local time. This is the accepted
# pre-open operational boundary. This helper does not attempt to reproduce
# the NYSE holiday calendar in Task Scheduler -- the daemon's own
# required-universe scheduler already fails closed or reports legitimate
# non-trading-day no-work from authoritative market-calendar truth.
# ---------------------------------------------------------------------------
$trigger = New-ScheduledTaskTrigger -Weekly -DaysOfWeek Monday,Tuesday,Wednesday,Thursday,Friday -At $StartTime

$action = New-ScheduledTaskAction -Execute $WindowsPowerShellPath -Argument $ExpectedArguments -WorkingDirectory $RepoRoot

$settings = New-ScheduledTaskSettingsSet `
    -MultipleInstances IgnoreNew `
    -RestartCount 2 `
    -RestartInterval (New-TimeSpan -Minutes 10) `
    -ExecutionTimeLimit (New-TimeSpan -Hours 1) `
    -StartWhenAvailable `
    -WakeToRun

$principal = New-ScheduledTaskPrincipal `
    -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) `
    -LogonType Interactive `
    -RunLevel Limited

if (-not (Get-Module -Name ScheduledTasks -ListAvailable)) {
    Write-Fail 'ScheduledTasks module not available. Windows 8 / Server 2012+ required.'
    exit 1
}

# ---------------------------------------------------------------------------
# Idempotent create-or-update. An existing task's current enabled/disabled
# state is preserved unless -Enable is passed. A brand-new task is always
# left DISABLED unless -Enable is passed -- during temporary-soak
# coexistence, two enabled tasks could invoke the official Paper launcher
# concurrently.
# ---------------------------------------------------------------------------
Write-Sect 'Register/reconcile permanent Paper pre-open startup task'
Write-Step "Task name : $TaskName"
Write-Step "Task path : $TaskPath"
Write-Step "Trigger   : Monday-Friday at $StartTime local time"

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

try {
    if ($taskExistedBefore) {
        Write-Step "Task exists at '$TaskPath'; reconciling definition (update path: action/trigger/settings/principal)..."
        Set-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath `
            -Action $action -Trigger $trigger -Settings $settings -Principal $principal | Out-Null
        Write-Ok "Task '$TaskName' definition reconciled."
    } else {
        Write-Step "Task does not exist at '$TaskPath'; creating (registration path)..."
        Register-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath `
            -Action $action -Trigger $trigger -Settings $settings -Principal $principal -Force | Out-Null
        Write-Ok "Task '$TaskName' created."
    }
} catch {
    Write-Fail "Failed to register/update task: $_"
    exit 1
}

try {
    if ($desiredEnabled) {
        Enable-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath | Out-Null
    } else {
        Disable-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath | Out-Null
    }
} catch {
    Write-Fail "Failed to set activation state: $_"
    exit 1
}

# ---------------------------------------------------------------------------
# Post-registration self-check: re-read the task from Task Scheduler (not
# the in-memory objects just built) and fail the helper if reality does not
# match the intended definition.
# ---------------------------------------------------------------------------
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
    $readBackEnabled = ($readBack.State -ne 'Disabled')
    if ($readBackEnabled -ne $desiredEnabled) {
        $selfCheckFailures += 'activation_state_mismatch'
    }
}

if ($selfCheckFailures.Count -gt 0) {
    Write-Fail "Post-registration self-check FAILED: $($selfCheckFailures -join '; ')"
    exit 1
}
Write-Ok 'Post-registration self-check passed: exactly one action, executable/arguments/working directory match, activation state matches.'

Write-Sect 'Registration complete'
Write-Ok "Task '$TaskName' at '$TaskPath' will run Monday-Friday at $StartTime local time."
Write-Ok "Activation state: $(if ($desiredEnabled) { 'ENABLED' } else { 'DISABLED' })"
if (-not $desiredEnabled) {
    Write-Warn 'Task registered DISABLED (temporary-soak coexistence). Pass -Enable to activate once accepted.'
}
Write-Ok 'Principal: Interactive logon, Limited run level -- requires the desktop user to remain signed in (session may be locked).'
Write-Ok "To verify: Get-ScheduledTask -TaskName '$TaskName' -TaskPath '$TaskPath'"
exit 0
