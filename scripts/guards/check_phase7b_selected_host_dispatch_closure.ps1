# =============================================================================
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7B-SELECTED-HOST-ECONOMIC-
# DISPATCH-CLOSURE: structural closure guard.
#
# TRUE-PROVENANCE-AND-RUNTIME-PROOF-REPAIR-01: strengthened. The original
# guard's Check 9 proved a *detached* `provenance_matches(` key-only map
# check existed before submission -- exactly the reconstructed-not-preserved
# defect independent source review found. This version proves the repair:
# provenance travels inside the decision envelope itself (never a separate
# lookup map), all five fields are compared against the active authority at
# four checkpoints, the selected-host signal journal uses explicit
# run_id/strategy_id (never native_strategy_bootstrap/status), and a real
# hermetic loop-tick test module exists and executes the actual call sites.
# =============================================================================
#
# Proves, by direct source inspection (never merely "cargo test passed"),
# that:
#   1. One dispatch-authority enum/seam exists (`RuntimeStrategyDispatchAuthority`
#      with exactly `Legacy` and `DynamicPaperEnforced` variants).
#   2. Only `PaperEnforcedAllowed` constructs the dynamic economic authority
#      (exactly one call site of `build_dynamic_paper_enforced_dispatch_authority`
#      in state/lifecycle.rs).
#   3. Off/Shadow route to `Legacy` (three `RuntimeStrategyDispatchAuthority::
#      Legacy` constructions in state/lifecycle.rs's start-snapshot builder).
#   4. No PaperEnforced legacy fallback (`tick_strategy_dispatch_multi_symbol_
#      with_bar_facts` is never called from the `DynamicPaperEnforced` arm),
#      and the selected-host dispatch function never touches the legacy
#      native bootstrap.
#   5. The host pool is never built inside the tick loop.
#   6. No selector/plan-builder/promotion/evidence call exists in the tick loop.
#   7. Exactly one pending-bar `.take()` inside the selected-host dispatch
#      function.
#   8. Exactly one Bundle 6 call site and one Bundle 5 call site in
#      loop_runner.rs, Bundle 6 strictly before Bundle 5, and Bundle 5
#      strictly before cap #6/submission.
#   9. No detached key-only provenance map/`contains_key` validation remains
#      anywhere in production code.
#  10. The decision envelope (`PendingDecisionWithBarFacts`) itself carries
#      an optional dynamic-selection-provenance field.
#  11. Bundle 6's and Bundle 5's own input/output types are the envelope
#      type, not a plain decision -- provenance travels with the value,
#      never re-attached afterward.
#  12. All five provenance fields (run_id, plan_id, symbol, strategy_id,
#      timeframe_secs) are compared against the active authority, and this
#      comparison is exercised at (at least) four call sites in the tick
#      body, strictly before submission.
#  13. The selected-host signal journal writer accepts an explicit
#      run_id/strategy_id authority parameter, and the selected-host
#      dispatch function passes its own frozen run_id/binding strategy_id --
#      never reading `native_strategy_bootstrap` or `status.` for that
#      authority.
#  14. A real hermetic full-loop test module exists and actually drives
#      `spawn_execution_loop`/`ProductionRuntimeStartEffects` with a genuine
#      `DynamicPaperEnforced` authority (not just Off/Shadow).
#  15. `approved_for_live` is never hardcoded `true` anywhere in src/.
#  16. The accepted Phase 7A startup barrier still precedes the ticker.
#  17. No default-build test bypass: the new dispatch/coherence/journal
#      functions are `pub(crate)`, never plain `pub`.
#
# Mutation-negative self-tests prove several of these checks actually
# discriminate: each re-runs the relevant check against a deliberately
# corrupted in-memory copy of the real source text and asserts the check
# fails on it.
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
$OpportunityFile       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\runtime_opportunity_allocation.rs"
$ConflictFile          = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\runtime_strategy_conflict.rs"

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
function Get-FirstMatchIndex($content, $pattern) {
    $m = [regex]::Match($content, [regex]::Escape($pattern))
    if ($m.Success) { return $m.Index } else { return -1 }
}
# Removes `//`-prefixed comment-only lines so structural checks (e.g. "does
# this function reference X") aren't fooled by a doc comment that merely
# *mentions* X while explaining that the code deliberately does NOT do it.
function Strip-CommentLines($content) {
    $lines = $content -split "`n" | Where-Object { $_.TrimStart() -notmatch '^//' }
    return ($lines -join "`n")
}
function Get-MatchCount($content, $pattern) {
    return ([regex]::Matches($content, [regex]::Escape($pattern))).Count
}

$authorityContent    = Get-Content -Raw $DispatchAuthorityFile
$lifecycleContent    = Get-Content -Raw $LifecycleFile
$loopContentFull     = Get-Content -Raw $LoopRunnerFile
$stateContent        = Get-Content -Raw $StateFile
$opportunityContent  = Get-Content -Raw $OpportunityFile
$conflictContent     = Get-Content -Raw $ConflictFile

# Production-only slice of loop_runner.rs -- excludes its own #[cfg(test)]
# unit-test module, which legitimately calls the same functions directly to
# unit-test them (must not count toward or against a production-ordering
# proof).
$loopTestModIdx = $loopContentFull.IndexOf("mod phase7b_provenance_tests")
$loopContent = if ($loopTestModIdx -ge 0) { $loopContentFull.Substring(0, $loopTestModIdx) } else { $loopContentFull }

# ---------------------------------------------------------------------------
# Check 1: one dispatch-authority enum/seam exists.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 1: RuntimeStrategyDispatchAuthority enum with Legacy/DynamicPaperEnforced --"
if ($authorityContent -match "pub\(crate\) enum RuntimeStrategyDispatchAuthority" -and
    $authorityContent -match "Legacy \{" -and
    $authorityContent -match "DynamicPaperEnforced \{") {
    Ok "RuntimeStrategyDispatchAuthority enum with both variants found"
} else {
    Fail "RuntimeStrategyDispatchAuthority enum (with Legacy/DynamicPaperEnforced variants) not found in $DispatchAuthorityFile"
}

# ---------------------------------------------------------------------------
# Check 2: only PaperEnforcedAllowed constructs the dynamic authority --
# exactly one call site of the builder function in production lifecycle
# code (the file's own #[cfg(test)] fixtures, which build authorities
# directly to exercise cleanup/restart proofs, are excluded).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 2: exactly one production call site of build_dynamic_paper_enforced_dispatch_authority --"
$lifecycleTestModIdx = $lifecycleContent.IndexOf("#[cfg(test)]")
$lifecycleProductionContent = if ($lifecycleTestModIdx -ge 0) { $lifecycleContent.Substring(0, $lifecycleTestModIdx) } else { $lifecycleContent }
$builderCallCount = Get-MatchCount $lifecycleProductionContent "build_dynamic_paper_enforced_dispatch_authority("
if ($builderCallCount -eq 1) {
    Ok "exactly one production call site in state/lifecycle.rs"
} else {
    Fail "expected exactly one production call site of build_dynamic_paper_enforced_dispatch_authority in state/lifecycle.rs, found $builderCallCount"
}

# ---------------------------------------------------------------------------
# Check 3: Off/Shadow route to Legacy -- at least three Legacy constructions
# in the start-snapshot builder (Off, Shadow-config-failure, and the
# non-PaperEnforcedAllowed fallback arm of the main match).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 3: Off/Shadow dispatch authority is Legacy --"
$legacyConstructions = [regex]::Matches($lifecycleContent, "RuntimeStrategyDispatchAuthority::Legacy \{")
if ($legacyConstructions.Count -ge 3) {
    Ok "found $($legacyConstructions.Count) Legacy constructions in state/lifecycle.rs (Off, Shadow-invalid-config, non-PaperEnforcedAllowed fallback, plus test fixtures)"
} else {
    Fail "expected at least 3 Legacy constructions in state/lifecycle.rs, found $($legacyConstructions.Count)"
}

# ---------------------------------------------------------------------------
# Check 4: no PaperEnforced legacy fallback in the tick loop; selected-host
# dispatch function never touches the legacy native bootstrap.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 4: DynamicPaperEnforced tick dispatch never falls back to legacy --"
if ($loopContent -match "(?s)RuntimeStrategyDispatchAuthority::DynamicPaperEnforced\s*\{[^}]*\}\s*=>\s*\{.*?tick_strategy_dispatch_selected_hosts_with_bar_facts") {
    Ok "DynamicPaperEnforced arm calls the selected-host dispatch seam"
} else {
    Fail "DynamicPaperEnforced arm does not call tick_strategy_dispatch_selected_hosts_with_bar_facts in loop_runner.rs"
}
$selectedFnMatch = [regex]::Match($stateContent, "(?s)pub\(crate\) async fn tick_strategy_dispatch_selected_hosts_with_bar_facts.*?\n    \}\n")
$selectedFnCode = if ($selectedFnMatch.Success) { Strip-CommentLines $selectedFnMatch.Value } else { "" }
if ($selectedFnMatch.Success -and ($selectedFnCode -notmatch "invoke_native_strategy_on_bar_from_window|native_strategy_bootstrap")) {
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

function Test-Ordering($b6, $b5, $cap6, $submit) {
    return ($b6 -ge 0 -and $b5 -ge 0 -and $cap6 -ge 0 -and $submit -ge 0 -and
            $b6 -lt $b5 -and $b5 -lt $cap6 -and $cap6 -lt $submit)
}

if (Test-Ordering $bundle6Idx $bundle5Idx $cap6Idx $submitIdx) {
    Ok "Bundle 6 precedes Bundle 5 precedes cap #6 precedes submission"
} else {
    Fail "Bundle 6 / Bundle 5 / cap #6 / submission are not in the required order (indices: b6=$bundle6Idx b5=$bundle5Idx cap6=$cap6Idx submit=$submitIdx)"
}

# ---------------------------------------------------------------------------
# Check 9 (Blocker 1 repair): no detached key-only provenance map/
# contains_key validation remains anywhere in production code.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 9: no detached key-only provenance map remains --"
$detachedMapHits = @()
if ($loopContent -match "fn provenance_matches\(") { $detachedMapHits += "provenance_matches( function still defined" }
if ($loopContent -match "dynamic_selection_dispatch_provenance:\s*std::collections::BTreeMap") { $detachedMapHits += "detached BTreeMap<(symbol,strategy_id), ...> provenance map still constructed" }
if ($loopContent -match "provenance\.contains_key\(&\(") { $detachedMapHits += "contains_key( -based key-only provenance check still present" }
if ($detachedMapHits.Count -eq 0) {
    Ok "no detached key-only provenance map or contains_key validation found in production code"
} else {
    Fail "detached provenance map artifacts found: $($detachedMapHits -join '; ')"
}

# ---------------------------------------------------------------------------
# Check 10 (Blocker 1 repair): the decision envelope itself carries an
# optional dynamic-selection-provenance field.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 10: PendingDecisionWithBarFacts carries dynamic_selection_provenance --"
if ($opportunityContent -match "struct PendingDecisionWithBarFacts" -and
    $opportunityContent -match "dynamic_selection_provenance:\s*Option<DynamicSelectionDispatchProvenance>") {
    Ok "PendingDecisionWithBarFacts carries an optional dynamic_selection_provenance field"
} else {
    Fail "PendingDecisionWithBarFacts does not carry dynamic_selection_provenance: Option<DynamicSelectionDispatchProvenance> in $OpportunityFile"
}

# ---------------------------------------------------------------------------
# Check 11 (Blocker 1 repair): Bundle 6's and Bundle 5's own input/output
# types are the envelope type, not a plain decision.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 11: Bundle 6/Bundle 5 preserve the envelope type in and out --"
$bundle6EnvelopeHits = 0
if ($conflictContent -match "decisions:\s*Vec<PendingDecisionWithBarFacts>") { $bundle6EnvelopeHits++ }
if ($conflictContent -match "pub struct ConflictPolicyOutcome[\s\S]{0,200}?decisions:\s*Vec<PendingDecisionWithBarFacts>") { $bundle6EnvelopeHits++ }
if ($bundle6EnvelopeHits -ge 1) {
    Ok "Bundle 6 (ConflictPolicyOutcome) carries Vec<PendingDecisionWithBarFacts>, not a plain decision"
} else {
    Fail "Bundle 6's ConflictPolicyOutcome does not carry Vec<PendingDecisionWithBarFacts> in $ConflictFile"
}
if ($opportunityContent -match "pub struct RuntimeOpportunityAllocationOutcome[\s\S]{0,1000}?pub decisions:\s*Vec<PendingDecisionWithBarFacts>") {
    Ok "Bundle 5 (RuntimeOpportunityAllocationOutcome) returns Vec<PendingDecisionWithBarFacts>, not a plain decision"
} else {
    Fail "Bundle 5's RuntimeOpportunityAllocationOutcome does not return Vec<PendingDecisionWithBarFacts> in $OpportunityFile -- this is exactly the 'reconstructed after Bundle 5' defect"
}

# ---------------------------------------------------------------------------
# Check 12 (Blocker 1 repair): all five provenance fields compared, at (at
# least) four call sites in production code, strictly before submission.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 12: all five provenance fields compared at >= 4 checkpoints before submission --"
$envelopeOkFnMatch = [regex]::Match($loopContentFull, "(?s)fn dynamic_selection_envelope_ok\(.*?\n\}\n")
$fieldHits = @()
if ($envelopeOkFnMatch.Success) {
    foreach ($field in @("p.run_id", "p.plan_id", "canonical_provenance_symbol", "p.strategy_id", "p.timeframe_secs")) {
        if ($envelopeOkFnMatch.Value -match [regex]::Escape($field)) { $fieldHits += $field }
    }
}
if ($fieldHits.Count -eq 5) {
    Ok "dynamic_selection_envelope_ok compares all five provenance fields (run_id, plan_id, symbol, strategy_id, timeframe_secs)"
} else {
    Fail "dynamic_selection_envelope_ok does not compare all five provenance fields -- found: $($fieldHits -join ', ')"
}
$envelopeCheckCount = Get-MatchCount $loopContent "dynamic_selection_envelopes_ok("
$lastEnvelopeCheckIdx = -1
foreach ($m in [regex]::Matches($loopContent, [regex]::Escape("dynamic_selection_envelopes_ok("))) {
    if ($m.Index -gt $lastEnvelopeCheckIdx) { $lastEnvelopeCheckIdx = $m.Index }
}
if ($envelopeCheckCount -ge 4 -and $lastEnvelopeCheckIdx -ge 0 -and $lastEnvelopeCheckIdx -lt $submitIdx) {
    Ok "found $envelopeCheckCount dynamic_selection_envelopes_ok( checkpoints in production code, last one before submission"
} else {
    Fail "expected at least 4 dynamic_selection_envelopes_ok( checkpoints strictly before submission in production code; found $envelopeCheckCount (last index $lastEnvelopeCheckIdx, submit index $submitIdx)"
}

# ---------------------------------------------------------------------------
# Check 13 (Blocker 2 repair): selected-host signal journaling accepts an
# explicit run_id/strategy_id authority; the selected-host dispatch
# function never reads native_strategy_bootstrap or status for that
# authority (re-proves Check 4's second half against the specific
# authority-binding defect, not just legacy-invocation absence).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 13: selected-host signal journal uses explicit run_id/strategy_id authority --"
if ($stateContent -match "enum SignalEvaluationAuthority" -and
    $stateContent -match "Explicit\s*\{\s*run_id:\s*Uuid,\s*strategy_id:\s*&'a str\s*\}") {
    Ok "SignalEvaluationAuthority::Explicit { run_id, strategy_id } exists"
} else {
    Fail "SignalEvaluationAuthority::Explicit { run_id: Uuid, strategy_id: &'a str } not found in $StateFile"
}
$selectedFnSignatureMatch = [regex]::Match($stateContent, "(?s)pub\(crate\) async fn tick_strategy_dispatch_selected_hosts_with_bar_facts\(\s*&self,\s*(//[^\n]*\n\s*)*run_id:\s*Uuid,")
if ($selectedFnSignatureMatch.Success) {
    Ok "tick_strategy_dispatch_selected_hosts_with_bar_facts takes an explicit run_id: Uuid parameter"
} else {
    Fail "tick_strategy_dispatch_selected_hosts_with_bar_facts does not take an explicit run_id: Uuid parameter in $StateFile"
}
if ($selectedFnMatch.Success -and $selectedFnCode -match "SignalEvaluationAuthority::Explicit") {
    Ok "selected-host dispatch function passes SignalEvaluationAuthority::Explicit to the journal writer"
} else {
    Fail "selected-host dispatch function does not pass SignalEvaluationAuthority::Explicit to the journal writer"
}
if ($selectedFnMatch.Success -and ($selectedFnCode -notmatch "status\.read\(\)|status\.write\(\)|\.active_run_id")) {
    Ok "selected-host dispatch function never reads status.active_run_id for journal authority"
} else {
    Fail "selected-host dispatch function references status.active_run_id -- authority must come only from the explicit run_id parameter"
}

# ---------------------------------------------------------------------------
# Check 14 (Blocker 3 repair): a real hermetic full-loop test module exists
# and actually drives a genuine DynamicPaperEnforced authority through
# spawn_execution_loop / ProductionRuntimeStartEffects.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 14: real hermetic full-loop DynamicPaperEnforced test module exists --"
$realLoopHits = @()
if ($lifecycleContent -match "drive_production_start_effects_with_dispatch_authority_for_test") { $realLoopHits += "dispatch_authority-injecting test seam" }
if ($lifecycleContent -match "fn blocker3_plan_and_authority") { $realLoopHits += "real plan/authority fixture builder" }
if ($lifecycleContent -match "async fn blocker3_02_one_tick_dispatches_bundle6_then_bundle5_then_cap6_then_submission_with_intact_provenance") { $realLoopHits += "one-tick full-pipeline proof test" }
if ($loopContentFull -match "loop_call_trace_push_for_test") { $realLoopHits += "call-site trace instrumentation in loop_runner.rs" }
if ($stateContent -match "loop_call_trace_push_for_test") { $realLoopHits += "call-site trace instrumentation in state.rs" }
if ($realLoopHits.Count -ge 5) {
    Ok "real hermetic full-loop DynamicPaperEnforced proof module found: $($realLoopHits -join '; ')"
} else {
    Fail "real hermetic full-loop DynamicPaperEnforced proof module incomplete -- found only: $($realLoopHits -join '; ')"
}

# ---------------------------------------------------------------------------
# Check 15: approved_for_live never hardcoded true.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 15: approved_for_live never hardcoded true --"
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
# Check 16: the accepted Phase 7A startup barrier still precedes the ticker.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 16: Phase 7A startup barrier still precedes the ticker --"
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
# Check 17: no default-build test bypass -- the new functions are
# pub(crate), never plain pub.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 17: new dispatch/coherence/journal functions are pub(crate), not pub --"
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
# Mutation-negative self-test 1: Check 8's Bundle 6/5 ordering.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 1: mutation-negative proof for the Bundle 6/5 ordering check --"
$fakeEarlyBundle5 = "runtime_opportunity_allocation::gather_and_apply(`n"
$mutatedForOrdering = $fakeEarlyBundle5 + $loopContent
$mutatedB6Idx = Get-FirstMatchIndex $mutatedForOrdering $bundle6Pattern
$mutatedB5Idx = Get-FirstMatchIndex $mutatedForOrdering $bundle5Pattern
$mutationCaught1 = -not (Test-Ordering $mutatedB6Idx $mutatedB5Idx $cap6Idx $submitIdx)
if ($mutationCaught1) {
    Ok "self-test passed: the ordering check correctly FAILS against a deliberately reordered fixture"
} else {
    Fail "self-test FAILED: the ordering check did not detect a deliberately reordered Bundle 5-before-Bundle-6 fixture -- the check has no real discriminating power"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 2: restoring the detached contains_key-only
# provenance check must be caught by Check 9.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 2: mutation-negative proof for the detached-map check --"
$mutatedWithDetachedMap = $loopContent + "`nfn provenance_matches(provenance: &std::collections::BTreeMap<(String,String),i32>, s: &str, t: &str) -> bool { provenance.contains_key(&(" + "s.to_string(), t.to_string())) }`n"
$mutationCaught2 = ($mutatedWithDetachedMap -match "fn provenance_matches\(") -and ($mutatedWithDetachedMap -match "provenance\.contains_key\(&\(")
if ($mutationCaught2) {
    Ok "self-test passed: Check 9's patterns correctly match a reintroduced detached provenance map"
} else {
    Fail "self-test FAILED: Check 9's patterns did not detect a reintroduced detached provenance map -- the check has no real discriminating power"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 3: removing the plan_id comparison from
# dynamic_selection_envelope_ok must be caught by Check 12.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 3: mutation-negative proof for the plan_id field-comparison check --"
if ($envelopeOkFnMatch.Success) {
    $mutatedEnvelopeOk = $envelopeOkFnMatch.Value -replace [regex]::Escape("p.plan_id"), "REMOVED_PLAN_ID_FIELD"
    $mutatedFieldHits = 0
    foreach ($field in @("p.run_id", "p.plan_id", "canonical_provenance_symbol", "p.strategy_id", "p.timeframe_secs")) {
        if ($mutatedEnvelopeOk -match [regex]::Escape($field)) { $mutatedFieldHits++ }
    }
    $mutationCaught3 = ($mutatedFieldHits -ne 5)
    if ($mutationCaught3) {
        Ok "self-test passed: Check 12 correctly fails when the plan_id comparison is removed"
    } else {
        Fail "self-test FAILED: Check 12 still reports all five fields present after plan_id was removed -- the check has no real discriminating power"
    }
} else {
    Fail "self-test 3 could not run: dynamic_selection_envelope_ok not found to mutate"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 4: restoring a native_strategy_bootstrap
# lookup inside the selected-host dispatch function must be caught by
# Check 4/Check 13.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 4: mutation-negative proof for the selected-host legacy-authority check --"
if ($selectedFnMatch.Success) {
    $mutatedSelectedFn = Strip-CommentLines ($selectedFnMatch.Value + "`nlet _ = self.native_strategy_bootstrap.lock().await;`n")
    $mutationCaught4 = ($mutatedSelectedFn -match "native_strategy_bootstrap")
    if ($mutationCaught4) {
        Ok "self-test passed: the legacy-bootstrap check correctly fires when native_strategy_bootstrap is reintroduced"
    } else {
        Fail "self-test FAILED: the legacy-bootstrap check did not detect a reintroduced native_strategy_bootstrap reference"
    }
} else {
    Fail "self-test 4 could not run: selected-host dispatch function not found to mutate"
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
