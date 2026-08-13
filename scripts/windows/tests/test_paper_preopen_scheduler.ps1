# =============================================================================
# test_paper_preopen_scheduler.ps1
# PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01
#
# Proof for scripts\windows\Register-PaperStartupTask.ps1.
#
# NON-MUTATING: this test performs static source-guard assertions against
# the registration helper's own text (an AST parse plus regex/substring
# checks). It never calls Register-ScheduledTask, Set-ScheduledTask,
# Enable-ScheduledTask, Disable-ScheduledTask, Unregister-ScheduledTask,
# Start-ScheduledTask, or Stop-ScheduledTask, and never invokes the helper
# itself. Zero real Task Scheduler side effects.
#
# Exit codes: 0 = all proofs held, 1 = at least one did not.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$WindowsDir = (Resolve-Path (Join-Path $ScriptDir '..')).Path.TrimEnd('\')
$RepoRoot   = (Resolve-Path (Join-Path $WindowsDir '..\..')).Path.TrimEnd('\')
$Helper     = Join-Path $WindowsDir 'Register-PaperStartupTask.ps1'
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

if (-not (Test-Path -LiteralPath $Helper)) {
    Show-Red "FATAL -- registration helper not found: $Helper"
    exit 1
}
if (-not (Test-Path -LiteralPath $Launcher)) {
    Show-Red "FATAL -- launcher script not found: $Launcher"
    exit 1
}

$HelperText = Get-Content -Path $Helper -Raw

# ---------------------------------------------------------------------------
# Section 1: the helper parses successfully (AST, no execution)
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 1: parse guard ==='

$parseErrors = $null
$tokens = $null
[System.Management.Automation.Language.Parser]::ParseFile($Helper, [ref]$tokens, [ref]$parseErrors) | Out-Null
Assert-True 'registration helper parses successfully (0 AST parse errors)' `
    ($null -ne $parseErrors -and $parseErrors.Count -eq 0)

# ---------------------------------------------------------------------------
# Section 2: default configuration
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 2: default configuration ==='

Assert-True 'default task name is stable (MiniQuantDesk-Paper-Preopen-Startup)' `
    ($HelperText -match "\`$TaskName\s*=\s*'MiniQuantDesk-Paper-Preopen-Startup'")

Assert-True 'default time is 02:00' `
    ($HelperText -match "\`$StartTime\s*=\s*'02:00'")

Assert-True 'trigger is Monday-Friday weekly' `
    ($HelperText -match 'New-ScheduledTaskTrigger\s+-Weekly\s+-DaysOfWeek\s+Monday,Tuesday,Wednesday,Thursday,Friday')

Assert-True 'uses the \MiniQuantDesk\ Task Scheduler folder convention by default' `
    ($HelperText -match "\`$TaskPath\s*=\s*'\\MiniQuantDesk\\'")

# ---------------------------------------------------------------------------
# Section 3: task action contract
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 3: task action contract ==='

Assert-True 'action resolves Start-MiniQuantDesk.ps1' `
    ($HelperText -match [regex]::Escape('Start-MiniQuantDesk.ps1'))

Assert-True 'action contains -Mode Paper -Scheduled' `
    ($HelperText -match [regex]::Escape('-Mode Paper -Scheduled'))

Assert-True 'action does not pass -SkipGui directly' `
    (-not ($HelperText -match [regex]::Escape('-SkipGui')))

Assert-True 'action does not pass -ArmPaper directly' `
    (-not ($HelperText -match [regex]::Escape('-ArmPaper')))

Assert-True 'action does not pass a -RepoRoot argument to the launcher' `
    (-not ($HelperText -match [regex]::Escape('-RepoRoot "') -or $HelperText -match [regex]::Escape("-RepoRoot '")))

Assert-True 'no Start-PaperTradingSmoke.ps1 authority' `
    (-not ($HelperText -match [regex]::Escape('Start-PaperTradingSmoke.ps1')))

Assert-True 'no Prep-PremarketMarketData.ps1 authority' `
    (-not ($HelperText -match [regex]::Escape('Prep-PremarketMarketData.ps1')))

Assert-True 'no Refresh-IntradayMarketData.ps1 authority' `
    (-not ($HelperText -match [regex]::Escape('Refresh-IntradayMarketData.ps1')))

Assert-True 'no Live scheduling (helper source never mentions Live trading mode)' `
    (-not ($HelperText -match '(?i)live'))

Assert-True 'no direct runtime/order action (no action_key literals, no HTTP calls)' `
    (-not ($HelperText -match [regex]::Escape('start-system') -or `
           $HelperText -match [regex]::Escape('arm-execution') -or `
           $HelperText -match [regex]::Escape('adopt-broker-position-baseline') -or `
           $HelperText -match 'Invoke-WebRequest|Invoke-RestMethod'))

$expectedArgumentsLine = ($HelperText -split "`n") | Where-Object { $_ -match '\$ExpectedArguments\s*=' } | Select-Object -First 1
Assert-True 'no secrets in action arguments (the built action string has no env-var interpolation and no credential/token/API-key literal)' `
    ($null -ne $expectedArgumentsLine -and -not ($expectedArgumentsLine -match '\$env:' -or $expectedArgumentsLine -match '(?i)(api[_-]?key|api[_-]?secret|token|password|credential)'))

# ---------------------------------------------------------------------------
# Section 4: working directory, executable, principal, settings
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 4: working directory, executable, principal, settings ==='

Assert-True 'working directory is RepoRoot' `
    ($HelperText -match [regex]::Escape('-WorkingDirectory $RepoRoot'))

Assert-True 'Windows PowerShell path is explicit (absolute System32 path, with Get-Command fallback)' `
    ($HelperText -match [regex]::Escape('System32\WindowsPowerShell\v1.0\powershell.exe'))

Assert-True 'Interactive + Limited principal' `
    ($HelperText -match [regex]::Escape('-LogonType Interactive') -and $HelperText -match [regex]::Escape('-RunLevel Limited'))

Assert-True 'MultipleInstances IgnoreNew' `
    ($HelperText -match [regex]::Escape('-MultipleInstances IgnoreNew'))

Assert-True 'RestartCount 2' `
    ($HelperText -match [regex]::Escape('-RestartCount 2'))

Assert-True 'RestartInterval 10 minutes' `
    ($HelperText -match [regex]::Escape('-RestartInterval (New-TimeSpan -Minutes 10)'))

Assert-True 'ExecutionTimeLimit 1 hour' `
    ($HelperText -match [regex]::Escape('-ExecutionTimeLimit (New-TimeSpan -Hours 1)'))

Assert-True 'StartWhenAvailable' `
    ($HelperText -match [regex]::Escape('-StartWhenAvailable'))

Assert-True 'WakeToRun' `
    ($HelperText -match [regex]::Escape('-WakeToRun'))

# ---------------------------------------------------------------------------
# Section 5: idempotency and coexistence contract
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 5: idempotency and coexistence contract ==='

Assert-True 'existing task uses the update path (Set-ScheduledTask) guarded by existence check' `
    ($HelperText -match 'if\s*\(\s*\$taskExistedBefore\s*\)' -and $HelperText -match 'Set-ScheduledTask')

Assert-True 'absent task uses the registration path (Register-ScheduledTask)' `
    ($HelperText -match 'Register-ScheduledTask')

Assert-True 'new task defaults DISABLED (desiredEnabled=false when no prior task and no -Enable)' `
    ($HelperText -match [regex]::Escape('$desiredEnabled = $false'))

Assert-True '-Enable is an explicit switch parameter' `
    ($HelperText -match '\[switch\]\s*\$Enable')

Assert-True 'existing task preserves its own current enabled/disabled state absent -Enable' `
    ($HelperText -match [regex]::Escape('$priorEnabledState'))

Assert-True 'helper contains no Unregister-ScheduledTask call' `
    (-not ($HelperText -match [regex]::Escape('Unregister-ScheduledTask')))

Assert-True 'helper contains no Stop-ScheduledTask call' `
    (-not ($HelperText -match [regex]::Escape('Stop-ScheduledTask')))

Assert-True 'helper does not name or mutate the temporary August soak task' `
    (-not ($HelperText -match [regex]::Escape('MiniQuantDesk-2026-08-PaperSoak-Startup')))

# ---------------------------------------------------------------------------
# Section 6: post-registration self-check
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 6: post-registration self-check ==='

Assert-True 'post-registration exactly-one-action verification exists' `
    ($HelperText -match [regex]::Escape('@($readBack.Actions).Count') -and $HelperText -match [regex]::Escape('expected_exactly_one_action_found'))

Assert-True 'post-registration verification re-reads the task via Get-ScheduledTask rather than trusting in-memory objects' `
    ($HelperText -match [regex]::Escape('$readBack = Get-ScheduledTask'))

Assert-True 'post-registration verification checks executable, arguments, and working directory' `
    ($HelperText -match [regex]::Escape('action_executable_mismatch') -and `
     $HelperText -match [regex]::Escape('action_arguments_mismatch') -and `
     $HelperText -match [regex]::Escape('working_directory_mismatch'))

Assert-True 'post-registration verification checks activation state and fails closed (exit 1) on any self-check failure' `
    ($HelperText -match [regex]::Escape('activation_state_mismatch') -and $HelperText -match [regex]::Escape('exit 1'))

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
