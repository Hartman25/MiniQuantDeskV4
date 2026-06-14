# =============================================================================
# Script guard: WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01 (Patch 11B)
#
# Verifies the multi-symbol dispatch summary extension to the paper-smoke
# evidence capture/review scripts:
#
#  - Capture-PaperSmokeEvidence.ps1 captures GET /api/v1/strategy/multi-symbol-
#    dispatch-summary to multi_symbol_dispatch_summary.json via the GET-only,
#    fail-soft Save-DaemonJson helper. Not mandatory; no mutation calls.
#  - Review-PaperSmokeEvidence.ps1 reads multi_symbol_dispatch_summary.json,
#    renders a dedicated "Multi-symbol dispatch summary" section, and exposes
#    the captured/truth_state/per-symbol fields in review_summary.json.
#  - Per-symbol fields (symbol, strategy_id, current_qty, target_qty, delta,
#    no_order_reason, last_decision_id, last_decision_disposition,
#    day_order_count, day_order_limit, bar_staleness_secs) are surfaced
#    verbatim -- no fabricated rows, no inferred trade success.
#  - schema_version remains 'review-v2' (additive only).
#  - MS-EV-01..14 test coverage exists in the capture/review guard tests.
#  - No forbidden files touched (core-rs/, mqk-gui/, watchlist_promotion.py
#    and its tests, mqk-execution/mqk-broker-*/mqk-risk/mqk-reconcile/
#    mqk-portfolio).
#  - Docs mark WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01 CLOSED while
#    the parent WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01 remains OPEN.
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# Exit codes: 0 = all assertions pass, 1 = any assertion failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$Failures  = [System.Collections.Generic.List[string]]::new()
$PassCount = 0

function Assert-Pass([string]$Msg) {
    $script:PassCount++
    Write-Host "  OK:   $Msg" -ForegroundColor Green
}

function Assert-Fail([string]$Msg) {
    $script:Failures.Add("  FAIL: $Msg")
    Write-Host "  FAIL: $Msg" -ForegroundColor Red
}

$RepoRoot     = (Resolve-Path "$PSScriptRoot\..\..\").Path
$CaptureFile  = Join-Path $RepoRoot "scripts\windows\Capture-PaperSmokeEvidence.ps1"
$ReviewFile   = Join-Path $RepoRoot "scripts\windows\Review-PaperSmokeEvidence.ps1"
$CaptureTest  = Join-Path $RepoRoot "tests\script_guards\test_capture_paper_smoke_evidence.ps1"
$ReviewTest   = Join-Path $RepoRoot "tests\script_guards\test_paper_smoke_evidence_review.ps1"
$DesignDoc    = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"
$EvidenceDoc  = Join-Path $RepoRoot "docs\runbooks\paper_smoke_evidence_pack.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_multi_symbol_smoke_evidence.ps1'
Write-Host '============================================================'

# -----------------------------------------------------------------------
# G01: Capture script exists
# -----------------------------------------------------------------------
if (Test-Path $CaptureFile) {
    Assert-Pass "G01: Capture-PaperSmokeEvidence.ps1 exists"
} else {
    Assert-Fail "G01: Capture-PaperSmokeEvidence.ps1 not found at $CaptureFile"
}

# -----------------------------------------------------------------------
# G02: Review script exists
# -----------------------------------------------------------------------
if (Test-Path $ReviewFile) {
    Assert-Pass "G02: Review-PaperSmokeEvidence.ps1 exists"
} else {
    Assert-Fail "G02: Review-PaperSmokeEvidence.ps1 not found at $ReviewFile"
}

$CaptureContent = if (Test-Path $CaptureFile) { Get-Content $CaptureFile -Raw } else { '' }
$ReviewContent  = if (Test-Path $ReviewFile) { Get-Content $ReviewFile -Raw } else { '' }

# -----------------------------------------------------------------------
# G03: Capture script references the multi-symbol-dispatch-summary route
# -----------------------------------------------------------------------
if ($CaptureContent -match [regex]::Escape('/api/v1/strategy/multi-symbol-dispatch-summary')) {
    Assert-Pass "G03: Capture script references /api/v1/strategy/multi-symbol-dispatch-summary"
} else {
    Assert-Fail "G03: Capture script does not reference /api/v1/strategy/multi-symbol-dispatch-summary"
}

# -----------------------------------------------------------------------
# G04: Captured via Save-DaemonJson (GET-only, fail-soft) to
# multi_symbol_dispatch_summary.json
# -----------------------------------------------------------------------
if ($CaptureContent -match "Save-DaemonJson\s+'/api/v1/strategy/multi-symbol-dispatch-summary'\s+'multi_symbol_dispatch_summary\.json'") {
    Assert-Pass "G04: multi_symbol_dispatch_summary.json captured via Save-DaemonJson (GET-only, fail-soft)"
} else {
    Assert-Fail "G04: multi_symbol_dispatch_summary.json capture not found via Save-DaemonJson"
}

# -----------------------------------------------------------------------
# G05: Capture script header doc-comment lists multi_symbol_dispatch_summary.json
# among the captured artifacts
# -----------------------------------------------------------------------
if ($CaptureContent -match '#\s*multi_symbol_dispatch_summary\.json') {
    Assert-Pass "G05: Capture script header doc-comment lists multi_symbol_dispatch_summary.json"
} else {
    Assert-Fail "G05: Capture script header doc-comment does not list multi_symbol_dispatch_summary.json"
}

# -----------------------------------------------------------------------
# G06: Review script reads multi_symbol_dispatch_summary.json via Read-JsonSnapshot
# -----------------------------------------------------------------------
if ($ReviewContent -match 'Read-JsonSnapshot\s*\(Join-Path\s+\$ApiDir\s+''multi_symbol_dispatch_summary\.json''\)') {
    Assert-Pass "G06: Review script reads multi_symbol_dispatch_summary.json via Read-JsonSnapshot"
} else {
    Assert-Fail "G06: Review script does not read multi_symbol_dispatch_summary.json via Read-JsonSnapshot"
}

# -----------------------------------------------------------------------
# G07: Review script renders a "Multi-symbol dispatch summary" section
# -----------------------------------------------------------------------
if ($ReviewContent -match '(?i)## Multi-symbol dispatch summary') {
    Assert-Pass "G07: Review script renders a 'Multi-symbol dispatch summary' section"
} else {
    Assert-Fail "G07: Review script does not render a 'Multi-symbol dispatch summary' section"
}

# -----------------------------------------------------------------------
# G08: Review script defines the blocked/skipped no_order_reason classification
# (b5_short_sale_guard, max_new_orders_per_tick_reached, symbol_mismatch_skipped)
# -----------------------------------------------------------------------
$BlockedReasons = @('b5_short_sale_guard', 'max_new_orders_per_tick_reached', 'symbol_mismatch_skipped')
$MissingBlockedReasons = @($BlockedReasons | Where-Object { $ReviewContent -notmatch [regex]::Escape($_) })
if ($ReviewContent -match 'MSD_BLOCKED_OR_SKIPPED_REASONS' -and $MissingBlockedReasons.Count -eq 0) {
    Assert-Pass "G08: MSD_BLOCKED_OR_SKIPPED_REASONS defines b5_short_sale_guard/max_new_orders_per_tick_reached/symbol_mismatch_skipped"
} else {
    Assert-Fail "G08: MSD_BLOCKED_OR_SKIPPED_REASONS missing or incomplete (missing: $($MissingBlockedReasons -join ', '))"
}

# -----------------------------------------------------------------------
# G09: Review script honestly distinguishes no_snapshot/active truth_state and
# an explicit "not captured" state for a missing/unavailable snapshot
# -----------------------------------------------------------------------
if ($ReviewContent -match "'no_snapshot'" -and $ReviewContent -match "'active'" -and $ReviewContent -match '(?i)not captured') {
    Assert-Pass "G09: Review script distinguishes truth_state=no_snapshot/active and an explicit 'not captured' state"
} else {
    Assert-Fail "G09: Review script does not honestly distinguish no_snapshot/active/not-captured states"
}

# -----------------------------------------------------------------------
# G10: review_summary.json includes all multi_symbol_dispatch_* summary fields
# -----------------------------------------------------------------------
$RequiredJsonFields = @(
    'multi_symbol_dispatch_captured',
    'multi_symbol_dispatch_truth_state',
    'multi_symbol_dispatch_canonical_route',
    'multi_symbol_dispatch_backend',
    'multi_symbol_dispatch_runtime_execution_mode',
    'multi_symbol_dispatch_configured_symbol_count',
    'multi_symbol_dispatch_row_count',
    'multi_symbol_dispatch_symbols_seen',
    'multi_symbol_dispatch_blocked_or_skipped_symbols',
    'multi_symbol_dispatch_per_symbol',
    'multi_symbol_dispatch_warnings'
)
$MissingJsonFields = @($RequiredJsonFields | Where-Object { $ReviewContent -notmatch [regex]::Escape($_) })
if ($MissingJsonFields.Count -eq 0) {
    Assert-Pass "G10: review_summary.json includes all multi_symbol_dispatch_* summary fields"
} else {
    Assert-Fail "G10: review_summary.json missing fields: $($MissingJsonFields -join ', ')"
}

# -----------------------------------------------------------------------
# G11: per-symbol row extraction includes all required per-row fields
# -----------------------------------------------------------------------
$RequiredRowFields = @(
    'symbol', 'strategy_id', 'current_qty', 'target_qty', 'delta',
    'no_order_reason', 'last_decision_id', 'last_decision_disposition',
    'day_order_count', 'day_order_limit', 'bar_staleness_secs'
)
$MissingRowFields = @($RequiredRowFields | Where-Object { $ReviewContent -notmatch "Get-Field\s+\`$row\s+'$_'" })
if ($MissingRowFields.Count -eq 0) {
    Assert-Pass "G11: per-symbol row extraction includes all required fields (symbol, strategy_id, current/target_qty, delta, no_order_reason, last_decision_id/disposition, day_order_count/limit, bar_staleness_secs)"
} else {
    Assert-Fail "G11: per-symbol row extraction missing fields: $($MissingRowFields -join ', ')"
}

# -----------------------------------------------------------------------
# G12: schema_version remains 'review-v2' (additive-only change)
# -----------------------------------------------------------------------
if ($ReviewContent -match "schema_version\s*=\s*'review-v2'") {
    Assert-Pass "G12: review_summary schema_version remains 'review-v2' (additive-only)"
} else {
    Assert-Fail "G12: review_summary schema_version is not 'review-v2'"
}

# -----------------------------------------------------------------------
# G13: Capture's multi-symbol dispatch block contains no mutation calls
# -----------------------------------------------------------------------
$CaptureLines = $CaptureContent -split "`r?`n"
$msdLineIndex = -1
for ($i = 0; $i -lt $CaptureLines.Count; $i++) {
    if ($CaptureLines[$i] -match [regex]::Escape('multi_symbol_dispatch_summary.json')) {
        $msdLineIndex = $i
        break
    }
}
if ($msdLineIndex -ge 0) {
    $msdStart = [Math]::Max(0, $msdLineIndex - 5)
    $msdBlock = ($CaptureLines[$msdStart..$msdLineIndex] -join "`n")
    $MutationPatterns = @('/ops/action', '/strategy/signal', '-Method\s+(Post|Put|Patch|Delete)')
    $FoundMutations = @($MutationPatterns | Where-Object { $msdBlock -match $_ })
    if ($FoundMutations.Count -eq 0) {
        Assert-Pass "G13: multi-symbol dispatch capture block contains no mutation calls"
    } else {
        Assert-Fail "G13: multi-symbol dispatch capture block contains mutation pattern(s): $($FoundMutations -join ', ')"
    }
} else {
    Assert-Fail "G13: could not locate multi_symbol_dispatch_summary.json capture line"
}

# -----------------------------------------------------------------------
# G14: no forbidden files touched (diff-based)
# -----------------------------------------------------------------------
Push-Location $RepoRoot
$DiffNames = @(
    git diff --name-only HEAD
    git diff --cached --name-only
) | Where-Object { $_ } | Sort-Object -Unique
Pop-Location

$ForbiddenPatterns = @(
    '^core-rs/',
    '^core-rs/mqk-gui/',
    '^research-py/src/mqk_research/scanner/watchlist_promotion\.py$',
    '^research-py/tests/test_scanner_watchlist_promotion\.py$',
    '^core-rs/crates/mqk-execution/',
    '^core-rs/crates/mqk-broker-',
    '^core-rs/crates/mqk-risk/',
    '^core-rs/crates/mqk-reconcile/',
    '^core-rs/crates/mqk-portfolio/'
)
$ForbiddenTouched = @($DiffNames | Where-Object {
    $name = $_
    $hit = $false
    foreach ($pat in $ForbiddenPatterns) {
        if ($name -match $pat) { $hit = $true; break }
    }
    $hit
})
if ($ForbiddenTouched.Count -eq 0) {
    Assert-Pass "G14: no forbidden files touched (core-rs/, mqk-gui/, watchlist_promotion.py + tests, mqk-execution/mqk-broker-*/mqk-risk/mqk-reconcile/mqk-portfolio)"
} else {
    Assert-Fail "G14: forbidden files touched: $($ForbiddenTouched -join ', ')"
}

# -----------------------------------------------------------------------
# G15: MS-EV-01..14 test coverage exists in capture/review guard tests
# -----------------------------------------------------------------------
$CaptureTestContent = if (Test-Path $CaptureTest) { Get-Content $CaptureTest -Raw } else { '' }
$ReviewTestContent  = if (Test-Path $ReviewTest) { Get-Content $ReviewTest -Raw } else { '' }
$CombinedTestContent = $CaptureTestContent + "`n" + $ReviewTestContent

$MsEvLabels = @(
    'MS-EV-01', 'MS-EV-02', 'MS-EV-03', 'MS-EV-04', 'MS-EV-05', 'MS-EV-06',
    'MS-EV-07', 'MS-EV-08', 'MS-EV-09', 'MS-EV-10', 'MS-EV-11', 'MS-EV-12',
    'MS-EV-13', 'MS-EV-14'
)
$MissingMsEv = @($MsEvLabels | Where-Object { $CombinedTestContent -notmatch [regex]::Escape($_) })
if ($MissingMsEv.Count -eq 0) {
    Assert-Pass "G15: MS-EV-01..14 test coverage present in capture/review guard tests"
} else {
    Assert-Fail "G15: missing MS-EV labels in guard tests: $($MissingMsEv -join ', ')"
}

# -----------------------------------------------------------------------
# G16: docs mark WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01 CLOSED
# while the parent WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01 remains OPEN.
# -----------------------------------------------------------------------
$DesignContent   = if (Test-Path $DesignDoc) { Get-Content $DesignDoc -Raw } else { '' }
$EvidenceContent = if (Test-Path $EvidenceDoc) { Get-Content $EvidenceDoc -Raw } else { '' }

$SubPatchClosedInDesign   = $DesignContent -match 'WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01.*CLOSED'
$ParentOpenInDesign       = $DesignContent -match 'WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01.*OPEN'
$SubPatchClosedInEvidence = $EvidenceContent -match 'WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01.*CLOSED'

if ($SubPatchClosedInDesign -and $ParentOpenInDesign -and $SubPatchClosedInEvidence) {
    Assert-Pass "G16: docs mark WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01 CLOSED and parent WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01 OPEN"
} else {
    Assert-Fail "G16: docs do not correctly mark sub-patch CLOSED / parent OPEN (design closed=$SubPatchClosedInDesign open=$ParentOpenInDesign; evidence-pack closed=$SubPatchClosedInEvidence)"
}

# -----------------------------------------------------------------------
# Summary
# -----------------------------------------------------------------------
Write-Host ''
Write-Host '============================================================'
$Total = $PassCount + $Failures.Count
if ($Failures.Count -eq 0) {
    Write-Host "All $Total assertions passed." -ForegroundColor Green
    exit 0
} else {
    Write-Host "$($Failures.Count) of $Total assertion(s) failed:" -ForegroundColor Red
    $Failures | ForEach-Object { Write-Host $_ -ForegroundColor Red }
    exit 1
}
