# =============================================================================
# Paper premarket validation runner
# CI-GUARD-CONSOLIDATION-01
#
# ONE safe "before market open" validation command. Answers, without running
# the expensive full repo proof:
#   - Are script guards passing?
#   - Are GUI tests passing?
#   - Does the GUI build/typecheck?
#   - Are the key smoke-prep scripts still safe (covered by script guards)?
#   - Are the key paper baseline/reconcile tests still passing?
#   - Is the repo in a safe state for a market-smoke retest?
#
# This is NOT full_repo_proof.ps1. It does not, and must never:
#   - start the daemon trading runtime
#   - arm paper trading
#   - clear a halted run
#   - flatten positions
#   - submit, replace, or cancel orders
#   - inject strategy signals
#   - call any broker/Alpaca endpoint
#   - write to any database
#   - run a market smoke (normal mode)
#   - send a Discord alert / call a Discord webhook
#   - print .env.local or any secret value
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-PaperPremarketValidation.ps1
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-PaperPremarketValidation.ps1 -Fast
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-PaperPremarketValidation.ps1 -SkipRust -SkipGuiBuild
#   powershell -ExecutionPolicy Bypass -File scripts\windows\Invoke-PaperPremarketValidation.ps1 -AllowDirty
#
# Exit codes: 0 = all selected checks passed, 1 = one or more checks failed
# (or the tracked working tree is dirty without -AllowDirty).
# =============================================================================

[CmdletBinding()]
param(
    # -AllowDirty: continue even if the tracked working tree has uncommitted
    #   changes. Untracked evidence/exports/logs/generated artifacts never
    #   block, with or without this switch.
    [switch]$AllowDirty,

    # -SkipGuiBuild: skip `npm run build` (tsc + vite build) in core-rs/mqk-gui.
    [switch]$SkipGuiBuild,

    # -SkipRust: skip the targeted Rust baseline/reconcile and orchestrator tests.
    [switch]$SkipRust,

    # -Fast: shorthand for -SkipGuiBuild -SkipRust. Script guards and GUI unit
    #   tests still run.
    [switch]$Fast
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Get-RepoRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}

function Invoke-CheckedCommand {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [string]$WorkingDirectory
    )

    $pushed = $false
    if ($WorkingDirectory) {
        Push-Location $WorkingDirectory
        $pushed = $true
    }
    try {
        & $FilePath @Arguments
        $exitCode = $LASTEXITCODE
        if ($null -eq $exitCode) { $exitCode = 0 }
        if ($exitCode -ne 0) {
            $argText = if ($Arguments.Count -gt 0) { $Arguments -join ' ' } else { '' }
            throw ("Command failed (exit={0}): {1} {2}" -f $exitCode, $FilePath, $argText).Trim()
        }
    } finally {
        if ($pushed) { Pop-Location }
    }
}

$TotalSteps = 6
$Results = [System.Collections.Generic.List[object]]::new()

function Add-StepResult {
    param([string]$Name, [string]$Status, [double]$ElapsedSeconds, [string]$Detail = '')
    $Results.Add([pscustomobject]@{
        Name           = $Name
        Status         = $Status
        ElapsedSeconds = $ElapsedSeconds
        Detail         = $Detail
    })
}

function Write-StepResult {
    param([int]$Index, [string]$Name, [string]$Status, [double]$Elapsed)
    $color = switch ($Status) {
        'PASS'    { 'Green' }
        'SKIPPED' { 'Yellow' }
        default   { 'Red' }
    }
    Write-Host ("[{0}/{1}] {2}: {3} ({4:N1}s)" -f $Index, $TotalSteps, $Name, $Status, $Elapsed) -ForegroundColor $color
}

function Invoke-ValidationStep {
    param(
        [int]$Index,
        [string]$Name,
        [scriptblock]$Action,
        [bool]$Skip,
        [string]$SkipReason = ''
    )

    Write-Host ''
    Write-Host "--- [$Index/$TotalSteps] $Name ---" -ForegroundColor Cyan

    if ($Skip) {
        Write-Host "  SKIPPED: $SkipReason" -ForegroundColor Yellow
        Write-StepResult -Index $Index -Name $Name -Status 'SKIPPED' -Elapsed 0
        Add-StepResult -Name $Name -Status 'SKIPPED' -ElapsedSeconds 0 -Detail $SkipReason
        return
    }

    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $status = 'PASS'
    $detail = ''
    try {
        & $Action
        Write-Host '  PASS' -ForegroundColor Green
    } catch {
        $status = 'FAIL'
        $detail = $_.Exception.Message
        Write-Host "  FAIL: $detail" -ForegroundColor Red
    }
    $sw.Stop()

    Write-StepResult -Index $Index -Name $Name -Status $status -Elapsed $sw.Elapsed.TotalSeconds
    Add-StepResult -Name $Name -Status $status -ElapsedSeconds $sw.Elapsed.TotalSeconds -Detail $detail
}

# -----------------------------------------------------------------------------
$skipGuiBuild = $SkipGuiBuild.IsPresent -or $Fast.IsPresent
$skipRust     = $SkipRust.IsPresent -or $Fast.IsPresent

$RepoRoot = Get-RepoRoot

Write-Host ''
Write-Host '============================================================'
Write-Host 'Paper Premarket Validation (CI-GUARD-CONSOLIDATION-01)'
Write-Host '============================================================'
Write-Host "Repo root: $RepoRoot"
Write-Host 'This is NOT full_repo_proof.ps1. No daemon start, no arm, no'
Write-Host 'halt-clear, no flatten, no orders, no signal injection, no'
Write-Host 'broker/Alpaca calls, no Discord, no DB writes, no .env.local read.'

# =============================================================================
# [1/6] Repo status
# =============================================================================
$stepIndex = 1
Write-Host ''
Write-Host "--- [$stepIndex/$TotalSteps] Repo status ---" -ForegroundColor Cyan

$sw = [System.Diagnostics.Stopwatch]::StartNew()
$repoStatus = 'PASS'
$repoDetail = ''
$hardStop = $false

try {
    $requiredPaths = @(
        (Join-Path $RepoRoot 'tests\script_guards\run_all_script_guards.ps1'),
        (Join-Path $RepoRoot 'core-rs\mqk-gui\package.json'),
        (Join-Path $RepoRoot 'core-rs\Cargo.toml')
    )
    foreach ($p in $requiredPaths) {
        if (-not (Test-Path -LiteralPath $p)) {
            throw "Required repo path missing: $p"
        }
    }
    Write-Host "  Repo root layout OK ($($requiredPaths.Count) anchor paths found)" -ForegroundColor Green

    $gitExe = (Get-Command 'git' -ErrorAction Stop).Source
    $statusLines = & $gitExe -C $RepoRoot status --porcelain=v1 --untracked-files=all
    if ($null -eq $statusLines) { $statusLines = @() }
    $statusLines = @($statusLines)

    $trackedDirty = @($statusLines | Where-Object { $_ -notmatch '^\?\?' })
    $untracked    = @($statusLines | Where-Object { $_ -match '^\?\?' })

    $artifactPatterns = @(
        '^evidence/',
        '^exports/',
        '\.log$',
        'node_modules/',
        '(^|/)target/',
        '(^|/)target_test/',
        '(^|/)dist/',
        '^scripts/windows/generated/',
        '^\.proof/'
    )

    $nonArtifactUntracked = @()
    foreach ($line in $untracked) {
        $path = $line.Substring(3)
        $isArtifact = $false
        foreach ($pattern in $artifactPatterns) {
            if ($path -match $pattern) { $isArtifact = $true; break }
        }
        if (-not $isArtifact) { $nonArtifactUntracked += $path }
    }

    if ($trackedDirty.Count -gt 0) {
        Write-Host "  Tracked working tree has $($trackedDirty.Count) uncommitted change(s):" -ForegroundColor Red
        foreach ($line in $trackedDirty) { Write-Host "    $line" -ForegroundColor Red }

        if ($AllowDirty.IsPresent) {
            Write-Host '  -AllowDirty set: continuing despite dirty tracked tree.' -ForegroundColor Yellow
            $repoDetail = "Tracked working tree dirty ($($trackedDirty.Count) file(s)); continued via -AllowDirty."
        } else {
            $repoStatus = 'FAIL'
            $repoDetail = "Tracked working tree has $($trackedDirty.Count) uncommitted change(s). Re-run with -AllowDirty to override, or commit/stash first."
        }
    } else {
        Write-Host '  Tracked working tree is clean.' -ForegroundColor Green
    }

    if ($untracked.Count -gt 0) {
        Write-Host "  Untracked files: $($untracked.Count) (evidence/exports/logs/generated artifacts ignored)"
    }
    if ($nonArtifactUntracked.Count -gt 0) {
        Write-Host '  Untracked non-artifact file(s) present (informational only):' -ForegroundColor Yellow
        foreach ($path in $nonArtifactUntracked) { Write-Host "    $path" -ForegroundColor Yellow }
    }
} catch {
    $repoStatus = 'FAIL'
    $repoDetail = $_.Exception.Message
}

$sw.Stop()
if ($repoStatus -eq 'FAIL') {
    if (-not $repoDetail) { $repoDetail = 'Repo status check failed.' }
    Write-Host "  FAIL: $repoDetail" -ForegroundColor Red
    $hardStop = $true
} else {
    Write-Host '  PASS' -ForegroundColor Green
}
Write-StepResult -Index $stepIndex -Name 'Repo status' -Status $repoStatus -Elapsed $sw.Elapsed.TotalSeconds
Add-StepResult -Name 'Repo status' -Status $repoStatus -ElapsedSeconds $sw.Elapsed.TotalSeconds -Detail $repoDetail

$hardStopReason = 'repo status check failed (see step 1)'

# =============================================================================
# [2/6] Script guards
# =============================================================================
Invoke-ValidationStep -Index 2 -Name 'Script guards' -Skip $hardStop -SkipReason $hardStopReason -Action {
    $guardScript = Join-Path $RepoRoot 'tests\script_guards\run_all_script_guards.ps1'
    Invoke-CheckedCommand -FilePath 'powershell.exe' -Arguments @('-ExecutionPolicy', 'Bypass', '-NonInteractive', '-File', $guardScript)
}

# =============================================================================
# [3/6] GUI tests
# =============================================================================
Invoke-ValidationStep -Index 3 -Name 'GUI tests' -Skip $hardStop -SkipReason $hardStopReason -Action {
    $npmExe = (Get-Command 'npm' -ErrorAction Stop).Source
    $guiDir = Join-Path $RepoRoot 'core-rs\mqk-gui'
    Invoke-CheckedCommand -FilePath $npmExe -Arguments @('run', 'test') -WorkingDirectory $guiDir
}

# =============================================================================
# [4/6] GUI build
# =============================================================================
$guiBuildSkip = $hardStop -or $skipGuiBuild
$guiBuildSkipReason = if ($hardStop) { $hardStopReason } else { '-SkipGuiBuild (or -Fast) specified' }
Invoke-ValidationStep -Index 4 -Name 'GUI build' -Skip $guiBuildSkip -SkipReason $guiBuildSkipReason -Action {
    $npmExe = (Get-Command 'npm' -ErrorAction Stop).Source
    $guiDir = Join-Path $RepoRoot 'core-rs\mqk-gui'
    Invoke-CheckedCommand -FilePath $npmExe -Arguments @('run', 'build') -WorkingDirectory $guiDir
}

# =============================================================================
# [5/6] Rust baseline snapshot tests
# =============================================================================
$rustSkip = $hardStop -or $skipRust
$rustSkipReason = if ($hardStop) { $hardStopReason } else { '-SkipRust (or -Fast) specified' }
Invoke-ValidationStep -Index 5 -Name 'Rust baseline snapshot tests' -Skip $rustSkip -SkipReason $rustSkipReason -Action {
    $cargoExe = (Get-Command 'cargo' -ErrorAction Stop).Source
    $manifest = Join-Path $RepoRoot 'core-rs\Cargo.toml'

    Invoke-CheckedCommand -FilePath $cargoExe -Arguments @('test', '--manifest-path', $manifest, '-p', 'mqk-daemon', '--lib', 'state::snapshot', '--', '--test-threads=1')
    Invoke-CheckedCommand -FilePath $cargoExe -Arguments @('test', '--manifest-path', $manifest, '-p', 'mqk-daemon', '--test', 'scenario_reconcile_baseline_seed_01', '--', '--test-threads=1')
    Invoke-CheckedCommand -FilePath $cargoExe -Arguments @('test', '--manifest-path', $manifest, '-p', 'mqk-daemon', '--test', 'scenario_runtime_start_reconcile_baseline_01', '--', '--test-threads=1')
}

# =============================================================================
# [6/6] Rust runtime orchestrator tests
# =============================================================================
Invoke-ValidationStep -Index 6 -Name 'Rust runtime orchestrator tests' -Skip $rustSkip -SkipReason $rustSkipReason -Action {
    $cargoExe = (Get-Command 'cargo' -ErrorAction Stop).Source
    $manifest = Join-Path $RepoRoot 'core-rs\Cargo.toml'
    Invoke-CheckedCommand -FilePath $cargoExe -Arguments @('test', '--manifest-path', $manifest, '-p', 'mqk-runtime', '--lib', 'orchestrator', '--', '--test-threads=1')
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ''
Write-Host '============================================================'
Write-Host '=== Paper Premarket Validation ==='
Write-Host '============================================================'

$idx = 0
foreach ($r in $Results) {
    $idx++
    $color = switch ($r.Status) {
        'PASS'    { 'Green' }
        'SKIPPED' { 'Yellow' }
        default   { 'Red' }
    }
    Write-Host ("[{0}/{1}] {2}: {3}" -f $idx, $TotalSteps, $r.Name, $r.Status) -ForegroundColor $color
    if ($r.Status -eq 'FAIL' -and $r.Detail) {
        Write-Host "       $($r.Detail)" -ForegroundColor Red
    } elseif ($r.Status -eq 'SKIPPED' -and $r.Detail) {
        Write-Host "       ($($r.Detail))" -ForegroundColor DarkGray
    } elseif ($r.Detail) {
        Write-Host "       $($r.Detail)" -ForegroundColor DarkGray
    }
}

Write-Host ''
$failedSteps = @($Results | Where-Object { $_.Status -eq 'FAIL' })

if ($failedSteps.Count -eq 0) {
    Write-Host 'FINAL: PASS - safe to proceed to Launch-VeritasLedger.ps1 -CheckOnly and market-smoke CheckOnly.' -ForegroundColor Green
    exit 0
} else {
    Write-Host ("FINAL: FAIL - {0} of {1} check(s) failed. Fix the failing command(s) above and re-run before any market-smoke retest." -f $failedSteps.Count, $TotalSteps) -ForegroundColor Red
    exit 1
}
