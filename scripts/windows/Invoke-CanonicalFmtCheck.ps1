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
# source file into one rustfmt invocation, and even `cargo fmt -p <package>`
# exceeds Windows' CreateProcess 32767-char limit for the largest packages
# (mqk-daemon has hundreds of integration-test targets). cargo canonicalizes
# all source paths to `\\?\C:\...` before building the command, so
# path-shortening (subst, CARGO_TARGET_DIR) cannot reduce this.
# The Windows lane therefore enumerates every workspace target root from
# `cargo metadata` (the same set `cargo fmt --all` formats: lib, bin, test,
# example and bench roots, each with its package edition) and runs the pinned
# `rustfmt --check` over them in deterministic batches bounded well below the
# command-line limit. rustfmt follows `mod` declarations from each root, so
# coverage is identical to `cargo fmt --all --check`.
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
Write-Host " Platform lane:   $(if ($IsWindowsPlatform) { 'Windows (bounded rustfmt batches, WINPATH-01)' } else { 'non-Windows (--all)' })"
Write-Host "============================================================"

if ($IsWindowsPlatform) {
    # Bound on the summed argument length of one rustfmt invocation. Kept far
    # below the 32767-char CreateProcess limit (exe path + quoting overhead).
    $MaxBatchChars = 16000

    $RustfmtCmd = Get-Command rustfmt -ErrorAction SilentlyContinue
    if (-not $RustfmtCmd) {
        Write-Error "rustfmt not found on PATH."
        exit 1
    }
    $RustfmtExe = $RustfmtCmd.Source

    $metadataJson = (& $CargoExe metadata --format-version 1 --no-deps --manifest-path $CargoManifest 2>$null) -join ''
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($metadataJson)) {
        Write-Error "cargo metadata failed; cannot enumerate workspace targets for fmt check."
        exit 1
    }
    $packages = @(($metadataJson | ConvertFrom-Json).packages)
    if ($packages.Count -eq 0) {
        Write-Error "cargo metadata returned no packages for fmt check."
        exit 1
    }

    # (edition, source root) pairs, de-duplicated by path, in a stable order.
    $seen = @{}
    $roots = [System.Collections.Generic.List[object]]::new()
    foreach ($pkg in $packages) {
        foreach ($target in @($pkg.targets)) {
            $src = [string]$target.src_path
            if ([string]::IsNullOrWhiteSpace($src) -or -not (Test-Path -LiteralPath $src -PathType Leaf)) {
                Write-Error "workspace target '$($target.name)' of package '$($pkg.name)' has no readable source root ('$src'); refusing to skip it."
                exit 1
            }
            $key = $src.ToLowerInvariant()
            if ($seen.ContainsKey($key)) { continue }
            $seen[$key] = $true
            $roots.Add([pscustomobject]@{ Edition = [string]$pkg.edition; Path = $src })
        }
    }
    if ($roots.Count -eq 0) {
        Write-Error "cargo metadata returned no formattable targets for fmt check."
        exit 1
    }
    $sortedRoots = @($roots | Sort-Object Edition, Path)

    $batches = [System.Collections.Generic.List[object]]::new()
    foreach ($group in ($sortedRoots | Group-Object Edition)) {
        $current = [System.Collections.Generic.List[string]]::new()
        $chars = 0
        foreach ($root in $group.Group) {
            $cost = $root.Path.Length + 3
            if ($current.Count -gt 0 -and ($chars + $cost) -gt $MaxBatchChars) {
                $batches.Add([pscustomobject]@{ Edition = $group.Name; Paths = $current.ToArray() })
                $current = [System.Collections.Generic.List[string]]::new()
                $chars = 0
            }
            $current.Add($root.Path)
            $chars += $cost
        }
        if ($current.Count -gt 0) {
            $batches.Add([pscustomobject]@{ Edition = $group.Name; Paths = $current.ToArray() })
        }
    }

    $failedBatches = 0
    $checkedRoots = 0
    # Run from the workspace directory so rustup resolves the pinned toolchain
    # from core-rs/rust-toolchain.toml, exactly as `cargo fmt` does.
    Push-Location (Split-Path -Parent $CargoManifest)
    try {
        foreach ($batch in $batches) {
            & $RustfmtExe --edition $batch.Edition --check @($batch.Paths)
            if ($LASTEXITCODE -ne 0) {
                $failedBatches++
            }
            $checkedRoots += $batch.Paths.Count
        }
    } finally {
        Pop-Location
    }
    if ($failedBatches -ne 0) {
        Write-Host ""
        Write-Error "rustfmt --check failed in $failedBatches of $($batches.Count) batches (see rustfmt diff/error output above)."
        exit 1
    }
    Write-Host "rustfmt --check passed for all $checkedRoots workspace target roots in $($batches.Count) bounded batches (Windows; WINPATH-01)." -ForegroundColor Green
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
