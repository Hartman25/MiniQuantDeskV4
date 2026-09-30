# =============================================================================
# Script guard: MULTI-SYMBOL-DAY-ORDER-CAP-01
#
# Verifies that the optional per-symbol daily order count cap (cap #4,
# design doc §6, new Gate 1f):
#   1. Defines AppState::per_symbol_day_order_count_limit_from_env() reading
#      MQK_PER_SYMBOL_DAY_ORDER_LIMIT into Option<u32> (None disables Gate 1f).
#   2. Defines the per-symbol counter storage day_signal_count_by_symbol as
#      PerDomain<Arc<RwLock<HashMap<String, u32>>>> (B2.6: one independent
#      map per ExecutionDomain) and per_symbol_day_order_limit
#      (Arc<RwLock<Option<u32>>>, process-wide config) on AppState, both
#      initialized in new_inner.
#   3. Defines AppState accessor/mutator methods. The counter methods take an
#      explicit ExecutionDomain: symbol_day_order_count,
#      increment_symbol_day_order_count, symbol_day_order_limit_exceeded,
#      set_symbol_day_order_count_for_test, reset_symbol_day_order_counts.
#      The limit config methods are process-wide: per_symbol_day_order_limit,
#      set_per_symbol_day_order_limit_for_test.
#   4. state.rs pairs reset_symbol_day_order_counts(domain) with the SAME
#      domain's day_signal_count reset at the real run-reset seams (run start
#      and economic-mirror clear).
#   5. decision.rs inserts Gate 1f between Gate 1 (day_signal_limit,
#      account-wide) and Gate 1e (capital_budget), passing the same
#      ExecutionDomain as Gate 1 and producing disposition
#      "symbol_day_limit_reached" on trip.
#   6. decision.rs Gate 7 OutboxEnqueueOutcome::Enqueued arm increments both
#      the account/domain counter and the symbol/domain counter for the same
#      domain; Duplicate, RunNotRunning and Err arms (and every pre-Gate-7
#      refusal) increment neither.
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

# Returns $null when the Gate 7 structure holds, otherwise a failure reason.
function Get-Gate7Failure([string]$Content) {
    $Anchor = [regex]::Match($Content, 'mqk_db::outbox_enqueue_for_running_run\s*\(')
    if (-not $Anchor.Success) {
        return "outbox_enqueue_for_running_run call not found in decision.rs"
    }
    $Tail = $Content.Substring($Anchor.Index)

    $EnqueuedArm = [regex]::Match(
        $Tail,
        '(?s)Ok\(\s*mqk_db::OutboxEnqueueOutcome::Enqueued\s*\)\s*=>\s*\{(.*?)\r?\n\s*\}\r?\n\s*Ok\(\s*mqk_db::OutboxEnqueueOutcome::Duplicate\s*\)'
    )
    $RefusedTail = [regex]::Match(
        $Tail,
        '(?s)Ok\(\s*mqk_db::OutboxEnqueueOutcome::Duplicate\s*\)\s*=>\s*(.*?)\r?\n\}\r?\n'
    )
    if (-not $EnqueuedArm.Success -or -not $RefusedTail.Success) {
        return "typed Enqueued/Duplicate outcome arms not found in decision.rs"
    }
    $EnqueuedBlock = $EnqueuedArm.Groups[1].Value
    $RefusedBlock  = $RefusedTail.Groups[1].Value

    $Account = [regex]::Match(
        $EnqueuedBlock,
        'state\s*\.\s*increment_day_signal_count\(\s*(?:crate::state::)?ExecutionDomain::(\w+)\s*,?\s*\);'
    )
    $Symbol = [regex]::Match(
        $EnqueuedBlock,
        'state\s*\.\s*increment_symbol_day_order_count\(\s*(?:crate::state::)?ExecutionDomain::(\w+),\s*&decision\.symbol,?\s*\)\s*\.\s*await;'
    )
    if (-not $Account.Success -or -not $Symbol.Success) {
        return "Enqueued arm does not increment both the account/domain and symbol/domain counters"
    }
    if ($Account.Groups[1].Value -ne $Symbol.Groups[1].Value) {
        return "Enqueued arm increments the account and symbol counters for DIFFERENT domains"
    }
    if ($RefusedBlock -match 'increment_day_signal_count' -or
        $RefusedBlock -match 'increment_symbol_day_order_count') {
        return "a Duplicate/RunNotRunning/Err path consumes day-order quota"
    }
    foreach ($Fn in @('increment_day_signal_count', 'increment_symbol_day_order_count')) {
        $Sites = [regex]::Matches($Content, "\b$Fn\(")
        if ($Sites.Count -ne 1) {
            return "$Fn( has $($Sites.Count) call site(s) in decision.rs; only the Enqueued arm may increment"
        }
    }
    return $null
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
    # B2.6: counter methods carry an explicit ExecutionDomain (\s* tolerates
    # rustfmt wrapping the parameter list); limit-config methods stay
    # process-wide.
    $RequiredFns = @(
        'pub async fn symbol_day_order_count\(&self,\s*domain: ExecutionDomain,\s*symbol: &str\) -> u32',
        'pub\(crate\) async fn increment_symbol_day_order_count\(\s*&self,\s*domain: ExecutionDomain,\s*symbol: &str,?\s*\)',
        'pub async fn per_symbol_day_order_limit\(&self\) -> Option<u32>',
        'pub async fn symbol_day_order_limit_exceeded\(\s*&self,\s*domain: ExecutionDomain,\s*symbol: &str,?\s*\) -> bool',
        'pub fn set_symbol_day_order_count_for_test\(\s*&self,\s*domain: ExecutionDomain,\s*symbol: &str,\s*count: u32,?\s*\)',
        'pub fn set_per_symbol_day_order_limit_for_test\(&self, limit: Option<u32>\)',
        'pub async fn reset_symbol_day_order_counts\(&self, domain: ExecutionDomain\)'
    )
    $MissingFns = @($RequiredFns | Where-Object { $SignalContent -notmatch $_ })
    if ($MissingFns.Count -eq 0) {
        Assert-Pass "G03: all Gate 1f accessor/mutator methods defined on AppState with ExecutionDomain-scoped counters (signal_intake.rs)"
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

    # B2.6: per-symbol counter storage is one independent map per domain --
    # a PerDomain<...> declaration, initialized with PerDomain::new(...) over
    # two distinct Arc<RwLock<HashMap>> (not one shared Arc cloned twice).
    if ($StateContent -match 'day_signal_count_by_symbol:\s*PerDomain<Arc<RwLock<HashMap<String,\s*u32>>>>' -and
        $StateContent -match 'per_symbol_day_order_limit:\s*Arc<RwLock<Option<u32>>>' -and
        $StateContent -match 'day_signal_count_by_symbol:\s*PerDomain::new\(\s*Arc::new\(RwLock::new\(HashMap::new\(\)\)\),\s*Arc::new\(RwLock::new\(HashMap::new\(\)\)\),?\s*\)' -and
        $StateContent -match 'per_symbol_day_order_count_limit_from_env\(\)') {
        Assert-Pass "G04: day_signal_count_by_symbol is PerDomain-scoped (two independent maps) and per_symbol_day_order_limit is defined and initialized in state.rs"
    } else {
        Assert-Fail "G04: domain-scoped Gate 1f state fields NOT found in state.rs"
    }
} else {
    Assert-Fail "G04: state.rs not found at $StateRs"
}

# G05 -- per-symbol counters are reset alongside the account-wide
# day_signal_count reset at the same run-start/economic-mirror-clear
# boundary, FOR THE SAME DOMAIN. The pairing lives in state.rs at two seams:
# the run-start completion path and clear_economic_mirrors_for_run. The
# backreference \1 requires reset_symbol_day_order_counts(d) to receive the
# very same domain expression as day_signal_count.get(d), so one domain's
# account counter reset cannot be paired with another domain's per-symbol
# reset.
$G05ResetPattern = 'self\.day_signal_count\.get\((\w+)\)\.store\(0,\s*Ordering::SeqCst\);\s*\r?\n\s*self\.reset_symbol_day_order_counts\(\1\)\.await;'
if (Test-Path $StateRs) {
    $StateContentForG05 = Get-Content $StateRs -Raw
    $G05Matches = [regex]::Matches($StateContentForG05, $G05ResetPattern)

    if ($G05Matches.Count -ge 2) {
        Assert-Pass "G05: state.rs pairs reset_symbol_day_order_counts(domain) with the same-domain day_signal_count.store(0, ...) at $($G05Matches.Count) run-start/economic-mirror-clear boundaries"
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

    # B2.6: both gates receive an explicit ExecutionDomain; the internal
    # decision path is equity-only today, so both must name the SAME domain.
    $Gate1Match  = [regex]::Match($DecisionContent, 'state\s*\.\s*day_signal_limit_exceeded\(\s*(?:crate::state::)?ExecutionDomain::(\w+)\s*,?\s*\)')
    $Gate1fMatch = [regex]::Match($DecisionContent, 'state\s*\.\s*symbol_day_order_limit_exceeded\(\s*(?:crate::state::)?ExecutionDomain::(\w+),\s*&decision\.symbol,?\s*\)\s*\.\s*await')
    $Gate1eMatch = [regex]::Match($DecisionContent, 'evaluate_strategy_budget_from_env\(&sid\)')

    $Gate1Idx  = if ($Gate1Match.Success)  { $Gate1Match.Index }  else { -1 }
    $Gate1fIdx = if ($Gate1fMatch.Success) { $Gate1fMatch.Index } else { -1 }
    $Gate1eIdx = if ($Gate1eMatch.Success) { $Gate1eMatch.Index } else { -1 }

    $Gate1fDomainMatchesGate1 = (
        $Gate1Match.Success -and $Gate1fMatch.Success -and
        $Gate1Match.Groups[1].Value -eq $Gate1fMatch.Groups[1].Value
    )

    if ($Gate1Idx -ge 0 -and $Gate1fIdx -gt $Gate1Idx -and $Gate1eIdx -gt $Gate1fIdx -and
        $Gate1fDomainMatchesGate1 -and
        $DecisionContent -match '"symbol_day_limit_reached"') {
        Assert-Pass "G06: decision.rs Gate 1f (symbol_day_order_limit_exceeded(ExecutionDomain::$($Gate1fMatch.Groups[1].Value), ...) -> 'symbol_day_limit_reached') sits between Gate 1 and Gate 1e and uses Gate 1's domain"
    } else {
        Assert-Fail "G06: Gate 1f NOT found in expected position/domain (between Gate 1 and Gate 1e, same ExecutionDomain as Gate 1) in decision.rs"
    }

    # G07 -- Gate 7 Enqueued outcome increments both counters, for the same
    # domain, and nothing else does.
    #
    # The DB enqueue seam returns OutboxEnqueueOutcome. Scope the proof to the
    # Enqueued arm and bound it by the following Duplicate arm so increments
    # elsewhere cannot create a false pass. Every non-Enqueued arm
    # (Duplicate/RunNotRunning/Err, i.e. everything from the Duplicate arm to
    # the end of the enclosing function) must consume neither quota, and the
    # only increment call sites in decision.rs must be the Enqueued arm's.
    $G07Failure = Get-Gate7Failure $DecisionContent
    if ($null -eq $G07Failure) {
        Assert-Pass "G07: Gate 7 Enqueued arm increments the account/domain and symbol/domain counters for the same domain; Duplicate/refused paths consume neither"
    } else {
        Assert-Fail "G07: $G07Failure"
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
# SELFTEST -- mutation-negative proofs that G05/G07 are not vacuously true.
#
# Re-runs the exact G05 pattern / G07 checker against deliberately mutated
# copies of state.rs / decision.rs. If a mutated copy still passes, the
# guard is too loose to catch a real regression.
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host '  -- SELFTEST: guard fails when the per-symbol reset is removed --'

if ($StateContentForG05) {
    $mutatedStateForG05 = $StateContentForG05 -replace `
        [regex]::Escape('self.reset_symbol_day_order_counts(domain).await;'), `
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

if ($StateContentForG05) {
    # Reset paired with a DIFFERENT domain than the account-wide reset.
    $wrongDomainState = $StateContentForG05 -replace `
        [regex]::Escape('self.reset_symbol_day_order_counts(domain).await;'), `
        'self.reset_symbol_day_order_counts(ExecutionDomain::EquityNyse).await;'
    $wrongDomainMatches = [regex]::Matches($wrongDomainState, $G05ResetPattern)
    if ($wrongDomainMatches.Count -eq 0) {
        Assert-Pass "SELFTEST-G05-DOMAIN: guard correctly fails against mutated (per-symbol reset bound to a different domain) content"
    } else {
        Assert-Fail "SELFTEST-G05-DOMAIN: guard still finds $($wrongDomainMatches.Count) paired site(s) with a mismatched domain -- pattern is too loose"
    }
}

if ($DecisionContent) {
    $G7Mutations = [ordered]@{
        'symbol counter bound to a different domain than the account counter' =
            ($DecisionContent -replace '(increment_symbol_day_order_count\(\s*crate::state::)ExecutionDomain::EquityNyse', '${1}ExecutionDomain::Crypto24_7')
        'symbol counter increment removed from the Enqueued arm' =
            ($DecisionContent -replace '(?s)\.increment_symbol_day_order_count\(.*?\.await;', '/* removed */')
        'Duplicate arm consumes quota' =
            ($DecisionContent -replace '(Ok\(\s*mqk_db::OutboxEnqueueOutcome::Duplicate\s*\)\s*=>\s*)outcome\(', '${1}{ state.increment_day_signal_count(crate::state::ExecutionDomain::EquityNyse); outcome(')
    }
    foreach ($Name in $G7Mutations.Keys) {
        $Mutated = $G7Mutations[$Name]
        if ($Mutated -eq $DecisionContent) {
            Assert-Fail "SELFTEST-G07: mutation '$Name' did not change decision.rs -- selftest is vacuous"
        } elseif ($null -ne (Get-Gate7Failure $Mutated)) {
            Assert-Pass "SELFTEST-G07: guard correctly fails when $Name"
        } else {
            Assert-Fail "SELFTEST-G07: guard still passes when $Name -- checker is too loose"
        }
    }
} else {
    Assert-Fail "SELFTEST-G07: decision.rs content unavailable -- cannot run mutation-negative proof"
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
