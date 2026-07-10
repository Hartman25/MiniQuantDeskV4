<#
.SYNOPSIS
  Validates the INTRADAY-PROVIDER-CLOCK-SKEW-01D freshness-headroom guard
  added to scripts/windows/Start-PaperTradingSmoke.ps1.

.DESCRIPTION
  Static text-presence / parser checks only. No network calls, no DB access,
  no daemon/runtime interaction, no order submission. Exits 0 on all checks
  passing, 1 otherwise.
#>

$ErrorActionPreference = 'Stop'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$ScriptPath = Join-Path $RepoRoot 'scripts\windows\Start-PaperTradingSmoke.ps1'

if (-not (Test-Path $ScriptPath)) {
    Write-Host "FAIL: script not found at $ScriptPath"
    exit 1
}

$content = Get-Content -Path $ScriptPath -Raw
$failures = @()

# 1. Parser check -- must be valid PowerShell.
$parseErrors = $null
$tokens = $null
[System.Management.Automation.Language.Parser]::ParseFile($ScriptPath, [ref]$tokens, [ref]$parseErrors) | Out-Null
if ($parseErrors.Count -gt 0) {
    foreach ($e in $parseErrors) { $failures += "parse error: $($e.Message)" }
}

# 2. Parameter must exist with default 120.
if ($content -notmatch '\[int\]\s*\$MinFreshnessHeadroomSeconds\s*=\s*120') {
    $failures += "missing '[int] `$MinFreshnessHeadroomSeconds = 120' parameter declaration"
}

# 3. Must reference the route's freshness_headroom_secs field (or the
#    aggregate proof_window_ready/proof_window_risk fields it is derived from).
if (($content -notmatch 'freshness_headroom_secs') -and ($content -notmatch 'proof_window_ready')) {
    $failures += "missing reference to freshness_headroom_secs or proof_window_ready"
}

# 4. Must not edit .env.local.
if ($content -match '\.env\.local["'']?\s*[|>]') {
    $failures += "appears to write/redirect into .env.local"
}
if ($content -match 'Set-Content[^\n]*\.env\.local' -or $content -match 'Out-File[^\n]*\.env\.local' -or $content -match 'Add-Content[^\n]*\.env\.local') {
    $failures += "appears to write into .env.local"
}

# 5. Must not change MQK_INTRADAY_BAR_MAX_AGE_SECS.
if ($content -match '\$env:MQK_INTRADAY_BAR_MAX_AGE_SECS\s*=') {
    $failures += "sets MQK_INTRADAY_BAR_MAX_AGE_SECS -- forbidden (must not widen the daemon freshness cap)"
}

# 6. Must not bypass -RequireIntradayRefresh (no unconditional skip of STEP 14C).
if ($content -match 'if\s*\(\s*\$false\s*\)\s*\{\s*#?\s*RequireIntradayRefresh') {
    $failures += "RequireIntradayRefresh check appears disabled"
}
if ($content -notmatch '\[switch\]\$RequireIntradayRefresh') {
    $failures += "missing [switch]`$RequireIntradayRefresh parameter"
}

# 7. The new 01D guard block itself must not submit orders / call
#    ops-action / strategy-signal routes. (The wider script legitimately
#    calls /api/v1/ops/action elsewhere for pre-existing arm/disarm/stop
#    actions -- this check is scoped to only the new guard block so it does
#    not false-positive on that unrelated, pre-existing behavior.)
$blockStart = $content.IndexOf('INTRADAY-PROVIDER-CLOCK-SKEW-01D')
if ($blockStart -lt 0) {
    $failures += "INTRADAY-PROVIDER-CLOCK-SKEW-01D guard block marker not found"
} else {
    $blockEnd = $content.IndexOf('} else {', $blockStart)
    if ($blockEnd -lt 0) { $blockEnd = $content.Length }
    $guardBlock = $content.Substring($blockStart, $blockEnd - $blockStart)

    $forbiddenPatterns = @(
        '/api/v1/ops/action',
        '/api/v1/strategy/signal',
        'Submit-Order'
    )
    foreach ($pattern in $forbiddenPatterns) {
        if ($guardBlock -match $pattern) {
            $failures += "forbidden pattern present in 01D guard block: $pattern"
        }
    }
}

if ($failures.Count -gt 0) {
    Write-Host "FAIL: Start-PaperTradingSmoke.ps1 freshness-headroom guard validation failed:"
    foreach ($f in $failures) { Write-Host "  - $f" }
    exit 1
}

Write-Host "PASS: $ScriptPath freshness-headroom guard (INTRADAY-PROVIDER-CLOCK-SKEW-01D) validated."
exit 0
