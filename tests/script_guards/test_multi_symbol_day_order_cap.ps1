# =============================================================================
# Script guard: MULTI-SYMBOL-DAY-ORDER-CAP-01
#
# Verifies that the optional per-symbol daily order count cap (cap #4,
# design doc §6, new Gate 1f):
#   1. Defines AppState::per_symbol_day_order_count_limit_from_env() reading
#      MQK_PER_SYMBOL_DAY_ORDER_LIMIT into Option<u32> (None disables Gate 1f).
#   2. Defines the new state fields day_signal_count_by_symbol
#      (Arc<RwLock<HashMap<String, u32>>>) and per_symbol_day_order_limit
#      (Arc<RwLock<Option<u32>>>) on AppState, both initialized in new_inner.
#   3. Defines AppState accessor/mutator methods: symbol_day_order_count,
#      increment_symbol_day_order_count, per_symbol_day_order_limit,
#      symbol_day_order_limit_exceeded, set_symbol_day_order_count_for_test,
#      set_per_symbol_day_order_limit_for_test, reset_symbol_day_order_counts.
#   4. lifecycle.rs resets per-symbol counters at run start, alongside the
#      account-wide day_signal_count reset (same boundary).
#   5. decision.rs inserts a new Gate 1f between Gate 1 (day_signal_limit,
#      account-wide) and Gate 1e (capital_budget), producing disposition
#      "symbol_day_limit_reached" on trip.
#   6. decision.rs Gate 7 Ok(true) arm increments both
#      increment_day_signal_count and increment_symbol_day_order_count.
#   7. Has the 9 D01..D09 proof tests in
#      scenario_multi_symbol_day_order_cap_01.rs.
#   8. This patch's diff introduces no broker/OMS/portfolio direct writes, no
#      order submit/cancel/replace calls, no approved_for_live references, and
#      no MultiSymbolRiskCaps struct (out of scope for this patch).
#   9. Is documented in the native multi-symbol dispatch design doc as
#      implemented (Cap #4 / Gate 1f no longer OPEN).
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
$LifecycleRs  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\lifecycle.rs"
$DecisionRs   = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\decision.rs"
$TestFile     = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_multi_symbol_day_order_cap_01.rs"
$DesignDoc    = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_multi_symbol_day_order_cap.ps1'
Write-Host '============================================================'

# G01 -- per_symbol_day_order_count_limit_from_env reads MQK_PER_SYMBOL_DAY_ORDER_LIMIT
if (Test-Path $SignalIntake) {
    $SignalContent = Get-Content $SignalIntake -Raw

    if ($SignalContent -match 'fn per_symbol_day_order_count_limit_from_env\(\)\s*->\s*Option<u32>' -and
        $SignalContent -match 'MQK_PER_SYMBOL_DAY_ORDER_LIMIT') {
        Assert-Pass "G01: per_symbol_day_order_count_limit_from_env() -> Option<u32> reads MQK_PER_SYMBOL_DAY_ORDER_LIMIT"
    } else {
        Assert-Fail "G01: per_symbol_day_order_count_limit_from_env() reading MQK_PER_SYMBOL_DAY_ORDER_LIMIT NOT found in signal_intake.rs"
    }

    # G02 -- normalize_symbol_key helper (trim + uppercase)
    if ($SignalContent -match 'fn normalize_symbol_key\(symbol: &str\) -> String' -and
        $SignalContent -match '\.trim\(\)\.to_ascii_uppercase\(\)') {
        Assert-Pass "G02: normalize_symbol_key(symbol: &str) -> String (trim + uppercase) defined"
    } else {
        Assert-Fail "G02: normalize_symbol_key helper NOT found in signal_intake.rs"
    }

    # G03 -- accessor/mutator methods present
    $RequiredFns = @(
        'pub async fn symbol_day_order_count\(&self, symbol: &str\) -> u32',
        'pub\(crate\) async fn increment_symbol_day_order_count\(&self, symbol: &str\)',
        'pub async fn per_symbol_day_order_limit\(&self\) -> Option<u32>',
        'pub async fn symbol_day_order_limit_exceeded\(&self, symbol: &str\) -> bool',
        'pub fn set_symbol_day_order_count_for_test\(&self, symbol: &str, count: u32\)',
        'pub fn set_per_symbol_day_order_limit_for_test\(&self, limit: Option<u32>\)',
        'pub async fn reset_symbol_day_order_counts\(&self\)'
    )
    $MissingFns = @($RequiredFns | Where-Object { $SignalContent -notmatch $_ })
    if ($MissingFns.Count -eq 0) {
        Assert-Pass "G03: all Gate 1f accessor/mutator methods defined on AppState (signal_intake.rs)"
    } else {
        Assert-Fail "G03: missing Gate 1f methods in signal_intake.rs: $($MissingFns -join ' | ')"
    }
} else {
    Assert-Fail "G01: signal_intake.rs not found at $SignalIntake"
    Assert-Fail "G02: signal_intake.rs not found at $SignalIntake"
    Assert-Fail "G03: signal_intake.rs not found at $SignalIntake"
}

# G04 -- new state fields defined and initialized
if (Test-Path $StateRs) {
    $StateContent = Get-Content $StateRs -Raw

    if ($StateContent -match 'day_signal_count_by_symbol:\s*Arc<RwLock<HashMap<String,\s*u32>>>' -and
        $StateContent -match 'per_symbol_day_order_limit:\s*Arc<RwLock<Option<u32>>>' -and
        $StateContent -match 'day_signal_count_by_symbol:\s*Arc::new\(RwLock::new\(HashMap::new\(\)\)\)' -and
        $StateContent -match 'per_symbol_day_order_count_limit_from_env\(\)') {
        Assert-Pass "G04: day_signal_count_by_symbol and per_symbol_day_order_limit fields defined and initialized in state.rs"
    } else {
        Assert-Fail "G04: new Gate 1f state fields NOT found (as expected) in state.rs"
    }
} else {
    Assert-Fail "G04: state.rs not found at $StateRs"
}

# G05 -- per-symbol counters are reset alongside the account-wide
# day_signal_count reset at the same run-start/economic-mirror-clear
# boundary. An "ownership consolidation" refactor (see git history) moved
# this pairing out of lifecycle.rs and into state.rs, where it now appears
# twice: once in commit_run_start_bundle's completion path, once in
# clear_economic_mirrors_for_run -- both call sites pair the two resets
# together identically. The reset IS paired correctly; this guard previously
# looked in the wrong file.
$G05ResetPattern = 'self\.day_signal_count\.store\(0,\s*Ordering::SeqCst\);\s*\r?\n\s*self\.reset_symbol_day_order_counts\(\)\.await;'
if (Test-Path $StateRs) {
    $StateContentForG05 = Get-Content $StateRs -Raw
    $G05Matches = [regex]::Matches($StateContentForG05, $G05ResetPattern)

    if ($G05Matches.Count -ge 2) {
        Assert-Pass "G05: state.rs pairs reset_symbol_day_order_counts() with day_signal_count.store(0, ...) at $($G05Matches.Count) run-start/economic-mirror-clear boundaries"
    } else {
        Assert-Fail "G05: state.rs does not pair reset_symbol_day_order_counts() with the account-wide day_signal_count reset as expected (found $($G05Matches.Count) paired site(s), need >= 2)"
    }
} else {
    Assert-Fail "G05: state.rs not found at $StateRs"
}

# G06 -- decision.rs Gate 1f inserted between Gate 1 and Gate 1e
#
# Matched with regex (not IndexOf literal substrings): rustfmt line-wraps long
# method chains (`state\n    .method(...)\n    .await`), so an exact
# single-line substring search goes stale the moment the chain is reformatted
# even though the gate ordering and behavior are unchanged. `\s` in .NET regex
# spans newlines, so this tolerates any whitespace the formatter inserts
# between `state`, `.method(...)`, and `.await`.
if (Test-Path $DecisionRs) {
    $DecisionContent = Get-Content $DecisionRs -Raw

    $Gate1Match  = [regex]::Match($DecisionContent, 'state\s*\.\s*day_signal_limit_exceeded\(\)')
    $Gate1fMatch = [regex]::Match($DecisionContent, 'state\s*\.\s*symbol_day_order_limit_exceeded\(&decision\.symbol\)\s*\.\s*await')
    $Gate1eMatch = [regex]::Match($DecisionContent, 'evaluate_strategy_budget_from_env\(&sid\)')

    $Gate1Idx  = if ($Gate1Match.Success)  { $Gate1Match.Index }  else { -1 }
    $Gate1fIdx = if ($Gate1fMatch.Success) { $Gate1fMatch.Index } else { -1 }
    $Gate1eIdx = if ($Gate1eMatch.Success) { $Gate1eMatch.Index } else { -1 }

    if ($Gate1Idx -ge 0 -and $Gate1fIdx -gt $Gate1Idx -and $Gate1eIdx -gt $Gate1fIdx -and
        $DecisionContent -match '"symbol_day_limit_reached"') {
        Assert-Pass "G06: decision.rs Gate 1f (symbol_day_order_limit_exceeded -> 'symbol_day_limit_reached') sits between Gate 1 and Gate 1e"
    } else {
        Assert-Fail "G06: Gate 1f NOT found in expected position (between Gate 1 and Gate 1e) in decision.rs"
    }

    # G07 -- Gate 7 Ok(true) increments both counters
    #
    # Scoped to the specific outbox-enqueue match statement rather than a
    # first-occurrence 'Ok(true) => {' search, so a second, unrelated Ok(true)
    # arm elsewhere in the file (e.g. added by a later patch) cannot cause
    # this guard to inspect the wrong branch. The arm body is bounded to its
    # own closing brace (up to the sibling 'Ok(false)' arm that follows in
    # this match statement) rather than a fixed-size character window -- a
    # fixed window can silently bleed past the arm's actual close brace and
    # match content that was moved into the *next* arm, producing a false
    # pass. The counter-increment match itself is whitespace-tolerant for the
    # same rustfmt line-wrap reason as G06 above.
    $Gate7AnchorMatch = [regex]::Match($DecisionContent, 'mqk_db::outbox_enqueue\(')
    $OkTrueArmMatch = if ($Gate7AnchorMatch.Success) {
        [regex]::Match(
            $DecisionContent.Substring($Gate7AnchorMatch.Index),
            '(?s)Ok\(true\)\s*=>\s*\{(.*?)\r?\n\s*\}\r?\n\s*Ok\(false\)'
        )
    } else { [regex]::Match('', '(?!)') }
    if ($OkTrueArmMatch.Success) {
        $OkTrueBlock = $OkTrueArmMatch.Groups[1].Value
        if ($OkTrueBlock -match 'state\s*\.\s*increment_day_signal_count\(\);' -and
            $OkTrueBlock -match 'state\s*\.\s*increment_symbol_day_order_count\(&decision\.symbol\)\s*\.\s*await;') {
            Assert-Pass "G07: decision.rs Gate 7 Ok(true) arm increments both increment_day_signal_count and increment_symbol_day_order_count"
        } else {
            Assert-Fail "G07: Gate 7 Ok(true) arm does NOT increment both day-order counters"
        }
    } else {
        Assert-Fail "G07: Gate 7 'Ok(true) => { ... } Ok(false)' arm (outbox_enqueue accepted branch) not found in decision.rs"
    }

    # G08 -- module-doc gate sequence documents Gate 1f
    if ($DecisionContent -match '1f\.\s*symbol_day_order_cap') {
        Assert-Pass "G08: decision.rs module doc gate sequence documents Gate 1f (symbol_day_order_cap)"
    } else {
        Assert-Fail "G08: decision.rs module doc gate sequence does NOT document Gate 1f"
    }
} else {
    Assert-Fail "G06: decision.rs not found at $DecisionRs"
    Assert-Fail "G07: decision.rs not found at $DecisionRs"
    Assert-Fail "G08: decision.rs not found at $DecisionRs"
}

# G09 -- scenario test file has D01..D09 labels
if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $MissingLabels = [System.Collections.Generic.List[string]]::new()
    $Labels = @('d01', 'd02', 'd03', 'd04', 'd05', 'd06', 'd07', 'd08', 'd09')
    foreach ($lbl in $Labels) {
        if ($TestContent -notmatch $lbl) {
            $MissingLabels.Add($lbl)
        }
    }
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "G09: scenario_multi_symbol_day_order_cap_01.rs has D01..D09 tests"
    } else {
        Assert-Fail "G09: missing test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "G09: scenario_multi_symbol_day_order_cap_01.rs not found at $TestFile"
}

# G10 -- this patch's diff introduces no broker/OMS/portfolio direct writes, no
# order submit/cancel/replace calls, no approved_for_live references, and no
# MultiSymbolRiskCaps struct (out of scope for this patch). Checked against
# added lines only (git diff HEAD), since some files legitimately contain
# pre-existing references to these symbols elsewhere.
$DiffPaths = @($StateRs, $SignalIntake, $LifecycleRs, $DecisionRs)
Push-Location $RepoRoot
$AddedLines = git diff HEAD -- $DiffPaths |
    Where-Object { $_ -match '^\+[^+]' }
Pop-Location

$ForbiddenPattern = 'mqk_broker|mqk_portfolio::.*apply|submit_order|place_order|cancel_order|replace_order|outbox_enqueue\(|MultiSymbolRiskCaps|approved_for_live'
$ForbiddenAdded = $AddedLines | Where-Object { $_ -match $ForbiddenPattern }
if (-not $ForbiddenAdded) {
    Assert-Pass "G10: this patch's diff adds no broker/OMS/portfolio writes, order calls, MultiSymbolRiskCaps, or approved_for_live references"
} else {
    Assert-Fail "G10: this patch's diff adds an out-of-scope reference -- FORBIDDEN in this patch: $($ForbiddenAdded -join ' | ')"
}

# G11 -- design doc documents Cap #4 / Gate 1f as CLOSED (no longer OPEN)
if (Test-Path $DesignDoc) {
    $DesignContent = Get-Content $DesignDoc -Raw
    if ($DesignContent -match 'MULTI-SYMBOL-DAY-ORDER-CAP-01' -and
        $DesignContent -match 'Cap #4 `per_symbol_day_order_count_limit` \(Gate 1f\) \| \*\*CLOSED\*\*' -and
        $DesignContent -match 'symbol_day_limit_reached') {
        Assert-Pass "G11: native_multi_symbol_dispatch.md documents Cap #4 / Gate 1f (MULTI-SYMBOL-DAY-ORDER-CAP-01) as CLOSED"
    } else {
        Assert-Fail "G11: native_multi_symbol_dispatch.md does NOT document Cap #4 / Gate 1f as CLOSED"
    }
} else {
    Assert-Fail "G11: native_multi_symbol_dispatch.md not found at $DesignDoc"
}

# ---------------------------------------------------------------------------
# SELFTEST -- mutation-negative proof that G05 is not vacuously true.
#
# Re-runs the exact G05 pattern against a deliberately mutated copy of
# state.rs with the per-symbol reset removed from both paired sites
# (simulating the reset being dropped/disconnected). If the mutated content
# still matches, G05's pattern is too loose to catch a real regression.
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host '  -- SELFTEST: guard fails when the per-symbol reset is removed --'

if ($StateContentForG05) {
    $mutatedStateForG05 = $StateContentForG05 -replace `
        [regex]::Escape('self.reset_symbol_day_order_counts().await;'), `
        '/* reset removed */'
    $mutatedMatches = [regex]::Matches($mutatedStateForG05, $G05ResetPattern)
    if ($mutatedMatches.Count -eq 0) {
        Assert-Pass "SELFTEST-G05: guard correctly fails (0 paired sites) against mutated (per-symbol reset removed) content"
    } else {
        Assert-Fail "SELFTEST-G05: guard's pattern still finds $($mutatedMatches.Count) paired site(s) against mutated content -- pattern is too loose"
    }
} else {
    Assert-Fail "SELFTEST-G05: state.rs content unavailable -- cannot run mutation-negative proof"
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
