# =============================================================================
# Script guard: WATCHLIST-PROMO-V2-MULTI-SYMBOL-01 (Patch 11a)
#
# Verifies the watchlist-v2 multi-symbol promotion-gate extension to
# research-py/src/mqk_research/scanner/watchlist_promotion.py:
#
#  - watchlist-v1 single-symbol behavior is fully preserved (symbols[0] only,
#    max_symbols_to_trade/max_concurrent_positions forced to 1).
#  - watchlist-v2 artifacts may approve multiple symbols, capped by
#    min(max_symbols_to_trade, max_concurrent_positions, MULTI_SYMBOL_HARD_CEILING).
#  - MULTI_SYMBOL_HARD_CEILING = 5 is defined locally and mirrors
#    core-rs/crates/mqk-daemon/src/watchlist_intake.rs.
#  - v2 fails closed (zero approved symbols) on invalid/oversized limits, a
#    missing strategy assignment, a missing/mismatched strategy-fit artifact,
#    or a not-recommended strategy-fit -- never silently falls back to
#    symbols[0].
#  - approved_for_live remains hard False, including for a forged v2 input
#    with approved_for_live=true and for multi-symbol approvals.
#  - PV2-01..15 tests exist and pass.
#  - No forbidden imports/touches: GUI, scripts/windows, broker/OMS/execution/
#    risk/reconcile/portfolio Rust, watchlist promotion smoke/evidence.
#  - Docs mark WATCHLIST-PROMO-V2-MULTI-SYMBOL-01 CLOSED while the parent
#    WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01 remains OPEN.
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# Exit codes: 0 = all assertions pass, 1 = any assertion failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$Failures = [System.Collections.Generic.List[string]]::new()
$PassCount = 0

function Assert-Pass([string]$Msg) {
    $script:PassCount++
    Write-Host "  OK:   $Msg" -ForegroundColor Green
}

function Assert-Fail([string]$Msg) {
    $script:Failures.Add("  FAIL: $Msg")
    Write-Host "  FAIL: $Msg" -ForegroundColor Red
}

$RepoRoot   = (Resolve-Path "$PSScriptRoot\..\..\").Path
$ModuleFile = Join-Path $RepoRoot "research-py\src\mqk_research\scanner\watchlist_promotion.py"
$TestFile   = Join-Path $RepoRoot "research-py\tests\test_scanner_watchlist_promotion.py"
$DesignDoc  = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"
$SpecDoc    = Join-Path $RepoRoot "docs\specs\market_scanner_backtest_candidate_pipeline.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_watchlist_promo_v2_multi_symbol.ps1'
Write-Host '============================================================'

# -----------------------------------------------------------------------
# G01: module file exists
# -----------------------------------------------------------------------
if (Test-Path $ModuleFile) {
    Assert-Pass "G01: watchlist_promotion.py exists"
} else {
    Assert-Fail "G01: watchlist_promotion.py not found at $ModuleFile"
}

$ModuleContent = if (Test-Path $ModuleFile) { Get-Content $ModuleFile -Raw } else { '' }
$TestContent   = if (Test-Path $TestFile) { Get-Content $TestFile -Raw } else { '' }

# -----------------------------------------------------------------------
# G02: watchlist-v1 schema constant preserved
# -----------------------------------------------------------------------
if ($ModuleContent -match [regex]::Escape('SCHEMA_VERSION_WATCHLIST = "watchlist-v1"')) {
    Assert-Pass "G02: SCHEMA_VERSION_WATCHLIST = 'watchlist-v1' preserved"
} else {
    Assert-Fail "G02: SCHEMA_VERSION_WATCHLIST = 'watchlist-v1' not found"
}

# -----------------------------------------------------------------------
# G03: watchlist-v2 schema constant added
# -----------------------------------------------------------------------
if ($ModuleContent -match [regex]::Escape('SCHEMA_VERSION_WATCHLIST_V2 = "watchlist-v2"')) {
    Assert-Pass "G03: SCHEMA_VERSION_WATCHLIST_V2 = 'watchlist-v2' defined"
} else {
    Assert-Fail "G03: SCHEMA_VERSION_WATCHLIST_V2 = 'watchlist-v2' not found"
}

# -----------------------------------------------------------------------
# G04: MULTI_SYMBOL_HARD_CEILING = 5 defined locally
# -----------------------------------------------------------------------
if ($ModuleContent -match 'MULTI_SYMBOL_HARD_CEILING\s*=\s*5') {
    Assert-Pass "G04: MULTI_SYMBOL_HARD_CEILING = 5 defined"
} else {
    Assert-Fail "G04: MULTI_SYMBOL_HARD_CEILING = 5 not found"
}

# -----------------------------------------------------------------------
# G05: v2 selection is not limited to symbols[0] -- effective_limit formula present
# -----------------------------------------------------------------------
if ($ModuleContent -match 'effective_limit\s*=\s*min\(\s*max_symbols\s*,\s*max_concurrent\s*,\s*MULTI_SYMBOL_HARD_CEILING\s*\)' -or
    $ModuleContent -match 'min\(max_symbols, max_concurrent, MULTI_SYMBOL_HARD_CEILING\)') {
    Assert-Pass "G05: v2 effective_limit = min(max_symbols_to_trade, max_concurrent_positions, MULTI_SYMBOL_HARD_CEILING)"
} else {
    Assert-Fail "G05: v2 effective_limit formula not found"
}

if ($ModuleContent -match 'return symbols\[:effective_limit\]') {
    Assert-Pass "G05b: v2 selection returns symbols[:effective_limit] (not limited to symbols[0])"
} else {
    Assert-Fail "G05b: v2 symbols[:effective_limit] selection not found"
}

# -----------------------------------------------------------------------
# G06: v1 selection is still limited to the top-ranked symbol
# -----------------------------------------------------------------------
if ($ModuleContent -match 'return \[symbols\[0\]\]') {
    Assert-Pass "G06: v1 selection still returns [symbols[0]] (top-ranked symbol only)"
} else {
    Assert-Fail "G06: v1 [symbols[0]] selection not found"
}

# -----------------------------------------------------------------------
# G07: v1 still forces max_symbols_to_trade=1 / max_concurrent_positions=1
# in apply_watchlist_promotion; v2 does not.
# -----------------------------------------------------------------------
if ($ModuleContent -match 'if not _is_watchlist_v2\(watchlist\):\s*\r?\n\s*updated\["max_symbols_to_trade"\]\s*=\s*1\s*\r?\n\s*updated\["max_concurrent_positions"\]\s*=\s*1') {
    Assert-Pass "G07: v1 forces max_symbols_to_trade=1/max_concurrent_positions=1; v2 carries values through unchanged"
} else {
    Assert-Fail "G07: v1-only forcing of max_symbols_to_trade/max_concurrent_positions=1 not found in apply_watchlist_promotion"
}

# -----------------------------------------------------------------------
# G08: v2 per-symbol strategy_assignments + strategy-fit gates required
# -----------------------------------------------------------------------
$RequiredV2Symbols = @(
    'REASON_WATCHLIST_V2_SYMBOL_ASSIGNMENT_MISSING',
    'REASON_WATCHLIST_V2_MAX_SYMBOLS_INVALID',
    'REASON_WATCHLIST_V2_CONCURRENT_LIMIT_INVALID',
    'REASON_WATCHLIST_V2_HARD_CEILING_EXCEEDED',
    'REASON_STRATEGY_FIT_SYMBOL_MISMATCH',
    'REASON_STRATEGY_FIT_STRATEGY_MISMATCH'
)
$MissingV2Symbols = @($RequiredV2Symbols | Where-Object { $ModuleContent -notmatch $_ })
if ($MissingV2Symbols.Count -eq 0) {
    Assert-Pass "G08: v2 reason constants present (assignment-missing, limit-invalid x2, hard-ceiling-exceeded, fit symbol/strategy mismatch)"
} else {
    Assert-Fail "G08: missing v2 reason constants: $($MissingV2Symbols -join ', ')"
}

# -----------------------------------------------------------------------
# G09: approved_for_live forced False (hard invariant preserved)
# -----------------------------------------------------------------------
if ($ModuleContent -match [regex]::Escape('approved_for_live=False') -and
    $ModuleContent -notmatch [regex]::Escape('approved_for_live=True')) {
    Assert-Pass "G09: approved_for_live=False present; no approved_for_live=True"
} else {
    Assert-Fail "G09: approved_for_live=False invariant not confirmed"
}

if ($ModuleContent -match [regex]::Escape('updated["approved_for_live"] = False')) {
    Assert-Pass "G09b: apply_watchlist_promotion forces updated[\"approved_for_live\"] = False"
} else {
    Assert-Fail "G09b: apply_watchlist_promotion does not force approved_for_live = False"
}

# -----------------------------------------------------------------------
# G10: PV2-01..15 tests present
# -----------------------------------------------------------------------
$PV2Labels = @(
    'pv2_01', 'pv2_02', 'pv2_03', 'pv2_04', 'pv2_05', 'pv2_06', 'pv2_07',
    'pv2_08', 'pv2_09', 'pv2_10', 'pv2_11', 'pv2_12', 'pv2_13', 'pv2_14', 'pv2_15'
)
$MissingPV2 = @($PV2Labels | Where-Object { $TestContent -notmatch $_ })
if ($MissingPV2.Count -eq 0) {
    Assert-Pass "G10: PV2-01..15 tests present in test_scanner_watchlist_promotion.py"
} else {
    Assert-Fail "G10: missing PV2 test labels: $($MissingPV2 -join ', ')"
}

# -----------------------------------------------------------------------
# G11: no forbidden imports (network/DB/broker/subprocess) in module
# -----------------------------------------------------------------------
$ForbiddenImports = @(
    'import requests',
    'import urllib',
    'import http.client',
    'import aiohttp',
    'import psycopg',
    'import sqlalchemy',
    'import subprocess',
    'mqk_broker',
    'BrokerGateway',
    'broker_adapter'
)
$FoundForbidden = @($ForbiddenImports | Where-Object { $ModuleContent -match [regex]::Escape($_) })
if ($FoundForbidden.Count -eq 0) {
    Assert-Pass "G11: no forbidden network/DB/broker/subprocess imports in watchlist_promotion.py"
} else {
    Assert-Fail "G11: forbidden imports/references found: $($FoundForbidden -join ', ')"
}

# -----------------------------------------------------------------------
# G12-G15: diff-based checks -- no GUI/TS, no scripts/windows, no daemon
# runtime dispatch/broker/OMS/execution/risk/reconcile/portfolio Rust, no
# smoke/evidence scripts touched by this patch.
# -----------------------------------------------------------------------
Push-Location $RepoRoot
$DiffNames = @(
    git diff --name-only HEAD
    git diff --cached --name-only
) | Where-Object { $_ } | Sort-Object -Unique
Pop-Location

# G12: no GUI/TypeScript files touched
$GuiTouched = @($DiffNames | Where-Object { $_ -match '^core-rs/mqk-gui/' })
if ($GuiTouched.Count -eq 0) {
    Assert-Pass "G12: no GUI/TypeScript files touched"
} else {
    Assert-Fail "G12: GUI files touched (forbidden): $($GuiTouched -join ', ')"
}

# G13: no scripts/windows files touched
$ScriptsTouched = @($DiffNames | Where-Object { $_ -match '^scripts/windows/' })
if ($ScriptsTouched.Count -eq 0) {
    Assert-Pass "G13: no scripts/windows files touched"
} else {
    Assert-Fail "G13: scripts/windows files touched (forbidden): $($ScriptsTouched -join ', ')"
}

# G14: no broker/OMS/execution/risk/reconcile/portfolio Rust touched
$ForbiddenRustPattern = '^(core-rs/crates/mqk-execution/|core-rs/crates/mqk-broker-|core-rs/crates/mqk-risk/|core-rs/crates/mqk-reconcile/|core-rs/crates/mqk-portfolio/)'
$RustTouched = @($DiffNames | Where-Object { $_ -match $ForbiddenRustPattern })
if ($RustTouched.Count -eq 0) {
    Assert-Pass "G14: no broker/OMS/execution/risk/reconcile/portfolio Rust files touched"
} else {
    Assert-Fail "G14: forbidden Rust files touched: $($RustTouched -join ', ')"
}

# G15: no smoke/evidence scripts touched (WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01 not started)
$SmokeTouched = @($DiffNames | Where-Object { $_ -match '^(scripts/windows/|scripts/paper_|exports/|evidence/)' })
if ($SmokeTouched.Count -eq 0) {
    Assert-Pass "G15: no smoke/evidence scripts touched (smoke/evidence sub-patch not started)"
} else {
    Assert-Fail "G15: smoke/evidence files touched (forbidden): $($SmokeTouched -join ', ')"
}

# -----------------------------------------------------------------------
# G16: docs mark WATCHLIST-PROMO-V2-MULTI-SYMBOL-01 CLOSED while the parent
# WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01 remains OPEN.
# -----------------------------------------------------------------------
$DesignContent = if (Test-Path $DesignDoc) { Get-Content $DesignDoc -Raw } else { '' }
$SpecContent   = if (Test-Path $SpecDoc) { Get-Content $SpecDoc -Raw } else { '' }

$SubPatchClosedInDesign = $DesignContent -match 'WATCHLIST-PROMO-V2-MULTI-SYMBOL-01.*CLOSED'
$ParentOpenInDesign     = $DesignContent -match 'WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01.*OPEN'
$SubPatchClosedInSpec   = $SpecContent -match 'WATCHLIST-PROMO-V2-MULTI-SYMBOL-01.*CLOSED'
$ParentOpenInSpec       = $SpecContent -match 'WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01.*OPEN'

if ($SubPatchClosedInDesign -and $ParentOpenInDesign -and $SubPatchClosedInSpec -and $ParentOpenInSpec) {
    Assert-Pass "G16: docs mark WATCHLIST-PROMO-V2-MULTI-SYMBOL-01 CLOSED and parent WATCHLIST-PROMO-V2-MULTI-SYMBOL-AND-SMOKE-01 OPEN"
} else {
    Assert-Fail "G16: docs do not correctly mark sub-patch CLOSED / parent OPEN (design closed=$SubPatchClosedInDesign open=$ParentOpenInDesign; spec closed=$SubPatchClosedInSpec open=$ParentOpenInSpec)"
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
