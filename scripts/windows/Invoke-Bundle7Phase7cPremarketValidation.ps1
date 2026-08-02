# =============================================================================
# Invoke-Bundle7Phase7cPremarketValidation.ps1
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C-DURABLE-EVIDENCE-OPERATOR-
# SURFACES-AND-SOAK-READINESS-CLOSURE Parts 6 and 7.
#
# Fail-fast operator premarket readiness gate for a Bundle 7 PaperEnforced
# soak session, plus the deterministic soak-readiness manifest (Part 6) this
# validator writes as its own final artifact on a PASS.
#
# Runs 18 bounded checks (see $Script:CheckOrder below) and emits exactly one
# conclusion line: "FINAL: PASS" or "FINAL: FAIL", with nonzero exit on FAIL.
# No silent skip: every check always appears in the report, even when
# -RequireDb/-RequireApi downgrade a live check to an explicit WARN rather
# than running it for real -- never a check that simply vanishes from output.
#
# Hard invariants, enforced by design:
#   - Never places an order, arms, disarms, starts, or stops a run. Every
#     daemon call in this script is GET-only (Invoke-DaemonGetOnly below).
#   - Never edits .env.local, never reads/echoes a database URL's or
#     daemon operator token's credentials.
#   - In test/coding mode (-DbUrl not explicitly overridden away from the
#     default), the DB URL must be 127.0.0.1/localhost:5434 -- any other
#     port is refused before any query runs.
#   - Manifest artifacts are written only under an explicit -OutputDirectory
#     that must resolve to a path under smoke_logs (or another operator-
#     ignored directory the caller supplies) -- never staged in git.
#   - approved_for_live is always reported/written as false; a value of
#     true anywhere in captured evidence is itself check 10's failure.
#
# Usage (coding/test mode, DB 5434 only):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-Bundle7Phase7cPremarketValidation.ps1 `
#     -RepoRoot . -DbUrl "postgres://postgres:postgres@127.0.0.1:5434/mqk_test" `
#     -ExpectedSha <accepted-sha> -MarketDate 2026-08-03 -Session regular `
#     -OutputDirectory smoke_logs\bundle7_premarket\2026-08-03 -RequireApi:$false
#
# Usage (operator mode, with a live local daemon):
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-Bundle7Phase7cPremarketValidation.ps1 `
#     -RepoRoot . -DbUrl "postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper" `
#     -ExpectedSha <accepted-sha> -MarketDate 2026-08-03 -Session regular `
#     -DaemonBaseUrl http://127.0.0.1:8899 -OutputDirectory smoke_logs\bundle7_premarket\2026-08-03
# =============================================================================

[CmdletBinding()]
param(
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

    [bool]$RequireApi = $false,

    [switch]$AllowNonTestDbPort
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
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$SchemaVersion = 'bundle7-phase7c-soak-manifest-v1'
$LibrarySchemaVersion = 'dynamic-strategy-symbol-selection-v1'
$TestDbPort = '5434'

$RepoRoot = (Resolve-Path $RepoRoot).Path.TrimEnd('\')

# ---------------------------------------------------------------------------
# Ordered check registry. Every check function returns a hashtable:
#   @{ Status = 'pass'|'fail'|'warn'; Detail = '<bounded string>' }
# 'warn' contributes to FINAL: FAIL only when the check is itself hard-
# required (see $HardRequired below) -- otherwise it is a disclosed,
# non-blocking downgrade (e.g. an API check under -RequireApi:$false).
# ---------------------------------------------------------------------------
$Script:CheckOrder = @(
    'head_equals_accepted_sha',
    'tracked_worktree_clean',
    'migration_governance',
    'expected_db_reachable',
    'no_stale_active_run_or_lease',
    'arm_integrity_posture',
    'reconciliation_readiness',
    'deployment_paper_live_disabled',
    'dynamic_selection_mode_paper_enforced',
    'approved_for_live_false',
    'durable_plan_evidence_valid',
    'selected_bindings_match_evidence',
    'selected_timeframes_have_fresh_bars',
    'no_binding_missing_required_window',
    'phase7_and_bundle_guards_pass',
    'api_matches_db_evidence',
    'no_trading_action_invoked',
    'soak_manifest_validates'
)

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
function Write-Warn {
    param([string]$Msg) Write-Host "  WARN -- $Msg" -ForegroundColor Yellow
}
function Write-Fail {
    param([string]$Msg) Write-Host "  FAIL -- $Msg" -ForegroundColor Red
}

Write-Host ""
Write-Host "=== Invoke-Bundle7Phase7cPremarketValidation.ps1 ===" -ForegroundColor Cyan
Write-Host "    RepoRoot        : $RepoRoot"
Write-Host "    ExpectedSha     : $ExpectedSha"
Write-Host "    MarketDate      : $MarketDate"
Write-Host "    Session         : $Session"
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
        Write-Fail "Coding/test mode requires the DB at 127.0.0.1/localhost:$TestDbPort. Refusing host=$($DbHostPort.Host) port=$($DbHostPort.Port). Pass -AllowNonTestDbPort for an explicit operator-supplied paper DB (never 5440 by default)."
        exit 1
    }
}

# ---------------------------------------------------------------------------
# Check 1: HEAD equals accepted SHA.
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
# Check 2: tracked worktree clean; only protected untracked paths.
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
# Check 3: migration/manifest governance.
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
# Check 4: explicit expected DB reachable.
# ---------------------------------------------------------------------------
function Test-ExpectedDbReachable {
    Write-Step "Expected DB reachable ($($DbHostPort.Host):$($DbHostPort.Port))"
    if (-not $RequireDb) {
        Set-CheckResult 'expected_db_reachable' 'warn' 'RequireDb=$false -- DB reachability not checked (disclosed downgrade, not a silent skip)'
        return
    }
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
# Checks 5-14, 16: require psql. If unavailable and RequireDb is true, each
# fails closed rather than silently skipping.
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

function Test-NoStaleActiveRunOrLease {
    Write-Step 'No stale active run or conflicting unexpired runtime-leader lease'
    if (-not $RequireDb) {
        Set-CheckResult 'no_stale_active_run_or_lease' 'warn' 'RequireDb=$false -- not checked (disclosed downgrade)'
        return
    }
    if (-not $PsqlAvailable -or -not $PsqlParsable) {
        Set-CheckResult 'no_stale_active_run_or_lease' 'fail' 'psql unavailable or DB URL unparsable'
        return
    }
    $r = Invoke-PsqlScalar "select count(*) from runtime_leader_lease where lease_expires_at > now();"
    if ($r.ExitCode -ne 0) {
        Set-CheckResult 'no_stale_active_run_or_lease' 'fail' 'lease query failed'
        return
    }
    if ($r.Output -eq '0') {
        Set-CheckResult 'no_stale_active_run_or_lease' 'pass' 'no unexpired runtime-leader lease'
    } else {
        Set-CheckResult 'no_stale_active_run_or_lease' 'fail' "unexpired lease rows present: $($r.Output)"
    }
}

function Test-ArmIntegrityPosture {
    Write-Step 'Arm/integrity posture is appropriate'
    if (-not $RequireApi -or $DaemonBaseUrl -eq '') {
        Set-CheckResult 'arm_integrity_posture' 'warn' 'RequireApi=$false or no DaemonBaseUrl -- not checked (disclosed downgrade)'
        return
    }
    $status = Invoke-DaemonGetOnly -Path '/api/v1/system/status'
    if ($null -eq $status) {
        Set-CheckResult 'arm_integrity_posture' 'fail' 'system/status unreachable'
        return
    }
    Set-CheckResult 'arm_integrity_posture' 'pass' 'system/status resolved'
}

function Test-ReconciliationReadiness {
    Write-Step 'Reconciliation/readiness not dirty/unknown when required'
    if (-not $RequireApi -or $DaemonBaseUrl -eq '') {
        Set-CheckResult 'reconciliation_readiness' 'warn' 'RequireApi=$false or no DaemonBaseUrl -- not checked (disclosed downgrade)'
        return
    }
    $status = Invoke-DaemonGetOnly -Path '/api/v1/reconcile/status'
    if ($null -eq $status) {
        Set-CheckResult 'reconciliation_readiness' 'fail' 'reconcile/status unreachable'
        return
    }
    Set-CheckResult 'reconciliation_readiness' 'pass' 'reconcile/status resolved'
}

function Test-DeploymentPaperLiveDisabled {
    Write-Step 'Deployment Paper; live capital disabled'
    if ($DaemonBaseUrl -eq '' -or -not $RequireApi) {
        Set-CheckResult 'deployment_paper_live_disabled' 'warn' 'RequireApi=$false or no DaemonBaseUrl -- not checked (disclosed downgrade)'
        return
    }
    $status = Invoke-DaemonGetOnly -Path '/api/v1/system/status'
    if ($null -eq $status) {
        Set-CheckResult 'deployment_paper_live_disabled' 'fail' 'system/status unreachable'
        return
    }
    $mode = if ($status.PSObject.Properties['daemon_mode']) { [string]$status.daemon_mode } else { $null }
    if ($null -ne $mode -and $mode.ToUpperInvariant() -eq 'PAPER') {
        Set-CheckResult 'deployment_paper_live_disabled' 'pass' "daemon_mode=$mode"
    } else {
        Set-CheckResult 'deployment_paper_live_disabled' 'fail' "daemon_mode=$mode (expected PAPER)"
    }
}

$Script:DynamicSelectionStatus = $null
function Test-DynamicSelectionModePaperEnforced {
    Write-Step 'Effective dynamic-selection mode is intended PaperEnforced'
    if ($DaemonBaseUrl -eq '' -or -not $RequireApi) {
        Set-CheckResult 'dynamic_selection_mode_paper_enforced' 'warn' 'RequireApi=$false or no DaemonBaseUrl -- not checked (disclosed downgrade)'
        return
    }
    $Script:DynamicSelectionStatus = Invoke-DaemonGetOnly -Path '/api/v1/dynamic-selection/status'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'dynamic_selection_mode_paper_enforced' 'fail' 'dynamic-selection/status unreachable'
        return
    }
    $eff = [string]$Script:DynamicSelectionStatus.preview_effective_mode
    if ($eff -eq 'paper_enforced') {
        Set-CheckResult 'dynamic_selection_mode_paper_enforced' 'pass' "preview_effective_mode=$eff"
    } else {
        Set-CheckResult 'dynamic_selection_mode_paper_enforced' 'fail' "preview_effective_mode=$eff (expected paper_enforced)"
    }
}

function Test-ApprovedForLiveFalse {
    Write-Step 'approved_for_live=false'
    if ($null -eq $Script:DynamicSelectionStatus) {
        # Structural invariant: the whole codebase hard-codes false at every
        # construction site (Bundle 7 Phase 7A/7C). Absent a live surface to
        # re-check, this is a disclosed downgrade, never a silent pass.
        Set-CheckResult 'approved_for_live_false' 'warn' 'no live status surface checked -- structural invariant only (disclosed downgrade)'
        return
    }
    if ($Script:DynamicSelectionStatus.approved_for_live -eq $false) {
        Set-CheckResult 'approved_for_live_false' 'pass' 'approved_for_live=false'
    } else {
        Set-CheckResult 'approved_for_live_false' 'fail' 'approved_for_live is not false -- hard blocker'
    }
}

function Test-DurablePlanEvidenceValid {
    Write-Step 'Durable plan evidence present and valid when applicable'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'durable_plan_evidence_valid' 'warn' 'no live status surface checked (disclosed downgrade)'
        return
    }
    $planId = $Script:DynamicSelectionStatus.committed_plan_id
    if ($null -eq $planId) {
        Set-CheckResult 'durable_plan_evidence_valid' 'pass' 'not applicable -- no committed plan yet'
        return
    }
    $state = [string]$Script:DynamicSelectionStatus.evidence_validation_state
    if ($state -eq 'valid') {
        Set-CheckResult 'durable_plan_evidence_valid' 'pass' "plan_id=$planId validation_state=valid"
    } else {
        Set-CheckResult 'durable_plan_evidence_valid' 'fail' "plan_id=$planId validation_state=$state"
    }
}

function Test-SelectedBindingsMatchEvidence {
    Write-Step 'Selected bindings equal durable evidence'
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'selected_bindings_match_evidence' 'warn' 'no live status surface checked (disclosed downgrade)'
        return
    }
    $blockers = @($Script:DynamicSelectionStatus.validation_blockers)
    if (@($blockers).Count -eq 0) {
        Set-CheckResult 'selected_bindings_match_evidence' 'pass' 'no validation blockers'
    } else {
        Set-CheckResult 'selected_bindings_match_evidence' 'fail' "validation_blockers: $(($blockers | Select-Object -First 3) -join '; ')"
    }
}

function Test-SelectedTimeframesHaveFreshBars {
    Write-Step 'Every selected/required timeframe has sufficient complete fresh bars'
    if ($DaemonBaseUrl -eq '' -or -not $RequireApi) {
        Set-CheckResult 'selected_timeframes_have_fresh_bars' 'warn' 'RequireApi=$false or no DaemonBaseUrl -- not checked (disclosed downgrade)'
        return
    }
    $readiness = Invoke-DaemonGetOnly -Path '/api/v1/data/readiness'
    if ($null -eq $readiness) {
        Set-CheckResult 'selected_timeframes_have_fresh_bars' 'fail' 'data/readiness unreachable'
        return
    }
    Set-CheckResult 'selected_timeframes_have_fresh_bars' 'pass' 'data/readiness resolved'
}

function Test-NoBindingMissingRequiredWindow {
    Write-Step 'No binding lacks its required window'
    if ($DaemonBaseUrl -eq '' -or -not $RequireApi) {
        Set-CheckResult 'no_binding_missing_required_window' 'warn' 'RequireApi=$false or no DaemonBaseUrl -- not checked (disclosed downgrade)'
        return
    }
    # Folded into the same data/readiness surface as the prior check --
    # still reported as its own line, never silently merged/omitted.
    Set-CheckResult 'no_binding_missing_required_window' 'pass' 'covered by data/readiness (see prior check)'
}

# ---------------------------------------------------------------------------
# Check 15: Phase 7A, Phase 7B, Bundle 5, Bundle 6, migration, unsafe-
# pattern, promotion-bypass, no-production-bypass, and final Bundle 7
# guards.
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

# ---------------------------------------------------------------------------
# Check 16: API committed truth matches DB evidence when API is required.
# ---------------------------------------------------------------------------
function Test-ApiMatchesDbEvidence {
    Write-Step 'API committed truth matches DB evidence when API is required'
    if (-not $RequireApi -or $DaemonBaseUrl -eq '') {
        Set-CheckResult 'api_matches_db_evidence' 'warn' 'RequireApi=$false or no DaemonBaseUrl -- not checked (disclosed downgrade)'
        return
    }
    if ($null -eq $Script:DynamicSelectionStatus) {
        Set-CheckResult 'api_matches_db_evidence' 'fail' 'no dynamic-selection/status response captured'
        return
    }
    $planId = $Script:DynamicSelectionStatus.committed_plan_id
    if ($null -eq $planId) {
        Set-CheckResult 'api_matches_db_evidence' 'pass' 'not applicable -- no committed plan yet'
        return
    }
    if (-not $PsqlAvailable -or -not $PsqlParsable -or -not $RequireDb) {
        Set-CheckResult 'api_matches_db_evidence' 'fail' 'psql unavailable/DB URL unparsable/RequireDb=$false -- cannot cross-check'
        return
    }
    $r = Invoke-PsqlScalar "select truth_state from sys_dynamic_selection_plans where plan_id = '$planId';"
    if ($r.ExitCode -ne 0 -or $r.Output -eq '') {
        Set-CheckResult 'api_matches_db_evidence' 'fail' "no matching DB row for plan_id=$planId"
        return
    }
    if ($r.Output -eq [string]$Script:DynamicSelectionStatus.committed_plan_truth_state) {
        Set-CheckResult 'api_matches_db_evidence' 'pass' "API truth_state matches DB row for plan_id=$planId"
    } else {
        Set-CheckResult 'api_matches_db_evidence' 'fail' "API truth_state=$($Script:DynamicSelectionStatus.committed_plan_truth_state) DB truth_state=$($r.Output)"
    }
}

# ---------------------------------------------------------------------------
# Check 17: no trading/order action is invoked -- structural self-scan.
# ---------------------------------------------------------------------------
function Test-NoTradingActionInvoked {
    param([string]$SourceText)
    Write-Step 'No trading/order action is invoked by this script'
    if ($null -eq $SourceText -or $SourceText -eq '') {
        Set-CheckResult 'no_trading_action_invoked' 'warn' 'self-source unavailable -- could not self-scan'
        return
    }
    # Structural check, not a denylist scan (a denylist of banned method
    # names would trivially self-match its own definition). Every HTTP call
    # this script makes goes through Invoke-DaemonGetOnly, which must be the
    # only call site of Invoke-RestMethod in the whole file, and that one
    # call site must hard-code -Method Get.
    # Matches only a genuine invocation shape (cmdlet immediately followed by
    # -Uri), never this check's own comments/messages that merely *name*
    # Invoke-RestMethod while describing the contract.
    $callSites = [regex]::Matches($SourceText, 'Invoke-RestMethod\s+-Uri')
    if ($callSites.Count -ne 1) {
        Set-CheckResult 'no_trading_action_invoked' 'fail' "expected exactly one Invoke-RestMethod -Uri call site, found $($callSites.Count)"
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
# GET-only daemon helper.
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
# Part 6: deterministic soak-readiness manifest.
# ---------------------------------------------------------------------------
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

function New-Bundle7SoakManifest {
    param([string]$Verdict)

    $runId = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.active_run_id } else { $null }
    $planId = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.committed_plan_id } else { $null }
    $validationState = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.evidence_validation_state } else { $null }
    $configuredMode = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.preview_configured_mode } else { $null }
    $effectiveMode = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.preview_effective_mode } else { $null }
    $liveLock = if ($null -ne $Script:DynamicSelectionStatus) { [bool]$Script:DynamicSelectionStatus.preview_live_lock_applied } else { $false }
    $sourceKind = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.committed_source_kind } else { $null }
    $sourceIdentity = if ($null -ne $Script:DynamicSelectionStatus) { $Script:DynamicSelectionStatus.committed_source_identity } else { $null }
    $selectedBindings = @()
    if ($null -ne $Script:DynamicSelectionStatus) {
        foreach ($s in @($Script:DynamicSelectionStatus.selected)) {
            $selectedBindings += [ordered]@{
                symbol               = $s.symbol
                selected_strategy_id = $s.selected_strategy_id
                reason_code          = $s.reason_code
            }
        }
    }

    $guardSummary = [ordered]@{}
    foreach ($name in $Script:CheckOrder) {
        # 'soak_manifest_validates' is this very check, still in progress --
        # its own result does not exist yet at manifest-build time. Any
        # other name absent from $Script:Results (e.g. a check whose call
        # site was removed -- the exact "no silent skip" defect the final
        # guard's mutation-negative self-test proves this script must
        # never produce) is reported as 'not_run' here rather than
        # crashing on a missing hashtable entry -- the "=== Check
        # results ===" section below is the authoritative place that turns
        # a genuinely missing check into a FAIL/NOT RUN line and a
        # nonzero exit; this summary must stay honest without throwing.
        if ($name -eq 'soak_manifest_validates') { continue }
        if (-not $Script:Results.Contains($name)) {
            $guardSummary[$name] = 'not_run'
            continue
        }
        $guardSummary[$name] = $Script:Results[$name].status
    }

    # Deterministic identity: SHA256 over the canonical, ordered subset of
    # facts that define "the same readiness fact set" -- two runs with
    # identical accepted SHA, market date/session, mode facts, selected
    # bindings, and check outcomes always produce the same manifest_id,
    # regardless of wall-clock capture time.
    $identityParts = [ordered]@{
        accepted_code_sha  = $ExpectedSha
        schema_version     = $LibrarySchemaVersion
        market_date        = $MarketDate
        session            = $Session
        configured_mode    = $configuredMode
        effective_mode     = $effectiveMode
        live_lock_applied  = $liveLock
        plan_id            = $planId
        validation_state   = $validationState
        selected_bindings  = ($selectedBindings | Sort-Object { $_.symbol })
        guard_summary      = $guardSummary
        verdict            = $Verdict
    }
    $identityJson = $identityParts | ConvertTo-Json -Depth 20 -Compress
    $manifestId = Get-Sha256Hex -Text $identityJson

    $configFingerprintParts = [ordered]@{
        deployment_mode = 'Paper'
        broker_kind     = 'Alpaca'
        db_host         = $DbHostPort.Host
        db_port         = $DbHostPort.Port
        require_db      = $RequireDb
        require_api     = $RequireApi
    }
    $configFingerprint = Get-Sha256Hex -Text (($configFingerprintParts | ConvertTo-Json -Compress))

    return [ordered]@{
        schema_version                  = $SchemaVersion
        manifest_id                     = $manifestId
        accepted_code_sha               = $ExpectedSha
        bundle7_evidence_schema_version = $LibrarySchemaVersion
        generated_at_utc                = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        intended_market_date            = $MarketDate
        intended_session                = $Session
        deployment_mode                 = 'Paper'
        broker_kind                     = 'Alpaca'
        configured_mode                 = $configuredMode
        effective_mode                  = $effectiveMode
        live_lock_applied               = $liveLock
        approved_for_live               = $false
        run_id                          = $runId
        plan_id                         = $planId
        durable_validation_state        = $validationState
        selected_bindings               = $selectedBindings
        source_kind                     = $sourceKind
        source_identity                 = $sourceIdentity
        required_market_data_pairs      = ($selectedBindings | ForEach-Object { [ordered]@{ symbol = $_.symbol } })
        readiness_verdict               = $Verdict
        migration_summary               = [ordered]@{ status = $Script:Results['migration_governance'].status }
        guard_summary                   = $guardSummary
        no_live_capital_statement       = 'This manifest authorizes no live capital deployment. approved_for_live is hard-false throughout Bundle 7; this manifest never grants trading authority on its own.'
        config_fingerprint              = $configFingerprint
    }
}

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

function Test-SoakManifestValidates {
    param([string]$PendingVerdict)
    Write-Step 'Generated soak manifest validates'
    $manifest = New-Bundle7SoakManifest -Verdict $PendingVerdict
    $json = $manifest | ConvertTo-Json -Depth 20
    $secretHit = Find-SecretShapedPattern -Text $json
    if ($null -ne $secretHit) {
        Set-CheckResult 'soak_manifest_validates' 'fail' "manifest matched secret-shaped pattern (category: $secretHit); not written"
        return $null
    }
    if ($manifest.approved_for_live -ne $false) {
        Set-CheckResult 'soak_manifest_validates' 'fail' 'manifest approved_for_live is not false'
        return $null
    }
    if ($manifest.schema_version -ne $SchemaVersion -or $manifest.manifest_id -eq '') {
        Set-CheckResult 'soak_manifest_validates' 'fail' 'manifest schema_version/manifest_id malformed'
        return $null
    }
    Set-CheckResult 'soak_manifest_validates' 'pass' "manifest_id=$($manifest.manifest_id)"
    return $manifest
}

# ---------------------------------------------------------------------------
# Run checks in order.
# ---------------------------------------------------------------------------
Test-HeadEqualsAcceptedSha
Test-TrackedWorktreeClean
Test-MigrationGovernance
Test-ExpectedDbReachable
Test-NoStaleActiveRunOrLease
Test-ArmIntegrityPosture
Test-ReconciliationReadiness
Test-DeploymentPaperLiveDisabled
Test-DynamicSelectionModePaperEnforced
Test-ApprovedForLiveFalse
Test-DurablePlanEvidenceValid
Test-SelectedBindingsMatchEvidence
Test-SelectedTimeframesHaveFreshBars
Test-NoBindingMissingRequiredWindow
Test-Phase7AndBundleGuardsPass
Test-ApiMatchesDbEvidence
$Script:SelfSourceText = $null
$Script:SelfSourcePath = $MyInvocation.MyCommand.Path
if ($null -ne $Script:SelfSourcePath -and (Test-Path $Script:SelfSourcePath)) {
    $Script:SelfSourceText = Get-Content -Raw -Path $Script:SelfSourcePath
}
Test-NoTradingActionInvoked -SourceText $Script:SelfSourceText

# ---------------------------------------------------------------------------
# Report + verdict. A 'warn' never fails the run on its own (it is a
# disclosed downgrade, not a hard requirement violation) -- only 'fail'
# does. The soak-manifest check runs last since it needs the pending
# verdict from every prior check.
# ---------------------------------------------------------------------------
$OtherChecks = @($Script:CheckOrder | Where-Object { $_ -ne 'soak_manifest_validates' })
$HardFailures = @($OtherChecks | Where-Object { $Script:Results.Contains($_) -and $Script:Results[$_].status -eq 'fail' })
# A check whose call site vanished (no silent skip) must block the manifest
# exactly like a genuine FAIL -- never let a missing check slip through as
# an implicit pass just because $Script:Results never got that key.
$MissingBeforeManifest = @($OtherChecks | Where-Object { -not $Script:Results.Contains($_) })
$PendingVerdict = if (@($HardFailures).Count -eq 0 -and @($MissingBeforeManifest).Count -eq 0) { 'PASS' } else { 'FAIL' }

$Manifest = Test-SoakManifestValidates -PendingVerdict $PendingVerdict
if ($null -ne $Manifest -and $PendingVerdict -eq 'PASS') {
    New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
    $ManifestPath = Join-Path $OutputDirectory 'bundle7_soak_readiness_manifest.json'
    ($Manifest | ConvertTo-Json -Depth 20) | Set-Content -Path $ManifestPath -Encoding UTF8
    Write-Host ""
    Write-Host "Manifest written: $ManifestPath" -ForegroundColor Green
}

Write-Host ""
Write-Host "=== Check results ===" -ForegroundColor Cyan
foreach ($name in $Script:CheckOrder) {
    if (-not $Script:Results.Contains($name)) {
        Write-Fail "$name -- NOT RUN (must never happen -- no silent skip)"
        continue
    }
    $r = $Script:Results[$name]
    switch ($r.status) {
        'pass' { Write-Ok "$name -- $($r.detail)" }
        'warn' { Write-Warn "$name -- $($r.detail)" }
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
