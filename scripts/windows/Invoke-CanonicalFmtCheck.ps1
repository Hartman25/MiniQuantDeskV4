# =============================================================================
# Invoke-CanonicalFmtCheck.ps1
#
# The one `cargo fmt --check` authority for this repo, shared by CI's
# `windows` job (.github/workflows/ci.yml) and local verification
# (full_repo_proof.ps1). Before this script existed, CI ran a bare
# `cargo fmt --check` directly while full_repo_proof.ps1 ran a different,
# per-package WINPATH-01 workaround inline -- two independent
# implementations of "the format check" that could silently drift apart.
# Both callers now invoke this single script.
#
# WINPATH-01: on Windows, `cargo fmt --all --check` batches every workspace
# source file into one rustfmt invocation. On this repo (452+ files) the
# combined command line exceeds Windows' CreateProcess 32767-char limit.
# cargo canonicalizes all source paths to `\\?\C:\...` before building the
# command, so path-shortening (subst, CARGO_TARGET_DIR) cannot reduce this.
# Checking each workspace package individually is identical in substance to
# `--all --check`, just split across multiple invocations to stay under the
# limit.
#
# Usage:
#   pwsh -File scripts\windows\Invoke-CanonicalFmtCheck.ps1 `
#       -RepoRoot <path> -CargoManifest <path to core-rs/Cargo.toml>
#
# Exit codes: 0 = clean, 1 = format drift or a tooling failure.
# =============================================================================

param(
    [string]$RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot "..\..")).Path,
    [string]$CargoManifest = (Join-Path $RepoRoot "core-rs\Cargo.toml")
)

$ErrorActionPreference = "Stop"

function Resolve-CargoExe {
    $cmd = Get-Command cargo -ErrorAction SilentlyContinue
    if (-not $cmd) {
        Write-Error "cargo not found on PATH."
        exit 1
    }
    return $cmd.Source
}

$CargoExe = Resolve-CargoExe
$IsWindowsPlatform = $env:OS -eq "Windows_NT" -or $IsWindows

Write-Host "============================================================"
Write-Host " Canonical cargo fmt --check"
Write-Host " Repo root:       $RepoRoot"
Write-Host " Cargo manifest:  $CargoManifest"
Write-Host " Platform lane:   $(if ($IsWindowsPlatform) { 'Windows (per-package, WINPATH-01)' } else { 'non-Windows (--all)' })"
Write-Host "============================================================"

if ($IsWindowsPlatform) {
    $metadataJson = (& $CargoExe metadata --format-version 1 --no-deps --manifest-path $CargoManifest 2>$null) -join ''
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($metadataJson)) {
        Write-Error "cargo metadata failed; cannot enumerate workspace packages for fmt check."
        exit 1
    }
    $packageNames = @(($metadataJson | ConvertFrom-Json).packages | ForEach-Object { $_.name })
    if ($packageNames.Count -eq 0) {
        Write-Error "cargo metadata returned no packages for fmt check."
        exit 1
    }

    $failed = $false
    foreach ($pkg in $packageNames) {
        & $CargoExe fmt --manifest-path $CargoManifest -p $pkg -- --check
        if ($LASTEXITCODE -ne 0) {
            $failed = $true
        }
    }
    if ($failed) {
        Write-Host ""
        Write-Error "cargo fmt --check failed for one or more of the $($packageNames.Count) workspace packages (see rustfmt diff output above)."
        exit 1
    }
    Write-Host "cargo fmt --check passed for all $($packageNames.Count) workspace packages (Windows per-package; WINPATH-01)." -ForegroundColor Green
    exit 0
} else {
    & $CargoExe fmt --manifest-path $CargoManifest --all -- --check
    if ($LASTEXITCODE -ne 0) {
        Write-Error "cargo fmt --check failed (see rustfmt diff output above)."
        exit 1
    }
    Write-Host "cargo fmt --check passed." -ForegroundColor Green
    exit 0
}
