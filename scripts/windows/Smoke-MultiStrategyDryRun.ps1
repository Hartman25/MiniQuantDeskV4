# =============================================================================
# MARKET-SMOKE-RUNBOOK-01
# Smoke-MultiStrategyDryRun.ps1
#
# Preflight + command-emitter helper for the next trading-day
# MULTI-STRATEGY-DRY-RUN-MARKET-SMOKE-01 smoke (intraday_short_scalper
# evaluated as a dry-run secondary alongside the primary intraday_scalper).
#
# This script is a CHECKLIST HELPER, not a smoke runner. It automates every
# READ-ONLY check below and prints the exact commands for every step that
# would mutate daemon/runtime/market-data state. See
# docs/runbooks/multi_strategy_dry_run_market_smoke.md for the full runbook
# this script accompanies.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Smoke-MultiStrategyDryRun.ps1 -PreflightOnly
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Smoke-MultiStrategyDryRun.ps1 -RunSmoke
#
# Parameters:
#   -PreflightOnly       Explicit safe default. Read-only checks + prints the
#                         manual command checklist. Does not poll a daemon.
#   -RunSmoke            Same read-only checks, PLUS read-only live polling of
#                         whatever daemon/paper-DB is already reachable (dry-run
#                         status, system status, outbox no-order count) so the
#                         final proof checklist can be rendered with live
#                         values during/after a manually-run smoke. Still never
#                         starts/arms/stops anything. If both switches are
#                         passed, -PreflightOnly wins (safer) and a warning is
#                         printed.
#   -DaemonPort          Daemon HTTP port. Default: 8899.
#   -Symbol              Smoke symbol. Default: AAPL.
#   -Timeframe           Bar timeframe. Default: 5m.
#   -PrimaryStrategyId   Primary (live-order-eligible) strategy id. Default: intraday_scalper.
#   -DryRunStrategyId    Secondary dry-run-only strategy id. Default: intraday_short_scalper.
#   -RepoRoot            Repo root directory. Default: two levels up from this script.
#   -AllowDirty          Continue even if the tracked working tree has uncommitted
#                         changes. Untracked files (smoke_logs/, ledger drafts) never
#                         block, with or without this switch.
#
# Exit codes:
#   0 = all selected checks passed (or printed cleanly in -PreflightOnly mode)
#   1 = a check failed (dirty tracked tree, calendar truth unavailable, etc.)
#   2 = BLOCKED_MARKET_CLOSED — repo calendar/session truth says market is
#       closed. This is an expected, non-error outcome, not a bug.
#
# Hard safety rules enforced by this script:
#   - Never starts the daemon.
#   - Never starts the market-data feed scheduler.
#   - Never calls arm-execution, start-system, stop-system, or disarm-execution.
#   - Never submits, cancels, or replaces an order.
#   - Never enables live routing and never sets MQK_DAEMON_DEPLOYMENT_MODE to
#     anything other than "paper".
#   - Never writes to .env.local.
#   - Never prints MQK_OPERATOR_TOKEN, ALPACA_API_KEY*, ALPACA_API_SECRET*,
#     DISCORD_WEBHOOK_URL, or DB password.
#   - Only mutation performed: setting MQK_DAEMON_DEPLOYMENT_MODE,
#     MQK_DAEMON_ADAPTER_ID, MQK_STRATEGY_IDS, MQK_STRATEGY_SYMBOL,
#     MQK_STRATEGY_MD_TIMEFRAME, and MQK_DRY_RUN_STRATEGY_IDS in THIS
#     PowerShell process's own environment (never in .env.local, never in the
#     daemon process unless the operator starts it from this same shell).
# =============================================================================

[CmdletBinding()]
param(
    [switch]$PreflightOnly,
    [switch]$RunSmoke,
    [int]   $DaemonPort        = 8899,
    [string]$Symbol             = 'AAPL',
    [string]$Timeframe          = '5m',
    [string]$PrimaryStrategyId  = 'intraday_scalper',
    [string]$DryRunStrategyId   = 'intraday_short_scalper',
    [string]$RepoRoot           = '',
    [switch]$AllowDirty
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Logging helpers (matches scripts/windows/*.ps1 convention)
# ---------------------------------------------------------------------------
function Write-Step    { param([string]$M) Write-Host "[DRYRUN-SMOKE] $M" -ForegroundColor Cyan }
function Write-Ok      { param([string]$M) Write-Host "[DRYRUN-SMOKE] OK: $M" -ForegroundColor Green }
function Write-Warn    { param([string]$M) Write-Host "[DRYRUN-SMOKE] WARN: $M" -ForegroundColor Yellow }
function Write-Fail    { param([string]$M) Write-Host "[DRYRUN-SMOKE] FAIL: $M" -ForegroundColor Red }
function Write-Unavail { param([string]$M) Write-Host "[DRYRUN-SMOKE] UNAVAILABLE: $M" -ForegroundColor DarkGray }
function Write-Section { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Secret guard: never print these names' values (mirrors other operator scripts)
# ---------------------------------------------------------------------------
$SECRET_NAMES = @(
    'ALPACA_API_KEY_PAPER', 'ALPACA_API_SECRET_PAPER',
    'ALPACA_API_KEY_LIVE',  'ALPACA_API_SECRET_LIVE',
    'MQK_OPERATOR_TOKEN',   'DISCORD_WEBHOOK_URL',
    'POSTGRES_PASSWORD',    'DATABASE_URL', 'MQK_DATABASE_URL'
)

function Assert-NotSecret {
    param([string]$Name, [string]$Value)
    foreach ($s in $SECRET_NAMES) {
        if ($Name -eq $s -and -not [string]::IsNullOrWhiteSpace($Value)) {
            throw "BUG: script attempted to print secret env var '$Name'. Aborting."
        }
    }
}

# ---------------------------------------------------------------------------
# Mode resolution
# ---------------------------------------------------------------------------
if ($PreflightOnly -and $RunSmoke) {
    Write-Warn "-PreflightOnly and -RunSmoke both passed. -PreflightOnly wins (safer)."
    $RunSmoke = $false
}
if (-not $PreflightOnly -and -not $RunSmoke) {
    $PreflightOnly = $true
}
Write-Host ""
Write-Host "=== Smoke-MultiStrategyDryRun.ps1 (MARKET-SMOKE-RUNBOOK-01) ===" -ForegroundColor Cyan
Write-Host "    Mode: $(if ($RunSmoke) { 'RunSmoke (read-only live polling enabled)' } else { 'PreflightOnly (no daemon contact)' })"
Write-Host "    Time: $(Get-Date -Format 'yyyy-MM-dd HH:mm:ss') (local)"
Write-Host ""

# ---------------------------------------------------------------------------
# Resolve repo root
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"
$DaemonBaseUrl = "http://127.0.0.1:$DaemonPort"

$OverallBlocked = $false
$OverallFailed  = $false

# ===========================================================================
# STEP 1: Repo cleanliness (tracked files only)
# ===========================================================================
Write-Section "STEP 1: Repo cleanliness"

$branch = $null
$headLog = $null
try {
    $branch  = (& git -C $RepoRoot branch --show-current 2>&1)
    $headLog = (& git -C $RepoRoot log --oneline -1 2>&1)
    Write-Ok "Branch: $branch"
    Write-Ok "HEAD:   $headLog"
} catch {
    Write-Warn "git metadata unavailable: $_"
}

$dirtyTracked = $null
try {
    $dirtyTracked = & git -C $RepoRoot status --short --untracked-files=no 2>&1
} catch {
    Write-Warn "git status unavailable: $_"
}

if ($dirtyTracked) {
    Write-Fail "Tracked working tree is dirty:"
    $dirtyTracked | ForEach-Object { Write-Fail "  $_" }
    if ($AllowDirty) {
        Write-Warn "-AllowDirty set -- continuing despite dirty tracked tree."
    } else {
        Write-Fail "Refusing to proceed against an uncommitted diff. Resolve, or pass -AllowDirty."
        $OverallFailed = $true
    }
} else {
    Write-Ok "Tracked working tree is clean (untracked files, if any, are not checked)."
}

# ===========================================================================
# STEP 2: Market-session check (fail-closed)
# ===========================================================================
Write-Section "STEP 2: Market-session check"

function Get-NyseSessionHeuristic {
    param([string]$CalendarRsPath)

    $result = [ordered]@{
        source         = 'parsed_calendar_rs_heuristic'
        state          = 'unknown'
        is_trading_day = $false
        et_now         = $null
        note           = $null
    }

    if (-not (Test-Path $CalendarRsPath)) {
        $result.note = "calendar.rs not found at $CalendarRsPath -- cannot determine session truth."
        return $result
    }

    $text = Get-Content -Path $CalendarRsPath -Raw

    $holidaySet = New-Object System.Collections.Generic.HashSet[string]
    $holidayBlock = [regex]::Match($text, 'const HOLIDAYS:[^=]*=\s*&\[(?<body>.*?)\];', 'Singleline')
    if ($holidayBlock.Success) {
        foreach ($m in [regex]::Matches($holidayBlock.Groups['body'].Value, '\((\d{4}),\s*(\d{1,2}),\s*(\d{1,2})\)')) {
            [void]$holidaySet.Add("$($m.Groups[1].Value)-$($m.Groups[2].Value)-$($m.Groups[3].Value)")
        }
    }

    $earlyCloseMap = @{}
    $ecBlock = [regex]::Match($text, 'const EARLY_CLOSE_DATES:[^=]*=\s*&\[(?<body>.*?)\];', 'Singleline')
    if ($ecBlock.Success) {
        foreach ($m in [regex]::Matches($ecBlock.Groups['body'].Value, '\((\d{4}),\s*(\d{1,2}),\s*(\d{1,2}),\s*(\d{1,2}),\s*(\d{1,2})\)')) {
            $key = "$($m.Groups[1].Value)-$($m.Groups[2].Value)-$($m.Groups[3].Value)"
            $earlyCloseMap[$key] = [pscustomobject]@{ Hour = [int]$m.Groups[4].Value; Minute = [int]$m.Groups[5].Value }
        }
    }

    if ($holidaySet.Count -eq 0) {
        $result.note = "Parsed 0 holiday entries from calendar.rs -- parser may be broken; cannot trust calendar truth."
        return $result
    }

    try {
        $tz    = [System.TimeZoneInfo]::FindSystemTimeZoneById('Eastern Standard Time')
        $etNow = [System.TimeZoneInfo]::ConvertTimeFromUtc((Get-Date).ToUniversalTime(), $tz)
    } catch {
        $result.note = "Could not resolve US Eastern timezone on this system."
        return $result
    }
    $result.et_now = $etNow

    $dateKey   = "{0}-{1}-{2}" -f $etNow.Year, $etNow.Month, $etNow.Day
    $isWeekend = ($etNow.DayOfWeek -eq [System.DayOfWeek]::Saturday) -or ($etNow.DayOfWeek -eq [System.DayOfWeek]::Sunday)

    if ($isWeekend) {
        $result.state          = 'closed'
        $result.is_trading_day = $false
        $result.note           = "Weekend (ET day: $($etNow.DayOfWeek))."
        return $result
    }

    if ($holidaySet.Contains($dateKey)) {
        $result.state          = 'holiday'
        $result.is_trading_day = $false
        $result.note           = "Matched NYSE holiday entry ($dateKey) parsed from calendar.rs HOLIDAYS table."
        return $result
    }

    $openSecs     = 9 * 3600 + 30 * 60
    $closeSecs    = 16 * 3600
    $isEarlyClose = $false
    if ($earlyCloseMap.ContainsKey($dateKey)) {
        $ec           = $earlyCloseMap[$dateKey]
        $closeSecs    = $ec.Hour * 3600 + $ec.Minute * 60
        $isEarlyClose = $true
    }
    $nowSecs = $etNow.Hour * 3600 + $etNow.Minute * 60 + $etNow.Second

    $result.is_trading_day = $true
    if ($nowSecs -le $openSecs) {
        $result.state = 'premarket'
    } elseif ($nowSecs -le $closeSecs) {
        $result.state = 'regular_open'
    } elseif ($isEarlyClose) {
        $result.state = 'early_close'
    } else {
        $result.state = 'after_hours'
    }
    $result.note = if ($isEarlyClose) {
        "Early-close day per calendar.rs EARLY_CLOSE_DATES ($dateKey); close=$($earlyCloseMap[$dateKey].Hour):$($earlyCloseMap[$dateKey].Minute) ET."
    } else {
        "Normal session hours 09:30-16:00 ET (calendar.rs HOLIDAYS table; $($holidaySet.Count) entries parsed)."
    }
    return $result
}

$CalendarRsPath = Join-Path $RepoRoot 'core-rs\crates\mqk-integrity\src\calendar.rs'
$sessionSource  = 'heuristic'
$sessionState   = $null

# Prefer live daemon truth if a daemon is already reachable -- it calls the
# same calendar.rs logic directly via NyseWeekdaysProvider.
$liveReadiness = $null
try {
    $liveReadiness = Invoke-RestMethod -Uri "$DaemonBaseUrl/api/v1/autonomous/readiness" -Method Get -TimeoutSec 3 -ErrorAction Stop
} catch {
    $liveReadiness = $null
}

if ($null -ne $liveReadiness -and $null -ne $liveReadiness.PSObject.Properties['session_window_state']) {
    $sessionSource = 'live_daemon_readiness'
    $sessionState  = $liveReadiness.session_window_state
    Write-Ok "Live daemon reachable -- using authoritative session_window_state=$sessionState (session_in_window=$($liveReadiness.session_in_window))."
    $marketOpen = ($liveReadiness.session_in_window -eq $true)
} else {
    Write-Step "Daemon not reachable -- using calendar.rs heuristic parse (preflight-only signal)."
    $heuristic = Get-NyseSessionHeuristic -CalendarRsPath $CalendarRsPath
    $sessionState = $heuristic.state
    Write-Step "Heuristic source: $($heuristic.source)"
    if ($null -ne $heuristic.et_now) {
        Write-Step "Current ET time: $($heuristic.et_now.ToString('yyyy-MM-dd HH:mm:ss')) ($($heuristic.et_now.DayOfWeek))"
    }
    Write-Step "Note: $($heuristic.note)"
    $marketOpen = ($heuristic.state -eq 'regular_open')
    if ($heuristic.state -eq 'unknown') {
        Write-Fail "Calendar truth could not be determined (fail-closed) -- treating as CLOSED."
    }
}

Write-Host ""
if ($marketOpen) {
    Write-Ok "Market session: REGULAR_OPEN (source=$sessionSource). Safe to proceed with the smoke."
} else {
    Write-Fail "BLOCKED_MARKET_CLOSED"
    Write-Fail "  session_state : $sessionState"
    Write-Fail "  source        : $sessionSource"
    Write-Fail "  Do not start the daemon, the feed scheduler, or the runtime."
    Write-Fail "  See docs/runbooks/multi_strategy_dry_run_market_smoke.md section 15."
    $OverallBlocked = $true
}

# ===========================================================================
# STEP 3: Paper DB readiness
# ===========================================================================
Write-Section "STEP 3: Paper DB readiness (mqk-paper-postgres)"

$dbReachable = $false
try {
    $null = Get-Command 'docker' -ErrorAction Stop
    $inspect = docker inspect mqk-paper-postgres 2>&1
    if ($LASTEXITCODE -eq 0) {
        $pgReady = docker exec mqk-paper-postgres pg_isready -U postgres -d miniquantdesk_paper 2>&1
        if ($LASTEXITCODE -eq 0) {
            Write-Ok "mqk-paper-postgres is running and accepting connections (port 5440)."
            $dbReachable = $true
        } else {
            Write-Warn "mqk-paper-postgres container exists but Postgres is not ready: $pgReady"
        }
    } else {
        Write-Warn "mqk-paper-postgres container not found. Create it per Start-PaperTradingSmoke.ps1's header before the full smoke."
    }
} catch {
    Write-Warn "docker not available -- cannot verify paper DB readiness: $_"
}

# ===========================================================================
# STEP 4: Daemon port readiness
# ===========================================================================
Write-Section "STEP 4: Daemon port readiness (port $DaemonPort)"

$daemonReachable = $false
try {
    $health = Invoke-RestMethod -Uri "$DaemonBaseUrl/v1/health" -Method Get -TimeoutSec 3 -ErrorAction Stop
    if ($health.service -eq 'mqk-daemon') {
        $daemonReachable = $true
        Write-Ok "A daemon is already reachable on port $DaemonPort. Checking identity..."
        $sys = $null
        try { $sys = Invoke-RestMethod -Uri "$DaemonBaseUrl/api/v1/system/status" -Method Get -TimeoutSec 3 -ErrorAction Stop } catch {}
        if ($null -ne $sys) {
            Write-Step "  daemon_mode=$($sys.daemon_mode)  adapter_id=$($sys.adapter_id)  live_routing_enabled=$($sys.live_routing_enabled)  runtime_status=$($sys.runtime_status)"
            if ($sys.daemon_mode -ne 'paper') {
                Write-Fail "  daemon_mode != 'paper'. Do not reuse this daemon for this smoke."
                $OverallFailed = $true
            }
            if ($sys.live_routing_enabled -eq $true) {
                Write-Fail "  live_routing_enabled=true on the already-running daemon. Refusing to associate this smoke with it."
                $OverallFailed = $true
            }
        }
    }
} catch {
    Write-Step "No daemon currently reachable on port $DaemonPort (expected before the smoke is started)."
}

if (-not $daemonReachable) {
    $portInUse = $false
    $tcpCheck = New-Object System.Net.Sockets.TcpClient
    try {
        $iar = $tcpCheck.BeginConnect('127.0.0.1', $DaemonPort, $null, $null)
        $portInUse = $iar.AsyncWaitHandle.WaitOne(300)
        if ($portInUse) { $tcpCheck.EndConnect($iar) }
    } catch { $portInUse = $false } finally { $tcpCheck.Close() }

    if ($portInUse) {
        Write-Warn "Port $DaemonPort is occupied but did not answer /v1/health -- a stale, non-daemon process may hold it."
        Write-Warn "Stop it before starting: Get-Process -Name mqk-daemon | Stop-Process -Force"
    } else {
        Write-Ok "Port $DaemonPort is free."
    }
}

# ===========================================================================
# STEP 5: Required environment
# ===========================================================================
Write-Section "STEP 5: Required environment"

$env:MQK_DAEMON_DEPLOYMENT_MODE = 'paper'
$env:MQK_DAEMON_ADAPTER_ID      = 'alpaca'
$env:MQK_STRATEGY_IDS           = $PrimaryStrategyId
$env:MQK_STRATEGY_SYMBOL        = $Symbol
$env:MQK_STRATEGY_MD_TIMEFRAME  = $Timeframe
$env:MQK_DRY_RUN_STRATEGY_IDS   = $DryRunStrategyId

Write-Ok "Exported in THIS PowerShell process only (not .env.local):"
Write-Step "  MQK_DAEMON_DEPLOYMENT_MODE = paper"
Write-Step "  MQK_DAEMON_ADAPTER_ID      = alpaca"
Write-Step "  MQK_STRATEGY_IDS           = $PrimaryStrategyId"
Write-Step "  MQK_STRATEGY_SYMBOL        = $Symbol"
Write-Step "  MQK_STRATEGY_MD_TIMEFRAME  = $Timeframe"
Write-Step "  MQK_DRY_RUN_STRATEGY_IDS   = $DryRunStrategyId"
Write-Warn "Start the daemon from THIS shell (or re-export these vars in the daemon's shell) so the process inherits them."

# ===========================================================================
# Manual command checklist (printed in both modes -- never executed here)
# ===========================================================================
Write-Section "Manual command checklist (steps 6-10, 13 -- not executed by this script)"

Write-Host ""
Write-Host "  STEP 6  Start daemon (MANUAL):" -ForegroundColor White
Write-Host "    powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -CheckOnly"
Write-Host "    powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -WatchSeconds 1800"
Write-Host ""
Write-Host "  STEP 7  Start AAPL/$Timeframe feed scheduler (MANUAL):" -ForegroundColor White
Write-Host "    `$body = @{ provider_id='alpaca'; symbols=@('$Symbol'); timeframe='$Timeframe'; dry_run=`$false; allow_provider_api_calls=`$true } | ConvertTo-Json -Compress"
Write-Host "    Invoke-RestMethod -Uri $DaemonBaseUrl/api/v1/market-data/feed/scheduler/start -Method Post -ContentType 'application/json' -Body `$body"
Write-Host ""
Write-Host "  STEP 8  Arm / start runtime (handled by Start-PaperTradingSmoke.ps1 -- see runbook section 8 if doing it by hand)." -ForegroundColor White
Write-Host ""
Write-Host "  STEP 9  Poll dry-run + readiness (read-only; this script does this for you under -RunSmoke):" -ForegroundColor White
Write-Host "    Invoke-RestMethod $DaemonBaseUrl/api/v1/strategy/dry-run/status | ConvertTo-Json -Depth 6"
Write-Host ""
Write-Host "  STEP 10 GUI check (MANUAL, cannot be automated by this script):" -ForegroundColor White
Write-Host "    Open http://127.0.0.1:1420 -> Strategy screen -> 'Multi-strategy dry-run diagnostics' panel."
Write-Host "    Confirm Submitted=false for every row."
Write-Host ""
Write-Host "  STEP 13 Shutdown (MANUAL):" -ForegroundColor White
Write-Host "    powershell -ExecutionPolicy Bypass -File scripts\windows\Stop-PaperTradingClean.ps1 -Label dry_run_market_smoke"
Write-Host ""

# ===========================================================================
# -RunSmoke: read-only live polling (still never mutates anything)
# ===========================================================================
$dryRunStatus   = $null
$liveSysStatus  = $null
$outboxCount    = $null

if ($RunSmoke -and -not $OverallBlocked) {
    Write-Section "RunSmoke: read-only live polling"

    try {
        $dryRunStatus = Invoke-RestMethod -Uri "$DaemonBaseUrl/api/v1/strategy/dry-run/status" -Method Get -TimeoutSec 5 -ErrorAction Stop
        Write-Ok "Fetched /api/v1/strategy/dry-run/status"
    } catch {
        Write-Unavail "/api/v1/strategy/dry-run/status -- daemon likely not started yet."
    }

    try {
        $liveSysStatus = Invoke-RestMethod -Uri "$DaemonBaseUrl/api/v1/system/status" -Method Get -TimeoutSec 5 -ErrorAction Stop
        Write-Ok "Fetched /api/v1/system/status"
    } catch {
        Write-Unavail "/api/v1/system/status -- daemon likely not started yet."
    }

    if ($dbReachable) {
        try {
            $q = "SELECT count(*) FROM oms_outbox WHERE order_json->>'strategy_id' = '$DryRunStrategyId';"
            $raw = docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -t -A -q -c $q 2>$null
            $num = ($raw -join '') -replace '\s', ''
            if ($num -match '^\d+$') {
                $outboxCount = [int]$num
                if ($outboxCount -eq 0) {
                    Write-Ok "oms_outbox rows for strategy_id='$DryRunStrategyId': 0 (expected)."
                } else {
                    Write-Fail "oms_outbox rows for strategy_id='$DryRunStrategyId': $outboxCount -- INVESTIGATE IMMEDIATELY."
                    $OverallFailed = $true
                }
            }
        } catch {
            Write-Unavail "outbox no-order proof query failed: $_"
        }
    } else {
        Write-Unavail "outbox no-order proof query -- paper DB not reachable."
    }
}

# ===========================================================================
# Final proof checklist
# ===========================================================================
Write-Section "Final proof checklist"

function Field {
    param([string]$Label, $Value)
    $v = if ($null -eq $Value -or [string]::IsNullOrWhiteSpace([string]$Value)) { 'NOT_AVAILABLE -- see manual steps above' } else { $Value }
    Write-Host ("{0,-44} {1}" -f "${Label}:", $v)
}

$diagRow = $null
if ($null -ne $dryRunStatus -and $null -ne $dryRunStatus.dry_run_strategy_diagnostics -and @($dryRunStatus.dry_run_strategy_diagnostics).Count -gt 0) {
    $diagRow = @($dryRunStatus.dry_run_strategy_diagnostics)[0]
}

Field 'Verdict'                                            $(if ($OverallBlocked) { 'BLOCKED_MARKET_CLOSED' } elseif ($OverallFailed) { 'FAILED -- see above' } elseif ($RunSmoke) { 'See live values below' } else { 'PREFLIGHT_PRINTED -- run -RunSmoke during/after the smoke' })
Field 'Branch/commit'                                       "$branch / $headLog"
Field 'Market session'                                      "$sessionState (source=$sessionSource)"
Field 'Daemon mode'                                          $(if ($null -ne $liveSysStatus) { $liveSysStatus.daemon_mode } else { $null })
Field 'MQK_DRY_RUN_STRATEGY_IDS'                              $DryRunStrategyId
Field 'Dry-run status truth_state'                            $(if ($null -ne $dryRunStatus) { $dryRunStatus.truth_state } else { $null })
Field 'Dry-run configured ids'                                $(if ($null -ne $dryRunStatus) { ($dryRunStatus.configured_dry_run_strategy_ids -join ',') } else { $null })
Field 'Dry-run diagnostics count'                             $(if ($null -ne $dryRunStatus) { @($dryRunStatus.dry_run_strategy_diagnostics).Count } else { $null })
Field 'Dry-run strategy_id'                                   $(if ($null -ne $diagRow) { $diagRow.strategy_id } else { $null })
Field 'Dry-run target_qty'                                    $(if ($null -ne $diagRow) { $diagRow.target_qty } else { $null })
Field 'Dry-run would_classify_as'                             $(if ($null -ne $diagRow) { $diagRow.would_classify_as } else { $null })
Field 'Dry-run would_b5_block'                                 $(if ($null -ne $diagRow) { $diagRow.would_b5_block } else { $null })
Field 'Dry-run would_policy_block'                             $(if ($null -ne $diagRow) { $diagRow.would_policy_block } else { $null })
Field 'Dry-run policy_reason_code'                             $(if ($null -ne $diagRow) { $diagRow.policy_reason_code } else { $null })
Field 'Dry-run submitted'                                      $(if ($null -ne $diagRow) { $diagRow.submitted } else { $null })
Field 'GUI observed'                                           'MANUAL -- see STEP 10 above'
Field "$DryRunStrategyId outbox row count"                     $outboxCount
Field "$DryRunStrategyId order/execution row count"            $(if ($null -ne $outboxCount) { "$outboxCount (outbox is the only persisted strategy_id source; see runbook section 11)" } else { $null })
Field 'live_routing'                                            $(if ($null -ne $liveSysStatus) { $liveSysStatus.live_routing_enabled } else { $null })
Field 'Shutdown proof'                                          'MANUAL -- run Stop-PaperTradingClean.ps1 (STEP 13 above)'

Write-Host ""

# ===========================================================================
# Exit code
# ===========================================================================
if ($OverallBlocked) {
    exit 2
}
if ($OverallFailed) {
    exit 1
}
exit 0
