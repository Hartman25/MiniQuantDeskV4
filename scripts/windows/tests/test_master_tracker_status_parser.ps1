$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$WindowsDir = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$RepoRoot = (Resolve-Path (Join-Path $WindowsDir '..\..')).Path

$Launcher = Join-Path $WindowsDir 'Start-MiniQuantDesk.ps1'
$CanonicalMaster = Join-Path $RepoRoot 'MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md'

if (-not (Test-Path -LiteralPath $Launcher -PathType Leaf)) {
    throw "Launcher not found: $Launcher"
}

if (-not (Test-Path -LiteralPath $CanonicalMaster -PathType Leaf)) {
    throw "Canonical master not found: $CanonicalMaster"
}

# Load the REAL production parser seam. Start-MiniQuantDesk.ps1 explicitly
# suppresses main dispatch when dot-sourced.
. $Launcher

if (-not (Get-Command Get-LedgerPatchStatus -ErrorAction SilentlyContinue)) {
    throw 'Get-LedgerPatchStatus production seam was not loaded.'
}

$script:Passed = 0
$script:Failed = 0
$script:TempFiles = @()

function Assert-Equal {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)]$Expected,
        [Parameter(Mandatory = $true)]$Actual
    )

    if ($Expected -ceq $Actual) {
        Write-Host "PASS: $Name" -ForegroundColor Green
        $script:Passed++
    }
    else {
        Write-Host "FAIL: $Name" -ForegroundColor Red
        Write-Host "  expected: <$Expected>" -ForegroundColor Red
        Write-Host "  actual:   <$Actual>" -ForegroundColor Red
        $script:Failed++
    }
}

function Assert-True {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][bool]$Condition
    )

    if ($Condition) {
        Write-Host "PASS: $Name" -ForegroundColor Green
        $script:Passed++
    }
    else {
        Write-Host "FAIL: $Name" -ForegroundColor Red
        $script:Failed++
    }
}

function New-TempMaster {
    param(
        [Parameter(Mandatory = $true)][string]$Content
    )

    $path = Join-Path `
        ([System.IO.Path]::GetTempPath()) `
        ("mqk_master_status_{0}.md" -f ([guid]::NewGuid().ToString()))

    $utf8 = New-Object System.Text.UTF8Encoding($false)

    [System.IO.File]::WriteAllText(
        $path,
        $Content,
        $utf8
    )

    $script:TempFiles += $path
    return $path
}

$Utf8 = New-Object System.Text.UTF8Encoding($false, $true)
$MasterContent = [System.IO.File]::ReadAllText($CanonicalMaster, $Utf8)

$Begin = '<!-- BEGIN MQK_CURRENT_PATCH_STATUS -->'
$End   = '<!-- END MQK_CURRENT_PATCH_STATUS -->'

try {
    # ============================================================
    # POSITIVE CURRENT-TRUTH CONTROLS
    # ============================================================

    Assert-Equal `
        'LIVE-SECRETS-CONSOLIDATION-01 -> CLOSED' `
        'CLOSED' `
        (Get-LedgerPatchStatus `
            -LedgerPath $CanonicalMaster `
            -PatchId 'LIVE-SECRETS-CONSOLIDATION-01')

    Assert-Equal `
        'LIVE-ACCOUNT-TRUTH-01 -> CLOSED' `
        'CLOSED' `
        (Get-LedgerPatchStatus `
            -LedgerPath $CanonicalMaster `
            -PatchId 'LIVE-ACCOUNT-TRUTH-01')

    Assert-Equal `
        'LIVE-FLATTEN-PROOF-01 -> CLOSED' `
        'CLOSED' `
        (Get-LedgerPatchStatus `
            -LedgerPath $CanonicalMaster `
            -PatchId 'LIVE-FLATTEN-PROOF-01')

    # ============================================================
    # NON-CLOSED CURRENT-TRUTH CONTROLS
    # ============================================================

    Assert-Equal `
        'LIVE-TINY-CAPITAL-SMOKE-01 remains operator-validation deferred' `
        'IMPLEMENTATION_COMPLETE_OPERATOR_VALIDATION_DEFERRED' `
        (Get-LedgerPatchStatus `
            -LedgerPath $CanonicalMaster `
            -PatchId 'LIVE-TINY-CAPITAL-SMOKE-01')

    foreach ($PatchId in @(
        'LIVE-TRUST-CHAIN-SHADOW-CAPTURE-01',
        'LIVE-TRUST-CHAIN-PARITY-SCORER-01',
        'LIVE-TRUST-CHAIN-EVIDENCE-SIGNER-01',
        'LIVE-CAPITAL-EXTERNAL-PROOF-01'
    )) {
        Assert-Equal `
            "$PatchId remains blocked by deferred operator validation" `
            'BLOCKED_BY_DEFERRED_OPERATOR_VALIDATION' `
            (Get-LedgerPatchStatus `
                -LedgerPath $CanonicalMaster `
                -PatchId $PatchId)
    }

    # ============================================================
    # PRODUCTION CALLER CONTRACT
    # ============================================================

    $BrokerTruth = Test-LiveBrokerTruth -LedgerPath $CanonicalMaster

    Assert-Equal `
        'broker truth passes accepted CLOSED prerequisite' `
        'PASS' `
        $BrokerTruth.Status

    Assert-True `
        'broker truth no longer emits stale pre-repair defect text' `
        (-not $BrokerTruth.Detail.Contains('not yet routed'))

    $AccountTruth = Test-LiveAccountTruth -LedgerPath $CanonicalMaster

    Assert-Equal `
        'account truth passes accepted CLOSED prerequisite' `
        'PASS' `
        $AccountTruth.Status

    Assert-True `
        'account truth no longer emits stale pre-repair defect text' `
        (-not $AccountTruth.Detail.Contains('not yet fixed'))

    Assert-Equal `
        'risk truth passes accepted CLOSED prerequisite' `
        'PASS' `
        (Test-LiveRisk -LedgerPath $CanonicalMaster).Status

    $ReconciliationTruth =
        Test-LiveReconciliation -LedgerPath $CanonicalMaster

    Assert-Equal `
        'implemented-but-unvalidated live smoke remains blocking' `
        'BLOCKED_OPERATOR_VALIDATION_DEFERRED' `
        $ReconciliationTruth.Status

    Assert-True `
        'reconciliation reports truthful remaining operator proof' `
        ($ReconciliationTruth.Detail.Contains('real operational LiveShadow validation remains NOT RUN'))

    # ============================================================    # NEGATIVE A — MISSING PATCH ID
    # ============================================================

    $MissingPatchContent =
        $MasterContent.Replace(
            'LIVE-SECRETS-CONSOLIDATION-01 = CLOSED',
            ''
        )

    $MissingPatchFile =
        New-TempMaster -Content $MissingPatchContent

    Assert-Equal `
        'missing PatchId fails closed' `
        'NOT_FOUND_IN_STATUS_INDEX' `
        (Get-LedgerPatchStatus `
            -LedgerPath $MissingPatchFile `
            -PatchId 'LIVE-SECRETS-CONSOLIDATION-01')

    # ============================================================
    # NEGATIVE B — DUPLICATE SAME STATUS
    # ============================================================

    $DuplicateSameContent =
        $MasterContent.Replace(
            $End,
            "LIVE-SECRETS-CONSOLIDATION-01 = CLOSED`n$End"
        )

    $DuplicateSameFile =
        New-TempMaster -Content $DuplicateSameContent

    Assert-Equal `
        'duplicate same PatchId fails closed' `
        'STATUS_INDEX_DUPLICATE_PATCH_ID' `
        (Get-LedgerPatchStatus `
            -LedgerPath $DuplicateSameFile `
            -PatchId 'LIVE-SECRETS-CONSOLIDATION-01')

    # ============================================================
    # NEGATIVE C — CONFLICTING DUPLICATE STATUS
    # ============================================================

    $DuplicateConflictContent =
        $MasterContent.Replace(
            $End,
            "LIVE-SECRETS-CONSOLIDATION-01 = BLOCKED_BY_DEFERRED_OPERATOR_VALIDATION`n$End"
        )

    $DuplicateConflictFile =
        New-TempMaster -Content $DuplicateConflictContent

    Assert-Equal `
        'conflicting duplicate PatchId fails closed' `
        'STATUS_INDEX_DUPLICATE_PATCH_ID' `
        (Get-LedgerPatchStatus `
            -LedgerPath $DuplicateConflictFile `
            -PatchId 'LIVE-SECRETS-CONSOLIDATION-01')

    # ============================================================
    # NEGATIVE D — MALFORMED ROW
    # ============================================================

    $MalformedContent =
        $MasterContent.Replace(
            $End,
            "THIS IS NOT A VALID STATUS ROW`n$End"
        )

    $MalformedFile =
        New-TempMaster -Content $MalformedContent

    Assert-Equal `
        'malformed row fails closed' `
        'STATUS_INDEX_MALFORMED_ROW' `
        (Get-LedgerPatchStatus `
            -LedgerPath $MalformedFile `
            -PatchId 'LIVE-SECRETS-CONSOLIDATION-01')

    # ============================================================
    # NEGATIVE E — MISSING BEGIN SENTINEL
    # ============================================================

    $NoBeginContent =
        $MasterContent.Replace(
            $Begin,
            '<!-- REMOVED BEGIN SENTINEL -->'
        )

    $NoBeginFile =
        New-TempMaster -Content $NoBeginContent

    Assert-Equal `
        'missing begin sentinel fails closed' `
        'STATUS_INDEX_INVALID_SENTINELS' `
        (Get-LedgerPatchStatus `
            -LedgerPath $NoBeginFile `
            -PatchId 'LIVE-SECRETS-CONSOLIDATION-01')

    # ============================================================
    # NEGATIVE F — MISSING END SENTINEL
    # ============================================================

    $NoEndContent =
        $MasterContent.Replace(
            $End,
            '<!-- REMOVED END SENTINEL -->'
        )

    $NoEndFile =
        New-TempMaster -Content $NoEndContent

    Assert-Equal `
        'missing end sentinel fails closed' `
        'STATUS_INDEX_INVALID_SENTINELS' `
        (Get-LedgerPatchStatus `
            -LedgerPath $NoEndFile `
            -PatchId 'LIVE-SECRETS-CONSOLIDATION-01')

    # ============================================================
    # NEGATIVE G — MISLEADING HISTORICAL OCCURRENCE BEFORE INDEX
    # ============================================================

    $BeforeHistoryContent =
        $MasterContent.Replace(
            $Begin,
            "LIVE-TINY-CAPITAL-SMOKE-01 = CLOSED`n$Begin"
        )

    $BeforeHistoryFile =
        New-TempMaster -Content $BeforeHistoryContent

    Assert-Equal `
        'historical-like occurrence before index cannot spoof current truth' `
        'IMPLEMENTATION_COMPLETE_OPERATOR_VALIDATION_DEFERRED' `
        (Get-LedgerPatchStatus `
            -LedgerPath $BeforeHistoryFile `
            -PatchId 'LIVE-TINY-CAPITAL-SMOKE-01')

    # ============================================================
    # NEGATIVE H — MISLEADING HISTORICAL OCCURRENCE AFTER INDEX
    # ============================================================

    $AfterHistoryContent =
        $MasterContent.Replace(
            $End,
            "$End`nLIVE-TINY-CAPITAL-SMOKE-01 = CLOSED"
        )

    $AfterHistoryFile =
        New-TempMaster -Content $AfterHistoryContent

    Assert-Equal `
        'historical-like occurrence after index cannot spoof current truth' `
        'IMPLEMENTATION_COMPLETE_OPERATOR_VALIDATION_DEFERRED' `
        (Get-LedgerPatchStatus `
            -LedgerPath $AfterHistoryFile `
            -PatchId 'LIVE-TINY-CAPITAL-SMOKE-01')

    # ============================================================
    # NEGATIVE I — SUBSTRING / COLLISION MUST NOT MATCH
    # ============================================================

    $CollisionContent =
        $MasterContent.Replace(
            'LIVE-SECRETS-CONSOLIDATION-01 = CLOSED',
            'LIVE-SECRETS-CONSOLIDATION-010 = CLOSED'
        )

    $CollisionFile =
        New-TempMaster -Content $CollisionContent

    Assert-Equal `
        'substring/collision PatchId does not match exact requested ID' `
        'NOT_FOUND_IN_STATUS_INDEX' `
        (Get-LedgerPatchStatus `
            -LedgerPath $CollisionFile `
            -PatchId 'LIVE-SECRETS-CONSOLIDATION-01')

    # ============================================================
    # NEGATIVE J — DUPLICATE SENTINEL
    # ============================================================

    $DuplicateBeginContent =
        $MasterContent.Replace(
            $Begin,
            "$Begin`n$Begin"
        )

    $DuplicateBeginFile =
        New-TempMaster -Content $DuplicateBeginContent

    Assert-Equal `
        'duplicate begin sentinel fails closed' `
        'STATUS_INDEX_INVALID_SENTINELS' `
        (Get-LedgerPatchStatus `
            -LedgerPath $DuplicateBeginFile `
            -PatchId 'LIVE-SECRETS-CONSOLIDATION-01')

    # ============================================================
    # PRODUCTION AUTHORITY FILENAME
    # ============================================================

    $LauncherText =
        [System.IO.File]::ReadAllText($Launcher, $Utf8)

    Assert-True `
        'launcher points to canonical master filename' `
        ($LauncherText.Contains(
            'MiniQuantDeskV4_Master_Program_Plan_and_Ledger.md'
        ))

    Assert-True `
        'launcher no longer points to superseded updated-ledger filename' `
        (-not $LauncherText.Contains(
            'MiniQuantDesk_Master_Patch_Ledger_v2_updated.md'
        ))
}
finally {
    foreach ($Path in $script:TempFiles) {
        if (Test-Path -LiteralPath $Path) {
            Remove-Item -LiteralPath $Path -Force
        }
    }
}

Write-Host ""
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "MT-01A FOCUSED TEST RESULT" -ForegroundColor Cyan
Write-Host "============================================================" -ForegroundColor Cyan
Write-Host "Passed: $script:Passed"
Write-Host "Failed: $script:Failed"

if ($script:Failed -ne 0) {
    throw "MT-01A focused parser tests failed: $script:Failed"
}

Write-Host "ALL MT-01A FOCUSED TESTS PASSED." -ForegroundColor Green