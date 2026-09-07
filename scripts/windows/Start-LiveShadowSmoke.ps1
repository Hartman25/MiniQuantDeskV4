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
#      run. Fixed: Resolve-LiveShadowRunEvidence takes the exact SET of
#      pre-existing log paths and computes this run's log as the (post minus
#      pre) set difference. Evidence is derived ONLY from that exact new log;
#      zero or more-than-one new logs both fail closed to 'not_observed' with
#      an explicit reason -- "newest file" is never used as evidence identity.
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
# R1 (MQK-LEDGER-BURN-CONTROLLER-04): pure, dot-sourceable evidence-resolution
# function -- factored out so tests\script_guards\test_live_shadow_smoke.ps1
# can prove the stale-log-rejection and started-vs-attached distinction with
# hermetic fixture JSON files, without spawning a real daemon. Never called
# for the CheckOnly path (that stays the unconditional 'not_run' evidence
# below -- CheckOnly performs no daemon stage at all, so there is no log to
# resolve).
#
# -PreExistingLogPaths is the EXACT SET of launch_*.json full paths that
# existed under -LauncherLogDir before this invocation's child launcher ran.
# "This run's own log" is defined ONLY as (current set) minus (that exact
# pre-existing set) -- never "the newest file by timestamp", which a stale
# prior LiveShadow log could satisfy on a failed/no-op current invocation.
# ---------------------------------------------------------------------------
function Resolve-LiveShadowRunEvidence {
    param(
        [Parameter(Mandatory = $true)][string]$LauncherLogDir,
        [Parameter(Mandatory = $true)][AllowEmptyCollection()][string[]]$PreExistingLogPaths
    )

    $result = [ordered]@{
        daemon_started_by_this_invocation = 'not_observed'
        daemon_reachable_and_verified     = 'not_observed'
        source_log                        = $null
        reason                            = $null
    }

    if (-not (Test-Path $LauncherLogDir)) {
        $result.reason = 'launcher log directory does not exist'
        return [pscustomobject]$result
    }

    $currentPaths = @(Get-ChildItem -Path $LauncherLogDir -Filter 'launch_*.json' -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty FullName)
    $preSet = [System.Collections.Generic.HashSet[string]]::new([string[]]$PreExistingLogPaths)
    $newPaths = @($currentPaths | Where-Object { -not $preSet.Contains($_) })

    if ($newPaths.Count -eq 0) {
        $result.reason = 'no new launcher log produced by this invocation (pre-existing logs are never consumed as evidence)'
        return [pscustomobject]$result
    }
    if ($newPaths.Count -gt 1) {
        $result.reason = "ambiguous: $($newPaths.Count) new launcher logs found; refusing to guess which one belongs to this invocation"
        return [pscustomobject]$result
    }

    $sourceLog = $newPaths[0]
    $result.source_log = $sourceLog

    $entry = $null
    try { $entry = Get-Content -Path $sourceLog -Raw | ConvertFrom-Json } catch {}

    if ($null -eq $entry -or $entry.mode -ne 'live-shadow') {
        $result.reason = 'this run''s own launcher log is missing or not mode=live-shadow'
        return [pscustomobject]$result
    }

    if ($null -ne $entry.PSObject.Properties['daemon_started_by_this_invocation']) {
        $result.daemon_started_by_this_invocation = $entry.daemon_started_by_this_invocation
    }
    if ($null -ne $entry.PSObject.Properties['daemon_reachable_and_verified']) {
        $result.daemon_reachable_and_verified = $entry.daemon_reachable_and_verified
    }
    return [pscustomobject]$result
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
# Deterministic evidence layout.
# ---------------------------------------------------------------------------
$evStamp = [DateTime]::UtcNow.ToString('yyyyMMdd_HHmmss')
$evDir = Join-Path $RepoRoot "exports\live_shadow_smoke\evidence_$evStamp"
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

$launcherArgs = @('-Mode', 'LiveShadow')
if ($effectiveCheckOnly) { $launcherArgs += '-CheckOnly' }

$launcherLogDir = Join-Path $RepoRoot 'smoke_logs\launcher\live-shadow'
$launcherLogBefore = if (Test-Path $launcherLogDir) {
    @(Get-ChildItem -Path $launcherLogDir -Filter 'launch_*.json' -ErrorAction SilentlyContinue |
        Select-Object -ExpandProperty FullName)
} else { @() }

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
# R1: derive OBSERVED runtime evidence from EXACTLY this run's own launcher
# log (Resolve-LiveShadowRunEvidence above) -- never a new network call from
# this script, and never "the newest file" (see this file's header, defect
# 4). daemon_started_by_this_invocation and daemon_reachable_and_verified are
# two distinct facts the canonical launcher itself now records (defect 5) --
# an attach to an already-running daemon reports started=observed_false,
# reachable_and_verified=observed_true, never a fabricated "real start".
# ---------------------------------------------------------------------------
$observedDaemonStarted = 'not_run'
$observedDaemonVerified = 'not_run'
$observedBrokerCall = 'not_run'
$observedOrderSubmitted = 'not_run'

if (-not $effectiveCheckOnly) {
    $observedBrokerCall = 'not_observed'
    $observedOrderSubmitted = 'not_observed'

    $evidence = Resolve-LiveShadowRunEvidence -LauncherLogDir $launcherLogDir -PreExistingLogPaths $launcherLogBefore
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
    schema_version         = 'live-shadow-smoke-manifest-v3'
    checked_at_utc         = [DateTime]::UtcNow.ToString('o')
    deployment_mode_forced = 'live-shadow'
    check_only             = $effectiveCheckOnly
    canonical_launcher     = 'scripts\windows\Start-MiniQuantDesk.ps1'
    canonical_launcher_mode = 'LiveShadow'
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
    # own launcher log directly proves the outcome), or 'not_observed' (a
    # full run was attempted but no exact single new log could be resolved,
    # or the log has no instrumentation for this specific fact yet).
    # daemon_started_by_this_invocation and daemon_reachable_and_verified are
    # deliberately distinct fields (R1) -- an attach to an already-running
    # daemon is started=observed_false, reachable_and_verified=observed_true.
    daemon_started_by_this_invocation = $observedDaemonStarted
    daemon_reachable_and_verified     = $observedDaemonVerified
    real_broker_call_performed        = $observedBrokerCall
    real_order_submitted              = $observedOrderSubmitted
    note = 'wrapper_* fields are provable-by-construction facts about this script''s own source. daemon_*/real_* fields are observed runtime evidence derived from EXACTLY this invocation''s own launcher JSON log (never a value this script invents, never "the newest file") -- see this file''s header for the full truth-repair rationale (MQK-LEDGER-BURN-CONTROLLER-03 A3B, MQK-LEDGER-BURN-CONTROLLER-04 R1).'
}
$manifest | ConvertTo-Json -Depth 5 | Set-Content -Path $manifestPath -Encoding ASCII
Write-Ok "Manifest written: $manifestPath"

if ($launcherExitCode -eq 0) {
    Write-Ok "Canonical launcher reported readiness (exit 0)."
} else {
    Write-Warn "Canonical launcher exited $launcherExitCode -- see $reportLog for the reason."
}

exit $launcherExitCode
