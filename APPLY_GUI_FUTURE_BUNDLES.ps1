param(
    [string]$RepoRoot = ".",
    [string[]]$Bundles = @(
        ".\gui_future_bundle.zip",
        ".\gui_future_bundle_phase2.zip",
        ".\gui_future_bundle_phase3.zip",
        ".\gui_future_bundle_phase4.zip",
        ".\gui_future_bundle_phase5.zip",
        ".\gui_future_bundle_phase6.zip"
    )
)

$ErrorActionPreference = "Stop"
$repo = Resolve-Path $RepoRoot
$tempRoot = Join-Path $repo "__gui_future_merge_temp"

if (Test-Path $tempRoot) {
    Remove-Item $tempRoot -Recurse -Force
}
New-Item -ItemType Directory -Path $tempRoot | Out-Null

foreach ($bundle in $Bundles) {
    if (-not (Test-Path $bundle)) {
        Write-Host "Skipping missing bundle: $bundle"
        continue
    }

    $name = [System.IO.Path]::GetFileNameWithoutExtension($bundle)
    $dest = Join-Path $tempRoot $name
    Expand-Archive -Path $bundle -DestinationPath $dest -Force

    Get-ChildItem -Path $dest -Recurse -File | ForEach-Object {
        $relative = $_.FullName.Substring($dest.Length).TrimStart('\')
        if ($relative -like "core-rs\mqk-gui\src\*") {
            $target = Join-Path $repo $relative
            $targetDir = Split-Path $target -Parent
            if (-not (Test-Path $targetDir)) {
                New-Item -ItemType Directory -Path $targetDir -Force | Out-Null
            }
            Copy-Item $_.FullName $target -Force
            Write-Host "Applied $relative"
        }
    }
}

Write-Host ""
Write-Host "Done. Next:"
Write-Host "cd core-rs\mqk-gui"
Write-Host "npm run build"
