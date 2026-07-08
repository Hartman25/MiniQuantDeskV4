# =============================================================================
# CRYPTO-DATA-03B -- Kraken Scheduler Task Scripts Validator
# validate_crypto_data_03b_kraken_scheduler_task_scripts.ps1
#
# Validates Run-KrakenOhlcSync.ps1 and Register-KrakenOhlcSyncTask.ps1
# without registering a Windows Scheduled Task, without a live Kraken
# network call, and without mutating any database.
#
# Checks:
#   [1]  PowerShell parser: zero errors on both scripts.
#   [2]  Both scripts exist and mention the expected task name.
#   [3]  Registration script defaults to check-only (no mode flag = preview).
#   [4]  Registration script exposes explicit -Register and -Unregister switches.
#   [5]  Registration script text never embeds MQK_DATABASE_URL as a value
#        (only as an informational env_vars_required reference).
#   [6]  Registration script text never reads .env.local.
#   [7]  Registration script text never starts the task it registers
#        (no Start-ScheduledTask reference).
#   [8]  Runner script requires MQK_DATABASE_URL before any real action.
#   [9]  Runner script requires MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1 for a real
#        (non-CheckOnly) run.
#   [10] Runner script calls kraken-scheduler-readiness.
#   [11] Runner script calls kraken-ohlc-sync only inside the real-run path
#        (not inside the CheckOnly branch).
#   [12] Runner script enforces sequential per-symbol calls with a sleep
#        between them (Start-Sleep present, no parallel/job dispatch).
#   [13] Neither script references forbidden daemon/broker/order/risk/
#        runtime patterns.
#   [14] Runner -CheckOnly (paper DB URL supplied, no real credentials)
#        exits 0 and evidence shows network_call_made=false / db_write=false
#        / md_bars_write=false / scheduled_task_mutation=false.
#   [15] Registration -CheckOnly with a throwaway test task name exits 0 and
#        leaves no scheduled task registered.
#   [16] Registration -Unregister with a non-existent test task name exits 0
#        (idempotent).
#   [17] docs/specs/crypto_data_03b_kraken_scheduler_task_scripts.md exists
#        and states no task/trading is enabled by default.
#
# No live Kraken network call. No DB mutation. No Windows Scheduled Task is
# left registered by this validator.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_crypto_data_03b_kraken_scheduler_task_scripts.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir     = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot      = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$RunnerScript  = Join-Path $RepoRoot 'scripts\windows\Run-KrakenOhlcSync.ps1'
$RegScript     = Join-Path $RepoRoot 'scripts\windows\Register-KrakenOhlcSyncTask.ps1'
$SpecDoc       = Join-Path $RepoRoot 'docs\specs\crypto_data_03b_kraken_scheduler_task_scripts.md'
$TempOutputDir = Join-Path ([System.IO.Path]::GetTempPath()) 'mqk_validate_kraken_sched_03b'
$TestTaskName  = 'MiniQuantDesk-KrakenOhlcSync-TEST-DO-NOT-USE'
$TestTaskPath  = '\MiniQuantDesk\'
$ExpectedTaskName = 'MiniQuantDesk-KrakenOhlcSync'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red    ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green  }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan   }
function Show-Warn  { param([string]$M) Write-Host "  WARN -- $M" -ForegroundColor Yellow }

Write-Host '============================================================'
Write-Host ' CRYPTO-DATA-03B validate_crypto_data_03b_kraken_scheduler_task_scripts'
Write-Host " Runner script : $RunnerScript"
Write-Host " Reg script    : $RegScript"
Write-Host " Spec doc      : $SpecDoc"
Write-Host ' No live Kraken network call. No DB mutation. No task left behind.'
Write-Host '============================================================'

try { New-Item -ItemType Directory -Force -Path $TempOutputDir | Out-Null } catch {}

# =============================================================================
# [1] PowerShell parser: zero errors on both scripts
# =============================================================================
Write-Host ''
Show-Info '--- [1] PowerShell parser: zero errors on both scripts ---'

foreach ($s in @($RunnerScript, $RegScript)) {
    if (-not (Test-Path -LiteralPath $s)) {
        Show-Fail "Script not found: $s"
        continue
    }
    $parseErrors = $null
    $parseTokens = $null
    [System.Management.Automation.Language.Parser]::ParseFile($s, [ref]$parseTokens, [ref]$parseErrors) | Out-Null
    if ($null -ne $parseErrors -and $parseErrors.Count -gt 0) {
        Show-Fail "Parse errors in $($s): $($parseErrors.Count)"
        foreach ($pe in $parseErrors) { Write-Host "    $pe" -ForegroundColor Red }
    } else {
        Show-Green "$(Split-Path -Leaf $s) parses with zero errors"
    }
}

# =============================================================================
# [2] Both scripts exist and mention the expected task name
# =============================================================================
Write-Host ''
Show-Info '--- [2] Both scripts exist and mention the expected task name ---'

$runnerText = ''
$regText    = ''
if (Test-Path -LiteralPath $RunnerScript) {
    $runnerText = [System.IO.File]::ReadAllText($RunnerScript)
    Show-Green "Run-KrakenOhlcSync.ps1 exists"
} else {
    Show-Fail "Run-KrakenOhlcSync.ps1 not found"
}
if (Test-Path -LiteralPath $RegScript) {
    $regText = [System.IO.File]::ReadAllText($RegScript)
    Show-Green "Register-KrakenOhlcSyncTask.ps1 exists"
    if ($regText -match [regex]::Escape($ExpectedTaskName)) {
        Show-Green "Registration script references task name '$ExpectedTaskName'"
    } else {
        Show-Fail "Registration script does not reference expected task name '$ExpectedTaskName'"
    }
} else {
    Show-Fail "Register-KrakenOhlcSyncTask.ps1 not found"
}

# =============================================================================
# [3] Registration script defaults to check-only
# =============================================================================
Write-Host ''
Show-Info '--- [3] Registration script defaults to check-only ---'

if ($regText -match '(?s)if\s*\(\s*-not\s*\$Register\s*-and\s*-not\s*\$Unregister\s*\)\s*\{\s*\$CheckOnly\s*=\s*\$true') {
    Show-Green "Registration script defaults to -CheckOnly when neither -Register nor -Unregister is passed"
} else {
    Show-Fail "Registration script does not clearly default to check-only when no mode flag is passed"
}

# =============================================================================
# [4] Registration script exposes -Register and -Unregister
# =============================================================================
Write-Host ''
Show-Info '--- [4] Registration script exposes -Register and -Unregister ---'

if ($regText -match '\[switch\]\s*\$Register\b') {
    Show-Green "-Register switch present"
} else {
    Show-Fail "-Register switch not found"
}
if ($regText -match '\[switch\]\s*\$Unregister\b') {
    Show-Green "-Unregister switch present"
} else {
    Show-Fail "-Unregister switch not found"
}

# =============================================================================
# [5] Registration script never embeds MQK_DATABASE_URL as a value
# =============================================================================
Write-Host ''
Show-Info '--- [5] Registration script never embeds MQK_DATABASE_URL as a value ---'

$dbUrlAssignPattern = '\$env:MQK_DATABASE_URL\s*='
if ($regText -match $dbUrlAssignPattern) {
    Show-Fail "Registration script assigns \$env:MQK_DATABASE_URL -- must never embed a DB URL in the task action"
} else {
    Show-Green "Registration script does not assign \$env:MQK_DATABASE_URL"
}
if ($regText -match 'postgres://[^'']*:[^'']*@') {
    Show-Fail "Registration script appears to embed a literal DB connection string with credentials"
} else {
    Show-Green "No literal DB connection string with credentials found in registration script"
}

# =============================================================================
# [6] Registration script never reads .env.local
# =============================================================================
Write-Host ''
Show-Info '--- [6] Registration script never reads .env.local ---'

# Functional check: a read call (Get-Content/Test-Path/ReadAllText/
# ReadAllLines/dotenvy) touching .env.local, not a plain doc-comment mention
# such as "does NOT read .env.local" (which the script intentionally states).
if ($regText -match '(?i)(Get-Content|Test-Path|ReadAllText|ReadAllLines|dotenvy)[^\r\n]{0,80}\.env\.local') {
    Show-Fail "Registration script appears to functionally read .env.local"
} else {
    Show-Green "Registration script does not functionally read .env.local"
}

# =============================================================================
# [7] Registration script never starts the task it registers
# =============================================================================
Write-Host ''
Show-Info '--- [7] Registration script never starts the task ---'

if ($regText -match 'Start-ScheduledTask') {
    Show-Fail "Registration script calls Start-ScheduledTask -- must never start the task after registering it"
} else {
    Show-Green "Registration script never calls Start-ScheduledTask"
}

# =============================================================================
# [8] Runner script requires MQK_DATABASE_URL
# =============================================================================
Write-Host ''
Show-Info '--- [8] Runner script requires MQK_DATABASE_URL ---'

if ($runnerText -match 'MQK_DATABASE_URL') {
    Show-Green "Runner script references MQK_DATABASE_URL"
    if ($runnerText -match 'database_url_missing' -or $runnerText -match 'database_url_not_paper_db') {
        Show-Green "Runner script fails closed on missing/non-paper MQK_DATABASE_URL"
    } else {
        Show-Fail "Runner script does not appear to fail closed on a missing/non-paper MQK_DATABASE_URL"
    }
} else {
    Show-Fail "Runner script does not reference MQK_DATABASE_URL"
}

# =============================================================================
# [9] Runner script requires MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1 for real mode
# =============================================================================
Write-Host ''
Show-Info '--- [9] Runner script requires MQK_ALLOW_KRAKEN_SCHEDULED_SYNC for real mode ---'

if ($runnerText -match 'MQK_ALLOW_KRAKEN_SCHEDULED_SYNC') {
    Show-Green "Runner script references MQK_ALLOW_KRAKEN_SCHEDULED_SYNC"
    if ($runnerText -match 'scheduled_sync_env_not_set') {
        Show-Green "Runner script fails closed when MQK_ALLOW_KRAKEN_SCHEDULED_SYNC is unset for a real run"
    } else {
        Show-Fail "Runner script does not appear to fail closed when MQK_ALLOW_KRAKEN_SCHEDULED_SYNC is unset"
    }
} else {
    Show-Fail "Runner script does not reference MQK_ALLOW_KRAKEN_SCHEDULED_SYNC"
}

# =============================================================================
# [10] Runner script calls kraken-scheduler-readiness
# =============================================================================
Write-Host ''
Show-Info '--- [10] Runner script calls kraken-scheduler-readiness ---'

if ($runnerText -match 'kraken-scheduler-readiness') {
    Show-Green "Runner script calls kraken-scheduler-readiness"
} else {
    Show-Fail "Runner script does not call kraken-scheduler-readiness"
}

# =============================================================================
# [11] Runner script calls kraken-ohlc-sync only in the real-run path
# =============================================================================
Write-Host ''
Show-Info '--- [11] Runner script calls kraken-ohlc-sync only in the real-run path ---'

if ($runnerText -match 'kraken-ohlc-sync') {
    Show-Green "Runner script references kraken-ohlc-sync"
    # The CheckOnly branch is delimited between the CHECK-ONLY section header
    # and the REAL RUN section header. Check for the actual CLI invocation
    # ("md kraken-ohlc-sync", as it appears in the cargo command line), not
    # any substring mention -- the CheckOnly header text itself legitimately
    # says "(no kraken-ohlc-sync call)" as a doc comment.
    $checkOnlyBlockMatch = [regex]::Match(
        $runnerText,
        '(?s)CHECK-ONLY: gate validation only.*?REAL RUN:'
    )
    if ($checkOnlyBlockMatch.Success -and ($checkOnlyBlockMatch.Value -match 'md kraken-ohlc-sync')) {
        Show-Fail "kraken-ohlc-sync is actually invoked inside the CheckOnly code path"
    } else {
        Show-Green "kraken-ohlc-sync is not invoked inside the CheckOnly code path"
    }
} else {
    Show-Fail "Runner script does not call kraken-ohlc-sync anywhere"
}

# =============================================================================
# [12] Runner script enforces sequential calls with a sleep between symbols
# =============================================================================
Write-Host ''
Show-Info '--- [12] Runner script enforces sequential calls with inter-symbol sleep ---'

if ($runnerText -match 'Start-Sleep') {
    Show-Green "Runner script calls Start-Sleep between symbol calls"
} else {
    Show-Fail "Runner script does not call Start-Sleep -- no enforced spacing between pair calls"
}
if ($runnerText -match 'Start-Job|Start-Process.*-NoNewWindow.*&|ForEach-Object\s+-Parallel') {
    Show-Fail "Runner script appears to dispatch symbol calls in parallel"
} else {
    Show-Green "No parallel dispatch pattern found for symbol calls"
}

# =============================================================================
# [13] Forbidden patterns absent from both scripts
# =============================================================================
Write-Host ''
Show-Info '--- [13] Forbidden daemon/broker/order/risk/runtime patterns absent ---'

$forbidden = [ordered]@{
    '/api/v1/ops/action'      = 'operator action route'
    '/api/v1/strategy/signal' = 'strategy signal route'
    '/api/v1/execution'       = 'execution route'
    'Start-PaperTradingSmoke' = 'paper trading smoke invocation'
    'mqk-daemon'              = 'daemon binary reference'
    'mqk-runtime'             = 'runtime binary reference'
    'mqk-broker'              = 'broker binary reference'
    'sync-provider'           = 'unrelated provider sync CLI command'
    'ingest-provider'         = 'unrelated provider ingest CLI command'
}

foreach ($scriptPair in @(@{ Name = 'Run-KrakenOhlcSync.ps1'; Text = $runnerText }, @{ Name = 'Register-KrakenOhlcSyncTask.ps1'; Text = $regText })) {
    foreach ($pat in $forbidden.Keys) {
        if ($scriptPair.Text -match [regex]::Escape($pat)) {
            Show-Fail "$($scriptPair.Name) contains forbidden pattern '$pat' ($($forbidden[$pat]))"
        } else {
            Show-Green "$($scriptPair.Name): forbidden pattern absent: '$pat'"
        }
    }
}

# =============================================================================
# Helper: run a script in a subprocess
# =============================================================================
function Invoke-Ps1 {
    param([string]$Path, [string[]]$Arguments)
    $local:ErrorActionPreference = 'Continue'
    try {
        $out = & powershell -NoProfile -ExecutionPolicy Bypass -File $Path @Arguments 2>&1
        $ec = $LASTEXITCODE
    } catch {
        $out = @("EXCEPTION: $_")
        $ec  = -99
    }
    return [pscustomobject]@{ Output = $out; ExitCode = $ec }
}

# =============================================================================
# [14] Runner -CheckOnly with a paper DB URL exits 0, evidence is safe
# =============================================================================
Write-Host ''
Show-Info '--- [14] Runner -CheckOnly exits 0; evidence proves zero network/DB/task mutation ---'

if (Test-Path -LiteralPath $RunnerScript) {
    $prevDbUrl = $env:MQK_DATABASE_URL
    $env:MQK_DATABASE_URL = 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable'
    try {
        $r14 = Invoke-Ps1 -Path $RunnerScript -Arguments @('-CheckOnly', '-OutputDir', $TempOutputDir)
        $r14.Output | ForEach-Object { Write-Host "    $_" }

        if ($r14.ExitCode -eq 0) {
            Show-Green "Runner -CheckOnly exited 0"
        } else {
            Show-Fail "Runner -CheckOnly exited $($r14.ExitCode) (expected 0 -- check readiness/DB reachability)"
        }

        $runnerEvidence = Get-ChildItem -Path $TempOutputDir -Filter 'kraken_ohlc_scheduled_runner_*.json' -ErrorAction SilentlyContinue |
            Sort-Object LastWriteTime -Descending | Select-Object -First 1
        if ($null -ne $runnerEvidence) {
            try {
                $ev14 = Get-Content -Raw -Path $runnerEvidence.FullName | ConvertFrom-Json
                if ($ev14.network_call_made -eq $false) { Show-Green "network_call_made=false" } else { Show-Fail "network_call_made was not false" }
                if ($ev14.db_write -eq $false)          { Show-Green "db_write=false" }          else { Show-Fail "db_write was not false" }
                if ($ev14.md_bars_write -eq $false)     { Show-Green "md_bars_write=false" }     else { Show-Fail "md_bars_write was not false" }
                if ($ev14.scheduled_task_mutation -eq $false) { Show-Green "scheduled_task_mutation=false" } else { Show-Fail "scheduled_task_mutation was not false" }
                if ($ev14.mode -eq 'check_only') { Show-Green "mode=check_only" } else { Show-Fail "mode was not check_only" }
            } catch {
                Show-Fail "Could not parse runner evidence JSON: $_"
            }
        } else {
            Show-Fail "No runner evidence JSON found in $TempOutputDir"
        }
    } finally {
        if ($null -eq $prevDbUrl) { Remove-Item Env:\MQK_DATABASE_URL -ErrorAction SilentlyContinue } else { $env:MQK_DATABASE_URL = $prevDbUrl }
    }
} else {
    Show-Fail "Runner script not found; skipping runtime check"
}

# =============================================================================
# [15] Registration -CheckOnly with a test task name exits 0, no task left
# =============================================================================
Write-Host ''
Show-Info '--- [15] Registration -CheckOnly (test task name) exits 0, no mutation ---'

if (Test-Path -LiteralPath $RegScript) {
    $r15 = Invoke-Ps1 -Path $RegScript -Arguments @(
        '-CheckOnly', '-TaskName', $TestTaskName, '-TaskPath', $TestTaskPath, '-OutputDir', $TempOutputDir
    )
    $r15.Output | ForEach-Object { Write-Host "    $_" }

    if ($r15.ExitCode -eq 0) {
        Show-Green "Registration -CheckOnly exited 0"
    } else {
        Show-Fail "Registration -CheckOnly exited $($r15.ExitCode) (expected 0)"
    }

    try {
        $testTaskAfterCheck = Get-ScheduledTask -TaskName $TestTaskName -TaskPath $TestTaskPath -ErrorAction SilentlyContinue
        if ($null -ne $testTaskAfterCheck) {
            Show-Fail "-CheckOnly left a task registered: '$TestTaskName'"
        } else {
            Show-Green "No task '$TestTaskName' registered after -CheckOnly"
        }
    } catch {
        Show-Green "No task '$TestTaskName' registered after -CheckOnly"
    }

    $regEvidence = Join-Path $TempOutputDir 'kraken_ohlc_task_registration.json'
    if (Test-Path -LiteralPath $regEvidence) {
        try {
            $ev15 = Get-Content -Raw -Path $regEvidence | ConvertFrom-Json
            if ($ev15.check_only -eq $true -and $ev15.registered -eq $false) {
                Show-Green "Registration evidence check_only=true, registered=false"
            } else {
                Show-Fail "Registration evidence check_only/registered not in expected safe state"
            }
            if (@($ev15.env_vars_embedded).Count -eq 0) {
                Show-Green "Registration evidence env_vars_embedded is empty"
            } else {
                Show-Fail "Registration evidence env_vars_embedded is not empty: $($ev15.env_vars_embedded -join ', ')"
            }
        } catch {
            Show-Fail "Could not parse registration evidence JSON: $_"
        }
    } else {
        Show-Fail "Registration evidence JSON not found at $regEvidence"
    }
} else {
    Show-Fail "Registration script not found; skipping runtime check"
}

# =============================================================================
# [16] Registration -Unregister (non-existent test task) exits 0, idempotent
# =============================================================================
Write-Host ''
Show-Info '--- [16] Registration -Unregister (non-existent test task): idempotent ---'

if (Test-Path -LiteralPath $RegScript) {
    $r16 = Invoke-Ps1 -Path $RegScript -Arguments @(
        '-Unregister', '-TaskName', $TestTaskName, '-TaskPath', $TestTaskPath, '-OutputDir', $TempOutputDir
    )
    $r16.Output | ForEach-Object { Write-Host "    $_" }

    if ($r16.ExitCode -eq 0) {
        Show-Green "-Unregister (non-existent task) exited 0 (idempotent)"
    } else {
        Show-Fail "-Unregister (non-existent task) exited $($r16.ExitCode) (expected 0)"
    }
} else {
    Show-Fail "Registration script not found; skipping unregister check"
}

# =============================================================================
# [17] Spec doc exists and warns no task/trading enabled by default
# =============================================================================
Write-Host ''
Show-Info '--- [17] Spec doc exists and warns no task/trading enabled by default ---'

if (-not (Test-Path -LiteralPath $SpecDoc)) {
    Show-Fail "Spec doc not found: $SpecDoc"
} else {
    $specText = [System.IO.File]::ReadAllText($SpecDoc)
    Show-Green "Spec doc found: $SpecDoc"
    if ($specText -match '(?i)not\s+register' -or $specText -match '(?i)default[- ]unregistered') {
        Show-Green "Spec doc states the task is not registered by default"
    } else {
        Show-Fail "Spec doc does not clearly state the task is unregistered by default"
    }
    if ($specText -match '(?i)no\s+crypto\s+trading' -or $specText -match '(?i)trading\s+remains?\s+disabled') {
        Show-Green "Spec doc states crypto trading is not enabled"
    } else {
        Show-Fail "Spec doc does not clearly state crypto trading remains disabled"
    }
}

# =============================================================================
# Final safety re-check: neither test task name remains registered
# =============================================================================
Write-Host ''
Show-Info '--- Final safety re-check: no test task remains registered ---'

foreach ($tn in @($TestTaskName, $ExpectedTaskName)) {
    try {
        $finalCheck = Get-ScheduledTask -TaskName $tn -TaskPath $TestTaskPath -ErrorAction SilentlyContinue
        if ($null -ne $finalCheck) {
            Show-Fail "Task '$tn' still exists after validation -- cleanup required"
        } else {
            Show-Green "No task '$tn' remains in Task Scheduler"
        }
    } catch {
        Show-Green "No task '$tn' remains in Task Scheduler"
    }
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
