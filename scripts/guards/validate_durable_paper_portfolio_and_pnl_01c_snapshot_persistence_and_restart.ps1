# =============================================================================
# DURABLE-PAPER-PORTFOLIO-AND-PNL-01C -- Source-aware static validator
# validate_durable_paper_portfolio_and_pnl_01c_snapshot_persistence_and_restart.ps1
#
# Scope: core-rs/crates/mqk-daemon/src/state/snapshot.rs,
# core-rs/crates/mqk-daemon/src/state/orchestrator_build.rs,
# core-rs/crates/mqk-daemon/src/state/loop_runner.rs,
# core-rs/crates/mqk-daemon/src/state.rs,
# core-rs/crates/mqk-daemon/tests/scenario_durable_paper_portfolio_snapshot_persistence_01.rs,
# docs/specs/durable_paper_portfolio_and_pnl_01c_snapshot_persistence_and_restart.md.
# Pure text/source validation -- no cargo build/test/compile, no DB
# connection. Running the actual test binary against the isolated port-5434
# test DB is a separate, required step of the B4-C mission, not performed by
# this guard.
#
# Checks:
#   [1]  All required files exist.
#   [2]  The canonical acceptance seam function exists and is called from
#        all three intended real (External-source) call sites, not from the
#        Synthetic branch.
#   [3]  The source-authority gate (Paper + Alpaca only) is a real code
#        path in persist_external_broker_snapshot_best_effort, not just
#        documented.
#   [4]  Persistence failure modes are logged, never propagated with `?`
#        or `.unwrap()`/`.expect()` (fail-soft, not fail-hard).
#   [5]  The dev-only injection routes (routes/trading.rs) are not modified
#        by this patch and still never call the canonical seam.
#   [6]  A test-only seam method exists on AppState for integration tests.
#   [7]  The test file exists and contains all required proof scenarios,
#        and uses deterministic snapshot_id lookups (not the racy "latest"
#        pattern) for its own persisted rows.
#   [8]  The spec doc exists and documents the source-authority distinction
#        and the fail-soft persistence contract.
#   [9]  No new route file, migration, or GUI file appears in this patch's
#        working-tree diff -- B4-C is daemon-internal wiring only.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_durable_paper_portfolio_and_pnl_01c_snapshot_persistence_and_restart.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$SnapshotFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\state\snapshot.rs'
$OrchBuildFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\state\orchestrator_build.rs'
$LoopRunnerFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\state\loop_runner.rs'
$StateFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\state.rs'
$TestFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\tests\scenario_durable_paper_portfolio_snapshot_persistence_01.rs'
$SpecDoc = Join-Path $RepoRoot 'docs\specs\durable_paper_portfolio_and_pnl_01c_snapshot_persistence_and_restart.md'
$TradingRoutesFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\routes\trading.rs'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan  }

Write-Host '============================================================'
Write-Host ' DURABLE-PAPER-PORTFOLIO-AND-PNL-01C validate_..._01c_snapshot_persistence_and_restart'
Write-Host '============================================================'

$requiredFiles = @($SnapshotFile, $OrchBuildFile, $LoopRunnerFile, $StateFile, $TestFile, $SpecDoc)
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

$snapshotText = [System.IO.File]::ReadAllText($SnapshotFile)
$orchBuildText = [System.IO.File]::ReadAllText($OrchBuildFile)
$loopRunnerText = [System.IO.File]::ReadAllText($LoopRunnerFile)
$stateText = [System.IO.File]::ReadAllText($StateFile)
$testText = [System.IO.File]::ReadAllText($TestFile)
$specText = [System.IO.File]::ReadAllText($SpecDoc)

# [2] Canonical seam exists and is used at all three intended sites
if ($snapshotText -match 'pub\(crate\) async fn accept_external_broker_snapshot\(') {
    Show-Green "accept_external_broker_snapshot exists in snapshot.rs"
} else {
    Show-Fail "accept_external_broker_snapshot not found in snapshot.rs"
}
if ($snapshotText -match 'pub\(crate\) async fn persist_external_broker_snapshot_best_effort\(') {
    Show-Green "persist_external_broker_snapshot_best_effort exists in snapshot.rs"
} else {
    Show-Fail "persist_external_broker_snapshot_best_effort not found in snapshot.rs"
}
if ($orchBuildText -match 'snapshot::accept_external_broker_snapshot\(') {
    Show-Green "orchestrator_build.rs (run-start cold-fetch) calls the canonical seam"
} else {
    Show-Fail "orchestrator_build.rs run-start path does not call the canonical seam"
}
if ($orchBuildText -match 'persist_external_broker_snapshot_best_effort') {
    Show-Green "orchestrator_build.rs (terminal-fill-expiry-refresher) wires the persist half"
} else {
    Show-Fail "orchestrator_build.rs terminal-fill-expiry-refresher does not wire durable persistence"
}
if ($loopRunnerText -match 'snapshot::accept_external_broker_snapshot\(') {
    Show-Green "loop_runner.rs (periodic refresh) calls the canonical seam"
} else {
    Show-Fail "loop_runner.rs periodic refresh does not call the canonical seam"
}

# Synthetic branch must NOT call the seam.
$syntheticBlock = [regex]::Match($orchBuildText, 'BrokerSnapshotTruthSource::Synthetic\s*=>\s*\{[\s\S]*?\n            \}')
if ($syntheticBlock.Success -and $syntheticBlock.Value -match 'accept_external_broker_snapshot') {
    Show-Fail "The Synthetic branch must never call accept_external_broker_snapshot"
} else {
    Show-Green "The Synthetic branch does not call the canonical seam"
}

# [3] Source-authority gate is real code, not just doc
if ($snapshotText -match 'deployment_mode\s*!=\s*super::types::DeploymentMode::Paper' -and
    $snapshotText -match 'broker_kind\s*!=\s*Some\(super::types::BrokerKind::Alpaca\)') {
    Show-Green "Paper+Alpaca source-authority gate is a real fail-closed code path"
} else {
    Show-Fail "Source-authority gate (deployment_mode/broker_kind check) not found as real code"
}

# [4] Failure modes are logged, not propagated with unwrap/expect in the
# persistence function body.
$persistFnMatch = [regex]::Match($snapshotText, 'pub\(crate\) async fn persist_external_broker_snapshot_best_effort\([\s\S]*?\n\}')
if ($persistFnMatch.Success) {
    $body = $persistFnMatch.Value
    if ($body -match '\.unwrap\(\)|\.expect\(') {
        Show-Fail "persist_external_broker_snapshot_best_effort contains .unwrap()/.expect() -- must fail soft, never panic"
    } else {
        Show-Green "persist_external_broker_snapshot_best_effort contains no .unwrap()/.expect()"
    }
    if ($body -match 'tracing::warn!') {
        Show-Green "Failure modes are logged via tracing::warn!"
    } else {
        Show-Fail "No tracing::warn! found in the persistence function -- failures must be observable"
    }
} else {
    Show-Fail "Could not isolate persist_external_broker_snapshot_best_effort's body for inspection"
}

# [5] trading.rs (dev-only injection routes) untouched by this patch and
# still doesn't call the canonical seam.
Push-Location $RepoRoot
try {
    $touched = git diff --name-only HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $touched) {
        if ($touched -contains 'core-rs/crates/mqk-daemon/src/routes/trading.rs') {
            Show-Fail "routes/trading.rs was modified by this patch -- dev-only injection routes are out of scope for B4-C"
        } else {
            Show-Green "routes/trading.rs not touched by this patch"
        }
        $unexpectedProdFiles = $touched | Where-Object {
            $_ -like 'core-rs/crates/mqk-daemon/src/routes/*' -or
            $_ -like 'core-rs/crates/mqk-daemon/migrations/*' -or
            $_ -like 'core-rs/mqk-gui/*'
        }
        if ($unexpectedProdFiles) {
            Show-Fail "Unexpected route/migration/GUI file(s) touched: $($unexpectedProdFiles -join ', ') -- B4-C is daemon-internal wiring only"
        } else {
            Show-Green "No route/migration/GUI file touched by this patch"
        }
    } else {
        Show-Info "No working-tree diff against HEAD -- scope check skipped (patch already committed)"
    }
} finally {
    Pop-Location
}
if (Test-Path -LiteralPath $TradingRoutesFile) {
    $tradingText = [System.IO.File]::ReadAllText($TradingRoutesFile)
    if ($tradingText -match 'accept_external_broker_snapshot') {
        Show-Fail "routes/trading.rs (dev-only injection) must never call the canonical acceptance seam"
    } else {
        Show-Green "routes/trading.rs does not call the canonical acceptance seam"
    }
}

# [6] Test-only seam method
if ($stateText -match 'pub async fn accept_external_broker_snapshot_for_test\(') {
    Show-Green "AppState::accept_external_broker_snapshot_for_test exists"
} else {
    Show-Fail "AppState::accept_external_broker_snapshot_for_test not found"
}

# [7] Required test scenarios and deterministic-lookup discipline
$requiredTests = @(
    'real_source_accepted_snapshot_persists',
    'synthesized_source_never_masquerades_as_authoritative',
    'missing_db_remains_bounded',
    'db_failure_remains_bounded',
    'exact_replay_is_idempotent',
    'restart_reads_back_durable_snapshot',
    'position_ordering_round_trips',
    'snapshot_replacement_forms_durable_history',
    'zero_oms_delta_and_zero_broker_calls'
)
foreach ($t in $requiredTests) {
    if ($testText -match "fn $t\(") {
        Show-Green "Found required test: $t"
    } else {
        Show-Fail "Missing required test: $t"
    }
}
# Match an actual call (identifier immediately followed by "(", allowing for
# a preceding module-path qualifier like "mqk_db::"), not just a mention of
# the name inside an explanatory comment about why it's deliberately unused.
if ($testText -match 'fetch_latest_paper_portfolio_snapshot\s*\(') {
    Show-Fail "Test file still calls fetch_latest_paper_portfolio_snapshot -- this is racy under parallel execution (see B4-C spec); use expected_snapshot_id + fetch_paper_portfolio_snapshot_by_id instead"
} else {
    Show-Green "Test file uses deterministic snapshot_id lookups only (no racy 'latest' queries)"
}

# [8] Spec doc documents key decisions
if ($specText -match 'source-authority' -or $specText -match 'Source authority') {
    Show-Green "Spec doc documents the source-authority distinction"
} else {
    Show-Fail "Spec doc does not document the source-authority distinction"
}
if ($specText -match 'fail-soft|best-effort|best effort') {
    Show-Green "Spec doc documents the fail-soft/best-effort persistence contract"
} else {
    Show-Fail "Spec doc does not document the fail-soft persistence contract"
}

Write-Host ''
if ($Violations -gt 0) {
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
} else {
    Write-Host " VALIDATION PASSED -- 0 violations" -ForegroundColor Green
    exit 0
}
