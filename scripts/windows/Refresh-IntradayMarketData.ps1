# =============================================================================
# INTRADAY-5M-LIVE-BAR-INGESTION-01
# Refresh-IntradayMarketData.ps1
#
# Intraday market-data top-off for MiniQuantDesk V4 paper smoke.
# Calls mqk-cli md sync-provider (incremental) to pull completed current-session
# bars into md_bars so the strategy evaluates live bars, not stale prior-day bars.
#
# Data ingestion only. No orders, signals, OMS, outbox, or broker calls.
#
# Usage:
#   # Check only (read-only):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 -CheckOnly
#
#   # One refresh then exit:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 -Once
#
#   # Recurring refresh for 30 min, every 5 min:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 `
#       -IntervalSeconds 300 -DurationSeconds 1800
#
# Parameters:
#   -Source               Market-data provider: twelvedata | alpaca. Default: '' (empty) --
#                          auto-derived from the canonical instrument registry
#                          (config\instruments\equities.json) for -Symbols/-Timeframe
#                          (MARKET-DATA-PROVIDER-PROVENANCE-01-REPAIR-01). A stale
#                          hardcoded default must never override the registry; pass
#                          -Source explicitly only to deliberately override it.
#   -Symbols              Comma-separated ticker list. Default: AAPL
#   -Timeframe            Bar timeframe. Default: 5m
#   -RegistryPath          Instrument registry path, relative to -RepoRoot, used only
#                          for -Source auto-derivation. Default: config\instruments\equities.json
#   -IntervalSeconds      Seconds between refreshes in loop mode. Default: 300
#   -MinRefreshIntervalSeconds
#                          Minimum seconds between refresh attempts in loop mode. Default: 300
#   -DurationSeconds      Total loop duration in seconds. Default: 1800
#   -MinCompletedBars     Minimum completed bars to report OK. Default: 30
#   -MaxStalenessMinutes  Maximum minutes since latest complete bar before warning.
#                          Default: derived from timeframe. Intraday uses
#                          MQK_INTRADAY_BAR_MAX_AGE_SECS or 900 seconds.
#   -PaperDbUrl           Paper DB connection URL. Default: postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable
#   -RepoRoot             Repo root. Default: auto-resolved two levels up from this script.
#   -CheckOnly            Read-only check: bar count, freshness, key presence. No mutations.
#   -Once                 One refresh then exit (no loop).
#
# Hard rules:
#   - Paper DB only. Refuses if MQK_DATABASE_URL does not contain port 5440.
#   - Never prints TWELVEDATA_API_KEY, ALPACA keys, or any DB credentials.
#   - Never touches oms_outbox, oms_inbox, broker_order_map, runs, or arm_state.
#   - Never calls broker order endpoints.
#   - No signal injection, no OMS writes.
#   - Fails clearly on missing provider key when sync is needed.
#   - Fails closed (no guessing) if -Source is not passed and the registry does not
#     cleanly authorize a single provider for every requested -Symbols/-Timeframe pair.
# =============================================================================

[CmdletBinding()]
param(
    [AllowEmptyString()]
    [string] $Source              = '',
    [string] $Symbols             = 'AAPL',
    [string] $Timeframe           = '5m',
    [string] $RegistryPath        = 'config\instruments\equities.json',
    [int]    $IntervalSeconds     = 300,
    [int]    $MinRefreshIntervalSeconds = 300,
    [int]    $DurationSeconds     = 1800,
    [int]    $MinCompletedBars    = 30,
    [int]    $MaxStalenessMinutes = -1,
    [string] $PaperDbUrl         = 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable',
    [string] $RepoRoot            = '',
    [switch] $CheckOnly,
    [switch] $Once
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
function Write-Step { param([string]$M) Write-Host "[INTRADAY] $M"          -ForegroundColor Cyan    }
function Write-Ok   { param([string]$M) Write-Host "[INTRADAY] OK: $M"      -ForegroundColor Green   }
function Write-Warn { param([string]$M) Write-Host "[INTRADAY] WARN: $M"    -ForegroundColor Yellow  }
function Write-Fail { param([string]$M) Write-Host "[INTRADAY] FAIL: $M"    -ForegroundColor Red     }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Resolve repo root
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

# ---------------------------------------------------------------------------
# Load .env.local if MQK_DATABASE_URL is not already set (process env wins)
# ---------------------------------------------------------------------------
$envLocalPath = Join-Path $RepoRoot '.env.local'
if ([string]::IsNullOrWhiteSpace($env:MQK_DATABASE_URL) -and (Test-Path $envLocalPath)) {
    foreach ($line in Get-Content -Path $envLocalPath) {
        if ([string]::IsNullOrWhiteSpace($line)) { continue }
        $trimmed = $line.Trim()
        if ($trimmed.StartsWith('#')) { continue }
        $idx = $trimmed.IndexOf('=')
        if ($idx -lt 1) { continue }
        $varName  = $trimmed.Substring(0, $idx).Trim()
        $varValue = $trimmed.Substring($idx + 1).Trim()
        if (($varValue.StartsWith('"') -and $varValue.EndsWith('"')) -or
            ($varValue.StartsWith("'") -and $varValue.EndsWith("'"))) {
            if ($varValue.Length -ge 2) { $varValue = $varValue.Substring(1, $varValue.Length - 2) }
        }
        if ([string]::IsNullOrWhiteSpace($varName)) { continue }
        $existing = [Environment]::GetEnvironmentVariable($varName, 'Process')
        if ([string]::IsNullOrWhiteSpace($existing)) {
            Set-Item -Path "Env:$varName" -Value $varValue
        }
    }
    Write-Ok ".env.local loaded (values not printed)."
}

# ---------------------------------------------------------------------------
# Force paper DB URL
# ---------------------------------------------------------------------------
$env:MQK_DATABASE_URL = $PaperDbUrl

# ---------------------------------------------------------------------------
# GUARD: refuse if URL does not contain port 5440
# ---------------------------------------------------------------------------
if ($env:MQK_DATABASE_URL -notmatch '5440') {
    Write-Fail "MQK_DATABASE_URL does not contain port 5440 (paper DB expected). Refusing."
    exit 1
}
Write-Ok "Paper DB guard passed (port 5440 confirmed)."

# ---------------------------------------------------------------------------
# GUARD: no live routing
# (Declared explicitly; actual guard is port 5440 above and no broker calls below.)
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# Parse symbol list
# ---------------------------------------------------------------------------
$symbolList = @($Symbols -split '[,; ]+' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { $_.Trim().ToUpper() })
if ($symbolList.Count -eq 0) {
    Write-Fail "No symbols provided."
    exit 1
}
Write-Step "Symbols: $($symbolList -join ', ')  Timeframe: $Timeframe"

# ---------------------------------------------------------------------------
# Resolve -Source from the instrument registry when not explicitly passed
# (MARKET-DATA-PROVIDER-PROVENANCE-01-REPAIR-01, defect B). A stale hardcoded
# default must never override the registry: -Source now defaults to '' and,
# when empty, is derived per-symbol from config\instruments\equities.json,
# scoped to instruments that are enabled, asset_class=equity, and whose
# timeframes authorize -Timeframe -- mirroring the same admission contract
# `mqk-cli md sync-provider --symbols-from-registry` enforces. Fails closed
# (exit 1, no guessing) rather than silently picking a provider when the
# registry doesn't cleanly resolve, or when -Symbols mixes providers this
# single -Source script cannot represent (that grouping/orchestration
# belongs to MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01).
# ---------------------------------------------------------------------------
function Resolve-RegistryProvider {
    param([string[]]$Symbols, [string]$Timeframe, [string]$RegistryFullPath)
    if (-not (Test-Path $RegistryFullPath)) {
        throw "instrument registry not found at $RegistryFullPath -- cannot auto-derive provider. Pass -Source explicitly."
    }
    try {
        $registryJson = Get-Content -Path $RegistryFullPath -Raw | ConvertFrom-Json
    } catch {
        throw "instrument registry at $RegistryFullPath could not be parsed ($_) -- cannot auto-derive provider. Pass -Source explicitly."
    }
    $tfTrim = $Timeframe.Trim()
    $resolvedProviders = @{}
    foreach ($sym in $Symbols) {
        $registryMatches = @($registryJson | Where-Object {
            $_.enabled -eq $true -and
            $_.asset_class -eq 'equity' -and
            $_.symbol -eq $sym -and
            (@($_.timeframes) -contains $tfTrim)
        })
        if ($registryMatches.Count -eq 0) {
            throw "registry has no enabled equity instrument for symbol=$sym timeframe=$tfTrim -- refusing to guess a provider. Pass -Source explicitly, or fix the registry entry."
        }
        if ($registryMatches.Count -gt 1) {
            throw "registry has $($registryMatches.Count) enabled equity instruments for symbol=$sym timeframe=$tfTrim -- ambiguous. Pass -Source explicitly."
        }
        $resolvedProviders[$sym] = $registryMatches[0].provider
    }
    $distinctProviders = @($resolvedProviders.Values | Select-Object -Unique)
    if ($distinctProviders.Count -gt 1) {
        throw "symbols [$($Symbols -join ', ')] resolve to multiple providers ($($distinctProviders -join ', ')) for timeframe=$tfTrim -- this script represents a single -Source value and refuses to silently pick one. Split into separate invocations, pass -Source explicitly, or wait for MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01."
    }
    return $distinctProviders[0]
}

if ([string]::IsNullOrWhiteSpace($Source)) {
    $registryFullPath = Join-Path $RepoRoot $RegistryPath
    try {
        $Source = Resolve-RegistryProvider -Symbols $symbolList -Timeframe $Timeframe -RegistryFullPath $registryFullPath
        Write-Ok "Provider auto-derived from instrument registry: source=$Source (symbols=$($symbolList -join ','), timeframe=$Timeframe, registry=$registryFullPath)"
    } catch {
        Write-Fail "Could not auto-derive provider from registry: $_"
        Write-Fail "Pass -Source twelvedata|alpaca explicitly, or fix the registry entry at $registryFullPath."
        exit 1
    }
}
if ($Source -ne 'twelvedata' -and $Source -ne 'alpaca') {
    Write-Fail "Unsupported -Source '$Source'. Supported: twelvedata, alpaca."
    exit 1
}

# ---------------------------------------------------------------------------
# Provider key presence check (never print values)
# ---------------------------------------------------------------------------
Write-Step "Provider source: $Source"

$twelvekeyPresent = -not [string]::IsNullOrWhiteSpace($env:TWELVEDATA_API_KEY)
$alpacaKeyPresent = (-not [string]::IsNullOrWhiteSpace($env:ALPACA_API_KEY_PAPER)) -and `
                   (-not [string]::IsNullOrWhiteSpace($env:ALPACA_API_SECRET_PAPER))

if ($Source -eq 'alpaca') {
    if ($alpacaKeyPresent) {
        Write-Ok "ALPACA_API_KEY_PAPER and ALPACA_API_SECRET_PAPER are configured (values not printed)."
    } else {
        Write-Warn "ALPACA_API_KEY_PAPER or ALPACA_API_SECRET_PAPER is not set. Provider sync top-off will be skipped."
        Write-Warn "Add ALPACA_API_KEY_PAPER and ALPACA_API_SECRET_PAPER to .env.local to enable Alpaca sync."
    }
} else {
    if ($twelvekeyPresent) {
        Write-Ok "TWELVEDATA_API_KEY is configured (value not printed)."
    } else {
        Write-Warn "TWELVEDATA_API_KEY is not set. Provider sync top-off will be skipped."
        Write-Warn "Add TWELVEDATA_API_KEY to .env.local to enable live sync."
    }
}

function Test-IsIntradayTimeframe {
    param([string]$Tf)
    $t = $Tf.Trim().ToLowerInvariant().Replace(' ', '')
    if ([string]::IsNullOrWhiteSpace($t)) { return $false }
    switch ($t) {
        '1d' { return $false }
        'd1' { return $false }
        '1day' { return $false }
        'daily' { return $false }
        default { return $true }
    }
}

function Get-MaxAllowedAgeSeconds {
    param([string]$Tf, [int]$ExplicitMaxStalenessMinutes)
    if ($ExplicitMaxStalenessMinutes -ge 0) {
        return [int64]$ExplicitMaxStalenessMinutes * 60
    }
    if (Test-IsIntradayTimeframe -Tf $Tf) {
        $raw = $env:MQK_INTRADAY_BAR_MAX_AGE_SECS
        $parsed = 0
        if ((-not [string]::IsNullOrWhiteSpace($raw)) -and [int]::TryParse($raw, [ref]$parsed) -and $parsed -ge 0) {
            return [int64]$parsed
        }
        return [int64]900
    }
    return [int64](4 * 24 * 3600)
}

$MaxAllowedAgeSecs = Get-MaxAllowedAgeSeconds -Tf $Timeframe -ExplicitMaxStalenessMinutes $MaxStalenessMinutes
$EffectiveMaxStalenessMinutes = [int][math]::Ceiling($MaxAllowedAgeSecs / 60.0)
if ($MaxStalenessMinutes -lt 0) {
    $MaxStalenessMinutes = $EffectiveMaxStalenessMinutes
}

Write-Step "MinCompletedBars: $MinCompletedBars  MaxStalenessMinutes: $MaxStalenessMinutes  MaxAllowedAgeSecs: $MaxAllowedAgeSecs"

if ($MinRefreshIntervalSeconds -lt 300) {
    Write-Warn "MinRefreshIntervalSeconds=$MinRefreshIntervalSeconds is below 300; using 300 to avoid provider hammering."
    $MinRefreshIntervalSeconds = 300
}
$EffectiveIntervalSeconds = [math]::Max($IntervalSeconds, $MinRefreshIntervalSeconds)
if ($EffectiveIntervalSeconds -ne $IntervalSeconds) {
    Write-Warn "IntervalSeconds=$IntervalSeconds suppressed by MinRefreshIntervalSeconds=$MinRefreshIntervalSeconds; effective interval=${EffectiveIntervalSeconds}s."
}
Write-Step "Effective refresh interval: ${EffectiveIntervalSeconds}s"

# ---------------------------------------------------------------------------
# Helper: query md_bars summary for one symbol/timeframe
# Returns: completed_count, max_ts_unix (bigint), max_ts_iso (ISO string), query_ok
# ---------------------------------------------------------------------------
function Get-MdBarSummary {
    param([string]$Sym, [string]$Tf)
    $r = [pscustomobject]@{
        completed_count = 0
        max_ts_unix     = $null
        max_ts_iso      = $null
        query_ok        = $false
    }
    try {
        $cQ   = "SELECT count(*) FROM md_bars WHERE symbol='$Sym' AND timeframe='$Tf' AND is_complete=true"
        $rawC = docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -t -A -q -c $cQ 2>$null
        $numC = ($rawC -join '') -replace '\s',''
        if ($numC -notmatch '^\d+$') { return $r }
        $r.completed_count = [int]$numC

        # Max end_ts as unix epoch and as ISO 8601
        $tQ   = "SELECT max(end_ts), to_timestamp(max(end_ts)) AT TIME ZONE 'UTC' FROM md_bars WHERE symbol='$Sym' AND timeframe='$Tf' AND is_complete=true"
        $rawT = docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -t -A -q -F'|' -c $tQ 2>$null
        $lineT = ($rawT -join '').Trim()
        if ($lineT -match '\|') {
            $parts = $lineT -split '\|'
            if ($parts.Count -ge 2) {
                $tsRaw = $parts[0].Trim()
                if ($tsRaw -match '^\d+$') { $r.max_ts_unix = [long]$tsRaw }
                $r.max_ts_iso = $parts[1].Trim()
            }
        }
        $r.query_ok = $true
    } catch {}
    return $r
}

# ---------------------------------------------------------------------------
# Helper: staleness in seconds from latest bar end_ts to now UTC
# Returns -1 if end_ts is null/unparseable.
# ---------------------------------------------------------------------------
function Get-StalenessSeconds {
    param($MaxTsUnix)
    if ($null -eq $MaxTsUnix) { return -1 }
    try {
        $nowUnix     = [long](Get-Date -UFormat '%s' -Date (Get-Date).ToUniversalTime())
        $diffSeconds = $nowUnix - [long]$MaxTsUnix
        return [int64][math]::Max(0, $diffSeconds)
    } catch { return -1 }
}

function Get-StalenessMinutes {
    param($MaxTsUnix)
    $seconds = Get-StalenessSeconds -MaxTsUnix $MaxTsUnix
    if ($seconds -lt 0) { return -1 }
    return [int][math]::Floor($seconds / 60)
}

function New-FreshnessVerdict {
    param(
        [bool]$ProviderSuccess,
        $ProviderRowsRead,
        $LatestTsUnix,
        [int]$CompletedCount,
        [string[]]$ExistingReasons
    )
    $ageSecs = Get-StalenessSeconds -MaxTsUnix $LatestTsUnix
    $reasonCode = 'fresh_after_refresh'
    $truthState = 'fresh_after_refresh'
    $passed = $true

    if ($ProviderSuccess -and $null -ne $ProviderRowsRead -and [int64]$ProviderRowsRead -eq 0) {
        $passed = $false
        $truthState = 'provider_returned_no_rows'
        $reasonCode = 'provider_returned_no_rows'
    } elseif ($CompletedCount -le 0 -or $null -eq $LatestTsUnix) {
        $passed = $false
        $truthState = 'latest_completed_bar_missing'
        $reasonCode = 'latest_completed_bar_missing'
    } elseif ($ageSecs -gt $MaxAllowedAgeSecs) {
        $passed = $false
        $truthState = 'stale_after_refresh'
        $reasonCode = if ($ProviderSuccess) { 'provider_returned_stale_intraday_data' } else { 'latest_bar_stale_after_refresh' }
    } elseif ($ExistingReasons.Count -gt 0) {
        $passed = $false
        $truthState = 'refresh_failed'
        $reasonCode = if ($ExistingReasons -match 'provider') { 'provider_error' } else { 'refresh_failed' }
    }

    return [pscustomobject]@{
        latest_completed_bar_age_secs = if ($ageSecs -ge 0) { $ageSecs } else { $null }
        max_allowed_age_secs          = $MaxAllowedAgeSecs
        freshness_truth_state         = $truthState
        reason_code                   = $reasonCode
        passed                        = $passed
    }
}

# ---------------------------------------------------------------------------
# Helper: write evidence JSON
# ---------------------------------------------------------------------------
function Write-Evidence {
    param([object[]]$SymbolResults, [bool]$AllPassed, [string]$Reason, [string]$Mode)
    $evDir = Join-Path $RepoRoot 'exports\market_data'
    try { New-Item -ItemType Directory -Force -Path $evDir | Out-Null } catch {}
    $stamp   = Get-Date -Format 'yyyyMMdd_HHmmss'
    $evFile  = Join-Path $evDir "intraday_refresh_${stamp}.json"
    $payload = [pscustomobject]@{
        schema_version              = 'intraday-refresh-v1'
        produced_at_utc             = (Get-Date).ToUniversalTime().ToString('o')
        mode                        = $Mode
        source                      = $Source
        timeframe                   = $Timeframe
        provider_configured         = if ($Source -eq 'alpaca') { $alpacaKeyPresent } else { $twelvekeyPresent }
        min_refresh_interval_secs   = $MinRefreshIntervalSeconds
        effective_interval_secs     = $EffectiveIntervalSeconds
        min_completed_bars_required = $MinCompletedBars
        max_staleness_minutes       = $MaxStalenessMinutes
        max_allowed_age_secs        = $MaxAllowedAgeSecs
        all_passed                  = $AllPassed
        reason                      = $Reason
        symbols                     = $SymbolResults
    }
    try {
        $payload | ConvertTo-Json -Depth 6 | Set-Content -Path $evFile -Encoding ASCII
        Write-Ok "Evidence written: $evFile"
    } catch {
        Write-Warn "Could not write evidence file: $_"
    }
}

# ---------------------------------------------------------------------------
# CHECK-ONLY mode
# ---------------------------------------------------------------------------
if ($CheckOnly) {
    Write-Sect "CHECK-ONLY: bar counts, freshness, key presence (no mutations)"

    try {
        $null = Get-Command 'docker' -ErrorAction Stop
        Write-Ok "docker command available."
    } catch {
        Write-Fail "docker command not found."
        exit 1
    }

    $pgOk = $false
    try {
        $null = docker exec mqk-paper-postgres pg_isready -U postgres -d miniquantdesk_paper 2>$null
        $pgOk = ($LASTEXITCODE -eq 0)
    } catch {}
    if ($pgOk) { Write-Ok "Paper DB is reachable." } else { Write-Warn "Paper DB not reachable." }

    $checkResults = @()
    foreach ($sym in $symbolList) {
        $s = Get-MdBarSummary -Sym $sym -Tf $Timeframe
        if ($s.query_ok) {
            $staleMin = Get-StalenessMinutes -MaxTsUnix $s.max_ts_unix
            Write-Ok ("${sym}/${Timeframe}: completed=$($s.completed_count)  latest_bar=$($s.max_ts_iso)  staleness=${staleMin}min")
            if ($s.completed_count -lt $MinCompletedBars) {
                Write-Warn "  -> below MinCompletedBars=$MinCompletedBars"
            }
            if ($staleMin -gt $MaxStalenessMinutes) {
                Write-Warn "  -> stale by ${staleMin}min (threshold=${MaxStalenessMinutes}min)"
            } elseif ($staleMin -ge 0) {
                Write-Ok "  -> freshness OK (${staleMin}min <= ${MaxStalenessMinutes}min threshold)"
            }
        } else {
            Write-Warn "${sym}/${Timeframe}: could not query md_bars."
        }
        $checkResults += [pscustomobject]@{
            symbol          = $sym
            timeframe       = $Timeframe
            provider_source = $Source
            provider_configured = if ($Source -eq 'alpaca') { $alpacaKeyPresent } else { $twelvekeyPresent }
            completed_count = $s.completed_count
            max_ts_iso      = $s.max_ts_iso
            latest_completed_bar_age_secs = (Get-StalenessSeconds -MaxTsUnix $s.max_ts_unix)
            max_allowed_age_secs = $MaxAllowedAgeSecs
            staleness_min   = (Get-StalenessMinutes -MaxTsUnix $s.max_ts_unix)
            query_ok        = $s.query_ok
        }
    }

    if ($Source -eq 'alpaca') {
        if ($alpacaKeyPresent) { Write-Ok "ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER configured." }
        else { Write-Warn "ALPACA_API_KEY_PAPER or ALPACA_API_SECRET_PAPER not configured -- Alpaca sync unavailable." }
    } else {
        if ($twelvekeyPresent) { Write-Ok "TWELVEDATA_API_KEY configured." }
        else { Write-Warn "TWELVEDATA_API_KEY not configured -- sync unavailable." }
    }

    Write-Evidence -SymbolResults $checkResults -AllPassed $true -Reason 'check-only' -Mode 'check_only'
    Write-Sect "CHECK-ONLY complete (no mutations)"
    exit 0
}

# ---------------------------------------------------------------------------
# Run one refresh for all symbols, return $true if all passed gates
#
# PAPER-SMOKE-MD-REFRESH-FAIL-CLOSED-01: smoke readiness ($allPassed -- the
# -Once mode exit code and the all_passed evidence field) fails closed when
# ANY of the following occur for ANY symbol:
#   - provider sync fails or throws
#   - provider key/config is missing
#   - completed bar count is below -MinCompletedBars
#   - latest completed bar is staler than -MaxStalenessMinutes
# This does not change interval-loop behavior -- the loop discards
# Invoke-OneRefresh's return value (`$null = Invoke-OneRefresh`) and keeps
# refreshing on its own schedule regardless of readiness outcome.
# ---------------------------------------------------------------------------
function Invoke-OneRefresh {
    $coreRs    = Join-Path $RepoRoot 'core-rs'
    $symResults = @()
    $anyFailed  = $false

    foreach ($sym in $symbolList) {
        $symFailed  = $false
        $symReasons = @()
        $attemptedAtUtc = (Get-Date).ToUniversalTime().ToString('o')
        $providerAttempted = $false
        $providerSuccess = $false
        $providerExitCode = $null
        $providerRowsRead = $null
        $providerRowsOk = $null
        $providerRowsInserted = $null
        $providerRowsUpdated = $null
        $providerRowsKept = $null
        $providerDroppedIncomplete = $null
        $providerDroppedCurrent = $null
        $providerLatestCompletedEndTs = $null

        Write-Step "[$sym/$Timeframe] Before refresh..."
        $before = Get-MdBarSummary -Sym $sym -Tf $Timeframe
        if ($before.query_ok) {
            $staleMin = Get-StalenessMinutes -MaxTsUnix $before.max_ts_unix
            Write-Step "  before: completed=$($before.completed_count)  latest_bar=$($before.max_ts_iso)  staleness=${staleMin}min"
        } else {
            Write-Warn "  could not query md_bars before refresh."
        }

        $providerKeyReady = if ($Source -eq 'alpaca') { $alpacaKeyPresent } else { $twelvekeyPresent }
        if ($providerKeyReady) {
            Write-Step "[$sym/$Timeframe] Running provider sync top-off (source=$Source)..."
            Push-Location $coreRs
            try {
                $local:ErrorActionPreference = 'Continue'
                $providerAttempted = $true
                $syncOutput = cargo run -p mqk-cli --bin mqk-cli -- md sync-provider `
                    --source $Source `
                    --symbols $sym `
                    --timeframe $Timeframe `
                    --full-start "2024-01-01" 2>&1
                $providerExitCode = $LASTEXITCODE
                $syncOutput | Out-Host
                foreach ($line in $syncOutput) {
                    $lineText = [string]$line
                    if ($lineText -match 'completed_bar_filter .*rows_in=(\d+) rows_kept=(\d+) dropped_incomplete_flag=(\d+) dropped_current=(\d+) latest_completed_end_ts=([^\s]+)') {
                        $providerRowsKept = [int]$Matches[2]
                        $providerDroppedIncomplete = [int]$Matches[3]
                        $providerDroppedCurrent = [int]$Matches[4]
                        $providerLatestCompletedEndTs = $Matches[5]
                    }
                    if ($lineText -match 'rows_read=(\d+) rows_ok=(\d+) rejected=(\d+) inserted=(\d+) updated=(\d+)') {
                        $providerRowsRead = [int]$Matches[1]
                        $providerRowsOk = [int]$Matches[2]
                        $providerRowsInserted = [int]$Matches[4]
                        $providerRowsUpdated = [int]$Matches[5]
                    }
                }
                if ($providerExitCode -ne 0) {
                    Write-Warn "[$sym/$Timeframe] Provider sync failed (exit $providerExitCode). Using existing bars."
                    $symFailed = $true
                    $symReasons += "provider sync failed (exit $providerExitCode)"
                } else {
                    $providerSuccess = $true
                    Write-Ok "[$sym/$Timeframe] Provider sync complete."
                }
            } catch {
                Write-Warn "[$sym/$Timeframe] Provider sync threw exception ($_). Using existing bars."
                $symFailed = $true
                $symReasons += "provider sync threw exception"
            } finally { Pop-Location }
        } else {
            $missingVar = if ($Source -eq 'alpaca') { 'ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER' } else { 'TWELVEDATA_API_KEY' }
            Write-Warn "[$sym/$Timeframe] Skipping provider sync ($missingVar not set)."
            $symFailed = $true
            $symReasons += "provider key/config missing ($missingVar)"
        }

        $after = Get-MdBarSummary -Sym $sym -Tf $Timeframe
        if (-not $after.query_ok) {
            Write-Fail "[$sym/$Timeframe] Could not query md_bars after refresh."
            $anyFailed = $true
            $symReasons += "could not query md_bars after refresh"
            $symResults += [pscustomobject]@{
                symbol          = $sym
                timeframe       = $Timeframe
                provider_source = $Source
                attempted_at_utc = $attemptedAtUtc
                provider_configured = $providerKeyReady
                provider_attempted = $providerAttempted
                provider_success = $providerSuccess
                provider_exit_code = $providerExitCode
                provider_rows_read = $providerRowsRead
                provider_rows_ok = $providerRowsOk
                provider_rows_inserted = $providerRowsInserted
                provider_rows_updated = $providerRowsUpdated
                provider_rows_kept_after_completion_filter = $providerRowsKept
                provider_rows_dropped_incomplete = $providerDroppedIncomplete
                provider_rows_dropped_current = $providerDroppedCurrent
                provider_latest_completed_end_ts = $providerLatestCompletedEndTs
                gate            = 'FAIL'
                completed_count = 0
                max_ts_iso      = $null
                latest_completed_bar_age_secs = $null
                max_allowed_age_secs = $MaxAllowedAgeSecs
                freshness_truth_state = 'latest_completed_bar_missing'
                reason_code = 'latest_completed_bar_missing'
                passed = $false
                staleness_min   = -1
                fail_reasons    = $symReasons
            }
            continue
        }

        $staleMinAfter = Get-StalenessMinutes -MaxTsUnix $after.max_ts_unix
        Write-Ok "[$sym/$Timeframe] After refresh: completed=$($after.completed_count)  latest_bar=$($after.max_ts_iso)  staleness=${staleMinAfter}min"

        if ($after.completed_count -lt $MinCompletedBars) {
            Write-Warn "[$sym/$Timeframe] Below MinCompletedBars ($($after.completed_count) < $MinCompletedBars)."
            $symFailed = $true
            $symReasons += "completed_count $($after.completed_count) below MinCompletedBars $MinCompletedBars"
        }
        $staleSecsAfter = Get-StalenessSeconds -MaxTsUnix $after.max_ts_unix
        if ($staleSecsAfter -gt $MaxAllowedAgeSecs) {
            Write-Warn "[$sym/$Timeframe] Still stale by ${staleSecsAfter}s (threshold=${MaxAllowedAgeSecs}s)."
            Write-Warn "  This is expected outside market hours or when no new 5m bars have completed."
            Write-Warn "  Smoke readiness fails closed on stale data regardless of cause (PAPER-SMOKE-MD-REFRESH-FAIL-CLOSED-01)."
            $symFailed = $true
            $symReasons += "latest_bar_stale_after_refresh: stale by ${staleSecsAfter}s (threshold=${MaxAllowedAgeSecs}s)"
        } else {
            Write-Ok "[$sym/$Timeframe] Freshness OK (${staleSecsAfter}s <= ${MaxAllowedAgeSecs}s)."
        }

        $verdict = New-FreshnessVerdict `
            -ProviderSuccess $providerSuccess `
            -ProviderRowsRead $providerRowsRead `
            -LatestTsUnix $after.max_ts_unix `
            -CompletedCount $after.completed_count `
            -ExistingReasons $symReasons
        if (-not $verdict.passed) {
            $symFailed = $true
            if ($symReasons -notcontains $verdict.reason_code) {
                $symReasons += $verdict.reason_code
            }
        }

        if ($symFailed) { $anyFailed = $true }
        $gate = if ($symFailed) { 'FAIL' } else { 'PASS' }

        $symResults += [pscustomobject]@{
            symbol          = $sym
            timeframe       = $Timeframe
            provider_source = $Source
            attempted_at_utc = $attemptedAtUtc
            provider_configured = $providerKeyReady
            provider_attempted = $providerAttempted
            provider_success = $providerSuccess
            provider_exit_code = $providerExitCode
            provider_rows_read = $providerRowsRead
            provider_rows_ok = $providerRowsOk
            provider_rows_inserted = $providerRowsInserted
            provider_rows_updated = $providerRowsUpdated
            provider_rows_kept_after_completion_filter = $providerRowsKept
            provider_rows_dropped_incomplete = $providerDroppedIncomplete
            provider_rows_dropped_current = $providerDroppedCurrent
            provider_latest_completed_end_ts = $providerLatestCompletedEndTs
            gate            = $gate
            completed_count = $after.completed_count
            max_ts_iso      = $after.max_ts_iso
            latest_completed_bar_age_secs = $verdict.latest_completed_bar_age_secs
            max_allowed_age_secs = $verdict.max_allowed_age_secs
            freshness_truth_state = $verdict.freshness_truth_state
            reason_code = $verdict.reason_code
            passed = $verdict.passed
            staleness_min   = $staleMinAfter
            fail_reasons    = $symReasons
        }
    }

    $allPassed = (-not $anyFailed)
    $reason    = if ($allPassed) {
        "refresh complete; all symbols passed fail-closed readiness gates"
    } else {
        $failingSymbols = (($symResults | Where-Object { $_.gate -eq 'FAIL' }) | ForEach-Object {
            "$($_.symbol) [$($_.fail_reasons -join '; ')]"
        }) -join ' | '
        "fail-closed: $failingSymbols"
    }
    $refreshMode = if ($Once) { 'once' } else { 'interval' }
    Write-Evidence -SymbolResults $symResults -AllPassed $allPassed -Reason $reason -Mode $refreshMode
    return $allPassed
}

# ---------------------------------------------------------------------------
# ONCE mode
# ---------------------------------------------------------------------------
if ($Once) {
    Write-Sect "One-shot intraday refresh: $($symbolList -join ', ') / $Timeframe"
    $ok = Invoke-OneRefresh
    Write-Sect "One-shot complete"
    $exitCode = if ($ok) { 0 } else { 1 }
    exit $exitCode
}

# ---------------------------------------------------------------------------
# INTERVAL LOOP mode
# ---------------------------------------------------------------------------
Write-Sect "Intraday refresh loop: $($symbolList -join ', ') / $Timeframe  interval=${EffectiveIntervalSeconds}s  duration=${DurationSeconds}s"
Write-Step "Press Ctrl-C to stop early."

$loopEnd  = (Get-Date).AddSeconds($DurationSeconds)
$iteration = 0

while ((Get-Date) -lt $loopEnd) {
    $iteration++
    $remaining = [int]($loopEnd - (Get-Date)).TotalSeconds
    Write-Sect "Refresh iteration $iteration (${remaining}s remaining)"

    $null = Invoke-OneRefresh

    $nowAfter  = (Get-Date)
    $remaining = [int]($loopEnd - $nowAfter).TotalSeconds
    if ($remaining -le 0) { break }

    $sleepSec = [math]::Min($EffectiveIntervalSeconds, $remaining)
    Write-Step "Next refresh in ${sleepSec}s  (loop ends in ${remaining}s)"
    Start-Sleep -Seconds $sleepSec
}

Write-Sect "Intraday refresh loop complete ($iteration iteration(s))"
exit 0
