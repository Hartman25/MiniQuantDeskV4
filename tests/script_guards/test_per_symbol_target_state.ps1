# =============================================================================
# Script guard: PER-SYMBOL-TARGET-STATE-01
#
# Verifies the Patch 8 in-memory, observability-only per-symbol target state.
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

$RepoRoot    = (Resolve-Path "$PSScriptRoot\..\..\").Path
$StateRs     = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state.rs"
$LoopRunner  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\loop_runner.rs"
$TestFile    = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_per_symbol_target_state_01.rs"
$DesignDoc   = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_per_symbol_target_state.ps1'
Write-Host '============================================================'

$StateContent = ''
if (Test-Path $StateRs) {
    $StateContent = Get-Content $StateRs -Raw

    $RequiredFields = @(
        'pub symbol: String',
        'pub strategy_id: String',
        'pub current_qty: i64',
        'pub target_qty: i64',
        'pub delta: i64',
        'pub no_order_reason: String',
        'pub last_decision_id: Option<String>',
        'pub last_decision_disposition: Option<String>',
        'pub updated_at_utc: String'
    )
    $MissingFields = @($RequiredFields | Where-Object { $StateContent -notmatch [regex]::Escape($_) })
    if ($StateContent -match 'pub struct PerSymbolTargetState' -and $MissingFields.Count -eq 0) {
        Assert-Pass "G01: PerSymbolTargetState struct exists with required fields"
    } else {
        Assert-Fail "G01: PerSymbolTargetState missing or fields absent: $($MissingFields -join ', ')"
    }

    if ($StateContent -match 'per_symbol_target_state:\s*Arc<RwLock<BTreeMap<String,\s*PerSymbolTargetState>>>' -and
        $StateContent -match 'per_symbol_target_state:\s*Arc::new\(RwLock::new\(BTreeMap::new\(\)\)\)') {
        Assert-Pass "G02: AppState has per_symbol_target_state BTreeMap field and constructor initialization"
    } else {
        Assert-Fail "G02: AppState per_symbol_target_state field or initialization missing"
    }

    if ($StateContent -match 'pub async fn record_per_symbol_target_state\(&self, mut state: PerSymbolTargetState\)') {
        Assert-Pass "G03: record_per_symbol_target_state method exists"
    } else {
        Assert-Fail "G03: record_per_symbol_target_state method missing"
    }

    if ($StateContent -match 'pub async fn per_symbol_target_states\(&self\) -> Vec<PerSymbolTargetState>') {
        Assert-Pass "G04: per_symbol_target_states method exists"
    } else {
        Assert-Fail "G04: per_symbol_target_states method missing"
    }

    if ($StateContent -match 'pub async fn clear_per_symbol_target_states\(&self\)') {
        Assert-Pass "G05: clear_per_symbol_target_states method exists"
    } else {
        Assert-Fail "G05: clear_per_symbol_target_states method missing"
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

    if ($LoopContent -match 'record_per_symbol_target_state' -and
        $LoopContent -match 'build_per_symbol_target_state' -and
        $LoopContent -match 'per_symbol_target_state_for_symbol') {
        Assert-Pass "G06: loop_runner.rs records and updates target states from the B1C flow"
    } else {
        Assert-Fail "G06: loop_runner.rs target-state recording/update wiring missing"
    }

    if ($LoopContent -match 'max_new_orders_per_tick_reached') {
        Assert-Pass "G07: max_new_orders_per_tick_reached is recorded as no_order_reason"
    } else {
        Assert-Fail "G07: max_new_orders_per_tick_reached target-state recording missing"
    }

    $RequiredReasons = @(
        'b5_short_sale_guard',
        'already_at_target',
        'order_will_be_submitted'
    )
    $MissingReasons = @($RequiredReasons | Where-Object { $LoopContent -notmatch $_ })
    if ($MissingReasons.Count -eq 0) {
        Assert-Pass "G08: b5_short_sale_guard, already_at_target, and order_will_be_submitted are represented"
    } else {
        Assert-Fail "G08: missing no_order_reason strings in loop_runner.rs: $($MissingReasons -join ', ')"
    }
} else {
    Assert-Fail "G06: loop_runner.rs not found at $LoopRunner"
    Assert-Fail "G07: loop_runner.rs not found at $LoopRunner"
    Assert-Fail "G08: loop_runner.rs not found at $LoopRunner"
}

if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $Labels = @(
        't01_', 't02_', 't03_', 't04_', 't05_', 't06_', 't07_',
        't08_', 't09_', 't10_', 't11_', 't12_', 't13_', 't14_'
    )
    $MissingLabels = @($Labels | Where-Object { $TestContent -notmatch $_ })
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "G09: scenario_per_symbol_target_state_01.rs exists with T01-T14 tests"
    } else {
        Assert-Fail "G09: missing scenario test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "G09: scenario_per_symbol_target_state_01.rs not found at $TestFile"
}

Push-Location $RepoRoot
$DbHits = @(git grep -n -e 'PerSymbolTargetState' -e 'per_symbol_target_state' -- 'core-rs/crates/mqk-db' 'core-rs/migrations' 2>$null)
Pop-Location
if ($DbHits.Count -eq 0) {
    Assert-Pass "G10: no DB migration or mqk-db write path added for target state"
} else {
    Assert-Fail "G10: target-state DB/migration references found: $($DbHits -join ' | ')"
}

Push-Location $RepoRoot
$DiffNames = @(
    git diff --name-only HEAD
    git diff --cached --name-only
) | Where-Object { $_ }
Pop-Location

$ForbiddenPathPattern = '^(core-rs/crates/mqk-execution/|core-rs/crates/mqk-broker-|core-rs/crates/mqk-risk/|core-rs/crates/mqk-reconcile/|core-rs/crates/mqk-portfolio/)'
$ForbiddenTouched = @($DiffNames | Where-Object { $_ -match $ForbiddenPathPattern })
if ($ForbiddenTouched.Count -eq 0) {
    Assert-Pass "G11: no broker/OMS/execution/risk/reconcile/portfolio files touched in current diff"
} else {
    Assert-Fail "G11: forbidden files touched: $($ForbiddenTouched -join ', ')"
}

Push-Location $RepoRoot
$RouteHits = @(git grep -n -e '/api/v1/strategy/multi-symbol-dispatch-summary' -e 'multi-symbol-dispatch-summary' -- 'core-rs/crates/mqk-daemon/src/routes' 'core-rs/crates/mqk-daemon/src/api_types.rs' 2>$null)
Pop-Location
if ($RouteHits.Count -eq 0) {
    Assert-Pass "G12: no API route for multi-symbol dispatch summary added before Patch 9"
} elseif ((Test-Path $DesignDoc) -and ((Get-Content $DesignDoc -Raw) -match 'MULTI-SYMBOL-DISPATCH-SUMMARY-01.*CLOSED')) {
    Assert-Pass "G12: dispatch-summary route/API references are allowed because Patch 9 is CLOSED"
} else {
    Assert-Fail "G12: dispatch-summary route/API references found before Patch 9 closure: $($RouteHits -join ' | ')"
}

$GuiTouched = @($DiffNames | Where-Object { $_ -match '^core-rs/mqk-gui/' })
$DesignContentForG13 = if (Test-Path $DesignDoc) { Get-Content $DesignDoc -Raw } else { '' }
if ($GuiTouched.Count -eq 0) {
    Assert-Pass "G13: no GUI files touched in current diff"
} elseif ($DesignContentForG13 -match 'MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01.*CLOSED') {
    Assert-Pass "G13: GUI files touched are accounted for by MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01 (documented CLOSED): $($GuiTouched -join ', ')"
} else {
    Assert-Fail "G13: GUI files touched without MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01 closure documented: $($GuiTouched -join ', ')"
}

$Patch8Files = @(
    'core-rs/crates/mqk-daemon/src/state.rs',
    'core-rs/crates/mqk-daemon/src/state/lifecycle.rs',
    'core-rs/crates/mqk-daemon/src/state/loop_runner.rs',
    'core-rs/crates/mqk-daemon/tests/scenario_per_symbol_target_state_01.rs',
    'docs/design/native_multi_symbol_dispatch.md',
    'tests/script_guards/test_per_symbol_target_state.ps1',
    'tests/script_guards/run_all_script_guards.ps1'
)
Push-Location $RepoRoot
$ApprovedLiveTruePattern = '(^|[^[:alnum:]_])["'']?approved_for_live["'']?[[:space:]]*[:=][[:space:]]*true[[:space:]]*([,;})\]#]|//|$)'
$ApprovedLiveTrueHits = @(git grep -n -E $ApprovedLiveTruePattern -- $Patch8Files 2>$null)
Pop-Location
if ($ApprovedLiveTrueHits.Count -eq 0) {
    Assert-Pass "G14: no Patch 8 file introduces approved_for_live live authority"
} else {
    Assert-Fail "G14: approved_for_live live-authority assignment found: $($ApprovedLiveTrueHits -join ' | ')"
}

if (Test-Path $DesignDoc) {
    $DesignContent = Get-Content $DesignDoc -Raw
    if ($DesignContent -match 'PER-SYMBOL-TARGET-STATE-01.*CLOSED' -and
        $DesignContent -match 'PerSymbolTargetState.*in-memory' -and
        $DesignContent -match 'MULTI-SYMBOL-DISPATCH-SUMMARY-01.*(OPEN|CLOSED)') {
        Assert-Pass "G15: docs mark PER-SYMBOL-TARGET-STATE-01 as CLOSED and track Patch 9 lifecycle"
    } else {
        Assert-Fail "G15: design doc does not mark Patch 8 closed with Patch 9 lifecycle"
    }
} else {
    Assert-Fail "G15: native_multi_symbol_dispatch.md not found at $DesignDoc"
}

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
