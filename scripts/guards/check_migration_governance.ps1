$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$CoreRsDir = Join-Path $RepoRoot "core-rs"
$AuthoritativeDir = (Resolve-Path (Join-Path $CoreRsDir "crates/mqk-db/migrations")).Path.TrimEnd('\')
$ManifestPath = Join-Path $AuthoritativeDir "manifest.json"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan }

Write-Host "============================================================"
Write-Host " MQK Migration Governance Guard (PowerShell)"
Write-Host " Repo root: $RepoRoot"
Write-Host "============================================================"

if (-not (Test-Path $ManifestPath)) {
    Show-Red "Migration manifest missing: $ManifestPath"
    exit 1
}

Write-Host ""
Show-Info "--- [A] Unmanaged migration directories under core-rs/ ---"

$ExtraMigrationDirs = @(
    Get-ChildItem -Path $CoreRsDir -Recurse -Directory |
        Where-Object {
            $_.Name -eq "migrations" -and
            $_.FullName.TrimEnd('\') -ne $AuthoritativeDir
        } |
        ForEach-Object { $_.FullName.Substring($RepoRoot.Length + 1) } |
        Sort-Object
)

if ($ExtraMigrationDirs.Count -eq 0) {
    Show-Green "  OK -- no unmanaged migration directories under core-rs/"
} else {
    $Violations += $ExtraMigrationDirs.Count
    Show-Red "  FAIL -- unmanaged migration directories detected:"
    $ExtraMigrationDirs | ForEach-Object { Write-Host "  $_" }
}

Write-Host ""
Show-Info "--- [B] Authoritative manifest drift ---"

$Manifest = Get-Content $ManifestPath -Raw | ConvertFrom-Json
$ManifestPaths = @($Manifest.migrations | ForEach-Object { $_.path.Replace('\', '/') })
$ManifestDuplicatePaths = @(
    $ManifestPaths |
        Group-Object |
        Where-Object { $_.Count -gt 1 } |
        ForEach-Object { $_.Name } |
        Sort-Object
)

$ManifestUniquePaths = @($ManifestPaths | Sort-Object -Unique)
$ActualSqlPaths = @(
    Get-ChildItem -Path $AuthoritativeDir -Recurse -File -Filter "*.sql" |
        ForEach-Object { $_.FullName.Substring($AuthoritativeDir.Length + 1).Replace('\', '/') } |
        Sort-Object
)

if ($ManifestDuplicatePaths.Count -gt 0) {
    $Violations += $ManifestDuplicatePaths.Count
    Show-Red "  FAIL -- manifest.json contains duplicate migration path entries:"
    $ManifestDuplicatePaths | ForEach-Object { Write-Host "  $_" }
}

$Drift = Compare-Object -ReferenceObject $ManifestUniquePaths -DifferenceObject $ActualSqlPaths
if ($Drift) {
    $Violations += 1
    Show-Red "  FAIL -- manifest.json does not match authoritative SQL files:"
    $Drift | ForEach-Object { Write-Host "  $($_.SideIndicator) $($_.InputObject)" }
} else {
    Show-Green "  OK -- manifest.json matches the authoritative SQL chain"
}

Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " MIGRATION GOVERNANCE GUARD PASSED."
    exit 0
} else {
    Show-Red " MIGRATION GOVERNANCE GUARD FAILED -- $Violations violation(s) found."
    exit 1
}

