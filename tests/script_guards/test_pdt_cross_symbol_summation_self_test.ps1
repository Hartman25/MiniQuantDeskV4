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
# This is a mutation self-test: it creates small, uniquely-named temporary
# fixtures (and, for the tracked cases, stages them with `git add`) and
# always removes/unstages them in `finally`. It never touches the real
# smoke_logs/ evidence, the real updated-ledger file, or any other tracked
# content.
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

$RepoRoot   = (Resolve-Path "$PSScriptRoot\..\..\").Path
$GuardScript = Join-Path $PSScriptRoot 'test_pdt_cross_symbol_summation.ps1'
$Stamp = [Guid]::NewGuid().ToString('N').Substring(0, 8)

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
    try {
        & $Setup
        $line = Invoke-GuardG14Line
        if (-not $line) {
            Assert-Fail "$Name -- G14 line not found in guard output"
            return
        }
        $failed = $line -match 'FAIL'
        if ($failed -eq $ExpectFail) {
            Assert-Pass "$Name (expected FAIL=$ExpectFail, got: $($line.Trim()))"
        } else {
            Assert-Fail "$Name -- expected FAIL=$ExpectFail but got: $($line.Trim())"
        }
    } catch {
        Assert-Fail "$Name -- exception: $($_.Exception.Message)"
    } finally {
        & $Teardown
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
# 3. The real untracked updated-ledger file. Read-only: it already exists as
#    real operator evidence and already contains the forbidden strings; this
#    proves G14 passes against the real file as-is. No fixture is created or
#    removed.
# ---------------------------------------------------------------------------
$LedgerFile = Join-Path $RepoRoot 'MiniQuantDesk_Master_Patch_Ledger_v2_updated.md'
if (Test-Path $LedgerFile) {
    $LedgerContent = Get-Content $LedgerFile -Raw
    if ($LedgerContent -match 'approved_for_live' -or $LedgerContent -match 'live_routing_enabled') {
        Test-G14Case -Name "3: real untracked updated-ledger file (contains forbidden strings)" -ExpectFail $false `
            -Setup {} -Teardown {}
    } else {
        Assert-Pass "3: real untracked updated-ledger file present but contains no forbidden strings (nothing to prove); skipping as vacuous"
    }
} else {
    Assert-Pass "3: real untracked updated-ledger file not present in this run; skipping as vacuous"
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
        Set-Content -Path $f8 -Value 'approved_for_live: true'
        git -C $RepoRoot add -- $f8 2>&1 | Out-Null
    } `
    -Teardown {
        git -C $RepoRoot reset -- $f8 2>&1 | Out-Null
        Remove-Item -Path $f8 -Force -ErrorAction SilentlyContinue
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
        Set-Content -Path $f9 -Value '# no approved_for_live=true / live_routing_enabled=true allowed here'
        git -C $RepoRoot add -- $f9 2>&1 | Out-Null
    } `
    -Teardown {
        git -C $RepoRoot reset -- $f9 2>&1 | Out-Null
        Remove-Item -Path $f9 -Force -ErrorAction SilentlyContinue
    }

# ---------------------------------------------------------------------------
# 10. Existing clean repository state (real smoke_logs/ + real untracked
#     ledger file present, no fixtures) passes.
# ---------------------------------------------------------------------------
Test-G14Case -Name '10: existing repo state with real untracked evidence present' -ExpectFail $false `
    -Setup {} -Teardown {}

# ---------------------------------------------------------------------------
# Cleanup verification -- confirm no self-test fixtures or git index changes
# were left behind.
# ---------------------------------------------------------------------------
$Leftover = git -C $RepoRoot status --porcelain=v1 --untracked-files=all 2>$null |
    Where-Object { $_ -match '_g14_selftest_' }
if (-not $Leftover) {
    Assert-Pass "cleanup: no _g14_selftest_ fixtures left in git status"
} else {
    Assert-Fail "cleanup: leftover self-test fixtures detected: $($Leftover -join ' | ')"
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
