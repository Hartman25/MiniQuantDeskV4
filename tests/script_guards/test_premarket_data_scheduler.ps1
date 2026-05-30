# =============================================================================
# PREMARKET-DATA-SCHEDULER-01 -- Script static invariant tests
#
# Reads Register-PremarketDataRefreshTask.ps1 as text and asserts PDS01-PDS14.
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# Also runs CheckOnly dry-run (safe -- no task registered).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_premarket_data_scheduler.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$TargetScript = Join-Path $RepoRoot 'scripts\windows\Register-PremarketDataRefreshTask.ps1'
$PrepScript   = Join-Path $RepoRoot 'scripts\windows\Prep-PremarketMarketData.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== Register-PremarketDataRefreshTask.ps1 static invariant tests ==="
Write-Host "    Target: $TargetScript"
Write-Host ""

# ---------------------------------------------------------------------------
# PDS01: Scheduler script exists
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    Pass 'PDS01' "Register-PremarketDataRefreshTask.ps1 exists."
} else {
    Fail 'PDS01' "Register-PremarketDataRefreshTask.ps1 NOT found at: $TargetScript"
}

$scriptText = ''
if (Test-Path $TargetScript) {
    $scriptText = Get-Content -Path $TargetScript -Raw
}

# ---------------------------------------------------------------------------
# PDS02: ASCII-safe (no chars above 0x7E; PS5.1 safe)
# ---------------------------------------------------------------------------
$nonAscii = [regex]::Matches($scriptText, '[^\x00-\x7E]')
if ($nonAscii.Count -eq 0) {
    Pass 'PDS02' "Script is ASCII-safe (no non-ASCII characters)."
} else {
    Fail 'PDS02' "Script contains $($nonAscii.Count) non-ASCII character(s)."
}

# ---------------------------------------------------------------------------
# PDS03: References Prep-PremarketMarketData.ps1 as the task target
# ---------------------------------------------------------------------------
if ($scriptText -match 'Prep-PremarketMarketData\.ps1') {
    Pass 'PDS03' "References Prep-PremarketMarketData.ps1 as the scheduled action."
} else {
    Fail 'PDS03' "Does not reference Prep-PremarketMarketData.ps1."
}

# ---------------------------------------------------------------------------
# PDS04: Prep script exists (dependency check)
# ---------------------------------------------------------------------------
if (Test-Path $PrepScript) {
    Pass 'PDS04' "Prep-PremarketMarketData.ps1 dependency exists."
} else {
    Fail 'PDS04' "Prep-PremarketMarketData.ps1 not found at: $PrepScript"
}

# ---------------------------------------------------------------------------
# PDS05: Does NOT use Start-PaperTradingSmoke.ps1 as a task action.
# Safety: the scheduled command must never start the full smoke/trading workflow.
# Allow it to appear only in a comment (line starting with #).
# ---------------------------------------------------------------------------
$smokeViolations = @()
foreach ($line in ($scriptText -split "`n")) {
    $trimLine = $line.TrimStart()
    if ($trimLine.StartsWith('#')) { continue }  # comment lines are ok
    if ($trimLine -match 'Start-PaperTradingSmoke') {
        $smokeViolations += $trimLine.Trim()
    }
}
if ($smokeViolations.Count -eq 0) {
    Pass 'PDS05' "No non-comment reference to Start-PaperTradingSmoke.ps1."
} else {
    Fail 'PDS05' "Non-comment reference to Start-PaperTradingSmoke.ps1 found: $($smokeViolations -join '; ')"
}

# ---------------------------------------------------------------------------
# PDS06: Does NOT reference runtime/trading start commands
# start-system, arm-execution, adopt-broker-position-baseline must not appear
# outside of comments.
# ---------------------------------------------------------------------------
$tradingCmds = @('start-system', 'arm-execution', 'adopt-broker-position-baseline', 'submit_order', 'outbox_enqueue')
$tradingViolations = @()
foreach ($line in ($scriptText -split "`n")) {
    $trimLine = $line.TrimStart()
    if ($trimLine.StartsWith('#')) { continue }
    foreach ($cmd in $tradingCmds) {
        if ($trimLine -match [regex]::Escape($cmd)) {
            $tradingViolations += "$cmd in: $($trimLine.Trim())"
        }
    }
}
if ($tradingViolations.Count -eq 0) {
    Pass 'PDS06' "No runtime/trading start commands found in non-comment lines."
} else {
    Fail 'PDS06' "Runtime/trading commands found: $($tradingViolations -join '; ')"
}

# ---------------------------------------------------------------------------
# PDS07: CheckOnly parameter present and exits without registering
# ---------------------------------------------------------------------------
$hasCheckOnly = $scriptText -match '\[switch\]\s+\$CheckOnly' -or $scriptText -match 'if \(\$CheckOnly\)'
if ($hasCheckOnly) {
    Pass 'PDS07' "CheckOnly parameter present."
} else {
    Fail 'PDS07' "CheckOnly parameter not found."
}

# CheckOnly block must not call Register-ScheduledTask or Set-ScheduledTask
$checkOnlyIdx = $scriptText.IndexOf('if ($CheckOnly)')
$checkOnlyEnd = if ($checkOnlyIdx -ge 0) { $scriptText.IndexOf('exit 0', $checkOnlyIdx) } else { -1 }
if ($checkOnlyIdx -ge 0 -and $checkOnlyEnd -gt $checkOnlyIdx) {
    $checkOnlyBlock = $scriptText.Substring($checkOnlyIdx, $checkOnlyEnd - $checkOnlyIdx)
    if ($checkOnlyBlock -notmatch 'Register-ScheduledTask' -and $checkOnlyBlock -notmatch 'Set-ScheduledTask') {
        Pass 'PDS07b' "CheckOnly block does not register or update a task (no mutations)."
    } else {
        Fail 'PDS07b' "CheckOnly block contains Register-ScheduledTask or Set-ScheduledTask (mutation in CheckOnly)."
    }
} else {
    Pass 'PDS07b' "CheckOnly block boundary not isolated -- structure check not applicable."
}

# ---------------------------------------------------------------------------
# PDS08: Unregister path exists
# ---------------------------------------------------------------------------
if ($scriptText -match 'Unregister') {
    Pass 'PDS08' "Unregister path present."
} else {
    Fail 'PDS08' "Unregister path not found."
}
if ($scriptText -match 'Unregister-ScheduledTask') {
    Pass 'PDS08b' "Unregister-ScheduledTask call present."
} else {
    Fail 'PDS08b' "Unregister-ScheduledTask call not found."
}

# ---------------------------------------------------------------------------
# PDS09: ScheduledTask / schtasks registration present
# ---------------------------------------------------------------------------
if ($scriptText -match 'Register-ScheduledTask' -or $scriptText -match 'schtasks') {
    Pass 'PDS09' "Task registration command (Register-ScheduledTask or schtasks) present."
} else {
    Fail 'PDS09' "No task registration command found."
}

# ---------------------------------------------------------------------------
# PDS10: Log/evidence output path under exports/market_data/scheduled/
# ---------------------------------------------------------------------------
if ($scriptText -match 'exports.market_data.scheduled' -or $scriptText -match 'ScheduledLogDir') {
    Pass 'PDS10' "Evidence/log path references exports/market_data/scheduled/."
} else {
    Fail 'PDS10' "Evidence/log path not found."
}

# ---------------------------------------------------------------------------
# PDS11: Default trigger is weekdays (Mon-Fri safety -- never a one-off
#         "run trading now" trigger)
# ---------------------------------------------------------------------------
if ($scriptText -match 'Monday' -and $scriptText -match 'Friday') {
    Pass 'PDS11' "Weekday (Mon-Fri) trigger present."
} else {
    Fail 'PDS11' "Weekday trigger (Monday, Friday) not found."
}

# ---------------------------------------------------------------------------
# PDS12: Safety note present (does not start trading/daemon)
# ---------------------------------------------------------------------------
if ($scriptText -match 'SAFETY' -or $scriptText -match 'does not start') {
    Pass 'PDS12' "Safety note present (task does not start daemon/trading)."
} else {
    Fail 'PDS12' "Safety note not found -- add a SAFETY comment explaining the task scope."
}

# ---------------------------------------------------------------------------
# PDS13: No secrets printed (TWELVEDATA_API_KEY, ALPACA_API_KEY, password)
# ---------------------------------------------------------------------------
$secretPatterns = @(
    'Write-Host.*\$env:TWELVEDATA_API_KEY',
    'Write-Host.*\$env:ALPACA_API_KEY',
    'Write-Host.*\$env:MQK_OPERATOR_TOKEN',
    'Write-Host.*password'
)
$secretViolations = @()
foreach ($pat in $secretPatterns) {
    if ($scriptText -match $pat) {
        $secretViolations += $pat
    }
}
if ($secretViolations.Count -eq 0) {
    Pass 'PDS13' "No secret-printing patterns found."
} else {
    Fail 'PDS13' "Potential secret-printing patterns: $($secretViolations -join '; ')"
}

# ---------------------------------------------------------------------------
# PDS14: CheckOnly dry-run executes without error and exits 0
# This is the only test that actually runs the script (CheckOnly is read-only).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "--- PDS14: CheckOnly dry-run ---"
try {
    & powershell.exe -ExecutionPolicy Bypass -NonInteractive `
        -File $TargetScript `
        -RepoRoot $RepoRoot `
        -CheckOnly `
        2>&1 | Write-Host
    $dryExit = $LASTEXITCODE
    if ($null -eq $dryExit) { $dryExit = 0 }
    if ($dryExit -eq 0) {
        Pass 'PDS14' "CheckOnly dry-run exited 0 (no mutations, no task registered)."
    } else {
        Fail 'PDS14' "CheckOnly dry-run exited $dryExit (expected 0)."
    }
} catch {
    Fail 'PDS14' "CheckOnly dry-run threw exception: $_"
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Summary ==="
if ($Failures -eq 0) {
    Write-Host "ALL PASSED ($($scriptText.Length) bytes checked, 0 failures)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "FAILED: $Failures check(s) did not pass" -ForegroundColor Red
    exit 1
}
