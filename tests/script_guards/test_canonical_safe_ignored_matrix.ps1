# =============================================================================
# Mutation-negative tests for scripts/windows/Invoke-CanonicalSafeIgnoredMatrix.ps1
#
# FULL-AUDIT-CHECKPOINT-HARDENING-REPAIR-01 Part 3: proves the exact-identity
# rework of the ignored-test inventory actually catches duplicate exact
# keys, a bare function name colliding across two different targets, missing
# rows, stale rows, and unknown classifications -- and that
# BLOCKED_LOCAL_PREREQUISITE rows are excluded from execution by default.
#
# This dot-sources the real script to reach its pure comparison functions
# (Test-InventorySelfValidation, Get-InventoryCompletenessResult,
# Get-ExactLiveKeysForPackage, Get-BlockedTestRows,
# Get-SkipArgsForBlockedRows) directly against synthetic data -- no real
# cargo invocation, no real inventory CSV, no live workspace build. Dot-
# sourcing does not trigger Main (see the InvocationName guard at the
# bottom of the script under test).
#
# Usage: pwsh -File tests\script_guards\test_canonical_safe_ignored_matrix.ps1
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path
$ScriptUnderTest = Join-Path $RepoRoot 'scripts\windows\Invoke-CanonicalSafeIgnoredMatrix.ps1'

$FAILURES = 0
function Pass([string]$id, [string]$msg) { Write-Host "  PASS  [$id] $msg" -ForegroundColor Green }
function Fail([string]$id, [string]$msg) { Write-Host "  FAIL  [$id] $msg" -ForegroundColor Red; $script:FAILURES++ }

Write-Host ""
Write-Host "=== Invoke-CanonicalSafeIgnoredMatrix.ps1 mutation-negative tests ==="
Write-Host "    Script: $ScriptUnderTest"
Write-Host ""

if (-not (Test-Path -LiteralPath $ScriptUnderTest)) {
    Fail "CSIM00" "Script under test not found at $ScriptUnderTest"
    Write-Host ""
    Write-Host "=== $FAILURES INVARIANT(S) FAILED ==="
    exit 1
}
Pass "CSIM00" "Script under test exists"

# Dot-source: binds default params (harmless path resolution only) and
# defines every function below, but does not invoke Main (InvocationName is
# '.' when dot-sourced).
. $ScriptUnderTest
Pass "CSIM01" "Script dot-sources cleanly without invoking cargo or Main"

function New-Row {
    param($Crate, $Target, $Function, $Classification = "SAFE_DB_5434")
    [pscustomobject]@{
        crate          = $Crate
        target         = $Target
        file           = "crates/$Crate/tests/fake.rs"
        line           = "1"
        function       = $Function
        classification = $Classification
        ignore_reason  = ""
    }
}

function New-LiveRow {
    param($Package, $Target, $Function)
    [pscustomobject]@{
        Package  = $Package
        Target   = $Target
        Function = $Function
        Key      = "{0}|{1}|{2}" -f $Package, $Target, $Function
    }
}

# ---------------------------------------------------------------------------
# CSIM02: baseline -- a clean inventory with no duplicates and known
# classifications produces zero self-validation errors.
# ---------------------------------------------------------------------------
$clean = @(
    (New-Row "mqk-db" "lib" "runtime_lease::tests::acquire_when_no_lease_exists"),
    (New-Row "mqk-daemon" "scenario_x" "foo_test")
)
$errs = Test-InventorySelfValidation -Inventory $clean -KnownClassifications $KnownClassifications
if ($errs.Count -eq 0) {
    Pass "CSIM02" "Clean inventory produces zero self-validation errors"
} else {
    Fail "CSIM02" "Clean inventory unexpectedly produced errors: $($errs -join '; ')"
}

# ---------------------------------------------------------------------------
# CSIM03: duplicate exact key within the CSV must be caught.
# ---------------------------------------------------------------------------
$duplicate = @(
    (New-Row "mqk-daemon" "scenario_x" "foo_test"),
    (New-Row "mqk-daemon" "scenario_x" "foo_test")
)
$errs = Test-InventorySelfValidation -Inventory $duplicate -KnownClassifications $KnownClassifications
if (($errs.Count -eq 1) -and ($errs[0] -match 'duplicate exact key')) {
    Pass "CSIM03" "Duplicate exact key (same crate/target/function) is caught"
} else {
    Fail "CSIM03" "Duplicate exact key was NOT caught as expected (errors: $($errs -join '; '))"
}

# ---------------------------------------------------------------------------
# CSIM04: same bare function name in two different targets must NOT be
# treated as a duplicate -- the exact key (crate|target|function) must keep
# them distinct. This is the exact defect the bare-name-only identity had.
# ---------------------------------------------------------------------------
$sameBareDifferentTarget = @(
    (New-Row "mqk-daemon" "scenario_a" "plans_list_respects_limit_and_run_scoping"),
    (New-Row "mqk-daemon" "scenario_b" "plans_list_respects_limit_and_run_scoping")
)
$errs = Test-InventorySelfValidation -Inventory $sameBareDifferentTarget -KnownClassifications $KnownClassifications
if ($errs.Count -eq 0) {
    Pass "CSIM04" "Same bare function name across two different targets remains distinct (no false duplicate)"
} else {
    Fail "CSIM04" "Same bare function name in two targets was wrongly flagged as duplicate: $($errs -join '; ')"
}

# ---------------------------------------------------------------------------
# CSIM05: unknown classification value must be caught.
# ---------------------------------------------------------------------------
$unknownClass = @(New-Row "mqk-daemon" "scenario_x" "foo_test" "NOT_A_REAL_CLASSIFICATION")
$errs = Test-InventorySelfValidation -Inventory $unknownClass -KnownClassifications $KnownClassifications
if (($errs.Count -eq 1) -and ($errs[0] -match 'unknown classification')) {
    Pass "CSIM05" "Unknown classification value is caught"
} else {
    Fail "CSIM05" "Unknown classification was NOT caught as expected (errors: $($errs -join '; '))"
}

# ---------------------------------------------------------------------------
# CSIM06: a live test with no corresponding inventory row is "missing" --
# and, crucially, if the inventory has a row for the SAME bare name but a
# DIFFERENT target, that must NOT satisfy completeness (proving the exact
# key, not the bare name, drives the missing check).
# ---------------------------------------------------------------------------
$inventoryMissingCase = @(
    (New-Row "mqk-daemon" "scenario_a" "plans_list_respects_limit_and_run_scoping")
)
$liveMissingCase = @(
    (New-LiveRow "mqk-daemon" "scenario_a" "plans_list_respects_limit_and_run_scoping"),
    (New-LiveRow "mqk-daemon" "scenario_b" "plans_list_respects_limit_and_run_scoping")
)
$result = Get-InventoryCompletenessResult -Inventory $inventoryMissingCase -LiveRows $liveMissingCase -Narrowed $false
if (($result.Missing.Count -eq 1) -and ($result.Missing[0].Key -eq "mqk-daemon|scenario_b|plans_list_respects_limit_and_run_scoping")) {
    Pass "CSIM06" "Live test in a second target with the same bare name as a classified test is still reported missing (exact key, not bare name, decides)"
} else {
    Fail "CSIM06" "Missing-row detection did not correctly isolate the unclassified same-bare-name test (missing: $($result.Missing | ForEach-Object { $_.Key }))"
}

# ---------------------------------------------------------------------------
# CSIM07: an inventory row with no corresponding live test is "stale".
# ---------------------------------------------------------------------------
$inventoryStaleCase = @(
    (New-Row "mqk-daemon" "scenario_x" "renamed_or_deleted_test")
)
$liveStaleCase = @()
$result = Get-InventoryCompletenessResult -Inventory $inventoryStaleCase -LiveRows $liveStaleCase -Narrowed $false
if (($result.Stale.Count -eq 1) -and ($result.Missing.Count -eq 0)) {
    Pass "CSIM07" "Inventory row with no live counterpart is reported stale"
} else {
    Fail "CSIM07" "Stale-row detection failed (stale count=$($result.Stale.Count), missing count=$($result.Missing.Count))"
}

# ---------------------------------------------------------------------------
# CSIM08: -Narrowed suppresses stale-row detection (a partial package scan
# cannot honestly judge a row outside its scope as stale).
# ---------------------------------------------------------------------------
$result = Get-InventoryCompletenessResult -Inventory $inventoryStaleCase -LiveRows $liveStaleCase -Narrowed $true
if ($result.Stale.Count -eq 0) {
    Pass "CSIM08" "Narrowed completeness scan suppresses stale-row detection"
} else {
    Fail "CSIM08" "Narrowed completeness scan should not report stale rows, but did: $($result.Stale.Count)"
}

# ---------------------------------------------------------------------------
# CSIM09: Get-ExactLiveKeysForPackage correctly assigns target context
# across multiple 'Running ...' sections in one package's --list output
# (real captured shapes from this repo: unittests, then two integration
# test binaries).
# ---------------------------------------------------------------------------
$syntheticListOutput = @(
    "    Finished `test` profile [unoptimized + debuginfo] target(s) in 5.88s",
    "     Running unittests src\lib.rs (target\debug\deps\mqk_db-6c2951fba3a20d6d.exe)",
    "test_support::tests::split_db_url_handles_no_query_string: test",
    "runtime_lease::tests::acquire_when_no_lease_exists: test",
    "     Running tests\test_support_disposable_db.rs (target\debug\deps\test_support_disposable_db-43c90f6143cec1ce.exe)",
    "cancellation_racing_the_create_database_statement_leaves_zero_residue: test",
    "     Running tests\other_scenario.rs (target\debug\deps\other_scenario-aaaa.exe)",
    "runtime_lease::tests::acquire_when_no_lease_exists: test"
)
$rows = Get-ExactLiveKeysForPackage -Package "mqk-db" -ListOutput $syntheticListOutput
$expectedKeys = @(
    "mqk-db|lib|test_support::tests::split_db_url_handles_no_query_string",
    "mqk-db|lib|runtime_lease::tests::acquire_when_no_lease_exists",
    "mqk-db|test_support_disposable_db|cancellation_racing_the_create_database_statement_leaves_zero_residue",
    "mqk-db|other_scenario|runtime_lease::tests::acquire_when_no_lease_exists"
)
$actualKeys = @($rows | ForEach-Object { $_.Key })
$missingExpected = @($expectedKeys | Where-Object { $_ -notin $actualKeys })
if (($rows.Count -eq 4) -and ($missingExpected.Count -eq 0)) {
    Pass "CSIM09" "Target context (lib vs. two distinct integration-test binaries) is tracked correctly across a multi-section --list output, and a bare name repeated in two targets is kept distinct"
} else {
    Fail "CSIM09" "Target-context tracking failed (got $($rows.Count) rows: $($actualKeys -join ', '); missing expected: $($missingExpected -join ', '))"
}

# ---------------------------------------------------------------------------
# CSIM10: BLOCKED_LOCAL_PREREQUISITE rows are excluded from execution by
# default (--skip added for each), and are NOT excluded when AuditOnly is
# requested.
# ---------------------------------------------------------------------------
$withBlocked = @(
    (New-Row "mqk-daemon" "scenario_x" "safe_test" "SAFE_DB_5434"),
    (New-Row "mqk-daemon" "scenario_y" "blocked_test_one" "BLOCKED_LOCAL_PREREQUISITE"),
    (New-Row "mqk-daemon" "scenario_z" "blocked_test_two" "BLOCKED_LOCAL_PREREQUISITE")
)
$blockedRows = Get-BlockedTestRows -Inventory $withBlocked
$skipArgsDefault = Get-SkipArgsForBlockedRows -BlockedRows $blockedRows -AuditOnly $false
$skipArgsAudit = Get-SkipArgsForBlockedRows -BlockedRows $blockedRows -AuditOnly $true

$defaultOk = ($blockedRows.Count -eq 2) -and
    ($skipArgsDefault -contains "blocked_test_one") -and
    ($skipArgsDefault -contains "blocked_test_two") -and
    ($skipArgsDefault.Count -eq 4)
$auditOk = ($skipArgsAudit.Count -eq 0)

if ($defaultOk -and $auditOk) {
    Pass "CSIM10" "BLOCKED_LOCAL_PREREQUISITE rows are --skip-excluded by default and only included with explicit AuditOnly (false-green prevention)"
} else {
    Fail "CSIM10" "Blocked-row skip-arg computation is wrong (default skip args: $($skipArgsDefault -join ' '); audit skip args: $($skipArgsAudit -join ' '))"
}

Write-Host ""
if ($FAILURES -eq 0) {
    Write-Host "=== ALL CSIM INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $FAILURES INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
