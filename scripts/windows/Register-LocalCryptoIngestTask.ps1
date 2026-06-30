# =============================================================================
# CRYPTO-DATA-01E-LOCAL-CRYPTO-INGEST-TASK-REGISTRATION-01-COMBINED
# Register-LocalCryptoIngestTask.ps1
#
# Creates, updates, checks, or removes a Windows Scheduled Task that runs the
# local crypto marks import runner (Import-LocalCryptoMarks.ps1) daily at a
# configurable local time.
#
# SAFETY INVARIANT: this script calls ONLY Import-LocalCryptoMarks.ps1.
# It does NOT start the daemon, runtime, or trading.  It does NOT call any
# provider, broker, or order script.  It does NOT set MQK_DATABASE_URL.
# It does NOT read .env.local.  It does NOT embed any secret or credential.
#
# IMPORTANT -- MQK_DATABASE_URL and -Once mode:
#   The scheduled task action always calls Import-LocalCryptoMarks.ps1 -Once.
#   -Once requires MQK_DATABASE_URL to be set to the paper DB URL in the
#   task's environment.  Shell-session variables ($env:VAR = ...) are NOT
#   inherited by scheduled tasks; the variable must be a persistent user or
#   system environment variable.  If MQK_DATABASE_URL is absent or does not
#   target port 5440 / miniquantdesk_paper, the import runner fails closed
#   and writes evidence with all_passed=false / reason_code=database_url_missing.
#   No partial write occurs.  This is correct fail-closed behavior.
#
# IMPORTANT -- Default mode:
#   If neither -CheckOnly nor -Unregister is passed, the script registers
#   (creates or updates) the scheduled task.  -CsvPath is required for
#   registration.  This matches the Register-PremarketDataRefreshTask.ps1
#   pattern in this repo.
#
# Usage:
#   # Preview (no mutation, no registration):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-LocalCryptoIngestTask.ps1 `
#       -CheckOnly -CsvPath .\path\to\btcusd_1d_local.csv
#
#   # Register the task:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-LocalCryptoIngestTask.ps1 `
#       -CsvPath .\path\to\btcusd_1d_local.csv
#
#   # Unregister:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Register-LocalCryptoIngestTask.ps1 `
#       -Unregister
#
# Parameters:
#   -TaskName      Windows Task Scheduler task name.
#                  Default: MiniQuantDesk Local Crypto Marks Import
#   -TaskPath      Windows Task Scheduler task folder path.
#                  Default: \MiniQuantDesk\
#   -CsvPath       Path to the local crypto CSV file passed to the runner.
#                  Required for registration.  Optional for -CheckOnly / -Unregister.
#   -Timeframe     Bar timeframe passed to runner. Default: 1D
#   -Source        Source label passed to runner. Default: local_crypto_manual
#   -OutputDir     Evidence output directory passed to runner.
#                  Default: <RepoRoot>\exports\market_data
#   -MaxAgeHours   Staleness gate hours passed to runner. Default: 36
#   -At            Local time for the daily task trigger. Default: 07:30
#   -CheckOnly     Preview mode: show config and planned action. No mutation.
#   -Unregister    Remove the scheduled task if it exists. Idempotent.
#
# Evidence (always written):
#   exports\market_data\local_crypto_ingest_task_registration.json
#   exports\market_data\local_crypto_ingest_task_registration.txt
#
# Requirements:
#   - PowerShell 5.1+
#   - Windows 8 / Server 2012 or later (ScheduledTasks module)
#   - No elevated privileges required (task runs as current interactive user)
#
# Hard rules enforced by validate_register_local_crypto_ingest_task.ps1:
#   - Action references ONLY Import-LocalCryptoMarks.ps1
#   - Action always includes -Once
#   - No daemon, runtime, broker, provider, or order script in action
#   - -CheckOnly exits without any task mutations
#   - -Unregister is safe/idempotent when no task exists
#   - Evidence path is deterministic under exports\market_data\
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [string] $TaskName    = 'MiniQuantDesk Local Crypto Marks Import',
    [string] $TaskPath    = '\MiniQuantDesk\',
    [string] $CsvPath     = '',
    [string] $Timeframe   = '1D',
    [string] $Source      = 'local_crypto_manual',
    [string] $OutputDir   = '',
    [int]    $MaxAgeHours = 36,
    [string] $At          = '07:30',
    [switch] $CheckOnly,
    [switch] $Unregister
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$PATCH_ID       = 'CRYPTO-DATA-01E-LOCAL-CRYPTO-INGEST-TASK-REGISTRATION-01-COMBINED'
$SCHEMA_VERSION = 'local-crypto-task-registration-v1'

function Write-Step { param([string]$M) Write-Host "[SCHED-CRYPTO] $M"         -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[SCHED-CRYPTO] OK: $M"     -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[SCHED-CRYPTO] WARN: $M"   -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[SCHED-CRYPTO] FAIL: $M"   -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Resolve repo root (two levels up from this script)
# ---------------------------------------------------------------------------
$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

# ---------------------------------------------------------------------------
# Validate import runner exists
# ---------------------------------------------------------------------------
$ImportScript = Join-Path $RepoRoot 'scripts\windows\Import-LocalCryptoMarks.ps1'
if (-not (Test-Path $ImportScript)) {
    Write-Fail "Import runner not found: $ImportScript"
    Write-Fail "Ensure Import-LocalCryptoMarks.ps1 is committed before registering this task."
    exit 1
}
Write-Ok "Import runner found: $ImportScript"

# ---------------------------------------------------------------------------
# Resolve OutputDir
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $RepoRoot 'exports\market_data'
}
$ResolvedOutputDir = $OutputDir

# ---------------------------------------------------------------------------
# Resolve CsvPath to absolute if provided
# ---------------------------------------------------------------------------
$ResolvedCsvPath = ''
if (-not [string]::IsNullOrWhiteSpace($CsvPath)) {
    $ResolvedCsvPath = [System.IO.Path]::GetFullPath(
        [System.IO.Path]::Combine((Get-Location).Path, $CsvPath)
    )
}

# ---------------------------------------------------------------------------
# Build the task action argument string.
#
# The action runs powershell.exe and:
#   1. Creates the output directory if absent.
#   2. Starts a transcript for this run.
#   3. Calls Import-LocalCryptoMarks.ps1 -Once with configured parameters.
#   4. Stops the transcript.
#
# SAFETY: the action references ONLY Import-LocalCryptoMarks.ps1.
# It does not call any daemon, runtime, broker, provider, or order script.
# MQK_DATABASE_URL is NOT set here; the import runner resolves it at runtime
# from the task's environment (must be a persistent user/system env var).
# ---------------------------------------------------------------------------
$ImportScriptEsc   = $ImportScript       -replace "'", "''"
$ResolvedOutDirEsc = $ResolvedOutputDir  -replace "'", "''"
$ResolvedCsvEsc    = $ResolvedCsvPath    -replace "'", "''"
$TimeframeEsc      = $Timeframe          -replace "'", "''"
$SourceEsc         = $Source             -replace "'", "''"

$innerCommand = (
    "`$d='$ResolvedOutDirEsc';" +
    "[IO.Directory]::CreateDirectory(`$d)|Out-Null;" +
    "`$f=Join-Path `$d ('local_crypto_import_task_'+(Get-Date -f 'yyyyMMdd_HHmmss')+'.log');" +
    "Start-Transcript `$f -Append|Out-Null;" +
    "try{& '$ImportScriptEsc' -Once " +
        "-CsvPath '$ResolvedCsvEsc' " +
        "-Timeframe '$TimeframeEsc' " +
        "-Source '$SourceEsc' " +
        "-OutputDir '$ResolvedOutDirEsc' " +
        "-MaxAgeHours $MaxAgeHours}" +
    "finally{Stop-Transcript|Out-Null}"
)
$TaskArgument = "-ExecutionPolicy Bypass -NonInteractive -WindowStyle Hidden -Command `"$innerCommand`""
$TriggerDesc  = "Daily at $At local time"

# ---------------------------------------------------------------------------
# Evidence paths (deterministic; overwritten each run)
# ---------------------------------------------------------------------------
try { New-Item -ItemType Directory -Force -Path $ResolvedOutputDir | Out-Null } catch {}
$EvJsonFile = Join-Path $ResolvedOutputDir 'local_crypto_ingest_task_registration.json'
$EvTxtFile  = Join-Path $ResolvedOutputDir 'local_crypto_ingest_task_registration.txt'

# ---------------------------------------------------------------------------
# Helper: query task existence at the configured name + path
# ---------------------------------------------------------------------------
function Get-TaskExists {
    try {
        $t = Get-ScheduledTask -TaskName $TaskName -TaskPath $TaskPath -ErrorAction SilentlyContinue
        return ($null -ne $t)
    } catch {
        return $false
    }
}

# ---------------------------------------------------------------------------
# Query task existence before any mutation
# ---------------------------------------------------------------------------
$taskExistsBefore = Get-TaskExists
Write-Step "Task '$TaskName' exists before: $taskExistsBefore"

# ---------------------------------------------------------------------------
# Write evidence (always called before exit)
# ---------------------------------------------------------------------------
function Write-Evidence {
    param(
        [string]   $Mode,
        [bool]     $TaskExistsBefore,
        [object]   $TaskExistsAfter,
        [bool]     $Registered,
        [bool]     $Unregistered,
        [bool]     $AllPassed,
        [string]   $ReasonCode,
        [string[]] $FailReasons
    )

    $ev = [pscustomobject]@{
        schema_version             = $SCHEMA_VERSION
        patch_id                   = $PATCH_ID
        produced_at_utc            = (Get-Date).ToUniversalTime().ToString('o')
        mode                       = $Mode
        task_name                  = $TaskName
        task_path                  = $TaskPath
        task_exists_before         = $TaskExistsBefore
        task_exists_after          = $TaskExistsAfter
        registered                 = $Registered
        unregistered               = $Unregistered
        check_only                 = ($Mode -eq 'check_only')
        task_action                = $TaskArgument
        runner_path                = $ImportScript
        csv_path                   = $ResolvedCsvPath
        timeframe                  = $Timeframe
        source                     = $Source
        output_dir                 = $ResolvedOutputDir
        max_age_hours              = $MaxAgeHours
        trigger                    = $TriggerDesc
        all_passed                 = $AllPassed
        reason_code                = $ReasonCode
        fail_reasons               = @($FailReasons)
        safety                     = [pscustomobject]@{
            calls_import_runner_only                           = $true
            no_daemon_runtime_broker_provider_order_references = $true
        }
    }

    try {
        $ev | ConvertTo-Json -Depth 6 | Set-Content -Path $EvJsonFile -Encoding ASCII
        Write-Ok "Evidence JSON: $EvJsonFile"
    } catch {
        Write-Warn "Could not write evidence JSON: $_"
    }

    $actionPreview = if ($TaskArgument.Length -gt 300) {
        $TaskArgument.Substring(0, 300) + '...'
    } else {
        $TaskArgument
    }

    $txtLines = @(
        'Register-LocalCryptoIngestTask Evidence'
        '======================================='
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
        "runner_path          : $ImportScript"
        "csv_path             : $ResolvedCsvPath"
        "timeframe            : $Timeframe"
        "source               : $Source"
        "output_dir           : $ResolvedOutputDir"
        "max_age_hours        : $MaxAgeHours"
        "trigger              : $TriggerDesc"
        "all_passed           : $AllPassed"
        "reason_code          : $ReasonCode"
        "fail_reasons         : $($FailReasons -join '; ')"
        ''
        'safety:'
        '  calls_import_runner_only                           : true'
        '  no_daemon_runtime_broker_provider_order_references : true'
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
    Write-Step "Import runner     : $ImportScript"
    Write-Step "CSV path          : $(if ($ResolvedCsvPath) { $ResolvedCsvPath } else { '(not provided)' })"
    Write-Step "Timeframe         : $Timeframe"
    Write-Step "Source            : $Source"
    Write-Step "Output dir        : $ResolvedOutputDir"
    Write-Step "Max age hours     : $MaxAgeHours"
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

    # Re-query to prove no mutation occurred
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
    Write-Ok 'CheckOnly passed.  No task was created, updated, or removed.'
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
        Write-Warn "Task '$TaskName' not found at '$TaskPath'.  Nothing to unregister."
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
Write-Sect 'Register local crypto ingest task'

# -CsvPath is required for registration (the action needs a stable source file)
if ([string]::IsNullOrWhiteSpace($CsvPath)) {
    Write-Fail '-CsvPath is required for registration.  The scheduled action needs a stable source file.'
    Write-Fail 'Run with -CheckOnly to preview the config without a CsvPath.'
    exit 1
}

if (-not (Test-Path -LiteralPath $ResolvedCsvPath)) {
    Write-Warn "CsvPath '$ResolvedCsvPath' does not currently exist."
    Write-Warn "The task will be registered but will fail closed on first run until the file is present."
}

Write-Step "Task name         : $TaskName"
Write-Step "Task path         : $TaskPath"
Write-Step "Import runner     : $ImportScript"
Write-Step "CSV path          : $ResolvedCsvPath"
Write-Step "Timeframe         : $Timeframe"
Write-Step "Source            : $Source"
Write-Step "Output dir        : $ResolvedOutputDir"
Write-Step "Max age hours     : $MaxAgeHours"
Write-Step "Trigger           : $TriggerDesc"

# Verify ScheduledTasks module is available
if (-not (Get-Module -Name ScheduledTasks -ListAvailable)) {
    Write-Fail "ScheduledTasks module not available.  Windows 8 / Server 2012+ required."
    exit 1
}

# Build task components
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
    Write-Ok "To verify : Get-ScheduledTask -TaskName '$TaskName'"
    Write-Ok "To unregister: run this script with -Unregister"
    Write-Warn ''
    Write-Warn 'IMPORTANT: MQK_DATABASE_URL must be set as a persistent user or system'
    Write-Warn 'environment variable for the import to succeed.  Shell-session variables'
    Write-Warn 'are NOT inherited by scheduled tasks.  If absent, the runner fails closed.'
    Write-Warn 'CRYPTO TRADING REMAINS DISABLED.  This task imports local data only.'
    exit 0
} else {
    Write-Sect 'Registration FAILED'
    exit 1
}
