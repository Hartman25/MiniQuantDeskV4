# =============================================================================
# Mutation-negative tests for scripts/windows/Invoke-CanonicalSafeIgnoredMatrix.ps1
#
# FULL-AUDIT-CHECKPOINT-HARDENING-REPAIR-01 Part 3: proves the exact-identity
# rework of the ignored-test inventory actually catches duplicate exact
# keys, a bare function name colliding across two different targets, missing
# rows, stale rows, and unknown classifications -- and that
# BLOCKED_LOCAL_PREREQUISITE rows are excluded from execution by default.
#
# FULL-AUDIT-TESTKIT-AND-IGNORED-AUTHORITY-FINAL-REPAIR-01 Part 2/3: proves
# `Get-ExactLiveKeysForPackage` resolves a `Running unittests <path> (...)`
# header via real cargo-metadata-derived target lookup (never assumes
# "lib"), so a package with both a lib and one or more bin unit-test
# binaries (or two bin targets sharing a same-named test) keeps every
# target's identity distinct -- and proves `Get-MatrixDisposition` gates the
# final disposition on BLOCKED_LOCAL_PREREQUISITE rows, not just exit codes.
#
# This dot-sources the real script to reach its pure comparison functions
# (Test-InventorySelfValidation, Get-InventoryCompletenessResult,
# Get-ExactLiveKeysForPackage, Get-TargetLookupFromMetadataTargets,
# Get-BlockedTestRows, Get-SkipArgsForBlockedRows, Get-MatrixDisposition)
# directly against synthetic data -- no real cargo invocation, no real
# inventory CSV, no live workspace build. Dot-sourcing does not trigger Main
# (see the InvocationName guard at the bottom of the script under test).
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

# Builds a synthetic cargo-metadata `targets` array entry, matching the
# shape `Get-TargetLookupFromMetadataTargets` reads (`.kind`, `.name`,
# `.src_path`).
function New-MetadataTarget {
    param([string]$Kind, [string]$Name, [string]$SrcPath)
    [pscustomobject]@{
        kind     = @($Kind)
        name     = $Name
        src_path = $SrcPath
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
$mqkDbTargetLookup = Get-TargetLookupFromMetadataTargets -MetadataTargets @(
    (New-MetadataTarget "lib" "mqk-db" "src\lib.rs"),
    (New-MetadataTarget "test" "test_support_disposable_db" "tests\test_support_disposable_db.rs"),
    (New-MetadataTarget "test" "other_scenario" "tests\other_scenario.rs")
)
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
$rows = Get-ExactLiveKeysForPackage -Package "mqk-db" -ListOutput $syntheticListOutput -TargetLookup $mqkDbTargetLookup
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
# CSIM09b: a package with BOTH a lib and a bin unit-test binary must not
# collapse both `Running unittests ...` headers to `lib` -- this is the
# exact defect FULL-AUDIT-TESTKIT-AND-IGNORED-AUTHORITY-FINAL-REPAIR-01
# Part 2 closes (mirrors this repo's real mqk-daemon shape: src/lib.rs +
# src/main.rs, each compiled as its own separate unit-test binary).
# ---------------------------------------------------------------------------
$libBinTargetLookup = Get-TargetLookupFromMetadataTargets -MetadataTargets @(
    (New-MetadataTarget "lib" "mqk-daemon" "src\lib.rs"),
    (New-MetadataTarget "bin" "mqk-daemon" "src\main.rs")
)
$libBinListOutput = @(
    "     Running unittests src\lib.rs (target\debug\deps\mqk_daemon-aaa.exe)",
    "dynamic_selection_evidence_validator::tests::some_lib_test: test",
    "     Running unittests src\main.rs (target\debug\deps\mqk_daemon-bbb.exe)",
    "tests::some_main_only_test: test"
)
$rows = Get-ExactLiveKeysForPackage -Package "mqk-daemon" -ListOutput $libBinListOutput -TargetLookup $libBinTargetLookup
$libRow = $rows | Where-Object { $_.Function -eq "dynamic_selection_evidence_validator::tests::some_lib_test" }
$binRow = $rows | Where-Object { $_.Function -eq "tests::some_main_only_test" }
if (($rows.Count -eq 2) -and $libRow -and ($libRow.Target -eq "lib") -and ($libRow.TargetKind -eq "Lib") -and `
        $binRow -and ($binRow.Target -eq "mqk-daemon") -and ($binRow.TargetKind -eq "Bin") -and `
        ($libRow.Key -ne $binRow.Key)) {
    Pass "CSIM09b" "lib and main.rs unit-test binaries resolve to distinct target names/kinds (main.rs is never collapsed into 'lib')"
} else {
    Fail "CSIM09b" "lib vs. bin unit-test target resolution failed (rows: $(($rows | ForEach-Object { $_.Key + '=' + $_.TargetKind }) -join ', '))"
}

# ---------------------------------------------------------------------------
# CSIM09c: the SAME qualified test name present in two different bin unit-
# test binaries within one package remains two distinct identities (never
# deduplicated or collided just because the qualified function string
# matches).
# ---------------------------------------------------------------------------
$twoBinsTargetLookup = Get-TargetLookupFromMetadataTargets -MetadataTargets @(
    (New-MetadataTarget "bin" "mqk_paper_loop" "src\bin\mqk_paper_loop.rs"),
    (New-MetadataTarget "bin" "mqk_other_tool" "src\bin\mqk_other_tool.rs")
)
$twoBinsListOutput = @(
    "     Running unittests src\bin\mqk_paper_loop.rs (target\debug\deps\mqk_paper_loop-aaa.exe)",
    "tests::shared_helper_name: test",
    "     Running unittests src\bin\mqk_other_tool.rs (target\debug\deps\mqk_other_tool-bbb.exe)",
    "tests::shared_helper_name: test"
)
$rows = Get-ExactLiveKeysForPackage -Package "mqk-cli" -ListOutput $twoBinsListOutput -TargetLookup $twoBinsTargetLookup
$keys = @($rows | ForEach-Object { $_.Key })
if (($rows.Count -eq 2) -and ("mqk-cli|mqk_paper_loop|tests::shared_helper_name" -in $keys) -and `
        ("mqk-cli|mqk_other_tool|tests::shared_helper_name" -in $keys)) {
    Pass "CSIM09c" "The same qualified test name in two different bin targets remains two distinct exact identities"
} else {
    Fail "CSIM09c" "Same-qualified-name-in-two-bins collision was not kept distinct (rows: $($keys -join ', '))"
}

# ---------------------------------------------------------------------------
# CSIM09d: a `Running unittests <path> (...)` header whose path has no
# corresponding cargo-metadata lib/bin target is a hard failure, not a
# silent fallback to "lib" -- proves the lookup miss path fails closed.
# ---------------------------------------------------------------------------
$threw = $false
try {
    Get-ExactLiveKeysForPackage -Package "mqk-daemon" -ListOutput @(
        "     Running unittests src\lib.rs (target\debug\deps\mqk_daemon-aaa.exe)",
        "tests::some_test: test"
    ) -TargetLookup @()
} catch {
    $threw = $true
}
if ($threw) {
    Pass "CSIM09d" "Unresolvable unit-test source path throws instead of silently defaulting to 'lib'"
} else {
    Fail "CSIM09d" "Unresolvable unit-test source path did NOT throw -- risks a silent fallback identity"
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

# ---------------------------------------------------------------------------
# CSIM11: one or more BLOCKED_LOCAL_PREREQUISITE rows makes the default
# (non-audit) disposition non-green -- BLOCKED, nonzero exit, and critically
# the message text is never the "PASSED" string -- even though both cargo
# exit codes are 0.
# ---------------------------------------------------------------------------
$d = Get-MatrixDisposition -SafeExit 0 -ManualCompileExit 0 -BlockedCount 2 -AuditOnlyIncludeBlocked $false
if (($d.Disposition -eq "BLOCKED") -and ($d.ExitCode -ne 0) -and ($d.Disposition -ne "PASSED") -and ($d.Message -notmatch "^PASSED")) {
    Pass "CSIM11" "One or more BLOCKED_LOCAL_PREREQUISITE rows makes the default disposition BLOCKED/non-green, never PASSED, even with clean exit codes"
} else {
    Fail "CSIM11" "Blocked rows did not force a non-green disposition (got Disposition=$($d.Disposition) ExitCode=$($d.ExitCode) Message=$($d.Message))"
}

# ---------------------------------------------------------------------------
# CSIM12: -AuditOnlyIncludeBlocked only removes the BLOCKED disposition once
# execution actually happened AND succeeded (SafeExit -eq 0) -- audit mode
# does not get a free pass on its own.
# ---------------------------------------------------------------------------
$dAuditPass = Get-MatrixDisposition -SafeExit 0 -ManualCompileExit 0 -BlockedCount 2 -AuditOnlyIncludeBlocked $true
$dAuditFail = Get-MatrixDisposition -SafeExit 1 -ManualCompileExit 0 -BlockedCount 2 -AuditOnlyIncludeBlocked $true
if (($dAuditPass.Disposition -eq "PASSED") -and ($dAuditFail.Disposition -eq "FAILED") -and ($dAuditFail.Disposition -ne "PASSED")) {
    Pass "CSIM12" "AuditOnlyIncludeBlocked is only green after blocked rows actually executed and passed (SafeExit=0); a failing execution still reports FAILED, not PASSED"
} else {
    Fail "CSIM12" "AuditOnlyIncludeBlocked disposition is wrong (pass-case=$($dAuditPass.Disposition), fail-case=$($dAuditFail.Disposition))"
}

# ---------------------------------------------------------------------------
# CSIM13: zero blocked rows and clean exit codes is the only case that
# produces PASSED -- and a nonzero SafeExit is never masked by a zero
# BlockedCount.
# ---------------------------------------------------------------------------
$dClean = Get-MatrixDisposition -SafeExit 0 -ManualCompileExit 0 -BlockedCount 0 -AuditOnlyIncludeBlocked $false
$dSafeFailedNoBlocked = Get-MatrixDisposition -SafeExit 1 -ManualCompileExit 0 -BlockedCount 0 -AuditOnlyIncludeBlocked $false
if (($dClean.Disposition -eq "PASSED") -and ($dClean.ExitCode -eq 0) -and ($dSafeFailedNoBlocked.Disposition -eq "FAILED")) {
    Pass "CSIM13" "Zero blocked rows with clean exit codes is PASSED; a real test failure with zero blocked rows still reports FAILED"
} else {
    Fail "CSIM13" "Baseline disposition logic is wrong (clean=$($dClean.Disposition), safe-fail-no-blocked=$($dSafeFailedNoBlocked.Disposition))"
}

# ===========================================================================
# FULL-AUDIT-CANONICAL-IGNORED-RUNNER-FEATURE-AUTHORITY-01 Part 1:
# Get-PackageFeaturePlan mutation fixtures. Uses synthetic
# `cargo metadata --no-deps`-shaped dependency arrays (never a real cargo
# invocation) built by New-DepEntry below.
# ===========================================================================

function New-DepEntry {
    param([string]$Name, [string]$Kind = $null, [string[]]$Features = @())
    [pscustomobject]@{
        name     = $Name
        kind     = $Kind
        features = $Features
    }
}

# ---------------------------------------------------------------------------
# CSIM14: a package with no mqk-db dependency entry at all receives no
# foreign-package feature argument -- this is the exact Defect 1 case
# reproduced against mqk-artifacts/mqk-backtest (a real, direct cargo error
# if mqk-db/testkit were injected here).
# ---------------------------------------------------------------------------
$depsNoMqkDb = @{
    "mqk-artifacts" = @(New-DepEntry -Name "anyhow")
}
$plan = Get-PackageFeaturePlan -PackageNames @("mqk-artifacts") -PackageDependencies $depsNoMqkDb
if (($plan["mqk-artifacts"].FeatureArgs.Count -eq 0)) {
    Pass "CSIM14" "Package with no mqk-db dependency receives no foreign-package feature argument"
} else {
    Fail "CSIM14" "Package with no mqk-db dependency wrongly received: $($plan['mqk-artifacts'].FeatureArgs -join ' ')"
}

# ---------------------------------------------------------------------------
# CSIM15: mqk-db itself always receives its own local `--features testkit`
# (never the foreign `mqk-db/testkit` form, which would be nonsensical for
# the package itself).
# ---------------------------------------------------------------------------
$plan = Get-PackageFeaturePlan -PackageNames @("mqk-db") -PackageDependencies @{}
$mqkDbArgs = $plan["mqk-db"].FeatureArgs -join ' '
if ($mqkDbArgs -eq "--features testkit") {
    Pass "CSIM15" "mqk-db itself receives its own local --features testkit"
} else {
    Fail "CSIM15" "mqk-db itself received the wrong feature args: '$mqkDbArgs'"
}

# ---------------------------------------------------------------------------
# CSIM16: a package whose OWN dev-dependency declaration already enables
# mqk-db/testkit (this repo's real shape for mqk-daemon/mqk-execution/
# mqk-runtime/mqk-testkit) receives no explicit foreign-package feature --
# it already compiles correctly without one, since cargo always activates
# dev-dependencies for `cargo test`.
# ---------------------------------------------------------------------------
$depsAlreadyTestkit = @{
    "mqk-daemon" = @(
        (New-DepEntry -Name "mqk-db" -Kind $null -Features @()),
        (New-DepEntry -Name "mqk-db" -Kind "dev" -Features @("testkit"))
    )
}
$plan = Get-PackageFeaturePlan -PackageNames @("mqk-daemon") -PackageDependencies $depsAlreadyTestkit
if ($plan["mqk-daemon"].FeatureArgs.Count -eq 0) {
    Pass "CSIM16" "Package whose own dev-dependency already enables mqk-db/testkit receives no foreign feature injection"
} else {
    Fail "CSIM16" "Package with dev-dependency-granted testkit wrongly received: $($plan['mqk-daemon'].FeatureArgs -join ' ')"
}

# ---------------------------------------------------------------------------
# CSIM17: a package that depends on mqk-db directly but has no pre-existing
# testkit activation anywhere in its own manifest (this repo's real shape
# for mqk-cli) receives the explicit --features mqk-db/testkit argument.
# ---------------------------------------------------------------------------
$depsPlainDependency = @{
    "mqk-cli" = @(New-DepEntry -Name "mqk-db" -Kind $null -Features @())
}
$plan = Get-PackageFeaturePlan -PackageNames @("mqk-cli") -PackageDependencies $depsPlainDependency
$cliArgs = $plan["mqk-cli"].FeatureArgs -join ' '
if ($cliArgs -eq "--features mqk-db/testkit") {
    Pass "CSIM17" "Package depending on mqk-db with no pre-existing testkit activation receives the explicit foreign-package feature"
} else {
    Fail "CSIM17" "Plain mqk-db dependent received the wrong feature args: '$cliArgs'"
}

# ---------------------------------------------------------------------------
# CSIM18: one package cannot inherit another package's feature arguments --
# building the plan for several packages at once keeps each package's
# FeatureArgs fully isolated (no shared array reference bleed, no
# cross-contamination of a neighbor's flag).
# ---------------------------------------------------------------------------
$depsMixed = @{
    "mqk-artifacts" = @(New-DepEntry -Name "anyhow")
    "mqk-cli"       = @(New-DepEntry -Name "mqk-db" -Kind $null -Features @())
    "mqk-daemon"    = @(
        (New-DepEntry -Name "mqk-db" -Kind $null -Features @()),
        (New-DepEntry -Name "mqk-db" -Kind "dev" -Features @("testkit"))
    )
}
$plan = Get-PackageFeaturePlan -PackageNames @("mqk-artifacts", "mqk-cli", "mqk-daemon", "mqk-db") -PackageDependencies $depsMixed
$artifactsArgs = $plan["mqk-artifacts"].FeatureArgs -join ' '
$cliArgs2 = $plan["mqk-cli"].FeatureArgs -join ' '
$daemonArgs = $plan["mqk-daemon"].FeatureArgs -join ' '
$dbArgs = $plan["mqk-db"].FeatureArgs -join ' '
if (($artifactsArgs -eq "") -and ($cliArgs2 -eq "--features mqk-db/testkit") -and ($daemonArgs -eq "") -and ($dbArgs -eq "--features testkit")) {
    Pass "CSIM18" "Each package's feature plan entry is computed independently -- no package inherits another's feature arguments"
} else {
    Fail "CSIM18" "Plan entries bled across packages (artifacts='$artifactsArgs' cli='$cliArgs2' daemon='$daemonArgs' db='$dbArgs')"
}

# ---------------------------------------------------------------------------
# CSIM19: a package name with no dependency information available at all
# (not merely "no mqk-db dependency", but entirely absent from the plan
# input) is an unresolvable feature requirement -- a hard failure, not a
# silent "no argument".
# ---------------------------------------------------------------------------
$threw = $false
try {
    Get-PackageFeaturePlan -PackageNames @("mqk-unknown-package") -PackageDependencies @{}
} catch {
    $threw = $true
}
if ($threw) {
    Pass "CSIM19" "A package with no dependency information at all is an unresolvable feature requirement and throws"
} else {
    Fail "CSIM19" "A package missing from the plan input did NOT throw -- risks silently resolving to an empty/wrong plan"
}

# ===========================================================================
# FULL-AUDIT-CANONICAL-IGNORED-RUNNER-FEATURE-AUTHORITY-01 Part 2:
# Get-ManualExternalDiffResult mutation fixtures.
# ===========================================================================

# ---------------------------------------------------------------------------
# CSIM20: the clean case -- feature-on-minus-feature-off equals the
# MANUAL_EXTERNAL inventory rows exactly. Missing/Stale/Leaked all empty.
# ---------------------------------------------------------------------------
$manualInventoryClean = @(
    (New-Row "mqk-daemon" "scenario_manual" "manual_test_one" "MANUAL_EXTERNAL"),
    (New-Row "mqk-daemon" "scenario_manual" "manual_test_two" "MANUAL_EXTERNAL")
)
$featureOnClean = @(
    (New-LiveRow "mqk-daemon" "scenario_manual" "manual_test_one"),
    (New-LiveRow "mqk-daemon" "scenario_manual" "manual_test_two"),
    (New-LiveRow "mqk-daemon" "scenario_manual" "always_present_test")
)
$featureOffClean = @(
    (New-LiveRow "mqk-daemon" "scenario_manual" "always_present_test")
)
$result = Get-ManualExternalDiffResult -Inventory $manualInventoryClean -FeatureOnRows $featureOnClean -FeatureOffRows $featureOffClean
if (($result.Missing.Count -eq 0) -and ($result.Stale.Count -eq 0) -and ($result.Leaked.Count -eq 0) -and ($result.DiffKeys.Count -eq 2)) {
    Pass "CSIM20" "Clean feature-on-minus-feature-off difference equals the MANUAL_EXTERNAL inventory exactly (a test present in both builds is correctly excluded from the difference)"
} else {
    Fail "CSIM20" "Clean manual-external case unexpectedly reported problems (missing=$($result.Missing.Count) stale=$($result.Stale.Count) leaked=$($result.Leaked.Count) diff=$($result.DiffKeys.Count))"
}

# ---------------------------------------------------------------------------
# CSIM21: missing -- a feature-on-only test with no MANUAL_EXTERNAL
# inventory row at all (an unclassified new manual-external test).
# ---------------------------------------------------------------------------
$featureOnMissing = @(
    (New-LiveRow "mqk-daemon" "scenario_manual" "manual_test_one"),
    (New-LiveRow "mqk-daemon" "scenario_manual" "brand_new_manual_test")
)
$manualInventoryMissing = @(
    (New-Row "mqk-daemon" "scenario_manual" "manual_test_one" "MANUAL_EXTERNAL")
)
$result = Get-ManualExternalDiffResult -Inventory $manualInventoryMissing -FeatureOnRows $featureOnMissing -FeatureOffRows @()
if (($result.Missing.Count -eq 1) -and ($result.Missing[0] -eq "mqk-daemon|scenario_manual|brand_new_manual_test") -and ($result.Stale.Count -eq 0)) {
    Pass "CSIM21" "A feature-on test not classified MANUAL_EXTERNAL is reported missing"
} else {
    Fail "CSIM21" "Missing manual-external detection failed (missing=$($result.Missing -join ', '), stale count=$($result.Stale.Count))"
}

# ---------------------------------------------------------------------------
# CSIM22: stale -- a MANUAL_EXTERNAL inventory row that no longer appears in
# the feature-on-minus-feature-off difference (renamed, moved, or deleted).
# ---------------------------------------------------------------------------
$manualInventoryStale = @(
    (New-Row "mqk-daemon" "scenario_manual" "renamed_or_deleted_manual_test" "MANUAL_EXTERNAL")
)
$result = Get-ManualExternalDiffResult -Inventory $manualInventoryStale -FeatureOnRows @() -FeatureOffRows @()
if (($result.Stale.Count -eq 1) -and ($result.Missing.Count -eq 0) -and ($result.Leaked.Count -eq 0)) {
    Pass "CSIM22" "A MANUAL_EXTERNAL inventory row absent from the feature-on-minus-feature-off difference is reported stale"
} else {
    Fail "CSIM22" "Stale manual-external detection failed (stale=$($result.Stale.Count) missing=$($result.Missing.Count) leaked=$($result.Leaked.Count))"
}

# ---------------------------------------------------------------------------
# CSIM23: leaked-to-default -- a MANUAL_EXTERNAL inventory row whose exact
# key ALSO appears in the feature-OFF (default) live set -- its
# #[cfg(feature = "manual-external")] compile-time boundary has been lost.
# ---------------------------------------------------------------------------
$manualInventoryLeaked = @(
    (New-Row "mqk-daemon" "scenario_manual" "leaked_test" "MANUAL_EXTERNAL")
)
$featureOnLeaked = @(
    (New-LiveRow "mqk-daemon" "scenario_manual" "leaked_test")
)
$featureOffLeaked = @(
    (New-LiveRow "mqk-daemon" "scenario_manual" "leaked_test")
)
$result = Get-ManualExternalDiffResult -Inventory $manualInventoryLeaked -FeatureOnRows $featureOnLeaked -FeatureOffRows $featureOffLeaked
if (($result.Leaked.Count -eq 1) -and ($result.DiffKeys.Count -eq 0)) {
    Pass "CSIM23" "A MANUAL_EXTERNAL row also present in the feature-off build is reported leaked (compile-time boundary lost), and never counted in the difference itself"
} else {
    Fail "CSIM23" "Leaked-to-default manual-external detection failed (leaked=$($result.Leaked.Count) diff=$($result.DiffKeys.Count))"
}

# ---------------------------------------------------------------------------
# CSIM24: the safe/default classification filter (the exact list Main uses
# before calling Get-InventoryCompletenessResult) excludes MANUAL_EXTERNAL
# rows -- proving Defect 2 (MANUAL_EXTERNAL rows becoming false stale rows
# once Defect 1 is fixed and every package's --ignored --list succeeds) is
# actually closed at the filtering step, not just in prose.
# ---------------------------------------------------------------------------
$mixedClassInventory = @(
    (New-Row "mqk-daemon" "scenario_manual" "manual_test_one" "MANUAL_EXTERNAL"),
    (New-Row "mqk-daemon" "scenario_x" "safe_test" "SAFE_DB_5434"),
    (New-Row "mqk-daemon" "scenario_y" "blocked_test" "BLOCKED_LOCAL_PREREQUISITE")
)
$filtered = @($mixedClassInventory | Where-Object { $_.classification -in $SafeDefaultClassifications })
if (($filtered.Count -eq 2) -and (-not ($filtered | Where-Object { $_.classification -eq "MANUAL_EXTERNAL" }))) {
    Pass "CSIM24" "The safe/default classification filter excludes MANUAL_EXTERNAL rows before completeness comparison"
} else {
    Fail "CSIM24" "Safe/default filter wrongly included or excluded rows (filtered count=$($filtered.Count))"
}

# ===========================================================================
# FULL-AUDIT-CANONICAL-IGNORED-RUNNER-FEATURE-AUTHORITY-01 Part 3:
# metadata-authoritative target mapping mutation fixtures.
# ===========================================================================

# ---------------------------------------------------------------------------
# CSIM25: a custom [[test]] name/path pair where the declared Cargo target
# name differs from the file's stem must resolve to the real metadata name,
# never a guessed stem -- the exact defect Part 3 closes (this repo's
# current [[test]] blocks all coincidentally have name == stem, so this
# fixture is the only proof the code does not merely get lucky).
# ---------------------------------------------------------------------------
$customNameTargetLookup = Get-TargetLookupFromMetadataTargets -MetadataTargets @(
    (New-MetadataTarget "test" "totally_custom_target_name" "tests/scenario_with_a_different_filename.rs")
)
$customNameListOutput = @(
    "     Running tests\scenario_with_a_different_filename.rs (target\debug\deps\totally_custom_target_name-aaa.exe)",
    "some_module::tests::an_ignored_case: test"
)
$rows = Get-ExactLiveKeysForPackage -Package "mqk-db" -ListOutput $customNameListOutput -TargetLookup $customNameTargetLookup
if (($rows.Count -eq 1) -and ($rows[0].Target -eq "totally_custom_target_name") -and ($rows[0].TargetKind -eq "IntegrationTest")) {
    Pass "CSIM25" "A custom [[test]] name/path pair resolves to the real Cargo target name, not a guessed file stem"
} else {
    Fail "CSIM25" "Custom integration-test name/path resolution failed (rows: $(($rows | ForEach-Object { $_.Target + '=' + $_.TargetKind }) -join ', '))"
}

# ---------------------------------------------------------------------------
# CSIM26: an example target (testable, harness=true) resolves via the same
# "Running unittests <path> (...)" header convention as lib/bin, with its
# own real metadata name and Kind=Example -- never collapsed into Lib/Bin.
# ---------------------------------------------------------------------------
$exampleTargetLookup = Get-TargetLookupFromMetadataTargets -MetadataTargets @(
    (New-MetadataTarget "example" "demo_example" "examples/demo_example.rs")
)
$exampleListOutput = @(
    "     Running unittests examples\demo_example.rs (target\debug\deps\demo_example-aaa.exe)",
    "tests::example_has_an_ignored_case: test"
)
$rows = Get-ExactLiveKeysForPackage -Package "mqk-db" -ListOutput $exampleListOutput -TargetLookup $exampleTargetLookup
if (($rows.Count -eq 1) -and ($rows[0].Target -eq "demo_example") -and ($rows[0].TargetKind -eq "Example")) {
    Pass "CSIM26" "An example target resolves to its own real metadata name and Kind=Example"
} else {
    Fail "CSIM26" "Example target resolution failed (rows: $(($rows | ForEach-Object { $_.Target + '=' + $_.TargetKind }) -join ', '))"
}

# ---------------------------------------------------------------------------
# CSIM27: a proc-macro library target resolves the same way a plain lib
# target does -- Kind=Lib, literal Name="lib" -- since both compile as the
# crate's own single library unit-test binary and select via --lib.
# ---------------------------------------------------------------------------
$procMacroTargetLookup = Get-TargetLookupFromMetadataTargets -MetadataTargets @(
    (New-MetadataTarget "proc-macro" "mqk_some_macros" "src/lib.rs")
)
$procMacroListOutput = @(
    "     Running unittests src\lib.rs (target\debug\deps\mqk_some_macros-aaa.exe)",
    "tests::macro_expansion_case: test"
)
$rows = Get-ExactLiveKeysForPackage -Package "mqk-some-macros" -ListOutput $procMacroListOutput -TargetLookup $procMacroTargetLookup
if (($rows.Count -eq 1) -and ($rows[0].Target -eq "lib") -and ($rows[0].TargetKind -eq "Lib")) {
    Pass "CSIM27" "A proc-macro library target resolves identically to a plain lib target (Kind=Lib, Name='lib')"
} else {
    Fail "CSIM27" "Proc-macro target resolution failed (rows: $(($rows | ForEach-Object { $_.Target + '=' + $_.TargetKind }) -join ', '))"
}

# ---------------------------------------------------------------------------
# CSIM28: an ambiguous path match -- two distinct metadata targets whose
# normalized source paths both satisfy the same printed-path resolution
# (here: an exact duplicate path, the simplest way to force ambiguity) --
# is a hard failure, never a silent "whichever one was found first".
# ---------------------------------------------------------------------------
$ambiguousLookup = @(
    [pscustomobject]@{ Kind = "Bin"; Name = "tool_a"; NormalizedSrcPath = "crates/mqk-cli/src/bin/shared.rs" },
    [pscustomobject]@{ Kind = "Bin"; Name = "tool_b"; NormalizedSrcPath = "crates/mqk-cli/src/bin/shared.rs" }
)
$threw = $false
try {
    Resolve-TestableTarget -TargetLookup $ambiguousLookup -PrintedPath "crates/mqk-cli/src/bin/shared.rs"
} catch {
    $threw = $true
}
if ($threw) {
    Pass "CSIM28" "Two metadata targets sharing the exact same source path is an ambiguous match and throws, never silently resolved to the first one found"
} else {
    Fail "CSIM28" "Ambiguous path resolution did NOT throw -- risks silently resolving to the wrong target's identity"
}

# ---------------------------------------------------------------------------
# CSIM29: a missing path match -- no metadata target corresponds to the
# printed path at all -- returns $null (never a guessed default), so the
# caller (Get-ExactLiveKeysForPackage) can turn it into its own explicit
# "target lookup is out of date" failure.
# ---------------------------------------------------------------------------
$missingLookup = @(
    [pscustomobject]@{ Kind = "Lib"; Name = "lib"; NormalizedSrcPath = "crates/mqk-db/src/lib.rs" }
)
$resolved = Resolve-TestableTarget -TargetLookup $missingLookup -PrintedPath "tests/completely_unrelated.rs"
if ($null -eq $resolved) {
    Pass "CSIM29" "A printed path with no corresponding metadata target resolves to `$null, never a guessed default"
} else {
    Fail "CSIM29" "Missing path resolution unexpectedly returned a match: $($resolved.Kind):$($resolved.Name)"
}

# ---------------------------------------------------------------------------
# CSIM30: Get-TargetSelectorArgs produces the correct exact-scoped cargo
# selector flags for Example and Bench kinds (added alongside Lib/Bin/
# IntegrationTest), and still throws for an unsupported kind (e.g. Doctest,
# which this script never selects directly).
# ---------------------------------------------------------------------------
$exampleSelector = Get-TargetSelectorArgs -TargetKind "Example" -TargetName "demo_example"
$benchSelector = Get-TargetSelectorArgs -TargetKind "Bench" -TargetName "demo_bench"
$doctestThrew = $false
try {
    Get-TargetSelectorArgs -TargetKind "Doctest" -TargetName "doctest"
} catch {
    $doctestThrew = $true
}
if ((($exampleSelector -join ' ') -eq "--example demo_example") -and (($benchSelector -join ' ') -eq "--bench demo_bench") -and $doctestThrew) {
    Pass "CSIM30" "Get-TargetSelectorArgs supports Example/Bench exact-scoped selection and still rejects unsupported kinds (e.g. Doctest)"
} else {
    Fail "CSIM30" "Target selector args are wrong (example='$($exampleSelector -join ' ')' bench='$($benchSelector -join ' ')' doctest-threw=$doctestThrew)"
}

Write-Host ""
if ($FAILURES -eq 0) {
    Write-Host "=== ALL CSIM INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $FAILURES INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
