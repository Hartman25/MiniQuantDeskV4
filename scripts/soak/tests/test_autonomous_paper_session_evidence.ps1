# =============================================================================
# test_autonomous_paper_session_evidence.ps1
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-F2-F3-G-FINAL-OPERATIONAL-SAFETY-REPAIR
# -- REPAIR I: executable local-fixture tests for the F3 capture/validator
#    pair.
#
# Scope: exercises capture_autonomous_paper_session_evidence.ps1 and
# validate_autonomous_paper_session_evidence.ps1 entirely against local
# temporary fixture files and -FixturePath. No daemon is started, no real
# network call is made, and Alpaca/Discord are never contacted -- every
# scenario below either short-circuits before any HTTP call (the
# -DaemonBaseUrl rejection cases) or routes through -FixturePath, which
# reads canned local JSON instead of calling a daemon.
#
# What this proves:
#   Positive: a manifest built from a fixture set with deployment_mode
#   "paper", adapter_id "alpaca", operator_supervised true,
#   live_routing_enabled false (observed), and valid current/history counts
#   is accepted by both the capture script (exit 0, manifest written) and
#   the validator (exit 0).
#
#   Negative (validator-level -- capture still succeeds and writes a
#   manifest verbatim from the fixture/switch value; the validator then
#   rejects it):
#     - deployment_mode null / "live"
#     - adapter_id null / not "alpaca"
#     - operator_supervised false
#     - live_routing_enabled absent from every surface / true
#     - current-operation count missing / history-row count missing /
#       non-integer count
#
#   Negative (capture-level -- the capture script itself refuses before any
#   directory is created or file is written; exit nonzero and the manifest
#   path never exists):
#     - -DaemonBaseUrl containing UserInfo / a query string / a fragment
#     - -OperatorNotes containing a secret-shaped pattern
#     - a captured fixture surface containing a secret-shaped pattern
#
# All fixtures and outputs live under a fresh temp directory removed at the
# end of the run. Exit codes: 0 = all proofs held, 1 = at least one did not.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir           = Split-Path -Parent $MyInvocation.MyCommand.Definition
$SoakDir              = (Resolve-Path (Join-Path $ScriptDir '..')).Path.TrimEnd('\')
$CaptureScriptPath   = Join-Path $SoakDir 'capture_autonomous_paper_session_evidence.ps1'
$ValidatorScriptPath = Join-Path $SoakDir 'validate_autonomous_paper_session_evidence.ps1'

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
function Assert-ExitCode {
    param([string]$Label, [int]$Expected, [int]$Actual)
    Assert-True -Label "$Label (expected exit $Expected, got $Actual)" -Condition ($Actual -eq $Expected)
}
function Assert-ExitNonZero {
    param([string]$Label, [int]$Actual)
    Assert-True -Label "$Label (expected nonzero exit, got $Actual)" -Condition ($Actual -ne 0)
}

if (-not (Test-Path $CaptureScriptPath)) {
    Show-Red "FATAL -- capture script not found: $CaptureScriptPath"
    exit 1
}
if (-not (Test-Path $ValidatorScriptPath)) {
    Show-Red "FATAL -- validator script not found: $ValidatorScriptPath"
    exit 1
}

# ---------------------------------------------------------------------------
# Child-process invocation. The scripts under test call `exit` on refusal --
# invoking them via a real child `powershell.exe` process (rather than `&`
# in-process) means that `exit` only terminates the child, and $LASTEXITCODE
# reliably reflects its result, same convention the F1-G guards already use
# to invoke each other.
# ---------------------------------------------------------------------------
function Invoke-ChildScript {
    param([string]$ScriptPath, [string[]]$ScriptArgs)
    $allArgs = @('-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $ScriptPath) + $ScriptArgs
    & powershell @allArgs *> $null
    return $LASTEXITCODE
}

# ---------------------------------------------------------------------------
# -File cannot bind a plain command-line string ("false"/"0") to a
# [switch]/[bool] parameter -- PowerShell's own binder requires an actual
# Boolean or numeric object for those types, not a string, and refuses any
# string with ParameterArgumentTransformationError. To flip
# -OperatorSupervised:$false specifically, build a -Command invocation where
# $false is real PowerShell source (evaluated as the boolean singleton, not
# a stringified argument) rather than a -File command-line token.
# ---------------------------------------------------------------------------
function Invoke-CaptureScriptOperatorUnsupervised {
    param([string]$OutputDirectory, [string]$FixtureDir)
    $commandText = "& '$CaptureScriptPath' -OutputDirectory '$OutputDirectory' -CapturePhase 'pre_session' -DaemonBaseUrl 'http://127.0.0.1:8899' -FixturePath '$FixtureDir' -OperatorSupervised:`$false"
    & powershell -NoProfile -ExecutionPolicy Bypass -Command $commandText *> $null
    return $LASTEXITCODE
}

# ---------------------------------------------------------------------------
# Fixture helpers. Every fixture lives under one temp root, removed on exit.
# ---------------------------------------------------------------------------
function Write-JsonFixture {
    param([string]$Dir, [string]$FileName, $Object)
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    $path = Join-Path $Dir $FileName
    ($Object | ConvertTo-Json -Depth 10) | Set-Content -Path $path -Encoding UTF8
}

function New-BaselineFixtureSet {
    param([string]$Dir)
    New-Item -ItemType Directory -Force -Path $Dir | Out-Null
    Write-JsonFixture $Dir 'api_v1_system_status.json' ([ordered]@{
        daemon_mode          = 'paper'
        adapter_id           = 'alpaca'
        live_routing_enabled = $false
    })
    Write-JsonFixture $Dir 'api_v1_system_preflight.json' ([ordered]@{
        live_routing_enabled = $false
    })
    Write-JsonFixture $Dir 'api_v1_autonomous_readiness.json' ([ordered]@{ truth_state = 'active' })
    Write-JsonFixture $Dir 'api_v1_autonomous_paper-status.json' ([ordered]@{ truth_state = 'active' })
    Write-JsonFixture $Dir 'api_v1_autonomous_daily-operation.json' ([ordered]@{
        truth_state = 'active'
        operation   = [ordered]@{
            operation_id              = 'op-fixture-1'
            strategy_evaluation_count = 3
            order_activity_count      = 2
            fill_count                = 1
        }
    })
    Write-JsonFixture $Dir 'api_v1_autonomous_daily-operations.json' ([ordered]@{
        truth_state = 'active'
        rows        = @(
            [ordered]@{
                operation_id              = 'op-fixture-0'
                strategy_evaluation_count = 5
                order_activity_count      = 1
                fill_count                = 0
            }
        )
    })
    Write-JsonFixture $Dir 'api_v1_execution_orders.json'    ([ordered]@{})
    Write-JsonFixture $Dir 'api_v1_portfolio_fills.json'     ([ordered]@{})
    Write-JsonFixture $Dir 'api_v1_reconcile_status.json'    ([ordered]@{})
    Write-JsonFixture $Dir 'api_v1_risk_summary.json'        ([ordered]@{})
    Write-JsonFixture $Dir 'api_v1_alerts_active.json'       ([ordered]@{})
}

function New-FixtureVariant {
    param([string]$BaselineDir, [string]$VariantDir, [hashtable]$Overrides)
    Copy-Item -Path $BaselineDir -Destination $VariantDir -Recurse -Force
    foreach ($fileName in $Overrides.Keys) {
        Write-JsonFixture $VariantDir $fileName $Overrides[$fileName]
    }
}

# ---------------------------------------------------------------------------
# Scenario runners.
# ---------------------------------------------------------------------------
$script:CaseIndex = 0
function New-CaseOutputDir {
    param([string]$TestRoot, [string]$CaseSlug)
    $script:CaseIndex++
    return (Join-Path $TestRoot ("out_{0:D2}_{1}" -f $script:CaseIndex, $CaseSlug))
}

function Test-ValidatorLevelRejection {
    param(
        [string]$TestRoot,
        [string]$CaseName,
        [string]$CaseSlug,
        [string]$FixtureDir,
        [switch]$OperatorUnsupervised
    )
    Write-Host ""
    Show-Info "--- Negative (validator-level): $CaseName ---"
    $outDir = New-CaseOutputDir -TestRoot $TestRoot -CaseSlug $CaseSlug
    if ($OperatorUnsupervised) {
        $captureExit = Invoke-CaptureScriptOperatorUnsupervised -OutputDirectory $outDir -FixtureDir $FixtureDir
    } else {
        $captureArgs = @(
            '-OutputDirectory', $outDir,
            '-CapturePhase', 'pre_session',
            '-DaemonBaseUrl', 'http://127.0.0.1:8899',
            '-FixturePath', $FixtureDir
        )
        $captureExit = Invoke-ChildScript -ScriptPath $CaptureScriptPath -ScriptArgs $captureArgs
    }
    $manifestPath = Join-Path $outDir 'autonomous_paper_session_manifest.json'
    Assert-ExitCode -Label "$CaseName -- capture succeeds (fixture content is captured verbatim)" -Expected 0 -Actual $captureExit
    Assert-True -Label "$CaseName -- manifest file was written" -Condition (Test-Path $manifestPath)
    if (Test-Path $manifestPath) {
        $validateExit = Invoke-ChildScript -ScriptPath $ValidatorScriptPath -ScriptArgs @('-ManifestPath', $manifestPath)
        Assert-ExitNonZero -Label "$CaseName -- validator rejects the manifest" -Actual $validateExit
    }
}

# ---------------------------------------------------------------------------
# BUNDLE-3-FINAL-GUARD-AND-EVIDENCE-INTEGRITY-REPAIR (REPAIR H): a handcrafted
# baseline manifest for validator-level type/shape rejection proofs the real
# capture script cannot itself produce (it always emits a well-typed Boolean
# for operator_supervised and reads deployment_mode/adapter_id verbatim as
# strings) -- these tests exercise the VALIDATOR's own exact-type/exact-shape
# enforcement directly, writing a manifest file without going through
# capture_autonomous_paper_session_evidence.ps1 at all.
# ---------------------------------------------------------------------------
function New-BaselineManifestForValidatorTests {
    $baseline = [ordered]@{
        schema_version           = 'autonomous-paper-soak-evidence-v1'
        session_evidence_id      = 'pre_session-20260722-000000'
        capture_phase            = 'pre_session'
        captured_at_utc          = '2026-07-22T00:00:00Z'
        market_date              = $null
        repository_commit        = '0123456789abcdef0123456789abcdef01234567'
        deployment_mode          = 'paper'
        adapter_id               = 'alpaca'
        daemon_base_url          = 'http://127.0.0.1:8899'
        operator_supervised      = $true

        system_status             = [ordered]@{ daemon_mode = 'paper'; adapter_id = 'alpaca'; live_routing_enabled = $false }
        system_preflight          = [ordered]@{ live_routing_enabled = $false }
        autonomous_readiness      = [ordered]@{ truth_state = 'active' }
        autonomous_paper_status   = [ordered]@{ truth_state = 'active' }
        current_daily_operation   = [ordered]@{
            truth_state = 'active'
            operation   = [ordered]@{
                operation_id              = 'op-fixture-1'
                strategy_evaluation_count = 3
                order_activity_count      = 2
                fill_count                = 1
            }
        }
        recent_daily_operations   = [ordered]@{
            truth_state = 'active'
            rows        = @(
                [ordered]@{
                    operation_id              = 'op-fixture-0'
                    strategy_evaluation_count = 5
                    order_activity_count      = 1
                    fill_count                = 0
                }
            )
        }
        orders_summary            = [ordered]@{}
        fills_summary             = [ordered]@{}
        reconcile_status          = [ordered]@{}
        risk_posture              = [ordered]@{}
        completed_bar_task_status = [ordered]@{}
        gui_build_version         = '0.0.0'

        capture_errors      = @()
        missing_endpoints   = @()
        operator_notes      = $null
        artifact_hashes     = [ordered]@{}
    }
    # Round-trip through JSON to produce the same PSCustomObject/typed-number
    # shape the real validator reads from a manifest file (a raw hashtable's
    # value types do not always match what ConvertFrom-Json would produce).
    return ($baseline | ConvertTo-Json -Depth 20 | ConvertFrom-Json)
}

function Test-ValidatorRejectsHandcraftedManifest {
    param(
        [string]$TestRoot,
        [string]$CaseName,
        [string]$CaseSlug,
        [scriptblock]$Mutate
    )
    Write-Host ""
    Show-Info "--- Negative (validator-level, handcrafted manifest): $CaseName ---"
    $manifestObj = New-BaselineManifestForValidatorTests
    & $Mutate $manifestObj
    $outDir = New-CaseOutputDir -TestRoot $TestRoot -CaseSlug $CaseSlug
    New-Item -ItemType Directory -Force -Path $outDir | Out-Null
    $manifestPath = Join-Path $outDir 'autonomous_paper_session_manifest.json'
    ($manifestObj | ConvertTo-Json -Depth 20) | Set-Content -Path $manifestPath -Encoding UTF8
    $validateExit = Invoke-ChildScript -ScriptPath $ValidatorScriptPath -ScriptArgs @('-ManifestPath', $manifestPath)
    Assert-ExitNonZero -Label "$CaseName -- validator rejects the handcrafted manifest" -Actual $validateExit
}

function Test-CaptureLevelRejection {
    param(
        [string]$TestRoot,
        [string]$CaseName,
        [string]$CaseSlug,
        [string]$FixtureDir,
        [string]$DaemonBaseUrl = 'http://127.0.0.1:8899',
        [string]$OperatorNotes = ''
    )
    Write-Host ""
    Show-Info "--- Negative (capture-level): $CaseName ---"
    $outDir = New-CaseOutputDir -TestRoot $TestRoot -CaseSlug $CaseSlug
    $captureArgs = @(
        '-OutputDirectory', $outDir,
        '-CapturePhase', 'pre_session',
        '-DaemonBaseUrl', $DaemonBaseUrl,
        '-FixturePath', $FixtureDir
    )
    if ($OperatorNotes -ne '') {
        $captureArgs += @('-OperatorNotes', $OperatorNotes)
    }
    $captureExit = Invoke-ChildScript -ScriptPath $CaptureScriptPath -ScriptArgs $captureArgs
    $manifestPath = Join-Path $outDir 'autonomous_paper_session_manifest.json'
    Assert-ExitNonZero -Label "$CaseName -- capture refuses" -Actual $captureExit
    Assert-True -Label "$CaseName -- manifest file does NOT exist (zero writes on refusal)" -Condition (-not (Test-Path $manifestPath))
}

# =============================================================================
# Run
# =============================================================================
Write-Host "============================================================"
Write-Host " test_autonomous_paper_session_evidence.ps1"
Write-Host "============================================================"

$TestRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_f3_fixture_test_" + [Guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $TestRoot | Out-Null

try {
    $BaselineFixtures = Join-Path $TestRoot 'fixtures_baseline'
    New-BaselineFixtureSet -Dir $BaselineFixtures

    # -------------------------------------------------------------------
    # Positive: valid paper/alpaca/supervised/live-routing-false/valid
    # counts -> capture and validator both pass.
    # -------------------------------------------------------------------
    Write-Host ""
    Show-Info "--- Positive: valid supervised Paper + Alpaca fixture set ---"
    $posOutDir = New-CaseOutputDir -TestRoot $TestRoot -CaseSlug 'positive'
    $posCaptureExit = Invoke-ChildScript -ScriptPath $CaptureScriptPath -ScriptArgs @(
        '-OutputDirectory', $posOutDir,
        '-CapturePhase', 'pre_session',
        '-DaemonBaseUrl', 'http://127.0.0.1:8899',
        '-FixturePath', $BaselineFixtures
    )
    $posManifestPath = Join-Path $posOutDir 'autonomous_paper_session_manifest.json'
    Assert-ExitCode -Label "positive -- capture exits 0" -Expected 0 -Actual $posCaptureExit
    Assert-True -Label "positive -- manifest file was written" -Condition (Test-Path $posManifestPath)
    if (Test-Path $posManifestPath) {
        $posValidateExit = Invoke-ChildScript -ScriptPath $ValidatorScriptPath -ScriptArgs @('-ManifestPath', $posManifestPath)
        Assert-ExitCode -Label "positive -- validator exits 0" -Expected 0 -Actual $posValidateExit
    }

    # -------------------------------------------------------------------
    # Negative, validator-level.
    # -------------------------------------------------------------------
    $depModeNullDir = Join-Path $TestRoot 'fx_deployment_mode_null'
    New-FixtureVariant -BaselineDir $BaselineFixtures -VariantDir $depModeNullDir -Overrides @{
        'api_v1_system_status.json' = [ordered]@{ adapter_id = 'alpaca'; live_routing_enabled = $false }
    }
    Test-ValidatorLevelRejection -TestRoot $TestRoot -CaseName 'deployment_mode null' -CaseSlug 'depmode_null' -FixtureDir $depModeNullDir

    $depModeLiveDir = Join-Path $TestRoot 'fx_deployment_mode_live'
    New-FixtureVariant -BaselineDir $BaselineFixtures -VariantDir $depModeLiveDir -Overrides @{
        'api_v1_system_status.json' = [ordered]@{ daemon_mode = 'live'; adapter_id = 'alpaca'; live_routing_enabled = $false }
    }
    Test-ValidatorLevelRejection -TestRoot $TestRoot -CaseName 'deployment_mode "live"' -CaseSlug 'depmode_live' -FixtureDir $depModeLiveDir

    $adapterNullDir = Join-Path $TestRoot 'fx_adapter_id_null'
    New-FixtureVariant -BaselineDir $BaselineFixtures -VariantDir $adapterNullDir -Overrides @{
        'api_v1_system_status.json' = [ordered]@{ daemon_mode = 'paper'; live_routing_enabled = $false }
    }
    Test-ValidatorLevelRejection -TestRoot $TestRoot -CaseName 'adapter_id null' -CaseSlug 'adapter_null' -FixtureDir $adapterNullDir

    $adapterWrongDir = Join-Path $TestRoot 'fx_adapter_id_wrong'
    New-FixtureVariant -BaselineDir $BaselineFixtures -VariantDir $adapterWrongDir -Overrides @{
        'api_v1_system_status.json' = [ordered]@{ daemon_mode = 'paper'; adapter_id = 'ibkr'; live_routing_enabled = $false }
    }
    Test-ValidatorLevelRejection -TestRoot $TestRoot -CaseName 'adapter_id not alpaca' -CaseSlug 'adapter_wrong' -FixtureDir $adapterWrongDir

    Test-ValidatorLevelRejection -TestRoot $TestRoot -CaseName 'operator_supervised false' -CaseSlug 'opsup_false' -FixtureDir $BaselineFixtures -OperatorUnsupervised

    $liveRoutingAbsentDir = Join-Path $TestRoot 'fx_live_routing_absent'
    New-FixtureVariant -BaselineDir $BaselineFixtures -VariantDir $liveRoutingAbsentDir -Overrides @{
        'api_v1_system_status.json'    = [ordered]@{ daemon_mode = 'paper'; adapter_id = 'alpaca' }
        'api_v1_system_preflight.json' = [ordered]@{}
    }
    Test-ValidatorLevelRejection -TestRoot $TestRoot -CaseName 'live_routing_enabled absent everywhere' -CaseSlug 'liverouting_absent' -FixtureDir $liveRoutingAbsentDir

    $liveRoutingTrueDir = Join-Path $TestRoot 'fx_live_routing_true'
    New-FixtureVariant -BaselineDir $BaselineFixtures -VariantDir $liveRoutingTrueDir -Overrides @{
        'api_v1_system_status.json' = [ordered]@{ daemon_mode = 'paper'; adapter_id = 'alpaca'; live_routing_enabled = $true }
    }
    Test-ValidatorLevelRejection -TestRoot $TestRoot -CaseName 'live_routing_enabled true' -CaseSlug 'liverouting_true' -FixtureDir $liveRoutingTrueDir

    $currentCountMissingDir = Join-Path $TestRoot 'fx_current_count_missing'
    New-FixtureVariant -BaselineDir $BaselineFixtures -VariantDir $currentCountMissingDir -Overrides @{
        'api_v1_autonomous_daily-operation.json' = [ordered]@{
            truth_state = 'active'
            operation   = [ordered]@{
                operation_id              = 'op-fixture-1'
                strategy_evaluation_count = 3
                order_activity_count      = 2
                # fill_count intentionally omitted
            }
        }
    }
    Test-ValidatorLevelRejection -TestRoot $TestRoot -CaseName 'current-operation count missing' -CaseSlug 'current_count_missing' -FixtureDir $currentCountMissingDir

    $historyCountMissingDir = Join-Path $TestRoot 'fx_history_count_missing'
    New-FixtureVariant -BaselineDir $BaselineFixtures -VariantDir $historyCountMissingDir -Overrides @{
        'api_v1_autonomous_daily-operations.json' = [ordered]@{
            truth_state = 'active'
            rows        = @(
                [ordered]@{
                    operation_id              = 'op-fixture-0'
                    strategy_evaluation_count = 5
                    # order_activity_count intentionally omitted
                    fill_count                = 0
                }
            )
        }
    }
    Test-ValidatorLevelRejection -TestRoot $TestRoot -CaseName 'history-row count missing' -CaseSlug 'history_count_missing' -FixtureDir $historyCountMissingDir

    $nonIntegerCountDir = Join-Path $TestRoot 'fx_non_integer_count'
    New-FixtureVariant -BaselineDir $BaselineFixtures -VariantDir $nonIntegerCountDir -Overrides @{
        'api_v1_autonomous_daily-operation.json' = [ordered]@{
            truth_state = 'active'
            operation   = [ordered]@{
                operation_id              = 'op-fixture-1'
                strategy_evaluation_count = 3
                order_activity_count      = 2
                fill_count                = 'zero'
            }
        }
    }
    Test-ValidatorLevelRejection -TestRoot $TestRoot -CaseName 'non-integer count' -CaseSlug 'noninteger_count' -FixtureDir $nonIntegerCountDir

    # -------------------------------------------------------------------
    # Negative, capture-level (zero writes on refusal).
    # -------------------------------------------------------------------
    Test-CaptureLevelRejection -TestRoot $TestRoot -CaseName 'DaemonBaseUrl containing UserInfo' -CaseSlug 'url_userinfo' -FixtureDir $BaselineFixtures -DaemonBaseUrl 'http://user:pass@127.0.0.1:8899'
    Test-CaptureLevelRejection -TestRoot $TestRoot -CaseName 'DaemonBaseUrl containing a query string' -CaseSlug 'url_query' -FixtureDir $BaselineFixtures -DaemonBaseUrl 'http://127.0.0.1:8899/?foo=bar'
    Test-CaptureLevelRejection -TestRoot $TestRoot -CaseName 'DaemonBaseUrl containing a fragment' -CaseSlug 'url_fragment' -FixtureDir $BaselineFixtures -DaemonBaseUrl 'http://127.0.0.1:8899/#frag'
    Test-CaptureLevelRejection -TestRoot $TestRoot -CaseName 'OperatorNotes containing a secret pattern' -CaseSlug 'notes_secret' -FixtureDir $BaselineFixtures -OperatorNotes 'leaked MQK_OPERATOR_TOKEN=abc123'

    $fixtureSecretDir = Join-Path $TestRoot 'fx_captured_secret'
    New-FixtureVariant -BaselineDir $BaselineFixtures -VariantDir $fixtureSecretDir -Overrides @{
        'api_v1_alerts_active.json' = [ordered]@{ detail = 'leaked credential: ALPACA_API_KEY=abcdef123456' }
    }
    Test-CaptureLevelRejection -TestRoot $TestRoot -CaseName 'captured fixture containing a secret pattern' -CaseSlug 'fixture_secret' -FixtureDir $fixtureSecretDir

    # -------------------------------------------------------------------
    # BUNDLE-3-FINAL-GUARD-AND-EVIDENCE-INTEGRITY-REPAIR (REPAIR H):
    # focused rejection proofs for every edge this repair closed. Every
    # case below is a validator-level rejection against a handcrafted
    # manifest, proving the exact-type/exact-shape enforcement directly
    # (Repairs D/F/G), except the final one, which is a capture-level proof
    # for the new strict repository-commit capture (Repair E).
    # -------------------------------------------------------------------
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'operator_supervised = 1 (Int64, not Boolean)' -CaseSlug 'opsup_int1' -Mutate {
        param($m) $m.operator_supervised = 1
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'operator_supervised = "true" (string, not Boolean)' -CaseSlug 'opsup_strtrue' -Mutate {
        param($m) $m.operator_supervised = 'true'
    }

    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'live_routing_enabled = null' -CaseSlug 'liverouting_null' -Mutate {
        param($m) $m.system_status.live_routing_enabled = $null; $m.system_preflight.live_routing_enabled = $null
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'live_routing_enabled = 0 (Int64, not Boolean)' -CaseSlug 'liverouting_int0' -Mutate {
        param($m) $m.system_status.live_routing_enabled = 0
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'live_routing_enabled = "false" (string, not Boolean)' -CaseSlug 'liverouting_strfalse' -Mutate {
        param($m) $m.system_status.live_routing_enabled = 'false'
    }

    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'deployment_mode = "PAPER" (wrong case)' -CaseSlug 'depmode_case' -Mutate {
        param($m) $m.deployment_mode = 'PAPER'
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'deployment_mode = "paper " (trailing whitespace)' -CaseSlug 'depmode_trailingspace' -Mutate {
        param($m) $m.deployment_mode = 'paper '
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'adapter_id = "ALPACA" (wrong case)' -CaseSlug 'adapter_case' -Mutate {
        param($m) $m.adapter_id = 'ALPACA'
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'adapter_id = "alpaca " (trailing whitespace)' -CaseSlug 'adapter_trailingspace' -Mutate {
        param($m) $m.adapter_id = 'alpaca '
    }

    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'repository_commit malformed / non-SHA' -CaseSlug 'commit_malformed' -Mutate {
        param($m) $m.repository_commit = 'not-a-real-sha'
    }

    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'current active-operation count = 1.5 (Double, not integral)' -CaseSlug 'current_active_double' -Mutate {
        param($m) $m.current_daily_operation.operation.fill_count = 1.5
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'current query_failed retained-operation count = 1.5 (Double, not integral)' -CaseSlug 'current_queryfailed_double' -Mutate {
        param($m)
        $m.current_daily_operation.truth_state = 'query_failed'
        $m.current_daily_operation.operation.fill_count = 1.5
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'history active row count = 1.5 (Double, not integral)' -CaseSlug 'history_active_double' -Mutate {
        param($m) $m.recent_daily_operations.rows[0].fill_count = 1.5
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'history query_failed partial row missing a count field' -CaseSlug 'history_queryfailed_missing' -Mutate {
        param($m)
        $m.recent_daily_operations.truth_state = 'query_failed'
        $m.recent_daily_operations.rows[0].PSObject.Properties.Remove('fill_count')
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'history query_failed partial row count = 1.5 (Double, not integral)' -CaseSlug 'history_queryfailed_double' -Mutate {
        param($m)
        $m.recent_daily_operations.truth_state = 'query_failed'
        $m.recent_daily_operations.rows[0].fill_count = 1.5
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'negative count' -CaseSlug 'negative_count' -Mutate {
        param($m) $m.current_daily_operation.operation.fill_count = -1
    }

    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'daemon_base_url with a non-root path' -CaseSlug 'daemonurl_nonrootpath' -Mutate {
        param($m) $m.daemon_base_url = 'http://127.0.0.1:8899/status'
    }
    Test-ValidatorRejectsHandcraftedManifest -TestRoot $TestRoot -CaseName 'capture_errors containing a raw string' -CaseSlug 'captureerrors_rawstring' -Mutate {
        param($m) $m.capture_errors = @('raw exception text leaked: fatal: something failed')
    }

    # ---------------------------------------------------------------
    # Capture-level (real capture script, no daemon/network call): an
    # invalid -RepoRoot (not a Git repository) proves repository_commit
    # capture failure is bounded -- never raw Git stderr -- and that the
    # resulting manifest (repository_commit null) is rejected by the
    # validator.
    # ---------------------------------------------------------------
    Write-Host ""
    Show-Info "--- Negative (capture-level, invalid RepoRoot): repository_commit capture failure is bounded, never raw Git stderr ---"
    $InvalidRepoRoot = Join-Path $TestRoot 'not_a_git_repo'
    New-Item -ItemType Directory -Force -Path $InvalidRepoRoot | Out-Null
    $InvalidRepoOutDir = New-CaseOutputDir -TestRoot $TestRoot -CaseSlug 'invalid_reporoot'
    $InvalidRepoArgs = @(
        '-OutputDirectory', $InvalidRepoOutDir,
        '-CapturePhase', 'pre_session',
        '-DaemonBaseUrl', 'http://127.0.0.1:8899',
        '-FixturePath', $BaselineFixtures,
        '-RepoRoot', $InvalidRepoRoot
    )
    $InvalidRepoExit = Invoke-ChildScript -ScriptPath $CaptureScriptPath -ScriptArgs $InvalidRepoArgs
    $InvalidRepoManifestPath = Join-Path $InvalidRepoOutDir 'autonomous_paper_session_manifest.json'
    Assert-ExitCode -Label "invalid RepoRoot -- capture still exits 0 (a commit-capture failure is bounded, not fatal)" -Expected 0 -Actual $InvalidRepoExit
    Assert-True -Label "invalid RepoRoot -- manifest file was written" -Condition (Test-Path $InvalidRepoManifestPath)
    if (Test-Path $InvalidRepoManifestPath) {
        $InvalidRepoManifestRaw = Get-Content -Raw -Path $InvalidRepoManifestPath
        $InvalidRepoManifest = $InvalidRepoManifestRaw | ConvertFrom-Json
        Assert-True -Label "invalid RepoRoot -- repository_commit is null" -Condition ($null -eq $InvalidRepoManifest.repository_commit)
        $HasGitCaptureError = $false
        foreach ($e in @($InvalidRepoManifest.capture_errors)) {
            if ($e.route -eq 'git_rev_parse_head') { $HasGitCaptureError = $true }
        }
        Assert-True -Label "invalid RepoRoot -- a bounded git_rev_parse_head capture_errors entry exists" -Condition $HasGitCaptureError
        Assert-True -Label "invalid RepoRoot -- manifest text never contains raw Git stderr ('fatal:')" -Condition ($InvalidRepoManifestRaw.IndexOf('fatal:', [System.StringComparison]::OrdinalIgnoreCase) -lt 0)
        Assert-True -Label "invalid RepoRoot -- manifest text never contains raw Git stderr ('not a git repository')" -Condition ($InvalidRepoManifestRaw.IndexOf('not a git repository', [System.StringComparison]::OrdinalIgnoreCase) -lt 0)
        $InvalidRepoValidateExit = Invoke-ChildScript -ScriptPath $ValidatorScriptPath -ScriptArgs @('-ManifestPath', $InvalidRepoManifestPath)
        Assert-ExitNonZero -Label "invalid RepoRoot -- validator rejects the resulting manifest (null repository_commit)" -Actual $InvalidRepoValidateExit
    }
}
finally {
    Remove-Item -Path $TestRoot -Recurse -Force -ErrorAction SilentlyContinue
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"
if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- capture/validator fixture proofs hold ($script:CaseIndex scenarios run)."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
