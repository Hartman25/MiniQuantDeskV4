param(
    [Parameter(Mandatory = $false)]
    [ValidateSet('check', 'format')]
    [string]$Mode = 'check',

    [Parameter(Mandatory = $false)]
    [string]$ManifestPath = '.\core-rs\Cargo.toml'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) {
    throw 'cargo was not found in PATH.'
}

$resolvedManifest = (Resolve-Path $ManifestPath).Path
$workspaceRoot = Split-Path -Parent $resolvedManifest

Push-Location $workspaceRoot
try {
    $meta = cargo metadata --format-version 1 --no-deps | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) {
        throw 'cargo metadata failed.'
    }

    $workspaceMembers = @{}
    foreach ($id in $meta.workspace_members) {
        $workspaceMembers[$id] = $true
    }

    $packages = $meta.packages |
        Where-Object { $workspaceMembers.ContainsKey($_.id) } |
        Sort-Object name

    if (-not $packages -or $packages.Count -eq 0) {
        throw 'No workspace packages were found.'
    }

    foreach ($pkg in $packages) {
        if ($Mode -eq 'format') {
            Write-Host "Formatting $($pkg.name)..." -ForegroundColor Cyan
            cargo fmt -p $pkg.name
        }
        else {
            Write-Host "Checking fmt for $($pkg.name)..." -ForegroundColor Cyan
            cargo fmt -p $pkg.name -- --check
        }

        if ($LASTEXITCODE -ne 0) {
            exit $LASTEXITCODE
        }
    }

    if ($Mode -eq 'format') {
        Write-Host 'Workspace formatting completed.' -ForegroundColor Green
    }
    else {
        Write-Host 'Workspace formatting check passed.' -ForegroundColor Green
    }
}
finally {
    Pop-Location
}
