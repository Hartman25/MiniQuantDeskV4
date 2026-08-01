# =============================================================================
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A-FINAL-PRIVATE-PRODUCTION-
# EFFECTS-PROOF: final Phase 7A closure guard.
# =============================================================================
#
# Scope: this guard checks exactly the invariants this specific patch
# (R5/R6) could plausibly have disturbed. It deliberately does NOT re-derive
# invariants that earlier, already-closed Phase 7A patches proved (frozen
# start authority, ownership-is-sole-lifecycle-authority, reserve-before-
# run-link, barrier-before-ticker as a general property) -- those remain
# proven by the existing scenario/unit test suite continuing to pass
# (`cargo test -p mqk-daemon --lib`), which is the authoritative evidence
# for them, not a syntactic guard. Re-deriving them here via text matching
# would be a weaker, redundant proxy for a check `cargo test` already does
# properly.
#
# What this guard actually proves:
#   1. No default-build production-reachable Phase 7A effects-driver or
#      ownership-mutation bypass (delegates to
#      check_no_phase7a_production_effects_bypass.ps1).
#   2. The `paper_enforced` -> Phase 7B-dispatch-not-wired interlock string
#      is still present verbatim in state/lifecycle.rs -- Phase 7B
#      selected-host economic dispatch has not been silently wired in by
#      this patch.
#   3. The economic tick body inside `spawn_execution_loop` is unchanged,
#      by running the crate's OWN established structural guard test
#      (`scenario_bundle7_phase7a_loop_runner_unchanged_guard_01`) rather
#      than re-deriving a parallel, weaker byte-diff here. That test's own
#      `STARTING_HEAD` constant is this patch's required starting commit
#      (985dd0d1) -- each Phase 7A patch rebases it to its own starting
#      HEAD, per that file's documented convention, composing transitively
#      back to the original frozen reference `9323b769`.
#   4. No source file assigns a literal `true` to `approved_for_live` --
#      `approved_for_live` must remain hardcoded false everywhere it is
#      constructed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_phase7a_final_closure.ps1
#
# Exit codes: 0 = clean, 1 = a check failed.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$FrozenRef = "9323b769"

Write-Host "============================================================"
Write-Host " MQK Phase 7A Final Closure Guard"
Write-Host " Repo root: $RepoRoot"
Write-Host " Frozen economic-tick-body reference: $FrozenRef"
Write-Host "============================================================"

$Failures = 0

# ---------------------------------------------------------------------------
# Check 1: delegate to the bypass guard.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 1: no default-build production-effects bypass --"
& powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $ScriptDir "check_no_phase7a_production_effects_bypass.ps1")
if ($LASTEXITCODE -ne 0) {
    Write-Host " FAIL -- bypass guard failed" -ForegroundColor Red
    $Failures++
} else {
    Write-Host " OK" -ForegroundColor Green
}

# ---------------------------------------------------------------------------
# Check 2: the paper_enforced / Phase-7B-not-wired interlock string.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 2: paper_enforced dispatch-not-wired interlock present --"
$LifecycleFile = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\lifecycle.rs"
$InterlockHit = Select-String -Path $LifecycleFile -Pattern "runtime.start_refused.dynamic_selection_dispatch_not_wired" -SimpleMatch
if (-not $InterlockHit) {
    Write-Host " FAIL -- interlock fault_class string not found in $LifecycleFile" -ForegroundColor Red
    $Failures++
} else {
    Write-Host " OK -- interlock present at line $($InterlockHit[0].LineNumber)" -ForegroundColor Green
}

# ---------------------------------------------------------------------------
# Check 3: economic tick body unchanged -- run the crate's own established
# structural guard test rather than re-deriving a parallel byte-diff here.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 3: economic tick body unchanged (delegates to the crate's own guard test) --"
Push-Location (Join-Path $RepoRoot "core-rs")
try {
    & cargo test -p mqk-daemon --test scenario_bundle7_phase7a_loop_runner_unchanged_guard_01
    if ($LASTEXITCODE -ne 0) {
        Write-Host " FAIL -- scenario_bundle7_phase7a_loop_runner_unchanged_guard_01 failed" -ForegroundColor Red
        $Failures++
    } else {
        Write-Host " OK" -ForegroundColor Green
    }
} finally {
    Pop-Location
}

# ---------------------------------------------------------------------------
# Check 4: no literal `approved_for_live: true`.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "-- Check 4: approved_for_live never hardcoded true --"
$SrcFiles = Get-ChildItem -Path "$RepoRoot\core-rs\crates" -Recurse -Filter "*.rs" |
    Where-Object { $_.FullName -match '\\src\\' -and $_.FullName -notmatch '\\target\\' }
$LiveTrueHits = @()
foreach ($File in $SrcFiles) {
    $Found = Select-String -Path $File.FullName -Pattern 'approved_for_live\s*:\s*true'
    if ($Found) {
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) {
            $LiveTrueHits += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim())"
        }
    }
}
if ($LiveTrueHits.Count -eq 0) {
    Write-Host " OK -- no literal approved_for_live: true anywhere in src/" -ForegroundColor Green
} else {
    Write-Host " FAIL -- approved_for_live hardcoded true found:" -ForegroundColor Red
    $LiveTrueHits | ForEach-Object { Write-Host $_ }
    $Failures++
}

Write-Host ""
Write-Host "============================================================"
if ($Failures -eq 0) {
    Write-Host " PHASE 7A FINAL CLOSURE GUARD: OK" -ForegroundColor Green
    exit 0
} else {
    Write-Host " PHASE 7A FINAL CLOSURE GUARD: $Failures check(s) FAILED" -ForegroundColor Red
    exit 1
}
