# =============================================================================
# PREMARKET-DATA-SCHEDULER-01
# Register-PremarketDataRefreshTask.ps1
#
# Creates, updates, or removes a Windows Scheduled Task that runs premarket
# market-data refresh before market open on weekdays.
#
# SAFETY INVARIANT: this script calls ONLY Prep-PremarketMarketData.ps1.
# It does NOT start the daemon, runtime, or trading.  It does NOT call
# Start-PaperTradingSmoke.ps1.  It does NOT enqueue orders.
#
# Usage:
#   # Show what would be registered (no changes):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PremarketDataRefreshTask.ps1 -CheckOnly
#
#   # Register (create or update) the scheduled task:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PremarketDataRefreshTask.ps1
#
#   # Unregister (remove) the scheduled task:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-PremarketDataRefreshTask.ps1 -Unregister
#
# Parameters:
#   -TaskName          Windows Task Scheduler task name.
#                      Default: MiniQuantDesk-PremarketDataRefresh
#   -RepoRoot          Absolute path to repo root.
#                      Default: auto-resolved two levels up from this script.
#   -Symbols           Comma-separated symbol list passed to prep script.
#                      Default: AAPL
#   -Timeframe         Bar timeframe passed to prep script. Default: 1D
#   -MinCompletedBars  Minimum bars gate passed to prep script. Default: 30
#   -TriggerTimeLocal  Local time to run daily Mon-Fri. Default: 08:30
#                      NOTE: local machine time -- set to at least 1 hour before
#                      your intended market open observation window.
#                      NYSE opens 09:30 ET; default 08:30 local allows for drift
#                      only if machine timezone is ET.  Adjust for your timezone.
#   -CheckOnly         Validate config and show what would be registered.
#                      No task is created, updated, or removed.
#   -Unregister        Remove the scheduled task if it exists.
#
# Logs and evidence:
#   Each scheduled run writes a transcript to:
#     <RepoRoot>\exports\market_data\scheduled\refresh_YYYYMMDD_HHMMSS.log
#   The prep script also writes a JSON evidence file to:
#     <RepoRoot>\exports\market_data\premarket_prep_YYYYMMDD_HHMMSS.json
#   A task registration record is written to:
#     <RepoRoot>\exports\market_data\scheduled\task_registration.json
#
# Requirements:
#   - PowerShell 5.1+
#   - Windows 8 / Server 2012 or later (ScheduledTasks module)
#   - No elevated privileges required (task runs as current interactive user)
#
# Hard rules enforced by test_premarket_data_scheduler.ps1:
#   - Never references Start-PaperTradingSmoke.ps1 as a task action
#   - Never references start-system, arm-execution, adopt-broker-position-baseline
#   - CheckOnly exits without any mutations
#   - Unregister path exists
#   - Evidence path is deterministic under exports/market_data/scheduled/
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [string] $TaskName         = 'MiniQuantDesk-PremarketDataRefresh',
    [string] $RepoRoot         = '',
    [string] $Symbols          = 'AAPL',
    [string] $Timeframe        = '1D',
    [int]    $MinCompletedBars = 30,
    [string] $TriggerTimeLocal = '08:30',
    [switch] $CheckOnly,
    [switch] $Unregister
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
function Write-Step { param([string]$M) Write-Host "[SCHED] $M"         -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[SCHED] OK: $M"     -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[SCHED] WARN: $M"   -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[SCHED] FAIL: $M"   -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Resolve repo root
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

# ---------------------------------------------------------------------------
# Validate prep script exists
# ---------------------------------------------------------------------------
$PrepScript = Join-Path $RepoRoot 'scripts\windows\Prep-PremarketMarketData.ps1'
if (-not (Test-Path $PrepScript)) {
    Write-Fail "Prep script not found: $PrepScript"
    Write-Fail "Ensure Prep-PremarketMarketData.ps1 is present before registering the task."
    exit 1
}
Write-Ok "Prep script found: $PrepScript"

# ---------------------------------------------------------------------------
# Resolve paths
# ---------------------------------------------------------------------------
$ScheduledLogDir = Join-Path $RepoRoot 'exports\market_data\scheduled'
$RegRecord       = Join-Path $ScheduledLogDir 'task_registration.json'

# ---------------------------------------------------------------------------
# Build the task action argument string.
# The action runs powershell.exe and:
#   1. Creates the scheduled log directory if absent.
#   2. Starts a transcript for this run.
#   3. Calls Prep-PremarketMarketData.ps1 with configured parameters.
#   4. Stops the transcript.
#
# SAFETY: the action references ONLY Prep-PremarketMarketData.ps1.
# It does not call Start-PaperTradingSmoke.ps1 or any runtime command.
# ---------------------------------------------------------------------------
$PrepScriptEsc   = $PrepScript   -replace "'", "''"
$ScheduledLogEsc = $ScheduledLogDir -replace "'", "''"
$SymbolsEsc      = $Symbols      -replace "'", "''"
$TimeframeEsc    = $Timeframe    -replace "'", "''"

$innerCommand = (
    "`$d='$ScheduledLogEsc';" +
    "[IO.Directory]::CreateDirectory(`$d)|Out-Null;" +
    "`$f=Join-Path `$d ('refresh_'+(Get-Date -f 'yyyyMMdd_HHmmss')+'.log');" +
    "Start-Transcript `$f -Append|Out-Null;" +
    "try{& '$PrepScriptEsc' -Symbols '$SymbolsEsc' -Timeframe '$TimeframeEsc' -MinCompletedBars $MinCompletedBars}" +
    "finally{Stop-Transcript|Out-Null}"
)
$TaskArgument = "-ExecutionPolicy Bypass -NonInteractive -WindowStyle Hidden -Command `"$innerCommand`""

# ---------------------------------------------------------------------------
# Build trigger description for display
# ---------------------------------------------------------------------------
$TriggerDesc = "Mon-Fri at $TriggerTimeLocal local time"

# ---------------------------------------------------------------------------
# CHECK-ONLY mode: show what would be registered; no mutations
# ---------------------------------------------------------------------------
if ($CheckOnly) {
    Write-Sect "CHECK-ONLY: scheduler config preview (no changes)"
    Write-Step "Task name         : $TaskName"
    Write-Step "Prep script       : $PrepScript"
    Write-Step "Symbols           : $Symbols"
    Write-Step "Timeframe         : $Timeframe"
    Write-Step "MinCompletedBars  : $MinCompletedBars"
    Write-Step "Trigger           : $TriggerDesc"
    Write-Step "Log dir           : $ScheduledLogDir"
    Write-Step "Reg record        : $RegRecord"
    Write-Step "Task action exe   : powershell.exe"
    Write-Step "Task action arg   : $TaskArgument"

    # Check whether task already exists
    try {
        $existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if ($null -ne $existingTask) {
            Write-Warn "Task '$TaskName' already exists (state: $($existingTask.State)). Register would UPDATE it."
        } else {
            Write-Ok "Task '$TaskName' does not exist. Register would CREATE it."
        }
    } catch {
        Write-Warn "Could not query Task Scheduler: $_"
    }

    Write-Sect "CHECK-ONLY complete"
    Write-Ok "CheckOnly passed. No task was created, updated, or removed."
    exit 0
}

# ---------------------------------------------------------------------------
# UNREGISTER mode: remove the task
# ---------------------------------------------------------------------------
if ($Unregister) {
    Write-Sect "Unregister: remove scheduled task '$TaskName'"
    try {
        $existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        if ($null -eq $existingTask) {
            Write-Warn "Task '$TaskName' not found. Nothing to unregister."
            exit 0
        }
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false
        Write-Ok "Task '$TaskName' unregistered."
    } catch {
        Write-Fail "Failed to unregister task '$TaskName': $_"
        exit 1
    }
    exit 0
}

# ---------------------------------------------------------------------------
# REGISTER mode: create or update the scheduled task
# ---------------------------------------------------------------------------
Write-Sect "Register premarket data refresh task"
Write-Step "Task name         : $TaskName"
Write-Step "Prep script       : $PrepScript"
Write-Step "Symbols           : $Symbols"
Write-Step "Timeframe         : $Timeframe"
Write-Step "MinCompletedBars  : $MinCompletedBars"
Write-Step "Trigger           : $TriggerDesc"
Write-Step "Log dir           : $ScheduledLogDir"

# Verify ScheduledTasks module is available
if (-not (Get-Module -Name ScheduledTasks -ListAvailable)) {
    Write-Fail "ScheduledTasks module not available. Windows 8 / Server 2012+ required."
    exit 1
}

# Create log directory now so the evidence path is established before first run
try {
    New-Item -ItemType Directory -Force -Path $ScheduledLogDir | Out-Null
    Write-Ok "Log directory ready: $ScheduledLogDir"
} catch {
    Write-Warn "Could not create log directory: $_"
}

# Build task components
$action    = New-ScheduledTaskAction -Execute 'powershell.exe' -Argument $TaskArgument -WorkingDirectory $RepoRoot
$trigger   = New-ScheduledTaskTrigger -Weekly -DaysOfWeek Monday,Tuesday,Wednesday,Thursday,Friday -At $TriggerTimeLocal
$settings  = New-ScheduledTaskSettingsSet `
    -ExecutionTimeLimit (New-TimeSpan -Hours 1) `
    -StartWhenAvailable `
    -MultipleInstances IgnoreNew
$principal = New-ScheduledTaskPrincipal -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) `
    -LogonType Interactive -RunLevel Limited

# Register or update
try {
    $existingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($null -ne $existingTask) {
        Write-Step "Task exists; updating..."
        Set-ScheduledTask -TaskName $TaskName `
            -Action $action -Trigger $trigger -Settings $settings | Out-Null
        Write-Ok "Task '$TaskName' updated."
    } else {
        Write-Step "Creating new task..."
        Register-ScheduledTask -TaskName $TaskName `
            -Action $action -Trigger $trigger -Settings $settings `
            -Principal $principal -Force | Out-Null
        Write-Ok "Task '$TaskName' created."
    }
} catch {
    Write-Fail "Failed to register task: $_"
    exit 1
}

# Write registration record
try {
    $record = [pscustomobject]@{
        schema_version    = 'task-registration-v1'
        registered_at_utc = (Get-Date).ToUniversalTime().ToString('o')
        task_name         = $TaskName
        trigger           = $TriggerDesc
        symbols           = $Symbols
        timeframe         = $Timeframe
        min_completed_bars= $MinCompletedBars
        prep_script       = $PrepScript
        log_dir           = $ScheduledLogDir
        safety_note       = 'Task calls Prep-PremarketMarketData.ps1 only. Does not start daemon or trading.'
    }
    $record | ConvertTo-Json -Depth 4 | Set-Content -Path $RegRecord -Encoding ASCII
    Write-Ok "Registration record: $RegRecord"
} catch {
    Write-Warn "Could not write registration record: $_"
}

Write-Sect "Registration complete"
Write-Ok "Task '$TaskName' will run $TriggerDesc."
Write-Ok "Evidence will appear in: $ScheduledLogDir"
Write-Ok "To verify: Get-ScheduledTask -TaskName '$TaskName'"
Write-Ok "To unregister: run this script with -Unregister"
Write-Ok ""
Write-Step "SAFETY: this task runs market-data prep only. It does not start the daemon or trading."
exit 0
