# =============================================================================
# Script guard: PER-SYMBOL-BAR-WINDOW-01
#
# Verifies that the additive per-symbol bar input/window foundation:
#   1. Exists at core-rs/crates/mqk-daemon/src/state/per_symbol_bar_window.rs.
#   2. Defines PerSymbolBarInput { symbol, now_tick, end_ts, limit_price, qty }.
#   3. Defines PerSymbolBarWindow { symbol, timeframe, bars_loaded,
#      latest_end_ts, latest_end_utc, is_stale }.
#   4. Defines PerSymbolLoadedBars { window, bars }.
#   5. Defines PerSymbolPendingBarInputs with insert/take (symbol-keyed
#      pending input map).
#   6. Defines classify_bar_staleness (future cap #9 helper).
#   7. Defines load_recent_completed_bars_for_symbol_window +
#      per_symbol_loaded_bars_from_rows (bar-window loading/conversion).
#   8. Is registered in state.rs (mod + pub use) but NOT invoked from
#      loop_runner.rs or routes/strategy.rs (no dispatch wiring).
#   9. Adds AppState::tick_strategy_dispatch_for_symbol while preserving
#      AppState::tick_strategy_dispatch (legacy entrypoint unchanged).
#  10. Has no broker/OMS/execution/portfolio/risk/reconcile imports and no
#      order enqueue/submission calls.
#  11. Introduces no approved_for_live anywhere in the new module.
#  12. Has the 14 P01..P14 proof tests in
#      scenario_per_symbol_bar_window_01.rs.
#  13. Is documented in the design doc and scanner/watchlist spec.
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

$RepoRoot      = (Resolve-Path "$PSScriptRoot\..\..\").Path
$ModuleFile    = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\per_symbol_bar_window.rs"
$StateRs       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state.rs"
$LoopRunner    = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\loop_runner.rs"
$StrategyRoute = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\strategy.rs"
$TestFile      = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_per_symbol_bar_window_01.rs"
$DesignDoc     = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"
$SpecDoc       = Join-Path $RepoRoot "docs\specs\market_scanner_backtest_candidate_pipeline.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_per_symbol_bar_window.ps1'
Write-Host '============================================================'

# P-G01 -- per_symbol_bar_window.rs exists
if (Test-Path $ModuleFile) {
    Assert-Pass "P-G01: per_symbol_bar_window.rs exists"
} else {
    Assert-Fail "P-G01: per_symbol_bar_window.rs not found at $ModuleFile"
}

$ModuleContent = ''
if (Test-Path $ModuleFile) {
    $ModuleContent = Get-Content $ModuleFile -Raw
}

# P-G02 -- PerSymbolBarInput { symbol, now_tick, end_ts, limit_price, qty }
if ($ModuleContent -match 'struct PerSymbolBarInput' -and
    $ModuleContent -match 'pub symbol:\s*String' -and
    $ModuleContent -match 'pub now_tick:\s*u64' -and
    $ModuleContent -match 'pub end_ts:\s*i64' -and
    $ModuleContent -match 'pub limit_price:\s*Option<i64>' -and
    $ModuleContent -match 'pub qty:\s*i64') {
    Assert-Pass "P-G02: PerSymbolBarInput { symbol, now_tick, end_ts, limit_price, qty } present"
} else {
    Assert-Fail "P-G02: PerSymbolBarInput with expected fields not found"
}

# P-G03 -- PerSymbolBarWindow { symbol, timeframe, bars_loaded, latest_end_ts, latest_end_utc, is_stale }
if ($ModuleContent -match 'struct PerSymbolBarWindow' -and
    $ModuleContent -match 'pub timeframe:\s*String' -and
    $ModuleContent -match 'pub bars_loaded:\s*usize' -and
    $ModuleContent -match 'pub latest_end_ts:\s*Option<i64>' -and
    $ModuleContent -match 'pub latest_end_utc:\s*Option<String>' -and
    $ModuleContent -match 'pub is_stale:\s*Option<bool>') {
    Assert-Pass "P-G03: PerSymbolBarWindow { symbol, timeframe, bars_loaded, latest_end_ts, latest_end_utc, is_stale } present"
} else {
    Assert-Fail "P-G03: PerSymbolBarWindow with expected fields not found"
}

# P-G04 -- PerSymbolLoadedBars { window, bars }
if ($ModuleContent -match 'struct PerSymbolLoadedBars' -and
    $ModuleContent -match 'pub window:\s*PerSymbolBarWindow' -and
    $ModuleContent -match 'pub bars:\s*Vec<mqk_strategy::BarStub>') {
    Assert-Pass "P-G04: PerSymbolLoadedBars { window, bars } present"
} else {
    Assert-Fail "P-G04: PerSymbolLoadedBars with expected fields not found"
}

# P-G05 -- PerSymbolPendingBarInputs with insert/take (symbol-keyed pending map)
if ($ModuleContent -match 'struct PerSymbolPendingBarInputs' -and
    $ModuleContent -match 'pub fn insert\(' -and
    $ModuleContent -match 'pub fn take\(' -and
    $ModuleContent -match 'pub fn normalize_symbol\(') {
    Assert-Pass "P-G05: PerSymbolPendingBarInputs { insert, take, normalize_symbol } present"
} else {
    Assert-Fail "P-G05: PerSymbolPendingBarInputs with insert/take/normalize_symbol not found"
}

# P-G06 -- classify_bar_staleness helper (future cap #9)
if ($ModuleContent -match 'pub fn classify_bar_staleness\(') {
    Assert-Pass "P-G06: classify_bar_staleness helper present"
} else {
    Assert-Fail "P-G06: classify_bar_staleness helper not found"
}

# P-G07 -- load_recent_completed_bars_for_symbol_window + per_symbol_loaded_bars_from_rows
if ($ModuleContent -match 'pub async fn load_recent_completed_bars_for_symbol_window\(' -and
    $ModuleContent -match 'pub fn per_symbol_loaded_bars_from_rows\(') {
    Assert-Pass "P-G07: load_recent_completed_bars_for_symbol_window + per_symbol_loaded_bars_from_rows present"
} else {
    Assert-Fail "P-G07: load_recent_completed_bars_for_symbol_window / per_symbol_loaded_bars_from_rows not found"
}

# P-G08 / P-G09 -- registered in state.rs (mod + pub use re-export)
$StateContent = ''
if (Test-Path $StateRs) {
    $StateContent = Get-Content $StateRs -Raw

    if ($StateContent -match 'mod per_symbol_bar_window;') {
        Assert-Pass "P-G08: state.rs declares 'mod per_symbol_bar_window;'"
    } else {
        Assert-Fail "P-G08: state.rs does not declare 'mod per_symbol_bar_window;'"
    }

    if ($StateContent -match 'pub use per_symbol_bar_window::\{') {
        Assert-Pass "P-G09: state.rs re-exports per_symbol_bar_window items via 'pub use per_symbol_bar_window::{ ... }'"
    } else {
        Assert-Fail "P-G09: state.rs does not re-export per_symbol_bar_window items"
    }

    # P-G10 -- tick_strategy_dispatch_for_symbol added
    if ($StateContent -match 'pub async fn tick_strategy_dispatch_for_symbol\(') {
        Assert-Pass "P-G10: AppState::tick_strategy_dispatch_for_symbol defined in state.rs"
    } else {
        Assert-Fail "P-G10: AppState::tick_strategy_dispatch_for_symbol NOT found in state.rs"
    }

    # P-G11 -- legacy tick_strategy_dispatch entrypoint preserved
    if ($StateContent -match 'pub async fn tick_strategy_dispatch\(&self\)') {
        Assert-Pass "P-G11: legacy AppState::tick_strategy_dispatch(&self) entrypoint preserved in state.rs"
    } else {
        Assert-Fail "P-G11: legacy AppState::tick_strategy_dispatch(&self) entrypoint NOT found in state.rs"
    }
} else {
    Assert-Fail "P-G08: state.rs not found at $StateRs"
    Assert-Fail "P-G09: state.rs not found at $StateRs"
    Assert-Fail "P-G10: state.rs not found at $StateRs"
    Assert-Fail "P-G11: state.rs not found at $StateRs"
}

# P-G12 -- loop_runner.rs has zero references to new per-symbol types/helpers
$NewSymbolPattern = 'per_symbol_bar_window|PerSymbolBarInput|PerSymbolBarWindow|PerSymbolLoadedBars|PerSymbolPendingBarInputs|classify_bar_staleness|load_recent_completed_bars_for_symbol_window|per_symbol_loaded_bars_from_rows|tick_strategy_dispatch_for_symbol'
if (Test-Path $LoopRunner) {
    if (-not (Select-String -Path $LoopRunner -Pattern $NewSymbolPattern -Quiet)) {
        Assert-Pass "P-G12: loop_runner.rs has no per-symbol-bar-window references -- dispatch NOT wired"
    } else {
        Assert-Fail "P-G12: loop_runner.rs references per-symbol-bar-window helpers -- dispatch wiring FORBIDDEN in this patch"
    }
} else {
    Assert-Fail "P-G12: loop_runner.rs not found at $LoopRunner"
}

# P-G13 -- routes/strategy.rs has zero references to new per-symbol types/helpers
if (Test-Path $StrategyRoute) {
    if (-not (Select-String -Path $StrategyRoute -Pattern $NewSymbolPattern -Quiet)) {
        Assert-Pass "P-G13: routes/strategy.rs has no per-symbol-bar-window references -- signal path NOT wired"
    } else {
        Assert-Fail "P-G13: routes/strategy.rs references per-symbol-bar-window helpers -- signal-path wiring FORBIDDEN in this patch"
    }
} else {
    Assert-Fail "P-G13: routes/strategy.rs not found at $StrategyRoute"
}

# P-G14 -- new module has no broker/OMS/execution/portfolio/risk/reconcile imports
if ($ModuleContent -notmatch 'mqk_execution|mqk_broker|mqk_portfolio|mqk_risk|mqk_reconcile|mqk_audit') {
    Assert-Pass "P-G14: per_symbol_bar_window.rs has no broker/OMS/execution/portfolio/risk/reconcile/audit imports"
} else {
    Assert-Fail "P-G14: per_symbol_bar_window.rs references broker/OMS/execution/portfolio/risk/reconcile/audit -- FORBIDDEN in this patch"
}

# P-G15 -- new module has no order enqueue/submission calls
if ($ModuleContent -notmatch 'submit_order|place_order|outbox_enqueue|enqueue_order|cancel_order|replace_order') {
    Assert-Pass "P-G15: per_symbol_bar_window.rs has no order enqueue/submission calls"
} else {
    Assert-Fail "P-G15: per_symbol_bar_window.rs contains an order enqueue/submission call -- FORBIDDEN in this patch"
}

# P-G16 -- new module does not introduce approved_for_live
if ($ModuleContent -notmatch 'approved_for_live') {
    Assert-Pass "P-G16: per_symbol_bar_window.rs does not introduce approved_for_live"
} else {
    Assert-Fail "P-G16: per_symbol_bar_window.rs references approved_for_live -- FORBIDDEN in this patch"
}

# P-G17 -- scenario test file has P01..P14 labels
if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $MissingLabels = [System.Collections.Generic.List[string]]::new()
    $Labels = @('p01','p02','p03','p04','p05','p06','p07','p08','p09','p10','p11','p12','p13','p14')
    foreach ($lbl in $Labels) {
        if ($TestContent -notmatch $lbl) {
            $MissingLabels.Add($lbl)
        }
    }
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "P-G17: scenario_per_symbol_bar_window_01.rs has P01..P14 tests"
    } else {
        Assert-Fail "P-G17: missing test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "P-G17: scenario_per_symbol_bar_window_01.rs not found at $TestFile"
}

# P-G18 -- design doc references PER-SYMBOL-BAR-WINDOW-01
if (Test-Path $DesignDoc) {
    if (Select-String -Path $DesignDoc -Pattern 'PER-SYMBOL-BAR-WINDOW-01' -Quiet) {
        Assert-Pass "P-G18: PER-SYMBOL-BAR-WINDOW-01 referenced in native_multi_symbol_dispatch.md"
    } else {
        Assert-Fail "P-G18: PER-SYMBOL-BAR-WINDOW-01 not referenced in native_multi_symbol_dispatch.md"
    }
} else {
    Assert-Fail "P-G18: native_multi_symbol_dispatch.md not found at $DesignDoc"
}

# P-G19 -- scanner/watchlist spec doc references PER-SYMBOL-BAR-WINDOW-01
if (Test-Path $SpecDoc) {
    if (Select-String -Path $SpecDoc -Pattern 'PER-SYMBOL-BAR-WINDOW-01' -Quiet) {
        Assert-Pass "P-G19: PER-SYMBOL-BAR-WINDOW-01 referenced in market_scanner_backtest_candidate_pipeline.md"
    } else {
        Assert-Fail "P-G19: PER-SYMBOL-BAR-WINDOW-01 not referenced in market_scanner_backtest_candidate_pipeline.md"
    }
} else {
    Assert-Fail "P-G19: market_scanner_backtest_candidate_pipeline.md not found at $SpecDoc"
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
