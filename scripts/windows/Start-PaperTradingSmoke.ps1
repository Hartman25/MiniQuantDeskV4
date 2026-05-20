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
    [switch]$NoStartRuntime
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
    'MQK_OPERATOR_TOKEN',   'MQK_DISCORD_WEBHOOK_URL',
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

# Load .env.local lines safely — never print secret values
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
Write-Ok "MQK_DATABASE_URL reasserted to paper DB (not printed — contains credentials)."
Write-Ok "MQK_DAEMON_DEPLOYMENT_MODE=paper  MQK_DAEMON_ADAPTER_ID=alpaca"
Write-Ok "MQK_DAEMON_ADDR=127.0.0.1:$DaemonPort"
Write-Ok "Session window: $SessionStart - $SessionStop UTC"

# Verify operator token is configured (presence only — never print value)
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
        & $cargo run --quiet --bin sqlx -- migrate run --database-url $env:MQK_DATABASE_URL --source $migrationsPath 2>&1 | Out-Host
        if ($LASTEXITCODE -ne 0) {
            Write-Fail "cargo sqlx migrate run failed (exit $LASTEXITCODE)."
            exit 1
        }
    } finally { Pop-Location }
}
Write-Ok "DB migrations applied."

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
# STEP 7: Start daemon
# ---------------------------------------------------------------------------
Write-Section "STEP 7: Start daemon from current HEAD"

$logDir = Join-Path $RepoRoot 'exports\smoke'
New-Item -ItemType Directory -Force -Path $logDir | Out-Null
$stamp      = Get-Date -Format 'yyyyMMdd_HHmmss'
$stdoutLog  = Join-Path $logDir "daemon_$stamp.stdout.log"
$stderrLog  = Join-Path $logDir "daemon_$stamp.stderr.log"

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
    Write-Fail "daemon_mode='$($status.daemon_mode)' — expected 'paper'. Refusing to continue."
    exit 1
}
if ($status.adapter_id -ne 'alpaca') {
    Write-Fail "adapter_id='$($status.adapter_id)' — expected 'alpaca'. Refusing to continue."
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
    Write-Warn "Continuing — WS may still be establishing. Verify at /api/v1/system/status."
}

# ---------------------------------------------------------------------------
# STEP 10: Clear halted lifecycle if needed (via operator route only)
# ---------------------------------------------------------------------------
Write-Section "STEP 10: Clear halted run if present"

try {
    $readiness = Invoke-DaemonGet -Path '/api/v1/autonomous/readiness'
    $armState  = $readiness.arm_state

    if ($armState -eq 'halted') {
        Write-Warn "arm_state=halted. Clearing via disarm-execution then clear-halted-run."

        $disarm = Invoke-DaemonPost -Path '/api/v1/ops/action' -Body @{ action_key = 'disarm-execution' }
        if ($null -ne $disarm.Error -and $disarm.StatusCode -ne 200) {
            Write-Warn "disarm-execution returned HTTP $($disarm.StatusCode): $($disarm.RawBody)"
        } else {
            Write-Ok "disarm-execution accepted."
        }

        $clear = Invoke-DaemonPost -Path '/api/v1/ops/action' -Body @{ action_key = 'clear-halted-run' }
        if ($null -ne $clear.Error -and $clear.StatusCode -ne 200) {
            Write-Fail "clear-halted-run failed (HTTP $($clear.StatusCode)): $($clear.RawBody)"
            exit 1
        }
        Write-Ok "clear-halted-run accepted."
    } else {
        Write-Ok "arm_state=$armState — no halted run to clear."
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
    -Body @{ confirm = 'ADOPT_BROKER_POSITION_BASELINE' }

if ($adoptResp.StatusCode -eq 200) {
    Write-Ok "Broker position baseline adopted: adopted=$($adoptResp.Body.adopted)  baseline_id=$($adoptResp.Body.baseline_id)"
} elseif ($adoptResp.StatusCode -eq 409) {
    Write-Ok "Broker position baseline already adopted (409 conflict — idempotent path): $($adoptResp.RawBody)"
} else {
    Write-Fail "adopt-broker-position-baseline failed (HTTP $($adoptResp.StatusCode))."
    if ($adoptResp.RawBody) { Write-Fail "Response: $($adoptResp.RawBody)" }
    exit 1
}

# ---------------------------------------------------------------------------
# STEP 12: Verify reconcile ok
# ---------------------------------------------------------------------------
Write-Section "STEP 12: Verify reconcile status"

$reconcile = $null
try { $reconcile = Invoke-DaemonGet -Path '/api/v1/reconcile/status' } catch {
    Write-Warn "Could not GET /api/v1/reconcile/status: $_"
}

if ($null -ne $reconcile) {
    Write-Ok "reconcile truth_state=$($reconcile.truth_state)  status=$($reconcile.status)"
    if ($reconcile.status -eq 'dirty') {
        Write-Warn "Reconcile is dirty. Review mismatches: GET /api/v1/reconcile/mismatches"
        Write-Warn "Continuing (operator must decide whether to proceed)."
    }
} else {
    Write-Warn "Reconcile status unavailable — continuing."
}

# ---------------------------------------------------------------------------
# STEP 13: Verify durable arm state — arm if needed
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
    Write-Step "arm_state=$armState2. Calling arm-execution..."
    $armResp = Invoke-DaemonPost -Path '/api/v1/ops/action' -Body @{ action_key = 'arm-execution' }
    if ($armResp.StatusCode -eq 200 -and $armResp.Body.accepted -eq $true) {
        Write-Ok "arm-execution accepted. disposition=$($armResp.Body.disposition)"
    } else {
        Write-Fail "arm-execution failed (HTTP $($armResp.StatusCode))."
        if ($armResp.RawBody) { Write-Fail "Response: $($armResp.RawBody)" }
        exit 1
    }
} else {
    Write-Warn "Unexpected arm_state='$armState2'. Check /api/v1/autonomous/readiness manually."
}

# Verify arm state is now ARMED
Start-Sleep -Milliseconds 500
$readiness3 = $null
try { $readiness3 = Invoke-DaemonGet -Path '/api/v1/autonomous/readiness' } catch {}

if ($null -ne $readiness3) {
    if ($readiness3.arm_state -eq 'armed') {
        Write-Ok "Durable arm state confirmed: ARMED."
    } else {
        Write-Warn "arm_state=$($readiness3.arm_state) after arm attempt. Verify manually."
    }
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
}

if ($null -ne $readiness4) {
    Write-Ok "readiness: truth_state=$($readiness4.truth_state)  overall_ready=$($readiness4.overall_ready)  canonical_path=$($readiness4.canonical_path)"
    Write-Ok "  ws=$($readiness4.ws_continuity)  reconcile=$($readiness4.reconcile_status)  arm=$($readiness4.arm_state)  session=$($readiness4.session_window_state)"
}

# ---------------------------------------------------------------------------
# STEP 15: Start runtime (unless -NoStartRuntime)
# ---------------------------------------------------------------------------
Write-Section "STEP 15: Start runtime"

if ($NoStartRuntime) {
    Write-Warn "-NoStartRuntime set. Skipping start-system call."
    Write-Warn "When ready: POST /api/v1/ops/action {action_key: 'start-system'} with Bearer token."
} else {
    $startResp = Invoke-DaemonPost -Path '/api/v1/ops/action' -Body @{ action_key = 'start-system' }
    if ($startResp.StatusCode -eq 200 -and $startResp.Body.accepted -eq $true) {
        Write-Ok "start-system accepted. disposition=$($startResp.Body.disposition)"
    } elseif ($startResp.StatusCode -eq 503) {
        Write-Warn "start-system returned 503 — may be waiting for session window or WS continuity."
        if ($startResp.RawBody) { Write-Warn "Response: $($startResp.RawBody)" }
    } else {
        Write-Warn "start-system returned HTTP $($startResp.StatusCode)."
        if ($startResp.RawBody) { Write-Warn "Response: $($startResp.RawBody)" }
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
Write-Host "Key endpoints (requires Bearer token — not printed here):"
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
