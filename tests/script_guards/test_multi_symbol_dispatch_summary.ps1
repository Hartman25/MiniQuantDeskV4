# =============================================================================
# Script guard: MULTI-SYMBOL-DISPATCH-SUMMARY-01
#
# Verifies the Patch 9 read-only daemon API route:
# GET /api/v1/strategy/multi-symbol-dispatch-summary
#
# G12/G15 were updated for MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01 (Patch 10): that
# patch legitimately adds a read-only GUI panel consuming this route, so a
# blanket "no GUI files touched" / "Patch 10 open" check would falsely fail
# once Patch 10 is committed. See test_multi_symbol_oms_overview_gui.ps1 for
# Patch 10's own guard.
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
$ApiTypes      = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\api_types.rs"
$RoutesRs      = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes.rs"
$StrategyRoute = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\strategy.rs"
$TestFile      = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_multi_symbol_dispatch_summary_01.rs"
$DesignDoc     = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"

$Patch9Files = @(
    'core-rs/crates/mqk-daemon/src/api_types.rs',
    'core-rs/crates/mqk-daemon/src/routes.rs',
    'core-rs/crates/mqk-daemon/src/routes/strategy.rs',
    'core-rs/crates/mqk-daemon/tests/scenario_multi_symbol_dispatch_summary_01.rs',
    'docs/design/native_multi_symbol_dispatch.md',
    'tests/script_guards/test_multi_symbol_dispatch_summary.ps1',
    'tests/script_guards/run_all_script_guards.ps1'
)

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_multi_symbol_dispatch_summary.ps1'
Write-Host '============================================================'

$ApiContent = ''
if (Test-Path $ApiTypes) {
    $ApiContent = Get-Content $ApiTypes -Raw

    $SummaryFields = @(
        'pub canonical_route: String',
        'pub backend: String',
        'pub truth_state: String',
        'pub runtime_execution_mode: String',
        'pub configured_symbol_count: usize',
        'pub per_symbol: Vec<PerSymbolDispatchRow>'
    )
    $MissingSummaryFields = @($SummaryFields | Where-Object { $ApiContent -notmatch [regex]::Escape($_) })
    if ($ApiContent -match 'pub struct MultiSymbolDispatchSummaryResponse' -and $MissingSummaryFields.Count -eq 0) {
        Assert-Pass "G01: MultiSymbolDispatchSummaryResponse exists with required fields"
    } else {
        Assert-Fail "G01: MultiSymbolDispatchSummaryResponse missing fields: $($MissingSummaryFields -join ', ')"
    }

    $RowFields = @(
        'pub symbol: String',
        'pub strategy_id: String',
        'pub current_qty: i64',
        'pub target_qty: i64',
        'pub delta: i64',
        'pub no_order_reason: String',
        'pub last_decision_id: Option<String>',
        'pub last_decision_disposition: Option<String>',
        'pub day_order_count: u32',
        'pub day_order_limit: Option<u32>',
        'pub bar_staleness_secs: Option<i64>'
    )
    $MissingRowFields = @($RowFields | Where-Object { $ApiContent -notmatch [regex]::Escape($_) })
    if ($ApiContent -match 'pub struct PerSymbolDispatchRow' -and $MissingRowFields.Count -eq 0) {
        Assert-Pass "G02: PerSymbolDispatchRow exists with required fields"
    } else {
        Assert-Fail "G02: PerSymbolDispatchRow missing fields: $($MissingRowFields -join ', ')"
    }
} else {
    Assert-Fail "G01: api_types.rs not found at $ApiTypes"
    Assert-Fail "G02: api_types.rs not found at $ApiTypes"
}

$RoutesContent = ''
$StrategyContent = ''
if ((Test-Path $RoutesRs) -and (Test-Path $StrategyRoute)) {
    $RoutesContent = Get-Content $RoutesRs -Raw
    $StrategyContent = Get-Content $StrategyRoute -Raw

    if ($RoutesContent -match '/api/v1/strategy/multi-symbol-dispatch-summary' -and
        $RoutesContent -match 'get\(multi_symbol_dispatch_summary\)' -and
        $StrategyContent -match 'pub\(crate\) async fn multi_symbol_dispatch_summary') {
        Assert-Pass "G03: multi-symbol dispatch summary route path is registered"
    } else {
        Assert-Fail "G03: route registration or handler missing"
    }

    if ($StrategyContent -match 'per_symbol_target_states\(\)\.await') {
        Assert-Pass "G04: route/builder reads AppState::per_symbol_target_states()"
    } else {
        Assert-Fail "G04: route/builder does not read per_symbol_target_states()"
    }

    if ($StrategyContent -match '"no_snapshot"') {
        Assert-Pass "G05: no_snapshot truth state is represented"
    } else {
        Assert-Fail "G05: no_snapshot truth state missing"
    }

    $DesignTruthContent = ''
    if (Test-Path $DesignDoc) {
        $DesignTruthContent = Get-Content $DesignDoc -Raw
    }
    if ($StrategyContent -match '"active"' -and
        (($StrategyContent -match '"legacy_single_symbol"') -or
         ($DesignTruthContent -match '"legacy_single_symbol".*reserved'))) {
        Assert-Pass "G06: active behavior is represented and legacy_single_symbol behavior is represented or documented"
    } else {
        Assert-Fail "G06: active/legacy_single_symbol truth-state behavior missing or undocumented"
    }

    if ($StrategyContent -match 'no_order_reason' -and
        $StrategyContent -match 'last_decision_disposition') {
        Assert-Pass "G07: per_symbol rows include no_order_reason and last_decision_disposition"
    } else {
        Assert-Fail "G07: no_order_reason or last_decision_disposition mapping missing"
    }
} else {
    Assert-Fail "G03: routes.rs or strategy.rs not found"
    Assert-Fail "G04: routes.rs or strategy.rs not found"
    Assert-Fail "G05: routes.rs or strategy.rs not found"
    Assert-Fail "G06: routes.rs or strategy.rs not found"
    Assert-Fail "G07: routes.rs or strategy.rs not found"
}

if ((Test-Path $TestFile) -and ((Get-Content $TestFile -Raw) -match 'max_new_orders_per_tick_reached')) {
    Assert-Pass "G08: max_new_orders_per_tick_reached is preserved as a no_order_reason"
} else {
    Assert-Fail "G08: max_new_orders_per_tick_reached proof missing"
}

if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $Labels = @(
        's01_', 's02_', 's03_', 's04_', 's05_', 's06_', 's07_',
        's08_', 's09_', 's10_', 's11_', 's12_', 's13_', 's14_'
    )
    $MissingLabels = @($Labels | Where-Object { $TestContent -notmatch $_ })
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "G09: scenario_multi_symbol_dispatch_summary_01.rs exists with S01-S14 tests"
    } else {
        Assert-Fail "G09: missing scenario test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "G09: scenario_multi_symbol_dispatch_summary_01.rs not found at $TestFile"
}

Push-Location $RepoRoot
$DbDiff = @(git diff --name-only HEAD -- 'core-rs/crates/mqk-db' 'core-rs/migrations' 2>$null)
$ImplementationFiles = @(
    'core-rs/crates/mqk-daemon/src/api_types.rs',
    'core-rs/crates/mqk-daemon/src/routes.rs',
    'core-rs/crates/mqk-daemon/src/routes/strategy.rs'
)
$AddedImplementationLines = @(git diff HEAD -- $ImplementationFiles 2>$null | Where-Object { $_ -match '^\+[^+]' })
$DbPatchHits = @($AddedImplementationLines | Where-Object { $_ -match 'mqk_db::|outbox_enqueue|insert_|update_|delete_' })
Pop-Location
if ($DbDiff.Count -eq 0 -and $DbPatchHits.Count -eq 0) {
    Assert-Pass "G10: no DB migration or mqk-db write path added"
} else {
    Assert-Fail "G10: DB/migration/write-path references found: $($DbDiff + $DbPatchHits -join ' | ')"
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
    Assert-Pass "G11: no broker/OMS/execution/risk/reconcile/portfolio files touched"
} else {
    Assert-Fail "G11: forbidden files touched: $($ForbiddenTouched -join ', ')"
}

$GuiTouched = @($DiffNames | Where-Object { $_ -match '^core-rs/mqk-gui/' })
$DesignContentForG12 = if (Test-Path $DesignDoc) { Get-Content $DesignDoc -Raw } else { '' }
if ($GuiTouched.Count -eq 0) {
    Assert-Pass "G12: no GUI files touched"
} elseif ($DesignContentForG12 -match 'MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01.*CLOSED') {
    Assert-Pass "G12: GUI files touched are accounted for by MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01 (documented CLOSED): $($GuiTouched -join ', ')"
} else {
    Assert-Fail "G12: GUI files touched without MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01 closure documented: $($GuiTouched -join ', ')"
}

$SmokeTouched = @($DiffNames | Where-Object { $_ -match '^(scripts/windows/|scripts/paper_|research-py/|exports/|evidence/)' })
if ($SmokeTouched.Count -eq 0) {
    Assert-Pass "G13: no smoke/evidence scripts touched"
} else {
    Assert-Fail "G13: smoke/evidence files touched: $($SmokeTouched -join ', ')"
}

Push-Location $RepoRoot
$LiveAuthorityPattern = '(^|[^[:alnum:]_])["'']?approved_for_live["'']?[[:space:]]*[:=][[:space:]]*true[[:space:]]*([,;})\]#]|//|$)|live_routing_enabled[[:space:]]*[:=][[:space:]]*true'
$LiveAuthorityHits = @(git grep -n -E $LiveAuthorityPattern -- $Patch9Files 2>$null)
Pop-Location
if ($LiveAuthorityHits.Count -eq 0) {
    Assert-Pass "G14: no approved_for_live=true or live-routing authority introduced by Patch 9 files"
} else {
    Assert-Fail "G14: live authority assignment found: $($LiveAuthorityHits -join ' | ')"
}

if (Test-Path $DesignDoc) {
    $DesignContent = Get-Content $DesignDoc -Raw
    if ($DesignContent -match 'MULTI-SYMBOL-DISPATCH-SUMMARY-01.*CLOSED' -and
        $DesignContent -match 'GET /api/v1/strategy/multi-symbol-dispatch-summary.*exists' -and
        $DesignContent -match 'MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01.*CLOSED' -and
        $DesignContent -match 'WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01.*OPEN') {
        Assert-Pass "G15: docs mark Patch 9 and Patch 10 CLOSED, Patch 11 open"
    } else {
        Assert-Fail "G15: docs do not mark MULTI-SYMBOL-DISPATCH-SUMMARY-01/MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01 as CLOSED with Patch 11 open"
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
