# =============================================================================
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C-FORMAL-SOAK-GATE-TRUTH-
# REPAIR-01: final Bundle 7 structural closure guard.
#
# Proves, by direct source inspection (never merely "cargo test passed") plus
# delegation to the already-accepted Phase 7A/7B guards:
#   1. Phase 7A ownership/barrier/cleanup guard passes (delegated).
#   2. Phase 7B selected-host-only dispatch and true decision provenance
#      guard passes (delegated) -- also covers Bundle 6 -> Bundle 5 -> cap #6
#      ordering/policies unchanged and detached-provenance-lookup detection.
#   3. Exact selected-host signal journal authority (SignalEvaluationAuthority
#      ::Explicit) still drives the selected-host dispatch path.
#   4. Exactly one plan-ID derivation function exists, and the read-side
#      validator recomputes identity through that exact same function.
#   5. Exactly one transactional durable-plan writer exists (mqk-db).
#   6. Exactly one read-side evidence validator exists (mqk-daemon).
#   7. PaperEnforced evidence persistence textually precedes activation
#      (install_active_runtime / barrier release) in state/lifecycle.rs.
#   8. A payload collision is never silently ignored for PaperEnforcedAllowed.
#   9. The evidence API/GUI surfaces are read-only (no mutating axum route,
#      no mutating fetch in the GUI panel).
#  10. approved_for_live is constrained false at the DB level and hard-coded
#      false at every construction site touched by this patch.
#  11. The read-side validator's typed outcome enum still names all 8 states.
#  12. The premarket validator supports both -Stage PreStart and -Stage
#      ActiveCommit, names every check in each stage's registry, and emits
#      exactly one FINAL: PASS / FINAL: FAIL conclusion.
#  13. RequireApi=false/RequireDb=false/missing DaemonBaseUrl is rejected as
#      a hard precondition -- never downgraded to a non-blocking 'warn'.
#  14. No check in the premarket validator ever assigns a 'warn' status --
#      the WARN/HARD repair removed the escape hatch entirely, in both
#      stages, not just ActiveCommit.
#  15. Real-value checks (arm/integrity/risk/reconciliation/lease/market-data)
#      inspect actual response fields, never endpoint reachability alone.
#  16. no_binding_missing_required_window compares exact selected bindings
#      against exact readiness assignment rows -- never an unconditional
#      pass once the endpoint is reachable.
#  17. ActiveCommit's disposition/mode checks read committed_* truth, never
#      preview_* truth.
#  18. ActiveCommit's lease coherence check requires exactly one unexpired
#      lease, never a zero-lease requirement.
#  19. The formal ActiveCommit manifest binds selected_bindings with
#      timeframe_secs, and never hard-codes deployment_mode/adapter_id --
#      both are sourced from verified live system/status facts.
#  20. The formal ActiveCommit manifest is refused when run_id or plan_id is
#      null.
#  21. The status route serializes disposition through the canonical
#      as_str() string mapping, never format!("{:?}", ...) Debug text.
#  22. verify_signal_journal_attribution's exact-tuple-set matching function
#      exists (no per-binding nested nested nested-loop
#      required-all-not-any comparison).
#  23. Migration 0060 adds the exact-candidate-key UNIQUE constraint, and the
#      validator's own defense-in-depth duplicate-key check exists.
#  24. The validator's disposition/effective-mode coherence check exists.
#  25. The manifest writer secret-scans before every write (both stages).
#
# Mutation-negative self-tests prove several of these checks actually
# discriminate: each re-runs the relevant check against a deliberately
# corrupted in-memory copy of the real source text and asserts the check
# fails on it.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_bundle7_phase7c_final_closure.ps1
#
# Exit codes: 0 = clean, 1 = a check failed.
# =============================================================================

$ErrorActionPreference = "Stop"
# PS 7.3+ treats a delegated guard's native-command stderr (e.g. cargo's own
# build progress text) as a terminating error under $ErrorActionPreference=
# 'Stop', even when the stream is redirected to $null. No-op on Windows
# PowerShell 5.1, where this preference variable does not exist.
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$LifecycleFile       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\lifecycle.rs"
$StateFile           = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state.rs"
$DispatchAuthFile    = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\dynamic_selection_dispatch_authority.rs"
$StartGateFile       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\dynamic_selection_start_gate.rs"
$ValidatorFile       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\dynamic_selection_evidence_validator.rs"
$WriterFile          = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\dynamic_selection_evidence_writer.rs"
$DbEvidenceFile      = Join-Path $RepoRoot "core-rs\crates\mqk-db\src\dynamic_selection_evidence.rs"
$RoutesFile          = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\dynamic_selection_evidence.rs"
$GuiPanelFile        = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\system\DynamicSelectionEvidencePanel.tsx"
$MigrationFile       = Join-Path $RepoRoot "core-rs\crates\mqk-db\migrations\0059_dynamic_selection_plan_evidence.sql"
$Migration0060File   = Join-Path $RepoRoot "core-rs\crates\mqk-db\migrations\0060_dynamic_selection_plan_candidates_exact_key_unique.sql"
$PremarketScriptFile = Join-Path $RepoRoot "scripts\windows\Invoke-Bundle7Phase7cPremarketValidation.ps1"
$PremarketTestFile   = Join-Path $RepoRoot "scripts\windows\tests\test_bundle7_phase7c_premarket_validation.ps1"
$RunbookFile         = Join-Path $RepoRoot "docs\runbooks\autonomous_paper_ops.md"
$Phase7aGuard        = Join-Path $RepoRoot "scripts\guards\check_phase7a_final_closure.ps1"
$Phase7bGuard        = Join-Path $RepoRoot "scripts\guards\check_phase7b_selected_host_dispatch_closure.ps1"

Write-Host "============================================================"
Write-Host " MQK Bundle 7 Phase 7C Final Closure Guard"
Write-Host " Repo root: $RepoRoot"
Write-Host "============================================================"

$Failures = 0
function Fail($msg) { Write-Host " FAIL -- $msg" -ForegroundColor Red; $script:Failures++ }
function Ok($msg) { Write-Host " OK -- $msg" -ForegroundColor Green }
function Get-MatchCount($content, $pattern) {
    return ([regex]::Matches($content, [regex]::Escape($pattern))).Count
}
function Get-FirstMatchIndex($content, $pattern) {
    $m = [regex]::Match($content, [regex]::Escape($pattern))
    if ($m.Success) { return $m.Index } else { return -1 }
}
# Removes `#`-prefixed comment-only lines so structural checks (e.g. "does
# this script emit exactly one FINAL: PASS line") aren't fooled by a doc
# comment that merely *mentions* the literal while explaining the contract.
function Strip-CommentLines($content) {
    $lines = $content -split "`n" | Where-Object { $_.TrimStart() -notmatch '^#' }
    return ($lines -join "`n")
}

$lifecycleContent    = Get-Content -Raw $LifecycleFile
$stateContent        = Get-Content -Raw $StateFile
$dispatchAuthContent = Get-Content -Raw $DispatchAuthFile
$startGateContent    = Get-Content -Raw $StartGateFile
$validatorContent    = Get-Content -Raw $ValidatorFile
$writerContent       = Get-Content -Raw $WriterFile
$dbEvidenceContent   = Get-Content -Raw $DbEvidenceFile
$routesContent       = Get-Content -Raw $RoutesFile
$guiPanelContent     = Get-Content -Raw $GuiPanelFile
$migrationContent    = Get-Content -Raw $MigrationFile
$migration0060Content = if (Test-Path $Migration0060File) { Get-Content -Raw $Migration0060File } else { "" }
$premarketContent    = if (Test-Path $PremarketScriptFile) { Get-Content -Raw $PremarketScriptFile } else { "" }
$premarketTestContent = if (Test-Path $PremarketTestFile) { Get-Content -Raw $PremarketTestFile } else { "" }
$runbookContent      = if (Test-Path $RunbookFile) { Get-Content -Raw $RunbookFile } else { "" }

# ---------------------------------------------------------------------------
# Check 1/2: delegate to the already-accepted Phase 7A/7B guards.
# ---------------------------------------------------------------------------
function Invoke-DelegatedGuard {
    param([string]$Path)
    # A delegated guard's own stderr (e.g. a cargo-backed check it shells
    # out to) can be promoted to a terminating error under strict EAP on
    # Windows PowerShell 5.1, regardless of `*>` redirection here. Relax EAP
    # for just this one external-process call, then restore it.
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & powershell -NoProfile -ExecutionPolicy Bypass -File $Path *> $null
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $prevEap
    return $exitCode
}

Write-Host ""
Write-Host "-- Check 1: Phase 7A final closure guard (delegated) --"
if (Test-Path $Phase7aGuard) {
    $exitCode = Invoke-DelegatedGuard -Path $Phase7aGuard
    if ($exitCode -eq 0) { Ok "Phase 7A guard passed" } else { Fail "Phase 7A guard failed (exit $exitCode)" }
} else {
    Fail "Phase 7A guard script not found: $Phase7aGuard"
}

Write-Host ""
Write-Host "-- Check 2: Phase 7B selected-host dispatch closure guard (delegated) --"
if (Test-Path $Phase7bGuard) {
    $exitCode = Invoke-DelegatedGuard -Path $Phase7bGuard
    if ($exitCode -eq 0) { Ok "Phase 7B guard passed" } else { Fail "Phase 7B guard failed (exit $exitCode)" }
} else {
    Fail "Phase 7B guard script not found: $Phase7bGuard"
}

# ---------------------------------------------------------------------------
# Check 3: exact selected-host signal journal authority still drives the
# selected-host dispatch path (SignalEvaluationAuthority::Explicit).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 3: exact selected-host signal journal authority --"
if ($stateContent -match "SignalEvaluationAuthority::Explicit \{") {
    Ok "SignalEvaluationAuthority::Explicit construction present"
} else {
    Fail "SignalEvaluationAuthority::Explicit construction not found in state.rs"
}

# ---------------------------------------------------------------------------
# Check 4: exactly one plan-ID derivation function, and the validator
# recomputes identity through that exact same function.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 4: one plan-ID algorithm, shared by writer and validator --"
$planIdFnCount = Get-MatchCount $dispatchAuthContent "fn derive_dynamic_selection_plan_id("
if ($planIdFnCount -eq 1) {
    Ok "exactly one derive_dynamic_selection_plan_id definition"
} else {
    Fail "expected exactly one derive_dynamic_selection_plan_id definition, found $planIdFnCount"
}
if ($validatorContent -match "derive_dynamic_selection_plan_id") {
    Ok "validator imports/calls the shared derivation function"
} else {
    Fail "validator does not reference derive_dynamic_selection_plan_id -- identity recomputation may use a second algorithm"
}

# ---------------------------------------------------------------------------
# Check 5: exactly one transactional durable-plan writer (mqk-db).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 5: exactly one transactional durable-plan writer --"
$writerFnCount = Get-MatchCount $dbEvidenceContent "pub async fn insert_dynamic_selection_plan("
if ($writerFnCount -eq 1) {
    Ok "exactly one insert_dynamic_selection_plan definition"
} else {
    Fail "expected exactly one insert_dynamic_selection_plan definition, found $writerFnCount"
}
if ($dbEvidenceContent -match "pool\s*\.\s*begin\(\)" -and $dbEvidenceContent -match "tx\.commit\(\)") {
    Ok "writer uses an explicit transaction (begin/commit)"
} else {
    Fail "writer does not appear to use an explicit begin/commit transaction"
}

# ---------------------------------------------------------------------------
# Check 6: exactly one read-side evidence validator (mqk-daemon).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 6: exactly one read-side evidence validator --"
$validatorFnCount = Get-MatchCount $validatorContent "pub(crate) async fn validate_dynamic_selection_evidence("
if ($validatorFnCount -eq 1) {
    Ok "exactly one validate_dynamic_selection_evidence definition"
} else {
    Fail "expected exactly one validate_dynamic_selection_evidence definition, found $validatorFnCount"
}

# ---------------------------------------------------------------------------
# Check 7: PaperEnforced evidence persistence textually precedes activation.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 7: evidence persistence precedes activation --"
$persistPattern = "insert_dynamic_selection_plan(db, new_plan)"
$activatePattern = "self.state.install_active_runtime(run_id, handle)"
$barrierPattern = "barrier_tx.send(())"

function Test-PersistBeforeActivation($content) {
    $persistIdx = Get-FirstMatchIndex $content $persistPattern
    $activateIdx = Get-FirstMatchIndex $content $activatePattern
    $barrierIdx = Get-FirstMatchIndex $content $barrierPattern
    return ($persistIdx -ge 0 -and $activateIdx -ge 0 -and $barrierIdx -ge 0 -and
            $persistIdx -lt $activateIdx -and $persistIdx -lt $barrierIdx)
}
if (Test-PersistBeforeActivation $lifecycleContent) {
    Ok "insert_dynamic_selection_plan precedes install_active_runtime and barrier release"
} else {
    Fail "evidence persistence does not textually precede activation in state/lifecycle.rs"
}

# ---------------------------------------------------------------------------
# Check 8: a payload collision is never silently ignored for
# PaperEnforcedAllowed.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 8: PaperEnforcedAllowed payload collision is never silently ignored --"
$collisionBlockPattern = "(?s)InsertDynamicSelectionPlanOutcome::PayloadCollision\s*\{\s*detail,?\s*\}\)\s*=>\s*\{.*?PaperEnforcedAllowed.*?return Err\("
if ($lifecycleContent -match $collisionBlockPattern) {
    Ok "PayloadCollision handling returns Err for PaperEnforcedAllowed"
} else {
    Fail "could not confirm PayloadCollision is refused (returns Err) for PaperEnforcedAllowed"
}

# ---------------------------------------------------------------------------
# Check 9: API/GUI surfaces are read-only.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 9: API/GUI evidence surfaces are read-only --"
$mutatingRoutePattern = '\bpost\(|\bput\(|\bpatch\(|\bdelete\('
if ($routesContent -notmatch $mutatingRoutePattern) {
    Ok "no mutating axum route verb in routes/dynamic_selection_evidence.rs"
} else {
    Fail "routes/dynamic_selection_evidence.rs contains a mutating route verb (post/put/patch/delete)"
}
$mutatingFetchPattern = 'method:\s*["'']POST["'']|method:\s*["'']PUT["'']|method:\s*["'']DELETE["'']'
if ($guiPanelContent -notmatch $mutatingFetchPattern) {
    Ok "no mutating fetch call in DynamicSelectionEvidencePanel.tsx"
} else {
    Fail "DynamicSelectionEvidencePanel.tsx contains a mutating fetch call"
}

# ---------------------------------------------------------------------------
# Check 10: approved_for_live constrained false at the DB level and
# hard-coded false at every construction site this patch touches.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 10: approved_for_live can never be true --"
if ($migrationContent -match "CHECK \(approved_for_live = false\)") {
    Ok "DB-level CHECK constraint present in migration 0059"
} else {
    Fail "migration 0059 does not constrain approved_for_live = false at the DB level"
}
if ($writerContent -match "approved_for_live:\s*false") {
    Ok "writer hard-codes approved_for_live: false"
} else {
    Fail "writer does not hard-code approved_for_live: false"
}

# ---------------------------------------------------------------------------
# Check 11: the read-side validator's typed outcome enum names all 8 states.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 11: validator outcome enum names all 8 states --"
$requiredStates = @('Valid', 'Missing', 'Incomplete', 'IdentityMismatch', 'RunMismatch', 'CandidateMismatch', 'RuntimeMismatch', 'LiveApprovalViolation')
$missingStates = @()
foreach ($s in $requiredStates) {
    if ($validatorContent -notmatch [regex]::Escape($s)) { $missingStates += $s }
}
if ($missingStates.Count -eq 0) {
    Ok "all 8 validation states present"
} else {
    Fail "missing validation state(s) in validator: $($missingStates -join ', ')"
}

# ---------------------------------------------------------------------------
# Check 12: the premarket validator supports both stages, names every check
# in each stage's registry, and emits exactly one FINAL: PASS/FAIL.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 12: two-stage validator names all checks and emits FINAL PASS/FAIL --"
$requiredPreStartCheckNames = @(
    'head_equals_accepted_sha', 'tracked_worktree_clean', 'migration_governance',
    'require_api_and_db_true', 'expected_db_reachable',
    'no_conflicting_active_or_starting_run', 'no_unexpired_leader_lease',
    'deployment_paper_broker_config', 'live_routing_capital_disabled',
    'dynamic_selection_mode_preview_paper_enforced',
    'arm_integrity_posture_suitable_for_start', 'reconciliation_truth_acceptable',
    'required_market_data_pairs_complete_and_fresh', 'phase7_and_bundle_guards_pass',
    'no_trading_action_invoked', 'prestart_artifact_validates'
)
$requiredActiveCommitCheckNames = @(
    'head_equals_accepted_sha', 'tracked_worktree_clean', 'migration_governance',
    'require_api_and_db_true', 'expected_db_reachable', 'lifecycle_starting_or_running',
    'exactly_one_committed_active_run', 'run_lease_coherent',
    'committed_disposition_paper_enforced_allowed', 'committed_effective_mode_paper_enforced',
    'committed_live_lock_false', 'approved_for_live_false', 'committed_plan_id_present',
    'evidence_persisted_true', 'evidence_validation_state_valid', 'validation_blockers_empty',
    'api_selected_bindings_equal_durable_candidates', 'selected_bindings_have_fresh_bar_windows',
    'no_binding_missing_required_window', 'arm_integrity_reconciliation_live_routing_facts',
    'reconciliation_truth_acceptable', 'api_matches_db_evidence', 'phase7_and_bundle_guards_pass',
    'no_trading_action_invoked', 'active_commit_manifest_validates'
)
$missingCheckNames = @()
foreach ($n in ($requiredPreStartCheckNames + $requiredActiveCommitCheckNames | Select-Object -Unique)) {
    if ($premarketContent -notmatch [regex]::Escape("'$n'")) { $missingCheckNames += $n }
}
if ($missingCheckNames.Count -eq 0) {
    Ok "all required PreStart and ActiveCommit check names are present"
} else {
    Fail "missing check name(s) in premarket validator: $($missingCheckNames -join ', ')"
}
if ($premarketContent -match "ValidateSet\('PreStart',\s*'ActiveCommit'\)") {
    Ok "the validator's -Stage parameter is constrained to PreStart/ActiveCommit"
} else {
    Fail "the validator does not declare a -Stage PreStart|ActiveCommit parameter"
}
$premarketContentNoComments = Strip-CommentLines $premarketContent
$finalPassCount = Get-MatchCount $premarketContentNoComments "FINAL: PASS"
$finalFailCount = Get-MatchCount $premarketContentNoComments "FINAL: FAIL"
if ($finalPassCount -eq 1 -and $finalFailCount -eq 1) {
    Ok "exactly one FINAL: PASS and one FINAL: FAIL literal"
} else {
    Fail "expected exactly one FINAL: PASS and one FINAL: FAIL literal, found $finalPassCount / $finalFailCount"
}
if ($premarketContent -match [regex]::Escape('$PendingVerdict -eq ''PASS''')) {
    Ok "artifact/manifest write path checks `$PendingVerdict -eq 'PASS'"
} else {
    Fail "could not confirm the artifact/manifest write path is gated on a PASS verdict"
}

# ---------------------------------------------------------------------------
# Check 13: RequireApi=false/RequireDb=false/missing DaemonBaseUrl is a hard
# precondition rejection, never a downgrade.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 13: RequireApi/RequireDb/DaemonBaseUrl hard precondition --"
$hardPreconditionPattern = "(?s)function Test-RequireApiAndDbTrue.*?if \(-not \`$RequireApi\).*?if \(-not \`$RequireDb\).*?if \(\`$DaemonBaseUrl -eq ''\)"
if ($premarketContent -match $hardPreconditionPattern) {
    Ok "Test-RequireApiAndDbTrue rejects RequireApi=false, RequireDb=false, and a missing DaemonBaseUrl"
} else {
    Fail "could not confirm the RequireApi/RequireDb/DaemonBaseUrl hard-precondition check"
}
if ($premarketContent -match "Test-RequireApiAndDbTrue\s*$" -or $premarketContent -match "(?m)^Test-RequireApiAndDbTrue\s*$") {
    Ok "Test-RequireApiAndDbTrue is invoked in the check run order"
} else {
    Fail "Test-RequireApiAndDbTrue is defined but never invoked"
}

# ---------------------------------------------------------------------------
# Check 14: no check ever assigns a 'warn' status -- the WARN escape hatch
# is fully removed, in both stages.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 14: no 'warn' status is ever assigned --"
if ($premarketContentNoComments -notmatch "'warn'") {
    Ok "no 'warn' status literal anywhere in the premarket validator"
} else {
    Fail "the premarket validator still assigns a 'warn' status somewhere -- the WARN escape hatch was not fully removed"
}

# ---------------------------------------------------------------------------
# Check 15: real-value checks -- arm/integrity/risk/reconciliation/lease/
# market-data checks inspect actual response fields, never reachability
# alone.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 15: real-value checks inspect actual fields, not reachability alone --"
$realValueFieldMarkers = @(
    'risk_blocked', 'deadman_armed_state', 'readiness_state', 'lease_expired',
    'truth_state', 'leader_holder_id', 'run_owned_locally', 'kill_switch_active'
)
$missingMarkers = @()
foreach ($m in $realValueFieldMarkers) {
    if ($premarketContent -notmatch [regex]::Escape($m)) { $missingMarkers += $m }
}
if ($missingMarkers.Count -eq 0) {
    Ok "every real-value field marker is referenced by the validator"
} else {
    Fail "missing real-value field reference(s): $($missingMarkers -join ', ')"
}

# ---------------------------------------------------------------------------
# Check 16: no_binding_missing_required_window compares exact bindings
# against exact readiness rows -- never an unconditional pass.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 16: no_binding_missing_required_window is a real comparison --"
$unconditionalPassDefect = "covered by data/readiness (see prior check)"
if ($premarketContent -notmatch [regex]::Escape($unconditionalPassDefect)) {
    Ok "the prior unconditional-pass defect literal is absent"
} else {
    Fail "the prior unconditional no_binding_missing_required_window PASS literal is still present"
}
if ($premarketContent -match "effective_runtime_target_symbol" -and $premarketContent -match "effective_runtime_strategy_id" -and $premarketContent -match "effective_runtime_timeframe_secs") {
    Ok "no_binding_missing_required_window compares exact per-binding readiness fields"
} else {
    Fail "no_binding_missing_required_window does not appear to compare exact per-binding readiness fields"
}

# ---------------------------------------------------------------------------
# Check 17: ActiveCommit disposition/mode checks read committed_* truth,
# never preview_* truth.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 17: ActiveCommit reads committed truth, never preview truth --"
$activeCommitDispositionPattern = "(?s)function Test-CommittedDispositionPaperEnforcedAllowed.*?-eq 'paper_enforced_allowed'"
$m = [regex]::Match($premarketContent, $activeCommitDispositionPattern)
if ($m.Success -and $m.Value -notmatch 'preview_effective_mode|preview_configured_mode') {
    Ok "Test-CommittedDispositionPaperEnforcedAllowed does not reference preview_* fields"
} else {
    Fail "ActiveCommit's committed-disposition check appears to reference preview_* truth"
}
$activeCommitModePattern = "(?s)function Test-CommittedEffectiveModePaperEnforced.*?committed_effective_mode"
if ($premarketContent -match $activeCommitModePattern) {
    Ok "Test-CommittedEffectiveModePaperEnforced reads committed_effective_mode"
} else {
    Fail "Test-CommittedEffectiveModePaperEnforced does not appear to read committed_effective_mode"
}

# ---------------------------------------------------------------------------
# Check 18: ActiveCommit lease coherence requires exactly one unexpired
# lease, never a zero-lease requirement.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 18: ActiveCommit lease coherence requires exactly one lease, and DB/API holder+epoch agree --"
$leaseCoherentPattern = "(?s)function Test-RunLeaseCoherent.*?\`$countR\.Output -ne '1'.*?\`$dbHolder -ne \[string\]\`$apiHolder.*?\`$dbEpoch -ne \[string\]\`$apiEpoch"
if ($premarketContent -match $leaseCoherentPattern) {
    Ok "Test-RunLeaseCoherent requires exactly one singleton lease row and compares DB holder/epoch to the API"
} else {
    Fail "Test-RunLeaseCoherent does not appear to require exactly one lease row with DB/API holder+epoch agreement"
}

# ---------------------------------------------------------------------------
# Check 19: the formal manifest binds selected_bindings with timeframe_secs
# and never hard-codes deployment_mode/adapter_id.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 19: manifest bindings carry timeframe_secs; no hard-coded deployment evidence --"
$manifestBindingsPattern = "(?s)function New-Bundle7ActiveCommitManifest.*?selected_strategy_id\s*=\s*\`$_\.selected_strategy_id.*?timeframe_secs\s*=\s*\`$_\.timeframe_secs"
if ($premarketContent -match $manifestBindingsPattern) {
    Ok "New-Bundle7ActiveCommitManifest's selected_bindings carry timeframe_secs"
} else {
    Fail "New-Bundle7ActiveCommitManifest's selected_bindings do not appear to carry timeframe_secs"
}
$hardcodedEvidencePattern = "deployment_mode\s*=\s*'Paper'|broker_kind\s*=\s*'Alpaca'"
if ($premarketContent -notmatch $hardcodedEvidencePattern) {
    Ok "no hard-coded deployment_mode='Paper'/broker_kind='Alpaca' literal"
} else {
    Fail "the manifest still hard-codes deployment/broker evidence instead of sourcing it from verified facts"
}
if ($premarketContent -match [regex]::Escape('$deploymentMode = if ($null -ne $Script:SystemStatus) { $Script:SystemStatus.daemon_mode }')) {
    Ok "deployment_mode is sourced from the verified system/status response"
} else {
    Fail "could not confirm deployment_mode is sourced from a verified live fact"
}

# ---------------------------------------------------------------------------
# Check 20: the formal manifest is refused when run_id or plan_id is null.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 20: formal manifest refuses a null run_id/plan_id --"
$nullPlanRejectPattern = "(?s)function Test-ActiveCommitManifestValidates.*?if \(\`$null -eq \`$manifest\.run_id -or \`$null -eq \`$manifest\.plan_id\)"
if ($premarketContent -match $nullPlanRejectPattern) {
    Ok "Test-ActiveCommitManifestValidates rejects a null run_id/plan_id"
} else {
    Fail "Test-ActiveCommitManifestValidates does not appear to reject a null run_id/plan_id"
}

# ---------------------------------------------------------------------------
# Check 21: status route serializes disposition via as_str(), never Debug
# text.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 21: disposition is serialized via as_str(), never Debug text --"
if ($routesContent -notmatch [regex]::Escape('format!("{:?}", snapshot.disposition)')) {
    Ok "the Debug-formatted disposition literal is absent from routes/dynamic_selection_evidence.rs"
} else {
    Fail "routes/dynamic_selection_evidence.rs still Debug-formats disposition"
}
if ($routesContent -match [regex]::Escape('snapshot.disposition.as_str().to_string()')) {
    Ok "routes/dynamic_selection_evidence.rs serializes disposition via the canonical as_str() mapping"
} else {
    Fail "routes/dynamic_selection_evidence.rs does not appear to use the canonical as_str() disposition mapping"
}
if ($startGateContent -match "pub\(crate\) fn as_str\(&self\) -> &'static str" -and $startGateContent -match "pub\(crate\) fn parse\(raw: &str\) -> Option<Self>") {
    Ok "DynamicSelectionStartGateDisposition exposes the one canonical as_str()/parse() mapping"
} else {
    Fail "DynamicSelectionStartGateDisposition does not expose a canonical as_str()/parse() mapping"
}

# ---------------------------------------------------------------------------
# Check 22: verify_signal_journal_attribution's exact-tuple-set matching
# core exists -- never the prior required-all-not-any nested comparison.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 22: signal journal attribution uses exact-tuple-set matching --"
if ($validatorContent -match "fn find_signal_journal_attribution_violation\(") {
    Ok "find_signal_journal_attribution_violation exists"
} else {
    Fail "find_signal_journal_attribution_violation not found -- attribution may still use the old per-binding nested comparison"
}
if ($validatorContent -match "strategy_ids_selected" -and $validatorContent -match "std::collections::HashSet<\(String, String, String\)>") {
    Ok "attribution builds an exact allowed-tuple HashSet, not a nested per-binding loop"
} else {
    Fail "attribution does not appear to build an exact allowed-tuple set"
}

# ---------------------------------------------------------------------------
# Check 23: migration 0060 exact-candidate-key uniqueness + validator
# defense-in-depth duplicate check.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 23: exact candidate key uniqueness (DB + validator) --"
if ($migration0060Content -match "UNIQUE \(plan_id, symbol, strategy_id, timeframe_secs\)") {
    Ok "migration 0060 adds the exact-candidate-key UNIQUE constraint"
} else {
    Fail "migration 0060 does not add the expected UNIQUE (plan_id, symbol, strategy_id, timeframe_secs) constraint"
}
if ($migrationContent -match "PRIMARY KEY \(plan_id, ordinal\)") {
    Ok "migration 0059 is unmodified (still (plan_id, ordinal) primary key only)"
} else {
    Fail "migration 0059 appears to have been modified -- migrations are append-only"
}
if ($validatorContent -match "fn find_duplicate_candidate_key\(") {
    Ok "the validator's find_duplicate_candidate_key defense-in-depth check exists"
} else {
    Fail "find_duplicate_candidate_key not found in the validator"
}

# ---------------------------------------------------------------------------
# Check 24: disposition/effective-mode coherence check exists.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 24: disposition/effective-mode coherence check exists --"
if ($validatorContent -match "fn disposition_coherence_violation\(") {
    Ok "disposition_coherence_violation exists"
} else {
    Fail "disposition_coherence_violation not found in the validator"
}

# ---------------------------------------------------------------------------
# Check 25: the manifest/artifact writer secret-scans before every write, in
# both stages.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 25: manifest/artifact is secret-scanned before every write (both stages) --"
if ($premarketContent -eq "") {
    Fail "premarket validator script not found: $PremarketScriptFile"
} else {
    $secretScanIdx = Get-FirstMatchIndex $premarketContent "function Find-SecretShapedPattern"
    $activeCommitWriteIdx = Get-FirstMatchIndex $premarketContent "Join-Path `$OutputDirectory 'bundle7_soak_session_manifest.json'"
    $prestartWriteIdx = Get-FirstMatchIndex $premarketContent "Join-Path `$OutputDirectory 'bundle7_prestart_readiness_manifest.json'"
    $scanCallCount = Get-MatchCount $premarketContent "Find-SecretShapedPattern -Text `$json"
    if ($secretScanIdx -ge 0 -and $activeCommitWriteIdx -ge 0 -and $prestartWriteIdx -ge 0 -and
        $secretScanIdx -lt $activeCommitWriteIdx -and $secretScanIdx -lt $prestartWriteIdx -and $scanCallCount -ge 2) {
        Ok "secret scan runs and is called before both the PreStart artifact and ActiveCommit manifest are written"
    } else {
        Fail "could not confirm both artifact/manifest writers are secret-scanned strictly before being written"
    }
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 1: Check 7's persist-before-activation
# ordering.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 1: mutation-negative proof for persist-before-activation ordering --"
$fakeEarlyActivation = "self.state.install_active_runtime(run_id, handle)`n" + $lifecycleContent
$mutationCaught1 = -not (Test-PersistBeforeActivation $fakeEarlyActivation)
if ($mutationCaught1) {
    Ok "self-test passed: ordering check correctly FAILS when activation is moved earlier"
} else {
    Fail "self-test FAILED: ordering check did not detect activation moved before persistence"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 2: Check 8's payload-collision-not-ignored
# check.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 2: mutation-negative proof for the payload-collision check --"
$mutatedCollision = $lifecycleContent -replace [regex]::Escape("PaperEnforcedAllowed"), "SomeOtherDisposition"
$mutationCaught2 = ($mutatedCollision -notmatch $collisionBlockPattern)
if ($mutationCaught2) {
    Ok "self-test passed: collision check correctly FAILS when the PaperEnforcedAllowed guard is removed"
} else {
    Fail "self-test FAILED: collision check did not detect the PaperEnforcedAllowed guard's removal"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 3: Check 9's mutation-route detection.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 3: mutation-negative proof for the read-only route check --"
$mutatedRoutes = $routesContent + "`n.route(`"/api/v1/dynamic-selection/arm`", post(dynamic_selection_arm))`n"
$mutationCaught3 = ($mutatedRoutes -match $mutatingRoutePattern)
if ($mutationCaught3) {
    Ok "self-test passed: read-only route check correctly fires when a post( route is reintroduced"
} else {
    Fail "self-test FAILED: read-only route check did not detect a reintroduced post( route"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 4: Check 11's validator-state-completeness
# check (a removed validator state).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 4: mutation-negative proof for validator state completeness --"
$mutatedValidator = $validatorContent -replace [regex]::Escape("IdentityMismatch"), "REMOVED_STATE"
$stillPresent = 0
foreach ($s in $requiredStates) {
    if ($mutatedValidator -match [regex]::Escape($s)) { $stillPresent++ }
}
$mutationCaught4 = ($stillPresent -ne $requiredStates.Count)
if ($mutationCaught4) {
    Ok "self-test passed: state-completeness check correctly FAILS when IdentityMismatch is removed"
} else {
    Fail "self-test FAILED: state-completeness check did not detect a removed validator state"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 5: Check 10's approved_for_live=true DB
# constraint check.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 5: mutation-negative proof for the approved_for_live DB constraint check --"
$mutatedMigration = $migrationContent -replace [regex]::Escape("CHECK (approved_for_live = false)"), "-- REMOVED"
$mutationCaught5 = ($mutatedMigration -notmatch "CHECK \(approved_for_live = false\)")
if ($mutationCaught5) {
    Ok "self-test passed: DB constraint check correctly FAILS when the CHECK clause is removed"
} else {
    Fail "self-test FAILED: DB constraint check did not detect a removed CHECK (approved_for_live = false)"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 6: Check 12's fabricated-FINAL-PASS detection
# (a second unconditional FINAL: PASS injected elsewhere).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 6: mutation-negative proof for fabricated FINAL PASS detection --"
$mutatedPremarket = Strip-CommentLines ($premarketContent + "`nWrite-Host `"FINAL: PASS`"`n")
$mutatedFinalPassCount = Get-MatchCount $mutatedPremarket "FINAL: PASS"
$mutationCaught6 = ($mutatedFinalPassCount -ne 1)
if ($mutationCaught6) {
    Ok "self-test passed: FINAL PASS count check correctly FAILS when a second FINAL: PASS is injected"
} else {
    Fail "self-test FAILED: FINAL PASS count check did not detect a fabricated second FINAL: PASS"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 7: Check 6's single-read-validator check (a
# second validator function reintroduced).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 7: mutation-negative proof for the single-read-validator check --"
$mutatedValidatorDup = $validatorContent + "`npub(crate) async fn validate_dynamic_selection_evidence(x: i32) -> i32 { x }`n"
$mutatedValidatorFnCount = Get-MatchCount $mutatedValidatorDup "pub(crate) async fn validate_dynamic_selection_evidence("
$mutationCaught7 = ($mutatedValidatorFnCount -ne 1)
if ($mutationCaught7) {
    Ok "self-test passed: single-validator check correctly FAILS when a second validator function is added"
} else {
    Fail "self-test FAILED: single-validator check did not detect a duplicated validator function"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 8: Check 14's WARN-elimination check (a 'warn'
# status reintroduced).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 8: mutation-negative proof for the no-WARN-status check --"
$mutatedWithWarn = $premarketContentNoComments + "`nSet-CheckResult 'x' 'warn' 'downgraded'`n"
$mutationCaught8 = ($mutatedWithWarn -match "'warn'")
if ($mutationCaught8) {
    Ok "self-test passed: no-WARN check correctly fires when a 'warn' status is reintroduced"
} else {
    Fail "self-test FAILED: no-WARN check did not detect a reintroduced 'warn' status"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 9: Check 16's unconditional-pass detection (the
# old no_binding_missing_required_window defect literal reintroduced).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 9: mutation-negative proof for the unconditional-pass defect literal --"
$mutatedWithUnconditionalPass = $premarketContent + "`n# covered by data/readiness (see prior check)`n"
$mutationCaught9 = ($mutatedWithUnconditionalPass -match [regex]::Escape($unconditionalPassDefect))
if ($mutationCaught9) {
    Ok "self-test passed: unconditional-pass literal detection correctly fires when the defect text is reintroduced"
} else {
    Fail "self-test FAILED: unconditional-pass literal detection did not fire"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 10: Check 17's preview-truth-in-ActiveCommit
# detection.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 10: mutation-negative proof for preview-truth-in-ActiveCommit detection --"
$mutatedPreviewLeak = $premarketContent -replace [regex]::Escape('$disposition = [string]$Script:DynamicSelectionStatus.disposition'), '$disposition = [string]$Script:DynamicSelectionStatus.preview_effective_mode'
$mMutated = [regex]::Match($mutatedPreviewLeak, $activeCommitDispositionPattern)
$mutationCaught10 = -not ($mMutated.Success -and $mMutated.Value -notmatch 'preview_effective_mode|preview_configured_mode')
if ($mutationCaught10) {
    Ok "self-test passed: preview-truth detection correctly fires when committed_disposition is swapped for preview truth"
} else {
    Fail "self-test FAILED: preview-truth detection did not fire on a preview/committed swap"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 11: Check 18's zero-lease-requirement
# detection.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 11: mutation-negative proof for the zero-lease-requirement / DB-API-agreement detection --"
$mutatedZeroLease = $premarketContent -replace [regex]::Escape("`$countR.Output -ne '1'"), "`$countR.Output -ne '0'"
$mutationCaught11 = ($mutatedZeroLease -notmatch $leaseCoherentPattern)
if ($mutationCaught11) {
    Ok "self-test passed: lease-coherence check correctly FAILS when the requirement is mutated to zero-lease"
} else {
    Fail "self-test FAILED: lease-coherence check did not detect a zero-lease-requirement mutation"
}
$mutatedNoHolderCompare = $premarketContent -replace [regex]::Escape("`$dbHolder -ne [string]`$apiHolder"), "`$false"
$mutationCaught11b = ($mutatedNoHolderCompare -notmatch $leaseCoherentPattern)
if ($mutationCaught11b) {
    Ok "self-test passed: lease-coherence check correctly FAILS when the DB-vs-API holder comparison is removed"
} else {
    Fail "self-test FAILED: lease-coherence check did not detect a removed DB-vs-API holder comparison"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 12: Check 23's duplicate-candidate-key
# detection (the UNIQUE constraint removed from migration 0060).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 12: mutation-negative proof for the exact-candidate-key uniqueness check --"
$mutatedMigration0060 = $migration0060Content -replace [regex]::Escape("UNIQUE (plan_id, symbol, strategy_id, timeframe_secs)"), "-- REMOVED"
$mutationCaught12 = ($mutatedMigration0060 -notmatch "UNIQUE \(plan_id, symbol, strategy_id, timeframe_secs\)")
if ($mutationCaught12) {
    Ok "self-test passed: exact-candidate-key uniqueness check correctly FAILS when the UNIQUE constraint is removed"
} else {
    Fail "self-test FAILED: exact-candidate-key uniqueness check did not detect a removed UNIQUE constraint"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 13: Check 21's Debug-disposition detection (a
# format!("{:?}", ...) reintroduced).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 13: mutation-negative proof for Debug-disposition detection --"
$mutatedRoutesDebug = $routesContent + "`nlet x = format!(`"{:?}`", snapshot.disposition);`n"
$mutationCaught13 = ($mutatedRoutesDebug -match [regex]::Escape('format!("{:?}", snapshot.disposition)'))
if ($mutationCaught13) {
    Ok "self-test passed: Debug-disposition detection correctly fires when the Debug-format literal is reintroduced"
} else {
    Fail "self-test FAILED: Debug-disposition detection did not fire"
}

# ---------------------------------------------------------------------------
# Check 26: a positive ActiveCommit executable proof exists and writes a
# formal manifest (Active-Commit repair, Defect 1) -- never a source-string
# guard substituted for the real executable path.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 26: a positive hermetic ActiveCommit proof exists and asserts a written manifest --"
if ($premarketTestContent -match [regex]::Escape('Start-Bundle7HermeticFixture') -and
    $premarketTestContent -match [regex]::Escape("hermetic ActiveCommit run reports FINAL: PASS") -and
    $premarketTestContent -match [regex]::Escape('exactly one countable session manifest is written')) {
    Ok "the test suite drives a hermetic fixture through a real ActiveCommit run and asserts FINAL: PASS plus a written manifest"
} else {
    Fail "no positive hermetic ActiveCommit proof (fixture + FINAL: PASS + written-manifest assertion) found in the test suite"
}

# ---------------------------------------------------------------------------
# Check 27: strict deployment/adapter/live-routing null checks (Defect 3) --
# both PreStart and ActiveCommit must reject a null/missing/wrong value, not
# merely an explicit true/HALTED.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 27: strict adapter/deployment/live-routing checks reject null, in both stages --"
$adapterCheckCount = Get-MatchCount $premarketContent "adapterId.ToLowerInvariant() -ne 'alpaca'"
$deploymentStartAllowedStrictCount = Get-MatchCount $premarketContent 'deployment_start_allowed -ne $true'
$preStartLiveRoutingPattern = "(?s)function Test-LiveRoutingCapitalDisabled.*?\`$liveRouting -eq \`$false"
$activeCommitLiveRoutingPattern = "(?s)function Test-ArmIntegrityReconciliationLiveRoutingFacts.*?live_routing_enabled -ne \`$false"
if ($adapterCheckCount -ge 2 -and $deploymentStartAllowedStrictCount -ge 2 -and
    $premarketContent -match $preStartLiveRoutingPattern -and $premarketContent -match $activeCommitLiveRoutingPattern) {
    Ok "adapter_id/deployment_start_allowed/live_routing_enabled are strictly checked (fail-closed on null) in both PreStart and ActiveCommit"
} else {
    Fail "expected the strict adapter_id/deployment_start_allowed/live_routing_enabled checks in both stages (found adapter=$adapterCheckCount start_allowed=$deploymentStartAllowedStrictCount)"
}

# ---------------------------------------------------------------------------
# Check 28: ActiveCommit counts all active runs and compares the exact
# run_id -- never a run_id-scoped query alone (Defect 4).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 28: exactly-one-active-run counts ALL applicable runs and compares exact run_id --"
$exactRunPattern = "(?s)function Test-ExactlyOneCommittedActiveRun.*?count\(\*\) from runs where engine_id = 'mqk-daemon' and mode = 'PAPER' and status in \('ARMED','RUNNING'\).*?\`$durableRunId -ne \[string\]\`$activeRunId"
if ($premarketContent -match $exactRunPattern) {
    Ok "Test-ExactlyOneCommittedActiveRun counts all applicable PAPER runs and compares the exact run_id"
} else {
    Fail "Test-ExactlyOneCommittedActiveRun does not appear to count all applicable runs and compare the exact run_id"
}
if ($premarketContent -match [regex]::Escape('$dsActiveRunId -ne [string]$activeRunId')) {
    Ok "Test-ExactlyOneCommittedActiveRun requires dynamic-selection/status and control/status to agree on active_run_id"
} else {
    Fail "Test-ExactlyOneCommittedActiveRun does not appear to cross-check dynamic-selection/status against control/status active_run_id"
}

# ---------------------------------------------------------------------------
# Check 29: formal manifest identity binds adapter, live-routing, source,
# matched readiness, lease, arm, and reconciliation facts (Defect 2).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 29: formal manifest identity binds adapter/routing/source/readiness/lease/arm/reconciliation --"
$identityPattern = "(?s)\`$identityParts = \[ordered\]@\{.*?adapter_id.*?live_routing_enabled.*?source_kind.*?source_identity.*?required_market_data_pairs.*?leader_holder_id.*?leader_epoch.*?integrity_state.*?reconciliation_status.*?\}"
if ($premarketContent -match $identityPattern) {
    Ok "manifest identityParts binds adapter/live-routing/source/readiness/lease/arm/reconciliation facts"
} else {
    Fail "manifest identityParts does not appear to bind all required formal facts"
}
$activeCommitIdentityStartMatch = [regex]::Match($premarketContent, "stage\s+=\s+'active_commit'")
$activeCommitIdentityStart = if ($activeCommitIdentityStartMatch.Success) { $activeCommitIdentityStartMatch.Index } else { -1 }
$activeCommitIdentityJsonLine = "`$identityJson = `$identityParts | ConvertTo-Json -Depth 20 -Compress"
$activeCommitIdentityEnd = $premarketContent.IndexOf($activeCommitIdentityJsonLine, [Math]::Max($activeCommitIdentityStart, 0))
if ($activeCommitIdentityStart -ge 0 -and $activeCommitIdentityEnd -gt $activeCommitIdentityStart) {
    $activeCommitIdentitySpan = $premarketContent.Substring($activeCommitIdentityStart, $activeCommitIdentityEnd - $activeCommitIdentityStart)
    if ($activeCommitIdentitySpan -notmatch 'generated_at_utc') {
        Ok "generated_at_utc is never folded into the manifest identity"
    } else {
        Fail "generated_at_utc appears inside the ActiveCommit manifest's identityParts block"
    }
} else {
    Fail "could not locate the ActiveCommit identityParts block to check for generated_at_utc"
}

# ---------------------------------------------------------------------------
# Check 30: required_market_data_pairs is selected-binding-specific, never
# every unrelated configuration-preview assignment row.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 30: required_market_data_pairs is selected-binding-specific --"
$selectedSpecificPattern = "(?s)function New-Bundle7ActiveCommitManifest.*?foreach \(\`$s in \`$selectedBindings\) \{.*?\`$assignments \| Where-Object"
if ($premarketContent -match $selectedSpecificPattern) {
    Ok "required_market_data_pairs is built by matching each selected binding against readiness assignments"
} else {
    Fail "required_market_data_pairs does not appear to be selected-binding-specific"
}

# ---------------------------------------------------------------------------
# Check 31: unknown journal strategy is not skipped in strict PaperEnforced
# mode (Defect 5).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 31: strict PaperEnforced journal attribution never skips an unknown strategy_id --"
$looseSkipPattern = "(?s)if \(!strategy_ids_selected\.contains\(row\.strategy_id\.as_str\(\)\)\)\s*\{\s*continue;"
if ($validatorContent -notmatch $looseSkipPattern) {
    Ok "the loose skip-unknown-strategy branch is absent from find_signal_journal_attribution_violation"
} else {
    Fail "find_signal_journal_attribution_violation still silently skips rows for an unknown strategy_id"
}
if ($validatorContent -match [regex]::Escape('strategy_id is not part of the committed selected-host set at all')) {
    Ok "an unknown strategy_id is reported as contradictory evidence, not skipped"
} else {
    Fail "could not confirm an unknown strategy_id is rejected as contradictory evidence"
}

# ---------------------------------------------------------------------------
# Check 32: zero-selected sessions are not described as countable in the
# runbook.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 32: runbook never describes a zero-selected session as countable --"
$staleRunbookDefect = "is still a`ncountable clean session provided every check"
if ($runbookContent -notmatch [regex]::Escape($staleRunbookDefect)) {
    Ok "the stale zero-selected-counts-as-clean runbook wording is absent"
} else {
    Fail "the runbook still describes a zero-selected-pair session as countable"
}
if ($runbookContent -match [regex]::Escape('A zero-selected-pair session never counts.')) {
    Ok "the runbook explicitly states a zero-selected-pair session never counts"
} else {
    Fail "the runbook does not explicitly state that a zero-selected-pair session never counts"
}

# ---------------------------------------------------------------------------
# Mutation-negative self-test 14: Check 31's strict-journal detection (the
# loose skip branch reintroduced).
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Self-test 14: mutation-negative proof for the strict-journal-attribution check --"
$mutatedValidatorLooseSkip = $validatorContent -replace [regex]::Escape("let key = (`n            row.symbol.clone(),"), "if (!strategy_ids_selected.contains(row.strategy_id.as_str())) { continue; }`n        let key = (`n            row.symbol.clone(),"
$mutationCaught14 = ($mutatedValidatorLooseSkip -match $looseSkipPattern)
if ($mutationCaught14) {
    Ok "self-test passed: strict-journal check correctly fires when the loose skip branch is reintroduced"
} else {
    Fail "self-test FAILED: strict-journal check did not detect a reintroduced loose skip branch"
}

Write-Host ""
Write-Host "============================================================"
if ($Failures -eq 0) {
    Write-Host " BUNDLE 7 PHASE 7C FINAL CLOSURE GUARD: OK" -ForegroundColor Green
    exit 0
} else {
    Write-Host " BUNDLE 7 PHASE 7C FINAL CLOSURE GUARD: $Failures check(s) FAILED" -ForegroundColor Red
    exit 1
}
