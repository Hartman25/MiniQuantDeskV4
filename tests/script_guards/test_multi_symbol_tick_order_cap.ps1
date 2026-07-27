# =============================================================================
# Script guard: MULTI-SYMBOL-TICK-ORDER-CAP-01
#
# Verifies cap #6 (design doc §6, "Patch 7"), the optional per-tick maximum
# new-order count:
#
#   1. signal_intake.rs defines max_new_orders_per_tick_from_env() ->
#      Option<u32> reading MQK_MAX_NEW_ORDERS_PER_TICK, with NO `.filter(...)`
#      (unlike cap #2 -- `0` is a valid, meaningful "no new orders this tick"
#      configuration).
#   2. signal_intake.rs defines AppState::max_new_orders_per_tick() and
#      set_max_new_orders_per_tick_for_test().
#   3. state.rs defines the field max_new_orders_per_tick:
#      Arc<RwLock<Option<u32>>>, initialized from
#      max_new_orders_per_tick_from_env() in the constructor.
#   4. state.rs defines the pure helper max_new_orders_per_tick_reason(
#      new_orders_this_tick: u32, cap: Option<u32>) -> Option<&'static str>,
#      using `>=` (boundary-inclusive) and returning
#      "max_new_orders_per_tick_reached".
#   5. loop_runner.rs wires cap #6 into the B1C per-symbol dispatch loop:
#      reads state_arc.max_new_orders_per_tick().await and initializes
#      new_orders_this_tick=0 BEFORE the per-symbol loop; calls
#      AppState::max_new_orders_per_tick_reason(...) as the FIRST check inside
#      each iteration (before the symbol-mismatch guard), `continue`-ing with
#      a b1c_symbol_skipped_max_new_orders_per_tick log on Some(reason);
#      increments new_orders_this_tick on each accepted decision.
#   6. Has the 9 C6-01..C6-09 proof tests in
#      scenario_multi_symbol_tick_order_cap_01.rs.
#   7. This patch's diff introduces no broker/OMS/portfolio direct writes, no
#      order submit/cancel/replace calls, no approved_for_live references, and
#      no MultiSymbolRiskCaps struct (out of scope for this patch).
#   8. Design doc documents MULTI-SYMBOL-TICK-ORDER-CAP-01 (cap #6, Patch 7) as
#      CLOSED.
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

$RepoRoot     = (Resolve-Path "$PSScriptRoot\..\..\").Path
$StateRs      = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state.rs"
$SignalIntake = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\signal_intake.rs"
$LoopRunnerRs = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\loop_runner.rs"
$TestFile     = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_multi_symbol_tick_order_cap_01.rs"
$DesignDoc    = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_multi_symbol_tick_order_cap.ps1'
Write-Host '============================================================'

# G01 -- max_new_orders_per_tick_from_env reads MQK_MAX_NEW_ORDERS_PER_TICK,
# no .filter() (0 is a valid value, unlike cap #2).
if (Test-Path $SignalIntake) {
    $SignalContent = Get-Content $SignalIntake -Raw

    $FnIdx = $SignalContent.IndexOf('fn max_new_orders_per_tick_from_env()')
    if ($FnIdx -ge 0) {
        $FnBlock = $SignalContent.Substring($FnIdx, [Math]::Min(300, $SignalContent.Length - $FnIdx))
        if ($FnBlock -match 'fn max_new_orders_per_tick_from_env\(\)\s*->\s*Option<u32>' -and
            $FnBlock -match 'MQK_MAX_NEW_ORDERS_PER_TICK' -and
            $FnBlock -notmatch '\.filter\(') {
            Assert-Pass "G01: max_new_orders_per_tick_from_env() -> Option<u32> reads MQK_MAX_NEW_ORDERS_PER_TICK with no .filter() (0 is valid)"
        } else {
            Assert-Fail "G01: max_new_orders_per_tick_from_env() does not match expected shape (env var / no .filter()): $FnBlock"
        }
    } else {
        Assert-Fail "G01: max_new_orders_per_tick_from_env() NOT found in signal_intake.rs"
    }

    # G02 -- cap #6 accessor/test-seam methods present
    $RequiredFns = @(
        'pub async fn max_new_orders_per_tick\(&self\) -> Option<u32>',
        'pub fn set_max_new_orders_per_tick_for_test\(&self, cap: Option<u32>\)'
    )
    $MissingFns = @($RequiredFns | Where-Object { $SignalContent -notmatch $_ })
    if ($MissingFns.Count -eq 0) {
        Assert-Pass "G02: cap #6 accessor/test-seam methods defined on AppState (signal_intake.rs)"
    } else {
        Assert-Fail "G02: missing cap #6 methods in signal_intake.rs: $($MissingFns -join ' | ')"
    }
} else {
    Assert-Fail "G01: signal_intake.rs not found at $SignalIntake"
    Assert-Fail "G02: signal_intake.rs not found at $SignalIntake"
}

# G03 -- state.rs field defined and initialized from env reader
if (Test-Path $StateRs) {
    $StateContent = Get-Content $StateRs -Raw

    if ($StateContent -match 'max_new_orders_per_tick:\s*Arc<RwLock<Option<u32>>>' -and
        $StateContent -match 'max_new_orders_per_tick:\s*Arc::new\(RwLock::new\(\s*signal_intake::max_new_orders_per_tick_from_env\(\),?\s*\)\)') {
        Assert-Pass "G03: max_new_orders_per_tick field defined and initialized from max_new_orders_per_tick_from_env() in state.rs"
    } else {
        Assert-Fail "G03: max_new_orders_per_tick field / constructor init NOT found (as expected) in state.rs"
    }

    # G04 -- pure max_new_orders_per_tick_reason helper, boundary-inclusive (>=)
    if ($StateContent -match 'pub fn max_new_orders_per_tick_reason\(\s*new_orders_this_tick:\s*u32,\s*cap:\s*Option<u32>,?\s*\)\s*->\s*Option<&''static str>' -and
        $StateContent -match 'new_orders_this_tick\s*>=\s*cap' -and
        $StateContent -match '"max_new_orders_per_tick_reached"') {
        Assert-Pass "G04: max_new_orders_per_tick_reason defined, boundary-inclusive (>=), returns max_new_orders_per_tick_reached"
    } else {
        Assert-Fail "G04: max_new_orders_per_tick_reason NOT found with the expected signature/logic"
    }
} else {
    Assert-Fail "G03: state.rs not found at $StateRs"
    Assert-Fail "G04: state.rs not found at $StateRs"
}

# G05 -- loop_runner.rs wires cap #6 into the post-allocation submission loop
#
# RUNTIME-OPPORTUNITY-ALLOCATION-01-READINESS-AND-AUTHORITY-REPAIR-01: cap #6
# necessarily relocated from "first check inside the per-symbol
# derive-decisions loop" to "first check inside the post-allocation
# submission loop" -- it counts *accepted* submissions, which can only be
# known after Bundle 5's opportunity allocator has narrowed the tick's whole
# same-cycle candidate batch. The cap's *effect* is unchanged (same running
# accepted-count, same dispatch order, same
# "max_new_orders_per_tick_reached" reason) -- only *where* in the pipeline
# the check runs. `Test-Cap6Wiring` below is the one shared check both the
# real file (this section) and the mutation-negative fixtures (further down)
# call, so there is exactly one definition of "correctly wired" to drift out
# of sync.
function Test-Cap6Wiring([string]$Content) {
    $CapReadIdx       = $Content.IndexOf('state_arc.max_new_orders_per_tick().await')
    $CounterInitIdx   = $Content.IndexOf('let mut new_orders_this_tick: u32 = 0;')
    $AllocCallIdx     = $Content.IndexOf('runtime_opportunity_allocation::gather_and_apply(')
    $PostAllocLoopIdx = $Content.IndexOf('for decision in allocation_outcome.decisions')
    $ReasonCheckIdx   = $Content.IndexOf('AppState::max_new_orders_per_tick_reason(')
    $SubmitIdx        = $Content.IndexOf('submit_internal_strategy_decision(')
    $AcceptedIdx      = $Content.IndexOf('if outcome.accepted {')
    $ElseSearchStart  = [Math]::Max($AcceptedIdx, 0)
    $ElseIdx          = $Content.IndexOf('} else {', $ElseSearchStart)
    $IncrementIdx     = $Content.IndexOf('new_orders_this_tick += 1;')
    $HasReasonString  = $Content -match [regex]::Escape('"max_new_orders_per_tick_reached"')

    return (
        $CapReadIdx -ge 0 -and
        $CounterInitIdx -gt $CapReadIdx -and
        $AllocCallIdx -gt $CounterInitIdx -and
        $PostAllocLoopIdx -gt $AllocCallIdx -and
        $ReasonCheckIdx -gt $PostAllocLoopIdx -and
        $ReasonCheckIdx -ge 0 -and $SubmitIdx -ge 0 -and $ReasonCheckIdx -lt $SubmitIdx -and
        $AcceptedIdx -gt $SubmitIdx -and
        $IncrementIdx -gt $AcceptedIdx -and
        ($ElseIdx -lt 0 -or $IncrementIdx -lt $ElseIdx) -and
        $HasReasonString
    )
}

if (Test-Path $LoopRunnerRs) {
    $LoopContent = Get-Content $LoopRunnerRs -Raw

    if (Test-Cap6Wiring $LoopContent) {
        Assert-Pass "G05: loop_runner.rs reads the cap #6 config + initializes new_orders_this_tick before the per-symbol dispatch loop, runs the opportunity allocator over the whole tick's batch, then checks max_new_orders_per_tick_reason FIRST inside the post-allocation submission loop (before submit_internal_strategy_decision), and increments the counter only inside the accepted branch"
    } else {
        Assert-Fail "G05: cap #6 wiring NOT found in expected order in loop_runner.rs's post-allocation submission loop"
    }
} else {
    Assert-Fail "G05: loop_runner.rs not found at $LoopRunnerRs"
}

# G05-MUT -- mutation-negative proof: `Test-Cap6Wiring` must FAIL on each of
# four minimal fixtures, each representing exactly one way the real wiring
# could regress. This proves the check above actually discriminates correct
# from incorrect wiring, rather than passing on anything cap#6-shaped.
$Cap6GoodFixture = @'
let max_new_orders_per_tick_cap = state_arc.max_new_orders_per_tick().await;
let mut new_orders_this_tick: u32 = 0;
let allocation_outcome = crate::runtime_opportunity_allocation::gather_and_apply(
    &state_arc, run_id, now_micros, market_date_today, dispatch_timeframe, all_decisions, &current_positions,
).await;
for decision in allocation_outcome.decisions {
    if AppState::max_new_orders_per_tick_reason(
        new_orders_this_tick,
        max_new_orders_per_tick_cap,
    ).is_some() {
        continue;
    }
    let outcome = crate::decision::submit_internal_strategy_decision(&state_arc, decision).await;
    if outcome.accepted {
        new_orders_this_tick += 1;
    } else {
    }
}
"max_new_orders_per_tick_reached"
'@

if (Test-Cap6Wiring $Cap6GoodFixture) {
    Assert-Pass "G05-MUT-00: Test-Cap6Wiring accepts the correctly-ordered positive fixture"
} else {
    Assert-Fail "G05-MUT-00: Test-Cap6Wiring rejected a correctly-ordered fixture -- the check itself is broken"
}

# G05-MUT-A: cap check moved to AFTER submission.
$Cap6MutMovedAfterSubmit = @'
let max_new_orders_per_tick_cap = state_arc.max_new_orders_per_tick().await;
let mut new_orders_this_tick: u32 = 0;
let allocation_outcome = crate::runtime_opportunity_allocation::gather_and_apply(
    &state_arc, run_id, now_micros, market_date_today, dispatch_timeframe, all_decisions, &current_positions,
).await;
for decision in allocation_outcome.decisions {
    let outcome = crate::decision::submit_internal_strategy_decision(&state_arc, decision).await;
    if AppState::max_new_orders_per_tick_reason(
        new_orders_this_tick,
        max_new_orders_per_tick_cap,
    ).is_some() {
        continue;
    }
    if outcome.accepted {
        new_orders_this_tick += 1;
    } else {
    }
}
"max_new_orders_per_tick_reached"
'@
if (-not (Test-Cap6Wiring $Cap6MutMovedAfterSubmit)) {
    Assert-Pass "G05-MUT-A: Test-Cap6Wiring correctly fails when the cap check is moved after submission"
} else {
    Assert-Fail "G05-MUT-A: Test-Cap6Wiring did NOT catch the cap check being moved after submission"
}

# G05-MUT-B: increment removed entirely.
$Cap6MutNoIncrement = @'
let max_new_orders_per_tick_cap = state_arc.max_new_orders_per_tick().await;
let mut new_orders_this_tick: u32 = 0;
let allocation_outcome = crate::runtime_opportunity_allocation::gather_and_apply(
    &state_arc, run_id, now_micros, market_date_today, dispatch_timeframe, all_decisions, &current_positions,
).await;
for decision in allocation_outcome.decisions {
    if AppState::max_new_orders_per_tick_reason(
        new_orders_this_tick,
        max_new_orders_per_tick_cap,
    ).is_some() {
        continue;
    }
    let outcome = crate::decision::submit_internal_strategy_decision(&state_arc, decision).await;
    if outcome.accepted {
    } else {
    }
}
"max_new_orders_per_tick_reached"
'@
if (-not (Test-Cap6Wiring $Cap6MutNoIncrement)) {
    Assert-Pass "G05-MUT-B: Test-Cap6Wiring correctly fails when the accepted-counter increment is removed"
} else {
    Assert-Fail "G05-MUT-B: Test-Cap6Wiring did NOT catch a missing increment"
}

# G05-MUT-C: increment moved outside the accepted-only branch (rejected
# submissions would also increment the count).
$Cap6MutUnconditionalIncrement = @'
let max_new_orders_per_tick_cap = state_arc.max_new_orders_per_tick().await;
let mut new_orders_this_tick: u32 = 0;
let allocation_outcome = crate::runtime_opportunity_allocation::gather_and_apply(
    &state_arc, run_id, now_micros, market_date_today, dispatch_timeframe, all_decisions, &current_positions,
).await;
for decision in allocation_outcome.decisions {
    if AppState::max_new_orders_per_tick_reason(
        new_orders_this_tick,
        max_new_orders_per_tick_cap,
    ).is_some() {
        continue;
    }
    let outcome = crate::decision::submit_internal_strategy_decision(&state_arc, decision).await;
    new_orders_this_tick += 1;
    if outcome.accepted {
    } else {
    }
}
"max_new_orders_per_tick_reached"
'@
if (-not (Test-Cap6Wiring $Cap6MutUnconditionalIncrement)) {
    Assert-Pass "G05-MUT-C: Test-Cap6Wiring correctly fails when the increment is unconditional (would count rejected submissions)"
} else {
    Assert-Fail "G05-MUT-C: Test-Cap6Wiring did NOT catch an unconditional (accepted-or-not) increment"
}

# G05-MUT-D: bounded reason string removed.
$Cap6MutNoReasonString = @'
let max_new_orders_per_tick_cap = state_arc.max_new_orders_per_tick().await;
let mut new_orders_this_tick: u32 = 0;
let allocation_outcome = crate::runtime_opportunity_allocation::gather_and_apply(
    &state_arc, run_id, now_micros, market_date_today, dispatch_timeframe, all_decisions, &current_positions,
).await;
for decision in allocation_outcome.decisions {
    if AppState::max_new_orders_per_tick_reason(
        new_orders_this_tick,
        max_new_orders_per_tick_cap,
    ).is_some() {
        continue;
    }
    let outcome = crate::decision::submit_internal_strategy_decision(&state_arc, decision).await;
    if outcome.accepted {
        new_orders_this_tick += 1;
    } else {
    }
}
'@
if (-not (Test-Cap6Wiring $Cap6MutNoReasonString)) {
    Assert-Pass "G05-MUT-D: Test-Cap6Wiring correctly fails when the bounded reason string is removed"
} else {
    Assert-Fail "G05-MUT-D: Test-Cap6Wiring did NOT catch a missing bounded reason string"
}

# G06 -- scenario test file has C6-01..C6-09 labels
if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $MissingLabels = [System.Collections.Generic.List[string]]::new()
    $Labels = @('c6_01', 'c6_02', 'c6_03', 'c6_04', 'c6_05', 'c6_06', 'c6_07', 'c6_08', 'c6_09')
    foreach ($lbl in $Labels) {
        if ($TestContent -notmatch $lbl) {
            $MissingLabels.Add($lbl)
        }
    }
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "G06: scenario_multi_symbol_tick_order_cap_01.rs has C6-01..C6-09 tests"
    } else {
        Assert-Fail "G06: missing test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "G06: scenario_multi_symbol_tick_order_cap_01.rs not found at $TestFile"
}

# G07 -- this patch's diff introduces no broker/OMS/portfolio direct writes, no
# order submit/cancel/replace calls, no approved_for_live references, and no
# MultiSymbolRiskCaps struct (out of scope for this patch). Checked against
# added lines only (git diff HEAD), since some files legitimately contain
# pre-existing references to these symbols elsewhere.
$DiffPaths = @($StateRs, $SignalIntake, $LoopRunnerRs)
Push-Location $RepoRoot
$AddedLines = git diff HEAD -- $DiffPaths |
    Where-Object { $_ -match '^\+[^+]' }
Pop-Location

$ForbiddenPattern = 'mqk_broker|mqk_portfolio::.*apply|submit_order|place_order|cancel_order|replace_order|outbox_enqueue\(|MultiSymbolRiskCaps|approved_for_live'
$ForbiddenAdded = $AddedLines | Where-Object { $_ -match $ForbiddenPattern }
if (-not $ForbiddenAdded) {
    Assert-Pass "G07: this patch's diff adds no broker/OMS/portfolio writes, order calls, MultiSymbolRiskCaps, or approved_for_live references"
} else {
    Assert-Fail "G07: this patch's diff adds an out-of-scope reference -- FORBIDDEN in this patch: $($ForbiddenAdded -join ' | ')"
}

# G08 -- design doc documents Cap #6 / MULTI-SYMBOL-TICK-ORDER-CAP-01 (Patch 7) as CLOSED
if (Test-Path $DesignDoc) {
    $DesignContent = Get-Content $DesignDoc -Raw
    if ($DesignContent -match 'MULTI-SYMBOL-TICK-ORDER-CAP-01' -and
        $DesignContent -match 'Cap #6 `max_new_orders_per_tick` \| \*\*CLOSED\*\*' -and
        $DesignContent -match 'max_new_orders_per_tick_reached') {
        Assert-Pass "G08: native_multi_symbol_dispatch.md documents Cap #6 / MULTI-SYMBOL-TICK-ORDER-CAP-01 (Patch 7) as CLOSED"
    } else {
        Assert-Fail "G08: native_multi_symbol_dispatch.md does NOT document Cap #6 / MULTI-SYMBOL-TICK-ORDER-CAP-01 as CLOSED"
    }
} else {
    Assert-Fail "G08: native_multi_symbol_dispatch.md not found at $DesignDoc"
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
