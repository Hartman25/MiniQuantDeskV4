# =============================================================================
# Invoke-CanonicalSafeIgnoredMatrix.ps1
#
# FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 1: the one
# canonical authority for "the safe ignored matrix" -- replaces the raw
# monolithic `cargo test --workspace -- --include-ignored --test-threads=1`,
# which is not a valid definition of the repo's safe matrix because it also
# executes 9 MANUAL_EXTERNAL tests that require real Alpaca credentials and
# are expected to fail without them.
#
# What this script proves, in order:
#   1. Every #[ignore]d test in the actually-compiled workspace test harness
#      (built with the same feature set this script itself uses) appears in
#      the tracked inventory (scripts/test/ignored_test_inventory.csv).
#      A newly added ignored test that is missing from the inventory is a
#      hard failure -- it must be classified before it can run here.
#   2. Every SAFE_LOCAL and SAFE_DB_5434 test executes and passes.
#      MANUAL_EXTERNAL tests are gated behind mqk-daemon's `manual-external`
#      Cargo feature (off in this invocation), so they do not even exist in
#      the compiled binary here -- there is no name-based exclusion list to
#      keep in sync, and no way to "accidentally" include them.
#   3. The 9 MANUAL_EXTERNAL tests still compile when `--features
#      manual-external` is explicitly requested (`--no-run`, never executed)
#      -- proving their source has not silently bit-rotted even though they
#      never run in the default or safe-ignored paths.
#
# Usage:
#   $env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5434/mqk_test"
#   pwsh -File scripts\windows\Invoke-CanonicalSafeIgnoredMatrix.ps1
#
# Exit codes: 0 = clean (inventory complete, safe tests green, manual-external
# compiles), 1 = any of the three proofs above failed.
# =============================================================================

param(
    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path,
    [string]$CargoManifest = (Join-Path $RepoRoot "core-rs\Cargo.toml")
)

$ErrorActionPreference = "Stop"

function Resolve-CargoExe {
    $cmd = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cmd) { throw "cargo not found on PATH." }
    return $cmd.Source
}

$CargoExe = Resolve-CargoExe
$InventoryCsv = Join-Path $RepoRoot "scripts\test\ignored_test_inventory.csv"

if (-not (Test-Path -LiteralPath $InventoryCsv)) {
    throw "Canonical ignored-test inventory not found: $InventoryCsv"
}

if ([string]::IsNullOrWhiteSpace($env:MQK_DATABASE_URL) -or $env:MQK_DATABASE_URL -notmatch ':5434') {
    throw "MQK_DATABASE_URL must be set to the port-5434 local test database (e.g. " + `
        "postgres://postgres:postgres@127.0.0.1:5434/mqk_test) before running the safe-ignored matrix. " + `
        "Ports 5432/5440 are never permitted here."
}

Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "Canonical safe-ignored matrix" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan

# ---------------------------------------------------------------------------
# Step 1: inventory completeness -- every #[ignore]d test in the compiled
# harness must be present in the tracked CSV, or this is a hard failure.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Step 1: inventory completeness --" -ForegroundColor Yellow

$inventory = Import-Csv -Path $InventoryCsv
$inventoryByName = @{}
foreach ($row in $inventory) {
    $inventoryByName[$row.function] = $row.classification
}

$listArgs = @(
    "test", "--manifest-path", $CargoManifest, "--workspace",
    "--features", "mqk-db/testkit", "--all-targets",
    "--", "--ignored", "--list"
)
$listOutput = & $CargoExe @listArgs 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Host ($listOutput -join "`n")
    throw "cargo test --ignored --list failed (exit $LASTEXITCODE) -- cannot verify inventory completeness."
}

$liveBareNames = New-Object System.Collections.Generic.List[string]
foreach ($line in $listOutput) {
    if ($line -match '^(?<name>[A-Za-z0-9_:]+): test$') {
        $qualified = $matches['name']
        # src-based #[cfg(test)] tests list as module::path::tests::fn_name;
        # tests/*.rs integration tests list bare. The inventory keys on the
        # bare function name in both cases.
        $bare = ($qualified -split '::')[-1]
        $liveBareNames.Add($bare)
    }
}

$missing = $liveBareNames | Sort-Object -Unique | Where-Object { -not $inventoryByName.ContainsKey($_) }
if ($missing.Count -gt 0) {
    Write-Host "FAIL: ignored test(s) exist in the compiled harness but are not in the canonical inventory:" -ForegroundColor Red
    $missing | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
    Write-Host "Classify each in $InventoryCsv (crate,file,line,function,classification,ignore_reason) before re-running." -ForegroundColor Red
    throw "Unclassified ignored test(s) found -- inventory is out of date."
}

Write-Host ("OK: all {0} live ignored tests are present in the canonical inventory." -f $liveBareNames.Count) -ForegroundColor Green

# ---------------------------------------------------------------------------
# Step 2: execute every SAFE_LOCAL + SAFE_DB_5434 test. MANUAL_EXTERNAL tests
# are absent from this build (manual-external feature not requested), so
# there is nothing to exclude by name.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Step 2: executing SAFE_LOCAL + SAFE_DB_5434 --" -ForegroundColor Yellow

$safeArgs = @(
    "test", "--manifest-path", $CargoManifest, "--workspace",
    "--features", "mqk-db/testkit", "--all-targets", "--no-fail-fast",
    "--", "--ignored", "--test-threads=1"
)
& $CargoExe @safeArgs
$safeExit = $LASTEXITCODE

# ---------------------------------------------------------------------------
# Step 3: MANUAL_EXTERNAL compile-only proof -- never executed.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Step 3: MANUAL_EXTERNAL compile-only proof (--no-run) --" -ForegroundColor Yellow

$manualArgs = @(
    "test", "--manifest-path", $CargoManifest, "--workspace",
    "--features", "mqk-db/testkit,mqk-daemon/manual-external", "--all-targets", "--no-run"
)
& $CargoExe @manualArgs
$manualCompileExit = $LASTEXITCODE

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
$safeLocalCount = @($inventory | Where-Object { $_.classification -eq "SAFE_LOCAL" }).Count
$safeDbCount = @($inventory | Where-Object { $_.classification -eq "SAFE_DB_5434" }).Count
$manualExternalCount = @($inventory | Where-Object { $_.classification -eq "MANUAL_EXTERNAL" }).Count
$blockedCount = @($inventory | Where-Object { $_.classification -eq "BLOCKED_LOCAL_PREREQUISITE" }).Count

Write-Host ""
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "Canonical safe-ignored matrix summary" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "SAFE_LOCAL classified:               $safeLocalCount"
Write-Host "SAFE_DB_5434 classified:              $safeDbCount"
Write-Host "MANUAL_EXTERNAL classified:           $manualExternalCount (excluded from execution; compile-proof exit=$manualCompileExit)"
Write-Host "BLOCKED_LOCAL_PREREQUISITE classified: $blockedCount"
Write-Host "Total inventory:                      $($inventory.Count)"
Write-Host "Safe execution exit code:             $safeExit"
Write-Host "Manual-external compile exit code:    $manualCompileExit"

if ($safeExit -ne 0) {
    throw "Safe-ignored matrix reported failures (see cargo test output above)."
}
if ($manualCompileExit -ne 0) {
    throw "MANUAL_EXTERNAL compile-only proof failed (--features mqk-db/testkit,mqk-daemon/manual-external --no-run)."
}

Write-Host ""
Write-Host ("PASSED: canonical safe-ignored matrix is green; inventory complete ({0} tests); " -f $inventory.Count) -NoNewline -ForegroundColor Green
Write-Host ("{0} MANUAL_EXTERNAL tests excluded from execution and compile-proven." -f $manualExternalCount) -ForegroundColor Green
exit 0
