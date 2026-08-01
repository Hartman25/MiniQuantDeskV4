# =============================================================================
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A-FINAL-PRIVATE-PRODUCTION-
# EFFECTS-PROOF (R5): no default-build production-reachable Phase 7A
# ProductionRuntimeStartEffects driver or ownership-mutation bypass.
# =============================================================================
#
# Before this patch, `mqk-daemon` exposed several `pub` (non-cfg-gated)
# `_for_test` seams that could drive the real `ProductionRuntimeStartEffects`
# directly, mutate Phase 7A fault seams, or plant active
# metadata/economic-mirror state -- all reachable from any downstream crate
# or external `tests/*.rs` integration test in a normal default-feature
# build, with no compiler or feature boundary enforcing "test-only":
#
#   - drive_production_start_effects_for_test
#   - set_dynamic_selection_fault_seam_for_test
#   - commit_dynamic_selection_runtime_state_for_test
#   - plant_accepted_artifact_for_test
#   - plant_day_signal_count_for_test
#   - accepted_artifact_snapshot_for_test
#   - day_signal_count_snapshot_for_test
#
# Each is now `pub(crate)` + `#[cfg(test)]` -- reachable only from this
# crate's own test build, never from production code or an external
# integration test. This guard fails if any of them ever regress back to a
# plain `pub fn` / `pub async fn` (the exact shape of the bypass), and fails
# if the removed direct-bypass driver name ever reappears as a `pub` method.
#
# This guard is a source-level regression check; the authoritative proof is
# `cargo check -p mqk-daemon --lib` (default features) succeeding, since a
# default (non-test) build no longer sees these symbols exist at all.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_no_phase7a_production_effects_bypass.ps1
#
# Exit codes: 0 = clean, 1 = violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

Write-Host "============================================================"
Write-Host " MQK Phase 7A Production-Effects Bypass Guard"
Write-Host " Repo root: $RepoRoot"
Write-Host "============================================================"

$SrcFiles = Get-ChildItem -Path "$RepoRoot\core-rs\crates\mqk-daemon\src" -Recurse -Filter "*.rs" |
    Where-Object { $_.FullName -notmatch '\\target\\' }

$BannedSymbols = @(
    'drive_production_start_effects_for_test',
    'set_dynamic_selection_fault_seam_for_test',
    'commit_dynamic_selection_runtime_state_for_test',
    'plant_accepted_artifact_for_test',
    'plant_day_signal_count_for_test',
    'accepted_artifact_snapshot_for_test',
    'day_signal_count_snapshot_for_test'
)

# A plain `pub fn NAME` / `pub async fn NAME` is the bypass shape. Note this
# deliberately does NOT match `pub(crate) fn NAME` / `pub(crate) async fn
# NAME` -- there is no whitespace between `pub` and `(crate)`, so the regex
# below only matches genuinely crate-external-reachable `pub`.
$Violations = 0
$MatchLines = @()

foreach ($Symbol in $BannedSymbols) {
    $Pattern = "pub\s+(async\s+)?fn\s+$Symbol\b"
    foreach ($File in $SrcFiles) {
        $Found = Select-String -Path $File.FullName -Pattern $Pattern
        if ($Found) {
            $Violations++
            $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
            foreach ($Hit in $Found) {
                $MatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim())"
            }
        }
    }
}

Write-Host ""
if ($Violations -eq 0) {
    Write-Host " OK -- no default-build production-reachable Phase 7A effects-driver or ownership-mutation bypass" -ForegroundColor Green
    exit 0
} else {
    Write-Host " FAIL -- production-reachable (non-cfg, plain `pub`) Phase 7A bypass symbol(s) found:" -ForegroundColor Red
    $MatchLines | ForEach-Object { Write-Host $_ }
    Write-Host ""
    Write-Host " Remediation: these seams must be `pub(crate)` and `#[cfg(test)]` -- reachable" -ForegroundColor Red
    Write-Host " only from this crate's own test build. Move any external-test consumer of" -ForegroundColor Red
    Write-Host " them into a private in-crate `#[cfg(test)]` module instead of widening" -ForegroundColor Red
    Write-Host " visibility back to `pub`." -ForegroundColor Red
    exit 1
}
