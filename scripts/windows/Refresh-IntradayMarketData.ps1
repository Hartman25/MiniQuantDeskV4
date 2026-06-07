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
#   -Source               Market-data provider: twelvedata | alpaca. Default: twelvedata
#   -Symbols              Comma-separated ticker list. Default: AAPL
#   -Timeframe            Bar timeframe. Default: 5m
#   -IntervalSeconds      Seconds between refreshes in loop mode. Default: 300
#   -DurationSeconds      Total loop duration in seconds. Default: 1800
#   -MinCompletedBars     Minimum completed bars to report OK. Default: 30
#   -MaxStalenessMinutes  Maximum minutes since latest complete bar before warning. Default: 1440
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
# =============================================================================

[CmdletBinding()]
param(
    [ValidateSet('twelvedata', 'alpaca')]
    [string] $Source              = 'twelvedata',
    [string] $Symbols             = 'AAPL',
    [string] $Timeframe           = '5m',
    [int]    $IntervalSeconds     = 300,
    [int]    $DurationSeconds     = 1800,
    [int]    $MinCompletedBars    = 30,
    [int]    $MaxStalenessMinutes = 1440,
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

# ---------------------------------------------------------------------------
# Parse symbol list
# ---------------------------------------------------------------------------
$symbolList = @($Symbols -split '[,; ]+' | Where-Object { -not [string]::IsNullOrWhiteSpace($_) } | ForEach-Object { $_.Trim().ToUpper() })
if ($symbolList.Count -eq 0) {
    Write-Fail "No symbols provided."
    exit 1
}
Write-Step "Symbols: $($symbolList -join ', ')  Timeframe: $Timeframe"
Write-Step "MinCompletedBars: $MinCompletedBars  MaxStalenessMinutes: $MaxStalenessMinutes"

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
# Helper: staleness in minutes from latest bar end_ts to now UTC
# Returns -1 if end_ts is null/unparseable.
# ---------------------------------------------------------------------------
function Get-StalenessMinutes {
    param($MaxTsUnix)
    if ($null -eq $MaxTsUnix) { return -1 }
    try {
        $nowUnix     = [long](Get-Date -UFormat '%s' -Date (Get-Date).ToUniversalTime())
        $diffSeconds = $nowUnix - [long]$MaxTsUnix
        return [int][math]::Floor($diffSeconds / 60)
    } catch { return -1 }
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
        timeframe                   = $Timeframe
        min_completed_bars_required = $MinCompletedBars
        max_staleness_minutes       = $MaxStalenessMinutes
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
            completed_count = $s.completed_count
            max_ts_iso      = $s.max_ts_iso
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
                cargo run -p mqk-cli --bin mqk-cli -- md sync-provider `
                    --source $Source `
                    --symbols $sym `
                    --timeframe $Timeframe `
                    --full-start "2024-01-01" 2>&1 | Out-Host
                if ($LASTEXITCODE -ne 0) {
                    Write-Warn "[$sym/$Timeframe] Provider sync failed (exit $LASTEXITCODE). Using existing bars."
                    $symFailed = $true
                    $symReasons += "provider sync failed (exit $LASTEXITCODE)"
                } else {
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
                gate            = 'FAIL'
                completed_count = 0
                max_ts_iso      = $null
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
        if ($staleMinAfter -gt $MaxStalenessMinutes) {
            Write-Warn "[$sym/$Timeframe] Still stale by ${staleMinAfter}min (threshold=${MaxStalenessMinutes}min)."
            Write-Warn "  This is expected outside market hours or when no new 5m bars have completed."
            Write-Warn "  Smoke readiness fails closed on stale data regardless of cause (PAPER-SMOKE-MD-REFRESH-FAIL-CLOSED-01)."
            $symFailed = $true
            $symReasons += "stale by ${staleMinAfter}min (threshold=${MaxStalenessMinutes}min)"
        } else {
            Write-Ok "[$sym/$Timeframe] Freshness OK (${staleMinAfter}min <= ${MaxStalenessMinutes}min)."
        }

        if ($symFailed) { $anyFailed = $true }
        $gate = if ($symFailed) { 'FAIL' } else { 'PASS' }

        $symResults += [pscustomobject]@{
            symbol          = $sym
            gate            = $gate
            completed_count = $after.completed_count
            max_ts_iso      = $after.max_ts_iso
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
Write-Sect "Intraday refresh loop: $($symbolList -join ', ') / $Timeframe  interval=${IntervalSeconds}s  duration=${DurationSeconds}s"
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

    $sleepSec = [math]::Min($IntervalSeconds, $remaining)
    Write-Step "Next refresh in ${sleepSec}s  (loop ends in ${remaining}s)"
    Start-Sleep -Seconds $sleepSec
}

Write-Sect "Intraday refresh loop complete ($iteration iteration(s))"
exit 0
