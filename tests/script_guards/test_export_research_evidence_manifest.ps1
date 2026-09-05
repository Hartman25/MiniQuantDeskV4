# =============================================================================
# Script guard: test_export_research_evidence_manifest.ps1
# WAVE06-LANE-B-RESEARCH-INFRASTRUCTURE-AND-GOVERNANCE-01 / B2
#
# Proof for scripts\windows\Export-ResearchEvidenceManifest.ps1: real create +
# verify cycles over disposable temp fixtures (never the retained Wave03 or
# DISCOVERY-01 evidence directories), plus the required negative controls:
# one-byte same-length mutation, deleted file, added file (strict refusal),
# renamed file, wrong byte_count, malformed manifest, path-traversal entry,
# byte-identical re-run over an unchanged fixture, unsupported/missing
# schema_version, wrong/missing file_count, reparse points (junctions) under
# EvidenceRoot in both create and verify mode, manifest content being
# location-neutral across two different absolute EvidenceRoot paths, and the
# written manifest carrying no UTF-8 BOM.
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
    # Keep file_count internally consistent with the appended entry so this
    # control still exercises the traversal-specific rejection rather than
    # tripping the (separately tested) file_count authority check first.
    $manifestJson2.file_count = @($manifestJson2.files).Count
    ($manifestJson2 | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $fx.Manifest -Encoding UTF8
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'a path-traversal manifest entry is rejected'
    Assert-True ($r.Stdout -match 'traversal') 'traversal failure is reported explicitly'

    # -------------------------------------------------------------------
    # Section 5: manifest schema/file_count authority (Finding 1).
    # -------------------------------------------------------------------
    Write-Host ''
    Write-Host '=== Section 5: manifest schema/file_count authority ==='

    # 5a. Unsupported schema_version must fail.
    $fx = New-FixtureCopy
    $mj = (Get-Content -LiteralPath $fx.Manifest -Raw) | ConvertFrom-Json
    $mj.schema_version = 'mqk-research-evidence-manifest-v99-unknown'
    ($mj | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $fx.Manifest -Encoding UTF8
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'an unsupported schema_version fails verification'
    Assert-True ($r.Stdout -match 'schema_version') 'unsupported schema_version failure names the field'

    # 5b. Missing schema_version must fail.
    $fx = New-FixtureCopy
    $mj = (Get-Content -LiteralPath $fx.Manifest -Raw) | ConvertFrom-Json
    $mj.PSObject.Properties.Remove('schema_version')
    ($mj | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $fx.Manifest -Encoding UTF8
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'a missing schema_version fails verification'
    Assert-True ($r.Stdout -match 'schema_version') 'missing schema_version failure names the field'

    # 5c. Wrong file_count must fail even when every declared file entry is
    #     otherwise perfectly valid.
    $fx = New-FixtureCopy
    $mj = (Get-Content -LiteralPath $fx.Manifest -Raw) | ConvertFrom-Json
    $mj.file_count = $mj.file_count + 1
    ($mj | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $fx.Manifest -Encoding UTF8
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'a wrong file_count fails verification even when all declared files are individually valid'
    Assert-True ($r.Stdout -match 'file_count') 'wrong file_count failure names the field'

    # 5d. Missing file_count must fail.
    $fx = New-FixtureCopy
    $mj = (Get-Content -LiteralPath $fx.Manifest -Raw) | ConvertFrom-Json
    $mj.PSObject.Properties.Remove('file_count')
    ($mj | ConvertTo-Json -Depth 8) | Set-Content -LiteralPath $fx.Manifest -Encoding UTF8
    $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $fx.Root, '-ManifestPath', $fx.Manifest, '-Verify')
    Assert-True ($r.ExitCode -ne 0) 'a missing file_count fails verification'
    Assert-True ($r.Stdout -match 'file_count') 'missing file_count failure names the field'

    # -------------------------------------------------------------------
    # Section 6: reparse points (junctions) fail closed under EvidenceRoot
    # in both create and verify mode (Finding 2). Directory junctions are
    # used because they do not require admin rights/Developer Mode on
    # Windows, unlike symbolic links.
    # -------------------------------------------------------------------
    Write-Host ''
    Write-Host '=== Section 6: reparse points fail closed ==='

    $reparseSupported = $true
    try {
        $probeRoot = New-TempDir
        $WorkRoots.Add($probeRoot)
        $probeTarget = Join-Path $probeRoot 'target'
        New-Item -ItemType Directory -Path $probeTarget -Force | Out-Null
        New-Item -ItemType Junction -Path (Join-Path $probeRoot 'link') -Target $probeTarget -ErrorAction Stop | Out-Null
    } catch {
        $reparseSupported = $false
        Write-Host "  PLATFORM RESTRICTION: cannot create a directory junction on this host: $($_.Exception.Message)" -ForegroundColor Yellow
    }

    if (-not $reparseSupported) {
        Write-Host '  SKIPPING Section 6 -- directory junctions unsupported on this platform (see platform restriction above).' -ForegroundColor Yellow
    } else {
        # 6a. Create must refuse when EvidenceRoot contains a reparse point.
        #     fixture\normal.txt + fixture\link_to_external\ -> external dir.
        $root6 = New-TempDir
        $WorkRoots.Add($root6)
        Set-Content -LiteralPath (Join-Path $root6 'normal.txt') -Value 'normal-content' -NoNewline -Encoding UTF8
        $external6 = New-TempDir
        $WorkRoots.Add($external6)
        Set-Content -LiteralPath (Join-Path $external6 'outside.txt') -Value 'outside-content' -NoNewline -Encoding UTF8
        New-Item -ItemType Junction -Path (Join-Path $root6 'link_to_external') -Target $external6 | Out-Null
        $manifest6 = Join-Path $root6 'manifest.json'
        $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $root6, '-ManifestPath', $manifest6)
        Assert-True ($r.ExitCode -ne 0) 'create refuses an EvidenceRoot containing a reparse point (junction)'
        Assert-True ($r.Stdout -match 'eparse') 'junction refusal is reported as a reparse point'
        Assert-False (Test-Path -LiteralPath $manifest6) 'no manifest written when a reparse point is present under EvidenceRoot'

        # 6b. Verify must refuse when a previously-normal declared directory
        #     has been replaced by a reparse point pointing to byte-identical
        #     content -- proves the check is not fooled by a matching
        #     hash/byte_count alone.
        $root7 = New-TempDir
        $WorkRoots.Add($root7)
        New-Item -ItemType Directory -Path (Join-Path $root7 'sub') -Force | Out-Null
        Set-Content -LiteralPath (Join-Path $root7 'sub\c.txt') -Value 'charlie-content' -NoNewline -Encoding UTF8
        $manifest7 = Join-Path $root7 'manifest.json'
        $create7 = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $root7, '-ManifestPath', $manifest7)
        Assert-True ($create7.ExitCode -eq 0) 'create succeeds over the Section 6b fixture before any reparse point exists'
        $verify7Baseline = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $root7, '-ManifestPath', $manifest7, '-Verify')
        Assert-True ($verify7Baseline.ExitCode -eq 0) 'verify passes against the unmodified Section 6b fixture'

        $externalSub7 = New-TempDir
        $WorkRoots.Add($externalSub7)
        Set-Content -LiteralPath (Join-Path $externalSub7 'c.txt') -Value 'charlie-content' -NoNewline -Encoding UTF8
        Remove-Item -LiteralPath (Join-Path $root7 'sub') -Recurse -Force
        New-Item -ItemType Junction -Path (Join-Path $root7 'sub') -Target $externalSub7 | Out-Null

        $r = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $root7, '-ManifestPath', $manifest7, '-Verify')
        Assert-True ($r.ExitCode -ne 0) 'verify refuses when a declared directory has been replaced by a reparse point to byte-identical content'
        Assert-True ($r.Stdout -match 'eparse') 'directory-to-junction replacement is reported as a reparse point'
    }

    # -------------------------------------------------------------------
    # Section 7: manifest content is location-neutral (Finding 3) -- a
    # stronger proof than the same-directory rerun in Section 3: byte-
    # identical evidence at two different absolute EvidenceRoot paths must
    # produce byte-identical manifest bytes.
    # -------------------------------------------------------------------
    Write-Host ''
    Write-Host '=== Section 7: manifest is byte-identical across different EvidenceRoot locations ==='

    $rootA = New-TempDir
    $WorkRoots.Add($rootA)
    $rootB = New-TempDir
    $WorkRoots.Add($rootB)
    Assert-True ($rootA -ne $rootB) 'the two Section 7 fixtures are at genuinely different absolute paths'
    New-Item -ItemType Directory -Path (Join-Path $rootA 'sub') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $rootB 'sub') -Force | Out-Null
    Set-Content -LiteralPath (Join-Path $rootA 'a.txt') -Value 'alpha-content' -NoNewline -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $rootB 'a.txt') -Value 'alpha-content' -NoNewline -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $rootA 'sub\b.txt') -Value 'bravo-content-longer' -NoNewline -Encoding UTF8
    Set-Content -LiteralPath (Join-Path $rootB 'sub\b.txt') -Value 'bravo-content-longer' -NoNewline -Encoding UTF8

    $manifestA = Join-Path $rootA 'manifest.json'
    $manifestB = Join-Path $rootB 'manifest.json'
    $rA = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $rootA, '-ManifestPath', $manifestA)
    $rB = Invoke-ManifestTool -ToolArgs @('-EvidenceRoot', $rootB, '-ManifestPath', $manifestB)
    Assert-True (($rA.ExitCode -eq 0) -and ($rB.ExitCode -eq 0)) 'both cross-directory Section 7 fixtures create successfully'

    $bytesA = [System.IO.File]::ReadAllBytes($manifestA)
    $bytesB = [System.IO.File]::ReadAllBytes($manifestB)
    $identical = ($bytesA.Length -eq $bytesB.Length)
    if ($identical) {
        for ($i = 0; $i -lt $bytesA.Length; $i++) {
            if ($bytesA[$i] -ne $bytesB[$i]) { $identical = $false; break }
        }
    }
    Assert-True $identical 'byte-identical evidence at two different absolute EvidenceRoot paths produces byte-identical manifest content (no absolute evidence_root leakage)'

    # -------------------------------------------------------------------
    # Section 8: manifest is written as UTF-8 with no BOM (Finding 4),
    # independent of Windows PowerShell vs pwsh host encoding defaults.
    # -------------------------------------------------------------------
    Write-Host ''
    Write-Host '=== Section 8: manifest has no UTF-8 BOM ==='
    $firstBytes = [System.IO.File]::ReadAllBytes($manifestA) | Select-Object -First 3
    $hasBom = ($firstBytes.Count -ge 3) -and ($firstBytes[0] -eq 0xEF) -and ($firstBytes[1] -eq 0xBB) -and ($firstBytes[2] -eq 0xBF)
    Assert-False $hasBom 'manifest file has no UTF-8 BOM'

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
