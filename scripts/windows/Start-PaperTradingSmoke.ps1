# =============================================================================
# OPERATOR-RUNBOOK-STARTUP-HARDENING-01
# Start-PaperTradingSmoke.ps1
#
# Repeatable paper-trading startup/runbook for MiniQuantDesk V4.
# Paper + Alpaca path only. Never enables live routing.
# All secrets (API keys, operator token, DB password) are never printed.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -NoStartRuntime -WatchSeconds 30
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -SkipGui
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Start-PaperTradingSmoke.ps1 -CheckOnly
#
# Parameters:
#   -RepoRoot         Repo root directory. Default: two levels up from this script.
#   -DaemonPort       Daemon HTTP port. Default: 8899.
#   -PaperDbUrl       Postgres URL for the paper DB. Default: postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable
#   -SessionStart     Session start time HH:MM UTC. Default: 13:30
#   -SessionStop      Session stop time HH:MM UTC. Default: 20:00
#   -WatchSeconds     How long to run the watcher loop (seconds). Default: 420
#   -SkipGui          Skip GUI launch step.
#   -NoStartRuntime   Set env and verify daemon readiness but do not call start-system.
#   -CheckOnly        Check prerequisites only (docker, .env.local). No daemon or runtime start.
#   -MultiSymbolSmoke Require and enforce a valid watchlist-v2 multi-symbol artifact in STEP 9B
#                      (docs/design/native_multi_symbol_dispatch.md Section 9.1). Default: off --
#                      STEP 9B then treats an absent watchlist-v2 artifact as this repo's normal
#                      single-symbol smoke path, not a failure. A watchlist-v2 artifact that IS
#                      configured but invalid/unapproved still fails closed in either mode.
#   -StartIntradayRefreshLoop
#                      Start scripts\windows\Refresh-IntradayMarketData.ps1 as a separate,
#                      explicitly-requested background process so intraday bars stay fresh for
#                      the duration of a market-hours smoke session. Default: off -- this script
#                      never starts provider refresh network activity unless this flag is passed.
#   -RequireIntradayRefresh
#                      Before STEP 15 (runtime start), poll
#                      GET /api/v1/market-data/intraday-refresh/status and fail closed with
#                      actionable guidance unless truth_state=active, stale_or_missing_evidence=
#                      false, and all_passed=true. Default: off -- default smoke behavior remains
#                      observe-only for data freshness beyond STEP 5B's one-shot prep; the
#                      per-tick DATA-FRESHNESS-READINESS-GATE-01 gate still applies either way.
#                      Also fails closed (INTRADAY-PROVIDER-CLOCK-SKEW-01D) when the response's
#                      proof_window_ready field is not true, or -- as a fallback when an older
#                      daemon build has no such field -- when any symbol's freshness_headroom_secs
#                      is below -MinFreshnessHeadroomSeconds. This is a script-preflight-only
#                      guard: it never changes daemon dispatch-tick freshness gate behavior.
#   -IntradayRefreshIntervalSeconds
#                      Refresh interval passed to Refresh-IntradayMarketData.ps1 when
#                      -StartIntradayRefreshLoop is set. Default: 300.
#   -IntradayRefreshDurationSeconds
#                      Total refresh-loop duration passed to Refresh-IntradayMarketData.ps1 when
#                      -StartIntradayRefreshLoop is set. Default: 1800.
#   -MinFreshnessHeadroomSeconds
#                      Minimum seconds of freshness headroom (max_allowed_age_secs minus
#                      latest_completed_bar_age_secs) required, per symbol, before STEP 14C
#                      (-RequireIntradayRefresh) will let the script proceed to STEP 15. Controls
#                      only this script's preflight quality bar -- it does not change
#                      MQK_INTRADAY_BAR_MAX_AGE_SECS or any daemon gate. Default: 120.
#   -StartRequiredUniverseScheduler
#                      MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-01: kept as an explicit
#                      compatibility flag, but it is no longer required for normal use -- see
#                      -SkipRequiredUniverseScheduler below. In STEP 8D, calls
#                      POST /api/v1/market-data/required-universe/start on the daemon so it
#                      maintains freshness for every symbol/timeframe the currently configured
#                      autonomous Paper operation requires (derived from the same canonical
#                      resolver as STEP 5B/STEP 9B) for the rest of this session. Data-only:
#                      never starts/stops the execution runtime, never arms/disarms, never
#                      clears halt/kill-switch/reconcile, submits zero orders.
#                      Passing both -StartRequiredUniverseScheduler and
#                      -StartIntradayRefreshLoop in the same run is refused BEFORE any child
#                      process, provider, or scheduler side effect (validated immediately after
#                      argument parsing, before STEP 1) -- two independent long-running
#                      market-data schedulers must never run concurrently for the same symbols;
#                      see Refresh-IntradayMarketData.ps1's own conflict guard for the reverse
#                      case (standalone script vs. an already-running daemon scheduler).
#   -SkipRequiredUniverseScheduler
#                      MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-01: normal (non-
#                      CheckOnly, non--StartIntradayRefreshLoop) Paper startup now starts the
#                      required-universe daemon scheduler by default, so an operator no longer
#                      has to remember -StartRequiredUniverseScheduler for ongoing data
#                      freshness to be maintained automatically. Pass this switch to opt out and
#                      leave data-freshness maintenance to an operator-run alternative (e.g. a
#                      standalone -StartIntradayRefreshLoop, or nothing at all). -CheckOnly and
#                      -StartIntradayRefreshLoop already imply "do not start the required-
#                      universe scheduler" on their own; this flag only matters for a normal run
#                      that wants neither scheduler.
#   -RequiredUniverseSchedulerDryRun
#                      Start the required-universe scheduler in dry-run mode (checkonly):
#                      zero provider API calls, zero DB writes, status/plan only. Default: off
#                      (real bounded refresh) whenever the scheduler starts, whether via the
#                      default path or -StartRequiredUniverseScheduler.
#
# Hard rules enforced by this script:
#   - Paper+Alpaca path only. Fails if daemon_mode != paper.
#   - Refuses to proceed if live_routing_enabled=true.
#   - Never prints ALPACA_API_KEY*, ALPACA_API_SECRET*, MQK_OPERATOR_TOKEN, DB password, Discord webhook.
#   - MQK_DATABASE_URL is always reasserted to PaperDbUrl after .env.local load.
#   - All order submit/cancel/replace/flatten actions are explicitly absent.
# =============================================================================

[CmdletBinding()]
param(
    [string]$RepoRoot      = '',
    [int]   $DaemonPort    = 8899,
    [string]$PaperDbUrl    = 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable',
    [string]$SessionStart  = '13:30',
    [string]$SessionStop   = '20:00',
    [int]   $WatchSeconds  = 420,
    [switch]$SkipGui,
    [switch]$NoStartRuntime,
    [switch]$CheckOnly,
    [switch]$MultiSymbolSmoke,
    [switch]$StartIntradayRefreshLoop,
    [switch]$RequireIntradayRefresh,
    [int]   $IntradayRefreshIntervalSeconds = 300,
    [int]   $IntradayRefreshDurationSeconds = 1800,
    [int]   $MinFreshnessHeadroomSeconds = 120,
    [switch]$StartRequiredUniverseScheduler,
    [switch]$RequiredUniverseSchedulerDryRun,
    [switch]$SkipRequiredUniverseScheduler
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Logging helpers
# ---------------------------------------------------------------------------
function Write-Step    { param([string]$M) Write-Host "[SMOKE] $M" -ForegroundColor Cyan }
function Write-Ok      { param([string]$M) Write-Host "[SMOKE] OK: $M" -ForegroundColor Green }
function Write-Warn    { param([string]$M) Write-Host "[SMOKE] WARN: $M" -ForegroundColor Yellow }
function Write-Fail    { param([string]$M) Write-Host "[SMOKE] FAIL: $M" -ForegroundColor Red }
function Write-Section { param([string]$M) Write-Host "" ; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Secret guard: never print these names' values
# ---------------------------------------------------------------------------
$SECRET_NAMES = @(
    'ALPACA_API_KEY_PAPER', 'ALPACA_API_SECRET_PAPER',
    'ALPACA_API_KEY_LIVE',  'ALPACA_API_SECRET_LIVE',
    'MQK_OPERATOR_TOKEN',   'DISCORD_WEBHOOK_URL',
    'POSTGRES_PASSWORD',    'DATABASE_URL'
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
# Resolve repo root
# ---------------------------------------------------------------------------
if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

$DaemonBaseUrl = "http://127.0.0.1:$DaemonPort"

# ---------------------------------------------------------------------------
# Early argument validation (before any side effect): scheduler flag conflict
# MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-01 Defect C: this check
# previously ran in STEP 8D, AFTER STEP 8C had already started the legacy
# intraday refresh loop as a side-effecting child process when both flags
# were passed together -- a conflicting invocation still left a stray
# background process running when it refused. Validating here, before
# STEP 1 stops stale processes and before Docker/DB/the daemon/either
# scheduler are ever touched, means a conflicting invocation makes zero
# child-process, provider, or scheduler side effects when it exits non-zero.
# ---------------------------------------------------------------------------
if ($StartRequiredUniverseScheduler -and $StartIntradayRefreshLoop) {
    Write-Fail "Both -StartRequiredUniverseScheduler and -StartIntradayRefreshLoop were passed."
    Write-Fail "Two independent long-running market-data schedulers must never run concurrently for the same symbols."
    Write-Fail "Pass exactly one of the two flags (or neither, to use the default required-universe scheduler)."
    exit 1
}

# MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01-REPAIR-01 §14: the
# required-universe daemon scheduler is now the default data-freshness path
# for a normal Paper startup -- an operator no longer has to remember
# -StartRequiredUniverseScheduler. Ways to get the old/other behavior:
#   -CheckOnly                     -> neither scheduler starts (dry preflight only)
#   -StartIntradayRefreshLoop      -> legacy interval-loop script only; the daemon
#                                      scheduler is explicitly not started (the
#                                      conflict check above already refused the
#                                      case where both were explicitly requested)
#   -SkipRequiredUniverseScheduler -> neither scheduler starts, for a normal run
#                                      that intentionally wants to own freshness
#                                      itself (e.g. no automated refresh at all)
$UseRequiredUniverseScheduler = (-not $CheckOnly) -and (-not $StartIntradayRefreshLoop) -and (-not $SkipRequiredUniverseScheduler)

# Script-scope log vars -- set properly in STEP 7; pre-declared for StrictMode.
$stdoutLog     = $null
$stderrLog     = $null
$logDir        = $null
$operatorToken = $null

# ---------------------------------------------------------------------------
# Helper: Get-PaperMdBarSummary
# Queries md_bars for a symbol/timeframe and returns a summary object with
# completed_count, min_time, max_time, latest_bar. Robust against multiline
# psql output by using -t -A -q flags and numeric-line filtering.
# Returns $null on query failure.
# ---------------------------------------------------------------------------
function Get-PaperMdBarSummary {
    param([string]$Symbol, [string]$Timeframe)
    $result = [pscustomobject]@{
        completed_count = 0
        min_time        = $null
        max_time        = $null
        latest_bar      = $null
        query_ok        = $false
    }
    try {
        $countQ = "SELECT count(*) FROM md_bars WHERE symbol='$Symbol' AND timeframe='$Timeframe' AND is_complete=true"
        $rawC   = docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -t -A -q -c $countQ 2>$null
        $numC   = ($rawC -join '') -replace '\s',''
        if ($numC -match '^\d+$') { $result.completed_count = [int]$numC } else { return $result }

        $detailQ = "SELECT to_timestamp(min(end_ts))::date::text, to_timestamp(max(end_ts))::date::text FROM md_bars WHERE symbol='$Symbol' AND timeframe='$Timeframe' AND is_complete=true"
        $rawD    = docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -t -A -q -F'|' -c $detailQ 2>$null
        $lineD   = ($rawD -join '').Trim()
        if ($lineD -match '\|') {
            $parts = $lineD -split '\|'
            if ($parts.Count -ge 2) {
                $result.min_time  = $parts[0].Trim()
                $result.max_time  = $parts[1].Trim()
                $result.latest_bar = $parts[1].Trim()
            }
        }
        $result.query_ok = $true
    } catch {
        # query failure -- return with query_ok=false
    }
    return $result
}

# ---------------------------------------------------------------------------
# Helper: Get-PaperArmStateSummary
# PAPER-SMOKE-RETEST-PREP-BUNDLE-01: Queries the persisted sys_arm_state
# singleton row (state, reason) read-only via docker exec psql. This is the
# durable arm/halt state written by persist_arm_state_canonical and survives
# daemon restarts. Returns $null fields on query failure.
# Does NOT call clear-halted-run, arm-execution, or any mutating route.
# ---------------------------------------------------------------------------
function Get-PaperArmStateSummary {
    $result = [pscustomobject]@{
        state    = $null
        reason   = $null
        query_ok = $false
    }
    try {
        $armQ  = "SELECT state, coalesce(reason, '') FROM sys_arm_state WHERE sentinel_id = 1"
        $rawA  = docker exec mqk-paper-postgres psql -U postgres -d miniquantdesk_paper -t -A -q -F'|' -c $armQ 2>$null
        $lineA = ($rawA -join '').Trim()
        if ($lineA -match '\|') {
            $parts = $lineA -split '\|'
            if ($parts.Count -ge 2) {
                $result.state  = $parts[0].Trim()
                $result.reason = $parts[1].Trim()
                if ([string]::IsNullOrWhiteSpace($result.reason)) { $result.reason = $null }
            }
        }
        $result.query_ok = $true
    } catch {
        # query failure -- return with query_ok=false
    }
    return $result
}

# ---------------------------------------------------------------------------
# Helper: Write-EvidenceCapture
# On startup failure, captures daemon log paths and (if daemon is up) dumps
# status endpoints to an evidence folder under exports\smoke\evidence_*.
# Never prints secrets.
# ---------------------------------------------------------------------------
function Write-EvidenceCapture {
    param([string]$Reason)
    Write-Fail "Evidence capture triggered: $Reason"
    $evStamp = Get-Date -Format 'yyyyMMdd_HHmmss'
    $evDir   = Join-Path $RepoRoot "exports\smoke\evidence_$evStamp"
    try { New-Item -ItemType Directory -Force -Path $evDir | Out-Null } catch {}

    if ($null -ne $script:stdoutLog -and (Test-Path $script:stdoutLog)) {
        try { Copy-Item $script:stdoutLog (Join-Path $evDir 'daemon.stdout.log') -ErrorAction SilentlyContinue } catch {}
        Write-Fail "  daemon stdout: $($script:stdoutLog)"
    }
    if ($null -ne $script:stderrLog -and (Test-Path $script:stderrLog)) {
        try { Copy-Item $script:stderrLog (Join-Path $evDir 'daemon.stderr.log') -ErrorAction SilentlyContinue } catch {}
        Write-Fail "  daemon stderr: $($script:stderrLog)"
    }
    # Best-effort status dumps if daemon is reachable
    try {
        $evStatus = Invoke-WebRequest -Uri "$DaemonBaseUrl/api/v1/system/status" -Method GET -TimeoutSec 3 -UseBasicParsing -ErrorAction Stop
        $evStatus.Content | Set-Content (Join-Path $evDir 'system_status.json') -Encoding ASCII -ErrorAction SilentlyContinue
    } catch {}
    try {
        $evReady = Invoke-WebRequest -Uri "$DaemonBaseUrl/api/v1/autonomous/readiness" -Method GET -TimeoutSec 3 -UseBasicParsing -ErrorAction Stop
        $evReady.Content | Set-Content (Join-Path $evDir 'readiness.json') -Encoding ASCII -ErrorAction SilentlyContinue
    } catch {}
    Write-Fail "  evidence folder: $evDir"
}

# ---------------------------------------------------------------------------
# Helper: Start-GuiObserveIfRequested
# GUI-RELAUNCH-DURING-SMOKE-01: STEP 1 stops any stale mqk-gui process before
# the daemon restarts, so GUI observation is unavailable for the rest of the
# run unless something relaunches it. This helper relaunches the desktop GUI
# in observe mode (plain launch, no arm/trade args) once daemon identity has
# been verified (STEP 8).
#
# Binary detection is read-only (same two candidate paths as
# Launch-VeritasLedger.ps1's Resolve-GuiBinary). This helper never builds the
# GUI -- a missing binary is reported as "GUI observation unavailable", not
# an error. Launch failure is caught and reported non-fatally. Respects
# -SkipGui: if set, no relaunch is attempted.
# ---------------------------------------------------------------------------
function Start-GuiObserveIfRequested {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [switch]$SkipGui
    )

    $result = [pscustomobject]@{
        requested    = -not $SkipGui.IsPresent
        skipped      = $SkipGui.IsPresent
        binary_found = $false
        binary_path  = $null
        launched     = $false
        gui_pid      = $null
        message      = $null
    }

    if ($SkipGui.IsPresent) {
        $result.message = 'GUI relaunch skipped (-SkipGui specified). GUI observation unavailable for this run.'
        return $result
    }

    $candidates = @(
        (Join-Path $RepoRoot 'core-rs\target\release\mqk-gui.exe'),
        (Join-Path $RepoRoot 'core-rs\mqk-gui\src-tauri\target\release\mqk-gui.exe')
    )

    $guiExe = $null
    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            $guiExe = (Resolve-Path $candidate).Path
            break
        }
    }

    if ($null -eq $guiExe) {
        $result.message = 'GUI binary not found (checked core-rs\target\release\mqk-gui.exe and core-rs\mqk-gui\src-tauri\target\release\mqk-gui.exe). GUI observation unavailable for this run. Build it first with Launch-VeritasLedger.ps1 or "npm run tauri build" in core-rs\mqk-gui.'
        return $result
    }

    $result.binary_found = $true
    $result.binary_path  = $guiExe

    try {
        $guiProc = Start-Process -FilePath $guiExe -WorkingDirectory (Split-Path -Parent $guiExe) -PassThru -ErrorAction Stop
        $result.launched = $true
        $result.gui_pid  = $guiProc.Id
        $result.message  = "GUI relaunched in observe mode (PID $($guiProc.Id))."
    } catch {
        $result.message = "GUI relaunch failed (non-fatal): $($_.Exception.Message). GUI observation unavailable for this run."
    }

    return $result
}

# ---------------------------------------------------------------------------
# CHECK-ONLY mode: verify prerequisites without starting anything
# ---------------------------------------------------------------------------
if ($CheckOnly) {
    Write-Section "CHECK-ONLY: prerequisites check (no daemon start)"
    $checkPassed = $true

    # Check docker command availability (not whether daemon is running)
    try {
        $null = Get-Command 'docker' -ErrorAction Stop
        Write-Ok "docker command available."
    } catch {
        Write-Fail "docker command not found. Install Docker Desktop and retry."
        $checkPassed = $false
    }

    # Check .env.local exists
    $envLocalCheckPath = Join-Path $RepoRoot '.env.local'
    if (Test-Path $envLocalCheckPath) {
        Write-Ok ".env.local present at $envLocalCheckPath"
    } else {
        Write-Fail ".env.local not found at $envLocalCheckPath"
        Write-Fail "Copy .env.local.example to .env.local and fill in credentials."
        $checkPassed = $false
    }

    # Check daemon binary (informational - warn only, build on first run is expected)
    $daemonBinCheckPath = Join-Path $RepoRoot 'core-rs\target\release\mqk-daemon.exe'
    if (Test-Path $daemonBinCheckPath) {
        Write-Ok "Daemon binary present at core-rs\target\release\mqk-daemon.exe"
    } else {
        Write-Warn "Daemon binary not yet built. It will be built on first full run."
    }

    # Load .env.local before reading strategy env vars so CheckOnly resolves the
    # same timeframe the full startup path would use. Process env overrides win;
    # .env.local fills gaps; fallback default applies if neither is present.
    $coEnvLocalPath = Join-Path $RepoRoot '.env.local'
    if (Test-Path $coEnvLocalPath) {
        foreach ($coLine in Get-Content -Path $coEnvLocalPath) {
            if ([string]::IsNullOrWhiteSpace($coLine)) { continue }
            $coTrimmed = $coLine.Trim()
            if ($coTrimmed.StartsWith('#')) { continue }
            $coIdx = $coTrimmed.IndexOf('=')
            if ($coIdx -lt 1) { continue }
            $coVarName  = $coTrimmed.Substring(0, $coIdx).Trim()
            $coVarValue = $coTrimmed.Substring($coIdx + 1).Trim()
            if (($coVarValue.StartsWith('"') -and $coVarValue.EndsWith('"')) -or
                ($coVarValue.StartsWith("'") -and $coVarValue.EndsWith("'"))) {
                if ($coVarValue.Length -ge 2) { $coVarValue = $coVarValue.Substring(1, $coVarValue.Length - 2) }
            }
            if ([string]::IsNullOrWhiteSpace($coVarName)) { continue }
            $coExisting = [Environment]::GetEnvironmentVariable($coVarName, 'Process')
            if ([string]::IsNullOrWhiteSpace($coExisting)) {
                Set-Item -Path "Env:$coVarName" -Value $coVarValue
            }
        }
    }

    # STEP 5B dry-check: query md_bars bar count without ingesting (read-only)
    $coSymbol    = if ($env:MQK_STRATEGY_SYMBOL)    { $env:MQK_STRATEGY_SYMBOL }    else { 'AAPL' }
    $coTimeframe = if ($env:MQK_STRATEGY_MD_TIMEFRAME) { $env:MQK_STRATEGY_MD_TIMEFRAME } else { '1D' }
    try {
        $null = Get-Command 'docker' -ErrorAction Stop
        $coPgCheck = docker exec mqk-paper-postgres pg_isready -U postgres -d miniquantdesk_paper 2>$null
        if ($LASTEXITCODE -eq 0) {
            $coSummary = Get-PaperMdBarSummary -Symbol $coSymbol -Timeframe $coTimeframe
            if ($coSummary.query_ok) {
                Write-Ok ("STEP 5B dry-check: md_bars completed rows for ${coSymbol}/${coTimeframe} = " + $coSummary.completed_count)
                if ($null -ne $coSummary.min_time) {
                    Write-Ok ("  bar range: min=$($coSummary.min_time)  max=$($coSummary.max_time)")
                }
                if ($coSummary.completed_count -lt 5) {
                    Write-Warn "Fewer than 5 completed bars -- ingest needed before full run."
                }
            } else {
                Write-Warn "STEP 5B dry-check: could not query md_bars (DB may be unavailable in CheckOnly)."
            }
        } else {
            Write-Warn "STEP 5B dry-check: paper Postgres container not ready -- skipping bar count."
        }
    } catch {
        Write-Warn "STEP 5B dry-check: docker not available -- skipping bar count."
    }

    # STEP 5C dry-check: query persisted sys_arm_state (read-only, no clear/arm)
    # PAPER-SMOKE-RETEST-PREP-BUNDLE-01: surfaces the durable arm/halt state
    # left over from the prior session so the operator knows what STEP 10 of
    # the full run will see. This check NEVER calls clear-halted-run or
    # arm-execution -- it only reports the persisted DB row.
    try {
        $null = Get-Command 'docker' -ErrorAction Stop
        $coPgCheck2 = docker exec mqk-paper-postgres pg_isready -U postgres -d miniquantdesk_paper 2>$null
        if ($LASTEXITCODE -eq 0) {
            $coArm = Get-PaperArmStateSummary
            if ($coArm.query_ok -and $null -ne $coArm.state) {
                $coArmReason = if ($null -ne $coArm.reason) { $coArm.reason } else { '(none)' }
                Write-Ok ("STEP 5C dry-check: persisted sys_arm_state = $($coArm.state)  reason=$coArmReason")
                if ($coArm.state -eq 'DISARMED') {
                    Write-Warn "Persisted arm state is DISARMED. This reflects the prior session's halt/disarm reason."
                    Write-Warn "STEP 10 of the full run reads kill_switch_active / arm_state / runtime_status from the FRESH daemon and automatically runs disarm-execution -> clear-halted-run -> arm-execution if a halted state is detected."
                    Write-Warn "No manual clear-halted-run or arm-execution call is required before running the full smoke."
                } else {
                    Write-Ok "Persisted sys_arm_state = ARMED. STEP 10 still re-verifies kill_switch_active / runtime_status on the fresh daemon before proceeding."
                }
            } else {
                Write-Warn "STEP 5C dry-check: could not query sys_arm_state (table may be empty or DB unavailable in CheckOnly)."
            }
        } else {
            Write-Warn "STEP 5C dry-check: paper Postgres container not ready -- skipping arm-state check."
        }
    } catch {
        Write-Warn "STEP 5C dry-check: docker not available -- skipping arm-state check."
    }

    # STEP 9B dry-check: verify the multi-symbol smoke preflight gate code is
    # present in this script (static self-check, no daemon required in
    # CheckOnly). Runtime validation (GET /api/v1/watchlist/status against the
    # MULTI_SYMBOL_SMOKE_BLOCKED_* conditions) happens during the full smoke.
    $selfPath9b = $PSCommandPath
    if ([string]::IsNullOrWhiteSpace($selfPath9b)) { $selfPath9b = $MyInvocation.MyCommand.Path }
    if ($selfPath9b -and (Test-Path $selfPath9b)) {
        $selfText9b = Get-Content -Path $selfPath9b -Raw
        if ($selfText9b -match 'STEP 9B: Multi-symbol smoke preflight gate' -and
            $selfText9b -match 'MULTI_SYMBOL_SMOKE_BLOCKED_SCHEMA_NOT_V2' -and
            $selfText9b -match 'MULTI_SYMBOL_SMOKE_BLOCKED_NOT_MULTI_SYMBOL' -and
            $selfText9b -match 'MULTI_SYMBOL_SMOKE_BLOCKED_NOT_APPROVED_FOR_AUTONOMOUS_PAPER' -and
            $selfText9b -match 'MULTI_SYMBOL_SMOKE_BLOCKED_APPROVED_FOR_LIVE_TRUE') {
            Write-Ok "STEP 9B dry-check: multi-symbol smoke preflight gate code present."
            Write-Ok "  Runtime validation (GET /api/v1/watchlist/status) happens during the full smoke run."
        } else {
            Write-Warn "STEP 9B dry-check: multi-symbol smoke preflight gate code not found in this script."
        }
    } else {
        Write-Warn "STEP 9B dry-check: could not resolve script path for self-check."
    }

    Write-Section "CHECK-ONLY complete"
    if ($checkPassed) {
        Write-Ok "All prerequisite checks passed. Ready for full startup."
        exit 0
    } else {
        Write-Fail "One or more prerequisite checks failed. Resolve above before running full startup."
        exit 1
    }
}

# ---------------------------------------------------------------------------
# STEP 1: Stop stale daemon / GUI / dev processes
# ---------------------------------------------------------------------------
Write-Section "STEP 1: Stop stale processes"

$staleProcesses = @('mqk-daemon', 'mqk-gui', 'cargo-watch')
foreach ($pname in $staleProcesses) {
    $procs = @(Get-Process -Name $pname -ErrorAction SilentlyContinue)
    foreach ($p in $procs) {
        Write-Warn "Stopping stale process: $pname (PID $($p.Id))"
        Stop-Process -Id $p.Id -Force -ErrorAction SilentlyContinue
    }
}

# Wait briefly for ports to release
Start-Sleep -Milliseconds 800

$portInUse = $false
$tcpCheck = New-Object System.Net.Sockets.TcpClient
try {
    $iar = $tcpCheck.BeginConnect('127.0.0.1', $DaemonPort, $null, $null)
    $portInUse = $iar.AsyncWaitHandle.WaitOne(300)
    if ($portInUse) { $tcpCheck.EndConnect($iar) }
} catch { $portInUse = $false } finally { $tcpCheck.Close() }

if ($portInUse) {
    Write-Fail "Port $DaemonPort is still occupied after stopping known processes."
    Write-Fail "Find and stop the process manually: netstat -ano | findstr :$DaemonPort"
    exit 1
}
Write-Ok "Port $DaemonPort is free."

# ---------------------------------------------------------------------------
# STEP 2: Verify Docker is running
# ---------------------------------------------------------------------------
Write-Section "STEP 2: Verify Docker"

try {
    $dockerInfo = docker info 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "Docker is not running or not accessible. Start Docker Desktop and retry."
        exit 1
    }
    Write-Ok "Docker daemon is running."
} catch {
    Write-Fail "docker command failed: $_"
    exit 1
}

# ---------------------------------------------------------------------------
# STEP 3: Verify mqk-paper-postgres container (5440->5432)
# ---------------------------------------------------------------------------
Write-Section "STEP 3: Verify paper Postgres container"

$containerName = 'mqk-paper-postgres'
$containerRunning = $false

try {
    $inspect = docker inspect $containerName 2>&1
    if ($LASTEXITCODE -eq 0) {
        $inspectJson = $inspect | ConvertFrom-Json
        $status = $inspectJson[0].State.Status
        if ($status -eq 'running') {
            $containerRunning = $true
            Write-Ok "Container '$containerName' is running."
        } else {
            Write-Warn "Container '$containerName' exists but status=$status. Starting..."
            docker start $containerName 2>&1 | Out-Null
            if ($LASTEXITCODE -ne 0) {
                Write-Fail "Failed to start container '$containerName'."
                exit 1
            }
            Start-Sleep -Seconds 2
            $containerRunning = $true
        }
    } else {
        Write-Fail "Container '$containerName' not found."
        Write-Fail "Create it with:"
        Write-Fail "  docker run --name mqk-paper-postgres -e POSTGRES_PASSWORD=postgres -e POSTGRES_USER=postgres -e POSTGRES_DB=miniquantdesk_paper -p 5440:5432 -d postgres:16"
        exit 1
    }
} catch {
    Write-Fail "Docker inspect failed: $_"
    exit 1
}

# Verify pg_isready on port 5440
$pgReady = $false
$pgRetries = 0
while (-not $pgReady -and $pgRetries -lt 10) {
    try {
        $pgResult = docker exec $containerName pg_isready -U postgres -d miniquantdesk_paper 2>&1
        if ($LASTEXITCODE -eq 0) {
            $pgReady = $true
        }
    } catch {}
    if (-not $pgReady) {
        $pgRetries++
        Start-Sleep -Seconds 1
    }
}

if (-not $pgReady) {
    Write-Fail "Postgres inside '$containerName' is not ready after $pgRetries retries."
    Write-Fail "Check: docker logs $containerName"
    exit 1
}
Write-Ok "Postgres is ready inside '$containerName' (port 5440)."

# ---------------------------------------------------------------------------
# STEP 4: Load .env.local without printing secrets, then reassert paper env
# ---------------------------------------------------------------------------
Write-Section "STEP 4: Load .env.local (safe) + reassert paper env"

$envLocalPath = Join-Path $RepoRoot '.env.local'
if (-not (Test-Path $envLocalPath)) {
    Write-Fail ".env.local not found at $envLocalPath"
    Write-Fail "Copy .env.local.example to .env.local and fill in your credentials."
    exit 1
}

# Load .env.local lines safely - never print secret values
$loadedVars = @()
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
    # Only set if not already set in process environment
    $existing = [Environment]::GetEnvironmentVariable($varName, 'Process')
    if ([string]::IsNullOrWhiteSpace($existing)) {
        Set-Item -Path "Env:$varName" -Value $varValue
        $loadedVars += $varName
    }
}
Write-Ok "Loaded $($loadedVars.Count) env vars from .env.local (values not printed)."

# STEP 4b: Reassert paper-safe env vars AFTER .env.local load.
# MQK_DATABASE_URL is always forced to the paper DB regardless of what .env.local contains.
$env:MQK_DATABASE_URL              = $PaperDbUrl
$env:MQK_DAEMON_DEPLOYMENT_MODE    = 'paper'
$env:MQK_DAEMON_ADAPTER_ID         = 'alpaca'
$env:MQK_DAEMON_ADDR               = "127.0.0.1:$DaemonPort"
$env:MQK_SESSION_START_HH_MM       = $SessionStart
$env:MQK_SESSION_STOP_HH_MM        = $SessionStop
Write-Ok "MQK_DATABASE_URL reasserted to paper DB (not printed - contains credentials)."
Write-Ok "MQK_DAEMON_DEPLOYMENT_MODE=paper  MQK_DAEMON_ADAPTER_ID=alpaca"
Write-Ok "MQK_DAEMON_ADDR=127.0.0.1:$DaemonPort"
Write-Ok "Session window: $SessionStart - $SessionStop UTC"

# Verify operator token is configured (presence only - never print value)
$operatorToken = [Environment]::GetEnvironmentVariable('MQK_OPERATOR_TOKEN', 'Process')
if ([string]::IsNullOrWhiteSpace($operatorToken)) {
    $operatorToken = [Environment]::GetEnvironmentVariable('MQK_OPERATOR_TOKEN', 'User')
}
if ([string]::IsNullOrWhiteSpace($operatorToken)) {
    Write-Fail "MQK_OPERATOR_TOKEN is not set. Add it to .env.local."
    exit 1
}
Write-Ok "MQK_OPERATOR_TOKEN is configured (value not printed)."

# Guard: never allow live routing
$liveRoutingEnv = [Environment]::GetEnvironmentVariable('MQK_LIVE_ROUTING_ENABLED', 'Process')
if ($liveRoutingEnv -eq 'true' -or $liveRoutingEnv -eq '1') {
    Write-Fail "MQK_LIVE_ROUTING_ENABLED is set to a truthy value in environment."
    Write-Fail "This script is paper-only. Unset MQK_LIVE_ROUTING_ENABLED and retry."
    exit 1
}

# ---------------------------------------------------------------------------
# STEP 5: Run DB migrations
# ---------------------------------------------------------------------------
Write-Section "STEP 5: Run DB migrations"

$migrationsPath = Join-Path $RepoRoot 'core-rs\crates\mqk-db\migrations'

# Prefer sqlx CLI if available; fall back to cargo sqlx
$sqlxCmd = $null
try { $sqlxCmd = (Get-Command 'sqlx' -ErrorAction Stop).Source } catch {}

if ($null -ne $sqlxCmd) {
    Write-Step "Running: sqlx migrate run"
    & $sqlxCmd migrate run --database-url $env:MQK_DATABASE_URL --source $migrationsPath 2>&1 | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "sqlx migrate run failed (exit $LASTEXITCODE). Check DB connectivity."
        exit 1
    }
} else {
    Write-Step "sqlx CLI not found; running via cargo sqlx"
    $cargo = (Get-Command 'cargo' -ErrorAction Stop).Source
    Push-Location (Join-Path $RepoRoot 'core-rs')
    try {
        $local:ErrorActionPreference = 'Continue'
        & $cargo run --quiet --bin sqlx -- migrate run --database-url $env:MQK_DATABASE_URL --source $migrationsPath 2>&1 | Out-Host
        if ($LASTEXITCODE -ne 0) {
            Write-Fail "cargo sqlx migrate run failed (exit $LASTEXITCODE)."
            exit 1
        }
    } finally { Pop-Location }
}
Write-Ok "DB migrations applied."

# ---------------------------------------------------------------------------
# STEP 5B: Market-data context prep -- bars for strategy lookback
# ---------------------------------------------------------------------------
# The intraday_scalper requires LOOKBACK=5 completed bars from md_bars for its
# 5-bar close-displacement signal.  The symbol and timeframe are driven by
# MQK_STRATEGY_SYMBOL (default: AAPL) and MQK_STRATEGY_MD_TIMEFRAME (default: 1D).
#
# Note on timeframe:  intraday_scalper declares TIMEFRAME_SECS=300 (5-minute)
# in its engine spec.  When MQK_STRATEGY_MD_TIMEFRAME=1D the strategy evaluates
# daily close displacement (slower signal, trades infrequently).  When set to
# "5m" it evaluates intraday 5-minute displacement (fires more often intraday).
# Either is valid; the operator chooses which bars to ingest and which timeframe
# to configure.  Without a matching timeframe in md_bars the daemon falls back
# to a stub context and signal_qty=0 on every tick.
#
# This step loads from the backup CSV (no API credit) then tops off via TwelveData
# sync-provider so the strategy has a full lookback window at session open.
#
# Safe: touches md_bars only. No orders/fills/outbox/inbox are created.
# DB guard: MQK_DATABASE_URL must point to paper DB (port 5440).
# ---------------------------------------------------------------------------
Write-Section "STEP 5B: Market-data context prep (delegates to Prep-PremarketMarketData.ps1)"

$mdSymbol    = $env:MQK_STRATEGY_SYMBOL
$mdTimeframe = $env:MQK_STRATEGY_MD_TIMEFRAME
if ([string]::IsNullOrWhiteSpace($mdSymbol))    { $mdSymbol    = "AAPL" }
if ([string]::IsNullOrWhiteSpace($mdTimeframe)) { $mdTimeframe = "1D"   }

Write-Step "Market-data context: symbol=$mdSymbol timeframe=$mdTimeframe"

# Safety guard: refuse if DATABASE_URL does not contain port 5440 (paper DB).
if ($env:MQK_DATABASE_URL -notmatch "5440") {
    Write-Fail "MQK_DATABASE_URL does not appear to be the paper DB (expected port 5440). Aborting market-data prep to prevent live/test DB mutation."
    exit 1
}

$prepScript = Join-Path $RepoRoot 'scripts\windows\Prep-PremarketMarketData.ps1'
if (Test-Path $prepScript) {
    Write-Step "Delegating to Prep-PremarketMarketData.ps1 ..."
    & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $prepScript `
        -Symbols $mdSymbol `
        -Timeframe $mdTimeframe `
        -MinCompletedBars 30 `
        -MaxStalenessDays 4 `
        -PaperDbUrl $PaperDbUrl `
        -RepoRoot $RepoRoot
    if ($LASTEXITCODE -ne 0) {
        Write-EvidenceCapture "STEP 5B: Prep-PremarketMarketData.ps1 failed (exit $LASTEXITCODE)"
        Write-Fail "Market-data prep failed. Resolve above before continuing smoke."
        exit 1
    }
    Write-Ok "Market-data prep passed."
} else {
    Write-Warn "Prep-PremarketMarketData.ps1 not found at $prepScript. Falling back to inline bar count check."
    $mdSummary = Get-PaperMdBarSummary -Symbol $mdSymbol -Timeframe $mdTimeframe
    if ($mdSummary.query_ok) {
        $mdRows = $mdSummary.completed_count
        Write-Ok "md_bars completed rows: $mdRows (${mdSymbol}/${mdTimeframe})"
        if ($null -ne $mdSummary.min_time) {
            Write-Ok "  bar range: min=$($mdSummary.min_time)  max=$($mdSummary.max_time)"
        }
        if ($mdRows -lt 5) {
            Write-EvidenceCapture "STEP 5B fallback: insufficient bars (have $mdRows, need >= 5)"
            Write-Fail "Insufficient bars for strategy lookback (need >= 5, have $mdRows)."
            exit 1
        }
    } else {
        Write-Warn "Could not verify md_bars row count. Proceeding with caution."
    }
}

# ---------------------------------------------------------------------------
# STEP 6: Build daemon from current HEAD (if binary is missing or stale)
# ---------------------------------------------------------------------------
Write-Section "STEP 6: Ensure daemon binary"

$daemonBin = Join-Path $RepoRoot 'core-rs\target\release\mqk-daemon.exe'

$needBuild = $false
if (-not (Test-Path $daemonBin)) {
    Write-Step "Daemon binary not found; building..."
    $needBuild = $true
} else {
    # If HEAD commit is newer than the binary, rebuild
    try {
        $headTime = git -C $RepoRoot log -1 --format='%ct' 2>$null
        $binTime  = [int][double](Get-Item $daemonBin).LastWriteTime.Subtract([datetime]'1970-01-01').TotalSeconds
        if ([int]$headTime -gt $binTime) {
            Write-Step "HEAD commit is newer than daemon binary; rebuilding..."
            $needBuild = $true
        } else {
            Write-Ok "Daemon binary is up-to-date."
        }
    } catch {
        Write-Warn "Could not compare git HEAD time to binary time; skipping stale check."
    }
}

if ($needBuild) {
    $cargo = (Get-Command 'cargo' -ErrorAction Stop).Source
    Push-Location (Join-Path $RepoRoot 'core-rs')
    try {
        # PS7 with ErrorActionPreference=Stop promotes cargo stderr (compilation messages)
        # to NativeCommandError. Use local Continue so the exit-code check governs failure.
        $local:ErrorActionPreference = 'Continue'
        & $cargo build -p mqk-daemon --release 2>&1 | Out-Host
        if ($LASTEXITCODE -ne 0) {
            Write-Fail "cargo build mqk-daemon --release failed."
            exit 1
        }
    } finally { Pop-Location }
    Write-Ok "Daemon built."
}

if (-not (Test-Path $daemonBin)) {
    Write-Fail "Daemon binary still not found at: $daemonBin"
    exit 1
}

# ---------------------------------------------------------------------------
# HTTP helpers (no secret printing, no order-mutating calls)
# ---------------------------------------------------------------------------

function Invoke-DaemonGet {
    param(
        [string]$Path,
        [switch]$AuthRequired
    )
    $url = "$DaemonBaseUrl$Path"
    $params = @{
        Uri             = $url
        Method          = 'GET'
        TimeoutSec      = 5
        UseBasicParsing = $true
        ErrorAction     = 'Stop'
    }
    if ($AuthRequired) {
        $params['Headers'] = @{ Authorization = "Bearer [REDACTED]" }
        # Rebuild with real token but without logging it
        $params['Headers'] = @{ Authorization = "Bearer $operatorToken" }
    }
    $resp = Invoke-WebRequest @params
    return $resp.Content | ConvertFrom-Json
}

function Invoke-DaemonPost {
    param(
        [string]$Path,
        [hashtable]$Body
    )
    $url = "$DaemonBaseUrl$Path"
    $bodyJson = $Body | ConvertTo-Json -Depth 4 -Compress
    $params = @{
        Uri             = $url
        Method          = 'POST'
        ContentType     = 'application/json'
        Body            = $bodyJson
        Headers         = @{ Authorization = "Bearer $operatorToken" }
        TimeoutSec      = 10
        UseBasicParsing = $true
        ErrorAction     = 'Stop'
    }
    try {
        $resp = Invoke-WebRequest @params
        return [pscustomobject]@{ StatusCode = [int]$resp.StatusCode; Body = ($resp.Content | ConvertFrom-Json) }
    } catch {
        $sc = $null
        $rawBody = $null
        if ($null -ne $_.Exception.Response) {
            try { $sc = [int]$_.Exception.Response.StatusCode } catch {}
            try {
                $stream = $_.Exception.Response.GetResponseStream()
                $reader = New-Object System.IO.StreamReader($stream)
                $rawBody = $reader.ReadToEnd()
                $reader.Dispose(); $stream.Dispose()
            } catch {}
        }
        $parsed = $null
        if ($rawBody) { try { $parsed = $rawBody | ConvertFrom-Json } catch {} }
        return [pscustomobject]@{ StatusCode = $sc; Body = $parsed; RawBody = $rawBody; Error = $_.Exception.Message }
    }
}

function Wait-DaemonReachable {
    param([int]$TimeoutSec = 30)
    $deadline = (Get-Date).AddSeconds($TimeoutSec)
    while ((Get-Date) -lt $deadline) {
        try {
            $h = Invoke-DaemonGet -Path '/v1/health'
            if ($h.service -eq 'mqk-daemon') { return $true }
        } catch {}
        Start-Sleep -Milliseconds 500
    }
    return $false
}

# ---------------------------------------------------------------------------
# Helper: Invoke-DeadmanRepair
# DEADMAN-EXPIRY-HALT-ON-RESTART-01
# Called when arm_state=halted or kill_switch_active=true is detected after
# daemon restart. On restart the prior process's deadman timer fires against
# the orphaned run, producing a stale halt. Repair: disarm -> clear-halted-run
# -> arm-execution. Does NOT disable deadman. Does NOT bypass risk/reconcile
# gates. Does NOT enable live routing. Returns $true when arm_state=armed and
# safety invariants hold; $false if repair was incomplete.
# ---------------------------------------------------------------------------
function Invoke-DeadmanRepair {
    param([string]$ContextLabel)
    Write-Warn "[$ContextLabel] Deadman-repair: calling disarm-execution..."
    $r1 = Invoke-DaemonPost -Path '/api/v1/ops/action' -Body @{ action_key = 'disarm-execution' }
    if ($r1.StatusCode -ne 200) {
        Write-Warn "[$ContextLabel] disarm-execution returned HTTP $($r1.StatusCode)"
    } else {
        Write-Ok "[$ContextLabel] disarm-execution accepted."
    }

    Write-Warn "[$ContextLabel] Deadman-repair: calling clear-halted-run..."
    $r2 = Invoke-DaemonPost -Path '/api/v1/ops/action' -Body @{ action_key = 'clear-halted-run' }
    if ($r2.StatusCode -ne 200) {
        Write-Warn "[$ContextLabel] clear-halted-run returned HTTP $($r2.StatusCode) -- repair incomplete"
        return $false
    }
    Write-Ok "[$ContextLabel] clear-halted-run accepted."
    Start-Sleep -Milliseconds 400

    Write-Warn "[$ContextLabel] Deadman-repair: calling arm-execution..."
    $r3 = Invoke-DaemonPost -Path '/api/v1/ops/action' -Body @{ action_key = 'arm-execution' }
    if ($r3.StatusCode -ne 200) {
        Write-Warn "[$ContextLabel] arm-execution returned HTTP $($r3.StatusCode) after repair"
        return $false
    }
    Write-Ok "[$ContextLabel] arm-execution accepted."
    Start-Sleep -Milliseconds 400

    # Verify safety invariants: kill_switch_active=false, live_routing_enabled=false, arm_state=armed.
    $stsR = $null
    $rdR  = $null
    try { $stsR = Invoke-DaemonGet -Path '/api/v1/system/status' }          catch {}
    try { $rdR  = Invoke-DaemonGet -Path '/api/v1/autonomous/readiness' }   catch {}

    $ksR  = ($null -ne $stsR) -and ($null -ne $stsR.PSObject.Properties['kill_switch_active']) -and ($stsR.kill_switch_active -eq $true)
    $lrR  = ($null -ne $stsR) -and ($stsR.live_routing_enabled -eq $true)
    $armR = if ($null -ne $rdR) { $rdR.arm_state } else { 'unknown' }

    if ($ksR) {
        Write-Fail "[$ContextLabel] kill_switch_active=true persists after deadman repair. Manual operator action required."
        return $false
    }
    if ($lrR) {
        Write-Fail "[$ContextLabel] live_routing_enabled=true after repair -- paper-only violation. Halting."
        exit 1
    }
    Write-Ok "[$ContextLabel] Repair verified: arm_state=$armR  kill_switch_active=false  live_routing_enabled=false"
    return ($armR -eq 'armed')
}

# ---------------------------------------------------------------------------
# STEP 7: Start daemon
# ---------------------------------------------------------------------------
Write-Section "STEP 7: Start daemon from current HEAD"

$logDir    = Join-Path $RepoRoot 'exports\smoke'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$stamp     = Get-Date -Format 'yyyyMMdd_HHmmss'
$stdoutLog = Join-Path $logDir "daemon_$stamp.stdout.log"
$stderrLog = Join-Path $logDir "daemon_$stamp.stderr.log"
# Also set script-scope vars so Write-EvidenceCapture (called from any step) can reference them.
$script:stdoutLog = $stdoutLog
$script:stderrLog = $stderrLog
$script:logDir    = $logDir

Write-Step "Starting: $daemonBin"
Write-Step "stdout -> $stdoutLog"
Write-Step "stderr -> $stderrLog"

$daemonProc = Start-Process `
    -FilePath $daemonBin `
    -WorkingDirectory $RepoRoot `
    -RedirectStandardOutput $stdoutLog `
    -RedirectStandardError  $stderrLog `
    -WindowStyle Hidden `
    -PassThru

Write-Step "Daemon PID: $($daemonProc.Id). Waiting for /v1/health..."
$reached = Wait-DaemonReachable -TimeoutSec 30
if (-not $reached) {
    Write-Fail "Daemon did not become reachable within 30 s."
    Write-Fail "stdout: $stdoutLog"
    Write-Fail "stderr: $stderrLog"
    if (-not $daemonProc.HasExited) { Stop-Process -Id $daemonProc.Id -Force -ErrorAction SilentlyContinue }
    exit 1
}
Write-Ok "Daemon is reachable at $DaemonBaseUrl."

# ---------------------------------------------------------------------------
# STEP 8: Verify daemon identity, mode, and live_routing_enabled=false
# ---------------------------------------------------------------------------
Write-Section "STEP 8: Verify daemon identity"

$status = $null
try { $status = Invoke-DaemonGet -Path '/api/v1/system/status' } catch {
    Write-Fail "Failed to GET /api/v1/system/status: $_"
    exit 1
}

if ($status.daemon_mode -ne 'paper') {
    Write-Fail "daemon_mode='$($status.daemon_mode)' - expected 'paper'. Refusing to continue."
    exit 1
}
if ($status.adapter_id -ne 'alpaca') {
    Write-Fail "adapter_id='$($status.adapter_id)' - expected 'alpaca'. Refusing to continue."
    exit 1
}
if ($status.live_routing_enabled -eq $true) {
    Write-Fail "live_routing_enabled=true on daemon. This script is paper-only. Refusing to continue."
    exit 1
}

Write-Ok "daemon_mode=paper  adapter_id=alpaca  live_routing_enabled=false"
Write-Ok "runtime_status=$($status.runtime_status)  db_status=$($status.db_status)"
Write-Ok "alpaca_ws_continuity=$($status.alpaca_ws_continuity)  deadman_status=$($status.deadman_status)"

# ---------------------------------------------------------------------------
# STEP 8B: Relaunch GUI in observe mode (GUI-RELAUNCH-DURING-SMOKE-01)
# STEP 1 stopped any stale mqk-gui process. Relaunch it now that daemon
# identity is verified, so the operator can observe the remainder of this
# run. Non-fatal: a missing binary, launch failure, or -SkipGui is reported
# but never stops the smoke.
# ---------------------------------------------------------------------------
Write-Section "STEP 8B: GUI observation"

$guiStatus = Start-GuiObserveIfRequested -RepoRoot $RepoRoot -SkipGui:$SkipGui
if ($guiStatus.launched) {
    Write-Ok $guiStatus.message
    Write-Step "GUI binary: $($guiStatus.binary_path)"
} else {
    Write-Warn $guiStatus.message
}

# ---------------------------------------------------------------------------
# STEP 8C: Intraday refresh loop (optional, -StartIntradayRefreshLoop)
# PAPER-SMOKE-FOLLOWUP-01D-INTRADAY-REFRESH-LOOP-SMOKE-SUPPORT-01
# Default behavior is unchanged: this script never starts provider refresh
# network activity on its own. Only when -StartIntradayRefreshLoop is
# explicitly passed does it launch the existing
# scripts\windows\Refresh-IntradayMarketData.ps1 as a separate process, in
# its own interval-loop mode, so intraday bars stay fresh for the rest of
# this smoke session instead of aging out ~15 minutes after STEP 5B's
# one-shot top-off (this is the root cause DATA-FRESHNESS-READINESS-GATE-01
# correctly caught during the market-hours proof sweep). Does not touch
# oms_outbox/oms_inbox/broker_order_map/runs/arm_state (same guarantee
# Refresh-IntradayMarketData.ps1 documents for itself). Never edits
# .env.local, never persists secrets, never stages evidence -- the child
# process writes its own untracked exports\market_data\intraday_refresh_*.json
# evidence exactly as it does when run standalone.
# ---------------------------------------------------------------------------
Write-Section "STEP 8C: Intraday refresh loop (optional, -StartIntradayRefreshLoop)"

if ($StartIntradayRefreshLoop) {
    $intradayRefreshScript = Join-Path $RepoRoot 'scripts\windows\Refresh-IntradayMarketData.ps1'
    if (-not (Test-Path $intradayRefreshScript)) {
        Write-Warn "Refresh-IntradayMarketData.ps1 not found at $intradayRefreshScript. Cannot start refresh loop (non-fatal)."
    } else {
        $intradaySymbol = $env:MQK_STRATEGY_SYMBOL
        if ([string]::IsNullOrWhiteSpace($intradaySymbol)) { $intradaySymbol = 'AAPL' }
        $intradayTimeframe = $env:MQK_STRATEGY_MD_TIMEFRAME
        if ([string]::IsNullOrWhiteSpace($intradayTimeframe) -or $intradayTimeframe -eq '1D') { $intradayTimeframe = '5m' }

        $intradayArgs = @(
            '-NoProfile', '-ExecutionPolicy', 'Bypass', '-NonInteractive',
            '-File', $intradayRefreshScript,
            '-Symbols', $intradaySymbol,
            '-Timeframe', $intradayTimeframe,
            '-IntervalSeconds', $IntradayRefreshIntervalSeconds,
            '-DurationSeconds', $IntradayRefreshDurationSeconds,
            '-PaperDbUrl', $PaperDbUrl
        )
        Write-Step "Starting intraday refresh loop as a separate process:"
        Write-Step "  powershell.exe $($intradayArgs -join ' ')"
        try {
            $intradayRefreshProc = Start-Process -FilePath 'powershell.exe' -ArgumentList $intradayArgs `
                -WorkingDirectory $RepoRoot -WindowStyle Hidden -PassThru
            Write-Ok "Intraday refresh loop started (PID $($intradayRefreshProc.Id)); symbol=$intradaySymbol timeframe=$intradayTimeframe interval=${IntradayRefreshIntervalSeconds}s duration=${IntradayRefreshDurationSeconds}s."
        } catch {
            Write-Warn "Failed to start intraday refresh loop (non-fatal): $_"
        }
    }
} else {
    Write-Step "Intraday refresh loop not requested (-StartIntradayRefreshLoop not set)."
    Write-Step "  STEP 5B's one-shot market-data prep does not stay fresh for a full market-hours session."
    Write-Step "  To keep intraday bars fresh yourself, run in a separate terminal:"
    Write-Step "  powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 -IntervalSeconds 300 -DurationSeconds 1800"
    Write-Step "  Note: since -StartIntradayRefreshLoop was not passed either, STEP 8D below will start the required-universe scheduler by default."
}

# ---------------------------------------------------------------------------
# STEP 8D: Required-universe scheduler (optional, -StartRequiredUniverseScheduler)
# MARKET-DATA-AUTOFRESH-REQUIRED-UNIVERSE-01
#
# Data-only: calls the daemon's required-universe market-data controller,
# which derives every symbol/timeframe the currently configured autonomous
# Paper operation requires (the same canonical resolver STEP 5B/STEP 9B
# already use) and keeps it fresh through the rest of this session. Never
# starts/stops the execution runtime, never arms/disarms, never clears
# halt/kill-switch/reconcile, submits zero orders -- this step only calls
# POST /api/v1/market-data/required-universe/start on the already-running
# daemon (STEP 7/STEP 8 verified it above). Default: off, same explicit
# opt-in posture -StartIntradayRefreshLoop already established -- this
# script never starts provider refresh network activity on its own.
# ---------------------------------------------------------------------------
Write-Section "STEP 8D: Required-universe scheduler (default-on; see -SkipRequiredUniverseScheduler / -StartIntradayRefreshLoop / -CheckOnly)"

# The -StartRequiredUniverseScheduler / -StartIntradayRefreshLoop conflict is
# validated immediately after argument parsing, before any side effect
# (including STEP 8C above) -- see the early-validation block near the top
# of this script. $UseRequiredUniverseScheduler is computed there.
if ($UseRequiredUniverseScheduler) {
    try {
        $ruStartBody = @{ dry_run = [bool]$RequiredUniverseSchedulerDryRun }
        $ruStart = Invoke-DaemonPost -Path '/api/v1/market-data/required-universe/start' -Body $ruStartBody
        if ($ruStart.StatusCode -eq 200) {
            $ruReport = $ruStart.Body.report
            Write-Ok ("Required-universe scheduler started: truth_state=$($ruStart.Body.truth_state) " +
                      "overall_state=$($ruReport.overall_state) requirements=$($ruReport.requirements.Count) " +
                      "groups=$($ruReport.groups.Count) dry_run=$([bool]$RequiredUniverseSchedulerDryRun)")
            foreach ($ruReq in @($ruReport.requirements)) {
                if (@($ruReq.blockers).Count -gt 0) {
                    Write-Warn "Required-universe requirement $($ruReq.symbol)/$($ruReq.timeframe): freshness_state=$($ruReq.freshness_state) blockers=$($ruReq.blockers -join '; ')"
                }
            }
        } elseif ($ruStart.StatusCode -eq 409) {
            Write-Warn "Required-universe scheduler was already running (truth_state=$($ruStart.Body.truth_state)); continuing."
        } else {
            Write-Warn "Required-universe scheduler start returned HTTP $($ruStart.StatusCode) (non-fatal): $($ruStart.Body.error)"
        }
    } catch {
        Write-Warn "Failed to start required-universe scheduler (non-fatal): $_"
    }
} else {
    if ($CheckOnly) {
        Write-Step "Required-universe scheduler not started (-CheckOnly: no daemon/runtime/scheduler side effects)."
    } elseif ($StartIntradayRefreshLoop) {
        Write-Step "Required-universe scheduler not started (-StartIntradayRefreshLoop requested the legacy interval-loop path instead)."
    } else {
        Write-Step "Required-universe scheduler not started (-SkipRequiredUniverseScheduler was passed)."
    }
    Write-Step "  Read-only plan/status without starting anything: GET /api/v1/market-data/required-universe/plan (or /status)."
}

# ---------------------------------------------------------------------------
# STEP 9: Verify Alpaca WS live (wait for continuity=live)
# ---------------------------------------------------------------------------
Write-Section "STEP 9: Verify Alpaca WS continuity"

$wsDeadline = (Get-Date).AddSeconds(45)
$wsContinuity = $null
while ((Get-Date) -lt $wsDeadline) {
    try {
        $s2 = Invoke-DaemonGet -Path '/api/v1/system/status'
        $wsContinuity = $s2.alpaca_ws_continuity
        if ($wsContinuity -eq 'live') { break }
        if ($wsContinuity -eq 'gap_detected') {
            Write-Fail "Alpaca WS continuity=gap_detected. Operator action required before proceeding."
            Write-Fail "See docs/runbooks/common_failure_modes.md for WS gap recovery."
            exit 1
        }
    } catch {}
    Start-Sleep -Milliseconds 2000
}

if ($wsContinuity -eq 'live') {
    Write-Ok "Alpaca WS continuity=live."
} else {
    Write-Warn "Alpaca WS continuity='$wsContinuity' (not live after 45 s). Check Alpaca credentials."
    Write-Warn "Continuing - WS may still be establishing. Verify at /api/v1/system/status."
}

# ---------------------------------------------------------------------------
# STEP 9B: Multi-symbol smoke preflight gate (watchlist-v2)
# MULTI-SYMBOL-SMOKE-RUNNER-PREFLIGHT-GATE-01
# PAPER-SMOKE-FOLLOWUP-01C-WATCHLIST-V2-GATE-SINGLE-SYMBOL-FIX-01: this gate
# enforces the *multi-symbol smoke runner* preconditions from
# docs/design/native_multi_symbol_dispatch.md Section 9.1, which states
# explicitly: "a single-symbol artifact should use the existing single-symbol
# smoke path unchanged." Read-only. GET /api/v1/watchlist/status.
#   - If -MultiSymbolSmoke is passed, the full watchlist-v2 preflight is
#     enforced (schema_version=watchlist-v2, >1 symbols, approved for paper)
#     before any mutating operator action (STEP 10+) runs -- unchanged from
#     the original gate behavior.
#   - If -MultiSymbolSmoke is not passed (default), an absent watchlist-v2
#     artifact (status=not_configured) is the expected, valid state for this
#     repo's canonical single-symbol smoke path and does not block the run.
#     A watchlist-v2 artifact that IS configured but invalid/unapproved still
#     fails closed in either mode -- that is a real misconfiguration, not an
#     absent-artifact single-symbol setup.
#   - approved_for_live=true is a hard invariant violation in both modes.
# Fails closed with a stable MULTI_SYMBOL_SMOKE_BLOCKED_* code on any unmet
# condition.
# ---------------------------------------------------------------------------
Write-Section "STEP 9B: Multi-symbol smoke preflight gate (watchlist-v2)"

$watchlistStatus = $null
try {
    $watchlistStatus = Invoke-DaemonGet -Path '/api/v1/watchlist/status'
} catch {
    Write-Fail "MULTI_SYMBOL_SMOKE_BLOCKED_WATCHLIST_STATUS_UNAVAILABLE: GET /api/v1/watchlist/status failed: $_"
    Write-EvidenceCapture "STEP 9B: watchlist status unavailable -- $_"
    exit 1
}

if ($null -eq $watchlistStatus) {
    Write-Fail "MULTI_SYMBOL_SMOKE_BLOCKED_WATCHLIST_STATUS_UNAVAILABLE: watchlist status response was null/empty."
    Write-EvidenceCapture "STEP 9B: watchlist status response null"
    exit 1
}

$wlStatusLabel   = if ($null -ne $watchlistStatus.PSObject.Properties['status']) { $watchlistStatus.status } else { $null }
$wlSchemaVersion = if ($null -ne $watchlistStatus.PSObject.Properties['schema_version']) { $watchlistStatus.schema_version } else { $null }
$wlSymbols       = if ($null -ne $watchlistStatus.PSObject.Properties['symbols']) { $watchlistStatus.symbols } else { @() }
$wlSymbolCount   = @($wlSymbols).Count
$wlApprovedPaper = ($null -ne $watchlistStatus.PSObject.Properties['approved_for_autonomous_paper']) -and ($watchlistStatus.approved_for_autonomous_paper -eq $true)
$wlApprovedLive  = ($null -ne $watchlistStatus.PSObject.Properties['approved_for_live']) -and ($watchlistStatus.approved_for_live -eq $true)

Write-Step "watchlist/status: status=$wlStatusLabel  schema_version=$wlSchemaVersion  symbols_count=$wlSymbolCount  approved_for_autonomous_paper=$wlApprovedPaper  approved_for_live=$wlApprovedLive"

# Hard invariant in both modes: approved_for_live must never be true.
if ($wlApprovedLive) {
    Write-Fail "MULTI_SYMBOL_SMOKE_BLOCKED_APPROVED_FOR_LIVE_TRUE: approved_for_live=$wlApprovedLive (hard invariant violation)."
    Write-EvidenceCapture "STEP 9B: approved_for_live=true"
    exit 1
}

$smokeSymbol = $env:MQK_STRATEGY_SYMBOL
if ([string]::IsNullOrWhiteSpace($smokeSymbol)) { $smokeSymbol = 'AAPL' }

if ($MultiSymbolSmoke) {
    Write-Step "Multi-symbol smoke explicitly requested (-MultiSymbolSmoke). Enforcing full watchlist-v2 preflight."

    if ($wlSchemaVersion -ne 'watchlist-v2') {
        Write-Fail "MULTI_SYMBOL_SMOKE_BLOCKED_SCHEMA_NOT_V2: schema_version='$wlSchemaVersion' (expected 'watchlist-v2')."
        Write-EvidenceCapture "STEP 9B: schema_version != watchlist-v2 (got '$wlSchemaVersion')"
        exit 1
    }

    if ($wlSymbolCount -le 1) {
        Write-Fail "MULTI_SYMBOL_SMOKE_BLOCKED_NOT_MULTI_SYMBOL: symbols count=$wlSymbolCount (need > 1)."
        Write-EvidenceCapture "STEP 9B: symbols count <= 1 (got $wlSymbolCount)"
        exit 1
    }

    if (-not $wlApprovedPaper) {
        Write-Fail "MULTI_SYMBOL_SMOKE_BLOCKED_NOT_APPROVED_FOR_AUTONOMOUS_PAPER: approved_for_autonomous_paper=$wlApprovedPaper."
        Write-EvidenceCapture "STEP 9B: approved_for_autonomous_paper=false"
        exit 1
    }

    Write-Ok "Multi-symbol smoke preflight gate passed: schema_version=watchlist-v2  symbols_count=$wlSymbolCount  approved_for_autonomous_paper=true  approved_for_live=false"
} elseif ($wlStatusLabel -eq 'not_configured') {
    Write-Ok "watchlist_v2_status=not_configured_single_symbol_mode  watchlist_v2_required=false  single_symbol_smoke_symbol=$smokeSymbol"
    Write-Ok "No watchlist-v2 artifact configured -- this repo's single-symbol smoke path (MQK_STRATEGY_SYMBOL=$smokeSymbol) does not require one. Pass -MultiSymbolSmoke to require and enforce a watchlist-v2 artifact instead."
} elseif ($wlSchemaVersion -eq 'watchlist-v2' -and $wlSymbolCount -gt 1 -and $wlApprovedPaper) {
    Write-Ok "watchlist_v2_status=configured_and_valid  watchlist_v2_required=false  single_symbol_smoke_symbol=$smokeSymbol"
    Write-Ok "A valid multi-symbol watchlist-v2 artifact is configured even though -MultiSymbolSmoke was not passed; proceeding with the single-symbol smoke path (MQK_STRATEGY_SYMBOL=$smokeSymbol) unchanged."
} else {
    Write-Fail "MULTI_SYMBOL_SMOKE_BLOCKED_WATCHLIST_V2_CONFIGURED_BUT_INVALID: a watchlist-v2 artifact is configured (status=$wlStatusLabel) but is not valid/approved, and -MultiSymbolSmoke was not requested to bypass it."
    Write-Fail "  This indicates a real watchlist-v2 misconfiguration, not an absent-artifact single-symbol setup. Fix the artifact or unset MQK_PAPER_WATCHLIST_PATH to run the single-symbol smoke path."
    Write-EvidenceCapture "STEP 9B: watchlist-v2 configured but invalid/not-approved in single-symbol mode (status=$wlStatusLabel)"
    exit 1
}

# ---------------------------------------------------------------------------
# STEP 10: Clear halted lifecycle if needed (via operator route only)
# ---------------------------------------------------------------------------
Write-Section "STEP 10: Clear halted run if present"

try {
    $readiness     = Invoke-DaemonGet -Path '/api/v1/autonomous/readiness'
    $armState      = $readiness.arm_state
    $sysStatus     = Invoke-DaemonGet -Path '/api/v1/system/status'
    $runtimeStatus = $sysStatus.runtime_status

    # kill_switch_active persists across daemon restart when a run ended in integrity halt.
    # After restart the new daemon may report arm_state=disarmed + runtime_status=idle
    # while kill_switch_active=true is still set from the prior halted run -- check it explicitly.
    $killSwitchActive = ($null -ne $sysStatus.PSObject.Properties['kill_switch_active']) -and ($sysStatus.kill_switch_active -eq $true)

    # Log fresh daemon state explicitly so the operator can see what DB truth was loaded.
    Write-Step "Fresh daemon state (loaded from DB truth): arm_state=$armState  runtime_status=$runtimeStatus  kill_switch_active=$killSwitchActive"

    # Clear if ANY halted indicator is present: arm_state halted, runtime halted, or kill_switch active.
    $needClear = ($armState -eq 'halted') -or ($runtimeStatus -eq 'halted') -or $killSwitchActive

    if ($needClear) {
        Write-Warn "Halted/kill-switch state detected on fresh daemon (arm_state=$armState runtime_status=$runtimeStatus kill_switch_active=$killSwitchActive)."
        Write-Warn "Clearing via disarm-execution then clear-halted-run."

        # Invoke-DaemonPost returns {StatusCode, Body} on success and {StatusCode, Body, RawBody, Error}
        # on error. With Set-StrictMode -Version Latest, accessing .Error on a success object throws.
        # Check StatusCode only.
        $disarm = Invoke-DaemonPost -Path '/api/v1/ops/action' -Body @{ action_key = 'disarm-execution' }
        if ($disarm.StatusCode -ne 200) {
            Write-Warn "disarm-execution returned HTTP $($disarm.StatusCode): $($disarm.RawBody)"
        } else {
            Write-Ok "disarm-execution accepted."
        }

        $clear = Invoke-DaemonPost -Path '/api/v1/ops/action' -Body @{ action_key = 'clear-halted-run' }
        if ($clear.StatusCode -ne 200) {
            Write-Fail "clear-halted-run failed (HTTP $($clear.StatusCode)): $($clear.RawBody)"
            Write-EvidenceCapture "STEP 10: clear-halted-run failed HTTP $($clear.StatusCode)"
            exit 1
        }
        Write-Ok "clear-halted-run accepted."

        # Verify kill_switch_active=false after clearing. If it persists, the run cannot start.
        Start-Sleep -Milliseconds 400
        $stsAfterClear = Invoke-DaemonGet -Path '/api/v1/system/status'
        $ksAfterClear  = ($null -ne $stsAfterClear.PSObject.Properties['kill_switch_active']) -and ($stsAfterClear.kill_switch_active -eq $true)
        if ($ksAfterClear) {
            Write-Fail "kill_switch_active=true persists after clear-halted-run. Manual operator action required before smoke can proceed."
            Write-EvidenceCapture "STEP 10: kill_switch_active still true after clear-halted-run"
            exit 1
        }
        Write-Ok "kill_switch_active=false confirmed after halt clear."
    } else {
        Write-Ok "arm_state=$armState  runtime_status=$runtimeStatus  kill_switch_active=$killSwitchActive  -- no halted run to clear."
    }
} catch {
    Write-Warn "Could not read autonomous readiness for halt check: $_"
}

# ---------------------------------------------------------------------------
# STEP 11: Adopt broker baseline
# ---------------------------------------------------------------------------
Write-Section "STEP 11: Adopt broker position baseline"

$adoptResp = Invoke-DaemonPost `
    -Path '/api/v1/ops/repair/adopt-broker-position-baseline' `
    -Body @{ confirmation = 'ADOPT_BROKER_POSITION_BASELINE' }

if ($adoptResp.StatusCode -eq 200) {
    Write-Ok "Broker position baseline adopted: accepted=$($adoptResp.Body.accepted)  positions=$($adoptResp.Body.baseline_position_count)  orders=$($adoptResp.Body.baseline_order_count)  reconcile_after=$($adoptResp.Body.reconcile_status_after)"
} elseif ($adoptResp.StatusCode -eq 409) {
    # 409 means a baseline was already adopted for this run snapshot.
    # Prior fills may have dirtied reconcile since that adoption; verify reconcile before proceeding.
    Write-Warn "Baseline adoption returned 409 (already adopted for current snapshot): $($adoptResp.RawBody)"
    Write-Warn "Verifying reconcile is clean after prior adoption..."
    $adoptCheckRec = $null
    try { $adoptCheckRec = Invoke-DaemonGet -Path '/api/v1/reconcile/status' } catch {}
    if ($null -ne $adoptCheckRec -and $adoptCheckRec.status -eq 'dirty') {
        Write-Fail "Reconcile is dirty after 409 baseline (prior fills changed positions). Manual operator action required."
        Write-Fail "Review: GET /api/v1/reconcile/mismatches  then re-run this script to force fresh adoption."
        Write-EvidenceCapture "STEP 11: reconcile dirty after 409 baseline adoption"
        exit 1
    }
    Write-Ok "Reconcile clean after 409 baseline (idempotent path confirmed)."
} else {
    Write-Fail "adopt-broker-position-baseline failed (HTTP $($adoptResp.StatusCode))."
    if ($adoptResp.RawBody) { Write-Fail "Response: $($adoptResp.RawBody)" }
    Write-EvidenceCapture "STEP 11: baseline adoption failed HTTP $($adoptResp.StatusCode)"
    exit 1
}

# ---------------------------------------------------------------------------
# STEP 12: Verify reconcile status -- HARD GATE (PAPER-SMOKE-STARTUP-RECONCILE-HARD-GATE-01)
#
# This is the final pre-start gate. Reconcile must be provably clean
# (status=ok, truth_state=active) before the smoke run is allowed to start.
# Fail closed -- exit 1 -- on dirty, never-run/unknown, stale, unavailable,
# or any unrecognized state. Do not optimistically continue and do not rely
# solely on later preflight/paper-status checks to catch this.
# ---------------------------------------------------------------------------
Write-Section "STEP 12: Verify reconcile status (hard gate)"

$reconcile = $null
try { $reconcile = Invoke-DaemonGet -Path '/api/v1/reconcile/status' } catch {
    Write-Fail "Could not GET /api/v1/reconcile/status: $_"
    Write-Fail "Reconcile truth is unavailable. Refusing to start (fail-closed)."
    Write-EvidenceCapture "STEP 12: reconcile status unavailable -- $_"
    exit 1
}

if ($null -eq $reconcile) {
    Write-Fail "Reconcile status response was empty/null. Refusing to start (fail-closed)."
    Write-EvidenceCapture "STEP 12: reconcile status response null"
    exit 1
}

Write-Step "reconcile truth_state=$($reconcile.truth_state)  status=$($reconcile.status)"

if ($reconcile.status -eq 'ok' -and $reconcile.truth_state -eq 'active') {
    Write-Ok "Reconcile is clean (status=ok, truth_state=active). Safe to proceed."
} elseif ($reconcile.status -eq 'dirty') {
    Write-Fail "Reconcile is DIRTY. Review mismatches: GET /api/v1/reconcile/mismatches"
    Write-Fail "Manual operator action required before smoke can proceed. Refusing to start (fail-closed)."
    Write-EvidenceCapture "STEP 12: reconcile dirty -- refusing to start"
    exit 1
} elseif ($reconcile.truth_state -eq 'never_run' -or $reconcile.status -eq 'unknown') {
    Write-Fail "Reconcile has never completed a tick (truth_state=never_run, status=unknown)."
    Write-Fail "Reconcile truth is not yet established. Refusing to start (fail-closed)."
    Write-EvidenceCapture "STEP 12: reconcile truth_state=never_run -- refusing to start"
    exit 1
} elseif ($reconcile.truth_state -eq 'stale' -or $reconcile.status -eq 'stale') {
    Write-Fail "Reconcile truth is stale (truth_state=$($reconcile.truth_state), status=$($reconcile.status))."
    Write-Fail "Reconcile truth is not current. Refusing to start (fail-closed)."
    Write-EvidenceCapture "STEP 12: reconcile stale -- refusing to start"
    exit 1
} else {
    Write-Fail "Unrecognized reconcile state (truth_state=$($reconcile.truth_state), status=$($reconcile.status))."
    Write-Fail "Cannot prove reconcile is clean. Refusing to start (fail-closed)."
    Write-EvidenceCapture "STEP 12: reconcile state unrecognized -- refusing to start"
    exit 1
}

# ---------------------------------------------------------------------------
# STEP 13: Verify durable arm state - arm if needed
# ---------------------------------------------------------------------------
Write-Section "STEP 13: Verify / set arm state"

$readiness2 = $null
try { $readiness2 = Invoke-DaemonGet -Path '/api/v1/autonomous/readiness' } catch {
    Write-Fail "Could not GET /api/v1/autonomous/readiness: $_"
    exit 1
}

$armState2 = $readiness2.arm_state
Write-Step "arm_state=$armState2  reconcile_ready=$($readiness2.reconcile_ready)"

if ($armState2 -eq 'armed') {
    Write-Ok "Execution is already ARMED."
} elseif ($armState2 -eq 'disarmed' -or $armState2 -eq 'stopped') {
    Write-Step "arm_state=$armState2. Calling arm-execution via POST /api/v1/ops/action..."
    $armResp = Invoke-DaemonPost -Path '/api/v1/ops/action' -Body @{ action_key = 'arm-execution' }
    if ($armResp.StatusCode -eq 200 -and $armResp.Body.accepted -eq $true) {
        Write-Ok "arm-execution accepted. disposition=$($armResp.Body.disposition)"
    } else {
        Write-Fail "arm-execution failed (HTTP $($armResp.StatusCode))."
        if ($armResp.RawBody) { Write-Fail "Response: $($armResp.RawBody)" }
        Write-EvidenceCapture "STEP 13: arm-execution failed HTTP $($armResp.StatusCode)"
        exit 1
    }
} elseif ($armState2 -eq 'halted') {
    # halted at this point means STEP 10 did not clear it -- fail closed rather than warn.
    Write-Fail "arm_state=halted persists after STEP 10 clear attempt. Cannot arm for runtime."
    Write-EvidenceCapture "STEP 13: arm_state=halted after STEP 10 clear"
    exit 1
} else {
    Write-Warn "Unexpected arm_state='$armState2'. Check /api/v1/autonomous/readiness manually."
}

# Verify arm state is now ARMED, kill_switch_active=false, and live_routing_enabled=false.
Start-Sleep -Milliseconds 500
$readiness3 = $null
try { $readiness3 = Invoke-DaemonGet -Path '/api/v1/autonomous/readiness' } catch {}
$stsPostArm = $null
try { $stsPostArm = Invoke-DaemonGet -Path '/api/v1/system/status' } catch {}

if ($null -ne $readiness3) {
    if ($readiness3.arm_state -eq 'armed') {
        Write-Ok "Durable arm state confirmed: arm_state=ARMED."
    } else {
        Write-Warn "arm_state=$($readiness3.arm_state) after arm attempt. Verify manually."
    }
}

if ($null -ne $stsPostArm) {
    $ksPostArm = ($null -ne $stsPostArm.PSObject.Properties['kill_switch_active']) -and ($stsPostArm.kill_switch_active -eq $true)
    $lrPostArm = $stsPostArm.live_routing_enabled -eq $true

    if ($ksPostArm) {
        Write-Fail "kill_switch_active=true after arm-execution. Runtime cannot start while kill_switch is set."
        Write-EvidenceCapture "STEP 13: kill_switch_active=true after arm"
        exit 1
    }
    if ($lrPostArm) {
        Write-Fail "live_routing_enabled=true after arm-execution. This script is paper-only. Refusing to continue."
        exit 1
    }
    Write-Ok "Post-arm verification: kill_switch_active=false  live_routing_enabled=false"
}

# ---------------------------------------------------------------------------
# STEP 14: Verify readiness / preflight
# ---------------------------------------------------------------------------
Write-Section "STEP 14: Verify readiness and preflight"

$preflight = $null
try { $preflight = Invoke-DaemonGet -Path '/api/v1/system/preflight' } catch {
    Write-Warn "Could not GET /api/v1/system/preflight: $_"
}

$readiness4 = $null
try { $readiness4 = Invoke-DaemonGet -Path '/api/v1/autonomous/readiness' } catch {}

if ($null -ne $preflight) {
    Write-Ok "preflight: deployment_start_allowed=$($preflight.deployment_start_allowed)  broker_config_present=$($preflight.broker_config_present)  market_data_config_present=$($preflight.market_data_config_present)"
    $preflightBlockers = if ($null -ne $preflight.PSObject.Properties['blockers']) { @($preflight.blockers) } else { @() }
    if ($preflightBlockers.Count -gt 0) {
        Write-Warn "Preflight blockers: $($preflightBlockers -join '; ')"
    }
    # Hard-gate: if deployment_start_allowed=false, refuse to proceed to start-system.
    # Off-hours session_not_in_window is expected; operator may use -NoStartRuntime.
    if ($preflight.deployment_start_allowed -eq $false) {
        Write-Fail "deployment_start_allowed=false -- startup gated. Blockers above must be resolved."
        Write-Fail "If the only blocker is session timing, use -NoStartRuntime to prep without starting."
        Write-EvidenceCapture "STEP 14: deployment_start_allowed=false"
        exit 1
    }
}

if ($null -ne $readiness4) {
    Write-Ok "readiness: truth_state=$($readiness4.truth_state)  overall_ready=$($readiness4.overall_ready)  canonical_path=$($readiness4.canonical_path)"
    Write-Ok "  ws=$($readiness4.ws_continuity)  reconcile=$($readiness4.reconcile_status)  arm=$($readiness4.arm_state)  session=$($readiness4.session_window_state)"
}

# ---------------------------------------------------------------------------
# STEP 14b: Autonomous paper status pre-run triage
# PAPER-SMOKE-AUTOMATION-BUNDLE-01
# Read-only. Blocks on readiness_classification=blocked. Warns on others.
# Endpoint: GET /api/v1/autonomous/paper-status (public, no auth required).
# ---------------------------------------------------------------------------
Write-Section "STEP 14b: Autonomous paper status pre-run triage"

$paperStatus14b = $null
try { $paperStatus14b = Invoke-DaemonGet -Path '/api/v1/autonomous/paper-status' } catch {}

if ($null -ne $paperStatus14b) {
    $ps14b_class    = if ($null -ne $paperStatus14b.PSObject.Properties['readiness_classification']) { $paperStatus14b.readiness_classification } else { 'unknown' }
    $ps14b_next     = if ($null -ne $paperStatus14b.PSObject.Properties['next_operator_action'])     { $paperStatus14b.next_operator_action }     else { 'unknown' }
    $ps14b_live_rt  = if ($null -ne $paperStatus14b.PSObject.Properties['live_routing_enabled'])     { $paperStatus14b.live_routing_enabled }     else { 'unknown' }
    $ps14b_recon    = if ($null -ne $paperStatus14b.PSObject.Properties['reconcile_status'])         { $paperStatus14b.reconcile_status }         else { 'unknown' }
    $ps14b_ws       = if ($null -ne $paperStatus14b.PSObject.Properties['ws_continuity'])            { $paperStatus14b.ws_continuity }            else { 'unknown' }
    $ps14b_flat_avl = if ($null -ne $paperStatus14b.PSObject.Properties['flatten_available'])        { $paperStatus14b.flatten_available }        else { 'unknown' }
    $ps14b_cur_qty  = if ($null -ne $paperStatus14b.PSObject.Properties['current_position_qty'])     { $paperStatus14b.current_position_qty }     else { 'null' }
    $ps14b_tgt_qty  = if ($null -ne $paperStatus14b.PSObject.Properties['target_qty'])               { $paperStatus14b.target_qty }               else { 'null' }
    $ps14b_delta    = if ($null -ne $paperStatus14b.PSObject.Properties['computed_delta_qty'])       { $paperStatus14b.computed_delta_qty }       else { 'null' }
    $ps14b_no_ord   = if ($null -ne $paperStatus14b.PSObject.Properties['no_order_reason'])          { $paperStatus14b.no_order_reason }          else { 'null' }

    Write-Step "readiness_classification : $ps14b_class"
    Write-Step "next_operator_action     : $ps14b_next"
    Write-Step "live_routing_enabled     : $ps14b_live_rt"
    Write-Step "reconcile_status         : $ps14b_recon"
    Write-Step "ws_continuity            : $ps14b_ws"
    Write-Step "flatten_available        : $ps14b_flat_avl"
    Write-Step "current_position_qty     : $ps14b_cur_qty"
    Write-Step "target_qty               : $ps14b_tgt_qty"
    Write-Step "computed_delta_qty       : $ps14b_delta"
    Write-Step "no_order_reason          : $ps14b_no_ord"

    if ($ps14b_class -eq 'blocked') {
        $ps14b_blockers = if ($null -ne $paperStatus14b.PSObject.Properties['flatten_blockers']) { @($paperStatus14b.flatten_blockers) } else { @() }
        Write-Fail "autonomous paper-status readiness_classification=blocked. Cannot proceed to runtime start."
        foreach ($b in $ps14b_blockers) { Write-Fail "  blocker: $b" }
        Write-Fail "Resolve blockers then retry. next_operator_action: $ps14b_next"
        Write-EvidenceCapture "STEP 14b: readiness_classification=blocked"
        exit 1
    } elseif ($ps14b_class -eq 'market_proof_pending') {
        Write-Ok "readiness_classification=market_proof_pending -- proceeding (market proof cycle pending)."
    } elseif ($ps14b_class -eq 'ready_for_market_smoke') {
        Write-Ok "readiness_classification=ready_for_market_smoke -- proceeding."
    } else {
        Write-Warn "readiness_classification=$ps14b_class -- continuing with caution."
    }
} else {
    Write-Warn "autonomous/paper-status endpoint unavailable (older daemon build or daemon not yet ready). Continuing with existing readiness checks."
}

# ---------------------------------------------------------------------------
# STEP 14C: Intraday refresh readiness (optional, -RequireIntradayRefresh)
# PAPER-SMOKE-FOLLOWUP-01D-INTRADAY-REFRESH-LOOP-SMOKE-SUPPORT-01
# Read-only. Does NOT weaken DATA-FRESHNESS-READINESS-GATE-01 -- that gate
# still evaluates every strategy-dispatch tick independently of this check.
# This step only adds an earlier, more actionable preflight: when
# -RequireIntradayRefresh is passed, refuse to proceed to STEP 15 unless
# GET /api/v1/market-data/intraday-refresh/status proves fresh, verified
# evidence exists. Default (-RequireIntradayRefresh not set): unchanged
# observe-only behavior, no gate added here.
# ---------------------------------------------------------------------------
Write-Section "STEP 14C: Intraday refresh readiness (optional, -RequireIntradayRefresh)"

if ($RequireIntradayRefresh) {
    $intradayStatus = $null
    try {
        $intradayStatus = Invoke-DaemonGet -Path '/api/v1/market-data/intraday-refresh/status'
    } catch {
        Write-Fail "INTRADAY_REFRESH_BLOCKED_STATUS_UNAVAILABLE: GET /api/v1/market-data/intraday-refresh/status failed: $_"
        Write-EvidenceCapture "STEP 14C: intraday refresh status unavailable -- $_"
        exit 1
    }

    if ($null -eq $intradayStatus) {
        Write-Fail "INTRADAY_REFRESH_BLOCKED_STATUS_UNAVAILABLE: intraday-refresh status response was null/empty."
        Write-EvidenceCapture "STEP 14C: intraday refresh status response null"
        exit 1
    }

    $irTruthState = $intradayStatus.truth_state
    $irAllPassed  = $intradayStatus.all_passed
    $irStale      = $intradayStatus.stale_or_missing_evidence
    Write-Step "intraday-refresh/status: truth_state=$irTruthState  all_passed=$irAllPassed  stale_or_missing_evidence=$irStale"

    if ($irTruthState -ne 'active') {
        Write-Fail "INTRADAY_REFRESH_BLOCKED_TRUTH_STATE: truth_state='$irTruthState' (expected 'active'). No verified refresh evidence found."
        Write-Fail "  Resolve: powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 -Once"
        Write-Fail "  Or re-run this script with -StartIntradayRefreshLoop."
        Write-EvidenceCapture "STEP 14C: truth_state != active (got '$irTruthState')"
        exit 1
    }

    if ($irStale -eq $true) {
        Write-Fail "INTRADAY_REFRESH_BLOCKED_STALE_EVIDENCE: stale_or_missing_evidence=true. Refresh evidence is too old to trust."
        Write-Fail "  Resolve: powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 -Once"
        Write-Fail "  Or re-run this script with -StartIntradayRefreshLoop."
        Write-EvidenceCapture "STEP 14C: stale_or_missing_evidence=true"
        exit 1
    }

    if ($irAllPassed -ne $true) {
        Write-Fail "INTRADAY_REFRESH_BLOCKED_NOT_ALL_PASSED: all_passed=$irAllPassed. One or more symbols failed the fail-closed freshness gate on the last refresh."
        $irSymbols = if ($null -ne $intradayStatus.PSObject.Properties['symbols']) { @($intradayStatus.symbols) } else { @() }
        foreach ($irSym in $irSymbols) {
            if ($irSym.passed -ne $true) {
                Write-Fail "  symbol=$($irSym.symbol) passed=$($irSym.passed) reason_code=$($irSym.reason_code)"
            }
        }
        Write-Fail "  Resolve: powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 -Once"
        Write-Fail "  Or re-run this script with -StartIntradayRefreshLoop."
        Write-EvidenceCapture "STEP 14C: all_passed != true"
        exit 1
    }

    # -------------------------------------------------------------------------
    # INTRADAY-PROVIDER-CLOCK-SKEW-01D: freshness-headroom guard.
    # Script-preflight-only -- never changes DATA-FRESHNESS-READINESS-GATE-01
    # or any daemon dispatch-tick behavior. Evidence can report all_passed=true
    # while still having only seconds of headroom before the freshness cap;
    # this is exactly the condition that let the 2026-07-10 market-hours proof
    # pass STEP 14C and then fail 33 seconds into the run. Prefers the route's
    # own proof_window_ready/proof_window_risk fields when present; falls back
    # to a manual per-symbol freshness_headroom_secs comparison against
    # -MinFreshnessHeadroomSeconds when an older daemon build lacks those
    # fields, so this guard degrades gracefully instead of silently no-op'ing.
    # -------------------------------------------------------------------------
    $irSymbolsForHeadroom = if ($null -ne $intradayStatus.PSObject.Properties['symbols']) { @($intradayStatus.symbols) } else { @() }
    $hasProofWindowReadyField = $null -ne $intradayStatus.PSObject.Properties['proof_window_ready']
    $headroomFailures = @()

    function Format-HeadroomSymbolDetail {
        param($Sym)
        $latestTs  = if ($null -ne $Sym.PSObject.Properties['latest_completed_bar_ts']) { $Sym.latest_completed_bar_ts } else { $null }
        $age       = if ($null -ne $Sym.PSObject.Properties['latest_completed_bar_age_secs']) { $Sym.latest_completed_bar_age_secs } else { $null }
        $maxAge    = if ($null -ne $Sym.PSObject.Properties['max_allowed_age_secs']) { $Sym.max_allowed_age_secs } else { $null }
        $headroom  = if ($null -ne $Sym.PSObject.Properties['freshness_headroom_secs']) { $Sym.freshness_headroom_secs } else { $null }
        $nearExp   = if ($null -ne $Sym.PSObject.Properties['near_expiry']) { $Sym.near_expiry } else { $null }
        $risk      = if ($null -ne $Sym.PSObject.Properties['proof_window_risk']) { $Sym.proof_window_risk } else { 'unknown' }
        "symbol=$($Sym.symbol) latest_completed_bar_ts=$latestTs latest_completed_bar_age_secs=$age max_allowed_age_secs=$maxAge freshness_headroom_secs=$headroom near_expiry=$nearExp proof_window_risk=$risk"
    }

    if ($hasProofWindowReadyField) {
        if ($intradayStatus.proof_window_ready -ne $true) {
            foreach ($irSym in $irSymbolsForHeadroom) {
                $headroomFailures += (Format-HeadroomSymbolDetail -Sym $irSym)
            }
            Write-Fail "INTRADAY_REFRESH_BLOCKED_PROOF_WINDOW_NOT_READY: proof_window_ready=$($intradayStatus.proof_window_ready) proof_window_risk=$($intradayStatus.proof_window_risk). Evidence currently passes but is too close to its freshness cap (or has an evidence gap) to safely start a proof run."
            foreach ($f in $headroomFailures) { Write-Fail "  $f" }
            Write-Fail "  Recommended action: wait for a fresher provider bar, then re-run: powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 -Once"
            Write-Fail "  Then re-check: GET /api/v1/market-data/intraday-refresh/status (look for proof_window_ready=true)."
            Write-Fail "  Or re-run this script with -StartIntradayRefreshLoop."
            Write-EvidenceCapture "STEP 14C: proof_window_ready != true"
            exit 1
        }
        Write-Ok "proof_window_ready=true  proof_window_risk=$($intradayStatus.proof_window_risk)"
    } else {
        Write-Step "proof_window_ready field not present on this daemon build -- falling back to a per-symbol freshness_headroom_secs >= ${MinFreshnessHeadroomSeconds}s check."
        foreach ($irSym in $irSymbolsForHeadroom) {
            $hasHeadroomField = $null -ne $irSym.PSObject.Properties['freshness_headroom_secs']
            if ($hasHeadroomField -and $null -ne $irSym.freshness_headroom_secs -and $irSym.freshness_headroom_secs -lt $MinFreshnessHeadroomSeconds) {
                $headroomFailures += (Format-HeadroomSymbolDetail -Sym $irSym)
            }
        }
        if ($headroomFailures.Count -gt 0) {
            Write-Fail "INTRADAY_REFRESH_BLOCKED_INSUFFICIENT_HEADROOM: one or more symbols have less than ${MinFreshnessHeadroomSeconds}s of freshness headroom remaining (-MinFreshnessHeadroomSeconds=$MinFreshnessHeadroomSeconds)."
            foreach ($f in $headroomFailures) { Write-Fail "  $f" }
            Write-Fail "  Recommended action: wait for a fresher provider bar, then re-run: powershell -ExecutionPolicy Bypass -File scripts\windows\Refresh-IntradayMarketData.ps1 -Once"
            Write-Fail "  Then re-check: GET /api/v1/market-data/intraday-refresh/status."
            Write-Fail "  Or re-run this script with -StartIntradayRefreshLoop."
            Write-EvidenceCapture "STEP 14C: insufficient freshness headroom (fallback path, no proof_window_ready field)"
            exit 1
        }
    }

    Write-Ok "Intraday refresh readiness confirmed: truth_state=active  all_passed=true  stale_or_missing_evidence=false  headroom>=${MinFreshnessHeadroomSeconds}s"
} else {
    Write-Step "Intraday refresh verification not requested (-RequireIntradayRefresh not set)."
    Write-Step "  DATA-FRESHNESS-READINESS-GATE-01 still evaluates freshness on every strategy-dispatch tick regardless of this flag."
}

# ---------------------------------------------------------------------------
# STEP 15: Start runtime (unless -NoStartRuntime)
# ---------------------------------------------------------------------------
Write-Section "STEP 15: Start runtime"

if ($NoStartRuntime) {
    Write-Warn "-NoStartRuntime set. Skipping runtime start wait."
    Write-Warn "Autonomous session controller will start the run when in-window + armed + WS live."
} else {
    # Do NOT call start-system operator action. The autonomous session controller owns the start.
    # Calling start-system races with the session controller: start-system blocks the HTTP handler
    # for ~30s during the initial REST recovery tick; within that window the session_controller
    # sees no local ownership and also calls start_execution_runtime -> durable_active_without_local_owner.
    # The deadman then halts the orphaned run, setting integrity.halted=true and blocking re-arm.
    #
    # Correct flow: let the session controller fire (every 30s) and start autonomously.
    # Poll until runtime_status=running (timeout ~90s to cover 30s session_controller delay
    # + ~35s initial REST recovery tick).
    Write-Step "Waiting for autonomous session controller to start runtime (up to 90s)..."
    $runtimeStarted = $false
    $local:ErrorActionPreference = 'Continue'
    # DEADMAN-EXPIRY-HALT-ON-RESTART-01: Track repair attempts in this phase (max 1).
    $dmRepairAttempts15 = 0
    $startPollDeadline = (Get-Date).AddSeconds(90)
    while ((Get-Date) -lt $startPollDeadline) {
        $stsCheck = $null
        try { $stsCheck = Invoke-DaemonGet -Path '/api/v1/system/status' } catch {}
        if ($null -ne $stsCheck -and $stsCheck.runtime_status -eq 'running') {
            Write-Ok "runtime_status=running -- autonomous session controller started the run."
            $runtimeStarted = $true
            break
        }

        # DEADMAN-EXPIRY-HALT-ON-RESTART-01: Detect stale-deadman halt during autonomous start wait.
        # The orphaned run from the prior daemon process may get deadman-halted 10-15 minutes after
        # restart; when that happens arm_state flips to halted mid-poll. Repair once, fail closed.
        $rdCheck15 = $null
        try { $rdCheck15 = Invoke-DaemonGet -Path '/api/v1/autonomous/readiness' } catch {}
        $armNow15 = if ($null -ne $rdCheck15) { $rdCheck15.arm_state } else { $null }
        $dmNow15  = if ($null -ne $stsCheck)  { $stsCheck.deadman_status }         else { 'unknown' }
        $ksNow15  = ($null -ne $stsCheck) -and ($null -ne $stsCheck.PSObject.Properties['kill_switch_active']) -and ($stsCheck.kill_switch_active -eq $true)

        if ($dmRepairAttempts15 -lt 1 -and ($armNow15 -eq 'halted' -or $ksNow15)) {
            $dmRepairAttempts15++
            Write-Warn ("STEP 15: deadman/halt detected (arm_state=$armNow15 deadman_status=$dmNow15 " +
                        "kill_switch_active=$ksNow15). Running repair (attempt $dmRepairAttempts15 of 1).")
            $repaired15 = Invoke-DeadmanRepair -ContextLabel 'STEP-15'
            if (-not $repaired15) {
                Write-Warn "STEP 15: deadman repair did not confirm armed. Proceeding to watcher for diagnosis."
                break
            }
            Write-Ok "STEP 15: deadman repair succeeded. Continuing to wait for runtime start."
            continue
        }

        $rtNow = if ($null -ne $stsCheck) { $stsCheck.runtime_status } else { 'unreachable' }
        Write-Step "  ...runtime_status=$rtNow arm=$armNow15 deadman=$dmNow15 (polling again in 5s)"
        Start-Sleep -Seconds 5
    }
    if (-not $runtimeStarted) {
        $rdCheck = $null
        try { $rdCheck = Invoke-DaemonGet -Path '/api/v1/autonomous/readiness' } catch {}
        $blocker = if ($null -ne $rdCheck) { $rdCheck.blockers | ConvertTo-Json -Compress } else { 'unavailable' }
        Write-Warn "runtime_status not 'running' after 90s. Proceeding to watcher for diagnosis."
        Write-Warn "autonomous/readiness blockers: $blocker"
    }
}

# ---------------------------------------------------------------------------
# STEP 16: Watcher loop
# ---------------------------------------------------------------------------
Write-Section "STEP 16: Watcher (${WatchSeconds}s, every 15s)"
Write-Host "Fields: runtime | ws | db | reconcile | deadman | arm | session | bars_loaded | signal_qty | alerts | orders (active/pending) | live_routing"
Write-Host "Press Ctrl-C to stop the watcher early."
Write-Host ""

$watchStart  = Get-Date
$watchEnd    = $watchStart.AddSeconds($WatchSeconds)
$tickCounter = 0
# DEADMAN-EXPIRY-HALT-ON-RESTART-01: bounded repair in watcher (max 1 attempt).
$dmRepairAttempts16 = 0

while ((Get-Date) -lt $watchEnd) {
    $tickCounter++
    $elapsed = [int]((Get-Date) - $watchStart).TotalSeconds

    # Collect watcher fields
    $w_runtime        = '?'
    $w_ws             = '?'
    $w_db             = '?'
    $w_reconcile      = '?'
    $w_deadman        = '?'
    $w_arm            = '?'
    $w_session        = '?'
    $w_bars_loaded    = '?'
    $w_bar_ctx        = '?'
    $w_signal_qty     = '?'
    $w_alerts         = '?'
    $w_orders_active  = '?'
    $w_orders_pending = '?'
    $w_live_routing   = '?'

    try {
        $ws = Invoke-DaemonGet -Path '/api/v1/system/status'
        $w_runtime       = $ws.runtime_status
        $w_ws            = $ws.alpaca_ws_continuity
        $w_db            = $ws.db_status
        $w_deadman       = $ws.deadman_status
        $w_live_routing  = if ($ws.live_routing_enabled -eq $true) { 'TRUE-DANGER' } else { 'false' }
    } catch {}

    try {
        $wr = Invoke-DaemonGet -Path '/api/v1/autonomous/readiness'
        $w_reconcile   = $wr.reconcile_status
        $w_arm         = $wr.arm_state
        $w_session     = $wr.session_window_state
        $w_bar_ctx     = if ($null -ne $wr.PSObject.Properties['bar_context_bars_loaded'] -and $null -ne $wr.bar_context_bars_loaded) { $wr.bar_context_bars_loaded } else { 'null' }
        $w_bars_loaded = $w_bar_ctx
        $w_signal_qty  = if ($null -ne $wr.PSObject.Properties['last_bar_signal_qty'] -and $null -ne $wr.last_bar_signal_qty) { $wr.last_bar_signal_qty } else { 'null' }
    } catch {}

    try {
        $wa = Invoke-DaemonGet -Path '/api/v1/alerts/active'
        $w_alerts = if ($null -ne $wa.PSObject.Properties['rows']) { @($wa.rows).Count } else { '?' }
    } catch {}

    try {
        $wo = Invoke-DaemonGet -Path '/api/v1/execution/orders'
        $allOrders = if ($null -ne $wo.PSObject.Properties['orders']) { @($wo.orders) } else { @() }
        $w_orders_active  = @($allOrders | Where-Object { $_.status -eq 'active'  }).Count
        $w_orders_pending = @($allOrders | Where-Object { $_.status -eq 'pending' }).Count
    } catch {}

    # Alert if live routing somehow got enabled
    if ($w_live_routing -eq 'TRUE-DANGER') {
        Write-Fail "LIVE ROUTING IS ENABLED. This is a paper-only session. Halt immediately."
    }

    # DEADMAN-EXPIRY-HALT-ON-RESTART-01: Detect stale-deadman halt in watcher and repair.
    # The orphaned run from the prior daemon process may be halted by deadman 10-15 minutes after
    # restart; when detected here, run one repair cycle: disarm -> clear-halted-run -> arm-execution.
    # Bounded to 1 attempt; fails closed if repair does not confirm armed.
    if ($dmRepairAttempts16 -lt 1 -and $w_arm -eq 'halted') {
        $dmRepairAttempts16++
        Write-Warn ("STEP 16: arm_state=halted detected (deadman=$w_deadman). " +
                    "Running deadman repair (attempt $dmRepairAttempts16 of 1).")
        $null = Invoke-DeadmanRepair -ContextLabel 'STEP-16'
    }

    $line = ("[+{0,4}s] rt={1,-10} ws={2,-20} db={3,-12} rec={4,-10} dm={5,-10} arm={6,-8} sess={7,-15} bars={8,-6} sig={9,-5} alerts={10,-3} ord_act={11} ord_pend={12} live_routing={13}" -f
        $elapsed,
        $w_runtime, $w_ws, $w_db, $w_reconcile, $w_deadman,
        $w_arm, $w_session, $w_bars_loaded, $w_signal_qty,
        $w_alerts, $w_orders_active, $w_orders_pending, $w_live_routing)

    Write-Host $line

    $remaining = [int]($watchEnd - (Get-Date)).TotalSeconds
    if ($remaining -le 0) { break }
    $sleepSec = [Math]::Min(15, $remaining)
    Start-Sleep -Seconds $sleepSec
}

# ---------------------------------------------------------------------------
# Final summary
# ---------------------------------------------------------------------------
Write-Section "Startup complete"

Write-Host ""
Write-Host "Daemon logs:"
Write-Host "  stdout: $stdoutLog"
Write-Host "  stderr: $stderrLog"
Write-Host ""
Write-Host "Key endpoints (requires Bearer token - not printed here):"
Write-Host "  GET  $DaemonBaseUrl/api/v1/system/status"
Write-Host "  GET  $DaemonBaseUrl/api/v1/autonomous/readiness"
Write-Host "  GET  $DaemonBaseUrl/api/v1/system/preflight"
Write-Host "  GET  $DaemonBaseUrl/api/v1/alerts/active"
Write-Host "  GET  $DaemonBaseUrl/api/v1/reconcile/status"
Write-Host "  GET  $DaemonBaseUrl/api/v1/execution/orders"
Write-Host ""
Write-Host "To stop cleanly:"
Write-Host "  POST $DaemonBaseUrl/api/v1/ops/action  body: {action_key: 'stop-system'}  (Bearer token required)"
Write-Host "  POST $DaemonBaseUrl/api/v1/ops/action  body: {action_key: 'disarm-execution'}"
Write-Host "  Then kill mqk-daemon process if desired."
Write-Host ""
Write-Host "live_routing_enabled remains false. No order submission was performed by this script."
Write-Host ""
Write-Host "GUI observation:"
if ($guiStatus.launched) {
    Write-Host "  Relaunched in observe mode (PID $($guiStatus.gui_pid))."
    Write-Host "  Binary: $($guiStatus.binary_path)"
} elseif ($guiStatus.skipped) {
    Write-Host "  Skipped (-SkipGui specified). GUI observation unavailable for this run."
} else {
    Write-Host "  Unavailable: $($guiStatus.message)"
}
Write-Host ""
