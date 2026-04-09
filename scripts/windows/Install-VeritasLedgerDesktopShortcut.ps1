[CmdletBinding()]
param(
    [switch]$Rebuild
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Paths
# ---------------------------------------------------------------------------
$repoRoot   = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
$launcher   = (Resolve-Path (Join-Path $PSScriptRoot 'Launch-VeritasLedger.ps1')).Path
$desktop    = [Environment]::GetFolderPath('Desktop')
$iconPath   = Join-Path $repoRoot 'assets\logo\veritas_ledger_shield.ico'
$desktopFile = Join-Path $desktop 'Veritas Ledger.veritas'

# Old .lnk shortcuts from the previous two-shortcut layout — removed on install.
$oldLnks = @(
    (Join-Path $desktop 'Veritas Ledger.lnk'),
    (Join-Path $desktop 'Veritas Ledger (Trade Ready).lnk')
)

if (-not (Test-Path $launcher)) {
    throw "Launcher script not found: $launcher"
}

$psExe = Join-Path $env:SystemRoot 'System32\WindowsPowerShell\v1.0\powershell.exe'
if (-not (Test-Path $psExe)) {
    throw "powershell.exe not found at expected path: $psExe"
}

# ---------------------------------------------------------------------------
# Command strings
# ---------------------------------------------------------------------------
$rebuildSuffix     = if ($Rebuild.IsPresent) { ' -Rebuild' } else { '' }
$observeCommand    = "`"$psExe`" -NoProfile -ExecutionPolicy Bypass -File `"$launcher`" -Mode Observe$rebuildSuffix"
$tradeReadyCommand = "`"$psExe`" -NoProfile -ExecutionPolicy Bypass -File `"$launcher`" -Mode TradeReady$rebuildSuffix"
$iconLocation      = if (Test-Path $iconPath) { "$iconPath,0" } else { "$psExe,0" }

# ---------------------------------------------------------------------------
# Register custom .veritas file type in HKCU (no admin required)
#
# Layout:
#   HKCU\Software\Classes\.veritas             -> ProgID
#   HKCU\Software\Classes\<ProgID>             -> friendly name, NeverShowExt
#   HKCU\Software\Classes\<ProgID>\DefaultIcon -> icon
#   HKCU\Software\Classes\<ProgID>\shell       -> default verb = open
#   HKCU\Software\Classes\<ProgID>\shell\open\command       -> Observe
#   HKCU\Software\Classes\<ProgID>\shell\tradeready         -> right-click label
#   HKCU\Software\Classes\<ProgID>\shell\tradeready\command -> TradeReady
#
# Double-click invokes the "open" verb -> Observe (idle-only, no POSTs).
# Right-click shows "Open in Trade Ready mode" -> TradeReady (Bearer round-trip).
# ---------------------------------------------------------------------------
$progId      = 'VeritasLedger.launcher.1'
$classesRoot = 'HKCU:\Software\Classes'
$typeRoot    = "$classesRoot\$progId"

function Set-RegDefault {
    param(
        [Parameter(Mandatory = $true)][string]$KeyPath,
        [Parameter(Mandatory = $true)][string]$Value
    )
    if (-not (Test-Path $KeyPath)) {
        New-Item -Path $KeyPath -Force | Out-Null
    }
    Set-ItemProperty -Path $KeyPath -Name '(Default)' -Value $Value
}

# .veritas extension -> ProgID
Set-RegDefault -KeyPath "$classesRoot\.veritas" -Value $progId

# ProgID root: friendly name + suppress extension display in Explorer
Set-RegDefault -KeyPath $typeRoot -Value 'Veritas Ledger'
Set-ItemProperty -Path $typeRoot -Name 'NeverShowExt' -Value ''
Set-ItemProperty -Path $typeRoot -Name 'InfoTip' -Value (
    'Veritas Ledger launcher. Double-click: Observe mode (idle-only). ' +
    'Right-click for Trade Ready mode.'
)

# Icon
Set-RegDefault -KeyPath "$typeRoot\DefaultIcon" -Value $iconLocation

# Shell root: declare "open" as the default verb
if (-not (Test-Path "$typeRoot\shell")) {
    New-Item -Path "$typeRoot\shell" -Force | Out-Null
}
Set-ItemProperty -Path "$typeRoot\shell" -Name '(Default)' -Value 'open'

# open verb (double-click => Observe)
Set-RegDefault -KeyPath "$typeRoot\shell\open"          -Value 'Open (Observe mode)'
Set-RegDefault -KeyPath "$typeRoot\shell\open\command"  -Value $observeCommand

# tradeready verb (right-click => "Open in Trade Ready mode")
Set-RegDefault -KeyPath "$typeRoot\shell\tradeready"         -Value 'Open in Trade Ready mode'
Set-RegDefault -KeyPath "$typeRoot\shell\tradeready\command" -Value $tradeReadyCommand

# ---------------------------------------------------------------------------
# Create the desktop launcher file (zero-byte; its type drives behaviour)
# ---------------------------------------------------------------------------
if (-not (Test-Path $desktopFile)) {
    New-Item -ItemType File -Path $desktopFile -Force | Out-Null
}

# ---------------------------------------------------------------------------
# Remove old .lnk shortcuts (replaced by the single .veritas launcher file)
# ---------------------------------------------------------------------------
foreach ($old in $oldLnks) {
    if (Test-Path $old) {
        Remove-Item -Path $old -Force
        Write-Host "Removed old shortcut: $old" -ForegroundColor DarkGray
    }
}

# ---------------------------------------------------------------------------
# Notify Explorer to refresh file-type/icon associations
# ---------------------------------------------------------------------------
try {
    $sig = @'
[DllImport("shell32.dll", CharSet = CharSet.Auto)]
public static extern void SHChangeNotify(int wEventId, int uFlags, IntPtr dwItem1, IntPtr dwItem2);
'@
    $notifyType = Add-Type -MemberDefinition $sig -Name 'Shell32VL' -Namespace 'Win32VL' -PassThru
    $notifyType::SHChangeNotify(0x08000000, 0x0000, [IntPtr]::Zero, [IntPtr]::Zero)
}
catch {
    Write-Host 'Note: shell refresh notification failed; icon update may require sign-out/sign-in.' -ForegroundColor Yellow
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host "Desktop launcher created: $desktopFile" -ForegroundColor Green
Write-Host "  Double-click  =>  Observe mode (idle-only; no privileged POSTs)" -ForegroundColor Cyan
Write-Host "  Right-click   =>  'Open in Trade Ready mode' (Bearer auth round-trip)" -ForegroundColor Cyan
Write-Host "  Icon:            $iconLocation" -ForegroundColor DarkGray
Write-Host "  File type:       $progId (HKCU; no admin required)" -ForegroundColor DarkGray
if ($Rebuild.IsPresent) {
    Write-Host '  -Rebuild flag embedded in both command strings.' -ForegroundColor DarkGray
}
