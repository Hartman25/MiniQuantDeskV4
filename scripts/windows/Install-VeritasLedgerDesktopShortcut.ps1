[CmdletBinding()]
param(
    [switch]$Rebuild,
    [string]$ObserveShortcutName = 'Veritas Ledger.lnk',
    [string]$TradeReadyShortcutName = 'Veritas Ledger (Trade Ready).lnk'
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function New-VeritasShortcut {
    param(
        [Parameter(Mandatory = $true)][string]$ShortcutPath,
        [Parameter(Mandatory = $true)][string]$TargetPath,
        [Parameter(Mandatory = $true)][string]$Arguments,
        [Parameter(Mandatory = $true)][string]$WorkingDirectory,
        [Parameter(Mandatory = $true)][string]$Description,
        [string]$IconPath
    )

    $wsh = New-Object -ComObject WScript.Shell
    $shortcut = $wsh.CreateShortcut($ShortcutPath)
    $shortcut.TargetPath = $TargetPath
    $shortcut.Arguments = $Arguments
    $shortcut.WorkingDirectory = $WorkingDirectory
    $shortcut.Description = $Description
    if ($IconPath -and (Test-Path $IconPath)) {
        $shortcut.IconLocation = $IconPath
    }
    $shortcut.Save()
}

$repoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$launcher = (Resolve-Path (Join-Path $PSScriptRoot 'Launch-VeritasLedger.ps1')).Path
$desktop = [Environment]::GetFolderPath('Desktop')
$observeShortcutPath = Join-Path $desktop $ObserveShortcutName
$tradeReadyShortcutPath = Join-Path $desktop $TradeReadyShortcutName
$iconPath = Join-Path $repoRoot 'assets\logo\veritas_ledger_shield.ico'

if (-not (Test-Path $launcher)) {
    throw "Launcher script not found: $launcher"
}

$targetPath = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
if (-not (Test-Path $targetPath)) {
    throw "powershell.exe not found at expected path: $targetPath"
}

$rebuildArg = if ($Rebuild.IsPresent) { ' -Rebuild' } else { '' }
$observeArguments = "-NoProfile -ExecutionPolicy Bypass -File `"$launcher`" -Mode Observe$rebuildArg"
$tradeReadyArguments = "-NoProfile -ExecutionPolicy Bypass -File `"$launcher`" -Mode TradeReady$rebuildArg"

New-VeritasShortcut `
    -ShortcutPath $observeShortcutPath `
    -TargetPath $targetPath `
    -Arguments $observeArguments `
    -WorkingDirectory $repoRoot `
    -Description 'Launch Veritas Ledger in observe/attach mode against the verified canonical local paper+alpaca daemon. Opens for inspection even when not currently trade-ready; never auto-starts runtime.' `
    -IconPath $iconPath

New-VeritasShortcut `
    -ShortcutPath $tradeReadyShortcutPath `
    -TargetPath $targetPath `
    -Arguments $tradeReadyArguments `
    -WorkingDirectory $repoRoot `
    -Description 'Launch Veritas Ledger in trade-ready mode against the verified canonical local paper+alpaca daemon. Refuses to open unless mounted truth says the backend is start-capable; never auto-starts runtime.' `
    -IconPath $iconPath

Write-Host "Created desktop shortcut: $observeShortcutPath" -ForegroundColor Green
Write-Host "Created desktop shortcut: $tradeReadyShortcutPath" -ForegroundColor Green
if (Test-Path $iconPath) {
    Write-Host "Shortcut icon: $iconPath" -ForegroundColor DarkGray
}
Write-Host 'Observe/Attach mode verifies canonical backend identity and auth posture, then opens the GUI even when trade readiness is fail-closed.' -ForegroundColor Cyan
Write-Host 'Trade-Ready mode verifies the same identity plus full mounted readiness truth before opening the GUI. Neither shortcut auto-starts runtime.' -ForegroundColor Cyan
