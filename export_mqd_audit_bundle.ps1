param(
    [string]$RepoRoot = (Get-Location).Path,
    [string]$OutParent = (Join-Path $env:USERPROFILE "Downloads"),
    [string]$BundleName = "",
    [string]$LinuxToolchainPath = "",
    [switch]$RunVerification,
    [switch]$SkipVendor
)

$ErrorActionPreference = "Stop"

function Require-Command {
    param([Parameter(Mandatory = $true)][string]$Name)
    if (-not (Get-Command $Name -ErrorAction SilentlyContinue)) {
        throw "Required command not found: $Name"
    }
}

function New-Dir {
    param([Parameter(Mandatory = $true)][string]$Path)
    if (-not (Test-Path -LiteralPath $Path)) {
        New-Item -ItemType Directory -Path $Path | Out-Null
    }
}

function Write-TextFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][string]$Content
    )
    $parent = Split-Path -Parent $Path
    if ($parent) { New-Dir -Path $parent }
    Set-Content -LiteralPath $Path -Value $Content -Encoding UTF8
}

function Invoke-LoggedExternal {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$WorkDir,
        [Parameter(Mandatory = $true)][string]$LogPath,
        [Parameter(Mandatory = $true)][string]$Exe,
        [string[]]$Args = @()
    )

    $header = @(
        "NAME: $Name"
        "WORKDIR: $WorkDir"
        "COMMAND: $Exe $($Args -join ' ')"
        "START_UTC: $([DateTime]::UtcNow.ToString('o'))"
        ""
    ) -join [Environment]::NewLine
    Write-TextFile -Path $LogPath -Content $header

    Push-Location $WorkDir
    try {
        & $Exe @Args *>> $LogPath
        $exitCode = $LASTEXITCODE
    }
    finally {
        Pop-Location
    }

    Add-Content -LiteralPath $LogPath -Value ""
    Add-Content -LiteralPath $LogPath -Value "END_UTC: $([DateTime]::UtcNow.ToString('o'))"
    Add-Content -LiteralPath $LogPath -Value "EXIT_CODE: $exitCode"
    return $exitCode
}

function Copy-RepoSnapshot {
    param(
        [Parameter(Mandatory = $true)][string]$Source,
        [Parameter(Mandatory = $true)][string]$Dest
    )

    New-Dir -Path $Dest

    $excludeDirs = @(
        ".git",
        "target",
        "node_modules",
        ".next",
        ".turbo",
        ".venv",
        "venv",
        "__pycache__",
        ".pytest_cache",
        ".mypy_cache",
        ".ruff_cache",
        ".cache",
        "dist",
        "build",
        "coverage",
        "htmlcov",
        # SEC-SNAPSHOT-01: exclude generated proof output and AI tooling scratch dirs that
        # may contain session-local or secret-bearing state.
        ".proof",
        ".claude",
        ".claude-worktrees"
    )

    # SEC-SNAPSHOT-01: broadly exclude all .env / .env.* files from robocopy.
    # This catches .env, .env.local, .env.paper, .env.live, .env.production,
    # .env.development, .env.staging, .env.local.backup, and any future .env.* variants.
    # Safe example/template files (.env.local.example, .env.example) are also matched by
    # .env.* and are therefore excluded here; they are explicitly copied back after the
    # robocopy call so the bundle still contains the safe placeholder-only templates.
    $excludeFiles = @(
        "*.pyc",
        "*.pyo",
        "*.pyd",
        "*.tmp",
        "*.log",
        ".env",
        ".env.*",
        "secrets.env",
        "secrets.local",
        "*.pem",
        "*.key"
    )

    $args = @(
        $Source,
        $Dest,
        "/E",
        "/R:1",
        "/W:1",
        "/NFL",
        "/NDL",
        "/NP",
        "/NJH",
        "/NJS",
        "/XD"
    ) + $excludeDirs + @("/XF") + $excludeFiles

    & robocopy @args | Out-Null
    $code = $LASTEXITCODE

    if ($code -gt 7) {
        throw "robocopy failed with exit code $code"
    }
}

Require-Command -Name "git"
Require-Command -Name "robocopy"

if (-not $SkipVendor -or $RunVerification) {
    Require-Command -Name "cargo"
}

$RepoRoot = (Resolve-Path -LiteralPath $RepoRoot).Path
if (-not (Test-Path -LiteralPath (Join-Path $RepoRoot ".git"))) {
    throw "RepoRoot does not look like a git repo: $RepoRoot"
}

$repoName = Split-Path -Leaf $RepoRoot
$stamp = Get-Date -Format "yyyyMMdd_HHmmss"

if ([string]::IsNullOrWhiteSpace($BundleName)) {
    $BundleName = "${repoName}_audit_bundle_$stamp"
}

$bundleRoot = Join-Path $OutParent $BundleName
$repoCopy   = Join-Path $bundleRoot "repo"
$logsDir    = Join-Path $bundleRoot "logs"
$metaDir    = Join-Path $bundleRoot "meta"

New-Dir -Path $bundleRoot
New-Dir -Path $logsDir
New-Dir -Path $metaDir

Write-Host "Creating audit bundle at: $bundleRoot"

# ----------------------------
# Metadata capture
# ----------------------------
Write-Host "Capturing git metadata..."

$commit = (git -C $RepoRoot rev-parse HEAD).Trim()
Write-TextFile -Path (Join-Path $metaDir "AUDIT_COMMIT.txt") -Content $commit

git -C $RepoRoot status --short --branch | Set-Content -LiteralPath (Join-Path $metaDir "GIT_STATUS.txt") -Encoding UTF8
git -C $RepoRoot diff --stat | Set-Content -LiteralPath (Join-Path $metaDir "GIT_DIFF_STAT.txt") -Encoding UTF8
git -C $RepoRoot diff | Set-Content -LiteralPath (Join-Path $metaDir "WORKTREE.diff") -Encoding UTF8
git -C $RepoRoot ls-files --others --exclude-standard | Set-Content -LiteralPath (Join-Path $metaDir "UNTRACKED_FILES.txt") -Encoding UTF8

try {
    rustc -Vv | Set-Content -LiteralPath (Join-Path $metaDir "RUSTC_VERSION.txt") -Encoding UTF8
}
catch {
    Write-TextFile -Path (Join-Path $metaDir "RUSTC_VERSION.txt") -Content "rustc not available in PATH"
}

try {
    cargo -Vv | Set-Content -LiteralPath (Join-Path $metaDir "CARGO_VERSION.txt") -Encoding UTF8
}
catch {
    Write-TextFile -Path (Join-Path $metaDir "CARGO_VERSION.txt") -Content "cargo not available in PATH"
}

try {
    node -v | Set-Content -LiteralPath (Join-Path $metaDir "NODE_VERSION.txt") -Encoding UTF8
}
catch {
    Write-TextFile -Path (Join-Path $metaDir "NODE_VERSION.txt") -Content "node not available in PATH"
}

try {
    npm -v | Set-Content -LiteralPath (Join-Path $metaDir "NPM_VERSION.txt") -Encoding UTF8
}
catch {
    Write-TextFile -Path (Join-Path $metaDir "NPM_VERSION.txt") -Content "npm not available in PATH"
}

$commandsToRun = @"
# Run from bundle root: repo\
cargo build --workspace
cargo test -p mqk-daemon --test scenario_gui_daemon_contract_gate
cargo test -p mqk-daemon --test scenario_daemon_routes
cargo test -p mqk-backtest
cargo test -p mqk-artifacts
cargo test -p mqk-promotion
cargo clippy --workspace --all-targets -- -D warnings

# Optional GUI lane
# cd core-rs\mqk-gui
# npm test
# npm run build
"@
Write-TextFile -Path (Join-Path $bundleRoot "COMMANDS_TO_RUN.txt") -Content $commandsToRun

$envNotes = @"
# ENV_NOTES

Commit:
$commit

Repo root inside bundle:
repo\

What this bundle includes:
- full repo snapshot copied from working tree (including uncommitted tracked changes and untracked files)
- git metadata
- optional vendored Cargo dependencies inside repo\vendor
- optional local verification logs under logs\
- optional Linux Rust toolchain copy under rust-toolchain-linux\ if provided

Secret / snapshot hygiene (SEC-SNAPSHOT-01):
- All .env and .env.* files are EXCLUDED from the robocopy pass (.env, .env.local,
  .env.paper, .env.live, .env.production, .env.development, .env.staging, .env.local.backup,
  and any other .env.* variants). Never share any real .env.* file.
- secrets.env, secrets.local, *.pem, and *.key are excluded.
- .env.local.example and .env.example are safe placeholder-only templates; they are
  explicitly copied back into the bundle after the exclusion pass.
- .proof/ and .claude/ dirs are excluded (may contain session-local or secret-bearing state).
- If a real secret file was ever accidentally bundled, rotate all broker keys, API keys,
  operator tokens, and webhooks before running broker-connected sessions again.

Important:
- Windows Rust toolchain binaries are NOT useful for Linux audit execution.
- To let ChatGPT run cargo off-machine in a Linux container, provide a Linux x86_64 Rust toolchain path with -LinuxToolchainPath.
- If tests require DB/services, note them here before upload.

Local machine notes:
- Update this file with any DB assumptions, skipped lanes, env vars, or known unrelated failures.
"@
Write-TextFile -Path (Join-Path $bundleRoot "ENV_NOTES.md") -Content $envNotes

# ----------------------------
# Repo snapshot copy
# ----------------------------
Write-Host "Copying repo snapshot..."
Copy-RepoSnapshot -Source $RepoRoot -Dest $repoCopy

# SEC-SNAPSHOT-01: re-include safe env example/template files excluded by the broad .env.*
# pattern above. These files contain placeholder names only — no real credentials.
# Never add a real .env.local or any file that holds actual keys/tokens to this list.
$safeEnvExamples = @('.env.local.example', '.env.example')
foreach ($safeFile in $safeEnvExamples) {
    $srcFile = Join-Path $RepoRoot $safeFile
    $dstFile = Join-Path $repoCopy $safeFile
    if ((Test-Path -LiteralPath $srcFile) -and -not (Test-Path -LiteralPath $dstFile)) {
        Copy-Item -LiteralPath $srcFile -Destination $dstFile
        Write-Host "Restored safe env example to bundle: $safeFile" -ForegroundColor Cyan
    }
}

# ----------------------------
# Vendor dependencies offline
# ----------------------------
if (-not $SkipVendor) {
    Write-Host "Vendoring Cargo dependencies inside copied repo..."
    $cargoConfigDir = Join-Path $repoCopy ".cargo"
    New-Dir -Path $cargoConfigDir

    $vendorConfig = Join-Path $cargoConfigDir "config.toml"
    $vendorLog    = Join-Path $logsDir "cargo_vendor.stderr.log"

    Push-Location $repoCopy
    try {
        & cargo vendor 1> $vendorConfig 2> $vendorLog
        if ($LASTEXITCODE -ne 0) {
            throw "cargo vendor failed with exit code $LASTEXITCODE. See $vendorLog"
        }
    }
    finally {
        Pop-Location
    }
}
else {
    Write-Host "Skipping cargo vendor because -SkipVendor was set."
}

# ----------------------------
# Optional Linux toolchain copy
# ----------------------------
if (-not [string]::IsNullOrWhiteSpace($LinuxToolchainPath)) {
    $resolvedLinuxPath = (Resolve-Path -LiteralPath $LinuxToolchainPath).Path
    $toolchainDest = Join-Path $bundleRoot "rust-toolchain-linux"
    Write-Host "Copying Linux Rust toolchain from: $resolvedLinuxPath"
    Copy-RepoSnapshot -Source $resolvedLinuxPath -Dest $toolchainDest
}
else {
    Write-Host "No Linux toolchain path provided. Bundle will still be useful, but off-machine cargo execution may be blocked."
}

# ----------------------------
# Optional local verification logs
# ----------------------------
if ($RunVerification) {
    Write-Host "Running local Rust verification and capturing logs..."

    $verification = @(
        @{ Name = "cargo_build_workspace"; Args = @("build", "--workspace") },
        @{ Name = "cargo_test_gui_daemon_contract_gate"; Args = @("test", "-p", "mqk-daemon", "--test", "scenario_gui_daemon_contract_gate") },
        @{ Name = "cargo_test_daemon_routes"; Args = @("test", "-p", "mqk-daemon", "--test", "scenario_daemon_routes") },
        @{ Name = "cargo_test_mqk_backtest"; Args = @("test", "-p", "mqk-backtest") },
        @{ Name = "cargo_test_mqk_artifacts"; Args = @("test", "-p", "mqk-artifacts") },
        @{ Name = "cargo_test_mqk_promotion"; Args = @("test", "-p", "mqk-promotion") },
        @{ Name = "cargo_clippy_workspace"; Args = @("clippy", "--workspace", "--all-targets", "--", "-D", "warnings") }
    )

    $summary = New-Object System.Collections.Generic.List[string]

    foreach ($item in $verification) {
        $logPath = Join-Path $logsDir ("{0}.log" -f $item.Name)
        $exit = Invoke-LoggedExternal -Name $item.Name -WorkDir $repoCopy -LogPath $logPath -Exe "cargo" -Args $item.Args
        $summary.Add(("{0}: EXIT_CODE={1}" -f $item.Name, $exit))
    }

    Write-TextFile -Path (Join-Path $logsDir "VERIFICATION_SUMMARY.txt") -Content ($summary -join [Environment]::NewLine)
}

Write-Host ""
Write-Host "DONE"
Write-Host "Bundle: $bundleRoot"
Write-Host "Upload that folder zipped."
Write-Host ""
Write-Host "Best-case upload contents:"
Write-Host "- repo\"
Write-Host "- repo\vendor\"
Write-Host "- repo\.cargo\config.toml"
Write-Host "- rust-toolchain-linux\   (optional but strongly recommended)"
Write-Host "- logs\                   (if -RunVerification was used)"
Write-Host "- meta\"
Write-Host "- COMMANDS_TO_RUN.txt"
Write-Host "- ENV_NOTES.md"