# =============================================================================
# Start-LiveShadowSmoke.ps1
# LIVE-TINY-CAPITAL-SMOKE-01
#
# Reusable LiveShadow evidence/orchestration wrapper. Targets LiveShadow
# ONLY -- never LiveCapital. Delegates to the canonical launcher's real
# LiveShadow daemon-bootstrap path (Start-MiniQuantDesk.ps1 -Mode LiveShadow,
# MQK-LEDGER-BURN-CONTROLLER-03 A3A) -- this script does not reimplement any
# broker/order/arm/halt logic of its own; it only wraps that entrypoint with
# LiveShadow-specific framing and deterministic evidence capture.
#
# A3B TRUTH REPAIR (MQK-LEDGER-BURN-CONTROLLER-03): independent review of the
# prior version of this script found three defects, all fixed here:
#   1. It delegated to `-Mode Live` (LiveCapital's read-only-today preflight
#      path), which never starts anything and, before A3A, was the only
#      startup surface that existed -- so a "full" LiveShadow smoke was not
#      actually possible. It now delegates to the real `-Mode LiveShadow`.
#   2. The old non-CheckOnly path inherited `-Mode Live`'s interactive
#      `Confirm-LiveIntent` / `Read-Host 'Type LIVE'` prompt, which made
#      `-IAcknowledgeLiveShadowOnly` not a truthful noninteractive full-run
#      switch. `-Mode LiveShadow` never calls `Confirm-LiveIntent` (see
#      Invoke-LiveShadowStartup / the main dispatch's distinct `elseif`
#      branch in Start-MiniQuantDesk.ps1) -- fixed structurally, not by a
#      workaround in this file.
#   3. manifest.json hardcoded real_daemon_start_performed/real_broker_call_
#      performed/real_order_submitted to the literal `$false` regardless of
#      what actually happened, and the guard that validated those values was
#      circular (it validated the script's own constants). This version
#      distinguishes:
#        - WRAPPER STATIC CONTRACT (wrapper_direct_broker_call,
#          wrapper_direct_order_submission): real, provable-by-construction
#          facts about THIS FILE's own source -- it contains no broker/
#          order/arm/halt route literal (see the guard test's LSS05 check) --
#          unconditionally true regardless of CheckOnly or a full run.
#        - OBSERVED RUNTIME EVIDENCE (daemon_started_by_this_invocation,
#          daemon_reachable_and_verified, real_broker_call_performed,
#          real_order_submitted): derived from the canonical launcher's OWN
#          JSON log entry (read from a file it already wrote -- never a new
#          HTTP call from this script, per the hard rule below), using
#          'not_run' when the action category was never attempted
#          (CheckOnly), 'observed_true'/'observed_false' when the launcher's
#          log directly proves the outcome, and 'not_observed' when a full
#          run was attempted but this wrapper has no instrumentation to
#          directly prove that specific fact -- broker-call and
#          order-submission counters are not yet exposed by the launcher's
#          log or any daemon route this script may call, so a full run
#          always reports those two as 'not_observed' rather than
#          fabricating a value. LiveShadow's own no-order-submission
#          contract is a Rust-runtime design invariant (DeploymentMode::
#          LiveShadow), not something this manifest independently proves.
#
# R1 EVIDENCE-PROVENANCE REPAIR (MQK-LEDGER-BURN-CONTROLLER-04): independent
# review of the A3B version above found two further defects, both fixed here:
#   4. $launcherLogBefore captured only a COUNT of pre-existing launcher logs
#      and was never consulted again -- evidence derivation always selected
#      simply the newest launch_*.json under the launcher's log directory. A
#      failed/no-op current invocation could therefore silently consume a
#      stale prior LiveShadow launcher log and attribute its evidence to this
#      run. Fixed (later superseded by R1B below): Resolve-LiveShadowRunEvidence
#      took the exact SET of pre-existing log paths and computed this run's
#      log as the (post minus pre) set difference.
#   5. real_daemon_start_performed was inferred from safety_guard.ok, which
#      only proves this invocation re-verified a reachable, correctly-postured
#      daemon -- not that THIS invocation started it (the canonical launcher
#      can attach to an already-running daemon a prior invocation started).
#      Fixed: the canonical launcher itself now surfaces
#      daemon_started_by_this_invocation (Start-DaemonIfNeeded's own
#      Started/attached fact, via its deterministic stdout) as a field
#      distinct from daemon_reachable_and_verified (the safety-guard fact).
#      This wrapper consumes both, never conflates them.
#
# R1B EXACT-INVOCATION-IDENTITY REPAIR (MQK-LIVESHADOW-R1B-FINAL): independent
# review found that R1's "set difference since pre-run snapshot" only proves
# TEMPORAL novelty (a log that is new since this invocation started), not
# CAUSAL ownership (that log was written by THIS invocation's own child, not
# some concurrent foreign invocation). Deterministic failure case: this
# wrapper's own child produces no evidence, while a concurrent foreign
# LiveShadow invocation writes one otherwise-valid new launch_*.json log --
# the old set-difference resolver accepted that foreign log as if it were
# this run's own evidence (proven by LS-EV-08's pre-fix RED run). Fixed:
#   - This script now generates an opaque GUID before delegating and passes
#     it to Start-MiniQuantDesk.ps1 via the non-secret -InvocationId
#     parameter (never a secret, never printed as sensitive -- it is a random
#     identifier with no meaning outside this evidence-binding purpose).
#   - Start-MiniQuantDesk.ps1 writes that exact value into its own launch
#     JSON's invocation_id field (Invoke-LiveShadowStartup), regardless of
#     CheckOnly/full-run.
#   - Resolve-LiveShadowRunEvidence no longer looks at "new since a pre-run
#     snapshot" at all. It scans every launch_*.json under the launcher log
#     directory and accepts ONLY a log whose invocation_id field exactly
#     equals the GUID this invocation generated. Zero exact matches or more
#     than one exact match both fail closed to 'not_observed', with an
#     explicit reason. A foreign invocation's log -- however new, however
#     otherwise-valid -- is never even a candidate unless its invocation_id
#     happens to equal this run's GUID (practically impossible). "Newest
#     file", timestamps, and new-file-count heuristics are never consulted.
#
# Usage:
#   Start-LiveShadowSmoke.ps1                              (same as -CheckOnly)
#   Start-LiveShadowSmoke.ps1 -CheckOnly
#   Start-LiveShadowSmoke.ps1 -IAcknowledgeLiveShadowOnly   (full run -- real
#                                                             daemon-bootstrap
#                                                             attempt, see
#                                                             notes above)
#
# Parameters:
#   -RepoRoot                   Repo root. Default: two levels up from this script.
#   -CheckOnly                  Read-only preflight only. Default when no other
#                                run switch is passed. Delegates to the canonical
#                                launcher's own already-safe -Mode LiveShadow
#                                -CheckOnly path (Invoke-LiveShadowCheckOnly).
#   -IAcknowledgeLiveShadowOnly Explicit, named acknowledgement required to run
#                                the "full" (non-CheckOnly) path -- a real
#                                LiveShadow daemon-bootstrap attempt. Named for
#                                what it actually does (LiveShadow only, never
#                                LiveCapital), not a generic "-Force"/"-Yes"
#                                flag, so an operator cannot pass it by habit
#                                without reading it.
#
# Exit codes: passthrough of Start-MiniQuantDesk.ps1's own exit code
#   (0=ready, 1=generic failure, 2=safety refusal, 5=LIVE blocked, ...).
#
# Hard rules enforced by this script:
#   - MQK_DAEMON_DEPLOYMENT_MODE is always set to 'live-shadow', never 'live'
#     -- there is exactly one assignment to this variable in the whole file.
#     (Belt-and-suspenders: the canonical launcher's own -DeploymentMode
#     live-shadow parameter is the real authority: this script's own
#     assignment is redundant with, never in conflict with, that seam.)
#   - Never calls Invoke-WebRequest/Invoke-RestMethod/curl directly -- every
#     network-capable action is delegated to Start-MiniQuantDesk.ps1's own
#     process boundary, never duplicated here. Observed-evidence derivation
#     reads a JSON file the canonical launcher already wrote, never a new
#     network call.
#   - Never prints ALPACA_API_KEY*, ALPACA_API_SECRET*, MQK_OPERATOR_TOKEN,
#     DB password, Discord webhook, or any other secret value.
#   - Evidence layout is deterministic: exports\live_shadow_smoke\evidence_<UTC
#     timestamp>\ containing readiness_report.log and manifest.json.
# =============================================================================

[CmdletBinding()]
param(
    [string]$RepoRoot = '',
    [switch]$CheckOnly,
    [switch]$IAcknowledgeLiveShadowOnly
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

function Write-Step    { param([string]$M) Write-Host "[LiveShadowSmoke] $M" -ForegroundColor Cyan }
function Write-Ok      { param([string]$M) Write-Host "[LiveShadowSmoke] OK: $M" -ForegroundColor Green }
function Write-Warn    { param([string]$M) Write-Host "[LiveShadowSmoke] WARN: $M" -ForegroundColor Yellow }
function Write-Fail    { param([string]$M) Write-Host "[LiveShadowSmoke] FAIL: $M" -ForegroundColor Red }
function Write-Section { param([string]$M) Write-Host ''; Write-Host "=== $M ===" -ForegroundColor Magenta }

# ---------------------------------------------------------------------------
# Secret guard: never print these names' values (same convention as
# Start-PaperTradingSmoke.ps1's $SECRET_NAMES).
# ---------------------------------------------------------------------------
$SECRET_NAMES = @(
    'ALPACA_API_KEY_PAPER', 'ALPACA_API_SECRET_PAPER',
    'ALPACA_API_KEY_LIVE',  'ALPACA_API_SECRET_LIVE',
    'MQK_OPERATOR_TOKEN',   'DISCORD_WEBHOOK_URL',
    'POSTGRES_PASSWORD',    'DATABASE_URL', 'MQK_DATABASE_URL'
)

function Assert-NotSecret {
    param([string]$Name, [string]$Value)
    foreach ($s in $SECRET_NAMES) {
        if ($Name -eq $s -and -not [string]::IsNullOrWhiteSpace($Value)) {
            throw "BUG: script attempted to print secret env var '$Name'. Aborting."
        }
    }
}

# ---------------------------------------------------------------------------
# R1B (MQK-LIVESHADOW-R1B-FINAL): pure, dot-sourceable evidence-resolution
# function -- factored out so tests\script_guards\test_live_shadow_smoke.ps1
# can prove exact-invocation-identity binding with hermetic fixture JSON
# files, without spawning a real daemon. Never called for the CheckOnly path
# (that stays the unconditional 'not_run' evidence below -- CheckOnly
# performs no daemon stage at all, so there is no log to resolve).
#
# -ExpectedInvocationId is the opaque GUID this wrapper generated for its own
# invocation and passed to Start-MiniQuantDesk.ps1 via -InvocationId. "This
# run's own log" is defined ONLY as: a launch_*.json under -LauncherLogDir
# whose own invocation_id field is an EXACT string match for that GUID.
# Every other log -- regardless of when it was written, how new it is, or
# whether it is otherwise a perfectly valid live-shadow log -- is a foreign
# log and is never a candidate. Zero exact matches or more than one exact
# match both fail closed to 'not_observed' with an explicit reason. Neither
# file timestamps, "newest file", nor new-file counts are ever consulted.
#
# MQK-LIVESHADOW-PROVENANCE-FINAL-01 (R1C):
#   - LS-PROV-08 (closed evidence vocabulary): the two daemon-evidence
#     fields read from the matched log are validated against a closed set
#     (observed_true/observed_false/not_observed) before being trusted. An
#     unrecognized type or value (a JSON bool, a free-text string like
#     "yes", a number, an array/object, ...) never becomes authoritative --
#     it fails closed to this function's own 'not_observed' default, with an
#     explicit reason, exactly like a missing field.
#   - Read consistency: the exact-match scan below parses each candidate
#     file once and keeps that parsed object keyed by path. The selected
#     log's evidence is read from that SAME parsed object, never by
#     re-opening the file a second time -- closing a TOCTOU window where the
#     file content used for identity validation could differ from the
#     content used for evidence consumption.
# ---------------------------------------------------------------------------
function Test-LiveShadowEvidenceValue {
    param($Value)
    if ($null -eq $Value) { return $false }
    if ($Value -isnot [string]) { return $false }
    return @('observed_true', 'observed_false', 'not_observed') -contains $Value
}

function Resolve-LiveShadowRunEvidence {
    param(
        [Parameter(Mandatory = $true)][string]$LauncherLogDir,
        [Parameter(Mandatory = $true)][string]$ExpectedInvocationId
    )

    $result = [ordered]@{
        daemon_started_by_this_invocation = 'not_observed'
        daemon_reachable_and_verified     = 'not_observed'
        source_log                        = $null
        reason                            = $null
    }

    if ([string]::IsNullOrWhiteSpace($ExpectedInvocationId)) {
        $result.reason = 'no expected invocation id was supplied -- refusing to resolve evidence'
        return [pscustomobject]$result
    }

    if (-not (Test-Path $LauncherLogDir)) {
        $result.reason = 'launcher log directory does not exist'
        return [pscustomobject]$result
    }

    $candidatePaths = @(Get-ChildItem -Path $LauncherLogDir -Filter 'launch_*.json' -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty FullName)

    $exactMatches = @()
    $parsedByPath = @{}
    foreach ($p in $candidatePaths) {
        $entry = $null
        try { $entry = Get-Content -Path $p -Raw | ConvertFrom-Json } catch { continue }
        if ($null -eq $entry) { continue }
        if ($entry.mode -ne 'live-shadow') { continue }
        $idProp = $entry.PSObject.Properties['invocation_id']
        if ($null -eq $idProp) { continue }
        if ([string]::IsNullOrWhiteSpace([string]$entry.invocation_id)) { continue }
        if ([string]$entry.invocation_id -eq $ExpectedInvocationId) {
            $exactMatches += $p
            $parsedByPath[$p] = $entry
        }
    }

    if ($exactMatches.Count -eq 0) {
        $result.reason = "no launcher log with invocation_id=$ExpectedInvocationId found -- foreign/unrelated logs are never accepted as this invocation's evidence"
        return [pscustomobject]$result
    }
    if ($exactMatches.Count -gt 1) {
        $result.reason = "ambiguous: $($exactMatches.Count) launcher logs claim invocation_id=$ExpectedInvocationId; refusing to guess which is authoritative"
        return [pscustomobject]$result
    }

    $sourceLog = $exactMatches[0]
    $result.source_log = $sourceLog

    # Read-consistency: reuse the exact same parsed object captured during
    # the scan above -- never a second Get-Content of $sourceLog.
    $entry = $parsedByPath[$sourceLog]

    $malformedFields = @()

    $startedProp = $entry.PSObject.Properties['daemon_started_by_this_invocation']
    if ($null -ne $startedProp) {
        if (Test-LiveShadowEvidenceValue -Value $entry.daemon_started_by_this_invocation) {
            $result.daemon_started_by_this_invocation = $entry.daemon_started_by_this_invocation
        } else {
            $malformedFields += 'daemon_started_by_this_invocation'
        }
    }

    $verifiedProp = $entry.PSObject.Properties['daemon_reachable_and_verified']
    if ($null -ne $verifiedProp) {
        if (Test-LiveShadowEvidenceValue -Value $entry.daemon_reachable_and_verified) {
            $result.daemon_reachable_and_verified = $entry.daemon_reachable_and_verified
        } else {
            $malformedFields += 'daemon_reachable_and_verified'
        }
    }

    if ($malformedFields.Count -gt 0) {
        $result.reason = "malformed/unrecognized evidence value(s) in ${sourceLog}: $($malformedFields -join ', ') -- failing closed to not_observed for those fields"
    }

    return [pscustomobject]$result
}

# MQK-LIVESHADOW-PROVENANCE-FINAL-01 (LS-PROV-04): pure, dot-sourceable
# evidence-directory path construction -- factored out so
# tests\script_guards\test_live_shadow_smoke.ps1 can prove same-timestamp-
# bucket collision-freedom against the REAL production seam (LS-EV-13), not
# a duplicated copy of the algorithm. -InvocationId is this wrapper's own
# always-freshly-generated per-invocation GUID (see the call site below,
# generated BEFORE this path is computed) -- never a caller-reused value at
# this layer, so two same-second wrapper invocations always resolve to
# different evidence directories and therefore never share
# readiness_report.log/manifest.json.
function Get-LiveShadowEvidenceDirPath {
    param(
        [Parameter(Mandatory = $true)][string]$RepoRoot,
        [Parameter(Mandatory = $true)][string]$InvocationId
    )
    $evStamp = [DateTime]::UtcNow.ToString('yyyyMMdd_HHmmss')
    return (Join-Path $RepoRoot "exports\live_shadow_smoke\evidence_${evStamp}_${InvocationId}")
}

# ---------------------------------------------------------------------------
# Resolve repo root
# ---------------------------------------------------------------------------
if ($MyInvocation.InvocationName -eq '.') { return }

if ([string]::IsNullOrWhiteSpace($RepoRoot)) {
    $RepoRoot = (Resolve-Path (Join-Path $PSScriptRoot '..\..')).Path
}
$RepoRoot = $RepoRoot.TrimEnd('\')
Write-Step "Repo root: $RepoRoot"

$launcherPath = Join-Path $RepoRoot 'scripts\windows\Start-MiniQuantDesk.ps1'
if (-not (Test-Path $launcherPath)) {
    Write-Fail "Canonical launcher not found at $launcherPath. This script only wraps it -- it does not reimplement live startup."
    exit 1
}

# ---------------------------------------------------------------------------
# Effective run mode: CheckOnly unless the full-run path is explicitly
# acknowledged. Passing neither switch defaults to the safe CheckOnly path
# -- this script never silently attempts a "full" run.
# ---------------------------------------------------------------------------
$effectiveCheckOnly = $CheckOnly.IsPresent -or (-not $IAcknowledgeLiveShadowOnly.IsPresent)
if (-not $effectiveCheckOnly) {
    Write-Warn "Full-run path acknowledged (-IAcknowledgeLiveShadowOnly). This delegates to"
    Write-Warn "Start-MiniQuantDesk.ps1 -Mode LiveShadow, which performs a REAL daemon-bootstrap"
    Write-Warn "attempt against real broker connectivity (ALPACA_API_KEY_LIVE) -- no order is ever"
    Write-Warn "submitted (LiveShadow deployment-mode invariant), no arm, no runtime auto-start."
}

# ---------------------------------------------------------------------------
# Hard guard: force LiveShadow, never LiveCapital. This is the ONE and ONLY
# assignment to MQK_DAEMON_DEPLOYMENT_MODE in this file.
# ---------------------------------------------------------------------------
$env:MQK_DAEMON_DEPLOYMENT_MODE = 'live-shadow'
Write-Ok "MQK_DAEMON_DEPLOYMENT_MODE forced to 'live-shadow' (LiveCapital is never enabled by this script)."

# ---------------------------------------------------------------------------
# R1B: opaque, non-secret invocation identity for THIS wrapper invocation.
# Passed to the canonical launcher via -InvocationId; it writes this exact
# value into its own launch_*.json log entry. Evidence is later bound to
# this GUID, never to "the newest new log file" (see this file's header).
#
# MQK-LIVESHADOW-PROVENANCE-FINAL-01 (LS-PROV-04): generated BEFORE the
# evidence directory below is chosen, and folded into that directory name --
# a second-resolution timestamp alone let two same-second wrapper
# invocations share one evidence_<timestamp> directory (and therefore one
# readiness_report.log / manifest.json), proven pre-fix by LS-EV-13's RED
# run. This is the wrapper's own always-freshly-generated identity (never a
# caller-reused value at this layer), so folding it into the directory name
# does not weaken the resolver's duplicate/reused-invocation_id-must-be-
# ambiguous contract at the launcher-log layer.
# ---------------------------------------------------------------------------
$invocationId = [guid]::NewGuid().ToString()

# ---------------------------------------------------------------------------
# Deterministic evidence layout.
# ---------------------------------------------------------------------------
$evDir = Get-LiveShadowEvidenceDirPath -RepoRoot $RepoRoot -InvocationId $invocationId
New-Item -ItemType Directory -Force -Path $evDir | Out-Null
$reportLog = Join-Path $evDir 'readiness_report.log'
$manifestPath = Join-Path $evDir 'manifest.json'
Write-Ok "Evidence folder: $evDir"

# ---------------------------------------------------------------------------
# Delegate to the canonical launcher's real LiveShadow daemon-bootstrap path
# (A3A). Never construct HTTP requests here -- every network-capable action
# lives entirely inside Start-MiniQuantDesk.ps1's own process.
# ---------------------------------------------------------------------------
Write-Section "Delegating to Start-MiniQuantDesk.ps1 -Mode LiveShadow$(if ($effectiveCheckOnly) { ' -CheckOnly' } else { '' })"

$launcherArgs = @('-Mode', 'LiveShadow', '-InvocationId', $invocationId)
if ($effectiveCheckOnly) { $launcherArgs += '-CheckOnly' }

$launcherLogDir = Join-Path $RepoRoot 'smoke_logs\launcher\live-shadow'

$launcherExitCode = 0
try {
    & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $launcherPath @launcherArgs 2>&1 |
        Tee-Object -FilePath $reportLog | Out-Host
    $launcherExitCode = $LASTEXITCODE
} catch {
    "LAUNCHER_INVOCATION_ERROR: $($_.Exception.Message)" | Add-Content -Path $reportLog
    Write-Fail "Failed to invoke canonical launcher: $($_.Exception.Message)"
    $launcherExitCode = 1
}

# ---------------------------------------------------------------------------
# R1B: derive OBSERVED runtime evidence from EXACTLY the launcher log whose
# invocation_id matches this wrapper's own GUID (Resolve-LiveShadowRunEvidence
# above) -- never a new network call from this script, and never "the newest
# file" or "the only new file since a snapshot" (see this file's header).
# daemon_started_by_this_invocation and daemon_reachable_and_verified are two
# distinct facts the canonical launcher itself records -- an attach to an
# already-running daemon reports started=observed_false,
# reachable_and_verified=observed_true, never a fabricated "real start".
# ---------------------------------------------------------------------------
$observedDaemonStarted = 'not_run'
$observedDaemonVerified = 'not_run'
$observedBrokerCall = 'not_run'
$observedOrderSubmitted = 'not_run'

if (-not $effectiveCheckOnly) {
    $observedBrokerCall = 'not_observed'
    $observedOrderSubmitted = 'not_observed'

    $evidence = Resolve-LiveShadowRunEvidence -LauncherLogDir $launcherLogDir -ExpectedInvocationId $invocationId
    $observedDaemonStarted = $evidence.daemon_started_by_this_invocation
    $observedDaemonVerified = $evidence.daemon_reachable_and_verified
    if ($null -ne $evidence.reason) {
        Write-Warn "Evidence provenance: $($evidence.reason)"
    }
    # real_broker_call_performed / real_order_submitted: neither the
    # launcher's log nor any route this script may call currently exposes a
    # broker-call or order-submission counter. Honest 'not_observed' rather
    # than a fabricated value -- see this file's header for why.
}

$manifest = [ordered]@{
    schema_version         = 'live-shadow-smoke-manifest-v4'
    checked_at_utc         = [DateTime]::UtcNow.ToString('o')
    deployment_mode_forced = 'live-shadow'
    check_only             = $effectiveCheckOnly
    canonical_launcher     = 'scripts\windows\Start-MiniQuantDesk.ps1'
    canonical_launcher_mode = 'LiveShadow'
    invocation_id          = $invocationId
    launcher_args          = $launcherArgs
    launcher_exit_code     = $launcherExitCode
    # WRAPPER STATIC CONTRACT: provable facts about THIS FILE's own source,
    # unconditionally true regardless of CheckOnly/full-run (see LSS04/LSS05
    # in tests\script_guards\test_live_shadow_smoke.ps1). Never a claim about
    # what the daemon it delegates to actually did.
    wrapper_direct_broker_call      = $false
    wrapper_direct_order_submission = $false
    # OBSERVED RUNTIME EVIDENCE: 'not_run' (this action category was never
    # attempted -- CheckOnly), 'observed_true'/'observed_false' (this run's
    # own launcher log directly proves the outcome, bound by exact
    # invocation_id match -- R1B), or 'not_observed' (a full run was
    # attempted but no exact single invocation_id match could be resolved,
    # or the log has no instrumentation for this specific fact yet).
    # daemon_started_by_this_invocation and daemon_reachable_and_verified are
    # deliberately distinct fields -- an attach to an already-running daemon
    # is started=observed_false, reachable_and_verified=observed_true.
    daemon_started_by_this_invocation = $observedDaemonStarted
    daemon_reachable_and_verified     = $observedDaemonVerified
    real_broker_call_performed        = $observedBrokerCall
    real_order_submitted              = $observedOrderSubmitted
    note = 'wrapper_* fields are provable-by-construction facts about this script''s own source. daemon_*/real_* fields are observed runtime evidence derived ONLY from the launcher JSON log whose invocation_id field exactly equals this invocation''s own GUID (never a value this script invents, never "the newest file", never a foreign invocation''s log) -- see this file''s header for the full truth-repair rationale (MQK-LEDGER-BURN-CONTROLLER-03 A3B, MQK-LEDGER-BURN-CONTROLLER-04 R1, MQK-LIVESHADOW-R1B-FINAL R1B).'
}
$manifest | ConvertTo-Json -Depth 5 | Set-Content -Path $manifestPath -Encoding ASCII
Write-Ok "Manifest written: $manifestPath"

if ($launcherExitCode -eq 0) {
    Write-Ok "Canonical launcher reported readiness (exit 0)."
} else {
    Write-Warn "Canonical launcher exited $launcherExitCode -- see $reportLog for the reason."
}

exit $launcherExitCode
