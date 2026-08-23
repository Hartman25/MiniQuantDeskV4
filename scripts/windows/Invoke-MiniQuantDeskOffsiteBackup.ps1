# =============================================================================
# OPS-OFFSITE-BACKUP-01 -- REAL B2 CLOSURE (D-R2)
# Invoke-MiniQuantDeskOffsiteBackup.ps1
#
# The real, encrypted, offsite half of the recovery-backup workflow:
#   1. stage a fresh authoritative recovery set (Backup-MiniQuantDeskRecovery.ps1)
#   2. restic init the Backblaze B2 repository (S3-compatible API) only if not
#      already initialized
#   3. restic backup that staged set as one encrypted snapshot
#   4. restic snapshots / restic check confirm it
#   5. a NON-DESTRUCTIVE retention dry-run (never --prune, never real forget)
#   6. restic restore that exact snapshot into a disposable local directory
#   7. reuse Restore-MiniQuantDeskRecovery.ps1's already-hardened full-manifest
#      verification + fail-closed pg_restore + disposable-DB proof against the
#      RESTIC-RESTORED content (not the original staging dir) -- this is what
#      actually proves the round trip through B2, not just local staging.
#   8. an independent secret-canary re-scan of the restic-restored material.
#
# THIS SCRIPT NEVER:
#   - installs restic (fails closed with RESTIC_INSTALL_REQUIRED instead);
#   - guesses a B2 bucket/endpoint/region -- all three must come from an
#     explicit parameter or a narrowly-read local config value;
#   - generates or prints a restic repository password (fails closed with
#     OPERATOR_SECRET_SETUP_REQUIRED and prints a SAFE command the operator
#     can run themselves instead);
#   - loads/exports the whole .env.local into restic's process environment --
#     only AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY / AWS_DEFAULT_REGION /
#     RESTIC_REPOSITORY / RESTIC_PASSWORD_FILE are ever set, narrowly read
#     from allowlisted local variable names, and only for the duration of
#     each restic child-process call (cleared again in a finally block);
#   - prints a B2 key ID/secret/restic password value;
#   - runs `restic forget` without --dry-run, or `restic prune`;
#   - deletes the only/any existing snapshot;
#   - touches Live, a broker, or the real Paper DB beyond the same read-only
#     pg_dump Backup-MiniQuantDeskRecovery.ps1 already performs;
#   - emits REAL_B2_PROOF=PASS for any repository identity that is not
#     provably an S3-compatible *.backblazeb2.com URL bound to the frozen
#     bucket 'miniquantdesk-recovery-8f42c1' -- any other repository
#     (including a local/opaque MQK_RESTIC_REPOSITORY override) can only
#     ever produce LOCAL_RESTIC_ORCHESTRATION_PASS;
#   - accepts a restic repository password file that is relative, missing,
#     or resolves inside the primary repo/this repair worktree/any other
#     linked Git worktree;
#   - reports success (of either kind) before its own scratch-directory
#     cleanup is proven to have actually removed what it created.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-MiniQuantDeskOffsiteBackup.ps1 `
#       -B2Bucket <bucket> -B2Endpoint <s3.region.backblazeb2.com> -B2Region <region>
#
# Exit codes: 0 = full real B2 snapshot + restore + verification proof
# passed, 1 = any prerequisite/step failed (including an honest
# RESTIC_INSTALL_REQUIRED / B2_TRANSPORT_CONFIG_REQUIRED /
# OPERATOR_SECRET_SETUP_REQUIRED / B2_APPLICATION_KEY_REQUIRED checkpoint --
# these are truthful refusals, not crashes).
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [string] $RepoRoot              = '',
    [string] $B2Bucket              = '',
    [string] $B2Endpoint            = '',
    [string] $B2Region              = '',
    [string] $RepositoryPrefix      = 'miniquantdesk-recovery',
    [string] $DisposableDbContainer = 'mqk-test-postgres',
    [ValidateRange(1, [int]::MaxValue)][int] $ResticTimeoutSeconds = 600,

    # D-R2-R2-03 test-only mutation seam: when supplied, this exact path is
    # used as the staging scratch directory instead of a fresh GUID-named
    # one, so a test can pre-create it with a locked file inside to force a
    # real, deterministic Windows cleanup failure (no production behavior
    # change -- empty by default, and never documented as a real usage flag).
    [string] $StagingDirOverrideForCleanupTest = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# D-R2-R2-01: the only bucket a REAL_B2_PROOF=PASS claim may be bound to.
$RequiredB2Bucket = 'miniquantdesk-recovery-8f42c1'

function Write-Step { param([string]$M) Write-Host "[OFFSITE-B2] $M"       -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[OFFSITE-B2] OK: $M"   -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[OFFSITE-B2] WARN: $M" -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[OFFSITE-B2] FAIL: $M" -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')

$BackupScript  = Join-Path $PSScriptRoot 'Backup-MiniQuantDeskRecovery.ps1'
$RestoreScript = Join-Path $PSScriptRoot 'Restore-MiniQuantDeskRecovery.ps1'
foreach ($p in @($BackupScript, $RestoreScript)) {
    if (-not (Test-Path -LiteralPath $p)) {
        Write-Fail "Required script not found: $p"
        exit 1
    }
}

# ---------------------------------------------------------------------------
# Narrow local-config reader: reads ONE allowlisted variable name out of the
# same candidate .env.local/.env files the official launcher uses, WITHOUT
# ever calling Set-Item Env: and WITHOUT loading any other name. Mirrors
# Backup-MiniQuantDeskRecovery.ps1's Get-AllowlistedLocalSecretValues parsing
# rules (quoted-value handling), but returns a single named value instead of
# a scan set.
# ---------------------------------------------------------------------------
function Get-AllowlistedLocalConfigValue {
    param([Parameter(Mandatory = $true)][string]$RepoRoot, [Parameter(Mandatory = $true)][string]$Name)
    $candidates = @(
        (Join-Path $RepoRoot '.env.local'),
        (Join-Path $RepoRoot '.env'),
        (Join-Path $RepoRoot 'core-rs\.env.local'),
        (Join-Path $RepoRoot 'core-rs\.env')
    )
    foreach ($candidate in $candidates) {
        if (-not (Test-Path -LiteralPath $candidate)) { continue }
        foreach ($line in Get-Content -Path $candidate) {
            if ([string]::IsNullOrWhiteSpace($line)) { continue }
            $trimmed = $line.Trim()
            if ($trimmed.StartsWith('#')) { continue }
            $idx = $trimmed.IndexOf('=')
            if ($idx -lt 1) { continue }
            $entryName = $trimmed.Substring(0, $idx).Trim()
            if ($entryName -ne $Name) { continue }
            $value = $trimmed.Substring($idx + 1).Trim()
            if (($value.StartsWith('"') -and $value.EndsWith('"')) -or ($value.StartsWith("'") -and $value.EndsWith("'"))) {
                if ($value.Length -ge 2) { $value = $value.Substring(1, $value.Length - 2) }
            }
            if ($value.Length -gt 0) { return $value }
        }
    }
    return $null
}

# ---------------------------------------------------------------------------
# Bounded restic invocation: same async-capture pattern as
# Invoke-BoundedChildScript (Start-MiniQuantDesk.ps1) / Invoke-PaperLauncherProcess
# (Watch-MiniQuantDeskPaperHealth.ps1) -- WaitForExit(timeout) alone governs
# the child's wall-clock lifetime, never a blocking ReadToEnd beforehand.
# $ResticEnv supplies ONLY the narrow set of variables this call needs; nothing
# else from this process's own environment is added on top of the normal
# Windows process-environment inheritance baseline.
# ---------------------------------------------------------------------------
function Invoke-ResticCommand {
    param(
        [Parameter(Mandatory = $true)][string]$ResticPath,
        [Parameter(Mandatory = $true)][string[]]$ResticArgs,
        [Parameter(Mandatory = $true)][hashtable]$ResticEnv,
        [Parameter(Mandatory = $true)][int]$TimeoutSeconds,
        [string]$WorkingDirectory = ''
    )
    $quotedArgs = $ResticArgs | ForEach-Object { '"' + ($_ -replace '"', '""') + '"' }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $ResticPath
    $psi.Arguments = ($quotedArgs -join ' ')
    if (-not [string]::IsNullOrWhiteSpace($WorkingDirectory)) { $psi.WorkingDirectory = $WorkingDirectory }
    $psi.UseShellExecute = $false
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.CreateNoWindow = $true
    foreach ($key in $ResticEnv.Keys) {
        $psi.EnvironmentVariables[$key] = $ResticEnv[$key]
    }

    $process = New-Object System.Diagnostics.Process
    $process.StartInfo = $psi

    $stdoutBuilder = New-Object System.Text.StringBuilder
    $stderrBuilder = New-Object System.Text.StringBuilder
    Register-ObjectEvent -InputObject $process -EventName OutputDataReceived -Action { if ($null -ne $EventArgs.Data) { $Event.MessageData.AppendLine($EventArgs.Data) | Out-Null } } -MessageData $stdoutBuilder | Out-Null
    Register-ObjectEvent -InputObject $process -EventName ErrorDataReceived -Action { if ($null -ne $EventArgs.Data) { $Event.MessageData.AppendLine($EventArgs.Data) | Out-Null } } -MessageData $stderrBuilder | Out-Null

    try {
        [void]$process.Start()
        $process.BeginOutputReadLine()
        $process.BeginErrorReadLine()
        $finished = $process.WaitForExit($TimeoutSeconds * 1000)
        if (-not $finished) {
            try { $process.Kill(); $process.WaitForExit(5000) | Out-Null } catch {}
            return [pscustomobject]@{ ExitCode = 1; TimedOut = $true; Stdout = $stdoutBuilder.ToString(); Stderr = $stderrBuilder.ToString() }
        }
        # A fast-completing child (restic on a small repo can exit in well
        # under a second) can make the timed WaitForExit(ms) return true
        # before the async OutputDataReceived/ErrorDataReceived callbacks
        # have finished flushing the last lines into the StringBuilders --
        # a documented .NET race. The parameterless WaitForExit() below is
        # the documented fix: called after the process has already exited,
        # it blocks only long enough to guarantee the redirected streams
        # are fully drained before they're read.
        $process.WaitForExit() | Out-Null
        return [pscustomobject]@{ ExitCode = $process.ExitCode; TimedOut = $false; Stdout = $stdoutBuilder.ToString(); Stderr = $stderrBuilder.ToString() }
    } finally {
        Get-EventSubscriber | Where-Object { $_.SourceObject -eq $process } | Unregister-Event
    }
}

# =============================================================================
# 1. restic installed?
# =============================================================================
Write-Sect 'restic prerequisite'
$resticCmd = Get-Command 'restic' -ErrorAction SilentlyContinue
if (-not $resticCmd) {
    Write-Fail 'RESTIC_INSTALL_REQUIRED: restic is not installed on this machine. This script does not install it silently -- install restic, then re-run.'
    exit 1
}
$resticVersionInfo = (& $resticCmd.Source version 2>&1) -join ' '
Write-Ok "restic found: $resticVersionInfo"

# =============================================================================
# 2. Non-secret B2 transport config -- never guessed.
# =============================================================================
Write-Sect 'B2 transport configuration'
$repositoryOverride = Get-AllowlistedLocalConfigValue -RepoRoot $RepoRoot -Name 'MQK_RESTIC_REPOSITORY'

# D-R2-R2-01: REAL_B2_PROOF=PASS may only ever be emitted for a repository
# identity that is provably S3-compatible, provably a *.backblazeb2.com
# endpoint, AND provably bound to the one frozen intended bucket. Every
# other repository identity (including an arbitrary local/opaque
# MQK_RESTIC_REPOSITORY override) is LOCAL/TEST ORCHESTRATION ONLY and must
# never be reported as real-B2 authority.
$isRealB2Mode = $false

if ([string]::IsNullOrWhiteSpace($repositoryOverride)) {
    if ([string]::IsNullOrWhiteSpace($B2Bucket))   { $B2Bucket   = Get-AllowlistedLocalConfigValue -RepoRoot $RepoRoot -Name 'MQK_B2_BUCKET' }
    if ([string]::IsNullOrWhiteSpace($B2Endpoint)) { $B2Endpoint = Get-AllowlistedLocalConfigValue -RepoRoot $RepoRoot -Name 'MQK_B2_ENDPOINT' }
    if ([string]::IsNullOrWhiteSpace($B2Region))   { $B2Region   = Get-AllowlistedLocalConfigValue -RepoRoot $RepoRoot -Name 'MQK_B2_REGION' }

    if ([string]::IsNullOrWhiteSpace($B2Bucket) -or [string]::IsNullOrWhiteSpace($B2Endpoint) -or [string]::IsNullOrWhiteSpace($B2Region)) {
        Write-Fail 'B2_TRANSPORT_CONFIG_REQUIRED: bucket/endpoint/region are not fully known and this script never guesses them.'
        Write-Fail 'Supply all three via -B2Bucket/-B2Endpoint/-B2Region, or set MQK_B2_BUCKET/MQK_B2_ENDPOINT/MQK_B2_REGION in .env.local (non-secret values only), or set a fully-formed MQK_RESTIC_REPOSITORY to skip this composition entirely.'
        Write-Fail 'These three values are visible on the bucket''s own settings/details page in the Backblaze B2 dashboard -- never a secret.'
        exit 1
    }
    if ($B2Bucket -ne $RequiredB2Bucket) {
        Write-Fail "B2_BUCKET_MISMATCH: bucket '$B2Bucket' does not match the frozen intended bucket '$RequiredB2Bucket'. A REAL_B2_PROOF=PASS claim requires exactly this bucket."
        exit 1
    }
    if ($B2Endpoint -notmatch '(?i)\.backblazeb2\.com$') {
        Write-Fail "B2_TRANSPORT_CONFIG_REQUIRED: endpoint '$B2Endpoint' does not look like a Backblaze B2 S3-compatible endpoint (expected a host ending in .backblazeb2.com)."
        exit 1
    }
    $resticRepository = "s3:https://$B2Endpoint/$B2Bucket/$RepositoryPrefix"
    $isRealB2Mode = $true
    Write-Ok "B2 transport resolved: bucket=$B2Bucket endpoint=$B2Endpoint region=$B2Region prefix=$RepositoryPrefix"
} else {
    # An override CAN still qualify for real-B2 authority, but only if it is
    # itself provably an s3: URL against a *.backblazeb2.com host bound to
    # the frozen bucket -- an opaque/local/file repository never matches
    # this pattern and therefore can never claim REAL_B2_PROOF=PASS.
    $resticRepository = $repositoryOverride
    if ($repositoryOverride -match '(?i)^s3:https?://([^/]+)/([^/]+)(/.*)?$') {
        $overrideHost = $Matches[1]
        $overrideBucket = $Matches[2]
        if ($overrideHost -match '(?i)\.backblazeb2\.com$' -and $overrideBucket -eq $RequiredB2Bucket) {
            $isRealB2Mode = $true
        }
    }
    if ($isRealB2Mode) {
        Write-Ok "Using explicit MQK_RESTIC_REPOSITORY override -- validated as an S3-compatible Backblaze B2 URL bound to bucket '$RequiredB2Bucket'; real-B2 authority preserved."
    } else {
        Write-Warn "MQK_RESTIC_REPOSITORY override does not satisfy the S3 + *.backblazeb2.com + bucket='$RequiredB2Bucket' identity check -- this run is LOCAL/TEST ORCHESTRATION ONLY and cannot emit REAL_B2_PROOF=PASS."
    }
}

# =============================================================================
# 3. restic repository password -- never generated or printed here.
#    D-R2-R2-02: location authority is more than "the path exists" -- it must
#    be an absolute path to a regular file that lives entirely outside every
#    Git worktree this workflow touches, so it can never be swept up by the
#    backup staging step (which only ever reads from inside $RepoRoot).
# =============================================================================
function Get-GitWorktreeRoots {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)
    $porcelain = $null
    try { $porcelain = & git -C $RepoRoot worktree list --porcelain 2>$null } catch { $porcelain = $null }
    if ($LASTEXITCODE -ne 0 -or $null -eq $porcelain) { return $null }
    $roots = New-Object 'System.Collections.Generic.List[string]'
    foreach ($line in $porcelain) {
        if ($line -match '^worktree\s+(.+)$') {
            try {
                $resolved = (Resolve-Path -LiteralPath $Matches[1] -ErrorAction Stop).Path.TrimEnd('\')
                $roots.Add($resolved)
            } catch {}
        }
    }
    return @($roots)
}

function Test-ResticPasswordFileAuthority {
    param(
        [string]$PasswordFilePath,
        [string[]]$WorktreeRoots
    )
    $r = [ordered]@{ Present = $false; Absolute = $false; IsLeaf = $false; OutsideWorktrees = $false }
    if ([string]::IsNullOrWhiteSpace($PasswordFilePath)) { return [pscustomobject]$r }
    $r.Present = $true
    # Reject relative paths BEFORE any resolution -- Resolve-Path would
    # silently anchor a relative path to the current directory, defeating
    # the "must be absolute" requirement.
    if (-not [System.IO.Path]::IsPathRooted($PasswordFilePath)) { return [pscustomobject]$r }
    $r.Absolute = $true
    $resolved = $null
    try { $resolved = (Resolve-Path -LiteralPath $PasswordFilePath -ErrorAction Stop).Path } catch {}
    if ([string]::IsNullOrWhiteSpace($resolved) -or -not (Test-Path -LiteralPath $resolved -PathType Leaf)) { return [pscustomobject]$r }
    $r.IsLeaf = $true
    $resolvedTrimmed = $resolved.TrimEnd('\')
    $insideAny = $false
    foreach ($root in $WorktreeRoots) {
        if ($resolvedTrimmed.Equals($root, [System.StringComparison]::OrdinalIgnoreCase) -or
            $resolvedTrimmed.StartsWith(($root + '\'), [System.StringComparison]::OrdinalIgnoreCase)) {
            $insideAny = $true
            break
        }
    }
    $r.OutsideWorktrees = -not $insideAny
    return [pscustomobject]$r
}

Write-Sect 'restic repository password'
$passwordFilePath = Get-AllowlistedLocalConfigValue -RepoRoot $RepoRoot -Name 'MQK_RESTIC_PASSWORD_FILE'
if ([string]::IsNullOrWhiteSpace($passwordFilePath)) {
    $passwordFilePath = Get-AllowlistedLocalConfigValue -RepoRoot $RepoRoot -Name 'RESTIC_PASSWORD_FILE'
}

$OperatorSecretSetupHelp = {
    Write-Fail 'This script will not generate or print a password. Run this yourself, OUTSIDE the repo, then set MQK_RESTIC_PASSWORD_FILE (or RESTIC_PASSWORD_FILE) in .env.local to the resulting path:'
    Write-Host ''
    Write-Host '  $dir = Join-Path $env:USERPROFILE ''.miniquantdesk''' -ForegroundColor Yellow
    Write-Host '  New-Item -ItemType Directory -Force -Path $dir | Out-Null' -ForegroundColor Yellow
    Write-Host '  $bytes = New-Object byte[] 48' -ForegroundColor Yellow
    Write-Host '  [System.Security.Cryptography.RandomNumberGenerator]::Fill($bytes)' -ForegroundColor Yellow
    Write-Host '  [Convert]::ToBase64String($bytes) | Set-Content -NoNewline -Path (Join-Path $dir ''restic-password'')' -ForegroundColor Yellow
    Write-Host ''
    Write-Fail 'Losing this password file makes the entire repository unrecoverable -- back it up separately, outside Git.'
}

$worktreeRoots = Get-GitWorktreeRoots -RepoRoot $RepoRoot
if ($null -eq $worktreeRoots) {
    Write-Fail 'WORKTREE_AUTHORITY_UNAVAILABLE: unable to enumerate Git worktree roots via "git -C <repo> worktree list" -- refusing to trust an unproven password-file-location authority.'
    exit 1
}

$pwCheck = Test-ResticPasswordFileAuthority -PasswordFilePath $passwordFilePath -WorktreeRoots $worktreeRoots
if (-not ($pwCheck.Present -and $pwCheck.Absolute -and $pwCheck.IsLeaf)) {
    Write-Fail 'OPERATOR_SECRET_SETUP_REQUIRED: no existing, absolute-path restic repository password file was found.'
    & $OperatorSecretSetupHelp
    exit 1
}
if (-not $pwCheck.OutsideWorktrees) {
    Write-Fail 'OPERATOR_SECRET_SETUP_REQUIRED: the configured restic repository password file resolves inside a Git worktree root used by this workflow (primary repo, this repair worktree, or another linked worktree). It must live entirely outside all repo/worktree authority.'
    & $OperatorSecretSetupHelp
    exit 1
}
Write-Ok 'RESTIC_PASSWORD_FILE_PRESENT=YES'
Write-Ok 'RESTIC_PASSWORD_FILE_ABSOLUTE=YES'
Write-Ok 'RESTIC_PASSWORD_FILE_OUTSIDE_WORKTREES=YES'

# =============================================================================
# 4. B2 application key -- maps to AWS_ACCESS_KEY_ID / AWS_SECRET_ACCESS_KEY.
#    Read narrowly; never printed; never exported beyond the restic child call.
# =============================================================================
Write-Sect 'B2 application key'
$b2AccountId  = Get-AllowlistedLocalConfigValue -RepoRoot $RepoRoot -Name 'B2_ACCOUNT_ID'
$b2AccountKey = Get-AllowlistedLocalConfigValue -RepoRoot $RepoRoot -Name 'B2_ACCOUNT_KEY'
if ([string]::IsNullOrWhiteSpace($b2AccountId) -or [string]::IsNullOrWhiteSpace($b2AccountKey)) {
    Write-Fail 'B2_APPLICATION_KEY_REQUIRED: B2_ACCOUNT_ID / B2_ACCOUNT_KEY are not both present in .env.local. This script never invents them.'
    exit 1
}
Write-Ok 'B2 application key ID/secret resolved (values never printed).'

$ResticEnv = @{
    AWS_ACCESS_KEY_ID     = $b2AccountId
    AWS_SECRET_ACCESS_KEY = $b2AccountKey
    AWS_DEFAULT_REGION    = $B2Region
    RESTIC_REPOSITORY     = $resticRepository
    RESTIC_PASSWORD_FILE  = $passwordFilePath
}

# =============================================================================
# 5. Stage a fresh authoritative recovery set (reuses D-R1's hardened
#    Backup-MiniQuantDeskRecovery.ps1 -- secret-store-exclusion proof,
#    content-canary scan, git identity, Paper DB dump, safe config).
# =============================================================================
# From here on, both scratch directories below are cleaned up in a single
# `finally` block regardless of which step fails -- `exit` inside a
# try/finally still runs the finally block in PowerShell, so every failure
# path (including operator-checkpoint-style ones added later) is covered by
# one cleanup site instead of a Remove-Item repeated at each failure branch.
$stagingDir = $null
$restoreTargetDir = $null
$cleanupSucceeded = $true
$cleanupErrors = New-Object 'System.Collections.Generic.List[string]'
try {

Write-Sect 'Stage fresh recovery set'
$stagingDir = if (-not [string]::IsNullOrWhiteSpace($StagingDirOverrideForCleanupTest)) { $StagingDirOverrideForCleanupTest } else { Join-Path $env:TEMP ("mqk_offsite_stage_" + [guid]::NewGuid().ToString('N')) }
& powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $BackupScript -RepoRoot $RepoRoot -OutDir $stagingDir | Out-Host
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Backup-MiniQuantDeskRecovery.ps1 failed to stage a clean recovery set (exit $LASTEXITCODE) -- refusing to snapshot an unproven set."
    exit 1
}
Write-Ok "Recovery set staged: $stagingDir"

# =============================================================================
# 6. restic init only if not already initialized.
# =============================================================================
Write-Sect 'restic repository init (idempotent)'
$catConfig = Invoke-ResticCommand -ResticPath $resticCmd.Source -ResticArgs @('cat', 'config') -ResticEnv $ResticEnv -TimeoutSeconds 60
if ($catConfig.ExitCode -eq 0) {
    Write-Ok 'Repository already initialized (restic cat config succeeded).'
} else {
    Write-Step 'Repository not yet initialized -- running restic init.'
    $initResult = Invoke-ResticCommand -ResticPath $resticCmd.Source -ResticArgs @('init') -ResticEnv $ResticEnv -TimeoutSeconds 60
    if ($initResult.ExitCode -ne 0) {
        Write-Fail "restic init failed (exit $($initResult.ExitCode)): $($initResult.Stderr.Trim())"
        exit 1
    }
    Write-Ok 'Repository initialized.'
}

# =============================================================================
# 7. restic backup -> real encrypted snapshot; capture snapshot ID only.
# =============================================================================
Write-Sect 'restic backup (real encrypted snapshot)'
# Backing up a relative '.' path with WorkingDirectory=$stagingDir (rather
# than passing the absolute $stagingDir path from outside it) avoids a real,
# reproduced restic-on-Windows restore defect: when a snapshot is recorded
# from an absolute path, `restic restore` recreates that FULL ancestor
# directory chain (e.g. <target>\C\Users\...) under the restore target and
# then tries to set each ancestor's original timestamp -- which fails with
# "Access is denied" on some reconstructed segments and makes the whole
# restore exit nonzero even though the actual backed-up files restore fine.
$backupResult = Invoke-ResticCommand -ResticPath $resticCmd.Source -ResticArgs @('backup', '.', '--tag', 'mqk-recovery', '--json') -ResticEnv $ResticEnv -TimeoutSeconds $ResticTimeoutSeconds -WorkingDirectory $stagingDir
if ($backupResult.ExitCode -ne 0) {
    Write-Fail "restic backup failed (exit $($backupResult.ExitCode)): $($backupResult.Stderr.Trim())"
    exit 1
}
$snapshotId = $null
foreach ($line in ($backupResult.Stdout -split "`n")) {
    if ([string]::IsNullOrWhiteSpace($line)) { continue }
    try {
        $obj = $line | ConvertFrom-Json
        if ($obj.message_type -eq 'summary' -and $obj.PSObject.Properties['snapshot_id']) {
            $snapshotId = $obj.snapshot_id
        }
    } catch {}
}
if ([string]::IsNullOrWhiteSpace($snapshotId)) {
    Write-Fail 'restic backup exited 0 but no snapshot_id was found in its --json summary output -- refusing to trust this as a completed snapshot.'
    exit 1
}
Write-Ok "Real encrypted snapshot created: $snapshotId"

# =============================================================================
# 8. restic snapshots confirms it.
# =============================================================================
Write-Sect 'restic snapshots (confirmation)'
$snapshotsResult = Invoke-ResticCommand -ResticPath $resticCmd.Source -ResticArgs @('snapshots', '--json') -ResticEnv $ResticEnv -TimeoutSeconds 60
if ($snapshotsResult.ExitCode -ne 0) {
    Write-Fail "restic snapshots failed (exit $($snapshotsResult.ExitCode)): $($snapshotsResult.Stderr.Trim())"
    exit 1
}
$snapshotList = @($snapshotsResult.Stdout | ConvertFrom-Json)
$foundSnapshot = $snapshotList | Where-Object { $_.id -eq $snapshotId } | Select-Object -First 1
if ($null -eq $foundSnapshot) {
    Write-Fail "Snapshot $snapshotId does not appear in restic snapshots -- refusing to trust it."
    exit 1
}
Write-Ok "restic snapshots confirms $snapshotId is present ($($snapshotList.Count) total snapshot(s) in repository)."

# =============================================================================
# 9. restic check.
# =============================================================================
Write-Sect 'restic check'
$checkResult = Invoke-ResticCommand -ResticPath $resticCmd.Source -ResticArgs @('check') -ResticEnv $ResticEnv -TimeoutSeconds $ResticTimeoutSeconds
if ($checkResult.ExitCode -ne 0) {
    Write-Fail "restic check failed (exit $($checkResult.ExitCode)): $($checkResult.Stderr.Trim())"
    exit 1
}
Write-Ok 'restic check passed.'

# =============================================================================
# 10. Retention policy -- NON-DESTRUCTIVE dry-run only. Never --prune.
# =============================================================================
Write-Sect 'Retention policy (dry-run only, never destructive)'
$forgetResult = Invoke-ResticCommand -ResticPath $resticCmd.Source -ResticArgs @('forget', '--keep-daily', '14', '--keep-weekly', '8', '--dry-run') -ResticEnv $ResticEnv -TimeoutSeconds 60
if ($forgetResult.ExitCode -ne 0) {
    Write-Fail "restic forget --dry-run failed (exit $($forgetResult.ExitCode)): $($forgetResult.Stderr.Trim())"
    exit 1
}
Write-Ok 'Retention dry-run (--keep-daily 14 --keep-weekly 8) validated, nothing deleted.'

# =============================================================================
# 11. restic restore that EXACT snapshot into a disposable local directory.
# =============================================================================
Write-Sect 'restic restore (disposable local target)'
$restoreTargetDir = Join-Path $env:TEMP ("mqk_offsite_restore_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $restoreTargetDir | Out-Null
$resticRestoreResult = Invoke-ResticCommand -ResticPath $resticCmd.Source -ResticArgs @('restore', $snapshotId, '--target', $restoreTargetDir) -ResticEnv $ResticEnv -TimeoutSeconds $ResticTimeoutSeconds
if ($resticRestoreResult.ExitCode -ne 0) {
    Write-Fail "restic restore failed (exit $($resticRestoreResult.ExitCode)): $($resticRestoreResult.Stderr.Trim())"
    exit 1
}

# restic preserves the original absolute source path structure under the
# restore target (e.g. <target>\C\Users\...\<stagingDir-leaf>\...) -- locate
# the restored manifest.json rather than assuming a fixed relative depth.
$restoredManifest = Get-ChildItem -Path $restoreTargetDir -Filter 'manifest.json' -Recurse -File | Select-Object -First 1
if ($null -eq $restoredManifest) {
    Write-Fail 'restic restore succeeded but no manifest.json was found anywhere under the restored tree.'
    exit 1
}
$restoredBackupDir = $restoredManifest.DirectoryName
Write-Ok "restic restore succeeded; restored recovery set root: $restoredBackupDir"

# =============================================================================
# 12. Reuse D-R1's hardened Restore-MiniQuantDeskRecovery.ps1 against the
#     RESTIC-RESTORED content: full manifest verification, fail-closed
#     pg_restore, post-restore table proof, disposable-DB cleanup proof.
# =============================================================================
Write-Sect 'Restore + verify the restic-restored recovery set'
& powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $RestoreScript -BackupDir $restoredBackupDir -DisposableDbContainer $DisposableDbContainer | Out-Host
$dbRestoreExit = $LASTEXITCODE
if ($dbRestoreExit -ne 0) {
    Write-Fail "Restore-MiniQuantDeskRecovery.ps1 against the restic-restored set failed (exit $dbRestoreExit)."
    exit 1
}
Write-Ok 'Full manifest verification + disposable-DB restore proof passed against the restic-restored material.'

# =============================================================================
# 13. Independent secret-canary re-scan of the restic-restored material
#     (defense in depth -- proves the round trip through B2 did not
#     introduce/preserve anything the pre-stage scan should have caught).
# =============================================================================
Write-Sect 'Secret-canary re-scan (restic-restored material)'
$allowlistedNames = @(
    'MQK_OPERATOR_TOKEN', 'MQK_DATABASE_URL', 'MQK_RESTIC_REPOSITORY', 'MQK_RESTIC_PASSWORD_FILE',
    'ALPACA_API_KEY_ID', 'ALPACA_API_SECRET_KEY', 'ALPACA_API_KEY', 'ALPACA_API_SECRET',
    'B2_ACCOUNT_ID', 'B2_ACCOUNT_KEY', 'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY'
)
$canaryValuesRescan = New-Object 'System.Collections.Generic.HashSet[string]'
foreach ($name in $allowlistedNames) {
    $v = Get-AllowlistedLocalConfigValue -RepoRoot $RepoRoot -Name $name
    if (-not [string]::IsNullOrWhiteSpace($v) -and $v.Length -ge 8) { [void]$canaryValuesRescan.Add($v) }
}
$canaryValuesRescan = @($canaryValuesRescan)
$canaryRescanHit = $false
if ($canaryValuesRescan.Count -gt 0) {
    $restoredTextFiles = Get-ChildItem -Path $restoreTargetDir -Recurse -File | Where-Object { @('.dump', '.sqlite') -notcontains $_.Extension }
    foreach ($f in $restoredTextFiles) {
        $content = Get-Content -Path $f.FullName -Raw -ErrorAction SilentlyContinue
        if ([string]::IsNullOrEmpty($content)) { continue }
        foreach ($val in $canaryValuesRescan) {
            if ($content.Contains($val)) {
                Write-Fail "SECRET CANARY MATCH in restic-restored file (value withheld from log): $($f.FullName)"
                $canaryRescanHit = $true
            }
        }
    }
}
if ($canaryRescanHit) {
    exit 1
}
Write-Ok "No allowlisted local secret value found in the restic-restored material ($($canaryValuesRescan.Count) value(s) checked)."

# D-R2-R2-03: the workflow body falls through here on success WITHOUT
# exiting -- cleanup authority (below, in `finally`) must run and be proven
# successful before any success status is printed or the process exits 0.
# Every failure branch above still exits 1 directly and immediately, which
# is already correct: a failed run must never print a success token
# regardless of what cleanup does afterward.

} finally {
    # Runs on every exit path above (success fallthrough or an `exit 1`
    # failure branch) -- the disposable DB itself is already cleaned up by
    # Restore-MiniQuantDeskRecovery.ps1 (D-R1's own cleanup-failure-fails-
    # the-proof guarantee). This only removes the two local scratch
    # directories this script created, and -- unlike the prior
    # SilentlyContinue version -- a failure here is recorded rather than
    # swallowed, because a PASS claim over a proof that fails to clean up
    # after itself is not proof-grade.
    foreach ($dir in @($stagingDir, $restoreTargetDir)) {
        if ($null -eq $dir -or -not (Test-Path -LiteralPath $dir)) { continue }
        try {
            Remove-Item -Path $dir -Recurse -Force -ErrorAction Stop
        } catch {
            $cleanupSucceeded = $false
            $cleanupErrors.Add("Failed to remove scratch directory '$dir': $($_.Exception.Message)")
            continue
        }
        if (Test-Path -LiteralPath $dir) {
            $cleanupSucceeded = $false
            $cleanupErrors.Add("Scratch directory '$dir' still exists after Remove-Item reported success.")
        }
    }
}

# Only reached on the success fallthrough path above (every failure branch
# already called `exit 1` from inside the try block, which takes effect
# once `finally` completes -- PowerShell does not resume script execution
# after that point). Cleanup authority gates the final result: a workflow
# success with a failed cleanup is still an overall failure, and
# REAL_B2_PROOF=PASS / LOCAL_RESTIC_ORCHESTRATION_PASS may only be printed
# after cleanup is proven to have held.
foreach ($msg in $cleanupErrors) { Write-Fail $msg }
if (-not $cleanupSucceeded) {
    Write-Fail 'Scratch-directory cleanup failed -- this is not an acceptable recovery proof regardless of workflow success.'
    exit 1
}

Write-Sect 'Summary'
if ($isRealB2Mode) {
    Write-Ok 'OPS_OFFSITE_BACKUP_CODE_AND_REAL_B2_PROOF=PASS'
    Write-Ok 'REAL_B2_PROOF=PASS'
} else {
    Write-Ok 'LOCAL_RESTIC_ORCHESTRATION_PASS'
}
Write-Ok "snapshot_id=$snapshotId"
exit 0
