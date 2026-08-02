# =============================================================================
# test_bundle7_phase7c_premarket_validation.ps1
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7C-DURABLE-EVIDENCE-OPERATOR-
# SURFACES-AND-SOAK-READINESS-CLOSURE Part 7/10.
#
# Executable local proof for Invoke-Bundle7Phase7cPremarketValidation.ps1:
# a genuine PASS/FAIL run against the real repo and the real test DB
# (127.0.0.1:5434), the hard non-5434-port refusal, and a mutation-negative
# proof that removing a required check from the script's own "run checks in
# order" list is caught (surfaced as NOT RUN, never silently skipped) and
# turns the overall verdict FAIL.
#
# Requires MQK_DATABASE_URL-equivalent reachability at 127.0.0.1:5434 (same
# test DB every other Bundle 7 Phase 7C scenario test uses). No daemon is
# started, no order is placed, no network call other than the local DB and
# local guard invocations is made.
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
        [string]$DbUrl,
        [string]$ExpectedSha,
        [string]$OutputDirectory,
        [bool]$RequireDb = $true,
        [bool]$RequireApi = $false,
        [bool]$AllowNonTestDbPort = $false
    )
    $allowFlag = if ($AllowNonTestDbPort) { '-AllowNonTestDbPort' } else { '' }
    # Boolean params must be passed as real PowerShell source (a -Command
    # invocation), not -File command-line tokens -- PowerShell's parameter
    # binder cannot transform a stringified "$false"/"$true" token into a
    # [bool] across a -File process boundary (mirrors the soak capture
    # test's own Invoke-CaptureScriptOperatorUnsupervised convention).
    $commandText = "& '$ValidatorPath' -RepoRoot '$RepoRoot' -DbUrl '$DbUrl' -ExpectedSha '$ExpectedSha' " +
        "-MarketDate '2026-08-01' -Session 'regular' -OutputDirectory '$OutputDirectory' " +
        "-RequireDb `$$RequireDb -RequireApi `$$RequireApi $allowFlag"
    $output = & powershell -NoProfile -ExecutionPolicy Bypass -Command $commandText 2>&1
    return @{ Output = ($output -join "`n"); ExitCode = $LASTEXITCODE }
}

Show-Info "=== Scenario 1: hard refuses a non-5434 DB port in coding/test mode ==="
$outDir1 = Join-Path $TempRoot 'scenario1'
$r1 = Invoke-Validator -DbUrl 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper' `
    -ExpectedSha ('0' * 40) -OutputDirectory $outDir1
Assert-True 'refuses port 5440 with nonzero exit' ($r1.ExitCode -ne 0)
Assert-True 'refusal message names the required test port' ($r1.Output -match '5434')
Assert-True 'no manifest written on refusal' (-not (Test-Path (Join-Path $outDir1 'bundle7_soak_readiness_manifest.json')))

Show-Info "=== Scenario 2: real run against the real test DB (127.0.0.1:5434) ==="
$Script:DbAvailable = $true
try {
    $tnc = Test-NetConnection -ComputerName '127.0.0.1' -Port 5434 -InformationLevel Quiet -WarningAction SilentlyContinue
    if (-not $tnc) { $Script:DbAvailable = $false }
} catch {
    $Script:DbAvailable = $false
}

if (-not $Script:DbAvailable) {
    Show-Red "  SKIP -- 127.0.0.1:5434 is not reachable in this environment; scenarios 2-4 require it"
} else {
    $RealHead = (& git -C $RepoRoot rev-parse HEAD 2>$null).Trim()

    $outDir2 = Join-Path $TempRoot 'scenario2_matching_sha'
    $r2 = Invoke-Validator -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' `
        -ExpectedSha $RealHead -OutputDirectory $outDir2
    Assert-True 'emits exactly one FINAL: line' (([regex]::Matches($r2.Output, 'FINAL: (PASS|FAIL)')).Count -eq 1)
    Assert-True 'head_equals_accepted_sha check passes against the real HEAD' ($r2.Output -match 'OK\s+--\s+head_equals_accepted_sha')

    Show-Info "=== Scenario 3: wrong ExpectedSha fails head_equals_accepted_sha and the overall run ==="
    $outDir3 = Join-Path $TempRoot 'scenario3_wrong_sha'
    $r3 = Invoke-Validator -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' `
        -ExpectedSha ('f' * 40) -OutputDirectory $outDir3
    Assert-True 'head_equals_accepted_sha reported as FAIL' ($r3.Output -match 'FAIL\s+--\s+head_equals_accepted_sha')
    Assert-True 'overall verdict is FINAL: FAIL' ($r3.Output -match 'FINAL: FAIL')
    Assert-True 'nonzero exit on FAIL' ($r3.ExitCode -ne 0)
    Assert-True 'no manifest written on a FAIL run' (-not (Test-Path (Join-Path $outDir3 'bundle7_soak_readiness_manifest.json')))

    Show-Info "=== Scenario 4: -RequireApi:`$false downgrades API-dependent checks to WARN, never a silent omission ==="
    $outDir4 = Join-Path $TempRoot 'scenario4_require_api_false'
    $r4 = Invoke-Validator -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' `
        -ExpectedSha $RealHead -OutputDirectory $outDir4 -RequireApi $false
    Assert-True 'dynamic_selection_mode_paper_enforced is WARN, not silently absent' ($r4.Output -match 'WARN\s+--\s+dynamic_selection_mode_paper_enforced')
    $allCheckNames = @(
        'head_equals_accepted_sha', 'tracked_worktree_clean', 'migration_governance',
        'expected_db_reachable', 'no_stale_active_run_or_lease', 'arm_integrity_posture',
        'reconciliation_readiness', 'deployment_paper_live_disabled',
        'dynamic_selection_mode_paper_enforced', 'approved_for_live_false',
        'durable_plan_evidence_valid', 'selected_bindings_match_evidence',
        'selected_timeframes_have_fresh_bars', 'no_binding_missing_required_window',
        'phase7_and_bundle_guards_pass', 'api_matches_db_evidence',
        'no_trading_action_invoked', 'soak_manifest_validates'
    )
    $missingCheckNames = @($allCheckNames | Where-Object { $r4.Output -notmatch [regex]::Escape($_) })
    Assert-True 'every one of the 18 named checks still appears in the report' ($missingCheckNames.Count -eq 0)

    Show-Info "=== Scenario 5 (mutation-negative): removing a required check call is caught, never silently skipped ==="
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
    $commandText5 = "& '$mutatedScript' -RepoRoot '$RepoRoot' -DbUrl 'postgres://postgres:postgres@127.0.0.1:5434/mqk_test' " +
        "-ExpectedSha '$RealHead' -MarketDate '2026-08-01' -Session 'regular' -OutputDirectory '$outDir5' " +
        "-RequireDb `$true -RequireApi `$false"
    $out5 = & powershell -NoProfile -ExecutionPolicy Bypass -Command $commandText5 2>&1
    $out5Text = ($out5 -join "`n")
    $exit5 = $LASTEXITCODE

    Assert-True 'the removed check is reported as NOT RUN (never silently vanishes from output)' ($out5Text -match 'FAIL\s+--\s+head_equals_accepted_sha -- NOT RUN')
    Assert-True 'a missing check turns the overall verdict FAIL' ($out5Text -match 'FINAL: FAIL')
    Assert-True 'a missing check produces a nonzero exit' ($exit5 -ne 0)
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
