# =============================================================================
# EVIDENCE-CAPTURE-TRADE-FLOW-01
# test_paper_smoke_evidence_trade_flow.ps1
#
# Static guard: proves that the paper smoke evidence capture and review scripts
# include all trade-flow endpoints and classification logic required to
# correctly classify a natural order/fill lifecycle.
#
# Assertions (TF01-TF14):
#   TF01  Capture script references execution/flow endpoint
#   TF02  Capture script references fill-quality endpoint
#   TF03  Capture script references portfolio/positions or portfolio endpoint
#   TF04  Capture script references oms/overview or execution/outbox
#   TF05  Capture script references reconcile/status and reconcile/mismatches
#   TF06  Review script contains NATURAL-TRADE-LIFECYCLE-CLOSED classification
#   TF07  Review script contains READINESS-CLOSED-NO-TRADE classification
#   TF08  Review script writes trade_lifecycle_detected field
#   TF09  Review script writes fill_detected field
#   TF10  Review script writes reconcile_clean field
#   TF11  Capture script does not print MQK_OPERATOR_TOKEN
#   TF12  Capture script does not contain raw Alpaca order endpoint /v2/orders
#   TF13  Capture script does not contain DB mutation keywords
#   TF14  Capture script does not submit orders (no /ops/action or /run/start calls)
#
# No daemon, no DB, no live calls. Pure static content checks.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_paper_smoke_evidence_trade_flow.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$CaptureScript = Join-Path $RepoRoot 'scripts\windows\Capture-PaperSmokeEvidence.ps1'
$ReviewScript  = Join-Path $RepoRoot 'scripts\windows\Review-PaperSmokeEvidence.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red; $script:Failures++ }

Write-Host ''
Write-Host '=== test_paper_smoke_evidence_trade_flow.ps1 (EVIDENCE-CAPTURE-TRADE-FLOW-01) ==='
Write-Host "    Capture: $CaptureScript"
Write-Host "    Review:  $ReviewScript"
Write-Host ''

# ---------------------------------------------------------------------------
# Load script contents (fail immediately if scripts are missing)
# ---------------------------------------------------------------------------
if (-not (Test-Path $CaptureScript)) {
    Fail 'TF00' "Capture script not found: $CaptureScript"
    Write-Host ''
    Write-Host "  FATAL: Capture script missing -- cannot continue." -ForegroundColor Red
    exit 1
}
if (-not (Test-Path $ReviewScript)) {
    Fail 'TF00' "Review script not found: $ReviewScript"
    Write-Host ''
    Write-Host "  FATAL: Review script missing -- cannot continue." -ForegroundColor Red
    exit 1
}

$CaptureText  = Get-Content $CaptureScript -Raw
$ReviewText   = Get-Content $ReviewScript  -Raw

# ---------------------------------------------------------------------------
# TF01: Capture script references execution/flow endpoint
# ---------------------------------------------------------------------------
Write-Host '  -- TF01: capture has execution/flow endpoint --'
if ($CaptureText -match "execution/flow") {
    Pass 'TF01' "Capture script references execution/flow endpoint"
} else {
    Fail 'TF01' "Capture script does NOT reference execution/flow endpoint"
}

# ---------------------------------------------------------------------------
# TF02: Capture script references fill-quality endpoint
# ---------------------------------------------------------------------------
Write-Host '  -- TF02: capture has fill-quality endpoint --'
if ($CaptureText -match "fill-quality|fill_quality") {
    Pass 'TF02' "Capture script references fill-quality endpoint"
} else {
    Fail 'TF02' "Capture script does NOT reference fill-quality endpoint"
}

# ---------------------------------------------------------------------------
# TF03: Capture script references portfolio/positions endpoint
# ---------------------------------------------------------------------------
Write-Host '  -- TF03: capture has portfolio/positions endpoint --'
if ($CaptureText -match "portfolio/positions|portfolio_positions") {
    Pass 'TF03' "Capture script references portfolio/positions endpoint"
} else {
    Fail 'TF03' "Capture script does NOT reference portfolio/positions endpoint"
}

# ---------------------------------------------------------------------------
# TF04: Capture script references oms/overview or execution/outbox
# (either satisfies the broker order map requirement)
# ---------------------------------------------------------------------------
Write-Host '  -- TF04: capture has oms/overview or execution/outbox --'
if ($CaptureText -match "oms/overview|execution/outbox") {
    Pass 'TF04' "Capture script references oms/overview or execution/outbox"
} else {
    Fail 'TF04' "Capture script does NOT reference oms/overview or execution/outbox"
}

# ---------------------------------------------------------------------------
# TF05: Capture script references both reconcile/status and reconcile/mismatches
# ---------------------------------------------------------------------------
Write-Host '  -- TF05: capture has reconcile/status AND reconcile/mismatches --'
if ($CaptureText -match "reconcile/status") {
    Pass 'TF05a' "Capture script references reconcile/status"
} else {
    Fail 'TF05a' "Capture script does NOT reference reconcile/status"
}
if ($CaptureText -match "reconcile/mismatches") {
    Pass 'TF05b' "Capture script references reconcile/mismatches"
} else {
    Fail 'TF05b' "Capture script does NOT reference reconcile/mismatches"
}

# ---------------------------------------------------------------------------
# TF06: Review script contains NATURAL-TRADE-LIFECYCLE-CLOSED classification
# ---------------------------------------------------------------------------
Write-Host '  -- TF06: review contains NATURAL-TRADE-LIFECYCLE-CLOSED --'
if ($ReviewText -match 'NATURAL-TRADE-LIFECYCLE-CLOSED') {
    Pass 'TF06' "Review script contains NATURAL-TRADE-LIFECYCLE-CLOSED classification"
} else {
    Fail 'TF06' "Review script does NOT contain NATURAL-TRADE-LIFECYCLE-CLOSED classification"
}

# ---------------------------------------------------------------------------
# TF07: Review script contains READINESS-CLOSED-NO-TRADE classification
# ---------------------------------------------------------------------------
Write-Host '  -- TF07: review contains READINESS-CLOSED-NO-TRADE --'
if ($ReviewText -match 'READINESS-CLOSED-NO-TRADE') {
    Pass 'TF07' "Review script contains READINESS-CLOSED-NO-TRADE classification"
} else {
    Fail 'TF07' "Review script does NOT contain READINESS-CLOSED-NO-TRADE classification"
}

# ---------------------------------------------------------------------------
# TF08: Review script writes trade_lifecycle_detected field
# ---------------------------------------------------------------------------
Write-Host '  -- TF08: review writes trade_lifecycle_detected --'
if ($ReviewText -match 'trade_lifecycle_detected') {
    Pass 'TF08' "Review script writes trade_lifecycle_detected field"
} else {
    Fail 'TF08' "Review script does NOT write trade_lifecycle_detected field"
}

# ---------------------------------------------------------------------------
# TF09: Review script writes fill_detected field
# ---------------------------------------------------------------------------
Write-Host '  -- TF09: review writes fill_detected --'
if ($ReviewText -match 'fill_detected') {
    Pass 'TF09' "Review script writes fill_detected field"
} else {
    Fail 'TF09' "Review script does NOT write fill_detected field"
}

# ---------------------------------------------------------------------------
# TF10: Review script writes reconcile_clean field
# ---------------------------------------------------------------------------
Write-Host '  -- TF10: review writes reconcile_clean --'
if ($ReviewText -match 'reconcile_clean') {
    Pass 'TF10' "Review script writes reconcile_clean field"
} else {
    Fail 'TF10' "Review script does NOT write reconcile_clean field"
}

# ---------------------------------------------------------------------------
# TF11: Capture script does not print MQK_OPERATOR_TOKEN
# (reference/check is OK; echoing the value is not)
# ---------------------------------------------------------------------------
Write-Host '  -- TF11: capture does not print MQK_OPERATOR_TOKEN --'
$tokenPrintLines = (Get-Content $CaptureScript) | Where-Object {
    $_ -match 'Write-Host|echo|Out-File|Set-Content' -and
    $_ -match 'MQK_OPERATOR_TOKEN' -and
    $_ -notmatch '^\s*#'
}
if ($tokenPrintLines) {
    Fail 'TF11' "Capture script may print MQK_OPERATOR_TOKEN: $($tokenPrintLines[0])"
} else {
    Pass 'TF11' "Capture script does not print MQK_OPERATOR_TOKEN via output commands"
}

# ---------------------------------------------------------------------------
# TF12: Capture script does not reference raw Alpaca /v2/orders endpoint
# ---------------------------------------------------------------------------
Write-Host '  -- TF12: capture does not reference /v2/orders --'
$alpacaOrderLines = (Get-Content $CaptureScript) | Where-Object {
    $_ -match '/v2/orders' -and $_ -notmatch '^\s*#'
}
if ($alpacaOrderLines) {
    Fail 'TF12' "Capture script references /v2/orders Alpaca endpoint: $($alpacaOrderLines[0])"
} else {
    Pass 'TF12' "Capture script does not reference /v2/orders Alpaca endpoint"
}

# ---------------------------------------------------------------------------
# TF13: Capture script does not contain DB mutation keywords
# ---------------------------------------------------------------------------
Write-Host '  -- TF13: capture has no DB mutation keywords --'
$dbMutationPatterns = @('INSERT ', 'UPDATE ', 'DELETE ', 'DROP ', 'TRUNCATE ', 'CREATE TABLE')
$dbMutationFound = $false
foreach ($pat in $dbMutationPatterns) {
    $offending = (Get-Content $CaptureScript) | Where-Object {
        $_ -match [regex]::Escape($pat) -and $_ -notmatch '^\s*#'
    }
    if ($offending) {
        Fail 'TF13' "Capture script contains DB mutation keyword '$pat'"
        $dbMutationFound = $true
    }
}
if (-not $dbMutationFound) {
    Pass 'TF13' "Capture script contains no DB mutation keywords"
}

# ---------------------------------------------------------------------------
# TF14: Capture script does not submit orders or call mutating action routes
# ---------------------------------------------------------------------------
Write-Host '  -- TF14: capture does not submit orders or call action routes --'
$orderSubmitPatterns = @('/ops/action', '/run/start', '/execution/orders')
$submitFound = $false
foreach ($pat in $orderSubmitPatterns) {
    $offending = (Get-Content $CaptureScript) | Where-Object {
        $_ -match [regex]::Escape($pat) -and
        $_ -notmatch '^\s*#' -and
        $_ -match '(?i)(Invoke-RestMethod|Invoke-WebRequest|Invoke-DaemonGet|curl)'
    }
    if ($offending) {
        Fail 'TF14' "Capture script calls '$pat' via HTTP: $($offending[0])"
        $submitFound = $true
    }
}
if (-not $submitFound) {
    Pass 'TF14' "Capture script does not call order-submit or action routes"
}

# ---------------------------------------------------------------------------
# Parse check: both scripts must parse without errors
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host '  -- TF-PARSE: PowerShell parse checks --'

foreach ($info in @(
    @{ Id = 'TF-PARSE-CAP'; Path = $CaptureScript; Name = 'Capture' },
    @{ Id = 'TF-PARSE-REV'; Path = $ReviewScript;  Name = 'Review'  }
)) {
    $parseErrs = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile(
        $info.Path, [ref]$null, [ref]$parseErrs)
    if ($parseErrs.Count -eq 0) {
        Pass $info.Id "$($info.Name) script parses without errors"
    } else {
        $msgs = ($parseErrs | ForEach-Object { $_.Message }) -join '; '
        Fail $info.Id "$($info.Name) script parse error(s): $msgs"
    }
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ''
if ($Failures -eq 0) {
    Write-Host '=== ALL TF INVARIANTS PASSED (EVIDENCE-CAPTURE-TRADE-FLOW-01) ===' -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
