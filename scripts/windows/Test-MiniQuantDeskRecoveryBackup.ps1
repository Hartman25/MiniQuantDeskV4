# =============================================================================
# OPS-OFFSITE-BACKUP-01
# Test-MiniQuantDeskRecoveryBackup.ps1
#
# End-to-end proof for Backup-MiniQuantDeskRecovery.ps1 /
# Restore-MiniQuantDeskRecovery.ps1: runs a REAL backup (real git bundle,
# real pg_dump against the Paper DB -- read-only), REAL restore into a
# disposable database on the existing disposable test-Postgres instance
# (127.0.0.1:5434), and a set of static/adversarial proofs over the
# secret-exclusion and unsafe-target-refusal logic.
#
# Never touches: the real Paper DB beyond a read-only pg_dump, Live, the
# primary repo (git bundle reads only), smoke_logs/, or any B2/restic
# credential (none are read, printed, or required for this proof -- restic
# itself is expected to report RESTIC_INSTALL_REQUIRED on a box where it is
# not installed, which this test treats as a truthful PARTIAL, not a
# failure).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Test-MiniQuantDeskRecoveryBackup.ps1
#
# Exit codes: 0 = all proofs held, 1 = at least one did not.
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$BackupScript  = Join-Path $ScriptDir 'Backup-MiniQuantDeskRecovery.ps1'
$RestoreScript = Join-Path $ScriptDir 'Restore-MiniQuantDeskRecovery.ps1'

# Real child invocations that are EXPECTED to fail/emit stderr (mutation and
# refusal controls) must not use `& powershell ... 2>&1` -- under
# $ErrorActionPreference = 'Stop', PowerShell treats a native child's own
# stderr output (e.g. a real docker/pg_restore error line) as a terminating
# NativeCommandError in THIS process, even though it originated one level
# down. Start-Process with file-redirected streams avoids that entirely.
function Invoke-TestSubprocess {
    param([Parameter(Mandatory = $true)][string[]]$ArgumentList)
    $stdoutFile = [System.IO.Path]::GetTempFileName()
    $stderrFile = [System.IO.Path]::GetTempFileName()
    try {
        $proc = Start-Process -FilePath 'powershell.exe' -NoNewWindow -Wait -PassThru `
            -ArgumentList $ArgumentList -RedirectStandardOutput $stdoutFile -RedirectStandardError $stderrFile
        return $proc.ExitCode
    } finally {
        Remove-Item -Path $stdoutFile, $stderrFile -Force -ErrorAction SilentlyContinue
    }
}

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

foreach ($p in @($BackupScript, $RestoreScript)) {
    if (-not (Test-Path -LiteralPath $p)) {
        Show-Red "FATAL -- required script not found: $p"
        exit 1
    }
}

# ---------------------------------------------------------------------------
# Section 1: static secret-exclusion pattern proof (adversarial fixture --
# same pattern list Backup-MiniQuantDeskRecovery.ps1 enforces).
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 1: secret-exclusion pattern proof (static) ==='

$SecretPatterns = @('^\.env(\..*)?$', '(?i)secret', '(?i)credential', '(?i)api[_-]?key', '(?i)api[_-]?secret', '(?i)password', '(?i)\.pem$', '(?i)\.pfx$')
function Test-NameAgainstPatterns {
    param([string]$Name)
    foreach ($pat in $SecretPatterns) { if ($Name -match $pat) { return $true } }
    return $false
}

$mustExclude = @('.env', '.env.local', '.env.local.example', 'api_key.json', 'api-secret.txt', 'db_password.yaml', 'my_credential_file.json', 'tls_cert.pem', 'client.pfx')
foreach ($name in $mustExclude) {
    Assert-True "secret pattern correctly excludes '$name'" (Test-NameAgainstPatterns -Name $name)
}

$mustInclude = @('equities.json', 'providers.json', 'manifest.json', 'paper_db.dump', 'git_identity.json', 'Research_Backtest_V1_Closeout_Audit.md')
foreach ($name in $mustInclude) {
    Assert-True "secret pattern correctly does NOT flag legitimate file '$name'" (-not (Test-NameAgainstPatterns -Name $name))
}

# ---------------------------------------------------------------------------
# Section 2: static unsafe-target-refusal proof against Restore script text.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 2: unsafe-target hard-fence proof (static) ==='

$RestoreText = Get-Content -Path $RestoreScript -Raw
Assert-True 'Restore script hard-fences the mqk-paper-postgres/mqk-live-postgres container names' `
    ($RestoreText -match [regex]::Escape("@('mqk-paper-postgres', 'mqk-live-postgres')"))
Assert-True 'Restore script refuses a disposable container/DB name containing paper/live' `
    (([regex]::Matches($RestoreText, [regex]::Escape("-match '(?i)paper|live'"))).Count -ge 2)
Assert-True 'Restore script requires manifest.json before trusting the dump' `
    ($RestoreText -match [regex]::Escape('manifest.json not found'))
Assert-True 'Restore script verifies SHA-256 before restoring' `
    ($RestoreText -match [regex]::Escape('SHA-256 mismatch'))
Assert-True 'Restore script drops the disposable database by default (cleanup)' `
    ($RestoreText -match [regex]::Escape('DROP DATABASE IF EXISTS'))
Assert-True 'Restore script requires a conservative SQL identifier for DisposableDbName before any docker/SQL use' `
    ($RestoreText -match [regex]::Escape("'^[A-Za-z_][A-Za-z0-9_]*`$'"))
Assert-True 'Restore script fails closed on a nonzero pg_restore exit (never blessed as benign)' `
    ($RestoreText -match [regex]::Escape('pg_restore exited nonzero') -and -not ($RestoreText -match [regex]::Escape('may include benign role/permission warnings')))
Assert-True 'Restore script fails the overall proof when disposable-DB cleanup fails' `
    ($RestoreText -match [regex]::Escape('$cleanupFailed = $true') -and $RestoreText -match [regex]::Escape('if ($restoreFailed -or $cleanupFailed)'))
Assert-True 'Restore script verifies the FULL manifest (every file), not paper_db.dump alone' `
    ($RestoreText -match [regex]::Escape('Full manifest verification') -and $RestoreText -match [regex]::Escape('foreach ($entry in $manifestFiles)'))
Assert-True 'Restore script rejects duplicate manifest paths' `
    ($RestoreText -match [regex]::Escape('lists duplicate path'))
Assert-True 'Restore script rejects absolute/traversal manifest paths' `
    ($RestoreText -match [regex]::Escape('IsPathRooted') -and $RestoreText -match [regex]::Escape('resolves outside the backup root'))
Assert-True 'Restore script compares actual restored file length against manifest size_bytes (D-R1-R2 defect 01)' `
    ($RestoreText -match [regex]::Escape('size_bytes mismatch for'))

$BackupText = Get-Content -Path $BackupScript -Raw
Assert-True 'Backup script never copies the Postgres data directory (pg_dump only)' `
    (($BackupText -match [regex]::Escape('pg_dump')) -and -not ($BackupText -match '(?i)Copy-Item.*(pgdata|data\\base)'))
Assert-True 'Backup script fails closed when a required component fails (D15)' `
    ($BackupText -match [regex]::Escape('$RequiredComponentFailed = $true'))
Assert-True 'Backup script performs a post-stage secret scan (defense in depth)' `
    ($BackupText -match [regex]::Escape('Post-stage secret-exclusion scan'))
Assert-True 'Backup script never invokes a B2/upload call' `
    (-not ($BackupText -match '(?i)b2 upload|Invoke-WebRequest|Invoke-RestMethod'))
Assert-True 'Backup script never captures the Git remote URL (D-R1 defect 3)' `
    (-not ($BackupText -match [regex]::Escape('remote_url')) -and -not ($BackupText -match [regex]::Escape('remote get-url')))
Assert-True 'Backup script proves .env.local/.env are gitignored and untracked BEFORE staging anything' `
    ($BackupText -match [regex]::Escape('Local secret-store exclusion authority') -and $BackupText -match [regex]::Escape('check-ignore'))
Assert-True 'Backup script scans staged content for allowlisted local secret VALUES, not just filenames (D-R1 defect 4)' `
    ($BackupText -match [regex]::Escape('function Get-AllowlistedLocalSecretValues') -and $BackupText -match [regex]::Escape('Content-canary secret scan'))
Assert-True 'Backup script''s own manifest never claims offsite success by itself (local-only status field)' `
    ($BackupText -match [regex]::Escape("overall_status    = 'LOCAL_RECOVERY_SET_STAGED'"))

# ---------------------------------------------------------------------------
# Section 3: real end-to-end backup + restore proof.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 3: real end-to-end backup + restore ==='

$scratchOutDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_recovery_backup_test_" + [guid]::NewGuid().ToString('N'))

& powershell -NoProfile -ExecutionPolicy Bypass -File $BackupScript -RepoRoot $RepoRoot -OutDir $scratchOutDir | Out-Host
$backupExit = $LASTEXITCODE
Assert-True 'Backup-MiniQuantDeskRecovery.ps1 exits 0' ($backupExit -eq 0)

if ($backupExit -eq 0) {
    $manifestPath = Join-Path $scratchOutDir 'manifest.json'
    Assert-True 'manifest.json was written' (Test-Path -LiteralPath $manifestPath)

    if (Test-Path -LiteralPath $manifestPath) {
        $manifest = Get-Content -Path $manifestPath -Raw | ConvertFrom-Json
        Assert-True 'git_identity component is ok' ($manifest.components.git_identity.status -eq 'ok')
        Assert-True 'paper_db_dump component is ok' ($manifest.components.paper_db_dump.status -eq 'ok')
        Assert-True 'safe_config component is ok' ($manifest.components.safe_config.status -eq 'ok')
        Assert-True 'secret_scan component is ok' ($manifest.components.secret_scan.status -eq 'ok')
        Assert-True 'total_size_bytes is measured and positive (D16)' ($manifest.total_size_bytes -gt 0)
        Assert-True 'retention_policy is documented' ($null -ne $manifest.retention_policy.daily_snapshots)

        if ($manifest.restic.status -eq 'not_installed') {
            Show-Info '  INFO -- restic not installed on this box: RESTIC_INSTALL_REQUIRED (truthful PARTIAL, not a test failure)'
        }

        & powershell -NoProfile -ExecutionPolicy Bypass -File $RestoreScript -BackupDir $scratchOutDir | Out-Host
        $restoreExit = $LASTEXITCODE
        Assert-True 'Restore-MiniQuantDeskRecovery.ps1 exits 0 (real restore into disposable DB, then cleanup)' ($restoreExit -eq 0)
    }
}

Remove-Item -Path $scratchOutDir -Recurse -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------
# Section 4: unsafe-target negative control -- Restore refuses a forbidden
# port even when a caller explicitly passes one (real invocation, not just
# static text -- proves the fence actually executes, not just that the
# source mentions it).
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 4: unsafe-target negative control (real invocation) ==='

$dummyBackupDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_recovery_dummy_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $dummyBackupDir | Out-Null
Set-Content -Path (Join-Path $dummyBackupDir 'manifest.json') -Value '{"files":[]}' -Encoding UTF8
Set-Content -Path (Join-Path $dummyBackupDir 'paper_db.dump') -Value 'not a real dump' -Encoding UTF8

& powershell -NoProfile -ExecutionPolicy Bypass -File $RestoreScript -BackupDir $dummyBackupDir -DisposableDbContainer 'mqk-paper-postgres' 2>&1 | Out-Null
$forbiddenContainerExit = $LASTEXITCODE
Assert-True 'Restore refuses (nonzero exit) when pointed at the mqk-paper-postgres container even with an explicit override' ($forbiddenContainerExit -ne 0)

& powershell -NoProfile -ExecutionPolicy Bypass -File $RestoreScript -BackupDir $dummyBackupDir -DisposableDbName 'paper_something' 2>&1 | Out-Null
$forbiddenNameExit = $LASTEXITCODE
Assert-True 'Restore refuses (nonzero exit) when the disposable DB name contains "paper"' ($forbiddenNameExit -ne 0)

Remove-Item -Path $dummyBackupDir -Recurse -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------
# Section 5: real pg_restore-failure mutation control (D-R1 defect 1).
# A corrupted dump whose manifest entry is hash-CORRECT (so manifest
# verification passes) must still fail the restore -- proves a nonzero
# pg_restore exit is no longer blessed by the post-restore table count
# alone.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 5: pg_restore failure mutation control (real) ==='

$corruptDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_recovery_corrupt_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $corruptDir | Out-Null
$corruptDumpPath = Join-Path $corruptDir 'paper_db.dump'
Set-Content -Path $corruptDumpPath -Value 'THIS IS NOT A VALID PG_DUMP CUSTOM-FORMAT FILE' -Encoding UTF8 -NoNewline
$corruptHash = (Get-FileHash -Path $corruptDumpPath -Algorithm SHA256).Hash.ToLowerInvariant()
$corruptManifest = [ordered]@{
    files = @(@{ path = 'paper_db.dump'; sha256 = $corruptHash; size_bytes = (Get-Item $corruptDumpPath).Length })
}
($corruptManifest | ConvertTo-Json -Depth 5) | Set-Content -Path (Join-Path $corruptDir 'manifest.json') -Encoding UTF8

$corruptRestoreExit = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $RestoreScript, '-BackupDir', $corruptDir, '-DisposableDbName', 'mqk_recovery_corrupt_mutation_test')
Assert-True 'pg_restore mutation: a hash-correct but content-corrupt dump fails the restore (nonzero exit), never blessed as success' ($corruptRestoreExit -ne 0)

Remove-Item -Path $corruptDir -Recurse -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------
# Section 6: full-manifest verification mutation controls (D-R1 defect 2).
# Each case fails BEFORE any docker/pg_restore call (manifest verification
# runs first), so these are cheap. A shared helper stages a minimal
# paper_db.dump + extra.txt pair and writes the caller-supplied manifest.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 6: full-manifest verification mutation controls (real) ==='

function New-ManifestMutationBackupDir {
    param([Parameter(Mandatory = $true)][array]$FilesEntries, [switch]$OmitExtraFile)
    $dir = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_recovery_manifest_mut_" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $dir | Out-Null
    Set-Content -Path (Join-Path $dir 'paper_db.dump') -Value 'irrelevant-for-manifest-stage-checks' -Encoding UTF8 -NoNewline
    if (-not $OmitExtraFile) {
        Set-Content -Path (Join-Path $dir 'extra.txt') -Value 'extra-file-content' -Encoding UTF8 -NoNewline
    }
    (@{ files = $FilesEntries } | ConvertTo-Json -Depth 5) | Set-Content -Path (Join-Path $dir 'manifest.json') -Encoding UTF8
    return $dir
}

$dumpHashForMutations = (Get-FileHash -InputStream ([System.IO.MemoryStream]::new([System.Text.Encoding]::UTF8.GetBytes('irrelevant-for-manifest-stage-checks'))) -Algorithm SHA256).Hash.ToLowerInvariant()
$extraHashCorrect = (Get-FileHash -InputStream ([System.IO.MemoryStream]::new([System.Text.Encoding]::UTF8.GetBytes('extra-file-content'))) -Algorithm SHA256).Hash.ToLowerInvariant()

$mutationCases = @(
    @{
        Name = 'missing file (manifest lists extra.txt but it was never staged)'
        FilesEntries = @(
            @{ path = 'paper_db.dump'; sha256 = $dumpHashForMutations; size_bytes = 37 },
            @{ path = 'extra.txt'; sha256 = $extraHashCorrect; size_bytes = 19 }
        )
        OmitExtraFile = $true
    },
    @{
        Name = 'hash mismatch on a non-dump file'
        FilesEntries = @(
            @{ path = 'paper_db.dump'; sha256 = $dumpHashForMutations; size_bytes = 37 },
            @{ path = 'extra.txt'; sha256 = ('0' * 64); size_bytes = 19 }
        )
        OmitExtraFile = $false
    },
    @{
        Name = 'duplicate manifest path'
        FilesEntries = @(
            @{ path = 'paper_db.dump'; sha256 = $dumpHashForMutations; size_bytes = 37 },
            @{ path = 'extra.txt'; sha256 = $extraHashCorrect; size_bytes = 19 },
            @{ path = 'extra.txt'; sha256 = $extraHashCorrect; size_bytes = 19 }
        )
        OmitExtraFile = $false
    },
    @{
        Name = 'path traversal outside backup root'
        FilesEntries = @(
            @{ path = 'paper_db.dump'; sha256 = $dumpHashForMutations; size_bytes = 37 },
            @{ path = '..\evil.txt'; sha256 = $extraHashCorrect; size_bytes = 19 }
        )
        OmitExtraFile = $false
    },
    @{
        Name = 'malformed entry (sha256 not 64 hex chars)'
        FilesEntries = @(
            @{ path = 'paper_db.dump'; sha256 = $dumpHashForMutations; size_bytes = 37 },
            @{ path = 'extra.txt'; sha256 = 'not-a-real-hash'; size_bytes = 19 }
        )
        OmitExtraFile = $false
    },
    @{
        Name = 'size_bytes mismatch on a hash-correct file (D-R1-R2 defect 01)'
        FilesEntries = @(
            @{ path = 'paper_db.dump'; sha256 = $dumpHashForMutations; size_bytes = 999999 },
            @{ path = 'extra.txt'; sha256 = $extraHashCorrect; size_bytes = 19 }
        )
        OmitExtraFile = $false
    },
    @{
        Name = 'size_bytes fractional value (D-R1-R2 defect 01)'
        FilesEntries = @(
            @{ path = 'paper_db.dump'; sha256 = $dumpHashForMutations; size_bytes = 37.5 },
            @{ path = 'extra.txt'; sha256 = $extraHashCorrect; size_bytes = 19 }
        )
        OmitExtraFile = $false
    }
)

foreach ($case in $mutationCases) {
    $mutDir = New-ManifestMutationBackupDir -FilesEntries $case.FilesEntries -OmitExtraFile:$case.OmitExtraFile
    $mutExit = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $RestoreScript, '-BackupDir', $mutDir, '-DisposableDbName', 'mqk_recovery_manifest_mutation_test')
    Assert-True "full-manifest mutation control: $($case.Name) is refused (nonzero exit, real invocation)" ($mutExit -ne 0)
    Remove-Item -Path $mutDir -Recurse -Force -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------------------
# Section 7: remote-URL secret leak + content-canary negative controls
# (D-R1 defects 3 + 4). A scratch git repo carries a fake remote URL with an
# embedded credential AND a committed config file that leaks an allowlisted
# local secret VALUE (a name that would NOT be caught by filename-pattern
# matching alone -- proving the content-authority fix, not just the
# filename one). Runs a REAL backup against this scratch repo (the Paper DB
# dump target is unaffected -- it uses the fixed default container name,
# not RepoRoot).
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 7: remote-URL + content-canary secret negative control (real) ==='

$canaryValue = 'CANARY-SECRET-VALUE-0f9e8d7c6b5a'
$remoteTokenValue = 'CANARY-REMOTE-TOKEN-1a2b3c4d5e6f'
$canaryRepoRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_recovery_canary_repo_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $canaryRepoRoot | Out-Null
New-Item -ItemType Directory -Force -Path (Join-Path $canaryRepoRoot 'config') | Out-Null

Set-Content -Path (Join-Path $canaryRepoRoot '.gitignore') -Value '.env.local' -Encoding UTF8
Set-Content -Path (Join-Path $canaryRepoRoot 'config\leaky_config.json') -Value "{`"leaked_value`": `"$canaryValue`"}" -Encoding UTF8

& git -C $canaryRepoRoot init -q 2>&1 | Out-Null
& git -C $canaryRepoRoot -c user.email='test@example.com' -c user.name='test' add .gitignore config 2>&1 | Out-Null
& git -C $canaryRepoRoot -c user.email='test@example.com' -c user.name='test' commit -q -m 'canary fixture' 2>&1 | Out-Null
& git -C $canaryRepoRoot remote add origin "https://canaryuser:$remoteTokenValue@example.invalid/repo.git" 2>&1 | Out-Null
Set-Content -Path (Join-Path $canaryRepoRoot '.env.local') -Value "MQK_OPERATOR_TOKEN=$canaryValue" -Encoding UTF8

$canaryOutDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_recovery_canary_out_" + [guid]::NewGuid().ToString('N'))
& powershell -NoProfile -ExecutionPolicy Bypass -File $BackupScript -RepoRoot $canaryRepoRoot -OutDir $canaryOutDir | Out-Host
$canaryBackupExit = $LASTEXITCODE
Assert-True 'content-canary negative control: backup refuses (nonzero exit) when a staged config file leaks an allowlisted local secret value' ($canaryBackupExit -ne 0)

if (Test-Path -LiteralPath $canaryOutDir) {
    $stagedFiles = Get-ChildItem -Path $canaryOutDir -Recurse -File -ErrorAction SilentlyContinue
    $remoteTokenLeaked = $false
    $canaryValueOutsideExpectedFail = $false
    foreach ($f in $stagedFiles) {
        $c = Get-Content -Path $f.FullName -Raw -ErrorAction SilentlyContinue
        if ($null -eq $c) { continue }
        if ($c.Contains($remoteTokenValue)) { $remoteTokenLeaked = $true }
    }
    Assert-True 'remote-URL negative control: the embedded remote credential never appears in any staged file (remote_url is never captured)' (-not $remoteTokenLeaked)

    $identityPath = Join-Path $canaryOutDir 'git_identity.json'
    if (Test-Path -LiteralPath $identityPath) {
        $identityJson = Get-Content -Path $identityPath -Raw | ConvertFrom-Json
        Assert-True 'remote-URL negative control: git_identity.json has no remote_url property at all' `
            ($null -eq $identityJson.PSObject.Properties['remote_url'])
    }

    $manifestPathCanary = Join-Path $canaryOutDir 'manifest.json'
    if (Test-Path -LiteralPath $manifestPathCanary) {
        $manifestCanary = Get-Content -Path $manifestPathCanary -Raw | ConvertFrom-Json
        Assert-True 'content-canary negative control: manifest records secret_canary_scan as failed (not silently ok)' `
            ($manifestCanary.components.secret_canary_scan.status -eq 'failed')
    }
}

Remove-Item -Path $canaryRepoRoot -Recurse -Force -ErrorAction SilentlyContinue
Remove-Item -Path $canaryOutDir -Recurse -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------
# Section 8: disposable DB name SQL-identifier-safety negative control
# (D-R1 defect 5). Real invocation -- refused before any docker/SQL call.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 8: disposable DB name SQL-identifier safety (real) ==='

$sqlUnsafeBackupDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_recovery_sqlunsafe_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $sqlUnsafeBackupDir | Out-Null
Set-Content -Path (Join-Path $sqlUnsafeBackupDir 'manifest.json') -Value '{"files":[]}' -Encoding UTF8
Set-Content -Path (Join-Path $sqlUnsafeBackupDir 'paper_db.dump') -Value 'not a real dump' -Encoding UTF8

$sqlUnsafeExit1 = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $RestoreScript, '-BackupDir', $sqlUnsafeBackupDir, '-DisposableDbName', 'evil"; DROP TABLE foo; --')
Assert-True 'SQL-identifier negative control: a DisposableDbName with SQL-breaking characters is refused (nonzero exit)' ($sqlUnsafeExit1 -ne 0)

$sqlUnsafeExit2 = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $RestoreScript, '-BackupDir', $sqlUnsafeBackupDir, '-DisposableDbName', '1_starts_with_digit')
Assert-True 'SQL-identifier negative control: a DisposableDbName starting with a digit is refused (nonzero exit)' ($sqlUnsafeExit2 -ne 0)

Remove-Item -Path $sqlUnsafeBackupDir -Recurse -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------
# Section 9: canonical local Paper DB canary exemption (real invocations).
# The content-canary scan in Backup-MiniQuantDeskRecovery.ps1 must exempt
# ONLY the exact, fully structurally-verified canonical local Paper Postgres
# URL under the MQK_DATABASE_URL name -- never MQK_DATABASE_URL in general,
# never all loopback URLs, never all Postgres URLs, and never another
# variable name merely because its value happens to look the same.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 9: canonical local Paper DB canary exemption (real) ==='

$CanonicalLocalPaperDbUrlForTest = 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable'

# Builds a fresh, git-initialized fixture repo whose tracked ledger doc
# quotes $LedgerValue verbatim, with .env.local setting $EnvVarName=$EnvValue
# (properly gitignored/untracked, so only the canary logic itself is under
# test). Returns the repo root; caller is responsible for cleanup.
function New-CanaryFixtureRepo {
    param(
        [Parameter(Mandatory = $true)][string]$EnvVarName,
        [Parameter(Mandatory = $true)][string]$EnvValue,
        [Parameter(Mandatory = $true)][string]$LedgerValue
    )
    $repoRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_canary_fixture_" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $repoRoot | Out-Null
    Set-Content -Path (Join-Path $repoRoot '.gitignore') -Value '.env.local' -Encoding UTF8
    Set-Content -Path (Join-Path $repoRoot 'MiniQuantDesk_Master_Patch_Ledger_v2_updated.md') -Value "Fixture doc quoting: $LedgerValue" -Encoding UTF8
    & git -C $repoRoot init -q 2>&1 | Out-Null
    & git -C $repoRoot -c user.email='test@example.com' -c user.name='test' add .gitignore MiniQuantDesk_Master_Patch_Ledger_v2_updated.md 2>&1 | Out-Null
    & git -C $repoRoot -c user.email='test@example.com' -c user.name='test' commit -q -m 'fixture' 2>&1 | Out-Null
    Set-Content -Path (Join-Path $repoRoot '.env.local') -Value "$EnvVarName=$EnvValue" -Encoding UTF8
    return $repoRoot
}

function Invoke-CanaryFixtureBackup {
    param([Parameter(Mandatory = $true)][string]$RepoRootPath)
    $outDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_canary_fixture_out_" + [guid]::NewGuid().ToString('N'))
    $r = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $BackupScript, '-RepoRoot', $RepoRootPath, '-OutDir', $outDir)
    Remove-Item -Path $outDir -Recurse -Force -ErrorAction SilentlyContinue
    return $r
}

# Same as above but also captures stdout, for the one control (C13) that
# needs to inspect captured output rather than just the exit code.
function Invoke-CanaryFixtureBackupWithStdout {
    param([Parameter(Mandatory = $true)][string]$RepoRootPath)
    $outDir = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_canary_fixture_out_" + [guid]::NewGuid().ToString('N'))
    $stdoutFile = [System.IO.Path]::GetTempFileName()
    $stderrFile = [System.IO.Path]::GetTempFileName()
    try {
        $proc = Start-Process -FilePath 'powershell.exe' -NoNewWindow -Wait -PassThru `
            -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $BackupScript, '-RepoRoot', $RepoRootPath, '-OutDir', $outDir) `
            -RedirectStandardOutput $stdoutFile -RedirectStandardError $stderrFile
        $stdout = Get-Content -Path $stdoutFile -Raw -ErrorAction SilentlyContinue
        $stderr = Get-Content -Path $stderrFile -Raw -ErrorAction SilentlyContinue
        return [pscustomobject]@{ ExitCode = $proc.ExitCode; Stdout = $stdout; Stderr = $stderr }
    } finally {
        Remove-Item -Path $stdoutFile, $stderrFile -Force -ErrorAction SilentlyContinue
        Remove-Item -Path $outDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}

# C1: exact canonical value -- must NOT trigger the canary.
$c1Repo = New-CanaryFixtureRepo -EnvVarName 'MQK_DATABASE_URL' -EnvValue $CanonicalLocalPaperDbUrlForTest -LedgerValue $CanonicalLocalPaperDbUrlForTest
$c1Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c1Repo
Assert-True 'C1: exact canonical local Paper DB URL in tracked docs does NOT trigger the secret canary (backup succeeds)' ($c1Exit -eq 0)
Remove-Item -Path $c1Repo -Recurse -Force -ErrorAction SilentlyContinue

# C2: non-loopback host -- must trigger.
$c2Value = 'postgres://postgres:postgres@example.com:5440/miniquantdesk_paper?sslmode=disable'
$c2Repo = New-CanaryFixtureRepo -EnvVarName 'MQK_DATABASE_URL' -EnvValue $c2Value -LedgerValue $c2Value
$c2Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c2Repo
Assert-True 'C2: MQK_DATABASE_URL with a non-loopback host is refused (nonzero exit)' ($c2Exit -ne 0)
Remove-Item -Path $c2Repo -Recurse -Force -ErrorAction SilentlyContinue

# C3: loopback but different credential -- must trigger.
$c3Value = 'postgres://postgres:differentpw1@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable'
$c3Repo = New-CanaryFixtureRepo -EnvVarName 'MQK_DATABASE_URL' -EnvValue $c3Value -LedgerValue $c3Value
$c3Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c3Repo
Assert-True 'C3: MQK_DATABASE_URL on loopback with a different credential than canonical is refused (nonzero exit)' ($c3Exit -ne 0)
Remove-Item -Path $c3Repo -Recurse -Force -ErrorAction SilentlyContinue

# C4: wrong port -- must trigger.
$c4Value = 'postgres://postgres:postgres@127.0.0.1:5432/miniquantdesk_paper?sslmode=disable'
$c4Repo = New-CanaryFixtureRepo -EnvVarName 'MQK_DATABASE_URL' -EnvValue $c4Value -LedgerValue $c4Value
$c4Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c4Repo
Assert-True 'C4: MQK_DATABASE_URL on the wrong port is refused (nonzero exit)' ($c4Exit -ne 0)
Remove-Item -Path $c4Repo -Recurse -Force -ErrorAction SilentlyContinue

# C5: different database identity -- must trigger.
$c5Value = 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_other?sslmode=disable'
$c5Repo = New-CanaryFixtureRepo -EnvVarName 'MQK_DATABASE_URL' -EnvValue $c5Value -LedgerValue $c5Value
$c5Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c5Repo
Assert-True 'C5: MQK_DATABASE_URL targeting a different database identity is refused (nonzero exit)' ($c5Exit -ne 0)
Remove-Item -Path $c5Repo -Recurse -Force -ErrorAction SilentlyContinue

# C6: malformed URL -- must NOT receive the canonical exemption.
$c6Value = 'not-a-valid-postgres-url-but-long-enough'
$c6Repo = New-CanaryFixtureRepo -EnvVarName 'MQK_DATABASE_URL' -EnvValue $c6Value -LedgerValue $c6Value
$c6Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c6Repo
Assert-True 'C6: a malformed MQK_DATABASE_URL value is refused, never exempted (nonzero exit)' ($c6Exit -ne 0)
Remove-Item -Path $c6Repo -Recurse -Force -ErrorAction SilentlyContinue

# C7: B2_ACCOUNT_KEY collision -- must still fail closed (regression).
$c7Value = 'fixture-b2-account-key-not-real-12345'
$c7Repo = New-CanaryFixtureRepo -EnvVarName 'B2_ACCOUNT_KEY' -EnvValue $c7Value -LedgerValue $c7Value
$c7Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c7Repo
Assert-True 'C7: a B2_ACCOUNT_KEY collision in tracked content still fails closed (nonzero exit, regression)' ($c7Exit -ne 0)
Remove-Item -Path $c7Repo -Recurse -Force -ErrorAction SilentlyContinue

# C8: MQK_OPERATOR_TOKEN collision -- must still fail closed (regression).
$c8Value = 'fixture-operator-token-not-real-67890'
$c8Repo = New-CanaryFixtureRepo -EnvVarName 'MQK_OPERATOR_TOKEN' -EnvValue $c8Value -LedgerValue $c8Value
$c8Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c8Repo
Assert-True 'C8: an MQK_OPERATOR_TOKEN collision in tracked content still fails closed (nonzero exit, regression)' ($c8Exit -ne 0)
Remove-Item -Path $c8Repo -Recurse -Force -ErrorAction SilentlyContinue

# C9: another allowlisted canary (ALPACA_API_KEY) remains unchanged/fail closed.
$c9Value = 'fixture-alpaca-api-key-not-real-11223'
$c9Repo = New-CanaryFixtureRepo -EnvVarName 'ALPACA_API_KEY' -EnvValue $c9Value -LedgerValue $c9Value
$c9Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c9Repo
Assert-True 'C9: an ALPACA_API_KEY collision (a canary untouched by this patch) still fails closed (nonzero exit)' ($c9Exit -ne 0)
Remove-Item -Path $c9Repo -Recurse -Force -ErrorAction SilentlyContinue

# C10: the exemption cannot be obtained merely by a loopback-shaped value
# under a DIFFERENT allowlisted variable name -- gating is on the exact
# name (MQK_DATABASE_URL) AND the exact canonical value together, not on
# "looks like localhost" alone.
$c10Repo = New-CanaryFixtureRepo -EnvVarName 'MQK_OPERATOR_TOKEN' -EnvValue $CanonicalLocalPaperDbUrlForTest -LedgerValue $CanonicalLocalPaperDbUrlForTest
$c10Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c10Repo
Assert-True 'C10: the canonical DB URL string under a DIFFERENT variable name (MQK_OPERATOR_TOKEN) is NOT exempted (nonzero exit -- name+value must both match)' ($c10Exit -ne 0)
Remove-Item -Path $c10Repo -Recurse -Force -ErrorAction SilentlyContinue

# C12: MQK_RESTIC_PASSWORD_FILE collisions remain excluded/fail-closed
# (unaffected by this patch -- regression).
$c12Value = 'fixture-restic-password-file-path-not-real'
$c12Repo = New-CanaryFixtureRepo -EnvVarName 'MQK_RESTIC_PASSWORD_FILE' -EnvValue $c12Value -LedgerValue $c12Value
$c12Exit = Invoke-CanaryFixtureBackup -RepoRootPath $c12Repo
Assert-True 'C12: an MQK_RESTIC_PASSWORD_FILE value collision in tracked content still fails closed (nonzero exit, regression)' ($c12Exit -ne 0)
Remove-Item -Path $c12Repo -Recurse -Force -ErrorAction SilentlyContinue

# C11: .env.local never appears as a staged manifest entry (re-verified
# from Section 3's own real end-to-end manifest, already asserted upstream).
Assert-True 'C11: .env.local never appears as a staged manifest entry (re-verified from Section 3''s own manifest)' `
    (-not ($manifest.files | Where-Object { $_.path -match '(?i)\.env\.local' }))

# C13: no fixture secret-shaped value (including the B2_ACCOUNT_KEY-style
# one from C7) ever appears in captured stdout/stderr, even though the
# canary correctly refuses the backup because of it.
$c13Value = 'fixture-b2-account-key-stdout-check-99887'
$c13Repo = New-CanaryFixtureRepo -EnvVarName 'B2_ACCOUNT_KEY' -EnvValue $c13Value -LedgerValue $c13Value
$c13Result = Invoke-CanaryFixtureBackupWithStdout -RepoRootPath $c13Repo
Assert-True 'C13 setup: the B2_ACCOUNT_KEY collision fixture is refused (nonzero exit)' ($c13Result.ExitCode -ne 0)
Assert-True 'C13: the fixture secret value never appears in captured stdout' (-not ($c13Result.Stdout -and $c13Result.Stdout.Contains($c13Value)))
Assert-True 'C13: the fixture secret value never appears in captured stderr' (-not ($c13Result.Stderr -and $c13Result.Stderr.Contains($c13Value)))
Remove-Item -Path $c13Repo -Recurse -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Summary ==='
if ($Violations -eq 0) {
    Show-Green "All proofs held. 0 violations."
    exit 0
} else {
    Show-Red "$Violations violation(s) found."
    exit 1
}
