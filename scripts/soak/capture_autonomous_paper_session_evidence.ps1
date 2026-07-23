# =============================================================================
# capture_autonomous_paper_session_evidence.ps1
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F3-SUPERVISED-SOAK-EVIDENCE-PREPARATION
#
# Read-only, GET-only evidence capture for one supervised Paper + Alpaca
# autonomous session snapshot. This tool prepares evidence-capture tooling
# only -- it does NOT perform, start, count, or claim an unattended soak
# session. One capture is one point-in-time snapshot, never a completed
# soak session by itself.
#
# Safety, enforced by design and proved by the F3 guard's source scan:
#   - GET only. This script never calls Invoke-RestMethod/Invoke-WebRequest
#     with any method other than GET, and never references POST/PUT/PATCH/
#     DELETE anywhere in its source.
#   - Contacts only the explicitly configured local daemon base URL, and
#     refuses to run against any host other than 127.0.0.1/localhost/::1
#     (fail-closed host check below).
#   - Never calls Alpaca or Discord directly, and makes no other external
#     network request.
#   - Never starts or stops a runtime, never arms, disarms, flattens, or
#     finalizes anything -- every route it calls is an existing read-only
#     GET surface.
#   - Never reads or copies .env.local (not referenced anywhere below).
#   - Never prints or persists a credential, API key, secret, or token --
#     this script never touches ALPACA_*, MQK_OPERATOR_TOKEN, or any
#     database-URL environment variable.
#   - Requires an explicit -OutputDirectory and writes only inside it.
#
# Modes:
#   -ValidateOnly   Performs no daemon calls and writes nothing to disk;
#                   reports what it would do and exits 0. Safe to run
#                   anywhere, including CI, with no daemon present.
#   -FixturePath    Reads canned JSON fixture files from a local directory
#                   instead of contacting a real daemon (loopback-test-only
#                   path for exercising the manifest-building logic without
#                   a live daemon).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\soak\capture_autonomous_paper_session_evidence.ps1 `
#     -OutputDirectory smoke_logs\autonomous_paper_soak\2026-07-22\pre_session `
#     -CapturePhase pre_session
#
#   powershell -ExecutionPolicy Bypass -File scripts\soak\capture_autonomous_paper_session_evidence.ps1 `
#     -OutputDirectory smoke_logs\autonomous_paper_soak\validate_only_check `
#     -CapturePhase pre_session -ValidateOnly
#
# Output:
#   <OutputDirectory>\autonomous_paper_session_manifest.json
#   Schema: scripts\soak\templates\autonomous_paper_session_manifest.template.json
# =============================================================================

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$OutputDirectory,

    [Parameter(Mandatory = $true)]
    [ValidateSet('pre_session', 'mid_session', 'post_session', 'incident', 'restart')]
    [string]$CapturePhase,

    [string]$DaemonBaseUrl = 'http://127.0.0.1:8899',

    [string]$RepoRoot = '',

    [switch]$OperatorSupervised = $true,

    [switch]$ValidateOnly,

    [string]$FixturePath = '',

    [string]$OperatorNotes = ''
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$SchemaVersion = 'autonomous-paper-soak-evidence-v1'

# ---------------------------------------------------------------------------
# Resolve repo root
# ---------------------------------------------------------------------------
if ($RepoRoot -eq '') {
    $ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
    $RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
}

# ---------------------------------------------------------------------------
# Fail-closed host check: never contact anything but a local daemon.
# ---------------------------------------------------------------------------
try {
    $parsedUri = [Uri]$DaemonBaseUrl
} catch {
    Write-Host "REFUSED: -DaemonBaseUrl '$DaemonBaseUrl' is not a valid URI." -ForegroundColor Red
    exit 1
}
$AllowedHosts = @('127.0.0.1', 'localhost', '::1')
if ($AllowedHosts -notcontains $parsedUri.Host) {
    Write-Host "REFUSED: -DaemonBaseUrl host '$($parsedUri.Host)' is not a local daemon host (allowed: $($AllowedHosts -join ', '))." -ForegroundColor Red
    Write-Host "This tool never contacts a non-local host -- refusing to proceed." -ForegroundColor Red
    exit 1
}

Write-Host ""
Write-Host "=== capture_autonomous_paper_session_evidence.ps1 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F3) ===" -ForegroundColor Cyan
Write-Host "    Capture phase : $CapturePhase"
Write-Host "    Daemon        : $DaemonBaseUrl"
Write-Host "    Output dir    : $OutputDirectory"
Write-Host "    ValidateOnly  : $($ValidateOnly.IsPresent)"
Write-Host "    FixturePath   : $(if ($FixturePath -eq '') { '(none -- real daemon)' } else { $FixturePath })"
Write-Host ""

# ---------------------------------------------------------------------------
# -ValidateOnly: report and exit, no daemon call, no disk write.
# ---------------------------------------------------------------------------
if ($ValidateOnly) {
    Write-Host "VALIDATE-ONLY MODE -- no daemon call will be made, no file will be written." -ForegroundColor Yellow
    Write-Host "Would capture phase '$CapturePhase' from '$DaemonBaseUrl' into '$OutputDirectory'."
    Write-Host "Manifest schema: $SchemaVersion"
    exit 0
}

# ---------------------------------------------------------------------------
# GET-only daemon helper. Never any other HTTP method. Fail-soft: returns
# $null and records the failure; never throws past this function.
# ---------------------------------------------------------------------------
$CaptureErrors = @()
$MissingEndpoints = @()

function Invoke-DaemonGetOnly {
    param([string]$Path)
    if ($FixturePath -ne '') {
        $PathNoQuery = $Path.Split('?')[0]
        $fixtureFile = Join-Path $FixturePath ($PathNoQuery.TrimStart('/').Replace('/', '_') + '.json')
        if (Test-Path $fixtureFile) {
            try {
                return (Get-Content -Raw -Path $fixtureFile | ConvertFrom-Json)
            } catch {
                $script:CaptureErrors += "fixture parse error for ${Path}: $_"
                $script:MissingEndpoints += $Path
                return $null
            }
        } else {
            $script:MissingEndpoints += $Path
            return $null
        }
    }
    try {
        $resp = Invoke-RestMethod -Uri "${DaemonBaseUrl}${Path}" -Method Get -TimeoutSec 5 -ErrorAction Stop
        return $resp
    } catch {
        $script:CaptureErrors += "GET ${Path} failed: $_"
        $script:MissingEndpoints += $Path
        return $null
    }
}

# ---------------------------------------------------------------------------
# Capture identity fields
# ---------------------------------------------------------------------------
$CapturedAtUtc = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
$SessionEvidenceId = "$CapturePhase-$((Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss'))"

$RepositoryCommit = $null
try {
    $RepositoryCommit = (& git -C $RepoRoot rev-parse HEAD 2>&1 | Select-Object -First 1)
} catch {
    $CaptureErrors += "git rev-parse HEAD failed: $_"
}

# ---------------------------------------------------------------------------
# Read-only daemon truth surfaces -- GET only, every one of the routes this
# runbook (docs/runbooks/autonomous_paper_ops.md, Part 2) documents as
# authoritative or evidence-relevant.
# ---------------------------------------------------------------------------
$SystemStatus            = Invoke-DaemonGetOnly -Path '/api/v1/system/status'
$SystemPreflight         = Invoke-DaemonGetOnly -Path '/api/v1/system/preflight'
$AutonomousReadiness     = Invoke-DaemonGetOnly -Path '/api/v1/autonomous/readiness'
$AutonomousPaperStatus   = Invoke-DaemonGetOnly -Path '/api/v1/autonomous/paper-status'
$CurrentDailyOperation   = Invoke-DaemonGetOnly -Path '/api/v1/autonomous/daily-operation'
$RecentDailyOperations   = Invoke-DaemonGetOnly -Path '/api/v1/autonomous/daily-operations?limit=20'
$OrdersSummary           = Invoke-DaemonGetOnly -Path '/api/v1/execution/orders'
$FillsSummary            = Invoke-DaemonGetOnly -Path '/api/v1/portfolio/fills'
$ReconcileStatus         = Invoke-DaemonGetOnly -Path '/api/v1/reconcile/status'
$RiskPosture             = Invoke-DaemonGetOnly -Path '/api/v1/risk/summary'

# completed_bar_task_status: no dedicated daemon route surfaces this field
# under its own name today. The closest existing read-only truth surface is
# the alerts feed (docs/runbooks/autonomous_paper_ops.md §19 points operators
# at "system/status / alerts for the specific adapter fault"). Captured
# verbatim from that surface, never synthesized or reinterpreted.
$CompletedBarTaskStatus  = Invoke-DaemonGetOnly -Path '/api/v1/alerts/active'

# ---------------------------------------------------------------------------
# deployment_mode / adapter_id -- read verbatim from system_status if present.
# Never defaulted or guessed when system_status itself is unavailable.
# ---------------------------------------------------------------------------
$DeploymentMode = $null
$AdapterId = $null
if ($null -ne $SystemStatus) {
    if ($null -ne $SystemStatus.PSObject.Properties['daemon_mode']) {
        $DeploymentMode = [string]$SystemStatus.daemon_mode
    }
    if ($null -ne $SystemStatus.PSObject.Properties['adapter_id']) {
        $AdapterId = [string]$SystemStatus.adapter_id
    }
}

# ---------------------------------------------------------------------------
# gui_build_version -- local file read only (package.json), no network.
# ---------------------------------------------------------------------------
$GuiBuildVersion = $null
try {
    $PackageJsonPath = Join-Path $RepoRoot 'core-rs\mqk-gui\package.json'
    if (Test-Path $PackageJsonPath) {
        $pkg = Get-Content -Raw -Path $PackageJsonPath | ConvertFrom-Json
        if ($null -ne $pkg.PSObject.Properties['version']) {
            $GuiBuildVersion = [string]$pkg.version
        }
    }
} catch {
    $CaptureErrors += "package.json version read failed: $_"
}

# ---------------------------------------------------------------------------
# Never a credential in operator_notes.
# ---------------------------------------------------------------------------
$OperatorNotesValue = if ($OperatorNotes -eq '') { $null } else { $OperatorNotes }

# ---------------------------------------------------------------------------
# artifact_hashes -- SHA256 of each captured section's serialized JSON, for
# tamper-evidence. Sections that are $null (unavailable) get no hash entry.
# ---------------------------------------------------------------------------
function Get-JsonSha256 {
    param($Value)
    if ($null -eq $Value) { return $null }
    $json = $Value | ConvertTo-Json -Depth 20 -Compress
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    $sha = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hashBytes = $sha.ComputeHash($bytes)
        return [System.BitConverter]::ToString($hashBytes).Replace('-', '').ToLowerInvariant()
    } finally {
        $sha.Dispose()
    }
}

$ArtifactHashes = [ordered]@{}
$HashCandidates = [ordered]@{
    system_status            = $SystemStatus
    system_preflight         = $SystemPreflight
    autonomous_readiness     = $AutonomousReadiness
    autonomous_paper_status  = $AutonomousPaperStatus
    current_daily_operation  = $CurrentDailyOperation
    recent_daily_operations  = $RecentDailyOperations
    orders_summary           = $OrdersSummary
    fills_summary            = $FillsSummary
    reconcile_status         = $ReconcileStatus
    risk_posture             = $RiskPosture
    completed_bar_task_status = $CompletedBarTaskStatus
}
foreach ($key in $HashCandidates.Keys) {
    $h = Get-JsonSha256 -Value $HashCandidates[$key]
    if ($null -ne $h) {
        $ArtifactHashes[$key] = $h
    }
}

# ---------------------------------------------------------------------------
# Build the manifest (field set matches
# scripts\soak\templates\autonomous_paper_session_manifest.template.json).
# ---------------------------------------------------------------------------
$Manifest = [ordered]@{
    schema_version           = $SchemaVersion
    session_evidence_id      = $SessionEvidenceId
    capture_phase            = $CapturePhase
    captured_at_utc          = $CapturedAtUtc
    market_date              = $null
    repository_commit        = $RepositoryCommit
    deployment_mode          = $DeploymentMode
    adapter_id               = $AdapterId
    daemon_base_url          = $DaemonBaseUrl
    operator_supervised      = [bool]$OperatorSupervised

    system_status             = $SystemStatus
    system_preflight          = $SystemPreflight
    autonomous_readiness      = $AutonomousReadiness
    autonomous_paper_status   = $AutonomousPaperStatus
    current_daily_operation   = $CurrentDailyOperation
    recent_daily_operations   = $RecentDailyOperations
    orders_summary            = $OrdersSummary
    fills_summary             = $FillsSummary
    reconcile_status          = $ReconcileStatus
    risk_posture              = $RiskPosture
    completed_bar_task_status = $CompletedBarTaskStatus
    gui_build_version         = $GuiBuildVersion

    capture_errors      = @($CaptureErrors)
    missing_endpoints   = @($MissingEndpoints | Select-Object -Unique)
    operator_notes      = $OperatorNotesValue
    artifact_hashes     = $ArtifactHashes
}

# market_date is read verbatim from current_daily_operation's own operation
# row when present, never invented or derived independently by this script.
if ($null -ne $CurrentDailyOperation -and
    $null -ne $CurrentDailyOperation.PSObject.Properties['operation'] -and
    $null -ne $CurrentDailyOperation.operation -and
    $null -ne $CurrentDailyOperation.operation.PSObject.Properties['market_date']) {
    $Manifest.market_date = [string]$CurrentDailyOperation.operation.market_date
}

# ---------------------------------------------------------------------------
# Write manifest -- only inside the explicit output directory.
# ---------------------------------------------------------------------------
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$ManifestPath = Join-Path $OutputDirectory 'autonomous_paper_session_manifest.json'
$Manifest | ConvertTo-Json -Depth 30 | Set-Content -Path $ManifestPath -Encoding UTF8

Write-Host "Manifest written: $ManifestPath" -ForegroundColor Green
if (@($CaptureErrors).Count -gt 0) {
    Write-Host "Capture errors ($(@($CaptureErrors).Count)):" -ForegroundColor Yellow
    foreach ($e in $CaptureErrors) { Write-Host "  - $e" -ForegroundColor Yellow }
}
Write-Host ""
Write-Host "This is one point-in-time evidence capture, not a completed soak session." -ForegroundColor Cyan
