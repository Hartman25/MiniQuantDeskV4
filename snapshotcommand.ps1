$ErrorActionPreference = "Stop"

$repo   = (Get-Location).Path
$stamp  = Get-Date -Format "yyyyMMdd_HHmmss"
$stage  = Join-Path $env:TEMP "MiniQuantDesk_snapshot_$stamp"
$zip    = Join-Path $repo "MiniQuantDesk_worktree_snapshot_$stamp.zip"

if (Test-Path $stage) { Remove-Item $stage -Recurse -Force }
New-Item -ItemType Directory -Path $stage | Out-Null

robocopy $repo $stage /MIR `
  /XD .git target node_modules dist build .venv venv __pycache__ .pytest_cache .mypy_cache .idea .vscode .next .turbo .cache `
  /XF *.pyc *.pyo *.pyd *.obj *.o *.dll *.exe *.so *.dylib *.zip

if ($LASTEXITCODE -ge 8) {
    throw "robocopy failed with exit code $LASTEXITCODE"
}

if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path (Join-Path $stage '*') -DestinationPath $zip -CompressionLevel Optimal

Write-Host ""
Write-Host "Created snapshot zip:"
Write-Host $zip