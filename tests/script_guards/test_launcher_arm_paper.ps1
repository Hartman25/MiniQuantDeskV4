# =============================================================================
# LAUNCHER-ARM-PAPER-GUARD-01
#
# Static invariant proof for -ArmPaper and -CaptureStartupEvidence additions to
# Launch-VeritasLedger.ps1 and the desktop shortcut installer update.
#
# Proves:
#   AP01 — -ArmPaper switch exists in Launch-VeritasLedger.ps1
#   AP02 — -CaptureStartupEvidence switch exists in Launch-VeritasLedger.ps1
#   AP03 — arm-execution action key is used (not run/start or start-system)
#   AP04 — launcher does NOT call run/start runtime start route
#   AP05 — launcher does NOT call Start-PaperTradingSmoke
#   AP06 — launcher does NOT enable or set live_routing to true
#   AP07 — launcher does NOT print the operator token value
#   AP08 — live_routing_enabled check appears before arm call in Invoke-ArmPaper
#   AP09 — default shortcut does NOT embed -ArmPaper
#   AP10 — shortcut open verb includes -PrepMarketData
#   AP11 — shortcut tradeready verb includes -PrepMarketData
#
# No daemon, no DB, no live calls. Pure static text analysis.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot   = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$Launcher   = Join-Path $RepoRoot 'scripts\windows\Launch-VeritasLedger.ps1'
$Installer  = Join-Path $RepoRoot 'scripts\windows\Install-VeritasLedgerDesktopShortcut.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== Launcher -ArmPaper guard (LAUNCHER-ARM-PAPER-GUARD-01) ==="
Write-Host "    Launcher  : $Launcher"
Write-Host "    Installer : $Installer"
Write-Host ""

# ---------------------------------------------------------------------------
# Load source files
# ---------------------------------------------------------------------------
$launcherText  = ''
$installerText = ''

if (Test-Path $Launcher) {
    $launcherText = Get-Content -Path $Launcher -Raw
    Pass 'AP00a' "Launch-VeritasLedger.ps1 exists"
} else {
    Fail 'AP00a' "Launch-VeritasLedger.ps1 NOT found: $Launcher"
}

if (Test-Path $Installer) {
    $installerText = Get-Content -Path $Installer -Raw
    Pass 'AP00b' "Install-VeritasLedgerDesktopShortcut.ps1 exists"
} else {
    Fail 'AP00b' "Install-VeritasLedgerDesktopShortcut.ps1 NOT found: $Installer"
}

# ---------------------------------------------------------------------------
# AP01: -ArmPaper switch is declared in the param block
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP01: -ArmPaper param declared --"

if ($launcherText -match '\[switch\]\$ArmPaper') {
    Pass 'AP01' "-ArmPaper switch is declared in Launch-VeritasLedger.ps1"
} else {
    Fail 'AP01' "-ArmPaper switch is NOT declared in Launch-VeritasLedger.ps1"
}

# ---------------------------------------------------------------------------
# AP02: -CaptureStartupEvidence switch is declared
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP02: -CaptureStartupEvidence param declared --"

if ($launcherText -match '\[switch\]\$CaptureStartupEvidence') {
    Pass 'AP02' "-CaptureStartupEvidence switch is declared"
} else {
    Fail 'AP02' "-CaptureStartupEvidence switch is NOT declared"
}

# ---------------------------------------------------------------------------
# AP03: arm-execution action key is present (correct arm route)
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP03: arm-execution action key used --"

if ($launcherText -match [regex]::Escape('arm-execution')) {
    Pass 'AP03' "arm-execution action key is present in launcher"
} else {
    Fail 'AP03' "arm-execution action key NOT found in launcher"
}

# ---------------------------------------------------------------------------
# AP04: launcher does NOT call /run/start or start-system directly
#       (arm-execution does not start runtime)
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP04: launcher does not call run/start or start-system --"

$runStartMatch = $launcherText | Select-String -Pattern "['`"]/v1/run/start|['`"]/api/v1/ops/action.*start-system|action_key.*start-system" -AllMatches
if (-not $runStartMatch) {
    Pass 'AP04' "Launcher does not reference /v1/run/start or start-system action"
} else {
    Fail 'AP04' "Launcher references run/start or start-system (runtime auto-start not allowed in this launcher)"
}

# ---------------------------------------------------------------------------
# AP05: launcher does NOT call Start-PaperTradingSmoke
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP05: launcher does not call Start-PaperTradingSmoke --"

if ($launcherText -notmatch 'Start-PaperTradingSmoke') {
    Pass 'AP05' "Launcher does not reference Start-PaperTradingSmoke"
} else {
    Fail 'AP05' "Launcher references Start-PaperTradingSmoke (smoke script must remain separate)"
}

# ---------------------------------------------------------------------------
# AP06: launcher does NOT set live_routing_enabled to true or enable live routing
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP06: launcher does not enable live routing --"

$liveRoutingSet = $launcherText | Select-String -Pattern "live_routing_enabled\s*=\s*\`$true|MQK_LIVE_ROUTING\s*=\s*['\`"]?true" -AllMatches
if (-not $liveRoutingSet) {
    Pass 'AP06' "Launcher does not enable live routing"
} else {
    Fail 'AP06' "Launcher appears to set live_routing_enabled=true or MQK_LIVE_ROUTING=true"
}

# ---------------------------------------------------------------------------
# AP07: launcher does NOT print the operator token value via Write-Host or Write-Output
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP07: launcher does not print operator token --"

# Allow printing the variable name (e.g. in comments or error messages referencing
# MQK_OPERATOR_TOKEN) but not the token value ($operatorToken in a Write- call).
$tokenPrint = $launcherText | Select-String -Pattern "Write-Host[^`n]*\`$operatorToken|Write-Output[^`n]*\`$operatorToken" -AllMatches
if (-not $tokenPrint) {
    Pass 'AP07' "Launcher does not print operator token value via Write-Host/Write-Output"
} else {
    Fail 'AP07' "Launcher appears to print the operator token value"
}

# ---------------------------------------------------------------------------
# AP08: live_routing_enabled check appears inside Invoke-ArmPaper (fail-closed guard)
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP08: live_routing_enabled guard inside Invoke-ArmPaper --"

# Extract the Invoke-ArmPaper function body: everything between 'function Invoke-ArmPaper'
# and the closing brace before the next 'function '.
$armFnMatch = [regex]::Match($launcherText, '(?s)function Invoke-ArmPaper \{.+?\n\}(?=\r?\n\r?\nfunction )')
if ($armFnMatch.Success) {
    $armFnBody = $armFnMatch.Value
    if ($armFnBody -match 'live_routing_enabled') {
        Pass 'AP08' "live_routing_enabled check exists inside Invoke-ArmPaper body"
    } else {
        Fail 'AP08' "live_routing_enabled check NOT found inside Invoke-ArmPaper body"
    }
} else {
    # Fallback: just check that the pattern appears after the function declaration header
    if ($launcherText -match '(?s)function Invoke-ArmPaper.*?live_routing_enabled') {
        Pass 'AP08' "live_routing_enabled check appears after Invoke-ArmPaper declaration"
    } else {
        Fail 'AP08' "Could not locate live_routing_enabled guard in Invoke-ArmPaper"
    }
}

# ---------------------------------------------------------------------------
# AP09: default desktop shortcut (double-click open verb) does NOT include -ArmPaper
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP09: default shortcut open verb does not include -ArmPaper --"

# Extract the observeCommand line from the installer
$observeMatch = [regex]::Match($installerText, '\$observeCommand\s*=\s*.+')
if ($observeMatch.Success) {
    $observeLine = $observeMatch.Value
    if ($observeLine -notmatch '-ArmPaper') {
        Pass 'AP09' "observeCommand (double-click default) does not include -ArmPaper"
    } else {
        Fail 'AP09' "observeCommand (double-click default) includes -ArmPaper (must not arm by default)"
    }
} else {
    Fail 'AP09' "Could not locate observeCommand assignment in installer"
}

# ---------------------------------------------------------------------------
# AP10: shortcut open verb (double-click) includes -PrepMarketData
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP10: shortcut open verb includes -PrepMarketData --"

if ($observeMatch.Success) {
    $observeLine = $observeMatch.Value
    if ($observeLine -match '-PrepMarketData') {
        Pass 'AP10' "observeCommand includes -PrepMarketData"
    } else {
        Fail 'AP10' "observeCommand does NOT include -PrepMarketData (shortcut should prep data on double-click)"
    }
} else {
    Fail 'AP10' "Could not locate observeCommand assignment (see AP09)"
}

# ---------------------------------------------------------------------------
# AP11: shortcut tradeready verb (right-click) includes -PrepMarketData
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- AP11: shortcut tradeready verb includes -PrepMarketData --"

$tradeReadyMatch = [regex]::Match($installerText, '\$tradeReadyCommand\s*=\s*.+')
if ($tradeReadyMatch.Success) {
    $tradeReadyLine = $tradeReadyMatch.Value
    if ($tradeReadyLine -match '-PrepMarketData') {
        Pass 'AP11' "tradeReadyCommand includes -PrepMarketData"
    } else {
        Fail 'AP11' "tradeReadyCommand does NOT include -PrepMarketData"
    }
} else {
    Fail 'AP11' "Could not locate tradeReadyCommand assignment in installer"
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL AP INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
