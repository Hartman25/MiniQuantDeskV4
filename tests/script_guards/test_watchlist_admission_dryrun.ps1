# =============================================================================
# Script guard: PAPER-HANDOFF-ENFORCE-DESIGN-ONLY-01
#
# Verifies that the dry-run watchlist admission check surface:
#   1. Endpoint exists in routes.rs
#   2. Response always includes "dry_run_only_not_enforced"
#   3. Real strategy/signal route does NOT reference watchlist admission yet
#   4. No /v2/orders in watchlist admission handler
#   5. No submit_order in watchlist admission handler
#   6. No oms_outbox write from watchlist admission route
#   7. No oms_inbox from watchlist admission route
#   8. No broker gateway import in watchlist route file
#   9. No DB mutation in watchlist admission handler
#  10. No live_routing_enabled=true
#  11. approved_for_live: false present in admission check response builder
#  12. approved_for_live=true not assigned in admission check response builder
#  13. Signal enforcement is documented as deferred in spec doc
#  14. Test file exists with AD01..AD14 labels
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
$WatchlistRoute = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\watchlist.rs"
$WatchlistIntake= Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\watchlist_intake.rs"
$RoutesRs       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes.rs"
$StrategyRoute  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\strategy.rs"
$ApiTypes       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\api_types.rs"
$SpecDoc        = Join-Path $RepoRoot "docs\specs\market_scanner_backtest_candidate_pipeline.md"
$TestFile       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_watchlist_admission_dryrun_01.rs"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_watchlist_admission_dryrun.ps1'
Write-Host '============================================================'

# AD-G01 -- admission-check endpoint registered in routes.rs
if (Select-String -Path $RoutesRs -Pattern 'api/v1/watchlist/admission-check' -Quiet) {
    Assert-Pass "AD-G01: /api/v1/watchlist/admission-check route registered in routes.rs"
} else {
    Assert-Fail "AD-G01: /api/v1/watchlist/admission-check not found in routes.rs"
}

# AD-G02 -- watchlist_admission_check handler exists in routes/watchlist.rs
if (Select-String -Path $WatchlistRoute -Pattern 'watchlist_admission_check' -Quiet) {
    Assert-Pass "AD-G02: watchlist_admission_check handler present in routes/watchlist.rs"
} else {
    Assert-Fail "AD-G02: watchlist_admission_check handler not found in routes/watchlist.rs"
}

# AD-G03 -- response always includes "dry_run_only_not_enforced"
$RouteContent = Get-Content $WatchlistRoute -Raw
if ($RouteContent -match 'dry_run_only_not_enforced') {
    Assert-Pass "AD-G03: 'dry_run_only_not_enforced' note present in routes/watchlist.rs"
} else {
    Assert-Fail "AD-G03: 'dry_run_only_not_enforced' note NOT found in routes/watchlist.rs"
}

# AD-G04 -- Real strategy/signal route does NOT reference watchlist admission
if (Test-Path $StrategyRoute) {
    if (-not (Select-String -Path $StrategyRoute -Pattern 'evaluate_watchlist_signal_admission|watchlist_admission' -Quiet)) {
        Assert-Pass "AD-G04: evaluate_watchlist_signal_admission not wired into strategy signal route"
    } else {
        Assert-Fail "AD-G04: evaluate_watchlist_signal_admission found in strategy.rs -- must NOT be wired yet"
    }
} else {
    Assert-Fail "AD-G04: strategy.rs not found at $StrategyRoute"
}

# AD-G05 -- No /v2/orders in watchlist route file
if (-not (Select-String -Path $WatchlistRoute -Pattern 'v2/orders' -Quiet)) {
    Assert-Pass "AD-G05: no /v2/orders in routes/watchlist.rs"
} else {
    Assert-Fail "AD-G05: /v2/orders found in routes/watchlist.rs -- broker call FORBIDDEN"
}

# AD-G06 -- No submit_order in watchlist route file
if (-not (Select-String -Path $WatchlistRoute -Pattern 'submit_order' -Quiet)) {
    Assert-Pass "AD-G06: no submit_order in routes/watchlist.rs"
} else {
    Assert-Fail "AD-G06: submit_order found in routes/watchlist.rs -- FORBIDDEN"
}

# AD-G07 -- No OMS outbox write in watchlist route
if (-not (Select-String -Path $WatchlistRoute -Pattern 'outbox_enqueue|outbox_insert|outbox_claim|oms_outbox' -Quiet)) {
    Assert-Pass "AD-G07: no OMS outbox write in routes/watchlist.rs"
} else {
    Assert-Fail "AD-G07: OMS outbox write found in routes/watchlist.rs -- FORBIDDEN"
}

# AD-G08 -- No OMS inbox in watchlist route
if (-not (Select-String -Path $WatchlistRoute -Pattern 'inbox_insert|inbox_apply|inbox_mark|oms_inbox' -Quiet)) {
    Assert-Pass "AD-G08: no OMS inbox in routes/watchlist.rs"
} else {
    Assert-Fail "AD-G08: OMS inbox found in routes/watchlist.rs -- FORBIDDEN"
}

# AD-G09 -- No broker gateway import in routes/watchlist.rs
if (-not (Select-String -Path $WatchlistRoute -Pattern 'mqk_broker|AlpacaBrokerAdapter|BrokerAdapter' -Quiet)) {
    Assert-Pass "AD-G09: no broker gateway import in routes/watchlist.rs"
} else {
    Assert-Fail "AD-G09: broker gateway import found in routes/watchlist.rs -- FORBIDDEN"
}

# AD-G10 -- No DB mutation in routes/watchlist.rs
if (-not (Select-String -Path $WatchlistRoute -Pattern 'sqlx|INSERT|UPDATE|DELETE' -Quiet)) {
    Assert-Pass "AD-G10: no DB mutation in routes/watchlist.rs"
} else {
    Assert-Fail "AD-G10: DB mutation pattern found in routes/watchlist.rs -- FORBIDDEN"
}

# AD-G11 -- No live_routing_enabled in watchlist route or intake
$LiveRoutingFound = $false
foreach ($f in @($WatchlistRoute, $WatchlistIntake)) {
    if (Test-Path $f) {
        if (Select-String -Path $f -Pattern 'live_routing_enabled' -Quiet) {
            $LiveRoutingFound = $true
        }
    }
}
if (-not $LiveRoutingFound) {
    Assert-Pass "AD-G11: no live_routing_enabled in watchlist route or intake"
} else {
    Assert-Fail "AD-G11: live_routing_enabled found in watchlist module -- FORBIDDEN"
}

# AD-G12 -- approved_for_live: false enforced in admission check response builder
if ($RouteContent -match 'approved_for_live:\s*false') {
    Assert-Pass "AD-G12: approved_for_live: false enforced in routes/watchlist.rs"
} else {
    Assert-Fail "AD-G12: approved_for_live: false not found in routes/watchlist.rs -- live lock missing"
}

# AD-G13 -- approved_for_live: true is never assigned in routes/watchlist.rs
if (-not ($RouteContent -match 'approved_for_live:\s*true')) {
    Assert-Pass "AD-G13: approved_for_live: true is never assigned in routes/watchlist.rs"
} else {
    Assert-Fail "AD-G13: approved_for_live: true found in routes/watchlist.rs -- hard live lock violated"
}

# AD-G14 -- Signal enforcement documented as deferred in spec doc
if (Test-Path $SpecDoc) {
    $SpecContent = Get-Content $SpecDoc -Raw
    if ($SpecContent -match 'PAPER-HANDOFF-ENFORCE-DESIGN-ONLY-01' -and
        $SpecContent -match 'dry_run_only_not_enforced|enforcement NOT active|deferred') {
        Assert-Pass "AD-G14: enforcement-deferred contract documented in spec doc"
    } else {
        Assert-Fail "AD-G14: enforcement-deferred contract not found in spec doc"
    }
} else {
    Assert-Fail "AD-G14: spec doc not found at $SpecDoc"
}

# AD-G15 -- Test file exists with AD01..AD14 labels
if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $MissingLabels = [System.Collections.Generic.List[string]]::new()
    $Labels = @('ad01','ad02','ad03','ad04','ad05','ad06','ad07','ad08',
                'ad09','ad10','ad11','ad12','ad13','ad14')
    foreach ($lbl in $Labels) {
        if ($TestContent -notmatch $lbl) {
            $MissingLabels.Add($lbl)
        }
    }
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "AD-G15: scenario_watchlist_admission_dryrun_01.rs has AD01..AD14 tests"
    } else {
        Assert-Fail "AD-G15: missing test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "AD-G15: scenario_watchlist_admission_dryrun_01.rs not found at $TestFile"
}

# AD-G16 -- WatchlistAdmissionCheckResponse type in api_types.rs
if (Test-Path $ApiTypes) {
    if (Select-String -Path $ApiTypes -Pattern 'WatchlistAdmissionCheckResponse' -Quiet) {
        Assert-Pass "AD-G16: WatchlistAdmissionCheckResponse declared in api_types.rs"
    } else {
        Assert-Fail "AD-G16: WatchlistAdmissionCheckResponse not found in api_types.rs"
    }
} else {
    Assert-Fail "AD-G16: api_types.rs not found at $ApiTypes"
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
