# =============================================================================
# Script guard: MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01 (Patch 10)
#
# Verifies the read-only "Multi-symbol dispatch summary" panel added to
# StrategyScreen, consuming the existing Patch 9 route:
# GET /api/v1/strategy/multi-symbol-dispatch-summary
#
# This patch is GUI-only operator visibility. It must not add order
# submit/cancel/replace/flatten controls, must not add
# approved_for_live/live_routing_enabled fields to the new surface, must not
# touch broker/OMS/execution/risk/reconcile/portfolio Rust, watchlist
# promotion Python, or smoke/evidence scripts, and must not start
# BACKTEST-GUI-CLOSURE-01 (the repo's name for the other open
# "backtest GUI experience" initiative, MiniQuantDesk_Master_Patch_Ledger_v2.md).
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

$RepoRoot         = (Resolve-Path "$PSScriptRoot\..\..\").Path
$ApiTypes         = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\api_types.rs"
$RoutesRs         = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes.rs"
$StrategyRoute    = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\strategy.rs"
$StrategyScreen   = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\strategy\StrategyScreen.tsx"
$ApiTs            = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\system\api.ts"
$LegacyTs         = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\system\legacy.ts"
$TypesStrategyTs  = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\system\types\strategy.ts"
$ApiTestTs        = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\system\api.test.ts"
$DesignDoc        = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"
$MasterLedger     = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_multi_symbol_oms_overview_gui.ps1'
Write-Host '============================================================'

# -----------------------------------------------------------------------
# G01: route reference — GUI fetches the existing Patch 9 route
# -----------------------------------------------------------------------
$ApiTsContent = if (Test-Path $ApiTs) { Get-Content $ApiTs -Raw } else { '' }
$StrategyScreenContent = if (Test-Path $StrategyScreen) { Get-Content $StrategyScreen -Raw } else { '' }

if ($ApiTsContent -match [regex]::Escape('/api/v1/strategy/multi-symbol-dispatch-summary') -and
    $StrategyScreenContent -match [regex]::Escape('/api/v1/strategy/multi-symbol-dispatch-summary')) {
    Assert-Pass "G01: GUI fetches and documents GET /api/v1/strategy/multi-symbol-dispatch-summary (Patch 9 route)"
} else {
    Assert-Fail "G01: multi-symbol-dispatch-summary route reference missing from api.ts or StrategyScreen.tsx"
}

# -----------------------------------------------------------------------
# G02: required top-level fields rendered
# -----------------------------------------------------------------------
$RequiredTopFields = @(
    'summary.truth_state',
    'summary.backend',
    'summary.runtime_execution_mode',
    'summary.configured_symbol_count'
)
$MissingTopFields = @($RequiredTopFields | Where-Object { $StrategyScreenContent -notmatch [regex]::Escape($_) })
if ($MissingTopFields.Count -eq 0) {
    Assert-Pass "G02: top-level fields (truth_state, backend, runtime_execution_mode, configured_symbol_count) are rendered"
} else {
    Assert-Fail "G02: missing top-level field renders: $($MissingTopFields -join ', ')"
}

# -----------------------------------------------------------------------
# G03: day_order_count / day_order_limit rendered
# -----------------------------------------------------------------------
if ($StrategyScreenContent -match [regex]::Escape('row.day_order_count') -and
    $StrategyScreenContent -match [regex]::Escape('row.day_order_limit')) {
    Assert-Pass "G03: day_order_count and day_order_limit are rendered per symbol"
} else {
    Assert-Fail "G03: day_order_count/day_order_limit not rendered"
}

# -----------------------------------------------------------------------
# G04: empty / no_snapshot state
# -----------------------------------------------------------------------
if ($StrategyScreenContent -match [regex]::Escape('No multi-symbol dispatch snapshot yet.')) {
    Assert-Pass "G04: empty/no_snapshot state renders 'No multi-symbol dispatch snapshot yet.'"
} else {
    Assert-Fail "G04: empty/no_snapshot state message missing"
}

# -----------------------------------------------------------------------
# G05: fail-soft "unavailable" state
# -----------------------------------------------------------------------
$LegacyTsContent = if (Test-Path $LegacyTs) { Get-Content $LegacyTs -Raw } else { '' }

if ($StrategyScreenContent -match 'truth_state === "unavailable"' -and
    $LegacyTsContent -match 'UNAVAILABLE_MULTI_SYMBOL_DISPATCH_SUMMARY' -and
    $LegacyTsContent -match '"unavailable"') {
    Assert-Pass "G05: fail-soft 'unavailable' truth_state sentinel exists and is rendered distinctly"
} else {
    Assert-Fail "G05: fail-soft 'unavailable' state not found in StrategyScreen.tsx / legacy.ts"
}

# -----------------------------------------------------------------------
# G06: no approved_for_live / live_routing_enabled on the new surface
# -----------------------------------------------------------------------
$TypesStrategyContent = if (Test-Path $TypesStrategyTs) { Get-Content $TypesStrategyTs -Raw } else { '' }
$ApiTestContent = if (Test-Path $ApiTestTs) { Get-Content $ApiTestTs -Raw } else { '' }

$SurfaceMatch = [regex]::Match($TypesStrategyContent, 'export interface MultiSymbolDispatchSummarySurface \{[^}]*\}')
$SurfaceHasLiveFields = $SurfaceMatch.Success -and ($SurfaceMatch.Value -match 'approved_for_live|live_routing_enabled')

if (-not $SurfaceHasLiveFields -and
    $ApiTestContent -match '"approved_for_live" in model\.multiSymbolDispatchSummary' -and
    $ApiTestContent -match '"live_routing_enabled" in model\.multiSymbolDispatchSummary') {
    Assert-Pass "G06: MultiSymbolDispatchSummarySurface carries no approved_for_live/live_routing_enabled field, proven by test"
} else {
    Assert-Fail "G06: approved_for_live/live_routing_enabled absence not proven for the multi-symbol dispatch summary surface"
}

# -----------------------------------------------------------------------
# G07: no order submit/cancel/replace/flatten controls in the new panel
# -----------------------------------------------------------------------
$PanelStart = $StrategyScreenContent.IndexOf('MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01')
$PanelSection = if ($PanelStart -ge 0) { $StrategyScreenContent.Substring($PanelStart) } else { '' }

if ($PanelStart -ge 0 -and
    $PanelSection -notmatch '<button' -and
    $PanelSection -notmatch 'onClick' -and
    $PanelSection -notmatch 'submit-order|cancel-order|replace-order|flatten-paper-positions') {
    Assert-Pass "G07: new multi-symbol dispatch summary panel adds no button/onClick/order-control markers"
} else {
    Assert-Fail "G07: order submit/cancel/replace/flatten control markers found in the new panel (or panel not found)"
}

# -----------------------------------------------------------------------
# G08-G10: diff-based checks
# -----------------------------------------------------------------------
Push-Location $RepoRoot
$DiffNames = @(
    git diff --name-only HEAD
    git diff --cached --name-only
) | Where-Object { $_ } | Sort-Object -Unique
Pop-Location

# G08: no forbidden Rust files touched unless justified
$ForbiddenPathPattern = '^(core-rs/crates/mqk-daemon/src/state\.rs|core-rs/crates/mqk-daemon/src/state/loop_runner\.rs|core-rs/crates/mqk-daemon/src/decision\.rs|core-rs/crates/mqk-execution/|core-rs/crates/mqk-broker-|core-rs/crates/mqk-risk/|core-rs/crates/mqk-reconcile/|core-rs/crates/mqk-portfolio/)'
$ForbiddenTouched = @($DiffNames | Where-Object { $_ -match $ForbiddenPathPattern })
if ($ForbiddenTouched.Count -eq 0) {
    Assert-Pass "G08: no broker/OMS/execution/risk/reconcile/portfolio/dispatch-loop/decision files touched"
} else {
    Assert-Fail "G08: forbidden files touched without justification: $($ForbiddenTouched -join ', ')"
}

# G09: no smoke/evidence scripts touched
$SmokeTouched = @($DiffNames | Where-Object { $_ -match '^(scripts/windows/|scripts/paper_|exports/|evidence/)' })
if ($SmokeTouched.Count -eq 0) {
    Assert-Pass "G09: no smoke/evidence scripts touched"
} else {
    Assert-Fail "G09: smoke/evidence files touched: $($SmokeTouched -join ', ')"
}

# G10: no watchlist promotion Python touched
$WatchlistPromoTouched = @($DiffNames | Where-Object { $_ -match 'watchlist_promotion' })
if ($WatchlistPromoTouched.Count -eq 0) {
    Assert-Pass "G10: no watchlist promotion Python touched"
} else {
    Assert-Fail "G10: watchlist promotion files touched: $($WatchlistPromoTouched -join ', ')"
}

# -----------------------------------------------------------------------
# G11: Patch-9 route still exists, unchanged
# -----------------------------------------------------------------------
$ApiContent = if (Test-Path $ApiTypes) { Get-Content $ApiTypes -Raw } else { '' }
$RoutesContent = if (Test-Path $RoutesRs) { Get-Content $RoutesRs -Raw } else { '' }
$StrategyRouteContent = if (Test-Path $StrategyRoute) { Get-Content $StrategyRoute -Raw } else { '' }

if ($ApiContent -match 'pub struct MultiSymbolDispatchSummaryResponse' -and
    $ApiContent -match 'pub struct PerSymbolDispatchRow' -and
    $RoutesContent -match '/api/v1/strategy/multi-symbol-dispatch-summary' -and
    $RoutesContent -match 'get\(multi_symbol_dispatch_summary\)' -and
    $StrategyRouteContent -match 'pub\(crate\) async fn multi_symbol_dispatch_summary') {
    Assert-Pass "G11: Patch 9 route (GET /api/v1/strategy/multi-symbol-dispatch-summary) still exists"
} else {
    Assert-Fail "G11: Patch 9 route/types missing or modified"
}

# -----------------------------------------------------------------------
# G12: focused GUI tests exist (GUIT01-GUIT08)
# -----------------------------------------------------------------------
$GuiLabels = @('GUIT01', 'GUIT02', 'GUIT03', 'GUIT04', 'GUIT05', 'GUIT06', 'GUIT07', 'GUIT08')
$MissingGuiLabels = @($GuiLabels | Where-Object { $ApiTestContent -notmatch $_ })
if ($MissingGuiLabels.Count -eq 0) {
    Assert-Pass "G12: GUIT01-GUIT08 focused tests exist in api.test.ts"
} else {
    Assert-Fail "G12: missing GUI test labels: $($MissingGuiLabels -join ', ')"
}

# -----------------------------------------------------------------------
# G13: docs mark Patch 10 CLOSED and Patch 11 next/open
# -----------------------------------------------------------------------
$DesignContent = if (Test-Path $DesignDoc) { Get-Content $DesignDoc -Raw } else { '' }

if ($DesignContent -match 'MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01.*CLOSED' -and
    $DesignContent -match 'WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01.*OPEN') {
    Assert-Pass "G13: docs mark MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01 CLOSED and Patch 11 open/next"
} else {
    Assert-Fail "G13: docs do not mark MULTI-SYMBOL-OMS-OVERVIEW-AND-GUI-01 CLOSED with Patch 11 open"
}

# -----------------------------------------------------------------------
# G14: BACKTEST-GUI-CLOSURE-01 (the repo's "backtest GUI experience"
# initiative) remains open/not-started and untouched by this patch
# -----------------------------------------------------------------------
$MasterLedgerContent = if (Test-Path $MasterLedger) { Get-Content $MasterLedger -Raw } else { '' }
$MasterLedgerTouched = @($DiffNames | Where-Object { $_ -match '^MiniQuantDesk_Master_Patch_Ledger_v2\.md$' })

if ($MasterLedgerTouched.Count -eq 0 -and
    $MasterLedgerContent -match 'BACKTEST-GUI-CLOSURE-01.*QUEUED') {
    Assert-Pass "G14: BACKTEST-GUI-CLOSURE-01 remains QUEUED/open and untouched by this patch"
} else {
    Assert-Fail "G14: BACKTEST-GUI-CLOSURE-01 status changed or master patch ledger touched by this patch"
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
