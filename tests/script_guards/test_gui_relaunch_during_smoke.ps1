# =============================================================================
# GUI-RELAUNCH-DURING-SMOKE-01
# Script guard: test_gui_relaunch_during_smoke.ps1
#
# Static text/AST assertions against scripts\windows\Start-PaperTradingSmoke.ps1's
# GUI relaunch-in-observe-mode behavior (Start-GuiObserveIfRequested + STEP 8B).
# No daemon, no DB, no live calls, no .env.local required. All checks are
# read-only source inspections.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_gui_relaunch_during_smoke.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
#
# Guard assertions:
#   GR01  Target script exists
#   GR02  -SkipGui is referenced in executable code (no longer a dead parameter)
#   GR03  Start-GuiObserveIfRequested exists and calls Start-Process -FilePath $guiExe
#   GR04  -SkipGui early-return precedes Start-Process inside the helper (SkipGui prevents relaunch)
#   GR05  GUI relaunch call site occurs after the CHECK-ONLY exit block (CheckOnly never launches GUI)
#   GR06  Launch is wrapped in try/catch; catch block does not call exit (non-fatal failure)
#   GR07  Missing GUI binary path is explicitly reported, not silently ignored
#   GR08  No secret env vars referenced in the new GUI relaunch code
#   GR09  No new arm/disarm/clear-halted-run/flatten/order actions introduced
#   GR10  Final summary reports GUI observation status
#   GR11  Script parses cleanly under the Windows PowerShell 5.1 AST parser
#   GR12  Script is ASCII-safe (no non-ASCII bytes beyond UTF-8 BOM)
#   GR13  Script contains no known mojibake or unsafe Unicode punctuation
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$TargetScript = Join-Path $RepoRoot 'scripts\windows\Start-PaperTradingSmoke.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== GUI relaunch-during-smoke static invariant tests (GUI-RELAUNCH-DURING-SMOKE-01) ==="
Write-Host "    Target: $TargetScript"
Write-Host ""

# ---------------------------------------------------------------------------
# GR01: Script exists
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    Pass 'GR01' "Script exists at scripts\windows\Start-PaperTradingSmoke.ps1"
} else {
    Fail 'GR01' "Script NOT found at: $TargetScript"
}

$scriptText = ''
if (Test-Path $TargetScript) {
    $scriptText = Get-Content -Path $TargetScript -Raw
}

# ---------------------------------------------------------------------------
# GR02: -SkipGui is referenced in executable code (functional, not dead)
# ---------------------------------------------------------------------------
if ($scriptText -match [regex]::Escape('-SkipGui:$SkipGui') -and
    $scriptText -match [regex]::Escape('$SkipGui.IsPresent')) {
    Pass 'GR02' '-SkipGui is passed to the GUI helper and checked via $SkipGui.IsPresent'
} else {
    Fail 'GR02' '-SkipGui must be passed to a helper (-SkipGui:$SkipGui) and checked ($SkipGui.IsPresent)'
}

# ---------------------------------------------------------------------------
# Extract the Start-GuiObserveIfRequested function body and the STEP 8B
# call-site block for the remaining checks.
# ---------------------------------------------------------------------------
$guiFnMatch = [regex]::Match($scriptText, '(?s)function Start-GuiObserveIfRequested.*?(?=# CHECK-ONLY mode:)')
$guiFnBody  = if ($guiFnMatch.Success) { $guiFnMatch.Value } else { '' }

$step8bMatch = [regex]::Match($scriptText, '(?s)# -{10,}\r?\n# STEP 8B: Relaunch GUI.*?(?=# -{10,}\r?\n# STEP 9:)')
$step8bBody  = if ($step8bMatch.Success) { $step8bMatch.Value } else { '' }

$guiCombined = $guiFnBody + "`n" + $step8bBody

# ---------------------------------------------------------------------------
# GR03: Start-GuiObserveIfRequested exists and calls Start-Process -FilePath $guiExe
# ---------------------------------------------------------------------------
if ($guiFnMatch.Success -and $guiFnBody -match [regex]::Escape('Start-Process -FilePath $guiExe')) {
    Pass 'GR03' 'Start-GuiObserveIfRequested exists and relaunches via Start-Process -FilePath $guiExe'
} else {
    Fail 'GR03' 'Start-GuiObserveIfRequested must exist and call Start-Process -FilePath $guiExe'
}

# ---------------------------------------------------------------------------
# GR04: -SkipGui early-return precedes Start-Process inside the helper
# ---------------------------------------------------------------------------
$skipIdx    = $guiFnBody.IndexOf('$SkipGui.IsPresent')
$startIdx   = $guiFnBody.IndexOf('Start-Process -FilePath $guiExe')
if ($guiFnMatch.Success -and $skipIdx -ge 0 -and $startIdx -gt $skipIdx) {
    Pass 'GR04' '-SkipGui is checked (and returns early) before Start-Process is reached'
} else {
    Fail 'GR04' '-SkipGui check must precede and short-circuit the Start-Process relaunch'
}

# ---------------------------------------------------------------------------
# GR05: GUI relaunch call site occurs after the CHECK-ONLY exit block
# ---------------------------------------------------------------------------
$checkOnlyIdx = $scriptText.IndexOf('Write-Section "CHECK-ONLY complete"')
$callSiteIdx  = $scriptText.IndexOf('$guiStatus = Start-GuiObserveIfRequested')
if ($checkOnlyIdx -ge 0 -and $callSiteIdx -gt $checkOnlyIdx) {
    Pass 'GR05' 'GUI relaunch call site is after the CHECK-ONLY exit block (CheckOnly never launches GUI)'
} else {
    Fail 'GR05' 'GUI relaunch call site must occur after the CHECK-ONLY exit block'
}

# ---------------------------------------------------------------------------
# GR06: Launch is wrapped in try/catch; catch block does not call exit
# ---------------------------------------------------------------------------
$catchMatch = [regex]::Match($guiFnBody, '(?s)catch\s*\{(.*?)\}')
if ($guiFnBody -match 'try\s*\{' -and $catchMatch.Success -and $catchMatch.Groups[1].Value -notmatch '\bexit\b') {
    Pass 'GR06' 'GUI relaunch is wrapped in try/catch and the catch block does not call exit (non-fatal)'
} else {
    Fail 'GR06' 'GUI relaunch must be wrapped in try/catch with a non-fatal catch block (no exit)'
}

# ---------------------------------------------------------------------------
# GR07: Missing GUI binary path is explicitly reported
# ---------------------------------------------------------------------------
if ($guiFnBody -match [regex]::Escape('$null -eq $guiExe') -and
    $guiFnBody -match '(?i)not found' -and
    $guiFnBody -match '(?i)unavailable') {
    Pass 'GR07' 'Missing GUI binary is detected and reported as unavailable (not silently ignored)'
} else {
    Fail 'GR07' 'Missing GUI binary path must be detected and reported as unavailable'
}

# ---------------------------------------------------------------------------
# GR08: No secret env vars referenced in the new GUI relaunch code
# ---------------------------------------------------------------------------
$secretPatterns = @(
    'ALPACA_API_KEY', 'ALPACA_API_SECRET', 'MQK_OPERATOR_TOKEN',
    'DISCORD_WEBHOOK', 'POSTGRES_PASSWORD', 'DATABASE_URL', '\$env:'
)
$secretsFound = @()
foreach ($pat in $secretPatterns) {
    if ($guiCombined -match $pat) { $secretsFound += $pat }
}
if ($secretsFound.Count -eq 0) {
    Pass 'GR08' 'No secret env vars referenced in Start-GuiObserveIfRequested or STEP 8B'
} else {
    Fail 'GR08' "Secret-related pattern(s) found in new GUI code: $($secretsFound -join ', ')"
}

# ---------------------------------------------------------------------------
# GR09: No new arm/disarm/clear-halted-run/flatten/order actions introduced
# ---------------------------------------------------------------------------
$tradingPatterns = @(
    '(?i)arm-execution', '(?i)disarm-execution', '(?i)clear-halted-run', '(?i)flatten',
    'submit_order', '(?i)cancel_order', '(?i)replace_order', '/v2/orders', 'order_type'
)
$tradingFound = @()
foreach ($pat in $tradingPatterns) {
    if ($guiCombined -match $pat) { $tradingFound += $pat }
}
if ($tradingFound.Count -eq 0) {
    Pass 'GR09' 'No new arm/disarm/clear-halted-run/flatten/order actions in the new GUI code'
} else {
    Fail 'GR09' "Trading-mutation pattern(s) found in new GUI code: $($tradingFound -join ', ')"
}

# ---------------------------------------------------------------------------
# GR10: Final summary reports GUI observation status
# ---------------------------------------------------------------------------
$summaryMatch = [regex]::Match($scriptText, '(?s)# Final summary.*')
$summaryBody  = if ($summaryMatch.Success) { $summaryMatch.Value } else { '' }
if ($summaryBody -match 'GUI observation:' -and $summaryBody -match [regex]::Escape('$guiStatus.launched')) {
    Pass 'GR10' 'Final summary reports GUI observation status from $guiStatus'
} else {
    Fail 'GR10' 'Final summary must report GUI observation status (launched / skipped / unavailable)'
}

# ---------------------------------------------------------------------------
# GR11: Script parses cleanly under the Windows PowerShell 5.1 AST parser
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    $gr11Tokens = $null
    $gr11Errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile(
        $TargetScript,
        [ref]$gr11Tokens,
        [ref]$gr11Errors
    ) | Out-Null
    if ($gr11Errors.Count -eq 0) {
        Pass 'GR11' ("PS5.1 AST parser: PARSE_OK (" + $gr11Tokens.Count + " tokens)")
    } else {
        foreach ($e in $gr11Errors) {
            Fail 'GR11' ("Parse error at line " + $e.Extent.StartLineNumber + ": " + $e.Message)
        }
    }
} else {
    Fail 'GR11' 'Cannot run parse check - script file not found (see GR01)'
}

# ---------------------------------------------------------------------------
# GR12: Script is ASCII-safe -- no non-ASCII bytes beyond the UTF-8 BOM
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    $gr12Bytes  = [System.IO.File]::ReadAllBytes($TargetScript)
    $gr12Offset = 0
    if ($gr12Bytes.Length -ge 3 -and
        $gr12Bytes[0] -eq 0xEF -and
        $gr12Bytes[1] -eq 0xBB -and
        $gr12Bytes[2] -eq 0xBF) {
        $gr12Offset = 3
    }
    $gr12High = @()
    for ($i = $gr12Offset; $i -lt $gr12Bytes.Length; $i++) {
        if ($gr12Bytes[$i] -gt 0x7F) { $gr12High += $i }
    }
    if ($gr12High.Count -eq 0) {
        Pass 'GR12' ("Script is ASCII-safe (no non-ASCII bytes after BOM; BOM offset=" + $gr12Offset + ")")
    } else {
        $gr12Preview = if ($gr12High.Count -gt 5) { $gr12High[0..4] } else { $gr12High[0..($gr12High.Count - 1)] }
        Fail 'GR12' ("Found " + $gr12High.Count + " non-ASCII byte(s) at positions: " + ($gr12Preview -join ', '))
    }
} else {
    Fail 'GR12' 'Cannot run ASCII check - script file not found (see GR01)'
}

# ---------------------------------------------------------------------------
# GR13: Script contains no known mojibake or unsafe Unicode punctuation
# ---------------------------------------------------------------------------
$gr13Chars = @(
    [char]0x00E2,  # circumflex-a (UTF-8 mojibake prefix)
    [char]0x2014,  # em dash
    [char]0x2013,  # en dash
    [char]0x2192,  # rightward arrow
    [char]0x2713,  # check mark
    [char]0x201C,  # left double quotation mark
    [char]0x201D,  # right double quotation mark
    [char]0x2018,  # left single quotation mark
    [char]0x2019   # right single quotation mark
)
$gr13Found = @()
foreach ($ch in $gr13Chars) {
    if ($scriptText.Contains([string]$ch)) {
        $gr13Found += ('U+' + ([int]$ch).ToString('X4'))
    }
}
if ($gr13Found.Count -eq 0) {
    Pass 'GR13' 'No mojibake or unsafe Unicode punctuation in script'
} else {
    Fail 'GR13' ("Unsafe Unicode chars found: " + ($gr13Found -join ', '))
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL GUI RELAUNCH GUARD INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
