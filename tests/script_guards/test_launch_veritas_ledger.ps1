# =============================================================================
# PAPER-OPERATOR-STARTUP-LAUNCHER-01
# Script guard: test_launch_veritas_ledger.ps1
#
# Static assertions against scripts\windows\Launch-VeritasLedger.ps1's
# -CheckOnly mode (read-only startup status report) and the preserved
# normal desktop-launch path. No daemon, no DB, no live calls, no
# .env.local required. All checks are read-only source inspections.
#
# Guard assertions:
#   LVL01  Script exists at scripts\windows\Launch-VeritasLedger.ps1
#   LVL02  Script supports -CheckOnly switch
#   LVL03  Normal launch path preserved (daemon + GUI start untouched)
#   LVL04  -CheckOnly checks .env.local presence only (Test-Path), never reads its contents
#   LVL05  Script does NOT call any Alpaca/broker HTTP endpoint
#   LVL06  Script does NOT submit, replace, or cancel orders
#   LVL07  Script does NOT call clear-halted-run
#   LVL08  Script does NOT call flatten-paper-positions (or any flatten action)
#   LVL09  Script does NOT reference a Discord webhook
#   LVL10  Script does NOT issue INSERT/UPDATE/DELETE/DROP SQL
#   LVL11  -CheckOnly DB checks are SELECT-only (md_bars count + sys_arm_state)
#   LVL12  -CheckOnly reports persisted sys_arm_state
#   LVL13  -CheckOnly daemon health check is fail-soft (try/catch -> $null)
#   LVL14  -CheckOnly prints a "Next action" recommendation
#   LVL15  No new desktop-shortcut target introduced (Start-PaperOperatorConsole.ps1 not created)
#   LVL16  -CheckOnly branch (with exit) runs before operator-token resolution and -ArmPaper
#   LVL17  -CheckOnly does not invoke smoke runners (mention-only in Next action text)
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'
if (Test-Path variable:PSNativeCommandUseErrorActionPreference) {
    $PSNativeCommandUseErrorActionPreference = $false
}

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path
$Target    = Join-Path $RepoRoot 'scripts\windows\Launch-VeritasLedger.ps1'

$Passed = 0
$Failed = 0

function Assert-True {
    param([string]$Id, [string]$Description, [bool]$Condition)
    if ($Condition) {
        Write-Host "  PASS [$Id] $Description" -ForegroundColor Green
        $script:Passed++
    } else {
        Write-Host "  FAIL [$Id] $Description" -ForegroundColor Red
        $script:Failed++
    }
}

Write-Host ""
Write-Host "=== test_launch_veritas_ledger.ps1 (PAPER-OPERATOR-STARTUP-LAUNCHER-01) ===" -ForegroundColor Cyan
Write-Host "    Target: $Target"
Write-Host ""

if (-not (Test-Path $Target)) {
    Write-Host "  FAIL [LVL01] Script not found: $Target" -ForegroundColor Red
    exit 1
}

$Content = Get-Content $Target -Raw

# LVL01: script exists (Test-Path above already gates this; record as a pass)
Assert-True 'LVL01' 'Script exists at scripts\windows\Launch-VeritasLedger.ps1' `
    (Test-Path $Target)

# LVL02: supports -CheckOnly
Assert-True 'LVL02' 'Script supports -CheckOnly switch' `
    ($Content -match '\[switch\]\$CheckOnly')

# LVL03: normal launch path preserved
Assert-True 'LVL03' 'Normal launch path preserved (daemon resolve/start + GUI resolve/start all present)' `
    ($Content -match 'Ensure-DaemonBinary' -and
     $Content -match 'Start-DaemonIfNeeded' -and
     $Content -match 'Ensure-GuiBinary' -and
     $Content -match [regex]::Escape('Start-Process -FilePath $guiExe'))

# LVL04: -CheckOnly checks .env.local presence only, never reads its contents
$checkOnlyFnMatch = [regex]::Match($Content, '(?s)function Invoke-StartupCheckOnly.*?\n}\r?\n')
Assert-True 'LVL04' '-CheckOnly checks .env.local via Test-Path only (no Get-Content of .env.local)' `
    ($checkOnlyFnMatch.Success -and
     $checkOnlyFnMatch.Value -match [regex]::Escape('Test-Path $envLocalPath') -and
     $checkOnlyFnMatch.Value -notmatch 'Get-Content')

# LVL05: no Alpaca/broker HTTP endpoints
Assert-True 'LVL05' 'Script does NOT call any Alpaca/broker HTTP endpoint' `
    ($Content -notmatch '(?i)alpaca\.markets' -and
     $Content -notmatch '(?i)paper-api\.alpaca' -and
     $Content -notmatch '/v2/orders' -and
     $Content -notmatch '/v2/account')

# LVL06: no order submission/replace/cancel
Assert-True 'LVL06' 'Script does NOT submit, replace, or cancel orders' `
    ($Content -notmatch 'submit_order' -and
     $Content -notmatch '(?i)cancel_order' -and
     $Content -notmatch '(?i)replace_order' -and
     $Content -notmatch 'order_type')

# LVL07: no clear-halted-run
Assert-True 'LVL07' 'Script does NOT call clear-halted-run' `
    ($Content -notmatch [regex]::Escape('clear-halted-run'))

# LVL08: no flatten action
Assert-True 'LVL08' 'Script does NOT call flatten-paper-positions or any flatten action' `
    ($Content -notmatch '(?i)flatten')

# LVL09: no Discord webhook reference
Assert-True 'LVL09' 'Script does NOT reference a Discord webhook' `
    ($Content -notmatch 'DISCORD_WEBHOOK_URL' -and $Content -notmatch '(?i)webhook')

# LVL10: no DML SQL
Assert-True 'LVL10' 'Script does NOT issue INSERT/UPDATE/DELETE/DROP SQL' `
    ($Content -notmatch '(?i)\b(INSERT|UPDATE|DELETE|DROP)\b')

# LVL11: -CheckOnly DB checks are SELECT-only (md_bars count + sys_arm_state)
Assert-True 'LVL11' '-CheckOnly DB checks are SELECT-only (md_bars count + sys_arm_state)' `
    ($Content -match 'SELECT count\(\*\) FROM md_bars WHERE symbol=' -and
     $Content -match 'AND is_complete=true' -and
     $Content -match "SELECT state, coalesce\(reason, ''\) FROM sys_arm_state WHERE sentinel_id = 1")

# LVL12: -CheckOnly reports persisted sys_arm_state
Assert-True 'LVL12' '-CheckOnly reports persisted sys_arm_state with field label' `
    ($Content -match 'Persisted sys_arm_state' -and $Content -match 'sys_arm_state')

# LVL13: -CheckOnly daemon health check is fail-soft
$invokeCheckOnlyGetMatch = [regex]::Match($Content, '(?s)function Invoke-CheckOnlyDaemonGet.*?\n}\r?\n')
Assert-True 'LVL13' '-CheckOnly daemon GET helper is fail-soft (try/catch returns $null)' `
    ($invokeCheckOnlyGetMatch.Success -and
     $invokeCheckOnlyGetMatch.Value -match 'try\s*\{' -and
     $invokeCheckOnlyGetMatch.Value -match 'catch\s*\{' -and
     $invokeCheckOnlyGetMatch.Value -match 'return \$null')

# LVL14: -CheckOnly prints a "Next action" recommendation
Assert-True 'LVL14' '-CheckOnly prints a "Next action" recommendation' `
    ($Content -match [regex]::Escape("Write-CheckField 'Next action'"))

# LVL15: no new desktop-shortcut target introduced
$consoleAltPath = Join-Path $RepoRoot 'scripts\windows\Start-PaperOperatorConsole.ps1'
Assert-True 'LVL15' 'No new desktop-shortcut target introduced (Start-PaperOperatorConsole.ps1 not created)' `
    (-not (Test-Path $consoleAltPath))

# LVL16: -CheckOnly branch (with exit) runs before operator-token resolution and -ArmPaper
$checkOnlyIdx        = $Content.IndexOf('if ($CheckOnly.IsPresent)')
$resolveTokenCallIdx = $Content.IndexOf('$operatorToken = Resolve-RequiredOperatorToken')
$armPaperIdx         = $Content.IndexOf('if ($ArmPaper.IsPresent)')
Assert-True 'LVL16' '-CheckOnly branch (with exit) precedes operator-token resolution and -ArmPaper handling' `
    ($checkOnlyIdx -ge 0 -and $resolveTokenCallIdx -gt $checkOnlyIdx -and $armPaperIdx -gt $checkOnlyIdx -and
     $Content -match [regex]::Escape('exit $checkOnlyExitCode'))

# LVL17: -CheckOnly does not invoke smoke runners (mention-only in Next action text)
Assert-True 'LVL17' '-CheckOnly does not invoke smoke runners (no & call of Run-AAPL5mMarketSmoke.ps1 / Start-PaperTradingSmoke.ps1)' `
    ($Content -notmatch '&\s*[''"]?[^\r\n]*Run-AAPL5mMarketSmoke\.ps1' -and
     $Content -notmatch '&\s*[''"]?[^\r\n]*Start-PaperTradingSmoke\.ps1' -and
     $Content -notmatch 'Invoke-ExternalCommand[^\r\n]*Smoke')

# ---------------------------------------------------------------------------
# Section: STALE-DAEMON-BINARY-PROVENANCE-01 functional proofs
#
# Root cause (Thursday 2026-08-13): a reused mqk-daemon.exe predated the
# required-universe/retry functionality that had landed later the same day.
# Ensure-DaemonBinary now requires a provenance sidecar recording the
# core-rs git tree identity the binary was built from, and rebuilds
# whenever that identity is missing or does not match the current tree.
#
# Dot-sourcing is safe: MAIN DISPATCH is guarded by
# `if ($MyInvocation.InvocationName -ne '.')`, so this only defines
# functions -- no daemon start, no real build, no exit. All git/build
# operations below run against a disposable temp fixture repo, never the
# real MiniQuantDeskV4 repo.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Section: STALE-DAEMON-BINARY-PROVENANCE-01 functional proofs ===" -ForegroundColor Cyan

. $Target

$FixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("lvl_provenance_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path (Join-Path $FixtureRoot 'core-rs') | Out-Null

$gitExe = (Get-Command 'git' -ErrorAction SilentlyContinue).Source
if ($null -eq $gitExe) {
    Write-Host "  SKIP [LVL18-26] git not found on PATH; provenance functional proofs skipped" -ForegroundColor Yellow
} else {
    Push-Location $FixtureRoot
    try {
        & $gitExe init -q . 2>&1 | Out-Null
        & $gitExe config user.email 'test@example.com' 2>&1 | Out-Null
        & $gitExe config user.name 'test' 2>&1 | Out-Null
        & $gitExe config core.autocrlf false 2>&1 | Out-Null
        # Mirror the real repo's .gitignore (target/ excluded) so creating the
        # fixture's fake build artifact under core-rs\target does not itself
        # make the tree look dirty.
        Set-Content -Path (Join-Path $FixtureRoot '.gitignore') -Value "target/`n**/target/`n"
        Set-Content -Path (Join-Path $FixtureRoot 'core-rs\marker.txt') -Value 'v1'
        & $gitExe add -A 2>&1 | Out-Null
        & $gitExe commit -q -m 'initial' 2>&1 | Out-Null

        $identityV1 = Get-CoreRsTreeIdentity -RepoRoot $FixtureRoot
        Assert-True 'LVL18' 'Get-CoreRsTreeIdentity resolves a 40-char git tree SHA for core-rs on a clean commit' `
            ($identityV1 -match '^[0-9a-f]{40}$')

        $releaseDir = Join-Path $FixtureRoot 'core-rs\target\release'
        New-Item -ItemType Directory -Force -Path $releaseDir | Out-Null
        $fakeExe = Join-Path $releaseDir 'mqk-daemon.exe'
        Set-Content -Path $fakeExe -Value 'fake'

        Assert-True 'LVL19' 'Missing provenance sidecar -> Test-DaemonBinaryProvenanceMatches=false (rebuild required)' `
            (-not (Test-DaemonBinaryProvenanceMatches -RepoRoot $FixtureRoot -DaemonExePath $fakeExe))

        Write-DaemonBuildProvenance -RepoRoot $FixtureRoot
        Assert-True 'LVL20' 'Matching provenance sidecar -> Test-DaemonBinaryProvenanceMatches=true (binary reusable)' `
            (Test-DaemonBinaryProvenanceMatches -RepoRoot $FixtureRoot -DaemonExePath $fakeExe)

        Set-Content -Path (Join-Path $FixtureRoot 'core-rs\marker.txt') -Value 'v2-uncommitted'
        $identityDirty = Get-CoreRsTreeIdentity -RepoRoot $FixtureRoot
        Assert-True 'LVL21' 'Uncommitted core-rs change -> identity suffixed -dirty, and no longer matches the sidecar' `
            ($identityDirty -eq "$identityV1-dirty" -and
             -not (Test-DaemonBinaryProvenanceMatches -RepoRoot $FixtureRoot -DaemonExePath $fakeExe))

        & $gitExe add -A 2>&1 | Out-Null
        & $gitExe commit -q -m 'second commit' 2>&1 | Out-Null
        $identityV2 = Get-CoreRsTreeIdentity -RepoRoot $FixtureRoot
        Assert-True 'LVL22' 'New commit changes core-rs tree identity -> stale sidecar still does not match (mismatched identity -> rebuild required)' `
            ($identityV2 -ne $identityV1 -and -not (Test-DaemonBinaryProvenanceMatches -RepoRoot $FixtureRoot -DaemonExePath $fakeExe))

        Write-DaemonBuildProvenance -RepoRoot $FixtureRoot
        Assert-True 'LVL23' 'Rewriting the provenance sidecar after a rebuild makes the binary reusable again' `
            (Test-DaemonBinaryProvenanceMatches -RepoRoot $FixtureRoot -DaemonExePath $fakeExe)

        # Ensure-DaemonBinary end-to-end, with the real build tools shadowed
        # so no real cargo build runs against the fixture.
        Remove-Item (Join-Path $FixtureRoot 'core-rs\target\release\mqk-daemon.build-tree.txt') -ErrorAction SilentlyContinue
        $script:BuildInvoked = $false
        function Get-CommandPath {
            param($Name)
            if ($Name -eq 'cargo') { return 'cargo.fake' }
            return (Get-Command $Name -ErrorAction Stop).Source
        }
        function Invoke-ExternalCommand {
            param($FilePath, $Arguments, $WorkingDirectory, [switch]$AllowFailure)
            $script:BuildInvoked = $true
            Set-Content -Path $fakeExe -Value 'rebuilt'
        }

        $null = Ensure-DaemonBinary -RepoRoot $FixtureRoot -ForceRebuild $false
        Assert-True 'LVL24' 'Ensure-DaemonBinary rebuilds when the provenance sidecar is missing (ForceRebuild=false)' `
            ($script:BuildInvoked -eq $true -and (Test-Path (Join-Path $FixtureRoot 'core-rs\target\release\mqk-daemon.build-tree.txt')))

        $script:BuildInvoked = $false
        $null = Ensure-DaemonBinary -RepoRoot $FixtureRoot -ForceRebuild $false
        Assert-True 'LVL25' 'Ensure-DaemonBinary reuses the binary when provenance matches (ForceRebuild=false, no rebuild triggered)' `
            ($script:BuildInvoked -eq $false)

        $script:BuildInvoked = $false
        $null = Ensure-DaemonBinary -RepoRoot $FixtureRoot -ForceRebuild $true
        Assert-True 'LVL26' 'Ensure-DaemonBinary -ForceRebuild always rebuilds regardless of matching provenance' `
            ($script:BuildInvoked -eq $true)
    }
    finally {
        Pop-Location
        Remove-Item -Recurse -Force $FixtureRoot -ErrorAction SilentlyContinue
    }
}

# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $Passed" -ForegroundColor Green
Write-Host "  Failed: $Failed" -ForegroundColor $(if ($Failed -gt 0) { 'Red' } else { 'Green' })
Write-Host ""

if ($Failed -gt 0) {
    Write-Host "GUARD FAILED: $Failed assertion(s) failed." -ForegroundColor Red
    exit 1
} else {
    Write-Host "GUARD PASSED: all $Passed assertions passed." -ForegroundColor Green
    exit 0
}
