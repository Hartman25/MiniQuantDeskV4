<#
.SYNOPSIS
  Validates docs/specs/intraday_provider_clock_skew_01a_current_truth_audit.md
  exists and contains the required grounding facts for
  INTRADAY-PROVIDER-CLOCK-SKEW-OPERATOR-GUARD-01-COMBINED Phase A.

.DESCRIPTION
  Static text-presence checks only. No network calls, no DB access, no
  daemon/runtime interaction. Exits 0 on all checks passing, 1 otherwise.
#>

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$AuditPath = Join-Path $RepoRoot 'docs\specs\intraday_provider_clock_skew_01a_current_truth_audit.md'

$failures = @()

if (-not (Test-Path $AuditPath)) {
    Write-Host "FAIL: audit doc not found at $AuditPath"
    exit 1
}

$content = Get-Content -Path $AuditPath -Raw

$requiredStrings = @(
    'INTRADAY-PROVIDER-CLOCK-SKEW-OPERATOR-GUARD-01',
    'provider_returned_stale_intraday_data',
    '913',
    '900',
    '2152',
    'must not weaken',
    'threshold changes',
    'forced paper order',
    'live routing',
    'No provider/broker/network calls in tests'
)

foreach ($needle in $requiredStrings) {
    if ($content -notmatch [regex]::Escape($needle)) {
        $failures += "missing required text: '$needle'"
    }
}

if ($failures.Count -gt 0) {
    Write-Host "FAIL: audit doc validation failed:"
    foreach ($f in $failures) { Write-Host "  - $f" }
    exit 1
}

Write-Host "PASS: $AuditPath contains all required audit facts."
exit 0
