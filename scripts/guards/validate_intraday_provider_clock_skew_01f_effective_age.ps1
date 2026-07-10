<#
.SYNOPSIS
  Validates the INTRADAY-PROVIDER-CLOCK-SKEW-01F effective-age recompute
  audit doc, and (once Phase B lands) the route repair itself.

.DESCRIPTION
  Static text-presence / parser checks only. No network calls, no DB access,
  no daemon/runtime interaction, no order submission. Exits 0 on all checks
  passing, 1 otherwise.
#>

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$AuditDocPath = Join-Path $RepoRoot 'docs\specs\intraday_provider_clock_skew_01f_effective_age_recompute_audit.md'
$RoutePath = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\routes\transport_quality.rs'
$ApiTypesPath = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\api_types.rs'

$failures = @()

# 1. Audit doc must exist.
if (-not (Test-Path $AuditDocPath)) {
    Write-Host "FAIL: audit doc not found at $AuditDocPath"
    exit 1
}
$auditContent = Get-Content -Path $AuditDocPath -Raw

$requiredAuditPhrases = @(
    'INTRADAY-PROVIDER-CLOCK-SKEW-01F',
    'produced_at_utc',
    'effective_latest_completed_bar_age_secs',
    'evidence_elapsed_secs',
    'snapshot age',
    'live age',
    'no gate weakening',
    'strategy',
    'forced paper order',
    'network calls in any test'
)
foreach ($phrase in $requiredAuditPhrases) {
    if ($auditContent -notmatch [regex]::Escape($phrase)) {
        $failures += "audit doc missing required phrase: '$phrase'"
    }
}

# 2. If the route repair has landed (Phase B+), verify it actually recomputes
#    effective age instead of relying on the snapshot value alone. This
#    block is skipped (not failed) when Phase B has not landed yet, so this
#    same validator is reusable across Phase A and Phase B/C commits.
if (Test-Path $RoutePath) {
    $routeContent = Get-Content -Path $RoutePath -Raw

    $routeRepairLanded = ($routeContent -match 'effective_latest_completed_bar_age_secs') -and
                          ($routeContent -match 'evidence_elapsed_secs')

    if ($routeRepairLanded) {
        if ($routeContent -notmatch 'fn\s+effective_bar_age_secs') {
            $failures += "route repair present but no 'effective_bar_age_secs' helper function found"
        }
        if ($routeContent -notmatch 'classify_proof_window_risk\s*\(\s*effective_latest_completed_bar_age_secs') {
            $failures += "classify_proof_window_risk does not appear to be called with effective_latest_completed_bar_age_secs"
        }
        # Must not widen the daemon-wide freshness cap env var.
        if ($routeContent -match 'MQK_INTRADAY_BAR_MAX_AGE_SECS\s*=') {
            $failures += "route repair appears to assign MQK_INTRADAY_BAR_MAX_AGE_SECS -- forbidden"
        }
    }

    if (Test-Path $ApiTypesPath) {
        $apiContent = Get-Content -Path $ApiTypesPath -Raw
        if ($routeRepairLanded) {
            if ($apiContent -notmatch 'effective_latest_completed_bar_age_secs') {
                $failures += "api_types.rs missing effective_latest_completed_bar_age_secs field"
            }
            if ($apiContent -notmatch 'evidence_elapsed_secs') {
                $failures += "api_types.rs missing evidence_elapsed_secs field"
            }
            # Backward compatibility: original snapshot field must remain.
            if ($apiContent -notmatch 'latest_completed_bar_age_secs') {
                $failures += "api_types.rs missing original latest_completed_bar_age_secs field (backward compatibility broken)"
            }
        }
    }
}

if ($failures.Count -gt 0) {
    Write-Host "FAIL: INTRADAY-PROVIDER-CLOCK-SKEW-01F effective-age validation failed:"
    foreach ($f in $failures) { Write-Host "  - $f" }
    exit 1
}

Write-Host "PASS: INTRADAY-PROVIDER-CLOCK-SKEW-01F effective-age recompute audit/repair validated."
exit 0
