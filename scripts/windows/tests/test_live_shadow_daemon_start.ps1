# =============================================================================
# test_live_shadow_daemon_start.ps1
# MQK-LEDGER-BURN-CONTROLLER-03 A3A
#
# Proof for the canonical LiveShadow daemon-bootstrap capability:
#   - Start-MiniQuantDesk.ps1's new `-Mode LiveShadow` (Invoke-LiveShadowStartup)
#   - Launch-VeritasLedger.ps1's new `-DeploymentMode live-shadow` parameter
#     (Set-LauncherEnvironment / Get-BackendProbe / Start-DaemonIfNeeded /
#     Assert-LiveShadowStartupPrerequisites / Invoke-LiveShadowCheckOnly)
#
# Required negative controls (per mission):
#   - LiveShadow cannot become LiveCapital
#   - missing required config fails closed
#   - CheckOnly produces no daemon/broker mutation
#   - no LiveCapital credential/routing path is accidentally selected
#   - the actual production launcher traverses the tested LiveShadow seam
#
# Hermetic/process-fixture only: no real daemon start, no real broker/Alpaca
# call, no real order, no Paper/Live DB touched. `-CheckOnly` subprocess
# invocations only ever perform local loopback probes (127.0.0.1:8899, which
# is not running in this test environment) -- never an external network call.
#
# Exit codes: 0 = all proofs held, 1 = at least one did not.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir     = Split-Path -Parent $MyInvocation.MyCommand.Definition
$WindowsDir    = (Resolve-Path (Join-Path $ScriptDir '..')).Path.TrimEnd('\')
$Launcher      = Join-Path $WindowsDir 'Start-MiniQuantDesk.ps1'
$VeritasLedger = Join-Path $WindowsDir 'Launch-VeritasLedger.ps1'

$Violations = 0
function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan }
function Assert-True {
    param([string]$Label, [bool]$Condition)
    if ($Condition) {
        Show-Green "  OK -- $Label"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label"
    }
}
function Assert-Throws {
    param([string]$Label, [scriptblock]$Block, [string]$MatchMessage = $null)
    $threw = $false
    $msg = $null
    try { & $Block } catch { $threw = $true; $msg = $_.Exception.Message }
    $ok = $threw -and ($null -eq $MatchMessage -or $msg -match $MatchMessage)
    Assert-True $Label $ok
}
function Assert-NoThrow {
    param([string]$Label, [scriptblock]$Block)
    $threw = $false
    try { & $Block } catch { $threw = $true }
    Assert-True $Label (-not $threw)
}

if (-not (Test-Path $Launcher)) {
    Show-Red "FATAL -- launcher script not found: $Launcher"
    exit 1
}
if (-not (Test-Path $VeritasLedger)) {
    Show-Red "FATAL -- Launch-VeritasLedger.ps1 not found: $VeritasLedger"
    exit 1
}

$LauncherText      = Get-Content -Path $Launcher -Raw
$VeritasLedgerText = Get-Content -Path $VeritasLedger -Raw

function Invoke-Launcher {
    param([string[]]$LauncherArgs)
    $output = & powershell -NoProfile -ExecutionPolicy Bypass -File $Launcher @LauncherArgs 2>&1
    return @{ Output = ($output -join "`n"); ExitCode = $LASTEXITCODE }
}

function Invoke-VeritasLedger {
    param([string[]]$Args2)
    $output = & powershell -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $VeritasLedger @Args2 2>&1
    return @{ Output = ($output -join "`n"); ExitCode = $LASTEXITCODE }
}

# Extract just the Invoke-LiveShadowStartup function body text (not the whole
# file) so source-guard checks below prove properties of THAT function
# specifically, not merely "somewhere in this large file".
$liveShadowStartupMatch = [regex]::Match(
    $LauncherText,
    '(?s)function Invoke-LiveShadowStartup \{.*?\n\}\r?\n'
)
Assert-True 'Invoke-LiveShadowStartup function located in Start-MiniQuantDesk.ps1' $liveShadowStartupMatch.Success
$LiveShadowStartupBody = if ($liveShadowStartupMatch.Success) { $liveShadowStartupMatch.Value } else { '' }

$dispatchBranchMatch = [regex]::Match(
    $LauncherText,
    "(?s)elseif \(\`$resolvedMode -eq 'LiveShadow'\) \{.*?\n    \}\r?\n"
)
Assert-True 'Main dispatch LiveShadow branch located' $dispatchBranchMatch.Success
$DispatchBranchBody = if ($dispatchBranchMatch.Success) { $dispatchBranchMatch.Value } else { '' }

# "Never calls X" checks must look at CODE only -- this function's own doc
# comments legitimately mention Confirm-LiveIntent/Read-Host in prose
# explaining that it does NOT use them, which would otherwise false-positive
# a naive substring search.
function Get-CodeOnlyText {
    param([string]$Text)
    return (($Text -split "`n" | Where-Object { $_ -notmatch '^\s*#' }) -join "`n")
}
$LiveShadowStartupCode = Get-CodeOnlyText $LiveShadowStartupBody
$DispatchBranchCode    = Get-CodeOnlyText $DispatchBranchBody

# ---------------------------------------------------------------------------
# Section 1: LiveShadow cannot become LiveCapital (source-guard)
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 1: LiveShadow cannot become LiveCapital ==='

Assert-True 'Start-MiniQuantDesk.ps1 -Mode ValidateSet is exactly Paper/Live/LiveShadow' `
    ($LauncherText -match "\[ValidateSet\('Paper',\s*'Live',\s*'LiveShadow'\)\]")

Assert-True 'Launch-VeritasLedger.ps1 -DeploymentMode ValidateSet is exactly paper/live-shadow (never live/live-capital)' `
    ($VeritasLedgerText -match "\[ValidateSet\('paper',\s*'live-shadow'\)\]")

Assert-True 'Set-LauncherEnvironment''s ONE assignment to MQK_DAEMON_DEPLOYMENT_MODE is the $DeploymentMode parameter, never a literal' `
    (([regex]::Matches($VeritasLedgerText, [regex]::Escape('$env:MQK_DAEMON_DEPLOYMENT_MODE = '))).Count -eq 1 -and
     $VeritasLedgerText -match [regex]::Escape('$env:MQK_DAEMON_DEPLOYMENT_MODE = $DeploymentMode'))

Assert-True 'Invoke-LiveShadowStartup passes the literal string ''live-shadow'' to -DeploymentMode, never ''live''' `
    (($LiveShadowStartupBody -match [regex]::Escape("'-DeploymentMode', 'live-shadow'")) -and
     (-not ($LiveShadowStartupBody -match "-DeploymentMode', 'live'")))

Assert-True 'Invoke-LiveShadowStartup never calls Confirm-LiveIntent / Invoke-LiveStartup / Test-Live* (LiveCapital-only functions)' `
    (-not ($LiveShadowStartupCode -match 'Confirm-LiveIntent' -or $LiveShadowStartupCode -match 'Invoke-LiveStartup' -or $LiveShadowStartupCode -match 'Test-Live[A-Z]'))

Assert-True 'Main dispatch LiveShadow branch never calls Confirm-LiveIntent (no inherited Read-Host)' `
    (-not ($DispatchBranchCode -match 'Confirm-LiveIntent' -or $DispatchBranchCode -match 'Read-Host'))

Assert-True 'Main dispatch LiveShadow branch is a distinct elseif, not merged into the Live branch' `
    ($LauncherText -match "(?s)if \(\`$resolvedMode -eq 'Live'\) \{.*?\}\r?\n    elseif \(\`$resolvedMode -eq 'LiveShadow'\)")

# Functional: the real Set-LauncherEnvironment function, dot-sourced, must
# reject a literal 'live' DeploymentMode outright (ValidateSet) and must set
# MQK_DAEMON_DEPLOYMENT_MODE to exactly 'live-shadow' -- never 'live' -- when
# given 'live-shadow'.
. $VeritasLedger

Assert-Throws 'Set-LauncherEnvironment -DeploymentMode ''live'' is rejected by ValidateSet (only paper/live-shadow accepted)' {
    Set-LauncherEnvironment -OperatorToken 'test-token' -RepoRoot 'C:\fake' -DeploymentMode 'live'
}

$envSnapshotProbe = Set-LauncherEnvironment -OperatorToken 'test-token' -RepoRoot 'C:\fake' -DeploymentMode 'live-shadow'
Assert-True 'Set-LauncherEnvironment -DeploymentMode live-shadow sets MQK_DAEMON_DEPLOYMENT_MODE=live-shadow exactly' `
    ($env:MQK_DAEMON_DEPLOYMENT_MODE -eq 'live-shadow')
Restore-EnvSnapshot -Snapshot $envSnapshotProbe

$envSnapshotProbe2 = Set-LauncherEnvironment -OperatorToken 'test-token' -RepoRoot 'C:\fake'
Assert-True 'Set-LauncherEnvironment with no -DeploymentMode still defaults to paper (existing callers unaffected)' `
    ($env:MQK_DAEMON_DEPLOYMENT_MODE -eq 'paper')
Restore-EnvSnapshot -Snapshot $envSnapshotProbe2

# ---------------------------------------------------------------------------
# Section 2: missing required config fails closed
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 2: missing required live-shadow config fails closed ==='

# Shadow Get-EnvValue (already dot-sourced above) to return controlled
# values per name -- the same function-shadowing technique
# test_official_dual_mode_launcher.ps1 already uses for Invoke-JsonGet/Post.
function Get-EnvValue {
    param([Parameter(Mandatory = $true)][string]$Name)
    if ($script:FakeEnvValues.ContainsKey($Name)) { return $script:FakeEnvValues[$Name] }
    return $null
}

$script:FakeEnvValues = @{
    'MQK_DATABASE_URL'     = 'postgres://x'
    'ALPACA_API_KEY_LIVE'  = 'fake-key'
    'ALPACA_API_SECRET_LIVE' = 'fake-secret'
}
Assert-NoThrow 'All three required vars present: Assert-LiveShadowStartupPrerequisites does not throw' {
    Assert-LiveShadowStartupPrerequisites -DeploymentMode 'live-shadow' -ArmPaperRequested $false
}

$script:FakeEnvValues = @{
    'MQK_DATABASE_URL'     = 'postgres://x'
    'ALPACA_API_KEY_LIVE'  = ''
    'ALPACA_API_SECRET_LIVE' = 'fake-secret'
}
Assert-Throws 'ALPACA_API_KEY_LIVE missing/blank: fails closed naming the missing var' {
    Assert-LiveShadowStartupPrerequisites -DeploymentMode 'live-shadow' -ArmPaperRequested $false
} 'ALPACA_API_KEY_LIVE'

$script:FakeEnvValues = @{}
Assert-Throws 'All three vars missing: fails closed naming all three' {
    Assert-LiveShadowStartupPrerequisites -DeploymentMode 'live-shadow' -ArmPaperRequested $false
} 'MQK_DATABASE_URL.*ALPACA_API_KEY_LIVE.*ALPACA_API_SECRET_LIVE'

$script:FakeEnvValues = @{
    'MQK_DATABASE_URL'     = 'postgres://x'
    'ALPACA_API_KEY_LIVE'  = 'fake-key'
    'ALPACA_API_SECRET_LIVE' = 'fake-secret'
}
Assert-Throws '-ArmPaper with -DeploymentMode live-shadow always fails closed, even with config present' {
    Assert-LiveShadowStartupPrerequisites -DeploymentMode 'live-shadow' -ArmPaperRequested $true
} 'ArmPaper'

$script:FakeEnvValues = @{}
Assert-NoThrow 'DeploymentMode paper is completely unaffected by these checks (early return), regardless of missing live-shadow config' {
    Assert-LiveShadowStartupPrerequisites -DeploymentMode 'paper' -ArmPaperRequested $false
}

Assert-True 'The fail-closed check never reads a required value into a variable it could print (presence-only via IsNullOrWhiteSpace)' `
    ($VeritasLedgerText -match [regex]::Escape('[string]::IsNullOrWhiteSpace((Get-EnvValue -Name $_))'))

# ---------------------------------------------------------------------------
# Section 3: CheckOnly produces no daemon/broker mutation
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 3: live-shadow CheckOnly produces no daemon/broker mutation ==='

$liveShadowCheckOnlyMatch = [regex]::Match(
    $VeritasLedgerText,
    '(?s)function Invoke-LiveShadowCheckOnly \{.*?\n\}\r?\n'
)
Assert-True 'Invoke-LiveShadowCheckOnly function located in Launch-VeritasLedger.ps1' $liveShadowCheckOnlyMatch.Success
$LiveShadowCheckOnlyBody = if ($liveShadowCheckOnlyMatch.Success) { $liveShadowCheckOnlyMatch.Value } else { '' }

Assert-True 'Invoke-LiveShadowCheckOnly never calls Ensure-DaemonBinary / Start-DaemonIfNeeded (no daemon start)' `
    (-not ($LiveShadowCheckOnlyBody -match 'Ensure-DaemonBinary' -or $LiveShadowCheckOnlyBody -match 'Start-DaemonIfNeeded'))

Assert-True 'Invoke-LiveShadowCheckOnly never calls Invoke-ArmPaper / arm-execution' `
    (-not ($LiveShadowCheckOnlyBody -match 'Invoke-ArmPaper' -or $LiveShadowCheckOnlyBody -match 'arm-execution'))

Assert-True 'Invoke-LiveShadowCheckOnly never runs docker start/exec/migrate (read-only Get-Command probe only)' `
    (-not ($LiveShadowCheckOnlyBody -match 'docker start' -or $LiveShadowCheckOnlyBody -match 'docker exec' -or $LiveShadowCheckOnlyBody -match 'migrate'))

Assert-True 'Invoke-LiveShadowCheckOnly only issues GET-style read-only daemon probes (Invoke-CheckOnlyDaemonGet)' `
    (-not ($LiveShadowCheckOnlyBody -match 'Invoke-JsonRequest.*POST' -or $LiveShadowCheckOnlyBody -match "Method 'POST'"))

Assert-True 'CheckOnly branch dispatches to Invoke-LiveShadowCheckOnly (not the paper-specific Invoke-StartupCheckOnly) for live-shadow' `
    ($VeritasLedgerText -match "(?s)if \(\`$DeploymentMode -eq 'live-shadow'\) \{\s*Invoke-LiveShadowCheckOnly")

# Real subprocess: -Mode LiveShadow -CheckOnly through the actual top-level
# entrypoint. Safe/read-only by construction (Invoke-LiveShadowCheckOnly is
# read-only; the only network activity is a fail-soft loopback GET to
# 127.0.0.1:8899, which nothing is listening on in this test environment).
$r1 = Invoke-Launcher -LauncherArgs @('-Mode', 'LiveShadow', '-CheckOnly')
Assert-True '-Mode LiveShadow -CheckOnly: completed without hanging, numeric exit code' ($null -ne $r1.ExitCode)
Assert-True '-Mode LiveShadow -CheckOnly: delegates to and surfaces Launch-VeritasLedger.ps1''s live-shadow CheckOnly banner' `
    ($r1.Output -match 'Veritas Ledger Startup -- CheckOnly \(live-shadow\)')
Assert-True '-Mode LiveShadow -CheckOnly: never mentions arm-execution / clear-halted-run / disarm-execution' `
    (-not ($r1.Output -match 'arm-execution' -or $r1.Output -match 'clear-halted-run' -or $r1.Output -match 'disarm-execution'))
Assert-True '-Mode LiveShadow -CheckOnly: never mentions start-system' (-not ($r1.Output -match 'start-system'))
Assert-True '-Mode LiveShadow -CheckOnly: never claims a live-shadow daemon was started' `
    (-not ($r1.Output -match 'Started verified local live-shadow daemon'))
Assert-True '-Mode LiveShadow -CheckOnly: never prompts (no ''Type LIVE'' text)' (-not ($r1.Output -match 'Type LIVE'))

$r2 = Invoke-VeritasLedger -Args2 @('-DeploymentMode', 'live-shadow', '-CheckOnly')
Assert-True 'Launch-VeritasLedger.ps1 -DeploymentMode live-shadow -CheckOnly: completed, numeric exit code' ($null -ne $r2.ExitCode)
Assert-True 'Launch-VeritasLedger.ps1 -DeploymentMode live-shadow -CheckOnly: never mentions the paper DB container' `
    (-not ($r2.Output -match 'mqk-paper-postgres'))

# ---------------------------------------------------------------------------
# Section 4: no LiveCapital credential/routing path is accidentally selected
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 4: no LiveCapital credential/routing path is accidentally selected ==='

Assert-True 'Get-BackendProbe''s daemon-mode identity check compares against the passed $DeploymentMode, never a hardcoded ''paper'' literal' `
    ($VeritasLedgerText -match [regex]::Escape('-ne $DeploymentMode -or $result.Status.daemon_mode -ne $DeploymentMode'))

Assert-True 'Get-BackendProbe never treats autonomous_readiness_applicable=true as required when DeploymentMode is live-shadow (paper-only concept, per routes/system.rs is_paper_alpaca)' `
    ($VeritasLedgerText -match [regex]::Escape('if ($DeploymentMode -eq ''paper'') {'))

Assert-True 'live_routing_enabled refusal check is unconditional (applies to both paper and live-shadow probes)' `
    ($VeritasLedgerText -match [regex]::Escape('if ($result.Status.live_routing_enabled -eq $true) {'))

Assert-True 'Invoke-LiveShadowStartup''s post-start safety guard refuses live_routing_enabled=true' `
    ($LiveShadowStartupBody -match [regex]::Escape('$status.Json.live_routing_enabled -eq $true'))

Assert-True 'Invoke-LiveShadowStartup''s post-start safety guard refuses any daemon_mode other than live-shadow' `
    ($LiveShadowStartupBody -match [regex]::Escape('$status.Json.daemon_mode -ne ''live-shadow'''))

Assert-True 'Invoke-LiveShadowStartup never passes -ArmPaper or -CaptureStartupEvidence to Launch-VeritasLedger.ps1' `
    (-not ($LiveShadowStartupBody -match '-ArmPaper' -or $LiveShadowStartupBody -match '-CaptureStartupEvidence'))

Assert-True 'Invoke-LiveShadowStartup always passes -SkipGui (headless, no GUI process)' `
    ($LiveShadowStartupBody -match "'-SkipGui'")

# ---------------------------------------------------------------------------
# Section 5: the actual production launcher traverses the tested LiveShadow seam
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 5: production launcher traverses the tested LiveShadow seam ==='

Assert-True 'Start-MiniQuantDesk.ps1''s LiveShadow branch calls the real Invoke-LiveShadowStartup (not a stub/placeholder)' `
    ($DispatchBranchBody -match 'Invoke-LiveShadowStartup -RepoRoot \$RepoRoot -CheckOnlyFlag \$CheckOnly\.IsPresent -LogPath \$logPath')

Assert-True 'Invoke-LiveShadowStartup delegates daemon bootstrap to Launch-VeritasLedger.ps1 (no reimplementation, no second framework)' `
    (($LiveShadowStartupBody -match [regex]::Escape("Join-Path `$RepoRoot 'scripts\windows\Launch-VeritasLedger.ps1'")) -and
     ($LiveShadowStartupBody -match 'Invoke-BoundedChildScript'))

Assert-True 'New-LauncherLog accepts ''live-shadow'' as a mode label (used by Invoke-LiveShadowStartup''s own log)' `
    ($LauncherText -match "\[ValidateSet\('paper',\s*'live',\s*'live-shadow'\)\]")

# Section 5's r1 above (real -Mode LiveShadow -CheckOnly subprocess through
# the actual top-level entrypoint) is itself part of this proof: its output
# containing Launch-VeritasLedger.ps1's live-shadow CheckOnly banner (Section
# 3 above) demonstrates the production entrypoint really reaches the new
# seam end-to-end, not merely that the function exists in isolation.
Assert-True 'End-to-end: the real top-level Start-MiniQuantDesk.ps1 -Mode LiveShadow invocation reached Launch-VeritasLedger.ps1 -DeploymentMode live-shadow' `
    ($r1.Output -match '\(live-shadow\)')

Show-Info ''
if ($Violations -eq 0) {
    Show-Green '=== ALL LIVE-SHADOW DAEMON-START INVARIANTS PASSED ==='
    exit 0
} else {
    Show-Red "=== $Violations INVARIANT(S) FAILED ==="
    exit 1
}
