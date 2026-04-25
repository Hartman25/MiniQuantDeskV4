[CmdletBinding()]
param(
    [Parameter(Mandatory = $false)]
    [ValidateSet('check')]
    [string]$Mode = 'check',

    [Parameter(Mandatory = $false)]
    [string]$ManifestPath = '.\\core-rs\\Cargo.toml'
)

$ErrorActionPreference = 'Stop'

if (-not (Test-Path $ManifestPath)) {
    throw "Manifest path not found: $ManifestPath"
}

$resolvedManifest = (Resolve-Path $ManifestPath).Path
$workspaceRoot = Split-Path -Parent $resolvedManifest

Push-Location $workspaceRoot
try {
    $metaJson = cargo metadata --manifest-path $resolvedManifest --format-version 1 --no-deps
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo metadata failed'
    }

    $meta = $metaJson | ConvertFrom-Json
    $workspace = @{}
    foreach ($id in $meta.workspace_members) {
        $workspace[$id] = $true
    }

    $packages = $meta.packages | Where-Object { $workspace.ContainsKey($_.id) } | Sort-Object name
    foreach ($p in $packages) {
        Write-Host "Running clippy for $($p.name)..." -ForegroundColor Cyan
        cargo clippy --manifest-path $resolvedManifest -p $p.name --all-targets -- -D warnings
        if ($LASTEXITCODE -ne 0) {
            exit $LASTEXITCODE
        }
    }

    Write-Host "clippy passed for all $($packages.Count) workspace packages (Windows per-package)." -ForegroundColor Green
}
finally {
    Pop-Location
}
