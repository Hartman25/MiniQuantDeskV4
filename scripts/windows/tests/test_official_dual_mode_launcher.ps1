# =============================================================================
# test_official_dual_mode_launcher.ps1
# OFFICIAL-DUAL-MODE-LAUNCHER-01 / PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01
#
# Proof for scripts\windows\Start-MiniQuantDesk.ps1: Paper/Live mode
# selection, fail-closed -Scheduled contract, Live confirmation gating, the
# current-truth LIVE_START_BLOCKED verdict, and (as of the autofresh/
# launcher integration) the daemon required-universe market-data scheduler
# as the launcher's SOLE ongoing market-data authority.
#
# Mix of:
#  (a) static source-guard assertions against the launcher's own text, and
#  (b) real subprocess invocations of modes that are safe to actually run
#      because they are read-only / source-guard-based by construction
#      (-Mode Live [-CheckOnly], -Scheduled without -Mode, -Mode Live
#      -Scheduled, -Mode Paper -CheckOnly). No daemon is started, no order
#      is placed, no live credential is read into output, no live DB is
#      touched. -Mode Paper -CheckOnly may legitimately report exit 1 on a
#      dev worktree that lacks .env.local -- this proves the CheckOnly path
#      runs and stays read-only, not that this box is soak-ready.
#  (c) functional proofs of the required-universe start/verify fail-closed
#      contract (Confirm-RequiredUniverseSchedulerOwnership /
#      Start-OrVerifyRequiredUniverseScheduler), dot-sourcing the real
#      launcher and shadowing its own Invoke-JsonGet/Invoke-JsonPost HTTP
#      helpers with mocked responses -- the same function-shadowing seam
#      this file already uses for Get-CimInstance/ConvertTo-Json fixtures,
#      and the same technique
#      scripts\guards\validate_market_data_autofresh_required_universe_01_repair_02.ps1
#      already uses against Start-PaperTradingSmoke.ps1's equivalent
#      functions. Zero real daemon/network/DB/order/runtime side effects.
#
# PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01 removed the prior PID/mutex/
# JSON-file Refresh-IntradayMarketData.ps1 ownership subsystem (Get-
# IntradayRefreshOwnerPath / Test-RefreshCommandLineIdentity / Get-
# RefreshOwnerProcessIdentity / Find-MatchingRefreshOwnerProcesses / Get-
# IntradayRefreshOwnerLockName / Get-IntradayRefreshOwnerState / Set-
# IntradayRefreshOwnerRecord / Stop-NewlyCreatedRefreshChild / Request-
# IntradayRefreshOwnership) and Get-AuthoritativeIntradayRefreshDuration --
# every test section that proved that subsystem (formerly Sections 5/6/7,
# REPAIR-02/03/04-era) is removed along with the code it proved. Their
# proof intent (idempotent, fail-closed, non-duplicating market-data
# authority) is now carried by the L1-L12 tests in Section 5 below, proving
# the daemon-scheduler-based replacement instead.
#
# Exit codes: 0 = all proofs held, 1 = at least one did not.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$WindowsDir = (Resolve-Path (Join-Path $ScriptDir '..')).Path.TrimEnd('\')
$RepoRoot   = (Resolve-Path (Join-Path $WindowsDir '..\..')).Path.TrimEnd('\')
$Launcher   = Join-Path $WindowsDir 'Start-MiniQuantDesk.ps1'
$VeritasLedger = Join-Path $WindowsDir 'Launch-VeritasLedger.ps1'

$Violations = 0
function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan }
function Assert-True {
    param([string]$Label, [bool]$Condition)
    if ($Condition) {
        Show-Green "  OK -- $Label"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label"
    }
}

if (-not (Test-Path $Launcher)) {
    Show-Red "FATAL -- launcher script not found: $Launcher"
    exit 1
}
if (-not (Test-Path $VeritasLedger)) {
    Show-Red "FATAL -- Launch-VeritasLedger.ps1 not found: $VeritasLedger"
    exit 1
}

$LauncherText = Get-Content -Path $Launcher -Raw
$VeritasLedgerText = Get-Content -Path $VeritasLedger -Raw

function Invoke-Launcher {
    param([string[]]$LauncherArgs)
    $output = & powershell -NoProfile -ExecutionPolicy Bypass -File $Launcher @LauncherArgs 2>&1
    return @{ Output = ($output -join "`n"); ExitCode = $LASTEXITCODE }
}

# ---------------------------------------------------------------------------
# Section 1: static source-guard checks (no process spawned)
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 1: static source-guard checks ==='

Assert-True 'ValidateSet restricts -Mode to Paper/Live only' `
    ($LauncherText -match "\[ValidateSet\('Paper',\s*'Live'\)\]")

Assert-True '-Scheduled with no -Mode is refused before any other logic runs' `
    ($LauncherText -match 'scheduled_mode_requires_explicit_trading_mode')

Assert-True 'Live mode requires typed "LIVE" confirmation for interactive non-CheckOnly runs' `
    ($LauncherText -match "Confirm-LiveIntent" -and $LauncherText -match "-ceq 'LIVE'")

Assert-True 'Live confirmation is skipped for -CheckOnly and -Scheduled (never prompts headless)' `
    ($LauncherText -match '-not \$Scheduled\.IsPresent -and -not \$CheckOnly\.IsPresent')

Assert-True 'Live readiness checks reference the real ledger file, not a hardcoded verdict' `
    ($LauncherText -match 'MiniQuantDesk_Master_Patch_Ledger_v2_updated\.md' -and $LauncherText -match 'Get-LedgerPatchStatus')

Assert-True 'Live trust-chain check reads live_trust_complete from research-py source, not a literal' `
    ($LauncherText.Contains('research-py\src\mqk_research\deployment\parity.py') -and $LauncherText.Contains('live_trust_complete\s*=\s*False'))

Assert-True 'Launcher never contains the start-system action_key literal (runtime start stays with the autonomous controller)' `
    (-not ($LauncherText -match "'start-system'" -or $LauncherText -match '"start-system"'))

Assert-True 'Launcher never sets deployment_mode/adapter to a live value anywhere in source' `
    (-not ($LauncherText -match "MQK_DAEMON_DEPLOYMENT_MODE\s*=\s*['""]live" -or $LauncherText -match "MQK_DAEMON_ADAPTER_ID\s*=\s*['""]live"))

Assert-True 'Launcher never prints ALPACA_API_KEY_LIVE / ALPACA_API_SECRET_LIVE values (presence-only checks)' `
    (-not ($LauncherText -match 'Write-Host.*\$env:ALPACA_API_(KEY|SECRET)_LIVE' -or $LauncherText -match 'Write-Ok.*\$env:ALPACA_API_(KEY|SECRET)_LIVE'))

Assert-True 'Paper full run delegates daemon/GUI startup to Launch-VeritasLedger.ps1 (no reimplementation)' `
    ($LauncherText -match 'Launch-VeritasLedger\.ps1')

Assert-True 'Paper full run never invokes Prep-PremarketMarketData.ps1 (PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01: daemon required-universe scheduler owns bootstrap/repair/provider-mapping instead)' `
    (-not ($LauncherText -match '&\s*powershell\.exe.*Prep-PremarketMarketData'))

Assert-True 'Paper full run never starts Refresh-IntradayMarketData.ps1 as a child process (no Start-Process call anywhere in the launcher)' `
    (-not ($LauncherText -match 'Start-Process'))

Assert-True 'Paper full run establishes required-universe scheduler authority via POST .../required-universe/start' `
    ($LauncherText -match '/api/v1/market-data/required-universe/start')

Assert-True 'Paper full run verifies scheduler ownership via GET .../required-universe/status (200/409 alone is never treated as proof)' `
    ($LauncherText -match '/api/v1/market-data/required-universe/status')

Assert-True 'Ingest-plan route is used for operator display/logging only, never to gate startup (required-universe truth is authoritative)' `
    ($LauncherText -match '/api/v1/market-data/ingest-plan' -and $LauncherText -match 'display only')

Assert-True 'Paper arm path re-checks live_routing_enabled/daemon_mode/adapter_id immediately before arm-execution' `
    ($LauncherText.Contains('$freshStatus.Json.live_routing_enabled -eq $true -or $freshStatus.Json.daemon_mode -ne ''paper'''))

# Scope to Invoke-PaperStartup's body first (function Invoke-OpsAction is
# defined earlier in the file and would otherwise produce a false match).
$PaperStartupBody = ($LauncherText -split 'function Invoke-PaperStartup ')[1]
$CheckOnlyBlockText = ($PaperStartupBody -split '# --- Full startup ---')[0]
Assert-True 'CheckOnly path never calls Invoke-OpsAction / arm-execution / clear-halted-run' `
    (-not ($CheckOnlyBlockText -match 'Invoke-OpsAction'))

Assert-True 'CheckOnly path never calls required-universe/start (POST) -- read-only status GET only' `
    (-not ($CheckOnlyBlockText -match '/required-universe/start'))

Assert-True 'Exit-code map documented in header comment matches the six defined script-scope constants' `
    (($LauncherText -match 'ExitOk = 0') -and ($LauncherText -match 'ExitSafetyRefusal = 2') -and
     ($LauncherText -match 'ExitDataReadiness = 3') -and ($LauncherText -match 'ExitBackendReconcile = 4') -and
     ($LauncherText -match 'ExitLiveBlocked = 5') -and ($LauncherText -match 'ExitUnattendedLiveUnauthorized = 6'))

Assert-True 'Launch-VeritasLedger.ps1 -SkipGui switch exists (used for -Scheduled paper attach)' `
    ($VeritasLedgerText -match '\[switch\]\$SkipGui')

Assert-True 'Launch-VeritasLedger.ps1 -SkipGui actually guards the GUI resolve/launch block' `
    ($VeritasLedgerText -match 'if \(-not \$SkipGui\.IsPresent\)')

# ---------------------------------------------------------------------------
# Section 2: real subprocess invocations (safe/read-only paths only)
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 2: real subprocess invocations (safe/read-only paths only) ==='

$r1 = Invoke-Launcher -LauncherArgs @('-Scheduled')
Assert-True '-Scheduled with no -Mode: STARTUP_REFUSED text present' ($r1.Output -match 'STARTUP_REFUSED')
Assert-True '-Scheduled with no -Mode: reason=scheduled_mode_requires_explicit_trading_mode present' ($r1.Output -match 'reason=scheduled_mode_requires_explicit_trading_mode')
Assert-True '-Scheduled with no -Mode: exit code 2' ($r1.ExitCode -eq 2)

$r2 = Invoke-Launcher -LauncherArgs @('-Mode', 'Live', '-Scheduled')
Assert-True '-Mode Live -Scheduled: unattended live authority reported BLOCKED' ($r2.Output -match 'unattended live authority')
Assert-True '-Mode Live -Scheduled: LIVE START REFUSED' ($r2.Output -match 'LIVE START REFUSED')
Assert-True '-Mode Live -Scheduled: no_live_order_authority_granted proof line present' ($r2.Output -match 'No live broker orders were enabled\.')
Assert-True '-Mode Live -Scheduled: exit code 6 (unattended live not authorized)' ($r2.ExitCode -eq 6)

$r3 = Invoke-Launcher -LauncherArgs @('-Mode', 'Live', '-CheckOnly')
Assert-True '-Mode Live -CheckOnly: completed without hanging on an interactive prompt' ($null -ne $r3.ExitCode)
Assert-True '-Mode Live -CheckOnly: reports at least one blocked live gate (current repo truth)' ($r3.Output -match 'BLOCKED')
Assert-True '-Mode Live -CheckOnly: reports a concrete ledger patch ID as a blocker' ($r3.Output -match 'LIVE-TRUST-CHAIN-EVIDENCE-SIGNER-01' -or $r3.Output -match 'LIVE-CAPITAL-EXTERNAL-PROOF-01')
Assert-True '-Mode Live -CheckOnly: never claims a live runtime was started' (-not ($r3.Output -match 'runtime was started\.\s*$' -and $r3.Output -notmatch 'No live runtime was started'))
Assert-True '-Mode Live -CheckOnly: explicitly confirms no live runtime was started' ($r3.Output -match 'No live runtime was started\.')
Assert-True '-Mode Live -CheckOnly: exit code is the documented LIVE-blocked/ready set {0,5}' ($r3.ExitCode -eq 0 -or $r3.ExitCode -eq 5)

$r4 = Invoke-Launcher -LauncherArgs @('-Mode', 'Paper', '-CheckOnly')
Assert-True '-Mode Paper -CheckOnly: delegates to and surfaces Launch-VeritasLedger.ps1 -CheckOnly output' ($r4.Output -match 'Veritas Ledger Startup -- CheckOnly')
Assert-True '-Mode Paper -CheckOnly: never mentions arm-execution, clear-halted-run, or disarm-execution' `
    (-not ($r4.Output -match 'arm-execution' -or $r4.Output -match 'clear-halted-run' -or $r4.Output -match 'disarm-execution'))
Assert-True '-Mode Paper -CheckOnly: never mentions start-system' (-not ($r4.Output -match 'start-system'))
Assert-True '-Mode Paper -CheckOnly: completed and returned a numeric exit code' ($null -ne $r4.ExitCode)

# ---------------------------------------------------------------------------
# Section 3: OFFICIAL-DUAL-MODE-LAUNCHER-01-REPAIR-01 proofs still in force
# (env loading / arm contract / DB prerequisites). The REPAIR-01 session-
# refresh-duration proofs (10-13) are removed along with
# Get-AuthoritativeIntradayRefreshDuration, which PAPER-OPS-AUTOFRESH-
# LAUNCHER-INTEGRATION-01 deleted -- the daemon required-universe scheduler
# now owns all session/calendar timing for market-data maintenance, so the
# launcher no longer computes or needs a refresh-loop duration at all.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 3: REPAIR-01 proofs still in force (env / arm / DB prereqs) ==='

# Isolate the full-startup body of Invoke-PaperStartup (after the CheckOnly
# early-return) so arm/db-prereq assertions are scoped correctly and cannot
# accidentally match CheckOnly-branch text.
$FullStartupBody = ($PaperStartupBody -split '# --- Full startup ---')[1]

# --- Environment (proofs 1-3) ---------------------------------------------
Assert-True 'Proof 1: official parent launcher defines its own safe .env.local/.env loader (Import-LauncherEnvironmentFiles)' `
    ($LauncherText -match 'function Import-LauncherEnvironmentFiles' -and $LauncherText -match 'function Import-DotEnvIfPresent' -and $LauncherText -match 'function Parse-DotEnvLine')

Assert-True 'Proof 1: main dispatch calls Import-LauncherEnvironmentFiles before any mode-specific logic runs' `
    ($LauncherText -match '(?s)\$RepoRoot = Get-RepoRoot.*?Import-LauncherEnvironmentFiles -RepoRoot \$RepoRoot.*?if \(\$Scheduled\.IsPresent')

Assert-True 'Proof 1: env loader never prints loaded values (only the source path)' `
    (-not ($LauncherText -match 'Write-(Step|Ok|Warn|Fail).*\$entry\.Value') -and -not ($LauncherText -match 'Write-(Step|Ok|Warn|Fail).*\$value\b.*Loaded'))

Assert-True 'Proof 2: operator-token resolution no longer assumes child-process env propagation (uses Get-EnvValue, not just Process/User on the parent alone)' `
    ($FullStartupBody -match "Get-EnvValue -Name 'MQK_OPERATOR_TOKEN'")

Assert-True 'Proof 3: Get-EnvValue precedence is Process -> User -> Machine (process/file values win, safe fallback order)' `
    ($LauncherText -match "(?s)function Get-EnvValue.*?'Process'.*?'User'.*?'Machine'")

# --- Paper arm (proofs 4-9) -------------------------------------------------
Assert-True 'Proof 4: full (non-CheckOnly) Paper startup always reaches an arm-execution stage, not gated behind -ArmPaper' `
    ($FullStartupBody -match "ActionKey 'arm-execution'" -and -not ($FullStartupBody -match "if\s*\(\s*\`$ArmPaperFlag\s*\)\s*\{[^}]*ActionKey 'arm-execution'"))

Assert-True 'Proof 4/5: -ArmPaper is no longer required for the official full Paper startup dispatch (launcherModeArg is unconditional, not `if ($ArmPaper.IsPresent)`)' `
    ($LauncherText -match "\`$launcherModeArg = 'Observe'" -and -not ($LauncherText -match "\`$launcherModeArg = if \(\`$ArmPaper\.IsPresent\)"))

$ArmSectionBody = ($FullStartupBody -split "Write-Section 'PAPER -- arm")[1]
$ArmSectionBody = ($ArmSectionBody -split "Write-Section 'PAPER -- evidence|Write-Section 'PAPER -- runtime start")[0]
Assert-True 'Proof 5: the arm-establishment section does not branch on -Scheduled (identical contract for interactive and scheduled full startup)' `
    (-not ($ArmSectionBody -match '\$ScheduledFlag'))

Assert-True 'Proof 6: CheckOnly path still never calls Invoke-PaperDbPrerequisites or arm-execution (unchanged from Section 1 guard)' `
    (-not ($CheckOnlyBlockText -match 'Invoke-PaperDbPrerequisites') -and -not ($CheckOnlyBlockText -match "ActionKey 'arm-execution'"))

Assert-True 'Proof 7: launcher refuses success unless authoritative arm_state=="armed"; arm_pending/disarmed_db/halted/unknown are not accepted' `
    ($FullStartupBody -match "if \(\`$finalArmState -ne 'armed'\)" -and $FullStartupBody -match 'return \$script:ExitBackendReconcile' -and -not ($FullStartupBody -match "-or \`$finalArmState -eq 'arm_pending'"))

Assert-True 'Proof 7: arm_pending is explicitly documented as NOT sufficient for launcher success (ambiguous with unreadable DB truth per source)' `
    ($LauncherText -match 'arm_pending.*is returned both when the' -or $LauncherText -match 'is deliberately NOT accepted as success')

Assert-True 'Proof 8/9: launcher still never contains the start-system action_key literal (runtime start stays with the autonomous controller)' `
    (-not ($LauncherText -match "'start-system'" -or $LauncherText -match '"start-system"'))

# --- DB/startup prerequisites (proofs 14-17) --------------------------------
Assert-True 'Proof 14: Paper DB hard fence targets 127.0.0.1:5440/miniquantdesk_paper explicitly' `
    ($LauncherText -match '127\.0\.0\.1:5440/miniquantdesk_paper')

Assert-True 'Proof 15: Docker/container readiness path exists (docker inspect / start / pg_isready against mqk-paper-postgres)' `
    ($LauncherText -match 'function Invoke-PaperDbPrerequisites' -and $LauncherText -match 'docker inspect \$containerName' -and $LauncherText -match 'pg_isready' -and $LauncherText -match "containerName = 'mqk-paper-postgres'")

Assert-True 'Proof 15: full startup invokes the DB-prerequisites stage before delegating to Launch-VeritasLedger.ps1' `
    ($FullStartupBody -match '(?s)Invoke-PaperDbPrerequisites -RepoRoot \$RepoRoot.*?Launch-VeritasLedger\.ps1')

Assert-True 'Proof 16: migration path exists (sqlx/cargo-sqlx migrate run against core-rs\crates\mqk-db\migrations)' `
    ($LauncherText -match 'migrate run' -and $LauncherText -match 'mqk-db\\migrations')

Assert-True 'Proof 17: DB-prerequisite stage never runs in CheckOnly (no docker start / migration mutation on a read-only run)' `
    (-not ($CheckOnlyBlockText -match 'Invoke-PaperDbPrerequisites'))

Assert-True 'Proof 17: paper DB URL constant never targets port 5432 or 5434 (live/test DB ports)' `
    (-not ($LauncherText -match "paperDbUrl = 'postgres://[^']*:543[24]"))

# --- Live (proofs 18-20) ----------------------------------------------------
# Invoke-LiveStartup is unmodified by this integration; proofs 18-20 are the
# same real subprocess invocations already exercised in Section 2 (r2, r3)
# plus the Section 1 static guards -- re-affirmed here for traceability.
Assert-True 'Proof 18/19: -Mode Live -Scheduled remains unattended-live-unauthorized (re-check of r2)' `
    ($r2.Output -match 'unattended live authority' -and $r2.ExitCode -eq 6)

Assert-True 'Proof 20: -Mode Live -CheckOnly still performs zero live order/runtime/DB mutation (re-check of r3)' `
    ($r3.Output -match 'No live runtime was started\.' -and $r3.Output -match 'No live broker orders were enabled\.' -and $r3.Output -match 'No live DB was mutated\.')

# ---------------------------------------------------------------------------
# Section 4: OFFICIAL-DUAL-MODE-LAUNCHER-01-REPAIR-02 Defect A proofs still
# in force (pre-open circularity: daemon bootstrap must use Observe, not
# TradeReady). Defect A's original proof A7 (refresh-loop ordering relative
# to arm) is superseded by Section 5's L1/L2/L3/L12 below, which prove the
# NEW required ordering: required-universe scheduler authority is
# established BEFORE reconcile/halt-recovery/arm (PAPER-OPS-AUTOFRESH-
# LAUNCHER-INTEGRATION-01 mission section 4), not after arm as the prior
# refresh-loop stage was.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 4: REPAIR-02 Defect A proofs (pre-open Observe bootstrap) ==='

Assert-True 'REPAIR-02 Proof A1: official full Paper startup delegates lower-level daemon bootstrap in Observe mode, not TradeReady' `
    ($LauncherText -match "\`$launcherModeArg = 'Observe'" -and -not ($LauncherText -match "\`$launcherModeArg = 'TradeReady'"))

$PreArmFullStartupBody = ($FullStartupBody -split "Write-Section 'PAPER -- arm")[0]
Assert-True 'REPAIR-02 Proof A2: launcher does not require session_in_window truth before it can reach its own arm stage' `
    (-not ($PreArmFullStartupBody -match 'session_in_window'))

Assert-True 'REPAIR-02 Proof A2: launcher does not require overall_ready truth before it can reach its own arm stage' `
    (-not ($PreArmFullStartupBody -match 'overall_ready'))

Assert-True 'REPAIR-02 Proof A2: launcher does not require runtime_start_allowed truth before it can reach its own arm stage' `
    (-not ($PreArmFullStartupBody -match 'runtime_start_allowed'))

Assert-True 'REPAIR-02 Proof A3: Launch-VeritasLedger.ps1 TradeReady semantics are unchanged -- still gate on arm_ready/session_in_window/runtime_start_allowed/overall_ready' `
    ($VeritasLedgerText -match 'arm_ready' -and $VeritasLedgerText -match 'session_in_window' -and
     $VeritasLedgerText -match 'runtime_start_allowed' -and $VeritasLedgerText -match 'overall_ready' -and
     $VeritasLedgerText -match "requireTradeReady = \`$LauncherMode -eq 'TradeReady'")

Assert-True 'REPAIR-02 Proof A3: Launch-VeritasLedger.ps1 -Mode ValidateSet still offers both Observe (default) and TradeReady, unchanged' `
    ($VeritasLedgerText -match "\[ValidateSet\('Observe',\s*'TradeReady'\)\]" -and $VeritasLedgerText -match "\[string\]\`$Mode = 'Observe'")

Assert-True 'REPAIR-02 Proof A4: Paper full startup still always reaches an arm-execution stage (unchanged contract)' `
    ($FullStartupBody -match "ActionKey 'arm-execution'")

Assert-True 'REPAIR-02 Proof A5: arm_state=="armed" remains required before launcher success (unchanged contract)' `
    ($FullStartupBody -match "if \(\`$finalArmState -ne 'armed'\)" -and -not ($FullStartupBody -match "-or \`$finalArmState -eq 'arm_pending'"))

Assert-True 'REPAIR-02 Proof A6: launcher never contains the start-system action_key literal' `
    (-not ($LauncherText -match "'start-system'" -or $LauncherText -match '"start-system"'))

# ---------------------------------------------------------------------------
# Section 5: PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01 proofs -- required-
# universe market-data scheduler is the SOLE ongoing market-data authority.
# L1-L12 per the mission's required minimum coverage. L2-L6/L10 are
# functional: dot-source the real launcher, then shadow its own
# Invoke-JsonGet/Invoke-JsonPost HTTP helpers with mocked responses so
# Start-OrVerifyRequiredUniverseScheduler / Confirm-RequiredUniverseSchedulerOwnership
# run for real against fully deterministic fixture data -- zero real
# daemon/network/DB/order/runtime side effects.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 5: PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01 proofs (required-universe scheduler) ==='

# --- L1: static -- normal Paper uses the required-universe scheduler, and
# that establishment happens BEFORE reconcile/halt-recovery/arm -----------
$RequiredUniverseSectionMatch = [regex]::Match($FullStartupBody, "(?s)Write-Section 'PAPER -- required-universe market-data scheduler.*?(?=Write-Section 'PAPER -- reconciliation)")
Assert-True 'L1: normal Paper startup calls Start-OrVerifyRequiredUniverseScheduler inside the required-universe section' `
    ($RequiredUniverseSectionMatch.Success -and $RequiredUniverseSectionMatch.Value.Contains('Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl $daemonBaseUrl -OperatorToken $operatorToken'))

Assert-True 'L1: required-universe scheduler establishment happens strictly BEFORE the reconciliation hard gate (mission-required ordering)' `
    ($FullStartupBody.IndexOf("Write-Section 'PAPER -- required-universe market-data scheduler") -ge 0 -and
     $FullStartupBody.IndexOf("Write-Section 'PAPER -- reconciliation") -gt $FullStartupBody.IndexOf("Write-Section 'PAPER -- required-universe market-data scheduler"))

Assert-True 'L1: required-universe scheduler establishment happens strictly BEFORE halt recovery' `
    ($FullStartupBody.IndexOf("Write-Section 'PAPER -- halt recovery") -gt $FullStartupBody.IndexOf("Write-Section 'PAPER -- required-universe market-data scheduler"))

Assert-True 'L1: required-universe scheduler establishment happens strictly BEFORE arm' `
    ($FullStartupBody.IndexOf("Write-Section 'PAPER -- arm") -gt $FullStartupBody.IndexOf("Write-Section 'PAPER -- required-universe market-data scheduler"))

Assert-True 'L1: unestablished authority fails closed with ExitDataReadiness before reconcile/arm (no fallthrough to reconcile)' `
    ($RequiredUniverseSectionMatch.Success -and $RequiredUniverseSectionMatch.Value -match '(?s)if \(-not \$ruResult\.Established\)\s*\{.*?return \$script:ExitDataReadiness')

# --- Functional mock harness ------------------------------------------------
# Dot-sourcing is safe: MAIN DISPATCH is guarded by
# `if ($MyInvocation.InvocationName -ne '.')`, so this only defines
# functions -- no daemon start, no Alpaca call, no trading runtime, no exit.
. $Launcher

$script:MockPostResult = $null
$script:MockGetResult  = $null
function Invoke-JsonPost {
    param($Url, $OperatorToken, $Body, $TimeoutSec)
    return $script:MockPostResult
}
function Invoke-JsonGet {
    param($Url, $TimeoutSec)
    return $script:MockGetResult
}
function Reset-RequiredUniverseMocks {
    $script:MockPostResult = $null
    $script:MockGetResult  = $null
}
function New-FakeRequiredUniverseReport {
    param(
        [string]$OverallState = 'ready',
        [bool]$IsTradingDay = $true,
        [string]$MarketDate = '2026-08-12',
        [array]$Requirements = @([pscustomobject]@{ symbol = 'AAPL'; timeframe = '5m'; provider_id = 'alpaca'; freshness_state = 'ready'; blockers = @() })
    )
    [pscustomobject]@{
        market_date    = $MarketDate
        overall_state  = $OverallState
        is_trading_day = $IsTradingDay
        requirements   = $Requirements
        groups         = @()
    }
}

# --- L2: POST request itself fails (unreachable/malformed) -> not
# established, fails closed, never non-fatal ---------------------------------
Reset-RequiredUniverseMocks
$script:MockPostResult = [pscustomobject]@{ StatusCode = $null; Json = $null }
$l2 = Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl 'http://127.0.0.1:8899' -OperatorToken 'fake-token'
Assert-True 'L2: required-universe/start request failure (no HTTP response) -> Established=false, REQUIRED_UNIVERSE_SCHEDULER_START_REQUEST_FAILED' `
    (-not $l2.Established -and $l2.Reason -eq 'REQUIRED_UNIVERSE_SCHEDULER_START_REQUEST_FAILED')

# --- L3: 200 response but overall_state=blocked -> fail before reconcile/arm
Reset-RequiredUniverseMocks
$blockedReq = [pscustomobject]@{ symbol = 'AAPL'; timeframe = '5m'; provider_id = 'alpaca'; freshness_state = 'instrument_registry_invalid'; blockers = @('instrument disabled') }
$script:MockPostResult = [pscustomobject]@{ StatusCode = 200; Json = [pscustomobject]@{ report = (New-FakeRequiredUniverseReport -OverallState 'blocked' -Requirements @($blockedReq)) } }
$l3 = Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl 'http://127.0.0.1:8899' -OperatorToken 'fake-token'
Assert-True 'L3: 200 response with overall_state=blocked -> Established=false, REQUIRED_UNIVERSE_SCHEDULER_BLOCKED, blocker surfaced' `
    (-not $l3.Established -and $l3.Reason -eq 'REQUIRED_UNIVERSE_SCHEDULER_BLOCKED' -and $l3.Detail -match 'AAPL')

# --- L4: 409 + running=true + dry_run=true -> fail (dry-run is not authority)
Reset-RequiredUniverseMocks
$script:MockPostResult = [pscustomobject]@{ StatusCode = 409; Json = [pscustomobject]@{ error = 'already_running' } }
$script:MockGetResult  = [pscustomobject]@{ Ok = $true; Json = [pscustomobject]@{ running = $true; dry_run = $true; report = (New-FakeRequiredUniverseReport) } }
$l4 = Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl 'http://127.0.0.1:8899' -OperatorToken 'fake-token'
Assert-True 'L4: 409 + running=true + dry_run=true -> Established=false, REQUIRED_UNIVERSE_SCHEDULER_BLOCKED_DRY_RUN_OWNER (dry-run owner is not authority)' `
    (-not $l4.Established -and $l4.Reason -eq 'REQUIRED_UNIVERSE_SCHEDULER_BLOCKED_DRY_RUN_OWNER')

# --- L5: 409 + valid running non-dry owner -> continue (verified reuse) ----
Reset-RequiredUniverseMocks
$script:MockPostResult = [pscustomobject]@{ StatusCode = 409; Json = [pscustomobject]@{ error = 'already_running' } }
$script:MockGetResult  = [pscustomobject]@{ Ok = $true; Json = [pscustomobject]@{ running = $true; dry_run = $false; report = (New-FakeRequiredUniverseReport) } }
$l5 = Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl 'http://127.0.0.1:8899' -OperatorToken 'fake-token'
Assert-True 'L5: 409 + verified running non-dry owner -> Established=true, REQUIRED_UNIVERSE_SCHEDULER_VERIFIED_REUSE' `
    ($l5.Established -and $l5.Reason -eq 'REQUIRED_UNIVERSE_SCHEDULER_VERIFIED_REUSE')

# --- L6: non-trading-day / not_applicable -> accepted no-work, not a failure
Reset-RequiredUniverseMocks
$script:MockPostResult = [pscustomobject]@{ StatusCode = 200; Json = [pscustomobject]@{ report = (New-FakeRequiredUniverseReport -OverallState 'not_applicable' -IsTradingDay $false -Requirements @()) } }
$l6 = Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl 'http://127.0.0.1:8899' -OperatorToken 'fake-token'
Assert-True 'L6: overall_state=not_applicable / non-trading-day -> Established=true, REQUIRED_UNIVERSE_NO_WORK_NOT_APPLICABLE (legitimate no-work, never a failure)' `
    ($l6.Established -and $l6.Reason -eq 'REQUIRED_UNIVERSE_NO_WORK_NOT_APPLICABLE')

# --- L7: static -- no Refresh-IntradayMarketData child is started by
# official normal Paper startup (already covered in Section 1, re-affirmed
# here under its L-series identity) -----------------------------------------
Assert-True 'L7: official normal Paper startup never starts Refresh-IntradayMarketData.ps1 as a child process (re-check)' `
    (-not ($FullStartupBody -match 'Refresh-IntradayMarketData') -and -not ($LauncherText -match 'Start-Process'))

# --- L8: static -- -CheckOnly starts neither the scheduler nor the refresh
# child (re-affirmed under its L-series identity) ---------------------------
Assert-True 'L8: -CheckOnly never calls required-universe/start (POST) or starts a refresh child' `
    (-not ($CheckOnlyBlockText -match '/required-universe/start') -and -not ($CheckOnlyBlockText -match 'Start-Process'))
Assert-True 'L8: -CheckOnly may query required-universe/status (GET, read-only) for operator visibility' `
    ($CheckOnlyBlockText -match '/required-universe/status')

# --- L9: static -- -Scheduled Paper uses the same daemon scheduler path as
# interactive (no branch on $ScheduledFlag inside the required-universe
# section, mirroring the existing arm-section proof) -------------------------
Assert-True 'L9: required-universe scheduler section does not branch on -Scheduled (identical contract for interactive and scheduled full startup)' `
    ($RequiredUniverseSectionMatch.Success -and -not ($RequiredUniverseSectionMatch.Value -match '\$ScheduledFlag'))

# --- L10: functional -- a multi-symbol required-universe response is not
# collapsed to a single symbol -----------------------------------------------
Reset-RequiredUniverseMocks
$multiReqs = @(
    [pscustomobject]@{ symbol = 'AAPL'; timeframe = '5m'; provider_id = 'alpaca'; freshness_state = 'ready'; blockers = @() },
    [pscustomobject]@{ symbol = 'MSFT'; timeframe = '5m'; provider_id = 'alpaca'; freshness_state = 'ready'; blockers = @() },
    [pscustomobject]@{ symbol = 'NVDA'; timeframe = '5m'; provider_id = 'alpaca'; freshness_state = 'ready'; blockers = @() }
)
$script:MockPostResult = [pscustomobject]@{ StatusCode = 200; Json = [pscustomobject]@{ report = (New-FakeRequiredUniverseReport -Requirements $multiReqs) } }
$script:MockGetResult  = [pscustomobject]@{ Ok = $true; Json = [pscustomobject]@{ running = $true; dry_run = $false; report = (New-FakeRequiredUniverseReport -Requirements $multiReqs) } }
$l10 = Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl 'http://127.0.0.1:8899' -OperatorToken 'fake-token'
Assert-True 'L10: multi-symbol required-universe response (AAPL/MSFT/NVDA) is not collapsed to a single symbol -- all three requirements preserved' `
    ($l10.Established -and $null -ne $l10.Report -and @($l10.Report.requirements).Count -eq 3 -and
     (@($l10.Report.requirements | ForEach-Object { $_.symbol }) -join ',') -eq 'AAPL,MSFT,NVDA')

Assert-True 'L10: static -- launcher never rebuilds provider grouping in PowerShell (no per-symbol provider-resolution helper in the launcher; the daemon owns it)' `
    (-not ($LauncherText -match 'function Resolve-SymbolRegistryProvider') -and -not ($LauncherText -match 'function Resolve-.*Provider'))

Assert-True 'L10: static -- the required-universe scheduler section itself never collapses the universe to MQK_STRATEGY_SYMBOL/AAPL (scoped to that section, not the whole file, since MAIN DISPATCH legitimately documents MQK_STRATEGY_SYMBOL as a .env.local hint elsewhere)' `
    ($RequiredUniverseSectionMatch.Success -and -not ($RequiredUniverseSectionMatch.Value -match 'MQK_STRATEGY_SYMBOL') -and -not ($RequiredUniverseSectionMatch.Value -match "'AAPL'"))

# --- L11: static -- Live mode never reaches the Paper required-universe
# scheduler route -------------------------------------------------------------
$LiveStartupBody = ($LauncherText -split 'function Invoke-LiveStartup ')[1]
$LiveStartupBody = ($LiveStartupBody -split '\r?\nfunction ', 2)[0]
Assert-True 'L11: Invoke-LiveStartup never references the required-universe route (Live mode never starts/verifies the Paper market-data scheduler)' `
    (-not ($LiveStartupBody -match 'required-universe'))

# --- L12: static -- successful Paper path still verifies arm before
# reporting success (required-universe establishment does not weaken or
# bypass the arm contract; re-affirmed under its L-series identity) ---------
Assert-True 'L12: required-universe establishment happens before arm, and arm_state=="armed" is still required before ExitOk' `
    ($FullStartupBody.IndexOf("Write-Section 'PAPER -- arm") -gt $FullStartupBody.IndexOf("Write-Section 'PAPER -- required-universe market-data scheduler") -and
     $FullStartupBody -match "if \(\`$finalArmState -ne 'armed'\)")

Assert-True 'L12: full-startup function returns ExitOk only after the arm section (arm gates final success, unchanged)' `
    ($FullStartupBody.IndexOf('return $script:ExitOk') -gt $FullStartupBody.IndexOf("Write-Section 'PAPER -- arm"))

# ---------------------------------------------------------------------------
# PAPER-OPS-AUTOFRESH-LAUNCHER-INTEGRATION-01-REPAIR-01: closed-set,
# fail-closed interpretation of overall_state. L13-L16 prove the launcher can
# no longer treat "anything except blocked" as success -- an empty required
# universe on a trading day and any unrecognized/missing state must both fail
# closed, on both the 200 (Start-OrVerifyRequiredUniverseScheduler) and 409
# reuse (Confirm-RequiredUniverseSchedulerOwnership) paths.
# ---------------------------------------------------------------------------

# --- L13: 200 response, overall_state=not_applicable, is_trading_day=true,
# empty required universe -> must fail closed (the verified defect) --------
Reset-RequiredUniverseMocks
$script:MockPostResult = [pscustomobject]@{ StatusCode = 200; Json = [pscustomobject]@{ report = (New-FakeRequiredUniverseReport -OverallState 'not_applicable' -IsTradingDay $true -Requirements @()) } }
$l13 = Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl 'http://127.0.0.1:8899' -OperatorToken 'fake-token'
Assert-True 'L13: overall_state=not_applicable / is_trading_day=true / empty universe -> Established=false, REQUIRED_UNIVERSE_NOT_APPLICABLE_ON_TRADING_DAY (must not continue toward reconcile/arm on a trading day with no required universe)' `
    (-not $l13.Established -and $l13.Reason -eq 'REQUIRED_UNIVERSE_NOT_APPLICABLE_ON_TRADING_DAY')

# --- L14: 200 response, unrecognized overall_state -> fail closed ----------
Reset-RequiredUniverseMocks
$script:MockPostResult = [pscustomobject]@{ StatusCode = 200; Json = [pscustomobject]@{ report = (New-FakeRequiredUniverseReport -OverallState 'mystery_state' -IsTradingDay $true) } }
$l14 = Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl 'http://127.0.0.1:8899' -OperatorToken 'fake-token'
Assert-True 'L14: 200 response with unrecognized overall_state=mystery_state -> Established=false, REQUIRED_UNIVERSE_SCHEDULER_STATE_UNKNOWN' `
    (-not $l14.Established -and $l14.Reason -eq 'REQUIRED_UNIVERSE_SCHEDULER_STATE_UNKNOWN')

# --- L15: 409 reuse, running=true/dry_run=false, but the reused scheduler's
# own report carries an unrecognized overall_state -> fail closed (a running
# scheduler is not sufficient if its report state is unrecognized) ----------
Reset-RequiredUniverseMocks
$script:MockPostResult = [pscustomobject]@{ StatusCode = 409; Json = [pscustomobject]@{ error = 'already_running' } }
$script:MockGetResult  = [pscustomobject]@{ Ok = $true; Json = [pscustomobject]@{ running = $true; dry_run = $false; report = (New-FakeRequiredUniverseReport -OverallState 'mystery_state' -IsTradingDay $true) } }
$l15 = Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl 'http://127.0.0.1:8899' -OperatorToken 'fake-token'
Assert-True 'L15: 409 reuse + running=true + dry_run=false but report overall_state=mystery_state -> Established=false, REQUIRED_UNIVERSE_SCHEDULER_STATE_UNKNOWN' `
    (-not $l15.Established -and $l15.Reason -eq 'REQUIRED_UNIVERSE_SCHEDULER_STATE_UNKNOWN')

# --- L16: report present but overall_state missing/null/blank -> fail
# closed (PowerShell `$null -ne 'blocked'` must not become success) ---------
Reset-RequiredUniverseMocks
$script:MockPostResult = [pscustomobject]@{ StatusCode = 200; Json = [pscustomobject]@{ report = (New-FakeRequiredUniverseReport -OverallState $null -IsTradingDay $true) } }
$l16 = Start-OrVerifyRequiredUniverseScheduler -DaemonBaseUrl 'http://127.0.0.1:8899' -OperatorToken 'fake-token'
Assert-True 'L16: report present with overall_state=null/missing -> Established=false, REQUIRED_UNIVERSE_SCHEDULER_STATE_UNKNOWN (no optimistic fallthrough)' `
    (-not $l16.Established -and $l16.Reason -eq 'REQUIRED_UNIVERSE_SCHEDULER_STATE_UNKNOWN')

# --- L6 positive control re-affirmed: holiday/weekend (is_trading_day=false)
# with an empty required universe remains legitimate no-work, unchanged by
# the REPAIR-01 closed-set fix ------------------------------------------------
Assert-True 'L6 (re-affirmed post-REPAIR-01): overall_state=not_applicable / is_trading_day=false / empty universe still -> Established=true, REQUIRED_UNIVERSE_NO_WORK_NOT_APPLICABLE' `
    ($l6.Established -and $l6.Reason -eq 'REQUIRED_UNIVERSE_NO_WORK_NOT_APPLICABLE')

# --- Real repo: -Mode Paper -CheckOnly (Section 2's r4) created no active
# required-universe scheduler side effect (defense in depth -- CheckOnly
# only ever performed a read-only GET against a local daemon that may not
# even be running on this box) ------------------------------------------------
Assert-True 'Real-repo check: -Mode Paper -CheckOnly (Section 2''s r4) never printed a required-universe/start POST attempt' `
    (-not ($r4.Output -match 'required-universe/start'))

Show-Info ''
if ($Violations -eq 0) {
    Show-Green "=== ALL PROOFS HELD (0 violations) ==="
    exit 0
} else {
    Show-Red "=== $Violations PROOF(S) FAILED ==="
    exit 1
}
