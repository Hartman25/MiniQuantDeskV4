[CmdletBinding()]
param(
    [string]$ZipPath = "$HOME\Desktop\MiniQuantDeskV4_clean_repo.zip",
    [string]$RepoRoot = "$HOME\Desktop\MiniQuantDeskV4_snapshot",
    [string]$PostgresContainer = "mqk-reality-postgres",
    [int]$PostgresHostPort = 5440,
    [string]$DbUser = "mqk",
    [string]$DbPassword = "mqk",
    [string]$DbName = "mqk_v4",
    [string]$OperatorToken = "mqk-dev-token",
    [string]$DaemonAddr = "127.0.0.1:8899",
    [string]$LoopSymbol = "AAPL",
    [int]$CrashDelaySeconds = 8,
    [string]$AlpacaPaperKey = $env:ALPACA_API_KEY_PAPER,
    [string]$AlpacaPaperSecret = $env:ALPACA_API_SECRET_PAPER,
    [string]$AlpacaPaperBaseUrl = $env:ALPACA_PAPER_BASE_URL,
    [string]$StrategyId = "swing_momentum",
    [string]$DotEnvPath = "",
    [switch]$SkipDotEnvLoad,
    [switch]$SkipUnpack,
    [switch]$KeepRepo,
    [switch]$KeepPostgres,
    [switch]$SkipCrash,
    [switch]$AllowStartRefusal
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Import-DotEnvFile {
    param(
        [Parameter(Mandatory = $true)][string]$Path
    )

    if (-not (Test-Path $Path)) {
        throw ".env file not found: $Path"
    }

    Write-Host "Loading environment from $Path" -ForegroundColor DarkGray

    foreach ($line in Get-Content -Path $Path) {
        $trimmed = $line.Trim()

        if ([string]::IsNullOrWhiteSpace($trimmed)) { continue }
        if ($trimmed.StartsWith('#')) { continue }
        if (-not $trimmed.Contains('=')) { continue }

        $parts = $trimmed.Split('=', 2)
        $key = $parts[0].Trim()
        $value = $parts[1].Trim()

        if ([string]::IsNullOrWhiteSpace($key)) { continue }

        if (
            ($value.StartsWith('"') -and $value.EndsWith('"')) -or
            ($value.StartsWith("'") -and $value.EndsWith("'"))
        ) {
            $value = $value.Substring(1, $value.Length - 2)
        }

        [System.Environment]::SetEnvironmentVariable($key, $value, 'Process')
        Set-Item -Path ("Env:{0}" -f $key) -Value $value
    }
}

function Write-Step {
    param([string]$Message)
    Write-Host "`n=== $Message ===" -ForegroundColor Cyan
}

function Invoke-Checked {
    param(
        [Parameter(Mandatory = $true)][string]$FilePath,
        [Parameter(Mandatory = $true)][string[]]$ArgumentList,
        [string]$WorkingDirectory = (Get-Location).Path,
        [hashtable]$ExtraEnv = @{},
        [switch]$AllowFailure
    )

    Write-Host ("> {0} {1}" -f $FilePath, ($ArgumentList -join ' ')) -ForegroundColor DarkGray

    function Quote-ProcessArgument {
        param([AllowNull()][string]$Value)

        if ($null -eq $Value) { return '""' }
        if ($Value.Length -eq 0) { return '""' }

        if ($Value -notmatch '[\s"]') {
            return $Value
        }

        $escaped = $Value -replace '(\\*)"', '$1$1\"'
        $escaped = $escaped -replace '(\\+)$', '$1$1'
        return '"' + $escaped + '"'
    }

    $quotedArgs = @()
    foreach ($arg in $ArgumentList) {
        $quotedArgs += (Quote-ProcessArgument -Value ([string]$arg))
    }

    $psi = New-Object System.Diagnostics.ProcessStartInfo
    $psi.FileName = $FilePath
    $psi.WorkingDirectory = $WorkingDirectory
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $psi.Arguments = ($quotedArgs -join ' ')

    foreach ($key in [System.Environment]::GetEnvironmentVariables().Keys) {
        $name = [string]$key
        $psi.EnvironmentVariables[$name] = [string][System.Environment]::GetEnvironmentVariable($name)
    }
    foreach ($key in $ExtraEnv.Keys) {
        $psi.EnvironmentVariables[[string]$key] = [string]$ExtraEnv[$key]
    }

    $proc = New-Object System.Diagnostics.Process
    $proc.StartInfo = $psi
    [void]$proc.Start()
    $stdout = $proc.StandardOutput.ReadToEnd()
    $stderr = $proc.StandardError.ReadToEnd()
    $proc.WaitForExit()

    if ($stdout) { Write-Host $stdout.TrimEnd() }
    if ($stderr) { Write-Host $stderr.TrimEnd() -ForegroundColor Yellow }

    if (-not $AllowFailure -and $proc.ExitCode -ne 0) {
        throw "Command failed with exit code $($proc.ExitCode): $FilePath $($ArgumentList -join ' ')"
    }

    [pscustomobject]@{
        ExitCode = $proc.ExitCode
        StdOut   = $stdout
        StdErr   = $stderr
    }
}

function Invoke-JsonGet {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [switch]$AllowFailure
    )

    Write-Host ("> GET {0}" -f $Uri) -ForegroundColor DarkGray
    try {
        $resp = Invoke-RestMethod -Uri $Uri -Method Get -TimeoutSec 15

        if ($null -ne $resp) {
            Write-Host ($resp | ConvertTo-Json -Depth 12)
        }

        return $resp
    } catch {
        if (-not $AllowFailure) { throw }

        $status = $null
        $body = $null

        if ($_.Exception.Response) {
            try {
                $status = $_.Exception.Response.StatusCode.value__
            } catch {}

            try {
                $reader = New-Object System.IO.StreamReader($_.Exception.Response.GetResponseStream())
                $body = $reader.ReadToEnd()
            } catch {}
        }

        Write-Host "HTTP GET failed: $status $body" -ForegroundColor Yellow
        return [pscustomobject]@{
            __error = $true
            status  = $status
            body    = $body
        }
    }
}

function Invoke-JsonPost {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [hashtable]$Headers = @{},
        [object]$Body = $null,
        [switch]$AllowFailure
    )

    Write-Host ("> POST {0}" -f $Uri) -ForegroundColor DarkGray
    try {
        if ($null -ne $Body) {
            $json = $Body | ConvertTo-Json -Depth 12
            $resp = Invoke-RestMethod -Uri $Uri -Method Post -Headers $Headers -ContentType 'application/json' -Body $json -TimeoutSec 20
        } else {
            $resp = Invoke-RestMethod -Uri $Uri -Method Post -Headers $Headers -TimeoutSec 20
        }

        if ($null -ne $resp) {
            Write-Host ($resp | ConvertTo-Json -Depth 12)
        }

        return $resp
    } catch {
        if (-not $AllowFailure) { throw }

        $status = $null
        $bodyText = $null
        $parsedBody = $null

        if ($_.Exception.Response) {
            try {
                $status = $_.Exception.Response.StatusCode.value__
            } catch {}

            try {
                $reader = New-Object System.IO.StreamReader($_.Exception.Response.GetResponseStream())
                $bodyText = $reader.ReadToEnd()
            } catch {}
        }

        if (-not [string]::IsNullOrWhiteSpace($bodyText)) {
            try {
                $parsedBody = $bodyText | ConvertFrom-Json -Depth 20
            } catch {
                $parsedBody = $null
            }
        }

        Write-Host "HTTP POST failed: $status" -ForegroundColor Yellow
        if ($parsedBody -ne $null) {
            Write-Host ($parsedBody | ConvertTo-Json -Depth 20) -ForegroundColor Yellow
        } elseif (-not [string]::IsNullOrWhiteSpace($bodyText)) {
            Write-Host $bodyText -ForegroundColor Yellow
        }

        return [pscustomobject]@{
            __error    = $true
            status     = $status
            body       = $bodyText
            parsedBody = $parsedBody
        }
    }
}

function Wait-HttpReady {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [int]$TimeoutSeconds = 90
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        try {
            $null = Invoke-RestMethod -Uri $Uri -Method Get -TimeoutSec 5
            return
        } catch {
            Start-Sleep -Seconds 1
        }
    } while ((Get-Date) -lt $deadline)

    throw "Timed out waiting for HTTP readiness at $Uri"
}

function Wait-AutonomousReadiness {
    param(
        [Parameter(Mandatory = $true)][string]$Uri,
        [int]$TimeoutSeconds = 60
    )

    $deadline = (Get-Date).AddSeconds($TimeoutSeconds)
    do {
        $resp = Invoke-JsonGet -Uri $Uri -AllowFailure
        if (-not $resp.PSObject.Properties.Name.Contains('__error')) {
            if ($resp.canonical_path -eq $true -and $resp.ws_continuity_ready -eq $true) {
                return $resp
            }
        }
        Start-Sleep -Seconds 2
    } while ((Get-Date) -lt $deadline)

    return $resp
}

$repoCore = Join-Path $RepoRoot 'core-rs'

if (-not $SkipDotEnvLoad) {
    if ([string]::IsNullOrWhiteSpace($DotEnvPath)) {
        $coreEnv = Join-Path $repoCore '.env.local'
        $rootEnv = Join-Path $RepoRoot '.env.local'

        if (Test-Path $coreEnv) {
            $DotEnvPath = $coreEnv
        } elseif (Test-Path $rootEnv) {
            $DotEnvPath = $rootEnv
        }
    }

    if (-not [string]::IsNullOrWhiteSpace($DotEnvPath)) {
        Import-DotEnvFile -Path $DotEnvPath
    } else {
        Write-Host 'No .env.local found; using existing process environment only.' -ForegroundColor Yellow
    }
}

$dbUrl = "postgres://{0}:{1}@localhost:{2}/{3}" -f $DbUser, $DbPassword, $PostgresHostPort, $DbName
$healthUri = "http://$DaemonAddr/v1/health"
$statusUri = "http://$DaemonAddr/v1/status"
$armUri = "http://$DaemonAddr/v1/integrity/arm"
$runStartUri = "http://$DaemonAddr/v1/run/start"
$runStopUri = "http://$DaemonAddr/v1/run/stop"
$preflightUri = "http://$DaemonAddr/api/v1/system/preflight"
$autonomousUri = "http://$DaemonAddr/api/v1/autonomous/readiness"
$headers = @{ Authorization = "Bearer $OperatorToken" }
$daemonLog = Join-Path $RepoRoot 'daemon.out.log'
$daemonErrLog = Join-Path $RepoRoot 'daemon.err.log'

if (-not $SkipUnpack) {
    Write-Step "Unpack snapshot"
    if (-not (Test-Path $ZipPath)) {
        throw "Zip not found: $ZipPath"
    }
    if (Test-Path $RepoRoot) {
        Remove-Item -Recurse -Force $RepoRoot
    }
    Expand-Archive -Path $ZipPath -DestinationPath $RepoRoot -Force
}

if (-not (Test-Path $repoCore)) {
    throw "Repo core path not found: $repoCore"
}

Write-Step "Reset Postgres container"
Invoke-Checked -FilePath 'docker' -ArgumentList @('rm', '-f', $PostgresContainer) -AllowFailure | Out-Null
Invoke-Checked -FilePath 'docker' -ArgumentList @(
    'run', '--name', $PostgresContainer,
    '-e', "POSTGRES_USER=$DbUser",
    '-e', "POSTGRES_PASSWORD=$DbPassword",
    '-e', "POSTGRES_DB=$DbName",
    '-p', "${PostgresHostPort}:5432",
    '-d', 'postgres:16'
) | Out-Null

Write-Step "Set environment for canonical paper+alpaca daemon path"
$env:MQK_DATABASE_URL = $dbUrl
if ([string]::IsNullOrWhiteSpace($env:MQK_OPERATOR_TOKEN)) {
    $env:MQK_OPERATOR_TOKEN = $OperatorToken
}
if ([string]::IsNullOrWhiteSpace($env:RUST_LOG)) {
    $env:RUST_LOG = 'info'
}
if ([string]::IsNullOrWhiteSpace($env:MQK_DAEMON_ADDR)) {
    $env:MQK_DAEMON_ADDR = $DaemonAddr
}
if ([string]::IsNullOrWhiteSpace($env:MQK_DAEMON_DEPLOYMENT_MODE)) {
    $env:MQK_DAEMON_DEPLOYMENT_MODE = 'paper'
}
if ([string]::IsNullOrWhiteSpace($env:MQK_DAEMON_ADAPTER_ID)) {
    $env:MQK_DAEMON_ADAPTER_ID = 'alpaca'
}
if ([string]::IsNullOrWhiteSpace($env:MQK_STRATEGY_IDS)) {
    $env:MQK_STRATEGY_IDS = $StrategyId
}

$OperatorToken = $env:MQK_OPERATOR_TOKEN
$DaemonAddr = $env:MQK_DAEMON_ADDR
$headers = @{ Authorization = "Bearer $OperatorToken" }
$healthUri = "http://$DaemonAddr/v1/health"
$statusUri = "http://$DaemonAddr/v1/status"
$armUri = "http://$DaemonAddr/v1/integrity/arm"
$runStartUri = "http://$DaemonAddr/v1/run/start"
$runStopUri = "http://$DaemonAddr/v1/run/stop"
$preflightUri = "http://$DaemonAddr/api/v1/system/preflight"
$autonomousUri = "http://$DaemonAddr/api/v1/autonomous/readiness"

if ([string]::IsNullOrWhiteSpace($AlpacaPaperKey)) {
    $AlpacaPaperKey = $env:ALPACA_API_KEY_PAPER
}
if ([string]::IsNullOrWhiteSpace($AlpacaPaperSecret)) {
    $AlpacaPaperSecret = $env:ALPACA_API_SECRET_PAPER
}
if ([string]::IsNullOrWhiteSpace($AlpacaPaperBaseUrl)) {
    $AlpacaPaperBaseUrl = $env:ALPACA_PAPER_BASE_URL
}

if ([string]::IsNullOrWhiteSpace($AlpacaPaperKey) -or [string]::IsNullOrWhiteSpace($AlpacaPaperSecret)) {
    throw "ALPACA_API_KEY_PAPER and ALPACA_API_SECRET_PAPER are required for the canonical paper+alpaca path."
}

$env:ALPACA_API_KEY_PAPER = $AlpacaPaperKey
$env:ALPACA_API_SECRET_PAPER = $AlpacaPaperSecret
if (-not [string]::IsNullOrWhiteSpace($AlpacaPaperBaseUrl)) {
    $env:ALPACA_PAPER_BASE_URL = $AlpacaPaperBaseUrl
}

Write-Host "MQK_DATABASE_URL=$dbUrl"
Write-Host "MQK_OPERATOR_TOKEN=$OperatorToken"
Write-Host "MQK_DAEMON_ADDR=$DaemonAddr"
Write-Host "MQK_DAEMON_DEPLOYMENT_MODE=$($env:MQK_DAEMON_DEPLOYMENT_MODE)"
Write-Host "MQK_DAEMON_ADAPTER_ID=$($env:MQK_DAEMON_ADAPTER_ID)"
Write-Host "MQK_STRATEGY_IDS=$($env:MQK_STRATEGY_IDS)"

Write-Step "Migrate database"
Invoke-Checked -FilePath 'cargo' -ArgumentList @('run', '-p', 'mqk-cli', '--bin', 'mqk-cli', '--', 'db', 'migrate', '--yes') -WorkingDirectory $repoCore | Out-Null

Write-Step "Build workspace"
Invoke-Checked -FilePath 'cargo' -ArgumentList @('build', '--workspace') -WorkingDirectory $repoCore | Out-Null

Write-Step "Start daemon"
if (Test-Path $daemonLog) {
    Remove-Item -Force $daemonLog
}
if (Test-Path $daemonErrLog) {
    Remove-Item -Force $daemonErrLog
}
$daemonProc = Start-Process -FilePath 'cargo' `
    -ArgumentList @('run', '-p', 'mqk-daemon') `
    -WorkingDirectory $repoCore `
    -RedirectStandardOutput $daemonLog `
    -RedirectStandardError $daemonErrLog `
    -PassThru

Write-Host "Daemon PID: $($daemonProc.Id)"
Wait-HttpReady -Uri $healthUri -TimeoutSeconds 90

Write-Step "Pre-run daemon truth checks"
Invoke-JsonGet -Uri $healthUri | Out-Null
Invoke-JsonGet -Uri $statusUri | Out-Null
$preflight = Invoke-JsonGet -Uri $preflightUri
$autonomous = Wait-AutonomousReadiness -Uri $autonomousUri -TimeoutSeconds 45

Write-Step "Arm integrity"
Invoke-JsonPost -Uri $armUri -Headers $headers | Out-Null
Invoke-JsonGet -Uri $statusUri | Out-Null

Write-Step "Attempt canonical daemon run start"
$startResp = Invoke-JsonPost -Uri $runStartUri -Headers $headers -AllowFailure
$startFailed = $startResp.PSObject.Properties.Name.Contains('__error')

if ($startFailed) {
    Write-Host "Run start was refused by the daemon control plane." -ForegroundColor Yellow

    if ($startResp.PSObject.Properties.Name.Contains('status')) {
        Write-Host ("Start refusal HTTP status: {0}" -f $startResp.status) -ForegroundColor Yellow
    }

    if ($startResp.PSObject.Properties.Name.Contains('parsedBody') -and $null -ne $startResp.parsedBody) {
        Write-Host "Start refusal body (parsed):" -ForegroundColor Yellow
        Write-Host ($startResp.parsedBody | ConvertTo-Json -Depth 20) -ForegroundColor Yellow
    } elseif ($startResp.PSObject.Properties.Name.Contains('body') -and -not [string]::IsNullOrWhiteSpace($startResp.body)) {
        Write-Host "Start refusal body (raw):" -ForegroundColor Yellow
        Write-Host $startResp.body -ForegroundColor Yellow
    }

    Write-Host "Immediate post-refusal autonomous readiness snapshot:" -ForegroundColor Yellow
    Invoke-JsonGet -Uri $autonomousUri -AllowFailure | Out-Null

    Write-Host "This is only acceptable when the refusal is truthful (for example outside session window, WS continuity not yet proven, or another real gate)." -ForegroundColor Yellow
    if (-not $AllowStartRefusal) {
        throw "Daemon run start failed. Re-run with -AllowStartRefusal only if you are intentionally auditing truthful fail-closed behavior."
    }
}

$runId = $null
if (-not $startFailed -and $startResp.active_run_id) {
    $runId = [string]$startResp.active_run_id
    Write-Host "RUN_ID=$runId" -ForegroundColor Green
} else {
    $status = Invoke-JsonGet -Uri $statusUri -AllowFailure
    if (-not $status.PSObject.Properties.Name.Contains('__error') -and $status.active_run_id) {
        $runId = [string]$status.active_run_id
        Write-Host "RUN_ID=$runId" -ForegroundColor Green
    }
}

if ($runId -and -not $SkipCrash) {
    Write-Step "Schedule daemon crash"
    $crashJob = Start-Job -ArgumentList $daemonProc.Id, $CrashDelaySeconds -ScriptBlock {
        param($PidToKill, $Delay)
        Start-Sleep -Seconds $Delay
        try {
            Stop-Process -Id $PidToKill -Force -ErrorAction Stop
            "Killed daemon PID $PidToKill after $Delay seconds"
        } catch {
            "Failed to kill daemon PID ${PidToKill}: $($_.Exception.Message)"
        }
    }

    Write-Step "Wait for forced crash"
    $crashOutput = Receive-Job -Job $crashJob -Wait -AutoRemoveJob
    if ($crashOutput) {
        Write-Host $crashOutput -ForegroundColor Yellow
    }

    Write-Step "Restart daemon after crash"
    $daemonProc2 = Start-Process -FilePath 'cargo' `
        -ArgumentList @('run', '-p', 'mqk-daemon') `
        -WorkingDirectory $repoCore `
        -RedirectStandardOutput $daemonLog `
        -RedirectStandardError $daemonErrLog `
        -PassThru
    Write-Host "Restarted daemon PID: $($daemonProc2.Id)"
    Wait-HttpReady -Uri $healthUri -TimeoutSeconds 90

    Write-Step "Post-restart truth checks"
    Invoke-JsonGet -Uri $healthUri | Out-Null
    Invoke-JsonGet -Uri $statusUri | Out-Null
    Invoke-JsonGet -Uri $autonomousUri -AllowFailure | Out-Null
} else {
    Write-Step "Crash phase skipped"
    Write-Host "Either no active run was created or -SkipCrash was set." -ForegroundColor Yellow
}

Write-Step "Graceful daemon stop attempt"
Invoke-JsonPost -Uri $runStopUri -Headers $headers -AllowFailure | Out-Null
Invoke-JsonGet -Uri $statusUri -AllowFailure | Out-Null

Write-Step "Reality-test summary"
Write-Host "Run ID: $runId"
Write-Host "Daemon log: $daemonLog"
Write-Host "Daemon err log: $daemonErrLog"
Write-Host "DB URL: $dbUrl"
Write-Host "Canonical path: paper+alpaca via daemon control plane"
Write-Host "Strategy fleet: $StrategyId"

Write-Host "`nManual follow-up checks:" -ForegroundColor Cyan
Write-Host "1. Open daemon log and inspect WS continuity establishment, crash, restart, and lease behavior."
Write-Host "2. Verify /api/v1/autonomous/readiness stayed truthful before and after crash/restart."
Write-Host "3. Verify no synthetic success was reported if run/start was refused."
Write-Host "4. If run/start was refused, inspect the printed refusal body and the immediate post-refusal readiness snapshot."
Write-Host "5. If a run did start, verify post-restart runtime truth, not just HTTP liveness."

if (-not $KeepRepo) {
    Write-Host "`nRepo kept at: $RepoRoot" -ForegroundColor DarkYellow
}

if (-not $KeepPostgres) {
    Write-Host "Postgres container kept running as: $PostgresContainer" -ForegroundColor DarkYellow
    Write-Host "Remove manually when done: docker rm -f $PostgresContainer"
}