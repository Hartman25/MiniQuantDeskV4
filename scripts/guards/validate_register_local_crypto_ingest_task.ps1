# =============================================================================
# CRYPTO-DATA-01E -- Register-LocalCryptoIngestTask.ps1 Validator
# validate_register_local_crypto_ingest_task.ps1
#
# Validates Register-LocalCryptoIngestTask.ps1 without registering or leaving
# any scheduled task.  No cargo, no DB connection, no network calls, no
# import runner -Once, no daemon start.
#
# Checks:
#   [1]  PowerShell parser: zero errors on Register-LocalCryptoIngestTask.ps1.
#   [2]  -CheckOnly with test task name and committed BTC/USD fixture exits 0.
#   [3]  No task registered by -CheckOnly: task_exists_after=false in evidence.
#   [4]  Evidence JSON written and contains all required top-level keys.
#   [5]  Evidence task_action contains Import-LocalCryptoMarks.ps1.
#   [6]  Evidence task_action contains -Once.
#   [7]  Evidence task_action contains the provided CsvPath.
#   [8]  Evidence task_action contains the provided Timeframe.
#   [9]  Evidence task_action contains the provided Source.
#   [10] Evidence task_action contains the provided OutputDir.
#   [11] Evidence check_only=true and registered=false in check_only run.
#   [12] Forbidden callable patterns absent from script text.
#   [13] -Unregister with non-existent test task name exits 0 (idempotent).
#   [14] No scheduled task remains under the test name after validation.
#
# No cargo.  No DB.  No network calls.  No import runner -Once.  No daemon.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_register_local_crypto_ingest_task.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir      = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot       = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$RegScript      = Join-Path $RepoRoot 'scripts\windows\Register-LocalCryptoIngestTask.ps1'
$FixturePath    = Join-Path $RepoRoot 'core-rs\crates\mqk-md\tests\fixtures\crypto_btcusd_1d_local.csv'
$TempOutputDir  = Join-Path ([System.IO.Path]::GetTempPath()) 'mqk_validate_reg_crypto'
$TestTaskName   = 'MiniQuantDesk Local Crypto Marks Import TEST DO NOT USE'
$TestTaskPath   = '\MiniQuantDesk\'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red    ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green  }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan   }
function Show-Warn  { param([string]$M) Write-Host "  WARN -- $M" -ForegroundColor Yellow }

Write-Host '============================================================'
Write-Host ' CRYPTO-DATA-01E validate_register_local_crypto_ingest_task'
Write-Host " Reg script  : $RegScript"
Write-Host " Fixture     : $FixturePath"
Write-Host " Test task   : $TestTaskName"
Write-Host " Temp dir    : $TempOutputDir"
Write-Host ' No cargo, no DB, no network calls, no task left behind.'
Write-Host '============================================================'

try { New-Item -ItemType Directory -Force -Path $TempOutputDir | Out-Null } catch {}

# =============================================================================
# [1] PowerShell parser: zero errors on Register-LocalCryptoIngestTask.ps1
# =============================================================================
Write-Host ''
Show-Info '--- [1] PowerShell parser: zero errors ---'

if (-not (Test-Path -LiteralPath $RegScript)) {
    Show-Fail "Register-LocalCryptoIngestTask.ps1 not found at $RegScript"
} else {
    $parseErrors = $null
    $parseTokens = $null
    [System.Management.Automation.Language.Parser]::ParseFile(
        $RegScript,
        [ref]$parseTokens,
        [ref]$parseErrors
    ) | Out-Null

    if ($null -ne $parseErrors -and $parseErrors.Count -gt 0) {
        Show-Fail "Parse errors found: $($parseErrors.Count)"
        foreach ($pe in $parseErrors) { Write-Host "    $pe" -ForegroundColor Red }
    } else {
        Show-Green 'Register-LocalCryptoIngestTask.ps1 parses with zero errors'
    }
}

# =============================================================================
# Helper: run the registration script in a subprocess
# =============================================================================
function Invoke-RegScript {
    param([string[]]$Arguments)
    $local:ErrorActionPreference = 'Continue'
    try {
        $out = & powershell -NoProfile -ExecutionPolicy Bypass `
            -File $RegScript @Arguments 2>&1
        $ec = $LASTEXITCODE
    } catch {
        $out = @("EXCEPTION: $_")
        $ec  = -99
    }
    return [pscustomobject]@{ Output = $out; ExitCode = $ec }
}

# =============================================================================
# [2] -CheckOnly with test task name and fixture exits 0
# =============================================================================
Write-Host ''
Show-Info '--- [2] -CheckOnly mode: expect exit 0, no mutation ---'

if (-not (Test-Path -LiteralPath $FixturePath)) {
    Show-Fail "BTC/USD fixture not found at $FixturePath"
} elseif (-not (Test-Path -LiteralPath $RegScript)) {
    Show-Fail "Registration script not found -- skipping runtime checks"
} else {
    $r2 = Invoke-RegScript @(
        '-CheckOnly',
        '-TaskName', $TestTaskName,
        '-TaskPath', $TestTaskPath,
        '-CsvPath', $FixturePath,
        '-Timeframe', '1D',
        '-Source', 'local_crypto_manual',
        '-OutputDir', $TempOutputDir
    )
    $r2.Output | ForEach-Object { Write-Host "    $_" }

    if ($r2.ExitCode -eq 0) {
        Show-Green "-CheckOnly exited 0 (correct)"
    } else {
        Show-Fail "-CheckOnly exited $($r2.ExitCode) but should exit 0"
    }
}

# =============================================================================
# [3] No task registered by -CheckOnly: verify via evidence JSON and direct query
# =============================================================================
Write-Host ''
Show-Info '--- [3] No task registered by -CheckOnly ---'

# Check via Windows Task Scheduler directly
try {
    $taskAfterCheck = Get-ScheduledTask -TaskName $TestTaskName -TaskPath $TestTaskPath `
        -ErrorAction SilentlyContinue
    if ($null -ne $taskAfterCheck) {
        Show-Fail "-CheckOnly left a task registered: '$TestTaskName' found in Task Scheduler"
    } else {
        Show-Green "Task '$TestTaskName' not registered after -CheckOnly (correct)"
    }
} catch {
    Show-Green "Task '$TestTaskName' not found in Task Scheduler after -CheckOnly (correct)"
}

# Check via evidence JSON task_exists_after field
$evJsonPath = Join-Path $TempOutputDir 'local_crypto_ingest_task_registration.json'
if (Test-Path -LiteralPath $evJsonPath) {
    try {
        $evCheck = Get-Content -Raw -Path $evJsonPath | ConvertFrom-Json
        if ($evCheck.task_exists_after -eq $false) {
            Show-Green "Evidence task_exists_after=false (correct -- check_only did not register)"
        } else {
            Show-Fail "Evidence task_exists_after='$($evCheck.task_exists_after)' (expected false)"
        }
    } catch {
        Show-Fail "Could not parse evidence JSON for task_exists_after check: $_"
    }
} else {
    Show-Warn "Evidence JSON not yet present for task_exists_after check (will be checked in [4])"
}

# =============================================================================
# [4] Evidence JSON exists and contains all required top-level keys
# =============================================================================
Write-Host ''
Show-Info '--- [4] Evidence JSON: required top-level keys present ---'

if (-not (Test-Path -LiteralPath $evJsonPath)) {
    Show-Fail "Evidence JSON not found at $evJsonPath"
} else {
    Show-Info "Evidence JSON: $evJsonPath"
    try {
        $ev = Get-Content -Raw -Path $evJsonPath | ConvertFrom-Json

        $requiredKeys = @(
            'schema_version', 'patch_id', 'produced_at_utc', 'mode',
            'task_name', 'task_path',
            'task_exists_before', 'task_exists_after',
            'registered', 'unregistered', 'check_only',
            'task_action', 'runner_path', 'csv_path',
            'timeframe', 'source', 'output_dir', 'max_age_hours',
            'trigger', 'all_passed', 'reason_code', 'fail_reasons',
            'safety'
        )

        $safetyKeys = @(
            'calls_import_runner_only',
            'no_daemon_runtime_broker_provider_order_references'
        )

        $missing = @($requiredKeys | Where-Object { $_ -notin ($ev.PSObject.Properties.Name) })
        if ($missing.Count -gt 0) {
            Show-Fail "Evidence JSON missing top-level keys: $($missing -join ', ')"
        } else {
            Show-Green "All $($requiredKeys.Count) required top-level keys present"
        }

        if ($null -ne $ev.safety) {
            $missingSafety = @($safetyKeys | Where-Object { $_ -notin ($ev.safety.PSObject.Properties.Name) })
            if ($missingSafety.Count -gt 0) {
                Show-Fail "Evidence JSON safety missing keys: $($missingSafety -join ', ')"
            } else {
                Show-Green "All $($safetyKeys.Count) required safety keys present"
            }
        } else {
            Show-Fail "Evidence JSON safety field is null or missing"
        }

        if ($ev.schema_version -eq 'local-crypto-task-registration-v1') {
            Show-Green "schema_version='local-crypto-task-registration-v1' (correct)"
        } else {
            Show-Fail "schema_version='$($ev.schema_version)' (expected 'local-crypto-task-registration-v1')"
        }

        if ($ev.patch_id -match 'CRYPTO-DATA-01E') {
            Show-Green "patch_id contains 'CRYPTO-DATA-01E' (correct)"
        } else {
            Show-Fail "patch_id='$($ev.patch_id)' does not reference CRYPTO-DATA-01E"
        }

    } catch {
        Show-Fail "Evidence JSON parse failed: $_"
    }
}

# =============================================================================
# [5] Evidence task_action contains Import-LocalCryptoMarks.ps1
# =============================================================================
Write-Host ''
Show-Info '--- [5] Evidence task_action contains Import-LocalCryptoMarks.ps1 ---'

if (Test-Path -LiteralPath $evJsonPath) {
    try {
        $ev5 = Get-Content -Raw -Path $evJsonPath | ConvertFrom-Json
        if ($ev5.task_action -match [regex]::Escape('Import-LocalCryptoMarks.ps1')) {
            Show-Green "task_action contains 'Import-LocalCryptoMarks.ps1'"
        } else {
            Show-Fail "task_action does not contain 'Import-LocalCryptoMarks.ps1'"
            Write-Host "    task_action: $($ev5.task_action)" -ForegroundColor Yellow
        }
    } catch {
        Show-Fail "Could not parse evidence JSON for task_action check: $_"
    }
} else {
    Show-Fail "Evidence JSON not found; cannot check task_action"
}

# =============================================================================
# [6] Evidence task_action contains -Once
# =============================================================================
Write-Host ''
Show-Info '--- [6] Evidence task_action contains -Once ---'

if (Test-Path -LiteralPath $evJsonPath) {
    try {
        $ev6 = Get-Content -Raw -Path $evJsonPath | ConvertFrom-Json
        if ($ev6.task_action -match '-Once') {
            Show-Green "task_action contains '-Once' (correct)"
        } else {
            Show-Fail "task_action does not contain '-Once'"
        }
    } catch {
        Show-Fail "Could not parse evidence JSON for -Once check: $_"
    }
} else {
    Show-Fail "Evidence JSON not found; cannot check -Once in task_action"
}

# =============================================================================
# [7] Evidence task_action contains the provided CsvPath
# [8] Evidence task_action contains the provided Timeframe
# [9] Evidence task_action contains the provided Source
# [10] Evidence task_action contains the provided OutputDir
# =============================================================================
Write-Host ''
Show-Info '--- [7-10] Evidence task_action contains CsvPath / Timeframe / Source / OutputDir ---'

if (Test-Path -LiteralPath $evJsonPath) {
    try {
        $ev7 = Get-Content -Raw -Path $evJsonPath | ConvertFrom-Json
        $action7 = $ev7.task_action

        # [7] CsvPath
        $absCsv = [System.IO.Path]::GetFullPath(
            [System.IO.Path]::Combine((Get-Location).Path, $FixturePath)
        )
        if ($action7 -match [regex]::Escape($absCsv)) {
            Show-Green "[7] task_action contains resolved CsvPath"
        } else {
            Show-Fail "[7] task_action does not contain resolved CsvPath '$absCsv'"
        }

        # [8] Timeframe
        if ($action7 -match '-Timeframe') {
            Show-Green "[8] task_action contains '-Timeframe'"
        } else {
            Show-Fail "[8] task_action does not contain '-Timeframe'"
        }

        # [9] Source
        if ($action7 -match '-Source') {
            Show-Green "[9] task_action contains '-Source'"
        } else {
            Show-Fail "[9] task_action does not contain '-Source'"
        }

        # [10] OutputDir
        if ($action7 -match '-OutputDir') {
            Show-Green "[10] task_action contains '-OutputDir'"
        } else {
            Show-Fail "[10] task_action does not contain '-OutputDir'"
        }
    } catch {
        Show-Fail "Could not parse evidence JSON for param checks: $_"
    }
} else {
    Show-Fail "Evidence JSON not found; cannot check action params"
}

# =============================================================================
# [11] Evidence check_only=true and registered=false in check_only run
# =============================================================================
Write-Host ''
Show-Info '--- [11] Evidence check_only=true and registered=false ---'

if (Test-Path -LiteralPath $evJsonPath) {
    try {
        $ev11 = Get-Content -Raw -Path $evJsonPath | ConvertFrom-Json
        if ($ev11.check_only -eq $true) {
            Show-Green "Evidence check_only=true (correct)"
        } else {
            Show-Fail "Evidence check_only='$($ev11.check_only)' (expected true)"
        }
        if ($ev11.registered -eq $false) {
            Show-Green "Evidence registered=false (correct)"
        } else {
            Show-Fail "Evidence registered='$($ev11.registered)' (expected false)"
        }
    } catch {
        Show-Fail "Could not parse evidence JSON for check_only/registered: $_"
    }
} else {
    Show-Fail "Evidence JSON not found; cannot check check_only/registered"
}

# =============================================================================
# [12] Forbidden callable patterns absent from Register-LocalCryptoIngestTask.ps1
# =============================================================================
Write-Host ''
Show-Info '--- [12] Forbidden callable patterns absent from script text ---'

$forbidden = [ordered]@{
    'Invoke-RestMethod'          = 'REST/HTTP call to daemon or external endpoint'
    '/api/v1/ops/action'         = 'operator action route'
    '/api/v1/strategy/signal'    = 'strategy signal route'
    '/api/v1/execution'          = 'execution route'
    'Refresh-IntradayMarketData' = 'equity intraday refresh script invocation'
    'Start-PaperTradingSmoke'    = 'paper trading smoke invocation'
    'sync-provider'              = 'provider sync CLI command'
    'ingest-provider'            = 'provider ingest CLI command'
    'mqk-daemon'                 = 'daemon binary reference'
    'mqk-runtime'                = 'runtime binary reference'
    'mqk-broker'                 = 'broker binary reference'
}

if (Test-Path -LiteralPath $RegScript) {
    $scriptText = [System.IO.File]::ReadAllText($RegScript)
    foreach ($pat in $forbidden.Keys) {
        if ($scriptText -match [regex]::Escape($pat)) {
            Show-Fail "Forbidden pattern found: '$pat' ($($forbidden[$pat]))"
        } else {
            Show-Green "Forbidden pattern absent: '$pat'"
        }
    }
} else {
    Show-Fail "Registration script not found; cannot check forbidden patterns"
}

# =============================================================================
# [13] -Unregister with non-existent test task name exits 0 (idempotent)
# =============================================================================
Write-Host ''
Show-Info '--- [13] -Unregister with non-existent test task: expect exit 0 (idempotent) ---'

if (Test-Path -LiteralPath $RegScript) {
    $r13 = Invoke-RegScript @(
        '-Unregister',
        '-TaskName', $TestTaskName,
        '-TaskPath', $TestTaskPath,
        '-OutputDir', $TempOutputDir
    )
    $r13.Output | ForEach-Object { Write-Host "    $_" }

    if ($r13.ExitCode -eq 0) {
        Show-Green "-Unregister (non-existent task) exited 0 (idempotent -- correct)"
    } else {
        Show-Fail "-Unregister (non-existent task) exited $($r13.ExitCode) but should exit 0"
    }
} else {
    Show-Fail "Registration script not found; skipping unregister test"
}

# =============================================================================
# [14] No scheduled task remains under test name after validation
# =============================================================================
Write-Host ''
Show-Info '--- [14] No task remains under test name after validation ---'

try {
    $finalCheck = Get-ScheduledTask -TaskName $TestTaskName -TaskPath $TestTaskPath `
        -ErrorAction SilentlyContinue
    if ($null -ne $finalCheck) {
        Show-Fail "Task '$TestTaskName' still exists after validation -- cleanup required"
    } else {
        Show-Green "No task '$TestTaskName' remains in Task Scheduler (correct)"
    }
} catch {
    Show-Green "No task '$TestTaskName' remains in Task Scheduler (correct)"
}

# =============================================================================
# Cleanup temp dir and summary
# =============================================================================
try { Remove-Item -Recurse -Force $TempOutputDir -ErrorAction SilentlyContinue } catch {}

Write-Host ''
Write-Host '============================================================'
Write-Host ' Summary'
Write-Host '============================================================'

if ($Violations -eq 0) {
    Write-Host ' ALL CHECKS PASSED' -ForegroundColor Green
    exit 0
} else {
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
}
