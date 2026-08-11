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

Assert-True 'Proof 4/5: -ArmPaper is no longer required for the official full Paper startup dispatch (launcherModeArg is unconditional, not `if ($ArmPaper.IsPresent)`)' `
    ($LauncherText -match "\`$launcherModeArg = 'TradeReady'" -and -not ($LauncherText -match "\`$launcherModeArg = if \(\`$ArmPaper\.IsPresent\)"))

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

Show-Info ''
if ($Violations -eq 0) {
    Show-Green "=== ALL PROOFS HELD (0 violations) ==="
    exit 0
} else {
    Show-Red "=== $Violations PROOF(S) FAILED ==="
    exit 1
}
