# =============================================================================
# MULTI-SYMBOL-SMOKE-RUNNER-PREFLIGHT-GATE-01 -- Script static-invariant tests
#
# Reads Start-PaperTradingSmoke.ps1 as text and asserts MSG-01..MSG-14 for the
# STEP 9B multi-symbol smoke preflight gate (watchlist-v2).
# No daemon, no DB, no live calls. Pure text invariant checks.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_multi_symbol_smoke_runner_gate.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
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
Write-Host "=== Multi-symbol smoke preflight gate (STEP 9B) static invariant tests ==="
Write-Host "    Target: $TargetScript"
Write-Host ""

# ---------------------------------------------------------------------------
# MSG-01: Script exists at expected path
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    Pass 'MSG-01' "Script exists at scripts\windows\Start-PaperTradingSmoke.ps1"
} else {
    Fail 'MSG-01' "Script NOT found at: $TargetScript"
}

$scriptText = ''
if (Test-Path $TargetScript) {
    $scriptText = Get-Content -Path $TargetScript -Raw
}

# ---------------------------------------------------------------------------
# MSG-02: Script contains the STEP 9B section header
# ---------------------------------------------------------------------------
if ($scriptText -match [regex]::Escape('STEP 9B: Multi-symbol smoke preflight gate (watchlist-v2)')) {
    Pass 'MSG-02' 'STEP 9B section header present'
} else {
    Fail 'MSG-02' 'STEP 9B section header missing'
}

# ---------------------------------------------------------------------------
# MSG-03: Script calls GET /api/v1/watchlist/status (read-only) via Invoke-DaemonGet
# ---------------------------------------------------------------------------
if ($scriptText -match [regex]::Escape("Invoke-DaemonGet -Path '/api/v1/watchlist/status'")) {
    Pass 'MSG-03' 'Script calls Invoke-DaemonGet -Path /api/v1/watchlist/status'
} else {
    Fail 'MSG-03' 'Script must call Invoke-DaemonGet -Path /api/v1/watchlist/status'
}

# ---------------------------------------------------------------------------
# MSG-04: STEP 9B is positioned after STEP 9 and before STEP 10
# ---------------------------------------------------------------------------
$idxStep9  = $scriptText.IndexOf('# STEP 9: Verify Alpaca WS live')
$idxStep9b = $scriptText.IndexOf('# STEP 9B: Multi-symbol smoke preflight gate')
$idxStep10 = $scriptText.IndexOf('# STEP 10: Clear halted lifecycle if needed')

if ($idxStep9 -ge 0 -and $idxStep9b -gt $idxStep9 -and $idxStep10 -gt $idxStep9b) {
    Pass 'MSG-04' 'STEP 9B is positioned after STEP 9 (WS continuity) and before STEP 10 (first mutating step)'
} else {
    Fail 'MSG-04' "STEP 9B ordering invalid (idxStep9=$idxStep9 idxStep9b=$idxStep9b idxStep10=$idxStep10)"
}

# Isolate the STEP 9B block text for the remaining checks.
$step9bBlock = [regex]::Match($scriptText, '(?s)# STEP 9B: Multi-symbol smoke preflight gate.*?(?=# STEP 10: Clear halted lifecycle if needed)')
$step9bText  = if ($step9bBlock.Success) { $step9bBlock.Value } else { '' }

if (-not $step9bBlock.Success) {
    Fail 'MSG-04b' 'Could not isolate STEP 9B block (between STEP 9B and STEP 10 headers)'
}

# ---------------------------------------------------------------------------
# MSG-05: Gate checks schema_version against 'watchlist-v2' (null-safe -ne)
# ---------------------------------------------------------------------------
if ($step9bText -match [regex]::Escape("`$wlSchemaVersion -ne 'watchlist-v2'")) {
    Pass 'MSG-05' "Gate checks schema_version -ne 'watchlist-v2' (null-safe)"
} else {
    Fail 'MSG-05' "Gate must check schema_version -ne 'watchlist-v2'"
}

# ---------------------------------------------------------------------------
# MSG-06: Gate computes symbol count via @($watchlistStatus.symbols).Count and
# checks it against <= 1 (PS5.1 scalar/array safety)
# ---------------------------------------------------------------------------
if ($step9bText -match [regex]::Escape('@($wlSymbols).Count') -and $step9bText -match [regex]::Escape('$wlSymbolCount -le 1')) {
    Pass 'MSG-06' 'Gate computes symbol count via @(...).Count and gates on <= 1'
} else {
    Fail 'MSG-06' 'Gate must compute symbol count via @(...).Count and gate on count <= 1'
}

# ---------------------------------------------------------------------------
# MSG-07: Gate checks approved_for_autonomous_paper == true (fails closed if not)
# ---------------------------------------------------------------------------
if ($step9bText -match [regex]::Escape('approved_for_autonomous_paper -eq $true') -and $step9bText -match [regex]::Escape('-not $wlApprovedPaper')) {
    Pass 'MSG-07' 'Gate checks approved_for_autonomous_paper == true and fails closed otherwise'
} else {
    Fail 'MSG-07' 'Gate must check approved_for_autonomous_paper == true and fail closed otherwise'
}

# ---------------------------------------------------------------------------
# MSG-08: Gate checks approved_for_live == true and exits 1 (hard invariant)
# ---------------------------------------------------------------------------
if ($step9bText -match [regex]::Escape('approved_for_live -eq $true') -and $step9bText -match [regex]::Escape('if ($wlApprovedLive)')) {
    Pass 'MSG-08' 'Gate checks approved_for_live == true and fails closed if true'
} else {
    Fail 'MSG-08' 'Gate must check approved_for_live == true and fail closed if true'
}

# ---------------------------------------------------------------------------
# MSG-09: All five stable MULTI_SYMBOL_SMOKE_BLOCKED_* codes present verbatim
# ---------------------------------------------------------------------------
$requiredBlockerCodes = @(
    'MULTI_SYMBOL_SMOKE_BLOCKED_WATCHLIST_STATUS_UNAVAILABLE',
    'MULTI_SYMBOL_SMOKE_BLOCKED_SCHEMA_NOT_V2',
    'MULTI_SYMBOL_SMOKE_BLOCKED_NOT_MULTI_SYMBOL',
    'MULTI_SYMBOL_SMOKE_BLOCKED_NOT_APPROVED_FOR_AUTONOMOUS_PAPER',
    'MULTI_SYMBOL_SMOKE_BLOCKED_APPROVED_FOR_LIVE_TRUE'
)
foreach ($code in $requiredBlockerCodes) {
    if ($scriptText -match [regex]::Escape($code)) {
        Pass 'MSG-09' "Blocker code present: $code"
    } else {
        Fail 'MSG-09' "Blocker code missing: $code"
    }
}

# ---------------------------------------------------------------------------
# MSG-10 / MSG-11: STEP 9B fails closed -- every refusal path calls
# Write-EvidenceCapture before exit 1 (evidence-on-refusal pairing).
# ---------------------------------------------------------------------------
$step9bExecText = (($step9bText -split "`n") | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
$msg10Exits     = [regex]::Matches($step9bExecText, 'exit 1\b').Count
$msg11Evidence  = [regex]::Matches($step9bExecText, 'Write-EvidenceCapture').Count

if ($msg10Exits -ge 5) {
    Pass 'MSG-10' "STEP 9B contains >= 5 fail-closed exit 1 paths (found $msg10Exits)"
} else {
    Fail 'MSG-10' "STEP 9B must contain >= 5 fail-closed exit 1 paths (found $msg10Exits)"
}

if ($msg11Evidence -ge 5 -and $msg11Evidence -eq $msg10Exits) {
    Pass 'MSG-11' "STEP 9B calls Write-EvidenceCapture before every refusal exit ($msg11Evidence capture(s) / $msg10Exits exit(s))"
} else {
    Fail 'MSG-11' "STEP 9B evidence-capture/exit pairing mismatch (captures=$msg11Evidence, exits=$msg10Exits -- expected >=5 and equal)"
}

# ---------------------------------------------------------------------------
# MSG-12: STEP 9B is read-only -- no mutating POST calls
# ---------------------------------------------------------------------------
if ($step9bText -notmatch 'Invoke-DaemonPost') {
    Pass 'MSG-12' 'STEP 9B contains no Invoke-DaemonPost calls (read-only gate)'
} else {
    Fail 'MSG-12' 'STEP 9B must not call Invoke-DaemonPost -- gate is read-only'
}

# ---------------------------------------------------------------------------
# MSG-13: CheckOnly mode contains a static self-check for the STEP 9B gate code
# ---------------------------------------------------------------------------
$idxCheckOnlyDryCheck = $scriptText.IndexOf('STEP 9B dry-check')
$idxCheckOnlyComplete = $scriptText.IndexOf('CHECK-ONLY complete')

if ($idxCheckOnlyDryCheck -ge 0 -and $idxCheckOnlyComplete -gt $idxCheckOnlyDryCheck -and
    $scriptText -match [regex]::Escape('Runtime validation (GET /api/v1/watchlist/status) happens during the full smoke run')) {
    Pass 'MSG-13' 'CheckOnly mode contains STEP 9B static self-check (no daemon required)'
} else {
    Fail 'MSG-13' 'CheckOnly mode must contain a STEP 9B static self-check before CHECK-ONLY complete'
}

# ---------------------------------------------------------------------------
# MSG-14: STEP 9B contains no broker trading-endpoint patterns
# (defense-in-depth duplicate of OPR28, scoped to the new block)
# ---------------------------------------------------------------------------
$tradingEndpoints = @(
    '/api/v1/orders/submit',
    '/api/v1/orders/cancel',
    '/api/v1/orders/replace',
    '/api/v1/positions/flatten',
    'submit.*order',
    'cancel.*order'
)
$msg14Found = @()
foreach ($tp in $tradingEndpoints) {
    if ($step9bText -match $tp) { $msg14Found += $tp }
}
if ($msg14Found.Count -eq 0) {
    Pass 'MSG-14' 'STEP 9B contains no broker trading-endpoint patterns'
} else {
    Fail 'MSG-14' "STEP 9B must not contain broker trading-endpoint patterns. Found: $($msg14Found -join ', ')"
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL MULTI-SYMBOL SMOKE RUNNER GATE INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
