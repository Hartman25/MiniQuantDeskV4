# =============================================================================
# validate_autonomous_paper_session_evidence.ps1
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F3-SUPERVISED-SOAK-EVIDENCE-PREPARATION
#
# Validates one autonomous-paper-session evidence manifest (as written by
# capture_autonomous_paper_session_evidence.ps1) against the frozen schema in
# scripts\soak\templates\autonomous_paper_session_manifest.template.json.
#
# Read-only: this script only reads the manifest file(s) given to it. It
# never contacts a daemon, broker, or external service, and never mutates
# the manifest it validates.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\soak\validate_autonomous_paper_session_evidence.ps1 -ManifestPath <path-to-manifest.json>
#
# Exit codes: 0 = valid, 1 = validation failure.
# =============================================================================

[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$ManifestPath
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$Violations = 0
function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan }
function Fail       { param([string]$Msg) $script:Violations++; Show-Red "  FAIL -- $Msg" }
function Pass       { param([string]$Msg) Show-Green "  OK -- $Msg" }

Write-Host "============================================================"
Write-Host " validate_autonomous_paper_session_evidence.ps1"
Write-Host "============================================================"
Write-Host "Manifest: $ManifestPath"

if (-not (Test-Path $ManifestPath)) {
    Fail "manifest file not found: $ManifestPath"
    exit 1
}

$RawContent = Get-Content -Raw -Path $ManifestPath

# -----------------------------------------------------------------------
# [1] Valid JSON.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [1] Manifest is valid JSON ---"
$Manifest = $null
try {
    $Manifest = $RawContent | ConvertFrom-Json -ErrorAction Stop
    Pass "manifest parses as JSON"
} catch {
    Fail "manifest is not valid JSON: $_"
    Write-Host ""
    Write-Host "============================================================"
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}

# -----------------------------------------------------------------------
# [2] Known schema version.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [2] Known schema version ---"
$KnownSchemaVersions = @('autonomous-paper-soak-evidence-v1')
if ($null -ne $Manifest.PSObject.Properties['schema_version'] -and
    $KnownSchemaVersions -contains $Manifest.schema_version) {
    Pass "schema_version is known: $($Manifest.schema_version)"
} else {
    Fail "schema_version missing or unrecognized (expected one of: $($KnownSchemaVersions -join ', '))"
}

# -----------------------------------------------------------------------
# [3] Required top-level fields exist (key presence, not necessarily
#     non-null -- absence of a captured value is legitimate; absence of the
#     KEY itself is a schema violation).
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [3] Required fields exist ---"
$RequiredFields = @(
    'schema_version', 'session_evidence_id', 'capture_phase', 'captured_at_utc',
    'market_date', 'repository_commit', 'deployment_mode', 'adapter_id',
    'daemon_base_url', 'operator_supervised',
    'system_status', 'system_preflight', 'autonomous_readiness',
    'autonomous_paper_status', 'current_daily_operation', 'recent_daily_operations',
    'orders_summary', 'fills_summary', 'reconcile_status', 'risk_posture',
    'completed_bar_task_status', 'gui_build_version',
    'capture_errors', 'missing_endpoints', 'operator_notes', 'artifact_hashes'
)
foreach ($field in $RequiredFields) {
    if ($null -ne $Manifest.PSObject.Properties[$field]) {
        Pass "field present: $field"
    } else {
        Fail "required field missing: $field"
    }
}

# -----------------------------------------------------------------------
# [4] Capture phase is a valid closed value.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [4] Capture phase is valid ---"
$ValidPhases = @('pre_session', 'mid_session', 'post_session', 'incident', 'restart')
if ($null -ne $Manifest.PSObject.Properties['capture_phase'] -and
    $ValidPhases -contains $Manifest.capture_phase) {
    Pass "capture_phase is valid: $($Manifest.capture_phase)"
} else {
    Fail "capture_phase missing or not one of: $($ValidPhases -join ', ')"
}

# -----------------------------------------------------------------------
# [5] Deployment mode is paper.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5] Deployment mode is paper (when known) ---"
if ($null -eq $Manifest.deployment_mode) {
    Pass "deployment_mode is null (unavailable) -- not a violation by itself"
} elseif ($Manifest.deployment_mode -eq 'paper') {
    Pass "deployment_mode is paper"
} else {
    Fail "deployment_mode is '$($Manifest.deployment_mode)', expected 'paper' -- this lane is Paper + Alpaca only"
}

# -----------------------------------------------------------------------
# [6] Live routing is not enabled.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [6] Live routing is not enabled (when observable) ---"
$LiveRoutingEnabled = $null
foreach ($surface in @($Manifest.system_status, $Manifest.system_preflight)) {
    if ($null -ne $surface -and $null -ne $surface.PSObject.Properties['live_routing_enabled']) {
        $LiveRoutingEnabled = $surface.live_routing_enabled
    }
}
if ($null -eq $LiveRoutingEnabled) {
    Pass "live_routing_enabled not observable in this manifest -- not a violation by itself"
} elseif ($LiveRoutingEnabled -eq $false) {
    Pass "live_routing_enabled is false"
} else {
    Fail "live_routing_enabled is true -- this violates the Paper + Alpaca-only safety boundary"
}

# -----------------------------------------------------------------------
# [7] Repository commit present.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [7] Repository commit is present ---"
if ($null -ne $Manifest.repository_commit -and [string]$Manifest.repository_commit -ne '') {
    Pass "repository_commit present: $($Manifest.repository_commit)"
} else {
    Fail "repository_commit is missing or empty"
}

# -----------------------------------------------------------------------
# [8] Truth states preserved: any *_status/*_readiness surface carrying its
#     own truth_state must be one of the recognized closed values, never
#     an ad-hoc or collapsed string.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [8] Truth states are preserved, never collapsed ---"
$KnownTruthStates = @('active', 'not_found', 'backend_unavailable', 'query_failed', 'invalid_request', 'no_db')
$TruthStateBearingSurfaces = @('autonomous_readiness', 'autonomous_paper_status', 'current_daily_operation', 'recent_daily_operations')
foreach ($surfaceName in $TruthStateBearingSurfaces) {
    $surface = $Manifest.$surfaceName
    if ($null -ne $surface -and $null -ne $surface.PSObject.Properties['truth_state']) {
        if ($KnownTruthStates -contains $surface.truth_state) {
            Pass "$surfaceName.truth_state is a known value: $($surface.truth_state)"
        } else {
            Fail "$surfaceName.truth_state is an unrecognized value: $($surface.truth_state)"
        }
    } else {
        Pass "$surfaceName has no truth_state field or is unavailable -- not a violation by itself"
    }
}

# -----------------------------------------------------------------------
# [9] Null counts remain null -- never coerced to zero by this tooling.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [9] Null counts remain null, never coerced to zero ---"
$op = $Manifest.current_daily_operation
if ($null -ne $op -and $null -ne $op.PSObject.Properties['operation'] -and $null -ne $op.operation) {
    foreach ($countField in @('strategy_evaluation_count', 'order_activity_count', 'fill_count')) {
        if ($null -ne $op.operation.PSObject.Properties[$countField]) {
            $val = $op.operation.$countField
            if ($null -eq $val -or ($val -is [int] -or $val -is [long] -or $val -is [double])) {
                Pass "operation.$countField is null or numeric (never a fabricated non-numeric placeholder): $val"
            } else {
                Fail "operation.$countField has an unexpected non-numeric, non-null value: $val"
            }
        }
    }
} else {
    Pass "current_daily_operation.operation not present -- no counts to validate"
}

# -----------------------------------------------------------------------
# [10] Secrets absent -- scan the entire raw manifest text for secret-shaped
#      content. This is a defense-in-depth text scan, not a substitute for
#      the capture script itself never fetching secrets.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [10] No secret-shaped content anywhere in the manifest ---"
$SecretPatterns = @(
    'ALPACA_API_KEY', 'ALPACA_API_SECRET', 'ALPACA_SECRET',
    'MQK_OPERATOR_TOKEN', 'MQK_DATABASE_URL',
    'DISCORD_WEBHOOK',
    'Bearer ',
    'password',
    'api_secret',
    '.env.local'
)
foreach ($pattern in $SecretPatterns) {
    if ($RawContent.IndexOf($pattern, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        Fail "manifest contains a secret-shaped or forbidden pattern: '$pattern'"
    } else {
        Pass "manifest does not contain '$pattern'"
    }
}

# -----------------------------------------------------------------------
# [11] Each required capture is present or explicitly recorded unavailable
#      (missing_endpoints / capture_errors), never silently absent.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [11] Each daemon-sourced capture is present or explicitly recorded unavailable ---"
$DaemonSourcedFields = @{
    'system_status'             = '/api/v1/system/status'
    'system_preflight'          = '/api/v1/system/preflight'
    'autonomous_readiness'      = '/api/v1/autonomous/readiness'
    'autonomous_paper_status'   = '/api/v1/autonomous/paper-status'
    'current_daily_operation'   = '/api/v1/autonomous/daily-operation'
    'recent_daily_operations'   = '/api/v1/autonomous/daily-operations?limit=20'
    'orders_summary'            = '/api/v1/execution/orders'
    'fills_summary'             = '/api/v1/portfolio/fills'
    'reconcile_status'          = '/api/v1/reconcile/status'
    'risk_posture'              = '/api/v1/risk/summary'
    'completed_bar_task_status' = '/api/v1/alerts/active'
}
$MissingEndpointsList = @()
if ($null -ne $Manifest.PSObject.Properties['missing_endpoints']) {
    $MissingEndpointsList = @($Manifest.missing_endpoints)
}
foreach ($fieldName in $DaemonSourcedFields.Keys) {
    $val = $Manifest.$fieldName
    $route = $DaemonSourcedFields[$fieldName]
    if ($null -ne $val) {
        Pass "$fieldName is present"
    } elseif ($MissingEndpointsList -contains $route) {
        Pass "$fieldName is null but explicitly recorded in missing_endpoints ($route)"
    } else {
        Fail "$fieldName is null/absent but not explicitly recorded in missing_endpoints -- silent unavailability is not allowed"
    }
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"
if ($Violations -eq 0) {
    Show-Green " VALID -- manifest satisfies the autonomous-paper-soak-evidence-v1 schema."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
