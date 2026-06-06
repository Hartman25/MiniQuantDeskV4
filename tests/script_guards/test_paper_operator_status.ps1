# =============================================================================
# OPERATOR-CONTROL-SURFACE-BUNDLE-01
# Script guard: test_paper_operator_status.ps1
#
# Static assertions against scripts\windows\Get-PaperOperatorStatus.ps1.
# No daemon, no DB, no live calls, no .env.local required.
# All checks are read-only source inspections.
#
# Guard assertions:
#   OPS01  Script calls /api/v1/autonomous/paper-status
#   OPS02  Script calls /api/v1/system/status
#   OPS03  Script calls /api/v1/reconcile/status
#   OPS04  Script calls /api/v1/portfolio/positions or equivalent
#   OPS05  Script calls /api/v1/portfolio/orders/open or equivalent
#   OPS06  Script does NOT call /v2/orders (raw Alpaca broker endpoint)
#   OPS07  Script does NOT submit orders (no POST to orders endpoint)
#   OPS08  Script does NOT mutate DB (no UPDATE/INSERT/DELETE via psql/sqlx)
#   OPS09  Script does NOT print operator token (no Write-Host $operatorToken pattern)
#   OPS10  Script does NOT enable live routing
#   OPS11  Script handles daemon unavailable (fail-soft, not exception-abort)
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path
$Target    = Join-Path $RepoRoot 'scripts\windows\Get-PaperOperatorStatus.ps1'

$Passed = 0
$Failed = 0

function Assert-True {
    param([string]$Id, [string]$Description, [bool]$Condition)
    if ($Condition) {
        Write-Host "  PASS [$Id] $Description" -ForegroundColor Green
        $script:Passed++
    } else {
        Write-Host "  FAIL [$Id] $Description" -ForegroundColor Red
        $script:Failed++
    }
}

Write-Host ""
Write-Host "=== test_paper_operator_status.ps1 (OPERATOR-CONTROL-SURFACE-BUNDLE-01) ===" -ForegroundColor Cyan
Write-Host "    Target: $Target"
Write-Host ""

if (-not (Test-Path $Target)) {
    Write-Host "  FAIL [OPS00] Script not found: $Target" -ForegroundColor Red
    exit 1
}

$Content = Get-Content $Target -Raw

# OPS01: calls autonomous/paper-status
Assert-True 'OPS01' 'Script calls /api/v1/autonomous/paper-status' `
    ($Content -match '/api/v1/autonomous/paper-status')

# OPS02: calls system/status
Assert-True 'OPS02' 'Script calls /api/v1/system/status' `
    ($Content -match '/api/v1/system/status')

# OPS03: calls reconcile/status
Assert-True 'OPS03' 'Script calls /api/v1/reconcile/status' `
    ($Content -match '/api/v1/reconcile/status')

# OPS04: calls portfolio/positions
Assert-True 'OPS04' 'Script calls /api/v1/portfolio/positions' `
    ($Content -match '/api/v1/portfolio/positions')

# OPS05: calls portfolio/orders/open
Assert-True 'OPS05' 'Script calls /api/v1/portfolio/orders/open' `
    ($Content -match '/api/v1/portfolio/orders/open')

# OPS06: does NOT invoke the raw Alpaca /v2/orders endpoint via HTTP
# (a bare mention in a comment is acceptable; we check for Invoke-* call patterns)
Assert-True 'OPS06' 'Script does NOT invoke /v2/orders via Invoke-* (raw Alpaca broker endpoint)' `
    ($Content -notmatch 'Invoke-\w+.*[''"].*\/v2\/orders' -and
     $Content -notmatch 'Invoke-\w+.*Uri.*v2\/orders')

# OPS07: does NOT submit orders (no POST body with order_type/symbol/qty to a broker endpoint)
Assert-True 'OPS07' 'Script does NOT use Method POST with order submission patterns' `
    ($Content -notmatch "Method\s+'?POST'?.*order_type" -and $Content -notmatch 'submit_order')

# OPS08: does NOT mutate DB via psql/sqlx DML
Assert-True 'OPS08' 'Script does NOT mutate DB (no UPDATE/INSERT/DELETE/DROP via psql)' `
    ($Content -notmatch '\bpsql\b.*(UPDATE|INSERT|DELETE|DROP)\b' -and
     $Content -notmatch '\bsqlx\b.*(UPDATE|INSERT|DELETE|DROP)\b')

# OPS09: does NOT print operator token
Assert-True 'OPS09' 'Script does NOT Write-Host the operator token variable' `
    ($Content -notmatch 'Write-Host.*\$operatorToken' -and
     $Content -notmatch 'Write-Host.*MQK_OPERATOR_TOKEN\s*=')

# OPS10: does NOT enable live routing
Assert-True 'OPS10' 'Script does NOT set MQK_LIVE_ROUTING_ENABLED=true' `
    ($Content -notmatch 'MQK_LIVE_ROUTING_ENABLED\s*=\s*true' -and
     $Content -notmatch 'env:MQK_LIVE_ROUTING_ENABLED.*true')

# OPS11: handles daemon unavailable (uses ErrorAction or try/catch, returns null on failure)
Assert-True 'OPS11' 'Script handles daemon unavailable (ErrorAction Stop in try/catch or -ErrorAction)' `
    ($Content -match '-ErrorAction\s+Stop' -or $Content -match 'catch\s*\{' )

# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $Passed" -ForegroundColor Green
Write-Host "  Failed: $Failed" -ForegroundColor $(if ($Failed -gt 0) { 'Red' } else { 'Green' })
Write-Host ""

if ($Failed -gt 0) {
    Write-Host "GUARD FAILED: $Failed assertion(s) failed." -ForegroundColor Red
    exit 1
} else {
    Write-Host "GUARD PASSED: all $Passed assertions passed." -ForegroundColor Green
    exit 0
}
