# =============================================================================
# WAVE06-LANE-B-RESEARCH-INFRASTRUCTURE-AND-GOVERNANCE-01 / B2
# Export-ResearchEvidenceManifest.ps1
#
# Deterministic content-addressed manifest tool for a retained research-run
# evidence directory (e.g. a research-py/experiments/*/runs/<run_id> tree).
# This is NOT a new artifact framework and it does NOT upload anything -- it
# only hashes what is already on disk under a caller-supplied EvidenceRoot so
# that later restoration (from local disk, an archive, or an offsite B2/restic
# copy -- see Backup-MiniQuantDeskRecovery.ps1 / Restore-MiniQuantDeskRecovery.ps1
# for the whole-repo recovery lane) can be verified byte-for-byte without
# trusting a filename or a directory listing.
#
# Two modes, selected by -Verify:
#   Create (default): recursively hash every regular file under EvidenceRoot
#                      (excluding the manifest file itself) and write a
#                      deterministic manifest to ManifestPath.
#   Verify (-Verify):  read an existing manifest and confirm, for every
#                       declared file, that it exists under EvidenceRoot with
#                       the exact declared byte_count and sha256. Verification
#                       is STRICT: a file present under EvidenceRoot but not
#                       declared in the manifest also fails the run, so a
#                       silently added or renamed file cannot pass as "no
#                       change" (there is no established repo convention for
#                       a looser "additions allowed" mode, so this tool fails
#                       closed rather than inventing one).
#
# Never records: file contents, environment variables, credentials, or any
# inferred economic/statistical meaning. A hash proves integrity, not
# authority -- this tool makes no claim about data provenance or correctness.
#
# Reparse points (symlinks/junctions) beneath EvidenceRoot are unsupported by
# this tool and cause a fail-closed refusal in BOTH create and verify mode --
# they are never silently skipped and never followed, so a reparse point can
# neither escape the manifest's exact-evidence-set contract nor be used to
# smuggle content into or out of EvidenceRoot undetected.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Export-ResearchEvidenceManifest.ps1 `
#       -EvidenceRoot <dir> -ManifestPath <file>
#
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Export-ResearchEvidenceManifest.ps1 `
#       -EvidenceRoot <dir> -ManifestPath <file> -Verify
#
# Exit codes: 0 = manifest written / verification passed, 1 = refused or
# verification failed.
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $EvidenceRoot,

    [Parameter(Mandatory = $true)]
    [string] $ManifestPath,

    [switch] $Verify
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Ok   { param([string]$M) Write-Host "[EVIDENCE-MANIFEST] OK: $M"   -ForegroundColor Green }
function Write-Fail { param([string]$M) Write-Host "[EVIDENCE-MANIFEST] FAIL: $M" -ForegroundColor Red }
function Write-Sect { param([string]$M) Write-Host ''; Write-Host "=== $M ===" -ForegroundColor Magenta }

$ManifestSchemaVersion = 'mqk-research-evidence-manifest-v1'

# -----------------------------------------------------------------------------
# Enumerate regular files under $RootFull. Any reparse point (symlink/
# junction) encountered anywhere beneath the root -- as a directory or as a
# file -- is a fail-closed refusal, not a silent skip: this tool's exact-
# evidence-set contract cannot be honored for content that may not be a plain
# on-disk file. Recursion is done manually (rather than Get-ChildItem
# -Recurse) so each entry's reparse-point attribute can be checked before it
# is ever descended into.
# -----------------------------------------------------------------------------
function Get-EvidenceFiles {
    param([Parameter(Mandatory = $true)][string]$RootFull)

    $result = New-Object 'System.Collections.Generic.List[System.IO.FileInfo]'
    $stack = New-Object 'System.Collections.Generic.Stack[string]'
    $stack.Push($RootFull)

    while ($stack.Count -gt 0) {
        $dir = $stack.Pop()
        $entries = Get-ChildItem -LiteralPath $dir -Force -ErrorAction Stop
        foreach ($entry in $entries) {
            $isReparse = (([int]$entry.Attributes) -band ([int][System.IO.FileAttributes]::ReparsePoint)) -ne 0
            if ($isReparse) {
                throw "Reparse point (symlink/junction) is unsupported beneath EvidenceRoot: $($entry.FullName)"
            }
            if ($entry.PSIsContainer) {
                $stack.Push($entry.FullName)
            } else {
                $result.Add($entry)
            }
        }
    }
    return $result
}

function Get-RelativePath {
    param([Parameter(Mandatory = $true)][string]$RootFull, [Parameter(Mandatory = $true)][string]$FullPath)
    return $FullPath.Substring($RootFull.Length).TrimStart('\')
}

# -----------------------------------------------------------------------------
# Walk every path component of a manifest-declared relative path, from
# EvidenceRoot down to the leaf, and refuse if any intermediate component is
# itself a reparse point. This is checked independently of Get-EvidenceFiles
# because the per-declared-entry verification path resolves and hashes a
# candidate file directly -- it must not be possible for a declared path to
# traverse a reparse-point component (e.g. a directory replaced by a
# junction) to reach content outside, or elsewhere on, disk.
# -----------------------------------------------------------------------------
function Assert-NoReparseInPath {
    param(
        [Parameter(Mandatory = $true)][string]$RootFull,
        [Parameter(Mandatory = $true)][string]$RelativePath
    )
    $current = $RootFull
    foreach ($part in ($RelativePath -split '[\\/]')) {
        if ([string]::IsNullOrEmpty($part)) { continue }
        $current = Join-Path $current $part
        if (Test-Path -LiteralPath $current) {
            $item = Get-Item -LiteralPath $current -Force
            $isReparse = (([int]$item.Attributes) -band ([int][System.IO.FileAttributes]::ReparsePoint)) -ne 0
            if ($isReparse) {
                throw "Reparse point (symlink/junction) is unsupported beneath EvidenceRoot: $current"
            }
        }
    }
}

if (-not (Test-Path -LiteralPath $EvidenceRoot -PathType Container)) {
    Write-Fail "EvidenceRoot not found or not a directory: $EvidenceRoot"
    exit 1
}
$rootFull = (Resolve-Path -LiteralPath $EvidenceRoot).Path.TrimEnd('\')
$manifestFullTarget = [System.IO.Path]::GetFullPath($ManifestPath)

if (-not $Verify) {
    # -------------------------------------------------------------------
    # CREATE
    # -------------------------------------------------------------------
    Write-Sect 'Create manifest'

    $files = $null
    try {
        $files = Get-EvidenceFiles -RootFull $rootFull
    } catch {
        Write-Fail "Refusing to create manifest: $($_.Exception.Message)"
        exit 1
    }
    $entries = New-Object 'System.Collections.Generic.List[object]'
    foreach ($f in $files) {
        if ($f.FullName -ieq $manifestFullTarget) {
            continue
        }
        $rel = Get-RelativePath -RootFull $rootFull -FullPath $f.FullName
        $hash = $null
        try {
            $hash = (Get-FileHash -LiteralPath $f.FullName -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
        } catch {
            Write-Fail "Failed to hash '$rel': $($_.Exception.Message)"
            exit 1
        }
        $entries.Add([ordered]@{ relative_path = $rel; byte_count = [int64]$f.Length; sha256 = $hash })
    }

    $sortedEntries = $entries.ToArray()
    [Array]::Sort($sortedEntries, [Comparison[object]] { param($a, $b) [string]::CompareOrdinal($a.relative_path, $b.relative_path) })

    # evidence_root is deliberately NOT recorded: it is a caller-supplied
    # execution parameter, not evidence content, and including an absolute
    # path here would make byte-identical evidence stored at different
    # filesystem locations produce different manifest bytes.
    $manifest = [ordered]@{
        schema_version = $ManifestSchemaVersion
        file_count     = $sortedEntries.Count
        files          = $sortedEntries
    }

    $manifestDir = Split-Path -Parent $manifestFullTarget
    if ($manifestDir -and -not (Test-Path -LiteralPath $manifestDir)) {
        New-Item -ItemType Directory -Path $manifestDir -Force | Out-Null
    }
    # Explicit no-BOM UTF-8 via .NET, independent of whether this script runs
    # under Windows PowerShell 5.1 or pwsh -- their `-Encoding UTF8` BOM
    # defaults are not identical, and the operator runbook invokes
    # `powershell` while CI guards run under `pwsh`.
    $manifestJson = $manifest | ConvertTo-Json -Depth 8
    $utf8NoBom = New-Object System.Text.UTF8Encoding($false)
    [System.IO.File]::WriteAllText($manifestFullTarget, $manifestJson, $utf8NoBom)

    Write-Ok "Manifest written: $manifestFullTarget ($($sortedEntries.Count) file(s))"
    exit 0
}

# -----------------------------------------------------------------------------
# VERIFY
# -----------------------------------------------------------------------------
Write-Sect 'Verify manifest'

if (-not (Test-Path -LiteralPath $ManifestPath -PathType Leaf)) {
    Write-Fail "ManifestPath not found: $ManifestPath"
    exit 1
}

$raw = Get-Content -LiteralPath $ManifestPath -Raw
$manifestObj = $null
try {
    $manifestObj = $raw | ConvertFrom-Json -ErrorAction Stop
} catch {
    Write-Fail "Manifest is not valid JSON: $($_.Exception.Message)"
    exit 1
}

$schemaVersion = $null
try { $schemaVersion = [string]$manifestObj.schema_version } catch {}
if ([string]::IsNullOrWhiteSpace($schemaVersion) -or ($schemaVersion -ne $ManifestSchemaVersion)) {
    Write-Fail "Manifest schema_version is missing or unsupported (expected '$ManifestSchemaVersion', got '$schemaVersion') -- refusing to trust an unknown schema."
    exit 1
}

if ($null -eq $manifestObj.files) {
    Write-Fail "Manifest has no 'files' array -- refusing to trust a malformed manifest."
    exit 1
}
$declaredFiles = @($manifestObj.files)

$fileCountRaw = $null
try { $fileCountRaw = $manifestObj.file_count } catch {}
$fileCountWellFormed = ($null -ne $fileCountRaw) -and ($fileCountRaw -is [int] -or $fileCountRaw -is [long] -or $fileCountRaw -is [double]) `
    -and -not [double]::IsNaN([double]$fileCountRaw) -and -not [double]::IsInfinity([double]$fileCountRaw) `
    -and ([double]$fileCountRaw -ge 0) -and ([double]$fileCountRaw -le [long]::MaxValue) `
    -and ([double]$fileCountRaw -eq [math]::Floor([double]$fileCountRaw))
if (-not $fileCountWellFormed) {
    Write-Fail "Manifest file_count is missing or not a well-formed nonnegative integer -- refusing to trust a malformed manifest."
    exit 1
}
if ([int64]$fileCountRaw -ne $declaredFiles.Count) {
    Write-Fail "Manifest file_count ($([int64]$fileCountRaw)) does not match the number of declared 'files' entries ($($declaredFiles.Count)) -- refusing to trust an internally inconsistent manifest."
    exit 1
}

$seenPaths = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
$verifiedRelPaths = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
$failed = $false

foreach ($entry in $declaredFiles) {
    $relPath = $null
    $sha256 = $null
    $byteCount = $null
    try { $relPath = [string]$entry.relative_path } catch {}
    try { $sha256 = [string]$entry.sha256 } catch {}
    try { $byteCount = $entry.byte_count } catch {}

    $byteCountWellFormed = ($null -ne $byteCount) -and ($byteCount -is [int] -or $byteCount -is [long] -or $byteCount -is [double]) `
        -and -not [double]::IsNaN([double]$byteCount) -and -not [double]::IsInfinity([double]$byteCount) `
        -and ([double]$byteCount -ge 0) -and ([double]$byteCount -le [long]::MaxValue) `
        -and ([double]$byteCount -eq [math]::Floor([double]$byteCount))

    if ([string]::IsNullOrWhiteSpace($relPath) -or [string]::IsNullOrWhiteSpace($sha256) -or ($sha256 -notmatch '^[0-9a-fA-F]{64}$') -or -not $byteCountWellFormed) {
        Write-Fail "Malformed manifest entry -- refusing to trust it: $($entry | ConvertTo-Json -Compress -Depth 3)"
        $failed = $true
        continue
    }

    if ([System.IO.Path]::IsPathRooted($relPath) -or ($relPath -match '(^|[\\/])\.\.([\\/]|$)')) {
        Write-Fail "Manifest entry has an absolute or traversal path -- refusing: $relPath"
        $failed = $true
        continue
    }

    if (-not $seenPaths.Add($relPath)) {
        Write-Fail "Manifest lists duplicate path -- refusing to trust an ambiguous manifest: $relPath"
        $failed = $true
        continue
    }

    try {
        Assert-NoReparseInPath -RootFull $rootFull -RelativePath $relPath
    } catch {
        Write-Fail "Reparse point in manifest-declared path -- refusing: $relPath ($($_.Exception.Message))"
        $failed = $true
        continue
    }

    $candidateFull = Join-Path $rootFull $relPath
    $resolvedCandidate = $null
    try { $resolvedCandidate = (Resolve-Path -LiteralPath $candidateFull -ErrorAction Stop).Path } catch {}
    if ([string]::IsNullOrWhiteSpace($resolvedCandidate) -or -not ($resolvedCandidate.TrimEnd('\').StartsWith($rootFull, [System.StringComparison]::OrdinalIgnoreCase))) {
        Write-Fail "Manifest-listed file is missing on disk (or resolves outside EvidenceRoot): $relPath"
        $failed = $true
        continue
    }
    if (-not (Test-Path -LiteralPath $resolvedCandidate -PathType Leaf)) {
        Write-Fail "Manifest-listed file is missing on disk: $relPath"
        $failed = $true
        continue
    }

    $actualRel = Get-RelativePath -RootFull $rootFull -FullPath $resolvedCandidate
    $verifiedRelPaths.Add($actualRel) | Out-Null

    $actualHash = $null
    try {
        $actualHash = (Get-FileHash -LiteralPath $resolvedCandidate -Algorithm SHA256 -ErrorAction Stop).Hash.ToLowerInvariant()
    } catch {
        Write-Fail "Failed to hash '$relPath' during verification: $($_.Exception.Message)"
        $failed = $true
        continue
    }
    if ($actualHash -ne $sha256.ToLowerInvariant()) {
        Write-Fail "sha256 mismatch for ${relPath}: manifest=$sha256 actual=$actualHash"
        $failed = $true
        continue
    }

    $actualByteCount = (Get-Item -LiteralPath $resolvedCandidate).Length
    $manifestByteCount = [int64]$byteCount
    if ($manifestByteCount -ne $actualByteCount) {
        Write-Fail "byte_count mismatch for ${relPath}: manifest=$manifestByteCount actual=$actualByteCount"
        $failed = $true
        continue
    }
}

# Strict verification: a file present on disk under EvidenceRoot but not
# declared in the manifest also fails the run (covers silent additions and
# the "new name" half of a rename).
$manifestFullSelf = [System.IO.Path]::GetFullPath($ManifestPath)
$actualFiles = $null
try {
    $actualFiles = Get-EvidenceFiles -RootFull $rootFull
} catch {
    Write-Fail "Refusing to trust evidence set: $($_.Exception.Message)"
    $failed = $true
    $actualFiles = @()
}
foreach ($f in $actualFiles) {
    if ($f.FullName -ieq $manifestFullSelf) {
        continue
    }
    $rel = Get-RelativePath -RootFull $rootFull -FullPath $f.FullName
    if (-not $verifiedRelPaths.Contains($rel)) {
        Write-Fail "Undeclared file present under EvidenceRoot (strict verification): $rel"
        $failed = $true
    }
}

if ($failed) {
    Write-Fail 'Manifest verification failed -- do not trust this evidence set.'
    exit 1
}
Write-Ok "Manifest verification passed: $($declaredFiles.Count) file(s) verified against $rootFull."
exit 0
