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
#   - Contacts only a strictly validated, freshly-rebuilt local daemon base
#     URL: absolute http/https, host exactly 127.0.0.1/localhost/::1, no
#     UserInfo, no query, no fragment, no path other than empty or '/'
#     (Get-SanitizedDaemonBaseUrlOrExit below). The caller's raw
#     -DaemonBaseUrl string is never persisted or echoed past validation.
#   - Never calls Alpaca or Discord directly, and makes no other external
#     network request.
#   - Never starts or stops a runtime, never arms, disarms, flattens, or
#     finalizes anything -- every route it calls is an existing read-only
#     GET surface.
#   - Never reads or copies .env.local (not referenced anywhere below).
#   - Never prints or persists a credential, API key, secret, or token --
#     this script never touches ALPACA_*, MQK_OPERATOR_TOKEN, or any
#     database-URL environment variable, and the whole built manifest is
#     scanned for secret-shaped patterns before any write (Find-
#     SecretShapedPattern below); a match means zero files are written.
#   - Capture failures are recorded as bounded records only (route,
#     error_class, http_status) -- never raw exception text or response
#     bodies (Get-BoundedErrorClass below).
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
# REPAIR D: Strict daemon base URL validation, fail-closed before anything
# else runs. Requires an absolute http/https URI whose host is exactly
# 127.0.0.1, localhost, or ::1, with no UserInfo, no query, no fragment, and
# no path other than empty or '/'. On success, a single sanitized URL is
# rebuilt from scheme + host + explicit/default port ONLY -- the caller's
# raw -DaemonBaseUrl string is never persisted, echoed, or used again past
# this point (it could itself carry embedded credentials in UserInfo/query).
# Refusal output never echoes the rejected raw value.
# ---------------------------------------------------------------------------
function Get-SanitizedDaemonBaseUrlOrExit {
    param([string]$RawUrl)

    $parsed = $null
    try {
        $parsed = [Uri]$RawUrl
    } catch {
        Write-Host "REFUSED: -DaemonBaseUrl is not a valid URI." -ForegroundColor Red
        exit 1
    }
    if (-not $parsed.IsAbsoluteUri) {
        Write-Host "REFUSED: -DaemonBaseUrl must be an absolute URI." -ForegroundColor Red
        exit 1
    }
    $AllowedSchemes = @('http', 'https')
    if ($AllowedSchemes -notcontains $parsed.Scheme) {
        Write-Host "REFUSED: -DaemonBaseUrl scheme must be exactly http or https." -ForegroundColor Red
        exit 1
    }
    $AllowedHosts = @('127.0.0.1', 'localhost', '::1')
    if ($AllowedHosts -notcontains $parsed.Host) {
        Write-Host "REFUSED: -DaemonBaseUrl host is not a local daemon host (allowed: $($AllowedHosts -join ', '))." -ForegroundColor Red
        Write-Host "This tool never contacts a non-local host -- refusing to proceed." -ForegroundColor Red
        exit 1
    }
    if ($parsed.UserInfo -ne '') {
        Write-Host "REFUSED: -DaemonBaseUrl must not contain embedded user info (username/password/token)." -ForegroundColor Red
        exit 1
    }
    if ($parsed.Query -ne '') {
        Write-Host "REFUSED: -DaemonBaseUrl must not contain a query string." -ForegroundColor Red
        exit 1
    }
    if ($parsed.Fragment -ne '') {
        Write-Host "REFUSED: -DaemonBaseUrl must not contain a fragment." -ForegroundColor Red
        exit 1
    }
    if ($parsed.AbsolutePath -ne '' -and $parsed.AbsolutePath -ne '/') {
        Write-Host "REFUSED: -DaemonBaseUrl must not contain a path other than empty or '/'." -ForegroundColor Red
        exit 1
    }

    $hostPart = if ($parsed.Host -like '*:*') { "[$($parsed.Host)]" } else { $parsed.Host }
    if ($parsed.IsDefaultPort) {
        return "$($parsed.Scheme)://$hostPart"
    } else {
        return "$($parsed.Scheme)://$hostPart`:$($parsed.Port)"
    }
}

$DaemonBaseUrl = Get-SanitizedDaemonBaseUrlOrExit -RawUrl $DaemonBaseUrl

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
# REPAIR F: Bounded capture-error records. Never persist raw PowerShell
# exception text or raw HTTP response bodies -- only bounded fields (route,
# error_class, http_status when available) are ever recorded. Never
# includes: a raw URI with user info, a raw response body, headers, tokens,
# credentials, stack traces, or environment values.
# ---------------------------------------------------------------------------
function Get-BoundedErrorClass {
    param($ErrorRecord)
    $ex = $ErrorRecord.Exception
    $statusCode = $null
    try {
        if ($null -ne $ex -and $null -ne $ex.PSObject.Properties['Response'] -and $null -ne $ex.Response -and
            $null -ne $ex.Response.PSObject.Properties['StatusCode']) {
            $statusCode = [int]$ex.Response.StatusCode
        }
    } catch {
        $statusCode = $null
    }
    if ($null -ne $statusCode) {
        return @{ Class = 'http_status'; Status = $statusCode }
    }
    if ($null -ne $ex -and (
            $ex -is [System.Threading.Tasks.TaskCanceledException] -or
            $ex -is [System.TimeoutException]
        )) {
        return @{ Class = 'timeout'; Status = $null }
    }
    return @{ Class = 'transport'; Status = $null }
}

function New-BoundedErrorRecord {
    param([string]$Route, [string]$ErrorClass, $HttpStatus = $null)
    return [ordered]@{
        route       = $Route
        error_class = $ErrorClass
        http_status = $HttpStatus
    }
}

# ---------------------------------------------------------------------------
# GET-only daemon helper. Never any other HTTP method. Fail-soft: returns
# $null and records a bounded failure record; never throws past this
# function and never persists raw exception text.
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
                $script:CaptureErrors += (New-BoundedErrorRecord -Route $Path -ErrorClass 'fixture_parse_error')
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
        $cls = Get-BoundedErrorClass -ErrorRecord $_
        $script:CaptureErrors += (New-BoundedErrorRecord -Route $Path -ErrorClass $cls.Class -HttpStatus $cls.Status)
        $script:MissingEndpoints += $Path
        return $null
    }
}

# ---------------------------------------------------------------------------
# Capture identity fields
# ---------------------------------------------------------------------------
$CapturedAtUtc = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
$SessionEvidenceId = "$CapturePhase-$((Get-Date).ToUniversalTime().ToString('yyyyMMdd-HHmmss'))"

# ---------------------------------------------------------------------------
# BUNDLE-3-FINAL-GUARD-AND-EVIDENCE-INTEGRITY-REPAIR: repository_commit is
# now captured via `git rev-parse --verify HEAD`, checking $LASTEXITCODE
# explicitly, reading stdout only (stderr is always discarded to $null and
# never merged into stdout, so a failure's Git stderr text is never
# captured, stored, or persisted anywhere). A successful call is accepted
# only when it yields exactly one line matching a 40-hex-character SHA;
# anything else (command failure, malformed/multi-line output) leaves
# repository_commit null and appends one bounded error record -- never a
# raw Git error string.
# ---------------------------------------------------------------------------
$RepositoryCommit = $null
try {
    $CommitOutput = & git -C $RepoRoot rev-parse --verify HEAD 2>$null
    $CommitExitCode = $LASTEXITCODE
    if ($CommitExitCode -eq 0) {
        $CommitCandidates = @($CommitOutput | Where-Object { $_ -ne '' })
        if (@($CommitCandidates).Count -eq 1 -and $CommitCandidates[0] -match '^[0-9a-fA-F]{40}$') {
            $RepositoryCommit = $CommitCandidates[0]
        } else {
            $CaptureErrors += (New-BoundedErrorRecord -Route 'git_rev_parse_head' -ErrorClass 'invalid_commit_shape')
        }
    } else {
        $CaptureErrors += (New-BoundedErrorRecord -Route 'git_rev_parse_head' -ErrorClass 'local_command_failed')
    }
} catch {
    $CaptureErrors += (New-BoundedErrorRecord -Route 'git_rev_parse_head' -ErrorClass 'local_command_failed')
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
    $CaptureErrors += (New-BoundedErrorRecord -Route 'package_json_version_read' -ErrorClass 'local_read_failed')
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
# REPAIR E: Pre-write secret rejection. One shared secret-pattern detector,
# run over the fully-built manifest (which covers OperatorNotes, sanitized
# capture metadata, every serialized captured daemon surface, and capture
# errors in one pass) BEFORE any directory is created or any file is
# written. On a match: zero writes, exit nonzero, and the refusal reports
# only the bounded pattern/category that matched -- never the matching
# value or its surrounding text.
# ---------------------------------------------------------------------------
$SecretPatterns = @(
    'ALPACA_API_KEY', 'ALPACA_API_SECRET', 'ALPACA_SECRET',
    'MQK_OPERATOR_TOKEN', 'MQK_DATABASE_URL',
    'DISCORD_WEBHOOK',
    'Authorization:',
    'Bearer ',
    'password',
    'api_secret',
    '.env.local'
)

function Find-SecretShapedPattern {
    param([string]$Text)
    if ($null -eq $Text) { return $null }
    foreach ($pattern in $SecretPatterns) {
        if ($Text.IndexOf($pattern, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            return $pattern
        }
    }
    return $null
}

$ManifestJson = $Manifest | ConvertTo-Json -Depth 30
$MatchedSecretPattern = Find-SecretShapedPattern -Text $ManifestJson
if ($null -ne $MatchedSecretPattern) {
    Write-Host "REFUSED: captured evidence matches a secret-shaped pattern (category: '$MatchedSecretPattern'). No file was written." -ForegroundColor Red
    exit 1
}

# ---------------------------------------------------------------------------
# Write manifest -- only inside the explicit output directory, only after
# the secret-pattern rejection above has passed.
# ---------------------------------------------------------------------------
New-Item -ItemType Directory -Force -Path $OutputDirectory | Out-Null
$ManifestPath = Join-Path $OutputDirectory 'autonomous_paper_session_manifest.json'
$ManifestJson | Set-Content -Path $ManifestPath -Encoding UTF8

Write-Host "Manifest written: $ManifestPath" -ForegroundColor Green
if (@($CaptureErrors).Count -gt 0) {
    Write-Host "Capture errors ($(@($CaptureErrors).Count)):" -ForegroundColor Yellow
    foreach ($e in $CaptureErrors) { Write-Host "  - route=$($e.route) error_class=$($e.error_class) http_status=$($e.http_status)" -ForegroundColor Yellow }
}
Write-Host ""
Write-Host "This is one point-in-time evidence capture, not a completed soak session." -ForegroundColor Cyan
