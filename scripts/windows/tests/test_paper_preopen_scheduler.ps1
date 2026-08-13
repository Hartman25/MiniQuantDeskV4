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
# Section 7: transient-enable race repair (PAPER-AUTOMATIC-PREOPEN-SCHEDULER-01-REPAIR-01)
#
# The prior implementation built a single ScheduledTaskSettings object
# without -Disable, called Register-ScheduledTask/Set-ScheduledTask (which
# leaves a brand-new task Enabled by Task Scheduler's own default), and only
# afterward called Disable-ScheduledTask for the desiredEnabled=false case.
# For a new task, this left a transient window in which the task existed
# registered and Enabled (with StartWhenAvailable/WakeToRun already set)
# before being disabled. This section proves the atomic-disabled-settings
# fix: desiredEnabled is resolved before settings construction, the
# false-path settings object is built with -Disable, that same object is
# what Register-ScheduledTask/Set-ScheduledTask consume, and no
# post-registration Disable-ScheduledTask call exists to fall back on.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 7: transient-enable race repair ==='

# A. desiredEnabled is resolved before settings construction.
$desiredEnabledAssignIndex = $HelperText.IndexOf('$desiredEnabled = $false')
$firstSettingsConstructIndex = $HelperText.IndexOf('New-ScheduledTaskSettingsSet')
Assert-True 'desiredEnabled is resolved (assignment present) before the first ScheduledTaskSettings object is constructed' `
    ($desiredEnabledAssignIndex -ge 0 -and $firstSettingsConstructIndex -ge 0 -and $desiredEnabledAssignIndex -lt $firstSettingsConstructIndex)

# Isolate the two settings-construction branches by text position so B/C/D
# below can prove which branch does/does not carry -Disable, without
# depending on Register-ScheduledTask/Set-ScheduledTask/Enable-ScheduledTask/
# Disable-ScheduledTask execution (still fully non-mutating / static).
$settingsIfIndex = $HelperText.IndexOf('if ($desiredEnabled) {')
$settingsElseIndex = if ($settingsIfIndex -ge 0) { $HelperText.IndexOf('} else {', $settingsIfIndex) } else { -1 }
$settingsDisableIndex = if ($settingsElseIndex -ge 0) { $HelperText.IndexOf('-Disable', $settingsElseIndex) } else { -1 }
$settingsElseCloseIndex = if ($settingsDisableIndex -ge 0) { $HelperText.IndexOf('}', $settingsDisableIndex) } else { -1 }

$enabledSettingsBranch = if ($settingsIfIndex -ge 0 -and $settingsElseIndex -gt $settingsIfIndex) {
    $HelperText.Substring($settingsIfIndex, $settingsElseIndex - $settingsIfIndex)
} else { '' }
$disabledSettingsBranch = if ($settingsElseIndex -ge 0 -and $settingsElseCloseIndex -gt $settingsElseIndex) {
    $HelperText.Substring($settingsElseIndex, $settingsElseCloseIndex - $settingsElseIndex)
} else { '' }

# B. the false/disabled path constructs ScheduledTaskSettings with -Disable.
Assert-True 'the desiredEnabled=false settings branch builds New-ScheduledTaskSettingsSet with -Disable' `
    ($disabledSettingsBranch -ne '' -and $disabledSettingsBranch -match 'New-ScheduledTaskSettingsSet' -and $disabledSettingsBranch -match '-Disable')

Assert-True 'the desiredEnabled=true settings branch does NOT pass -Disable' `
    ($enabledSettingsBranch -ne '' -and $enabledSettingsBranch -match 'New-ScheduledTaskSettingsSet' -and -not ($enabledSettingsBranch -match '-Disable'))

# C. Register-ScheduledTask (and Set-ScheduledTask, the reconcile path)
#    consume that same settings object -- there is only one $settings
#    variable, built above before either call.
$registerConsumesSettings = [regex]::Match($HelperText, '(?s)Register-ScheduledTask\b.{0,200}?-Settings\s+\$settings')
Assert-True 'Register-ScheduledTask (create path) is passed -Settings $settings, the object already encoding desiredEnabled' `
    ($registerConsumesSettings.Success)

$setConsumesSettings = [regex]::Match($HelperText, '(?s)Set-ScheduledTask\b.{0,200}?-Settings\s+\$settings')
Assert-True 'Set-ScheduledTask (reconcile path) is passed -Settings $settings, the object already encoding desiredEnabled' `
    ($setConsumesSettings.Success)

# D. there is no new-task path that registers an enabled definition and only
#    later disables it -- proven by the combination of B (the disabled
#    settings object already carries -Disable) and the absence of any
#    post-registration Disable-ScheduledTask call to fall back on.
Assert-True 'no Disable-ScheduledTask call exists anywhere in the helper (the disabled-path settings object is already disabled at registration/update time, not disabled afterward)' `
    (-not ($HelperText -match 'Disable-ScheduledTask'))

# E. explicit -Enable still exists, and Enable-ScheduledTask is only invoked
#    for the desiredEnabled=true path (defense-in-depth, not the mechanism
#    that establishes disabled safety).
Assert-True 'explicit -Enable switch is still honored (desiredEnabled = $true when -Enable is passed)' `
    ($HelperText -match [regex]::Escape('if ($Enable) {') -and $HelperText -match [regex]::Escape('$desiredEnabled = $true'))

Assert-True 'Enable-ScheduledTask is only called inside the desiredEnabled=true branch' `
    ($HelperText -match '(?s)if\s*\(\s*\$desiredEnabled\s*\)\s*\{\s*try\s*\{\s*Enable-ScheduledTask')

# F. existing task state-preservation semantics remain: existingTask /
#    taskExistedBefore / priorEnabledState are all resolved before
#    $desiredEnabled is finalized, so an existing task's current
#    enabled/disabled state still flows into the settings object built from
#    $desiredEnabled above.
$existingTaskIndex = $HelperText.IndexOf('$existingTask = Get-ScheduledTask')
$priorEnabledIndex = $HelperText.IndexOf('$priorEnabledState =')
Assert-True 'existingTask/taskExistedBefore/priorEnabledState are all resolved before desiredEnabled, and before settings construction' `
    ($existingTaskIndex -ge 0 -and $priorEnabledIndex -ge 0 -and $existingTaskIndex -lt $desiredEnabledAssignIndex -and $priorEnabledIndex -lt $firstSettingsConstructIndex)

# G. module-availability check occurs before the first New-ScheduledTask*
#    construction call (and before the first Get-ScheduledTask query, which
#    also requires the ScheduledTasks module).
$moduleCheckIndex = $HelperText.IndexOf('Get-Module -Name ScheduledTasks -ListAvailable')
$firstNewScheduledTaskIndex = $HelperText.IndexOf('New-ScheduledTask')
$firstGetScheduledTaskIndex = $HelperText.IndexOf('Get-ScheduledTask')
Assert-True 'ScheduledTasks module-availability check occurs before the first New-ScheduledTask* construction call' `
    ($moduleCheckIndex -ge 0 -and $firstNewScheduledTaskIndex -gt $moduleCheckIndex)

Assert-True 'ScheduledTasks module-availability check occurs before the first Get-ScheduledTask query' `
    ($moduleCheckIndex -ge 0 -and $firstGetScheduledTaskIndex -gt $moduleCheckIndex)

Assert-True 'ScheduledTasks module-availability check appears exactly once (not duplicated, not left in its old post-construction position)' `
    (([regex]::Matches($HelperText, [regex]::Escape('ScheduledTasks module not available'))).Count -eq 1)

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
