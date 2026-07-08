# =============================================================================
# CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01
# Run-KrakenOhlcSync.ps1
#
# Optional, data-only Kraken OHLC sync runner for BTC/USD and ETH/USD.  This
# is what a future Windows Scheduled Task (Register-KrakenOhlcSyncTask.ps1)
# would call -- it is NOT itself a scheduled task, and running this script
# manually does not register or start anything recurring.
#
# SAFETY INVARIANT: this script calls ONLY:
#   cargo run -p mqk-cli --bin mqk-cli -- md kraken-scheduler-readiness
#   cargo run -p mqk-cli --bin mqk-cli -- md kraken-ohlc-sync
# It does NOT start the daemon, runtime, or trading.  It does NOT call any
# broker/order/risk/runtime command.  It does NOT register or unregister a
# Windows Scheduled Task (see Register-KrakenOhlcSyncTask.ps1 for that).
# It does NOT enable the kraken provider or flip any trading flag.
#
# Default mode is -CheckOnly: validates every gate below, writes evidence,
# and exits -- it never calls kraken-ohlc-sync, never makes a network call,
# never writes to md_bars, and never requires MQK_ALLOW_KRAKEN_SCHEDULED_SYNC.
#
# Real-run mode (-CheckOnly:$false) additionally requires
# MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1 to be set as a persistent environment
# variable (this script does not set it) and runs one live
# `kraken-ohlc-sync` per symbol, sequentially, with a sleep of at least
# the policy's min_seconds_between_pair_calls between symbols. It relies
# entirely on kraken-ohlc-sync's own CRYPTO-DATA-03A network gate --
# this script does not duplicate that gate's fail-closed logic, it only
# refuses to attempt a real run if its own prerequisite gates fail first.
#
# Usage:
#   # Default: check-only, safe to run anytime, no mutation:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Run-KrakenOhlcSync.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Run-KrakenOhlcSync.ps1 -CheckOnly
#
#   # Real run (only after MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1 is set persistently):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Run-KrakenOhlcSync.ps1 -CheckOnly:$false
#
# Parameters:
#   -RepoRoot                     Repo root. Default: auto-resolved two levels up.
#   -PolicyPath                   CRYPTO-DATA-02A policy JSON.
#                                 Default: docs\specs\crypto_data_02a_kraken_scheduler_rate_limit_decision.json
#   -RegistryPath                 Registry-v2 fixture with kraken aliases.
#                                 Default: config\instruments\instruments_v2.crypto_local_marks.example.json
#   -ProvidersPath                Provider registry JSON.
#                                 Default: config\providers\providers.json
#   -Symbols                      Comma-separated canonical symbols. Default: BTC/USD,ETH/USD
#   -Timeframe                    Bar timeframe. Default: 1D (only value supported).
#   -EvidenceDir                  Directory the readiness check may inspect for prior
#                                 Kraken evidence. Default: exports\market_data
#   -OutputDir                    Directory this script's own runner evidence is written to.
#                                 Default: exports\market_data
#   -MinSecondsBetweenPairCalls   Overrides the policy's own value if > 0. Default: 0 (use policy).
#   -MaxOhlcCallsPerRun           Overrides the policy's own value if > 0. Default: 0 (use policy).
#   -CheckOnly                    Default: $true. Validate gates and write evidence only.
#
# Evidence (always written):
#   exports\market_data\kraken_ohlc_scheduled_runner_<epoch_seconds>.json
#
# Requirements:
#   - PowerShell 5.1+
#   - cargo on PATH (Rust toolchain)
#
# Hard rules enforced by validate_crypto_data_03b_kraken_scheduler_task_scripts.ps1:
#   - Calls only kraken-scheduler-readiness and kraken-ohlc-sync
#   - Never references daemon/runtime/broker/order/risk commands
#   - Requires MQK_DATABASE_URL and validates it targets the paper DB (port 5440 /
#     miniquantdesk_paper) before any real run
#   - Requires MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1 for real (non-CheckOnly) runs
#   - Enforces sequential per-symbol calls with a policy-driven sleep between them
#   - CheckOnly never sets network_call_made/db_write/md_bars_write true in evidence
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [string] $RepoRoot                    = '',
    [string] $PolicyPath                  = '',
    [string] $RegistryPath                = '',
    [string] $ProvidersPath               = '',
    [string] $Symbols                     = 'BTC/USD,ETH/USD',
    [string] $Timeframe                   = '1D',
    [string] $EvidenceDir                 = '',
    [string] $OutputDir                   = '',
    [int]    $MinSecondsBetweenPairCalls  = 0,
    [int]    $MaxOhlcCallsPerRun          = 0,
    [switch] $CheckOnly                   = $true
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$PATCH_ID       = 'CRYPTO-DATA-03B-KRAKEN-SCHEDULER-TASK-SCRIPTS-01'
$SCHEMA_VERSION = 'kraken-ohlc-scheduled-runner-v1'

function Write-Step { param([string]$M) Write-Host "[KRAKEN-RUNNER] $M"       -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[KRAKEN-RUNNER] OK: $M"   -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[KRAKEN-RUNNER] WARN: $M" -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[KRAKEN-RUNNER] FAIL: $M" -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# [Gate 1] Resolve and verify repo root
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')

$fail_reasons = New-Object System.Collections.Generic.List[string]
$warnings     = New-Object System.Collections.Generic.List[string]

if (-not (Test-Path -LiteralPath $RepoRoot)) {
    Write-Fail "RepoRoot does not exist: $RepoRoot"
    $fail_reasons.Add("repo_root_missing: $RepoRoot")
} else {
    Write-Ok "Repo root: $RepoRoot"
}

# ---------------------------------------------------------------------------
# Resolve remaining default paths (relative to RepoRoot)
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($PolicyPath)) {
    $PolicyPath = Join-Path $RepoRoot 'docs\specs\crypto_data_02a_kraken_scheduler_rate_limit_decision.json'
}
if ([string]::IsNullOrWhiteSpace($RegistryPath)) {
    $RegistryPath = Join-Path $RepoRoot 'config\instruments\instruments_v2.crypto_local_marks.example.json'
}
if ([string]::IsNullOrWhiteSpace($ProvidersPath)) {
    $ProvidersPath = Join-Path $RepoRoot 'config\providers\providers.json'
}
if ([string]::IsNullOrWhiteSpace($EvidenceDir)) {
    $EvidenceDir = Join-Path $RepoRoot 'exports\market_data'
}
if ([string]::IsNullOrWhiteSpace($OutputDir)) {
    $OutputDir = Join-Path $RepoRoot 'exports\market_data'
}
try { New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null } catch {}

Write-Step "Policy path    : $PolicyPath"
Write-Step "Registry path  : $RegistryPath"
Write-Step "Providers path : $ProvidersPath"
Write-Step "Evidence dir   : $EvidenceDir"
Write-Step "Output dir     : $OutputDir"

# ---------------------------------------------------------------------------
# [Gate 2] Policy JSON exists and validates (schema_version + required fields)
# ---------------------------------------------------------------------------
Write-Sect 'Gate checks'

$policyObj = $null
if (-not (Test-Path -LiteralPath $PolicyPath)) {
    Write-Fail "Policy JSON not found: $PolicyPath"
    $fail_reasons.Add("policy_missing: $PolicyPath")
} else {
    try {
        $policyObj = Get-Content -Raw -Path $PolicyPath | ConvertFrom-Json
        $requiredPolicyFields = @(
            'schema_version', 'min_seconds_between_pair_calls',
            'min_seconds_between_scheduled_runs', 'max_ohlc_calls_per_run',
            'max_total_network_calls_per_run', 'concurrency',
            'scheduler_registration_status'
        )
        $missing = @($requiredPolicyFields | Where-Object { $_ -notin ($policyObj.PSObject.Properties.Name) })
        if ($missing.Count -gt 0) {
            Write-Fail "Policy JSON missing required fields: $($missing -join ', ')"
            $fail_reasons.Add("policy_invalid_missing_fields: $($missing -join ', ')")
            $policyObj = $null
        } elseif ($policyObj.concurrency -ne 'sequential_only') {
            Write-Fail "Policy concurrency='$($policyObj.concurrency)' (expected 'sequential_only')"
            $fail_reasons.Add("policy_invalid_concurrency: $($policyObj.concurrency)")
            $policyObj = $null
        } else {
            Write-Ok "Policy JSON valid: schema_version=$($policyObj.schema_version) scheduler_registration_status=$($policyObj.scheduler_registration_status)"
        }
    } catch {
        Write-Fail "Policy JSON parse failed: $_"
        $fail_reasons.Add("policy_parse_error: $_")
    }
}

# ---------------------------------------------------------------------------
# [Gate 3] Registry file exists
# ---------------------------------------------------------------------------
if (-not (Test-Path -LiteralPath $RegistryPath)) {
    Write-Fail "Registry file not found: $RegistryPath"
    $fail_reasons.Add("registry_missing: $RegistryPath")
} else {
    Write-Ok "Registry file found: $RegistryPath"
}

# ---------------------------------------------------------------------------
# [Gate 4] Providers file exists
# ---------------------------------------------------------------------------
if (-not (Test-Path -LiteralPath $ProvidersPath)) {
    Write-Fail "Providers file not found: $ProvidersPath"
    $fail_reasons.Add("providers_missing: $ProvidersPath")
} else {
    Write-Ok "Providers file found: $ProvidersPath"
}

# ---------------------------------------------------------------------------
# [Gate 5] cargo / mqk-cli reachable
# ---------------------------------------------------------------------------
$cargoOk = $false
try {
    $null = Get-Command 'cargo' -ErrorAction Stop
    $cargoOk = $true
    Write-Ok "cargo command available."
} catch {
    Write-Fail "cargo command not found on PATH."
    $fail_reasons.Add('cargo_not_found')
}
$coreRs = Join-Path $RepoRoot 'core-rs'
if ($cargoOk -and -not (Test-Path -LiteralPath (Join-Path $coreRs 'Cargo.toml'))) {
    Write-Fail "core-rs\Cargo.toml not found under RepoRoot: $coreRs"
    $fail_reasons.Add('core_rs_cargo_toml_missing')
    $cargoOk = $false
}

# ---------------------------------------------------------------------------
# [Gate 6/7] MQK_DATABASE_URL present and targets the paper DB only
# ---------------------------------------------------------------------------
$dbUrlPresent = -not [string]::IsNullOrWhiteSpace($env:MQK_DATABASE_URL)
$dbUrlIsPaper = $false
if (-not $dbUrlPresent) {
    Write-Fail 'MQK_DATABASE_URL is not set.'
    $fail_reasons.Add('database_url_missing')
} else {
    $dbUrlIsPaper = ($env:MQK_DATABASE_URL -match '5440') -and ($env:MQK_DATABASE_URL -match 'miniquantdesk_paper')
    if ($dbUrlIsPaper) {
        Write-Ok 'MQK_DATABASE_URL targets the paper DB (port 5440 / miniquantdesk_paper). Value not printed.'
    } else {
        Write-Fail 'MQK_DATABASE_URL is set but does not target port 5440 / miniquantdesk_paper. Refusing.'
        $fail_reasons.Add('database_url_not_paper_db')
    }
}

# ---------------------------------------------------------------------------
# [Gate 8] MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1 -- only required for a real run
# ---------------------------------------------------------------------------
$scheduledSyncEnvSet = ($env:MQK_ALLOW_KRAKEN_SCHEDULED_SYNC -eq '1') -or
                       ($env:MQK_ALLOW_KRAKEN_SCHEDULED_SYNC -ieq 'true')
if ($CheckOnly) {
    Write-Step "MQK_ALLOW_KRAKEN_SCHEDULED_SYNC set: $scheduledSyncEnvSet (not required for -CheckOnly)"
} elseif (-not $scheduledSyncEnvSet) {
    Write-Fail 'MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1 is required for a real (non-CheckOnly) run.'
    $fail_reasons.Add('scheduled_sync_env_not_set')
} else {
    Write-Ok 'MQK_ALLOW_KRAKEN_SCHEDULED_SYNC=1 is set.'
}

# ---------------------------------------------------------------------------
# [Gate 11/12/13] Symbols/timeframe validation
# ---------------------------------------------------------------------------
$symbolList = @($Symbols -split ',' | ForEach-Object { $_.Trim() } | Where-Object { -not [string]::IsNullOrWhiteSpace($_) })
if ($symbolList.Count -eq 0) {
    Write-Fail 'No symbols provided.'
    $fail_reasons.Add('no_symbols_provided')
}

$defaultSymbolSet = @('BTC/USD', 'ETH/USD')
$isDefaultSymbolSet = ($symbolList.Count -eq 2) -and
                      (@($symbolList | Sort-Object) -join ',') -eq (@($defaultSymbolSet | Sort-Object) -join ',')
if (-not $isDefaultSymbolSet) {
    Write-Warn "Symbols override active: $($symbolList -join ', ') (default is BTC/USD,ETH/USD)"
    $warnings.Add("symbols_overridden: $($symbolList -join ',')")
} else {
    Write-Ok "Symbols: $($symbolList -join ', ') (default set)"
}

if ($Timeframe -ne '1D') {
    Write-Fail "Timeframe must be 1D (this Kraken adapter supports only 1D), got: $Timeframe"
    $fail_reasons.Add("unsupported_timeframe: $Timeframe")
} else {
    Write-Ok 'Timeframe: 1D'
}

# Resolve MinSecondsBetweenPairCalls / MaxOhlcCallsPerRun from policy unless overridden
$effectiveMinSecondsBetweenPairCalls = $MinSecondsBetweenPairCalls
$effectiveMaxOhlcCallsPerRun         = $MaxOhlcCallsPerRun
if ($null -ne $policyObj) {
    if ($effectiveMinSecondsBetweenPairCalls -le 0) {
        $effectiveMinSecondsBetweenPairCalls = [int]$policyObj.min_seconds_between_pair_calls
    }
    if ($effectiveMaxOhlcCallsPerRun -le 0) {
        $effectiveMaxOhlcCallsPerRun = [int]$policyObj.max_ohlc_calls_per_run
    }
}
if ($effectiveMinSecondsBetweenPairCalls -le 0) { $effectiveMinSecondsBetweenPairCalls = 2 }
if ($effectiveMaxOhlcCallsPerRun -le 0) { $effectiveMaxOhlcCallsPerRun = 2 }

Write-Step "Effective min_seconds_between_pair_calls: $effectiveMinSecondsBetweenPairCalls"
Write-Step "Effective max_ohlc_calls_per_run: $effectiveMaxOhlcCallsPerRun"

# ---------------------------------------------------------------------------
# [Gate 11] Symbol count does not exceed max_ohlc_calls_per_run
# ---------------------------------------------------------------------------
if ($symbolList.Count -gt $effectiveMaxOhlcCallsPerRun) {
    Write-Fail "Symbol count ($($symbolList.Count)) exceeds max_ohlc_calls_per_run ($effectiveMaxOhlcCallsPerRun)."
    $fail_reasons.Add("symbol_count_exceeds_max_ohlc_calls_per_run: $($symbolList.Count) > $effectiveMaxOhlcCallsPerRun")
} else {
    Write-Ok "Symbol count ($($symbolList.Count)) within max_ohlc_calls_per_run ($effectiveMaxOhlcCallsPerRun)."
}

# ---------------------------------------------------------------------------
# [Gate 10] Scheduler registration status from policy
# ---------------------------------------------------------------------------
$schedulerRegistrationChecked = $false
if ($null -ne $policyObj) {
    $schedulerRegistrationChecked = $true
    if ($policyObj.scheduler_registration_status -ne 'not_registered') {
        Write-Fail "Policy scheduler_registration_status='$($policyObj.scheduler_registration_status)' (expected 'not_registered' for this evidence-only proof)."
        $fail_reasons.Add("scheduler_registration_status_unexpected: $($policyObj.scheduler_registration_status)")
    } else {
        Write-Ok "Policy scheduler_registration_status='not_registered'."
    }
}

# ---------------------------------------------------------------------------
# [Gate 9] mqk md kraken-scheduler-readiness reports truth_state="active"
# Safe to invoke even in -CheckOnly: this command itself never opens a DB
# connection and never makes a provider/network call (proven by
# CRYPTO-DATA-02B/02C).
# ---------------------------------------------------------------------------
$readinessTruthState = 'not_checked'
$readinessAllPassed  = $false
if ($cargoOk -and (Test-Path -LiteralPath $PolicyPath) -and (Test-Path -LiteralPath $RegistryPath) -and (Test-Path -LiteralPath $ProvidersPath)) {
    Write-Step 'Checking kraken-scheduler-readiness...'
    Push-Location $coreRs
    try {
        $local:ErrorActionPreference = 'Continue'
        $readinessOutput = cargo run -p mqk-cli --bin mqk-cli -- md kraken-scheduler-readiness `
            --policy $PolicyPath `
            --registry $RegistryPath `
            --providers $ProvidersPath `
            --symbols ($symbolList -join ',') 2>&1
        $readinessExit = $LASTEXITCODE
        $readinessOutput | Out-Host

        foreach ($line in $readinessOutput) {
            $lineText = [string]$line
            if ($lineText -match '^truth_state=(.+)$') { $readinessTruthState = $Matches[1] }
        }
        $readinessAllPassed = ($readinessExit -eq 0) -and ($readinessTruthState -eq 'active')
    } catch {
        Write-Warn "kraken-scheduler-readiness threw an exception: $_"
        $readinessTruthState = 'error'
    } finally {
        Pop-Location
    }
} else {
    Write-Warn 'Skipping kraken-scheduler-readiness check (a prior gate already failed).'
}

if ($readinessAllPassed) {
    Write-Ok "kraken-scheduler-readiness: truth_state=active"
} else {
    Write-Fail "kraken-scheduler-readiness: truth_state=$readinessTruthState (expected active)"
    $fail_reasons.Add("readiness_not_active: truth_state=$readinessTruthState")
}

# ---------------------------------------------------------------------------
# Evidence writer
# ---------------------------------------------------------------------------
function Write-RunnerEvidence {
    param(
        [string]   $Mode,
        [bool]     $NetworkCallMade,
        [bool]     $DbWrite,
        [bool]     $MdBarsWrite,
        [object[]] $SymbolResults,
        [bool]     $AllPassed,
        [string]   $ReasonCode
    )
    $epoch = [long][DateTimeOffset]::UtcNow.ToUnixTimeSeconds()
    $evPath = Join-Path $OutputDir "kraken_ohlc_scheduled_runner_${epoch}.json"
    $ev = [pscustomobject]@{
        schema_version                = $SCHEMA_VERSION
        producer                      = 'Run-KrakenOhlcSync.ps1'
        produced_at_utc               = (Get-Date).ToUniversalTime().ToString('o')
        mode                          = $Mode
        repo_root                     = $RepoRoot
        policy_path                   = $PolicyPath
        registry_path                 = $RegistryPath
        providers_path                = $ProvidersPath
        symbols_requested              = $symbolList
        timeframe                     = $Timeframe
        network_call_made             = $NetworkCallMade
        db_write                      = $DbWrite
        md_bars_write                 = $MdBarsWrite
        scheduled_task_mutation       = $false
        scheduler_registration_checked = $schedulerRegistrationChecked
        readiness_truth_state         = $readinessTruthState
        min_seconds_between_pair_calls = $effectiveMinSecondsBetweenPairCalls
        max_ohlc_calls_per_run        = $effectiveMaxOhlcCallsPerRun
        symbols_attempted             = @($SymbolResults | ForEach-Object { $_.symbol })
        symbol_results                = $SymbolResults
        all_passed                    = $AllPassed
        reason_code                   = $ReasonCode
        fail_reasons                  = @($fail_reasons)
        warnings                      = @($warnings)
        safety                        = [pscustomobject]@{
            calls_only_kraken_scheduler_readiness_and_kraken_ohlc_sync = $true
            no_daemon_runtime_broker_order_risk_references             = $true
            no_scheduled_task_registration                             = $true
            no_trading_enabled                                         = $true
        }
    }
    try {
        $ev | ConvertTo-Json -Depth 8 | Set-Content -Path $evPath -Encoding ASCII
        Write-Ok "Evidence written: $evPath"
    } catch {
        Write-Warn "Could not write evidence: $_"
    }
    return $evPath
}

$gatesPassed = ($fail_reasons.Count -eq 0)

# ---------------------------------------------------------------------------
# CHECK-ONLY mode: gates only, no kraken-ohlc-sync call, no network, no DB
# write.
# ---------------------------------------------------------------------------
if ($CheckOnly) {
    Write-Sect 'CHECK-ONLY: gate validation only (no kraken-ohlc-sync call)'
    $reasonCode = if ($gatesPassed) { 'check_only_all_gates_passed' } else { 'check_only_one_or_more_gates_failed' }
    $evPath = Write-RunnerEvidence `
        -Mode 'check_only' `
        -NetworkCallMade $false `
        -DbWrite $false `
        -MdBarsWrite $false `
        -SymbolResults @() `
        -AllPassed $gatesPassed `
        -ReasonCode $reasonCode

    Write-Sect 'CHECK-ONLY complete'
    if ($gatesPassed) {
        Write-Ok 'All gates passed. CheckOnly did not call kraken-ohlc-sync, make a network call, or write to the DB.'
        exit 0
    } else {
        Write-Fail "One or more gates failed: $($fail_reasons -join '; ')"
        exit 1
    }
}

# ---------------------------------------------------------------------------
# REAL RUN: only if every gate passed, including MQK_ALLOW_KRAKEN_SCHEDULED_SYNC.
# One kraken-ohlc-sync call per symbol, sequential, with a sleep of at least
# min_seconds_between_pair_calls between symbols. No --input-file: this is
# the live scheduled path, and relies on kraken-ohlc-sync's own
# CRYPTO-DATA-03A network gate to actually authorize/deny the network call.
# ---------------------------------------------------------------------------
Write-Sect 'REAL RUN: kraken-ohlc-sync per symbol (sequential)'

if (-not $gatesPassed) {
    Write-Fail "Refusing real run: one or more gates failed: $($fail_reasons -join '; ')"
    Write-RunnerEvidence `
        -Mode 'scheduled_run' `
        -NetworkCallMade $false `
        -DbWrite $false `
        -MdBarsWrite $false `
        -SymbolResults @() `
        -AllPassed $false `
        -ReasonCode 'real_run_refused_gate_failure' | Out-Null
    exit 1
}

$symbolResults   = New-Object System.Collections.Generic.List[object]
$anyNetworkCall  = $false
$anyDbWrite      = $false
$anyMdBarsWrite  = $false
$anyFailed       = $false

for ($i = 0; $i -lt $symbolList.Count; $i++) {
    $sym = $symbolList[$i]
    Write-Step "kraken-ohlc-sync: symbol=$sym timeframe=$Timeframe"

    Push-Location $coreRs
    $syncExit = -1
    $syncOutput = @()
    try {
        $local:ErrorActionPreference = 'Continue'
        $syncOutput = cargo run -p mqk-cli --bin mqk-cli -- md kraken-ohlc-sync `
            --registry $RegistryPath `
            --symbol $sym `
            --timeframe $Timeframe `
            --output-dir $EvidenceDir 2>&1
        $syncExit = $LASTEXITCODE
        $syncOutput | Out-Host
    } catch {
        Write-Fail "kraken-ohlc-sync threw an exception for ${sym}: $_"
    } finally {
        Pop-Location
    }

    $symNetworkCall = $false
    $symDbWrite     = $false
    $symMdBarsWrite = $false
    foreach ($line in $syncOutput) {
        $lineText = [string]$line
        if ($lineText -match '^network_call_made=(true|false)$') { $symNetworkCall = ($Matches[1] -eq 'true') }
        if ($lineText -match '^db_write=(true|false)$')          { $symDbWrite     = ($Matches[1] -eq 'true') }
        if ($lineText -match '^md_bars_write=(true|false)$')     { $symMdBarsWrite = ($Matches[1] -eq 'true') }
    }
    $anyNetworkCall = $anyNetworkCall -or $symNetworkCall
    $anyDbWrite     = $anyDbWrite -or $symDbWrite
    $anyMdBarsWrite = $anyMdBarsWrite -or $symMdBarsWrite

    if ($syncExit -eq 0) {
        Write-Ok "kraken-ohlc-sync succeeded for $sym"
    } else {
        Write-Fail "kraken-ohlc-sync failed for $sym (exit $syncExit)"
        $anyFailed = $true
    }

    $symbolResults.Add([pscustomobject]@{
        symbol            = $sym
        exit_code         = $syncExit
        network_call_made = $symNetworkCall
        db_write          = $symDbWrite
        md_bars_write     = $symMdBarsWrite
        succeeded         = ($syncExit -eq 0)
    })

    if ($i -lt ($symbolList.Count - 1)) {
        Write-Step "Sleeping ${effectiveMinSecondsBetweenPairCalls}s before next symbol (sequential, policy-driven spacing)..."
        Start-Sleep -Seconds $effectiveMinSecondsBetweenPairCalls
    }
}

$allPassed  = (-not $anyFailed)
$reasonCode = if ($allPassed) { 'scheduled_run_all_symbols_succeeded' } else { 'scheduled_run_one_or_more_symbols_failed' }

Write-RunnerEvidence `
    -Mode 'scheduled_run' `
    -NetworkCallMade $anyNetworkCall `
    -DbWrite $anyDbWrite `
    -MdBarsWrite $anyMdBarsWrite `
    -SymbolResults $symbolResults `
    -AllPassed $allPassed `
    -ReasonCode $reasonCode | Out-Null

Write-Sect 'Run result'
if ($allPassed) {
    Write-Ok 'kraken-ohlc-sync succeeded for all symbols.'
    exit 0
} else {
    Write-Fail 'One or more symbols failed. See evidence for detail.'
    exit 1
}
