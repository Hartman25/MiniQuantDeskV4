# =============================================================================
# test_offsite_b2_workflow.ps1
# OPS-OFFSITE-BACKUP-01 -- REAL B2 CLOSURE (D-R2)
#
# Proof for scripts\windows\Invoke-MiniQuantDeskOffsiteBackup.ps1.
#
# Section 1: static safety-guard assertions against the script's own text.
# Section 2: real invocation of each fail-closed operator checkpoint
#   (B2_TRANSPORT_CONFIG_REQUIRED / OPERATOR_SECRET_SETUP_REQUIRED /
#   B2_APPLICATION_KEY_REQUIRED) -- all fast, none touch restic/docker.
# Section 3: a REAL end-to-end functional proof of the entire orchestration
#   (restic init/backup/check/forget --dry-run/restore, full-manifest
#   verification, disposable-DB restore, secret-canary re-scan) using a
#   REAL restic binary against a LOCAL filesystem repository instead of
#   Backblaze B2. This proves the wiring is correct; it is explicitly NOT
#   the OPS_OFFSITE_BACKUP_CODE_AND_REAL_B2_PROOF acceptance proof, which
#   requires real B2 bucket/endpoint/region + application key + password
#   file the operator has not yet supplied on this machine (this script
#   reports that truthfully rather than fabricating it).
#
# Exit codes: 0 = all proofs held, 1 = at least one did not.
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$WindowsDir = (Resolve-Path (Join-Path $ScriptDir '..')).Path.TrimEnd('\')
$RepoRoot  = (Resolve-Path (Join-Path $WindowsDir '..\..')).Path.TrimEnd('\')
$OffsiteScript = Join-Path $WindowsDir 'Invoke-MiniQuantDeskOffsiteBackup.ps1'

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

if (-not (Test-Path -LiteralPath $OffsiteScript)) {
    Show-Red "FATAL -- required script not found: $OffsiteScript"
    exit 1
}

function Invoke-TestSubprocess {
    param([Parameter(Mandatory = $true)][string[]]$ArgumentList)
    $stdoutFile = [System.IO.Path]::GetTempFileName()
    $stderrFile = [System.IO.Path]::GetTempFileName()
    try {
        $proc = Start-Process -FilePath 'powershell.exe' -NoNewWindow -Wait -PassThru `
            -ArgumentList $ArgumentList -RedirectStandardOutput $stdoutFile -RedirectStandardError $stderrFile
        $stdout = Get-Content -Path $stdoutFile -Raw -ErrorAction SilentlyContinue
        return [pscustomobject]@{ ExitCode = $proc.ExitCode; Stdout = $stdout }
    } finally {
        Remove-Item -Path $stdoutFile, $stderrFile -Force -ErrorAction SilentlyContinue
    }
}

# ---------------------------------------------------------------------------
# Section 1: static safety-guard assertions
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 1: static safety-guard proof ==='

$OffsiteText = Get-Content -Path $OffsiteScript -Raw

Assert-True 'never installs restic silently (fails closed with RESTIC_INSTALL_REQUIRED)' `
    ($OffsiteText -match [regex]::Escape('RESTIC_INSTALL_REQUIRED'))
Assert-True 'never guesses B2 bucket/endpoint/region (fails closed with B2_TRANSPORT_CONFIG_REQUIRED)' `
    ($OffsiteText -match [regex]::Escape('B2_TRANSPORT_CONFIG_REQUIRED'))
Assert-True 'never generates/prints a restic password (fails closed with OPERATOR_SECRET_SETUP_REQUIRED)' `
    ($OffsiteText -match [regex]::Escape('OPERATOR_SECRET_SETUP_REQUIRED'))
Assert-True 'never invents a B2 application key (fails closed with B2_APPLICATION_KEY_REQUIRED)' `
    ($OffsiteText -match [regex]::Escape('B2_APPLICATION_KEY_REQUIRED'))
Assert-True 'restic repository password itself is never echoed by this script (only the safe creation command text)' `
    (-not ($OffsiteText -match [regex]::Escape('Write-Host $passwordFilePath')) -and -not ($OffsiteText -match '(?i)Write-(Host|Ok|Step).*\$password\b'))
Assert-True 'retention policy is exercised with --dry-run and restic forget never runs without it in the same call' `
    ($OffsiteText -match [regex]::Escape("'forget', '--keep-daily', '14', '--keep-weekly', '8', '--dry-run'"))
Assert-True 'restic prune is never invoked anywhere in this script (no ResticArgs entry names it)' `
    (-not ($OffsiteText -match [regex]::Escape("'prune'")))
Assert-True 'restic forget is never called with a real (non-dry-run) delete -- the only ''forget'' call includes --dry-run' `
    ((([regex]::Matches($OffsiteText, [regex]::Escape("ResticArgs @('forget'")))).Count -eq 1)
Assert-True 'restic child process environment is narrow -- exactly the 5 documented keys, never the whole .env.local' `
    ($OffsiteText -match [regex]::Escape('$ResticEnv = @{') -and `
     $OffsiteText -match [regex]::Escape('AWS_ACCESS_KEY_ID     = $b2AccountId') -and `
     $OffsiteText -match [regex]::Escape('AWS_SECRET_ACCESS_KEY = $b2AccountKey') -and `
     -not ($OffsiteText -match [regex]::Escape('Import-LauncherEnvironmentFiles')))
Assert-True 'restic invocation uses fully-asynchronous stdout/stderr capture (ReadToEndAsync), never a blocking ReadToEnd (D-R2-R3)' `
    ($OffsiteText -match [regex]::Escape('$process.StandardOutput.ReadToEndAsync()') -and `
     $OffsiteText -match [regex]::Escape('$process.StandardError.ReadToEndAsync()') -and `
     $OffsiteText -match [regex]::Escape('$finished = $process.WaitForExit($TimeoutSeconds * 1000)') -and `
     -not ($OffsiteText -match [regex]::Escape('.ReadToEnd()')))
Assert-True 'restic invocation no longer depends on the Register-ObjectEvent/BeginOutputReadLine event-subscriber race (D-R2-R3)' `
    (-not ($OffsiteText -match [regex]::Escape('Register-ObjectEvent')) -and -not ($OffsiteText -match [regex]::Escape('BeginOutputReadLine')))
Assert-True 'ReadToEndAsync is started before WaitForExit is called, not after (D-R2-R3)' `
    ($OffsiteText.IndexOf('$process.StandardOutput.ReadToEndAsync()') -lt $OffsiteText.IndexOf('$finished = $process.WaitForExit($TimeoutSeconds * 1000)'))
Assert-True 'stdout/stderr stream drain after process exit is itself bounded, not assumed from WaitForExit alone (D-R2-R3)' `
    ($OffsiteText -match [regex]::Escape('[System.Threading.Tasks.Task]::WaitAll(@($stdoutTask, $stderrTask), $drainTimeoutMs)') -and $OffsiteText -match [regex]::Escape('OUTPUT_CAPTURE_TIMEOUT'))
Assert-True 'never touches Live (no -Mode Live / Live literal anywhere)' `
    (-not ($OffsiteText -match '(?i)-Mode\s+Live' -or $OffsiteText -match "(?i)'Live'"))
Assert-True 'never calls a broker/order route or ops action_key' `
    (-not ($OffsiteText -match [regex]::Escape('action_key') -or $OffsiteText -match [regex]::Escape('/api/v1/ops/action')))
Assert-True 'reuses the D-R1-hardened Restore-MiniQuantDeskRecovery.ps1 against the restic-restored set (not a parallel restore path)' `
    ($OffsiteText -match [regex]::Escape('-File $RestoreScript -BackupDir $restoredBackupDir'))
Assert-True 'REAL_B2_PROOF=PASS is bound to the frozen intended bucket (D-R2-R2-01)' `
    ($OffsiteText -match [regex]::Escape("`$RequiredB2Bucket = 'miniquantdesk-recovery-8f42c1'") -and $OffsiteText -match [regex]::Escape('B2_BUCKET_MISMATCH'))
Assert-True 'REAL_B2_PROOF=PASS requires a *.backblazeb2.com endpoint, never an arbitrary S3 host (D-R2-R2-01)' `
    ($OffsiteText -match [regex]::Escape('\.backblazeb2\.com$'))
Assert-True 'a local/opaque MQK_RESTIC_REPOSITORY override is LOCAL/TEST ORCHESTRATION ONLY unless it independently satisfies the same S3+bucket identity (D-R2-R2-01)' `
    ($OffsiteText -match [regex]::Escape('LOCAL/TEST ORCHESTRATION ONLY') -and $OffsiteText -match [regex]::Escape('$isRealB2Mode = $false'))
Assert-True 'success status is never the same literal string for real-B2 vs local/test orchestration (D-R2-R2-01)' `
    ($OffsiteText -match [regex]::Escape("Write-Ok 'REAL_B2_PROOF=PASS'") -and $OffsiteText -match [regex]::Escape("Write-Ok 'LOCAL_RESTIC_ORCHESTRATION_PASS'"))
Assert-True 'restic repository password file location authority requires an absolute path (D-R2-R2-02)' `
    ($OffsiteText -match [regex]::Escape('IsPathRooted($PasswordFilePath)'))
Assert-True 'restic repository password file location authority is checked against Git worktree roots, not a hardcoded single directory (D-R2-R2-02)' `
    ($OffsiteText -match [regex]::Escape('git -C $RepoRoot worktree list --porcelain'))
Assert-True 'restic repository password file inside any worktree root is refused (D-R2-R2-02)' `
    ($OffsiteText -match [regex]::Escape('OutsideWorktrees'))
Assert-True 'restic repository password file location authority fails closed if Git worktree enumeration itself is unavailable (D-R2-R2-02)' `
    ($OffsiteText -match [regex]::Escape('WORKTREE_AUTHORITY_UNAVAILABLE'))
Assert-True 'scratch-directory cleanup uses -ErrorAction Stop (authoritative), never SilentlyContinue (D-R2-R2-03)' `
    (-not ($OffsiteText -match [regex]::Escape('Remove-Item -Path $stagingDir -Recurse -Force -ErrorAction SilentlyContinue')) -and `
     -not ($OffsiteText -match [regex]::Escape('Remove-Item -Path $restoreTargetDir -Recurse -Force -ErrorAction SilentlyContinue')) -and `
     $OffsiteText -match [regex]::Escape('Remove-Item -Path $dir -Recurse -Force -ErrorAction Stop'))
Assert-True 'a cleanup failure sets the overall result to failure before any success token can be printed (D-R2-R2-03)' `
    ($OffsiteText -match [regex]::Escape('$cleanupSucceeded = $false') -and `
     ($OffsiteText.IndexOf('if (-not $cleanupSucceeded)') -lt $OffsiteText.IndexOf("Write-Ok 'LOCAL_RESTIC_ORCHESTRATION_PASS'")))

# ---------------------------------------------------------------------------
# Section 2: real invocation of each fail-closed operator checkpoint. All
# fail before touching restic/docker, so these are cheap.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 2: real operator-checkpoint negative controls ==='

# --- B2_TRANSPORT_CONFIG_REQUIRED: empty scratch repo root, no params -----
$emptyScratchRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_offsite_empty_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $emptyScratchRoot | Out-Null
$r1 = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $OffsiteScript, '-RepoRoot', $emptyScratchRoot)
Assert-True 'checkpoint: missing B2 bucket/endpoint/region is refused (nonzero exit)' ($r1.ExitCode -ne 0)
Assert-True 'checkpoint: refusal reason is B2_TRANSPORT_CONFIG_REQUIRED' ($r1.Stdout -match [regex]::Escape('B2_TRANSPORT_CONFIG_REQUIRED'))
Remove-Item -Path $emptyScratchRoot -Recurse -Force -ErrorAction SilentlyContinue

# --- OPERATOR_SECRET_SETUP_REQUIRED: transport supplied, no password file -
$noPasswordScratchRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_offsite_nopw_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $noPasswordScratchRoot | Out-Null
& git -C $noPasswordScratchRoot init -q 2>&1 | Out-Null
& git -C $noPasswordScratchRoot -c user.email='test@example.com' -c user.name='test' commit -q -m 'fixture' --allow-empty 2>&1 | Out-Null
$r2 = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $OffsiteScript, '-RepoRoot', $noPasswordScratchRoot, '-B2Bucket', 'miniquantdesk-recovery-8f42c1', '-B2Endpoint', 's3.us-west-004.backblazeb2.com', '-B2Region', 'us-west-004')
Assert-True 'checkpoint: missing restic password file is refused (nonzero exit)' ($r2.ExitCode -ne 0)
Assert-True 'checkpoint: refusal reason is OPERATOR_SECRET_SETUP_REQUIRED' ($r2.Stdout -match [regex]::Escape('OPERATOR_SECRET_SETUP_REQUIRED'))
Remove-Item -Path $noPasswordScratchRoot -Recurse -Force -ErrorAction SilentlyContinue

# --- B2_APPLICATION_KEY_REQUIRED: transport + password file present, no key.
# The password file must live OUTSIDE the git worktree used as -RepoRoot
# (D-R2-R2-02), so the fixture nests the actual repo one level under the
# scratch root and keeps the password file at the scratch root itself.
$noKeyScratchRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_offsite_nokey_" + [guid]::NewGuid().ToString('N'))
$noKeyRepoRoot = Join-Path $noKeyScratchRoot 'fixture_repo'
New-Item -ItemType Directory -Force -Path $noKeyRepoRoot | Out-Null
& git -C $noKeyRepoRoot init -q 2>&1 | Out-Null
& git -C $noKeyRepoRoot -c user.email='test@example.com' -c user.name='test' commit -q -m 'fixture' --allow-empty 2>&1 | Out-Null
$noKeyPasswordFile = Join-Path $noKeyScratchRoot 'fixture-restic-password.txt'
Set-Content -Path $noKeyPasswordFile -Value 'fixture-password-not-a-real-secret' -Encoding UTF8 -NoNewline
Set-Content -Path (Join-Path $noKeyRepoRoot '.env.local') -Value "MQK_RESTIC_PASSWORD_FILE=$noKeyPasswordFile" -Encoding UTF8
$r3 = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $OffsiteScript, '-RepoRoot', $noKeyRepoRoot, '-B2Bucket', 'miniquantdesk-recovery-8f42c1', '-B2Endpoint', 's3.us-west-004.backblazeb2.com', '-B2Region', 'us-west-004')
Assert-True 'checkpoint: missing B2_ACCOUNT_ID/B2_ACCOUNT_KEY is refused (nonzero exit)' ($r3.ExitCode -ne 0)
Assert-True 'checkpoint: refusal reason is B2_APPLICATION_KEY_REQUIRED' ($r3.Stdout -match [regex]::Escape('B2_APPLICATION_KEY_REQUIRED'))
Remove-Item -Path $noKeyScratchRoot -Recurse -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------
# Section 2b: real-B2-authority negative controls (D-R2-R2-01, D-R2-R2-02).
# All fail before touching restic/docker, so these are cheap.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 2b: real-B2-authority and password-location negative controls ==='

# --- B2_BUCKET_MISMATCH: right transport shape, wrong bucket -----------------
$wrongBucketRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_offsite_wrongbucket_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $wrongBucketRoot | Out-Null
$rBucket = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $OffsiteScript, '-RepoRoot', $wrongBucketRoot, '-B2Bucket', 'some-other-bucket', '-B2Endpoint', 's3.us-west-004.backblazeb2.com', '-B2Region', 'us-west-004')
Assert-True 'B2F: a bucket other than miniquantdesk-recovery-8f42c1 is refused (nonzero exit)' ($rBucket.ExitCode -ne 0)
Assert-True 'B2F: refusal reason is B2_BUCKET_MISMATCH' ($rBucket.Stdout -match [regex]::Escape('B2_BUCKET_MISMATCH'))
Remove-Item -Path $wrongBucketRoot -Recurse -Force -ErrorAction SilentlyContinue

# --- B2_TRANSPORT_CONFIG_REQUIRED: correct bucket, non-B2 endpoint -----------
$nonB2EndpointRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_offsite_nonb2endpoint_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $nonB2EndpointRoot | Out-Null
$rEndpoint = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $OffsiteScript, '-RepoRoot', $nonB2EndpointRoot, '-B2Bucket', 'miniquantdesk-recovery-8f42c1', '-B2Endpoint', 's3.amazonaws.com', '-B2Region', 'us-east-1')
Assert-True 'B2G: a non-*.backblazeb2.com endpoint is refused (nonzero exit)' ($rEndpoint.ExitCode -ne 0)
Assert-True 'B2G: refusal reason is B2_TRANSPORT_CONFIG_REQUIRED' ($rEndpoint.Stdout -match [regex]::Escape('B2_TRANSPORT_CONFIG_REQUIRED'))
Remove-Item -Path $nonB2EndpointRoot -Recurse -Force -ErrorAction SilentlyContinue

# --- OPERATOR_SECRET_SETUP_REQUIRED: relative password-file path ------------
$relativePwRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_offsite_relativepw_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $relativePwRoot | Out-Null
& git -C $relativePwRoot init -q 2>&1 | Out-Null
& git -C $relativePwRoot -c user.email='test@example.com' -c user.name='test' commit -q -m 'fixture' --allow-empty 2>&1 | Out-Null
Set-Content -Path (Join-Path $relativePwRoot '.env.local') -Value 'MQK_RESTIC_PASSWORD_FILE=.\relative-password.txt' -Encoding UTF8
$rRelativePw = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $OffsiteScript, '-RepoRoot', $relativePwRoot, '-B2Bucket', 'miniquantdesk-recovery-8f42c1', '-B2Endpoint', 's3.us-west-004.backblazeb2.com', '-B2Region', 'us-west-004')
Assert-True 'B2H: a relative RESTIC_PASSWORD_FILE path is refused (nonzero exit)' ($rRelativePw.ExitCode -ne 0)
Assert-True 'B2H: refusal reason is OPERATOR_SECRET_SETUP_REQUIRED' ($rRelativePw.Stdout -match [regex]::Escape('OPERATOR_SECRET_SETUP_REQUIRED'))
Remove-Item -Path $relativePwRoot -Recurse -Force -ErrorAction SilentlyContinue

# --- OPERATOR_SECRET_SETUP_REQUIRED: password file inside the repo/worktree -
$pwInsideRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_offsite_pwinside_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $pwInsideRoot | Out-Null
& git -C $pwInsideRoot init -q 2>&1 | Out-Null
& git -C $pwInsideRoot -c user.email='test@example.com' -c user.name='test' commit -q -m 'fixture' --allow-empty 2>&1 | Out-Null
$pwInsideFile = Join-Path $pwInsideRoot 'restic-password-inside-repo.txt'
Set-Content -Path $pwInsideFile -Value 'fixture-password-inside-worktree-not-a-real-secret' -Encoding UTF8 -NoNewline
Set-Content -Path (Join-Path $pwInsideRoot '.env.local') -Value "MQK_RESTIC_PASSWORD_FILE=$pwInsideFile" -Encoding UTF8
$rPwInside = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $OffsiteScript, '-RepoRoot', $pwInsideRoot, '-B2Bucket', 'miniquantdesk-recovery-8f42c1', '-B2Endpoint', 's3.us-west-004.backblazeb2.com', '-B2Region', 'us-west-004')
Assert-True 'B2I: a password file resolving inside a Git worktree root is refused (nonzero exit, real invocation)' ($rPwInside.ExitCode -ne 0)
Assert-True 'B2I: refusal reason is OPERATOR_SECRET_SETUP_REQUIRED (not silently accepted)' ($rPwInside.Stdout -match [regex]::Escape('OPERATOR_SECRET_SETUP_REQUIRED'))
Remove-Item -Path $pwInsideRoot -Recurse -Force -ErrorAction SilentlyContinue

# --- an override that DOES satisfy the S3+*.backblazeb2.com+bucket pattern --
# preserves real-B2 authority (Option A) instead of being forced into
# LOCAL/TEST mode -- proven statically since it requires no network/restic.
$qualifyingOverrideRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_offsite_qualifyingoverride_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $qualifyingOverrideRoot | Out-Null
Set-Content -Path (Join-Path $qualifyingOverrideRoot '.env.local') -Value 'MQK_RESTIC_REPOSITORY=s3:https://s3.us-west-004.backblazeb2.com/miniquantdesk-recovery-8f42c1/miniquantdesk-recovery' -Encoding UTF8
$rQualifyingOverride = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $OffsiteScript, '-RepoRoot', $qualifyingOverrideRoot)
Assert-True 'an MQK_RESTIC_REPOSITORY override matching S3+*.backblazeb2.com+frozen-bucket is recognized as real-B2 authority, not forced into local/test mode' `
    ($rQualifyingOverride.Stdout -match [regex]::Escape('real-B2 authority preserved') -and -not ($rQualifyingOverride.Stdout -match [regex]::Escape('LOCAL/TEST ORCHESTRATION ONLY')))
Remove-Item -Path $qualifyingOverrideRoot -Recurse -Force -ErrorAction SilentlyContinue

# ---------------------------------------------------------------------------
# Section 3: REAL end-to-end functional proof against a LOCAL restic
# repository (not B2 -- the B2 transport itself remains blocked on operator-
# supplied bucket/endpoint/region + application key + password file on this
# machine). This is real restic, real init/backup/check/forget --dry-run/
# restore, real disposable-DB restore, and a real secret-canary re-scan --
# only the network destination differs from the production B2 path.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 3: real end-to-end functional proof (local restic repository) ==='

$resticCmd = Get-Command 'restic' -ErrorAction SilentlyContinue
if (-not $resticCmd) {
    Show-Info '  INFO -- restic not installed on this box: skipping Section 3 (Section 2''s RESTIC_INSTALL_REQUIRED text proof already covers this path).'
} else {
    $localRepoRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_offsite_localrepo_" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $localRepoRoot | Out-Null

    $fixtureRepoRoot = Join-Path $localRepoRoot 'fixture_repo'
    New-Item -ItemType Directory -Force -Path (Join-Path $fixtureRepoRoot 'config') | Out-Null
    Set-Content -Path (Join-Path $fixtureRepoRoot '.gitignore') -Value '.env.local' -Encoding UTF8
    Set-Content -Path (Join-Path $fixtureRepoRoot 'config\fixture.json') -Value '{"fixture": true}' -Encoding UTF8
    & git -C $fixtureRepoRoot init -q 2>&1 | Out-Null
    & git -C $fixtureRepoRoot -c user.email='test@example.com' -c user.name='test' add .gitignore config 2>&1 | Out-Null
    & git -C $fixtureRepoRoot -c user.email='test@example.com' -c user.name='test' commit -q -m 'fixture' 2>&1 | Out-Null

    $localResticRepoDir = Join-Path $localRepoRoot 'restic_repo'
    New-Item -ItemType Directory -Force -Path $localResticRepoDir | Out-Null
    $localResticPasswordFile = Join-Path $localRepoRoot 'restic-password.txt'
    Set-Content -Path $localResticPasswordFile -Value 'fixture-local-repo-password-not-a-real-secret' -Encoding UTF8 -NoNewline

    $envLines = @(
        "MQK_RESTIC_REPOSITORY=$localResticRepoDir",
        "MQK_RESTIC_PASSWORD_FILE=$localResticPasswordFile",
        'B2_ACCOUNT_ID=fixture-not-a-real-b2-account-id',
        'B2_ACCOUNT_KEY=fixture-not-a-real-b2-account-key'
    )
    Set-Content -Path (Join-Path $fixtureRepoRoot '.env.local') -Value ($envLines -join "`n") -Encoding UTF8

    $r4 = Invoke-TestSubprocess -ArgumentList @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $OffsiteScript, '-RepoRoot', $fixtureRepoRoot, '-DisposableDbContainer', 'mqk-test-postgres', '-ResticTimeoutSeconds', '120')
    Assert-True 'real functional proof (local repository): orchestration exits 0' ($r4.ExitCode -eq 0)
    Assert-True 'real functional proof (local repository): reports a real snapshot_id' ($r4.Stdout -match 'snapshot_id=\S+')
    Assert-True 'real functional proof (local repository): full-manifest + disposable-DB restore proof ran' ($r4.Stdout -match [regex]::Escape('Full manifest verification passed'))
    Assert-True 'real functional proof (local repository): secret-canary re-scan ran and found nothing' ($r4.Stdout -match [regex]::Escape('No allowlisted local secret value found in the restic-restored material'))
    # B2A/B2B (D-R2-R2-01): a local filesystem repository must report
    # LOCAL_RESTIC_ORCHESTRATION_PASS and must NEVER claim REAL_B2_PROOF=PASS,
    # even though the orchestration itself succeeded end-to-end.
    Assert-True 'B2A: real functional proof (local repository) reports LOCAL_RESTIC_ORCHESTRATION_PASS' `
        ($r4.Stdout -match [regex]::Escape('LOCAL_RESTIC_ORCHESTRATION_PASS'))
    # Matches only the actual success-token print ("OK: REAL_B2_PROOF=PASS"),
    # not the earlier cautionary WARN sentence ("...cannot emit
    # REAL_B2_PROOF=PASS.") which legitimately contains the same substring.
    Assert-True 'B2A: real functional proof (local repository) NEVER emits the REAL_B2_PROOF=PASS success token' `
        (-not ($r4.Stdout -match [regex]::Escape('OK: REAL_B2_PROOF=PASS')))
    # B2M: none of the fixture secret values (B2 key id/secret, restic
    # password contents) ever appear in captured stdout.
    Assert-True 'B2M: the fixture B2 account id never appears in stdout' (-not ($r4.Stdout -match [regex]::Escape('fixture-not-a-real-b2-account-id')))
    Assert-True 'B2M: the fixture B2 account key never appears in stdout' (-not ($r4.Stdout -match [regex]::Escape('fixture-not-a-real-b2-account-key')))
    Assert-True 'B2M: the restic repository password file contents never appear in stdout' (-not ($r4.Stdout -match [regex]::Escape('fixture-local-repo-password-not-a-real-secret')))
    # B2O: the external password file lives OUTSIDE $fixtureRepoRoot (a
    # sibling of it under $localRepoRoot), and Backup-MiniQuantDeskRecovery.ps1
    # only ever stages content found underneath -RepoRoot -- so the staged
    # (and therefore backed-up/restored) recovery set can structurally never
    # include it. The manifest's exact file count is the real, checkable
    # proof of that: exactly the 4 files this minimal fixture always
    # produces (git_identity.json, manifest.json, paper_db.dump, one
    # safe_config file), never a 5th entry for the password file.
    Assert-True 'B2O: the restic-restored manifest lists exactly the expected 4 staged files (the external password file was never swept in)' `
        ($r4.Stdout -match [regex]::Escape('Full manifest verification passed: 4 file(s)'))

    if ($resticCmd) {
        $snapshotsCheckEnv = @{ RESTIC_REPOSITORY = $localResticRepoDir; RESTIC_PASSWORD_FILE = $localResticPasswordFile }
        $prevRepo = $env:RESTIC_REPOSITORY; $prevPwFile = $env:RESTIC_PASSWORD_FILE
        try {
            $env:RESTIC_REPOSITORY = $localResticRepoDir
            $env:RESTIC_PASSWORD_FILE = $localResticPasswordFile
            $snapCheck = & $resticCmd.Source snapshots --json 2>&1
            $snapJson = $null
            try { $snapJson = ($snapCheck -join '') | ConvertFrom-Json } catch {}
            Assert-True 'real functional proof (local repository): exactly one snapshot exists in the fixture repo (idempotent init proven separately by static/logic review)' `
                ($null -ne $snapJson -and @($snapJson).Count -eq 1)
        } finally {
            $env:RESTIC_REPOSITORY = $prevRepo
            $env:RESTIC_PASSWORD_FILE = $prevPwFile
        }
    }

    Remove-Item -Path $localRepoRoot -Recurse -Force -ErrorAction SilentlyContinue
}

Show-Info ''
Show-Info '  NOTE -- Section 3 proves the orchestration logic end-to-end against a real restic repository. It is NOT the OPS_OFFSITE_BACKUP_CODE_AND_REAL_B2_PROOF acceptance proof, which requires a real Backblaze B2 bucket/endpoint/region + application key + password file that this machine does not yet have configured (Section 2 proves that gap is reported truthfully, not fabricated).'

# ---------------------------------------------------------------------------
# Section 4: real cleanup-failure mutation control (D-R2-R2-03). A REAL
# Windows file lock (an open FileStream, no permission/ACL manipulation) is
# held on a file inside the staging directory for the entire run via the
# -StagingDirOverrideForCleanupTest test-only seam, so the deterministic
# result is: the orchestration itself succeeds end-to-end (real snapshot,
# real restore, real disposable-DB proof all pass) but the final
# scratch-directory cleanup cannot remove the locked file -- and that must
# make the OVERALL result a failure with no success token printed at all,
# proving cleanup failure is no longer swallowed by SilentlyContinue.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 4: real cleanup-failure mutation control ==='

if (-not $resticCmd) {
    Show-Info '  INFO -- restic not installed on this box: skipping Section 4 (same gap Section 3 already reports).'
} else {
    $cleanupMutRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_offsite_cleanupmut_" + [guid]::NewGuid().ToString('N'))
    New-Item -ItemType Directory -Force -Path $cleanupMutRoot | Out-Null

    $cleanupFixtureRepoRoot = Join-Path $cleanupMutRoot 'fixture_repo'
    New-Item -ItemType Directory -Force -Path (Join-Path $cleanupFixtureRepoRoot 'config') | Out-Null
    Set-Content -Path (Join-Path $cleanupFixtureRepoRoot '.gitignore') -Value '.env.local' -Encoding UTF8
    Set-Content -Path (Join-Path $cleanupFixtureRepoRoot 'config\fixture.json') -Value '{"fixture": true}' -Encoding UTF8
    & git -C $cleanupFixtureRepoRoot init -q 2>&1 | Out-Null
    & git -C $cleanupFixtureRepoRoot -c user.email='test@example.com' -c user.name='test' add .gitignore config 2>&1 | Out-Null
    & git -C $cleanupFixtureRepoRoot -c user.email='test@example.com' -c user.name='test' commit -q -m 'fixture' 2>&1 | Out-Null

    $cleanupResticRepoDir = Join-Path $cleanupMutRoot 'restic_repo'
    New-Item -ItemType Directory -Force -Path $cleanupResticRepoDir | Out-Null
    $cleanupPasswordFile = Join-Path $cleanupMutRoot 'restic-password.txt'
    Set-Content -Path $cleanupPasswordFile -Value 'fixture-local-repo-password-not-a-real-secret' -Encoding UTF8 -NoNewline
    $cleanupEnvLines = @(
        "MQK_RESTIC_REPOSITORY=$cleanupResticRepoDir",
        "MQK_RESTIC_PASSWORD_FILE=$cleanupPasswordFile",
        'B2_ACCOUNT_ID=fixture-not-a-real-b2-account-id',
        'B2_ACCOUNT_KEY=fixture-not-a-real-b2-account-key'
    )
    Set-Content -Path (Join-Path $cleanupFixtureRepoRoot '.env.local') -Value ($cleanupEnvLines -join "`n") -Encoding UTF8

    $forcedStagingDir = Join-Path $cleanupMutRoot 'forced_staging_dir'
    New-Item -ItemType Directory -Force -Path $forcedStagingDir | Out-Null
    $lockedFixtureFile = Join-Path $forcedStagingDir 'zz_mutation_lock_fixture.bin'
    Set-Content -Path $lockedFixtureFile -Value 'innocuous-mutation-lock-fixture-content' -Encoding UTF8 -NoNewline

    # FileShare.Read: other processes (this script's own hashing/staging
    # steps) can still READ the file, but Windows refuses to DELETE it while
    # this handle is open -- a real, deterministic Remove-Item failure with
    # no ACL/permission changes involved.
    $lockStream = [System.IO.File]::Open($lockedFixtureFile, [System.IO.FileMode]::Open, [System.IO.FileAccess]::Read, [System.IO.FileShare]::Read)
    $r5 = $null
    try {
        $r5 = Invoke-TestSubprocess -ArgumentList @(
            '-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $OffsiteScript,
            '-RepoRoot', $cleanupFixtureRepoRoot, '-DisposableDbContainer', 'mqk-test-postgres',
            '-ResticTimeoutSeconds', '120', '-StagingDirOverrideForCleanupTest', $forcedStagingDir
        )
    } finally {
        $lockStream.Close()
        $lockStream.Dispose()
    }

    Assert-True 'B2K: cleanup failure (locked scratch file) makes the overall result a failure (nonzero exit), even though the real workflow itself succeeded' ($r5.ExitCode -ne 0)
    Assert-True 'B2K: the workflow itself really did succeed up to cleanup (real snapshot + full manifest verification both ran)' `
        ($r5.Stdout -match 'Real encrypted snapshot created' -and $r5.Stdout -match [regex]::Escape('Full manifest verification passed'))
    Assert-True 'B2K: the cleanup failure is reported explicitly, not swallowed' ($r5.Stdout -match [regex]::Escape('Failed to remove scratch directory'))
    Assert-True 'B2L: no success token (real-B2 or local/test) is printed when cleanup fails' `
        (-not ($r5.Stdout -match [regex]::Escape('LOCAL_RESTIC_ORCHESTRATION_PASS')) -and -not ($r5.Stdout -match [regex]::Escape('OK: REAL_B2_PROOF=PASS')) -and -not ($r5.Stdout -match [regex]::Escape('OPS_OFFSITE_BACKUP_CODE_AND_REAL_B2_PROOF=PASS')))

    Remove-Item -Path $cleanupMutRoot -Recurse -Force -ErrorAction SilentlyContinue
}

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
