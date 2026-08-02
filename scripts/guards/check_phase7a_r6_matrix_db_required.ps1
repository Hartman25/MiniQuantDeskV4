# =============================================================================
# PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 Part 5: dedicated fail-fast
# DB-gated matrix command.
# =============================================================================
#
# Unlike a plain `cargo test -p mqk-daemon --lib`, which lets every DB-gated
# test in `real_production_effects_matrix_tests` silently `eprintln!` a skip
# and report "ok" when no DB is reachable (correct default behavior for a
# general lib-test run that must stay DB-optional), THIS command must never
# green with zero executed tests. It:
#
#   1. Requires MQK_DATABASE_URL to already be set to the port-5434 local
#      test DB before running anything (refuses 5440, refuses unset).
#   2. Sets MQK_REQUIRE_PHASE7A_R6_MATRIX=1, which flips every
#      `db_pool_or_skip` early-return in `real_production_effects_matrix_
#      tests` from a silent skip into a hard `panic!` (see
#      `state/lifecycle.rs`) -- so a DB that becomes unreachable mid-run
#      fails loudly instead of quietly skipping.
#   3. Runs exactly the `real_production_effects_matrix_tests` module,
#      single-threaded (the module's own tests share one reusable run row
#      and serialize via an internal mutex; --test-threads=1 avoids
#      unnecessary contention/flakiness, not a correctness requirement).
#   4. Parses the harness's own "N passed; M failed; ... finished" summary
#      line and asserts M == 0 and N >= the minimum expected count below --
#      never merely "the process exit code was 0", since a filtered-out-
#      everything run can also exit 0 with zero tests executed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_phase7a_r6_matrix_db_required.ps1
#
# Exit codes: 0 = clean (>= MinExpectedTests passed, 0 failed), 1 = failed.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$CoreRs    = Join-Path $RepoRoot "core-rs"

# Bumped as new rows are added to real_production_effects_matrix_tests --
# a lower actual count than this is a real regression (a test vanished,
# didn't compile, or a filter swallowed it), not something to silently pass.
# TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01 Blocker 3: bumped 19 -> 26 for
# the 7 new real DynamicPaperEnforced full-loop proof rows (BLOCKER3-01..07).
$MinExpectedTests = 26

Write-Host "============================================================"
Write-Host " MQK Phase 7A R6 Exhaustive Matrix -- Fail-Fast DB Command"
Write-Host " Repo root: $RepoRoot"
Write-Host " Minimum expected passing tests: $MinExpectedTests"
Write-Host "============================================================"

# ---------------------------------------------------------------------------
# Precondition: MQK_DATABASE_URL must already be explicitly the port-5434
# local test DB. Refuse to silently default or fall back -- never 5440.
# ---------------------------------------------------------------------------
$DbUrl = $env:MQK_DATABASE_URL
if ([string]::IsNullOrWhiteSpace($DbUrl)) {
    Write-Host " FAIL -- MQK_DATABASE_URL is not set. This command requires the" -ForegroundColor Red
    Write-Host "         port-5434 local test DB, e.g.:" -ForegroundColor Red
    Write-Host "         `$env:MQK_DATABASE_URL = 'postgres://postgres:postgres@localhost:5434/mqk_test'" -ForegroundColor Red
    exit 1
}
if ($DbUrl -notmatch ":5434") {
    Write-Host " FAIL -- MQK_DATABASE_URL must be the port-5434 local test DB; refusing: $DbUrl" -ForegroundColor Red
    Write-Host "         Never 5440 (paper) for this command." -ForegroundColor Red
    exit 1
}
Write-Host ""
Write-Host " MQK_DATABASE_URL OK: $DbUrl"

$env:MQK_REQUIRE_PHASE7A_R6_MATRIX = "1"

Write-Host ""
Write-Host "-- Running real_production_effects_matrix_tests (fail-fast) --"
Push-Location $CoreRs
$PriorErrorActionPreference = $ErrorActionPreference
$ErrorActionPreference = "Continue"
try {
    $Output = & cargo test -p mqk-daemon --lib real_production_effects_matrix_tests -- --test-threads=1 2>&1 |
        ForEach-Object { $_.ToString() }
    $ExitCode = $LASTEXITCODE
    $Output | ForEach-Object { Write-Host $_ }
} finally {
    $ErrorActionPreference = $PriorErrorActionPreference
    Pop-Location
    Remove-Item Env:\MQK_REQUIRE_PHASE7A_R6_MATRIX -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------------------
# Parse the harness's own summary line -- never trust exit code alone (a run
# with every test filtered out can still exit 0).
# ---------------------------------------------------------------------------
$SummaryLine = $Output | Where-Object { $_ -match 'test result:\s*(ok|FAILED)\.\s*(\d+)\s*passed;\s*(\d+)\s*failed;\s*(\d+)\s*ignored' } | Select-Object -Last 1

if (-not $SummaryLine) {
    Write-Host ""
    Write-Host " FAIL -- could not find a 'test result: ...' summary line in cargo test output;" -ForegroundColor Red
    Write-Host "         the run may have crashed before producing results (exit code $ExitCode)." -ForegroundColor Red
    exit 1
}

$null = $SummaryLine -match 'test result:\s*(ok|FAILED)\.\s*(\d+)\s*passed;\s*(\d+)\s*failed;\s*(\d+)\s*ignored'
$Passed = [int]$Matches[2]
$Failed = [int]$Matches[3]
$Ignored = [int]$Matches[4]

Write-Host ""
Write-Host "============================================================"
Write-Host " Parsed result: $Passed passed, $Failed failed, $Ignored ignored"

$Failures = 0

if ($Failed -ne 0) {
    Write-Host " FAIL -- $Failed test(s) failed" -ForegroundColor Red
    $Failures++
}
if ($Ignored -ne 0) {
    Write-Host " FAIL -- $Ignored test(s) were ignored; this command must exhaustively run every matrix test" -ForegroundColor Red
    $Failures++
}
if ($Passed -lt $MinExpectedTests) {
    Write-Host " FAIL -- only $Passed test(s) passed; expected at least $MinExpectedTests" -ForegroundColor Red
    $Failures++
}

if ($Failures -eq 0) {
    Write-Host " PHASE 7A R6 MATRIX (DB-REQUIRED): OK -- $Passed tests genuinely executed and passed" -ForegroundColor Green
    exit 0
} else {
    Write-Host " PHASE 7A R6 MATRIX (DB-REQUIRED): $Failures check(s) FAILED" -ForegroundColor Red
    exit 1
}
