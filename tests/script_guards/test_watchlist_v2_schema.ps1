# =============================================================================
# Script guard: WATCHLIST-V2-SCHEMA-01
#
# Verifies that the watchlist-v2 multi-symbol schema layer:
#   1. Defines watchlist-v1 / watchlist-v2 schema version constants and the
#      MULTI_SYMBOL_HARD_CEILING cap (=5).
#   2. Extends LoadedWatchlistArtifact with schema_version.
#   3. Extends WatchlistStatusResponse with schema_version (additive).
#   4. Defines all new stable v2 failure-reason strings.
#   5. Preserves the approved_for_live=false hard live-lock invariant.
#   6. Does NOT wire multi-symbol dispatch into loop_runner.rs, state.rs, or
#      the strategy signal route (schema/validation only).
#   7. Has the 12 new W21..W32 proof tests.
#   8. Is documented in the design doc and scanner/watchlist spec.
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

$RepoRoot        = (Resolve-Path "$PSScriptRoot\..\..\").Path
$WatchlistIntake = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\watchlist_intake.rs"
$WatchlistRoute  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\watchlist.rs"
$ApiTypes        = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\api_types.rs"
$LoopRunner      = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\loop_runner.rs"
$StateRs         = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state.rs"
$StrategyRoute   = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\strategy.rs"
$TestFile        = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_watchlist_readonly_handoff_01.rs"
$DesignDoc       = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"
$SpecDoc         = Join-Path $RepoRoot "docs\specs\market_scanner_backtest_candidate_pipeline.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_watchlist_v2_schema.ps1'
Write-Host '============================================================'

# V-G01 -- watchlist_intake.rs exists
if (Test-Path $WatchlistIntake) {
    Assert-Pass "V-G01: watchlist_intake.rs exists"
} else {
    Assert-Fail "V-G01: watchlist_intake.rs not found at $WatchlistIntake"
}

$IntakeContent = Get-Content $WatchlistIntake -Raw

# V-G02 -- WATCHLIST_SCHEMA_VERSION_V1 == "watchlist-v1"
if ($IntakeContent -match 'WATCHLIST_SCHEMA_VERSION_V1:\s*&str\s*=\s*"watchlist-v1"') {
    Assert-Pass "V-G02: WATCHLIST_SCHEMA_VERSION_V1 = \"watchlist-v1\" defined"
} else {
    Assert-Fail "V-G02: WATCHLIST_SCHEMA_VERSION_V1 = \"watchlist-v1\" not found in watchlist_intake.rs"
}

# V-G03 -- WATCHLIST_SCHEMA_VERSION_V2 == "watchlist-v2"
if ($IntakeContent -match 'WATCHLIST_SCHEMA_VERSION_V2:\s*&str\s*=\s*"watchlist-v2"') {
    Assert-Pass "V-G03: WATCHLIST_SCHEMA_VERSION_V2 = \"watchlist-v2\" defined"
} else {
    Assert-Fail "V-G03: WATCHLIST_SCHEMA_VERSION_V2 = \"watchlist-v2\" not found in watchlist_intake.rs"
}

# V-G04 -- MULTI_SYMBOL_HARD_CEILING == 5
if ($IntakeContent -match 'MULTI_SYMBOL_HARD_CEILING:\s*u64\s*=\s*5') {
    Assert-Pass "V-G04: MULTI_SYMBOL_HARD_CEILING = 5 defined"
} else {
    Assert-Fail "V-G04: MULTI_SYMBOL_HARD_CEILING = 5 not found in watchlist_intake.rs"
}

# V-G05 -- LoadedWatchlistArtifact carries schema_version: String
if ($IntakeContent -match 'pub schema_version:\s*String') {
    Assert-Pass "V-G05: LoadedWatchlistArtifact.schema_version: String present"
} else {
    Assert-Fail "V-G05: schema_version field not found on LoadedWatchlistArtifact"
}

# V-G06 -- WatchlistStatusResponse carries schema_version: Option<String> (additive)
if (Test-Path $ApiTypes) {
    if (Select-String -Path $ApiTypes -Pattern 'pub schema_version:\s*Option<String>' -Quiet) {
        Assert-Pass "V-G06: WatchlistStatusResponse.schema_version: Option<String> present in api_types.rs"
    } else {
        Assert-Fail "V-G06: schema_version: Option<String> not found in api_types.rs"
    }
} else {
    Assert-Fail "V-G06: api_types.rs not found at $ApiTypes"
}

# V-G07..V-G16 -- all new stable v2 failure-reason strings present
$ReasonStrings = @(
    'watchlist_schema_invalid',
    'watchlist_mode_not_paper',
    'watchlist_live_approval_forbidden',
    'watchlist_approved_for_autonomous_paper_invalid',
    'watchlist_symbols_invalid',
    'watchlist_strategy_assignments_invalid',
    'watchlist_strategy_assignment_missing',
    'watchlist_max_symbols_invalid',
    'watchlist_max_concurrent_positions_invalid',
    'watchlist_multi_symbol_ceiling_exceeded'
)
$ReasonIdx = 7
foreach ($reason in $ReasonStrings) {
    if ($IntakeContent -match [regex]::Escape($reason)) {
        Assert-Pass "V-G$ReasonIdx`: failure-reason '$reason' present in watchlist_intake.rs"
    } else {
        Assert-Fail "V-G$ReasonIdx`: failure-reason '$reason' NOT found in watchlist_intake.rs"
    }
    $ReasonIdx++
}

# V-G17 -- approved_for_live: false hard lock still enforced in routes/watchlist.rs
if (Test-Path $WatchlistRoute) {
    $RouteContent = Get-Content $WatchlistRoute -Raw
    if ($RouteContent -match 'approved_for_live:\s*false') {
        Assert-Pass "V-G17: approved_for_live: false still enforced in routes/watchlist.rs"
    } else {
        Assert-Fail "V-G17: approved_for_live: false not found in routes/watchlist.rs -- live lock missing"
    }

    # V-G18 -- approved_for_live: true never assigned
    if (-not ($RouteContent -match 'approved_for_live:\s*true')) {
        Assert-Pass "V-G18: approved_for_live: true is never assigned in routes/watchlist.rs"
    } else {
        Assert-Fail "V-G18: approved_for_live: true found in routes/watchlist.rs -- hard live lock violated"
    }
} else {
    Assert-Fail "V-G17: routes/watchlist.rs not found at $WatchlistRoute"
    Assert-Fail "V-G18: routes/watchlist.rs not found at $WatchlistRoute"
}

# V-G19 -- "ALWAYS false" / hard live-lock invariant documented for v2 too
if ($IntakeContent -match 'ALWAYS false' -and $IntakeContent -match 'v1 and v2') {
    Assert-Pass "V-G19: hard live-lock invariant documented for v1 and v2"
} else {
    Assert-Fail "V-G19: hard live-lock invariant not clearly documented for v1 and v2"
}

# V-G20 -- loop_runner.rs not touched for multi-symbol dispatch (schema-only patch)
if (Test-Path $LoopRunner) {
    if (-not (Select-String -Path $LoopRunner -Pattern 'MULTI_SYMBOL_HARD_CEILING|watchlist-v2|WATCHLIST_SCHEMA_VERSION_V2' -Quiet)) {
        Assert-Pass "V-G20: loop_runner.rs has no watchlist-v2 / multi-symbol dispatch wiring"
    } else {
        Assert-Fail "V-G20: loop_runner.rs references watchlist-v2 / MULTI_SYMBOL_HARD_CEILING -- runtime dispatch FORBIDDEN in this patch"
    }
} else {
    Assert-Fail "V-G20: loop_runner.rs not found at $LoopRunner"
}

# V-G21 -- state.rs strategy dispatch not touched for multi-symbol dispatch
if (Test-Path $StateRs) {
    if (-not (Select-String -Path $StateRs -Pattern 'MULTI_SYMBOL_HARD_CEILING|watchlist-v2|WATCHLIST_SCHEMA_VERSION_V2' -Quiet)) {
        Assert-Pass "V-G21: state.rs has no watchlist-v2 / multi-symbol dispatch wiring"
    } else {
        Assert-Fail "V-G21: state.rs references watchlist-v2 / MULTI_SYMBOL_HARD_CEILING -- runtime dispatch FORBIDDEN in this patch"
    }
} else {
    Assert-Fail "V-G21: state.rs not found at $StateRs"
}

# V-G22 -- strategy signal route still does not reference watchlist admission/v2
if (Test-Path $StrategyRoute) {
    if (-not (Select-String -Path $StrategyRoute -Pattern 'evaluate_watchlist_signal_admission|watchlist-v2|MULTI_SYMBOL_HARD_CEILING' -Quiet)) {
        Assert-Pass "V-G22: strategy signal route has no watchlist-v2 admission wiring"
    } else {
        Assert-Fail "V-G22: strategy.rs references watchlist-v2 / admission -- enforcement FORBIDDEN in this patch"
    }
} else {
    Assert-Fail "V-G22: strategy.rs not found at $StrategyRoute"
}

# V-G23 -- Test file has W21..W32 labels
if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $MissingLabels = [System.Collections.Generic.List[string]]::new()
    $Labels = @('w21','w22','w23','w24','w25','w26','w27','w28','w29','w30','w31','w32')
    foreach ($lbl in $Labels) {
        if ($TestContent -notmatch $lbl) {
            $MissingLabels.Add($lbl)
        }
    }
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "V-G23: scenario_watchlist_readonly_handoff_01.rs has W21..W32 tests"
    } else {
        Assert-Fail "V-G23: missing test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "V-G23: scenario_watchlist_readonly_handoff_01.rs not found at $TestFile"
}

# V-G24 -- design doc references WATCHLIST-V2-SCHEMA-01
if (Test-Path $DesignDoc) {
    if (Select-String -Path $DesignDoc -Pattern 'WATCHLIST-V2-SCHEMA-01' -Quiet) {
        Assert-Pass "V-G24: WATCHLIST-V2-SCHEMA-01 referenced in native_multi_symbol_dispatch.md"
    } else {
        Assert-Fail "V-G24: WATCHLIST-V2-SCHEMA-01 not referenced in native_multi_symbol_dispatch.md"
    }
} else {
    Assert-Fail "V-G24: native_multi_symbol_dispatch.md not found at $DesignDoc"
}

# V-G25 -- scanner/watchlist spec doc references watchlist-v2
if (Test-Path $SpecDoc) {
    if (Select-String -Path $SpecDoc -Pattern 'watchlist-v2' -Quiet) {
        Assert-Pass "V-G25: watchlist-v2 referenced in market_scanner_backtest_candidate_pipeline.md"
    } else {
        Assert-Fail "V-G25: watchlist-v2 not referenced in market_scanner_backtest_candidate_pipeline.md"
    }
} else {
    Assert-Fail "V-G25: market_scanner_backtest_candidate_pipeline.md not found at $SpecDoc"
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
