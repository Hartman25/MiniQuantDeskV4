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
# BUNDLE-3-FINAL-GUARD-AND-EVIDENCE-INTEGRITY-REPAIR: exact-type safety-
# identity helpers. `-eq` in PowerShell coerces the right-hand operand to
# the left-hand operand's type and compares strings case-insensitively --
# so, before this repair, an `operator_supervised` value of `1` (Int64)
# would coerce `$true` to `1` and pass; a `deployment_mode` of `"PAPER"` or
# `"paper "` would pass a case-insensitive/whitespace-tolerant `-eq`; and a
# `live_routing_enabled` of `"true"` (string) would coerce `$true` to the
# string `"True"` and pass a case-insensitive string compare. These helpers
# require the exact runtime .NET type (never a coerced string/number) AND
# an exact, ordinal (case-sensitive, no normalization) value match.
# -----------------------------------------------------------------------
function Test-ExactStringField {
    param([string]$Label, $Obj, [string]$FieldName, [string]$Expected)
    if ($null -eq $Obj -or $null -eq $Obj.PSObject.Properties[$FieldName]) {
        Fail "$Label.$FieldName property does not exist"
        return
    }
    $Value = $Obj.$FieldName
    if ($null -eq $Value) {
        Fail "$Label.$FieldName is null -- expected exact string '$Expected'"
        return
    }
    if ($Value -isnot [string]) {
        Fail "$Label.$FieldName is not a string (runtime type: $($Value.GetType().FullName)) -- expected exact string '$Expected'"
        return
    }
    if ([string]::Equals($Value, $Expected, [System.StringComparison]::Ordinal)) {
        Pass "$Label.$FieldName is exactly '$Expected'"
    } else {
        Fail "$Label.$FieldName is a string but not exactly '$Expected' (got: '$Value') -- no case/whitespace normalization is applied"
    }
}

function Test-ExactBooleanTrueField {
    param([string]$Label, $Obj, [string]$FieldName)
    if ($null -eq $Obj -or $null -eq $Obj.PSObject.Properties[$FieldName]) {
        Fail "$Label.$FieldName property does not exist"
        return
    }
    $Value = $Obj.$FieldName
    if ($null -eq $Value) {
        Fail "$Label.$FieldName is null -- expected exact boolean true"
        return
    }
    if ($Value -isnot [bool]) {
        Fail "$Label.$FieldName is not System.Boolean (runtime type: $($Value.GetType().FullName)) -- expected exact boolean true"
        return
    }
    if ($Value -eq $true) {
        Pass "$Label.$FieldName is exactly boolean true"
    } else {
        Fail "$Label.$FieldName is System.Boolean but false"
    }
}

# -----------------------------------------------------------------------
# [5] deployment_mode: property exists, value is a string, ordinal value
#     exactly "paper". Rejects 1, "true", "paper " (trailing space),
#     "PAPER" (case), or any other value/type.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5] deployment_mode MUST be the exact string 'paper' (type- and case-exact) ---"
Test-ExactStringField -Label "manifest" -Obj $Manifest -FieldName 'deployment_mode' -Expected 'paper'

# -----------------------------------------------------------------------
# [5b] adapter_id: property exists, value is a string, ordinal value
#      exactly "alpaca". Rejects "alpaca " (trailing space), "ALPACA"
#      (case), or any other value/type.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5b] adapter_id MUST be the exact string 'alpaca' (type- and case-exact) ---"
Test-ExactStringField -Label "manifest" -Obj $Manifest -FieldName 'adapter_id' -Expected 'alpaca'

# -----------------------------------------------------------------------
# [5c] operator_supervised: property exists, value type is exactly
#      System.Boolean, value is true. Rejects 1, "true", or any non-Boolean
#      type even when it would coerce to a truthy value.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [5c] operator_supervised MUST be exactly System.Boolean true (never a coerced 1/'true') ---"
Test-ExactBooleanTrueField -Label "manifest" -Obj $Manifest -FieldName 'operator_supervised'

# -----------------------------------------------------------------------
# [6] live_routing_enabled across system_status and system_preflight: at
#     least one property must exist; every existing value must be exactly
#     System.Boolean; every existing value must be false. Rejects null, 0,
#     "false", "true", true, and missing-everywhere -- the value true
#     remains a safety failure under every circumstance.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [6] live_routing_enabled MUST be observed exactly Boolean false on at least one surface ---"
$LiveRoutingObserved = $false
$LiveRoutingAllExactFalse = $true
foreach ($surfaceInfo in @(
        @{ Name = 'system_status'; Obj = $Manifest.system_status },
        @{ Name = 'system_preflight'; Obj = $Manifest.system_preflight }
    )) {
    $surface = $surfaceInfo.Obj
    if ($null -ne $surface -and $null -ne $surface.PSObject.Properties['live_routing_enabled']) {
        $LiveRoutingObserved = $true
        $val = $surface.live_routing_enabled
        if ($null -eq $val) {
            $LiveRoutingAllExactFalse = $false
            Fail "$($surfaceInfo.Name).live_routing_enabled is null -- expected exact boolean false"
        } elseif ($val -isnot [bool]) {
            $LiveRoutingAllExactFalse = $false
            Fail "$($surfaceInfo.Name).live_routing_enabled is not System.Boolean (runtime type: $($val.GetType().FullName)) -- expected exact boolean false"
        } elseif ($val -ne $false) {
            $LiveRoutingAllExactFalse = $false
            Fail "$($surfaceInfo.Name).live_routing_enabled is boolean true -- this violates the Paper + Alpaca-only safety boundary; true is never a safety pass under any circumstance"
        } else {
            Pass "$($surfaceInfo.Name).live_routing_enabled is exactly boolean false"
        }
    }
}
if (-not $LiveRoutingObserved) {
    Fail "live_routing_enabled was not observed on any required surface (system_status/system_preflight) -- absence is a validation failure, not an acceptable 'unobservable' state"
} elseif ($LiveRoutingAllExactFalse) {
    Pass "live_routing_enabled is observed exactly boolean false on every surface where present, and never observed true or any other type"
}

# -----------------------------------------------------------------------
# [6b] daemon_base_url re-validated at the manifest level -- absolute local
#      http/https URI, no UserInfo, no query, no fragment, no path other
#      than empty or '/'.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [6b] daemon_base_url is a valid local http/https URI with no UserInfo/query/fragment/non-root path ---"
$DaemonUrlValue = $Manifest.daemon_base_url
if ($null -eq $DaemonUrlValue -or [string]$DaemonUrlValue -eq '') {
    Fail "daemon_base_url is missing or empty"
} else {
    $parsedDaemonUrl = $null
    try { $parsedDaemonUrl = [Uri]$DaemonUrlValue } catch { $parsedDaemonUrl = $null }
    if ($null -eq $parsedDaemonUrl -or -not $parsedDaemonUrl.IsAbsoluteUri) {
        Fail "daemon_base_url is not a valid absolute URI"
    } elseif ($parsedDaemonUrl.Scheme -ne 'http' -and $parsedDaemonUrl.Scheme -ne 'https') {
        Fail "daemon_base_url scheme is not http or https"
    } elseif (@('127.0.0.1', 'localhost', '::1') -notcontains $parsedDaemonUrl.Host) {
        Fail "daemon_base_url host is not a local daemon host"
    } elseif ($parsedDaemonUrl.UserInfo -ne '') {
        Fail "daemon_base_url contains embedded UserInfo"
    } elseif ($parsedDaemonUrl.Query -ne '') {
        Fail "daemon_base_url contains a query string"
    } elseif ($parsedDaemonUrl.Fragment -ne '') {
        Fail "daemon_base_url contains a fragment"
    } elseif ($parsedDaemonUrl.AbsolutePath -ne '' -and $parsedDaemonUrl.AbsolutePath -ne '/') {
        Fail "daemon_base_url path is not empty or '/' (got: '$($parsedDaemonUrl.AbsolutePath)')"
    } else {
        Pass "daemon_base_url is a valid local http/https URI with no UserInfo/query/fragment and an empty or '/' path"
    }
}

# -----------------------------------------------------------------------
# [7] Repository commit is present and is a valid 40-hex-character SHA. A
#     nonempty arbitrary string (e.g. a truncated hash or raw Git error
#     text) is not sufficient -- the exact shape is required.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [7] repository_commit matches ^[0-9a-fA-F]{40}\$ ---"
if ($null -ne $Manifest.repository_commit -and
    $Manifest.repository_commit -is [string] -and
    $Manifest.repository_commit -match '^[0-9a-fA-F]{40}$') {
    Pass "repository_commit is a valid 40-hex-character SHA: $($Manifest.repository_commit)"
} else {
    Fail "repository_commit is missing, null, or not a valid 40-hex-character SHA (got: $($Manifest.repository_commit)) -- a nonempty arbitrary string is not sufficient"
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
# [9] BUNDLE-3-FINAL-GUARD-AND-EVIDENCE-INTEGRITY-REPAIR: strict
#     integer-or-null count authority. A valid count is null, or a
#     nonnegative integer represented by an integral .NET numeric type
#     (Byte/Int16/Int32/Int64/UInt16/UInt32/UInt64/SByte). Double/Single/
#     Decimal, NaN/Infinity, a negative integer, or a numeric string are all
#     rejected -- never coerced. A count field missing on any RETAINED row
#     (a row this truth_state requires to carry counts) is a failure; a
#     genuinely absent row (not_found/backend_unavailable) is not evaluated
#     for counts at all. Applied per the frozen API truth-state shape:
#       Single operation:
#         active           -- operation must exist; validate all 3 counts.
#         query_failed     -- operation may be null; when present, validate.
#         not_found/backend_unavailable -- operation must be null.
#       History:
#         active           -- rows must be an array; validate every row.
#         query_failed     -- rows must be an array; validate every
#                              retained partial row (never skipped).
#         backend_unavailable -- rows must be absent or an empty array.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [9] Strict integer-or-null count authority (single operation + history, every retained truth state) ---"

function Test-IntegerOrNullCount {
    param([string]$Label, $Value)
    if ($null -eq $Value) {
        Pass "$Label is null (unavailable, never a fabricated zero)"
        return
    }
    $IntegralTypes = @([byte], [sbyte], [int16], [uint16], [int32], [uint32], [int64], [uint64])
    $isIntegral = $false
    foreach ($t in $IntegralTypes) {
        if ($Value -is $t) { $isIntegral = $true; break }
    }
    if (-not $isIntegral) {
        Fail "$Label is not null and not an integral numeric type (runtime type: $($Value.GetType().FullName), value: $Value) -- Double/Decimal/numeric-string values are never accepted as a count"
        return
    }
    if ($Value -lt 0) {
        Fail "$Label is a negative integer ($Value) -- counts must be nonnegative"
        return
    }
    Pass "$Label is a nonnegative integer: $Value"
}

function Test-CountFieldsOnRow {
    param([string]$RowLabel, $Row)
    foreach ($countField in @('strategy_evaluation_count', 'order_activity_count', 'fill_count')) {
        if ($null -ne $Row.PSObject.Properties[$countField]) {
            Test-IntegerOrNullCount -Label "$RowLabel.$countField" -Value $Row.$countField
        } else {
            Fail "$RowLabel.$countField is missing on a retained row -- a required count field must never be silently absent"
        }
    }
}

$op = $Manifest.current_daily_operation
if ($null -ne $op -and $null -ne $op.PSObject.Properties['truth_state']) {
    $opTruthState = $op.truth_state
    $opHasOperation = ($null -ne $op.PSObject.Properties['operation'] -and $null -ne $op.operation)
    if ($opTruthState -eq 'active') {
        if ($opHasOperation) {
            Test-CountFieldsOnRow -RowLabel "current_daily_operation.operation (active)" -Row $op.operation
        } else {
            Fail "current_daily_operation.truth_state is 'active' but operation is missing/null -- an active operation row is required"
        }
    } elseif ($opTruthState -eq 'query_failed') {
        if ($opHasOperation) {
            Test-CountFieldsOnRow -RowLabel "current_daily_operation.operation (query_failed, retained)" -Row $op.operation
        } else {
            Pass "current_daily_operation.truth_state is 'query_failed' and operation is null -- permitted; no counts to validate"
        }
    } elseif ($opTruthState -eq 'not_found' -or $opTruthState -eq 'backend_unavailable') {
        if ($opHasOperation) {
            Fail "current_daily_operation.truth_state is '$opTruthState' but operation is non-null -- $opTruthState must never carry a retained operation row"
        } else {
            Pass "current_daily_operation.truth_state is '$opTruthState' and operation is null, as required"
        }
    } else {
        Pass "current_daily_operation.truth_state is '$opTruthState' -- no active/query_failed/not_found/backend_unavailable-specific rule applies; truth_state preserved distinctly, not collapsed"
    }
} else {
    Pass "current_daily_operation not present or has no truth_state -- no counts to validate"
}

$history = $Manifest.recent_daily_operations
if ($null -ne $history -and $null -ne $history.PSObject.Properties['truth_state']) {
    $histTruthState = $history.truth_state
    $histHasRows = ($null -ne $history.PSObject.Properties['rows'] -and $null -ne $history.rows)
    if ($histTruthState -eq 'active' -or $histTruthState -eq 'query_failed') {
        if ($histHasRows) {
            if ($history.rows -isnot [array]) {
                Fail "recent_daily_operations.rows (truth_state '$histTruthState') is not an array (runtime type: $($history.rows.GetType().FullName))"
            } else {
                $rowIndex = 0
                foreach ($row in @($history.rows)) {
                    Test-CountFieldsOnRow -RowLabel "recent_daily_operations.rows[$rowIndex] (truth_state $histTruthState, retained)" -Row $row
                    $rowIndex++
                }
                if (@($history.rows).Count -eq 0) {
                    Pass "recent_daily_operations.rows is an authoritative empty array (truth_state '$histTruthState') -- no rows to validate"
                }
            }
        } else {
            Fail "recent_daily_operations.truth_state is '$histTruthState' but rows is missing/null -- rows must be an array (possibly empty)"
        }
    } elseif ($histTruthState -eq 'backend_unavailable') {
        if (-not $histHasRows -or @($history.rows).Count -eq 0) {
            Pass "recent_daily_operations.truth_state is 'backend_unavailable' and rows is absent or an empty array, as the frozen API shape requires"
        } else {
            Fail "recent_daily_operations.truth_state is 'backend_unavailable' but rows is a nonempty array -- backend_unavailable must never carry retained rows"
        }
    } else {
        Pass "recent_daily_operations.truth_state is '$histTruthState' -- no active/query_failed/backend_unavailable-specific rule applies"
    }
} else {
    Pass "recent_daily_operations not present or has no truth_state -- no counts to validate"
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
    'Authorization:',
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

# -----------------------------------------------------------------------
# [12] BUNDLE-3-FINAL-GUARD-AND-EVIDENCE-INTEGRITY-REPAIR: capture_errors is
#      a bounded array. Each entry may carry only route/error_class/
#      http_status -- no other field, and never a raw string in place of a
#      bounded object (a raw exception/response-body string is exactly what
#      this check exists to reject). http_status must be an integer or
#      null. Raw manifest values are never echoed into a failure message
#      when they might carry sensitive material -- only field names/types
#      are reported.
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [12] capture_errors is a bounded array; each entry carries only route/error_class/http_status ---"
if ($null -eq $Manifest.PSObject.Properties['capture_errors']) {
    Fail "capture_errors field is missing"
} elseif ($null -eq $Manifest.capture_errors) {
    Fail "capture_errors is null -- must be an array (possibly empty)"
} elseif ($Manifest.capture_errors -isnot [array]) {
    Fail "capture_errors is not an array (runtime type: $($Manifest.capture_errors.GetType().FullName))"
} else {
    $CaptureErrorsArray = @($Manifest.capture_errors)
    $AllowedCaptureErrorFields = @('route', 'error_class', 'http_status')
    $errIndex = 0
    foreach ($errEntry in $CaptureErrorsArray) {
        $EntryLabel = "capture_errors[$errIndex]"
        if ($errEntry -is [string]) {
            Fail "$EntryLabel is a raw string, not a bounded object -- raw exception/response text must never be persisted"
        } elseif ($null -eq $errEntry) {
            Fail "$EntryLabel is null -- every retained capture-error entry must be a bounded object"
        } else {
            $ActualFieldNames = @($errEntry.PSObject.Properties.Name)
            $ExtraFields = $ActualFieldNames | Where-Object { $AllowedCaptureErrorFields -notcontains $_ }
            if ($null -ne $ExtraFields -and @($ExtraFields).Count -gt 0) {
                Fail "$EntryLabel has unbounded field(s) beyond route/error_class/http_status: $($ExtraFields -join ', ')"
            } else {
                Pass "$EntryLabel has only bounded field(s) among route/error_class/http_status"
            }
            foreach ($req in @('route', 'error_class')) {
                if ($null -eq $errEntry.PSObject.Properties[$req]) {
                    Fail "$EntryLabel is missing required field '$req'"
                } else {
                    Pass "$EntryLabel has field '$req'"
                }
            }
            if ($null -ne $errEntry.PSObject.Properties['http_status']) {
                $hs = $errEntry.http_status
                if ($null -eq $hs -or $hs -is [int32] -or $hs -is [int64]) {
                    Pass "$EntryLabel.http_status is null or an integer"
                } else {
                    Fail "$EntryLabel.http_status is neither null nor an integer (runtime type: $($hs.GetType().FullName))"
                }
            }
        }
        $errIndex++
    }
    if ($CaptureErrorsArray.Count -eq 0) {
        Pass "capture_errors is an authoritative empty array"
    }
}

# -----------------------------------------------------------------------
# [12b] daemon_base_url path is empty or '/' (re-asserted independently of
#       [6b]'s full URI validation, per the mission's explicit structural
#       requirement).
# -----------------------------------------------------------------------
Write-Host ""
Show-Info "--- [12b] daemon_base_url path is empty or '/' ---"
if ($null -ne $DaemonUrlValue -and [string]$DaemonUrlValue -ne '' -and $null -ne $parsedDaemonUrl -and $parsedDaemonUrl.IsAbsoluteUri) {
    if ($parsedDaemonUrl.AbsolutePath -eq '' -or $parsedDaemonUrl.AbsolutePath -eq '/') {
        Pass "daemon_base_url path is empty or '/'"
    } else {
        Fail "daemon_base_url path is not empty or '/' (got: '$($parsedDaemonUrl.AbsolutePath)')"
    }
} else {
    Fail "daemon_base_url could not be parsed as an absolute URI -- path cannot be verified"
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
