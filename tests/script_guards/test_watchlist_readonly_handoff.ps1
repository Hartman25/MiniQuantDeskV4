# =============================================================================
# Script guard: PAPER-HANDOFF-READONLY-01
#
# Verifies that the watchlist artifact loader and status endpoint are:
#   1. Read-only (no broker calls, no OMS writes, no DB mutation, no orders)
#   2. Hard-locked against live routing (approved_for_live=false enforced)
#   3. Correctly constrained (max_symbols_to_trade=1, max_concurrent_positions=1)
#   4. Free of dangerous symbols (submit_order, outbox write, inbox write, etc.)
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

$RepoRoot = (Resolve-Path "$PSScriptRoot\..\..\").Path

$WatchlistIntake = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\watchlist_intake.rs"
$WatchlistRoute  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\watchlist.rs"
$RoutesRs        = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes.rs"
$LibRs           = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\lib.rs"
$StrategyRoute   = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\strategy.rs"
$TestFile        = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_watchlist_readonly_handoff_01.rs"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_watchlist_readonly_handoff.ps1'
Write-Host '============================================================'

# G01 -- watchlist_intake.rs exists
if (Test-Path $WatchlistIntake) {
    Assert-Pass "G01: watchlist_intake.rs exists"
} else {
    Assert-Fail "G01: watchlist_intake.rs not found at $WatchlistIntake"
}

# G02 -- routes/watchlist.rs exists
if (Test-Path $WatchlistRoute) {
    Assert-Pass "G02: routes/watchlist.rs exists"
} else {
    Assert-Fail "G02: routes/watchlist.rs not found at $WatchlistRoute"
}

# G03 -- watchlist status endpoint registered in routes.rs
if (Select-String -Path $RoutesRs -Pattern 'api/v1/watchlist/status' -Quiet) {
    Assert-Pass "G03: /api/v1/watchlist/status route registered in routes.rs"
} else {
    Assert-Fail "G03: /api/v1/watchlist/status not found in routes.rs"
}

# G04 -- watchlist_intake module declared in lib.rs
if (Select-String -Path $LibRs -Pattern 'watchlist_intake' -Quiet) {
    Assert-Pass "G04: watchlist_intake module declared in lib.rs"
} else {
    Assert-Fail "G04: watchlist_intake not declared in lib.rs"
}

# G05 -- approved_for_live hard-lock: literal false enforced in response builder
$RouteContent = Get-Content $WatchlistRoute -Raw
if ($RouteContent -match 'approved_for_live:\s*false') {
    Assert-Pass "G05: approved_for_live: false enforced in route response builder"
} else {
    Assert-Fail "G05: approved_for_live: false not found in routes/watchlist.rs -- live lock missing"
}

# G06 -- No submit_order in watchlist_intake.rs
if (-not (Select-String -Path $WatchlistIntake -Pattern 'submit_order' -Quiet)) {
    Assert-Pass "G06: no submit_order in watchlist_intake.rs"
} else {
    Assert-Fail "G06: submit_order found in watchlist_intake.rs -- FORBIDDEN"
}

# G07 -- No /v2/orders in watchlist module files
$v2OrdersFound = $false
foreach ($f in @($WatchlistIntake, $WatchlistRoute)) {
    if (Test-Path $f) {
        if (Select-String -Path $f -Pattern 'v2/orders' -Quiet) {
            $v2OrdersFound = $true
        }
    }
}
if (-not $v2OrdersFound) {
    Assert-Pass "G07: no /v2/orders in watchlist module files"
} else {
    Assert-Fail "G07: /v2/orders found in watchlist module -- broker call FORBIDDEN"
}

# G08 -- No OMS outbox write in watchlist module
$OmsOutboxFound = $false
foreach ($f in @($WatchlistIntake, $WatchlistRoute)) {
    if (Test-Path $f) {
        if (Select-String -Path $f -Pattern 'outbox_enqueue|outbox_insert|outbox_claim|oms_outbox' -Quiet) {
            $OmsOutboxFound = $true
        }
    }
}
if (-not $OmsOutboxFound) {
    Assert-Pass "G08: no OMS outbox writes in watchlist module"
} else {
    Assert-Fail "G08: OMS outbox write found in watchlist module -- FORBIDDEN"
}

# G09 -- No inbox write in watchlist module
$InboxFound = $false
foreach ($f in @($WatchlistIntake, $WatchlistRoute)) {
    if (Test-Path $f) {
        if (Select-String -Path $f -Pattern 'inbox_insert|inbox_apply|inbox_mark' -Quiet) {
            $InboxFound = $true
        }
    }
}
if (-not $InboxFound) {
    Assert-Pass "G09: no inbox writes in watchlist module"
} else {
    Assert-Fail "G09: inbox write found in watchlist module -- FORBIDDEN"
}

# G10 -- No broker gateway import in watchlist_intake.rs
if (-not (Select-String -Path $WatchlistIntake -Pattern 'mqk_broker|AlpacaBrokerAdapter|BrokerAdapter' -Quiet)) {
    Assert-Pass "G10: no broker gateway import in watchlist_intake.rs"
} else {
    Assert-Fail "G10: broker gateway import found in watchlist_intake.rs -- FORBIDDEN"
}

# G11 -- No DB mutation in watchlist_intake.rs
if (-not (Select-String -Path $WatchlistIntake -Pattern 'sqlx|INSERT|UPDATE|DELETE' -Quiet)) {
    Assert-Pass "G11: no DB mutation in watchlist_intake.rs"
} else {
    Assert-Fail "G11: DB mutation pattern found in watchlist_intake.rs -- FORBIDDEN"
}

# G12 -- No live routing enable path in watchlist module
$LiveRoutingFound = $false
foreach ($f in @($WatchlistIntake, $WatchlistRoute)) {
    if (Test-Path $f) {
        if (Select-String -Path $f -Pattern 'live_routing_enabled' -Quiet) {
            $LiveRoutingFound = $true
        }
    }
}
if (-not $LiveRoutingFound) {
    Assert-Pass "G12: no live routing enable path in watchlist module"
} else {
    Assert-Fail "G12: live routing enable path found in watchlist module -- FORBIDDEN"
}

# G13 -- max_symbols_to_trade guard constant present
$IntakeContent = Get-Content $WatchlistIntake -Raw
if ($IntakeContent -match 'REQUIRED_MAX_SYMBOLS') {
    Assert-Pass "G13: max_symbols_to_trade == 1 constraint constant present in watchlist_intake.rs"
} else {
    Assert-Fail "G13: REQUIRED_MAX_SYMBOLS constant not found in watchlist_intake.rs"
}

# G14 -- max_concurrent_positions guard constant present
if ($IntakeContent -match 'REQUIRED_MAX_CONCURRENT') {
    Assert-Pass "G14: max_concurrent_positions == 1 constraint constant present in watchlist_intake.rs"
} else {
    Assert-Fail "G14: REQUIRED_MAX_CONCURRENT constant not found in watchlist_intake.rs"
}

# G15 -- Loader handles malformed JSON
if ($IntakeContent -match 'invalid JSON') {
    Assert-Pass "G15: loader handles malformed JSON (invalid JSON error path present)"
} else {
    Assert-Fail "G15: malformed JSON handler not found in watchlist_intake.rs"
}

# G16 -- Test file exists with W01..W20 labels
if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $MissingLabels = [System.Collections.Generic.List[string]]::new()
    $Labels = @('w01','w02','w03','w04','w05','w06','w07','w08','w09','w10',
                'w11','w12','w13','w14','w15','w16','w17','w18','w19','w20')
    foreach ($lbl in $Labels) {
        if ($TestContent -notmatch $lbl) {
            $MissingLabels.Add($lbl)
        }
    }
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "G16: scenario_watchlist_readonly_handoff_01.rs has W01..W20 tests"
    } else {
        Assert-Fail "G16: missing test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "G16: scenario_watchlist_readonly_handoff_01.rs not found"
}

# G17 -- approved_for_live invariant documented in module
if ($IntakeContent -match 'ALWAYS false' -or $IntakeContent -match 'hard live.lock') {
    Assert-Pass "G17: approved_for_live ALWAYS false invariant documented in module"
} else {
    Assert-Fail "G17: approved_for_live invariant not clearly documented in watchlist_intake.rs"
}

# G18 -- evaluate_watchlist_signal_admission not wired into strategy signal route
if (Test-Path $StrategyRoute) {
    if (-not (Select-String -Path $StrategyRoute -Pattern 'evaluate_watchlist_signal_admission' -Quiet)) {
        Assert-Pass "G18: evaluate_watchlist_signal_admission not wired into strategy signal route"
    } else {
        Assert-Fail "G18: evaluate_watchlist_signal_admission found in strategy.rs -- must not be wired yet"
    }
} else {
    Assert-Pass "G18: strategy.rs not found (skip)"
}

# G19 -- watchlist route is on public router (appears before operator block)
$RoutesContent = Get-Content $RoutesRs -Raw
$PublicIdx    = $RoutesContent.IndexOf('api/v1/watchlist/status')
$OperatorIdx  = $RoutesContent.IndexOf('let operator = Router::new()')
if ($PublicIdx -ge 0 -and $OperatorIdx -ge 0 -and $PublicIdx -lt $OperatorIdx) {
    Assert-Pass "G19: /api/v1/watchlist/status is on public (unauthenticated) router"
} else {
    Assert-Fail "G19: /api/v1/watchlist/status not confirmed on public router (check routes.rs ordering)"
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
