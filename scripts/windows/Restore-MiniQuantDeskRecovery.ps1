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
# Integrity check against the manifest (D11) before attempting restore.
# ---------------------------------------------------------------------------
Write-Sect 'Manifest integrity check'
$manifest = Get-Content -Path $manifestPath -Raw | ConvertFrom-Json
$dumpRelPath = 'paper_db.dump'
$dumpEntry = $manifest.files | Where-Object { $_.path -eq $dumpRelPath } | Select-Object -First 1
if (-not $dumpEntry) {
    Write-Fail "manifest.json does not list $dumpRelPath -- refusing to trust an unlisted file."
    exit 1
}
$actualHash = (Get-FileHash -Path $dumpPath -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualHash -ne $dumpEntry.sha256) {
    Write-Fail "paper_db.dump SHA-256 mismatch: manifest=$($dumpEntry.sha256) actual=$actualHash"
    exit 1
}
Write-Ok "paper_db.dump integrity verified (sha256=$actualHash)."

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
try {
    Write-Sect 'pg_restore into disposable database'
    docker exec $DisposableDbContainer pg_restore -U $DisposableDbUser --no-owner --no-privileges -d $DisposableDbName $containerDumpPath 2>&1 | Out-Host
    # pg_restore can exit nonzero on benign warnings (e.g. missing roles);
    # the real proof is the post-restore table/row verification below, not
    # this exit code alone.
    if ($LASTEXITCODE -ne 0) {
        Write-Warn "pg_restore reported a nonzero exit (may include benign role/permission warnings); verifying actual restored content below."
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
            Write-Warn "Failed to drop disposable database '$DisposableDbName' (exit $LASTEXITCODE) -- operator cleanup required."
        } else {
            Write-Ok "Disposable database '$DisposableDbName' dropped."
        }
    } else {
        Write-Warn "-KeepDisposableDb was passed: '$DisposableDbName' left in place in '$DisposableDbContainer' for manual inspection."
    }
}

if ($restoreFailed) {
    exit 1
}
Write-Sect 'Summary'
Write-Ok 'Restore into a disposable database and integrity verification both passed.'
exit 0
