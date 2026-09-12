# =============================================================================
# PAPER-OPERATOR-STARTUP-LAUNCHER-01
# Script guard: test_launch_veritas_ledger.ps1
#
# Static assertions against scripts\windows\Launch-VeritasLedger.ps1's
# -CheckOnly mode (read-only startup status report) and the preserved
# normal desktop-launch path. No daemon, no DB, no live calls, no
# .env.local required. All checks are read-only source inspections.
#
# Guard assertions:
#   LVL01  Script exists at scripts\windows\Launch-VeritasLedger.ps1
#   LVL02  Script supports -CheckOnly switch
#   LVL03  Normal launch path preserved (daemon + GUI start untouched)
#   LVL04  -CheckOnly checks .env.local presence only (Test-Path), never reads its contents
#   LVL05  Script does NOT call any Alpaca/broker HTTP endpoint
#   LVL06  Script does NOT submit, replace, or cancel orders
#   LVL07  Script does NOT call clear-halted-run
#   LVL08  Script does NOT call flatten-paper-positions (or any flatten action)
#   LVL09  Script does NOT reference a Discord webhook
#   LVL10  Script does NOT issue INSERT/UPDATE/DELETE/DROP SQL
#   LVL11  -CheckOnly DB checks are SELECT-only (md_bars count + sys_arm_state)
#   LVL12  -CheckOnly reports persisted sys_arm_state
#   LVL13  -CheckOnly daemon health check is fail-soft (try/catch -> $null)
#   LVL14  -CheckOnly prints a "Next action" recommendation
#   LVL15  No new desktop-shortcut target introduced (Start-PaperOperatorConsole.ps1 not created)
#   LVL16  -CheckOnly branch (with exit) runs before operator-token resolution and -ArmPaper
#   LVL17  -CheckOnly does not invoke smoke runners (mention-only in Next action text)
#   LVL27  -CheckOnly Paper DB port mismatch message does not claim a false
#          "verify" action is needed (M1-PAPER-READINESS-WAVE-01)
#   LVL28  -CheckOnly reachable-daemon branch compares arm state to
#          DISARMED, not the unreachable HALTED literal (M1-PAPER-READINESS-WAVE-01)
#   LVL29  -CheckOnly DISARMED message does not claim automatic recovery
#          on normal startup (M1-PAPER-READINESS-WAVE-01)
#   B2-01..B2-05  Set-LauncherEnvironment hard-fences MQK_DATABASE_URL to the
#          accepted Paper 5440 literal regardless of a contaminated caller
#          shell, restores the caller's original value via Restore-EnvSnapshot,
#          and never touches a separately configured live-shadow DB URL
#          (M1-PAPER-READINESS-WAVE-01 CORRECTION B2)
#   LVL30..LVL34  Get-StartupCheckOnlyNextAction functional proofs: reachable-
#          daemon halt truth (kill_switch_active/runtime_status=halted) drives
#          halt-recovery guidance, DISARMED-without-halt never claims a halt,
#          an offline daemon's persisted DISARMED never asserts a halt
#          definitely exists, and the function itself is pure
#   LVL35  readiness endpoint's arm_state=='halted' alone (independent of
#          kill_switch_active/runtime_status) still drives halt-recovery
#          guidance -- proves the three-way OR matches Start-MiniQuantDesk.
#          ps1's real $needsHaltRecovery (M1-PAPER-READINESS-WAVE-01
#          CORRECTION C2, independent self-review follow-up)
#   C3-01..03  confirmed-halt guidance (runtime_status=halted,
#          kill_switch_active=true, readiness arm_state=halted alone) each
#          name Start-MiniQuantDesk.ps1 -Mode Paper explicitly
#   C3-04  none of the halt/offline-DISARMED guidance strings present direct
#          Launch-VeritasLedger.ps1 as an equivalent recovery authority
#   C3-05  reconcile-dirty outranks generic DISARMED-without-halt guidance
#   C3-06  an unknown/unavailable halt-truth signal (runtime_status,
#          readiness arm_state, or kill_switch_active) fails closed to an
#          explicit UNPROVEN result, never "no active halt was detected",
#          and never recommends arm
#   C3-07  reachable + all three halt signals observed non-halted + DISARMED
#          may still use the generic DISARMED-without-halt guidance
#   C3-08  offline persisted DISARMED never asserts a halt definitely exists
#          and never presents Launch-VeritasLedger.ps1 as an equivalent
#          recovery authority
#          (M1-PAPER-READINESS-WAVE-01-CORRECTION-C3)
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path
$Target    = Join-Path $RepoRoot 'scripts\windows\Launch-VeritasLedger.ps1'

$Passed = 0
$Failed = 0

function Assert-True {
    param([string]$Id, [string]$Description, [bool]$Condition)
    if ($Condition) {
        Write-Host "  PASS [$Id] $Description" -ForegroundColor Green
        $script:Passed++
    } else {
        Write-Host "  FAIL [$Id] $Description" -ForegroundColor Red
        $script:Failed++
    }
}

Write-Host ""
Write-Host "=== test_launch_veritas_ledger.ps1 (PAPER-OPERATOR-STARTUP-LAUNCHER-01) ===" -ForegroundColor Cyan
Write-Host "    Target: $Target"
Write-Host ""

if (-not (Test-Path $Target)) {
    Write-Host "  FAIL [LVL01] Script not found: $Target" -ForegroundColor Red
    exit 1
}

$Content = Get-Content $Target -Raw

# LVL01: script exists (Test-Path above already gates this; record as a pass)
Assert-True 'LVL01' 'Script exists at scripts\windows\Launch-VeritasLedger.ps1' `
    (Test-Path $Target)

# LVL02: supports -CheckOnly
Assert-True 'LVL02' 'Script supports -CheckOnly switch' `
    ($Content -match '\[switch\]\$CheckOnly')

# LVL03: normal launch path preserved
Assert-True 'LVL03' 'Normal launch path preserved (daemon resolve/start + GUI resolve/start all present)' `
    ($Content -match 'Ensure-DaemonBinary' -and
     $Content -match 'Start-DaemonIfNeeded' -and
     $Content -match 'Ensure-GuiBinary' -and
     $Content -match [regex]::Escape('Start-Process -FilePath $guiExe'))

# LVL04: -CheckOnly checks .env.local presence only, never reads its contents
$checkOnlyFnMatch = [regex]::Match($Content, '(?s)function Invoke-StartupCheckOnly.*?\n}\r?\n')
Assert-True 'LVL04' '-CheckOnly checks .env.local via Test-Path only (no Get-Content of .env.local)' `
    ($checkOnlyFnMatch.Success -and
     $checkOnlyFnMatch.Value -match [regex]::Escape('Test-Path $envLocalPath') -and
     $checkOnlyFnMatch.Value -notmatch 'Get-Content')

# LVL05: no Alpaca/broker HTTP endpoints
Assert-True 'LVL05' 'Script does NOT call any Alpaca/broker HTTP endpoint' `
    ($Content -notmatch '(?i)alpaca\.markets' -and
     $Content -notmatch '(?i)paper-api\.alpaca' -and
     $Content -notmatch '/v2/orders' -and
     $Content -notmatch '/v2/account')

# LVL06: no order submission/replace/cancel
Assert-True 'LVL06' 'Script does NOT submit, replace, or cancel orders' `
    ($Content -notmatch 'submit_order' -and
     $Content -notmatch '(?i)cancel_order' -and
     $Content -notmatch '(?i)replace_order' -and
     $Content -notmatch 'order_type')

# LVL07: no clear-halted-run
Assert-True 'LVL07' 'Script does NOT call clear-halted-run' `
    ($Content -notmatch [regex]::Escape('clear-halted-run'))

# LVL08: no flatten action
Assert-True 'LVL08' 'Script does NOT call flatten-paper-positions or any flatten action' `
    ($Content -notmatch '(?i)flatten')

# LVL09: no Discord webhook reference
Assert-True 'LVL09' 'Script does NOT reference a Discord webhook' `
    ($Content -notmatch 'DISCORD_WEBHOOK_URL' -and $Content -notmatch '(?i)webhook')

# LVL10: no DML SQL
Assert-True 'LVL10' 'Script does NOT issue INSERT/UPDATE/DELETE/DROP SQL' `
    ($Content -notmatch '(?i)\b(INSERT|UPDATE|DELETE|DROP)\b')

# LVL11: -CheckOnly DB checks are SELECT-only (md_bars count + sys_arm_state)
Assert-True 'LVL11' '-CheckOnly DB checks are SELECT-only (md_bars count + sys_arm_state)' `
    ($Content -match 'SELECT count\(\*\) FROM md_bars WHERE symbol=' -and
     $Content -match 'AND is_complete=true' -and
     $Content -match "SELECT state, coalesce\(reason, ''\) FROM sys_arm_state WHERE sentinel_id = 1")

# LVL12: -CheckOnly reports persisted sys_arm_state
Assert-True 'LVL12' '-CheckOnly reports persisted sys_arm_state with field label' `
    ($Content -match 'Persisted sys_arm_state' -and $Content -match 'sys_arm_state')

# LVL13: -CheckOnly daemon health check is fail-soft
$invokeCheckOnlyGetMatch = [regex]::Match($Content, '(?s)function Invoke-CheckOnlyDaemonGet.*?\n}\r?\n')
Assert-True 'LVL13' '-CheckOnly daemon GET helper is fail-soft (try/catch returns $null)' `
    ($invokeCheckOnlyGetMatch.Success -and
     $invokeCheckOnlyGetMatch.Value -match 'try\s*\{' -and
     $invokeCheckOnlyGetMatch.Value -match 'catch\s*\{' -and
     $invokeCheckOnlyGetMatch.Value -match 'return \$null')

# LVL14: -CheckOnly prints a "Next action" recommendation
Assert-True 'LVL14' '-CheckOnly prints a "Next action" recommendation' `
    ($Content -match [regex]::Escape("Write-CheckField 'Next action'"))

# LVL15: no new desktop-shortcut target introduced
$consoleAltPath = Join-Path $RepoRoot 'scripts\windows\Start-PaperOperatorConsole.ps1'
Assert-True 'LVL15' 'No new desktop-shortcut target introduced (Start-PaperOperatorConsole.ps1 not created)' `
    (-not (Test-Path $consoleAltPath))

# LVL16: -CheckOnly branch (with exit) runs before operator-token resolution and -ArmPaper
$checkOnlyIdx        = $Content.IndexOf('if ($CheckOnly.IsPresent)')
$resolveTokenCallIdx = $Content.IndexOf('$operatorToken = Resolve-RequiredOperatorToken')
$armPaperIdx         = $Content.IndexOf('if ($ArmPaper.IsPresent)')
Assert-True 'LVL16' '-CheckOnly branch (with exit) precedes operator-token resolution and -ArmPaper handling' `
    ($checkOnlyIdx -ge 0 -and $resolveTokenCallIdx -gt $checkOnlyIdx -and $armPaperIdx -gt $checkOnlyIdx -and
     $Content -match [regex]::Escape('exit $checkOnlyExitCode'))

# LVL17: -CheckOnly does not invoke smoke runners (mention-only in Next action text)
Assert-True 'LVL17' '-CheckOnly does not invoke smoke runners (no & call of Run-AAPL5mMarketSmoke.ps1 / Start-PaperTradingSmoke.ps1)' `
    ($Content -notmatch '&\s*[''"]?[^\r\n]*Run-AAPL5mMarketSmoke\.ps1' -and
     $Content -notmatch '&\s*[''"]?[^\r\n]*Start-PaperTradingSmoke\.ps1' -and
     $Content -notmatch 'Invoke-ExternalCommand[^\r\n]*Smoke')

# LVL27 (M1-PAPER-READINESS-WAVE-01): -CheckOnly's "Paper DB port" mismatch
# message must not claim an operator "verify" action is needed when the
# current shell's MQK_DATABASE_URL differs from 5440. Actual Paper startup
# (Start-MiniQuantDesk.ps1 / Start-PaperTradingSmoke.ps1) unconditionally
# reasserts MQK_DATABASE_URL to the hardcoded paper DB literal before any
# DB-dependent step, so a shell-only mismatch never affects routing. The
# message must say so instead of implying the operator must act.
Assert-True 'LVL27' '-CheckOnly Paper DB port mismatch message does not claim an operator "verify" action is required' `
    ($Content -notmatch 'does not match -- verify' -and
     $Content -match [regex]::Escape('Paper startup unconditionally reasserts 5440'))

# LVL28 (M1-PAPER-READINESS-WAVE-01 CORRECTION C2, extended per independent
# self-review): -CheckOnly's reachable-daemon halt branch must key off the
# same three-way live daemon halt truth Start-MiniQuantDesk.ps1's own
# $needsHaltRecovery uses (kill_switch_active, runtime_status='halted', OR
# the /api/v1/autonomous/readiness arm_state='halted'), never off bare
# DISARMED. sys_arm_state.state is DB-constrained (CHECK
# sys_arm_state_state_check) to only ever be 'ARMED' or 'DISARMED' --
# 'HALTED' is exclusively a runs.status value, and DISARMED alone (e.g. from
# a normal operator disarm with no halted run) is not proof of an active
# halt. The readiness arm_state signal is not redundant with runtime_status:
# runtime_status's underlying locally_halted collapses to false whenever the
# durable disarm reason isn't literally "OperatorHalt" (state.rs
# current_status_snapshot), while readiness's arm_state reads
# integrity.halted directly with no such filter.
Assert-True 'LVL28' '-CheckOnly reachable-daemon halt branch keys off the three-way kill_switch_active/runtime_status=halted/readiness-arm_state=halted OR, and a separate DISARMED-without-halt branch never fires on bare $armState -eq ''HALTED''' `
    ($Content -match [regex]::Escape("(`$KillSwitchActive -eq `$true) -or (`$RuntimeStatus -eq 'halted') -or (`$ReadinessArmState -eq 'halted')") -and
     $Content -notmatch [regex]::Escape('$daemonReachable -and $armState -eq ''HALTED'''))

# LVL29 (M1-PAPER-READINESS-WAVE-01 CORRECTION C2, wording updated by
# CORRECTION C3): no blanket "recovery is never automatic / operator must
# explicitly clear then arm" wording remains anywhere in the CheckOnly
# recovery guidance. That wording (introduced by the original Patch C)
# conflated "mqk-daemon's autonomous coordinator does not auto-retry" (true)
# with "an operator must manually intervene right now" (false as an
# instruction -- Start-MiniQuantDesk.ps1 -Mode Paper performs the accepted
# disarm-execution, clear-halt, then arm-execution recovery sequence
# automatically as part of a normal full Paper startup). CORRECTION C3
# replaced the earlier "official Paper startup (Start-MiniQuantDesk.ps1, or
# Launch-VeritasLedger.ps1 without -CheckOnly)" wording (false: direct
# Launch-VeritasLedger.ps1 has no halt-recovery stage) with the explicit
# canonical command.
Assert-True 'LVL29' 'No blanket "recovery is never automatic / operator must explicitly clear then arm" wording remains in CheckOnly guidance' `
    ($Content -notmatch 'Recovery requires explicit operator action \(clear the halted run, then arm-execution\) -- it is never automatic' -and
     $Content -notmatch 'an operator must explicitly clear the halted run, then arm-execution' -and
     $Content -match [regex]::Escape('canonical full Paper startup/recovery command') -and
     $Content -match [regex]::Escape('Start-MiniQuantDesk.ps1 -Mode Paper'))

# ---------------------------------------------------------------------------
# Section: M1-PAPER-READINESS-WAVE-01 CORRECTION B2 functional proofs
#
# Set-LauncherEnvironment is exercised for real (RepoRoot/OperatorToken here
# are harmless fixture strings -- the function only assigns process env
# vars, it never starts a daemon, reads a file, or makes a network call).
# Every mutated env var is restored via the real Restore-EnvSnapshot,
# mirroring the production MAIN DISPATCH try/finally call pattern. Dot-
# sourcing is safe: MAIN DISPATCH is guarded by
# `if ($MyInvocation.InvocationName -ne '.')`.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Section: M1-PAPER-READINESS-WAVE-01 CORRECTION B2 functional proofs ===" -ForegroundColor Cyan

. $Target

$B2FakeTestDbUrl = 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test'
$B2FakeLiveShadowDbUrl = 'postgres://postgres:postgres@127.0.0.1:5432/mqk_live_shadow_fixture'
$B2OriginalDbUrlEnv = [Environment]::GetEnvironmentVariable('MQK_DATABASE_URL', 'Process')

# A/B/C: contaminated shell (fake 5434 test URL) + Paper mode -> effective
# MQK_DATABASE_URL is unconditionally the accepted 5440 Paper DB literal,
# never 5432 or 5434.
try {
    $env:MQK_DATABASE_URL = $B2FakeTestDbUrl
    $b2Snapshot = Set-LauncherEnvironment -OperatorToken 'fake-test-token' -RepoRoot 'C:\fake-repo-root' -DeploymentMode 'paper'
    $b2EffectivePaperUrl = $env:MQK_DATABASE_URL
    Assert-True 'B2-01' 'Paper mode: Set-LauncherEnvironment overrides a contaminated 5434 shell value with the accepted 5440/miniquantdesk_paper literal' `
        ($b2EffectivePaperUrl -match ':5440/miniquantdesk_paper')
    Assert-True 'B2-02' 'Paper mode: effective MQK_DATABASE_URL can never be port 5432 or 5434' `
        ($b2EffectivePaperUrl -notmatch ':5432' -and $b2EffectivePaperUrl -notmatch ':5434')

    # D/E: restoring the snapshot returns the caller's original contaminated
    # value -- the fence is scoped to the launcher's own session, not a
    # permanent mutation of the caller's shell.
    Restore-EnvSnapshot -Snapshot $b2Snapshot
    Assert-True 'B2-03' 'Restore-EnvSnapshot restores the caller''s original (contaminated 5434) MQK_DATABASE_URL after a Paper-mode call' `
        ($env:MQK_DATABASE_URL -eq $B2FakeTestDbUrl)
} finally {
    if ($null -eq $B2OriginalDbUrlEnv) { Remove-Item Env:MQK_DATABASE_URL -ErrorAction SilentlyContinue } else { $env:MQK_DATABASE_URL = $B2OriginalDbUrlEnv }
}

# F/G/H: live-shadow mode must never overwrite a separately configured
# MQK_DATABASE_URL -- live-shadow's own required-config assertion
# (Assert-LiveShadowStartupPrerequisites) is the sole authority there.
try {
    $env:MQK_DATABASE_URL = $B2FakeLiveShadowDbUrl
    $b2LiveSnapshot = Set-LauncherEnvironment -OperatorToken 'fake-test-token' -RepoRoot 'C:\fake-repo-root' -DeploymentMode 'live-shadow'
    Assert-True 'B2-04' 'live-shadow mode: Set-LauncherEnvironment does NOT overwrite a separately configured MQK_DATABASE_URL' `
        ($env:MQK_DATABASE_URL -eq $B2FakeLiveShadowDbUrl)

    # I: restore is still exercised for live-shadow (no-op since the value
    # was never changed, but the snapshot/restore contract must still hold).
    Restore-EnvSnapshot -Snapshot $b2LiveSnapshot
    Assert-True 'B2-05' 'Restore-EnvSnapshot cleanly no-ops for live-shadow (MQK_DATABASE_URL was never changed, so it is unchanged after restore too)' `
        ($env:MQK_DATABASE_URL -eq $B2FakeLiveShadowDbUrl)
} finally {
    if ($null -eq $B2OriginalDbUrlEnv) { Remove-Item Env:MQK_DATABASE_URL -ErrorAction SilentlyContinue } else { $env:MQK_DATABASE_URL = $B2OriginalDbUrlEnv }
}

# ---------------------------------------------------------------------------
# Section: M1-PAPER-READINESS-WAVE-01 CORRECTION C2 functional proofs
#
# Get-StartupCheckOnlyNextAction is a pure decision function (no daemon/DB/
# HTTP/docker calls) extracted specifically so this recovery-guidance logic
# is testable without mocking Invoke-CheckOnlyDaemonGet or docker/psql --
# same rationale as the L1-L16 mocked-HTTP proofs in
# test_official_dual_mode_launcher.ps1, but here no mocking is needed at
# all because the function itself takes only already-observed state.
# Dot-sourcing is safe: MAIN DISPATCH is guarded by
# `if ($MyInvocation.InvocationName -ne '.')`, so this only defines
# functions -- no daemon start, no DB call, no exit.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Section: M1-PAPER-READINESS-WAVE-01 CORRECTION C2 functional proofs ===" -ForegroundColor Cyan

. $Target

$C2Base = @{
    EnvLocalPresent      = $true
    DockerAvailable      = $true
    LiveRoutingEnabled   = $false
    PaperDbContainerName = 'mqk-paper-postgres'
}

# LVL30: reachable daemon + runtime_status=halted -> halt-recovery guidance
# naming the canonical Start-MiniQuantDesk.ps1 -Mode Paper command,
# regardless of arm state. (Wording updated by CORRECTION C3.)
$lvl30 = Get-StartupCheckOnlyNextAction @C2Base -DaemonReachable $true -KillSwitchActive $false -RuntimeStatus 'halted' -ReadinessArmState 'halted' -ArmState 'DISARMED' -ArmReason 'ExecutionLoopTickFailure' -ReconcileStatus 'clean' -DbStatus 'running'
Assert-True 'LVL30' 'Reachable daemon + runtime_status=halted -> halt-recovery guidance naming Start-MiniQuantDesk.ps1 -Mode Paper' `
    ($lvl30 -match 'active halt' -and $lvl30 -match [regex]::Escape('Start-MiniQuantDesk.ps1 -Mode Paper'))

# LVL31: reachable daemon + kill_switch_active=true -> same halt-recovery
# guidance, even if runtime_status is not literally 'halted'.
$lvl31 = Get-StartupCheckOnlyNextAction @C2Base -DaemonReachable $true -KillSwitchActive $true -RuntimeStatus 'idle' -ReadinessArmState 'armed' -ArmState 'ARMED' -ArmReason $null -ReconcileStatus 'clean' -DbStatus 'running'
Assert-True 'LVL31' 'Reachable daemon + kill_switch_active=true -> halt-recovery guidance naming Start-MiniQuantDesk.ps1 -Mode Paper' `
    ($lvl31 -match 'active halt' -and $lvl31 -match [regex]::Escape('Start-MiniQuantDesk.ps1 -Mode Paper'))

# LVL32: reachable daemon + DISARMED but NOT halted by ANY of the three
# signals (kill switch false, runtime_status not 'halted', readiness
# arm_state not 'halted', all three OBSERVED not unknown) -> must NOT claim
# a halt or instruct clear-halted-run; must surface DISARMED as observed
# fact only.
$lvl32 = Get-StartupCheckOnlyNextAction @C2Base -DaemonReachable $true -KillSwitchActive $false -RuntimeStatus 'idle' -ReadinessArmState 'disarmed_db' -ArmState 'DISARMED' -ArmReason 'operator_disarm' -ReconcileStatus 'clean' -DbStatus 'running'
Assert-True 'LVL32' 'Reachable daemon + DISARMED-without-halt (all three signals observed clear) -> no halt claim, no clear-halted-run instruction' `
    ($lvl32 -notmatch 'reports an active halt' -and $lvl32 -notmatch 'clear-halted-run' -and $lvl32 -notmatch 'clear the halted run' -and $lvl32 -match 'no active halt was detected')

# LVL35 (independent self-review follow-up): reachable daemon where
# kill_switch_active=false AND runtime_status != 'halted' (the exact
# under-detection case the durable-disarm-reason-filtered locally_halted
# recompute in state.rs produces for any reason other than the literal
# "OperatorHalt" string -- e.g. this wave's own real-world
# ExecutionLoopTickFailure incident), but the readiness endpoint's
# unfiltered arm_state=='halted' -> halt-recovery guidance must still fire.
# Proves the third signal is load-bearing, not decorative.
$lvl35 = Get-StartupCheckOnlyNextAction @C2Base -DaemonReachable $true -KillSwitchActive $false -RuntimeStatus 'idle' -ReadinessArmState 'halted' -ArmState 'DISARMED' -ArmReason 'ExecutionLoopTickFailure' -ReconcileStatus 'clean' -DbStatus 'running'
Assert-True 'LVL35' 'Reachable daemon + readiness arm_state=halted ALONE (kill_switch_active=false, runtime_status!=halted) -> halt-recovery guidance still fires' `
    ($lvl35 -match 'active halt' -and $lvl35 -match [regex]::Escape('Start-MiniQuantDesk.ps1 -Mode Paper'))

# LVL33: offline daemon + persisted DISARMED -> must not assert a halted run
# definitely exists, and must not instruct manual clear/arm now. Wording
# updated by CORRECTION C3 to name the canonical command explicitly.
$lvl33 = Get-StartupCheckOnlyNextAction @C2Base -DaemonReachable $false -KillSwitchActive $null -RuntimeStatus 'unknown' -ArmState 'DISARMED' -ArmReason 'ExecutionLoopTickFailure' -ReconcileStatus 'unknown' -DbStatus 'running'
Assert-True 'LVL33' 'Offline daemon + persisted DISARMED -> reports observed fact only, never asserts a halt definitely exists, never instructs manual clear/arm now' `
    ($lvl33 -match 'this alone does not prove a halted run currently requires recovery' -and
     $lvl33 -notmatch 'clear-halted-run' -and $lvl33 -notmatch 'operator must explicitly clear' -and
     $lvl33 -match [regex]::Escape('Start-MiniQuantDesk.ps1 -Mode Paper'))

# LVL34: Get-StartupCheckOnlyNextAction itself performs zero daemon/DB/
# docker/HTTP mutation -- it is a pure decision function over its own
# parameters (re-affirms CheckOnly stays read-only even after the C2
# refactor moved this logic into its own function).
$c2FnMatch = [regex]::Match($Content, '(?s)function Get-StartupCheckOnlyNextAction.*?\n}\r?\n')
Assert-True 'LVL34' 'Get-StartupCheckOnlyNextAction is a pure function (no docker/psql/HTTP/Invoke- calls inside it)' `
    ($c2FnMatch.Success -and
     $c2FnMatch.Value -notmatch 'docker exec' -and
     $c2FnMatch.Value -notmatch 'docker inspect' -and
     $c2FnMatch.Value -notmatch 'Invoke-CheckOnlyDaemonGet' -and
     $c2FnMatch.Value -notmatch 'Invoke-JsonRequest')

# ---------------------------------------------------------------------------
# Section: M1-PAPER-READINESS-WAVE-01 CORRECTION C3 functional proofs
#
# Defect: Get-StartupCheckOnlyNextAction's confirmed-halt and offline-
# persisted-DISARMED guidance told the operator that a direct
# "Launch-VeritasLedger.ps1 without -CheckOnly" invocation is an equivalent
# recovery authority to Start-MiniQuantDesk.ps1's full Paper startup. It is
# not: direct Launch-VeritasLedger.ps1 has no halt-recovery stage of its own
# (it only optionally calls Invoke-ArmPaper, which is arm-execution only).
# This section proves the corrected guidance names Start-MiniQuantDesk.ps1
# -Mode Paper explicitly and never presents Launch-VeritasLedger.ps1 as an
# equivalent recovery path; that reconcile-dirty now outranks generic
# DISARMED-without-halt guidance; and that an unknown/unavailable halt-truth
# signal fails closed to an explicit UNPROVEN result rather than a false
# "no active halt was detected" claim.
#
# Dot-sourcing is safe: MAIN DISPATCH is guarded by
# `if ($MyInvocation.InvocationName -ne '.')`, so this only defines
# functions -- no daemon start, no DB call, no exit.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Section: M1-PAPER-READINESS-WAVE-01 CORRECTION C3 functional proofs ===" -ForegroundColor Cyan

. $Target

$C3Base = @{
    EnvLocalPresent      = $true
    DockerAvailable      = $true
    LiveRoutingEnabled   = $false
    PaperDbContainerName = 'mqk-paper-postgres'
}

# A guidance string is considered to falsely present direct
# Launch-VeritasLedger.ps1 as an equivalent recovery authority if it either
# (a) offers it as an alternative to Start-MiniQuantDesk.ps1 for
# recovery/startup ("Start-MiniQuantDesk.ps1, or Launch-VeritasLedger.ps1
# without -CheckOnly"), or (b) claims Launch-VeritasLedger.ps1 itself owns
# or performs halt recovery.
function Test-ClaimsLaunchVeritasLedgerOwnsRecovery {
    param([string]$Guidance)
    if ($Guidance -match [regex]::Escape('Start-MiniQuantDesk.ps1, or Launch-VeritasLedger.ps1 without -CheckOnly')) { return $true }
    if ($Guidance -match '(?i)Launch-VeritasLedger\.ps1[^.]*\b(owns|performs)\b[^.]*\b(halt|recovery)\b') { return $true }
    return $false
}

# C3-01: confirmed runtime halt (runtime_status=halted) guidance names
# Start-MiniQuantDesk.ps1 -Mode Paper.
$c301 = Get-StartupCheckOnlyNextAction @C3Base -DaemonReachable $true -KillSwitchActive $false -RuntimeStatus 'halted' -ReadinessArmState 'armed' -ArmState 'DISARMED' -ArmReason 'OperatorHalt' -ReconcileStatus 'clean' -DbStatus 'running'
Assert-True 'C3-01' 'Confirmed runtime_status=halted guidance names Start-MiniQuantDesk.ps1 -Mode Paper' `
    ($c301 -match [regex]::Escape('Start-MiniQuantDesk.ps1 -Mode Paper'))

# C3-02: confirmed kill-switch halt guidance names Start-MiniQuantDesk.ps1
# -Mode Paper.
$c302 = Get-StartupCheckOnlyNextAction @C3Base -DaemonReachable $true -KillSwitchActive $true -RuntimeStatus 'idle' -ReadinessArmState 'armed' -ArmState 'ARMED' -ArmReason $null -ReconcileStatus 'clean' -DbStatus 'running'
Assert-True 'C3-02' 'Confirmed kill_switch_active=true guidance names Start-MiniQuantDesk.ps1 -Mode Paper' `
    ($c302 -match [regex]::Escape('Start-MiniQuantDesk.ps1 -Mode Paper'))

# C3-03: readiness arm_state=halted ALONE names Start-MiniQuantDesk.ps1
# -Mode Paper.
$c303 = Get-StartupCheckOnlyNextAction @C3Base -DaemonReachable $true -KillSwitchActive $false -RuntimeStatus 'idle' -ReadinessArmState 'halted' -ArmState 'DISARMED' -ArmReason 'ExecutionLoopTickFailure' -ReconcileStatus 'clean' -DbStatus 'running'
Assert-True 'C3-03' 'readiness arm_state=halted alone names Start-MiniQuantDesk.ps1 -Mode Paper' `
    ($c303 -match [regex]::Escape('Start-MiniQuantDesk.ps1 -Mode Paper'))

# C3-08 fixture (offline persisted DISARMED) computed here so C3-04 can
# check it alongside C3-01..03.
$c308 = Get-StartupCheckOnlyNextAction @C3Base -DaemonReachable $false -KillSwitchActive $null -RuntimeStatus 'unknown' -ReadinessArmState 'unknown' -ArmState 'DISARMED' -ArmReason 'ExecutionLoopTickFailure' -ReconcileStatus 'unknown' -DbStatus 'running'

# C3-04: none of the halt/offline-DISARMED guidance strings present direct
# Launch-VeritasLedger.ps1 as an equivalent recovery authority.
Assert-True 'C3-04' 'None of the halt/offline-DISARMED guidance strings present direct Launch-VeritasLedger.ps1 as an equivalent recovery authority' `
    (-not (Test-ClaimsLaunchVeritasLedgerOwnsRecovery -Guidance $c301) -and
     -not (Test-ClaimsLaunchVeritasLedgerOwnsRecovery -Guidance $c302) -and
     -not (Test-ClaimsLaunchVeritasLedgerOwnsRecovery -Guidance $c303) -and
     -not (Test-ClaimsLaunchVeritasLedgerOwnsRecovery -Guidance $c308))

# C3-05: reachable + DISARMED + reconcile dirty returns reconcile-dirty
# guidance, not generic DISARMED guidance (reconcile-dirty must outrank
# generic DISARMED-without-halt).
$c305 = Get-StartupCheckOnlyNextAction @C3Base -DaemonReachable $true -KillSwitchActive $false -RuntimeStatus 'idle' -ReadinessArmState 'disarmed_db' -ArmState 'DISARMED' -ArmReason 'operator_disarm' -ReconcileStatus 'dirty' -DbStatus 'running'
Assert-True 'C3-05' 'reachable + DISARMED + reconcile dirty returns reconcile-dirty guidance, not generic DISARMED guidance' `
    ($c305 -match 'Reconcile status is dirty' -and $c305 -notmatch 'no active halt was detected')

# C3-06: reachable + unknown/unavailable halt surfaces produce UNPROVEN
# guidance, not "no active halt detected". Three independent negative
# controls: each of the three signals unknown in turn, with the other two
# observed non-halted.
$c306a = Get-StartupCheckOnlyNextAction @C3Base -DaemonReachable $true -KillSwitchActive $false -RuntimeStatus 'unknown' -ReadinessArmState 'armed' -ArmState 'DISARMED' -ArmReason 'operator_disarm' -ReconcileStatus 'clean' -DbStatus 'running'
$c306b = Get-StartupCheckOnlyNextAction @C3Base -DaemonReachable $true -KillSwitchActive $false -RuntimeStatus 'idle' -ReadinessArmState 'unknown' -ArmState 'DISARMED' -ArmReason 'operator_disarm' -ReconcileStatus 'clean' -DbStatus 'running'
$c306c = Get-StartupCheckOnlyNextAction @C3Base -DaemonReachable $true -KillSwitchActive $null -RuntimeStatus 'idle' -ReadinessArmState 'armed' -ArmState 'DISARMED' -ArmReason 'operator_disarm' -ReconcileStatus 'clean' -DbStatus 'running'
Assert-True 'C3-06' 'reachable + runtime_status=unknown produces UNPROVEN guidance, not "no active halt detected", and does not recommend arm' `
    ($c306a -match 'UNPROVEN' -and $c306a -notmatch 'no active halt was detected' -and $c306a -notmatch 'deciding on arm-execution')
Assert-True 'C3-06b' 'reachable + readiness arm_state=unknown produces UNPROVEN guidance, not "no active halt detected", and does not recommend arm' `
    ($c306b -match 'UNPROVEN' -and $c306b -notmatch 'no active halt was detected' -and $c306b -notmatch 'deciding on arm-execution')
Assert-True 'C3-06c' 'reachable + kill_switch_active=null/unknown produces UNPROVEN guidance, not "no active halt detected", and does not recommend arm' `
    ($c306c -match 'UNPROVEN' -and $c306c -notmatch 'no active halt was detected' -and $c306c -notmatch 'deciding on arm-execution')

# C3-07: reachable + all three authoritative halt signals observed non-
# halted + DISARMED may use the generic DISARMED-without-halt guidance.
$c307 = Get-StartupCheckOnlyNextAction @C3Base -DaemonReachable $true -KillSwitchActive $false -RuntimeStatus 'idle' -ReadinessArmState 'disarmed_db' -ArmState 'DISARMED' -ArmReason 'operator_disarm' -ReconcileStatus 'clean' -DbStatus 'running'
Assert-True 'C3-07' 'reachable + all three halt signals observed non-halted + DISARMED uses the generic DISARMED-without-halt guidance' `
    ($c307 -match 'no active halt was detected' -and $c307 -notmatch 'UNPROVEN')

# C3-08: offline persisted DISARMED still does not assert a halt definitely
# exists, and does not present Launch-VeritasLedger.ps1 as an equivalent
# recovery authority (fixture computed above as $c308).
Assert-True 'C3-08' 'Offline persisted DISARMED does not assert a halt definitely exists and names Start-MiniQuantDesk.ps1 -Mode Paper, not Launch-VeritasLedger.ps1 directly' `
    ($c308 -match 'this alone does not prove a halted run currently requires recovery' -and
     $c308 -match [regex]::Escape('Start-MiniQuantDesk.ps1 -Mode Paper') -and
     -not (Test-ClaimsLaunchVeritasLedgerOwnsRecovery -Guidance $c308))

# ---------------------------------------------------------------------------
# Section: STALE-DAEMON-BINARY-PROVENANCE-01 functional proofs
#
# Root cause (Thursday 2026-08-13): a reused mqk-daemon.exe predated the
# required-universe/retry functionality that had landed later the same day.
# Ensure-DaemonBinary now requires a provenance sidecar recording the
# core-rs git tree identity the binary was built from, and rebuilds
# whenever that identity is missing or does not match the current tree.
#
# Dot-sourcing is safe: MAIN DISPATCH is guarded by
# `if ($MyInvocation.InvocationName -ne '.')`, so this only defines
# functions -- no daemon start, no real build, no exit. All git/build
# operations below run against a disposable temp fixture repo, never the
# real MiniQuantDeskV4 repo.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Section: STALE-DAEMON-BINARY-PROVENANCE-01 functional proofs ===" -ForegroundColor Cyan

. $Target

$FixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("lvl_provenance_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path (Join-Path $FixtureRoot 'core-rs') | Out-Null

$gitExe = (Get-Command 'git' -ErrorAction SilentlyContinue).Source
if ($null -eq $gitExe) {
    Write-Host "  SKIP [LVL18-26] git not found on PATH; provenance functional proofs skipped" -ForegroundColor Yellow
} else {
    Push-Location $FixtureRoot
    try {
        & $gitExe init -q . 2>&1 | Out-Null
        & $gitExe config user.email 'test@example.com' 2>&1 | Out-Null
        & $gitExe config user.name 'test' 2>&1 | Out-Null
        & $gitExe config core.autocrlf false 2>&1 | Out-Null
        # Mirror the real repo's .gitignore (target/ excluded) so creating the
        # fixture's fake build artifact under core-rs\target does not itself
        # make the tree look dirty.
        Set-Content -Path (Join-Path $FixtureRoot '.gitignore') -Value "target/`n**/target/`n"
        Set-Content -Path (Join-Path $FixtureRoot 'core-rs\marker.txt') -Value 'v1'
        & $gitExe add -A 2>&1 | Out-Null
        & $gitExe commit -q -m 'initial' 2>&1 | Out-Null

        $identityV1 = Get-CoreRsTreeIdentity -RepoRoot $FixtureRoot
        Assert-True 'LVL18' 'Get-CoreRsTreeIdentity resolves a 40-char git tree SHA for core-rs on a clean commit' `
            ($identityV1 -match '^[0-9a-f]{40}$')

        $releaseDir = Join-Path $FixtureRoot 'core-rs\target\release'
        New-Item -ItemType Directory -Force -Path $releaseDir | Out-Null
        $fakeExe = Join-Path $releaseDir 'mqk-daemon.exe'
        Set-Content -Path $fakeExe -Value 'fake'

        Assert-True 'LVL19' 'Missing provenance sidecar -> Test-DaemonBinaryProvenanceMatches=false (rebuild required)' `
            (-not (Test-DaemonBinaryProvenanceMatches -RepoRoot $FixtureRoot -DaemonExePath $fakeExe))

        Write-DaemonBuildProvenance -RepoRoot $FixtureRoot
        Assert-True 'LVL20' 'Matching provenance sidecar -> Test-DaemonBinaryProvenanceMatches=true (binary reusable)' `
            (Test-DaemonBinaryProvenanceMatches -RepoRoot $FixtureRoot -DaemonExePath $fakeExe)

        Set-Content -Path (Join-Path $FixtureRoot 'core-rs\marker.txt') -Value 'v2-uncommitted'
        $identityDirty = Get-CoreRsTreeIdentity -RepoRoot $FixtureRoot
        Assert-True 'LVL21' 'Uncommitted core-rs change -> identity suffixed -dirty, and no longer matches the sidecar' `
            ($identityDirty -eq "$identityV1-dirty" -and
             -not (Test-DaemonBinaryProvenanceMatches -RepoRoot $FixtureRoot -DaemonExePath $fakeExe))

        & $gitExe add -A 2>&1 | Out-Null
        & $gitExe commit -q -m 'second commit' 2>&1 | Out-Null
        $identityV2 = Get-CoreRsTreeIdentity -RepoRoot $FixtureRoot
        Assert-True 'LVL22' 'New commit changes core-rs tree identity -> stale sidecar still does not match (mismatched identity -> rebuild required)' `
            ($identityV2 -ne $identityV1 -and -not (Test-DaemonBinaryProvenanceMatches -RepoRoot $FixtureRoot -DaemonExePath $fakeExe))

        Write-DaemonBuildProvenance -RepoRoot $FixtureRoot
        Assert-True 'LVL23' 'Rewriting the provenance sidecar after a rebuild makes the binary reusable again' `
            (Test-DaemonBinaryProvenanceMatches -RepoRoot $FixtureRoot -DaemonExePath $fakeExe)

        # Ensure-DaemonBinary end-to-end, with the real build tools shadowed
        # so no real cargo build runs against the fixture.
        Remove-Item (Join-Path $FixtureRoot 'core-rs\target\release\mqk-daemon.build-tree.txt') -ErrorAction SilentlyContinue
        $script:BuildInvoked = $false
        function Get-CommandPath {
            param($Name)
            if ($Name -eq 'cargo') { return 'cargo.fake' }
            return (Get-Command $Name -ErrorAction Stop).Source
        }
        function Invoke-ExternalCommand {
            param($FilePath, $Arguments, $WorkingDirectory, [switch]$AllowFailure)
            $script:BuildInvoked = $true
            Set-Content -Path $fakeExe -Value 'rebuilt'
        }

        $null = Ensure-DaemonBinary -RepoRoot $FixtureRoot -ForceRebuild $false
        Assert-True 'LVL24' 'Ensure-DaemonBinary rebuilds when the provenance sidecar is missing (ForceRebuild=false)' `
            ($script:BuildInvoked -eq $true -and (Test-Path (Join-Path $FixtureRoot 'core-rs\target\release\mqk-daemon.build-tree.txt')))

        $script:BuildInvoked = $false
        $null = Ensure-DaemonBinary -RepoRoot $FixtureRoot -ForceRebuild $false
        Assert-True 'LVL25' 'Ensure-DaemonBinary reuses the binary when provenance matches (ForceRebuild=false, no rebuild triggered)' `
            ($script:BuildInvoked -eq $false)

        $script:BuildInvoked = $false
        $null = Ensure-DaemonBinary -RepoRoot $FixtureRoot -ForceRebuild $true
        Assert-True 'LVL26' 'Ensure-DaemonBinary -ForceRebuild always rebuilds regardless of matching provenance' `
            ($script:BuildInvoked -eq $true)
    }
    finally {
        Pop-Location
        Remove-Item -Recurse -Force $FixtureRoot -ErrorAction SilentlyContinue
    }
}

# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $Passed" -ForegroundColor Green
Write-Host "  Failed: $Failed" -ForegroundColor $(if ($Failed -gt 0) { 'Red' } else { 'Green' })
Write-Host ""

if ($Failed -gt 0) {
    Write-Host "GUARD FAILED: $Failed assertion(s) failed." -ForegroundColor Red
    exit 1
} else {
    Write-Host "GUARD PASSED: all $Passed assertions passed." -ForegroundColor Green
    exit 0
}
