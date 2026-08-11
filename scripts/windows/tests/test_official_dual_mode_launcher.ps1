# =============================================================================
# test_official_dual_mode_launcher.ps1
# OFFICIAL-DUAL-MODE-LAUNCHER-01
#
# Proof for scripts\windows\Start-MiniQuantDesk.ps1: Paper/Live mode
# selection, fail-closed -Scheduled contract, Live confirmation gating,
# and the current-truth LIVE_START_BLOCKED verdict.
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

Assert-True 'Paper full run delegates market-data prep to Prep-PremarketMarketData.ps1 -SymbolsFromIngestPlan' `
    ($LauncherText -match 'Prep-PremarketMarketData\.ps1' -and $LauncherText -match '-SymbolsFromIngestPlan')

Assert-True 'Paper full run delegates intraday refresh to Refresh-IntradayMarketData.ps1' `
    ($LauncherText -match 'Refresh-IntradayMarketData\.ps1')

Assert-True 'Paper full run resolves symbol universe from the daemon ingest-plan route, not a hardcoded default' `
    ($LauncherText -match '/api/v1/market-data/ingest-plan')

Assert-True 'Paper arm path re-checks live_routing_enabled/daemon_mode/adapter_id immediately before arm-execution' `
    ($LauncherText.Contains('$freshStatus.Json.live_routing_enabled -eq $true -or $freshStatus.Json.daemon_mode -ne ''paper'''))

# Scope to Invoke-PaperStartup's body first (function Invoke-OpsAction is
# defined earlier in the file and would otherwise produce a false match).
$PaperStartupBody = ($LauncherText -split 'function Invoke-PaperStartup ')[1]
$CheckOnlyBlockText = ($PaperStartupBody -split '# --- Full startup ---')[0]
Assert-True 'CheckOnly path never calls Invoke-OpsAction / arm-execution / clear-halted-run' `
    (-not ($CheckOnlyBlockText -match 'Invoke-OpsAction'))

Assert-True 'Exit-code map documented in header comment matches the six defined script-scope constants' `
    (($LauncherText -match 'ExitOk = 0') -and ($LauncherText -match 'ExitSafetyRefusal = 2') -and
     ($LauncherText -match 'ExitDataReadiness = 3') -and ($LauncherText -match 'ExitBackendReconcile = 4') -and
     ($LauncherText -match 'ExitLiveBlocked = 5') -and ($LauncherText -match 'ExitUnattendedLiveUnauthorized = 6'))

Assert-True 'Launch-VeritasLedger.ps1 -SkipGui switch exists (used for -Scheduled paper attach)' `
    ($VeritasLedgerText -match '\[switch\]\$SkipGui')

Assert-True 'Launch-VeritasLedger.ps1 -SkipGui actually guards the GUI resolve/launch block' `
    ($VeritasLedgerText -match 'if \(-not \$SkipGui\.IsPresent\)')

# ---------------------------------------------------------------------------
# Section 2: real subprocess invocations of safe, read-only modes
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
# Section 3: OFFICIAL-DUAL-MODE-LAUNCHER-01-REPAIR-01 proofs
# (mix of static source-guard checks and the existing safe real-subprocess
# invocations above, cross-referenced where a Section-1/2 assertion already
# covers a proof point).
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 3: REPAIR-01 proofs (env / arm / session refresh / DB prereqs / Live) ==='

# Isolate the full-startup body of Invoke-PaperStartup (after the CheckOnly
# early-return) so arm/refresh/db-prereq assertions are scoped correctly and
# cannot accidentally match CheckOnly-branch text.
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

# NOTE: this assertion targeted the literal daemon-bootstrap mode value.
# REPAIR-02 changed that value from 'TradeReady' to 'Observe' (see Section 4
# below for the full pre-open-circularity proof) -- the underlying contract
# this proof exists to protect (launcherModeArg is unconditional, never
# `if ($ArmPaper.IsPresent)`-gated) is unchanged and re-asserted here against
# the corrected value.
Assert-True 'Proof 4/5: -ArmPaper is no longer required for the official full Paper startup dispatch (launcherModeArg is unconditional, not `if ($ArmPaper.IsPresent)`)' `
    ($LauncherText -match "\`$launcherModeArg = 'Observe'" -and -not ($LauncherText -match "\`$launcherModeArg = if \(\`$ArmPaper\.IsPresent\)"))

$ArmSectionBody = ($FullStartupBody -split "Write-Section 'PAPER -- arm")[1]
$ArmSectionBody = ($ArmSectionBody -split "Write-Section 'PAPER -- recurring intraday refresh|Write-Section 'PAPER -- evidence|Write-Section 'PAPER -- runtime start")[0]
Assert-True 'Proof 5: the arm-establishment section does not branch on -Scheduled (identical contract for interactive and scheduled full startup)' `
    (-not ($ArmSectionBody -match '\$ScheduledFlag'))

Assert-True 'Proof 6: CheckOnly path still never calls Invoke-PaperDbPrerequisites or arm-execution (unchanged from Section 1 guard)' `
    (-not ($CheckOnlyBlockText -match 'Invoke-PaperDbPrerequisites') -and -not ($CheckOnlyBlockText -match "ActionKey 'arm-execution'"))

Assert-True 'Proof 7: launcher refuses success unless authoritative arm_state=="armed"; arm_pending/disarmed_db/halted/unknown are not accepted' `
    ($FullStartupBody -match "if \(\`$finalArmState -ne 'armed'\)" -and $FullStartupBody -match 'return \$script:ExitBackendReconcile' -and -not ($FullStartupBody -match "-or \`$finalArmState -eq 'arm_pending'"))

Assert-True 'Proof 7: arm_pending is explicitly documented as NOT sufficient for launcher success (ambiguous with unreadable DB truth per source)' `
    ($LauncherText -match 'arm_pending.*is returned both when the' -or $LauncherText -match 'is deliberately NOT accepted as success')

Assert-True 'Proof 8/9: launcher still never contains the start-system action_key literal (runtime start stays with the autonomous controller) -- re-verified after REPAIR-01' `
    (-not ($LauncherText -match "'start-system'" -or $LauncherText -match '"start-system"'))

# --- Session refresh (proofs 10-13) ----------------------------------------
Assert-True 'Proof 10: launcher no longer splits session_stop_utc on a bare colon (dead-field/fragile-parse removed)' `
    (-not ($LauncherText -match "session_stop_utc\s*-split\s*':'"))

Assert-True 'Proof 10: launcher derives refresh duration from the authoritative NYSE-calendar-backed market-data/readiness route, RFC3339-parsed' `
    ($LauncherText -match '/api/v1/market-data/readiness' -and $LauncherText -match 'calendar_coverage_state' -and $LauncherText -match '\[DateTimeOffset\]::Parse')

Assert-True 'Proof 11: refresh window is computed as session close + 15 minutes' `
    ($LauncherText -match '\.AddMinutes\(15\)')

Assert-True 'Proof 12: unavailable authoritative close truth fails closed (returns ExitDataReadiness), not just a warning' `
    ($FullStartupBody -match '(?s)if \(-not \$refreshDuration\.Ok\)\s*\{.*?return \$script:ExitDataReadiness')

Assert-True 'Proof 13: no 1800-second scheduled fallback duration literal remains in the launcher' `
    (-not ($LauncherText -match '=\s*1800\b'))

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
# Invoke-LiveStartup is unmodified by REPAIR-01 (mission section 12: "Do NOT
# expand Live behavior in this repair"); proofs 18-20 are the same real
# subprocess invocations already exercised in Section 2 (r2, r3) plus the
# Section 1 static guards -- re-affirmed here for REPAIR-01 traceability.
Assert-True 'Proof 18/19: -Mode Live -Scheduled remains unattended-live-unauthorized after REPAIR-01 (re-check of r2)' `
    ($r2.Output -match 'unattended live authority' -and $r2.ExitCode -eq 6)

Assert-True 'Proof 20: -Mode Live -CheckOnly still performs zero live order/runtime/DB mutation after REPAIR-01 (re-check of r3)' `
    ($r3.Output -match 'No live runtime was started\.' -and $r3.Output -match 'No live broker orders were enabled\.' -and $r3.Output -match 'No live DB was mutated\.')

# ---------------------------------------------------------------------------
# Section 4: OFFICIAL-DUAL-MODE-LAUNCHER-01-REPAIR-02 proofs -- Defect A
# (pre-open circularity: daemon bootstrap must use Observe, not TradeReady)
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

Assert-True 'REPAIR-02 Proof A4: Paper full startup still always reaches an arm-execution stage (unchanged contract from REPAIR-01 proof 4)' `
    ($FullStartupBody -match "ActionKey 'arm-execution'")

Assert-True 'REPAIR-02 Proof A5: arm_state=="armed" remains required before launcher success (unchanged contract from REPAIR-01 proof 7)' `
    ($FullStartupBody -match "if \(\`$finalArmState -ne 'armed'\)" -and -not ($FullStartupBody -match "-or \`$finalArmState -eq 'arm_pending'"))

Assert-True 'REPAIR-02 Proof A6: launcher never contains the start-system action_key literal (re-verified after REPAIR-02 reordering)' `
    (-not ($LauncherText -match "'start-system'" -or $LauncherText -match '"start-system"'))

Assert-True 'REPAIR-02 Proof A7: reordered sequence starts/reuses the refresh loop after arm verification, before evidence capture/return (per mission section 11)' `
    ($FullStartupBody -match "(?s)final_arm_state = 'armed'\s*\}\s*.*?PAPER -- recurring intraday refresh.*?PAPER -- evidence capture")

# ---------------------------------------------------------------------------
# Section 5: OFFICIAL-DUAL-MODE-LAUNCHER-01-REPAIR-02 proofs -- Defect B
# (intraday refresh loop must be idempotent / ownership-aware)
#
# Sub-section 5a is static source-guard checks. Sub-section 5b dot-sources
# Start-MiniQuantDesk.ps1 (safe: MAIN DISPATCH is guarded by
# `if ($MyInvocation.InvocationName -ne '.')`, so dot-sourcing only defines
# functions -- no daemon start, no Alpaca call, no trading runtime, no exit)
# and exercises the real Get-/Set-IntradayRefreshOwner* functions against a
# disposable temp-repo fixture and a real (but harmless) long-lived
# powershell.exe fixture process, per mission section 16's "safe process
# fixture/mock" instruction.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 5: REPAIR-02 Defect B proofs (intraday refresh ownership) ==='

# --- 5a: static source-guard checks ----------------------------------------
Assert-True 'REPAIR-02/03 Proof B-static: ownership helper functions exist (Get-IntradayRefreshOwnerPath / Get-RefreshOwnerProcessIdentity / Get-IntradayRefreshOwnerState / Set-IntradayRefreshOwnerRecord / Request-IntradayRefreshOwnership)' `
    ($LauncherText -match 'function Get-IntradayRefreshOwnerPath' -and $LauncherText -match 'function Get-RefreshOwnerProcessIdentity' -and
     $LauncherText -match 'function Get-IntradayRefreshOwnerState' -and $LauncherText -match 'function Set-IntradayRefreshOwnerRecord' -and
     $LauncherText -match 'function Request-IntradayRefreshOwnership')

$OwnershipBlockText = ($LauncherText -split 'function Get-IntradayRefreshOwnerPath ')[1]
$OwnershipBlockText = ($OwnershipBlockText -split '# PAPER STARTUP -- reuses the existing accepted')[0]
Assert-True 'REPAIR-02 Proof B11-static: ownership functions never call Stop-Process / kill any process (arbitrary PowerShell process is never terminated)' `
    (-not ($OwnershipBlockText -match 'Stop-Process|\.Kill\('))

Assert-True 'REPAIR-02 Proof B-static: owner record lives under smoke_logs\launcher\paper\ (same untracked-runtime-evidence convention as New-LauncherLog)' `
    ($LauncherText -match 'smoke_logs\\launcher\\paper' -and $LauncherText -match 'intraday_refresh_owner\.json')

Assert-True 'REPAIR-02 Proof B14-static: CheckOnly branch never references the ownership functions (creates no active ownership state)' `
    (-not ($CheckOnlyBlockText -match 'IntradayRefreshOwner'))

$RealOwnerPath = Join-Path $RepoRoot 'smoke_logs\launcher\paper\intraday_refresh_owner.json'
Assert-True 'REPAIR-02 Proof B14-functional: -Mode Paper -CheckOnly (Section 2''s r4) created no active refresh-ownership record in the real repo' `
    (-not (Test-Path $RealOwnerPath))

# --- 5b: functional fixture proofs (dot-sourced functions, disposable temp repo) ---
$FixtureRepo = $null
$FixtureProc = $null
$UnrelatedProc = $null
try {
    . $Launcher

    $FixtureRepo = Join-Path $env:TEMP ("mqk_launcher_test_repo_" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $FixtureRepo | Out-Null
    $fixtureRefreshScript = Join-Path $FixtureRepo 'Refresh-IntradayMarketData.ps1'
    Set-Content -Path $fixtureRefreshScript -Value 'Start-Sleep -Seconds 90' -Encoding UTF8

    # A real long-lived powershell.exe process whose command line references
    # Refresh-IntradayMarketData.ps1 -- stands in for the actual background
    # refresh child without touching Alpaca or any daemon.
    $FixtureProc = Start-Process -FilePath 'powershell.exe' `
        -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $fixtureRefreshScript) `
        -WindowStyle Hidden -PassThru
    Start-Sleep -Milliseconds 800

    $sym = @('AAPL', 'MSFT')
    $tf = '5m'
    $port = 5440
    $mktDate = '2026-08-10'

    $before = Get-IntradayRefreshOwnerState -RepoRoot $FixtureRepo -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate
    Assert-True 'REPAIR-02 Proof B7: with no owner record yet, state is not reusable (first invocation would launch exactly one owner)' `
        (-not $before.Reusable)

    $ownerPath = Set-IntradayRefreshOwnerRecord -RepoRoot $FixtureRepo -ProcessId $FixtureProc.Id -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate
    Assert-True 'REPAIR-02 Proof B7: Set-IntradayRefreshOwnerRecord writes exactly one owner record at the expected path' `
        (Test-Path $ownerPath)

    $sameScope = Get-IntradayRefreshOwnerState -RepoRoot $FixtureRepo -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate
    Assert-True 'REPAIR-02 Proof B8: second same-scope invocation reuses the existing owner (Reusable=true, matching live pid)' `
        ($sameScope.Reusable -and [int]$sameScope.Record.pid -eq $FixtureProc.Id)

    $mismatchedSymbols = Get-IntradayRefreshOwnerState -RepoRoot $FixtureRepo -Symbols @('TSLA') -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate
    $mismatchedTimeframe = Get-IntradayRefreshOwnerState -RepoRoot $FixtureRepo -Symbols $sym -Timeframe '1m' -PaperDbPort $port -MarketDate $mktDate
    $mismatchedDate = Get-IntradayRefreshOwnerState -RepoRoot $FixtureRepo -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate '2026-08-11'
    Assert-True 'REPAIR-02 Proof B10: mismatched symbol scope is not silently treated as matching' (-not $mismatchedSymbols.Reusable)
    Assert-True 'REPAIR-02 Proof B10: mismatched timeframe scope is not silently treated as matching' (-not $mismatchedTimeframe.Reusable)
    Assert-True 'REPAIR-02 Proof B10: mismatched market-date scope is not silently treated as matching' (-not $mismatchedDate.Reusable)

    $deadPid = 999999
    Set-IntradayRefreshOwnerRecord -RepoRoot $FixtureRepo -ProcessId $deadPid -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate | Out-Null
    $deadState = Get-IntradayRefreshOwnerState -RepoRoot $FixtureRepo -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate
    Assert-True 'REPAIR-02 Proof B9: a stale/dead recorded pid is not reused (one replacement would be launched instead)' `
        (-not $deadState.Reusable)

    # An unrelated real powershell.exe process (command line does NOT reference
    # Refresh-IntradayMarketData.ps1). Even if a stale/forged owner record
    # pointed at it, it must never be treated as reusable, and -- critically
    # -- it must never be killed by any ownership check.
    $UnrelatedProc = Start-Process -FilePath 'powershell.exe' `
        -ArgumentList @('-NoProfile', '-Command', 'Start-Sleep -Seconds 90') `
        -WindowStyle Hidden -PassThru
    Start-Sleep -Milliseconds 800
    Set-IntradayRefreshOwnerRecord -RepoRoot $FixtureRepo -ProcessId $UnrelatedProc.Id -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate | Out-Null
    $unrelatedState = Get-IntradayRefreshOwnerState -RepoRoot $FixtureRepo -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate
    Assert-True 'REPAIR-02 Proof B11: an unrelated PowerShell process (no matching command line) is not treated as a valid owner' `
        (-not $unrelatedState.Reusable)
    Start-Sleep -Milliseconds 200
    Assert-True 'REPAIR-02 Proof B11-functional: the unrelated PowerShell process was never killed by the ownership check' `
        ($null -ne (Get-Process -Id $UnrelatedProc.Id -ErrorAction SilentlyContinue))

    $ownerJson = Get-Content -Path $ownerPath -Raw
    $ownerObj = $ownerJson | ConvertFrom-Json
    $allowedKeys = @('pid', 'started_at_utc', 'market_date', 'symbols', 'timeframe', 'paper_db_port', 'repo_root')
    $actualKeys = @($ownerObj.PSObject.Properties.Name)
    $unexpectedKeys = @($actualKeys | Where-Object { $allowedKeys -notcontains $_ })
    Assert-True 'REPAIR-02 Proof B12: owner record contains only the documented non-secret fields, nothing else' `
        ($unexpectedKeys.Count -eq 0)
    Assert-True 'REPAIR-02 Proof B12: owner record content contains no secret-shaped strings (token/password/secret/api_key/bearer)' `
        (-not ($ownerJson -match '(?i)token|password|secret|api_key|bearer'))

    $trackedOwnerFiles = @(& git -C $RepoRoot ls-files 'smoke_logs' 2>&1 | Where-Object { $_ -match 'intraday_refresh_owner' })
    Assert-True 'REPAIR-02 Proof B13: the owner-record path is not tracked by git (untracked runtime evidence, same as other smoke_logs\launcher\* state)' `
        ($trackedOwnerFiles.Count -eq 0)
}
finally {
    if ($null -ne $FixtureProc) { Stop-Process -Id $FixtureProc.Id -Force -ErrorAction SilentlyContinue }
    if ($null -ne $UnrelatedProc) { Stop-Process -Id $UnrelatedProc.Id -Force -ErrorAction SilentlyContinue }
    if ($null -ne $FixtureRepo -and (Test-Path $FixtureRepo)) { Remove-Item -Recurse -Force $FixtureRepo -ErrorAction SilentlyContinue }
}

# ---------------------------------------------------------------------------
# Section 6: OFFICIAL-DUAL-MODE-LAUNCHER-01-REPAIR-03 proofs -- process
# identity contract (four distinguishable states, fail-closed on ambiguity),
# atomic single-owner lock (bounded, abandoned-mutex-aware, release-in-
# finally, mandatory post-lock re-read), and authoritative market_date.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 6: REPAIR-03 proofs (identity states / atomic lock / market_date) ==='

# --- 6a: static source-guard checks ----------------------------------------
Assert-True 'REPAIR-03 Proof 1-4-static: Get-RefreshOwnerProcessIdentity returns all four distinguishable identity states' `
    ($LauncherText.Contains("return 'dead'") -and $LauncherText.Contains("return 'wrong_process'") -and
     $LauncherText.Contains("return 'identity_unavailable'") -and $LauncherText.Contains("return 'verified_refresh_owner'"))

$IdentityFnBody = ($LauncherText -split 'function Get-RefreshOwnerProcessIdentity ')[1]
$IdentityFnBody = ($IdentityFnBody -split 'function Get-IntradayRefreshOwnerLockName')[0]
Assert-True 'REPAIR-03 Proof 4-static: a CIM/WMI query failure returns identity_unavailable directly from the catch block (never falls through to a weaker process-name-only verdict)' `
    ($IdentityFnBody.Contains("} catch {`n        return 'identity_unavailable'"))

Assert-True 'REPAIR-03 Proof 4-static: identity resolution never returns a bare boolean -- fully replaced by explicit string states, so identity_unavailable can never be silently collapsed into a truthy "reusable" verdict' `
    (-not ($IdentityFnBody -match 'return \$true|return \$false'))

Assert-True 'REPAIR-03 Proof 5-static: Get-IntradayRefreshOwnerState fails closed (Reusable stays false, Disposition=identity_unavailable) and never falls through to the scope-matching branch for that disposition' `
    ($LauncherText.Contains("if (`$identity -eq 'identity_unavailable') {`n        `$result.Disposition = 'identity_unavailable'"))

Assert-True 'REPAIR-03 Proof 5-static: no process is ever killed anywhere in the ownership block, including the new lock/identity/start-proof code (re-check of Section 5a''s whole-block guard)' `
    (-not ($OwnershipBlockText -match 'Stop-Process|\.Kill\('))

Assert-True 'REPAIR-03 Proof 6-static: a deterministic named cross-process Mutex guards owner acquisition (not a fragile text-file existence check as the sole lock)' `
    ($OwnershipBlockText.Contains('System.Threading.Mutex') -and $OwnershipBlockText.Contains('MiniQuantDeskV4-Paper-IntradayRefreshOwner'))

Assert-True 'REPAIR-03 Proof 6-static: lock name is scoped Local\ (documented session-scoped choice)' `
    ($OwnershipBlockText.Contains('Local\MiniQuantDeskV4-Paper-IntradayRefreshOwner-'))

Assert-True 'REPAIR-03 Proof 6-static: the owner record is explicitly re-read AFTER lock acquisition inside Request-IntradayRefreshOwnership (mandatory second read)' `
    ($OwnershipBlockText.Contains('Mandatory re-read AFTER lock acquisition') -and $OwnershipBlockText.Contains('$ownerState = Get-IntradayRefreshOwnerState -RepoRoot $RepoRoot'))

Assert-True 'REPAIR-03 Proof 7-static: lock acquisition is bounded (WaitOne with a timeout, not an unbounded wait) and fails closed with REFRESH_OWNER_LOCK_TIMEOUT' `
    ($OwnershipBlockText.Contains('$mutex.WaitOne($LockTimeoutMilliseconds)') -and $OwnershipBlockText.Contains('REFRESH_OWNER_LOCK_TIMEOUT'))

Assert-True 'REPAIR-03 Proof 7-static: the mutex is released in a finally block' `
    ($OwnershipBlockText.Contains('finally {') -and $OwnershipBlockText.Contains('$mutex.ReleaseMutex()') -and $OwnershipBlockText.Contains('$mutex.Dispose()'))

Assert-True 'REPAIR-03 Proof 7-static: an abandoned mutex is explicitly handled (AbandonedMutexException caught and treated as acquired, never left to crash the launcher)' `
    ($OwnershipBlockText.Contains('[System.Threading.AbandonedMutexException]'))

Assert-True 'REPAIR-03 Proof 8-static: after Start-Process, a bounded alive-check runs and the owner record is written only when the child survives it (no false-green record on immediate exit)' `
    ($OwnershipBlockText.Contains('Start-Sleep -Milliseconds $StartAliveCheckMilliseconds') -and $OwnershipBlockText.Contains('$stillAlive') -and $OwnershipBlockText.Contains("Outcome = 'START_FAILED'"))

Assert-True 'REPAIR-03 Proof 9-static: market_date is sourced from GET /api/v1/market-data/readiness (market_date field), not the machine-local calendar date' `
    ($LauncherText.Contains('$marketDateStr = $readiness.Json.market_date') -and $FullStartupBody.Contains('$marketDateLabel = $refreshDuration.MarketDate'))

Assert-True 'REPAIR-03 Proof 9-static: the old machine-local Get-Date market_date assignment is gone from the full-startup refresh section' `
    (-not ($FullStartupBody -match "\`$marketDateLabel = Get-Date -Format 'yyyy-MM-dd'"))

Assert-True 'REPAIR-03 Proof 9-static: authoritative refresh duration is refused (Ok=false) when market_date is blank, same fail-closed treatment as session_close_utc/calendar_coverage_state' `
    ($LauncherText.Contains('IsNullOrWhiteSpace($marketDateStr)'))

Assert-True 'REPAIR-03 Proof 10-static (order unchanged): full Paper startup still reaches arm before the refresh/ownership section' `
    ($FullStartupBody -match "(?s)final_arm_state = 'armed'\s*\}\s*.*?PAPER -- recurring intraday refresh.*?Request-IntradayRefreshOwnership")

Assert-True 'REPAIR-03 Proof 13-static: CheckOnly branch never references Request-IntradayRefreshOwnership (creates no lock, no lease, no owner record, no refresh process)' `
    (-not ($CheckOnlyBlockText -match 'Request-IntradayRefreshOwnership'))

# --- 6b: functional fixture proofs (dot-sourced functions, disposable temp repos) ---
. $Launcher

# --- Identity states: dead / wrong_process / verified_refresh_owner --------
$DeadIdentity = Get-RefreshOwnerProcessIdentity -ProcessId 999999
Assert-True 'REPAIR-03 Proof 1: a dead PID resolves to disposition "dead"' ($DeadIdentity -eq 'dead')

$WrongProc = $null
try {
    $WrongProc = Start-Process -FilePath 'powershell.exe' -ArgumentList @('-NoProfile', '-Command', 'Start-Sleep -Seconds 30') -WindowStyle Hidden -PassThru
    Start-Sleep -Milliseconds 700
    $WrongIdentity = Get-RefreshOwnerProcessIdentity -ProcessId $WrongProc.Id
    Assert-True 'REPAIR-03 Proof 2: a live PowerShell process whose command line does not reference the refresh script resolves to disposition "wrong_process"' `
        ($WrongIdentity -eq 'wrong_process')
    Assert-True 'REPAIR-03 Proof 2/11: the wrong_process process was never killed by the identity check' `
        ($null -ne (Get-Process -Id $WrongProc.Id -ErrorAction SilentlyContinue))
} finally {
    if ($null -ne $WrongProc) { Stop-Process -Id $WrongProc.Id -Force -ErrorAction SilentlyContinue }
}

$VerifiedRepo = $null
$VerifiedProc = $null
try {
    $VerifiedRepo = Join-Path $env:TEMP ("mqk_launcher_repair03_verified_" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $VerifiedRepo | Out-Null
    $verifiedFixtureScript = Join-Path $VerifiedRepo 'Refresh-IntradayMarketData.ps1'
    Set-Content -Path $verifiedFixtureScript -Value 'Start-Sleep -Seconds 30' -Encoding UTF8
    $VerifiedProc = Start-Process -FilePath 'powershell.exe' `
        -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $verifiedFixtureScript) `
        -WindowStyle Hidden -PassThru
    Start-Sleep -Milliseconds 800
    $VerifiedIdentity = Get-RefreshOwnerProcessIdentity -ProcessId $VerifiedProc.Id
    Assert-True 'REPAIR-03 Proof 3: a live PowerShell process whose command line references Refresh-IntradayMarketData.ps1 resolves to disposition "verified_refresh_owner"' `
        ($VerifiedIdentity -eq 'verified_refresh_owner')
} finally {
    if ($null -ne $VerifiedProc) { Stop-Process -Id $VerifiedProc.Id -Force -ErrorAction SilentlyContinue }
    if ($null -ne $VerifiedRepo -and (Test-Path $VerifiedRepo)) { Remove-Item -Recurse -Force $VerifiedRepo -ErrorAction SilentlyContinue }
}

# --- identity_unavailable / fail-closed proofs (CIM shadowed to fail so the
# real result is deterministic rather than depending on this box's WMI health) ---
$CimFailProc = $null
$CimFailRepo = $null
try {
    $CimFailProc = Start-Process -FilePath 'powershell.exe' -ArgumentList @('-NoProfile', '-Command', 'Start-Sleep -Seconds 30') -WindowStyle Hidden -PassThru
    Start-Sleep -Milliseconds 700

    function Get-CimInstance { throw 'CIM/WMI unavailable (REPAIR-03 test fixture)' }
    try {
        $UnavailableIdentity = Get-RefreshOwnerProcessIdentity -ProcessId $CimFailProc.Id
        Assert-True 'REPAIR-03 Proof 4: a CIM/WMI failure resolves to disposition "identity_unavailable", never "verified_refresh_owner"' `
            ($UnavailableIdentity -eq 'identity_unavailable')
        Assert-True 'REPAIR-03 Proof 4/11: the ambiguous process was never killed while its identity was unprovable' `
            ($null -ne (Get-Process -Id $CimFailProc.Id -ErrorAction SilentlyContinue))

        $CimFailRepo = Join-Path $env:TEMP ("mqk_launcher_repair03_unproven_" + [guid]::NewGuid().ToString('N'))
        New-Item -ItemType Directory -Force -Path $CimFailRepo | Out-Null
        $sym = @('AAPL'); $tf = '5m'; $port = 5440; $mktDate = '2026-08-10'
        Set-IntradayRefreshOwnerRecord -RepoRoot $CimFailRepo -ProcessId $CimFailProc.Id -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate | Out-Null

        $unprovenState = Get-IntradayRefreshOwnerState -RepoRoot $CimFailRepo -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate
        Assert-True 'REPAIR-03 Proof 5: Get-IntradayRefreshOwnerState reports Disposition=identity_unavailable and Reusable=false when identity cannot be proven' `
            ($unprovenState.Disposition -eq 'identity_unavailable' -and -not $unprovenState.Reusable)

        $ownershipUnproven = Request-IntradayRefreshOwnership -RepoRoot $CimFailRepo -Symbols $sym -Timeframe $tf -PaperDbPort $port -MarketDate $mktDate -DurationSeconds 60
        Assert-True 'REPAIR-03 Proof 4/5: Request-IntradayRefreshOwnership returns IDENTITY_UNPROVEN (fails closed) instead of reusing or starting a replacement' `
            ($ownershipUnproven.Outcome -eq 'IDENTITY_UNPROVEN')

        $recordAfter = Get-Content -Path (Get-IntradayRefreshOwnerPath -RepoRoot $CimFailRepo) -Raw | ConvertFrom-Json
        Assert-True 'REPAIR-03 Proof 4/5: the ambiguous owner record was left completely unchanged (no replacement process started, no record overwritten)' `
            ([int]$recordAfter.pid -eq $CimFailProc.Id)
    } finally {
        Remove-Item Function:\Get-CimInstance -ErrorAction SilentlyContinue
    }
} finally {
    if ($null -ne $CimFailProc) { Stop-Process -Id $CimFailProc.Id -Force -ErrorAction SilentlyContinue }
    if ($null -ne $CimFailRepo -and (Test-Path $CimFailRepo)) { Remove-Item -Recurse -Force $CimFailRepo -ErrorAction SilentlyContinue }
}

# --- Atomic lock: bounded timeout fails closed, no child created -----------
# NOTE: Windows named mutexes have THREAD affinity, not object-handle
# affinity -- if the SAME thread that holds the mutex calls WaitOne() again
# (even via a brand-new Mutex .NET object for the same name), it reacquires
# recursively and returns immediately instead of blocking. The external
# holder therefore has to live on a genuinely different thread/process
# (a background job) for this to be a real contention proof.
$LockTimeoutRepo = $null
$HolderJob = $null
try {
    $LockTimeoutRepo = Join-Path $env:TEMP ("mqk_launcher_repair03_locktimeout_" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $LockTimeoutRepo | Out-Null
    $lockName = Get-IntradayRefreshOwnerLockName -RepoRoot $LockTimeoutRepo

    $HolderJob = Start-Job -ScriptBlock {
        param($LockName)
        $m = New-Object System.Threading.Mutex($false, $LockName)
        $null = $m.WaitOne()
        Start-Sleep -Seconds 8
        $m.ReleaseMutex()
        $m.Dispose()
    } -ArgumentList $lockName
    Start-Sleep -Milliseconds 1500
    Assert-True 'REPAIR-03 Proof 7 setup: background holder job acquired the scope lock on a separate process/thread' `
        ($HolderJob.State -eq 'Running')

    $timeoutResult = Request-IntradayRefreshOwnership -RepoRoot $LockTimeoutRepo -Symbols @('AAPL') -Timeframe '5m' -PaperDbPort 5440 -MarketDate '2026-08-10' -DurationSeconds 60 -LockTimeoutMilliseconds 800
    Assert-True 'REPAIR-03 Proof 7: lock acquisition that cannot complete within the bounded timeout returns LOCK_TIMEOUT' `
        ($timeoutResult.Outcome -eq 'LOCK_TIMEOUT')
    Assert-True 'REPAIR-03 Proof 7: a lock-timeout run never starts a refresh process or writes an owner record' `
        (-not (Test-Path (Get-IntradayRefreshOwnerPath -RepoRoot $LockTimeoutRepo)))
} finally {
    if ($null -ne $HolderJob) {
        Wait-Job -Job $HolderJob -Timeout 15 | Out-Null
        Remove-Job -Job $HolderJob -Force -ErrorAction SilentlyContinue
    }
    if ($null -ne $LockTimeoutRepo -and (Test-Path $LockTimeoutRepo)) { Remove-Item -Recurse -Force $LockTimeoutRepo -ErrorAction SilentlyContinue }
}

# --- Atomic lock: released in finally even when the critical section throws.
# A malformed non-numeric paper_db_port field forces a real, uncaught cast
# exception inside the locked scope-matching check -- a realistic corrupt-
# record scenario, not a synthetic test-only hook.
$ExceptionRepo = $null
$ExceptionProc = $null
try {
    $ExceptionRepo = Join-Path $env:TEMP ("mqk_launcher_repair03_exception_" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $ExceptionRepo | Out-Null
    $exceptionFixtureScript = Join-Path $ExceptionRepo 'Refresh-IntradayMarketData.ps1'
    Set-Content -Path $exceptionFixtureScript -Value 'Start-Sleep -Seconds 30' -Encoding UTF8
    $ExceptionProc = Start-Process -FilePath 'powershell.exe' `
        -ArgumentList @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $exceptionFixtureScript) `
        -WindowStyle Hidden -PassThru
    Start-Sleep -Milliseconds 800

    $sym = @('AAPL'); $tf = '5m'; $mktDate = '2026-08-10'
    $badRecord = [ordered]@{
        pid = $ExceptionProc.Id; started_at_utc = (Get-Date).ToUniversalTime().ToString('o')
        market_date = $mktDate; symbols = $sym; timeframe = $tf; paper_db_port = 'notanumber'; repo_root = $ExceptionRepo
    }
    $badOwnerPath = Get-IntradayRefreshOwnerPath -RepoRoot $ExceptionRepo
    ($badRecord | ConvertTo-Json -Depth 4) | Set-Content -Path $badOwnerPath -Encoding UTF8

    $threw = $false
    try {
        Request-IntradayRefreshOwnership -RepoRoot $ExceptionRepo -Symbols $sym -Timeframe $tf -PaperDbPort 5440 -MarketDate $mktDate -DurationSeconds 60 | Out-Null
    } catch {
        $threw = $true
    }
    Assert-True 'REPAIR-03 Proof 9 setup: the malformed owner record forced a real exception inside the locked critical section (proves the release-in-finally path actually ran, not a no-op)' $threw

    # NOTE: re-acquiring from the SAME thread that (maybe) still holds the
    # mutex would trivially succeed either way (Windows named mutexes are
    # thread-affine/recursive) and would not actually prove the release
    # happened. Verify from a background job (a genuinely different thread)
    # instead: it can only acquire within the bounded timeout if the throwing
    # call's `finally` truly released the mutex.
    $lockName = Get-IntradayRefreshOwnerLockName -RepoRoot $ExceptionRepo
    $verifyJob = Start-Job -ScriptBlock {
        param($LockName)
        $m = New-Object System.Threading.Mutex($false, $LockName)
        $acquired = $m.WaitOne(3000)
        if ($acquired) { $m.ReleaseMutex() }
        $m.Dispose()
        return $acquired
    } -ArgumentList $lockName
    $null = Wait-Job -Job $verifyJob -Timeout 10
    $reacquired = Receive-Job -Job $verifyJob -ErrorAction SilentlyContinue
    Remove-Job -Job $verifyJob -Force -ErrorAction SilentlyContinue
    Assert-True 'REPAIR-03 Proof 9: the ownership lock is released in `finally` even when the critical section throws (a different thread can acquire it immediately afterward)' `
        ($reacquired -eq $true)
} finally {
    if ($null -ne $ExceptionProc) { Stop-Process -Id $ExceptionProc.Id -Force -ErrorAction SilentlyContinue }
    if ($null -ne $ExceptionRepo -and (Test-Path $ExceptionRepo)) { Remove-Item -Recurse -Force $ExceptionRepo -ErrorAction SilentlyContinue }
}

# --- Concurrency proof (mission section 16): two real PowerShell processes,
# same owner scope, same acquisition function. Expected: exactly one refresh
# process and one owner record survive; the second caller re-reads the owner
# after acquiring the lock and reuses it.
$ConcurrencyRepo = $null
$Job1 = $null
$Job2 = $null
try {
    $ConcurrencyRepo = Join-Path $env:TEMP ("mqk_launcher_repair03_concurrency_" + [guid]::NewGuid().ToString('N'))
    $concurrencyScriptsDir = Join-Path $ConcurrencyRepo 'scripts\windows'
    New-Item -ItemType Directory -Force -Path $concurrencyScriptsDir | Out-Null
    $concurrencyRefreshScript = Join-Path $concurrencyScriptsDir 'Refresh-IntradayMarketData.ps1'
    Set-Content -Path $concurrencyRefreshScript -Value 'Start-Sleep -Seconds 90' -Encoding UTF8

    $sym = @('AAPL', 'MSFT'); $tf = '5m'; $port = 5440; $mktDate = '2026-08-10'

    $callerScriptBlock = {
        param($LauncherPath, $RepoRoot, $Symbols, $Timeframe, $Port, $MarketDate)
        . $LauncherPath
        Request-IntradayRefreshOwnership -RepoRoot $RepoRoot -Symbols $Symbols -Timeframe $Timeframe -PaperDbPort $Port -MarketDate $MarketDate -DurationSeconds 90
    }

    $Job1 = Start-Job -ScriptBlock $callerScriptBlock -ArgumentList $Launcher, $ConcurrencyRepo, $sym, $tf, $port, $mktDate
    $Job2 = Start-Job -ScriptBlock $callerScriptBlock -ArgumentList $Launcher, $ConcurrencyRepo, $sym, $tf, $port, $mktDate

    $null = Wait-Job -Job $Job1, $Job2 -Timeout 60
    $Result1 = Receive-Job -Job $Job1 -ErrorAction SilentlyContinue
    $Result2 = Receive-Job -Job $Job2 -ErrorAction SilentlyContinue

    Assert-True 'REPAIR-03 Concurrency proof: both concurrent callers completed (no hang, no unhandled exception)' `
        ($null -ne $Result1 -and $null -ne $Result2)

    $outcomes = @($Result1.Outcome, $Result2.Outcome) | Sort-Object
    Assert-True 'REPAIR-03 Concurrency proof: outcomes are exactly one STARTED and one REUSED -- never two STARTED (no duplicate owner)' `
        (($outcomes -join ',') -eq 'REUSED,STARTED')

    $startedResult = if ($Result1.Outcome -eq 'STARTED') { $Result1 } else { $Result2 }
    $reusedResult = if ($Result1.Outcome -eq 'REUSED') { $Result1 } else { $Result2 }
    Assert-True 'REPAIR-03 Concurrency proof: the REUSED caller re-read the owner record after acquiring the lock and saw the pid the STARTED caller just wrote (proves the mandatory post-lock re-read, not a stale pre-lock snapshot)' `
        ($null -ne $startedResult -and $null -ne $reusedResult -and [int]$reusedResult.Pid -eq [int]$startedResult.Pid)

    $refreshProcessCount = @(Get-CimInstance -ClassName Win32_Process -Filter "Name = 'powershell.exe'" -ErrorAction SilentlyContinue |
        Where-Object { $_.CommandLine -and $_.CommandLine.Contains($concurrencyRefreshScript) }).Count
    Assert-True 'REPAIR-03 Concurrency proof: refresh_process_count=1 (exactly one fixture refresh process exists for this scope)' `
        ($refreshProcessCount -eq 1)

    $ownerRecordFiles = @(Get-ChildItem -Path (Join-Path $ConcurrencyRepo 'smoke_logs\launcher\paper') -Filter 'intraday_refresh_owner.json' -ErrorAction SilentlyContinue)
    Assert-True 'REPAIR-03 Concurrency proof: owner_record_count=1 (exactly one owner record file exists for this scope)' `
        ($ownerRecordFiles.Count -eq 1)

    if ($null -ne $startedResult -and $startedResult.Pid) {
        Stop-Process -Id $startedResult.Pid -Force -ErrorAction SilentlyContinue
    }
} finally {
    if ($null -ne $Job1) { Remove-Job -Job $Job1 -Force -ErrorAction SilentlyContinue }
    if ($null -ne $Job2) { Remove-Job -Job $Job2 -Force -ErrorAction SilentlyContinue }
    if ($null -ne $ConcurrencyRepo -and (Test-Path $ConcurrencyRepo)) { Remove-Item -Recurse -Force $ConcurrencyRepo -ErrorAction SilentlyContinue }
}

Show-Info ''
if ($Violations -eq 0) {
    Show-Green "=== ALL PROOFS HELD (0 violations) ==="
    exit 0
} else {
    Show-Red "=== $Violations PROOF(S) FAILED ==="
    exit 1
}
