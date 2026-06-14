# =============================================================================
# Script guard: PDT-CROSS-SYMBOL-SUMMATION-PROOF-01 (Patch 11C)
#
# Verifies cap #13 (design doc §6, "Cap #13 -- pdt_round_trip_awareness"):
#
#   G01: focused PDT cross-symbol proof test file exists
#        (core-rs/crates/mqk-risk/tests/scenario_pdt_cross_symbol_summation_01.rs).
#   G02: proof mentions at least two symbols, AAPL and MSFT.
#   G03: proof represents 2 day trades on one symbol and 2 day trades on
#        another symbol.
#   G04: proof asserts total account day trades equals 4.
#   G05: proof asserts the account-wide PDT rule/threshold trips at the
#        cross-symbol total (PDT-X03, cross.flagged_pdt).
#   G06: proof asserts the below-threshold mixed-symbol case does not trip
#        (PDT-X05, !s.flagged_pdt).
#   G07: proof asserts PDT is not per-symbolized (PDT-X09).
#   G08: proof contains PDT-X01 through PDT-X10 labels.
#   G09: no broker/OMS/execution/daemon runtime files touched.
#   G10: no GUI files touched.
#   G11: no research-py watchlist promotion files touched.
#   G12: no smoke/evidence scripts touched.
#   G13: no DB migrations or DB write paths touched.
#   G14: no approved_for_live=true / live-routing authority introduced.
#   G15: docs mark PDT-CROSS-SYMBOL-SUMMATION-PROOF-01 as CLOSED and keep
#        MULTI-SYMBOL-PAPER-SMOKE-END-TO-END-01 open.
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
$ProofTest  = Join-Path $RepoRoot "core-rs\crates\mqk-risk\tests\scenario_pdt_cross_symbol_summation_01.rs"
$DesignDoc  = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_pdt_cross_symbol_summation.ps1'
Write-Host '============================================================'

# G01 -- focused PDT cross-symbol proof test file exists.
if (Test-Path $ProofTest) {
    Assert-Pass "G01: scenario_pdt_cross_symbol_summation_01.rs exists at $ProofTest"
    $ProofContent = Get-Content $ProofTest -Raw
} else {
    Assert-Fail "G01: scenario_pdt_cross_symbol_summation_01.rs NOT found at $ProofTest"
    $ProofContent = ''
}

if ($ProofContent) {
    # G02 -- proof mentions AAPL and MSFT.
    if ($ProofContent -match 'AAPL' -and $ProofContent -match 'MSFT') {
        Assert-Pass "G02: proof mentions both AAPL and MSFT"
    } else {
        Assert-Fail "G02: proof does NOT mention both AAPL and MSFT"
    }

    # G03 -- proof represents 2 day trades on one symbol and 2 on another.
    if ($ProofContent -match 'AAPL day trade #1' -and $ProofContent -match 'AAPL day trade #2' -and
        $ProofContent -match 'MSFT day trade #1' -and $ProofContent -match 'MSFT day trade #2') {
        Assert-Pass "G03: proof represents 2 AAPL day trades + 2 MSFT day trades"
    } else {
        Assert-Fail "G03: proof does NOT represent 2 day trades on AAPL and 2 on MSFT"
    }

    # G04 -- proof asserts total account day trades equals 4.
    if ($ProofContent -match 'assert_eq!\(\s*total,\s*4') {
        Assert-Pass "G04: proof asserts total account day trades == 4"
    } else {
        Assert-Fail "G04: proof does NOT assert total account day trades == 4"
    }

    # G05 -- proof asserts the account-wide PDT rule trips at the
    # cross-symbol total (PDT-X03, cross.flagged_pdt).
    if ($ProofContent -match 'PDT-X03' -and
        $ProofContent -match 'assert!\(\s*cross\.flagged_pdt') {
        Assert-Pass "G05: proof asserts account-wide PDT rule trips at the cross-symbol total (PDT-X03, cross.flagged_pdt)"
    } else {
        Assert-Fail "G05: proof does NOT assert PDT trips at the cross-symbol total"
    }

    # G06 -- proof asserts the below-threshold mixed-symbol case does not
    # trip (PDT-X05, !s.flagged_pdt).
    if ($ProofContent -match 'PDT-X05' -and
        $ProofContent -match '!s\.flagged_pdt') {
        Assert-Pass "G06: proof asserts below-threshold mixed-symbol case does not trip PDT (PDT-X05, !s.flagged_pdt)"
    } else {
        Assert-Fail "G06: proof does NOT assert the below-threshold mixed-symbol case stays untripped"
    }

    # G07 -- proof asserts PDT is not per-symbolized (PDT-X09).
    if ($ProofContent -match 'PDT-X09' -and
        $ProofContent -match 'no_per_symbol_pdt_limit_introduced') {
        Assert-Pass "G07: proof asserts PDT is not per-symbolized (PDT-X09, no_per_symbol_pdt_limit_introduced)"
    } else {
        Assert-Fail "G07: proof does NOT assert PDT is not per-symbolized"
    }

    # G08 -- proof contains PDT-X01 through PDT-X10 labels.
    $MissingLabels = [System.Collections.Generic.List[string]]::new()
    for ($i = 1; $i -le 10; $i++) {
        $lbl = "PDT-X{0:D2}" -f $i
        if ($ProofContent -notmatch [regex]::Escape($lbl)) {
            $MissingLabels.Add($lbl)
        }
    }
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "G08: proof contains PDT-X01 through PDT-X10 labels"
    } else {
        Assert-Fail "G08: proof is missing labels: $($MissingLabels -join ', ')"
    }
} else {
    foreach ($g in @('G02','G03','G04','G05','G06','G07','G08')) {
        Assert-Fail "${g}: skipped (proof test file not found)"
    }
}

# ---------------------------------------------------------------------------
# G09-G14: scope checks against changed files (uncommitted + staged).
# Working tree is clean both before this patch's edits are staged (untracked
# new files show in `git status`) and after commit (empty diff, vacuously
# passes).
# ---------------------------------------------------------------------------
Push-Location $RepoRoot
$ChangedFiles = @()
$ChangedFiles += (git diff --name-only HEAD 2>$null)
$ChangedFiles += (git status --porcelain=v1 --untracked-files=all 2>$null |
    ForEach-Object { $_.Substring(3).Trim() })
Pop-Location
$ChangedFiles = $ChangedFiles | Where-Object { $_ } | Select-Object -Unique

# G09 -- no broker/OMS/execution/daemon runtime files touched.
$ForbiddenRuntime = $ChangedFiles | Where-Object {
    $_ -match '^core-rs/crates/mqk-broker-' -or
    $_ -match '^core-rs/crates/mqk-execution/' -or
    $_ -match '^core-rs/crates/mqk-reconcile/' -or
    $_ -match '^core-rs/crates/mqk-portfolio/' -or
    $_ -match '^core-rs/crates/mqk-daemon/src/' -or
    $_ -match '^core-rs/crates/mqk-risk/src/engine\.rs$'
}
if (-not $ForbiddenRuntime) {
    Assert-Pass "G09: no broker/OMS/execution/daemon runtime files touched"
} else {
    Assert-Fail "G09: out-of-scope runtime files touched: $($ForbiddenRuntime -join ', ')"
}

# G10 -- no GUI files touched.
$ForbiddenGui = $ChangedFiles | Where-Object { $_ -match '^core-rs/mqk-gui/' }
if (-not $ForbiddenGui) {
    Assert-Pass "G10: no GUI files touched"
} else {
    Assert-Fail "G10: GUI files touched: $($ForbiddenGui -join ', ')"
}

# G11 -- no research-py watchlist promotion files touched.
$ForbiddenResearch = $ChangedFiles | Where-Object { $_ -match '^research-py/' }
if (-not $ForbiddenResearch) {
    Assert-Pass "G11: no research-py watchlist promotion files touched"
} else {
    Assert-Fail "G11: research-py files touched: $($ForbiddenResearch -join ', ')"
}

# G12 -- no smoke/evidence scripts touched.
$ForbiddenSmoke = $ChangedFiles | Where-Object {
    $_ -match '^scripts/windows/' -or
    $_ -match 'PaperSmokeEvidence' -or
    $_ -match 'multi_symbol_smoke_evidence'
}
if (-not $ForbiddenSmoke) {
    Assert-Pass "G12: no smoke/evidence scripts touched"
} else {
    Assert-Fail "G12: smoke/evidence scripts touched: $($ForbiddenSmoke -join ', ')"
}

# G13 -- no DB migrations or DB write paths touched.
$ForbiddenDb = $ChangedFiles | Where-Object {
    $_ -match '^core-rs/crates/mqk-db/' -or
    $_ -match 'migrations/'
}
if (-not $ForbiddenDb) {
    Assert-Pass "G13: no DB migrations or DB write paths touched"
} else {
    Assert-Fail "G13: DB files touched: $($ForbiddenDb -join ', ')"
}

# G14 -- no approved_for_live=true / live-routing authority introduced.
# Checked against ADDED lines only (git diff HEAD) plus full content of new
# untracked files, so historical prose elsewhere in the repo discussing
# approved_for_live cannot trip this guard.
Push-Location $RepoRoot
$AddedLines = @()
$AddedLines += (git diff HEAD 2>$null | Where-Object { $_ -match '^\+[^+]' })
# Exclude script_guards files themselves -- guard scripts describe forbidden
# patterns (e.g. "no approved_for_live=true") in comments, which would
# otherwise false-positive against this check.
$Untracked = git status --porcelain=v1 --untracked-files=all 2>$null |
    Where-Object { $_ -match '^\?\?' } |
    ForEach-Object { $_.Substring(3).Trim() } |
    Where-Object { $_ -notmatch '^tests/script_guards/' }
foreach ($f in $Untracked) {
    $full = Join-Path $RepoRoot $f
    if (Test-Path $full -PathType Leaf) {
        $AddedLines += (Get-Content $full -ErrorAction SilentlyContinue)
    }
}
Pop-Location

$LiveAuthorityPattern = 'approved_for_live\s*[:=]\s*(true|True)|live_routing_enabled\s*[:=]\s*(true|True)'
$LiveAuthorityHits = $AddedLines | Where-Object { $_ -match $LiveAuthorityPattern }
if (-not $LiveAuthorityHits) {
    Assert-Pass "G14: no approved_for_live=true / live-routing authority introduced"
} else {
    Assert-Fail "G14: live-routing authority introduced: $($LiveAuthorityHits -join ' | ')"
}

# G15 -- docs mark PDT-CROSS-SYMBOL-SUMMATION-PROOF-01 as CLOSED and keep
# MULTI-SYMBOL-PAPER-SMOKE-END-TO-END-01 open.
if (Test-Path $DesignDoc) {
    $DesignContent = Get-Content $DesignDoc -Raw
    $ClosedOk = $DesignContent -match 'PDT-CROSS-SYMBOL-SUMMATION-PROOF-01[\s\S]{0,80}CLOSED' -or
                $DesignContent -match 'CLOSED[\s\S]{0,80}PDT-CROSS-SYMBOL-SUMMATION-PROOF-01'
    $SmokeOpenOk = $DesignContent -match 'MULTI-SYMBOL-PAPER-SMOKE-END-TO-END-01[\s\S]{0,120}(OPEN|open|next)'
    if ($ClosedOk -and $SmokeOpenOk) {
        Assert-Pass "G15: design doc marks PDT-CROSS-SYMBOL-SUMMATION-PROOF-01 CLOSED and keeps MULTI-SYMBOL-PAPER-SMOKE-END-TO-END-01 open"
    } else {
        Assert-Fail "G15: design doc does NOT correctly reflect PDT-CROSS-SYMBOL-SUMMATION-PROOF-01 CLOSED / MULTI-SYMBOL-PAPER-SMOKE-END-TO-END-01 open"
    }
} else {
    Assert-Fail "G15: design doc not found at $DesignDoc"
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
