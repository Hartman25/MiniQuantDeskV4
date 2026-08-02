# =============================================================================
# Invoke-Bundle7Phase7cPremarketValidation.ps1
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C-FORMAL-SOAK-GATE-TRUTH-
# REPAIR-01.
#
# Two-stage formal soak gate for a Bundle 7 PaperEnforced soak session.
#
# -Stage PreStart:
#   Runs before the paper runtime starts a run. Proves it is safe to start:
#   no conflicting active run/lease, Paper deployment/broker configured,
#   live capital disabled, dynamic-selection mode preview is
#   paper_enforced, arm/integrity/reconciliation posture is startable, and
#   every required market-data pair is complete/fresh. A PreStart PASS may
#   write a *prestart* artifact (bundle7_prestart_readiness_manifest.json)
#   that explicitly states run_id/plan_id are not yet committed -- it is
#   never a formal session manifest and can never authorize or count a
#   soak session on its own.
#
# -Stage ActiveCommit:
#   Runs after the paper runtime has started a run. Proves the durable
#   committed truth of that run: exactly one committed active run/lease,
#   committed disposition/mode/plan/evidence, API-vs-DB agreement, and
#   every selected binding's exact fresh bar window -- by real value, never
#   endpoint reachability alone. Only an ActiveCommit FINAL: PASS may write
#   the formal, countable session manifest
#   (bundle7_soak_session_manifest.json). RequireApi=false, RequireDb=false,
#   or a missing -DaemonBaseUrl are rejected immediately, before any other
#   ActiveCommit check runs -- there is no non-blocking downgrade path for
#   this stage's checks.
#
# Hard/WARN policy (applies to BOTH stages -- this is the repair, not just
# an ActiveCommit-only tightening): a check that cannot be executed for real
# (no DaemonBaseUrl, RequireApi/RequireDb disabled, an unreachable endpoint)
# is reported FAIL, never a non-blocking WARN. The prior one-stage script's
# only escape from that rule was PreStart's bounded `not_applicable` state,
# reserved here for facts that structurally cannot exist yet (e.g. no
# committed plan_id before a run starts) -- never used to paper over an
# unproven check.
#
# Every check always appears in the report -- no silent skip, ever.
#
# Hard invariants, enforced by design:
#   - Never places an order, arms, disarms, starts, or stops a run. Every
#     daemon call in this script is GET-only (Invoke-DaemonGetOnly below,
#     the single call site of Invoke-RestMethod in the whole file).
#   - Never edits .env.local, never reads/echoes a database URL's or
#     daemon operator token's credentials.
#   - In test/coding mode (-DbUrl not explicitly overridden away from the
#     default), the DB URL must be 127.0.0.1/localhost:5434 -- any other
#     port is refused before any query runs. The documented Paper operator
#     command uses -AllowNonTestDbPort with the real operator-supplied
#     paper DB URL.
#   - Manifest/artifact writes are only under an explicit -OutputDirectory
#     that must resolve to a path under smoke_logs (or another operator-
#     ignored directory the caller supplies) -- never staged in git.
#   - approved_for_live is always reported/written as false; a value of
#     true anywhere in captured evidence is itself a hard failure.
#
# Usage -- PreStart (coding/test mode, DB 5434 only, live local daemon on
# DaemonBaseUrl required -- the daemon process must already be running,
# just with no active paper run yet):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-Bundle7Phase7cPremarketValidation.ps1 `
#     -Stage PreStart -RepoRoot . -DbUrl "postgres://postgres:postgres@127.0.0.1:5434/mqk_test" `
#     -ExpectedSha <accepted-sha> -MarketDate 2026-08-03 -Session regular `
#     -DaemonBaseUrl http://127.0.0.1:8899 -OutputDirectory smoke_logs\bundle7_premarket\2026-08-03
#
# Usage -- ActiveCommit (operator mode, real paper DB, after the runtime has
# started a run):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-Bundle7Phase7cPremarketValidation.ps1 `
#     -Stage ActiveCommit -RepoRoot . -DbUrl "postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper" `
#     -AllowNonTestDbPort -Environment Paper `
#     -ExpectedSha <accepted-sha> -MarketDate 2026-08-03 -Session regular `
#     -DaemonBaseUrl http://127.0.0.1:8899 -OutputDirectory smoke_logs\bundle7_premarket\2026-08-03
# =============================================================================

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('PreStart', 'ActiveCommit')]
    [string]$Stage,

    [Parameter(Mandatory = $true)]
    [string]$RepoRoot,

    [Parameter(Mandatory = $true)]
    [string]$DbUrl,

    [Parameter(Mandatory = $true)]
    [string]$ExpectedSha,

    [Parameter(Mandatory = $true)]
    [string]$MarketDate,

    [string]$Session = 'regular',

    [string]$DaemonBaseUrl = '',

    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory,

    [bool]$RequireDb = $true,

    [bool]$RequireApi = $true,

    # Explicit operator acknowledgement that -DbUrl intentionally targets a
    # non-test-port, operator-supplied Paper database (e.g. 5440). Required
    # for any port other than the coding/test default (5434). Never implies
    # -Environment Paper on its own -- both are required together for a
    # real operator run against the live paper DB; see the -Environment
    # parameter below.
    [switch]$AllowNonTestDbPort,

    # Explicit operator-mode acknowledgement, paired with
    # -AllowNonTestDbPort. 'Test' (default) is the coding/CI posture and
    # requires the DB at 127.0.0.1/localhost:5434. 'Paper' is the real
    # operator posture against the live paper DB and requires
    # -AllowNonTestDbPort to also be passed -- one switch alone from either
    # side is never sufficient, so a caller cannot silently drift into the
    # operator posture (or vice versa) by forgetting the other flag.
    [ValidateSet('Test', 'Paper')]
    [string]$Environment = 'Test'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
# PS 7.3+ treats any native-command stderr line (e.g. cargo's own build
# progress text, which the guard invocations below can emit) as a
# terminating error under $ErrorActionPreference='Stop', even when the
# stream is redirected to $null -- mirrors the same guard already used by
# scripts/soak/tests/test_autonomous_paper_session_evidence.ps1. A no-op on
# Windows PowerShell 5.1, where this preference variable does not exist.
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$SchemaVersion = 'bundle7-phase7c-formal-soak-gate-v2'
$LibrarySchemaVersion = 'dynamic-strategy-symbol-selection-v1'
$TestDbPort = '5434'

$RepoRoot = (Resolve-Path $RepoRoot).Path.TrimEnd('\')

if ($Environment -eq 'Paper' -and -not $AllowNonTestDbPort) {
    Write-Host "FAIL -- -Environment Paper requires -AllowNonTestDbPort as an explicit paired acknowledgement." -ForegroundColor Red
    exit 1
}
if ($AllowNonTestDbPort -and $Environment -ne 'Paper') {
    Write-Host "FAIL -- -AllowNonTestDbPort requires -Environment Paper as an explicit paired acknowledgement." -ForegroundColor Red
    exit 1
}

# ---------------------------------------------------------------------------
# Ordered check registries, one per stage. Every check function returns a
# hashtable: @{ Status = 'pass'|'fail'|'not_applicable'; Detail = '<bounded
# string>' }. 'not_applicable' is reserved for PreStart facts that cannot
# exist until a run actually starts (see Test-NoCommittedPlanYet) -- it is
# never used as a synonym for an unproven check, and it never appears in
# the ActiveCommit registry at all.
# ---------------------------------------------------------------------------
$Script:PreStartCheckOrder = @(
    'head_equals_accepted_sha',
    'tracked_worktree_clean',
    'migration_governance',
    'require_api_and_db_true',
    'expected_db_reachable',
    'no_conflicting_active_or_starting_run',
    'no_unexpired_leader_lease',
    'deployment_paper_broker_config',
    'live_routing_capital_disabled',
    'dynamic_selection_mode_preview_paper_enforced',
    'arm_integrity_posture_suitable_for_start',
    'reconciliation_truth_acceptable',
    'required_market_data_pairs_complete_and_fresh',
    'phase7_and_bundle_guards_pass',
    'no_trading_action_invoked',
    'prestart_artifact_validates'
)

$Script:ActiveCommitCheckOrder = @(
    'head_equals_accepted_sha',
    'tracked_worktree_clean',
    'migration_governance',
    'require_api_and_db_true',
    'expected_db_reachable',
    'lifecycle_starting_or_running',
    'exactly_one_committed_active_run',
    'run_lease_coherent',
    'committed_disposition_paper_enforced_allowed',
    'committed_effective_mode_paper_enforced',
    'committed_live_lock_false',
    'approved_for_live_false',
    'committed_plan_id_present',
    'evidence_persisted_true',
    'evidence_validation_state_valid',
    'validation_blockers_empty',
    'api_selected_bindings_equal_durable_candidates',
    'selected_bindings_have_fresh_bar_windows',
    'no_binding_missing_required_window',
    'arm_integrity_reconciliation_live_routing_facts',
    'reconciliation_truth_acceptable',
    'api_matches_db_evidence',
    'phase7_and_bundle_guards_pass',
    'no_trading_action_invoked',
    'active_commit_manifest_validates'
)

$Script:CheckOrder = if ($Stage -eq 'PreStart') { $Script:PreStartCheckOrder } else { $Script:ActiveCommitCheckOrder }

$Script:Results = [ordered]@{}
function Set-CheckResult {
    param([string]$Name, [string]$Status, [string]$Detail)
    $Script:Results[$Name] = [ordered]@{ status = $Status; detail = $Detail }
}

function Write-Step {
    param([string]$Msg) Write-Host "[check] $Msg" -ForegroundColor Cyan
}
function Write-Ok {
    param([string]$Msg) Write-Host "  OK   -- $Msg" -ForegroundColor Green
}
function Write-NotApplicable {
    param([string]$Msg) Write-Host "  N/A  -- $Msg" -ForegroundColor DarkGray
}
function Write-Fail {
    param([string]$Msg) Write-Host "  FAIL -- $Msg" -ForegroundColor Red
}

Write-Host ""
Write-Host "=== Invoke-Bundle7Phase7cPremarketValidation.ps1 ===" -ForegroundColor Cyan
Write-Host "    Stage           : $Stage"
Write-Host "    RepoRoot        : $RepoRoot"
Write-Host "    ExpectedSha     : $ExpectedSha"
Write-Host "    MarketDate      : $MarketDate"
Write-Host "    Session         : $Session"
Write-Host "    Environment     : $Environment"
Write-Host "    RequireDb       : $RequireDb"
Write-Host "    RequireApi      : $RequireApi"
Write-Host "    OutputDirectory : $OutputDirectory"
Write-Host ""

# ---------------------------------------------------------------------------
# Resolve a real Git-Bash-compatible bash.exe explicitly. On this class of
# Windows box, a bare `bash` on PATH can resolve to the WSL launcher
# (System32\bash.exe) instead of Git Bash -- WSL uses a `/mnt/c/...` mount
# convention and (on some distros) an incompatible default shell, so
# invoking guard .sh scripts through it silently breaks. Preferring a
# well-known Git-for-Windows path avoids that ambiguity; PATH `bash` is only
# a last-resort fallback.
# ---------------------------------------------------------------------------
function Get-BashExe {
    $candidates = @(
        "$env:ProgramFiles\Git\bin\bash.exe",
        "${env:ProgramFiles(x86)}\Git\bin\bash.exe",
        "$env:LOCALAPPDATA\Programs\Git\bin\bash.exe"
    )
    foreach ($c in $candidates) {
        if ($c -and (Test-Path $c)) { return $c }
    }
    $onPath = Get-Command bash -ErrorAction SilentlyContinue
    if ($onPath) { return $onPath.Source }
    return $null
}
$Script:BashExe = Get-BashExe

function Invoke-BashScript {
    param([string]$ScriptPath)
    if ($null -eq $Script:BashExe) { return 127 }
    $forwardSlashPath = $ScriptPath -replace '\\', '/'
    # See the matching comment at the nested-powershell guard call site --
    # a delegated .sh guard's own stderr (e.g. a cargo-backed check) can be
    # promoted to a terminating error under strict EAP on Windows
    # PowerShell 5.1. Relax EAP for just this one external-process call.
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    & $Script:BashExe $forwardSlashPath *> $null
    $exitCode = $LASTEXITCODE
    $ErrorActionPreference = $prevEap
    return $exitCode
}

# ---------------------------------------------------------------------------
# DB URL parse + hard test-port refusal. Never echoes the raw URL (it may
# carry a password); only the sanitized host:port is ever printed.
# ---------------------------------------------------------------------------
function Get-SanitizedDbHostPort {
    param([string]$RawUrl)
    try {
        $u = [Uri]$RawUrl
        return @{ Host = $u.Host; Port = [string]$u.Port; Ok = $true }
    } catch {
        return @{ Host = $null; Port = $null; Ok = $false }
    }
}
$DbHostPort = Get-SanitizedDbHostPort -RawUrl $DbUrl
if (-not $DbHostPort.Ok) {
    Write-Fail "DbUrl is not a parseable URI (raw value never echoed)."
    exit 1
}
$DbHostAllowed = @('127.0.0.1', 'localhost', '::1')
if (-not $AllowNonTestDbPort) {
    if ($DbHostPort.Port -ne $TestDbPort -or ($DbHostAllowed -notcontains $DbHostPort.Host)) {
        Write-Fail "Coding/test mode requires the DB at 127.0.0.1/localhost:$TestDbPort. Refusing host=$($DbHostPort.Host) port=$($DbHostPort.Port). Pass -AllowNonTestDbPort -Environment Paper for an explicit operator-supplied paper DB (never 5440 by default)."
        exit 1
    }
}

# ---------------------------------------------------------------------------
# Check: RequireApi/RequireDb/DaemonBaseUrl hard precondition. Applies to
# BOTH stages -- Part 2's WARN/HARD repair is not an ActiveCommit-only
# tightening. A caller cannot request a downgraded, non-blocking version of
# any check in this script; RequireApi=false/RequireDb=false only ever
# produces an explicit FAIL for the checks that depend on them, never a
# silent pass and never a WARN.
# ---------------------------------------------------------------------------
function Test-RequireApiAndDbTrue {
    Write-Step 'RequireApi/RequireDb/DaemonBaseUrl hard precondition'
    $problems = @()
    if (-not $RequireApi) { $problems += 'RequireApi=false is not accepted by this script' }
    if (-not $RequireDb) { $problems += 'RequireDb=false is not accepted by this script' }
    if ($DaemonBaseUrl -eq '') { $problems += '-DaemonBaseUrl is required and was not supplied' }
    if (@($problems).Count -eq 0) {
        Set-CheckResult 'require_api_and_db_true' 'pass' 'RequireApi=true, RequireDb=true, DaemonBaseUrl supplied'
    } else {
        Set-CheckResult 'require_api_and_db_true' 'fail' ($problems -join '; ')
    }
}

# ---------------------------------------------------------------------------
# Check: HEAD equals accepted SHA.
# ---------------------------------------------------------------------------
function Test-HeadEqualsAcceptedSha {
    Write-Step 'HEAD equals accepted SHA'
    try {
        $head = (& git -C $RepoRoot rev-parse HEAD 2>$null).Trim()
        if ($LASTEXITCODE -ne 0 -or $head -eq '') {
            Set-CheckResult 'head_equals_accepted_sha' 'fail' 'git rev-parse HEAD failed'
            return
        }
        if ($head -eq $ExpectedSha) {
            Set-CheckResult 'head_equals_accepted_sha' 'pass' "HEAD=$head"
        } else {
            Set-CheckResult 'head_equals_accepted_sha' 'fail' "HEAD=$head does not match ExpectedSha=$ExpectedSha"
        }
    } catch {
        Set-CheckResult 'head_equals_accepted_sha' 'fail' 'git invocation failed'
    }
}

# ---------------------------------------------------------------------------
# Check: tracked worktree clean; only protected untracked paths.
# ---------------------------------------------------------------------------
function Test-TrackedWorktreeClean {
    Write-Step 'Tracked worktree clean (only protected untracked paths)'
    $AllowedUntracked = @(
        'MiniQuantDesk_Master_Patch_Ledger_v2_updated.md',
        'smoke_logs/'
    )
    try {
        $lines = & git -C $RepoRoot status --short --untracked-files=all 2>$null
        if ($LASTEXITCODE -ne 0) {
            Set-CheckResult 'tracked_worktree_clean' 'fail' 'git status failed'
            return
        }
        $offending = @()
        foreach ($line in @($lines)) {
            if ($line -eq '') { continue }
            $code = $line.Substring(0, 2)
            $path = $line.Substring(3).Trim()
            $isAllowedUntracked = ($code -eq '??') -and (
                $AllowedUntracked | Where-Object { $path -eq $_ -or $path.StartsWith($_) }
            )
            if (-not $isAllowedUntracked) {
                $offending += $line
            }
        }
        if (@($offending).Count -eq 0) {
            Set-CheckResult 'tracked_worktree_clean' 'pass' 'no offending tracked/untracked changes'
        } else {
            Set-CheckResult 'tracked_worktree_clean' 'fail' "offending paths: $(($offending | Select-Object -First 5) -join '; ')"
        }
    } catch {
        Set-CheckResult 'tracked_worktree_clean' 'fail' 'git status invocation failed'
    }
}

# ---------------------------------------------------------------------------
# Check: migration/manifest governance.
# ---------------------------------------------------------------------------
function Test-MigrationGovernance {
    Write-Step 'Migration/manifest governance'
    $guardPath = Join-Path $RepoRoot 'scripts/guards/check_migration_governance.sh'
    if (-not (Test-Path $guardPath)) {
        Set-CheckResult 'migration_governance' 'fail' 'guard script not found'
        return
    }
    $exitCode = Invoke-BashScript -ScriptPath $guardPath
    if ($exitCode -eq 0) {
        Set-CheckResult 'migration_governance' 'pass' 'check_migration_governance.sh exited 0'
    } else {
        Set-CheckResult 'migration_governance' 'fail' "check_migration_governance.sh exited $exitCode"
    }
}

# ---------------------------------------------------------------------------
# Check: explicit expected DB reachable.
# ---------------------------------------------------------------------------
function Test-ExpectedDbReachable {
    Write-Step "Expected DB reachable ($($DbHostPort.Host):$($DbHostPort.Port))"
    try {
        $tnc = Test-NetConnection -ComputerName $DbHostPort.Host -Port ([int]$DbHostPort.Port) -InformationLevel Quiet -WarningAction SilentlyContinue
        if ($tnc) {
            Set-CheckResult 'expected_db_reachable' 'pass' 'TCP connect succeeded'
        } else {
            Set-CheckResult 'expected_db_reachable' 'fail' 'TCP connect failed'
        }
    } catch {
        Set-CheckResult 'expected_db_reachable' 'fail' 'Test-NetConnection threw'
    }
}

# ---------------------------------------------------------------------------
# psql helpers. Never echoes the raw DB URL or password.
# ---------------------------------------------------------------------------
function Invoke-PsqlScalar {
    param([string]$Sql)
    $env:PGPASSWORD = $Script:DbPassword
    $out = & psql -h $Script:DbHostForPsql -p $Script:DbPortForPsql -U $Script:DbUserForPsql -d $Script:DbNameForPsql -t -A -c $Sql 2>$null
    $code = $LASTEXITCODE
    return @{ Output = ($out -join "`n").Trim(); ExitCode = $code }
}

function Initialize-PsqlConnectionParts {
    try {
        $u = [Uri]$DbUrl
        $Script:DbHostForPsql = $u.Host
        $Script:DbPortForPsql = $u.Port
        $Script:DbNameForPsql = $u.AbsolutePath.TrimStart('/')
        $userInfo = $u.UserInfo -split ':', 2
        $Script:DbUserForPsql = if ($userInfo.Length -ge 1) { $userInfo[0] } else { 'postgres' }
        $Script:DbPassword = if ($userInfo.Length -ge 2) { $userInfo[1] } else { '' }
        return $true
    } catch {
        return $false
    }
}
$PsqlParsable = Initialize-PsqlConnectionParts
$PsqlAvailable = $null -ne (Get-Command psql -ErrorAction SilentlyContinue)

function Test-PsqlUsable {
    return ($RequireDb -and $PsqlAvailable -and $PsqlParsable)
}

# ---------------------------------------------------------------------------
# GET-only daemon helper -- the single call site of Invoke-RestMethod in
# this whole file (Test-NoTradingActionInvoked's self-scan below proves
# that structurally).
# ---------------------------------------------------------------------------
function Invoke-DaemonGetOnly {
    param([string]$Path)
    if ($DaemonBaseUrl -eq '') { return $null }
    try {
        return Invoke-RestMethod -Uri "${DaemonBaseUrl}${Path}" -Method Get -TimeoutSec 5 -ErrorAction Stop
    } catch {
        return $null
    }
}

# ---------------------------------------------------------------------------
# Live facts, fetched once and shared across checks (never refetched per
# check -- a stale/inconsistent cross-check would otherwise be possible if
# two checks raced against two different HTTP responses).
# ---------------------------------------------------------------------------
$Script:SystemStatus = $null
$Script:ControlStatus = $null
$Script:ReconcileStatus = $null
$Script:DynamicSelectionStatus = $null
$Script:MarketDataReadiness = $null

function Initialize-LiveFacts {
    if ($DaemonBaseUrl -eq '' -or -not $RequireApi) { return }
    $Script:SystemStatus = Invoke-DaemonGetOnly -Path '/api/v1/system/status'
    $Script:ControlStatus = Invoke-DaemonGetOnly -Path '/api/v1/control/status'
    $Script:ReconcileStatus = Invoke-DaemonGetOnly -Path '/api/v1/reconcile/status'
    $Script:DynamicSelectionStatus = Invoke-DaemonGetOnly -Path '/api/v1/dynamic-selection/status'
    $Script:MarketDataReadiness = Invoke-DaemonGetOnly -Path '/api/v1/market-data/readiness'
}

# ---------------------------------------------------------------------------
# PreStart checks
# ---------------------------------------------------------------------------

function Test-NoConflictingActiveOrStartingRun {
    Write-Step 'No conflicting active/starting durable PAPER run'
    if (-not (Test-PsqlUsable)) {
        Set-CheckResult 'no_conflicting_active_or_starting_run' 'fail' 'psql unavailable, DB URL unparsable, or RequireDb=false'
        return
    }
    $r = Invoke-PsqlScalar "select count(*) from runs where engine_id = 'mqk-daemon' and mode = 'PAPER' and status in ('ARMED','RUNNING');"
    if ($r.ExitCode -ne 0) {
        Set-CheckResult 'no_conflicting_active_or_starting_run' 'fail' 'runs query failed'
        return
    }
    if ($r.Output -eq '0') {
        Set-CheckResult 'no_conflicting_active_or_starting_run' 'pass' 'no ARMED/RUNNING PAPER run for mqk-daemon'
    } else {
        Set-CheckResult 'no_conflicting_active_or_starting_run' 'fail' "conflicting ARMED/RUNNING run rows present: $($r.Output)"
    }
}

function Test-NoUnexpiredLeaderLease {
    Write-Step 'No unexpired runtime-leader lease (PreStart: none may exist yet)'
    if (-not (Test-PsqlUsable)) {
        Set-CheckResult 'no_unexpired_leader_lease' 'fail' 'psql unavailable, DB URL unparsable, or RequireDb=false'
        return
    }
    $r = Invoke-PsqlScalar "select count(*) from runtime_leader_lease where lease_expires_at > now();"
    if ($r.ExitCode -ne 0) {
        Set-CheckResult 'no_unexpired_leader_lease' 'fail' 'lease query failed'
        return
    }
    if ($r.Output -eq '0') {
        Set-CheckResult 'no_unexpired_leader_lease' 'pass' 'no unexpired runtime-leader lease'
    } else {
        Set-CheckResult 'no_unexpired_leader_lease' 'fail' "unexpired lease rows present: $($r.Output)"
    }
}

function Test-DeploymentPaperBrokerConfig {
    Write-Step 'Paper deployment and paper broker configuration'
    if ($null -eq $Script:SystemStatus) {
        Set-CheckResult 'deployment_paper_broker_config' 'fail' 'system/status unreachable'
        return
    }
    $mode = [string]$Script:SystemStatus.daemon_mode
    if ($mode.ToUpperInvariant() -eq 'PAPER') {
        Set-CheckResult 'deployment_paper_broker_config' 'pass' "daemon_mode=$mode adapter_id=$($Script:SystemStatus.adapter_id)"
    } else {
        Set-CheckResult 'deployment_paper_broker_config' 'fail' "daemon_mode=$mode (expected paper)"
    }
}

function Test-LiveRoutingCapitalDisabled {
    Write-Step 'Live routing/capital disabled'
    if ($null -eq $Script:SystemStatus) {
        Set-CheckResult 'live_routing_capital_disabled' 'fail' 'system/status unreachable'
        return
    }
    $liveRouting = $Script:SystemStatus.live_routing_enabled
    if ($null -ne $liveRouting -and $liveRouting -eq $true) {
        Set-CheckResult 'live_routing_capital_disabled' 'fail' 'live_routing_enabled=true -- hard blocker'
        return
    }
    Set-CheckResult 'live_routing_capital_disabled' 'pass' "live_routing_enabled=$liveRouting"
}

function Test-DynamicSelectionModePreviewPaperEnforced {
    Write-Step 'Preview dynamic-selection effective mode is paper_enforced'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'dynamic_selection_mode_preview_paper_enforced' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    $eff = [string]$Script:DynamicSelectionStatus.preview_effective_mode
    if ($eff -eq 'paper_enforced') {
        Set-CheckResult 'dynamic_selection_mode_preview_paper_enforced' 'pass' "preview_effective_mode=$eff"
    } else {
        Set-CheckResult 'dynamic_selection_mode_preview_paper_enforced' 'fail' "preview_effective_mode=$eff (expected paper_enforced)"
    }
}

function Test-ArmIntegrityPostureSuitableForStart {
    Write-Step 'Arm/integrity/risk/deadman posture is suitable for starting'
    if ($null -eq $Script:ControlStatus) {
        Set-CheckResult 'arm_integrity_posture_suitable_for_start' 'fail' 'control/status unreachable'
        return
    }
    $problems = @()
    if ($Script:ControlStatus.risk_blocked -eq $true) { $problems += "risk_blocked=true ($($Script:ControlStatus.risk_reason))" }
    if ([string]$Script:ControlStatus.deadman_armed_state -eq 'HALTED') { $problems += 'deadman_armed_state=HALTED' }
    if ([string]$Script:ControlStatus.integrity_state -eq 'HALTED') { $problems += 'integrity_state=HALTED' }
    if (@($problems).Count -eq 0) {
        Set-CheckResult 'arm_integrity_posture_suitable_for_start' 'pass' "integrity_state=$($Script:ControlStatus.integrity_state) risk_blocked=false deadman_armed_state=$($Script:ControlStatus.deadman_armed_state)"
    } else {
        Set-CheckResult 'arm_integrity_posture_suitable_for_start' 'fail' ($problems -join '; ')
    }
}

# ---------------------------------------------------------------------------
# Reconciliation truth check, shared by both stages -- reject dirty,
# mismatch, unknown, stale, or unavailable truth. `status` is a free-form
# string (see mqk-daemon's ReconcileSummaryResponse) with no formal Rust
# enum; the only values ever assigned in the codebase are unknown/ok/dirty/
# stale/unavailable, so "ok" is the only acceptable value here. truth_state
# in {never_run, active} is acceptable (never_run is honest for a session
# that has not ticked reconciliation yet); "stale" is never acceptable.
# ---------------------------------------------------------------------------
function Test-ReconciliationTruthAcceptable {
    Write-Step 'Reconciliation truth is acceptable (never dirty/stale/unavailable/unknown)'
    if ($null -eq $Script:ReconcileStatus) {
        Set-CheckResult 'reconciliation_truth_acceptable' 'fail' 'reconcile/status unreachable'
        return
    }
    $status = [string]$Script:ReconcileStatus.status
    $truthState = [string]$Script:ReconcileStatus.truth_state
    $statusOk = $status -eq 'ok'
    $truthStateOk = ($truthState -eq 'never_run') -or ($truthState -eq 'active')
    if ($statusOk -and $truthStateOk) {
        Set-CheckResult 'reconciliation_truth_acceptable' 'pass' "status=$status truth_state=$truthState"
    } else {
        Set-CheckResult 'reconciliation_truth_acceptable' 'fail' "status=$status truth_state=$truthState (status must be ok, truth_state must be never_run or active)"
    }
}

function Test-RequiredMarketDataPairsCompleteAndFresh {
    Write-Step 'Required market-data pairs are complete and fresh (PreStart: configured/preview set)'
    if ($null -eq $Script:MarketDataReadiness) {
        Set-CheckResult 'required_market_data_pairs_complete_and_fresh' 'fail' 'market-data/readiness unreachable'
        return
    }
    if ($Script:MarketDataReadiness.start_allowed -ne $true) {
        Set-CheckResult 'required_market_data_pairs_complete_and_fresh' 'fail' "start_allowed=$($Script:MarketDataReadiness.start_allowed) top_level_blocker=$($Script:MarketDataReadiness.top_level_blocker)"
        return
    }
    $notReady = @($Script:MarketDataReadiness.assignments | Where-Object { $_.readiness_state -ne 'ready' })
    if (@($notReady).Count -gt 0) {
        $detail = ($notReady | Select-Object -First 3 | ForEach-Object { "$($_.assignment_symbol)/$($_.assignment_timeframe)=$($_.readiness_state)" }) -join '; '
        Set-CheckResult 'required_market_data_pairs_complete_and_fresh' 'fail' "not-ready assignment(s): $detail"
        return
    }
    Set-CheckResult 'required_market_data_pairs_complete_and_fresh' 'pass' "start_allowed=true, $(@($Script:MarketDataReadiness.assignments).Count) assignment(s) all ready"
}

# ---------------------------------------------------------------------------
# ActiveCommit checks
# ---------------------------------------------------------------------------

function Test-LifecycleStartingOrRunning {
    Write-Step 'Runtime lifecycle is starting or running'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'lifecycle_starting_or_running' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    $lifecycle = [string]$Script:DynamicSelectionStatus.lifecycle
    if ($lifecycle -eq 'starting' -or $lifecycle -eq 'running') {
        Set-CheckResult 'lifecycle_starting_or_running' 'pass' "lifecycle=$lifecycle"
    } else {
        Set-CheckResult 'lifecycle_starting_or_running' 'fail' "lifecycle=$lifecycle (expected starting or running)"
    }
}

function Test-ExactlyOneCommittedActiveRun {
    Write-Step 'Exactly one committed active run_id, owned locally, in an accepted durable status'
    if ($null -eq $Script:ControlStatus) {
        Set-CheckResult 'exactly_one_committed_active_run' 'fail' 'control/status unreachable'
        return
    }
    $activeRunId = $Script:ControlStatus.active_run_id
    if ($null -eq $activeRunId -or $activeRunId -eq '') {
        Set-CheckResult 'exactly_one_committed_active_run' 'fail' 'no active_run_id reported by control/status'
        return
    }
    if ($Script:ControlStatus.run_owned_locally -ne $true) {
        Set-CheckResult 'exactly_one_committed_active_run' 'fail' "active_run_id=$activeRunId is not run_owned_locally"
        return
    }
    if (-not (Test-PsqlUsable)) {
        Set-CheckResult 'exactly_one_committed_active_run' 'fail' 'psql unavailable, DB URL unparsable, or RequireDb=false -- cannot verify durable run status'
        return
    }
    $r = Invoke-PsqlScalar "select status from runs where run_id = '$activeRunId';"
    if ($r.ExitCode -ne 0 -or $r.Output -eq '') {
        Set-CheckResult 'exactly_one_committed_active_run' 'fail' "no durable runs row for run_id=$activeRunId"
        return
    }
    if ($r.Output -eq 'ARMED' -or $r.Output -eq 'RUNNING') {
        $Script:ActiveRunId = $activeRunId
        Set-CheckResult 'exactly_one_committed_active_run' 'pass' "run_id=$activeRunId status=$($r.Output) run_owned_locally=true"
    } else {
        Set-CheckResult 'exactly_one_committed_active_run' 'fail' "run_id=$activeRunId has durable status=$($r.Output) (expected ARMED or RUNNING)"
    }
}

function Test-RunLeaseCoherent {
    Write-Step 'Exactly one coherent, unexpired leader lease'
    if ($null -eq $Script:ControlStatus) {
        Set-CheckResult 'run_lease_coherent' 'fail' 'control/status unreachable'
        return
    }
    $holder = $Script:ControlStatus.leader_holder_id
    if ($null -eq $holder -or $holder -eq '') {
        Set-CheckResult 'run_lease_coherent' 'fail' 'no leader_holder_id -- no lease held'
        return
    }
    if ($Script:ControlStatus.lease_expired -eq $true) {
        Set-CheckResult 'run_lease_coherent' 'fail' "lease held by $holder is expired"
        return
    }
    if (-not (Test-PsqlUsable)) {
        Set-CheckResult 'run_lease_coherent' 'fail' 'psql unavailable, DB URL unparsable, or RequireDb=false -- cannot verify durable lease count'
        return
    }
    $r = Invoke-PsqlScalar "select count(*) from runtime_leader_lease where lease_expires_at > now();"
    if ($r.ExitCode -ne 0) {
        Set-CheckResult 'run_lease_coherent' 'fail' 'lease query failed'
        return
    }
    if ($r.Output -eq '1') {
        Set-CheckResult 'run_lease_coherent' 'pass' "exactly one unexpired lease, holder=$holder, lease_expired=false"
    } else {
        Set-CheckResult 'run_lease_coherent' 'fail' "expected exactly one unexpired lease row, found $($r.Output)"
    }
}

function Test-CommittedDispositionPaperEnforcedAllowed {
    Write-Step 'Committed disposition is paper_enforced_allowed'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'committed_disposition_paper_enforced_allowed' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    $disposition = [string]$Script:DynamicSelectionStatus.disposition
    if ($disposition -eq 'paper_enforced_allowed') {
        Set-CheckResult 'committed_disposition_paper_enforced_allowed' 'pass' "disposition=$disposition"
    } else {
        Set-CheckResult 'committed_disposition_paper_enforced_allowed' 'fail' "disposition=$disposition (expected paper_enforced_allowed)"
    }
}

function Test-CommittedEffectiveModePaperEnforced {
    Write-Step 'Committed effective mode is paper_enforced'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'committed_effective_mode_paper_enforced' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    $mode = [string]$Script:DynamicSelectionStatus.committed_effective_mode
    if ($mode -eq 'paper_enforced') {
        Set-CheckResult 'committed_effective_mode_paper_enforced' 'pass' "committed_effective_mode=$mode"
    } else {
        Set-CheckResult 'committed_effective_mode_paper_enforced' 'fail' "committed_effective_mode=$mode (expected paper_enforced)"
    }
}

function Test-CommittedLiveLockFalse {
    Write-Step 'Committed live_lock_applied is false'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'committed_live_lock_false' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    if ($Script:DynamicSelectionStatus.committed_live_lock_applied -eq $false) {
        Set-CheckResult 'committed_live_lock_false' 'pass' 'committed_live_lock_applied=false'
    } else {
        Set-CheckResult 'committed_live_lock_false' 'fail' 'committed_live_lock_applied is not false -- deployment is not paper+Alpaca, dispatch has zero effect'
    }
}

function Test-ApprovedForLiveFalseHard {
    Write-Step 'approved_for_live=false (hard, no downgrade)'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'approved_for_live_false' 'fail' 'dynamic-selection/status unreachable -- cannot verify approved_for_live'
        return
    }
    if ($Script:DynamicSelectionStatus.approved_for_live -eq $false) {
        Set-CheckResult 'approved_for_live_false' 'pass' 'approved_for_live=false'
    } else {
        Set-CheckResult 'approved_for_live_false' 'fail' 'approved_for_live is not false -- hard blocker'
    }
}

function Test-CommittedPlanIdPresent {
    Write-Step 'Committed plan_id is present (non-null)'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'committed_plan_id_present' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    $planId = $Script:DynamicSelectionStatus.committed_plan_id
    if ($null -ne $planId -and $planId -ne '') {
        $Script:ActivePlanId = $planId
        Set-CheckResult 'committed_plan_id_present' 'pass' "committed_plan_id=$planId"
    } else {
        Set-CheckResult 'committed_plan_id_present' 'fail' 'committed_plan_id is null -- no committed plan; ActiveCommit requires one'
    }
}

function Test-EvidencePersistedTrue {
    Write-Step 'evidence_persisted=true'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'evidence_persisted_true' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    if ($Script:DynamicSelectionStatus.evidence_persisted -eq $true) {
        Set-CheckResult 'evidence_persisted_true' 'pass' 'evidence_persisted=true'
    } else {
        Set-CheckResult 'evidence_persisted_true' 'fail' 'evidence_persisted is not true'
    }
}

function Test-EvidenceValidationStateValid {
    Write-Step 'evidence_validation_state=valid'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'evidence_validation_state_valid' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    $state = [string]$Script:DynamicSelectionStatus.evidence_validation_state
    if ($state -eq 'valid') {
        Set-CheckResult 'evidence_validation_state_valid' 'pass' "evidence_validation_state=$state"
    } else {
        Set-CheckResult 'evidence_validation_state_valid' 'fail' "evidence_validation_state=$state (expected valid)"
    }
}

function Test-ValidationBlockersEmpty {
    Write-Step 'validation_blockers is empty'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'validation_blockers_empty' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    $blockers = @($Script:DynamicSelectionStatus.validation_blockers)
    if (@($blockers).Count -eq 0) {
        Set-CheckResult 'validation_blockers_empty' 'pass' 'no validation blockers'
    } else {
        Set-CheckResult 'validation_blockers_empty' 'fail' "validation_blockers: $(($blockers | Select-Object -First 3) -join '; ')"
    }
}

function Test-ApiSelectedBindingsEqualDurableCandidates {
    Write-Step 'API selected bindings exactly equal durable selected candidates (symbol, strategy_id, timeframe_secs)'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'api_selected_bindings_equal_durable_candidates' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    $planId = $Script:DynamicSelectionStatus.committed_plan_id
    if ($null -eq $planId) {
        Set-CheckResult 'api_selected_bindings_equal_durable_candidates' 'fail' 'no committed_plan_id -- cannot cross-check'
        return
    }
    if (-not (Test-PsqlUsable)) {
        Set-CheckResult 'api_selected_bindings_equal_durable_candidates' 'fail' 'psql unavailable, DB URL unparsable, or RequireDb=false'
        return
    }
    $apiBindings = @($Script:DynamicSelectionStatus.selected | ForEach-Object {
        "$($_.symbol)|$($_.selected_strategy_id)|$($_.timeframe_secs)"
    }) | Sort-Object
    $r = Invoke-PsqlScalar "select string_agg(symbol || '|' || strategy_id || '|' || timeframe_secs, ',' order by symbol, strategy_id, timeframe_secs) from sys_dynamic_selection_plan_candidates where plan_id = '$planId' and selected = true;"
    if ($r.ExitCode -ne 0) {
        Set-CheckResult 'api_selected_bindings_equal_durable_candidates' 'fail' 'durable selected-candidate query failed'
        return
    }
    $dbBindings = if ($r.Output -eq '') { @() } else { @($r.Output -split ',') | Sort-Object }
    $apiJoined = $apiBindings -join ','
    $dbJoined = $dbBindings -join ','
    if ($apiJoined -eq $dbJoined) {
        Set-CheckResult 'api_selected_bindings_equal_durable_candidates' 'pass' "$(@($apiBindings).Count) selected binding(s) match exactly"
    } else {
        Set-CheckResult 'api_selected_bindings_equal_durable_candidates' 'fail' "api=[$apiJoined] db=[$dbJoined]"
    }
}

function Test-SelectedBindingsHaveFreshBarWindows {
    Write-Step 'Every selected binding has a ready, complete/fresh bar window'
    if ($null -eq $Script:DynamicSelectionStatus -or $null -eq $Script:MarketDataReadiness) {
        Set-CheckResult 'selected_bindings_have_fresh_bar_windows' 'fail' 'dynamic-selection/status or market-data/readiness unreachable'
        return
    }
    $selected = @($Script:DynamicSelectionStatus.selected)
    if (@($selected).Count -eq 0) {
        Set-CheckResult 'selected_bindings_have_fresh_bar_windows' 'fail' 'no selected bindings -- ActiveCommit requires at least one for paper_enforced_allowed'
        return
    }
    $assignments = @($Script:MarketDataReadiness.assignments)
    $notReady = @()
    foreach ($s in $selected) {
        $match = $assignments | Where-Object {
            $_.effective_runtime_target_symbol -eq $s.symbol -and
            $_.effective_runtime_strategy_id -eq $s.selected_strategy_id -and
            $_.effective_runtime_timeframe_secs -eq $s.timeframe_secs
        } | Select-Object -First 1
        if ($null -eq $match -or $match.readiness_state -ne 'ready') {
            $state = if ($null -eq $match) { 'no_matching_assignment' } else { $match.readiness_state }
            $notReady += "$($s.symbol)/$($s.selected_strategy_id)/$($s.timeframe_secs)=$state"
        }
    }
    if (@($notReady).Count -eq 0) {
        Set-CheckResult 'selected_bindings_have_fresh_bar_windows' 'pass' "$(@($selected).Count) selected binding(s) all ready"
    } else {
        Set-CheckResult 'selected_bindings_have_fresh_bar_windows' 'fail' ($notReady -join '; ')
    }
}

function Test-NoBindingMissingRequiredWindow {
    Write-Step 'No selected binding is absent from the readiness assignment set'
    if ($null -eq $Script:DynamicSelectionStatus -or $null -eq $Script:MarketDataReadiness) {
        Set-CheckResult 'no_binding_missing_required_window' 'fail' 'dynamic-selection/status or market-data/readiness unreachable'
        return
    }
    $selected = @($Script:DynamicSelectionStatus.selected)
    $assignments = @($Script:MarketDataReadiness.assignments)
    $missing = @()
    foreach ($s in $selected) {
        $match = $assignments | Where-Object {
            $_.effective_runtime_target_symbol -eq $s.symbol -and
            $_.effective_runtime_strategy_id -eq $s.selected_strategy_id -and
            $_.effective_runtime_timeframe_secs -eq $s.timeframe_secs
        } | Select-Object -First 1
        if ($null -eq $match) {
            $missing += "$($s.symbol)/$($s.selected_strategy_id)/$($s.timeframe_secs)"
        }
    }
    if (@($missing).Count -eq 0) {
        Set-CheckResult 'no_binding_missing_required_window' 'pass' "every selected binding has a matching readiness assignment row"
    } else {
        Set-CheckResult 'no_binding_missing_required_window' 'fail' "binding(s) with no readiness assignment row: $($missing -join '; ')"
    }
}

function Test-ArmIntegrityReconciliationLiveRoutingFacts {
    Write-Step 'Arm/integrity/risk/deadman/kill-switch/DB/live-routing facts, by value'
    if ($null -eq $Script:ControlStatus -or $null -eq $Script:SystemStatus) {
        Set-CheckResult 'arm_integrity_reconciliation_live_routing_facts' 'fail' 'control/status or system/status unreachable'
        return
    }
    $problems = @()
    if ($Script:ControlStatus.risk_blocked -eq $true) { $problems += "risk_blocked=true ($($Script:ControlStatus.risk_reason))" }
    if ([string]$Script:ControlStatus.deadman_armed_state -eq 'HALTED') { $problems += 'deadman_armed_state=HALTED' }
    if ($Script:SystemStatus.kill_switch_active -eq $true) { $problems += 'kill_switch_active=true' }
    if ($Script:SystemStatus.risk_halt_active -eq $true) { $problems += 'risk_halt_active=true' }
    if ($Script:SystemStatus.integrity_halt_active -eq $true) { $problems += 'integrity_halt_active=true' }
    if ([string]$Script:SystemStatus.db_status -ne 'ok') { $problems += "db_status=$($Script:SystemStatus.db_status) (expected ok)" }
    if ($null -ne $Script:SystemStatus.live_routing_enabled -and $Script:SystemStatus.live_routing_enabled -eq $true) {
        $problems += 'live_routing_enabled=true'
    }
    if ([string]$Script:SystemStatus.daemon_mode -ne 'paper') { $problems += "daemon_mode=$($Script:SystemStatus.daemon_mode) (expected paper)" }
    if (@($problems).Count -eq 0) {
        Set-CheckResult 'arm_integrity_reconciliation_live_routing_facts' 'pass' 'risk/deadman/kill-switch/integrity/db/live-routing/mode facts all clean'
    } else {
        Set-CheckResult 'arm_integrity_reconciliation_live_routing_facts' 'fail' ($problems -join '; ')
    }
}

function Test-ApiMatchesDbEvidence {
    Write-Step 'API committed truth (truth_state, disposition) matches DB evidence'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'api_matches_db_evidence' 'fail' 'no dynamic-selection/status response captured'
        return
    }
    $planId = $Script:DynamicSelectionStatus.committed_plan_id
    if ($null -eq $planId) {
        Set-CheckResult 'api_matches_db_evidence' 'fail' 'no committed_plan_id -- cannot cross-check'
        return
    }
    if (-not (Test-PsqlUsable)) {
        Set-CheckResult 'api_matches_db_evidence' 'fail' 'psql unavailable/DB URL unparsable/RequireDb=false -- cannot cross-check'
        return
    }
    $r = Invoke-PsqlScalar "select truth_state || '|' || disposition from sys_dynamic_selection_plans where plan_id = '$planId';"
    if ($r.ExitCode -ne 0 -or $r.Output -eq '') {
        Set-CheckResult 'api_matches_db_evidence' 'fail' "no matching DB row for plan_id=$planId"
        return
    }
    $dbParts = $r.Output -split '\|', 2
    $dbTruthState = $dbParts[0]
    $dbDisposition = if ($dbParts.Length -gt 1) { $dbParts[1] } else { '' }
    $apiTruthState = [string]$Script:DynamicSelectionStatus.committed_plan_truth_state
    $apiDisposition = [string]$Script:DynamicSelectionStatus.disposition
    if ($dbTruthState -eq $apiTruthState -and $dbDisposition -eq $apiDisposition) {
        Set-CheckResult 'api_matches_db_evidence' 'pass' "API truth_state/disposition match DB row for plan_id=$planId"
    } else {
        Set-CheckResult 'api_matches_db_evidence' 'fail' "API truth_state=$apiTruthState/disposition=$apiDisposition DB truth_state=$dbTruthState/disposition=$dbDisposition"
    }
}

# ---------------------------------------------------------------------------
# Shared checks
# ---------------------------------------------------------------------------

function Test-Phase7AndBundleGuardsPass {
    Write-Step 'Phase 7A/7B, Bundle 5/6, migration, unsafe-pattern, bypass, and final Bundle 7 guards'
    $guards = @(
        @{ Path = 'scripts/guards/check_phase7a_final_closure.ps1'; Kind = 'ps1' },
        @{ Path = 'scripts/guards/check_phase7b_selected_host_dispatch_closure.ps1'; Kind = 'ps1' },
        @{ Path = 'scripts/guards/check_runtime_opportunity_allocation_01.sh'; Kind = 'sh' },
        @{ Path = 'scripts/guards/check_multi_strategy_conflict_policy_01.sh'; Kind = 'sh' },
        @{ Path = 'scripts/guards/check_migration_governance.sh'; Kind = 'sh' },
        @{ Path = 'scripts/guards/check_unsafe_patterns.ps1'; Kind = 'ps1' },
        @{ Path = 'scripts/guards/check_no_promotion_evidence_bypass.ps1'; Kind = 'ps1' },
        @{ Path = 'scripts/guards/check_no_phase7a_production_effects_bypass.ps1'; Kind = 'ps1' },
        @{ Path = 'scripts/guards/check_bundle7_phase7c_final_closure.ps1'; Kind = 'ps1' }
    )
    $failed = @()
    $missing = @()
    foreach ($g in $guards) {
        $full = Join-Path $RepoRoot $g.Path
        if (-not (Test-Path $full)) {
            $missing += $g.Path
            continue
        }
        if ($g.Kind -eq 'sh') {
            $exitCode = Invoke-BashScript -ScriptPath $full
        } else {
            # Windows PowerShell 5.1 promotes a nested child process's own
            # stderr (e.g. a delegated guard's own cargo build progress
            # text) into a terminating error under $ErrorActionPreference=
            # 'Stop', regardless of the `*>` stream redirection below --
            # the redirection controls where the text goes, not whether
            # PowerShell treats its presence as an error. Relax EAP for
            # just this one external-process call, then restore it.
            $prevEap = $ErrorActionPreference
            $ErrorActionPreference = 'Continue'
            & powershell -NoProfile -ExecutionPolicy Bypass -File $full *> $null
            $exitCode = $LASTEXITCODE
            $ErrorActionPreference = $prevEap
        }
        if ($exitCode -ne 0) {
            $failed += $g.Path
        }
    }
    if (@($failed).Count -eq 0 -and @($missing).Count -eq 0) {
        Set-CheckResult 'phase7_and_bundle_guards_pass' 'pass' "$(@($guards).Count) guards passed"
    } else {
        $detail = @()
        if (@($failed).Count -gt 0) { $detail += "failed: $($failed -join ', ')" }
        if (@($missing).Count -gt 0) { $detail += "missing: $($missing -join ', ')" }
        Set-CheckResult 'phase7_and_bundle_guards_pass' 'fail' ($detail -join ' | ')
    }
}

function Test-NoTradingActionInvoked {
    param([string]$SourceText)
    Write-Step 'No trading/order action is invoked by this script'
    if ($null -eq $SourceText -or $SourceText -eq '') {
        Set-CheckResult 'no_trading_action_invoked' 'fail' 'self-source unavailable -- could not self-scan'
        return
    }
    # Structural check, not a denylist scan (a denylist of banned method
    # names would trivially self-match its own definition). Every HTTP call
    # this script makes goes through Invoke-DaemonGetOnly, which must be the
    # only call site of Invoke-RestMethod in the whole file, and that one
    # call site must hard-code -Method Get.
    # NB: the fail-message text below must never itself contain the literal
    # adjacent phrase "Invoke-RestMethod" + whitespace + "-Uri" -- that
    # would make this check self-match its own diagnostic text and always
    # report a spurious extra call site.
    $callSites = [regex]::Matches($SourceText, 'Invoke-RestMethod\s+-Uri')
    if ($callSites.Count -ne 1) {
        Set-CheckResult 'no_trading_action_invoked' 'fail' "expected exactly one GET-only Invoke-RestMethod call site (the '-Uri'-pattern one), found $($callSites.Count)"
        return
    }
    $callLineStart = $callSites[0].Index
    $callLineEnd = [Math]::Min($SourceText.Length, $callLineStart + 200)
    $callContext = $SourceText.Substring($callLineStart, $callLineEnd - $callLineStart)
    if ($callContext -match '-Method\s+Get\b') {
        Set-CheckResult 'no_trading_action_invoked' 'pass' 'the one Invoke-RestMethod call site is hard-coded -Method Get'
    } else {
        Set-CheckResult 'no_trading_action_invoked' 'fail' 'the one Invoke-RestMethod call site is not hard-coded -Method Get'
    }
}

# ---------------------------------------------------------------------------
# Secret scan, shared by both artifact writers.
# ---------------------------------------------------------------------------
$Script:SecretPatterns = @(
    'ALPACA_API_KEY', 'ALPACA_API_SECRET', 'ALPACA_SECRET',
    'MQK_OPERATOR_TOKEN', 'MQK_DATABASE_URL', 'DISCORD_WEBHOOK',
    'Authorization:', 'Bearer ', 'password', 'api_secret', '.env.local'
)
function Find-SecretShapedPattern {
    param([string]$Text)
    if ($null -eq $Text) { return $null }
    foreach ($pattern in $Script:SecretPatterns) {
        if ($Text.IndexOf($pattern, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            return $pattern
        }
    }
    return $null
}

function Get-Sha256Hex {
    param([string]$Text)
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($Text)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hashBytes = $sha.ComputeHash($bytes)
        return [System.BitConverter]::ToString($hashBytes).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

# ---------------------------------------------------------------------------
# PreStart artifact -- never a formal session manifest, explicitly states
# run_id/plan_id are not yet committed, never written to
# bundle7_soak_session_manifest.json (the ActiveCommit-only countable
# filename).
# ---------------------------------------------------------------------------
function New-Bundle7PrestartArtifact {
    param([string]$Verdict)

    $guardSummary = [ordered]@{}
    foreach ($name in $Script:PreStartCheckOrder) {
        if ($name -eq 'prestart_artifact_validates') { continue }
        if (-not $Script:Results.Contains($name)) {
            $guardSummary[$name] = 'not_run'
            continue
        }
        $guardSummary[$name] = $Script:Results[$name].status
    }

    $identityParts = [ordered]@{
        stage              = 'prestart'
        accepted_code_sha  = $ExpectedSha
        schema_version     = $LibrarySchemaVersion
        market_date        = $MarketDate
        session            = $Session
        guard_summary      = $guardSummary
        verdict            = $Verdict
    }
    $identityJson = $identityParts | ConvertTo-Json -Depth 20 -Compress
    $artifactId = Get-Sha256Hex -Text $identityJson

    return [ordered]@{
        schema_version                  = $SchemaVersion
        stage                           = 'prestart'
        artifact_id                     = $artifactId
        accepted_code_sha               = $ExpectedSha
        bundle7_evidence_schema_version = $LibrarySchemaVersion
        generated_at_utc                = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        intended_market_date            = $MarketDate
        intended_session                = $Session
        run_id                          = $null
        plan_id                         = $null
        not_a_formal_session_manifest   = $true
        statement                       = 'This PreStart artifact does not authorize or count a formal soak session. run_id and plan_id are not yet committed. Only a passing ActiveCommit gate run after the runtime has started a run may write the formal, countable session manifest.'
        readiness_verdict               = $Verdict
        guard_summary                   = $guardSummary
        approved_for_live               = $false
        no_live_capital_statement       = 'This artifact authorizes no live capital deployment.'
    }
}

function Test-PrestartArtifactValidates {
    param([string]$PendingVerdict)
    Write-Step 'Generated PreStart artifact validates'
    $artifact = New-Bundle7PrestartArtifact -Verdict $PendingVerdict
    $json = $artifact | ConvertTo-Json -Depth 20
    $secretHit = Find-SecretShapedPattern -Text $json
    if ($null -ne $secretHit) {
        Set-CheckResult 'prestart_artifact_validates' 'fail' "artifact matched secret-shaped pattern (category: $secretHit); not written"
        return $null
    }
    if ($artifact.approved_for_live -ne $false -or $null -ne $artifact.run_id -or $null -ne $artifact.plan_id) {
        Set-CheckResult 'prestart_artifact_validates' 'fail' 'artifact must carry approved_for_live=false and null run_id/plan_id'
        return $null
    }
    Set-CheckResult 'prestart_artifact_validates' 'pass' "artifact_id=$($artifact.artifact_id)"
    return $artifact
}

# ---------------------------------------------------------------------------
# ActiveCommit formal manifest -- the only artifact this script ever writes
# that may count a soak session. Every fact is bound from verified live/DB
# truth; never hard-coded.
# ---------------------------------------------------------------------------
function New-Bundle7ActiveCommitManifest {
    param([string]$Verdict)

    $runId = if ($null -ne $Script:ControlStatus) { $Script:ControlStatus.active_run_id } else { $null }
    $planId = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.committed_plan_id } else { $null }
    $validationState = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.evidence_validation_state } else { $null }
    $configuredMode = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.committed_configured_mode } else { $null }
    $effectiveMode = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.committed_effective_mode } else { $null }
    $disposition = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.disposition } else { $null }
    $liveLock = if ($null -ne $Script:DynamicSelectionStatus) { [bool]$Script:DynamicSelectionStatus.committed_live_lock_applied } else { $true }
    $sourceKind = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.committed_source_kind } else { $null }
    $sourceIdentity = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.committed_source_identity } else { $null }
    $deploymentMode = if ($null -ne $Script:SystemStatus) { $Script:SystemStatus.daemon_mode } else { $null }
    $adapterId = if ($null -ne $Script:SystemStatus) { $Script:SystemStatus.adapter_id } else { $null }
    $liveRoutingEnabled = if ($null -ne $Script:SystemStatus) { $Script:SystemStatus.live_routing_enabled } else { $null }
    $leaseHolderId = if ($null -ne $Script:ControlStatus) { $Script:ControlStatus.leader_holder_id } else { $null }
    $leaseExpired = if ($null -ne $Script:ControlStatus) { $Script:ControlStatus.lease_expired } else { $null }
    $integrityState = if ($null -ne $Script:ControlStatus) { $Script:ControlStatus.integrity_state } else { $null }
    $riskBlocked = if ($null -ne $Script:ControlStatus) { $Script:ControlStatus.risk_blocked } else { $null }
    $reconcileStatusValue = if ($null -ne $Script:ReconcileStatus) { $Script:ReconcileStatus.status } else { $null }
    $reconcileTruthState = if ($null -ne $Script:ReconcileStatus) { $Script:ReconcileStatus.truth_state } else { $null }

    $selectedBindings = @()
    if ($null -ne $Script:DynamicSelectionStatus) {
        foreach ($s in @($Script:DynamicSelectionStatus.selected)) {
            $selectedBindings += [ordered]@{
                symbol               = $s.symbol
                selected_strategy_id = $s.selected_strategy_id
                timeframe_secs       = $s.timeframe_secs
                reason_code          = $s.reason_code
            }
        }
    }
    $requiredMarketDataPairs = @()
    if ($null -ne $Script:MarketDataReadiness) {
        foreach ($a in @($Script:MarketDataReadiness.assignments)) {
            $requiredMarketDataPairs += [ordered]@{
                symbol         = $a.assignment_symbol
                timeframe_secs = $a.effective_runtime_timeframe_secs
                readiness_state = $a.readiness_state
            }
        }
    }

    $guardSummary = [ordered]@{}
    foreach ($name in $Script:ActiveCommitCheckOrder) {
        if ($name -eq 'active_commit_manifest_validates') { continue }
        if (-not $Script:Results.Contains($name)) {
            $guardSummary[$name] = 'not_run'
            continue
        }
        $guardSummary[$name] = $Script:Results[$name].status
    }

    $identityParts = [ordered]@{
        stage              = 'active_commit'
        accepted_code_sha  = $ExpectedSha
        schema_version     = $LibrarySchemaVersion
        market_date        = $MarketDate
        session            = $Session
        deployment_mode    = $deploymentMode
        configured_mode    = $configuredMode
        effective_mode     = $effectiveMode
        disposition        = $disposition
        live_lock_applied  = $liveLock
        run_id             = $runId
        plan_id            = $planId
        validation_state   = $validationState
        selected_bindings  = ($selectedBindings | Sort-Object { $_.symbol })
        guard_summary      = $guardSummary
        verdict            = $Verdict
    }
    $identityJson = $identityParts | ConvertTo-Json -Depth 20 -Compress
    $manifestId = Get-Sha256Hex -Text $identityJson

    return [ordered]@{
        schema_version                  = $SchemaVersion
        stage                           = 'active_commit'
        manifest_id                     = $manifestId
        accepted_code_sha               = $ExpectedSha
        bundle7_evidence_schema_version = $LibrarySchemaVersion
        generated_at_utc                = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        intended_market_date            = $MarketDate
        intended_session                = $Session
        deployment_mode                 = $deploymentMode
        adapter_id                      = $adapterId
        live_routing_enabled            = $liveRoutingEnabled
        configured_mode                 = $configuredMode
        effective_mode                  = $effectiveMode
        disposition                     = $disposition
        live_lock_applied               = $liveLock
        approved_for_live               = $false
        run_id                          = $runId
        plan_id                         = $planId
        durable_validation_state        = $validationState
        selected_bindings               = $selectedBindings
        source_kind                     = $sourceKind
        source_identity                 = $sourceIdentity
        required_market_data_pairs      = $requiredMarketDataPairs
        run_lease_coherence             = [ordered]@{ leader_holder_id = $leaseHolderId; lease_expired = $leaseExpired }
        arm_integrity_verdict           = [ordered]@{ integrity_state = $integrityState; risk_blocked = $riskBlocked }
        reconciliation_verdict          = [ordered]@{ status = $reconcileStatusValue; truth_state = $reconcileTruthState }
        readiness_verdict               = $Verdict
        migration_summary               = [ordered]@{ status = $Script:Results['migration_governance'].status }
        guard_summary                   = $guardSummary
        no_live_capital_statement       = 'This manifest authorizes no live capital deployment. approved_for_live is hard-false throughout Bundle 7; this manifest never grants trading authority on its own.'
    }
}

function Test-ActiveCommitManifestValidates {
    param([string]$PendingVerdict)
    Write-Step 'Generated ActiveCommit session manifest validates'
    $manifest = New-Bundle7ActiveCommitManifest -Verdict $PendingVerdict
    $json = $manifest | ConvertTo-Json -Depth 20
    $secretHit = Find-SecretShapedPattern -Text $json
    if ($null -ne $secretHit) {
        Set-CheckResult 'active_commit_manifest_validates' 'fail' "manifest matched secret-shaped pattern (category: $secretHit); not written"
        return $null
    }
    if ($manifest.approved_for_live -ne $false) {
        Set-CheckResult 'active_commit_manifest_validates' 'fail' 'manifest approved_for_live is not false'
        return $null
    }
    if ($null -eq $manifest.run_id -or $null -eq $manifest.plan_id) {
        Set-CheckResult 'active_commit_manifest_validates' 'fail' 'manifest run_id/plan_id is null -- a formal session manifest must never be written without both'
        return $null
    }
    if (@($manifest.selected_bindings).Count -eq 0) {
        Set-CheckResult 'active_commit_manifest_validates' 'fail' 'manifest has zero selected_bindings -- a formal session manifest must never be written without them'
        return $null
    }
    if ($manifest.schema_version -ne $SchemaVersion -or $manifest.manifest_id -eq '') {
        Set-CheckResult 'active_commit_manifest_validates' 'fail' 'manifest schema_version/manifest_id malformed'
        return $null
    }
    Set-CheckResult 'active_commit_manifest_validates' 'pass' "manifest_id=$($manifest.manifest_id)"
    return $manifest
}

# ---------------------------------------------------------------------------
# Run checks in order, per stage.
# ---------------------------------------------------------------------------
Test-HeadEqualsAcceptedSha
Test-TrackedWorktreeClean
Test-MigrationGovernance
Test-RequireApiAndDbTrue
Test-ExpectedDbReachable

Initialize-LiveFacts

if ($Stage -eq 'PreStart') {
    Test-NoConflictingActiveOrStartingRun
    Test-NoUnexpiredLeaderLease
    Test-DeploymentPaperBrokerConfig
    Test-LiveRoutingCapitalDisabled
    Test-DynamicSelectionModePreviewPaperEnforced
    Test-ArmIntegrityPostureSuitableForStart
    Test-ReconciliationTruthAcceptable
    Test-RequiredMarketDataPairsCompleteAndFresh
} else {
    Test-LifecycleStartingOrRunning
    Test-ExactlyOneCommittedActiveRun
    Test-RunLeaseCoherent
    Test-CommittedDispositionPaperEnforcedAllowed
    Test-CommittedEffectiveModePaperEnforced
    Test-CommittedLiveLockFalse
    Test-ApprovedForLiveFalseHard
    Test-CommittedPlanIdPresent
    Test-EvidencePersistedTrue
    Test-EvidenceValidationStateValid
    Test-ValidationBlockersEmpty
    Test-ApiSelectedBindingsEqualDurableCandidates
    Test-SelectedBindingsHaveFreshBarWindows
    Test-NoBindingMissingRequiredWindow
    Test-ArmIntegrityReconciliationLiveRoutingFacts
    Test-ReconciliationTruthAcceptable
    Test-ApiMatchesDbEvidence
}

Test-Phase7AndBundleGuardsPass

$Script:SelfSourceText = $null
$Script:SelfSourcePath = $MyInvocation.MyCommand.Path
if ($null -ne $Script:SelfSourcePath -and (Test-Path $Script:SelfSourcePath)) {
    $Script:SelfSourceText = Get-Content -Raw -Path $Script:SelfSourcePath
}
Test-NoTradingActionInvoked -SourceText $Script:SelfSourceText

# ---------------------------------------------------------------------------
# Report + verdict. No status other than 'pass' contributes to a PASS
# verdict -- 'fail' and any missing/not-run check are both hard blockers.
# The artifact/manifest check runs last since it needs the pending verdict
# from every prior check.
# ---------------------------------------------------------------------------
$FinalCheckName = if ($Stage -eq 'PreStart') { 'prestart_artifact_validates' } else { 'active_commit_manifest_validates' }
$OtherChecks = @($Script:CheckOrder | Where-Object { $_ -ne $FinalCheckName })
$HardFailures = @($OtherChecks | Where-Object { $Script:Results.Contains($_) -and $Script:Results[$_].status -eq 'fail' })
# A check whose call site vanished (no silent skip) must block the
# artifact/manifest exactly like a genuine FAIL -- never let a missing check
# slip through as an implicit pass just because $Script:Results never got
# that key.
$MissingBeforeFinal = @($OtherChecks | Where-Object { -not $Script:Results.Contains($_) })
$PendingVerdict = if (@($HardFailures).Count -eq 0 -and @($MissingBeforeFinal).Count -eq 0) { 'PASS' } else { 'FAIL' }

if ($Stage -eq 'PreStart') {
    $Artifact = Test-PrestartArtifactValidates -PendingVerdict $PendingVerdict
    if ($null -ne $Artifact -and $PendingVerdict -eq 'PASS') {
        New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
        $ArtifactPath = Join-Path $OutputDirectory 'bundle7_prestart_readiness_manifest.json'
        ($Artifact | ConvertTo-Json -Depth 20) | Set-Content -Path $ArtifactPath -Encoding UTF8
        Write-Host ""
        Write-Host "PreStart artifact written (not a countable session manifest): $ArtifactPath" -ForegroundColor Green
    }
} else {
    $Manifest = Test-ActiveCommitManifestValidates -PendingVerdict $PendingVerdict
    if ($null -ne $Manifest -and $PendingVerdict -eq 'PASS') {
        New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
        $ManifestPath = Join-Path $OutputDirectory 'bundle7_soak_session_manifest.json'
        ($Manifest | ConvertTo-Json -Depth 20) | Set-Content -Path $ManifestPath -Encoding UTF8
        Write-Host ""
        Write-Host "Formal countable session manifest written: $ManifestPath" -ForegroundColor Green
    }
}

Write-Host ""
Write-Host "=== Check results ($Stage) ===" -ForegroundColor Cyan
foreach ($name in $Script:CheckOrder) {
    if (-not $Script:Results.Contains($name)) {
        Write-Fail "$name -- NOT RUN (must never happen -- no silent skip)"
        continue
    }
    $r = $Script:Results[$name]
    switch ($r.status) {
        'pass' { Write-Ok "$name -- $($r.detail)" }
        'not_applicable' { Write-NotApplicable "$name -- $($r.detail)" }
        'fail' { Write-Fail "$name -- $($r.detail)" }
    }
}

$AllFailures = @($Script:CheckOrder | Where-Object { $Script:Results.Contains($_) -and $Script:Results[$_].status -eq 'fail' })
$NotRun = @($Script:CheckOrder | Where-Object { -not $Script:Results.Contains($_) })

Write-Host ""
if (@($AllFailures).Count -eq 0 -and @($NotRun).Count -eq 0) {
    Write-Host "FINAL: PASS" -ForegroundColor Green
    exit 0
} else {
    Write-Host "FINAL: FAIL" -ForegroundColor Red
    exit 1
}
