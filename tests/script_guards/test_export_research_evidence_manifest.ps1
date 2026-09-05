# =============================================================================
# Script guard: test_export_research_evidence_manifest.ps1
# WAVE06-LANE-B-RESEARCH-INFRASTRUCTURE-AND-GOVERNANCE-01 / B2
#
# Proof for scripts\windows\Export-ResearchEvidenceManifest.ps1: real create +
# verify cycles over disposable temp fixtures (never the retained Wave03 or
# DISCOVERY-01 evidence directories), plus the required negative controls:
# one-byte same-length mutation, deleted file, added file (strict refusal),
# renamed file, wrong byte_count, malformed manifest, path-traversal entry,
# and byte-identical re-run over an unchanged fixture.
#
# No daemon, no DB, no live calls, no .env.local, no secrets.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$ScriptUnderTest = Join-Path $RepoRoot 'scripts\windows\Export-ResearchEvidenceManifest.ps1'

$Failures = 0
function Assert-True {
    param([bool]$Condition, [string]$Message)
    if ($Condition) {
        Write-Host "  PASS: $Message" -ForegroundColor Green
    } else {
        Write-Host "  FAIL: $Message" -ForegroundColor Red
        $script:Failures++
    }
}
function Assert-False {
    param([bool]$Condition, [string]$Message)
    Assert-True (-not $Condition) $Message
}

Write-Host ''
Write-Host '--- test_export_research_evidence_manifest.ps1 ---'

if (-not (Test-Path -LiteralPath $ScriptUnderTest)) {
    Write-Host "  FATAL -- script under test not found: $ScriptUnderTest" -ForegroundColor Red
    exit 1
}

# Real child-process invocations so the tool's own `exit 1` calls never tear
# down this test host (same reasoning as Test-MiniQuantDeskRecoveryBackup.ps1's
# Invoke-TestSubprocess) -- re-invoke via this host's own running executable
# rather than a hard-coded legacy PowerShell path.
$HostExe = (Get-Process -Id $PID).Path
function Invoke-ManifestTool {
    param([Parameter(Mandatory = $true)][string[]]$ToolArgs)
    $stdoutFile = [System.IO.Path]::GetTempFileName()
    $stderrFile = [System.IO.Path]::GetTempFileName()
    try {
        $fullArgs = @('-ExecutionPolicy', 'Bypass', '-NonInteractive', '-File', $ScriptUnderTest) + $ToolArgs
        $proc = Start-Process -FilePath $HostExe -NoNewWindow -Wait -PassThru `
            -ArgumentList $fullArgs -RedirectStandardOutput $stdoutFile -RedirectStandardError $stderrFile
        $stdout = Get-Content -LiteralPath $stdoutFile -Raw -ErrorAction SilentlyContinue
        return [pscustomobject]@{ ExitCode = $proc.ExitCode; Stdout = $stdout }
    } finally {
        Remove-Item -Path $stdoutFile, $stderrFile -Force -ErrorAction SilentlyContinue
    }
}

function New-TempDir {
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk-evidence-manifest-test-" + [System.Guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Path $dir -Force | Out-Null
    return $dir
}

$WorkRoots = New-Object 'System.Collections.Generic.List[string]'

try {
    # -------------------------------------------------------------------
    # Section 1: refuse a nonexistent EvidenceRoot (create mode).
    # -------------------------------------------------------------------
    Write-Host ''
    Write-Host '=== Section 1: refuse nonexistent EvidenceRoot ==='
    $missingRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk-evidence-missing-" + [System.Guid]::NewGuid().ToString('N'))
    $missingManifest = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk-evidence-missing-manifest-" + [System.Guid]::NewGuid().ToString('N') + '.json')
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $missingRoot, '-ManifestPath', $missingManifest)
    Assert-True ($r.ExitCode -ne 0) 'create refuses a nonexistent EvidenceRoot'
    Assert-False (Test-Path -LiteralPath $missingManifest) 'no manifest written for a refused EvidenceRoot'

    # -------------------------------------------------------------------
    # Section 2: real create + verify cycle over a disposable fixture.
    # -------------------------------------------------------------------
    Write-Host ''
    Write-Host '=== Section 2: create + verify over a disposable fixture ==='
    $root = New-TempDir
    $WorkRoots.Add($root)
    New-Item -ItemType Directory -Path (Join-Path $root 'sub') -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $root 'a.txt') -Value 'alpha-content' -NoNewline -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $root 'sub\b.txt') -Value 'bravo-content-longer' -NoNewline -Encoding UTF8
    $manifestPath = Join-Path $root 'manifest.json'

    $createResult = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $root, '-ManifestPath', $manifestPath)
    Assert-True ($createResult.ExitCode -eq 0) 'create succeeds over a real disposable fixture'
    Assert-True (Test-Path -LiteralPath $manifestPath) 'manifest file was written'

    $manifestObj = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
    Assert-True ($manifestObj.schema_version -eq 'mqk-research-evidence-manifest-v1') 'manifest carries the expected schema_version'
    Assert-True ($manifestObj.files.Count -eq 2) 'manifest declares exactly the 2 fixture files (manifest itself excluded)'
    $relPaths = @($manifestObj.files | ForEach-Object { $_.relative_path })
    Assert-True (($relPaths -contains 'a.txt') -and ($relPaths -contains 'sub\b.txt')) 'manifest lists both fixture files by relative path'
    $sortedCheck = $relPaths.Clone()
    [Array]::Sort($sortedCheck, [Comparison[object]] { param($x, $y) [string]::CompareOrdinal($x, $y) })
    Assert-True (@(Compare-Object $relPaths $sortedCheck -SyncWindow 0).Count -eq 0) 'files array is in deterministic ordinal sort order'

    $expectedHashA = (Get-FileHash -LiteralPath (Join-Path $root 'a.txt') -Algorithm SHA256).Hash.ToLowerInvariant()
    $entryA = $manifestObj.files | Where-Object { $_.relative_path -eq 'a.txt' } | Select-Object -First 1
    Assert-True ($entryA.sha256 -eq $expectedHashA) 'a.txt sha256 matches an independently computed hash'
    Assert-True ($entryA.byte_count -eq (Get-Item -LiteralPath (Join-Path $root 'a.txt')).Length) 'a.txt byte_count matches actual file size'

    $verifyResult = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $root, '-ManifestPath', $manifestPath, '-Verify')
    Assert-True ($verifyResult.ExitCode -eq 0) 'verify passes against the unmodified fixture it was generated from'

    # -------------------------------------------------------------------
    # Section 3: stable re-run produces byte-identical manifest bytes.
    # -------------------------------------------------------------------
    Write-Host ''
    Write-Host '=== Section 3: deterministic re-run over an unchanged fixture ==='
    # Capture the first manifest's bytes, then remove it before regenerating
    # -- otherwise manifest.json itself (a real file now sitting inside
    # $root) would become an extra undeclared input to the second create,
    # which would be a false-positive determinism failure, not a real one:
    # the fixture (a.txt, sub\b.txt) must stay byte-for-byte unchanged
    # between the two create invocations for this to be a valid proof.
    $manifestBytesFirst = Get-Content -LiteralPath $manifestPath -Raw
    Remove-Item -LiteralPath $manifestPath -Force
    $rerun = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $root, '-ManifestPath', $manifestPath)
    Assert-True ($rerun.ExitCode -eq 0) 'second create over the same unchanged fixture succeeds'
    $manifestBytesSecond = Get-Content -LiteralPath $manifestPath -Raw
    Assert-True ($manifestBytesFirst -ceq $manifestBytesSecond) 'stable re-run over an unchanged fixture produces byte-identical manifest content'

    # -------------------------------------------------------------------
    # Section 4: negative controls (each on its own disposable copy of the
    # fixture so failures don't cascade between assertions).
    # -------------------------------------------------------------------
    Write-Host ''
    Write-Host '=== Section 4: negative controls ==='

    function New-FixtureCopy {
        $newRoot = New-TempDir
        $script:WorkRoots.Add($newRoot)
        Copy-Item -LiteralPath (Join-Path $root 'a.txt') -Destination (Join-Path $newRoot 'a.txt')
        New-Item -ItemType Directory -Path (Join-Path $newRoot 'sub') -Force | Out-Null
        Copy-Item -LiteralPath (Join-Path $root 'sub\b.txt') -Destination (Join-Path $newRoot 'sub\b.txt')
        $newManifest = Join-Path $newRoot 'manifest.json'
        Copy-Item -LiteralPath $manifestPath -Destination $newManifest
        return [pscustomobject]@{ Root = $newRoot; Manifest = $newManifest }
    }

    # 4a. One-byte same-length content mutation must fail (sha256 mismatch,
    #     byte_count alone cannot catch this).
    $fx = New-FixtureCopy
    $bytes = [System.IO.File]::ReadAllBytes((Join-Path $fx.Root 'a.txt'))
    $bytes[0] = $bytes[0] -bxor 0xFF
    [System.IO.File]::WriteAllBytes((Join-Path $fx.Root 'a.txt'), $bytes)
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'one-byte same-length content mutation fails verification'
    Assert-True ($r.Stdout -match 'sha256 mismatch') 'mutation failure is reported as a sha256 mismatch'

    # 4b. Deleted declared file must fail.
    $fx = New-FixtureCopy
    Remove-Item -LiteralPath (Join-Path $fx.Root 'a.txt') -Force
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'a deleted declared file fails verification'

    # 4c. Added undeclared file must fail (strict verification policy).
    $fx = New-FixtureCopy
    Set-Content -LiteralPath (Join-Path $fx.Root 'unexpected.txt') -Value 'surprise' -NoNewline -Encoding UTF8
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'an undeclared extra file fails verification (strict policy)'
    Assert-True ($r.Stdout -match 'Undeclared file') 'added-file failure is reported as an undeclared file'

    # 4d. Renamed file must fail (old path missing; new path undeclared).
    $fx = New-FixtureCopy
    Rename-Item -LiteralPath (Join-Path $fx.Root 'a.txt') -NewName 'a_renamed.txt'
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'a renamed file fails verification'

    # 4e. Wrong byte_count (hash still correct) must fail independently of
    #     the sha256 check.
    $fx = New-FixtureCopy
    $manifestText = Get-Content -LiteralPath $fx.Manifest -Raw
    $manifestJson = $manifestText | ConvertFrom-Json
    foreach ($entry in $manifestJson.files) {
        if ($entry.relative_path -eq 'a.txt') { $entry.byte_count = $entry.byte_count + 1 }
    }
    ($manifestJson | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $fx.Manifest -Encoding UTF8
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'wrong byte_count fails verification even when sha256 matches'
    Assert-True ($r.Stdout -match 'byte_count mismatch') 'wrong byte_count failure is reported as a byte_count mismatch'

    # 4f. Malformed manifest (invalid JSON) must fail.
    $fx = New-FixtureCopy
    Set-Content -LiteralPath $fx.Manifest -Value '{ this is not valid json' -Encoding UTF8
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'a malformed (invalid JSON) manifest fails verification'

    # 4g. Path-traversal entry must be rejected regardless of hash/size.
    $fx = New-FixtureCopy
    $manifestJson2 = (Get-Content -LiteralPath $fx.Manifest -Raw) | ConvertFrom-Json
    $traversalEntry = [pscustomobject]@{ relative_path = '..\outside.txt'; byte_count = 1; sha256 = ('0' * 64) }
    $manifestJson2.files = @($manifestJson2.files) + @($traversalEntry)
    ($manifestJson2 | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $fx.Manifest -Encoding UTF8
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'a path-traversal manifest entry is rejected'
    Assert-True ($r.Stdout -match 'traversal') 'traversal failure is reported explicitly'

} finally {
    foreach ($dir in $WorkRoots) {
        Remove-Item -LiteralPath $dir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

if ($Failures -eq 0) {
    Write-Host '  ALL ASSERTIONS PASSED (test_export_research_evidence_manifest)' -ForegroundColor Green
    exit 0
}

Write-Host "  $Failures ASSERTION(S) FAILED (test_export_research_evidence_manifest)" -ForegroundColor Red
exit 1
