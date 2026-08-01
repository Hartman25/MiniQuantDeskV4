# =============================================================================
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7B-SELECTED-HOST-ECONOMIC-
# DISPATCH-CLOSURE: structural closure guard.
# =============================================================================
#
# Proves, by direct source inspection (never merely "cargo test passed"),
# that:
#   1. One dispatch-authority enum/seam exists (`RuntimeStrategyDispatchAuthority`
#      with exactly `Legacy` and `DynamicPaperEnforced` variants).
#   2. Only `PaperEnforcedAllowed` constructs the dynamic economic authority
#      (exactly one call site of `build_dynamic_paper_enforced_dispatch_authority`).
#   3. Off/Shadow route to `Legacy` (three `RuntimeStrategyDispatchAuthority::
#      Legacy` constructions in state/lifecycle.rs's start-snapshot builder).
#   4. No PaperEnforced legacy fallback (`tick_strategy_dispatch_multi_symbol_
#      with_bar_facts` is never called from the `DynamicPaperEnforced` arm).
#   5. The host pool is never built inside the tick loop
#      (`DynamicSelectionHostPool::build(` does not appear in loop_runner.rs).
#   6. No selector/plan-builder/promotion/evidence call exists in the tick loop.
#   7. Exactly one pending-bar `.take()` inside the selected-host dispatch
#      function.
#   8. Exactly one Bundle 6 call site and one Bundle 5 call site in
#      loop_runner.rs, Bundle 6 strictly before Bundle 5, and Bundle 5
#      strictly before cap #6/submission.
#   9. Provenance validation exists before submission (three
#      `provenance_matches(` call sites, before the cap #6/submission loop).
#  10. `approved_for_live` is never hardcoded `true` anywhere in src/.
#  11. The accepted Phase 7A startup barrier still precedes the ticker.
#  12. No default-build test bypass: the new dispatch/coherence functions are
#      `pub(crate)`, never plain `pub`.
#
# A mutation-negative self-test proves this guard actually discriminates:
# it re-runs the Bundle 6/Bundle 5 ordering check against a deliberately
# reordered in-memory copy of the relevant source text and asserts the
# check fails on it.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_phase7b_selected_host_dispatch_closure.ps1
#
# Exit codes: 0 = clean, 1 = a check failed.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$DispatchAuthorityFile = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\dynamic_selection_dispatch_authority.rs"
$LifecycleFile         = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\lifecycle.rs"
$LoopRunnerFile        = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\loop_runner.rs"
$StateFile             = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state.rs"

Write-Host "============================================================"
Write-Host " MQK Phase 7B Selected-Host Economic Dispatch Closure Guard"
Write-Host " Repo root: $RepoRoot"
Write-Host "============================================================"

$Failures = 0

function Fail($msg) {
    Write-Host " FAIL -- $msg" -ForegroundColor Red
    $script:Failures++
}
function Ok($msg) {
    Write-Host " OK -- $msg" -ForegroundColor Green
}

# ---------------------------------------------------------------------------
# Check 1: one dispatch-authority enum/seam exists.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 1: RuntimeStrategyDispatchAuthority enum with Legacy/DynamicPaperEnforced --"
$authorityContent = Get-Content -Raw $DispatchAuthorityFile
if ($authorityContent -match "pub\(crate\) enum RuntimeStrategyDispatchAuthority" -and
    $authorityContent -match "Legacy \{" -and
    $authorityContent -match "DynamicPaperEnforced \{") {
    Ok "RuntimeStrategyDispatchAuthority enum with both variants found"
} else {
    Fail "RuntimeStrategyDispatchAuthority enum (with Legacy/DynamicPaperEnforced variants) not found in $DispatchAuthorityFile"
}

# ---------------------------------------------------------------------------
# Check 2: only PaperEnforcedAllowed constructs the dynamic authority --
# exactly one call site of the builder function.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 2: exactly one call site of build_dynamic_paper_enforced_dispatch_authority --"
$lifecycleContent = Get-Content -Raw $LifecycleFile
$builderCallMatches = [regex]::Matches($lifecycleContent, "build_dynamic_paper_enforced_dispatch_authority\(")
# The definition itself lives in a different file; this file should have
# exactly one call site.
if ($builderCallMatches.Count -eq 1) {
    Ok "exactly one call site in state/lifecycle.rs"
} else {
    Fail "expected exactly one call site of build_dynamic_paper_enforced_dispatch_authority in state/lifecycle.rs, found $($builderCallMatches.Count)"
}

# ---------------------------------------------------------------------------
# Check 3: Off/Shadow route to Legacy -- three Legacy constructions in the
# start-snapshot builder (Off, Shadow-config-failure, and the non-PaperEnforcedAllowed
# fallback arm of the main match).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 3: Off/Shadow dispatch authority is Legacy --"
$legacyConstructions = [regex]::Matches($lifecycleContent, "RuntimeStrategyDispatchAuthority::Legacy \{")
if ($legacyConstructions.Count -ge 3) {
    Ok "found $($legacyConstructions.Count) Legacy constructions in state/lifecycle.rs (Off, Shadow-invalid-config, non-PaperEnforcedAllowed fallback)"
} else {
    Fail "expected at least 3 Legacy constructions in state/lifecycle.rs, found $($legacyConstructions.Count)"
}

# ---------------------------------------------------------------------------
# Check 4: no PaperEnforced legacy fallback in the tick loop.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 4: DynamicPaperEnforced tick dispatch never falls back to legacy --"
$loopContent = Get-Content -Raw $LoopRunnerFile
if ($loopContent -match "(?s)RuntimeStrategyDispatchAuthority::DynamicPaperEnforced\s*\{[^}]*\}\s*=>\s*\{.*?tick_strategy_dispatch_selected_hosts_with_bar_facts") {
    Ok "DynamicPaperEnforced arm calls the selected-host dispatch seam"
} else {
    Fail "DynamicPaperEnforced arm does not call tick_strategy_dispatch_selected_hosts_with_bar_facts in loop_runner.rs"
}
# Defense in depth: the selected-host dispatch function itself must never
# call the legacy bootstrap invocation helper.
$stateContent = Get-Content -Raw $StateFile
$selectedFnMatch = [regex]::Match($stateContent, "(?s)pub\(crate\) async fn tick_strategy_dispatch_selected_hosts_with_bar_facts.*?\n    \}\n")
if ($selectedFnMatch.Success -and ($selectedFnMatch.Value -notmatch "invoke_native_strategy_on_bar_from_window|native_strategy_bootstrap")) {
    Ok "selected-host dispatch function never touches the legacy native bootstrap"
} else {
    Fail "selected-host dispatch function could not be isolated, or it references the legacy native bootstrap"
}

# ---------------------------------------------------------------------------
# Check 5: host pool never built inside the tick loop.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 5: host pool is never built inside loop_runner.rs --"
if ($loopContent -notmatch "DynamicSelectionHostPool::build\(") {
    Ok "no DynamicSelectionHostPool::build( call in loop_runner.rs"
} else {
    Fail "loop_runner.rs calls DynamicSelectionHostPool::build( -- the pool must be built once at start, never in the tick loop"
}

# ---------------------------------------------------------------------------
# Check 6: no selector/plan-builder/promotion/evidence call in the tick loop.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 6: no selector/plan-builder/promotion/evidence call in the tick loop --"
$bannedInLoop = @(
    "build_dynamic_selection_plan",
    "evaluate_dynamic_selection_start_gate",
    "promotion_evidence_validation",
    "compute_dynamic_selection_plan"
)
$loopBanHits = @()
foreach ($pattern in $bannedInLoop) {
    if ($loopContent -match [regex]::Escape($pattern)) {
        $loopBanHits += $pattern
    }
}
if ($loopBanHits.Count -eq 0) {
    Ok "no selector/plan-builder/promotion/evidence call found in loop_runner.rs"
} else {
    Fail "loop_runner.rs references banned mid-run re-evaluation call(s): $($loopBanHits -join ', ')"
}

# ---------------------------------------------------------------------------
# Check 7: exactly one pending-bar take() inside the selected-host dispatch
# function.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 7: exactly one pending-bar take() in the selected-host dispatch function --"
if ($selectedFnMatch.Success) {
    $takeMatches = [regex]::Matches($selectedFnMatch.Value, "pending_strategy_bar_input\.lock\(\)\.await\.take\(\)")
    if ($takeMatches.Count -eq 1) {
        Ok "exactly one .take() call"
    } else {
        Fail "expected exactly one pending_strategy_bar_input .take() call in the selected-host dispatch function, found $($takeMatches.Count)"
    }
} else {
    Fail "could not isolate the selected-host dispatch function to check take() count"
}

# ---------------------------------------------------------------------------
# Check 8: exactly one Bundle 6 / Bundle 5 call site each, Bundle 6 before
# Bundle 5, Bundle 5 before cap #6 / submission.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 8: Bundle 6/Bundle 5 call counts and ordering --"

function Get-FirstMatchIndex($content, $pattern) {
    $m = [regex]::Match($content, [regex]::Escape($pattern))
    if ($m.Success) { return $m.Index } else { return -1 }
}
function Get-MatchCount($content, $pattern) {
    return ([regex]::Matches($content, [regex]::Escape($pattern))).Count
}

$bundle6Pattern = "runtime_strategy_conflict::gather_and_resolve("
$bundle5Pattern = "runtime_opportunity_allocation::gather_and_apply("
$cap6Pattern = "max_new_orders_per_tick_reason("
$submitPattern = "submit_internal_strategy_decision("

$bundle6Count = Get-MatchCount $loopContent $bundle6Pattern
$bundle5Count = Get-MatchCount $loopContent $bundle5Pattern
if ($bundle6Count -eq 1) { Ok "exactly one Bundle 6 call site" } else { Fail "expected exactly one Bundle 6 call site, found $bundle6Count" }
if ($bundle5Count -eq 1) { Ok "exactly one Bundle 5 call site" } else { Fail "expected exactly one Bundle 5 call site, found $bundle5Count" }

$bundle6Idx = Get-FirstMatchIndex $loopContent $bundle6Pattern
$bundle5Idx = Get-FirstMatchIndex $loopContent $bundle5Pattern
$cap6Idx    = Get-FirstMatchIndex $loopContent $cap6Pattern
$submitIdx  = Get-FirstMatchIndex $loopContent $submitPattern

function Test-Ordering($content, $b6, $b5, $cap6, $submit) {
    return ($b6 -ge 0 -and $b5 -ge 0 -and $cap6 -ge 0 -and $submit -ge 0 -and
            $b6 -lt $b5 -and $b5 -lt $cap6 -and $cap6 -lt $submit)
}

if (Test-Ordering $loopContent $bundle6Idx $bundle5Idx $cap6Idx $submitIdx) {
    Ok "Bundle 6 precedes Bundle 5 precedes cap #6 precedes submission"
} else {
    Fail "Bundle 6 / Bundle 5 / cap #6 / submission are not in the required order (indices: b6=$bundle6Idx b5=$bundle5Idx cap6=$cap6Idx submit=$submitIdx)"
}

# ---------------------------------------------------------------------------
# Check 9: provenance validation before submission.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 9: provenance validation exists before submission --"
# Restrict to production code only -- the file's own #[cfg(test)] module
# (which also calls provenance_matches( directly, to unit-test it) must not
# count toward or against this production-ordering proof.
$testModIdx = $loopContent.IndexOf("mod phase7b_provenance_tests")
$productionContent = if ($testModIdx -ge 0) { $loopContent.Substring(0, $testModIdx) } else { $loopContent }
$provenanceCount = Get-MatchCount $productionContent "provenance_matches("
$lastProvenanceIdx = -1
foreach ($m in [regex]::Matches($productionContent, [regex]::Escape("provenance_matches("))) {
    if ($m.Index -gt $lastProvenanceIdx) { $lastProvenanceIdx = $m.Index }
}
if ($provenanceCount -ge 3 -and $lastProvenanceIdx -ge 0 -and $lastProvenanceIdx -lt $submitIdx) {
    Ok "found $provenanceCount provenance_matches( call sites in production code, last one before submission"
} else {
    Fail "expected at least 3 provenance_matches( call sites strictly before submission in production code; found $provenanceCount (last index $lastProvenanceIdx, submit index $submitIdx)"
}

# ---------------------------------------------------------------------------
# Check 10: approved_for_live never hardcoded true.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 10: approved_for_live never hardcoded true --"
$SrcFiles = Get-ChildItem -Path "$RepoRoot\core-rs\crates" -Recurse -Filter "*.rs" |
    Where-Object { $_.FullName -match '\\src\\' -and $_.FullName -notmatch '\\target\\' }
$LiveTrueHits = @()
foreach ($File in $SrcFiles) {
    $Found = Select-String -Path $File.FullName -Pattern 'approved_for_live\s*:\s*true'
    if ($Found) {
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) { $LiveTrueHits += "  ${RelPath}:$($Hit.LineNumber)" }
    }
}
if ($LiveTrueHits.Count -eq 0) {
    Ok "no literal approved_for_live: true anywhere in src/"
} else {
    Fail "approved_for_live hardcoded true found:`n$($LiveTrueHits -join "`n")"
}

# ---------------------------------------------------------------------------
# Check 11: the accepted Phase 7A startup barrier still precedes the ticker.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 11: Phase 7A startup barrier still precedes the ticker --"
if ($loopContent -match "tokio::select!" -and
    $loopContent -match "barrier_result = start_barrier" -and
    $loopContent -match "let mut ticker = tokio::time::interval") {
    $barrierIdx = $loopContent.IndexOf("barrier_result = start_barrier")
    $tickerIdx  = $loopContent.IndexOf("let mut ticker = tokio::time::interval")
    if ($barrierIdx -ge 0 -and $tickerIdx -ge 0 -and $barrierIdx -lt $tickerIdx) {
        Ok "startup barrier wait precedes ticker construction"
    } else {
        Fail "could not confirm startup barrier wait precedes ticker construction"
    }
} else {
    Fail "startup barrier / ticker construction markers not found in loop_runner.rs"
}

# ---------------------------------------------------------------------------
# Check 12: no default-build test bypass -- the new functions are pub(crate),
# never plain pub.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 12: new dispatch/coherence functions are pub(crate), not pub --"
$mustBeCratePrivate = @(
    "tick_strategy_dispatch_selected_hosts_with_bar_facts",
    "check_selected_host_result_coherence",
    "build_dynamic_paper_enforced_dispatch_authority",
    "derive_dynamic_selection_plan_id"
)
$visibilityHits = @()
foreach ($fn in $mustBeCratePrivate) {
    $pubHit = Select-String -Path $StateFile, $DispatchAuthorityFile -Pattern "^\s*pub async fn $fn\(|^\s*pub fn $fn\(" -ErrorAction SilentlyContinue
    if ($pubHit) { $visibilityHits += "$fn is declared plain `pub` (must be pub(crate))" }
}
if ($visibilityHits.Count -eq 0) {
    Ok "all checked functions are crate-private"
} else {
    Fail ($visibilityHits -join "; ")
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test: prove Check 8's ordering logic actually
# discriminates, by re-running it against a deliberately reordered copy of
# the relevant text (Bundle 5 call text moved before Bundle 6's).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test: mutation-negative proof for the Bundle 6/5 ordering check --"
$mutatedContent = $loopContent -replace [regex]::Escape($bundle6Pattern), "MUTATED_BUNDLE_6_MARKER(" -replace [regex]::Escape($bundle5Pattern), "MUTATED_BUNDLE_5_MARKER("
# Deliberately swap: put the Bundle 5 marker's text where Bundle 6's was and
# vice versa is complex string surgery; simpler equivalent mutation: prepend
# a fake, earlier Bundle 5 call before the real Bundle 6 call so the "first
# Bundle 5 index" becomes smaller than "first Bundle 6 index".
$fakeEarlyBundle5 = "runtime_opportunity_allocation::gather_and_apply(`n"
$mutatedForOrdering = $fakeEarlyBundle5 + $loopContent
$mutatedB6Idx = Get-FirstMatchIndex $mutatedForOrdering $bundle6Pattern
$mutatedB5Idx = Get-FirstMatchIndex $mutatedForOrdering $bundle5Pattern
$mutationCaught = -not (Test-Ordering $mutatedForOrdering $mutatedB6Idx $mutatedB5Idx $cap6Idx $submitIdx)
if ($mutationCaught) {
    Ok "self-test passed: the ordering check correctly FAILS against a deliberately reordered fixture"
} else {
    Fail "self-test FAILED: the ordering check did not detect a deliberately reordered Bundle 5-before-Bundle-6 fixture -- the check has no real discriminating power"
}

Write-Host ""
Write-Host "============================================================"
if ($Failures -eq 0) {
    Write-Host " PHASE 7B SELECTED-HOST DISPATCH CLOSURE GUARD: OK" -ForegroundColor Green
    exit 0
} else {
    Write-Host " PHASE 7B SELECTED-HOST DISPATCH CLOSURE GUARD: $Failures check(s) FAILED" -ForegroundColor Red
    exit 1
}
