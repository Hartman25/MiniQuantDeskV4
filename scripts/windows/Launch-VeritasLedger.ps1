[CmdletBinding()]
param(
    [ValidateSet('Observe', 'TradeReady')]
    [string]$Mode = 'Observe',
    [switch]$Rebuild
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
        & $FilePath @Arguments
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

function Ensure-DaemonBinary {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][bool]$RebuildRequested
    )

    if ($RebuildRequested) {
        $cargo = Get-CommandPath 'cargo'
        Write-LauncherStep 'Rebuilding mqk-daemon --release'
        Invoke-ExternalCommand -FilePath $cargo -Arguments @('build', '-p', 'mqk-daemon', '--release') -WorkingDirectory (Join-Path $RepoRoot 'core-rs')
    }

    try {
        return Resolve-DaemonBinary -RepoRoot $RepoRoot
    }
    catch {
        $cargo = Get-CommandPath 'cargo'
        Write-LauncherStep 'Building mqk-daemon --release'
        Invoke-ExternalCommand -FilePath $cargo -Arguments @('build', '-p', 'mqk-daemon', '--release') -WorkingDirectory (Join-Path $RepoRoot 'core-rs')
        return Resolve-DaemonBinary -RepoRoot $RepoRoot
    }
}

function Ensure-GuiBinary {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][bool]$RebuildRequested
    )

    if (-not $RebuildRequested) {
        $existing = Resolve-GuiBinary -RepoRoot $RepoRoot
        if ($existing) {
            return $existing
        }
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
    param([Parameter(Mandatory = $true)][string]$OperatorToken)

    $names = @(
        'MQK_DAEMON_DEPLOYMENT_MODE',
        'MQK_DAEMON_ADAPTER_ID',
        'MQK_DAEMON_ADDR',
        'MQK_GUI_DAEMON_URL',
        'MQK_GUI_OPERATOR_TOKEN',
        'MQK_OPERATOR_TOKEN'
    )

    $snapshot = New-EnvSnapshot -Names $names

    $env:MQK_DAEMON_DEPLOYMENT_MODE = 'paper'
    $env:MQK_DAEMON_ADAPTER_ID = 'alpaca'
    $env:MQK_DAEMON_ADDR = '127.0.0.1:8899'
    $env:MQK_GUI_DAEMON_URL = 'http://127.0.0.1:8899'
    $env:MQK_GUI_OPERATOR_TOKEN = $OperatorToken
    $env:MQK_OPERATOR_TOKEN = $OperatorToken

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
        [Parameter(Mandatory = $true)][System.Collections.ArrayList]$Reasons,
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
    param([string[]]$Reasons)

    if ($null -eq $Reasons -or $Reasons.Count -eq 0) {
        return 'none'
    }

    return ($Reasons -join '; ')
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

    foreach ($blocker in @($Probe.Preflight.blockers)) {
        Add-UniqueReason -Reasons $reasons -Reason ([string]$blocker)
    }

    foreach ($blocker in @($Probe.AutonomousReadiness.blockers)) {
        Add-UniqueReason -Reasons $reasons -Reason ([string]$blocker)
    }

    return $reasons.ToArray()
}

function Get-BackendProbe {
    param(
        [Parameter(Mandatory = $true)][string]$BaseUrl,
        [Parameter(Mandatory = $true)][string]$OperatorToken,
        [ValidateSet('Observe', 'TradeReady')]
        [string]$Mode = 'Observe'
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

    if ($Mode -eq 'TradeReady') {
        # TradeReady mode requires a live Bearer auth round-trip before attaching.
        # Observe/Attach mode is strictly idle-only and must not POST to operator routes.
        # This probe calls the canonical dispatcher with an impossible action_key;
        # 400 + unknown_action + accepted=false proves Bearer auth worked without
        # changing runtime state.
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
            if ($details.StatusCode -eq 400 -and $details.Json.disposition -eq 'unknown_action' -and $details.Json.accepted -eq $false) {
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
    }

    $result.IdentityVerified = $true
    $result.TradeReadinessReasons = Get-TradeReadinessReasons -Probe ([pscustomobject]$result)
    $result.TradeReady = ($result.TradeReadinessReasons.Count -eq 0)
    $result.FailureReason = $null
    return [pscustomobject]$result
}

function Wait-ForBackendState {
    param(
        [Parameter(Mandatory = $true)][string]$BaseUrl,
        [Parameter(Mandatory = $true)][string]$OperatorToken,
        [Parameter(Mandatory = $true)][int]$TimeoutSeconds,
        [Parameter(Mandatory = $true)][bool]$RequireTradeReady,
        [ValidateSet('Observe', 'TradeReady')]
        [string]$Mode = 'Observe'
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    $lastProbe = $null
    while ((Get-Date) -lt $deadline) {
        $lastProbe = Get-BackendProbe -BaseUrl $BaseUrl -OperatorToken $OperatorToken -Mode $Mode
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

    Write-LauncherWarn ('Backend is NOT trade-ready: ' + (Join-Reasons -Reasons $Probe.TradeReadinessReasons))
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
    $existingProbe = Get-BackendProbe -BaseUrl $BaseUrl -OperatorToken $OperatorToken -Mode $LauncherMode
    if ($existingProbe.IdentityVerified) {
        if ($requireTradeReady -and -not $existingProbe.TradeReady) {
            throw "Verified canonical backend is not trade-ready. $(Join-Reasons -Reasons $existingProbe.TradeReadinessReasons)"
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
        $probe = Wait-ForBackendState -BaseUrl $BaseUrl -OperatorToken $OperatorToken -TimeoutSeconds 30 -RequireTradeReady:$requireTradeReady -Mode $LauncherMode
        if (-not $probe.IdentityVerified) {
            throw "daemon did not reach verified canonical identity. $($probe.FailureReason)"
        }
    }
    catch {
        if (-not $process.HasExited) {
            $process | Stop-Process -Force -ErrorAction SilentlyContinue
        }
        throw "mqk-daemon failed to reach required $((Get-ModeDisplayName -LauncherMode $LauncherMode)) launcher state. stdout=$stdoutLog stderr=$stderrLog"
    }

    if ($requireTradeReady -and -not $probe.TradeReady) {
        if (-not $process.HasExited) {
            $process | Stop-Process -Force -ErrorAction SilentlyContinue
        }
        throw "Verified canonical backend started but is not trade-ready. $(Join-Reasons -Reasons $probe.TradeReadinessReasons) stdout=$stdoutLog stderr=$stderrLog"
    }

    return [pscustomobject]@{
        Started = $true
        Probe = $probe
        ProcessId = $process.Id
        StdoutLog = $stdoutLog
        StderrLog = $stderrLog
    }
}

$repoRoot = Get-RepoRoot
Import-LauncherEnvironmentFiles -RepoRoot $repoRoot
$operatorToken = Resolve-RequiredOperatorToken
$envSnapshot = Set-LauncherEnvironment -OperatorToken $operatorToken

try {
    Write-LauncherStep "Launcher mode: $(Get-ModeDisplayName -LauncherMode $Mode)"
    Write-LauncherStep 'Resolving daemon binary'
    $daemonExe = Ensure-DaemonBinary -RepoRoot $repoRoot -RebuildRequested:$Rebuild.IsPresent

    $daemonInfo = Start-DaemonIfNeeded -DaemonExe $daemonExe -RepoRoot $repoRoot -BaseUrl $env:MQK_GUI_DAEMON_URL -OperatorToken $operatorToken -LauncherMode $Mode
    $verified = $daemonInfo.Probe

    Write-BackendSummary -Probe $verified -LauncherMode $Mode

    Write-LauncherStep 'Resolving desktop GUI binary'
    $guiExe = Ensure-GuiBinary -RepoRoot $repoRoot -RebuildRequested:$Rebuild.IsPresent

    Write-LauncherStep 'Launching desktop GUI against verified local daemon'
    Start-Process -FilePath $guiExe -WorkingDirectory (Split-Path -Parent $guiExe) | Out-Null

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

    if ($Mode -eq 'TradeReady') {
        Write-LauncherSuccess 'GUI opened in trade-ready mode against the verified canonical backend. Trading runtime remains idle until you explicitly start it.'
    }
    else {
        Write-LauncherSuccess 'GUI opened in observe/attach mode against the verified canonical backend. No runtime auto-start was performed.'
    }
}
finally {
    Restore-EnvSnapshot -Snapshot $envSnapshot
}
