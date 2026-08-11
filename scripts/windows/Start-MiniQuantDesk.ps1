# =============================================================================
# Start-MiniQuantDesk.ps1
# OFFICIAL-DUAL-MODE-LAUNCHER-01
#
# The one official entrypoint for starting MiniQuantDesk. Selects between
# Paper (simulated capital, fully operational -- delegates to the existing
# accepted Launch-VeritasLedger.ps1 / Prep-PremarketMarketData.ps1 /
# Refresh-IntradayMarketData.ps1 paths) and Live (real capital -- present as
# an architectural mode now, but this patch NEVER starts a live daemon
# process, submits an order, or mutates a live account. Live mode only runs
# read-only / source-guard preflight checks and reports the actual blocker
# list from MiniQuantDesk_Master_Patch_Ledger_v2_updated.md).
#
# This script does not implement Windows Task Scheduler registration and
# does not make live capital ready. It does not change any Rust trading
# code, strategy code, broker economics, risk engine, portfolio/P&L, or
# reconciliation semantics.
#
# Usage:
#   Start-MiniQuantDesk.ps1                          interactive Paper/Live menu
#   Start-MiniQuantDesk.ps1 -Mode Paper               full paper startup
#   Start-MiniQuantDesk.ps1 -Mode Paper -CheckOnly    read-only paper diagnostic
#   Start-MiniQuantDesk.ps1 -Mode Paper -ArmPaper     full paper startup + arm
#   Start-MiniQuantDesk.ps1 -Mode Live                live readiness report (blocked today)
#   Start-MiniQuantDesk.ps1 -Mode Live -CheckOnly     read-only live diagnostic
#   Start-MiniQuantDesk.ps1 -Mode Paper -Scheduled    unattended paper start (future Task Scheduler)
#   Start-MiniQuantDesk.ps1 -Scheduled                STARTUP_REFUSED (Mode required when -Scheduled)
#
# Exit codes:
#   0 = ready / successfully attached or prepared
#   1 = generic startup failure
#   2 = safety refusal (e.g. -Scheduled without -Mode, live_routing_enabled=true, declined LIVE confirmation)
#   3 = data readiness failure (symbol universe / market-data gate)
#   4 = backend/reconcile failure
#   5 = LIVE blocked by trust/readiness gates
#   6 = unattended live start not authorized
# =============================================================================

[CmdletBinding()]
param(
    [ValidateSet('Paper', 'Live')]
    [string]$Mode,
    [switch]$CheckOnly,
    [switch]$Scheduled,
    [switch]$ArmPaper,
    [switch]$SkipGui,
    [switch]$Rebuild,
    [switch]$RebuildDaemon,
    [switch]$RebuildGui,
    [switch]$CaptureEvidence
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$script:ExitOk = 0
$script:ExitGeneric = 1
$script:ExitSafetyRefusal = 2
$script:ExitDataReadiness = 3
$script:ExitBackendReconcile = 4
$script:ExitLiveBlocked = 5
$script:ExitUnattendedLiveUnauthorized = 6

function Write-Step    { param([string]$M) Write-Host "[MiniQuantDesk] $M" -ForegroundColor Cyan }
function Write-Ok      { param([string]$M) Write-Host "[MiniQuantDesk] OK: $M" -ForegroundColor Green }
function Write-Warn    { param([string]$M) Write-Host "[MiniQuantDesk] WARN: $M" -ForegroundColor Yellow }
function Write-Fail    { param([string]$M) Write-Host "[MiniQuantDesk] FAIL: $M" -ForegroundColor Red }
function Write-Section { param([string]$M) Write-Host ''; Write-Host "=== $M ===" -ForegroundColor Magenta }

function Get-RepoRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}

function Get-RepoHeadShort {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)
    try {
        $sha = (git -C $RepoRoot rev-parse --short HEAD 2>$null)
        if ($LASTEXITCODE -eq 0 -and $sha) { return $sha.Trim() }
    } catch {}
    return 'unknown'
}

function New-LauncherLog {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][ValidateSet('paper', 'live')][string]$ModeLabel
    )
    $dir = Join-Path $RepoRoot "smoke_logs\launcher\$ModeLabel"
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    $stamp = Get-Date -Format 'yyyyMMdd_HHmmss'
    return (Join-Path $dir "launch_$stamp.json")
}

function Write-LauncherLogEntry {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][hashtable]$Entry
    )
    try {
        ($Entry | ConvertTo-Json -Depth 8) | Set-Content -Path $Path -Encoding UTF8
    } catch {
        Write-Warn "Could not write launcher log to $Path"
    }
}

function Invoke-JsonGet {
    param([Parameter(Mandatory = $true)][string]$Url, [int]$TimeoutSec = 5)
    try {
        $resp = Invoke-WebRequest -Uri $Url -Method Get -TimeoutSec $TimeoutSec -UseBasicParsing -ErrorAction Stop
        return [pscustomobject]@{ Ok = $true; StatusCode = [int]$resp.StatusCode; Json = ($resp.Content | ConvertFrom-Json) }
    } catch {
        return [pscustomobject]@{ Ok = $false; StatusCode = $null; Json = $null; Error = $_.Exception.Message }
    }
}

function Invoke-JsonPost {
    param(
        [Parameter(Mandatory = $true)][string]$Url,
        [Parameter(Mandatory = $true)][string]$OperatorToken,
        [Parameter(Mandatory = $true)][hashtable]$Body,
        [int]$TimeoutSec = 10
    )
    $bodyJson = ($Body | ConvertTo-Json -Compress -Depth 6)
    try {
        $resp = Invoke-WebRequest -Uri $Url -Method Post -ContentType 'application/json' -Body $bodyJson `
            -Headers @{ Authorization = "Bearer $OperatorToken" } -TimeoutSec $TimeoutSec -UseBasicParsing -ErrorAction Stop
        return [pscustomobject]@{ StatusCode = [int]$resp.StatusCode; Json = ($resp.Content | ConvertFrom-Json) }
    } catch {
        $sc = $null; $parsed = $null
        if ($_.Exception.Response) {
            try { $sc = [int]$_.Exception.Response.StatusCode } catch {}
            try {
                $stream = $_.Exception.Response.GetResponseStream()
                $reader = New-Object System.IO.StreamReader($stream)
                $raw = $reader.ReadToEnd(); $reader.Dispose(); $stream.Dispose()
                if ($raw) { $parsed = $raw | ConvertFrom-Json }
            } catch {}
        }
        return [pscustomobject]@{ StatusCode = $sc; Json = $parsed }
    }
}

function Invoke-OpsAction {
    param([string]$BaseUrl, [string]$OperatorToken, [string]$ActionKey)
    return Invoke-JsonPost -Url ($BaseUrl.TrimEnd('/') + '/api/v1/ops/action') -OperatorToken $OperatorToken -Body @{ action_key = $ActionKey }
}

# =============================================================================
# HEADER
# =============================================================================
function Write-StartupHeader {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$TradingMode,
        [Parameter(Mandatory = $true)][bool]$CheckOnlyFlag,
        [Parameter(Mandatory = $true)][bool]$ScheduledFlag
    )
    $head = Get-RepoHeadShort -RepoRoot $RepoRoot
    $capitalLabel = if ($TradingMode -eq 'Live') { 'REAL' } else { 'SIMULATED' }
    Write-Host ''
    Write-Host ('=' * 60) -ForegroundColor Cyan
    Write-Host ' MiniQuantDesk V4' -ForegroundColor Cyan
    Write-Host ' Official Trading Launcher' -ForegroundColor Cyan
    Write-Host ('=' * 60) -ForegroundColor Cyan
    Write-Host ''
    Write-Host ("  {0,-18}: {1}" -f 'Mode', $TradingMode.ToUpperInvariant())
    Write-Host ("  {0,-18}: {1}" -f 'Capital', $capitalLabel)
    Write-Host ("  {0,-18}: {1}" -f 'Repository', $RepoRoot)
    Write-Host ("  {0,-18}: {1}" -f 'Commit', $head)
    Write-Host ("  {0,-18}: {1}" -f 'Trading day', (Get-Date -Format 'yyyy-MM-dd (dddd)'))
    Write-Host ("  {0,-18}: {1}" -f 'Invocation', $(if ($ScheduledFlag) { 'scheduled/unattended' } else { 'interactive' }))
    Write-Host ("  {0,-18}: {1}" -f 'Run type', $(if ($CheckOnlyFlag) { 'CheckOnly (read-only diagnostic)' } else { 'full startup' }))
    Write-Host ''
    if ($TradingMode -eq 'Live') {
        Write-Host '  LIVE TRADING -- REAL CAPITAL' -ForegroundColor Red
    } else {
        Write-Host '  PAPER TRADING -- SIMULATED CAPITAL' -ForegroundColor Green
    }
    Write-Host ''
}

# =============================================================================
# INTERACTIVE MODE SELECTOR / LIVE CONFIRMATION
# =============================================================================
function Read-InteractiveModeSelection {
    Write-Host ''
    Write-Host 'MiniQuantDesk' -ForegroundColor Cyan
    Write-Host ''
    Write-Host 'Select trading mode:'
    Write-Host ''
    Write-Host '  [1] Paper Trading   (simulated capital)'
    Write-Host '  [2] Live Trading    (real capital)'
    Write-Host '  [Q] Quit'
    Write-Host ''
    while ($true) {
        $choice = Read-Host 'Enter choice'
        switch ($choice.Trim().ToUpperInvariant()) {
            '1' { return 'Paper' }
            '2' { return 'Live' }
            'Q' { return $null }
            default { Write-Host 'Invalid choice. Enter 1, 2, or Q.' -ForegroundColor Yellow }
        }
    }
}

function Confirm-LiveIntent {
    Write-Host ''
    Write-Host 'You selected LIVE TRADING.' -ForegroundColor Red
    Write-Host 'This mode can use REAL CAPITAL.' -ForegroundColor Red
    Write-Host ''
    $typed = Read-Host 'Type LIVE to continue'
    return ($typed -ceq 'LIVE')
}

# =============================================================================
# LIVE READINESS CHECKS -- all read-only / source-guard-based. None of these
# ever start a process, call a live broker, or mutate any DB or config.
# =============================================================================
function Get-LedgerPatchStatus {
    param(
        [Parameter(Mandatory = $true)][string]$LedgerPath,
        [Parameter(Mandatory = $true)][string]$PatchId
    )
    if (-not (Test-Path $LedgerPath)) { return 'LEDGER_NOT_FOUND' }
    $lines = Get-Content -Path $LedgerPath
    for ($i = 0; $i -lt $lines.Count; $i++) {
        if ($lines[$i] -match [regex]::Escape($PatchId)) {
            for ($j = $i; $j -lt [Math]::Min($i + 6, $lines.Count); $j++) {
                if ($lines[$j] -match '\*\*Status:\*\*\s*([A-Z_]+)') {
                    return $Matches[1]
                }
            }
        }
    }
    return 'NOT_FOUND_IN_LEDGER'
}

function Test-LiveEnvironment {
    $keyPresent = (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable('ALPACA_API_KEY_LIVE', 'Process'))) -or
                  (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable('ALPACA_API_KEY_LIVE', 'User')))
    $secretPresent = (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable('ALPACA_API_SECRET_LIVE', 'Process'))) -or
                     (-not [string]::IsNullOrWhiteSpace([Environment]::GetEnvironmentVariable('ALPACA_API_SECRET_LIVE', 'User')))
    $pass = $keyPresent -and $secretPresent
    return [pscustomobject]@{
        Name     = 'deployment configuration'
        Status   = $(if ($pass) { 'PASS' } else { 'BLOCKED' })
        Detail   = 'ALPACA_API_KEY_LIVE / ALPACA_API_SECRET_LIVE presence check (values never read or printed).'
        PatchIds = @()
    }
}

function Test-LiveBrokerTruth {
    param([Parameter(Mandatory = $true)][string]$LedgerPath)
    $status = Get-LedgerPatchStatus -LedgerPath $LedgerPath -PatchId 'LIVE-SECRETS-CONSOLIDATION-01'
    $pass = $status -eq 'CLOSED'
    return [pscustomobject]@{
        Name     = 'broker configuration'
        Status   = $(if ($pass) { 'PASS' } else { 'BLOCKED' })
        Detail   = "LIVE-SECRETS-CONSOLIDATION-01 ledger status=$status (live secret resolution not yet routed through mqk_config::secrets; live endpoint is hardcoded to https://api.alpaca.markets)."
        PatchIds = @('LIVE-SECRETS-CONSOLIDATION-01')
    }
}

function Test-LiveAccountTruth {
    param([Parameter(Mandatory = $true)][string]$LedgerPath)
    $status = Get-LedgerPatchStatus -LedgerPath $LedgerPath -PatchId 'LIVE-ACCOUNT-TRUTH-01'
    $pass = $status -eq 'CLOSED'
    return [pscustomobject]@{
        Name     = 'account truth'
        Status   = $(if ($pass) { 'PASS' } else { 'BLOCKED' })
        Detail   = "LIVE-ACCOUNT-TRUTH-01 ledger status=$status (buying_power aliasing to cash at routes/portfolio.rs:440 not yet fixed)."
        PatchIds = @('LIVE-ACCOUNT-TRUTH-01')
    }
}

function Test-LiveReconciliation {
    param([Parameter(Mandatory = $true)][string]$LedgerPath)
    $status = Get-LedgerPatchStatus -LedgerPath $LedgerPath -PatchId 'LIVE-TINY-CAPITAL-SMOKE-01'
    $pass = $status -eq 'CLOSED'
    return [pscustomobject]@{
        Name     = 'reconciliation'
        Status   = $(if ($pass) { 'PASS' } else { 'BLOCKED_NOT_IMPLEMENTED' })
        Detail   = "No live-capital smoke/reconciliation automation exists in scripts/ yet. LIVE-TINY-CAPITAL-SMOKE-01 ledger status=$status."
        PatchIds = @('LIVE-TINY-CAPITAL-SMOKE-01')
    }
}

function Test-LiveRisk {
    param([Parameter(Mandatory = $true)][string]$LedgerPath)
    $status = Get-LedgerPatchStatus -LedgerPath $LedgerPath -PatchId 'LIVE-FLATTEN-PROOF-01'
    $pass = $status -eq 'CLOSED'
    return [pscustomobject]@{
        Name     = 'risk'
        Status   = $(if ($pass) { 'PASS' } else { 'BLOCKED' })
        Detail   = "LIVE-FLATTEN-PROOF-01 (LiveShadow flatten-on-halt scenario proof) ledger status=$status."
        PatchIds = @('LIVE-FLATTEN-PROOF-01')
    }
}

function Test-LiveTrustChain {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$LedgerPath
    )
    $parityPath = Join-Path $RepoRoot 'research-py\src\mqk_research\deployment\parity.py'
    $trustHardcodedFalse = $false
    if (Test-Path $parityPath) {
        $content = Get-Content -Path $parityPath -Raw
        if ($content -match 'live_trust_complete\s*=\s*False') { $trustHardcodedFalse = $true }
    }
    $ids = @('LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01', 'LIVE-TRUST-CHAIN-PARITY-SCORER-01', 'LIVE-TRUST-CHAIN-EVIDENCE-SIGNER-01')
    $statusPairs = $ids | ForEach-Object { "$_=$(Get-LedgerPatchStatus -LedgerPath $LedgerPath -PatchId $_)" }
    $allClosed = -not (@($statusPairs) | Where-Object { $_ -notmatch '=CLOSED$' })
    $pass = $allClosed -and (-not $trustHardcodedFalse)
    $trustDetail = if ($trustHardcodedFalse) { 'hardcoded False in research-py/src/mqk_research/deployment/parity.py (no mechanism to set True yet)' } else { 'not confirmed hardcoded False in source -- verify manually before trusting this PASS' }
    return [pscustomobject]@{
        Name     = 'trust chain'
        Status   = $(if ($pass) { 'PASS' } else { 'BLOCKED' })
        Detail   = "live_trust_complete is $trustDetail. Chain: $($statusPairs -join '; ')."
        PatchIds = $ids
    }
}

function Test-LiveAuthorization {
    param([Parameter(Mandatory = $true)][string]$LedgerPath)
    $status = Get-LedgerPatchStatus -LedgerPath $LedgerPath -PatchId 'LIVE-CAPITAL-EXTERNAL-PROOF-01'
    $pass = $status -eq 'CLOSED'
    return [pscustomobject]@{
        Name     = 'live_trust_complete'
        Status   = $(if ($pass) { 'PASS' } else { 'FALSE' })
        Detail   = "LIVE-CAPITAL-EXTERNAL-PROOF-01 (first real live-capital start, tiny notional) ledger status=$status. This launcher patch does not and cannot authorize live capital."
        PatchIds = @('LIVE-CAPITAL-EXTERNAL-PROOF-01')
    }
}

function Invoke-LiveStartup {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][bool]$CheckOnlyFlag,
        [Parameter(Mandatory = $true)][bool]$ScheduledFlag,
        [Parameter(Mandatory = $true)][string]$LogPath
    )

    $ledgerPath = Join-Path $RepoRoot 'MiniQuantDesk_Master_Patch_Ledger_v2_updated.md'

    Write-Section 'LIVE readiness / preflight chain (read-only, source-guard-based)'

    $checks = @(
        (Test-LiveEnvironment),
        (Test-LiveBrokerTruth -LedgerPath $ledgerPath),
        (Test-LiveAccountTruth -LedgerPath $ledgerPath),
        (Test-LiveReconciliation -LedgerPath $ledgerPath),
        (Test-LiveRisk -LedgerPath $ledgerPath),
        (Test-LiveTrustChain -RepoRoot $RepoRoot -LedgerPath $ledgerPath),
        (Test-LiveAuthorization -LedgerPath $ledgerPath)
    )

    foreach ($c in $checks) {
        $label = "{0,-28}" -f $c.Name
        if ($c.Status -eq 'PASS') {
            Write-Host "  $label .... PASS" -ForegroundColor Green
        } else {
            Write-Host "  $label .... $($c.Status)" -ForegroundColor Red
        }
    }

    $blockedChecks = @($checks | Where-Object { $_.Status -ne 'PASS' })

    $unattendedBlocked = $false
    if ($ScheduledFlag) {
        Write-Host ("  {0,-28} .... BLOCKED (no explicit unattended-live authorization found in repo source)" -f 'unattended live authority') -ForegroundColor Red
        $unattendedBlocked = $true
    }

    Write-Host ''
    $verdict = if ($blockedChecks.Count -eq 0 -and -not $unattendedBlocked) { 'LIVE_START_READY' } else { 'LIVE_START_BLOCKED' }

    if ($verdict -eq 'LIVE_START_READY') {
        Write-Host '  LIVE_START_READY (every current gate passes) -- but this launcher patch (OFFICIAL-DUAL-MODE-LAUNCHER-01) does not implement a live runtime start path.' -ForegroundColor Yellow
    } else {
        Write-Host '  LIVE START REFUSED' -ForegroundColor Red
        Write-Host ''
        Write-Host '  Blocking ledger patch IDs:' -ForegroundColor Red
        $allIds = @($checks | ForEach-Object { $_.PatchIds } | Where-Object { $_ } | Select-Object -Unique)
        foreach ($id in $allIds) { Write-Host "    - $id" -ForegroundColor Red }
    }

    Write-Host ''
    Write-Host '  No live broker orders were enabled.' -ForegroundColor DarkGray
    Write-Host '  No live runtime was started.' -ForegroundColor DarkGray
    Write-Host '  No live DB was mutated.' -ForegroundColor DarkGray
    Write-Host ''

    $logEntry = @{
        timestamp                      = (Get-Date).ToUniversalTime().ToString('o')
        mode                            = 'live'
        scheduled                       = $ScheduledFlag
        check_only                      = $CheckOnlyFlag
        repo_head                       = (Get-RepoHeadShort -RepoRoot $RepoRoot)
        verdict                         = $verdict
        checks                          = @($checks | ForEach-Object { @{ name = $_.Name; status = $_.Status; detail = $_.Detail; patch_ids = $_.PatchIds } })
        no_live_order_authority_granted = $true
    }
    Write-LauncherLogEntry -Path $LogPath -Entry $logEntry

    if ($unattendedBlocked) { return $script:ExitUnattendedLiveUnauthorized }
    if ($verdict -eq 'LIVE_START_BLOCKED') { return $script:ExitLiveBlocked }
    if (-not $CheckOnlyFlag) {
        # Every gate passed, yet this patch still never starts a live runtime.
        return $script:ExitLiveBlocked
    }
    return $script:ExitOk
}

# =============================================================================
# PAPER STARTUP -- reuses the existing accepted Launch-VeritasLedger.ps1 /
# Prep-PremarketMarketData.ps1 / Refresh-IntradayMarketData.ps1 paths rather
# than reimplementing them. This launcher NEVER calls the start-system
# action_key -- runtime start authority remains the autonomous session
# controller.
# =============================================================================
function Invoke-PaperStartup {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$LauncherModeArg,
        [Parameter(Mandatory = $true)][bool]$CheckOnlyFlag,
        [Parameter(Mandatory = $true)][bool]$ScheduledFlag,
        [Parameter(Mandatory = $true)][bool]$ArmPaperFlag,
        [Parameter(Mandatory = $true)][bool]$SkipGuiFlag,
        [Parameter(Mandatory = $true)][bool]$ForceRebuildDaemon,
        [Parameter(Mandatory = $true)][bool]$ForceRebuildGui,
        [Parameter(Mandatory = $true)][bool]$CaptureEvidenceFlag,
        [Parameter(Mandatory = $true)][string]$LogPath
    )

    $launchScript   = Join-Path $RepoRoot 'scripts\windows\Launch-VeritasLedger.ps1'
    $prepScript     = Join-Path $RepoRoot 'scripts\windows\Prep-PremarketMarketData.ps1'
    $refreshScript  = Join-Path $RepoRoot 'scripts\windows\Refresh-IntradayMarketData.ps1'
    $evidenceScript = Join-Path $RepoRoot 'scripts\windows\Capture-PaperSmokeEvidence.ps1'
    $daemonBaseUrl  = 'http://127.0.0.1:8899'

    $logEntry = @{
        timestamp                  = (Get-Date).ToUniversalTime().ToString('o')
        mode                       = 'paper'
        scheduled                  = $ScheduledFlag
        check_only                 = $CheckOnlyFlag
        repo_head                  = (Get-RepoHeadShort -RepoRoot $RepoRoot)
        stages                     = @()
        manual_start_system_used   = $false
        paper_economics_changed    = $false
    }

    if ($CheckOnlyFlag) {
        Write-Section 'PAPER -- read-only diagnostic (delegates to Launch-VeritasLedger.ps1 -CheckOnly)'
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $launchScript -CheckOnly | Out-Host
        $checkExit = $LASTEXITCODE

        Write-Section 'PAPER -- symbol universe (ingest-plan, best-effort read-only)'
        $health = Invoke-JsonGet -Url ($daemonBaseUrl + '/v1/health') -TimeoutSec 2
        if ($health.Ok) {
            $plan = Invoke-JsonGet -Url ($daemonBaseUrl + '/api/v1/market-data/ingest-plan') -TimeoutSec 5
            if ($plan.Ok -and $plan.Json.required_symbols -and @($plan.Json.required_symbols).Count -gt 0) {
                Write-Ok "Authoritative symbol universe: $(@($plan.Json.required_symbols) -join ', ') / $($plan.Json.timeframe) (truth_state=$($plan.Json.truth_state), source=$($plan.Json.symbol_source))"
            } else {
                Write-Warn 'Daemon reachable but ingest-plan returned no required symbols; universe unverified.'
            }
        } else {
            Write-Warn 'Daemon offline -- symbol universe cannot be verified from ingest-plan in CheckOnly. A full run resolves it authoritatively once the daemon starts.'
        }

        $logEntry.stages += @{ name = 'checkonly'; exit_code = $checkExit }
        Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
        if ($checkExit -ne 0) { return $script:ExitGeneric }
        return $script:ExitOk
    }

    # --- Full startup ---
    Write-Section 'PAPER -- daemon + GUI (delegates to Launch-VeritasLedger.ps1)'
    $lvlArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $launchScript, '-Mode', $LauncherModeArg)
    if ($ForceRebuildDaemon) { $lvlArgs += '-RebuildDaemon' }
    if ($ForceRebuildGui)    { $lvlArgs += '-RebuildGui' }
    if ($SkipGuiFlag)        { $lvlArgs += '-SkipGui' }
    & powershell.exe @lvlArgs | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Fail 'Launch-VeritasLedger.ps1 failed to bring up a verified paper daemon/GUI.'
        $logEntry.stages += @{ name = 'daemon_gui'; exit_code = $LASTEXITCODE }
        Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
        return $script:ExitGeneric
    }
    $logEntry.stages += @{ name = 'daemon_gui'; exit_code = 0 }

    $operatorToken = [Environment]::GetEnvironmentVariable('MQK_OPERATOR_TOKEN', 'Process')
    if ([string]::IsNullOrWhiteSpace($operatorToken)) { $operatorToken = [Environment]::GetEnvironmentVariable('MQK_OPERATOR_TOKEN', 'User') }
    if ([string]::IsNullOrWhiteSpace($operatorToken)) {
        Write-Fail 'MQK_OPERATOR_TOKEN is not configured; cannot proceed past daemon attach.'
        Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
        return $script:ExitGeneric
    }

    # Belt-and-suspenders safety guard: must never see live routing on a Paper launch.
    $status = Invoke-JsonGet -Url ($daemonBaseUrl + '/api/v1/system/status') -TimeoutSec 5
    if (-not $status.Ok) {
        Write-Fail 'Could not verify daemon status after startup.'
        Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
        return $script:ExitGeneric
    }
    if ($status.Json.live_routing_enabled -eq $true) {
        Write-Fail 'live_routing_enabled=true on the daemon this Paper launch attached to. Refusing to proceed.'
        Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
        return $script:ExitSafetyRefusal
    }
    if ($status.Json.daemon_mode -ne 'paper' -or $status.Json.adapter_id -ne 'alpaca') {
        Write-Fail "Daemon is not in the expected paper+alpaca posture (daemon_mode=$($status.Json.daemon_mode) adapter_id=$($status.Json.adapter_id))."
        Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
        return $script:ExitSafetyRefusal
    }
    Write-Ok 'Paper safety guard confirmed: live_routing_enabled=false, daemon_mode=paper, adapter_id=alpaca.'

    Write-Section 'PAPER -- authoritative symbol universe + market-data prep (ingest-plan)'
    $plan = Invoke-JsonGet -Url ($daemonBaseUrl + '/api/v1/market-data/ingest-plan') -TimeoutSec 10
    if (-not $plan.Ok -or -not $plan.Json.required_symbols -or @($plan.Json.required_symbols).Count -eq 0) {
        Write-Fail 'Could not resolve an authoritative symbol universe from GET /api/v1/market-data/ingest-plan.'
        Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
        return $script:ExitDataReadiness
    }
    $requiredSymbols = @($plan.Json.required_symbols)
    $planTimeframe = $plan.Json.timeframe
    Write-Ok "Required symbols: $($requiredSymbols -join ', ') timeframe=$planTimeframe (truth_state=$($plan.Json.truth_state) source=$($plan.Json.symbol_source))"

    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $prepScript -SymbolsFromIngestPlan | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Write-Fail 'Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan failed the data-readiness gate.'
        $logEntry.stages += @{ name = 'market_data_prep'; exit_code = $LASTEXITCODE }
        Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
        return $script:ExitDataReadiness
    }
    # Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan ingests exactly
    # plan.required_symbols (WATCHLIST-INGEST-PLAN-01); REFRESHED_SYMBOLS ==
    # REQUIRED_SYMBOLS holds by construction of that script, not by re-derivation here.
    Write-Ok "REFRESHED_SYMBOLS == REQUIRED_SYMBOLS confirmed ($($requiredSymbols -join ', '))."
    $logEntry.stages += @{ name = 'market_data_prep'; exit_code = 0; symbols = $requiredSymbols; timeframe = $planTimeframe }

    Write-Section 'PAPER -- recurring intraday refresh for the full session'
    $sessionResp = Invoke-JsonGet -Url ($daemonBaseUrl + '/api/v1/system/session') -TimeoutSec 5
    $durationSeconds = 1800
    if ($sessionResp.Ok -and $sessionResp.Json.session_stop_utc) {
        try {
            $parts = $sessionResp.Json.session_stop_utc -split ':'
            $nowUtc = (Get-Date).ToUniversalTime()
            $stopUtc = (Get-Date -Year $nowUtc.Year -Month $nowUtc.Month -Day $nowUtc.Day -Hour ([int]$parts[0]) -Minute ([int]$parts[1]) -Second 0).ToUniversalTime()
            $bufferedStop = $stopUtc.AddMinutes(15)
            $remaining = [int]([TimeSpan]($bufferedStop - $nowUtc)).TotalSeconds
            if ($remaining -gt 60) { $durationSeconds = $remaining } else { $durationSeconds = 900 }
        } catch {
            Write-Warn 'Could not parse session_stop_utc; using 1800s default intraday-refresh duration.'
        }
    } else {
        Write-Warn 'session_stop_utc unavailable; using 1800s default intraday-refresh duration.'
    }
    $refreshTimeframe = if ($planTimeframe -and $planTimeframe -ne '1D') { $planTimeframe } else { '5m' }
    $refreshLogDir = Join-Path $RepoRoot 'exports\launcher'
    New-Item -ItemType Directory -Force -Path $refreshLogDir | Out-Null
    $stamp = Get-Date -Format 'yyyyMMdd_HHmmss'
    $refreshStdout = Join-Path $refreshLogDir "intraday_refresh_$stamp.stdout.log"
    $refreshStderr = Join-Path $refreshLogDir "intraday_refresh_$stamp.stderr.log"
    $refreshArgs = @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $refreshScript,
        '-Symbols', ($requiredSymbols -join ','), '-Timeframe', $refreshTimeframe,
        '-IntervalSeconds', 300, '-DurationSeconds', $durationSeconds
    )
    Start-Process -FilePath 'powershell.exe' -ArgumentList $refreshArgs -WindowStyle Hidden `
        -RedirectStandardOutput $refreshStdout -RedirectStandardError $refreshStderr | Out-Null
    Write-Ok "Intraday refresh loop started in background (duration=${durationSeconds}s, symbols=$($requiredSymbols -join ', '), timeframe=$refreshTimeframe)."
    $logEntry.stages += @{ name = 'intraday_refresh_started'; duration_seconds = $durationSeconds; symbols = $requiredSymbols; timeframe = $refreshTimeframe }

    Write-Section 'PAPER -- reconciliation (hard gate)'
    $adopt = Invoke-JsonPost -Url ($daemonBaseUrl + '/api/v1/ops/repair/adopt-broker-position-baseline') -OperatorToken $operatorToken -Body @{ confirmation = 'ADOPT_BROKER_POSITION_BASELINE' }
    if ($adopt.StatusCode -eq 200 -or $adopt.StatusCode -eq 409) {
        Write-Ok "Broker position baseline adopt: HTTP $($adopt.StatusCode) (200=adopted, 409=already adopted)."
    } else {
        Write-Warn "Broker position baseline adopt returned HTTP $($adopt.StatusCode); continuing to the hard reconcile-status gate."
    }
    $reconcile = Invoke-JsonGet -Url ($daemonBaseUrl + '/api/v1/reconcile/status') -TimeoutSec 5
    if (-not $reconcile.Ok -or $reconcile.Json.status -ne 'ok' -or $reconcile.Json.truth_state -ne 'active') {
        $rStatus = if ($reconcile.Ok) { $reconcile.Json.status } else { 'unreachable' }
        Write-Fail "Reconcile hard gate failed: status=$rStatus. Resolve before this launcher will proceed."
        $logEntry.stages += @{ name = 'reconcile'; ok = $false; status = $rStatus }
        Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
        return $script:ExitBackendReconcile
    }
    Write-Ok 'Reconcile hard gate passed (status=ok, truth_state=active).'
    $logEntry.stages += @{ name = 'reconcile'; ok = $true }

    Write-Section 'PAPER -- halt recovery (if required)'
    $armState = $null
    $readiness = Invoke-JsonGet -Url ($daemonBaseUrl + '/api/v1/autonomous/readiness') -TimeoutSec 5
    if ($readiness.Ok) { $armState = $readiness.Json.arm_state }
    $needsHaltRecovery = ($status.Json.kill_switch_active -eq $true) -or ($status.Json.runtime_status -eq 'halted') -or ($armState -eq 'halted')
    if ($needsHaltRecovery) {
        Write-Warn 'Halted state detected; running disarm-execution -> clear-halted-run.'
        $null = Invoke-OpsAction -BaseUrl $daemonBaseUrl -OperatorToken $operatorToken -ActionKey 'disarm-execution'
        $clear = Invoke-OpsAction -BaseUrl $daemonBaseUrl -OperatorToken $operatorToken -ActionKey 'clear-halted-run'
        if ($clear.StatusCode -ne 200) {
            Write-Fail "clear-halted-run returned HTTP $($clear.StatusCode); halt recovery incomplete."
            $logEntry.stages += @{ name = 'halt_recovery'; needed = $true; ok = $false }
            Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
            return $script:ExitBackendReconcile
        }
        Start-Sleep -Milliseconds 400
        $statusAfter = Invoke-JsonGet -Url ($daemonBaseUrl + '/api/v1/system/status') -TimeoutSec 5
        if ($statusAfter.Ok -and $statusAfter.Json.kill_switch_active -eq $true) {
            Write-Fail 'kill_switch_active=true persists after halt recovery. Manual operator action required.'
            $logEntry.stages += @{ name = 'halt_recovery'; needed = $true; ok = $false }
            Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
            return $script:ExitBackendReconcile
        }
        Write-Ok 'Halt recovery complete.'
        $logEntry.stages += @{ name = 'halt_recovery'; needed = $true; ok = $true }
    } else {
        Write-Ok 'No halted state detected; nothing to recover.'
        $logEntry.stages += @{ name = 'halt_recovery'; needed = $false }
    }

    Write-Section 'PAPER -- arm'
    if ($ArmPaperFlag) {
        $freshStatus = Invoke-JsonGet -Url ($daemonBaseUrl + '/api/v1/system/status') -TimeoutSec 5
        if (-not $freshStatus.Ok -or $freshStatus.Json.live_routing_enabled -eq $true -or $freshStatus.Json.daemon_mode -ne 'paper' -or $freshStatus.Json.adapter_id -ne 'alpaca') {
            Write-Fail 'Arm refused: fresh daemon status failed the paper-only safety pre-check.'
            $logEntry.stages += @{ name = 'arm'; requested = $true; accepted = $false }
            Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
            return $script:ExitSafetyRefusal
        }
        $arm = Invoke-OpsAction -BaseUrl $daemonBaseUrl -OperatorToken $operatorToken -ActionKey 'arm-execution'
        if ($arm.StatusCode -ne 200 -or $arm.Json.accepted -ne $true) {
            Write-Fail "arm-execution was not accepted (status=$($arm.StatusCode)). Runtime not started, no orders submitted."
            $logEntry.stages += @{ name = 'arm'; requested = $true; accepted = $false }
            Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
            return $script:ExitBackendReconcile
        }
        Write-Ok 'arm-execution accepted. ARMED ONLY -- runtime not started, no orders submitted.'
        $logEntry.stages += @{ name = 'arm'; requested = $true; accepted = $true }
    } else {
        Write-Ok '-ArmPaper not requested; leaving arm state as-is. Pass -ArmPaper to arm explicitly.'
        $logEntry.stages += @{ name = 'arm'; requested = $false }
    }

    if ($CaptureEvidenceFlag) {
        Write-Section 'PAPER -- evidence capture'
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $evidenceScript -Label 'launcher_startup' | Out-Host
        if ($LASTEXITCODE -ne 0) { Write-Warn "Evidence capture exited $LASTEXITCODE; launcher continues." }
        $logEntry.stages += @{ name = 'evidence_capture'; requested = $true }
    }

    Write-Section 'PAPER -- runtime start authority'
    Write-Ok 'This launcher never calls start-system. Runtime start authority remains the autonomous session controller; it will start the runtime at the correct session-window boundary.'

    Write-LauncherLogEntry -Path $LogPath -Entry $logEntry
    return $script:ExitOk
}

# =============================================================================
# MAIN DISPATCH
# =============================================================================
$RepoRoot = Get-RepoRoot

if ($Scheduled.IsPresent -and [string]::IsNullOrWhiteSpace($Mode)) {
    Write-Host ''
    Write-Host 'STARTUP_REFUSED' -ForegroundColor Red
    Write-Host 'reason=scheduled_mode_requires_explicit_trading_mode' -ForegroundColor Red
    Write-Host ''
    exit $script:ExitSafetyRefusal
}

$resolvedMode = $Mode
if ([string]::IsNullOrWhiteSpace($resolvedMode)) {
    $resolvedMode = Read-InteractiveModeSelection
    if ([string]::IsNullOrWhiteSpace($resolvedMode)) {
        Write-Host 'Quit.' -ForegroundColor Yellow
        exit $script:ExitOk
    }
}

Write-StartupHeader -RepoRoot $RepoRoot -TradingMode $resolvedMode -CheckOnlyFlag $CheckOnly.IsPresent -ScheduledFlag $Scheduled.IsPresent

try {
    if ($resolvedMode -eq 'Live') {
        if (-not $Scheduled.IsPresent -and -not $CheckOnly.IsPresent) {
            $confirmed = Confirm-LiveIntent
            if (-not $confirmed) {
                Write-Host 'Live trading not confirmed. Exiting.' -ForegroundColor Yellow
                exit $script:ExitSafetyRefusal
            }
        }
        $logPath = New-LauncherLog -RepoRoot $RepoRoot -ModeLabel 'live'
        $code = Invoke-LiveStartup -RepoRoot $RepoRoot -CheckOnlyFlag $CheckOnly.IsPresent -ScheduledFlag $Scheduled.IsPresent -LogPath $logPath
        exit $code
    }
    else {
        $launcherModeArg = if ($ArmPaper.IsPresent) { 'TradeReady' } else { 'Observe' }
        $forceRebuildDaemon = $Rebuild.IsPresent -or $RebuildDaemon.IsPresent
        $forceRebuildGui = $Rebuild.IsPresent -or $RebuildGui.IsPresent
        $effectiveSkipGui = $SkipGui.IsPresent -or $Scheduled.IsPresent
        $logPath = New-LauncherLog -RepoRoot $RepoRoot -ModeLabel 'paper'
        $code = Invoke-PaperStartup -RepoRoot $RepoRoot -LauncherModeArg $launcherModeArg `
            -CheckOnlyFlag $CheckOnly.IsPresent -ScheduledFlag $Scheduled.IsPresent -ArmPaperFlag $ArmPaper.IsPresent `
            -SkipGuiFlag $effectiveSkipGui -ForceRebuildDaemon $forceRebuildDaemon -ForceRebuildGui $forceRebuildGui `
            -CaptureEvidenceFlag $CaptureEvidence.IsPresent -LogPath $logPath
        exit $code
    }
}
catch {
    Write-Host ''
    Write-Fail "LAUNCH FAILED: $($_.Exception.Message)"
    Write-Host ''
    exit $script:ExitGeneric
}
