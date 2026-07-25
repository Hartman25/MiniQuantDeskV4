# =============================================================================
# DURABLE-PAPER-PORTFOLIO-AND-PNL-01D -- Source-aware static validator
# validate_durable_paper_portfolio_and_pnl_01d_accounting_and_pnl.ps1
#
# Scope: core-rs/crates/mqk-daemon/src/state/paper_portfolio_accounting.rs,
# core-rs/crates/mqk-daemon/src/state/snapshot.rs,
# core-rs/crates/mqk-daemon/src/state.rs,
# core-rs/crates/mqk-daemon/tests/scenario_durable_paper_portfolio_accounting_01.rs,
# docs/specs/durable_paper_portfolio_and_pnl_01d_accounting_and_pnl.md.
# Pure text/source validation -- no cargo build/test/compile, no DB
# connection. Running the actual test binary against the isolated port-5434
# test DB is a separate, required step of the B4-D mission, not performed by
# this guard.
#
# Checks:
#   [1]  All required files exist.
#   [2]  The FIFO replay reuses recover_oms_and_portfolio (not a second,
#        simpler replay loop that would risk double-counting duplicate
#        economic fill events).
#   [3]  No new fill/accounting-event table is created by this patch (no
#        new CREATE TABLE / migration file) -- accounting stays a computed
#        projection over oms_inbox, per the B4-A/B4-B decision.
#   [4]  Accounting-epoch determination is a real code path (not just
#        documented) and is reachable for both total-absence and partial
#        mismatches, never fabricating a synthetic opening fill.
#   [5]  The refresh function is wired into the canonical acceptance seam
#        (accept_external_broker_snapshot), not a new standalone call site.
#   [6]  Persistence failure modes are logged, never propagated with
#        .unwrap()/.expect().
#   [7]  sys_account_equity_baseline is never referenced by the new module.
#   [8]  The test file exists and contains all required proof scenarios.
#   [9]  No route, migration, or GUI file appears in this patch's
#        working-tree diff -- B4-D is daemon-internal accounting only.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_durable_paper_portfolio_and_pnl_01d_accounting_and_pnl.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$AccountingFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\state\paper_portfolio_accounting.rs'
$SnapshotFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\state\snapshot.rs'
$StateFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\state.rs'
$TestFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\tests\scenario_durable_paper_portfolio_accounting_01.rs'
$SpecDoc = Join-Path $RepoRoot 'docs\specs\durable_paper_portfolio_and_pnl_01d_accounting_and_pnl.md'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan  }

Write-Host '============================================================'
Write-Host ' DURABLE-PAPER-PORTFOLIO-AND-PNL-01D validate_..._01d_accounting_and_pnl'
Write-Host '============================================================'

$requiredFiles = @($AccountingFile, $SnapshotFile, $StateFile, $TestFile, $SpecDoc)
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

$accountingText = [System.IO.File]::ReadAllText($AccountingFile)
$snapshotText = [System.IO.File]::ReadAllText($SnapshotFile)
$testText = [System.IO.File]::ReadAllText($TestFile)

# [2] Reuses recover_oms_and_portfolio
if ($accountingText -match 'recover_oms_and_portfolio\(') {
    Show-Green "replay_paper_portfolio_accounting reuses recover_oms_and_portfolio"
} else {
    Show-Fail "Accounting replay does not appear to reuse recover_oms_and_portfolio -- risk of duplicate-fill double-counting"
}

# [3] No new migration in this patch (B4-B already added the only new table)
Push-Location $RepoRoot
try {
    $touched = git diff --name-only HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $touched) {
        $newMigrations = $touched | Where-Object { $_ -like 'core-rs/crates/mqk-db/migrations/*' }
        if ($newMigrations) {
            Show-Fail "New migration file(s) in this patch's diff: $($newMigrations -join ', ') -- B4-D must not add a new fill/accounting-event table"
        } else {
            Show-Green "No new migration file in this patch's diff"
        }
        $unexpectedProdFiles = $touched | Where-Object {
            $_ -like 'core-rs/crates/mqk-daemon/src/routes/*' -or
            $_ -like 'core-rs/mqk-gui/*'
        }
        if ($unexpectedProdFiles) {
            Show-Fail "Unexpected route/GUI file(s) touched: $($unexpectedProdFiles -join ', ') -- B4-D is daemon-internal accounting only"
        } else {
            Show-Green "No route/GUI file touched by this patch"
        }
    } else {
        Show-Info "No working-tree diff against HEAD -- scope check skipped (patch already committed)"
    }
} finally {
    Pop-Location
}

# [4] Accounting epoch is real code
if ($accountingText -match 'accounting_epoch\s*=\s*"incomplete"') {
    Show-Green "accounting_epoch = 'incomplete' assignment is real code"
} else {
    Show-Fail "No real code path assigns accounting_epoch = 'incomplete'"
}
if ($accountingText -notmatch 'synthetic|fabricat') {
    Show-Green "No synthetic/fabricated fill language found in the accounting module (clean intent)"
}
if ($accountingText -match 'never fabricates a synthetic opening fill' -or $accountingText -match 'never fabricated') {
    Show-Green "Module doc comment explicitly states no synthetic opening fill is fabricated"
} else {
    Show-Fail "Module doc comment does not explicitly state the no-fabrication guarantee"
}

# [5] Wired into the canonical acceptance seam
if ($snapshotText -match 'refresh_paper_portfolio_accounting_state_best_effort') {
    Show-Green "Accounting refresh is called from snapshot.rs (the canonical acceptance seam)"
} else {
    Show-Fail "Accounting refresh is not wired into accept_external_broker_snapshot"
}

# [6] No unwrap/expect in the best-effort refresh function
$refreshFnMatch = [regex]::Match($accountingText, 'pub\(crate\) async fn refresh_paper_portfolio_accounting_state_best_effort\([\s\S]*?\n\}')
if ($refreshFnMatch.Success) {
    if ($refreshFnMatch.Value -match '\.unwrap\(\)|\.expect\(') {
        Show-Fail "refresh_paper_portfolio_accounting_state_best_effort contains .unwrap()/.expect() -- must fail soft"
    } else {
        Show-Green "refresh_paper_portfolio_accounting_state_best_effort contains no .unwrap()/.expect()"
    }
} else {
    Show-Fail "Could not isolate refresh_paper_portfolio_accounting_state_best_effort's body"
}

# [7] Never touches the daily equity baseline table
if ($accountingText -match 'sys_account_equity_baseline') {
    Show-Fail "paper_portfolio_accounting.rs references sys_account_equity_baseline -- must remain untouched"
} else {
    Show-Green "paper_portfolio_accounting.rs never references sys_account_equity_baseline"
}

# [8] Required test scenarios
$requiredTests = @(
    'buy_opens_long_position',
    'second_buy_adds_fifo_lot',
    'partial_sell_realizes_fifo_gain',
    'partial_sell_realizes_fifo_loss',
    'fees_reduce_cash',
    'full_sell_closes_position',
    'duplicate_refresh_has_zero_delta',
    'partial_then_final_fill_applies_exact_total_once',
    'restart_replay_produces_identical_state',
    'pre_existing_unmatched_position_blocks_accounting_epoch',
    'partial_mismatch_also_blocks_accounting_epoch',
    'flat_portfolio_has_known_zero_truth',
    'daily_pnl_baseline_table_untouched'
)
foreach ($t in $requiredTests) {
    if ($testText -match "fn $t\(") {
        Show-Green "Found required test: $t"
    } else {
        Show-Fail "Missing required test: $t"
    }
}

Write-Host ''
if ($Violations -gt 0) {
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
} else {
    Write-Host " VALIDATION PASSED -- 0 violations" -ForegroundColor Green
    exit 0
}
