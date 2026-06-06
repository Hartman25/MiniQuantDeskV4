# =============================================================================
# Script guard: PAPER-AUTONOMOUS-COMPLETION-BUNDLE-01
#
# Verifies that the autonomous paper status endpoint implementation in
# core-rs/crates/mqk-daemon/src/routes/autonomous_paper_status.rs:
#
#  G01  - module file exists at expected path
#  G02  - response type has live_routing_enabled field in api_types.rs
#  G03  - response type has reconcile_status field in api_types.rs
#  G04  - response type has ws_continuity field in api_types.rs
#  G05  - response type has flatten_available and flatten_blockers fields
#  G06  - response type has next_operator_action field
#  G07  - route is registered in routes.rs at /api/v1/autonomous/paper-status
#  G08  - no /v2/orders (Alpaca broker endpoint) in module
#  G09  - no submit_order in module
#  G10  - no live_routing_enabled = true in module
#  G11  - no DB mutation calls (insert/update/delete) in module
#  G12  - no outbox_enqueue (order submission) in module
#  G13  - response type has readiness_classification field
#  G14  - response type has autonomous_session_state field
#  G15  - test file exists with expected test names
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$RepoRoot = Resolve-Path (Join-Path $PSScriptRoot '..\..')
$ModulePath   = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\routes\autonomous_paper_status.rs'
$ApiTypesPath = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\api_types.rs'
$RoutesPath   = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\routes.rs'
$TestPath     = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\tests\scenario_autonomous_paper_status_summary_01.rs'

$Failures = [System.Collections.Generic.List[string]]::new()

function Assert-Contains {
    param([string]$Content, [string]$Pattern, [string]$Label, [string]$File)
    if ($Content -notmatch [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: '$Pattern' not found in $File")
    }
}

function Assert-NotContains {
    param([string]$Content, [string]$Pattern, [string]$Label, [string]$File)
    if ($Content -match [regex]::Escape($Pattern)) {
        $script:Failures.Add("FAIL [$Label]: forbidden '$Pattern' found in $File")
    }
}

# ---------------------------------------------------------------------------
# G01: module file exists
# ---------------------------------------------------------------------------
if (-not (Test-Path $ModulePath)) {
    Write-Host "FAIL [G01]: autonomous_paper_status.rs not found at: $ModulePath" -ForegroundColor Red
    exit 1
}
Write-Host "PASS [G01]: autonomous_paper_status.rs exists"

$ModuleContent  = Get-Content -Path $ModulePath  -Raw -Encoding UTF8
$ApiTypesContent = Get-Content -Path $ApiTypesPath -Raw -Encoding UTF8
$RoutesContent  = Get-Content -Path $RoutesPath  -Raw -Encoding UTF8

# ---------------------------------------------------------------------------
# G02: live_routing_enabled in AutonomousPaperStatusResponse
# ---------------------------------------------------------------------------
Assert-Contains -Content $ApiTypesContent -Pattern 'pub live_routing_enabled: bool' `
    -Label 'G02' -File 'api_types.rs'
if ($Failures.Count -eq 0) { Write-Host "PASS [G02]: live_routing_enabled field present" }

# ---------------------------------------------------------------------------
# G03: reconcile_status in AutonomousPaperStatusResponse
# ---------------------------------------------------------------------------
Assert-Contains -Content $ApiTypesContent -Pattern 'pub reconcile_status: String' `
    -Label 'G03' -File 'api_types.rs'
if ($Failures.Count -eq 0) { Write-Host "PASS [G03]: reconcile_status field present" }

# ---------------------------------------------------------------------------
# G04: ws_continuity in AutonomousPaperStatusResponse
# ---------------------------------------------------------------------------
Assert-Contains -Content $ApiTypesContent -Pattern 'pub ws_continuity: String' `
    -Label 'G04' -File 'api_types.rs'
if ($Failures.Count -eq 0) { Write-Host "PASS [G04]: ws_continuity field present" }

# ---------------------------------------------------------------------------
# G05: flatten_available and flatten_blockers in AutonomousPaperStatusResponse
# ---------------------------------------------------------------------------
Assert-Contains -Content $ApiTypesContent -Pattern 'pub flatten_available: bool' `
    -Label 'G05a' -File 'api_types.rs'
Assert-Contains -Content $ApiTypesContent -Pattern 'pub flatten_blockers: Vec<String>' `
    -Label 'G05b' -File 'api_types.rs'
if ($Failures.Count -eq 0) { Write-Host "PASS [G05]: flatten_available + flatten_blockers present" }

# ---------------------------------------------------------------------------
# G06: next_operator_action in AutonomousPaperStatusResponse
# ---------------------------------------------------------------------------
Assert-Contains -Content $ApiTypesContent -Pattern 'pub next_operator_action: String' `
    -Label 'G06' -File 'api_types.rs'
if ($Failures.Count -eq 0) { Write-Host "PASS [G06]: next_operator_action field present" }

# ---------------------------------------------------------------------------
# G07: route registered in routes.rs
# ---------------------------------------------------------------------------
Assert-Contains -Content $RoutesContent -Pattern '/api/v1/autonomous/paper-status' `
    -Label 'G07' -File 'routes.rs'
Assert-Contains -Content $RoutesContent -Pattern 'autonomous_paper_status' `
    -Label 'G07b' -File 'routes.rs'
if ($Failures.Count -eq 0) { Write-Host "PASS [G07]: route registered in routes.rs" }

# ---------------------------------------------------------------------------
# G08: no Alpaca broker endpoint reference
# ---------------------------------------------------------------------------
Assert-NotContains -Content $ModuleContent -Pattern '/v2/orders' `
    -Label 'G08' -File 'autonomous_paper_status.rs'
Write-Host "PASS [G08]: no /v2/orders in module"

# ---------------------------------------------------------------------------
# G09: no submit_order call
# ---------------------------------------------------------------------------
Assert-NotContains -Content $ModuleContent -Pattern 'submit_order' `
    -Label 'G09' -File 'autonomous_paper_status.rs'
Write-Host "PASS [G09]: no submit_order in module"

# ---------------------------------------------------------------------------
# G10: no live_routing_enabled = true
# ---------------------------------------------------------------------------
Assert-NotContains -Content $ModuleContent -Pattern 'live_routing_enabled = true' `
    -Label 'G10' -File 'autonomous_paper_status.rs'
Write-Host "PASS [G10]: no live_routing_enabled=true in module"

# ---------------------------------------------------------------------------
# G11: no DB mutation calls
# ---------------------------------------------------------------------------
$MutationPatterns = @('outbox_enqueue', 'insert_fill', 'update_arm', 'delete_run', 'execute(')
foreach ($pattern in $MutationPatterns) {
    Assert-NotContains -Content $ModuleContent -Pattern $pattern `
        -Label 'G11' -File 'autonomous_paper_status.rs'
}
Write-Host "PASS [G11]: no DB mutation patterns in module"

# ---------------------------------------------------------------------------
# G12: no outbox_enqueue (order submission)
# ---------------------------------------------------------------------------
Assert-NotContains -Content $ModuleContent -Pattern 'outbox_enqueue' `
    -Label 'G12' -File 'autonomous_paper_status.rs'
Write-Host "PASS [G12]: no outbox_enqueue in module"

# ---------------------------------------------------------------------------
# G13: readiness_classification in AutonomousPaperStatusResponse
# ---------------------------------------------------------------------------
Assert-Contains -Content $ApiTypesContent -Pattern 'pub readiness_classification: String' `
    -Label 'G13' -File 'api_types.rs'
if ($Failures.Count -eq 0) { Write-Host "PASS [G13]: readiness_classification field present" }

# ---------------------------------------------------------------------------
# G14: autonomous_session_state in AutonomousPaperStatusResponse
# ---------------------------------------------------------------------------
Assert-Contains -Content $ApiTypesContent -Pattern 'pub autonomous_session_state: String' `
    -Label 'G14' -File 'api_types.rs'
if ($Failures.Count -eq 0) { Write-Host "PASS [G14]: autonomous_session_state field present" }

# ---------------------------------------------------------------------------
# G15: test file exists with expected scenario names
# ---------------------------------------------------------------------------
if (-not (Test-Path $TestPath)) {
    $Failures.Add("FAIL [G15]: test file not found at: $TestPath")
} else {
    $TestContent = Get-Content -Path $TestPath -Raw -Encoding UTF8
    $RequiredTests = @('ps01_paper_alpaca_returns_200_active', 'ps02_live_routing_enabled_always_false',
                       'ps04_reconcile_dirty_creates_blocker', 'ps05_kill_switch_active_surfaces_in_response',
                       'ps06_ws_not_live_creates_blocker', 'ps07_flatten_unavailable_without_active_run',
                       'ps11_no_broker_imports_in_paper_status_module')
    foreach ($t in $RequiredTests) {
        Assert-Contains -Content $TestContent -Pattern $t -Label 'G15' -File 'test file'
    }
    Write-Host "PASS [G15]: test file exists with required test names"
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
if ($Failures.Count -gt 0) {
    Write-Host ''
    Write-Host '=== GUARD FAILURES ===' -ForegroundColor Red
    foreach ($f in $Failures) {
        Write-Host $f -ForegroundColor Red
    }
    exit 1
}

Write-Host ''
Write-Host 'All autonomous_paper_status guards pass.' -ForegroundColor Green
exit 0
