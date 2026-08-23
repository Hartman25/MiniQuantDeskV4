# =============================================================================
# OPS-OFFSITE-BACKUP-01
# Backup-MiniQuantDeskRecovery.ps1
#
# Stages a local recovery-backup set: exact source/Git identity, a logical
# (never physical-directory) dump of the Paper PostgreSQL database, safe
# non-secret configuration/manifests, and (when supplied) a research
# registry SQLite copy / promotion-evidence directory -- then writes a
# content-addressed manifest (SHA-256 per file) over everything actually
# staged.
#
# ARCHITECTURE (decided, not re-litigated by this script):
#   backup engine       = restic
#   offsite destination = Backblaze B2
#
# THIS SCRIPT NEVER:
#   - copies the live PostgreSQL data directory (D1 -- logical dump only,
#     via pg_dump; a physical directory copy of a running Postgres instance
#     is not a valid backup and is never attempted here);
#   - uploads anywhere (no B2 call exists in this script at all);
#   - invents or requests B2/restic credentials;
#   - writes a credential/token/API-key value into the staged manifest or
#     any staged file;
#   - includes .env/.env.local/.env.local.example or anything else matching
#     the secret-exclusion patterns below (checked twice: at copy time, and
#     again as a post-stage scan that FAILS CLOSED if any excluded pattern
#     is found in the staged tree);
#   - reports success if any REQUIRED component (git identity, Paper DB
#     logical dump) failed (D15).
#
# If `restic` is not installed, this script stages everything above (still
# genuinely useful recovery content) and reports RESTIC_INSTALL_REQUIRED --
# it does not fabricate an "encrypted"/"offsite" claim. If a Backblaze B2
# destination/restic repository is not configured (MQK_RESTIC_REPOSITORY /
# MQK_RESTIC_PASSWORD_FILE env vars), the restic step is skipped with an
# explicit reason -- never silently, never invented.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Backup-MiniQuantDeskRecovery.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Backup-MiniQuantDeskRecovery.ps1 -ResearchRegistryDb C:\path\to\registry.sqlite
#
# Exit codes: 0 = all REQUIRED components staged (restic may still be
# RESTIC_INSTALL_REQUIRED/not-configured -- see manifest.json), 1 = a
# REQUIRED component failed.
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param(
    [string] $RepoRoot             = '',
    [string] $OutDir               = '',
    [string] $PaperDbContainer     = 'mqk-paper-postgres',
    [string] $PaperDbUser          = 'postgres',
    [string] $PaperDbName          = 'miniquantdesk_paper',
    [string] $ResearchRegistryDb   = '',
    [string] $PromotionEvidenceDir = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Step { param([string]$M) Write-Host "[BACKUP] $M"       -ForegroundColor Cyan   }
function Write-Ok   { param([string]$M) Write-Host "[BACKUP] OK: $M"   -ForegroundColor Green  }
function Write-Warn { param([string]$M) Write-Host "[BACKUP] WARN: $M" -ForegroundColor Yellow }
function Write-Fail { param([string]$M) Write-Host "[BACKUP] FAIL: $M" -ForegroundColor Red    }
function Write-Sect { param([string]$M) Write-Host ""; Write-Host "=== $M ===" -ForegroundColor Magenta }

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')

$Stamp = Get-Date -Format 'yyyyMMdd_HHmmss'
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $env:USERPROFILE "MiniQuantDeskBackups\$Stamp"
}
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
Write-Step "Staging directory: $OutDir"

# ---------------------------------------------------------------------------
# Secret-exclusion patterns (D6). Applied both at copy time (never copy a
# matching path) and as a post-stage scan (fail closed if one slipped in).
# ---------------------------------------------------------------------------
$SecretPatterns = @(
    '^\.env(\..*)?$',           # .env, .env.local, .env.local.example, ...
    '(?i)secret',
    '(?i)credential',
    '(?i)api[_-]?key',
    '(?i)api[_-]?secret',
    '(?i)password',
    '(?i)\.pem$',
    '(?i)\.pfx$',
    '(?i)restic.*password'
)

function Test-IsSecretPath {
    param([Parameter(Mandatory = $true)][string]$LeafName)
    foreach ($pat in $SecretPatterns) {
        if ($LeafName -match $pat) { return $true }
    }
    return $false
}

$Components = [ordered]@{}
$RequiredComponentFailed = $false

# ---------------------------------------------------------------------------
# 0. Local secret-store exclusion authority (D-R1 defect 4a) -- proven BEFORE
#    anything is staged. A git bundle can only ever contain committed
#    objects, so this is what actually establishes that .env.local/.env
#    cannot reach the bundle: they must be both gitignored (so a future
#    accidental `git add` is blocked) and currently untracked (so no
#    historical commit already carries one into HEAD). If either check
#    fails, staging is refused outright rather than proceeding on an
#    unproven assumption.
# ---------------------------------------------------------------------------
Write-Sect 'Local secret-store exclusion authority'

# The expected/normal outcome of `git check-ignore` (no match) and
# `git ls-files --error-unmatch` (file not tracked) is a nonzero exit with a
# message on the native process's OWN stderr -- under this script's
# $ErrorActionPreference = 'Stop', that stderr text would otherwise be
# converted into an uncaught terminating error for what is actually the
# expected control-flow path. Locally relax to 'Continue' for just these
# probes so only $LASTEXITCODE is consulted, then restore 'Stop'.
function Test-GitCommandSucceeds {
    param([Parameter(Mandatory = $true)][string[]]$GitArgs)
    $prevEap = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        & git @GitArgs *> $null
        return ($LASTEXITCODE -eq 0)
    } finally {
        $ErrorActionPreference = $prevEap
    }
}

$LocalSecretStoreRelPaths = @('.env.local', '.env')
$secretStoreProofFailed = $false
$secretStoreProofDetail = @()
foreach ($rel in $LocalSecretStoreRelPaths) {
    $full = Join-Path $RepoRoot $rel
    if (-not (Test-Path -LiteralPath $full)) {
        Write-Ok "$rel does not exist on this machine -- nothing to prove."
        continue
    }
    $isIgnored = Test-GitCommandSucceeds -GitArgs @('-C', $RepoRoot, 'check-ignore', '-q', '--', $rel)
    if (-not $isIgnored) {
        Write-Fail "$rel exists but is NOT gitignored -- refusing to stage anything."
        $secretStoreProofFailed = $true
        $secretStoreProofDetail += "${rel}:not_gitignored"
        continue
    }
    $isTracked = Test-GitCommandSucceeds -GitArgs @('-C', $RepoRoot, 'ls-files', '--error-unmatch', '--', $rel)
    if ($isTracked) {
        Write-Fail "$rel is TRACKED by git -- refusing to stage anything (expected untracked)."
        $secretStoreProofFailed = $true
        $secretStoreProofDetail += "${rel}:tracked"
        continue
    }
    Write-Ok "$rel confirmed gitignored and untracked."
    $secretStoreProofDetail += "${rel}:ok"
}
if ($secretStoreProofFailed) {
    $Components['secret_store_exclusion_proof'] = @{ status = 'failed'; detail = $secretStoreProofDetail }
    $RequiredComponentFailed = $true
} else {
    $Components['secret_store_exclusion_proof'] = @{ status = 'ok'; detail = $secretStoreProofDetail }
}

# ---------------------------------------------------------------------------
# 1. Exact source/Git identity (D2). A bundle of HEAD -- self-contained,
#    restorable via `git clone <bundle>` -- plus a small identity JSON.
#    Never includes untracked/ignored files (git bundle only ever contains
#    committed objects), so this cannot smuggle a secret that was never
#    committed.
#
#    The Git remote URL is deliberately NEVER captured here (D-R1 defect 3):
#    a remote may embed userinfo/token credentials, and the commit SHA plus
#    bundle identity are the actual recovery authority -- a remote URL is
#    not needed to restore from this backup set.
# ---------------------------------------------------------------------------
Write-Sect 'Git identity + bundle'
try {
    $headSha    = (git -C $RepoRoot rev-parse HEAD).Trim()
    $branch     = (git -C $RepoRoot rev-parse --abbrev-ref HEAD).Trim()
    $shortStat  = (git -C $RepoRoot status --short) -join "`n"

    $bundlePath = Join-Path $OutDir 'source.bundle'
    git -C $RepoRoot bundle create $bundlePath HEAD 2>&1 | Out-Null
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $bundlePath)) {
        throw "git bundle create failed (exit $LASTEXITCODE)"
    }

    $identity = [ordered]@{
        head_sha            = $headSha
        branch              = $branch
        working_tree_dirty  = -not [string]::IsNullOrWhiteSpace($shortStat)
        working_tree_status = $shortStat
        captured_at_utc     = (Get-Date).ToUniversalTime().ToString('o')
    }
    ($identity | ConvertTo-Json -Depth 4) | Set-Content -Path (Join-Path $OutDir 'git_identity.json') -Encoding UTF8

    Write-Ok "Git identity captured: head=$headSha branch=$branch"
    $Components['git_identity'] = @{ status = 'ok'; head_sha = $headSha }
} catch {
    Write-Fail "Git identity/bundle capture failed: $($_.Exception.Message)"
    $Components['git_identity'] = @{ status = 'failed'; reason = $_.Exception.Message }
    $RequiredComponentFailed = $true
}

# ---------------------------------------------------------------------------
# 2. Logical Paper PostgreSQL dump (D1 -- REQUIRED). Never a physical
#    directory copy. Runs pg_dump INSIDE the mqk-paper-postgres container
#    (docker exec) rather than a separately-installed Windows client --
#    this guarantees the pg_dump binary's major version always matches the
#    server's own (avoids a client-newer-than-server pg_dump emitting a
#    session GUC the server's own pg_restore/psql can't recognize), and
#    removes any dependency on what happens to be installed on this host.
#    The dump file is written inside the container, then pulled out via
#    `docker cp` (safe for the binary custom-format dump -- no PowerShell
#    stdout-redirection encoding risk) and removed from the container.
# ---------------------------------------------------------------------------
Write-Sect 'Paper PostgreSQL logical dump'
$dumpPath = Join-Path $OutDir 'paper_db.dump'
$containerDumpPath = '/tmp/mqk_backup_paper_db.dump'

$dockerInspect = docker inspect $PaperDbContainer 2>&1
if ($LASTEXITCODE -ne 0) {
    Write-Fail "Container '$PaperDbContainer' not found/running. Is Docker Desktop running? (docker inspect exit $LASTEXITCODE)"
    $Components['paper_db_dump'] = @{ status = 'failed'; reason = 'container_not_found' }
    $RequiredComponentFailed = $true
} else {
    docker exec $PaperDbContainer pg_dump -U $PaperDbUser --format=custom --file=$containerDumpPath $PaperDbName 2>&1 | Out-Host
    $dumpExit = $LASTEXITCODE
    if ($dumpExit -ne 0) {
        Write-Fail "pg_dump inside '$PaperDbContainer' failed (exit $dumpExit)."
        $Components['paper_db_dump'] = @{ status = 'failed'; reason = "pg_dump_exit_$dumpExit" }
        $RequiredComponentFailed = $true
    } else {
        docker cp "${PaperDbContainer}:${containerDumpPath}" $dumpPath 2>&1 | Out-Host
        $cpExit = $LASTEXITCODE
        docker exec $PaperDbContainer rm -f $containerDumpPath 2>&1 | Out-Null
        if ($cpExit -ne 0 -or -not (Test-Path -LiteralPath $dumpPath) -or (Get-Item $dumpPath).Length -eq 0) {
            Write-Fail "docker cp of the dump failed or produced an empty file (exit $cpExit)."
            $Components['paper_db_dump'] = @{ status = 'failed'; reason = "docker_cp_exit_$cpExit" }
            $RequiredComponentFailed = $true
        } else {
            $sizeBytes = (Get-Item $dumpPath).Length
            Write-Ok "Paper DB logical dump written: $dumpPath ($sizeBytes bytes)"
            $Components['paper_db_dump'] = @{ status = 'ok'; size_bytes = $sizeBytes; source = "docker exec $PaperDbContainer pg_dump --format=custom (logical, not a data-directory copy)" }
        }
    }
}

# ---------------------------------------------------------------------------
# 3. Research registry SQLite copy (D3 -- captured only if supplied and
#    present; never fabricated). Uses Python's sqlite3 online-backup API
#    (safe against a concurrently-open DB) rather than a raw file copy.
# ---------------------------------------------------------------------------
Write-Sect 'Research registry (optional)'
if ([string]::IsNullOrWhiteSpace($ResearchRegistryDb)) {
    Write-Warn 'ResearchRegistryDb not supplied -- skipping (not a failure).'
    $Components['research_registry'] = @{ status = 'not_configured' }
} elseif (-not (Test-Path -LiteralPath $ResearchRegistryDb)) {
    Write-Warn "ResearchRegistryDb path does not exist: $ResearchRegistryDb -- skipping (not a failure)."
    $Components['research_registry'] = @{ status = 'not_found'; path = $ResearchRegistryDb }
} else {
    $destDb = Join-Path $OutDir 'research_registry.sqlite'
    $py = Get-Command 'python' -ErrorAction SilentlyContinue
    if (-not $py) { $py = Get-Command 'python3' -ErrorAction SilentlyContinue }
    if (-not $py) {
        Write-Warn 'No python interpreter found -- cannot safely online-backup the SQLite registry; skipping.'
        $Components['research_registry'] = @{ status = 'skipped'; reason = 'python_not_found' }
    } else {
        $pyScript = "import sqlite3,sys`nsrc=sqlite3.connect(sys.argv[1])`ndst=sqlite3.connect(sys.argv[2])`nsrc.backup(dst)`ndst.close()`nsrc.close()`n"
        $tmpPy = Join-Path $OutDir '_sqlite_backup.py'
        Set-Content -Path $tmpPy -Value $pyScript -Encoding UTF8
        & $py.Source $tmpPy $ResearchRegistryDb $destDb 2>&1 | Out-Host
        $pyExit = $LASTEXITCODE
        Remove-Item -Path $tmpPy -Force -ErrorAction SilentlyContinue
        if ($pyExit -ne 0 -or -not (Test-Path -LiteralPath $destDb)) {
            Write-Fail "Research registry online-backup failed (exit $pyExit)."
            $Components['research_registry'] = @{ status = 'failed'; reason = "python_exit_$pyExit" }
        } else {
            Write-Ok "Research registry copied: $destDb"
            $Components['research_registry'] = @{ status = 'ok'; size_bytes = (Get-Item $destDb).Length }
        }
    }
}

# ---------------------------------------------------------------------------
# 4. Promotion evidence directory (D4 -- optional, captured only if supplied
#    and present).
# ---------------------------------------------------------------------------
Write-Sect 'Promotion evidence (optional)'
if ([string]::IsNullOrWhiteSpace($PromotionEvidenceDir)) {
    Write-Warn 'PromotionEvidenceDir not supplied -- skipping (not a failure).'
    $Components['promotion_evidence'] = @{ status = 'not_configured' }
} elseif (-not (Test-Path -LiteralPath $PromotionEvidenceDir)) {
    Write-Warn "PromotionEvidenceDir does not exist: $PromotionEvidenceDir -- skipping (not a failure)."
    $Components['promotion_evidence'] = @{ status = 'not_found'; path = $PromotionEvidenceDir }
} else {
    $destDir = Join-Path $OutDir 'promotion_evidence'
    New-Item -ItemType Directory -Force -Path $destDir | Out-Null
    $copied = 0
    Get-ChildItem -Path $PromotionEvidenceDir -Recurse -File | ForEach-Object {
        if (Test-IsSecretPath -LeafName $_.Name) {
            Write-Warn "Excluded (secret-pattern match): $($_.FullName)"
            return
        }
        $rel = $_.FullName.Substring($PromotionEvidenceDir.TrimEnd('\').Length).TrimStart('\')
        $dest = Join-Path $destDir $rel
        New-Item -ItemType Directory -Force -Path (Split-Path $dest -Parent) | Out-Null
        Copy-Item -LiteralPath $_.FullName -Destination $dest -Force
        $copied++
    }
    Write-Ok "Promotion evidence copied: $copied file(s)"
    $Components['promotion_evidence'] = @{ status = 'ok'; files_copied = $copied }
}

# ---------------------------------------------------------------------------
# 5. Safe configuration / manifests (D5): master ledger, research closeout
#    audit, and the non-secret config/ tree (instruments/providers/risk
#    profiles -- JSON only, no credentials live here).
# ---------------------------------------------------------------------------
Write-Sect 'Safe configuration / manifests'
$safeDestRoot = Join-Path $OutDir 'safe_config'
New-Item -ItemType Directory -Force -Path $safeDestRoot | Out-Null
$safeCopied = 0

$namedFiles = @(
    'MiniQuantDesk_Master_Patch_Ledger_v2_updated.md',
    'docs\research\Research_Backtest_V1_Closeout_Audit.md'
)
foreach ($rel in $namedFiles) {
    $src = Join-Path $RepoRoot $rel
    if (Test-Path -LiteralPath $src) {
        $dest = Join-Path $safeDestRoot ($rel -replace '\\', '_')
        Copy-Item -LiteralPath $src -Destination $dest -Force
        $safeCopied++
    } else {
        Write-Warn "Expected file not found (skipped, not a failure): $rel"
    }
}

$configSrc = Join-Path $RepoRoot 'config'
if (Test-Path -LiteralPath $configSrc) {
    $configDest = Join-Path $safeDestRoot 'config'
    Get-ChildItem -Path $configSrc -Recurse -File | ForEach-Object {
        if (Test-IsSecretPath -LeafName $_.Name) {
            Write-Warn "Excluded (secret-pattern match): $($_.FullName)"
            return
        }
        $rel = $_.FullName.Substring($configSrc.TrimEnd('\').Length).TrimStart('\')
        $dest = Join-Path $configDest $rel
        New-Item -ItemType Directory -Force -Path (Split-Path $dest -Parent) | Out-Null
        Copy-Item -LiteralPath $_.FullName -Destination $dest -Force
        $safeCopied++
    }
}
Write-Ok "Safe configuration/manifests copied: $safeCopied file(s)"
$Components['safe_config'] = @{ status = 'ok'; files_copied = $safeCopied }

# ---------------------------------------------------------------------------
# 6. Post-stage secret-exclusion scan (D6, defense in depth). Fails the
#    ENTIRE backup closed if any staged file matches an excluded pattern --
#    a passing scan is required for success regardless of which component
#    it came from.
# ---------------------------------------------------------------------------
Write-Sect 'Post-stage secret-exclusion scan'
$secretHits = Get-ChildItem -Path $OutDir -Recurse -File | Where-Object { Test-IsSecretPath -LeafName $_.Name }
if ($secretHits -and $secretHits.Count -gt 0) {
    foreach ($hit in $secretHits) { Write-Fail "SECRET-PATTERN FILE STAGED: $($hit.FullName)" }
    $Components['secret_scan'] = @{ status = 'failed'; hits = @($secretHits | ForEach-Object { $_.FullName }) }
    $RequiredComponentFailed = $true
} else {
    Write-Ok 'No secret-pattern files found in the staged tree.'
    $Components['secret_scan'] = @{ status = 'ok' }
}

# ---------------------------------------------------------------------------
# 6b. Content-canary secret scan (D-R1 defect 4b, defense in depth beyond
#     filename matching). Reads the VALUES of an allowlisted set of known
#     local secret variable names out of .env.local/.env (never their
#     names alone -- an attacker-controlled config value could otherwise
#     collide with a name check), then scans every staged non-binary file
#     for those exact values. Never prints a matched value -- only the path
#     of the file it was found in.
# ---------------------------------------------------------------------------
Write-Sect 'Content-canary secret scan'
function Get-AllowlistedLocalSecretValues {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)
    $allowlistedNames = @(
        'MQK_OPERATOR_TOKEN', 'MQK_DATABASE_URL', 'MQK_RESTIC_REPOSITORY', 'MQK_RESTIC_PASSWORD_FILE',
        'ALPACA_API_KEY_ID', 'ALPACA_API_SECRET_KEY', 'ALPACA_API_KEY', 'ALPACA_API_SECRET',
        'B2_ACCOUNT_ID', 'B2_ACCOUNT_KEY', 'AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY'
    )
    $values = New-Object 'System.Collections.Generic.HashSet[string]'
    foreach ($candidate in @((Join-Path $RepoRoot '.env.local'), (Join-Path $RepoRoot '.env'))) {
        if (-not (Test-Path -LiteralPath $candidate)) { continue }
        foreach ($line in Get-Content -Path $candidate) {
            if ([string]::IsNullOrWhiteSpace($line)) { continue }
            $trimmed = $line.Trim()
            if ($trimmed.StartsWith('#')) { continue }
            $idx = $trimmed.IndexOf('=')
            if ($idx -lt 1) { continue }
            $name = $trimmed.Substring(0, $idx).Trim()
            $value = $trimmed.Substring($idx + 1).Trim()
            if (($value.StartsWith('"') -and $value.EndsWith('"')) -or ($value.StartsWith("'") -and $value.EndsWith("'"))) {
                if ($value.Length -ge 2) { $value = $value.Substring(1, $value.Length - 2) }
            }
            # A minimum length avoids a short/trivial value producing
            # spurious matches against unrelated staged content.
            if (($allowlistedNames -contains $name) -and ($value.Length -ge 8)) {
                [void]$values.Add($value)
            }
        }
    }
    return @($values)
}

$canaryValues = @(Get-AllowlistedLocalSecretValues -RepoRoot $RepoRoot)
$canaryHit = $false
$binaryExtensions = @('.dump', '.sqlite')
if ($canaryValues.Count -gt 0) {
    $textCandidates = Get-ChildItem -Path $OutDir -Recurse -File | Where-Object { $binaryExtensions -notcontains $_.Extension }
    foreach ($f in $textCandidates) {
        $content = Get-Content -Path $f.FullName -Raw -ErrorAction SilentlyContinue
        if ([string]::IsNullOrEmpty($content)) { continue }
        foreach ($val in $canaryValues) {
            if ($content.Contains($val)) {
                Write-Fail "SECRET CANARY MATCH in staged file (value withheld from log): $($f.FullName)"
                $canaryHit = $true
            }
        }
    }
}
if ($canaryHit) {
    $Components['secret_canary_scan'] = @{ status = 'failed' }
    $RequiredComponentFailed = $true
} else {
    Write-Ok "No allowlisted local secret value found in any staged non-binary file ($($canaryValues.Count) value(s) checked)."
    $Components['secret_canary_scan'] = @{ status = 'ok'; values_checked = $canaryValues.Count }
}

# ---------------------------------------------------------------------------
# 7. Manifest: SHA-256 + size for every staged file (D7), total backup-set
#    size (D16), and a documented (not yet enforced -- no restic repo
#    exists) retention policy (D10).
# ---------------------------------------------------------------------------
Write-Sect 'Manifest (content-addressed)'
$allFiles = Get-ChildItem -Path $OutDir -Recurse -File
$manifestFiles = @()
$totalBytes = 0
foreach ($f in $allFiles) {
    $hash = (Get-FileHash -Path $f.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
    $rel = $f.FullName.Substring($OutDir.TrimEnd('\').Length).TrimStart('\')
    $manifestFiles += [ordered]@{ path = $rel; sha256 = $hash; size_bytes = $f.Length }
    $totalBytes += $f.Length
}

# ---------------------------------------------------------------------------
# 8. restic (only if installed) -- D8/D9 proof, never fabricated.
# ---------------------------------------------------------------------------
Write-Sect 'restic (encryption / versioning)'
$resticCmd = Get-Command 'restic' -ErrorAction SilentlyContinue
$resticInfo = $null
if (-not $resticCmd) {
    Write-Warn 'restic not installed -- RESTIC_INSTALL_REQUIRED. Staged backup set above is still genuinely useful local recovery content.'
    $resticInfo = @{ status = 'not_installed'; reason = 'RESTIC_INSTALL_REQUIRED' }
} else {
    $resticVersion = (& restic version 2>&1) -join ' '
    $repoConfigured = -not [string]::IsNullOrWhiteSpace($env:MQK_RESTIC_REPOSITORY)
    $passwordConfigured = -not [string]::IsNullOrWhiteSpace($env:MQK_RESTIC_PASSWORD_FILE)
    if (-not $repoConfigured -or -not $passwordConfigured) {
        Write-Warn "restic is installed ($resticVersion) but MQK_RESTIC_REPOSITORY/MQK_RESTIC_PASSWORD_FILE are not both configured -- skipping (external prerequisite, not invented)."
        $resticInfo = @{ status = 'not_configured'; restic_version = $resticVersion }
    } else {
        Write-Warn 'restic + repository env vars are present but this script does not itself decide B2 vs local -- see Test-MiniQuantDeskRecoveryBackup.ps1 for the real snapshot/check/restore proof.'
        $resticInfo = @{ status = 'configured_not_run_by_this_script'; restic_version = $resticVersion }
    }
}

$manifest = [ordered]@{
    schema_version    = 'mqk-recovery-backup-manifest-v1'
    # This script's own authority is LOCAL staging only -- it never uploads
    # anywhere. Whatever the restic status below, this field must never read
    # as an offsite-backup success claim (D-R1 defect 7); a real offsite
    # result comes only from the operational restic snapshot/check workflow.
    overall_status    = 'LOCAL_RECOVERY_SET_STAGED'
    captured_at_utc   = (Get-Date).ToUniversalTime().ToString('o')
    staging_dir       = $OutDir
    components        = $Components
    files             = $manifestFiles
    total_size_bytes  = $totalBytes
    retention_policy  = [ordered]@{
        # Documented, conservative default. Not yet enforced anywhere (no
        # restic repository exists to prune) -- policy-validation only
        # until a real B2 destination is configured (D10).
        daily_snapshots  = 14
        weekly_snapshots = 8
        enforced         = $false
        enforcement_tool = 'restic forget --keep-daily 14 --keep-weekly 8 (future, requires a configured repository)'
    }
    restic            = $resticInfo
    required_component_failed = $RequiredComponentFailed
}
$manifestPath = Join-Path $OutDir 'manifest.json'
($manifest | ConvertTo-Json -Depth 8) | Set-Content -Path $manifestPath -Encoding UTF8
Write-Ok "Manifest written: $manifestPath"
Write-Ok "Total staged backup-set size: $totalBytes bytes ($([math]::Round($totalBytes/1MB,2)) MB)"

Write-Sect 'Summary'
if ($RequiredComponentFailed) {
    Write-Fail 'One or more REQUIRED components failed -- this backup set MUST NOT be reported as successful.'
    exit 1
}
Write-Ok 'LOCAL_RECOVERY_SET_STAGED: all REQUIRED components staged successfully.'
Write-Warn 'This is a LOCAL staging result only -- it is NOT an offsite/encrypted backup success claim.'
if (-not $resticCmd) {
    Write-Warn 'RESTIC_INSTALL_REQUIRED -- no encrypted/offsite step was performed.'
}
exit 0
