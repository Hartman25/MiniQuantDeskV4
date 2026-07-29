# =============================================================================
# BUNDLE-7-FOUNDATION-ACCEPTANCE-REPAIR-04: no unrestricted legacy-evidence
# bypass in production src/
# =============================================================================
#
# `insert_legacy_evidence_transition_unchecked` (removed by this patch)
# bypassed `validate_fresh_evidence_bundle` and was a normal, callable public
# async function in the production mqk-db library despite "never call this
# from production" documentation -- no compiler or feature boundary enforced
# that restriction, so any current or future production crate could call it.
#
# This guard fails if that exact symbol -- or any future reintroduction of it
# -- appears anywhere under core-rs/crates/*/src/. A genuine pre-migration-
# 0058 legacy row must be seeded via direct SQL inside the test that needs
# it (see `seed_legacy_v2_absent_transition_row` in
# mqk-db/tests/scenario_strategy_promotion_registry_01.rs), never through a
# public library function.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_no_promotion_evidence_bypass.ps1
#
# Exit codes: 0 = clean, 1 = violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

Write-Host "============================================================"
Write-Host " MQK Promotion-Evidence-Bypass Guard"
Write-Host " Repo root: $RepoRoot"
Write-Host "============================================================"

$SrcFiles = Get-ChildItem -Path "$RepoRoot\core-rs\crates" -Recurse -Filter "*.rs" |
    Where-Object {
        $_.FullName -match '\\src\\' -and
        $_.FullName -notmatch '\\target\\'
    }

$Pattern = 'insert_legacy_evidence_transition_unchecked'
$Violations = 0
$MatchLines = @()

foreach ($File in $SrcFiles) {
    $Found = Select-String -Path $File.FullName -Pattern $Pattern -SimpleMatch
    if ($Found) {
        $Violations++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) {
            $MatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim())"
        }
    }
}

Write-Host ""
if ($Violations -eq 0) {
    Write-Host " OK -- no unrestricted legacy-evidence bypass symbol in production src/" -ForegroundColor Green
    exit 0
} else {
    Write-Host " FAIL -- '$Pattern' found in production src/:" -ForegroundColor Red
    $MatchLines | ForEach-Object { Write-Host $_ }
    Write-Host ""
    Write-Host " Remediation: a legacy/historical DB row must be seeded via direct SQL in the" -ForegroundColor Red
    Write-Host " test itself, or via a Cargo feature that is absent from every normal" -ForegroundColor Red
    Write-Host " workspace/default feature set and cannot be enabled transitively by" -ForegroundColor Red
    Write-Host " daemon/runtime crates -- never a normal public production function." -ForegroundColor Red
    exit 1
}
