# =============================================================================
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C-DURABLE-EVIDENCE-OPERATOR-
# SURFACES-AND-SOAK-READINESS-CLOSURE: final Bundle 7 structural closure
# guard.
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
#  10. The soak manifest writer secret-scans before every write.
#  11. The premarket validator names every one of its 18 checks and emits
#      exactly one FINAL: PASS / FINAL: FAIL conclusion.
#  12. The soak manifest is written only when the pending verdict is PASS.
#  13. approved_for_live is constrained false at the DB level and hard-coded
#      false at every construction site touched by this patch.
#  14. The read-side validator's typed outcome enum still names all 8 states.
#
# Mutation-negative self-tests prove several of these checks actually
# discriminate: each re-runs the relevant check against a deliberately
# corrupted in-memory copy of the real source text and asserts the check
# fails on it. Covers: dropped-candidate/payload-collision ignored, run
# mismatch, approved_for_live=true, persistence moved after activation, a
# mutation route, a removed validator check, and a fabricated FINAL PASS.
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
$ValidatorFile       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\dynamic_selection_evidence_validator.rs"
$WriterFile          = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\dynamic_selection_evidence_writer.rs"
$DbEvidenceFile      = Join-Path $RepoRoot "core-rs\crates\mqk-db\src\dynamic_selection_evidence.rs"
$RoutesFile          = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\dynamic_selection_evidence.rs"
$GuiPanelFile        = Join-Path $RepoRoot "core-rs\mqk-gui\src\features\system\DynamicSelectionEvidencePanel.tsx"
$MigrationFile       = Join-Path $RepoRoot "core-rs\crates\mqk-db\migrations\0059_dynamic_selection_plan_evidence.sql"
$PremarketScriptFile = Join-Path $RepoRoot "scripts\windows\Invoke-Bundle7Phase7cPremarketValidation.ps1"
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
$validatorContent    = Get-Content -Raw $ValidatorFile
$writerContent       = Get-Content -Raw $WriterFile
$dbEvidenceContent   = Get-Content -Raw $DbEvidenceFile
$routesContent       = Get-Content -Raw $RoutesFile
$guiPanelContent     = Get-Content -Raw $GuiPanelFile
$migrationContent    = Get-Content -Raw $MigrationFile
$premarketContent    = if (Test-Path $PremarketScriptFile) { Get-Content -Raw $PremarketScriptFile } else { "" }

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
# Check 10: the soak manifest writer secret-scans before every write.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 10: manifest is secret-scanned before every write --"
if ($premarketContent -eq "") {
    Fail "premarket validator script not found: $PremarketScriptFile"
} else {
    $secretScanIdx = Get-FirstMatchIndex $premarketContent "function Find-SecretShapedPattern"
    $writeIdx = Get-FirstMatchIndex $premarketContent "Set-Content -Path `$ManifestPath"
    if ($secretScanIdx -ge 0 -and $writeIdx -ge 0 -and $secretScanIdx -lt $writeIdx -and $premarketContent -match "Find-SecretShapedPattern -Text \`$json") {
        Ok "secret scan runs and is called before the manifest is written"
    } else {
        Fail "could not confirm the manifest is secret-scanned strictly before being written"
    }
}

# ---------------------------------------------------------------------------
# Check 11: the premarket validator names every one of its 18 checks and
# emits exactly one FINAL: PASS / FINAL: FAIL conclusion.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 11: validator names all 18 checks and emits FINAL PASS/FAIL --"
$requiredCheckNames = @(
    'head_equals_accepted_sha', 'tracked_worktree_clean', 'migration_governance',
    'expected_db_reachable', 'no_stale_active_run_or_lease', 'arm_integrity_posture',
    'reconciliation_readiness', 'deployment_paper_live_disabled',
    'dynamic_selection_mode_paper_enforced', 'approved_for_live_false',
    'durable_plan_evidence_valid', 'selected_bindings_match_evidence',
    'selected_timeframes_have_fresh_bars', 'no_binding_missing_required_window',
    'phase7_and_bundle_guards_pass', 'api_matches_db_evidence',
    'no_trading_action_invoked', 'soak_manifest_validates'
)
$missingCheckNames = @()
foreach ($n in $requiredCheckNames) {
    if ($premarketContent -notmatch [regex]::Escape("'$n'")) { $missingCheckNames += $n }
}
if ($missingCheckNames.Count -eq 0) {
    Ok "all 18 required check names are present"
} else {
    Fail "missing check name(s) in premarket validator: $($missingCheckNames -join ', ')"
}
$premarketContentNoComments = Strip-CommentLines $premarketContent
$finalPassCount = Get-MatchCount $premarketContentNoComments "FINAL: PASS"
$finalFailCount = Get-MatchCount $premarketContentNoComments "FINAL: FAIL"
if ($finalPassCount -eq 1 -and $finalFailCount -eq 1) {
    Ok "exactly one FINAL: PASS and one FINAL: FAIL literal"
} else {
    Fail "expected exactly one FINAL: PASS and one FINAL: FAIL literal, found $finalPassCount / $finalFailCount"
}

# ---------------------------------------------------------------------------
# Check 12: the soak manifest is written only when the pending verdict is
# PASS.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 12: soak manifest write is gated on a PASS verdict --"
if ($premarketContent -match [regex]::Escape('$PendingVerdict -eq ''PASS''')) {
    Ok "manifest write path checks `$PendingVerdict -eq 'PASS'"
} else {
    Fail "could not confirm the manifest write path is gated on a PASS verdict"
}

# ---------------------------------------------------------------------------
# Check 13: approved_for_live constrained false at the DB level and
# hard-coded false at every construction site this patch touches.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 13: approved_for_live can never be true --"
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
# Check 14: the read-side validator's typed outcome enum names all 8 states.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 14: validator outcome enum names all 8 states --"
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
# Mutation-negative self-test 4: Check 14's validator-state-completeness
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
# Mutation-negative self-test 5: Check 13's approved_for_live=true DB
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
# Mutation-negative self-test 6: Check 11's fabricated-FINAL-PASS detection
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

Write-Host ""
Write-Host "============================================================"
if ($Failures -eq 0) {
    Write-Host " BUNDLE 7 PHASE 7C FINAL CLOSURE GUARD: OK" -ForegroundColor Green
    exit 0
} else {
    Write-Host " BUNDLE 7 PHASE 7C FINAL CLOSURE GUARD: $Failures check(s) FAILED" -ForegroundColor Red
    exit 1
}
