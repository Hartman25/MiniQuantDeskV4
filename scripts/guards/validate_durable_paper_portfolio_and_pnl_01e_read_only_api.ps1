# =============================================================================
# DURABLE-PAPER-PORTFOLIO-AND-PNL-01E -- Source-aware static validator
# validate_durable_paper_portfolio_and_pnl_01e_read_only_api.ps1
#
# Scope: core-rs/crates/mqk-daemon/src/routes/durable_portfolio.rs,
# core-rs/crates/mqk-daemon/src/routes/paper_lifecycle.rs,
# core-rs/crates/mqk-daemon/src/routes.rs, core-rs/crates/mqk-daemon/src/api_types.rs,
# core-rs/crates/mqk-daemon/tests/scenario_durable_paper_portfolio_read_only_api_01.rs,
# docs/specs/durable_paper_portfolio_and_pnl_01e_read_only_api.md.
# Pure text/source validation -- no cargo build/test/compile, no DB
# connection. Running the actual test binary against the isolated port-5434
# test DB is a separate, required step of the B4-E mission, not performed by
# this guard.
#
# Checks:
#   [1]  All required files exist.
#   [2]  The three new routes are GET-only in routes.rs (no POST/PUT/PATCH/
#        DELETE registered for any durable-* portfolio path).
#   [3]  durable_portfolio.rs contains no write helper call
#        (insert_or_confirm_paper_portfolio_snapshot / upsert_paper_portfolio_accounting_state).
#   [4]  paper_lifecycle.rs reads durable truth but never calls a write
#        helper either.
#   [5]  classify_overall_lifecycle_state's three-state durable-accounting
#        vocabulary is present as real code.
#   [6]  The test file exists and contains all required proof scenarios,
#        including the read-only zero-delta proof.
#   [7]  No migration file appears in this patch's working-tree diff (B4-E
#        is API-layer only).
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_durable_paper_portfolio_and_pnl_01e_read_only_api.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$DurablePortfolioFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\routes\durable_portfolio.rs'
$PaperLifecycleFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\routes\paper_lifecycle.rs'
$RoutesFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\routes.rs'
$ApiTypesFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\api_types.rs'
$TestFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\tests\scenario_durable_paper_portfolio_read_only_api_01.rs'
$SpecDoc = Join-Path $RepoRoot 'docs\specs\durable_paper_portfolio_and_pnl_01e_read_only_api.md'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan  }

Write-Host '============================================================'
Write-Host ' DURABLE-PAPER-PORTFOLIO-AND-PNL-01E validate_..._01e_read_only_api'
Write-Host '============================================================'

$requiredFiles = @($DurablePortfolioFile, $PaperLifecycleFile, $RoutesFile, $ApiTypesFile, $TestFile, $SpecDoc)
foreach ($f in $requiredFiles) {
    if (-not (Test-Path -LiteralPath $f)) {
        Show-Fail "Required file not found: $f"
    } else {
        Show-Green "Found: $f"
    }
}
if ($Violations -gt 0) {
    Write-Host ''
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
}

$routesText = [System.IO.File]::ReadAllText($RoutesFile)
$durablePortfolioText = [System.IO.File]::ReadAllText($DurablePortfolioFile)
$paperLifecycleText = [System.IO.File]::ReadAllText($PaperLifecycleFile)
$testText = [System.IO.File]::ReadAllText($TestFile)

# [2] GET-only registration for the three new routes
$durablePaths = @('durable-summary', 'durable-positions', 'durable-snapshots')
foreach ($path in $durablePaths) {
    if ($routesText -match "`"/api/v1/portfolio/$path`",\s*\r?\n?\s*get\(") {
        Show-Green "Route /api/v1/portfolio/$path is registered via get()"
    } else {
        Show-Fail "Route /api/v1/portfolio/$path not found registered via get() in routes.rs"
    }
    if ($routesText -match "`"/api/v1/portfolio/$path`"[\s\S]{0,80}(post\(|put\(|patch\(|delete\()") {
        Show-Fail "Route /api/v1/portfolio/$path appears registered with a mutation method"
    }
}

# [3] No write helper calls in durable_portfolio.rs
if ($durablePortfolioText -match 'insert_or_confirm_paper_portfolio_snapshot|upsert_paper_portfolio_accounting_state') {
    Show-Fail "durable_portfolio.rs calls a write helper -- this must be a read-only route file"
} else {
    Show-Green "durable_portfolio.rs contains no write-helper calls"
}

# [4] paper_lifecycle.rs reads durable truth, never writes
if ($paperLifecycleText -match 'fetch_latest_paper_portfolio_snapshot' -and $paperLifecycleText -match 'fetch_paper_portfolio_accounting_state') {
    Show-Green "paper_lifecycle.rs reads both durable snapshot and accounting state"
} else {
    Show-Fail "paper_lifecycle.rs does not read both durable snapshot and accounting state"
}
if ($paperLifecycleText -match 'insert_or_confirm_paper_portfolio_snapshot|upsert_paper_portfolio_accounting_state|accept_external_broker_snapshot|refresh_paper_portfolio_accounting_state') {
    Show-Fail "paper_lifecycle.rs appears to trigger a snapshot persist or accounting replay -- must be read-only"
} else {
    Show-Green "paper_lifecycle.rs never triggers a snapshot persist or accounting replay"
}

# [5] Three-state durable-accounting vocabulary
$requiredStates = @(
    'order_filled_pnl_pending',
    'order_filled_portfolio_durable_pnl_available',
    'order_filled_portfolio_durable_pnl_incomplete'
)
foreach ($s in $requiredStates) {
    if ($paperLifecycleText -match [regex]::Escape($s)) {
        Show-Green "Found required lifecycle state: $s"
    } else {
        Show-Fail "Missing required lifecycle state: $s"
    }
}

# [6] Required test scenarios
$requiredTests = @(
    'no_db_returns_db_unavailable_for_all_durable_routes',
    'mutation_methods_are_rejected',
    'durable_summary_no_snapshot_is_snapshot_unavailable',
    'durable_summary_with_complete_accounting_populates_realized_pnl',
    'durable_summary_incomplete_epoch_blocks_realized_pnl_but_shows_position',
    'durable_positions_returns_positions_from_latest_snapshot',
    'durable_positions_flags_stale_snapshot',
    'durable_snapshots_list_respects_limit_and_orders_newest_first',
    'paper_lifecycle_reports_durable_pnl_available_once_accounting_is_complete',
    'paper_lifecycle_reports_durable_pnl_incomplete',
    'repeated_gets_across_all_durable_routes_never_mutate_any_row'
)
foreach ($t in $requiredTests) {
    if ($testText -match "fn $t\(") {
        Show-Green "Found required test: $t"
    } else {
        Show-Fail "Missing required test: $t"
    }
}

# [7] No new migration in this patch
Push-Location $RepoRoot
try {
    $touched = git diff --name-only HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $touched) {
        $newMigrations = $touched | Where-Object { $_ -like 'core-rs/crates/mqk-db/migrations/*' }
        if ($newMigrations) {
            Show-Fail "New migration file(s) in this patch's diff: $($newMigrations -join ', ') -- B4-E is API-layer only"
        } else {
            Show-Green "No new migration file in this patch's diff"
        }
    } else {
        Show-Info "No working-tree diff against HEAD -- scope check skipped (patch already committed)"
    }
} finally {
    Pop-Location
}

Write-Host ''
if ($Violations -gt 0) {
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
} else {
    Write-Host " VALIDATION PASSED -- 0 violations" -ForegroundColor Green
    exit 0
}
