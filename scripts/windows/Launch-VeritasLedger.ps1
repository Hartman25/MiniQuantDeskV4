[CmdletBinding()]
param(
    [ValidateSet('Observe', 'TradeReady')]
    [string]$Mode = 'Observe',
    # -RebuildGui: force GUI rebuild only (safe when daemon is already running)
    [switch]$RebuildGui,
    # -RebuildDaemon: force daemon rebuild only (requires daemon not locked)
    [switch]$RebuildDaemon,
    # -RebuildAll: force rebuild of both daemon and GUI
    [switch]$RebuildAll,
    # -Rebuild: legacy alias for -RebuildAll (backward compatibility)
    [switch]$Rebuild,

    # Market-data gate parameters
    # -CheckMarketData: verify data meets bar-count/freshness gates; fail launcher if not met.
    #   Does not ingest data. Safe to run anytime.
    [switch]$CheckMarketData,
    # -PrepMarketData: ingest backup CSV + provider top-off, then verify gates.
    #   Requires TWELVEDATA_API_KEY for provider top-off (warns but does not fail if absent).
    [switch]$PrepMarketData,
    # -Symbols: comma-separated ticker list for market-data gate.
    #   Default: MQK_STRATEGY_SYMBOL env var, or 'AAPL'.
    [string]$Symbols = '',
    # -Timeframe: bar timeframe for market-data gate.
    #   Default: MQK_STRATEGY_MD_TIMEFRAME env var, or '1D'.
    [string]$Timeframe = '',
    # -MinCompletedBars: minimum completed bars required per symbol.
    #   Default: 30.
    [int]$MinCompletedBars = 30,
    # -MaxStalenessDays: maximum calendar days since latest complete bar.
    #   Default: auto-derived from timeframe (1 for 5m; 4 for 1D and others).
    [int]$MaxStalenessDays = -1,

    # -ArmPaper: arm the paper execution gate after daemon identity is verified.
    #   Requires paper mode, adapter alpaca, live_routing_enabled=false, and a
    #   trade-ready backend (reconcile clean, ws_continuity_ready, arm_ready).
    #   Does NOT start the runtime. Does NOT submit orders. Fail-closed.
    [switch]$ArmPaper,

    # -CaptureStartupEvidence: capture a startup evidence bundle after the daemon
    #   is verified and the GUI is launched.
    #   Calls Capture-PaperSmokeEvidence.ps1 -Label launcher_startup.
    #   Non-fatal: a capture failure warns but does not abort the launcher.
    [switch]$CaptureStartupEvidence,

    # -SkipGui: OFFICIAL-DUAL-MODE-LAUNCHER-01 -- skip resolving/launching the
    #   desktop GUI. For headless/scheduled attach where no interactive
    #   desktop session should receive a GUI window. Daemon start/verify
    #   behavior is unaffected.
    [switch]$SkipGui,

    # -CheckOnly: read-only startup status report (PAPER-OPERATOR-STARTUP-LAUNCHER-01).
    #   Prints repo/.env.local/Docker/paper-DB/daemon/GUI/AAPL bars/sys_arm_state/
    #   live-daemon-truth status and a "Next action" recommendation, then exits.
    #   Does not start, build, arm, or call the daemon, GUI, broker, or Discord.
    #   Does not require .env.local or MQK_OPERATOR_TOKEN to be present.
    [switch]$CheckOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-LauncherStep {
    param([Parameter(Mandatory = $true)][string]$Message)
    Write-Host "[Veritas Ledger] $Message" -ForegroundColor Cyan
}

function Write-LauncherSuccess {
    param([Parameter(Mandatory = $true)][string]$Message)
    Write-Host "[Veritas Ledger] $Message" -ForegroundColor Green
}

function Write-LauncherWarn {
    param([Parameter(Mandatory = $true)][string]$Message)
    Write-Host "[Veritas Ledger] $Message" -ForegroundColor Yellow
}

function Get-RepoRoot {
    return (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}

function Get-CommandPath {
    param([Parameter(Mandatory = $true)][string]$Name)
    return (Get-Command $Name -ErrorAction Stop).Source
}

function Invoke-ExternalCommand {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [string[]]$Arguments = @(),
        [string]$WorkingDirectory = (Get-Location).Path,
        [switch]$AllowFailure
    )

    $argText = if ($Arguments -and $Arguments.Count -gt 0) {
        $Arguments -join ' '
    } else {
        ''
    }

    if ([string]::IsNullOrWhiteSpace($argText)) {
        Write-LauncherStep "$FilePath"
    } else {
        Write-LauncherStep "$FilePath $argText"
    }

    Push-Location $WorkingDirectory
    try {
        & $FilePath @Arguments | Out-Host
        if (-not $AllowFailure -and $LASTEXITCODE -ne 0) {
            throw ("Command failed with exit code {0}: {1} {2}" -f $LASTEXITCODE, $FilePath, $argText)
        }
    } finally {
        Pop-Location
    }
}

function Resolve-DaemonBinary {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)

    $candidate = Join-Path $RepoRoot 'core-rs\target\release\mqk-daemon.exe'
    if (Test-Path $candidate) {
        return (Resolve-Path $candidate).Path
    }

    throw "mqk-daemon.exe was not found at expected path: $candidate"
}

function Resolve-GuiBinary {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)

    $candidates = @(
        (Join-Path $RepoRoot 'core-rs\target\release\mqk-gui.exe'),
        (Join-Path $RepoRoot 'core-rs\mqk-gui\src-tauri\target\release\mqk-gui.exe')
    )

    foreach ($candidate in $candidates) {
        if (Test-Path $candidate) {
            return (Resolve-Path $candidate).Path
        }
    }

    return $null
}

function Test-GuiBinaryFreshness {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [string]$GuiExe
    )

    # Missing binary is always stale
    if ([string]::IsNullOrWhiteSpace($GuiExe) -or -not (Test-Path $GuiExe)) {
        return $true
    }

    $binaryTime = (Get-Item $GuiExe).LastWriteTimeUtc
    $guiRoot = Join-Path $RepoRoot 'core-rs\mqk-gui'

    # Scan source/config files; exclude generated/build directories
    $sourceFiles = Get-ChildItem -Path $guiRoot -Recurse -File -ErrorAction SilentlyContinue |
        Where-Object { $_.FullName -notmatch '\\(node_modules|dist|target)\\' }

    $latestSource = $null
    foreach ($file in $sourceFiles) {
        if ($null -eq $latestSource -or $file.LastWriteTimeUtc -gt $latestSource) {
            $latestSource = $file.LastWriteTimeUtc
        }
    }

    if ($null -eq $latestSource) {
        # No source files found — treat as fresh to avoid spurious rebuilds
        return $false
    }

    return ($latestSource -gt $binaryTime)
}

function Ensure-NodeModules {
    param([Parameter(Mandatory = $true)][string]$GuiRoot)

    $nodeModules = Join-Path $GuiRoot 'node_modules'
    if (Test-Path $nodeModules) {
        return
    }

    $npm = Get-CommandPath 'npm.cmd'
    Write-LauncherStep 'Installing mqk-gui node_modules (npm ci)'
    Invoke-ExternalCommand -FilePath $npm -Arguments @('ci') -WorkingDirectory $GuiRoot
}

function Get-DaemonBuildProvenancePath {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)

    return Join-Path $RepoRoot 'core-rs\target\release\mqk-daemon.build-tree.txt'
}

# STALE-DAEMON-BINARY-PROVENANCE-01: deterministic build-identity check.
#
# Root cause (Thursday 2026-08-13 unattended failure): a reused
# mqk-daemon.exe predated the required-universe/retry functionality that
# had landed later the same day. Filesystem mtimes are not proof of
# correspondence to the current Rust workspace -- a binary can be newer
# than a stale checkout, or a working tree can be dirty in ways mtime
# cannot distinguish. The git tree SHA of core-rs is deterministic and
# exactly identifies the source the binary was (or wasn't) built from.
#
# Contract:
#   - Uses `git rev-parse HEAD:core-rs` as the current build identity.
#   - If core-rs has uncommitted changes, the identity is suffixed with
#     "-dirty" so a dirty rebuild is never mistaken for a committed one.
#   - A sidecar file next to the binary records the identity the binary
#     was actually built from. Reuse requires an exact string match.
#   - Any inability to resolve git identity fails closed to rebuild --
#     never trusts an unproven existing binary.
function Get-CoreRsTreeIdentity {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)

    try {
        $git = Get-CommandPath 'git'
    }
    catch {
        return $null
    }

    $treeSha = & $git -C $RepoRoot rev-parse 'HEAD:core-rs' 2>$null
    if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($treeSha)) {
        return $null
    }
    $treeSha = $treeSha.Trim()

    $statusOutput = & $git -C $RepoRoot status --porcelain -- core-rs 2>$null
    if ($LASTEXITCODE -ne 0) {
        return $null
    }

    if ($statusOutput -and @($statusOutput | Where-Object { $_ -and $_.Trim().Length -gt 0 }).Count -gt 0) {
        return "$treeSha-dirty"
    }

    return $treeSha
}

function Test-DaemonBinaryProvenanceMatches {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$DaemonExePath
    )

    if (-not (Test-Path $DaemonExePath)) {
        return $false
    }

    $sidecarPath = Get-DaemonBuildProvenancePath -RepoRoot $RepoRoot
    if (-not (Test-Path $sidecarPath)) {
        return $false
    }

    $currentIdentity = Get-CoreRsTreeIdentity -RepoRoot $RepoRoot
    if ($null -eq $currentIdentity) {
        # Cannot prove identity -- fail closed to rebuild rather than trust
        # an unproven existing binary.
        return $false
    }

    $recordedIdentity = (Get-Content -Path $sidecarPath -Raw -ErrorAction SilentlyContinue)
    if ($null -eq $recordedIdentity) {
        return $false
    }
    $recordedIdentity = $recordedIdentity.Trim()

    return ($recordedIdentity -eq $currentIdentity)
}

function Write-DaemonBuildProvenance {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot
    )

    $identity = Get-CoreRsTreeIdentity -RepoRoot $RepoRoot
    if ($null -eq $identity) {
        Write-LauncherWarn 'Could not resolve core-rs git tree identity after build; provenance sidecar not written. Next launch will rebuild.'
        return
    }

    $sidecarPath = Get-DaemonBuildProvenancePath -RepoRoot $RepoRoot
    Set-Content -Path $sidecarPath -Value $identity -NoNewline -Encoding ASCII
    Write-LauncherStep "Recorded daemon build provenance: $identity"
}

function Ensure-DaemonBinary {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][bool]$ForceRebuild
    )

    if ($ForceRebuild) {
        $cargo = Get-CommandPath 'cargo'
        Write-LauncherStep 'Rebuilding mqk-daemon --release'
        Invoke-ExternalCommand -FilePath $cargo -Arguments @('build', '-p', 'mqk-daemon', '--release') -WorkingDirectory (Join-Path $RepoRoot 'core-rs')
        Write-DaemonBuildProvenance -RepoRoot $RepoRoot
        return Resolve-DaemonBinary -RepoRoot $RepoRoot
    }

    $daemonExePath = Join-Path $RepoRoot 'core-rs\target\release\mqk-daemon.exe'
    if (Test-DaemonBinaryProvenanceMatches -RepoRoot $RepoRoot -DaemonExePath $daemonExePath) {
        Write-LauncherStep 'Daemon rebuild skipped; binary provenance matches current core-rs tree (use -RebuildDaemon or -RebuildAll to force)'
        return Resolve-DaemonBinary -RepoRoot $RepoRoot
    }

    if (Test-Path $daemonExePath) {
        Write-LauncherStep 'Daemon binary present but provenance is missing or does not match current core-rs tree; rebuilding mqk-daemon --release'
    }
    else {
        Write-LauncherStep 'Daemon binary missing; building mqk-daemon --release'
    }
    $cargo = Get-CommandPath 'cargo'
    Invoke-ExternalCommand -FilePath $cargo -Arguments @('build', '-p', 'mqk-daemon', '--release') -WorkingDirectory (Join-Path $RepoRoot 'core-rs')
    Write-DaemonBuildProvenance -RepoRoot $RepoRoot
    return Resolve-DaemonBinary -RepoRoot $RepoRoot
}

function Ensure-GuiBinary {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][bool]$ForceRebuild
    )

    $existing = Resolve-GuiBinary -RepoRoot $RepoRoot

    if (-not $ForceRebuild) {
        if ($null -eq $existing) {
            Write-LauncherStep 'GUI binary missing; building'
        }
        elseif (Test-GuiBinaryFreshness -RepoRoot $RepoRoot -GuiExe $existing) {
            Write-LauncherStep 'GUI source newer than binary; rebuilding'
        }
        else {
            Write-LauncherSuccess 'GUI binary fresh; reusing'
            return $existing
        }
    }
    else {
        Write-LauncherStep 'GUI rebuild requested; building'
    }

    $guiRoot = Join-Path $RepoRoot 'core-rs\mqk-gui'
    Ensure-NodeModules -GuiRoot $guiRoot

    $npm = Get-CommandPath 'npm.cmd'
    Write-LauncherStep 'Building desktop GUI executable (npm run tauri build -- --no-bundle)'
    Invoke-ExternalCommand -FilePath $npm -Arguments @('run', 'tauri', 'build', '--', '--no-bundle') -WorkingDirectory $guiRoot

    $built = Resolve-GuiBinary -RepoRoot $RepoRoot
    if (-not $built) {
        throw 'Desktop GUI build completed but no launchable GUI executable was found.'
    }

    return $built
}

function New-EnvSnapshot {
    param([Parameter(Mandatory = $true)][string[]]$Names)

    $snapshot = @{}
    foreach ($name in $Names) {
        $snapshot[$name] = [Environment]::GetEnvironmentVariable($name, 'Process')
    }
    return $snapshot
}

function Restore-EnvSnapshot {
    param([Parameter(Mandatory = $true)][hashtable]$Snapshot)

    foreach ($entry in $Snapshot.GetEnumerator()) {
        if ($null -eq $entry.Value) {
            Remove-Item "Env:$($entry.Key)" -ErrorAction SilentlyContinue
        }
        else {
            Set-Item "Env:$($entry.Key)" -Value $entry.Value
        }
    }
}

function Parse-DotEnvLine {
    param([Parameter(Mandatory = $true)][string]$Line)

    $trimmed = $Line.Trim()
    if (-not $trimmed) { return $null }
    if ($trimmed.StartsWith('#')) { return $null }

    $idx = $trimmed.IndexOf('=')
    if ($idx -lt 1) { return $null }

    $name = $trimmed.Substring(0, $idx).Trim()
    $value = $trimmed.Substring($idx + 1).Trim()

    if (-not $name) { return $null }

    if (($value.StartsWith('"') -and $value.EndsWith('"')) -or ($value.StartsWith("'") -and $value.EndsWith("'"))) {
        if ($value.Length -ge 2) {
            $value = $value.Substring(1, $value.Length - 2)
        }
    }

    return [pscustomobject]@{
        Name = $name
        Value = $value
    }
}

function Import-DotEnvIfPresent {
    param(
        [Parameter(Mandatory = $true)][string]$Path,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()]$ImportedNames
    )

    if (-not (Test-Path $Path)) {
        return
    }

    if ($null -eq $ImportedNames) {
        throw "ImportedNames cannot be null."
    }

    foreach ($line in Get-Content -Path $Path) {
        if ($null -eq $line) {
            continue
        }

        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }

        $entry = Parse-DotEnvLine -Line $line
        if ($null -eq $entry) {
            continue
        }

        if ($ImportedNames.Contains($entry.Name)) {
            continue
        }

        $existing = [Environment]::GetEnvironmentVariable($entry.Name, 'Process')
        if ($null -ne $existing -and $existing.Trim().Length -gt 0) {
            continue
        }

        Set-Item -Path ("Env:{0}" -f $entry.Name) -Value $entry.Value
        [void]$ImportedNames.Add($entry.Name)
    }

    Write-LauncherStep "Loaded launcher environment hints from $Path"
}

function Import-LauncherEnvironmentFiles {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)

    $importedNames = New-Object 'System.Collections.Generic.HashSet[string]' ([System.StringComparer]::OrdinalIgnoreCase)
    $candidates = @(
        (Join-Path $RepoRoot '.env.local'),
        (Join-Path $RepoRoot '.env'),
        (Join-Path $RepoRoot 'core-rs\.env.local'),
        (Join-Path $RepoRoot 'core-rs\.env'),
        (Join-Path $RepoRoot 'core-rs\mqk-gui\.env.local')
    )

    foreach ($candidate in $candidates) {
        Import-DotEnvIfPresent -Path $candidate -ImportedNames $importedNames
    }
}

function Get-EnvValue {
    param([Parameter(Mandatory = $true)][string]$Name)

    $processValue = [Environment]::GetEnvironmentVariable($Name, 'Process')
    if ($null -ne $processValue -and $processValue.Trim().Length -gt 0) {
        return $processValue
    }

    $userValue = [Environment]::GetEnvironmentVariable($Name, 'User')
    if ($null -ne $userValue -and $userValue.Trim().Length -gt 0) {
        return $userValue
    }

    $machineValue = [Environment]::GetEnvironmentVariable($Name, 'Machine')
    if ($null -ne $machineValue -and $machineValue.Trim().Length -gt 0) {
        return $machineValue
    }

    return $null
}

function Resolve-RequiredOperatorToken {
    $token = Get-EnvValue -Name 'MQK_OPERATOR_TOKEN'
    if ($null -eq $token -or $token.Trim().Length -eq 0) {
        throw 'MQK_OPERATOR_TOKEN is not configured. Desktop bootstrap fails closed until a real operator token is available in the environment or a local .env.local file.'
    }

    return $token.Trim()
}

function Set-LauncherEnvironment {
    param(
        [Parameter(Mandatory = $true)][string]$OperatorToken,
        [Parameter(Mandatory = $true)][string]$RepoRoot
    )

    $names = @(
        'MQK_DAEMON_DEPLOYMENT_MODE',
        'MQK_DAEMON_ADAPTER_ID',
        'MQK_DAEMON_ADDR',
        'MQK_GUI_DAEMON_URL',
        'MQK_GUI_OPERATOR_TOKEN',
        'MQK_OPERATOR_TOKEN',
        'MQK_REPO_ROOT'
    )

    $snapshot = New-EnvSnapshot -Names $names

    $env:MQK_DAEMON_DEPLOYMENT_MODE = 'paper'
    $env:MQK_DAEMON_ADAPTER_ID = 'alpaca'
    $env:MQK_DAEMON_ADDR = '127.0.0.1:8899'
    $env:MQK_GUI_DAEMON_URL = 'http://127.0.0.1:8899'
    $env:MQK_GUI_OPERATOR_TOKEN = $OperatorToken
    $env:MQK_OPERATOR_TOKEN = $OperatorToken
    $env:MQK_REPO_ROOT = $RepoRoot

    return $snapshot
}

function Invoke-JsonRequest {
    param(
        [Parameter(Mandatory = $true)][ValidateSet('GET', 'POST')][string]$Method,
        [Parameter(Mandatory = $true)][string]$Url,
        [hashtable]$Headers,
        [object]$Body
    )

    $params = @{
        Uri             = $Url
        Method          = $Method
        TimeoutSec      = 2
        ErrorAction     = 'Stop'
        UseBasicParsing = $true   # required for Windows PowerShell 5.1 (avoids IE COM dependency)
    }

    if ($null -ne $Headers) {
        $params['Headers'] = $Headers
    }

    if ($PSBoundParameters.ContainsKey('Body')) {
        $params['ContentType'] = 'application/json'
        $params['Body'] = ($Body | ConvertTo-Json -Depth 8 -Compress)
    }

    $response = Invoke-WebRequest @params
    $json = $null
    if ($response.Content) {
        try {
            $json = $response.Content | ConvertFrom-Json
        }
        catch {
            $json = $null
        }
    }

    return [pscustomobject]@{
        StatusCode = [int]$response.StatusCode
        Json = $json
        RawContent = $response.Content
    }
}

function Get-HttpFailureDetails {
    param([Parameter(Mandatory = $true)]$ErrorRecord)

    $statusCode = $null
    $json = $null
    $raw = $null

    $response = $null
    if ($null -ne $ErrorRecord.Exception) {
        if ($null -ne $ErrorRecord.Exception.Response) {
            $response = $ErrorRecord.Exception.Response
        }
        elseif ($null -ne $ErrorRecord.Exception.PSObject.Properties['Response']) {
            $response = $ErrorRecord.Exception.Response
        }
    }

    if ($null -ne $response) {
        try {
            if ($response.PSObject.Properties['StatusCode']) {
                $statusCode = [int]$response.StatusCode
            }
        }
        catch {
            $statusCode = $null
        }

        try {
            if ($response.PSObject.Properties['Content']) {
                $raw = [string]$response.Content
            }
        }
        catch {
            $raw = $null
        }

        if (-not $raw) {
            try {
                $stream = $response.GetResponseStream()
                if ($null -ne $stream) {
                    $reader = New-Object System.IO.StreamReader($stream)
                    $raw = $reader.ReadToEnd()
                    $reader.Dispose()
                    $stream.Dispose()
                }
            }
            catch {
                $raw = $null
            }
        }
    }

    if ($raw) {
        try {
            $json = $raw | ConvertFrom-Json
        }
        catch {
            $json = $null
        }
    }

    return [pscustomobject]@{
        StatusCode = $statusCode
        Json = $json
        RawContent = $raw
        Message = $ErrorRecord.Exception.Message
    }
}

function Test-LocalPortOccupied {
    param(
        [Parameter(Mandatory = $true)][string]$HostName,
        [Parameter(Mandatory = $true)][int]$Port
    )

    $client = New-Object System.Net.Sockets.TcpClient
    try {
        $iar = $client.BeginConnect($HostName, $Port, $null, $null)
        $connected = $iar.AsyncWaitHandle.WaitOne(300)
        if (-not $connected) {
            return $false
        }

        $client.EndConnect($iar)
        return $true
    }
    catch {
        return $false
    }
    finally {
        $client.Close()
    }
}

function Add-UniqueReason {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [System.Collections.ArrayList]$Reasons,

        [string]$Reason
    )

    if ($null -eq $Reason) { return }
    $trimmed = $Reason.Trim()
    if (-not $trimmed) { return }
    if (-not $Reasons.Contains($trimmed)) {
        [void]$Reasons.Add($trimmed)
    }
}

function Join-Reasons {
    param(
        $Reasons = $null
    )

    $normalized = @()
    if ($null -ne $Reasons) {
        $normalized = @($Reasons)
    }

    if ($normalized.Count -eq 0) {
        return 'none'
    }

    $filtered = @(
        $normalized | Where-Object {
            $null -ne $_ -and ([string]$_).Trim().Length -gt 0
        }
    )

    if ($filtered.Count -eq 0) {
        return 'none'
    }

    return ($filtered -join '; ')
}

function Get-TradeReadinessReasons {
    param([Parameter(Mandatory = $true)]$Probe)

    $reasons = New-Object System.Collections.ArrayList

    if ($Probe.Status.deployment_start_allowed -ne $true -or $Probe.Session.deployment_start_allowed -ne $true -or $Probe.Preflight.deployment_start_allowed -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason (
            "deployment_start_allowed is not consistently true (status=$($Probe.Status.deployment_start_allowed); session=$($Probe.Session.deployment_start_allowed); preflight=$($Probe.Preflight.deployment_start_allowed))"
        )
    }

    if ($Probe.Preflight.broker_config_present -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason 'preflight reports broker_config_present=false'
    }

    if ($Probe.Preflight.autonomous_readiness_applicable -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason 'preflight reports autonomous_readiness_applicable=false'
    }

    if ($Probe.Preflight.runtime_idle -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason ("preflight reports runtime_idle=$($Probe.Preflight.runtime_idle)")
    }

    if ($Probe.Status.runtime_status -eq 'running') {
        Add-UniqueReason -Reasons $reasons -Reason 'status reports runtime_status=running'
    }

    if ($Probe.Status.live_routing_enabled -eq $true) {
        Add-UniqueReason -Reasons $reasons -Reason 'status reports live_routing_enabled=true'
    }

    if ($null -eq $Probe.AutonomousReadiness) {
        Add-UniqueReason -Reasons $reasons -Reason 'autonomous readiness payload is missing'
        return $reasons.ToArray()
    }

    if ($Probe.AutonomousReadiness.truth_state -ne 'active') {
        Add-UniqueReason -Reasons $reasons -Reason ("autonomous readiness truth_state=$($Probe.AutonomousReadiness.truth_state)")
    }

    if ($Probe.AutonomousReadiness.canonical_path -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason 'autonomous readiness reports canonical_path=false'
    }

    if ($Probe.AutonomousReadiness.signal_ingestion_configured -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason 'autonomous readiness reports signal_ingestion_configured=false'
    }

    if ($Probe.AutonomousReadiness.ws_continuity_ready -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason (
            "autonomous readiness reports ws_continuity_ready=$($Probe.AutonomousReadiness.ws_continuity_ready) (ws_continuity=$($Probe.AutonomousReadiness.ws_continuity))"
        )
    }

    if ($Probe.AutonomousReadiness.reconcile_ready -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason (
            "autonomous readiness reports reconcile_ready=$($Probe.AutonomousReadiness.reconcile_ready) (reconcile_status=$($Probe.AutonomousReadiness.reconcile_status))"
        )
    }

    if ($Probe.AutonomousReadiness.arm_ready -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason (
            "autonomous readiness reports arm_ready=$($Probe.AutonomousReadiness.arm_ready) (arm_state=$($Probe.AutonomousReadiness.arm_state))"
        )
    }

    if ($Probe.AutonomousReadiness.session_in_window -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason (
            "autonomous readiness reports session_in_window=$($Probe.AutonomousReadiness.session_in_window) (session_window_state=$($Probe.AutonomousReadiness.session_window_state))"
        )
    }

    if ($Probe.AutonomousReadiness.runtime_start_allowed -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason (
            "autonomous readiness reports runtime_start_allowed=$($Probe.AutonomousReadiness.runtime_start_allowed)"
        )
    }

    if ($Probe.AutonomousReadiness.overall_ready -ne $true) {
        Add-UniqueReason -Reasons $reasons -Reason (
            "autonomous readiness reports overall_ready=$($Probe.AutonomousReadiness.overall_ready)"
        )
    }

    # PS5.1 ConvertFrom-Json converts empty JSON arrays [] to $null and may
    # not create the property on the PSCustomObject at all.  Use PSObject.Properties
    # to guard against PropertyNotFoundException under Set-StrictMode -Version Latest.
    $preflightBlockers = if ($Probe.Preflight -ne $null -and $null -ne $Probe.Preflight.PSObject.Properties['blockers']) { @($Probe.Preflight.blockers) } else { @() }
    foreach ($blocker in $preflightBlockers) {
        Add-UniqueReason -Reasons $reasons -Reason ([string]$blocker)
    }

    $arBlockers = if ($Probe.AutonomousReadiness -ne $null -and $null -ne $Probe.AutonomousReadiness.PSObject.Properties['blockers']) { @($Probe.AutonomousReadiness.blockers) } else { @() }
    foreach ($blocker in $arBlockers) {
        Add-UniqueReason -Reasons $reasons -Reason ([string]$blocker)
    }

    return $reasons.ToArray()
}

function Get-BackendProbe {
    param(
        [Parameter(Mandatory = $true)][string]$BaseUrl,
        [Parameter(Mandatory = $true)][string]$OperatorToken
    )

    $result = [ordered]@{
        Reachable = $false
        PortOccupied = $false
        IdentityVerified = $false
        TradeReady = $false
        FailureReason = $null
        TradeReadinessReasons = @()
        Health = $null
        Metadata = $null
        Status = $null
        Session = $null
        Preflight = $null
        AutonomousReadiness = $null
        AuthProbeStatus = $null
        AuthProbeDisposition = $null
        StartedByLauncher = $false
    }

    $parsedBase = [Uri]$BaseUrl
    $port = if ($parsedBase.IsDefaultPort) { if ($parsedBase.Scheme -eq 'https') { 443 } else { 80 } } else { $parsedBase.Port }
    $result.PortOccupied = Test-LocalPortOccupied -HostName $parsedBase.Host -Port $port

    try {
        $health = Invoke-JsonRequest -Method 'GET' -Url ($BaseUrl.TrimEnd('/') + '/v1/health')
        $result.Health = $health.Json
        $result.Reachable = $true
    }
    catch {
        $details = Get-HttpFailureDetails -ErrorRecord $_
        if ($details.StatusCode -ne $null) {
            $result.Reachable = $true
        }
    }

    $paths = @(
        @{ Name = 'Metadata'; Path = '/api/v1/system/metadata' },
        @{ Name = 'Status'; Path = '/api/v1/system/status' },
        @{ Name = 'Session'; Path = '/api/v1/system/session' },
        @{ Name = 'Preflight'; Path = '/api/v1/system/preflight' },
        @{ Name = 'AutonomousReadiness'; Path = '/api/v1/autonomous/readiness' }
    )

    foreach ($entry in $paths) {
        try {
            $response = Invoke-JsonRequest -Method 'GET' -Url ($BaseUrl.TrimEnd('/') + $entry.Path)
            $result[$entry.Name] = $response.Json
            $result.Reachable = $true
        }
        catch {
            $details = Get-HttpFailureDetails -ErrorRecord $_
            if ($details.StatusCode -ne $null) {
                $result.Reachable = $true
            }
            $result.FailureReason = if ($details.StatusCode -ne $null) {
                "backend refused $($entry.Path) with HTTP $($details.StatusCode)"
            }
            else {
                "backend probe failed for $($entry.Path): $($details.Message)"
            }
            return [pscustomobject]$result
        }
    }

    if ($null -eq $result.Health) {
        $result.FailureReason = 'reachable backend did not return /v1/health JSON'
        return [pscustomobject]$result
    }

    if ($result.Health.service -ne 'mqk-daemon') {
        $result.FailureReason = "service mismatch on /v1/health (expected mqk-daemon, got '$($result.Health.service)')"
        return [pscustomobject]$result
    }

    if ($result.Metadata.daemon_mode -ne 'paper' -or $result.Status.daemon_mode -ne 'paper' -or $result.Session.daemon_mode -ne 'paper' -or $result.Preflight.daemon_mode -ne 'paper') {
        $result.FailureReason = "daemon mode mismatch (metadata=$($result.Metadata.daemon_mode), status=$($result.Status.daemon_mode), session=$($result.Session.daemon_mode), preflight=$($result.Preflight.daemon_mode))"
        return [pscustomobject]$result
    }

    if ($result.Metadata.adapter_id -ne 'alpaca' -or $result.Status.adapter_id -ne 'alpaca' -or $result.Session.adapter_id -ne 'alpaca' -or $result.Preflight.adapter_id -ne 'alpaca') {
        $result.FailureReason = "adapter mismatch (metadata=$($result.Metadata.adapter_id), status=$($result.Status.adapter_id), session=$($result.Session.adapter_id), preflight=$($result.Preflight.adapter_id))"
        return [pscustomobject]$result
    }

    if ($result.Session.operator_auth_mode -ne 'token_required') {
        $result.FailureReason = "operator auth mismatch (expected token_required, got '$($result.Session.operator_auth_mode)')"
        return [pscustomobject]$result
    }

    if ($result.Status.live_routing_enabled -eq $true) {
        $result.FailureReason = 'live routing is enabled; canonical desktop launcher refuses to attach'
        return [pscustomobject]$result
    }

    if ($result.Status.deployment_start_allowed -ne $true -or $result.Session.deployment_start_allowed -ne $true -or $result.Preflight.deployment_start_allowed -ne $true) {
        $result.FailureReason = "deployment_start_allowed is not consistently true for the configured paper+alpaca backend (status=$($result.Status.deployment_start_allowed), session=$($result.Session.deployment_start_allowed), preflight=$($result.Preflight.deployment_start_allowed))"
        return [pscustomobject]$result
    }

    if ($result.Preflight.broker_config_present -ne $true) {
        $result.FailureReason = 'preflight reports broker_config_present=false; canonical alpaca broker wiring is absent'
        return [pscustomobject]$result
    }

    if ($result.Preflight.autonomous_readiness_applicable -ne $true) {
        $result.FailureReason = 'preflight reports autonomous_readiness_applicable=false; this is not the canonical paper+alpaca path'
        return [pscustomobject]$result
    }

    if ($null -eq $result.AutonomousReadiness) {
        $result.FailureReason = 'autonomous readiness payload is missing'
        return [pscustomobject]$result
    }

    if ($result.AutonomousReadiness.truth_state -ne 'active') {
        $result.FailureReason = "autonomous readiness is not authoritative (truth_state=$($result.AutonomousReadiness.truth_state))"
        return [pscustomobject]$result
    }

    if ($result.AutonomousReadiness.canonical_path -ne $true) {
        $result.FailureReason = 'autonomous readiness says canonical_path=false'
        return [pscustomobject]$result
    }

    if ($result.AutonomousReadiness.signal_ingestion_configured -ne $true) {
        $result.FailureReason = 'autonomous readiness says signal_ingestion_configured=false'
        return [pscustomobject]$result
    }

    # Bearer auth round-trip required in all modes — both Observe and TradeReady.
    # This probe calls the canonical dispatcher with an impossible action_key;
    # 400 + unknown_action + accepted=false proves Bearer auth passed without
    # mutating any runtime state.  A wrong or rotated token returns 401 and
    # blocks the attach.
    try {
        $authProbe = Invoke-JsonRequest `
            -Method 'POST' `
            -Url ($BaseUrl.TrimEnd('/') + '/api/v1/ops/action') `
            -Headers @{ Authorization = "Bearer $OperatorToken" } `
            -Body @{ action_key = '__veritas_launcher_auth_probe__' }

        $result.AuthProbeStatus = $authProbe.StatusCode
        $result.AuthProbeDisposition = $authProbe.Json.disposition
        if ($authProbe.StatusCode -ne 400 -or $authProbe.Json.disposition -ne 'unknown_action' -or $authProbe.Json.accepted -ne $false) {
            $result.FailureReason = "unexpected auth probe response (status=$($authProbe.StatusCode), disposition=$($authProbe.Json.disposition), accepted=$($authProbe.Json.accepted))"
            return [pscustomobject]$result
        }
    }
    catch {
        $details = Get-HttpFailureDetails -ErrorRecord $_
        $result.AuthProbeStatus = $details.StatusCode
        if ($details.Json -and $details.Json.disposition) {
            $result.AuthProbeDisposition = $details.Json.disposition
        }
        # PowerShell 5.1: Invoke-WebRequest throws on ALL non-2xx responses,
        # including 400.  400 + unknown_action + accepted=false is the expected
        # contract response proving Bearer auth worked without mutating state.
        # Treat it as success here and fall through to IdentityVerified.
        # PS5.1 strict-mode guard: $details.Json may be $null when the
        # WebException response stream was already consumed.  A 400 with
        # no readable body is still the expected contract response — any
        # 400 from this route proves Bearer auth passed (non-2xx means
        # auth was checked; 401/503 are handled below).
        if ($details.StatusCode -eq 400 -and ($null -eq $details.Json -or ($details.Json.disposition -eq 'unknown_action' -and $details.Json.accepted -eq $false))) {
            # Expected auth probe response — do not set FailureReason.
        }
        elseif ($details.StatusCode -eq 401) {
            $result.FailureReason = 'operator token was rejected by the daemon'
            return [pscustomobject]$result
        }
        elseif ($details.StatusCode -eq 503) {
            $result.FailureReason = 'daemon operator routes are fail-closed because operator auth is not fully configured'
            return [pscustomobject]$result
        }
        elseif ($details.StatusCode -ne $null) {
            $result.FailureReason = "auth probe failed with HTTP $($details.StatusCode)"
            return [pscustomobject]$result
        }
        else {
            $result.FailureReason = "auth probe failed: $($details.Message)"
            return [pscustomobject]$result
        }
    }

    $result.IdentityVerified = $true
    $result.TradeReadinessReasons = @(Get-TradeReadinessReasons -Probe ([pscustomobject]$result))
    $result.TradeReady = ($result.TradeReadinessReasons.Count -eq 0)
    $result.FailureReason = $null
    return [pscustomobject]$result
}

function Wait-ForBackendState {
    param(
        [Parameter(Mandatory = $true)][string]$BaseUrl,
        [Parameter(Mandatory = $true)][string]$OperatorToken,
        [Parameter(Mandatory = $true)][int]$TimeoutSeconds,
        [Parameter(Mandatory = $true)][bool]$RequireTradeReady
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $lastProbe = $null
    while ((Get-Date) -lt $deadline) {
        $lastProbe = Get-BackendProbe -BaseUrl $BaseUrl -OperatorToken $OperatorToken
        if ($lastProbe.IdentityVerified -and ((-not $RequireTradeReady) -or $lastProbe.TradeReady)) {
            return $lastProbe
        }
        Start-Sleep -Milliseconds 500
    }

    if ($null -ne $lastProbe) {
        return $lastProbe
    }

    throw "Timed out waiting for backend state at $BaseUrl"
}

function Invoke-MarketDataGate {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][bool]$CheckOnly,
        [string]$Symbols,
        [string]$Timeframe,
        [int]$MinCompletedBars,
        [int]$MaxStalenessDays
    )

    # Resolve symbol from param -> env -> default
    $resolvedSymbols = if (-not [string]::IsNullOrWhiteSpace($Symbols)) {
        $Symbols
    } elseif (-not [string]::IsNullOrWhiteSpace($env:MQK_STRATEGY_SYMBOL)) {
        $env:MQK_STRATEGY_SYMBOL
    } else {
        'AAPL'
    }

    # Resolve timeframe from param -> env -> default
    $resolvedTimeframe = if (-not [string]::IsNullOrWhiteSpace($Timeframe)) {
        $Timeframe
    } elseif (-not [string]::IsNullOrWhiteSpace($env:MQK_STRATEGY_MD_TIMEFRAME)) {
        $env:MQK_STRATEGY_MD_TIMEFRAME
    } else {
        '1D'
    }

    # Resolve staleness threshold: explicit param wins; otherwise derive from timeframe
    $resolvedStaleness = if ($MaxStalenessDays -ge 0) {
        $MaxStalenessDays
    } elseif ($resolvedTimeframe -eq '5m' -or $resolvedTimeframe -eq '5min') {
        1
    } else {
        4
    }

    $modeLabel = if ($CheckOnly) { 'check-only' } else { 'prep' }
    Write-LauncherStep "Market-data gate ($modeLabel): symbols=$resolvedSymbols timeframe=$resolvedTimeframe minBars=$MinCompletedBars maxStale=${resolvedStaleness}d"

    $prepScript = Join-Path $RepoRoot 'scripts\windows\Prep-PremarketMarketData.ps1'
    if (-not (Test-Path $prepScript)) {
        throw "Market-data gate requested but Prep-PremarketMarketData.ps1 not found at: $prepScript"
    }

    $args = @(
        '-ExecutionPolicy', 'Bypass', '-NonInteractive', '-File', $prepScript,
        '-Symbols', $resolvedSymbols,
        '-Timeframe', $resolvedTimeframe,
        '-MinCompletedBars', $MinCompletedBars,
        '-MaxStalenessDays', $resolvedStaleness
    )
    if ($CheckOnly) {
        $args += '-CheckOnly'
    }

    $result = & powershell.exe @args
    $result | Out-Host
    if ($LASTEXITCODE -ne 0) {
        $verb = if ($CheckOnly) { 'Market-data check' } else { 'Market-data prep' }
        throw "$verb failed (exit $LASTEXITCODE). Resolve bar count / freshness issues before launching."
    }

    $verb2 = if ($CheckOnly) { 'Market-data check passed' } else { 'Market-data prep passed' }
    Write-LauncherSuccess "$verb2 (${resolvedSymbols}/${resolvedTimeframe})"
}

function Invoke-ArmPaper {
    param(
        [Parameter(Mandatory = $true)][string]$BaseUrl,
        [Parameter(Mandatory = $true)][string]$OperatorToken,
        [Parameter(Mandatory = $true)]$Probe
    )

    # Fail-closed pre-checks before touching any mutating route.
    if ($Probe.Status.live_routing_enabled -eq $true) {
        throw '-ArmPaper refused: live_routing_enabled=true. Paper arm requires live routing disabled.'
    }

    if ($Probe.Status.daemon_mode -ne 'paper') {
        throw ("-ArmPaper refused: daemon_mode='{0}'; paper arm requires mode=paper." -f $Probe.Status.daemon_mode)
    }

    if ($Probe.Status.adapter_id -ne 'alpaca') {
        throw ("-ArmPaper refused: adapter_id='{0}'; paper arm requires adapter=alpaca." -f $Probe.Status.adapter_id)
    }

    if (-not $Probe.TradeReady) {
        $armReasons = Join-Reasons -Reasons $Probe.TradeReadinessReasons
        throw "-ArmPaper refused: backend is not trade-ready. $armReasons"
    }

    Write-LauncherStep 'Arming paper execution (POST /api/v1/ops/action arm-execution)'

    $armStatusCode = $null
    $armRespData   = $null

    try {
        $armRaw = Invoke-JsonRequest `
            -Method 'POST' `
            -Url ($BaseUrl.TrimEnd('/') + '/api/v1/ops/action') `
            -Headers @{ Authorization = "Bearer $OperatorToken" } `
            -Body @{ action_key = 'arm-execution' }
        $armStatusCode = $armRaw.StatusCode
        $armRespData   = $armRaw.Json
    }
    catch {
        $armDetails    = Get-HttpFailureDetails -ErrorRecord $_
        $armStatusCode = $armDetails.StatusCode
        $armRespData   = $armDetails.Json
        if ($null -eq $armStatusCode) {
            throw "-ArmPaper: arm-execution request failed (no HTTP response): $($armDetails.Message)"
        }
    }

    if ($armStatusCode -ne 200 -or $null -eq $armRespData -or $armRespData.accepted -ne $true) {
        $disp = if ($null -ne $armRespData -and $null -ne $armRespData.PSObject.Properties['disposition']) { $armRespData.disposition } else { 'unknown' }
        throw "-ArmPaper: arm-execution was not accepted (status=$armStatusCode disposition=$disp). Arm refused; runtime not started."
    }

    # Post-arm read-only verification — autonomous readiness reflects new arm state.
    try {
        $arCheck   = Invoke-JsonRequest -Method 'GET' -Url ($BaseUrl.TrimEnd('/') + '/api/v1/autonomous/readiness')
        $arArmState = if ($null -ne $arCheck.Json -and $null -ne $arCheck.Json.PSObject.Properties['arm_state']) { $arCheck.Json.arm_state } else { 'unknown' }
        if ($arArmState -eq 'armed') {
            Write-LauncherSuccess "Arm verified: arm_state=armed. ARMED ONLY -- runtime not started, no orders submitted."
        }
        else {
            Write-LauncherWarn "Arm action accepted but arm_state='$arArmState' (expected 'armed'). Verify via GUI before proceeding."
        }
    }
    catch {
        Write-LauncherWarn 'Could not verify arm state via autonomous readiness; check GUI or GET /api/v1/autonomous/readiness.'
    }
}

function Invoke-StartupEvidenceCapture {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)

    $captureScript = Join-Path $RepoRoot 'scripts\windows\Capture-PaperSmokeEvidence.ps1'
    if (-not (Test-Path $captureScript)) {
        Write-LauncherWarn "Startup evidence capture requested but script not found: $captureScript"
        return
    }

    Write-LauncherStep 'Capturing startup evidence (label=launcher_startup)'
    $captureArgs = @(
        '-ExecutionPolicy', 'Bypass', '-NonInteractive', '-File', $captureScript,
        '-Label', 'launcher_startup'
    )
    & powershell.exe @captureArgs | Out-Host
    if ($LASTEXITCODE -ne 0) {
        Write-LauncherWarn "Startup evidence capture exited with code $LASTEXITCODE; launcher continues."
    }
}

function Get-ModeDisplayName {
    param([Parameter(Mandatory = $true)][string]$LauncherMode)

    switch ($LauncherMode) {
        'TradeReady' { return 'trade-ready' }
        default { return 'observe/attach' }
    }
}

function Write-BackendSummary {
    param(
        [Parameter(Mandatory = $true)]$Probe,
        [Parameter(Mandatory = $true)][string]$LauncherMode
    )

    $modeLabel = Get-ModeDisplayName -LauncherMode $LauncherMode
    $runtimeStatus = $Probe.Status.runtime_status
    $dbStatus = $Probe.Status.db_status
    $reconcileStatus = $Probe.AutonomousReadiness.reconcile_status
    $wsContinuity = $Probe.AutonomousReadiness.ws_continuity
    $armState = $Probe.AutonomousReadiness.arm_state
    $sessionWindow = $Probe.AutonomousReadiness.session_window_state

    Write-LauncherSuccess "Verified canonical backend for $modeLabel mode: service=$($Probe.Health.service) mode=$($Probe.Status.daemon_mode) adapter=$($Probe.Status.adapter_id) auth=$($Probe.Session.operator_auth_mode) runtime=$runtimeStatus db=$dbStatus ws=$wsContinuity reconcile=$reconcileStatus arm=$armState session=$sessionWindow"

    if ($Probe.TradeReady) {
        Write-LauncherSuccess 'Backend is trade-ready under mounted daemon truth.'
        return
    }

    $tradeReasonText = if ($null -eq $Probe.TradeReadinessReasons -or @($Probe.TradeReadinessReasons).Count -eq 0) {
    'none'
}
else {
    @(
        @($Probe.TradeReadinessReasons) | Where-Object {
            $null -ne $_ -and ([string]$_).Trim().Length -gt 0
        }
    ) -join '; '
}

Write-LauncherWarn ('Backend is NOT trade-ready: ' + $tradeReasonText)
}

function Start-DaemonIfNeeded {
    param(
        [Parameter(Mandatory = $true)][string]$DaemonExe,
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$BaseUrl,
        [Parameter(Mandatory = $true)][string]$OperatorToken,
        [Parameter(Mandatory = $true)][string]$LauncherMode
    )

    $requireTradeReady = $LauncherMode -eq 'TradeReady'
    $existingProbe = Get-BackendProbe -BaseUrl $BaseUrl -OperatorToken $OperatorToken
    if ($existingProbe.IdentityVerified) {
        if ($requireTradeReady -and -not $existingProbe.TradeReady) {
            $existingReasonText = if ($null -eq $existingProbe.TradeReadinessReasons -or @($existingProbe.TradeReadinessReasons).Count -eq 0) {
    'none'
}
else {
    @(
        @($existingProbe.TradeReadinessReasons) | Where-Object {
            $null -ne $_ -and ([string]$_).Trim().Length -gt 0
        }
    ) -join '; '
}

throw "Verified canonical backend is not trade-ready. $existingReasonText"
        }

        $reuseLabel = if ($existingProbe.TradeReady) { 'trade-ready' } else { 'observe/attach' }
        Write-LauncherSuccess "Reusing verified local mqk-daemon for $reuseLabel mode"
        return [pscustomobject]@{
            Started = $false
            Probe = $existingProbe
            ProcessId = $null
            StdoutLog = $null
            StderrLog = $null
        }
    }

    if ($existingProbe.PortOccupied -or $existingProbe.Reachable) {
        $reason = $existingProbe.FailureReason
        if (-not $reason) {
            $reason = '127.0.0.1:8899 is already occupied by a non-verified backend'
        }
        throw "Refusing to open Veritas Ledger against an unverified or non-canonical local backend on 127.0.0.1:8899. $reason"
    }

    $logDir = Join-Path $RepoRoot 'exports\launcher'
    New-Item -ItemType Directory -Force -Path $logDir | Out-Null

    $stamp = Get-Date -Format 'yyyyMMdd_HHmmss'
    $stdoutLog = Join-Path $logDir "daemon_$stamp.stdout.log"
    $stderrLog = Join-Path $logDir "daemon_$stamp.stderr.log"

    Write-LauncherStep 'Starting mqk-daemon in canonical local paper+alpaca posture'
    $process = Start-Process `
        -FilePath $DaemonExe `
        -WorkingDirectory $RepoRoot `
        -RedirectStandardOutput $stdoutLog `
        -RedirectStandardError $stderrLog `
        -WindowStyle Hidden `
        -PassThru

    $probe = $null
    try {
        $probe = Wait-ForBackendState -BaseUrl $BaseUrl -OperatorToken $OperatorToken -TimeoutSeconds 30 -RequireTradeReady:$requireTradeReady
        if (-not $probe.IdentityVerified) {
            throw "daemon did not reach verified canonical identity. $($probe.FailureReason)"
        }
    }
    catch {
        if (-not $process.HasExited) {
            $process | Stop-Process -Force -ErrorAction SilentlyContinue
        }

        $innerMsg = $null
        if ($_.Exception -and $_.Exception.Message) {
            $innerMsg = $_.Exception.Message
        }
        elseif ($_ ) {
            $innerMsg = [string]$_
        }
        else {
            $innerMsg = 'unknown launcher failure'
        }

        throw "mqk-daemon failed to reach required $((Get-ModeDisplayName -LauncherMode $LauncherMode)) launcher state: $innerMsg stdout=$stdoutLog stderr=$stderrLog"
    }

    if ($requireTradeReady -and -not $probe.TradeReady) {
        if (-not $process.HasExited) {
            $process | Stop-Process -Force -ErrorAction SilentlyContinue
        }

        $probeReasonText = if ($null -eq $probe.TradeReadinessReasons -or @($probe.TradeReadinessReasons).Count -eq 0) {
            'none'
        }
        else {
            @(
                @($probe.TradeReadinessReasons) | Where-Object {
                    $null -ne $_ -and ([string]$_).Trim().Length -gt 0
                }
            ) -join '; '
        }

        throw "Verified canonical backend started but is not trade-ready. $probeReasonText stdout=$stdoutLog stderr=$stderrLog"
    }

    return [pscustomobject]@{
        Started = $true
        Probe = $probe
        ProcessId = $process.Id
        StdoutLog = $stdoutLog
        StderrLog = $stderrLog
    }
}

# =============================================================================
# PAPER-OPERATOR-STARTUP-LAUNCHER-01: -CheckOnly read-only status report.
#
# Helpers below are read-only: no docker start/exec mutations beyond
# read-only `pg_isready`/`psql SELECT`, no daemon mutating routes, no
# broker calls, no Discord, no .env.local contents printed.
# =============================================================================

function Write-CheckField {
    param([string]$Label, $Value)
    Write-Host ("  {0,-34} {1}" -f "${Label}:", $Value)
}

function Get-JsonField {
    param($Obj, [string]$Field, $Default = 'unknown')
    if ($null -eq $Obj) { return $Default }
    $prop = $Obj.PSObject.Properties[$Field]
    if ($null -eq $prop -or $null -eq $prop.Value) { return $Default }
    return $prop.Value
}

function Format-CheckOnlyBoolField {
    param($Value)
    if ($Value -is [bool]) {
        if ($Value) { return 'true' } else { return 'false' }
    }
    return $Value
}

function Get-PaperDbContainerStatus {
    param([Parameter(Mandatory = $true)][string]$ContainerName)
    try {
        $null = Get-Command 'docker' -ErrorAction Stop
    } catch {
        return 'docker_unavailable'
    }
    try {
        $inspect = docker inspect $ContainerName 2>&1
        if ($LASTEXITCODE -ne 0) { return 'missing' }
        $inspectJson = $inspect | ConvertFrom-Json
        $status = $inspectJson[0].State.Status
        if ($status -eq 'running') { return 'running' }
        return "stopped:$status"
    } catch {
        return 'unreachable'
    }
}

function Get-CheckOnlyMdBarSummary {
    param(
        [Parameter(Mandatory = $true)][string]$ContainerName,
        [Parameter(Mandatory = $true)][string]$Symbol,
        [Parameter(Mandatory = $true)][string]$Timeframe
    )
    $result = [pscustomobject]@{ completed_count = $null; latest_bar = $null; query_ok = $false }
    try {
        $countQ = "SELECT count(*) FROM md_bars WHERE symbol='$Symbol' AND timeframe='$Timeframe' AND is_complete=true"
        $rawC = docker exec $ContainerName psql -U postgres -d miniquantdesk_paper -t -A -q -c $countQ 2>$null
        $numC = ($rawC -join '') -replace '\s', ''
        if ($numC -notmatch '^\d+$') { return $result }
        $result.completed_count = [int]$numC

        $maxQ = "SELECT to_char(to_timestamp(max(end_ts)) AT TIME ZONE 'UTC', 'YYYY-MM-DD HH24:MI:SS') FROM md_bars WHERE symbol='$Symbol' AND timeframe='$Timeframe' AND is_complete=true"
        $rawM = docker exec $ContainerName psql -U postgres -d miniquantdesk_paper -t -A -q -c $maxQ 2>$null
        $lineM = ($rawM -join '').Trim()
        if (-not [string]::IsNullOrWhiteSpace($lineM)) { $result.latest_bar = (($lineM -replace ' ', 'T') + 'Z') }

        $result.query_ok = $true
    } catch {}
    return $result
}

function Get-CheckOnlyArmStateSummary {
    param([Parameter(Mandatory = $true)][string]$ContainerName)
    $result = [pscustomobject]@{ state = $null; reason = $null; query_ok = $false }
    try {
        $armQ = "SELECT state, coalesce(reason, '') FROM sys_arm_state WHERE sentinel_id = 1"
        $rawA = docker exec $ContainerName psql -U postgres -d miniquantdesk_paper -t -A -q -F'|' -c $armQ 2>$null
        $lineA = ($rawA -join '').Trim()
        if ($lineA -match '\|') {
            $parts = $lineA -split '\|'
            if ($parts.Count -ge 2) {
                $result.state  = $parts[0].Trim()
                $result.reason = $parts[1].Trim()
                if ([string]::IsNullOrWhiteSpace($result.reason)) { $result.reason = $null }
            }
        }
        $result.query_ok = $true
    } catch {}
    return $result
}

function Invoke-CheckOnlyDaemonGet {
    param([Parameter(Mandatory = $true)][string]$BaseUrl, [Parameter(Mandatory = $true)][string]$Path)
    try {
        $resp = Invoke-JsonRequest -Method 'GET' -Url ($BaseUrl.TrimEnd('/') + $Path)
        return $resp.Json
    } catch {
        return $null
    }
}

function Invoke-StartupCheckOnly {
    param([Parameter(Mandatory = $true)][string]$RepoRoot)

    $PaperDbContainerName = 'mqk-paper-postgres'
    $DaemonBaseUrl = 'http://127.0.0.1:8899'

    Write-Host ''
    Write-Host '=== Veritas Ledger Startup -- CheckOnly ===' -ForegroundColor Cyan
    Write-Host '    Read-only status report. No daemon/GUI start, no build, no arm,' -ForegroundColor DarkGray
    Write-Host '    no broker calls, no DB writes, no Discord alerts.' -ForegroundColor DarkGray
    Write-Host ''

    $hardFailure = $false

    # 1. Repo root
    Write-CheckField 'Repo root detected' $RepoRoot

    # 2. .env.local present (presence only -- contents never read or printed)
    $envLocalPath = Join-Path $RepoRoot '.env.local'
    $envLocalPresent = Test-Path $envLocalPath
    if ($envLocalPresent) {
        Write-CheckField '.env.local present' 'yes (contents not read or printed)'
    } else {
        Write-CheckField '.env.local present' 'no'
        $hardFailure = $true
    }

    # 3. Docker available
    $dockerAvailable = $false
    try {
        $null = Get-Command 'docker' -ErrorAction Stop
        $dockerAvailable = $true
        Write-CheckField 'Docker available' 'yes'
    } catch {
        Write-CheckField 'Docker available' 'no'
        $hardFailure = $true
    }

    # 4. Paper DB container (read-only `docker inspect`)
    $dbStatus = Get-PaperDbContainerStatus -ContainerName $PaperDbContainerName
    $dbStatusDisplay = switch -Regex ($dbStatus) {
        '^running$'            { 'RUNNING' }
        '^missing$'            { 'MISSING (container not created)' }
        '^stopped:'            { "STOPPED ($($dbStatus.Substring(8)))" }
        '^docker_unavailable$' { 'UNKNOWN (docker unavailable)' }
        default                { 'UNREACHABLE' }
    }
    Write-CheckField "Paper DB container ($PaperDbContainerName)" $dbStatusDisplay

    # 5. Paper DB port (cross-checked against MQK_DATABASE_URL if loaded)
    $dbUrl = $env:MQK_DATABASE_URL
    if (-not [string]::IsNullOrWhiteSpace($dbUrl) -and $dbUrl -match ':5440') {
        Write-CheckField 'Paper DB port' 'expected localhost:5440 (confirmed in MQK_DATABASE_URL)'
    } elseif (-not [string]::IsNullOrWhiteSpace($dbUrl)) {
        Write-CheckField 'Paper DB port' 'expected localhost:5440 (MQK_DATABASE_URL set but does not match -- verify)'
    } else {
        Write-CheckField 'Paper DB port' 'expected localhost:5440 (MQK_DATABASE_URL not set)'
    }

    # 6. mqk-daemon binary (Test-Path only -- Resolve-DaemonBinary throws if missing)
    $daemonExePath = Join-Path $RepoRoot 'core-rs\target\release\mqk-daemon.exe'
    if (Test-Path $daemonExePath) {
        Write-CheckField 'mqk-daemon binary exists' "yes ($daemonExePath)"
    } else {
        Write-CheckField 'mqk-daemon binary exists' 'no (will build on launch)'
    }

    # 7. Daemon health endpoint (fail-soft GET, no auth required)
    $health = Invoke-CheckOnlyDaemonGet -BaseUrl $DaemonBaseUrl -Path '/v1/health'
    $daemonReachable = $null -ne $health
    if ($daemonReachable) {
        $serviceName = Get-JsonField -Obj $health -Field 'service' -Default '(unknown)'
        Write-CheckField 'Daemon health endpoint reachable' "yes (service=$serviceName)"
    } else {
        Write-CheckField 'Daemon health endpoint reachable' 'no (expected before startup)'
    }

    # 8. GUI package
    $guiPackagePath = Join-Path $RepoRoot 'core-rs\mqk-gui\package.json'
    if (Test-Path $guiPackagePath) {
        Write-CheckField 'GUI package exists' "yes ($guiPackagePath)"
    } else {
        Write-CheckField 'GUI package exists' 'no'
    }

    # 9. GUI node_modules
    $guiNodeModulesPath = Join-Path $RepoRoot 'core-rs\mqk-gui\node_modules'
    if (Test-Path $guiNodeModulesPath) {
        Write-CheckField 'GUI node_modules present' 'yes'
    } else {
        Write-CheckField 'GUI node_modules present' 'no (npm ci will run on launch)'
    }

    # 10. GUI desktop binary (Resolve-GuiBinary is non-throwing / non-mutating)
    $guiExePath = Resolve-GuiBinary -RepoRoot $RepoRoot
    if ($null -ne $guiExePath) {
        Write-CheckField 'GUI desktop binary built' "yes ($guiExePath)"
    } else {
        Write-CheckField 'GUI desktop binary built' 'no (will build on launch)'
    }

    # 11. AAPL/5m bars (symbol/timeframe resolved from env, default AAPL/5m)
    $barSymbol = if (-not [string]::IsNullOrWhiteSpace($env:MQK_STRATEGY_SYMBOL)) { $env:MQK_STRATEGY_SYMBOL } else { 'AAPL' }
    $barTimeframe = if (-not [string]::IsNullOrWhiteSpace($env:MQK_STRATEGY_MD_TIMEFRAME)) { $env:MQK_STRATEGY_MD_TIMEFRAME } else { '5m' }
    $barsLabel = "$barSymbol/$barTimeframe bars"
    if ($dbStatus -eq 'running') {
        $pgReady = $false
        try {
            $null = docker exec $PaperDbContainerName pg_isready -U postgres -d miniquantdesk_paper 2>$null
            $pgReady = ($LASTEXITCODE -eq 0)
        } catch {}
        if ($pgReady) {
            $barSummary = Get-CheckOnlyMdBarSummary -ContainerName $PaperDbContainerName -Symbol $barSymbol -Timeframe $barTimeframe
            if ($barSummary.query_ok) {
                $latestText = if ($null -ne $barSummary.latest_bar) { $barSummary.latest_bar } else { '(none)' }
                Write-CheckField $barsLabel "$($barSummary.completed_count) rows, latest $latestText"
            } else {
                Write-CheckField $barsLabel 'UNKNOWN (md_bars query failed)'
            }
        } else {
            Write-CheckField $barsLabel 'UNKNOWN (paper DB not ready)'
        }
    } else {
        Write-CheckField $barsLabel 'UNKNOWN (paper DB container not running)'
    }

    # 12. Persisted sys_arm_state (read-only SELECT against the durable singleton row)
    $armState = $null
    $armReason = $null
    if ($dbStatus -eq 'running') {
        $armSummary = Get-CheckOnlyArmStateSummary -ContainerName $PaperDbContainerName
        if ($armSummary.query_ok -and $null -ne $armSummary.state) {
            $armState = $armSummary.state
            $armReason = $armSummary.reason
            $armDisplay = if ($null -ne $armReason) { "$armState, reason=$armReason" } else { $armState }
            Write-CheckField 'Persisted sys_arm_state' $armDisplay
        } else {
            Write-CheckField 'Persisted sys_arm_state' 'UNKNOWN (sys_arm_state query failed or empty)'
        }
    } else {
        Write-CheckField 'Persisted sys_arm_state' 'UNKNOWN (paper DB container not running)'
    }

    # 13-16. Live daemon truth (fail-soft GETs, only if daemon reachable)
    $liveRoutingEnabled = $null
    $runtimeStatus = 'unknown'
    $killSwitchActive = $null
    $reconcileStatus = 'unknown'
    if ($daemonReachable) {
        $sysStatus = Invoke-CheckOnlyDaemonGet -BaseUrl $DaemonBaseUrl -Path '/api/v1/system/status'
        $liveRoutingEnabled = Get-JsonField -Obj $sysStatus -Field 'live_routing_enabled' -Default 'unknown'
        $runtimeStatus = Get-JsonField -Obj $sysStatus -Field 'runtime_status' -Default 'unknown'
        $killSwitchActive = Get-JsonField -Obj $sysStatus -Field 'kill_switch_active' -Default 'unknown'

        $reconcile = Invoke-CheckOnlyDaemonGet -BaseUrl $DaemonBaseUrl -Path '/api/v1/reconcile/status'
        $reconcileStatus = Get-JsonField -Obj $reconcile -Field 'status' -Default 'unknown'

        Write-CheckField 'Live routing' (Format-CheckOnlyBoolField $liveRoutingEnabled)
        Write-CheckField 'Runtime status' $runtimeStatus
        Write-CheckField 'Kill switch' (Format-CheckOnlyBoolField $killSwitchActive)
        Write-CheckField 'Reconcile' $reconcileStatus
    } else {
        Write-CheckField 'Live routing' 'unknown (daemon offline)'
        Write-CheckField 'Runtime status' 'unknown (daemon offline)'
        Write-CheckField 'Kill switch' 'unknown (daemon offline)'
        Write-CheckField 'Reconcile' 'unknown (daemon offline)'
    }

    if ($liveRoutingEnabled -eq $true) {
        Write-Host ''
        Write-Host '  DANGER: live_routing_enabled=true on a reachable daemon.' -ForegroundColor Red
        Write-Host '  Investigate before proceeding. Do not start, arm, or trade.' -ForegroundColor Red
    }

    # 17. Next safe operator action (priority order, most urgent first)
    Write-Host ''
    if (-not $envLocalPresent) {
        $nextAction = 'Copy .env.local.example to .env.local and fill in credentials, then re-run -CheckOnly.'
    } elseif (-not $dockerAvailable) {
        $nextAction = 'Start Docker Desktop, then re-run -CheckOnly.'
    } elseif ($liveRoutingEnabled -eq $true) {
        $nextAction = 'DANGER: live_routing_enabled=true. Investigate immediately. Do not start, arm, or trade.'
    } elseif ($daemonReachable -and $armState -eq 'HALTED') {
        $nextAction = "Persisted arm state is HALTED (reason=$armReason). Do not manually clear or arm. Run Get-PaperOperatorStatus.ps1 for full halt context first."
    } elseif ($daemonReachable -and $reconcileStatus -eq 'dirty') {
        $nextAction = 'Reconcile status is dirty. Run Get-PaperOperatorStatus.ps1 to review the mismatch before proceeding.'
    } elseif ($daemonReachable) {
        $nextAction = 'Daemon is already reachable. Run Get-PaperOperatorStatus.ps1 for full live status, or run Launch-VeritasLedger.ps1 (no -CheckOnly) to attach the GUI.'
    } elseif ($dbStatus -ne 'running') {
        $nextAction = "Paper DB container ($PaperDbContainerName) is not running. Start Docker Desktop / the container, then re-run -CheckOnly."
    } elseif ($armState -eq 'DISARMED') {
        $nextAction = "Persisted arm state is DISARMED (reason=$armReason). Do not manually clear or arm -- normal startup re-verifies fresh daemon state. If market is open, run Run-AAPL5mMarketSmoke.ps1 -CheckOnly before smoke."
    } else {
        $nextAction = 'Prerequisites look OK. If market is open, run Run-AAPL5mMarketSmoke.ps1 -CheckOnly before a smoke run, otherwise run Launch-VeritasLedger.ps1 (no -CheckOnly) for a normal startup.'
    }
    Write-CheckField 'Next action' $nextAction

    Write-Host ''
    if ($hardFailure) {
        Write-Host '=== CheckOnly: prerequisite check FAILED (see above) ===' -ForegroundColor Red
        return 1
    } else {
        Write-Host '=== CheckOnly complete ===' -ForegroundColor Cyan
        return 0
    }
}

# STALE-DAEMON-BINARY-PROVENANCE-01 / SCHEDULED-HEADLESS-BOOTSTRAP-01: guard
# MAIN DISPATCH so this file can be safely dot-sourced by tests (defines
# functions only -- no daemon start, no build, no exit) when the file is
# dot-sourced; normal `powershell.exe -File Launch-VeritasLedger.ps1`
# invocation is unaffected.
#
# DESKTOP-LAUNCH-01: Outer catch surfaces any startup failure in the console
# window before it closes, so the operator can read the error instead of
# seeing a flash-and-close with no diagnostic information.
if ($MyInvocation.InvocationName -ne '.') {
try {
    $repoRoot = Get-RepoRoot
    Import-LauncherEnvironmentFiles -RepoRoot $repoRoot

    if ($CheckOnly.IsPresent) {
        $checkOnlyExitCode = Invoke-StartupCheckOnly -RepoRoot $repoRoot
        exit $checkOnlyExitCode
    }

    $operatorToken = Resolve-RequiredOperatorToken
    $envSnapshot = Set-LauncherEnvironment -OperatorToken $operatorToken -RepoRoot $repoRoot

    try {
        # Resolve rebuild flags: -Rebuild is a legacy alias for -RebuildAll
        $rebuildAll = $Rebuild.IsPresent -or $RebuildAll.IsPresent
        $forceRebuildDaemon = $RebuildDaemon.IsPresent -or $rebuildAll
        $forceRebuildGui = $RebuildGui.IsPresent -or $rebuildAll

        Write-LauncherStep "Launcher mode: $(Get-ModeDisplayName -LauncherMode $Mode)"
        Write-LauncherStep 'Tip: run with -CheckOnly first for a read-only startup status summary (no daemon/GUI start).'

        # Market-data gate — runs before daemon binary resolution so failures are fast.
        # -CheckMarketData: read-only bar-count/freshness check; fails launcher if not met.
        # -PrepMarketData: ingest + sync then check; fails launcher if gates not met.
        # Both flags may not be combined.
        if ($CheckMarketData.IsPresent -and $PrepMarketData.IsPresent) {
            throw '-CheckMarketData and -PrepMarketData are mutually exclusive. Use one at a time.'
        }
        if ($CheckMarketData.IsPresent) {
            Invoke-MarketDataGate `
                -RepoRoot $repoRoot `
                -CheckOnly $true `
                -Symbols $Symbols `
                -Timeframe $Timeframe `
                -MinCompletedBars $MinCompletedBars `
                -MaxStalenessDays $MaxStalenessDays
        }
        if ($PrepMarketData.IsPresent) {
            Invoke-MarketDataGate `
                -RepoRoot $repoRoot `
                -CheckOnly $false `
                -Symbols $Symbols `
                -Timeframe $Timeframe `
                -MinCompletedBars $MinCompletedBars `
                -MaxStalenessDays $MaxStalenessDays
        }

        Write-LauncherStep 'Resolving daemon binary'
        $daemonExe = Ensure-DaemonBinary -RepoRoot $repoRoot -ForceRebuild:$forceRebuildDaemon

        $daemonInfo = Start-DaemonIfNeeded -DaemonExe $daemonExe -RepoRoot $repoRoot -BaseUrl $env:MQK_GUI_DAEMON_URL -OperatorToken $operatorToken -LauncherMode $Mode
        $verified = $daemonInfo.Probe

        Write-BackendSummary -Probe $verified -LauncherMode $Mode

        if ($ArmPaper.IsPresent) {
            Invoke-ArmPaper -BaseUrl $env:MQK_GUI_DAEMON_URL -OperatorToken $operatorToken -Probe $verified
        }

        $guiLaunched = $false
        if (-not $SkipGui.IsPresent) {
            Write-LauncherStep 'Resolving desktop GUI binary'
            $guiExe = Ensure-GuiBinary -RepoRoot $repoRoot -ForceRebuild:$forceRebuildGui

            Write-LauncherStep 'Launching desktop GUI against verified local daemon'
            Start-Process -FilePath $guiExe -WorkingDirectory (Split-Path -Parent $guiExe) | Out-Null
            $guiLaunched = $true
        }
        else {
            Write-LauncherStep 'GUI launch skipped (-SkipGui)'
        }

        if ($daemonInfo.Started) {
            if ($Mode -eq 'TradeReady') {
                Write-LauncherSuccess "Started verified trade-ready local paper daemon (PID $($daemonInfo.ProcessId))"
            }
            else {
                Write-LauncherSuccess "Started verified local paper daemon (PID $($daemonInfo.ProcessId))"
            }
            Write-Host "[Veritas Ledger] stdout: $($daemonInfo.StdoutLog)" -ForegroundColor DarkGray
            Write-Host "[Veritas Ledger] stderr: $($daemonInfo.StderrLog)" -ForegroundColor DarkGray
        }
        else {
            Write-LauncherSuccess 'Verified local paper daemon was already running; GUI attached without starting runtime'
        }

        if ($guiLaunched) {
            if ($Mode -eq 'TradeReady') {
                Write-LauncherSuccess 'GUI opened in trade-ready mode against the verified canonical backend. Trading runtime remains idle until you explicitly start it.'
            }
            else {
                Write-LauncherSuccess 'GUI opened in observe/attach mode against the verified canonical backend. No runtime auto-start was performed.'
            }
        }
        else {
            Write-LauncherSuccess 'GUI launch skipped (-SkipGui); verified canonical backend is attached headless. No runtime auto-start was performed.'
        }

        if ($CaptureStartupEvidence.IsPresent) {
            Invoke-StartupEvidenceCapture -RepoRoot $repoRoot
        }
    }
    finally {
        Restore-EnvSnapshot -Snapshot $envSnapshot
    }
}
catch {
    Write-Host ''
    Write-Host '[Veritas Ledger] LAUNCH FAILED' -ForegroundColor Red
    Write-Host "  $($_.Exception.Message)" -ForegroundColor Red
    Write-Host ''

    # SCHEDULED-HEADLESS-BOOTSTRAP-NONINTERACTIVE-01: -SkipGui is the
    # headless/scheduled invocation contract (Start-MiniQuantDesk.ps1 always
    # sets it for -Scheduled). A headless failure must return a deterministic
    # nonzero exit immediately -- it must never block on Read-Host waiting for
    # a keypress a scheduled/unattended invocation can never supply. Prompting
    # remains available only for genuine interactive runs (no -SkipGui).
    if ($SkipGui.IsPresent) {
        Write-Host 'Headless invocation (-SkipGui): exiting nonzero without prompting for input.' -ForegroundColor Yellow
        exit 1
    }

    Write-Host 'Review the error above. Press Enter to close this window.' -ForegroundColor Yellow
    $null = Read-Host
    exit 1
}
}
