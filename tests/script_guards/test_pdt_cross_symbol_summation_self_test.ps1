# =============================================================================
# Self-test: G14 source-candidate scoping
# PREMARKET-GUARD-UNTRACKED-EVIDENCE-SCOPE-01
#
# Proves that test_pdt_cross_symbol_summation.ps1's G14 check:
#   - never fails on operator evidence (smoke_logs/, exports/, the untracked
#     updated-ledger doc) or on the script_guards tests themselves, even when
#     that evidence contains the literal forbidden strings in explanatory or
#     negative-result text;
#   - still fails when an eligible untracked or tracked source/config file
#     introduces genuine live-authority assignment syntax.
#
# This is a mutation self-test: it creates a disposable shallow clone under
# the OS temporary directory, creates/stages all fixtures only inside that
# clone, and removes the clone after verification. Fixture setup is fail-closed:
# if required test state cannot be created, the case fails before G14 runs.
# The source repository's smoke_logs/, index, and other operator evidence are
# never mutation targets.
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# Exit codes: 0 = all assertions pass, 1 = any assertion failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$Failures = [System.Collections.Generic.List[string]]::new()
$PassCount = 0

function Assert-Pass([string]$Msg) {
    $script:PassCount++
    Write-Host "  OK:   $Msg" -ForegroundColor Green
}

function Assert-Fail([string]$Msg) {
    $script:Failures.Add("  FAIL: $Msg")
    Write-Host "  FAIL: $Msg" -ForegroundColor Red
}

$SourceRepoRoot = (Resolve-Path "$PSScriptRoot\..\..\").Path
$Stamp = [Guid]::NewGuid().ToString('N').Substring(0, 8)
$ScratchRepo = Join-Path $env:TEMP ("mqk-pdt-g14-selftest-" + $Stamp)

if (Test-Path -LiteralPath $ScratchRepo) {
    Write-Host "  FAIL: disposable self-test repo already exists: $ScratchRepo" -ForegroundColor Red
    exit 1
}

git clone --quiet --depth 1 --no-local $SourceRepoRoot $ScratchRepo

if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $ScratchRepo -PathType Container)) {
    Write-Host "  FAIL: could not create disposable self-test repository" -ForegroundColor Red
    exit 1
}

$RepoRoot = $ScratchRepo
$GuardScript = Join-Path $RepoRoot 'tests\script_guards\test_pdt_cross_symbol_summation.ps1'
$ScratchSmokeLogs = Join-Path $RepoRoot 'smoke_logs'

try {
    New-Item `
        -ItemType Directory `
        -Path $ScratchSmokeLogs `
        -Force `
        -ErrorAction Stop |
        Out-Null
}
catch {
    Write-Host "  FAIL: could not create disposable smoke_logs fixture root: $($_.Exception.Message)" -ForegroundColor Red
    Remove-Item -LiteralPath $ScratchRepo -Recurse -Force -ErrorAction SilentlyContinue
    exit 1
}

$SourceIndexBefore = @(
    git -C $SourceRepoRoot diff --cached --name-only
)

Write-Host ''
Write-Host '============================================================'
Write-Host 'Self-test: G14 source-candidate scoping'
Write-Host '============================================================'

function Invoke-GuardG14Line {
    $out = & powershell -NoProfile -ExecutionPolicy Bypass -File $GuardScript 2>&1 | Out-String
    ($out -split "`r?`n") | Where-Object { $_ -match 'G14:' } | Select-Object -First 1
}

function Test-G14Case {
    param(
        [string]$Name,
        [scriptblock]$Setup,
        [scriptblock]$Teardown,
        [bool]$ExpectFail
    )

    $PreviousErrorActionPreference = $ErrorActionPreference

    try {
        # Fixture creation is load-bearing test setup. Any PowerShell setup
        # error must terminate this case before the production guard runs;
        # otherwise a missing fixture can manufacture a false green.
        $ErrorActionPreference = 'Stop'

        & $Setup

        $line = Invoke-GuardG14Line

        if (-not $line) {
            Assert-Fail "$Name -- G14 line not found in guard output"
        }
        else {
            $failed = $line -match 'FAIL'

            if ($failed -eq $ExpectFail) {
                Assert-Pass "$Name (expected FAIL=$ExpectFail, got: $($line.Trim()))"
            }
            else {
                Assert-Fail "$Name -- expected FAIL=$ExpectFail but got: $($line.Trim())"
            }
        }
    }
    catch {
        Assert-Fail "$Name -- setup/guard exception: $($_.Exception.Message)"
    }
    finally {
        try {
            & $Teardown
        }
        catch {
            Assert-Fail "$Name -- teardown exception: $($_.Exception.Message)"
        }

        $ErrorActionPreference = $PreviousErrorActionPreference
    }
}

function Invoke-FixtureGitChecked {
    param(
        [string[]]$GitArguments,
        [string]$FailureMessage
    )

    $PreviousErrorActionPreference = $ErrorActionPreference
    $GitOutput = @()
    $GitExit = $null

    try {
        # Windows PowerShell can promote native stderr to a terminating
        # ErrorRecord when the caller uses ErrorActionPreference=Stop.
        # Capture native stderr under Continue, then fail only on Git's
        # actual process exit status.
        $ErrorActionPreference = 'Continue'

        $GitOutput = @(
            & git @GitArguments 2>&1
        )

        $GitExit = $LASTEXITCODE
    }
    finally {
        $ErrorActionPreference = $PreviousErrorActionPreference
    }

    if ($GitExit -ne 0) {
        $Detail = @(
            $GitOutput |
                ForEach-Object {
                    $_.ToString().Trim()
                } |
                Where-Object { $_ }
        ) -join ' | '

        if ($Detail) {
            throw "$FailureMessage (git exit $GitExit): $Detail"
        }

        throw "$FailureMessage (git exit $GitExit)"
    }
}

# ---------------------------------------------------------------------------
# 1. smoke_logs/ untracked negative text.
# ---------------------------------------------------------------------------
$f1 = Join-Path $RepoRoot "smoke_logs\_g14_selftest_negative_$Stamp.txt"
Test-G14Case -Name "1: smoke_logs negative-text fixture" -ExpectFail $false `
    -Setup { Set-Content -Path $f1 -Value 'OK: no approved_for_live=true / live-routing authority introduced' -NoNewline } `
    -Teardown { Remove-Item -Path $f1 -Force -ErrorAction SilentlyContinue }

# ---------------------------------------------------------------------------
# 2. smoke_logs/ untracked premarket-style log with both forbidden strings
#    in explanatory/negative text.
# ---------------------------------------------------------------------------
$f2 = Join-Path $RepoRoot "smoke_logs\_g14_selftest_premarket_$Stamp.txt"
Test-G14Case -Name "2: smoke_logs premarket-log fixture (both strings)" -ExpectFail $false `
    -Setup {
        @(
            'FINAL: PASS',
            'Guard check: approved_for_live=true was NOT found in tracked or eligible source.',
            'Guard check: live_routing_enabled=true was NOT found in tracked or eligible source.'
        ) | Set-Content -Path $f2
    } `
    -Teardown { Remove-Item -Path $f2 -Force -ErrorAction SilentlyContinue }

# ---------------------------------------------------------------------------
# 3. Canonical master source-candidate contract.
#    The canonical master is tracked operational authority, so G14 must
#    explicitly include it as an eligible root authority file. The former
#    updated-ledger filename must not remain as an exclusion.
# ---------------------------------------------------------------------------
$GuardContent = Get-Content $GuardScript -Raw

$GuardCandidateStart = $GuardContent.IndexOf(
    'function Test-IsGuardSourceCandidate([string]$RelPath) {',
    [System.StringComparison]::Ordinal
)

$GuardCandidateEnd = $GuardContent.IndexOf(
    'Push-Location $RepoRoot',
    $GuardCandidateStart,
    [System.StringComparison]::Ordinal
)

if ($GuardCandidateStart -ge 0 -and $GuardCandidateEnd -gt $GuardCandidateStart) {
    $GuardCandidateSection = $GuardContent.Substring(
        $GuardCandidateStart,
        $GuardCandidateEnd - $GuardCandidateStart
    )

    if (
        $GuardCandidateSection.Contains('MiniQuantDeskV4_Master_Program_Plan_and_Ledger\.md') -and
        -not $GuardCandidateSection.Contains('MiniQuantDesk_Master_Patch_Ledger_v2_updated.md')
    ) {
        Assert-Pass '3: canonical master is explicitly guard-eligible and obsolete ledger exclusion is absent'
    }
    else {
        Assert-Fail '3: canonical master source-candidate contract is not correctly migrated'
    }
}
else {
    Assert-Fail '3: could not locate bounded Test-IsGuardSourceCandidate section in production guard'
}

# ---------------------------------------------------------------------------
# 4. exports/ untracked evidence file.
# ---------------------------------------------------------------------------
$ExportsDir = Join-Path $RepoRoot 'exports'
$f4 = Join-Path $ExportsDir "_g14_selftest_evidence_$Stamp.json"
Test-G14Case -Name "4: exports/ evidence fixture" -ExpectFail $false `
    -Setup {
        if (-not (Test-Path $ExportsDir)) { New-Item -ItemType Directory -Path $ExportsDir -Force | Out-Null }
        Set-Content -Path $f4 -Value '{"note": "approved_for_live=true and live_routing_enabled=true appear only as explanatory evidence text"}'
    } `
    -Teardown { Remove-Item -Path $f4 -Force -ErrorAction SilentlyContinue }

# ---------------------------------------------------------------------------
# 5. Untracked eligible Rust source positive fixture.
# ---------------------------------------------------------------------------
$f5 = Join-Path $RepoRoot "core-rs\_g14_selftest_source_tmp_$Stamp.rs"
Test-G14Case -Name "5: untracked Rust source fixture (approved_for_live: true)" -ExpectFail $true `
    -Setup { Set-Content -Path $f5 -Value 'approved_for_live: true' } `
    -Teardown { Remove-Item -Path $f5 -Force -ErrorAction SilentlyContinue }

# ---------------------------------------------------------------------------
# 6. Untracked eligible PowerShell startup/config fixture.
# ---------------------------------------------------------------------------
$f6 = Join-Path $RepoRoot "scripts\_g14_selftest_startup_tmp_$Stamp.ps1"
Test-G14Case -Name "6: untracked PowerShell fixture (`$live_routing_enabled = `$true)" -ExpectFail $true `
    -Setup { Set-Content -Path $f6 -Value '$live_routing_enabled = $true' } `
    -Teardown { Remove-Item -Path $f6 -Force -ErrorAction SilentlyContinue }

# ---------------------------------------------------------------------------
# 7. Untracked eligible JSON/config fixture.
# ---------------------------------------------------------------------------
$f7 = Join-Path $RepoRoot "config\_g14_selftest_config_tmp_$Stamp.json"
Test-G14Case -Name '7: untracked JSON config fixture ("live_routing_enabled": true)' -ExpectFail $true `
    -Setup { Set-Content -Path $f7 -Value '{ "live_routing_enabled": true }' } `
    -Teardown { Remove-Item -Path $f7 -Force -ErrorAction SilentlyContinue }

# ---------------------------------------------------------------------------
# 8. Tracked (staged) added source line positive fixture.
# ---------------------------------------------------------------------------
$f8 = Join-Path $RepoRoot "core-rs\_g14_selftest_tracked_tmp_$Stamp.rs"
Test-G14Case -Name "8: tracked (staged) source fixture (approved_for_live: true)" -ExpectFail $true `
    -Setup {
        Set-Content -LiteralPath $f8 -Value 'approved_for_live: true' -ErrorAction Stop

        Invoke-FixtureGitChecked `
            -GitArguments @('-C', $RepoRoot, 'add', '--', $f8) `
            -FailureMessage 'git add failed for staged Rust fixture'
    } `
    -Teardown {
        Invoke-FixtureGitChecked `
            -GitArguments @('-C', $RepoRoot, 'restore', '--staged', '--', $f8) `
            -FailureMessage 'git restore --staged failed for Rust fixture'

        if (Test-Path -LiteralPath $f8) {
            Remove-Item -LiteralPath $f8 -Force -ErrorAction Stop
        }
    }

# ---------------------------------------------------------------------------
# 9. Tracked (staged) guard-script-style comment under tests/script_guards/.
#    Proves the tracked-diff scan is also scoped through
#    Test-IsGuardSourceCandidate, so a guard-authoring change describing the
#    forbidden pattern cannot false-trip G14.
# ---------------------------------------------------------------------------
$f9 = Join-Path $RepoRoot "tests\script_guards\_g14_selftest_comment_tmp_$Stamp.ps1"
Test-G14Case -Name "9: tracked (staged) script_guards comment fixture" -ExpectFail $false `
    -Setup {
        Set-Content -LiteralPath $f9 -Value '# no approved_for_live=true / live_routing_enabled=true allowed here' -ErrorAction Stop

        Invoke-FixtureGitChecked `
            -GitArguments @('-C', $RepoRoot, 'add', '--', $f9) `
            -FailureMessage 'git add failed for staged script-guard fixture'
    } `
    -Teardown {
        Invoke-FixtureGitChecked `
            -GitArguments @('-C', $RepoRoot, 'restore', '--staged', '--', $f9) `
            -FailureMessage 'git restore --staged failed for script-guard fixture'

        if (Test-Path -LiteralPath $f9) {
            Remove-Item -LiteralPath $f9 -Force -ErrorAction Stop
        }
    }

# ---------------------------------------------------------------------------
# 10. Clean disposable repository state after the mutation fixtures passes.
# ---------------------------------------------------------------------------
Test-G14Case -Name '10: clean disposable repository state after fixtures' -ExpectFail $false `
    -Setup {} -Teardown {}

# ---------------------------------------------------------------------------
# Cleanup verification.
#
# All mutation state belongs to the disposable clone. The source repository
# index must remain byte-for-byte outside this self-test's authority.
# ---------------------------------------------------------------------------
$Leftover = git -C $RepoRoot status --porcelain=v1 --untracked-files=all 2>$null |
    Where-Object { $_ -match '_g14_selftest_' }

if (-not $Leftover) {
    Assert-Pass "cleanup: no _g14_selftest_ fixtures left in disposable git status"
}
else {
    Assert-Fail "cleanup: leftover disposable self-test fixtures detected: $($Leftover -join ' | ')"
}

$SourceIndexAfter = @(
    git -C $SourceRepoRoot diff --cached --name-only
)

$SourceIndexDelta = @(
    Compare-Object `
        -ReferenceObject $SourceIndexBefore `
        -DifferenceObject $SourceIndexAfter
)

if ($SourceIndexDelta.Count -eq 0) {
    Assert-Pass "cleanup: source repository index unchanged"
}
else {
    Assert-Fail "cleanup: source repository index changed during self-test"
}

try {
    Remove-Item `
        -LiteralPath $ScratchRepo `
        -Recurse `
        -Force `
        -ErrorAction Stop

    Assert-Pass "cleanup: disposable self-test repository removed"
}
catch {
    Assert-Fail "cleanup: could not remove disposable self-test repository: $($_.Exception.Message)"
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host '============================================================'
$Total = $PassCount + $Failures.Count
if ($Failures.Count -eq 0) {
    Write-Host "All $Total assertions passed." -ForegroundColor Green
    exit 0
} else {
    Write-Host "$($Failures.Count) of $Total assertion(s) failed:" -ForegroundColor Red
    $Failures | ForEach-Object { Write-Host $_ -ForegroundColor Red }
    exit 1
}
