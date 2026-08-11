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

Show-Info ''
if ($Violations -eq 0) {
    Show-Green "=== ALL PROOFS HELD (0 violations) ==="
    exit 0
} else {
    Show-Red "=== $Violations PROOF(S) FAILED ==="
    exit 1
}
