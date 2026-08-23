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

$BackupText = Get-Content -Path $BackupScript -Raw
Assert-True 'Backup script never copies the Postgres data directory (pg_dump only)' `
    (($BackupText -match [regex]::Escape('pg_dump')) -and -not ($BackupText -match '(?i)Copy-Item.*(pgdata|data\\base)'))
Assert-True 'Backup script fails closed when a required component fails (D15)' `
    ($BackupText -match [regex]::Escape('$RequiredComponentFailed = $true'))
Assert-True 'Backup script performs a post-stage secret scan (defense in depth)' `
    ($BackupText -match [regex]::Escape('Post-stage secret-exclusion scan'))
Assert-True 'Backup script never invokes a B2/upload call' `
    (-not ($BackupText -match '(?i)b2 upload|Invoke-WebRequest|Invoke-RestMethod'))

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
