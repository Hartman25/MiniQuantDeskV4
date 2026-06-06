# =============================================================================
# Script guard: PAPER-SMOKE-AUTOMATION-BUNDLE-01
#
# Verifies that the smoke workflow scripts correctly integrate the
# autonomous paper status summary endpoint (/api/v1/autonomous/paper-status).
#
# SA01  Capture script references /api/v1/autonomous/paper-status
# SA02  Capture script writes autonomous_paper_status.json
# SA03  Smoke runner (Start-PaperTradingSmoke) references readiness_classification
# SA04  Smoke runner references next_operator_action
# SA05  Smoke runner handles blocked readiness_classification
# SA06  Smoke runner handles market_proof_pending readiness_classification
# SA07  Review script reads autonomous_paper_status.json
# SA08  Review script writes computed_delta_qty
# SA09  Review script writes flatten_available
# SA10  No raw Alpaca /v2/orders in any of the four scripts
# SA11  No submit_order in any of the four scripts
# SA12  No live_routing_enabled=true assignment in any of the four scripts
# SA13  No DB mutation keywords (INSERT/UPDATE/DELETE) in automation scripts
# SA14  No secret printing in patched scripts
# SA15  No signal injection endpoint in automation scripts
# SA-PARSE  All four scripts parse without PowerShell errors
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_paper_smoke_automation_status.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot      = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path.TrimEnd('\')
$CaptureScript = Join-Path $RepoRoot 'scripts\windows\Capture-PaperSmokeEvidence.ps1'
$SmokeScript   = Join-Path $RepoRoot 'scripts\windows\Start-PaperTradingSmoke.ps1'
$RunnerScript  = Join-Path $RepoRoot 'scripts\windows\Run-AAPL5mMarketSmoke.ps1'
$ReviewScript  = Join-Path $RepoRoot 'scripts\windows\Review-PaperSmokeEvidence.ps1'

$TotalFailures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg)
    Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red
    $script:TotalFailures++
}

function Assert-Contains {
    param([string]$Content, [string]$Pattern, [string]$Id, [string]$File)
    if ($Content -notmatch [regex]::Escape($Pattern)) {
        Fail $Id "'$Pattern' not found in $File"
        return $false
    }
    return $true
}

function Assert-NotContainsNonComment {
    param([string]$Content, [string]$Pattern, [string]$Id, [string]$File)
    $lines = $Content -split "`n"
    $nonCommentText = ($lines | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"
    if ($nonCommentText -match [regex]::Escape($Pattern)) {
        Fail $Id "Forbidden pattern '$Pattern' found in non-comment line of $File"
        return $false
    }
    return $true
}

Write-Host ""
Write-Host "=== PAPER-SMOKE-AUTOMATION-BUNDLE-01 script guards (SA01-SA15 + SA-PARSE) ==="
Write-Host "    RepoRoot: $RepoRoot"
Write-Host ""

# ---------------------------------------------------------------------------
# Verify all four scripts exist
# ---------------------------------------------------------------------------
$missing = 0
foreach ($pair in @(
    @{ Path = $CaptureScript; Label = 'Capture-PaperSmokeEvidence.ps1' },
    @{ Path = $SmokeScript;   Label = 'Start-PaperTradingSmoke.ps1'    },
    @{ Path = $RunnerScript;  Label = 'Run-AAPL5mMarketSmoke.ps1'      },
    @{ Path = $ReviewScript;  Label = 'Review-PaperSmokeEvidence.ps1'  }
)) {
    if (-not (Test-Path $pair.Path)) {
        Fail 'SA-FILES' "Required file not found: $($pair.Label)"
        $missing++
    }
}
if ($missing -gt 0) {
    Write-Host ""
    Write-Host "=== GUARD FAILURES (missing files) ===" -ForegroundColor Red
    exit 1
}

$captureContent = Get-Content -Path $CaptureScript -Raw -Encoding UTF8
$smokeContent   = Get-Content -Path $SmokeScript   -Raw -Encoding UTF8
$runnerContent  = Get-Content -Path $RunnerScript  -Raw -Encoding UTF8
$reviewContent  = Get-Content -Path $ReviewScript  -Raw -Encoding UTF8

# ---------------------------------------------------------------------------
# SA01: Capture script references /api/v1/autonomous/paper-status
# ---------------------------------------------------------------------------
Write-Host "-- SA01: capture script references /api/v1/autonomous/paper-status --"
if (Assert-Contains -Content $captureContent -Pattern '/api/v1/autonomous/paper-status' `
        -Id 'SA01' -File 'Capture-PaperSmokeEvidence.ps1') {
    Pass 'SA01' 'Capture script references /api/v1/autonomous/paper-status'
}

# ---------------------------------------------------------------------------
# SA02: Capture script writes autonomous_paper_status.json
# ---------------------------------------------------------------------------
Write-Host "-- SA02: capture script writes autonomous_paper_status.json --"
if (Assert-Contains -Content $captureContent -Pattern 'autonomous_paper_status.json' `
        -Id 'SA02' -File 'Capture-PaperSmokeEvidence.ps1') {
    Pass 'SA02' 'Capture script references autonomous_paper_status.json'
}

# ---------------------------------------------------------------------------
# SA03: Smoke runners reference readiness_classification
# ---------------------------------------------------------------------------
Write-Host "-- SA03: smoke runners reference readiness_classification --"
$sa03ok = $true
if (-not (Assert-Contains -Content $smokeContent -Pattern 'readiness_classification' `
        -Id 'SA03' -File 'Start-PaperTradingSmoke.ps1')) { $sa03ok = $false }
if (-not (Assert-Contains -Content $runnerContent -Pattern 'readiness_classification' `
        -Id 'SA03' -File 'Run-AAPL5mMarketSmoke.ps1')) { $sa03ok = $false }
if ($sa03ok) { Pass 'SA03' 'Smoke runners reference readiness_classification' }

# ---------------------------------------------------------------------------
# SA04: Smoke runners reference next_operator_action
# ---------------------------------------------------------------------------
Write-Host "-- SA04: smoke runners reference next_operator_action --"
$sa04ok = $true
if (-not (Assert-Contains -Content $smokeContent -Pattern 'next_operator_action' `
        -Id 'SA04' -File 'Start-PaperTradingSmoke.ps1')) { $sa04ok = $false }
if (-not (Assert-Contains -Content $runnerContent -Pattern 'next_operator_action' `
        -Id 'SA04' -File 'Run-AAPL5mMarketSmoke.ps1')) { $sa04ok = $false }
if ($sa04ok) { Pass 'SA04' 'Smoke runners reference next_operator_action' }

# ---------------------------------------------------------------------------
# SA05: Smoke runners handle blocked
# ---------------------------------------------------------------------------
Write-Host "-- SA05: smoke runners handle blocked --"
$sa05ok = $true
if (-not (Assert-Contains -Content $smokeContent -Pattern "'blocked'" `
        -Id 'SA05' -File 'Start-PaperTradingSmoke.ps1')) { $sa05ok = $false }
if (-not (Assert-Contains -Content $runnerContent -Pattern "'blocked'" `
        -Id 'SA05' -File 'Run-AAPL5mMarketSmoke.ps1')) { $sa05ok = $false }
if ($sa05ok) { Pass 'SA05' "Smoke runners handle 'blocked' readiness_classification" }

# ---------------------------------------------------------------------------
# SA06: Smoke runners handle market_proof_pending
# ---------------------------------------------------------------------------
Write-Host "-- SA06: smoke runners handle market_proof_pending --"
$sa06ok = $true
if (-not (Assert-Contains -Content $smokeContent -Pattern 'market_proof_pending' `
        -Id 'SA06' -File 'Start-PaperTradingSmoke.ps1')) { $sa06ok = $false }
if (-not (Assert-Contains -Content $runnerContent -Pattern 'market_proof_pending' `
        -Id 'SA06' -File 'Run-AAPL5mMarketSmoke.ps1')) { $sa06ok = $false }
if ($sa06ok) { Pass 'SA06' 'Smoke runners handle market_proof_pending readiness_classification' }

# ---------------------------------------------------------------------------
# SA07: Review script reads autonomous_paper_status.json
# ---------------------------------------------------------------------------
Write-Host "-- SA07: review script reads autonomous_paper_status.json --"
if (Assert-Contains -Content $reviewContent -Pattern 'autonomous_paper_status.json' `
        -Id 'SA07' -File 'Review-PaperSmokeEvidence.ps1') {
    Pass 'SA07' 'Review script reads autonomous_paper_status.json'
}

# ---------------------------------------------------------------------------
# SA08: Review script writes computed_delta_qty
# ---------------------------------------------------------------------------
Write-Host "-- SA08: review script writes computed_delta_qty --"
if (Assert-Contains -Content $reviewContent -Pattern 'computed_delta_qty' `
        -Id 'SA08' -File 'Review-PaperSmokeEvidence.ps1') {
    Pass 'SA08' 'Review script references computed_delta_qty'
}

# ---------------------------------------------------------------------------
# SA09: Review script writes flatten_available
# ---------------------------------------------------------------------------
Write-Host "-- SA09: review script writes flatten_available --"
if (Assert-Contains -Content $reviewContent -Pattern 'flatten_available' `
        -Id 'SA09' -File 'Review-PaperSmokeEvidence.ps1') {
    Pass 'SA09' 'Review script references flatten_available'
}

# ---------------------------------------------------------------------------
# SA10: No raw Alpaca /v2/orders in any of the four scripts
# ---------------------------------------------------------------------------
Write-Host "-- SA10: no raw Alpaca /v2/orders --"
$sa10ok = $true
foreach ($pair in @(
    @{ Name = 'Capture-PaperSmokeEvidence.ps1'; Content = $captureContent },
    @{ Name = 'Start-PaperTradingSmoke.ps1';    Content = $smokeContent   },
    @{ Name = 'Run-AAPL5mMarketSmoke.ps1';      Content = $runnerContent  },
    @{ Name = 'Review-PaperSmokeEvidence.ps1';  Content = $reviewContent  }
)) {
    if (-not (Assert-NotContainsNonComment -Content $pair.Content -Pattern '/v2/orders' `
            -Id 'SA10' -File $pair.Name)) { $sa10ok = $false }
}
if ($sa10ok) { Pass 'SA10' 'No raw Alpaca /v2/orders in any script' }

# ---------------------------------------------------------------------------
# SA11: No submit_order in any of the four scripts
# ---------------------------------------------------------------------------
Write-Host "-- SA11: no submit_order --"
$sa11ok = $true
foreach ($pair in @(
    @{ Name = 'Capture-PaperSmokeEvidence.ps1'; Content = $captureContent },
    @{ Name = 'Start-PaperTradingSmoke.ps1';    Content = $smokeContent   },
    @{ Name = 'Run-AAPL5mMarketSmoke.ps1';      Content = $runnerContent  },
    @{ Name = 'Review-PaperSmokeEvidence.ps1';  Content = $reviewContent  }
)) {
    if (-not (Assert-NotContainsNonComment -Content $pair.Content -Pattern 'submit_order' `
            -Id 'SA11' -File $pair.Name)) { $sa11ok = $false }
}
if ($sa11ok) { Pass 'SA11' 'No submit_order in any script' }

# ---------------------------------------------------------------------------
# SA12: No script sets MQK_LIVE_ROUTING_ENABLED to a truthy value.
# Scripts legitimately CHECK for this condition; guard only blocks setting it.
# Dangerous patterns: $env:MQK_LIVE_ROUTING_ENABLED = 'true' / '1' / $true
# ---------------------------------------------------------------------------
Write-Host "-- SA12: MQK_LIVE_ROUTING_ENABLED not set to truthy --"
$sa12ok = $true
$liveRoutingSetPatterns = @(
    "MQK_LIVE_ROUTING_ENABLED`".*=.*`"true`"",
    "MQK_LIVE_ROUTING_ENABLED'.*=.*'true'",
    'MQK_LIVE_ROUTING_ENABLED.*=.*"1"',
    "MQK_LIVE_ROUTING_ENABLED.*=.*'1'"
)
foreach ($pair in @(
    @{ Name = 'Capture-PaperSmokeEvidence.ps1'; Content = $captureContent },
    @{ Name = 'Start-PaperTradingSmoke.ps1';    Content = $smokeContent   },
    @{ Name = 'Run-AAPL5mMarketSmoke.ps1';      Content = $runnerContent  },
    @{ Name = 'Review-PaperSmokeEvidence.ps1';  Content = $reviewContent  }
)) {
    $lines = ($pair.Content -split "`n") | Where-Object { $_ -notmatch '^\s*#' }
    $ncText = $lines -join "`n"
    foreach ($pat in $liveRoutingSetPatterns) {
        if ($ncText -match $pat) {
            Fail 'SA12' "Live routing assignment pattern '$pat' found in $($pair.Name)"
            $sa12ok = $false
        }
    }
}
if ($sa12ok) { Pass 'SA12' 'MQK_LIVE_ROUTING_ENABLED not set to truthy in any script' }

# ---------------------------------------------------------------------------
# SA13: No DB mutation keywords in automation scripts (non-comment lines)
# Start-PaperTradingSmoke runs sqlx migrate which is expected -- exclude it.
# ---------------------------------------------------------------------------
Write-Host "-- SA13: no DB mutation --"
$sa13ok = $true
$mutationPatterns = @('INSERT ', 'UPDATE ', 'DELETE FROM', 'DROP TABLE', 'TRUNCATE ')
foreach ($pair in @(
    @{ Name = 'Capture-PaperSmokeEvidence.ps1'; Content = $captureContent },
    @{ Name = 'Run-AAPL5mMarketSmoke.ps1';      Content = $runnerContent  },
    @{ Name = 'Review-PaperSmokeEvidence.ps1';  Content = $reviewContent  }
)) {
    foreach ($pat in $mutationPatterns) {
        if (-not (Assert-NotContainsNonComment -Content $pair.Content -Pattern $pat `
                -Id 'SA13' -File $pair.Name)) { $sa13ok = $false }
    }
}
if ($sa13ok) { Pass 'SA13' 'No DB mutation keywords in automation scripts' }

# ---------------------------------------------------------------------------
# SA14: No secret printing (canonical secret names via Write-Host)
# ---------------------------------------------------------------------------
Write-Host "-- SA14: no secret printing --"
$sa14ok = $true
$secretNames = @('ALPACA_API_KEY_PAPER', 'ALPACA_API_SECRET_PAPER', 'ALPACA_API_KEY_LIVE',
                 'ALPACA_API_SECRET_LIVE', 'MQK_OPERATOR_TOKEN', 'DISCORD_WEBHOOK_URL')
foreach ($secretName in $secretNames) {
    $offendingLines = ($captureContent -split "`n") | Where-Object {
        $_ -match [regex]::Escape($secretName) -and
        $_ -notmatch '^\s*#' -and
        $_ -match '(?i)(Write-Host|Out-File|Set-Content|echo)'
    }
    if ($offendingLines) {
        Fail 'SA14' "Possible secret print '$secretName' in Capture-PaperSmokeEvidence.ps1"
        $sa14ok = $false
    }
}
if ($sa14ok) { Pass 'SA14' 'No secret printing detected' }

# ---------------------------------------------------------------------------
# SA15: No signal injection endpoint in automation scripts
# ---------------------------------------------------------------------------
Write-Host "-- SA15: no signal injection --"
$sa15ok = $true
foreach ($pair in @(
    @{ Name = 'Capture-PaperSmokeEvidence.ps1'; Content = $captureContent },
    @{ Name = 'Run-AAPL5mMarketSmoke.ps1';      Content = $runnerContent  },
    @{ Name = 'Review-PaperSmokeEvidence.ps1';  Content = $reviewContent  }
)) {
    if (-not (Assert-NotContainsNonComment -Content $pair.Content -Pattern '/api/v1/strategy/signal' `
            -Id 'SA15' -File $pair.Name)) { $sa15ok = $false }
}
if ($sa15ok) { Pass 'SA15' 'No signal injection endpoint in automation scripts' }

# ---------------------------------------------------------------------------
# SA-PARSE: All four scripts parse without PowerShell errors
# ---------------------------------------------------------------------------
Write-Host "-- SA-PARSE: PowerShell parse check --"
foreach ($scriptPath in @($CaptureScript, $SmokeScript, $RunnerScript, $ReviewScript)) {
    $parseErrors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile(
        $scriptPath, [ref]$null, [ref]$parseErrors)
    $shortName = Split-Path -Leaf $scriptPath
    if ($parseErrors.Count -gt 0) {
        $errMsgs = ($parseErrors | ForEach-Object { $_.Message }) -join '; '
        Fail 'SA-PARSE' "Parse errors in ${shortName}: $errMsgs"
    } else {
        Pass 'SA-PARSE' "${shortName} parses without errors"
    }
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($TotalFailures -eq 0) {
    Write-Host "=== ALL SA GUARDS PASSED (SA01-SA15 + SA-PARSE) ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $TotalFailures SA GUARD(S) FAILED ===" -ForegroundColor Red
    exit 1
}
