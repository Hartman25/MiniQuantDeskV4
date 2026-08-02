# =============================================================================
# test_bundle7_phase7c_premarket_validation.ps1
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C-FORMAL-SOAK-GATE-TRUTH-
# REPAIR-01.
#
# Executable local proof for the two-stage
# Invoke-Bundle7Phase7cPremarketValidation.ps1: genuine PreStart/ActiveCommit
# runs against the real repo and the real test DB (127.0.0.1:5434), the hard
# non-5434-port refusal, the hard RequireApi/RequireDb precondition (no WARN
# downgrade in either stage), and a mutation-negative proof that removing a
# required check from the script's own "run checks in order" section is
# caught (surfaced as NOT RUN, never silently skipped) and turns the overall
# verdict FAIL.
#
# Requires MQK_DATABASE_URL-equivalent reachability at 127.0.0.1:5434 (same
# test DB every other Bundle 7 Phase 7C scenario test uses). No daemon is
# started, no order is placed, no network call other than the local DB and
# local guard invocations is made. Because no daemon is running,
# API-dependent checks are expected to legitimately FAIL in every scenario
# here -- this file proves the validator's mechanics and hard/fail policy,
# never a live end-to-end PASS (that requires a real daemon, out of scope
# for this local proof).
#
# Exit codes: 0 = all proofs held, 1 = at least one did not.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir      = Split-Path -Parent $MyInvocation.MyCommand.Definition
$WindowsDir     = (Resolve-Path (Join-Path $ScriptDir '..')).Path.TrimEnd('\')
$RepoRoot       = (Resolve-Path (Join-Path $WindowsDir '..\..')).Path.TrimEnd('\')
$ValidatorPath  = Join-Path $WindowsDir 'Invoke-Bundle7Phase7cPremarketValidation.ps1'

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

if (-not (Test-Path $ValidatorPath)) {
    Show-Red "FATAL -- validator script not found: $ValidatorPath"
    exit 1
}

$TempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("bundle7_premarket_test_" + [System.Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $TempRoot | Out-Null

function Invoke-Validator {
    param(
        [string]$Stage,
        [string]$DbUrl,
        [string]$ExpectedSha,
        [string]$OutputDirectory,
        [bool]$RequireDb = $true,
        [bool]$RequireApi = $true,
        [string]$DaemonBaseUrl = '',
        [bool]$AllowNonTestDbPort = $false
    )
    $allowFlag = if ($AllowNonTestDbPort) { "-AllowNonTestDbPort -Environment 'Paper'" } else { '' }
    # Boolean params must be passed as real PowerShell source (a -Command
    # invocation), not -File command-line tokens -- PowerShell's parameter
    # binder cannot transform a stringified "$false"/"$true" token into a
    # [bool] across a -File process boundary (mirrors the soak capture
    # test's own Invoke-CaptureScriptOperatorUnsupervised convention).
    $commandText = "& '$ValidatorPath' -Stage '$Stage' -RepoRoot '$RepoRoot' -DbUrl '$DbUrl' -ExpectedSha '$ExpectedSha' " +
        "-MarketDate '2026-08-01' -Session 'regular' -OutputDirectory '$OutputDirectory' " +
        "-DaemonBaseUrl '$DaemonBaseUrl' -RequireDb `$$RequireDb -RequireApi `$$RequireApi $allowFlag"
    $output = & powershell -NoProfile -ExecutionPolicy Bypass -Command $commandText 2>&1
    return @{ Output = ($output -join "`n"); ExitCode = $LASTEXITCODE }
}

# ---------------------------------------------------------------------------
# Hermetic local HTTP fixture (Bundle 7 Phase 7C Active-Commit repair,
# Defect 1) -- serves the exact response shapes of the 5 GET routes the
# ActiveCommit stage reads, driven entirely from an in-memory facts
# hashtable this test file controls. Never a real daemon process, never a
# real broker/provider network call, never real credentials. Runs in a
# background runspace so the synchronous child-process validator run (see
# Invoke-Validator above) can reach it over a real loopback TCP socket.
# ---------------------------------------------------------------------------
$Script:Bundle7FixtureListenerScript = {
    param($FixtureSync, $Prefix)
    $listener = New-Object System.Net.HttpListener
    $listener.Prefixes.Add($Prefix)
    $listener.Start()
    try {
        while ($listener.IsListening) {
            try {
                $context = $listener.GetContext()
            } catch {
                break
            }
            $path = $context.Request.Url.AbsolutePath
            if ($path -eq '/__shutdown__') {
                $bytes = [System.Text.Encoding]::UTF8.GetBytes('bye')
                $context.Response.StatusCode = 200
                $context.Response.ContentLength64 = $bytes.Length
                $context.Response.OutputStream.Write($bytes, 0, $bytes.Length)
                $context.Response.OutputStream.Close()
                $listener.Stop()
                break
            }
            $body = if ($FixtureSync.ContainsKey($path)) { $FixtureSync[$path] } else { $null }
            if ($null -eq $body) {
                $context.Response.StatusCode = 404
                $bytes = [System.Text.Encoding]::UTF8.GetBytes('not found')
            } else {
                $context.Response.StatusCode = 200
                $context.Response.ContentType = 'application/json'
                $bytes = [System.Text.Encoding]::UTF8.GetBytes($body)
            }
            $context.Response.ContentLength64 = $bytes.Length
            $context.Response.OutputStream.Write($bytes, 0, $bytes.Length)
            $context.Response.OutputStream.Close()
        }
    } finally {
        if ($listener.IsListening) { $listener.Stop() }
        $listener.Close()
    }
}

function Start-Bundle7HermeticFixture {
    param([int]$Port)
    $sync = [hashtable]::Synchronized(@{})
    $prefix = "http://127.0.0.1:$Port/"
    $rs = [runspacefactory]::CreateRunspace()
    $rs.Open()
    $ps = [powershell]::Create()
    $ps.Runspace = $rs
    [void]$ps.AddScript($Script:Bundle7FixtureListenerScript).AddArgument($sync).AddArgument($prefix)
    $handle = $ps.BeginInvoke()
    Start-Sleep -Milliseconds 300
    return [ordered]@{ PowerShell = $ps; Runspace = $rs; Handle = $handle; Sync = $sync; Prefix = $prefix }
}

function Stop-Bundle7HermeticFixture {
    param($Fixture)
    try {
        Invoke-WebRequest -Uri "$($Fixture.Prefix)__shutdown__" -Method Get -TimeoutSec 3 -ErrorAction SilentlyContinue | Out-Null
    } catch {}
    Start-Sleep -Milliseconds 200
    try { $Fixture.PowerShell.EndInvoke($Fixture.Handle) | Out-Null } catch {}
    try { $Fixture.PowerShell.Dispose() } catch {}
    try { $Fixture.Runspace.Close() } catch {}
    try { $Fixture.Runspace.Dispose() } catch {}
}

# Builds the exact response shapes (field names verified against
# core-rs/crates/mqk-daemon/src/api_types.rs and routes/*.rs) from a flat
# facts hashtable, so a mutation test only ever changes one named fact.
function Get-Bundle7BaselineFacts {
    param([string]$RunId, [string]$PlanId, [string]$HolderId)
    return [ordered]@{
        RunId = $RunId; PlanId = $PlanId; HolderId = $HolderId; Epoch = 1
        DsActiveRunId = $RunId
        Symbol = 'AAPL'; StrategyId = 'swing_momentum'; TimeframeSecs = 300
        DaemonMode = 'paper'; AdapterId = 'alpaca'; DeploymentStartAllowed = $true; DeploymentBlocker = $null
        DbStatus = 'ok'; LiveRoutingEnabled = $false; KillSwitchActive = $false; RiskHaltActive = $false; IntegrityHaltActive = $false
        DesiredArmed = $true; IntegrityState = 'ARMED'; DeadmanArmedState = 'ARMED'; RiskBlocked = $false; RiskReason = $null
        LeaseExpired = $false; RunOwnedLocally = $true
        Lifecycle = 'running'
        CommittedConfiguredMode = 'paper_enforced'; CommittedEffectiveMode = 'paper_enforced'; CommittedLiveLockApplied = $false
        ApprovedForLive = $false; Disposition = 'paper_enforced_allowed'
        EvidencePersisted = $true; EvidenceValidationState = 'valid'; ValidationBlockers = @()
        SourceKind = 'watchlist_v2'; SourceIdentity = 'watchlist'; CommittedPlanTruthState = 'computed'
        ReconcileStatus = 'ok'; ReconcileTruthState = 'active'
        StartAllowed = $true; TopLevelBlocker = $null
        ReadinessState = 'ready'; ContinuityState = 'ok'; ProvenanceState = 'ok'
        LoadedCompletedBars = 500; ExpectedLatestBarTs = 1754000000; ActualLatestBarTs = 1754000000
        EmptyAssignments = $false
    }
}

function Get-Bundle7SystemStatusJson {
    param($F)
    $obj = [ordered]@{
        environment = 'test'
        daemon_mode = $F.DaemonMode
        adapter_id = $F.AdapterId
        deployment_start_allowed = $F.DeploymentStartAllowed
        deployment_blocker = $F.DeploymentBlocker
        runtime_status = 'running'
        broker_status = 'ok'
        broker_snapshot_source = 'synthetic'
        alpaca_ws_continuity = 'not_applicable'
        db_status = $F.DbStatus
        market_data_health = 'ok'
        reconcile_status = $F.ReconcileStatus
        integrity_status = 'ok'
        audit_writer_status = 'ok'
        last_heartbeat = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        deadman_status = 'ok'
        loop_latency_ms = 50
        active_account_id = $null
        config_profile = $null
        has_warning = $false
        has_critical = $false
        strategy_armed = $true
        execution_armed = $true
        live_routing_enabled = $F.LiveRoutingEnabled
        kill_switch_active = $F.KillSwitchActive
        risk_halt_active = $F.RiskHaltActive
        integrity_halt_active = $F.IntegrityHaltActive
    }
    return ($obj | ConvertTo-Json -Depth 10 -Compress)
}

function Get-Bundle7ControlStatusJson {
    param($F)
    $obj = [ordered]@{
        daemon_mode = $F.DaemonMode
        adapter_id = $F.AdapterId
        deployment_start_allowed = $F.DeploymentStartAllowed
        deployment_blocker = $F.DeploymentBlocker
        desired_armed = $F.DesiredArmed
        leader_holder_id = $F.HolderId
        leader_epoch = $F.Epoch
        lease_expires_at_utc = (Get-Date).ToUniversalTime().AddHours(1).ToString('yyyy-MM-ddTHH:mm:ssZ')
        lease_expired = $F.LeaseExpired
        active_run_id = $F.RunId
        run_state = 'running'
        run_owned_locally = $F.RunOwnedLocally
        run_notes = $null
        reconcile_status = $F.ReconcileStatus
        reconcile_notes = $null
        integrity_state = $F.IntegrityState
        integrity_reason = $null
        risk_blocked = $F.RiskBlocked
        risk_reason = $F.RiskReason
        deadman_armed_state = $F.DeadmanArmedState
        deadman_reason = $null
    }
    return ($obj | ConvertTo-Json -Depth 10 -Compress)
}

function Get-Bundle7ReconcileStatusJson {
    param($F)
    $obj = [ordered]@{
        truth_state = $F.ReconcileTruthState
        status = $F.ReconcileStatus
        last_run_at = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
        snapshot_watermark_ms = 0
        mismatched_positions = 0
        mismatched_orders = 0
        mismatched_fills = 0
        unmatched_broker_events = 0
    }
    return ($obj | ConvertTo-Json -Depth 10 -Compress)
}

function Get-Bundle7DynamicSelectionStatusJson {
    param($F)
    $selected = @(, @{
        symbol               = $F.Symbol
        selected_strategy_id = $F.StrategyId
        timeframe_secs       = $F.TimeframeSecs
        reason_code          = 'selected_highest_score'
    })
    $obj = [ordered]@{
        lifecycle = $F.Lifecycle
        active_run_id = $F.DsActiveRunId
        committed_configured_mode = $F.CommittedConfiguredMode
        committed_effective_mode = $F.CommittedEffectiveMode
        committed_live_lock_applied = $F.CommittedLiveLockApplied
        approved_for_live = $F.ApprovedForLive
        disposition = $F.Disposition
        committed_plan_id = $F.PlanId
        committed_source_kind = $F.SourceKind
        committed_source_identity = $F.SourceIdentity
        committed_plan_truth_state = $F.CommittedPlanTruthState
        evidence_persisted = $F.EvidencePersisted
        evidence_validation_state = $F.EvidenceValidationState
        evidence_validation_detail = $null
        selected = $selected
        refused = @()
        host_pool_count = 1
        validation_blockers = @($F.ValidationBlockers)
        preview_configured_mode = $F.CommittedConfiguredMode
        preview_effective_mode = $F.CommittedEffectiveMode
    }
    return ($obj | ConvertTo-Json -Depth 10 -Compress)
}

function Get-Bundle7MarketDataReadinessJson {
    param($F)
    $assignments = @()
    if ($F.EmptyAssignments -ne $true) {
        $assignments = @(, @{
            assignment_symbol                = $F.Symbol
            assignment_timeframe             = '5m'
            configured_strategy_id           = $F.StrategyId
            effective_runtime_strategy_id    = $F.StrategyId
            effective_runtime_target_symbol  = $F.Symbol
            effective_runtime_timeframe_secs = $F.TimeframeSecs
            readiness_state                  = $F.ReadinessState
            continuity_state                 = $F.ContinuityState
            provenance_state                 = $F.ProvenanceState
            loaded_completed_bars            = $F.LoadedCompletedBars
            expected_latest_bar_ts           = $F.ExpectedLatestBarTs
            actual_latest_bar_ts             = $F.ActualLatestBarTs
            blockers                         = @()
        })
    }
    $obj = [ordered]@{
        canonical_route   = '/api/v1/market-data/readiness'
        schema_version    = 'daily-data-readiness-v1'
        applicability     = 'active'
        start_allowed     = $F.StartAllowed
        top_level_blocker = $F.TopLevelBlocker
        assignments       = $assignments
    }
    return ($obj | ConvertTo-Json -Depth 10 -Compress)
}

function Sync-Bundle7Fixture {
    param($Fixture, $F)
    $Fixture.Sync['/api/v1/system/status'] = Get-Bundle7SystemStatusJson -F $F
    $Fixture.Sync['/api/v1/control/status'] = Get-Bundle7ControlStatusJson -F $F
    $Fixture.Sync['/api/v1/reconcile/status'] = Get-Bundle7ReconcileStatusJson -F $F
    $Fixture.Sync['/api/v1/dynamic-selection/status'] = Get-Bundle7DynamicSelectionStatusJson -F $F
    $Fixture.Sync['/api/v1/market-data/readiness'] = Get-Bundle7MarketDataReadinessJson -F $F
}

# ---------------------------------------------------------------------------
# Real port-5434 test DB seeding for the hermetic ActiveCommit proof --
# psql INSERT/DELETE (test-fixture setup only; the operational validator
# script itself remains read-only). Never touches DB 5440.
# ---------------------------------------------------------------------------
function Invoke-Bundle7TestPsqlExec {
    param([string]$Sql)
    $env:PGPASSWORD = 'postgres'
    & psql -h '127.0.0.1' -p 5434 -U 'postgres' -d 'mqk_test' -v ON_ERROR_STOP=1 -q -c $Sql 2>&1 | Out-Null
    return $LASTEXITCODE
}

function Remove-Bundle7ActiveCommitDbFixture {
    param([string]$RunId, [string]$PlanId)
    Invoke-Bundle7TestPsqlExec "delete from sys_dynamic_selection_plan_candidates where plan_id = '$PlanId';" | Out-Null
    Invoke-Bundle7TestPsqlExec "delete from sys_dynamic_selection_plan_symbols where plan_id = '$PlanId';" | Out-Null
    Invoke-Bundle7TestPsqlExec "delete from sys_dynamic_selection_plans where plan_id = '$PlanId';" | Out-Null
    Invoke-Bundle7TestPsqlExec "delete from runs where run_id = '$RunId';" | Out-Null
    Invoke-Bundle7TestPsqlExec "delete from runtime_leader_lease where id = 1;" | Out-Null
    Invoke-Bundle7TestPsqlExec "delete from runtime_control_state where id = 1;" | Out-Null
    Invoke-Bundle7TestPsqlExec "delete from sys_arm_state where sentinel_id = 1;" | Out-Null
}

function New-Bundle7ActiveCommitDbFixture {
    param(
        [string]$RunId, [string]$PlanId, [string]$HolderId, [long]$Epoch,
        [string]$Symbol, [string]$StrategyId, [long]$TimeframeSecs, [string]$Sha
    )
    Remove-Bundle7ActiveCommitDbFixture -RunId $RunId -PlanId $PlanId

    $exitCodes = @()
    $exitCodes += Invoke-Bundle7TestPsqlExec @"
insert into runs (run_id, engine_id, mode, started_at_utc, git_hash, config_hash, config_json, host_fingerprint, status, armed_at_utc, running_at_utc)
values ('$RunId','mqk-daemon','PAPER', now(), '$Sha', 'bundle7-hermetic-fixture', '{}'::jsonb, 'bundle7-hermetic-fixture-host', 'RUNNING', now(), now());
"@
    $exitCodes += Invoke-Bundle7TestPsqlExec "insert into runtime_control_state (id, desired_armed, updated_at) values (1, true, now());"
    $exitCodes += Invoke-Bundle7TestPsqlExec "insert into runtime_leader_lease (id, holder_id, epoch, lease_expires_at, updated_at) values (1, '$HolderId', $Epoch, now() + interval '1 hour', now());"
    $exitCodes += Invoke-Bundle7TestPsqlExec "insert into sys_arm_state (sentinel_id, state, reason, updated_at_utc) values (1, 'ARMED', null, now());"

    $exitCodes += Invoke-Bundle7TestPsqlExec @"
insert into sys_dynamic_selection_plans (
    plan_id, run_id, library_schema_version, context_schema_version,
    configured_mode, effective_mode, live_lock_applied, approved_for_live,
    source_kind, source_identity, market_date, disposition, truth_state,
    blockers, symbol_count, candidate_count, selected_count, refused_count,
    writer_version, created_at_utc
) values (
    '$PlanId', '$RunId', 'dynamic-strategy-symbol-selection-v1', 'dynamic-strategy-symbol-selection-v1',
    'paper_enforced', 'paper_enforced', false, false,
    'watchlist_v2', 'watchlist', '2026-08-01', 'paper_enforced_allowed', 'computed',
    '{}', 1, 1, 1, 0,
    'bundle7-hermetic-fixture-test', now()
);
"@
    $exitCodes += Invoke-Bundle7TestPsqlExec @"
insert into sys_dynamic_selection_plan_symbols (plan_id, symbol, selected_strategy_id, disposition, reason_code, exact_reason_code)
values ('$PlanId', '$Symbol', '$StrategyId', 'selected', 'selected_highest_score', 'selected_highest_score');
"@
    $exitCodes += Invoke-Bundle7TestPsqlExec @"
insert into sys_dynamic_selection_plan_candidates (
    plan_id, ordinal, symbol, strategy_id, timeframe_secs,
    watchlist_assigned, promotion_query_ok, promotion_effective, promotion_expired,
    evidence_resolved, review_state_is_paper_candidate, legacy_fingerprint_matches,
    exact_fingerprint_v2_matches, registry_enabled, plugin_instantiable, timeframe_matches,
    data_ready, exact_reason_code, selected, disposition, reason_code
) values (
    '$PlanId', 0, '$Symbol', '$StrategyId', $TimeframeSecs,
    true, true, true, false,
    true, true, true,
    true, true, true, true,
    true, 'selected_highest_score', true, 'selected', 'selected_highest_score'
);
"@
    return @($exitCodes | Where-Object { $_ -ne 0 })
}

function Invoke-Bundle7ActiveCommitMutationScenario {
    param(
        [string]$Label,
        [string]$Sha,
        [string]$RunId,
        [string]$PlanId,
        [string]$HolderId,
        [int]$Port,
        [scriptblock]$Mutate,
        [string]$TempRoot
    )
    Show-Info "=== Mutation: $Label ==="
    $facts = Get-Bundle7BaselineFacts -RunId $RunId -PlanId $PlanId -HolderId $HolderId
    & $Mutate $facts
    $fixture = Start-Bundle7HermeticFixture -Port $Port
    try {
        Sync-Bundle7Fixture -Fixture $fixture -F $facts
        $outDir = Join-Path $TempRoot ("mutation_" + ($Label -replace '[^a-zA-Z0-9]', '_'))
        $r = Invoke-Validator -Stage 'ActiveCommit' -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' `
            -ExpectedSha $Sha -OutputDirectory $outDir -DaemonBaseUrl "http://127.0.0.1:$Port"
        Assert-True "$Label -- FINAL: FAIL" ($r.Output -match 'FINAL: FAIL')
        Assert-True "$Label -- nonzero exit" ($r.ExitCode -ne 0)
        $manifestPath = Join-Path $outDir 'bundle7_soak_session_manifest.json'
        Assert-True "$Label -- no formal manifest written" (-not (Test-Path $manifestPath))
    } finally {
        Stop-Bundle7HermeticFixture -Fixture $fixture
    }
}

Show-Info "=== Scenario 1: hard refuses a non-5434 DB port in coding/test mode (PreStart) ==="
$outDir1 = Join-Path $TempRoot 'scenario1'
$r1 = Invoke-Validator -Stage 'PreStart' -DbUrl 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper' `
    -ExpectedSha ('0' * 40) -OutputDirectory $outDir1
Assert-True 'refuses port 5440 with nonzero exit' ($r1.ExitCode -ne 0)
Assert-True 'refusal message names the required test port' ($r1.Output -match '5434')
Assert-True 'no artifact written on refusal' (-not (Test-Path (Join-Path $outDir1 'bundle7_prestart_readiness_manifest.json')))

Show-Info "=== Scenario 2: hard refuses a non-5434 DB port in coding/test mode (ActiveCommit) ==="
$outDir1b = Join-Path $TempRoot 'scenario1b'
$r1b = Invoke-Validator -Stage 'ActiveCommit' -DbUrl 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper' `
    -ExpectedSha ('0' * 40) -OutputDirectory $outDir1b
Assert-True 'refuses port 5440 with nonzero exit (ActiveCommit)' ($r1b.ExitCode -ne 0)
Assert-True 'no manifest written on refusal (ActiveCommit)' (-not (Test-Path (Join-Path $outDir1b 'bundle7_soak_session_manifest.json')))

$Script:DbAvailable = $true
try {
    $tnc = Test-NetConnection -ComputerName '127.0.0.1' -Port 5434 -InformationLevel Quiet -WarningAction SilentlyContinue
    if (-not $tnc) { $Script:DbAvailable = $false }
} catch {
    $Script:DbAvailable = $false
}

if (-not $Script:DbAvailable) {
    Show-Red "  SKIP -- 127.0.0.1:5434 is not reachable in this environment; scenarios 3-8 require it"
} else {
    $RealHead = (& git -C $RepoRoot rev-parse HEAD 2>$null).Trim()

    Show-Info "=== Scenario 3: PreStart real run against the real test DB (127.0.0.1:5434), no live daemon ==="
    $outDir2 = Join-Path $TempRoot 'scenario2_matching_sha'
    $r2 = Invoke-Validator -Stage 'PreStart' -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' `
        -ExpectedSha $RealHead -OutputDirectory $outDir2
    Assert-True 'emits exactly one FINAL: line' (([regex]::Matches($r2.Output, 'FINAL: (PASS|FAIL)')).Count -eq 1)
    Assert-True 'head_equals_accepted_sha check passes against the real HEAD' ($r2.Output -match 'OK\s+--\s+head_equals_accepted_sha')
    Assert-True 'no daemon means API-dependent checks legitimately FAIL, never a silent pass' ($r2.Output -match 'FAIL\s+--\s+deployment_paper_broker_config')
    Assert-True 'no WARN status appears anywhere -- the WARN escape hatch is fully removed' ($r2.Output -notmatch '\bWARN\b')

    Show-Info "=== Scenario 4: wrong ExpectedSha fails head_equals_accepted_sha and the overall run (PreStart) ==="
    $outDir3 = Join-Path $TempRoot 'scenario3_wrong_sha'
    $r3 = Invoke-Validator -Stage 'PreStart' -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' `
        -ExpectedSha ('f' * 40) -OutputDirectory $outDir3
    Assert-True 'head_equals_accepted_sha reported as FAIL' ($r3.Output -match 'FAIL\s+--\s+head_equals_accepted_sha')
    Assert-True 'overall verdict is FINAL: FAIL' ($r3.Output -match 'FINAL: FAIL')
    Assert-True 'nonzero exit on FAIL' ($r3.ExitCode -ne 0)
    Assert-True 'no artifact written on a FAIL run' (-not (Test-Path (Join-Path $outDir3 'bundle7_prestart_readiness_manifest.json')))

    Show-Info "=== Scenario 5: -RequireApi `$false is a hard FAIL precondition, never a WARN downgrade (PreStart) ==="
    $outDir4 = Join-Path $TempRoot 'scenario4_require_api_false'
    $r4 = Invoke-Validator -Stage 'PreStart' -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' `
        -ExpectedSha $RealHead -OutputDirectory $outDir4 -RequireApi $false
    Assert-True 'require_api_and_db_true is reported FAIL, not WARN' ($r4.Output -match 'FAIL\s+--\s+require_api_and_db_true')
    Assert-True 'no WARN status appears anywhere in the report' ($r4.Output -notmatch '\bWARN\b')
    Assert-True 'overall verdict is FINAL: FAIL when RequireApi is false' ($r4.Output -match 'FINAL: FAIL')

    Show-Info "=== Scenario 6: -RequireApi `$false is a hard FAIL precondition, never a WARN downgrade (ActiveCommit) ==="
    $outDir4b = Join-Path $TempRoot 'scenario4b_require_api_false_activecommit'
    $r4b = Invoke-Validator -Stage 'ActiveCommit' -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' `
        -ExpectedSha $RealHead -OutputDirectory $outDir4b -RequireApi $false -DaemonBaseUrl 'http://127.0.0.1:8899'
    Assert-True 'require_api_and_db_true is reported FAIL for ActiveCommit too' ($r4b.Output -match 'FAIL\s+--\s+require_api_and_db_true')
    Assert-True 'no manifest written when RequireApi is false' (-not (Test-Path (Join-Path $outDir4b 'bundle7_soak_session_manifest.json')))
    Assert-True 'active_commit_manifest_validates refuses a null run_id/plan_id' ($r4b.Output -match "FAIL\s+--\s+active_commit_manifest_validates -- manifest run_id/plan_id is null")

    $allPreStartCheckNames = @(
        'head_equals_accepted_sha', 'tracked_worktree_clean', 'migration_governance',
        'require_api_and_db_true', 'expected_db_reachable',
        'no_conflicting_active_or_starting_run', 'no_unexpired_leader_lease',
        'deployment_paper_broker_config', 'live_routing_capital_disabled',
        'dynamic_selection_mode_preview_paper_enforced',
        'arm_integrity_posture_suitable_for_start', 'reconciliation_truth_acceptable',
        'required_market_data_pairs_complete_and_fresh', 'phase7_and_bundle_guards_pass',
        'no_trading_action_invoked', 'prestart_artifact_validates'
    )
    Show-Info "=== Scenario 7: every one of the 16 PreStart checks appears in the report ==="
    $missingCheckNames = @($allPreStartCheckNames | Where-Object { $r4.Output -notmatch [regex]::Escape($_) })
    Assert-True 'every PreStart check name appears in the report' ($missingCheckNames.Count -eq 0)

    Show-Info "=== Scenario 8 (mutation-negative): removing a required check call is caught, never silently skipped ==="
    $mutatedScript = Join-Path $TempRoot 'Invoke-Bundle7Phase7cPremarketValidation.Mutated.ps1'
    $originalSource = Get-Content -Raw -Path $ValidatorPath
    # Targets only the bare call-site line in the "Run checks in order"
    # section (a line consisting of exactly the function name) -- never the
    # `function Test-HeadEqualsAcceptedSha {` definition itself, which would
    # otherwise corrupt the function name and produce a parse error instead
    # of the clean "check silently vanished" condition this test proves.
    $mutatedSource = $originalSource -replace '(?m)^Test-HeadEqualsAcceptedSha$', '# REMOVED (call site only)'
    Assert-True 'mutation actually removed the call site (sanity check on the mutation itself)' ($mutatedSource -ne $originalSource)
    Set-Content -Path $mutatedScript -Value $mutatedSource -Encoding UTF8

    $outDir5 = Join-Path $TempRoot 'scenario5_mutated'
    $commandText5 = "& '$mutatedScript' -Stage 'PreStart' -RepoRoot '$RepoRoot' -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' " +
        "-ExpectedSha '$RealHead' -MarketDate '2026-08-01' -Session 'regular' -OutputDirectory '$outDir5' " +
        "-RequireDb `$true -RequireApi `$true"
    $out5 = & powershell -NoProfile -ExecutionPolicy Bypass -Command $commandText5 2>&1
    $out5Text = ($out5 -join "`n")
    $exit5 = $LASTEXITCODE

    Assert-True 'the removed check is reported as NOT RUN (never silently vanishes from output)' ($out5Text -match 'FAIL\s+--\s+head_equals_accepted_sha -- NOT RUN')
    Assert-True 'a missing check turns the overall verdict FAIL' ($out5Text -match 'FINAL: FAIL')
    Assert-True 'a missing check produces a nonzero exit' ($exit5 -ne 0)

    # -----------------------------------------------------------------
    # Scenarios 9+ (Bundle 7 Phase 7C Active-Commit repair, Defect 1):
    # hermetic positive ActiveCommit proof, plus the required negative
    # fixture mutations for Defects 3 and 4. All require both the real
    # port-5434 test DB and a usable psql client (write access, for
    # fixture seed/cleanup only -- the validator script itself never
    # writes).
    # -----------------------------------------------------------------
    $Script:PsqlAvailableForFixture = $null -ne (Get-Command psql -ErrorAction SilentlyContinue)
    if (-not $Script:PsqlAvailableForFixture) {
        Show-Red "  SKIP -- psql not available in this environment; Scenarios 9+ (hermetic ActiveCommit fixture) require it"
    } else {
        Show-Info "=== Scenario 9: hermetic ActiveCommit fixture -- real positive PASS proof ==="
        $RunId9 = [guid]::NewGuid().ToString()
        $PlanId9 = [guid]::NewGuid().ToString()
        $HolderId9 = "bundle7-fixture-holder-$([guid]::NewGuid().ToString('N').Substring(0,8))"
        $seedErrors9 = New-Bundle7ActiveCommitDbFixture -RunId $RunId9 -PlanId $PlanId9 -HolderId $HolderId9 -Epoch 1 `
            -Symbol 'AAPL' -StrategyId 'swing_momentum' -TimeframeSecs 300 -Sha $RealHead
        Assert-True 'DB fixture seed succeeded (every insert exits 0)' (@($seedErrors9).Count -eq 0)

        $port9 = 18971
        $fixture9 = Start-Bundle7HermeticFixture -Port $port9
        try {
            $facts9 = Get-Bundle7BaselineFacts -RunId $RunId9 -PlanId $PlanId9 -HolderId $HolderId9
            Sync-Bundle7Fixture -Fixture $fixture9 -F $facts9

            $outDir9 = Join-Path $TempRoot 'scenario9_positive_active_commit'
            $r9 = Invoke-Validator -Stage 'ActiveCommit' -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' `
                -ExpectedSha $RealHead -OutputDirectory $outDir9 -DaemonBaseUrl "http://127.0.0.1:$port9"

            Assert-True 'hermetic ActiveCommit run reports FINAL: PASS' ($r9.Output -match 'FINAL: PASS')
            Assert-True 'hermetic ActiveCommit run exits 0' ($r9.ExitCode -eq 0)
            $manifestPath9 = Join-Path $outDir9 'bundle7_soak_session_manifest.json'
            $manifestCount9 = @(Get-ChildItem -Path $outDir9 -Filter 'bundle7_soak_session_manifest.json' -ErrorAction SilentlyContinue).Count
            Assert-True 'exactly one countable session manifest is written' ($manifestCount9 -eq 1)
            if (Test-Path $manifestPath9) {
                $manifest9 = Get-Content -Raw -Path $manifestPath9 | ConvertFrom-Json
                Assert-True 'manifest run_id matches the seeded run' ([string]$manifest9.run_id -eq $RunId9)
                Assert-True 'manifest plan_id matches the seeded plan' ([string]$manifest9.plan_id -eq $PlanId9)
                Assert-True 'manifest selected_bindings match the seeded binding exactly' (
                    (@($manifest9.selected_bindings).Count -eq 1) -and
                    ($manifest9.selected_bindings[0].symbol -eq 'AAPL') -and
                    ($manifest9.selected_bindings[0].selected_strategy_id -eq 'swing_momentum') -and
                    ($manifest9.selected_bindings[0].timeframe_secs -eq 300)
                )
                Assert-True 'manifest approved_for_live is false' ($manifest9.approved_for_live -eq $false)
                Assert-True 'no real daemon/broker/provider/order action statement is present' ($null -ne $manifest9.no_live_capital_statement)
            }
        } finally {
            Stop-Bundle7HermeticFixture -Fixture $fixture9
            Remove-Bundle7ActiveCommitDbFixture -RunId $RunId9 -PlanId $PlanId9
        }

        Show-Info "=== Scenarios 10+ (mutation-negative): Defect 3 strict live-value checks ==="
        $RunId10 = [guid]::NewGuid().ToString()
        $PlanId10 = [guid]::NewGuid().ToString()
        $HolderId10 = "bundle7-fixture-holder-$([guid]::NewGuid().ToString('N').Substring(0,8))"
        $seedErrors10 = New-Bundle7ActiveCommitDbFixture -RunId $RunId10 -PlanId $PlanId10 -HolderId $HolderId10 -Epoch 1 `
            -Symbol 'AAPL' -StrategyId 'swing_momentum' -TimeframeSecs 300 -Sha $RealHead
        Assert-True 'DB fixture seed for mutation scenarios succeeded' (@($seedErrors10).Count -eq 0)
        try {
            $mutationPort = 18980
            $defect3Mutations = @(
                @{ Label = 'wrong_adapter_id';               Mutate = { param($f) $f.AdapterId = 'bogus_adapter' } }
                @{ Label = 'deployment_start_allowed_false';  Mutate = { param($f) $f.DeploymentStartAllowed = $false } }
                @{ Label = 'deployment_blocker_present';      Mutate = { param($f) $f.DeploymentBlocker = 'some_deployment_blocker' } }
                @{ Label = 'live_routing_enabled_true';       Mutate = { param($f) $f.LiveRoutingEnabled = $true } }
                @{ Label = 'live_routing_enabled_null';       Mutate = { param($f) $f.LiveRoutingEnabled = $null } }
                @{ Label = 'desired_armed_false';             Mutate = { param($f) $f.DesiredArmed = $false } }
                @{ Label = 'integrity_state_disarmed';        Mutate = { param($f) $f.IntegrityState = 'DISARMED' } }
                @{ Label = 'integrity_state_halted';          Mutate = { param($f) $f.IntegrityState = 'HALTED' } }
                @{ Label = 'integrity_state_unknown';         Mutate = { param($f) $f.IntegrityState = 'UNKNOWN' } }
                @{ Label = 'deadman_armed_state_disarmed';    Mutate = { param($f) $f.DeadmanArmedState = 'DISARMED' } }
                @{ Label = 'deadman_armed_state_halted';      Mutate = { param($f) $f.DeadmanArmedState = 'HALTED' } }
                @{ Label = 'deadman_armed_state_unknown';     Mutate = { param($f) $f.DeadmanArmedState = 'UNKNOWN' } }
                @{ Label = 'risk_blocked_true';                Mutate = { param($f) $f.RiskBlocked = $true } }
                @{ Label = 'db_status_unavailable';            Mutate = { param($f) $f.DbStatus = 'unavailable' } }
                @{ Label = 'readiness_assignments_empty';      Mutate = { param($f) $f.EmptyAssignments = $true } }
                @{ Label = 'readiness_row_not_ready';          Mutate = { param($f) $f.ReadinessState = 'blocked' } }
            )
            foreach ($m in $defect3Mutations) {
                $mutationPort++
                Invoke-Bundle7ActiveCommitMutationScenario -Label $m.Label -Sha $RealHead -RunId $RunId10 -PlanId $PlanId10 `
                    -HolderId $HolderId10 -Port $mutationPort -Mutate $m.Mutate -TempRoot $TempRoot
            }

            Show-Info "=== Scenarios (mutation-negative): Defect 4 exact run/lease coherence ==="
            $defect4Mutations = @(
                @{ Label = 'lease_holder_mismatch'; Mutate = { param($f) $f.HolderId = 'a-different-holder-not-in-db' } }
                @{ Label = 'lease_epoch_mismatch';  Mutate = { param($f) $f.Epoch = 999 } }
                @{ Label = 'lease_expired_true';    Mutate = { param($f) $f.LeaseExpired = $true } }
                @{ Label = 'active_run_mismatch_vs_db'; Mutate = { param($f) $newRunId = [guid]::NewGuid().ToString(); $f.RunId = $newRunId; $f.DsActiveRunId = $newRunId } }
                @{ Label = 'dynamic_selection_and_control_active_run_disagree'; Mutate = { param($f) $f.DsActiveRunId = [guid]::NewGuid().ToString() } }
            )
            foreach ($m in $defect4Mutations) {
                $mutationPort++
                Invoke-Bundle7ActiveCommitMutationScenario -Label $m.Label -Sha $RealHead -RunId $RunId10 -PlanId $PlanId10 `
                    -HolderId $HolderId10 -Port $mutationPort -Mutate $m.Mutate -TempRoot $TempRoot
            }

            Show-Info "=== Scenario (mutation-negative): a second ARMED/RUNNING PAPER run is rejected ==="
            $SecondRunId = [guid]::NewGuid().ToString()
            $secondRunExit = Invoke-Bundle7TestPsqlExec @"
insert into runs (run_id, engine_id, mode, started_at_utc, git_hash, config_hash, config_json, host_fingerprint, status, armed_at_utc, running_at_utc)
values ('$SecondRunId','mqk-daemon','PAPER', now(), '$RealHead', 'bundle7-hermetic-fixture-second', '{}'::jsonb, 'bundle7-hermetic-fixture-host-2', 'RUNNING', now(), now());
"@
            Assert-True 'second concurrent PAPER run row inserted for the mutation proof' ($secondRunExit -eq 0)
            try {
                $mutationPort++
                Invoke-Bundle7ActiveCommitMutationScenario -Label 'second_active_paper_run_rejected' -Sha $RealHead -RunId $RunId10 `
                    -PlanId $PlanId10 -HolderId $HolderId10 -Port $mutationPort -Mutate { param($f) } -TempRoot $TempRoot
            } finally {
                Invoke-Bundle7TestPsqlExec "delete from runs where run_id = '$SecondRunId';" | Out-Null
            }
        } finally {
            Remove-Bundle7ActiveCommitDbFixture -RunId $RunId10 -PlanId $PlanId10
        }
    }
}

Remove-Item -Recurse -Force -Path $TempRoot -ErrorAction SilentlyContinue

Write-Host ""
if ($Violations -eq 0) {
    Show-Green "ALL PROOFS HELD (0 violations)"
    exit 0
} else {
    Show-Red "$Violations VIOLATION(S) FOUND"
    exit 1
}
