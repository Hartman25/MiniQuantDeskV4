# =============================================================================
# Script guard: MULTI-SYMBOL-CAPITAL-CAPS-01
#
# Verifies caps #2, #3, and #5 (design doc §6, "Patch 6"):
#
#   Cap #2 (per_symbol_max_position_qty, MQK_PER_SYMBOL_MAX_POSITION_QTY):
#   1. signal_intake.rs defines per_symbol_max_position_qty_from_env()
#      -> Option<i64> reading MQK_PER_SYMBOL_MAX_POSITION_QTY (filter > 0).
#   2. signal_intake.rs defines per_symbol_max_position_qty,
#      set_per_symbol_max_position_qty_for_test, and the cap #2 alert-dedup
#      helper try_claim_per_symbol_position_cap_alert; state.rs re-exports
#      try_claim_per_symbol_position_cap_alert_for_test.
#   3. state.rs defines fields per_symbol_max_position_qty
#      (Arc<RwLock<Option<i64>>>) and per_symbol_position_cap_alerted_symbols
#      (Arc<RwLock<HashSet<String>>>), both initialized in new_inner.
#   4. state.rs defines the pure clamp_targets_to_per_symbol_position_cap
#      helper (sign-preserving, |qty| > cap only).
#   5. loop_runner.rs wires the cap #2 clamp + Discord alert into the B1C loop
#      (b1c_target_qty_clamped_per_symbol_cap).
#   6. reset_signal_blocked_alert_state clears
#      per_symbol_position_cap_alerted_symbols at run start.
#
#   Cap #3 (per_symbol_max_notional_usd, MQK_PER_SYMBOL_MAX_NOTIONAL_USD):
#   7. position_sizing.rs defines PositionSizingOutcome::SizingDeniedPerSymbolCap
#      plus evaluate_per_symbol_notional_cap and
#      evaluate_per_symbol_notional_cap_from_env (reading
#      MQK_PER_SYMBOL_MAX_NOTIONAL_USD).
#   8. capital_policy/mod.rs re-exports both cap #3 evaluator functions.
#   9. decision.rs inserts Gate 1g between Gate 1e (capital_budget) and Gate 2
#      (db_present), producing disposition "rejected" on
#      SizingDeniedPerSymbolCap.
#  10. decision.rs module-doc gate sequence documents Gate 1g
#      (per_symbol_notional).
#
#   Cap #5 (aggregate_gross_exposure_cap_usd, MQK_AGGREGATE_GROSS_EXPOSURE_CAP_USD):
#  11. portfolio_risk.rs evaluate_portfolio_risk takes a 5th parameter
#      aggregate_gross_exposure_cap_usd: Option<f64>, and
#      evaluate_portfolio_risk_from_env reads
#      MQK_AGGREGATE_GROSS_EXPOSURE_CAP_USD.
#
#   Tests + scope:
#  12. scenario_multi_symbol_capital_caps_01.rs has the C2/C3/C5 proof tests.
#  13. This patch's diff introduces no broker/OMS/portfolio direct writes, no
#      order submit/cancel/replace calls, no approved_for_live references, and
#      no MultiSymbolRiskCaps struct (out of scope for this patch).
#  14. Design doc documents MULTI-SYMBOL-CAPITAL-CAPS-01 (caps #2/#3/#5,
#      Patch 6) as CLOSED.
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

$RepoRoot       = (Resolve-Path "$PSScriptRoot\..\..\").Path
$StateRs        = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state.rs"
$SignalIntake   = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\signal_intake.rs"
$LoopRunnerRs   = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\loop_runner.rs"
$DecisionRs     = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\decision.rs"
$PositionSizing = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\capital_policy\position_sizing.rs"
$PortfolioRisk  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\capital_policy\portfolio_risk.rs"
$CapitalPolicy  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\capital_policy\mod.rs"
$TestFile       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_multi_symbol_capital_caps_01.rs"
$DesignDoc      = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_multi_symbol_capital_caps.ps1'
Write-Host '============================================================'

# G01 -- per_symbol_max_position_qty_from_env reads MQK_PER_SYMBOL_MAX_POSITION_QTY
if (Test-Path $SignalIntake) {
    $SignalContent = Get-Content $SignalIntake -Raw

    if ($SignalContent -match 'fn per_symbol_max_position_qty_from_env\(\)\s*->\s*Option<i64>' -and
        $SignalContent -match 'MQK_PER_SYMBOL_MAX_POSITION_QTY' -and
        $SignalContent -match '\.filter\(\|&n\| n > 0\)') {
        Assert-Pass "G01: per_symbol_max_position_qty_from_env() -> Option<i64> reads MQK_PER_SYMBOL_MAX_POSITION_QTY (filter > 0)"
    } else {
        Assert-Fail "G01: per_symbol_max_position_qty_from_env() reading MQK_PER_SYMBOL_MAX_POSITION_QTY NOT found in signal_intake.rs"
    }

    # G02 -- cap #2 accessor/mutator/alert-dedup methods present
    $RequiredFns = @(
        'pub async fn per_symbol_max_position_qty\(&self\) -> Option<i64>',
        'pub fn set_per_symbol_max_position_qty_for_test\(&self, cap: Option<i64>\)',
        'pub\(crate\) async fn try_claim_per_symbol_position_cap_alert\(&self, symbol: &str\) -> bool'
    )
    $MissingFns = @($RequiredFns | Where-Object { $SignalContent -notmatch $_ })
    if ($MissingFns.Count -eq 0) {
        Assert-Pass "G02: cap #2 accessor/mutator/alert-dedup methods defined on AppState (signal_intake.rs)"
    } else {
        Assert-Fail "G02: missing cap #2 methods in signal_intake.rs: $($MissingFns -join ' | ')"
    }

    # G06 -- reset_signal_blocked_alert_state clears per_symbol_position_cap_alerted_symbols
    $ResetIdx = $SignalContent.IndexOf('fn reset_signal_blocked_alert_state')
    if ($ResetIdx -ge 0) {
        $ResetBlock = $SignalContent.Substring($ResetIdx, [Math]::Min(700, $SignalContent.Length - $ResetIdx))
        if ($ResetBlock -match 'per_symbol_position_cap_alerted_symbols') {
            Assert-Pass "G06: reset_signal_blocked_alert_state clears per_symbol_position_cap_alerted_symbols at run start"
        } else {
            Assert-Fail "G06: reset_signal_blocked_alert_state does NOT clear per_symbol_position_cap_alerted_symbols"
        }
    } else {
        Assert-Fail "G06: reset_signal_blocked_alert_state not found in signal_intake.rs"
    }
} else {
    Assert-Fail "G01: signal_intake.rs not found at $SignalIntake"
    Assert-Fail "G02: signal_intake.rs not found at $SignalIntake"
    Assert-Fail "G06: signal_intake.rs not found at $SignalIntake"
}

# G03 -- cap #2 state fields defined and initialized; state.rs re-exports the
# alert-dedup test seam.
if (Test-Path $StateRs) {
    $StateContent = Get-Content $StateRs -Raw

    if ($StateContent -match 'per_symbol_max_position_qty:\s*Arc<RwLock<Option<i64>>>' -and
        $StateContent -match 'per_symbol_position_cap_alerted_symbols:\s*Arc<RwLock<HashSet<String>>>' -and
        $StateContent -match 'per_symbol_max_position_qty:\s*Arc::new\(RwLock::new\(' -and
        $StateContent -match 'per_symbol_max_position_qty_from_env\(\)' -and
        $StateContent -match 'per_symbol_position_cap_alerted_symbols:\s*Arc::new\(RwLock::new\(HashSet::new\(\)\)\)' -and
        $StateContent -match 'pub async fn try_claim_per_symbol_position_cap_alert_for_test\(&self, symbol: &str\) -> bool') {
        Assert-Pass "G03: cap #2 state fields defined/initialized; try_claim_per_symbol_position_cap_alert_for_test re-exported"
    } else {
        Assert-Fail "G03: cap #2 state fields / test-seam re-export NOT found (as expected) in state.rs"
    }

    # G04 -- pure clamp helper, sign-preserving, |qty| > cap only
    if ($StateContent -match 'pub fn clamp_targets_to_per_symbol_position_cap\(\s*targets:\s*&mut \[mqk_strategy::TargetPosition\],\s*cap:\s*i64,?\s*\)\s*->\s*Vec<\(String,\s*i64,\s*i64\)>' -and
        $StateContent -match 't\.qty\.abs\(\)\s*>\s*cap' -and
        $StateContent -match 'if t\.qty < 0 \{ -cap \} else \{ cap \}') {
        Assert-Pass "G04: clamp_targets_to_per_symbol_position_cap defined, sign-preserving, |qty| > cap only"
    } else {
        Assert-Fail "G04: clamp_targets_to_per_symbol_position_cap NOT found with the expected sign-preserving clamp logic"
    }
} else {
    Assert-Fail "G03: state.rs not found at $StateRs"
    Assert-Fail "G04: state.rs not found at $StateRs"
}

# G05 -- loop_runner.rs wires cap #2 clamp + Discord alert into the B1C loop
if (Test-Path $LoopRunnerRs) {
    $LoopContent = Get-Content $LoopRunnerRs -Raw

    if ($LoopContent -match 'state_arc\.per_symbol_max_position_qty\(\)\.await' -and
        $LoopContent -match 'AppState::clamp_targets_to_per_symbol_position_cap\(' -and
        $LoopContent -match 'try_claim_per_symbol_position_cap_alert\(&symbol\)' -and
        $LoopContent -match 'b1c_target_qty_clamped_per_symbol_cap') {
        Assert-Pass "G05: loop_runner.rs wires cap #2 clamp + alert into the B1C loop (b1c_target_qty_clamped_per_symbol_cap)"
    } else {
        Assert-Fail "G05: cap #2 clamp + alert wiring NOT found in loop_runner.rs B1C loop"
    }
} else {
    Assert-Fail "G05: loop_runner.rs not found at $LoopRunnerRs"
}

# G07 -- cap #3 evaluator + outcome variant in position_sizing.rs
if (Test-Path $PositionSizing) {
    $SizingContent = Get-Content $PositionSizing -Raw

    if ($SizingContent -match 'SizingDeniedPerSymbolCap\s*\{' -and
        $SizingContent -match 'pub fn evaluate_per_symbol_notional_cap\(' -and
        $SizingContent -match 'pub fn evaluate_per_symbol_notional_cap_from_env\(' -and
        $SizingContent -match 'MQK_PER_SYMBOL_MAX_NOTIONAL_USD') {
        Assert-Pass "G07: position_sizing.rs defines SizingDeniedPerSymbolCap, evaluate_per_symbol_notional_cap(_from_env), reads MQK_PER_SYMBOL_MAX_NOTIONAL_USD"
    } else {
        Assert-Fail "G07: cap #3 evaluator / outcome variant NOT found in position_sizing.rs"
    }
} else {
    Assert-Fail "G07: position_sizing.rs not found at $PositionSizing"
}

# G08 -- capital_policy/mod.rs re-exports cap #3 evaluators
if (Test-Path $CapitalPolicy) {
    $CapPolicyContent = Get-Content $CapitalPolicy -Raw

    if ($CapPolicyContent -match 'evaluate_per_symbol_notional_cap' -and
        $CapPolicyContent -match 'evaluate_per_symbol_notional_cap_from_env') {
        Assert-Pass "G08: capital_policy/mod.rs re-exports evaluate_per_symbol_notional_cap(_from_env)"
    } else {
        Assert-Fail "G08: capital_policy/mod.rs does NOT re-export cap #3 evaluators"
    }
} else {
    Assert-Fail "G08: capital_policy/mod.rs not found at $CapitalPolicy"
}

# G09/G10 -- decision.rs Gate 1g inserted between Gate 1e and Gate 2; module
# doc documents Gate 1g.
if (Test-Path $DecisionRs) {
    $DecisionContent = Get-Content $DecisionRs -Raw

    $Gate1eIdx = $DecisionContent.IndexOf('evaluate_strategy_budget_from_env(&sid)')
    $Gate1gIdx = $DecisionContent.IndexOf('evaluate_per_symbol_notional_cap_from_env(')
    $Gate2Idx  = $DecisionContent.IndexOf('// Gate 2: DB must be present.')

    if ($Gate1eIdx -ge 0 -and $Gate1gIdx -gt $Gate1eIdx -and $Gate2Idx -gt $Gate1gIdx -and
        $DecisionContent -match 'SizingDeniedPerSymbolCap' -and
        $DecisionContent -match '"rejected"') {
        Assert-Pass "G09: decision.rs Gate 1g (evaluate_per_symbol_notional_cap_from_env -> SizingDeniedPerSymbolCap -> 'rejected') sits between Gate 1e and Gate 2"
    } else {
        Assert-Fail "G09: Gate 1g NOT found in expected position (between Gate 1e and Gate 2) in decision.rs"
    }

    if ($DecisionContent -match '1g\.\s*per_symbol_notional') {
        Assert-Pass "G10: decision.rs module doc gate sequence documents Gate 1g (per_symbol_notional)"
    } else {
        Assert-Fail "G10: decision.rs module doc gate sequence does NOT document Gate 1g"
    }
} else {
    Assert-Fail "G09: decision.rs not found at $DecisionRs"
    Assert-Fail "G10: decision.rs not found at $DecisionRs"
}

# G11 -- portfolio_risk.rs cap #5: 5th parameter + env var
if (Test-Path $PortfolioRisk) {
    $RiskContent = Get-Content $PortfolioRisk -Raw

    if ($RiskContent -match 'pub fn evaluate_portfolio_risk\(' -and
        $RiskContent -match 'aggregate_gross_exposure_cap_usd:\s*Option<f64>' -and
        $RiskContent -match 'MQK_AGGREGATE_GROSS_EXPOSURE_CAP_USD') {
        Assert-Pass "G11: evaluate_portfolio_risk takes aggregate_gross_exposure_cap_usd: Option<f64>; evaluate_portfolio_risk_from_env reads MQK_AGGREGATE_GROSS_EXPOSURE_CAP_USD"
    } else {
        Assert-Fail "G11: cap #5 aggregate_gross_exposure_cap_usd parameter / env var NOT found in portfolio_risk.rs"
    }
} else {
    Assert-Fail "G11: portfolio_risk.rs not found at $PortfolioRisk"
}

# G12 -- scenario test file has the C2/C3/C5 proof tests
if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $MissingLabels = [System.Collections.Generic.List[string]]::new()
    $Labels = @(
        'c2_01', 'c2_02', 'c2_03', 'c2_04', 'c2_05', 'c2_06', 'c2_07', 'c2_08',
        'c3_01', 'c3_02', 'c3_03', 'c3_04', 'c3_05', 'c3_06',
        'c5_01', 'c5_02', 'c5_03', 'c5_04', 'c5_05'
    )
    foreach ($lbl in $Labels) {
        if ($TestContent -notmatch $lbl) {
            $MissingLabels.Add($lbl)
        }
    }
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "G12: scenario_multi_symbol_capital_caps_01.rs has all C2/C3/C5 proof tests"
    } else {
        Assert-Fail "G12: missing test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "G12: scenario_multi_symbol_capital_caps_01.rs not found at $TestFile"
}

# G13 -- this patch's diff introduces no broker/OMS/portfolio direct writes, no
# order submit/cancel/replace calls, no approved_for_live references, and no
# MultiSymbolRiskCaps struct (out of scope for this patch). Checked against
# added lines only (git diff HEAD), since some files legitimately contain
# pre-existing references to these symbols elsewhere.
$DiffPaths = @($StateRs, $SignalIntake, $LoopRunnerRs, $DecisionRs, $PositionSizing, $PortfolioRisk, $CapitalPolicy)
Push-Location $RepoRoot
$AddedLines = git diff HEAD -- $DiffPaths |
    Where-Object { $_ -match '^\+[^+]' }
Pop-Location

$ForbiddenPattern = 'mqk_broker|mqk_portfolio::.*apply|submit_order|place_order|cancel_order|replace_order|outbox_enqueue\(|MultiSymbolRiskCaps|approved_for_live'
$ForbiddenAdded = $AddedLines | Where-Object { $_ -match $ForbiddenPattern }
if (-not $ForbiddenAdded) {
    Assert-Pass "G13: this patch's diff adds no broker/OMS/portfolio writes, order calls, MultiSymbolRiskCaps, or approved_for_live references"
} else {
    Assert-Fail "G13: this patch's diff adds an out-of-scope reference -- FORBIDDEN in this patch: $($ForbiddenAdded -join ' | ')"
}

# G14 -- design doc documents MULTI-SYMBOL-CAPITAL-CAPS-01 (caps #2/#3/#5,
# Patch 6) as CLOSED.
if (Test-Path $DesignDoc) {
    $DesignContent = Get-Content $DesignDoc -Raw
    if ($DesignContent -match 'MULTI-SYMBOL-CAPITAL-CAPS-01' -and
        $DesignContent -match 'per_symbol_max_position_qty' -and
        $DesignContent -match 'per_symbol_max_notional_usd' -and
        $DesignContent -match 'aggregate_gross_exposure_cap_usd' -and
        $DesignContent -match '\*\*CLOSED\*\*') {
        Assert-Pass "G14: native_multi_symbol_dispatch.md documents MULTI-SYMBOL-CAPITAL-CAPS-01 (caps #2/#3/#5, Patch 6) as CLOSED"
    } else {
        Assert-Fail "G14: native_multi_symbol_dispatch.md does NOT document MULTI-SYMBOL-CAPITAL-CAPS-01 as CLOSED"
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
