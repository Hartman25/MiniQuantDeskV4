# =============================================================================
# OPERATOR-RUNBOOK-STARTUP-HARDENING-01 -- Script static-invariant tests
#
# Reads Start-PaperTradingSmoke.ps1 as text and asserts OPR01-OPR08.
# No daemon, no DB, no live calls. Pure text invariant checks.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_start_paper_trading_smoke.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot   = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$TargetScript = Join-Path $RepoRoot 'scripts\windows\Start-PaperTradingSmoke.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== Start-PaperTradingSmoke.ps1 static invariant tests ==="
Write-Host "    Target: $TargetScript"
Write-Host ""

# ---------------------------------------------------------------------------
# OPR01: Script exists at expected path
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    Pass 'OPR01' "Script exists at scripts\windows\Start-PaperTradingSmoke.ps1"
} else {
    Fail 'OPR01' "Script NOT found at: $TargetScript"
}

# Load script text for remaining checks
$scriptText = ''
if (Test-Path $TargetScript) {
    $scriptText = Get-Content -Path $TargetScript -Raw
}

# ---------------------------------------------------------------------------
# OPR02: Script contains required endpoint calls
# ---------------------------------------------------------------------------
$requiredEndpoints = @(
    '/v1/health',
    '/api/v1/system/status',
    '/api/v1/system/preflight',
    '/api/v1/autonomous/readiness',
    '/api/v1/alerts/active',
    '/api/v1/execution/orders',
    '/api/v1/reconcile/status',
    '/api/v1/ops/action',
    '/api/v1/ops/repair/adopt-broker-position-baseline'
)

foreach ($ep in $requiredEndpoints) {
    if ($scriptText -match [regex]::Escape($ep)) {
        Pass 'OPR02' "Contains endpoint: $ep"
    } else {
        Fail 'OPR02' "Missing endpoint: $ep"
    }
}

# ---------------------------------------------------------------------------
# OPR03: Script reasserts MQK_DATABASE_URL after loading .env.local
# ---------------------------------------------------------------------------
# The reassert must appear AFTER the .env.local load block.
# We verify both patterns are present and that the reassert pattern follows the load pattern.
$loadPattern     = [regex]::Escape('.env.local')
$reassertPattern = [regex]::Escape('$env:MQK_DATABASE_URL')

$loadIdx     = $scriptText.IndexOf('.env.local')
$reassertIdx = $scriptText.IndexOf('$env:MQK_DATABASE_URL')

if ($reassertIdx -gt $loadIdx -and $loadIdx -ge 0) {
    Pass 'OPR03' 'MQK_DATABASE_URL reasserted after .env.local load'
} else {
    Fail 'OPR03' "MQK_DATABASE_URL reassert (idx=$reassertIdx) must follow .env.local load (idx=$loadIdx)"
}

# ---------------------------------------------------------------------------
# OPR04: Script refuses / halts on live_routing_enabled=true
# ---------------------------------------------------------------------------
$hasLiveRoutingGuard = (
    $scriptText -match 'live_routing_enabled' -and
    ($scriptText -match "Refusing to continue" -or $scriptText -match 'exit 1') -and
    $scriptText -match 'TRUE-DANGER|live_routing_enabled.*true'
)
if ($hasLiveRoutingGuard) {
    Pass 'OPR04' 'Script guards against live_routing_enabled=true'
} else {
    Fail 'OPR04' 'Script must refuse/halt if live_routing_enabled=true'
}

# ---------------------------------------------------------------------------
# OPR05: Script does not print secret env values
# ---------------------------------------------------------------------------
$secretPatterns = @(
    '\$env:ALPACA_API_KEY_PAPER',
    '\$env:ALPACA_API_SECRET_PAPER',
    '\$env:ALPACA_API_KEY_LIVE',
    '\$env:ALPACA_API_SECRET_LIVE',
    '\$env:MQK_OPERATOR_TOKEN\b',
    '\$env:MQK_DISCORD_WEBHOOK',
    'Write-Host.*ALPACA_API_KEY',
    'Write-Host.*ALPACA_API_SECRET',
    'Write-Host.*MQK_OPERATOR_TOKEN.*\$'
)

$leaked = @()
foreach ($pat in $secretPatterns) {
    if ($scriptText -match $pat) {
        # Allow $env:MQK_OPERATOR_TOKEN in assignment/check context, not in Write-Host
        # More precisely: flag only if the match is on a Write-Host / Out-Host line
        # We do a line-by-line check for the noisy patterns
        foreach ($line in ($scriptText -split "`n")) {
            if ($line -match $pat -and $line -match 'Write-Host|echo|Out-Host') {
                $leaked += "$pat in: $($line.Trim())"
            }
        }
    }
}

if ($leaked.Count -eq 0) {
    Pass 'OPR05' 'No secret env values printed via Write-Host / Out-Host'
} else {
    foreach ($l in $leaked) { Fail 'OPR05' "Potential secret print: $l" }
}

# Also verify the script contains the Assert-NotSecret guard or equivalent redaction comment
if ($scriptText -match 'Assert-NotSecret|SECRET_NAMES|values not printed|value not printed|never print') {
    Pass 'OPR05b' 'Script contains explicit secret-guard pattern'
} else {
    Fail 'OPR05b' 'Script must contain explicit secret-guard pattern (SECRET_NAMES / "not printed")'
}

# ---------------------------------------------------------------------------
# OPR06: Script includes adopt baseline and durable arm verification
# ---------------------------------------------------------------------------
if ($scriptText -match 'adopt-broker-position-baseline' -and $scriptText -match 'ADOPT_BROKER_POSITION_BASELINE') {
    Pass 'OPR06' 'Script calls adopt-broker-position-baseline with confirm string'
} else {
    Fail 'OPR06' 'Script must call POST adopt-broker-position-baseline with confirm=ADOPT_BROKER_POSITION_BASELINE'
}

if ($scriptText -match 'arm-execution' -and ($scriptText -match 'arm_state.*armed|armed.*arm_state|ARMED')) {
    Pass 'OPR06b' 'Script calls arm-execution and verifies ARMED state'
} else {
    Fail 'OPR06b' 'Script must call arm-execution and verify ARMED state'
}

# ---------------------------------------------------------------------------
# OPR07: Script includes watcher fields
# ---------------------------------------------------------------------------
$requiredWatcherFields = @(
    'runtime_status',
    'alpaca_ws_continuity',
    'db_status',
    'reconcile_status',
    'deadman_status',
    'arm_state',
    'session_window_state',
    'bar_context_bars_loaded',
    'last_bar_signal_qty',
    'alerts/active',
    'execution/orders',
    'live_routing_enabled'
)

foreach ($field in $requiredWatcherFields) {
    if ($scriptText -match [regex]::Escape($field)) {
        Pass 'OPR07' "Watcher reads field: $field"
    } else {
        Fail 'OPR07' "Watcher missing field: $field"
    }
}

# ---------------------------------------------------------------------------
# OPR08: Script has paper-only defaults
# ---------------------------------------------------------------------------
$paperChecks = @(
    @{ Pattern = "PaperDbUrl.*5440";                    Desc = "PaperDbUrl default uses port 5440" },
    @{ Pattern = "'paper'";                             Desc = "deployment mode default 'paper'" },
    @{ Pattern = "'alpaca'";                            Desc = "adapter id default 'alpaca'" },
    @{ Pattern = "daemon_mode.*paper|paper.*daemon_mode"; Desc = "Guard checks daemon_mode=paper" },
    @{ Pattern = "DaemonPort.*8899|8899.*DaemonPort";   Desc = "Default DaemonPort=8899" }
)

foreach ($c in $paperChecks) {
    if ($scriptText -match $c.Pattern) {
        Pass 'OPR08' $c.Desc
    } else {
        Fail 'OPR08' "Missing: $($c.Desc)  (pattern: $($c.Pattern))"
    }
}

# ---------------------------------------------------------------------------
# OPR09: Script parses cleanly under the Windows PowerShell 5.1 AST parser
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    $ps09Tokens = $null
    $ps09Errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile(
        $TargetScript,
        [ref]$ps09Tokens,
        [ref]$ps09Errors
    ) | Out-Null
    if ($ps09Errors.Count -eq 0) {
        Pass 'OPR09' ("PS5.1 AST parser: PARSE_OK (" + $ps09Tokens.Count + " tokens)")
    } else {
        foreach ($e in $ps09Errors) {
            Fail 'OPR09' ("Parse error at line " + $e.Extent.StartLineNumber + ": " + $e.Message)
        }
    }
} else {
    Fail 'OPR09' 'Cannot run parse check - script file not found (see OPR01)'
}

# ---------------------------------------------------------------------------
# OPR10: Script is ASCII-safe -- no non-ASCII bytes beyond the UTF-8 BOM
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    $opr10Bytes = [System.IO.File]::ReadAllBytes($TargetScript)
    $opr10Offset = 0
    if ($opr10Bytes.Length -ge 3 -and
        $opr10Bytes[0] -eq 0xEF -and
        $opr10Bytes[1] -eq 0xBB -and
        $opr10Bytes[2] -eq 0xBF) {
        $opr10Offset = 3
    }
    $opr10High = @()
    for ($i = $opr10Offset; $i -lt $opr10Bytes.Length; $i++) {
        if ($opr10Bytes[$i] -gt 0x7F) { $opr10High += $i }
    }
    if ($opr10High.Count -eq 0) {
        Pass 'OPR10' ("Script is ASCII-safe (no non-ASCII bytes after BOM; BOM offset=" + $opr10Offset + ")")
    } else {
        # Limit preview to available elements to avoid StrictMode array-bounds throw.
        $opr10Preview = if ($opr10High.Count -gt 5) { $opr10High[0..4] } else { $opr10High[0..($opr10High.Count - 1)] }
        Fail 'OPR10' ("Found " + $opr10High.Count + " non-ASCII byte(s) at positions: " + ($opr10Preview -join ', '))
    }
} else {
    Fail 'OPR10' 'Cannot run ASCII check - script file not found (see OPR01)'
}

# ---------------------------------------------------------------------------
# OPR11: Script contains no known mojibake or unsafe Unicode punctuation
# ---------------------------------------------------------------------------
$opr11Chars = @(
    [char]0x00E2,  # circumflex-a (UTF-8 mojibake prefix e.g. a^M--)
    [char]0x2014,  # em dash
    [char]0x2013,  # en dash
    [char]0x2192,  # rightward arrow
    [char]0x2713,  # check mark
    [char]0x201C,  # left double quotation mark
    [char]0x201D,  # right double quotation mark
    [char]0x2018,  # left single quotation mark
    [char]0x2019   # right single quotation mark
)
$opr11Found = @()
foreach ($ch in $opr11Chars) {
    if ($scriptText.Contains([string]$ch)) {
        $opr11Found += ('U+' + ([int]$ch).ToString('X4'))
    }
}
if ($opr11Found.Count -eq 0) {
    Pass 'OPR11' 'No mojibake or unsafe Unicode punctuation in script'
} else {
    Fail 'OPR11' ("Unsafe Unicode chars found: " + ($opr11Found -join ', '))
}

# ---------------------------------------------------------------------------
# OPR12: Script declares -CheckOnly parameter
# ---------------------------------------------------------------------------
if ($scriptText -match '\[switch\]\$CheckOnly') {
    Pass 'OPR12' 'Script declares -CheckOnly switch parameter'
} else {
    Fail 'OPR12' 'Script must declare -CheckOnly switch parameter'
}

# ---------------------------------------------------------------------------
# OPR13: STEP 5B uses robust psql count extraction (not bare [int](...2>&1).Trim())
# Requires -A (unaligned) flag or Get-PaperMdBarSummary helper to avoid multiline failures.
# ---------------------------------------------------------------------------
$hasRobustCount = (
    ($scriptText -match 'Get-PaperMdBarSummary') -or
    ($scriptText -match 'psql.*-A.*-q|-A.*-t.*psql|psql.*-t.*-A')
)
if ($hasRobustCount) {
    Pass 'OPR13' 'STEP 5B uses robust psql count extraction (Get-PaperMdBarSummary or -A -q flags)'
} else {
    Fail 'OPR13' 'STEP 5B must use Get-PaperMdBarSummary or psql with -A -q flags to avoid multiline parse failures'
}

# ---------------------------------------------------------------------------
# OPR14: STEP 5B gates on minimum completed bars threshold (>= 5)
# ---------------------------------------------------------------------------
if ($scriptText -match 'mdFinalRows.*-lt.*5|-lt.*5.*mdFinalRows|Insufficient bars') {
    Pass 'OPR14' 'STEP 5B gates on minimum completed bars threshold'
} else {
    Fail 'OPR14' 'STEP 5B must gate on minimum completed bars (need >= 5)'
}

# ---------------------------------------------------------------------------
# OPR15: Script gates on deployment_start_allowed=false before start-system
# ---------------------------------------------------------------------------
if ($scriptText -match 'deployment_start_allowed.*false|deployment_start_allowed.*eq.*false') {
    Pass 'OPR15' 'Script hard-gates on deployment_start_allowed=false'
} else {
    Fail 'OPR15' 'Script must hard-stop if deployment_start_allowed=false (STEP 14)'
}

# ---------------------------------------------------------------------------
# OPR16: Script contains Write-EvidenceCapture or equivalent evidence pattern on failure
# ---------------------------------------------------------------------------
if ($scriptText -match 'Write-EvidenceCapture|evidence.folder|evidence_capture|EvidenceCapture') {
    Pass 'OPR16' 'Script contains Write-EvidenceCapture evidence capture on failure'
} else {
    Fail 'OPR16' 'Script must include Write-EvidenceCapture or equivalent evidence capture on failure paths'
}

# ---------------------------------------------------------------------------
# OPR17: CheckOnly mode covers STEP 5B bar count dry-check
# ---------------------------------------------------------------------------
if ($scriptText -match 'STEP 5B dry-check|5B.*dry.check|dry.check.*5B|Get-PaperMdBarSummary') {
    Pass 'OPR17' 'CheckOnly mode covers STEP 5B bar count dry-check'
} else {
    Fail 'OPR17' 'CheckOnly mode must include STEP 5B bar count dry-check (read-only query)'
}

# ---------------------------------------------------------------------------
# OPR18: STEP 10 reads kill_switch_active from fresh daemon state
# The check must include kill_switch_active in the halt-detection condition so
# a daemon that restarted with arm_state=disarmed but kill_switch_active=true
# still triggers the clear path.
# ---------------------------------------------------------------------------
if ($scriptText -match 'kill_switch_active' -and $scriptText -match 'STEP 10|Clear halted run') {
    Pass 'OPR18' 'STEP 10 reads kill_switch_active from fresh daemon state'
} else {
    Fail 'OPR18' 'STEP 10 must read and check kill_switch_active from system/status on the fresh daemon'
}

# ---------------------------------------------------------------------------
# OPR19: Script verifies kill_switch_active=false before proceeding to runtime
# Must appear after the arm call (STEP 13) to catch residual kill_switch state.
# ---------------------------------------------------------------------------
$step10KsIdx  = $scriptText.IndexOf('kill_switch_active')
$step13ArmIdx = $scriptText.IndexOf('arm-execution')
$step13KsIdx  = $scriptText.IndexOf('kill_switch_active', $step13ArmIdx)

if ($step13ArmIdx -ge 0 -and $step13KsIdx -gt $step13ArmIdx) {
    Pass 'OPR19' 'Script verifies kill_switch_active=false after arm-execution (STEP 13)'
} else {
    Fail 'OPR19' 'Script must verify kill_switch_active=false after arm-execution (STEP 13)'
}

# ---------------------------------------------------------------------------
# OPR20: Post-arm verification checks live_routing_enabled=false
# Prevents the smoke from proceeding if live routing was accidentally enabled
# between STEP 8 (initial check) and STEP 13 (post-arm).
# ---------------------------------------------------------------------------
$armIdx    = $scriptText.IndexOf('arm-execution')
$lrArmIdx  = $scriptText.IndexOf('live_routing_enabled', $armIdx)
if ($armIdx -ge 0 -and $lrArmIdx -gt $armIdx) {
    Pass 'OPR20' 'Post-arm verification checks live_routing_enabled=false'
} else {
    Fail 'OPR20' 'Script must verify live_routing_enabled=false in post-arm verification block (STEP 13)'
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL STATIC INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
