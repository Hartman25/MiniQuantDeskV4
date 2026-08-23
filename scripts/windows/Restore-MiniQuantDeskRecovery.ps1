# =============================================================================
# OPS-OFFSITE-BACKUP-01
# Restore-MiniQuantDeskRecovery.ps1
#
# Restores a staged backup set (from Backup-MiniQuantDeskRecovery.ps1) into a
# DISPOSABLE PostgreSQL database only -- never the real Paper DB, never Live,
# never the primary repo, never the active Research registry (D14).
#
# HARD FENCE: refuses to run against the 'mqk-paper-postgres'/'mqk-live-postgres'
# containers, or any container/disposable-DB name containing "paper"/"live",
# regardless of what is passed in. The only accepted target is the existing
# disposable test-Postgres container (default mqk-test-postgres -- see
# LOCAL-TEST-DB-ENVIRONMENT-FIX-01) with a freshly created, uniquely named
# database that this script drops again at the end unless -KeepDisposableDb
# is passed.
#
# pg_restore/psql run INSIDE the disposable container via `docker exec`
# (its own bundled, version-matched client tools) rather than a separately
# installed Windows client -- this is the same reasoning
# Backup-MiniQuantDeskRecovery.ps1 uses for pg_dump: it guarantees the
# client major version always matches the server, so a dump produced by a
# newer/older client's pg_dump can never emit a session GUC the restoring
# server doesn't recognize. The dump is pushed into the container via
# `docker cp` (safe for the binary custom-format dump; no PowerShell
# stdout-redirection encoding risk).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Restore-MiniQuantDeskRecovery.ps1 -BackupDir <staged backup dir>
#
# Exit codes: 0 = restore + integrity verification passed, 1 = failure
# (including a refused unsafe target -- that is a safety refusal, not a
# successful restore).
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string] $BackupDir,

    [string] $DisposableDbContainer = 'mqk-test-postgres',
    [string] $DisposableDbUser      = 'postgres',
    [string] $DisposableDbName      = '',
    [switch] $KeepDisposableDb
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$M) Write-Host "[RESTORE] $M"       -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[RESTORE] OK: $M"   -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[RESTORE] WARN: $M" -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[RESTORE] FAIL: $M" -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# HARD FENCE -- checked before anything else touches the container/DB name.
# ---------------------------------------------------------------------------
$ForbiddenContainers = @('mqk-paper-postgres', 'mqk-live-postgres')
if (($ForbiddenContainers -contains $DisposableDbContainer) -or ($DisposableDbContainer -match '(?i)paper|live')) {
    Write-Fail "REFUSING: '$DisposableDbContainer' is (or names) a real Paper/Live database container. Restore targets only a disposable test instance."
    exit 1
}
if ([string]::IsNullOrWhiteSpace($DisposableDbName)) {
    $DisposableDbName = "mqk_recovery_restore_$(Get-Date -Format 'yyyyMMdd_HHmmss')"
}
if ($DisposableDbName -match '(?i)paper|live') {
    Write-Fail "REFUSING: disposable database name '$DisposableDbName' contains 'paper' or 'live'. Choose a name that cannot be confused with a real target."
    exit 1
}
# SQL-identifier safety (D-R1 defect 5): DisposableDbName is interpolated
# directly into CREATE DATABASE / DROP DATABASE statements below. Require a
# conservative identifier before it ever reaches docker/SQL -- rejects
# anything that could break out of the intended identifier position.
if ($DisposableDbName -notmatch '^[A-Za-z_][A-Za-z0-9_]*$') {
    Write-Fail "REFUSING: disposable database name '$DisposableDbName' is not a conservative SQL identifier (expected ^[A-Za-z_][A-Za-z0-9_]*`$)."
    exit 1
}

if (-not (Test-Path -LiteralPath $BackupDir)) {
    Write-Fail "BackupDir not found: $BackupDir"
    exit 1
}
$manifestPath = Join-Path $BackupDir 'manifest.json'
$dumpPath = Join-Path $BackupDir 'paper_db.dump'
if (-not (Test-Path -LiteralPath $manifestPath)) {
    Write-Fail "manifest.json not found in $BackupDir -- refusing to trust an unmanifested backup set."
    exit 1
}
if (-not (Test-Path -LiteralPath $dumpPath)) {
    Write-Fail "paper_db.dump not found in $BackupDir -- nothing to restore."
    exit 1
}

# ---------------------------------------------------------------------------
# Secret-exclusion re-check (defense in depth -- same patterns
# Backup-MiniQuantDeskRecovery.ps1 already enforced at stage time).
# ---------------------------------------------------------------------------
Write-Sect 'Secret-exclusion re-check'
$SecretPatterns = @('^\.env(\..*)?$', '(?i)secret', '(?i)credential', '(?i)api[_-]?key', '(?i)api[_-]?secret', '(?i)password', '(?i)\.pem$', '(?i)\.pfx$')
$secretHits = Get-ChildItem -Path $BackupDir -Recurse -File | Where-Object {
    $name = $_.Name
    $isSecret = $false
    foreach ($pat in $SecretPatterns) {
        if ($name -match $pat) { $isSecret = $true; break }
    }
    $isSecret
}
if ($secretHits -and $secretHits.Count -gt 0) {
    foreach ($hit in $secretHits) { Write-Fail "SECRET-PATTERN FILE IN STAGED SET: $($hit.FullName)" }
    Write-Fail 'Refusing to restore a backup set containing a secret-pattern file.'
    exit 1
}
Write-Ok 'No secret-pattern files found in the staged set.'

# ---------------------------------------------------------------------------
# Full manifest verification (D-R1 defect 2) before attempting restore.
# Every listed file -- not paper_db.dump alone -- must: have a well-formed
# entry (nonblank path, 32-hex-char-lowercase-normalized sha256, numeric
# size_bytes), resolve strictly inside $BackupDir (no absolute path, no ..
# traversal), exist on disk, and hash-match. Duplicate manifest paths fail
# closed rather than silently verifying only the first/last occurrence.
# ---------------------------------------------------------------------------
Write-Sect 'Full manifest verification'
$manifest = Get-Content -Path $manifestPath -Raw | ConvertFrom-Json
$manifestFiles = @($manifest.files)
if ($manifestFiles.Count -eq 0) {
    Write-Fail 'manifest.json lists zero files -- refusing to trust an empty manifest.'
    exit 1
}

$backupRootFull = (Resolve-Path -LiteralPath $BackupDir).Path.TrimEnd('\')
$seenPaths = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
$manifestVerificationFailed = $false

foreach ($entry in $manifestFiles) {
    $relPath = $null
    $sha256 = $null
    $sizeBytes = $null
    try { $relPath = [string]$entry.path } catch {}
    try { $sha256 = [string]$entry.sha256 } catch {}
    try { $sizeBytes = $entry.size_bytes } catch {}

    if ([string]::IsNullOrWhiteSpace($relPath) -or [string]::IsNullOrWhiteSpace($sha256) -or ($sha256 -notmatch '^[0-9a-fA-F]{64}$') -or ($null -eq $sizeBytes) -or -not ($sizeBytes -is [int] -or $sizeBytes -is [long] -or $sizeBytes -is [double]) -or ([double]$sizeBytes -lt 0)) {
        Write-Fail "Malformed manifest entry -- refusing to trust it: $($entry | ConvertTo-Json -Compress -Depth 3)"
        $manifestVerificationFailed = $true
        continue
    }

    if ([System.IO.Path]::IsPathRooted($relPath) -or ($relPath -match '(^|[\\/])\.\.([\\/]|$)')) {
        Write-Fail "Manifest entry has an absolute or traversal path -- refusing: $relPath"
        $manifestVerificationFailed = $true
        continue
    }

    if (-not $seenPaths.Add($relPath)) {
        Write-Fail "Manifest lists duplicate path -- refusing to trust an ambiguous manifest: $relPath"
        $manifestVerificationFailed = $true
        continue
    }

    $candidateFull = Join-Path $BackupDir $relPath
    $resolvedCandidate = $null
    try { $resolvedCandidate = (Resolve-Path -LiteralPath $candidateFull -ErrorAction Stop).Path } catch {}
    if ([string]::IsNullOrWhiteSpace($resolvedCandidate) -or -not ($resolvedCandidate.TrimEnd('\').StartsWith($backupRootFull, [System.StringComparison]::OrdinalIgnoreCase))) {
        Write-Fail "Manifest entry resolves outside the backup root -- refusing: $relPath"
        $manifestVerificationFailed = $true
        continue
    }

    if (-not (Test-Path -LiteralPath $resolvedCandidate -PathType Leaf)) {
        Write-Fail "Manifest-listed file is missing on disk: $relPath"
        $manifestVerificationFailed = $true
        continue
    }

    $actualEntryHash = (Get-FileHash -Path $resolvedCandidate -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actualEntryHash -ne $sha256.ToLowerInvariant()) {
        Write-Fail "SHA-256 mismatch for ${relPath}: manifest=$sha256 actual=$actualEntryHash"
        $manifestVerificationFailed = $true
        continue
    }
}

if ($manifestVerificationFailed) {
    Write-Fail 'Full manifest verification failed -- refusing to restore any content from this backup set.'
    exit 1
}
Write-Ok "Full manifest verification passed: $($manifestFiles.Count) file(s) verified against $BackupDir."

$dumpRelPath = 'paper_db.dump'
$dumpEntry = $manifest.files | Where-Object { $_.path -eq $dumpRelPath } | Select-Object -First 1
if (-not $dumpEntry) {
    Write-Fail "manifest.json does not list $dumpRelPath -- refusing to trust an unlisted file."
    exit 1
}
Write-Ok "paper_db.dump integrity verified as part of the full manifest above (sha256=$($dumpEntry.sha256))."

# ---------------------------------------------------------------------------
# Confirm the disposable container exists/is running before anything else.
# ---------------------------------------------------------------------------
docker inspect $DisposableDbContainer 2>&1 | Out-Null
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Container '$DisposableDbContainer' not found/running. Is Docker Desktop running?"
    exit 1
}

# ---------------------------------------------------------------------------
# Push the dump into the container, create the disposable database.
# ---------------------------------------------------------------------------
$containerDumpPath = '/tmp/mqk_restore_paper_db.dump'
Write-Sect "Copy dump into '$DisposableDbContainer'"
docker cp $dumpPath "${DisposableDbContainer}:${containerDumpPath}" 2>&1 | Out-Host
if ($LASTEXITCODE -ne 0) {
    Write-Fail "docker cp into container failed (exit $LASTEXITCODE)."
    exit 1
}

Write-Sect "Create disposable database '$DisposableDbName' in '$DisposableDbContainer'"
docker exec $DisposableDbContainer psql -U $DisposableDbUser -v ON_ERROR_STOP=1 -c "CREATE DATABASE $DisposableDbName;" 2>&1 | Out-Host
if ($LASTEXITCODE -ne 0) {
    Write-Fail "CREATE DATABASE failed (exit $LASTEXITCODE)."
    docker exec $DisposableDbContainer rm -f $containerDumpPath 2>&1 | Out-Null
    exit 1
}
Write-Ok "Disposable database '$DisposableDbName' created."

$restoreFailed = $false
$cleanupFailed = $false
try {
    Write-Sect 'pg_restore into disposable database'
    docker exec $DisposableDbContainer pg_restore -U $DisposableDbUser --no-owner --no-privileges -d $DisposableDbName $containerDumpPath 2>&1 | Out-Host
    # D-R1 defect 1: a nonzero pg_restore exit is no longer treated as
    # benign. --no-owner --no-privileges already suppresses the expected
    # ownership/role noise, so a remaining nonzero exit means a genuine
    # restore problem and must fail this proof outright.
    if ($LASTEXITCODE -ne 0) {
        Write-Fail "pg_restore exited nonzero ($LASTEXITCODE) -- this is not an acceptable recovery proof."
        $restoreFailed = $true
    } else {
        Write-Ok 'pg_restore exited 0.'
    }

    Write-Sect 'Post-restore verification'
    $tableCountRaw = docker exec $DisposableDbContainer psql -U $DisposableDbUser -d $DisposableDbName -t -A -c "SELECT count(*) FROM information_schema.tables WHERE table_schema = 'public';" 2>&1
    $tableCount = 0
    [int]::TryParse(($tableCountRaw | Select-Object -Last 1).Trim(), [ref]$tableCount) | Out-Null

    if ($tableCount -le 0) {
        Write-Fail "Post-restore verification failed: 0 public tables found in disposable database after restore."
        $restoreFailed = $true
    } else {
        Write-Ok "Post-restore verification: $tableCount public table(s) present in the disposable database."
    }
} finally {
    docker exec $DisposableDbContainer rm -f $containerDumpPath 2>&1 | Out-Null
    if (-not $KeepDisposableDb) {
        Write-Sect "Cleanup: dropping disposable database '$DisposableDbName'"
        docker exec $DisposableDbContainer psql -U $DisposableDbUser -v ON_ERROR_STOP=1 -c "DROP DATABASE IF EXISTS $DisposableDbName;" 2>&1 | Out-Host
        if ($LASTEXITCODE -ne 0) {
            # D-R1 defect 6: cleanup failure must fail the overall proof,
            # not just warn -- a PASS must never be printed alongside a
            # disposable database left behind un-dropped.
            Write-Fail "Failed to drop disposable database '$DisposableDbName' (exit $LASTEXITCODE) -- operator cleanup required."
            $cleanupFailed = $true
        } else {
            Write-Ok "Disposable database '$DisposableDbName' dropped."
        }
    } else {
        Write-Warn "-KeepDisposableDb was passed: '$DisposableDbName' left in place in '$DisposableDbContainer' for manual inspection."
    }
}

if ($restoreFailed -or $cleanupFailed) {
    exit 1
}
Write-Sect 'Summary'
Write-Ok 'Restore into a disposable database and integrity verification both passed.'
exit 0
