# =============================================================================
# CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01
# Register-KrakenOhlcSyncTask.ps1
#
# Optional Windows Scheduled Task registration wrapper for
# Run-KrakenOhlcSync.ps1. Mirrors the safe, default-unregistered pattern of
# Register-LocalCryptoIngestTask.ps1 (CRYPTO-DATA-01E).
#
# SAFETY INVARIANT: this script's task action calls ONLY
# Run-KrakenOhlcSync.ps1 (in real, non-CheckOnly mode). It does NOT embed
# MQK_DATABASE_URL, ALPACA/TWELVEDATA credentials, or any secret in the task
# action. It does NOT read .env.local. It does NOT start the daemon,
# runtime, or trading. It does NOT run the task after registering it.
#
# Default mode is -CheckOnly: previews the exact task action, reports
# whether the task already exists, writes evidence, and performs NO
# mutation -- no task is created, updated, or removed.
#
# -Register explicitly creates/updates the scheduled task. -Unregister
# explicitly removes it (idempotent if absent). Neither is the default.
#
# IMPORTANT -- persistent environment variables required by the task at run
# time (NOT set by this script, NOT embedded in the task action):
#   MQK_DATABASE_URL              must be a persistent user/system env var
#                                  targeting the paper DB (port 5440 /
#                                  miniquantdesk_paper). Shell-session
#                                  variables are NOT inherited by scheduled
#                                  tasks.
#   MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1
#                                  must also be a persistent user/system env
#                                  var. Without it, Run-KrakenOhlcSync.ps1's
#                                  own gates (and kraken-ohlc-sync's own
#                                  CRYPTO-DATA-03A network gate) refuse the
#                                  real run.
# If either is absent when the task actually runs, Run-KrakenOhlcSync.ps1
# fails closed and writes evidence with all_passed=false -- no partial
# write occurs. This is correct fail-closed behavior, not a bug.
#
# Usage:
#   # Preview only (no mutation, no registration) -- this is also the
#   # default if no mode switch is passed:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-KrakenOhlcSyncTask.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-KrakenOhlcSyncTask.ps1 -CheckOnly
#
#   # Register the task (creates or updates):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-KrakenOhlcSyncTask.ps1 -Register
#
#   # Unregister (remove) the task:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-KrakenOhlcSyncTask.ps1 -Unregister
#
# Parameters:
#   -RepoRoot      Repo root. Default: auto-resolved two levels up from this script.
#   -TaskName      Windows Task Scheduler task name.
#                  Default: MiniQuantDesk-KrakenOhlcSync
#   -TaskPath      Windows Task Scheduler task folder path. Default: \MiniQuantDesk\
#   -At            Local time for the daily task trigger. Default: 07:35
#   -RunnerPath    Path to Run-KrakenOhlcSync.ps1.
#                  Default: <RepoRoot>\scripts\windows\Run-KrakenOhlcSync.ps1
#   -PolicyPath    Passed through to the runner. Default: runner's own default.
#   -RegistryPath  Passed through to the runner. Default: runner's own default.
#   -ProvidersPath Passed through to the runner. Default: runner's own default.
#   -Symbols       Passed through to the runner. Default: BTC/USD,ETH/USD
#   -Timeframe     Passed through to the runner. Default: 1D
#   -OutputDir     Evidence output directory for the runner.
#                  Default: <RepoRoot>\exports\market_data
#   -Register      Explicitly create or update the scheduled task.
#   -Unregister    Explicitly remove the scheduled task if it exists. Idempotent.
#   -CheckOnly     Preview mode (default when neither -Register nor -Unregister
#                  is passed). No task mutation.
#
# Evidence (always written):
#   exports\market_data\kraken_ohlc_task_registration.json
#   exports\market_data\kraken_ohlc_task_registration.txt
#
# Requirements:
#   - PowerShell 5.1+
#   - Windows 8 / Server 2012 or later (ScheduledTasks module)
#   - No elevated privileges required (task runs as current interactive user)
#
# Hard rules enforced by validate_crypto_data_03b_kraken_scheduler_task_scripts.ps1:
#   - Task action references ONLY Run-KrakenOhlcSync.ps1
#   - Defaults to -CheckOnly (no mode flag = preview only)
#   - -Register and -Unregister both exist as explicit, separate switches
#   - Task action never embeds MQK_DATABASE_URL or any credential value
#   - Script never reads .env.local
#   - Script never starts the task after registering it
#   - Evidence path is deterministic under exports\market_data\
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [string] $RepoRoot      = '',
    [string] $TaskName      = 'MiniQuantDesk-KrakenOhlcSync',
    [string] $TaskPath      = '\MiniQuantDesk\',
    [string] $At            = '07:35',
    [string] $RunnerPath    = '',
    [string] $PolicyPath    = '',
    [string] $RegistryPath  = '',
    [string] $ProvidersPath = '',
    [string] $Symbols       = 'BTC/USD,ETH/USD',
    [string] $Timeframe     = '1D',
    [string] $OutputDir     = '',
    [switch] $Register,
    [switch] $Unregister,
    [switch] $CheckOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$PATCH_ID       = 'CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01'
$SCHEMA_VERSION = 'kraken-ohlc-task-registration-v1'

function Write-Step { param([string]$M) Write-Host "[KRAKEN-SCHED] $M"       -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[KRAKEN-SCHED] OK: $M"   -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[KRAKEN-SCHED] WARN: $M" -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[KRAKEN-SCHED] FAIL: $M" -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

if ($Register -and $Unregister) {
    Write-Fail '-Register and -Unregister are mutually exclusive. Provide at most one.'
    exit 1
}

# Default to CheckOnly when neither -Register nor -Unregister is passed.
if (-not $Register -and -not $Unregister) {
    $CheckOnly = $true
}

# ---------------------------------------------------------------------------
# Resolve repo root
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

# ---------------------------------------------------------------------------
# Validate runner script exists
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($RunnerPath)) {
    $RunnerPath = Join-Path $RepoRoot 'scripts\windows\Run-KrakenOhlcSync.ps1'
}
if (-not (Test-Path -LiteralPath $RunnerPath)) {
    Write-Fail "Runner script not found: $RunnerPath"
    Write-Fail "Ensure Run-KrakenOhlcSync.ps1 is committed before registering this task."
    exit 1
}
Write-Ok "Runner script found: $RunnerPath"

# ---------------------------------------------------------------------------
# Resolve pass-through paths (empty means "let the runner use its own default")
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $RepoRoot 'exports\market_data'
}

# ---------------------------------------------------------------------------
# Build the task action argument string.
#
# The action runs powershell.exe and calls Run-KrakenOhlcSync.ps1 in real
# (non-CheckOnly) mode with -CheckOnly:$false. It passes through the
# operator-chosen (or default) policy/registry/providers/symbols/timeframe/
# output-dir values as plain arguments -- none of these are secrets.
#
# SAFETY: the action references ONLY Run-KrakenOhlcSync.ps1. It does not set
# MQK_DATABASE_URL or MQK_ALLOW_KRAKEN_SCHEDULED_SYNC -- those must already
# be persistent user/system environment variables for the real run to do
# anything beyond fail closed. It does not call any daemon, runtime, broker,
# provider, or order script directly.
# ---------------------------------------------------------------------------
$RunnerPathEsc   = $RunnerPath   -replace "'", "''"
$OutputDirEsc    = $OutputDir    -replace "'", "''"
$SymbolsEsc      = $Symbols      -replace "'", "''"
$TimeframeEsc    = $Timeframe    -replace "'", "''"

$optionalArgs = ''
if (-not [string]::IsNullOrWhiteSpace($PolicyPath))    { $optionalArgs += " -PolicyPath '$($PolicyPath -replace "'", "''")'" }
if (-not [string]::IsNullOrWhiteSpace($RegistryPath))  { $optionalArgs += " -RegistryPath '$($RegistryPath -replace "'", "''")'" }
if (-not [string]::IsNullOrWhiteSpace($ProvidersPath)) { $optionalArgs += " -ProvidersPath '$($ProvidersPath -replace "'", "''")'" }

$innerCommand = (
    "`$d='$OutputDirEsc';" +
    "[IO.Directory]::CreateDirectory(`$d)|Out-Null;" +
    "`$f=Join-Path `$d ('kraken_ohlc_sync_task_'+(Get-Date -f 'yyyyMMdd_HHmmss')+'.log');" +
    "Start-Transcript `$f -Append|Out-Null;" +
    "try{& '$RunnerPathEsc' -CheckOnly:`$false " +
        "-Symbols '$SymbolsEsc' " +
        "-Timeframe '$TimeframeEsc' " +
        "-OutputDir '$OutputDirEsc'" +
        "$optionalArgs}" +
    "finally{Stop-Transcript|Out-Null}"
)
$TaskArgument = "-ExecutionPolicy Bypass -NonInteractive -WindowStyle Hidden -Command `"$innerCommand`""
$TriggerDesc  = "Daily at $At local time"

# ---------------------------------------------------------------------------
# Evidence paths (deterministic; overwritten each run)
# ---------------------------------------------------------------------------
try { New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null } catch {}
$EvJsonFile = Join-Path $OutputDir 'kraken_ohlc_task_registration.json'
$EvTxtFile  = Join-Path $OutputDir 'kraken_ohlc_task_registration.txt'

# ---------------------------------------------------------------------------
# Helper: query task existence
# ---------------------------------------------------------------------------
function Get-TaskExists {
    try {
        $t = Get-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath -ErrorAction SilentlyContinue
        return ($null -ne $t)
    } catch {
        return $false
    }
}

$taskExistsBefore = Get-TaskExists
Write-Step "Task '$TaskName' exists before: $taskExistsBefore"

# ---------------------------------------------------------------------------
# Write evidence (always called before exit)
# ---------------------------------------------------------------------------
function Write-Evidence {
    param(
        [string]   $Mode,
        [bool]     $TaskExistsBefore,
        [bool]     $TaskExistsAfter,
        [bool]     $Registered,
        [bool]     $Unregistered,
        [bool]     $AllPassed,
        [string]   $ReasonCode,
        [string[]] $FailReasons
    )

    $ev = [pscustomobject]@{
        schema_version          = $SCHEMA_VERSION
        patch_id                = $PATCH_ID
        producer                = 'Register-KrakenOhlcSyncTask.ps1'
        produced_at_utc         = (Get-Date).ToUniversalTime().ToString('o')
        mode                    = $Mode
        task_name               = $TaskName
        task_path               = $TaskPath
        task_exists_before      = $TaskExistsBefore
        task_exists_after       = $TaskExistsAfter
        registered              = $Registered
        unregistered            = $Unregistered
        check_only              = ($Mode -eq 'check_only')
        task_action             = $TaskArgument
        runner_path             = $RunnerPath
        policy_path             = $PolicyPath
        registry_path           = $RegistryPath
        providers_path          = $ProvidersPath
        symbols                 = $Symbols
        timeframe               = $Timeframe
        at                      = $At
        scheduled_task_mutation = ($Mode -ne 'check_only') -and $AllPassed
        network_call_made       = $false
        db_write                = $false
        md_bars_write           = $false
        env_vars_embedded       = @()
        env_vars_required       = @('MQK_DATABASE_URL', 'MQK_ALLOW_KRAKEN_SCHEDULED_SYNC')
        all_passed               = $AllPassed
        reason_code              = $ReasonCode
        fail_reasons              = @($FailReasons)
        warnings                 = @()
        safety = [pscustomobject]@{
            calls_runner_script_only                            = $true
            no_daemon_runtime_broker_provider_order_references  = $true
            no_env_vars_embedded_in_task_action                 = $true
            no_env_local_read                                   = $true
            task_never_started_by_this_script                   = $true
        }
    }

    try {
        $ev | ConvertTo-Json -Depth 6 | Set-Content -Path $EvJsonFile -Encoding ASCII
        Write-Ok "Evidence JSON: $EvJsonFile"
    } catch {
        Write-Warn "Could not write evidence JSON: $_"
    }

    $actionPreview = if ($TaskArgument.Length -gt 400) {
        $TaskArgument.Substring(0, 400) + '...'
    } else {
        $TaskArgument
    }

    $txtLines = @(
        'Register-KrakenOhlcSyncTask Evidence'
        '====================================='
        "patch_id             : $PATCH_ID"
        "produced_at_utc      : $((Get-Date).ToUniversalTime().ToString('o'))"
        "mode                 : $Mode"
        "task_name            : $TaskName"
        "task_path            : $TaskPath"
        "task_exists_before   : $TaskExistsBefore"
        "task_exists_after    : $TaskExistsAfter"
        "registered           : $Registered"
        "unregistered         : $Unregistered"
        "check_only           : $($Mode -eq 'check_only')"
        "runner_path          : $RunnerPath"
        "symbols              : $Symbols"
        "timeframe            : $Timeframe"
        "at                   : $At"
        "all_passed           : $AllPassed"
        "reason_code          : $ReasonCode"
        "fail_reasons         : $($FailReasons -join '; ')"
        ''
        'env_vars_required (persistent user/system vars, NOT set by this script):'
        '  MQK_DATABASE_URL'
        '  MQK_ALLOW_KRAKEN_SCHEDULED_SYNC'
        ''
        'env_vars_embedded_in_task_action: (none)'
        ''
        'safety:'
        '  calls_runner_script_only                            : true'
        '  no_daemon_runtime_broker_provider_order_references  : true'
        '  no_env_vars_embedded_in_task_action                 : true'
        '  no_env_local_read                                   : true'
        '  task_never_started_by_this_script                   : true'
        ''
        'task_action (preview):'
        "  $actionPreview"
    )

    try {
        $txtLines | Set-Content -Path $EvTxtFile -Encoding UTF8
        Write-Ok "Evidence TXT: $EvTxtFile"
    } catch {
        Write-Warn "Could not write evidence TXT: $_"
    }
}

# ===========================================================================
# CHECK-ONLY mode: preview config and planned action; no mutations
# ===========================================================================
if ($CheckOnly) {
    Write-Sect 'CHECK-ONLY: task config preview (no mutations)'
    Write-Step "Task name         : $TaskName"
    Write-Step "Task path         : $TaskPath"
    Write-Step "Runner script     : $RunnerPath"
    Write-Step "Symbols           : $Symbols"
    Write-Step "Timeframe         : $Timeframe"
    Write-Step "Output dir        : $OutputDir"
    Write-Step "Trigger           : $TriggerDesc"
    Write-Step "Evidence JSON     : $EvJsonFile"
    Write-Step "Evidence TXT      : $EvTxtFile"
    Write-Step "Task action exe   : powershell.exe"
    Write-Step "Task action arg   : $TaskArgument"

    if ($taskExistsBefore) {
        Write-Warn "Task '$TaskName' already exists at '$TaskPath'. Register would UPDATE it."
    } else {
        Write-Ok "Task '$TaskName' does not exist at '$TaskPath'. Register would CREATE it."
    }

    $taskExistsAfter = Get-TaskExists

    Write-Evidence `
        -Mode 'check_only' `
        -TaskExistsBefore $taskExistsBefore `
        -TaskExistsAfter $taskExistsAfter `
        -Registered $false `
        -Unregistered $false `
        -AllPassed $true `
        -ReasonCode 'check_only_complete' `
        -FailReasons @()

    Write-Sect 'CHECK-ONLY complete'
    Write-Ok 'CheckOnly passed. No task was created, updated, or removed.'
    exit 0
}

# ===========================================================================
# UNREGISTER mode: remove the task if it exists; idempotent
# ===========================================================================
if ($Unregister) {
    Write-Sect "Unregister: remove task '$TaskName' at '$TaskPath'"
    $unregistered = $false
    $allPassed    = $true
    $reasonCode   = 'task_not_found_nothing_to_unregister'
    $failReasons  = @()

    if (-not $taskExistsBefore) {
        Write-Warn "Task '$TaskName' not found at '$TaskPath'. Nothing to unregister."
    } else {
        try {
            Unregister-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath -Confirm:$false
            $unregistered = $true
            $reasonCode   = 'unregistered'
            Write-Ok "Task '$TaskName' unregistered."
        } catch {
            $allPassed   = $false
            $reasonCode  = 'unregister_failed'
            $failReasons = @("unregister_exception: $_")
            Write-Fail "Failed to unregister task '$TaskName': $_"
        }
    }

    $taskExistsAfter = Get-TaskExists

    Write-Evidence `
        -Mode 'unregister' `
        -TaskExistsBefore $taskExistsBefore `
        -TaskExistsAfter $taskExistsAfter `
        -Registered $false `
        -Unregistered $unregistered `
        -AllPassed $allPassed `
        -ReasonCode $reasonCode `
        -FailReasons $failReasons

    if ($allPassed) {
        Write-Sect 'Unregister complete'
        exit 0
    } else {
        Write-Sect 'Unregister FAILED'
        exit 1
    }
}

# ===========================================================================
# REGISTER mode: create or update the scheduled task
# ===========================================================================
Write-Sect 'Register Kraken OHLC sync task'
Write-Step "Task name         : $TaskName"
Write-Step "Task path         : $TaskPath"
Write-Step "Runner script     : $RunnerPath"
Write-Step "Symbols           : $Symbols"
Write-Step "Timeframe         : $Timeframe"
Write-Step "Output dir        : $OutputDir"
Write-Step "Trigger           : $TriggerDesc"

if (-not (Get-Module -Name ScheduledTasks -ListAvailable)) {
    Write-Fail 'ScheduledTasks module not available. Windows 8 / Server 2012+ required.'
    exit 1
}

$action    = New-ScheduledTaskAction `
    -Execute 'powershell.exe' `
    -Argument $TaskArgument `
    -WorkingDirectory $RepoRoot
$trigger   = New-ScheduledTaskTrigger -Daily -At $At
$settings  = New-ScheduledTaskSettingsSet `
    -ExecutionTimeLimit (New-TimeSpan -Hours 1) `
    -StartWhenAvailable `
    -MultipleInstances IgnoreNew
$principal = New-ScheduledTaskPrincipal `
    -UserId ([System.Security.Principal.WindowsIdentity]::GetCurrent().Name) `
    -LogonType Interactive `
    -RunLevel Limited

$registered  = $false
$allPassed   = $false
$reasonCode  = 'registration_failed'
$failReasons = @()

try {
    if ($taskExistsBefore) {
        Write-Step "Task exists at '$TaskPath'; updating..."
        Set-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath `
            -Action $action -Trigger $trigger -Settings $settings | Out-Null
        $registered = $true
        $reasonCode = 'updated'
        Write-Ok "Task '$TaskName' updated."
    } else {
        Write-Step "Creating new task at '$TaskPath'..."
        Register-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath `
            -Action $action -Trigger $trigger -Settings $settings `
            -Principal $principal -Force | Out-Null
        $registered = $true
        $reasonCode = 'registered'
        Write-Ok "Task '$TaskName' created."
    }
    $allPassed = $true
} catch {
    $failReasons = @("registration_exception: $_")
    Write-Fail "Failed to register task: $_"
}

$taskExistsAfter = Get-TaskExists

Write-Evidence `
    -Mode 'register' `
    -TaskExistsBefore $taskExistsBefore `
    -TaskExistsAfter $taskExistsAfter `
    -Registered $registered `
    -Unregistered $false `
    -AllPassed $allPassed `
    -ReasonCode $reasonCode `
    -FailReasons $failReasons

if ($allPassed) {
    Write-Sect 'Registration complete'
    Write-Ok "Task '$TaskName' will run $TriggerDesc."
    Write-Ok "Evidence: $EvJsonFile"
    Write-Ok "To verify   : Get-ScheduledTask -TaskName '$TaskName' -TaskPath '$TaskPath'"
    Write-Ok "To unregister: run this script with -Unregister"
    Write-Warn ''
    Write-Warn 'IMPORTANT: MQK_DATABASE_URL and MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1 must both be'
    Write-Warn 'set as persistent user or system environment variables for the real run to do'
    Write-Warn 'anything beyond fail closed. Shell-session variables are NOT inherited by'
    Write-Warn 'scheduled tasks. Neither is embedded in the task action.'
    Write-Warn 'This task was NOT started. CRYPTO TRADING REMAINS DISABLED.'
    exit 0
} else {
    Write-Sect 'Registration FAILED'
    exit 1
}
