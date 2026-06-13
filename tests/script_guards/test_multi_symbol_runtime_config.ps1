# =============================================================================
# Script guard: MULTI-SYMBOL-RUNTIME-CONFIG-01
#
# Verifies that the pure multi-symbol runtime configuration layer:
#   1. Exists at core-rs/crates/mqk-daemon/src/state/multi_symbol_config.rs.
#   2. Defines MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION = "multi-symbol-runtime-config-v1".
#   3. Defines SymbolStrategyAssignment, MultiSymbolConfigSource, and
#      MultiSymbolRuntimeConfig with their expected fields/variants.
#   4. Defines all 8 multi_symbol_config_* fail-closed reason strings.
#   5. Defines all 5 builder functions (legacy, watchlist-v2, selection,
#      both _from_env wrappers).
#   6. Is registered in state.rs (mod + pub use) but NOT invoked anywhere in
#      state.rs (config-construction seam only, not wired).
#   7. Is NOT referenced from loop_runner.rs (no dispatch wiring).
#   8. Is NOT referenced from routes/strategy.rs (no signal-path wiring).
#   9. Has the 18 M01..M18 proof tests in
#      scenario_multi_symbol_runtime_config_01.rs.
#  10. Is documented in the design doc and scanner/watchlist spec.
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

$RepoRoot       = (Resolve-Path "$PSScriptRoot\..\..\").Path
$ConfigModule   = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\multi_symbol_config.rs"
$StateRs        = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state.rs"
$LoopRunner     = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\state\loop_runner.rs"
$StrategyRoute  = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\src\routes\strategy.rs"
$TestFile       = Join-Path $RepoRoot "core-rs\crates\mqk-daemon\tests\scenario_multi_symbol_runtime_config_01.rs"
$DesignDoc      = Join-Path $RepoRoot "docs\design\native_multi_symbol_dispatch.md"
$SpecDoc        = Join-Path $RepoRoot "docs\specs\market_scanner_backtest_candidate_pipeline.md"

Write-Host ''
Write-Host '============================================================'
Write-Host 'Script guard: test_multi_symbol_runtime_config.ps1'
Write-Host '============================================================'

# M-G01 -- multi_symbol_config.rs exists
if (Test-Path $ConfigModule) {
    Assert-Pass "M-G01: multi_symbol_config.rs exists"
} else {
    Assert-Fail "M-G01: multi_symbol_config.rs not found at $ConfigModule"
}

$ConfigContent = ''
if (Test-Path $ConfigModule) {
    $ConfigContent = Get-Content $ConfigModule -Raw
}

# M-G02 -- MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION = "multi-symbol-runtime-config-v1"
if ($ConfigContent -match 'MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION:\s*&str\s*=\s*"multi-symbol-runtime-config-v1"') {
    Assert-Pass "M-G02: MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION = \"multi-symbol-runtime-config-v1\" defined"
} else {
    Assert-Fail "M-G02: MULTI_SYMBOL_RUNTIME_CONFIG_SCHEMA_VERSION = \"multi-symbol-runtime-config-v1\" not found in multi_symbol_config.rs"
}

# M-G03 -- SymbolStrategyAssignment struct with symbol/strategy_id/timeframe fields
if ($ConfigContent -match 'struct SymbolStrategyAssignment' -and
    $ConfigContent -match 'pub symbol:\s*String' -and
    $ConfigContent -match 'pub strategy_id:\s*String' -and
    $ConfigContent -match 'pub timeframe:\s*String') {
    Assert-Pass "M-G03: SymbolStrategyAssignment { symbol, strategy_id, timeframe: String } present"
} else {
    Assert-Fail "M-G03: SymbolStrategyAssignment with symbol/strategy_id/timeframe fields not found"
}

# M-G04 -- MultiSymbolConfigSource enum with EnvSingleSymbolFallback / WatchlistArtifactV2
if ($ConfigContent -match 'enum MultiSymbolConfigSource' -and
    $ConfigContent -match 'EnvSingleSymbolFallback' -and
    $ConfigContent -match 'WatchlistArtifactV2') {
    Assert-Pass "M-G04: MultiSymbolConfigSource { EnvSingleSymbolFallback, WatchlistArtifactV2 } present"
} else {
    Assert-Fail "M-G04: MultiSymbolConfigSource with both variants not found"
}

# M-G05 -- MultiSymbolRuntimeConfig struct with schema_version/symbols/max_concurrent_symbols/source
if ($ConfigContent -match 'struct MultiSymbolRuntimeConfig' -and
    $ConfigContent -match 'pub schema_version:\s*String' -and
    $ConfigContent -match 'pub symbols:\s*Vec<SymbolStrategyAssignment>' -and
    $ConfigContent -match 'pub max_concurrent_symbols:\s*usize' -and
    $ConfigContent -match 'pub source:\s*MultiSymbolConfigSource') {
    Assert-Pass "M-G05: MultiSymbolRuntimeConfig { schema_version, symbols, max_concurrent_symbols, source } present"
} else {
    Assert-Fail "M-G05: MultiSymbolRuntimeConfig with expected fields not found"
}

# M-G06..M-G13 -- all 8 multi_symbol_config_* fail-closed reason strings present
$ReasonStrings = @(
    'multi_symbol_config_missing_symbol',
    'multi_symbol_config_missing_strategy_id',
    'multi_symbol_config_missing_timeframe',
    'multi_symbol_config_watchlist_not_v2',
    'multi_symbol_config_watchlist_not_approved',
    'multi_symbol_config_missing_assignment',
    'multi_symbol_config_concurrent_limit_exceeded',
    'multi_symbol_config_hard_ceiling_exceeded'
)
$ReasonIdx = 6
foreach ($reason in $ReasonStrings) {
    if ($ConfigContent -match [regex]::Escape($reason)) {
        Assert-Pass "M-G$ReasonIdx`: failure-reason '$reason' present in multi_symbol_config.rs"
    } else {
        Assert-Fail "M-G$ReasonIdx`: failure-reason '$reason' NOT found in multi_symbol_config.rs"
    }
    $ReasonIdx++
}

# M-G14..M-G18 -- all 5 builder functions present
$Builders = @(
    'build_legacy_single_symbol_config',
    'build_legacy_single_symbol_config_from_env',
    'build_multi_symbol_config_from_watchlist_artifact',
    'build_multi_symbol_runtime_config_from_env_and_watchlist',
    'build_multi_symbol_runtime_config_from_env'
)
$BuilderIdx = 14
foreach ($fn in $Builders) {
    if ($ConfigContent -match "pub fn $fn") {
        Assert-Pass "M-G$BuilderIdx`: pub fn $fn defined in multi_symbol_config.rs"
    } else {
        Assert-Fail "M-G$BuilderIdx`: pub fn $fn NOT found in multi_symbol_config.rs"
    }
    $BuilderIdx++
}

# M-G19 / M-G20 -- registered in state.rs (mod + pub use re-export)
if (Test-Path $StateRs) {
    $StateContent = Get-Content $StateRs -Raw

    if ($StateContent -match 'mod multi_symbol_config;') {
        Assert-Pass "M-G19: state.rs declares 'mod multi_symbol_config;'"
    } else {
        Assert-Fail "M-G19: state.rs does not declare 'mod multi_symbol_config;'"
    }

    if ($StateContent -match 'pub use multi_symbol_config::\{') {
        Assert-Pass "M-G20: state.rs re-exports multi_symbol_config items via 'pub use multi_symbol_config::{ ... }'"
    } else {
        Assert-Fail "M-G20: state.rs does not re-export multi_symbol_config items"
    }

    # M-G21 -- state.rs registers but does NOT invoke any builder (no dispatch wiring)
    $InvocationPattern = '(build_legacy_single_symbol_config_from_env|build_legacy_single_symbol_config|build_multi_symbol_config_from_watchlist_artifact|build_multi_symbol_runtime_config_from_env_and_watchlist|build_multi_symbol_runtime_config_from_env)\('
    if (-not ($StateContent -match $InvocationPattern)) {
        Assert-Pass "M-G21: state.rs does not invoke any multi_symbol_config builder (registration only, not wired)"
    } else {
        Assert-Fail "M-G21: state.rs invokes a multi_symbol_config builder -- dispatch wiring FORBIDDEN in this patch"
    }
} else {
    Assert-Fail "M-G19: state.rs not found at $StateRs"
    Assert-Fail "M-G20: state.rs not found at $StateRs"
    Assert-Fail "M-G21: state.rs not found at $StateRs"
}

# M-G22 -- loop_runner.rs has zero references to multi_symbol_config / MultiSymbolRuntimeConfig / builders
if (Test-Path $LoopRunner) {
    if (-not (Select-String -Path $LoopRunner -Pattern 'multi_symbol_config|MultiSymbolRuntimeConfig|MultiSymbolConfigSource|MultiSymbolConfigError|build_multi_symbol_|build_legacy_single_symbol_' -Quiet)) {
        Assert-Pass "M-G22: loop_runner.rs has no multi-symbol runtime config references -- dispatch NOT wired"
    } else {
        Assert-Fail "M-G22: loop_runner.rs references multi_symbol_config -- dispatch wiring FORBIDDEN in this patch"
    }
} else {
    Assert-Fail "M-G22: loop_runner.rs not found at $LoopRunner"
}

# M-G23 -- routes/strategy.rs has zero references to multi_symbol runtime config
if (Test-Path $StrategyRoute) {
    if (-not (Select-String -Path $StrategyRoute -Pattern 'multi_symbol_config|MultiSymbolRuntimeConfig|MultiSymbolConfigSource|MultiSymbolConfigError|build_multi_symbol_|build_legacy_single_symbol_' -Quiet)) {
        Assert-Pass "M-G23: routes/strategy.rs has no multi-symbol runtime config references -- signal path NOT wired"
    } else {
        Assert-Fail "M-G23: routes/strategy.rs references multi_symbol_config -- signal-path wiring FORBIDDEN in this patch"
    }
} else {
    Assert-Fail "M-G23: routes/strategy.rs not found at $StrategyRoute"
}

# M-G24 -- scenario test file has M01..M18 labels
if (Test-Path $TestFile) {
    $TestContent = Get-Content $TestFile -Raw
    $MissingLabels = [System.Collections.Generic.List[string]]::new()
    $Labels = @('m01','m02','m03','m04','m05','m06','m07','m08','m09','m10','m11','m12','m13','m14','m15','m16','m17','m18')
    foreach ($lbl in $Labels) {
        if ($TestContent -notmatch $lbl) {
            $MissingLabels.Add($lbl)
        }
    }
    if ($MissingLabels.Count -eq 0) {
        Assert-Pass "M-G24: scenario_multi_symbol_runtime_config_01.rs has M01..M18 tests"
    } else {
        Assert-Fail "M-G24: missing test labels: $($MissingLabels -join ', ')"
    }
} else {
    Assert-Fail "M-G24: scenario_multi_symbol_runtime_config_01.rs not found at $TestFile"
}

# M-G25 -- design doc references MULTI-SYMBOL-RUNTIME-CONFIG-01
if (Test-Path $DesignDoc) {
    if (Select-String -Path $DesignDoc -Pattern 'MULTI-SYMBOL-RUNTIME-CONFIG-01' -Quiet) {
        Assert-Pass "M-G25: MULTI-SYMBOL-RUNTIME-CONFIG-01 referenced in native_multi_symbol_dispatch.md"
    } else {
        Assert-Fail "M-G25: MULTI-SYMBOL-RUNTIME-CONFIG-01 not referenced in native_multi_symbol_dispatch.md"
    }
} else {
    Assert-Fail "M-G25: native_multi_symbol_dispatch.md not found at $DesignDoc"
}

# M-G26 -- scanner/watchlist spec doc references MULTI-SYMBOL-RUNTIME-CONFIG-01
if (Test-Path $SpecDoc) {
    if (Select-String -Path $SpecDoc -Pattern 'MULTI-SYMBOL-RUNTIME-CONFIG-01' -Quiet) {
        Assert-Pass "M-G26: MULTI-SYMBOL-RUNTIME-CONFIG-01 referenced in market_scanner_backtest_candidate_pipeline.md"
    } else {
        Assert-Fail "M-G26: MULTI-SYMBOL-RUNTIME-CONFIG-01 not referenced in market_scanner_backtest_candidate_pipeline.md"
    }
} else {
    Assert-Fail "M-G26: market_scanner_backtest_candidate_pipeline.md not found at $SpecDoc"
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
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
