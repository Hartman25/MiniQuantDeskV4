# =============================================================================
# DATA-INGESTION-PREMARKET-REFRESH-01
# Prep-PremarketMarketData.ps1
#
# Standalone premarket market-data refresh for MiniQuantDesk V4.
# Targets paper DB only. Syncs md_bars for required symbols/timeframes,
# proves row count and freshness, and writes an evidence file.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1 -CheckOnly
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1 -Symbols AAPL,AMD -Timeframe 1D -MinCompletedBars 30
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan
#
# Parameters:
#   -Symbols           Comma-separated ticker list. Default: AAPL
#   -Timeframe         Bar timeframe (1D, 5m, ...). Default: 1D
#   -MinCompletedBars  Minimum completed bars required per symbol. Default: 30
#   -MaxStalenessDays  Maximum calendar days since latest complete bar. Default: 4 (covers weekends/holidays)
#   -PaperDbUrl        Paper DB connection URL. Default: postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable
#   -RepoRoot          Repo root. Default: auto-resolved two levels up from this script.
#   -CheckOnly         Verify schema, bar counts, key presence. No mutations.
#   -SkipAlpacaIntraday
#                      Skip normal-mode Alpaca intraday top-off for intraday timeframes.
#   -SymbolsFromIngestPlan
#                      WATCHLIST-INGEST-PLAN-01: resolve -Symbols/-Timeframe from
#                      GET /api/v1/market-data/ingest-plan on the local daemon instead of
#                      the -Symbols/-Timeframe parameters. Fails clearly (exit 1) if the
#                      daemon is unreachable or the plan has no required symbols -- this
#                      is an explicit opt-in, so it never silently falls back to AAPL.
#   -SymbolsFromRequiredUniverse
#                      MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01 (preferred going
#                      forward): resolve -Symbols/-Timeframe from
#                      GET /api/v1/market-data/required-universe/plan on the local
#                      daemon -- the same canonical required-symbol resolver as
#                      -SymbolsFromIngestPlan, plus per-requirement resolved
#                      provider/blocker truth. Fails clearly (exit 1) if the daemon is
#                      unreachable, the plan has no requirements, or any requirement
#                      carries a registry/provenance blocker (provider_registry_invalid,
#                      instrument_registry_invalid, provider_symbol_mismatch,
#                      unsupported_timeframe, provider_disabled,
#                      provider_capability_mismatch, provider_provenance_invalid) --
#                      never silently drops the blocked symbol and proceeds with the rest.
#   -DaemonPort        Daemon HTTP port, used only with -SymbolsFromIngestPlan or
#                      -SymbolsFromRequiredUniverse. Default: 8899.
#
# Provider sync top-off (MARKET-DATA-PROVIDER-PROVENANCE-01-REPAIR-01):
#   The per-symbol provider sync top-off stage no longer hardcodes
#   --source twelvedata. It resolves each symbol's provider from the
#   canonical instrument registry (config\instruments\equities.json), scoped
#   to that symbol/-Timeframe pair, and skips the stage (warn, non-fatal)
#   when it does not resolve to exactly one enabled equity instrument --
#   never guesses a provider. This keeps a -Timeframe 5m invocation (e.g.
#   -SymbolsFromIngestPlan, or STEP 5B's MQK_STRATEGY_MD_TIMEFRAME=5m) from
#   writing wrong-provider rows into a symbol's readiness window.
#
# Hard rules:
#   - Paper DB only. Refuses if MQK_DATABASE_URL does not contain port 5440.
#   - Never prints TWELVEDATA_API_KEY, ALPACA keys, or any DB credentials.
#   - Never touches oms_outbox, oms_inbox, broker_order_map, runs, or arm_state.
#   - Fails clearly on missing provider key when data is stale or insufficient.
#   - Never guesses a provider for the sync top-off stage; skips (non-fatal) when
#     the registry does not cleanly authorize exactly one provider for a symbol/timeframe.
# =============================================================================

[CmdletBinding()]
param(
    [string]  $Symbols          = 'AAPL',
    [string]  $Timeframe        = '1D',
    [int]     $MinCompletedBars = 30,
    [int]     $MaxStalenessDays = 4,
    [string]  $PaperDbUrl       = 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable',
    [string]  $RepoRoot         = '',
    [switch]  $CheckOnly,
    [switch]  $SkipAlpacaIntraday,
    [switch]  $SymbolsFromIngestPlan,
    [switch]  $SymbolsFromRequiredUniverse,
    [int]     $DaemonPort       = 8899
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
function Write-Step  { param([string]$M) Write-Host "[PREP] $M"         -ForegroundColor Cyan    }
function Write-Ok    { param([string]$M) Write-Host "[PREP] OK: $M"     -ForegroundColor Green   }
function Write-Warn  { param([string]$M) Write-Host "[PREP] WARN: $M"   -ForegroundColor Yellow  }
function Write-Fail  { param([string]$M) Write-Host "[PREP] FAIL: $M"   -ForegroundColor Red     }
function Write-Sect  { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Resolve repo root
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

# ---------------------------------------------------------------------------
# Safety: assert no trading-table mutations appear anywhere in this script
# (verified by test_premarket_market_data_prep.ps1 guard)
# ---------------------------------------------------------------------------

# ---------------------------------------------------------------------------
# Load .env.local if MQK_DATABASE_URL is not already set
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
# Force paper DB URL regardless of what .env.local may have set
# ---------------------------------------------------------------------------
$env:MQK_DATABASE_URL = $PaperDbUrl

# ---------------------------------------------------------------------------
# GUARD: refuse if URL does not contain port 5440
# ---------------------------------------------------------------------------
if ($env:MQK_DATABASE_URL -notmatch '5440') {
    Write-Fail "MQK_DATABASE_URL does not contain port 5440 (paper DB expected). Refusing to prevent live DB mutation."
    Write-Fail "Current URL port check failed. Set PaperDbUrl or ensure MQK_DATABASE_URL targets port 5440."
    exit 1
}
Write-Ok "Paper DB guard passed (port 5440 confirmed)."

# ---------------------------------------------------------------------------
# GUARD: TWELVEDATA_API_KEY presence check (never print value)
# ---------------------------------------------------------------------------
$twelvekeyPresent = -not [string]::IsNullOrWhiteSpace($env:TWELVEDATA_API_KEY)
if ($twelvekeyPresent) {
    Write-Ok "TWELVEDATA_API_KEY is configured (value not printed)."
} else {
    Write-Warn "TWELVEDATA_API_KEY is not set. Provider sync top-off will be skipped."
    Write-Warn "Add TWELVEDATA_API_KEY to .env.local to enable live sync."
}

# ---------------------------------------------------------------------------
# GUARD: ALPACA paper credential presence check (never print values)
# Used for intraday top-off of intraday timeframes during/after market open.
# TwelveData sync only provides data through previous day's close; Alpaca
# intraday ingest provides today's completed bars (DATA-FRESHNESS-READINESS-GATE-01).
# ---------------------------------------------------------------------------
$alpacaPaperConfigured = (-not [string]::IsNullOrWhiteSpace($env:ALPACA_API_KEY_PAPER)) -and
                         (-not [string]::IsNullOrWhiteSpace($env:ALPACA_API_SECRET_PAPER))
if ($alpacaPaperConfigured) {
    Write-Ok "ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER configured (values not printed)."
} else {
    Write-Warn "ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER not fully configured. Alpaca intraday top-off will be skipped."
    Write-Warn "Add ALPACA_API_KEY_PAPER and ALPACA_API_SECRET_PAPER to .env.local to enable intraday bar refresh."
}

# ---------------------------------------------------------------------------
# Parse symbol list -- or resolve from the daemon ingest-plan route
# (WATCHLIST-INGEST-PLAN-01) when -SymbolsFromIngestPlan is set.
# ---------------------------------------------------------------------------
if ($SymbolsFromRequiredUniverse) {
    # MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01: thin call into the daemon's
    # authoritative required-universe controller instead of re-deriving
    # per-symbol provider truth locally. Read-only (dry-run): zero provider
    # API calls, zero DB writes on the daemon side.
    Write-Sect "Resolving symbols/timeframe from daemon required-universe plan (port $DaemonPort)"
    $requiredUniversePlanUrl = "http://127.0.0.1:$DaemonPort/api/v1/market-data/required-universe/plan"
    try {
        $ruPlan = Invoke-RestMethod -Uri $requiredUniversePlanUrl -Method Get -TimeoutSec 5 -ErrorAction Stop
    } catch {
        Write-Fail "Could not reach daemon required-universe plan route at $requiredUniversePlanUrl ($_)."
        Write-Fail "Start the daemon first, or omit -SymbolsFromRequiredUniverse to use -Symbols/-Timeframe or -SymbolsFromIngestPlan directly."
        exit 1
    }

    if (-not $ruPlan.requirements -or @($ruPlan.requirements).Count -eq 0) {
        Write-Fail ("Daemon required-universe plan returned no requirements " +
                     "(overall_state=$($ruPlan.overall_state), symbol_source=$($ruPlan.symbol_source)).")
        Write-Fail "Configure MQK_PAPER_WATCHLIST_PATH or MQK_STRATEGY_SYMBOL/MQK_STRATEGY_MD_TIMEFRAME on the daemon, or omit -SymbolsFromRequiredUniverse."
        exit 1
    }

    $blockedRequirements = @($ruPlan.requirements | Where-Object { @($_.blockers).Count -gt 0 })
    if ($blockedRequirements.Count -gt 0) {
        foreach ($b in $blockedRequirements) {
            Write-Fail "Requirement $($b.symbol)/$($b.timeframe) is blocked: freshness_state=$($b.freshness_state) blockers=$($b.blockers -join '; ')"
        }
        Write-Fail "One or more required-universe requirements carry a registry/provenance blocker -- refusing to proceed with a partial universe."
        Write-Fail "Fix the blocked requirement(s) in the instrument/provider registry, or omit -SymbolsFromRequiredUniverse."
        exit 1
    }

    $symbolList = @($ruPlan.requirements | ForEach-Object { $_.symbol.Trim().ToUpper() })
    $Symbols    = ($symbolList -join ',')
    $Timeframe  = $ruPlan.requirements[0].timeframe
    Write-Ok ("Required-universe plan: source=$($ruPlan.symbol_source) market_date=$($ruPlan.market_date) " +
              "symbols=$($symbolList -join ', ') timeframe=$Timeframe groups=$($ruPlan.groups.Count)")
} elseif ($SymbolsFromIngestPlan) {
    Write-Sect "Resolving symbols/timeframe from daemon ingest-plan (port $DaemonPort)"
    $ingestPlanUrl = "http://127.0.0.1:$DaemonPort/api/v1/market-data/ingest-plan"
    try {
        $plan = Invoke-RestMethod -Uri $ingestPlanUrl -Method Get -TimeoutSec 5 -ErrorAction Stop
    } catch {
        Write-Fail "Could not reach daemon ingest-plan route at $ingestPlanUrl ($_)."
        Write-Fail "Start the daemon first, or omit -SymbolsFromIngestPlan to use -Symbols/-Timeframe directly."
        exit 1
    }

    if (-not $plan.required_symbols -or @($plan.required_symbols).Count -eq 0) {
        Write-Fail ("Daemon ingest-plan returned no required symbols " +
                     "(truth_state=$($plan.truth_state), symbol_source=$($plan.symbol_source)).")
        Write-Fail "Configure MQK_PAPER_WATCHLIST_PATH or MQK_STRATEGY_SYMBOL/MQK_STRATEGY_MD_TIMEFRAME on the daemon, or omit -SymbolsFromIngestPlan."
        exit 1
    }

    $symbolList = @($plan.required_symbols | ForEach-Object { $_.Trim().ToUpper() })
    $Symbols    = ($symbolList -join ',')
    $Timeframe  = $plan.timeframe
    Write-Ok ("Ingest plan: source=$($plan.symbol_source) truth_state=$($plan.truth_state) " +
              "symbols=$($symbolList -join ', ') timeframe=$Timeframe")
    foreach ($w in @($plan.warnings)) { Write-Warn "ingest-plan: $w" }
} else {
    $symbolList = @($Symbols -split '[,; ]+' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { $_.Trim().ToUpper() })
    if ($symbolList.Count -eq 0) {
        Write-Fail "No symbols provided. Use -Symbols AAPL or set MQK_STRATEGY_SYMBOL."
        exit 1
    }
}
Write-Step "Symbols: $($symbolList -join ', ')  Timeframe: $Timeframe"
Write-Step "MinCompletedBars: $MinCompletedBars  MaxStalenessDays: $MaxStalenessDays"

# ---------------------------------------------------------------------------
# Helper: query md_bars summary for one symbol/timeframe
# Returns pscustomobject with: completed_count, min_bar_date, max_bar_date, query_ok
# Uses end_ts (bigint unix timestamp). to_timestamp converts to date for display.
# ---------------------------------------------------------------------------
function Get-MdBarSummary {
    param([string]$Sym, [string]$Tf)
    $r = [pscustomobject]@{
        completed_count = 0
        min_bar_date    = $null
        max_bar_date    = $null
        query_ok        = $false
    }
    try {
        # Count query
        $cQ   = "SELECT count(*) FROM md_bars WHERE symbol='$Sym' AND timeframe='$Tf' AND is_complete=true"
        $rawC = docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -t -A -q -c $cQ 2>$null
        $numC = ($rawC -join '') -replace '\s',''
        if ($numC -notmatch '^\d+$') { return $r }
        $r.completed_count = [int]$numC

        # Min/max query: end_ts is unix epoch seconds; to_timestamp converts to timestamptz; ::date gives YYYY-MM-DD
        $dQ   = "SELECT to_timestamp(min(end_ts))::date::text, to_timestamp(max(end_ts))::date::text FROM md_bars WHERE symbol='$Sym' AND timeframe='$Tf' AND is_complete=true"
        $rawD = docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -t -A -q -F'|' -c $dQ 2>$null
        $lineD = ($rawD -join '').Trim()
        if ($lineD -match '\|') {
            $parts = $lineD -split '\|'
            if ($parts.Count -ge 2) {
                $r.min_bar_date = $parts[0].Trim()
                $r.max_bar_date = $parts[1].Trim()
            }
        }
        $r.query_ok = $true
    } catch {}
    return $r
}

# ---------------------------------------------------------------------------
# Helper: compute staleness in calendar days from latest bar date to today UTC
# Returns -1 if date cannot be parsed.
# ---------------------------------------------------------------------------
function Get-StalenessDays {
    param([string]$LatestBarDate)
    if ([string]::IsNullOrWhiteSpace($LatestBarDate)) { return -1 }
    try {
        $latestDate = [datetime]::ParseExact($LatestBarDate, 'yyyy-MM-dd', [System.Globalization.CultureInfo]::InvariantCulture)
        $todayUtc   = (Get-Date).ToUniversalTime().Date
        return [int][math]::Floor(($todayUtc - $latestDate).TotalDays)
    } catch { return -1 }
}

# ---------------------------------------------------------------------------
# Helper: resolve the authoritative registry provider for one symbol/timeframe
# (MARKET-DATA-PROVIDER-PROVENANCE-01-REPAIR-01, defect item 9).
#
# Returns the provider string ('twelvedata' | 'alpaca' | ...) when exactly
# one enabled equity instrument in the registry matches Symbol/Timeframe, or
# $null when it does not resolve cleanly (missing registry, unparseable
# registry, no match, or ambiguous match) -- callers must fail closed (skip
# the provider-sync stage, never guess a provider) on $null.
# ---------------------------------------------------------------------------
function Resolve-SymbolRegistryProvider {
    param([string]$Symbol, [string]$Timeframe, [string]$RegistryFullPath)
    if (-not (Test-Path $RegistryFullPath)) { return $null }
    try {
        $registryJson = Get-Content -Path $RegistryFullPath -Raw | ConvertFrom-Json
    } catch { return $null }
    $tfTrim = $Timeframe.Trim()
    $registryMatches = @($registryJson | Where-Object {
        $_.enabled -eq $true -and
        $_.asset_class -eq 'equity' -and
        $_.symbol -eq $Symbol -and
        (@($_.timeframes) -contains $tfTrim)
    })
    if ($registryMatches.Count -ne 1) { return $null }
    return $registryMatches[0].provider
}

# ---------------------------------------------------------------------------
# Helper: write evidence JSON to exports/market_data/
# ---------------------------------------------------------------------------
function Write-Evidence {
    param([object[]]$SymbolResults, [bool]$AllPassed, [string]$Reason)
    $evDir = Join-Path $RepoRoot 'exports\market_data'
    try { New-Item -ItemType Directory -Force -Path $evDir | Out-Null } catch {}
    $stamp   = Get-Date -Format 'yyyyMMdd_HHmmss'
    $evFile  = Join-Path $evDir "premarket_prep_${stamp}.json"
    $payload = [pscustomobject]@{
        schema_version  = 'premarket-prep-v1'
        produced_at_utc = (Get-Date).ToUniversalTime().ToString('o')
        timeframe       = $Timeframe
        min_completed_bars_required = $MinCompletedBars
        max_staleness_days          = $MaxStalenessDays
        all_passed      = $AllPassed
        reason          = $Reason
        symbols         = $SymbolResults
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
    Write-Sect "CHECK-ONLY: schema, bar counts, key presence (no mutations)"

    # Docker availability
    try {
        $null = Get-Command 'docker' -ErrorAction Stop
        Write-Ok "docker command available."
    } catch {
        Write-Fail "docker command not found."
        exit 1
    }

    # Paper DB reachable
    $pgOk = $false
    try {
        $null = docker exec mqk-paper-postgres pg_isready -U postgres -d miniquantdesk_paper 2>$null
        $pgOk = ($LASTEXITCODE -eq 0)
    } catch {}
    if ($pgOk) {
        Write-Ok "Paper DB is reachable (pg_isready OK)."
    } else {
        Write-Warn "Paper DB not reachable. Container may not be running."
    }

    # md_bars table exists
    if ($pgOk) {
        $tblCheck = docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -t -A -q -c "SELECT count(*) FROM information_schema.tables WHERE table_schema='public' AND table_name='md_bars'" 2>$null
        $tblNum   = ($tblCheck -join '') -replace '\s',''
        if ($tblNum -eq '1') {
            Write-Ok "md_bars table present."
        } else {
            Write-Warn "md_bars table not found. Run DB migrations first."
        }
    }

    # Bar counts per symbol
    $checkResults = @()
    foreach ($sym in $symbolList) {
        $s = Get-MdBarSummary -Sym $sym -Tf $Timeframe
        if ($s.query_ok) {
            $staleDays = Get-StalenessDays -LatestBarDate $s.max_bar_date
            Write-Ok ("${sym}/${Timeframe}: completed=$($s.completed_count)  " +
                      "range=$($s.min_bar_date)..$($s.max_bar_date)  " +
                      "staleness=${staleDays}d")
            if ($s.completed_count -lt $MinCompletedBars) {
                Write-Warn "  -> below MinCompletedBars=$MinCompletedBars (ingest needed)"
            }
            if ($staleDays -gt $MaxStalenessDays) {
                Write-Warn "  -> stale by ${staleDays}d (threshold=${MaxStalenessDays}d; sync needed)"
            }
        } else {
            Write-Warn "${sym}/${Timeframe}: could not query md_bars (DB unavailable or table missing)."
        }
        $checkResults += [pscustomobject]@{ symbol=$sym; completed_count=$s.completed_count; max_bar_date=$s.max_bar_date; query_ok=$s.query_ok }
    }

    # Key presence
    if ($twelvekeyPresent) {
        Write-Ok "TWELVEDATA_API_KEY configured."
    } else {
        Write-Warn "TWELVEDATA_API_KEY not configured -- provider sync unavailable."
    }
    if ($alpacaPaperConfigured) {
        Write-Ok "ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER configured -- intraday top-off available in normal mode."
    } else {
        Write-Warn "ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER not fully configured -- intraday top-off unavailable (DATA-FRESHNESS-READINESS-GATE-01 may fail during market hours)."
    }

    Write-Sect "CHECK-ONLY complete"
    Write-Ok "CheckOnly passed (no mutations performed)."
    exit 0
}

# ---------------------------------------------------------------------------
# FULL PREP: per-symbol sync loop
# ---------------------------------------------------------------------------
Write-Sect "Premarket data refresh: $($symbolList -join ', ') / $Timeframe"

$coreRs    = Join-Path $RepoRoot 'core-rs'
$allSymResults = @()
$anyFailed = $false

foreach ($sym in $symbolList) {
    Write-Sect "Symbol: $sym / $Timeframe"

    # --- Pre-sync summary ---
    $before = Get-MdBarSummary -Sym $sym -Tf $Timeframe
    if ($before.query_ok) {
        Write-Step "Before sync: completed=$($before.completed_count)  range=$($before.min_bar_date)..$($before.max_bar_date)"
    } else {
        Write-Warn "Could not query md_bars before sync. Proceeding."
    }

    $beforeCount = if ($before.query_ok) { $before.completed_count } else { 0 }

    # --- CSV bootstrap if row count is low ---
    if ($beforeCount -lt $MinCompletedBars) {
        $csvPath = Join-Path $RepoRoot "exports\md_backup\$Timeframe\${sym}_${Timeframe}.csv"
        if (Test-Path $csvPath) {
            Write-Step "Row count below threshold ($beforeCount < $MinCompletedBars). Loading backup CSV: $csvPath"
            Push-Location $coreRs
            try {
                cargo run -p mqk-cli --bin mqk-cli -- md ingest-csv `
                    --path $csvPath `
                    --timeframe $Timeframe `
                    --source db_backup 2>&1 | Out-Host
                if ($LASTEXITCODE -ne 0) {
                    Write-Fail "md ingest-csv failed for $sym (exit $LASTEXITCODE)."
                    $anyFailed = $true
                    $allSymResults += [pscustomobject]@{ symbol=$sym; stage='csv_ingest'; gate='FAIL'; completed_count=0; max_bar_date=$null; staleness_days=-1 }
                    continue
                }
                Write-Ok "CSV ingest complete for $sym."
            } finally { Pop-Location }
        } else {
            Write-Warn "No backup CSV at $csvPath. Will rely on provider sync (requires TWELVEDATA_API_KEY)."
        }
    } else {
        Write-Ok "Row count sufficient ($beforeCount >= $MinCompletedBars). Skipping CSV ingest."
    }

    # --- Provider sync top-off ---
    # MARKET-DATA-PROVIDER-PROVENANCE-01-REPAIR-01, defect item 9: this stage
    # previously hardcoded --source twelvedata regardless of $sym/$Timeframe,
    # so an invocation with -Timeframe 5m (e.g. via -SymbolsFromIngestPlan,
    # or STEP 5B's MQK_STRATEGY_MD_TIMEFRAME=5m) would write TwelveData-
    # labeled AAPL/5m rows into the readiness window even though the
    # registry assigns AAPL/5m to alpaca -- daily_data_readiness::
    # evaluate_bar_readiness checks provider_id on every bar in the bounded
    # window, not just the latest, so this silently broke provenance. The
    # provider is now resolved per-symbol from the registry and the stage is
    # skipped (not guessed) when it does not resolve cleanly.
    $registryFullPath = Join-Path $RepoRoot 'config\instruments\equities.json'
    $topOffProvider = Resolve-SymbolRegistryProvider -Symbol $sym -Timeframe $Timeframe -RegistryFullPath $registryFullPath
    if ([string]::IsNullOrWhiteSpace($topOffProvider)) {
        Write-Warn "Skipping provider sync top-off for $sym/$Timeframe (no single enabled registry instrument authorizes this symbol/timeframe pair at $registryFullPath; refusing to guess a provider)."
    } elseif ($topOffProvider -eq 'twelvedata') {
        if ($twelvekeyPresent) {
            Write-Step "Provider sync top-off for $sym / $Timeframe (registry provider=twelvedata) ..."
            Push-Location $coreRs
            try {
                # PS7 with $ErrorActionPreference='Stop' can promote cargo stderr (compilation
                # messages) to NativeCommandError. Suppress via local Continue so the exit-code
                # check below handles failures non-fatally.
                $local:ErrorActionPreference = 'Continue'
                cargo run -p mqk-cli --bin mqk-cli -- md sync-provider `
                    --source twelvedata `
                    --symbols $sym `
                    --timeframe $Timeframe `
                    --full-start "2024-01-01" 2>&1 | Out-Host
                if ($LASTEXITCODE -ne 0) {
                    Write-Warn "Provider sync top-off failed for $sym (exit $LASTEXITCODE). Using existing bars."
                } else {
                    Write-Ok "Provider sync top-off complete for $sym."
                }
            } catch {
                Write-Warn "Provider sync top-off threw exception for $sym ($_). Using existing bars."
            } finally { Pop-Location }
        } else {
            Write-Warn "Skipping provider sync top-off for $sym (registry provider=twelvedata but TWELVEDATA_API_KEY not set)."
        }
    } elseif ($topOffProvider -eq 'alpaca') {
        if ($alpacaPaperConfigured) {
            Write-Step "Provider sync top-off for $sym / $Timeframe (registry provider=alpaca) ..."
            Push-Location $coreRs
            try {
                $local:ErrorActionPreference = 'Continue'
                cargo run -p mqk-cli --bin mqk-cli -- md sync-provider `
                    --source alpaca `
                    --symbols $sym `
                    --timeframe $Timeframe `
                    --full-start "2024-01-01" 2>&1 | Out-Host
                if ($LASTEXITCODE -ne 0) {
                    Write-Warn "Provider sync top-off failed for $sym (exit $LASTEXITCODE). Using existing bars."
                } else {
                    Write-Ok "Provider sync top-off complete for $sym."
                }
            } catch {
                Write-Warn "Provider sync top-off threw exception for $sym ($_). Using existing bars."
            } finally { Pop-Location }
        } else {
            Write-Warn "Skipping provider sync top-off for $sym (registry provider=alpaca but ALPACA_API_KEY_PAPER/ALPACA_API_SECRET_PAPER not fully configured)."
        }
    } else {
        Write-Warn "Skipping provider sync top-off for $sym (unsupported registry provider '$topOffProvider')."
    }

    # --- Alpaca intraday top-off (intraday timeframes only) ---
    # TwelveData sync stops at the previous day's close. During and after market
    # open the daemon's DATA-FRESHNESS-READINESS-GATE-01 requires bars younger
    # than 900 s for intraday timeframes. Alpaca's historical API provides
    # today's completed bars; this step is non-fatal so premarket runs (before
    # bars exist) and missing credentials degrade gracefully.
    if ($SkipAlpacaIntraday) {
        Write-Step "Skipping Alpaca intraday top-off for $sym (SkipAlpacaIntraday set)."
    } elseif ($Timeframe -ine '1D') {
        if ($alpacaPaperConfigured) {
            $todayDate = (Get-Date).ToUniversalTime().ToString('yyyy-MM-dd')
            Write-Step "Alpaca intraday refresh: provider=alpaca timeframe=$Timeframe date=$todayDate symbols=$sym"
            Push-Location $coreRs
            try {
                $local:ErrorActionPreference = 'Continue'
                $alpacaOutput = cargo run -p mqk-cli --bin mqk-cli -- md ingest-provider `
                    --source alpaca `
                    --symbols $sym `
                    --timeframe $Timeframe `
                    --start $todayDate `
                    --end $todayDate 2>&1
                $alpacaExitCode = $LASTEXITCODE
                $alpacaOutput | Out-Host

                $alpacaRowsLine = $null
                foreach ($line in $alpacaOutput) {
                    $lineText = [string]$line
                    if ($lineText -match '^rows_read=\d+ rows_ok=\d+ rejected=\d+ inserted=\d+ updated=\d+$') {
                        $alpacaRowsLine = $lineText
                    }
                }
                $alpacaResult = if ([string]::IsNullOrWhiteSpace($alpacaRowsLine)) { "rows=unavailable" } else { $alpacaRowsLine }

                if ($alpacaExitCode -ne 0) {
                    Write-Warn "Alpaca intraday refresh failed: provider=alpaca timeframe=$Timeframe date=$todayDate symbols=$sym exit=$alpacaExitCode $alpacaResult; continuing."
                } else {
                    Write-Ok "Alpaca intraday refresh result: provider=alpaca timeframe=$Timeframe date=$todayDate symbols=$sym exit=0 $alpacaResult"
                }
            } catch {
                Write-Warn "Alpaca intraday refresh threw exception: provider=alpaca timeframe=$Timeframe date=$todayDate symbols=$sym error=$_; continuing."
            } finally { Pop-Location }
        } else {
            Write-Warn "Skipping Alpaca intraday top-off for $sym (ALPACA_API_KEY_PAPER / ALPACA_API_SECRET_PAPER not fully configured)."
        }
    } else {
        Write-Step "Skipping Alpaca intraday top-off for $sym (daily timeframe, not applicable)."
    }

    # --- Post-sync summary ---
    $after = Get-MdBarSummary -Sym $sym -Tf $Timeframe
    if ($after.query_ok) {
        Write-Ok "After sync: completed=$($after.completed_count)  range=$($after.min_bar_date)..$($after.max_bar_date)"
    } else {
        Write-Fail "Could not query md_bars after sync for $sym. Treating as failure."
        $anyFailed = $true
        $allSymResults += [pscustomobject]@{ symbol=$sym; stage='post_query'; gate='FAIL'; completed_count=0; max_bar_date=$null; staleness_days=-1 }
        continue
    }

    # --- Gate 1: row count ---
    if ($after.completed_count -lt $MinCompletedBars) {
        Write-Fail "${sym}: insufficient bars ($($after.completed_count) < $MinCompletedBars required)."
        if (-not $twelvekeyPresent) {
            Write-Fail "  -> Set TWELVEDATA_API_KEY in .env.local to pull historical bars from provider."
        }
        $anyFailed = $true
        $allSymResults += [pscustomobject]@{ symbol=$sym; stage='row_gate'; gate='FAIL'; completed_count=$after.completed_count; max_bar_date=$after.max_bar_date; staleness_days=-1 }
        continue
    }
    Write-Ok "${sym}: row gate passed ($($after.completed_count) >= $MinCompletedBars)."

    # --- Gate 2: freshness ---
    $staleDays = Get-StalenessDays -LatestBarDate $after.max_bar_date
    Write-Step "${sym}: latest bar = $($after.max_bar_date)  staleness = ${staleDays} calendar day(s)."
    if ($staleDays -lt 0) {
        Write-Warn "${sym}: could not determine staleness (latest bar date unparseable). Treating as stale."
        $anyFailed = $true
        $allSymResults += [pscustomobject]@{ symbol=$sym; stage='freshness_gate'; gate='FAIL'; completed_count=$after.completed_count; max_bar_date=$after.max_bar_date; staleness_days=$staleDays }
        continue
    }
    if ($staleDays -gt $MaxStalenessDays) {
        Write-Fail "${sym}: data stale by ${staleDays}d (threshold=${MaxStalenessDays}d). Latest bar: $($after.max_bar_date)."
        if (-not $twelvekeyPresent) {
            Write-Fail "  -> Set TWELVEDATA_API_KEY in .env.local to refresh via provider."
        }
        $anyFailed = $true
        $allSymResults += [pscustomobject]@{ symbol=$sym; stage='freshness_gate'; gate='FAIL'; completed_count=$after.completed_count; max_bar_date=$after.max_bar_date; staleness_days=$staleDays }
        continue
    }
    Write-Ok "${sym}: freshness gate passed (staleness=${staleDays}d <= ${MaxStalenessDays}d)."

    $allSymResults += [pscustomobject]@{
        symbol          = $sym
        stage           = 'complete'
        gate            = 'PASS'
        completed_count = $after.completed_count
        max_bar_date    = $after.max_bar_date
        staleness_days  = $staleDays
    }
}

# ---------------------------------------------------------------------------
# Evidence output
# ---------------------------------------------------------------------------
Write-Sect "Evidence"
$allPassed = (-not $anyFailed)
$reason    = if ($allPassed) { "all symbols passed row and freshness gates" } else { "one or more symbols failed" }
Write-Evidence -SymbolResults $allSymResults -AllPassed $allPassed -Reason $reason

# ---------------------------------------------------------------------------
# Final verdict
# ---------------------------------------------------------------------------
Write-Sect "Premarket data prep result"
foreach ($r in $allSymResults) {
    $gateColor = if ($r.gate -eq 'PASS') { 'Green' } else { 'Red' }
    Write-Host ("  $($r.gate)  $($r.symbol)/$Timeframe  bars=$($r.completed_count)  latest=$($r.max_bar_date)  stale=$($r.staleness_days)d") -ForegroundColor $gateColor
}

if ($allPassed) {
    Write-Ok "All symbols ready. Premarket data prep PASSED."
    exit 0
} else {
    Write-Fail "One or more symbols failed. Resolve above before starting runtime."
    Write-Fail "To diagnose: run with -CheckOnly for a read-only survey."
    exit 1
}
