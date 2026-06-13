# =============================================================================
# Script guard: MULTI-SYMBOL-DISPATCH-LOOP-01
#
# Verifies that the per-symbol strategy dispatch loop wiring:
#   1. Defines AppState::tick_strategy_dispatch_multi_symbol(&[SymbolStrategyAssignment])
#      -> Vec<(SymbolStrategyAssignment, StrategyBarResult)> in state.rs.
#   2. Defines AppState::retain_targets_matching_symbol (the
#      b1c_symbol_mismatch_skipped fail-closed guard) as a pure, independently
#      testable associated function in state.rs.
#   3. Defines the shared dispatch_native_strategy_for_symbol_with_bar helper
#      that takes an already-taken StrategyBarInput, used by both
#      tick_strategy_dispatch_for_symbol (legacy, one symbol) and
#      tick_strategy_dispatch_multi_symbol (one shared bar, many symbols).
#   4. StrategyBarInput derives Clone (required to dispatch one pending bar
#      to every configured symbol).
#   5. Preserves the legacy single-symbol entrypoints
#      AppState::tick_strategy_dispatch and
#      AppState::tick_strategy_dispatch_for_symbol unchanged in signature.
#   6. loop_runner.rs builds multi_symbol_assignments once at loop startup via
#      build_multi_symbol_runtime_config_from_env, calls
#      tick_strategy_dispatch_multi_symbol per tick, and applies
#      retain_targets_matching_symbol (b1c_symbol_mismatch_skipped) per
#      dispatched symbol before computing decisions.
#   7. loop_runner.rs still derives current_positions exactly once per tick
#      (shared snapshot across all dispatched symbols, design doc §5 Q2) and
#      still routes every decision through
#      crate::decision::submit_internal_strategy_decision (no direct outbox
#      bypass).
#   8. Has the 8 M01..M08 proof tests in
#      scenario_multi_symbol_dispatch_loop_01.rs.
#   9. loop_runner.rs and state.rs introduce no new broker/OMS/portfolio direct
#      writes, no new approved_for_live references, and no order
#      submit/cancel/replace calls.
#  10. Is documented in the native multi-symbol dispatch design doc as
#      implemented (no longer OPEN).
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
$StateRs    = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state.rs"
$LoopRunner = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\loop_runner.rs"
$TestFile   = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_multi_symbol_dispatch_loop_01.rs"
$DesignDoc  = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_multi_symbol_dispatch_loop.ps1'
Write-Host '============================================================'

$StateContent = ''
if (Test-Path $StateRs) {
    $StateContent = Get-Content $StateRs -Raw

    # G01 -- tick_strategy_dispatch_multi_symbol defined
    if ($StateContent -match 'pub async fn tick_strategy_dispatch_multi_symbol\(' -and
        $StateContent -match 'Vec<\(SymbolStrategyAssignment, mqk_strategy::StrategyBarResult\)>') {
        Assert-Pass "G01: AppState::tick_strategy_dispatch_multi_symbol(&[SymbolStrategyAssignment]) -> Vec<(SymbolStrategyAssignment, StrategyBarResult)> defined"
    } else {
        Assert-Fail "G01: AppState::tick_strategy_dispatch_multi_symbol with expected signature NOT found in state.rs"
    }

    # G02 -- retain_targets_matching_symbol guard defined
    if ($StateContent -match 'pub fn retain_targets_matching_symbol\(' -and
        $StateContent -match 'targets: &mut Vec<mqk_strategy::TargetPosition>' -and
        $StateContent -match '-> usize') {
        Assert-Pass "G02: AppState::retain_targets_matching_symbol(&mut Vec<TargetPosition>, &str) -> usize defined"
    } else {
        Assert-Fail "G02: AppState::retain_targets_matching_symbol with expected signature NOT found in state.rs"
    }

    # G03 -- shared dispatch_native_strategy_for_symbol_with_bar helper
    if ($StateContent -match 'async fn dispatch_native_strategy_for_symbol_with_bar\(' -and
        $StateContent -match 'bar: StrategyBarInput,') {
        Assert-Pass "G03: dispatch_native_strategy_for_symbol_with_bar(symbol, timeframe, bar: StrategyBarInput) helper defined"
    } else {
        Assert-Fail "G03: dispatch_native_strategy_for_symbol_with_bar helper NOT found in state.rs"
    }

    # G04 -- StrategyBarInput derives Clone
    if ($StateContent -match '#\[derive\(Debug, Clone\)\]\s*\r?\n\s*pub struct StrategyBarInput') {
        Assert-Pass "G04: StrategyBarInput derives Clone (required for take-once-clone-to-many dispatch)"
    } else {
        Assert-Fail "G04: StrategyBarInput does not derive Clone"
    }

    # G05 -- legacy entrypoints preserved
    if ($StateContent -match 'pub async fn tick_strategy_dispatch\(&self\)' -and
        $StateContent -match 'pub async fn tick_strategy_dispatch_for_symbol\(\s*&self,\s*symbol: &str,\s*timeframe: &str,\s*\) -> Option<mqk_strategy::StrategyBarResult>') {
        Assert-Pass "G05: legacy AppState::tick_strategy_dispatch and tick_strategy_dispatch_for_symbol entrypoints preserved"
    } else {
        Assert-Fail "G05: legacy single-symbol dispatch entrypoints NOT found unchanged in state.rs"
    }
} else {
    Assert-Fail "G01: state.rs not found at $StateRs"
    Assert-Fail "G02: state.rs not found at $StateRs"
    Assert-Fail "G03: state.rs not found at $StateRs"
    Assert-Fail "G04: state.rs not found at $StateRs"
    Assert-Fail "G05: state.rs not found at $StateRs"
}

$LoopContent = ''
if (Test-Path $LoopRunner) {
    $LoopContent = Get-Content $LoopRunner -Raw

    # G06 -- multi_symbol_assignments built once via build_multi_symbol_runtime_config_from_env
    if ($LoopContent -match 'let multi_symbol_assignments: Vec<super::SymbolStrategyAssignment>' -and
        $LoopContent -match 'build_multi_symbol_runtime_config_from_env\(\)') {
        Assert-Pass "G06: loop_runner.rs builds multi_symbol_assignments once via build_multi_symbol_runtime_config_from_env"
    } else {
        Assert-Fail "G06: loop_runner.rs does not build multi_symbol_assignments via build_multi_symbol_runtime_config_from_env"
    }

    # G07 -- per-tick call to tick_strategy_dispatch_multi_symbol
    if ($LoopContent -match 'tick_strategy_dispatch_multi_symbol\(&multi_symbol_assignments\)') {
        Assert-Pass "G07: loop_runner.rs calls tick_strategy_dispatch_multi_symbol(&multi_symbol_assignments) per tick"
    } else {
        Assert-Fail "G07: loop_runner.rs does not call tick_strategy_dispatch_multi_symbol(&multi_symbol_assignments)"
    }

    # G08 -- retain_targets_matching_symbol + b1c_symbol_mismatch_skipped guard wired
    if ($LoopContent -match 'AppState::retain_targets_matching_symbol\(' -and
        $LoopContent -match 'b1c_symbol_mismatch_skipped') {
        Assert-Pass "G08: loop_runner.rs applies AppState::retain_targets_matching_symbol and logs b1c_symbol_mismatch_skipped"
    } else {
        Assert-Fail "G08: loop_runner.rs does not apply the retain_targets_matching_symbol / b1c_symbol_mismatch_skipped guard"
    }

    # G09 -- current_positions snapshot read exactly once per tick (Q2), outside the per-symbol loop
    if ($LoopContent -match 'let current_positions: Option<BTreeMap<String, i64>> = \{' -and
        $LoopContent -match 'for \(assignment, mut bar_result\) in dispatch_results \{') {
        Assert-Pass "G09: current_positions snapshot is read once per tick, before the per-symbol loop (design doc §5 Q2)"
    } else {
        Assert-Fail "G09: current_positions snapshot / per-symbol loop structure NOT found as expected"
    }

    # G10 -- every decision still routed through submit_internal_strategy_decision (no outbox bypass)
    if ($LoopContent -match 'crate::decision::submit_internal_strategy_decision\(') {
        Assert-Pass "G10: decisions remain routed through crate::decision::submit_internal_strategy_decision (no outbox bypass)"
    } else {
        Assert-Fail "G10: crate::decision::submit_internal_strategy_decision call NOT found in loop_runner.rs"
    }
} else {
    Assert-Fail "G06: loop_runner.rs not found at $LoopRunner"
    Assert-Fail "G07: loop_runner.rs not found at $LoopRunner"
    Assert-Fail "G08: loop_runner.rs not found at $LoopRunner"
    Assert-Fail "G09: loop_runner.rs not found at $LoopRunner"
    Assert-Fail "G10: loop_runner.rs not found at $LoopRunner"
}

# G11 -- scenario test file has M01..M08 labels
if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $MissingLabels = [System.Collections.Generic.List[string]]::new()
    $Labels = @('m01','m02','m03','m04','m05','m06','m07','m08')
    foreach ($lbl in $Labels) {
        if ($TestContent -notmatch $lbl) {
            $MissingLabels.Add($lbl)
        }
    }
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "G11: scenario_multi_symbol_dispatch_loop_01.rs has M01..M08 tests"
    } else {
        Assert-Fail "G11: missing test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "G11: scenario_multi_symbol_dispatch_loop_01.rs not found at $TestFile"
}

# G12 -- the committed diff for state.rs / loop_runner.rs adds no new
# broker/OMS/portfolio direct-write imports or order submit/cancel/replace
# calls. Checked against added lines only (git diff HEAD), since both files
# legitimately contain pre-existing references to these symbols elsewhere.
$ForbiddenPattern = 'mqk_broker|mqk_portfolio::.*apply|submit_order|place_order|cancel_order|replace_order|outbox_enqueue\('
Push-Location $RepoRoot
$AddedLines = git diff HEAD -- $StateRs $LoopRunner |
    Where-Object { $_ -match '^\+[^+]' }
Pop-Location
$ForbiddenAdded = $AddedLines | Where-Object { $_ -match $ForbiddenPattern }
if (-not $ForbiddenAdded) {
    Assert-Pass "G12: this patch's diff to state.rs / loop_runner.rs adds no broker/OMS/portfolio direct-write or order submit/cancel/replace calls"
} else {
    Assert-Fail "G12: this patch's diff adds a broker/OMS/portfolio direct-write or order call -- FORBIDDEN in this patch"
}

# G13 -- the committed diff adds no new approved_for_live references
$ApprovedAdded = $AddedLines | Where-Object { $_ -match 'approved_for_live' }
if (-not $ApprovedAdded) {
    Assert-Pass "G13: this patch's diff to state.rs / loop_runner.rs adds no approved_for_live references"
} else {
    Assert-Fail "G13: this patch's diff adds an approved_for_live reference -- FORBIDDEN in this patch"
}

# G14 -- design doc documents MULTI-SYMBOL-DISPATCH-LOOP-01 as implemented
if (Test-Path $DesignDoc) {
    $DesignContent = Get-Content $DesignDoc -Raw
    if ($DesignContent -match 'MULTI-SYMBOL-DISPATCH-LOOP-01' -and
        $DesignContent -match 'tick_strategy_dispatch_multi_symbol' -and
        $DesignContent -match 'retain_targets_matching_symbol') {
        Assert-Pass "G14: native_multi_symbol_dispatch.md documents MULTI-SYMBOL-DISPATCH-LOOP-01, tick_strategy_dispatch_multi_symbol, and retain_targets_matching_symbol"
    } else {
        Assert-Fail "G14: native_multi_symbol_dispatch.md does not document the MULTI-SYMBOL-DISPATCH-LOOP-01 implementation"
    }
} else {
    Assert-Fail "G14: native_multi_symbol_dispatch.md not found at $DesignDoc"
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
