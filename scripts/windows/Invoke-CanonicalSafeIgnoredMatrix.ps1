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
# FULL-AUDIT-CHECKPOINT-HARDENING-REPAIR-01 Part 3 hardening: the previous
# version keyed the inventory purely by bare function name
# (`$inventoryByName[$row.function]`) and reduced every live `--ignored
# --list` line to its bare tail (`($qualified -split '::')[-1]`) before
# comparing. That bare-name identity has two proven failure modes in this
# repo's own inventory (see tests/script_guards/test_canonical_safe_ignored_matrix.ps1):
#   - two different tests in two different crates/targets sharing a bare
#     function name silently collide in the bare-name hashtable -- whichever
#     row is read last for that name wins, silently discarding the other
#     row's classification (12 such collisions already existed in this
#     repo's own inventory before this patch);
#   - a row whose test was renamed or deleted (a "stale" row) is never
#     detected, because completeness only checked live -> inventory, never
#     inventory -> live.
#
# The fix: every test's identity is now the exact triple (package, test
# target, fully-qualified test name) -- "package" from `cargo metadata`,
# "target" is `lib` for a crate's own unit tests, `doctest` for doc tests, or
# the tests/*.rs file's stem for an integration-test binary, and "fully-
# qualified test name" is exactly the string cargo's own `--list` output
# prints (module path preserved, not truncated to a bare tail). The CSV
# inventory (scripts/test/ignored_test_inventory.csv) now carries an
# explicit `target` column and a `function` column holding that full
# qualified name, so the composite key is directly comparable on both sides
# with no lossy reduction anywhere in the pipeline.
#
# FULL-AUDIT-TESTKIT-AND-IGNORED-AUTHORITY-FINAL-REPAIR-01 Part 2 hardening:
# the above rework still mapped *every* `Running unittests <path> (...)`
# header -- cargo emits one per unit-test binary, which includes a crate's
# own `src/lib.rs` *and* `src/main.rs` *and* every `src/bin/<name>.rs` -- to
# the single target name `lib`, unconditionally. A crate with both a lib and
# a bin (or multiple bins) would silently collapse their distinct unit-test
# identities into one. The fix: `Get-ExactLiveKeysForPackage` now takes a
# `-TargetLookup` table (built by `Get-TargetLookupFromMetadataTargets` from
# `cargo metadata`'s own `targets[].kind`/`targets[].name`/`targets[].src_path`
# -- compiler-artifact truth, not a guess) keyed by the exact source path
# cargo itself prints in the `Running unittests <path> (...)` header, and
# resolves each such header to its real target name (`lib`, or the exact bin
# name) and kind (Lib/Bin/IntegrationTest/Example). `Get-ExactLiveKeysForPackage`
# itself stays a pure, cargo-free function -- `Main` is the only thing that
# ever calls `cargo metadata` to build the real lookup; a test can still pass
# a synthetic one.
#
# FULL-AUDIT-TESTKIT-AND-IGNORED-AUTHORITY-FINAL-REPAIR-01 Part 3 hardening:
# two further defects in Step 2/the final disposition:
#   - `--skip <bare function name>` (no `--exact`) run against a single
#     `--workspace --all-targets` invocation is a *substring* match with
#     *workspace-wide* scope -- neither package- nor target-scoped, and not
#     even exact-string-scoped. A `BLOCKED_LOCAL_PREREQUISITE` row's
#     `--skip` could therefore also silently skip an unrelated, non-blocked
#     test elsewhere in the workspace whose qualified name merely contains
#     that same substring, which is a false-green risk this script must not
#     admit. The fix: Step 2 now executes each live (package, exact target)
#     pair as its own `cargo test -p <pkg> --lib|--bin <name>|--test <name>`
#     invocation (using the same `Get-TargetLookupFromMetadataTargets`-
#     derived kind/name from Step 1) with `--exact`, so a `--skip` can only
#     ever affect the one specific compiled test binary the blocked row
#     actually belongs to, matched by exact string, not substring.
#   - the final disposition printed the green "PASSED: ... is green" message
#     and exited 0 purely based on `$safeExit`/`$manualCompileExit`, even
#     when one or more `BLOCKED_LOCAL_PREREQUISITE` rows had been silently
#     excluded from execution by default -- a blocked proof is not a passing
#     proof. The fix: `Get-MatrixDisposition` (a pure function, mutation-
#     tested the same way as the rest of this file) now also gates on
#     blocked-row count and `-AuditOnlyIncludeBlocked`, and `Main` only ever
#     prints the green message when that function reports `PASSED`.
#
# The comparison/disposition logic itself (Test-InventorySelfValidation,
# Get-InventoryCompletenessResult, Get-ExactLiveKeysForPackage,
# Get-TargetLookupFromMetadataTargets, Get-MatrixDisposition) is defined as
# pure, cargo-free functions at script scope specifically so
# tests/script_guards/test_canonical_safe_ignored_matrix.ps1 can dot-source
# this file and exercise duplicate/stale/missing/unknown-classification/
# target-collision/blocked-disposition mutation fixtures against synthetic
# data, without needing a real workspace build for every fixture. Dot-
# sourcing this file does not invoke cargo or execute the matrix -- that
# only happens via the `Main` guard at the bottom, which is skipped when the
# file is dot-sourced.
#
# What this script proves, in order:
#   1. Every #[ignore]d test in the compiled workspace test harness (built
#      with the same feature set this script itself uses) appears in the
#      tracked inventory under its exact (package, target, function) key,
#      where `target` is the real cargo-metadata-derived target name (never
#      collapsed to `lib` for a non-lib unit-test binary). A newly added
#      ignored test that is missing from the inventory is a hard failure.
#      So is a CSV row whose exact key no longer matches any live test (a
#      stale row -- the test was renamed, moved, or deleted). So is a
#      duplicate exact key within the CSV itself. So is a row whose
#      `classification` is not one of the four closed values this script
#      knows how to handle.
#   2. Every SAFE_LOCAL and SAFE_DB_5434 test executes and passes, one exact
#      (package, target) invocation at a time. MANUAL_EXTERNAL tests are
#      gated behind mqk-daemon's `manual-external` Cargo feature (off in
#      this invocation), so they do not even exist in the compiled binary
#      here. BLOCKED_LOCAL_PREREQUISITE tests are explicitly, exactly
#      `--skip --exact`-excluded from only the one (package, target)
#      invocation they actually belong to -- they are classified as blocked
#      precisely because their local prerequisite is not available, and
#      letting them execute anyway (and either fail loudly or, worse,
#      silently no-op) is a false-green risk this script must not admit by
#      default. Pass `-AuditOnlyIncludeBlocked` to deliberately opt into
#      running them anyway for a one-off audit; the script still prints
#      which exact keys that flag pulled back in. Whenever one or more
#      BLOCKED_LOCAL_PREREQUISITE rows were excluded (the default), the
#      final disposition is `BLOCKED`, exits nonzero, and never prints the
#      green "PASSED" message -- a blocked proof is not a passing proof.
#   3. The 9 MANUAL_EXTERNAL tests still compile when `--features
#      manual-external` is explicitly requested (`--no-run`, never executed)
#      -- proving their source has not silently bit-rotted even though they
#      never run in the default or safe-ignored paths.
#
# Usage:
#   $env:MQK_DATABASE_URL = "postgres://postgres:postgres@127.0.0.1:5434/mqk_test"
#   pwsh -File scripts\windows\Invoke-CanonicalSafeIgnoredMatrix.ps1
#
#   # Narrow Step 1 to specific packages (e.g. for focused/local iteration
#   # instead of a full-workspace enumeration):
#   pwsh -File scripts\windows\Invoke-CanonicalSafeIgnoredMatrix.ps1 `
#       -PackageFilter mqk-db,mqk-daemon -ListOnly
#
# Exit codes: 0 = clean, 1 = any proof above failed.
# =============================================================================

param(
    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path,
    [string]$CargoManifest = (Join-Path $RepoRoot "core-rs\Cargo.toml"),
    # Restrict Step 1's completeness enumeration to these package names
    # instead of every workspace member. Intended for focused/local
    # iteration; omit for the full canonical run.
    [string[]]$PackageFilter = $null,
    # Run only Step 1 (completeness) and skip Step 2 (execution) and Step 3
    # (manual-external compile proof) entirely. Intended for focused/local
    # iteration; omit for the full canonical run.
    [switch]$ListOnly,
    # Deliberately let BLOCKED_LOCAL_PREREQUISITE-classified tests execute
    # in Step 2 for a one-off audit. Off by default: those tests are
    # classified as blocked precisely because their prerequisite is not
    # available locally, so letting them run by default risks a false-green
    # (or false-red) result that has nothing to do with this patch.
    [switch]$AuditOnlyIncludeBlocked
)

$ErrorActionPreference = "Stop"

# The complete, closed set of classification values this script understands.
# Any inventory row carrying anything else is a hard failure -- an unknown
# classification has no defined execution/skip behavior and must not be
# allowed to silently fall through as if it were SAFE_LOCAL.
$script:KnownClassifications = @(
    "SAFE_LOCAL",
    "SAFE_DB_5434",
    "MANUAL_EXTERNAL",
    "BLOCKED_LOCAL_PREREQUISITE"
)

function Resolve-CargoExe {
    $cmd = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cmd) { throw "cargo not found on PATH." }
    return $cmd.Source
}

function Get-InventoryExactKey {
    param($Row)
    return "{0}|{1}|{2}" -f $Row.crate, $Row.target, $Row.function
}

# Pure: validates the inventory rows themselves -- a closed classification
# vocabulary and zero duplicate exact keys. Takes plain objects (as produced
# by Import-Csv, or synthetic ones built in a test) so it never needs to
# read a file or invoke cargo.
function Test-InventorySelfValidation {
    param(
        [Parameter(Mandatory = $true)] $Inventory,
        [Parameter(Mandatory = $true)] [string[]]$KnownClassifications
    )
    $errors = New-Object System.Collections.Generic.List[string]
    $seenKeys = @{}
    foreach ($row in $Inventory) {
        if ($row.classification -notin $KnownClassifications) {
            $errors.Add(
                "unknown classification '$($row.classification)' for $(Get-InventoryExactKey $row) " + `
                "(known: $($KnownClassifications -join ', '))"
            )
            continue
        }
        $key = Get-InventoryExactKey $row
        if ($seenKeys.ContainsKey($key)) {
            $errors.Add("duplicate exact key in inventory: $key")
            continue
        }
        $seenKeys[$key] = $true
    }
    # Unary comma: prevents PowerShell from unrolling an empty/1-element
    # List[string] into zero/one pipeline objects on return -- the caller
    # must always receive one collection object, even when it has 0 items.
    return ,$errors
}

# Pure: given inventory rows and live (already-deduplicated) exact-key rows,
# returns the set of live tests missing from the inventory and the set of
# inventory rows that are stale (no longer correspond to any live test).
# `Narrowed` (true when the live-row scan only covered a subset of packages,
# e.g. via -PackageFilter) suppresses stale-row detection, since a row for
# an un-scanned package cannot honestly be judged stale from a partial scan.
function Get-InventoryCompletenessResult {
    param(
        [Parameter(Mandatory = $true)] $Inventory,
        [Parameter(Mandatory = $true)] $LiveRows,
        [bool]$Narrowed = $false
    )
    $inventoryKeys = @{}
    foreach ($row in $Inventory) {
        $inventoryKeys[(Get-InventoryExactKey $row)] = $true
    }
    $liveKeys = @{}
    foreach ($row in $LiveRows) {
        $liveKeys[$row.Key] = $true
    }

    $missing = @($LiveRows | Where-Object { -not $inventoryKeys.ContainsKey($_.Key) })
    # Deliberately an imperative if/else assigning `$stale` directly in each
    # branch, not `$stale = if (...) {...} else {...}` -- capturing an
    # if-expression's output collapses a zero-item `@()` branch to `$null`
    # rather than an empty array, which is exactly the kind of silent
    # "empty vs. absent" conflation CLAUDE.md's operator-truth discipline
    # forbids elsewhere in this repo.
    if ($Narrowed) {
        $stale = @()
    } else {
        $stale = @($Inventory | Where-Object { -not $liveKeys.ContainsKey((Get-InventoryExactKey $_)) })
    }

    return [pscustomobject]@{
        Missing = $missing
        Stale   = $stale
    }
}

function Get-BlockedTestRows {
    param([Parameter(Mandatory = $true)] $Inventory)
    $rows = @($Inventory | Where-Object { $_.classification -eq "BLOCKED_LOCAL_PREREQUISITE" })
    return ,$rows
}

# Pure: computes the `cargo test ... -- --skip <name>` arguments needed to
# exclude every BLOCKED_LOCAL_PREREQUISITE-classified row from execution.
# Returns an empty array when `AuditOnly` is set -- the caller is
# deliberately opting into running them, so nothing is skipped. Uses
# `$skipArgs` rather than `$args` to avoid shadowing PowerShell's automatic
# `$args` variable.
function Get-SkipArgsForBlockedRows {
    param(
        [Parameter(Mandatory = $true)] $BlockedRows,
        [bool]$AuditOnly = $false
    )
    if ($AuditOnly) { return ,@() }
    $skipArgs = @()
    foreach ($row in $BlockedRows) {
        $skipArgs += @("--skip", $row.function)
    }
    return ,$skipArgs
}

function Get-WorkspaceMetadata {
    param([string]$CargoExe, [string]$CargoManifest)
    $metaLines = & $CargoExe metadata --no-deps --format-version=1 --manifest-path $CargoManifest 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host ($metaLines -join "`n")
        throw "cargo metadata failed (exit $LASTEXITCODE) -- cannot enumerate workspace packages."
    }
    return ($metaLines -join "`n") | ConvertFrom-Json
}

# Pure: given one package's `cargo metadata` `targets` array (each element
# carrying `.kind` -- an array such as @("lib"), @("bin"), @("test"),
# @("example"), @("bench") -- plus `.name` and `.src_path`), builds a list of
# resolved lib/bin targets (real, cargo-metadata-authoritative name and
# kind, plus the source path normalized to forward slashes) that
# `Resolve-UnitTestTarget` matches a `Running unittests <path> (...)`
# header's `<path>` against. This is the "compiler-artifact truth"
# `Get-ExactLiveKeysForPackage` uses to resolve a unit-test header instead of
# guessing "lib" for every one. Returned as a list, not a hashtable keyed by
# the literal path: `cargo metadata`'s `src_path` is always absolute, while
# the path cargo test prints in its own "Running unittests <path> (...)"
# header is relative to the crate directory (e.g. "src/lib.rs") -- an exact
# key match would never hit, so resolution has to be a suffix match instead
# (see `Resolve-UnitTestTarget`).
function Get-TargetLookupFromMetadataTargets {
    param([Parameter(Mandatory = $true)] $MetadataTargets)
    $resolved = New-Object System.Collections.Generic.List[object]
    foreach ($t in $MetadataTargets) {
        $kindStr = @($t.kind)[0]
        $targetKind = switch ($kindStr) {
            "lib" { "Lib" }
            "bin" { "Bin" }
            default { $null }
        }
        if (-not $targetKind) {
            # Only lib/bin targets ever produce a "Running unittests ..."
            # header; "test" (integration-test binaries) and "example" are
            # resolved separately by their own path-stem regex below and
            # need no metadata lookup, and other kinds (bench, custom-build,
            # proc-macro, ...) never appear in `--ignored --list` output at
            # all under this script's fixed `--all-targets` invocation.
            continue
        }
        $normalizedPath = ($t.src_path -replace '\\', '/')
        # A crate's own lib unit-test binary keeps the literal target name
        # "lib" -- the long-standing convention this script's inventory
        # already carries for every one of its existing rows -- rather than
        # cargo metadata's raw lib target `name` (which is the crate name,
        # not "lib"). Only a Bin target's real, distinct name is ever used,
        # since that is exactly the identity that used to be wrongly
        # collapsed into "lib" too.
        $name = if ($targetKind -eq "Lib") { "lib" } else { $t.name }
        $resolved.Add([pscustomobject]@{
            Kind              = $targetKind
            Name              = $name
            NormalizedSrcPath = $normalizedPath
        })
    }
    return ,$resolved
}

# Pure: resolves the `<path>` captured from a `Running unittests <path> (...)`
# header against a `-TargetLookup` list built by
# `Get-TargetLookupFromMetadataTargets`. `cargo metadata`'s `src_path` is
# always absolute; the printed header path is crate-relative -- so this
# matches by exact equality first (covers a caller that already normalized
# both to the same absolute form, e.g. a test fixture) and otherwise by
# path-boundary suffix (the absolute metadata path ending in
# "/<printed path>"), never a bare, boundary-unaware substring match. Returns
# $null, never a guessed default, when nothing matches.
function Resolve-UnitTestTarget {
    param([Parameter(Mandatory = $true)] $TargetLookup, [string]$PrintedPath)
    $normalizedPrinted = ($PrintedPath -replace '\\', '/')
    foreach ($t in $TargetLookup) {
        if ($t.NormalizedSrcPath -eq $normalizedPrinted) { return $t }
        if ($t.NormalizedSrcPath.EndsWith("/$normalizedPrinted")) { return $t }
    }
    return $null
}

# Pure: parses one package's `cargo test -p <pkg> ... -- --ignored --list`
# output into exact (package, target, function) keys. `target`/`TargetKind`
# are derived from the "Running ..." header that precedes each test-binary's
# listing: for a `Running unittests <path> (...)` header (emitted once per
# unit-test binary -- a crate's own `src/lib.rs`, *and* `src/main.rs`, *and*
# every `src/bin/<name>.rs`, each separately), `<path>` is resolved via
# `Resolve-UnitTestTarget` against `-TargetLookup` (built from real `cargo
# metadata` truth by `Get-TargetLookupFromMetadataTargets`) to get the
# target's real name and kind -- never assumed to be `lib`. Integration-test
# binaries (`Running tests[\/]<name>.rs (...)`) use the file stem directly,
# and a `Doc-tests` section (present only if this is ever invoked without
# --all-targets, which excludes doctests) uses the literal name `doctest`.
# Takes plain text lines and a plain lookup list, never invokes cargo
# itself, so a test can feed it synthetic `--list` output and a synthetic
# lookup directly.
function Get-ExactLiveKeysForPackage {
    param(
        [string]$Package,
        [string[]]$ListOutput,
        [Parameter(Mandatory = $true)] $TargetLookup
    )

    $currentTarget = $null
    $currentTargetKind = $null
    $rows = New-Object System.Collections.Generic.List[object]
    $seen = New-Object System.Collections.Generic.HashSet[string]

    foreach ($line in $ListOutput) {
        if ($line -match '^\s*Running unittests (?<path>\S+) \(.+\)\s*$') {
            $resolved = Resolve-UnitTestTarget -TargetLookup $TargetLookup -PrintedPath $matches['path']
            if (-not $resolved) {
                throw "Package '$Package': no cargo-metadata lib/bin target matches unit-test source path '$($matches['path'])' -- the target lookup is out of date or incomplete."
            }
            $currentTarget = $resolved.Name
            $currentTargetKind = $resolved.Kind
            continue
        }
        if ($line -match '^\s*Running (?<relpath>tests[\\/][^\s]+\.rs) \(.+\)\s*$') {
            $stem = [System.IO.Path]::GetFileNameWithoutExtension($matches['relpath'])
            $currentTarget = $stem
            $currentTargetKind = "IntegrationTest"
            continue
        }
        if ($line -match '^\s*Doc-tests \S+\s*$') {
            $currentTarget = "doctest"
            $currentTargetKind = "Doctest"
            continue
        }
        if ($line -match '^(?<name>[A-Za-z0-9_:]+): test$') {
            if (-not $currentTarget) {
                throw "Package '$Package': test listing line encountered before any 'Running ...' header established target context: $line"
            }
            $qualified = $matches['name']
            $key = "{0}|{1}|{2}" -f $Package, $currentTarget, $qualified
            if ($seen.Add($key)) {
                $rows.Add([pscustomobject]@{
                    Package    = $Package
                    Target     = $currentTarget
                    TargetKind = $currentTargetKind
                    Function   = $qualified
                    Key        = $key
                })
            }
        }
    }
    return ,$rows
}

# Pure: maps a live row's TargetKind/Target to the exact cargo target
# selector flag(s) needed to invoke *only* that one compiled test binary --
# the scoping that makes Step 2's per-row `--skip` an exact-identity control
# instead of a workspace-wide substring match.
function Get-TargetSelectorArgs {
    param([string]$TargetKind, [string]$TargetName)
    switch ($TargetKind) {
        "Lib" { return @("--lib") }
        "Bin" { return @("--bin", $TargetName) }
        "IntegrationTest" { return @("--test", $TargetName) }
        default {
            throw "Unsupported TargetKind '$TargetKind' for exact-scoped execution (target '$TargetName') -- doctests are never selected here because this script always runs --all-targets."
        }
    }
}

# Pure: the final pass/fail/blocked disposition, decided purely from already-
# computed exit codes and blocked-row bookkeeping -- no cargo invocation, so
# every combination is directly mutation-testable. `Disposition` is one of
# "PASSED", "BLOCKED", or "FAILED"; `Main` only ever prints the green
# "PASSED: ... is green" message when this returns "PASSED".
function Get-MatrixDisposition {
    param(
        [Parameter(Mandatory = $true)] [int]$SafeExit,
        [Parameter(Mandatory = $true)] [int]$ManualCompileExit,
        [Parameter(Mandatory = $true)] [int]$BlockedCount,
        [Parameter(Mandatory = $true)] [bool]$AuditOnlyIncludeBlocked
    )
    if ($SafeExit -ne 0) {
        return [pscustomobject]@{
            Disposition = "FAILED"
            ExitCode    = 1
            Message     = "Safe-ignored matrix reported failures (see cargo test output above)."
        }
    }
    if ($ManualCompileExit -ne 0) {
        return [pscustomobject]@{
            Disposition = "FAILED"
            ExitCode    = 1
            Message     = "MANUAL_EXTERNAL compile-only proof failed (--features mqk-db/testkit,mqk-daemon/manual-external --no-run)."
        }
    }
    if (($BlockedCount -gt 0) -and (-not $AuditOnlyIncludeBlocked)) {
        return [pscustomobject]@{
            Disposition = "BLOCKED"
            ExitCode    = 1
            Message     = "$BlockedCount BLOCKED_LOCAL_PREREQUISITE test(s) were excluded from this run and never executed -- this is not a green proof. Pass -AuditOnlyIncludeBlocked to deliberately execute and prove them."
        }
    }
    return [pscustomobject]@{
        Disposition = "PASSED"
        ExitCode    = 0
        Message     = "canonical safe-ignored matrix is green."
    }
}

function Main {
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

    # -----------------------------------------------------------------------
    # Step 0: inventory self-validation -- required columns, a closed
    # classification vocabulary, and zero duplicate exact keys.
    # -----------------------------------------------------------------------
    Write-Host ""
    Write-Host "-- Step 0: inventory self-validation --" -ForegroundColor Yellow

    $inventory = Import-Csv -Path $InventoryCsv
    $requiredColumns = @("crate", "target", "file", "line", "function", "classification", "ignore_reason")
    if ($inventory.Count -gt 0) {
        $actualColumns = $inventory[0].PSObject.Properties.Name
        $missingColumns = $requiredColumns | Where-Object { $_ -notin $actualColumns }
        if ($missingColumns.Count -gt 0) {
            throw "Inventory CSV is missing required column(s): $($missingColumns -join ', '). " + `
                "Expected columns: $($requiredColumns -join ', ')."
        }
    }

    $selfValidationErrors = Test-InventorySelfValidation -Inventory $inventory -KnownClassifications $script:KnownClassifications
    if ($selfValidationErrors.Count -gt 0) {
        Write-Host "FAIL: inventory self-validation found $($selfValidationErrors.Count) problem(s):" -ForegroundColor Red
        $selfValidationErrors | ForEach-Object { Write-Host "  $_" -ForegroundColor Red }
        throw "Inventory CSV failed self-validation (duplicate keys and/or unknown classifications) -- fix $InventoryCsv before re-running."
    }
    Write-Host ("OK: {0} inventory rows, all exact keys unique, all classifications known." -f $inventory.Count) -ForegroundColor Green

    # -----------------------------------------------------------------------
    # Step 1: inventory completeness -- every #[ignore]d test in the
    # compiled harness must be present under its exact key, and every
    # inventory row must correspond to a still-live test (no stale rows).
    # -----------------------------------------------------------------------
    Write-Host ""
    Write-Host "-- Step 1: inventory completeness (exact identity) --" -ForegroundColor Yellow

    $meta = Get-WorkspaceMetadata -CargoExe $CargoExe -CargoManifest $CargoManifest
    $allPackageNames = @($meta.packages | ForEach-Object { $_.name } | Sort-Object -Unique)
    $packageTargetsByName = @{}
    foreach ($p in $meta.packages) { $packageTargetsByName[$p.name] = $p.targets }

    $packages = if ($PackageFilter) { $PackageFilter } else { $allPackageNames }
    Write-Host ("Enumerating ignored tests for {0} package(s): {1}" -f $packages.Count, ($packages -join ', '))

    $liveRows = New-Object System.Collections.Generic.List[object]
    $liveKeySet = New-Object System.Collections.Generic.HashSet[string]
    foreach ($pkg in $packages) {
        if (-not $packageTargetsByName.ContainsKey($pkg)) {
            throw "Package '$pkg' not found in cargo metadata -- cannot build its exact target lookup."
        }
        $targetLookup = Get-TargetLookupFromMetadataTargets -MetadataTargets $packageTargetsByName[$pkg]

        $listArgs = @(
            "test", "-p", $pkg, "--manifest-path", $CargoManifest,
            "--features", "mqk-db/testkit", "--all-targets",
            "--", "--ignored", "--list"
        )
        $listOutput = & $CargoExe @listArgs 2>&1
        if ($LASTEXITCODE -ne 0) {
            Write-Host ($listOutput -join "`n")
            throw "cargo test -p $pkg --ignored --list failed (exit $LASTEXITCODE) -- cannot verify inventory completeness."
        }
        foreach ($row in (Get-ExactLiveKeysForPackage -Package $pkg -ListOutput $listOutput -TargetLookup $targetLookup)) {
            if ($liveKeySet.Add($row.Key)) {
                $liveRows.Add($row)
            }
        }
    }

    $completeness = Get-InventoryCompletenessResult -Inventory $inventory -LiveRows $liveRows -Narrowed ([bool]$PackageFilter)

    if ($completeness.Missing.Count -gt 0) {
        Write-Host "FAIL: ignored test(s) exist in the compiled harness but are not in the canonical inventory (exact key):" -ForegroundColor Red
        $completeness.Missing | ForEach-Object { Write-Host "  $($_.Key)" -ForegroundColor Red }
        Write-Host "Classify each in $InventoryCsv (crate,target,file,line,function,classification,ignore_reason) before re-running." -ForegroundColor Red
        throw "Unclassified ignored test(s) found -- inventory is out of date."
    }

    if ($PackageFilter) {
        Write-Host ("OK: all {0} live ignored tests (within the -PackageFilter scope) are present in the canonical inventory." -f $liveRows.Count) -ForegroundColor Green
        Write-Host "NOTE: stale-row detection skipped -- -PackageFilter narrows completeness to a subset of packages." -ForegroundColor DarkYellow
    } else {
        if ($completeness.Stale.Count -gt 0) {
            Write-Host "FAIL: inventory row(s) no longer correspond to any live #[ignore]d test (stale -- renamed, moved, or deleted):" -ForegroundColor Red
            $completeness.Stale | ForEach-Object { Write-Host "  $(Get-InventoryExactKey $_)" -ForegroundColor Red }
            throw "Stale inventory row(s) found -- remove or correct them in $InventoryCsv before re-running."
        }
        Write-Host ("OK: all {0} live ignored tests are present in the canonical inventory, and no inventory row is stale." -f $liveRows.Count) -ForegroundColor Green
    }

    if ($ListOnly) {
        Write-Host ""
        Write-Host "PASSED (list-only): inventory completeness and self-validation are clean." -ForegroundColor Green
        exit 0
    }

    # -----------------------------------------------------------------------
    # Step 2: execute every SAFE_LOCAL + SAFE_DB_5434 test, one exact
    # (package, target) invocation at a time (never a single blanket
    # `--workspace --all-targets` call). MANUAL_EXTERNAL tests are absent
    # from this build (manual-external feature not requested).
    # BLOCKED_LOCAL_PREREQUISITE tests are excluded via `--skip --exact`
    # scoped to only the one (package, target) invocation they actually
    # belong to, unless -AuditOnlyIncludeBlocked.
    # -----------------------------------------------------------------------
    Write-Host ""
    Write-Host "-- Step 2: executing SAFE_LOCAL + SAFE_DB_5434 (exact package+target scoping) --" -ForegroundColor Yellow

    $blockedRows = Get-BlockedTestRows -Inventory $inventory
    $blockedKeySet = New-Object System.Collections.Generic.HashSet[string]
    foreach ($row in $blockedRows) { [void]$blockedKeySet.Add((Get-InventoryExactKey $row)) }
    if ($blockedRows.Count -gt 0) {
        if ($AuditOnlyIncludeBlocked) {
            Write-Host ("AUDIT MODE: {0} BLOCKED_LOCAL_PREREQUISITE test(s) will be allowed to execute:" -f $blockedRows.Count) -ForegroundColor DarkYellow
            $blockedRows | ForEach-Object { Write-Host "  $(Get-InventoryExactKey $_)" -ForegroundColor DarkYellow }
        } else {
            Write-Host ("Excluding {0} BLOCKED_LOCAL_PREREQUISITE test(s) from execution (pass -AuditOnlyIncludeBlocked to include them deliberately):" -f $blockedRows.Count)
            $blockedRows | ForEach-Object { Write-Host "  $(Get-InventoryExactKey $_)" }
        }
    }

    $safeExit = 0
    $targetGroups = @($liveRows | Group-Object -Property Package, Target)
    foreach ($group in $targetGroups) {
        $sample = $group.Group[0]
        $pkg = $sample.Package
        $targetName = $sample.Target
        $targetKind = $sample.TargetKind

        $blockedRowsForTarget = @($group.Group | Where-Object { $blockedKeySet.Contains($_.Key) } | ForEach-Object {
            [pscustomobject]@{ function = $_.Function }
        })
        $skipArgs = Get-SkipArgsForBlockedRows -BlockedRows $blockedRowsForTarget -AuditOnly ([bool]$AuditOnlyIncludeBlocked)

        if (($skipArgs.Count / 2) -eq $group.Group.Count) {
            # Every ignored test in this exact (package, target) is blocked
            # and being skipped -- nothing would actually run here, so skip
            # the invocation entirely rather than spinning up a process for
            # zero effective tests.
            continue
        }

        $selectorArgs = Get-TargetSelectorArgs -TargetKind $targetKind -TargetName $targetName
        $targetArgs = @(
            "test", "-p", $pkg, "--manifest-path", $CargoManifest,
            "--features", "mqk-db/testkit"
        ) + $selectorArgs + @(
            "--no-fail-fast", "--", "--ignored", "--test-threads=1", "--exact"
        ) + $skipArgs
        & $CargoExe @targetArgs
        if ($LASTEXITCODE -ne 0) {
            $safeExit = $LASTEXITCODE
        }
    }

    # -----------------------------------------------------------------------
    # Step 3: MANUAL_EXTERNAL compile-only proof -- never executed. Narrowed
    # to mqk-daemon, the only package with MANUAL_EXTERNAL-classified tests.
    # -----------------------------------------------------------------------
    Write-Host ""
    Write-Host "-- Step 3: MANUAL_EXTERNAL compile-only proof (--no-run) --" -ForegroundColor Yellow

    $manualArgs = @(
        "test", "-p", "mqk-daemon", "--manifest-path", $CargoManifest,
        "--features", "mqk-db/testkit,mqk-daemon/manual-external", "--all-targets", "--no-run"
    )
    & $CargoExe @manualArgs
    $manualCompileExit = $LASTEXITCODE

    # -----------------------------------------------------------------------
    # Summary
    # -----------------------------------------------------------------------
    $safeLocalCount = @($inventory | Where-Object { $_.classification -eq "SAFE_LOCAL" }).Count
    $safeDbCount = @($inventory | Where-Object { $_.classification -eq "SAFE_DB_5434" }).Count
    $manualExternalCount = @($inventory | Where-Object { $_.classification -eq "MANUAL_EXTERNAL" }).Count
    $blockedCount = $blockedRows.Count

    Write-Host ""
    Write-Host "============================================================" -ForegroundColor Cyan
    Write-Host "Canonical safe-ignored matrix summary" -ForegroundColor Cyan
    Write-Host "============================================================" -ForegroundColor Cyan
    Write-Host "SAFE_LOCAL classified:               $safeLocalCount"
    Write-Host "SAFE_DB_5434 classified:              $safeDbCount"
    Write-Host "MANUAL_EXTERNAL classified:           $manualExternalCount (excluded from execution; compile-proof exit=$manualCompileExit)"
    Write-Host "BLOCKED_LOCAL_PREREQUISITE classified: $blockedCount (excluded from execution unless -AuditOnlyIncludeBlocked)"
    Write-Host "Total inventory:                      $($inventory.Count)"
    Write-Host "Safe execution exit code:             $safeExit"
    Write-Host "Manual-external compile exit code:    $manualCompileExit"

    $disposition = Get-MatrixDisposition -SafeExit $safeExit -ManualCompileExit $manualCompileExit `
        -BlockedCount $blockedCount -AuditOnlyIncludeBlocked ([bool]$AuditOnlyIncludeBlocked)

    Write-Host ""
    switch ($disposition.Disposition) {
        "PASSED" {
            Write-Host ("PASSED: canonical safe-ignored matrix is green; inventory complete ({0} tests); " -f $inventory.Count) -NoNewline -ForegroundColor Green
            Write-Host ("{0} MANUAL_EXTERNAL tests excluded from execution and compile-proven." -f $manualExternalCount) -ForegroundColor Green
            exit 0
        }
        "BLOCKED" {
            Write-Host ("BLOCKED: {0}" -f $disposition.Message) -ForegroundColor Yellow
            $blockedRows | ForEach-Object { Write-Host "  $(Get-InventoryExactKey $_)" -ForegroundColor Yellow }
            exit $disposition.ExitCode
        }
        default {
            throw $disposition.Message
        }
    }
}

# Only run the matrix when this file is executed directly (`& script.ps1` or
# `pwsh -File script.ps1`), never when dot-sourced
# (tests/script_guards/test_canonical_safe_ignored_matrix.ps1 dot-sources
# this file to reach the pure functions above without triggering any cargo
# invocation).
if ($MyInvocation.InvocationName -ne '.') {
    Main
}
