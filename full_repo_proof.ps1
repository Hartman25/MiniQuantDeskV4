[CmdletBinding()]
param(
    [ValidateSet('local', 'full', 'exploratory')]
    [string]$ProofProfile = 'local'
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function New-LaneRecord {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [ValidateSet('PASSED', 'FAILED', 'IGNORED', 'SKIPPED')]
        [string]$Status,
        [Parameter(Mandatory = $true)]
        [bool]$Required,
        [Nullable[int]]$ExitCode = $null,
        [double]$DurationSeconds = 0,
        [string]$Note = ''
    )

    [pscustomobject]@{
        lane_name        = $Name
        status           = $Status
        required         = $Required
        exit_code        = $ExitCode
        duration_seconds = [math]::Round($DurationSeconds, 2)
        note             = $Note
    }
}

function New-LaneNote {
    param(
        [string]$Note = ''
    )

    [pscustomobject]@{
        Note = $Note
    }
}

function Get-CommandPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $command = Get-Command $Name -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($null -eq $command) {
        return $null
    }

    if ($command.Source) {
        return $command.Source
    }

    if ($command.Path) {
        return $command.Path
    }

    return $command.Name
}

function Invoke-NativeCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [string[]]$Arguments = @(),
        [string]$WorkingDirectory
    )

    if ($WorkingDirectory) {
        Push-Location $WorkingDirectory
    }

    try {
        & $FilePath @Arguments
        $exitCode = $LASTEXITCODE
        if ($null -eq $exitCode) {
            $exitCode = 0
        }

        if ($exitCode -ne 0) {
            $argText = if ($Arguments.Count -gt 0) { $Arguments -join ' ' } else { '' }
            throw ("EXITCODE={0};Command failed: {1} {2}" -f $exitCode, $FilePath, $argText).Trim()
        }
    }
    finally {
        if ($WorkingDirectory) {
            Pop-Location
        }
    }
}

function Invoke-RepoBashScript {
    param(
        [Parameter(Mandatory = $true)]
        [string]$BashExe,
        [Parameter(Mandatory = $true)]
        [string]$RepoRoot,
        [Parameter(Mandatory = $true)]
        [string]$ScriptPath
    )

    $resolvedRepoRoot = [System.IO.Path]::GetFullPath($RepoRoot)
    $resolvedScriptPath = [System.IO.Path]::GetFullPath($ScriptPath)

    if (-not $resolvedScriptPath.StartsWith($resolvedRepoRoot, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw ("Script path is outside repo root: {0}" -f $resolvedScriptPath)
    }

    $relativePath = $resolvedScriptPath.Substring($resolvedRepoRoot.Length).TrimStart('\', '/')
    if ([string]::IsNullOrWhiteSpace($relativePath)) {
        throw ("Unable to derive repo-relative script path for: {0}" -f $resolvedScriptPath)
    }

    $bashRelativePath = './' + ($relativePath -replace '\\', '/')

    Invoke-NativeCommand -FilePath $BashExe -Arguments @('--noprofile', '--norc', $bashRelativePath) -WorkingDirectory $resolvedRepoRoot
}

function Require-Path {
    param(
        [Parameter(Mandatory = $true)]
        [string]$PathToCheck,
        [Parameter(Mandatory = $true)]
        [string]$Label
    )

    if (-not (Test-Path -LiteralPath $PathToCheck)) {
        throw ("Missing required path for {0}: {1}" -f $Label, $PathToCheck)
    }
}

function Require-Command {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [string]$Purpose
    )

    $resolved = Get-CommandPath -Name $Name
    if ([string]::IsNullOrWhiteSpace($resolved)) {
        throw ("Required tool '{0}' was not found on PATH ({1})." -f $Name, $Purpose)
    }

    return $resolved
}

function Get-RedactedDbUrl {
    param(
        [string]$Url
    )

    if ([string]::IsNullOrWhiteSpace($Url)) {
        return '<not-set>'
    }

    if ($Url -match '^(postgres(?:ql)?://[^:/?#]+:)([^@/?#]+)(@.+)$') {
        return ($matches[1] + '****' + $matches[3])
    }

    return '<redacted-unparseable-db-url>'
}

function Get-ExceptionExitCode {
    param(
        [Parameter(Mandatory = $true)]
        [System.Management.Automation.ErrorRecord]$ErrorRecord
    )

    $message = $ErrorRecord.Exception.Message
    if ($message -match '^EXITCODE=(\d+);(.*)$') {
        return [int]$matches[1]
    }

    return 1
}

function Get-ExceptionNote {
    param(
        [Parameter(Mandatory = $true)]
        [System.Management.Automation.ErrorRecord]$ErrorRecord
    )

    $message = $ErrorRecord.Exception.Message
    if ($message -match '^EXITCODE=\d+;(.*)$') {
        return $matches[1].Trim()
    }

    return $message
}

function Test-IsWindowsPlatform {
    return ($env:OS -eq 'Windows_NT')
}

function Resolve-CompatibleRepoBash {
    param(
        [Parameter(Mandatory = $true)]
        [string]$GitExe
    )

    if (-not (Test-IsWindowsPlatform)) {
        return (Require-Command -Name 'bash' -Purpose 'guard scripts and DB bootstrap helper')
    }

    $candidates = [System.Collections.Generic.List[string]]::new()
    $rejections = [System.Collections.Generic.List[string]]::new()
    $seenCandidates = @{}

    $gitPath = $GitExe
    if (-not [string]::IsNullOrWhiteSpace($gitPath) -and (Test-Path -LiteralPath $gitPath)) {
        $gitDir = Split-Path -Parent $gitPath
        $gitRoot = Split-Path -Parent $gitDir
        foreach ($candidate in @(
            (Join-Path $gitRoot 'bin/bash.exe'),
            (Join-Path $gitRoot 'usr/bin/bash.exe')
        )) {
            if (-not [string]::IsNullOrWhiteSpace($candidate)) {
                $candidates.Add($candidate)
            }
        }
    }

    foreach ($root in @($env:ProgramFiles, $env:ProgramW6432, ${env:ProgramFiles(x86)})) {
        if (-not [string]::IsNullOrWhiteSpace($root)) {
            $candidates.Add((Join-Path $root 'Git/bin/bash.exe'))
            $candidates.Add((Join-Path $root 'Git/usr/bin/bash.exe'))
        }
    }

    $pathBash = Get-CommandPath -Name 'bash'
    if (-not [string]::IsNullOrWhiteSpace($pathBash)) {
        $candidates.Add($pathBash)
    }

    foreach ($candidate in $candidates) {
        if ([string]::IsNullOrWhiteSpace($candidate)) {
            continue
        }

        $candidateKey = $candidate.ToLowerInvariant()
        if ($seenCandidates.ContainsKey($candidateKey)) {
            continue
        }
        $seenCandidates[$candidateKey] = $true

        if (-not (Test-Path -LiteralPath $candidate)) {
            continue
        }

        $normalized = [System.IO.Path]::GetFullPath($candidate)
        $lower = $normalized.ToLowerInvariant()

        if ($lower -eq 'c:\windows\system32\bash.exe') {
            $rejections.Add("Rejected incompatible WSL bash shim: $normalized")
            continue
        }

        if ($lower -like '*\windows\system32\bash.exe') {
            $rejections.Add("Rejected incompatible Windows system bash: $normalized")
            continue
        }

        if (
            $lower -like '*\git\bin\bash.exe' -or
            $lower -like '*\git\usr\bin\bash.exe' -or
            $lower -like '*\mingw*\bash.exe' -or
            $lower -like '*\msys*\bash.exe'
        ) {
            return $normalized
        }

        $rejections.Add("Rejected non-Git/non-MSYS bash candidate: $normalized")
    }

    $reason = if ($rejections.Count -gt 0) {
        $rejections -join '; '
    }
    else {
        'No bash candidate was found.'
    }

    throw "Unable to resolve a Windows-compatible Git/MSYS bash for repo helper scripts. $reason"
}

$laneResults = [System.Collections.Generic.List[object]]::new()

function Invoke-ProofLane {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Name,
        [Parameter(Mandatory = $true)]
        [bool]$Required,
        [Parameter(Mandatory = $true)]
        [scriptblock]$Action,
        [string]$SkipReason
    )

    if ($SkipReason) {
        $script:laneResults.Add((New-LaneRecord -Name $Name -Status 'SKIPPED' -Required $Required -ExitCode $null -DurationSeconds 0 -Note $SkipReason))
        Write-Host ''
        Write-Host '============================================================' -ForegroundColor Yellow
        Write-Host "[SKIPPED] $Name" -ForegroundColor Yellow
        Write-Host '============================================================' -ForegroundColor Yellow
        Write-Host $SkipReason -ForegroundColor Yellow
        return
    }

    Write-Host ''
    Write-Host '============================================================' -ForegroundColor Cyan
    Write-Host $Name -ForegroundColor Cyan
    Write-Host '============================================================' -ForegroundColor Cyan

    $stopwatch = [System.Diagnostics.Stopwatch]::StartNew()

    try {
        $actionResult = & $Action
        $stopwatch.Stop()

        $note = ''
        if ($null -ne $actionResult -and $actionResult.PSObject.Properties['Note']) {
            $note = [string]$actionResult.Note
        }

        $script:laneResults.Add((New-LaneRecord -Name $Name -Status 'PASSED' -Required $Required -ExitCode 0 -DurationSeconds $stopwatch.Elapsed.TotalSeconds -Note $note))
    }
    catch {
        $stopwatch.Stop()
        $exitCode = Get-ExceptionExitCode -ErrorRecord $_
        $note = Get-ExceptionNote -ErrorRecord $_
        $script:laneResults.Add((New-LaneRecord -Name $Name -Status 'FAILED' -Required $Required -ExitCode $exitCode -DurationSeconds $stopwatch.Elapsed.TotalSeconds -Note $note))
        Write-Host "[FAILED] $Name" -ForegroundColor Red
        Write-Host $note -ForegroundColor Red
    }
}

$scriptDir = if ($PSScriptRoot) { $PSScriptRoot } else { Split-Path -Parent $MyInvocation.MyCommand.Path }
$gitFromPath = Get-CommandPath -Name 'git'

$repoRoot = $null
if ($gitFromPath) {
    try {
        $repoRootCandidate = (& $gitFromPath -C $scriptDir rev-parse --show-toplevel 2>$null)
        if ($LASTEXITCODE -eq 0 -and -not [string]::IsNullOrWhiteSpace($repoRootCandidate)) {
            $repoRoot = ($repoRootCandidate | Select-Object -First 1).Trim()
        }
    }
    catch {
        $repoRoot = $null
    }
}

if ([string]::IsNullOrWhiteSpace($repoRoot)) {
    $repoRoot = $scriptDir
}

$coreRsDir = Join-Path $repoRoot 'core-rs'
$guiDir = Join-Path $coreRsDir 'mqk-gui'
$cargoManifest = Join-Path $coreRsDir 'Cargo.toml'
$lockDoc = Join-Path $repoRoot 'docs/INSTITUTIONAL_READINESS_LOCK.md'
$scorecardDoc = Join-Path $repoRoot 'docs/INSTITUTIONAL_SCORECARD.md'
$ignoredGuard = Join-Path $repoRoot 'scripts/guards/check_ignored_load_bearing_proofs.sh'
$unsafeGuard = Join-Path $repoRoot 'scripts/guards/check_unsafe_patterns.sh'
$dbBootstrap = Join-Path $repoRoot 'scripts/db_proof_bootstrap.sh'
$dbUrl = $env:MQK_DATABASE_URL

$commitHash = '<unknown>'
$gitStatusShort = @()
$treeClean = $false
$untrackedFilesPresent = $false
$dbRequired = ($ProofProfile -eq 'full')
$dbAvailable = -not [string]::IsNullOrWhiteSpace($dbUrl)
$canonicalFullBundleRequested = ($ProofProfile -eq 'full')
$workspaceState = 'candidate_workspace_state'
$proofAuditProfile = switch ($ProofProfile) {
    'full' { 'full_db_backed_institutional_proof_audit' }
    'exploratory' { 'candidate_workspace_exploratory_proof' }
    default { 'local_non_db_proof_audit' }
}

$alwaysRequiredLaneNames = @(
    'Repo identity + working tree truth',
    'Rust fmt check (non-mutating)',
    'Workspace clippy',
    'Workspace tests',
    'Daemon proof lanes',
    'Runtime proof lane',
    'Broker Alpaca proof lane',
    'Market data proof lane',
    'GUI typecheck + build',
    'Ignored-proof guard',
    'Unsafe-pattern guard'
)

$mandatoryDbLaneNames = @(
    'DB proof bootstrap / CI-10 mandatory matrix',
    'DB-backed mqk-db ignored lanes',
    'DB-backed daemon lifecycle ignored lane',
    'DB-backed daemon routes ignored lane',
    'DB-backed market data ingest-provider ignored lane',
    'Broker map FK proof'
)

$expectedRequiredLaneNames = @($alwaysRequiredLaneNames)
if ($dbRequired) {
    $expectedRequiredLaneNames += $mandatoryDbLaneNames
}

$nonDbSkipReason = if (-not $dbRequired) { 'Skipped in non-DB profile.' } else { $null }

try {
    $script:GitExe = Require-Command -Name 'git' -Purpose 'repo identity and cleanliness checks'
    $script:CargoExe = Require-Command -Name 'cargo' -Purpose 'Rust proof lanes'
    $script:NpxExe = Require-Command -Name 'npx' -Purpose 'GUI typecheck lane'
    $script:NpmExe = Require-Command -Name 'npm' -Purpose 'GUI build lane'
    $script:BashExe = Resolve-CompatibleRepoBash -GitExe $script:GitExe

    Require-Path -PathToCheck $repoRoot -Label 'repo root'
    Require-Path -PathToCheck $coreRsDir -Label 'core-rs workspace'
    Require-Path -PathToCheck $guiDir -Label 'GUI workspace'
    Require-Path -PathToCheck $cargoManifest -Label 'Rust workspace manifest'
    Require-Path -PathToCheck $lockDoc -Label 'institutional readiness lock'
    Require-Path -PathToCheck $scorecardDoc -Label 'institutional scorecard'
    Require-Path -PathToCheck $ignoredGuard -Label 'ignored-proof guard'
    Require-Path -PathToCheck $unsafeGuard -Label 'unsafe-pattern guard'
    Require-Path -PathToCheck $dbBootstrap -Label 'DB proof bootstrap helper'

    if ($dbRequired -and [string]::IsNullOrWhiteSpace($dbUrl)) {
        throw 'MQK_DATABASE_URL is not set. Full DB-backed institutional proof cannot proceed and will not be downgraded silently.'
    }

    Write-Host ''
    Write-Host '============================================================' -ForegroundColor Green
    Write-Host 'MiniQuantDesk V4 proof harness preflight' -ForegroundColor Green
    Write-Host '============================================================' -ForegroundColor Green
    Write-Host "Proof profile: $ProofProfile" -ForegroundColor Yellow
    Write-Host "Repo root:      $repoRoot" -ForegroundColor Yellow
    Write-Host "core-rs:        $coreRsDir" -ForegroundColor Yellow
    Write-Host "GUI dir:        $guiDir" -ForegroundColor Yellow
    Write-Host "Readiness lock: $lockDoc" -ForegroundColor Yellow
    Write-Host "Scorecard:      $scorecardDoc" -ForegroundColor Yellow
    Write-Host "Repo shell:     $script:BashExe" -ForegroundColor Yellow
    Write-Host ("MQK_DATABASE_URL={0}" -f (Get-RedactedDbUrl -Url $dbUrl)) -ForegroundColor Green
}
catch {
    Write-Host ''
    Write-Host '============================================================' -ForegroundColor Red
    Write-Host 'MiniQuantDesk V4 proof harness preflight FAILED' -ForegroundColor Red
    Write-Host '============================================================' -ForegroundColor Red
    Write-Host $_.Exception.Message -ForegroundColor Red
    exit 1
}

Invoke-ProofLane -Name 'Repo identity + working tree truth' -Required $true -Action {
    $commitHashOutput = (& $script:GitExe -C $repoRoot rev-parse HEAD)
    if ($LASTEXITCODE -ne 0) {
        throw 'EXITCODE=1;Unable to resolve git rev-parse HEAD.'
    }
    $script:commitHash = ($commitHashOutput | Select-Object -First 1).Trim()

    $statusOutput = (& $script:GitExe -C $repoRoot status --short --untracked-files=all)
    if ($LASTEXITCODE -ne 0) {
        throw 'EXITCODE=1;Unable to resolve git status --short --untracked-files=all.'
    }

    if ($null -eq $statusOutput) {
        $statusLines = @()
    }
    elseif ($statusOutput -is [System.Array]) {
        $statusLines = @($statusOutput)
    }
    else {
        $statusLines = @([string]$statusOutput)
    }

    $script:gitStatusShort = @($statusLines | ForEach-Object { $_.TrimEnd() } | Where-Object { $_ -ne '' })
    $script:treeClean = ($script:gitStatusShort.Count -eq 0)
    $script:untrackedFilesPresent = (@($script:gitStatusShort | Where-Object { $_ -match '^\?\?\s' }).Count -gt 0)
    $script:workspaceState = if ($script:treeClean) { 'committed_repo_state' } else { 'candidate_workspace_state' }

    Write-Host "Commit hash: $script:commitHash" -ForegroundColor Yellow
    Write-Host "Tree clean:  $script:treeClean" -ForegroundColor Yellow
    Write-Host "Untracked:   $script:untrackedFilesPresent" -ForegroundColor Yellow
    Write-Host 'git status --short --untracked-files=all:' -ForegroundColor Yellow
    if ($script:gitStatusShort.Count -eq 0) {
        Write-Host '  <clean>' -ForegroundColor Green
    }
    else {
        foreach ($line in $script:gitStatusShort) {
            Write-Host ("  {0}" -f $line) -ForegroundColor Yellow
        }
    }

    return (New-LaneNote -Note ("commit={0}; tree_clean={1}; untracked={2}" -f $script:commitHash, $script:treeClean, $script:untrackedFilesPresent))
}

Invoke-ProofLane -Name 'Rust fmt check (non-mutating)' -Required $true -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('fmt', '--manifest-path', $cargoManifest, '--all', '--', '--check') -WorkingDirectory $repoRoot
    Write-Host 'cargo fmt --check passed.' -ForegroundColor Green
    return (New-LaneNote -Note 'cargo fmt --check passed.')
}

Invoke-ProofLane -Name 'Workspace clippy' -Required $true -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('clippy', '--manifest-path', $cargoManifest, '--workspace', '--all-targets', '--', '-D', 'warnings') -WorkingDirectory $repoRoot
}

Invoke-ProofLane -Name 'Workspace tests' -Required $true -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '--workspace', '--', '--test-threads=1') -WorkingDirectory $repoRoot
}

Invoke-ProofLane -Name 'Daemon proof lanes' -Required $true -Action {
    $daemonTests = @(
        'scenario_daemon_routes',
        'scenario_gui_daemon_contract_gate',
        'scenario_snapshot_inject_release_gate',
        'scenario_token_auth_middleware',
        'scenario_daemon_boot_is_fail_closed',
        'scenario_daemon_deadman_blocks_dispatch',
        'scenario_reconcile_tick_disarms_on_drift',
        'scenario_daemon_runtime_lifecycle'
    )

    foreach ($testName in $daemonTests) {
        Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '-p', 'mqk-daemon', '--test', $testName, '--', '--test-threads=1') -WorkingDirectory $repoRoot
    }
}

Invoke-ProofLane -Name 'Runtime proof lane' -Required $true -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '-p', 'mqk-runtime', '--', '--test-threads=1') -WorkingDirectory $repoRoot
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('clippy', '--manifest-path', $cargoManifest, '-p', 'mqk-runtime', '--all-targets', '--', '-D', 'warnings') -WorkingDirectory $repoRoot
}

Invoke-ProofLane -Name 'Broker Alpaca proof lane' -Required $true -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '-p', 'mqk-broker-alpaca', '--', '--test-threads=1') -WorkingDirectory $repoRoot
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('clippy', '--manifest-path', $cargoManifest, '-p', 'mqk-broker-alpaca', '--all-targets', '--', '-D', 'warnings') -WorkingDirectory $repoRoot
}

Invoke-ProofLane -Name 'Market data proof lane' -Required $true -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '-p', 'mqk-md', '--', '--test-threads=1') -WorkingDirectory $repoRoot
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('clippy', '--manifest-path', $cargoManifest, '-p', 'mqk-md', '--all-targets', '--', '-D', 'warnings') -WorkingDirectory $repoRoot
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '-p', 'mqk-db', '--test', 'scenario_md_ingest_provider', '--', '--test-threads=1') -WorkingDirectory $repoRoot
}

Invoke-ProofLane -Name 'GUI typecheck + build' -Required $true -Action {
    Invoke-NativeCommand -FilePath $script:NpxExe -Arguments @('tsc', '--noEmit') -WorkingDirectory $guiDir
    Invoke-NativeCommand -FilePath $script:NpmExe -Arguments @('run', 'build') -WorkingDirectory $guiDir
}

Invoke-ProofLane -Name 'Ignored-proof guard' -Required $true -Action {
    Invoke-RepoBashScript -BashExe $script:BashExe -RepoRoot $repoRoot -ScriptPath $ignoredGuard
    Write-Host 'Ignored-proof guard passed.' -ForegroundColor Green
    return (New-LaneNote -Note 'Ignored-proof guard passed.')
}

Invoke-ProofLane -Name 'Unsafe-pattern guard' -Required $true -Action {
    Invoke-RepoBashScript -BashExe $script:BashExe -RepoRoot $repoRoot -ScriptPath $unsafeGuard
    Write-Host 'Unsafe-pattern guard passed.' -ForegroundColor Green
    return (New-LaneNote -Note 'Unsafe-pattern guard passed.')
}

Invoke-ProofLane -Name 'DB proof bootstrap / CI-10 mandatory matrix' -Required $dbRequired -Action {
    Invoke-RepoBashScript -BashExe $script:BashExe -RepoRoot $repoRoot -ScriptPath $dbBootstrap
    Write-Host 'DB proof bootstrap / CI-10 mandatory matrix passed.' -ForegroundColor Green
    return (New-LaneNote -Note 'DB proof bootstrap / CI-10 mandatory matrix passed.')
} -SkipReason $nonDbSkipReason

Invoke-ProofLane -Name 'DB-backed mqk-db ignored lanes' -Required $dbRequired -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '-p', 'mqk-db', '--', '--include-ignored', '--test-threads=1') -WorkingDirectory $repoRoot
} -SkipReason $nonDbSkipReason

Invoke-ProofLane -Name 'DB-backed daemon lifecycle ignored lane' -Required $dbRequired -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '-p', 'mqk-daemon', '--test', 'scenario_daemon_runtime_lifecycle', '--', '--include-ignored', '--test-threads=1') -WorkingDirectory $repoRoot
} -SkipReason $nonDbSkipReason

Invoke-ProofLane -Name 'DB-backed daemon routes ignored lane' -Required $dbRequired -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '-p', 'mqk-daemon', '--test', 'scenario_daemon_routes', '--', '--include-ignored', '--test-threads=1') -WorkingDirectory $repoRoot
} -SkipReason $nonDbSkipReason

Invoke-ProofLane -Name 'DB-backed market data ingest-provider ignored lane' -Required $dbRequired -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '-p', 'mqk-db', '--test', 'scenario_md_ingest_provider', '--', '--include-ignored', '--test-threads=1') -WorkingDirectory $repoRoot
} -SkipReason $nonDbSkipReason

Invoke-ProofLane -Name 'Broker map FK proof' -Required $dbRequired -Action {
    Invoke-NativeCommand -FilePath $script:CargoExe -Arguments @('test', '--manifest-path', $cargoManifest, '-p', 'mqk-db', '--test', 'scenario_broker_map_fk_enforced', '--', '--test-threads=1') -WorkingDirectory $repoRoot
} -SkipReason $nonDbSkipReason

$allLaneNames = @($laneResults | ForEach-Object { $_.lane_name })

$passedLanes = @($laneResults | Where-Object { $_.status -eq 'PASSED' })
$failedLanes = @($laneResults | Where-Object { $_.status -eq 'FAILED' })
$ignoredLanes = @($laneResults | Where-Object { $_.status -eq 'IGNORED' })
$skippedLanes = @($laneResults | Where-Object { $_.status -eq 'SKIPPED' })

$requiredLaneResults = @($laneResults | Where-Object { $expectedRequiredLaneNames -contains $_.lane_name })
$requiredLaneNamesPresent = @($requiredLaneResults | ForEach-Object { $_.lane_name })
$missingRequiredLanes = @($expectedRequiredLaneNames | Where-Object { $requiredLaneNamesPresent -notcontains $_ })
$requiredLaneResultsPresent = ($missingRequiredLanes.Count -eq 0 -and $requiredLaneResults.Count -eq $expectedRequiredLaneNames.Count)

$failedRequiredLanes = @($requiredLaneResults | Where-Object { $_.status -eq 'FAILED' })
$skippedRequiredLanes = @($requiredLaneResults | Where-Object { $_.status -eq 'SKIPPED' })

$requiredLanesExecuted = ($requiredLaneResultsPresent -and $skippedRequiredLanes.Count -eq 0)
$requiredLanesCompleted = ($requiredLaneResultsPresent -and $failedRequiredLanes.Count -eq 0 -and $skippedRequiredLanes.Count -eq 0)

$mandatoryDbLaneResults = @($laneResults | Where-Object { $mandatoryDbLaneNames -contains $_.lane_name })
$mandatoryDbLaneNamesPresent = @($mandatoryDbLaneResults | ForEach-Object { $_.lane_name })
$missingMandatoryDbLanes = @()
if ($dbRequired) {
    $missingMandatoryDbLanes = @($mandatoryDbLaneNames | Where-Object { $mandatoryDbLaneNamesPresent -notcontains $_ })
}

$mandatoryDbResultsPresent = (
    (-not $dbRequired) -or
    ($missingMandatoryDbLanes.Count -eq 0 -and $mandatoryDbLaneResults.Count -eq $mandatoryDbLaneNames.Count)
)

$failedMandatoryDbLanes = @()
$skippedMandatoryDbLanes = @()
if ($dbRequired) {
    $failedMandatoryDbLanes = @($mandatoryDbLaneResults | Where-Object { $_.status -eq 'FAILED' })
    $skippedMandatoryDbLanes = @($mandatoryDbLaneResults | Where-Object { $_.status -eq 'SKIPPED' })
}

$mandatoryDbLanesExecuted = (
    $dbRequired -and
    $mandatoryDbResultsPresent -and
    $skippedMandatoryDbLanes.Count -eq 0
)

$mandatoryDbLanesCompleted = (
    $dbRequired -and
    $mandatoryDbResultsPresent -and
    $failedMandatoryDbLanes.Count -eq 0 -and
    $skippedMandatoryDbLanes.Count -eq 0
)

$canonicalFullBundleExecuted = ($canonicalFullBundleRequested -and $requiredLanesExecuted)
$canonicalFullBundleCompleted = ($canonicalFullBundleRequested -and $requiredLanesCompleted)

$requiredCompletenessIssues = @()
$requiredCompletenessIssues += $missingRequiredLanes
$requiredCompletenessIssues += @($skippedRequiredLanes | ForEach-Object { $_.lane_name })
$requiredCompletenessIssues += @($failedRequiredLanes | ForEach-Object { $_.lane_name })

$hasRequiredExecutionFailure = ($requiredCompletenessIssues.Count -gt 0)

$institutionalReadyProofCompleted = (
    $ProofProfile -eq 'full' -and
    $treeClean -and
    $dbAvailable -and
    $canonicalFullBundleCompleted -and
    $mandatoryDbLanesCompleted -and
    -not $hasRequiredExecutionFailure
)

$resultCounts = @(
    [pscustomobject]@{ status = 'PASSED'; count = $passedLanes.Count },
    [pscustomobject]@{ status = 'FAILED'; count = $failedLanes.Count },
    [pscustomobject]@{ status = 'IGNORED'; count = $ignoredLanes.Count },
    [pscustomobject]@{ status = 'SKIPPED'; count = $skippedLanes.Count }
)

$summary = [ordered]@{
    proof_profile                       = $ProofProfile
    audit_profile                       = $proofAuditProfile
    workspace_state                     = $workspaceState
    repo_root                           = $repoRoot
    commit_hash                         = $commitHash
    tree_clean                          = $treeClean
    untracked_files_present             = $untrackedFilesPresent
    db_required                         = $dbRequired
    db_available                        = $dbAvailable
    canonical_full_bundle_requested     = $canonicalFullBundleRequested
    canonical_full_bundle_executed      = $canonicalFullBundleExecuted
    canonical_full_bundle_completed     = $canonicalFullBundleCompleted
    required_lanes_executed             = $requiredLanesExecuted
    required_lanes_completed            = $requiredLanesCompleted
    missing_required_lanes              = @($missingRequiredLanes)
    mandatory_db_lanes_executed         = $mandatoryDbLanesExecuted
    mandatory_db_lanes_completed        = $mandatoryDbLanesCompleted
    missing_mandatory_db_lanes          = @($missingMandatoryDbLanes)
    institutional_ready_proof_completed = $institutionalReadyProofCompleted
    readiness_lock_doc                  = 'docs/INSTITUTIONAL_READINESS_LOCK.md'
    scorecard_doc                       = 'docs/INSTITUTIONAL_SCORECARD.md'
    passed_lanes                        = @($passedLanes | ForEach-Object { $_.lane_name })
    failed_lanes                        = @($failedLanes | ForEach-Object { $_.lane_name })
    ignored_lanes                       = @($ignoredLanes | ForEach-Object { $_.lane_name })
    skipped_lanes                       = @($skippedLanes | ForEach-Object { $_.lane_name })
    failed_required_lanes               = @($failedRequiredLanes | ForEach-Object { $_.lane_name })
    skipped_required_lanes              = @($skippedRequiredLanes | ForEach-Object { $_.lane_name })
    overall_result                      = if ($hasRequiredExecutionFailure) { 'failed' } else { 'passed' }
    lane_results                        = @($laneResults)
}

Write-Host ''
Write-Host '============================================================' -ForegroundColor Green
Write-Host 'MiniQuantDesk V4 proof lane summary table' -ForegroundColor Green
Write-Host '============================================================' -ForegroundColor Green
$laneResults |
    Select-Object lane_name, status, required, exit_code, duration_seconds, note |
    Format-Table -AutoSize |
    Out-String -Width 240 |
    Write-Host

Write-Host ''
Write-Host '============================================================' -ForegroundColor Green
Write-Host 'MiniQuantDesk V4 lane result counts' -ForegroundColor Green
Write-Host '============================================================' -ForegroundColor Green
$resultCounts |
    Format-Table -AutoSize |
    Out-String -Width 120 |
    Write-Host

Write-Host ''
Write-Host '============================================================' -ForegroundColor Green
Write-Host 'MiniQuantDesk V4 required lane completeness' -ForegroundColor Green
Write-Host '============================================================' -ForegroundColor Green
if ($missingRequiredLanes.Count -eq 0 -and $skippedRequiredLanes.Count -eq 0 -and $failedRequiredLanes.Count -eq 0) {
    Write-Host '<complete>' -ForegroundColor Green
}
else {
    if ($missingRequiredLanes.Count -gt 0) {
        Write-Host 'Missing required lanes:' -ForegroundColor Red
        $missingRequiredLanes |
            ForEach-Object { [pscustomobject]@{ lane_name = $_ } } |
            Format-Table -AutoSize |
            Out-String -Width 180 |
            Write-Host
    }

    if ($skippedRequiredLanes.Count -gt 0) {
        Write-Host 'Skipped required lanes:' -ForegroundColor Yellow
        $skippedRequiredLanes |
            Select-Object lane_name, exit_code, duration_seconds, note |
            Format-Table -AutoSize |
            Out-String -Width 240 |
            Write-Host
    }

    if ($failedRequiredLanes.Count -gt 0) {
        Write-Host 'Failed required lanes:' -ForegroundColor Red
        $failedRequiredLanes |
            Select-Object lane_name, exit_code, duration_seconds, note |
            Format-Table -AutoSize |
            Out-String -Width 240 |
            Write-Host
    }
}

Write-Host ''
Write-Host '============================================================' -ForegroundColor Green
Write-Host 'MiniQuantDesk V4 proof summary' -ForegroundColor Green
Write-Host '============================================================' -ForegroundColor Green
Write-Host (ConvertTo-Json $summary -Depth 6)

if ($institutionalReadyProofCompleted) {
    Write-Host 'VERDICT: Full DB-backed canonical proof completed on a clean committed state.' -ForegroundColor Green
}
elseif ($ProofProfile -eq 'full' -and $missingRequiredLanes.Count -gt 0) {
    Write-Host 'VERDICT: Full profile is invalid; one or more expected required lanes are missing from the harness result set.' -ForegroundColor Red
}
elseif ($ProofProfile -eq 'full' -and $skippedRequiredLanes.Count -gt 0) {
    Write-Host 'VERDICT: Full profile is incomplete; one or more required lanes were skipped; institutional readiness is NOT established.' -ForegroundColor Red
}
elseif ($ProofProfile -eq 'full' -and $failedRequiredLanes.Count -gt 0) {
    Write-Host 'VERDICT: Full profile was executed, but one or more required lanes failed; institutional readiness is NOT established.' -ForegroundColor Red
}
elseif ($ProofProfile -eq 'full') {
    Write-Host 'VERDICT: Full institutional-ready proof was NOT completed.' -ForegroundColor Red
}
elseif ($ProofProfile -eq 'exploratory') {
    Write-Host 'VERDICT: Exploratory proof only; not a canonical institutional-ready audit.' -ForegroundColor Yellow
}
else {
    Write-Host 'VERDICT: Local non-DB proof only; DB-backed institutional readiness not established.' -ForegroundColor Yellow
}

if ($hasRequiredExecutionFailure) {
    exit 1
}

exit 0