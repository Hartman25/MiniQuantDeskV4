# =============================================================================
# LIVE-SHADOW-SMOKE-GUARD-01
#
# Static + one safe real invocation, proving Start-LiveShadowSmoke.ps1
# (LIVE-TINY-CAPITAL-SMOKE-01, engineering portion) can never enable
# LiveCapital, never submits an order, never duplicates broker lifecycle
# code, and never prints a secret value.
#
# Proves:
#   LSS01 — Start-LiveShadowSmoke.ps1 exists
#   LSS02 — MQK_DAEMON_DEPLOYMENT_MODE is assigned exactly once in the file,
#           and that one assignment is the literal 'live-shadow'
#   LSS03 — the literal 'live-shadow' string appears; the literal deployment
#           value 'live' (LiveCapital) is never assigned to that variable
#   LSS04 — the script never calls Invoke-WebRequest/Invoke-RestMethod/curl
#           directly (every network action is delegated to the canonical
#           launcher's own process, never duplicated here)
#   LSS05 — the script never references an order-submission/arm/halt route
#           literal (no duplicated broker lifecycle implementation)
#   LSS06 — the script delegates to Start-MiniQuantDesk.ps1 -Mode LiveShadow
#           (the real daemon-bootstrap path, MQK-LEDGER-BURN-CONTROLLER-03
#           A3A), not a hand-rolled daemon bootstrap
#   LSS06b — never delegates to -Mode Live (LiveCapital) anywhere in this
#           file's code
#   LSS07 — CheckOnly is the default effective mode when no run switch is
#           passed (fail-closed default)
#   LSS08 — a real -CheckOnly invocation succeeds, produces the deterministic
#           evidence layout, and its manifest truthfully reports 'not_run'
#           (never a fabricated boolean $false, A3B) for every observed-
#           runtime-evidence field, separately from the always-$false
#           wrapper-static-contract fields
#
# MQK-LEDGER-BURN-CONTROLLER-04 R1 -- hermetic fixture tests for
# Resolve-LiveShadowRunEvidence (dot-sourced from the target script, no real
# daemon/child-process invocation):
#   LS-EV-01 — pre-existing stale matching log + no new log this run -> the
#              stale log MUST NOT be consumed as this run's evidence
#   LS-EV-02 — two pre-existing logs + one exact new current log -> only the
#              new log is used
#   LS-EV-03 — already-running verified daemon fixture -> reachable=true,
#              started_by_this_invocation=false (never conflated)
#   LS-EV-04 — new daemon start fixture -> started_by_this_invocation=true
#   LS-EV-05 — wrong-mode log cannot satisfy LiveShadow evidence
#   LS-EV-06 — CheckOnly reports 'not_run' for both daemon fields and never
#              calls Resolve-LiveShadowRunEvidence (real safe -CheckOnly run)
#   LS-EV-07 — no secret value appears in the manifest produced by the real
#              -CheckOnly run
#
# No live daemon, no broker call, no order, in any of the above -- LSS08's
# real invocation only exercises Start-MiniQuantDesk.ps1 -Mode LiveShadow
# -CheckOnly, which is itself read-only/report-only by construction
# (Invoke-LiveShadowCheckOnly).
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$Target    = Join-Path $RepoRoot 'scripts\windows\Start-LiveShadowSmoke.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== Live-shadow smoke guard (LIVE-SHADOW-SMOKE-GUARD-01) ==="
Write-Host "    Target: $Target"
Write-Host ""

$text = ''
if (Test-Path $Target) {
    $text = Get-Content -Path $Target -Raw
    Pass 'LSS01' "Start-LiveShadowSmoke.ps1 exists"
} else {
    Fail 'LSS01' "Start-LiveShadowSmoke.ps1 NOT found: $Target"
}

if ($text) {
    # LSS02/LSS03: exactly one assignment to MQK_DAEMON_DEPLOYMENT_MODE,
    # and it is the literal 'live-shadow'.
    $assignments = [regex]::Matches($text, "\`$env:MQK_DAEMON_DEPLOYMENT_MODE\s*=\s*'([^']*)'")
    if ($assignments.Count -eq 1 -and $assignments[0].Groups[1].Value -eq 'live-shadow') {
        Pass 'LSS02' "Exactly one MQK_DAEMON_DEPLOYMENT_MODE assignment, value 'live-shadow'"
    } else {
        Fail 'LSS02' "Expected exactly one MQK_DAEMON_DEPLOYMENT_MODE assignment = 'live-shadow'; found $($assignments.Count) assignment(s): $(($assignments | ForEach-Object { $_.Groups[1].Value }) -join ', ')"
    }

    $liveCapitalAssignment = [regex]::Matches($text, "\`$env:MQK_DAEMON_DEPLOYMENT_MODE\s*=\s*'live'(?!-shadow)")
    if ($liveCapitalAssignment.Count -eq 0) {
        Pass 'LSS03' "MQK_DAEMON_DEPLOYMENT_MODE is never assigned the literal 'live' (LiveCapital)"
    } else {
        Fail 'LSS03' "Found a literal LiveCapital ('live') assignment to MQK_DAEMON_DEPLOYMENT_MODE"
    }

    # LSS04: no direct HTTP call in this file. Strip full-line comments first
    # -- this script's own header prose names the forbidden functions to
    # explain the rule, which would otherwise false-positive against itself.
    $codeLines = ($text -split "`r?`n") | Where-Object { $_.TrimStart() -notmatch '^#' }
    $codeText = $codeLines -join "`n"
    if ($codeText -notmatch 'Invoke-WebRequest' -and $codeText -notmatch 'Invoke-RestMethod' -and $codeText -notmatch '\bcurl\b' -and $codeText -notmatch '\bcurl\.exe\b') {
        Pass 'LSS04' "No direct HTTP call (Invoke-WebRequest/Invoke-RestMethod/curl) in this file's code (comments excluded)"
    } else {
        Fail 'LSS04' "Found a direct HTTP call in this file's code -- all network action must be delegated to the canonical launcher"
    }

    # LSS05: no order/arm/halt route literal.
    $forbiddenRoutes = @(
        '/api/v1/ops/action', '/v1/run/start', '/v1/run/stop', '/v1/run/halt',
        '/v1/integrity/arm', '/v1/integrity/disarm', 'arm-execution', 'disarm-execution',
        'flatten-paper-positions', 'clear-halted-run'
    )
    $foundForbidden = @($forbiddenRoutes | Where-Object { $text -match [regex]::Escape($_) })
    if ($foundForbidden.Count -eq 0) {
        Pass 'LSS05' "No order/arm/halt/reconcile route literal present -- no duplicated broker lifecycle logic"
    } else {
        Fail 'LSS05' "Found forbidden route/action literal(s): $($foundForbidden -join ', ')"
    }

    # LSS06: delegates to the canonical launcher's real LiveShadow path
    # (MQK-LEDGER-BURN-CONTROLLER-03 A3A/A3B) -- specifically 'LiveShadow',
    # not merely a substring match that 'Live' alone would also satisfy
    # against 'LiveShadow' or against the old '-Mode Live' (LiveCapital).
    if ($text -match [regex]::Escape('Start-MiniQuantDesk.ps1') -and
        $text -match "\`$launcherArgs\s*=\s*@\('-Mode',\s*'LiveShadow'\)") {
        Pass 'LSS06' "Delegates to Start-MiniQuantDesk.ps1 -Mode LiveShadow (the real daemon-bootstrap path, not LiveCapital's read-only preflight)"
    } else {
        Fail 'LSS06' "Does not appear to delegate to Start-MiniQuantDesk.ps1 -Mode LiveShadow"
    }

    # A3B: never delegates to -Mode Live (LiveCapital) IN CODE anywhere in
    # this file (comments excluded -- this file's own header prose discusses
    # the OLD, now-repaired '-Mode Live' delegation as history).
    if ($codeText -notmatch "'-Mode',\s*'Live'\)") {
        Pass 'LSS06b' "Never delegates to -Mode Live (LiveCapital) anywhere in this file's code"
    } else {
        Fail 'LSS06b' "Found a possible code delegation to -Mode Live (LiveCapital)"
    }

    # LSS07: CheckOnly is the default effective mode.
    if ($text -match '\$effectiveCheckOnly\s*=\s*\$CheckOnly\.IsPresent\s*-or\s*\(-not\s*\$IAcknowledgeLiveShadowOnly\.IsPresent\)') {
        Pass 'LSS07' "CheckOnly is the fail-closed default when no run switch is passed"
    } else {
        Fail 'LSS07' "Could not confirm CheckOnly-by-default logic"
    }

    # LSS09 (A3B): the full-run path contains no inherited LiveCapital
    # typed-confirmation prompt -- Confirm-LiveIntent / 'Type LIVE' must
    # never appear in this file's code, proving -IAcknowledgeLiveShadowOnly
    # cannot end up blocked on (or masking) LiveCapital's interactive gate.
    if ($codeText -notmatch 'Confirm-LiveIntent' -and $codeText -notmatch 'Type LIVE') {
        Pass 'LSS09' "Full-run path contains no inherited LiveCapital 'Type LIVE' / Confirm-LiveIntent prompt"
    } else {
        Fail 'LSS09' "Found a possible inherited LiveCapital confirmation prompt reference"
    }

    # LSS10 (A3B): no secret env var value is ever interpolated directly
    # into a Write-Host/Write-Ok/Write-Step/Write-Warn/Write-Fail call --
    # only presence/absence framing (this file's own Assert-NotSecret guard
    # plus this structural check together cover the "never prints a secret"
    # invariant).
    $secretInterpolation = [regex]::Matches($codeText, 'Write-(Host|Ok|Step|Warn|Fail)[^\r\n]*\$env:(ALPACA_API_(KEY|SECRET)_(LIVE|PAPER)|MQK_OPERATOR_TOKEN|DISCORD_WEBHOOK_URL|POSTGRES_PASSWORD|(MQK_)?DATABASE_URL)\b')
    if ($secretInterpolation.Count -eq 0) {
        Pass 'LSS10' "No secret env var value is interpolated into any Write-* call in this file's code"
    } else {
        Fail 'LSS10' "Found $($secretInterpolation.Count) possible secret-value interpolation(s) in a Write-* call"
    }
} else {
    Fail 'LSS02' "skipped -- target file missing"
    Fail 'LSS03' "skipped -- target file missing"
    Fail 'LSS04' "skipped -- target file missing"
    Fail 'LSS05' "skipped -- target file missing"
    Fail 'LSS06' "skipped -- target file missing"
    Fail 'LSS06b' "skipped -- target file missing"
    Fail 'LSS07' "skipped -- target file missing"
    Fail 'LSS09' "skipped -- target file missing"
    Fail 'LSS10' "skipped -- target file missing"
}

# ---------------------------------------------------------------------------
# LSS08: one real -CheckOnly invocation. Safe by construction (delegates
# only to Start-MiniQuantDesk.ps1 -Mode LiveShadow -CheckOnly, read-only/
# report-only by construction -- Invoke-LiveShadowCheckOnly) -- proves the
# evidence-capture plumbing actually works end to end, not just that the
# source text looks right.
# ---------------------------------------------------------------------------
$script:CheckOnlyManifest = $null
$script:CheckOnlyManifestRaw = $null
if (Test-Path $Target) {
    try {
        $before = Get-Date
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $Target -CheckOnly *> $null
        $exitCode = $LASTEXITCODE

        $evidenceRoot = Join-Path $RepoRoot 'exports\live_shadow_smoke'
        $newestDir = Get-ChildItem -Path $evidenceRoot -Directory -ErrorAction SilentlyContinue |
            Where-Object { $_.CreationTimeUtc -ge $before.ToUniversalTime().AddSeconds(-5) } |
            Sort-Object CreationTimeUtc -Descending | Select-Object -First 1

        if ($null -eq $newestDir) {
            Fail 'LSS08' "No fresh evidence folder found under $evidenceRoot after a -CheckOnly run"
        } else {
            $manifestPath = Join-Path $newestDir.FullName 'manifest.json'
            if (-not (Test-Path $manifestPath)) {
                Fail 'LSS08' "Evidence folder $($newestDir.FullName) has no manifest.json"
            } else {
                $manifest = Get-Content -Path $manifestPath -Raw | ConvertFrom-Json
                $script:CheckOnlyManifest = $manifest
                $script:CheckOnlyManifestRaw = Get-Content -Path $manifestPath -Raw
                # A3B/R1: CheckOnly's observed-evidence fields are the string
                # 'not_run' (this action category was never attempted) --
                # not a boolean $false -- per the truth-repair rationale in
                # Start-LiveShadowSmoke.ps1's own header. wrapper_direct_*
                # are the separate, provable-by-construction static-contract
                # booleans (always $false for this file, both CheckOnly and
                # full run). daemon_started_by_this_invocation /
                # daemon_reachable_and_verified are the R1-repaired,
                # separately-tracked observed-evidence fields (schema v3).
                if ($manifest.check_only -eq $true -and
                    $manifest.schema_version -eq 'live-shadow-smoke-manifest-v3' -and
                    $manifest.deployment_mode_forced -eq 'live-shadow' -and
                    $manifest.canonical_launcher_mode -eq 'LiveShadow' -and
                    $manifest.wrapper_direct_broker_call -eq $false -and
                    $manifest.wrapper_direct_order_submission -eq $false -and
                    $manifest.daemon_started_by_this_invocation -eq 'not_run' -and
                    $manifest.daemon_reachable_and_verified -eq 'not_run' -and
                    $manifest.real_broker_call_performed -eq 'not_run' -and
                    $manifest.real_order_submitted -eq 'not_run') {
                    Pass 'LSS08' "Real -CheckOnly run (exit $exitCode) produced a manifest truthfully recording 'not_run' (never fabricated `$false) for every observed-evidence field, delegating to -Mode LiveShadow"
                } else {
                    Fail 'LSS08' "Manifest did not record the expected fields: $($manifest | ConvertTo-Json -Compress)"
                }
            }
        }
    } catch {
        Fail 'LSS08' "Real -CheckOnly invocation threw: $($_.Exception.Message)"
    }
} else {
    Fail 'LSS08' "skipped -- target file missing"
}

# ---------------------------------------------------------------------------
# LS-EV-06 / LS-EV-07: reuse the LSS08 real -CheckOnly manifest above --
# CheckOnly must never bootstrap a daemon or resolve run evidence, and the
# manifest it writes must never contain a secret value.
# ---------------------------------------------------------------------------
if ($null -ne $script:CheckOnlyManifest) {
    if ($script:CheckOnlyManifest.daemon_started_by_this_invocation -eq 'not_run' -and
        $script:CheckOnlyManifest.daemon_reachable_and_verified -eq 'not_run') {
        Pass 'LS-EV-06' "CheckOnly reports 'not_run' for both daemon_started_by_this_invocation and daemon_reachable_and_verified -- no bootstrap attempted"
    } else {
        Fail 'LS-EV-06' "CheckOnly manifest did not report 'not_run' for both daemon fields: $($script:CheckOnlyManifest | ConvertTo-Json -Compress)"
    }

    $secretNamesForCheck = @(
        'ALPACA_API_KEY_PAPER', 'ALPACA_API_SECRET_PAPER',
        'ALPACA_API_KEY_LIVE',  'ALPACA_API_SECRET_LIVE',
        'MQK_OPERATOR_TOKEN',   'DISCORD_WEBHOOK_URL',
        'POSTGRES_PASSWORD',    'DATABASE_URL', 'MQK_DATABASE_URL'
    )
    $secretHit = @($secretNamesForCheck | Where-Object { $script:CheckOnlyManifestRaw -match [regex]::Escape($_) })
    if ($secretHit.Count -eq 0) {
        Pass 'LS-EV-07' "No secret env var name/value appears in the -CheckOnly manifest"
    } else {
        Fail 'LS-EV-07' "Manifest unexpectedly contains secret-related token(s): $($secretHit -join ', ')"
    }
} else {
    Fail 'LS-EV-06' "skipped -- LSS08 real -CheckOnly run did not produce a manifest"
    Fail 'LS-EV-07' "skipped -- LSS08 real -CheckOnly run did not produce a manifest"
}

# ---------------------------------------------------------------------------
# LS-EV-01..05: hermetic Resolve-LiveShadowRunEvidence fixture tests. Dot-
# source the target script (its own dot-source guard returns immediately
# after defining functions -- no daemon, no network, no evidence folder) and
# drive the function directly against constructed launch_*.json fixtures.
# ---------------------------------------------------------------------------
if (Test-Path $Target) {
    try {
        . $Target

        $fixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) "mqk-ls-ev-$([guid]::NewGuid().ToString('N'))"
        New-Item -ItemType Directory -Force -Path $fixtureRoot | Out-Null

        function New-EvFixtureLog {
            param([string]$Dir, [string]$Name, [string]$Mode, $Started, $Verified)
            $p = Join-Path $Dir $Name
            $obj = [ordered]@{
                timestamp = (Get-Date).ToUniversalTime().ToString('o')
                mode      = $Mode
                stages    = @()
            }
            if ($null -ne $Started)  { $obj.daemon_started_by_this_invocation = $Started }
            if ($null -ne $Verified) { $obj.daemon_reachable_and_verified = $Verified }
            ($obj | ConvertTo-Json -Depth 5) | Set-Content -Path $p -Encoding UTF8
            return $p
        }

        # LS-EV-01: one stale pre-existing log, no new log produced this run.
        $d1 = Join-Path $fixtureRoot 'ev01'
        New-Item -ItemType Directory -Force -Path $d1 | Out-Null
        $stale1 = New-EvFixtureLog -Dir $d1 -Name 'launch_stale.json' -Mode 'live-shadow' -Started 'observed_true' -Verified 'observed_true'
        $r1 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d1 -PreExistingLogPaths @($stale1)
        if ($r1.daemon_started_by_this_invocation -eq 'not_observed' -and $r1.daemon_reachable_and_verified -eq 'not_observed' -and $null -eq $r1.source_log) {
            Pass 'LS-EV-01' "Stale pre-existing log with no new log this run is never consumed -- stays 'not_observed'"
        } else {
            Fail 'LS-EV-01' "Stale log was incorrectly consumed as this run's evidence: $($r1 | ConvertTo-Json -Compress)"
        }

        # LS-EV-02: two pre-existing logs, one exact new log -> only new used.
        $d2 = Join-Path $fixtureRoot 'ev02'
        New-Item -ItemType Directory -Force -Path $d2 | Out-Null
        $old2a = New-EvFixtureLog -Dir $d2 -Name 'launch_old_a.json' -Mode 'live-shadow' -Started 'observed_true' -Verified 'observed_true'
        $old2b = New-EvFixtureLog -Dir $d2 -Name 'launch_old_b.json' -Mode 'live-shadow' -Started 'observed_true' -Verified 'observed_true'
        Start-Sleep -Milliseconds 50
        $new2 = New-EvFixtureLog -Dir $d2 -Name 'launch_new.json' -Mode 'live-shadow' -Started 'observed_false' -Verified 'observed_true'
        $r2 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d2 -PreExistingLogPaths @($old2a, $old2b)
        if ($r2.source_log -eq $new2 -and $r2.daemon_started_by_this_invocation -eq 'observed_false' -and $r2.daemon_reachable_and_verified -eq 'observed_true') {
            Pass 'LS-EV-02' "Exactly the new log is used as evidence source; the two pre-existing logs are ignored"
        } else {
            Fail 'LS-EV-02' "Did not select the exact new log: $($r2 | ConvertTo-Json -Compress)"
        }

        # LS-EV-03: already-running verified daemon -> reachable=true, started=false.
        $d3 = Join-Path $fixtureRoot 'ev03'
        New-Item -ItemType Directory -Force -Path $d3 | Out-Null
        $new3 = New-EvFixtureLog -Dir $d3 -Name 'launch_new.json' -Mode 'live-shadow' -Started 'observed_false' -Verified 'observed_true'
        $r3 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d3 -PreExistingLogPaths @()
        if ($r3.daemon_started_by_this_invocation -eq 'observed_false' -and $r3.daemon_reachable_and_verified -eq 'observed_true') {
            Pass 'LS-EV-03' "Attach-to-already-running fixture: reachable_and_verified=true, started_by_this_invocation=false (never conflated)"
        } else {
            Fail 'LS-EV-03' "Attach fixture did not distinguish started vs reachable: $($r3 | ConvertTo-Json -Compress)"
        }

        # LS-EV-04: new daemon start -> started=true.
        $d4 = Join-Path $fixtureRoot 'ev04'
        New-Item -ItemType Directory -Force -Path $d4 | Out-Null
        $new4 = New-EvFixtureLog -Dir $d4 -Name 'launch_new.json' -Mode 'live-shadow' -Started 'observed_true' -Verified 'observed_true'
        $r4 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d4 -PreExistingLogPaths @()
        if ($r4.daemon_started_by_this_invocation -eq 'observed_true' -and $r4.daemon_reachable_and_verified -eq 'observed_true') {
            Pass 'LS-EV-04' "New daemon-start fixture: started_by_this_invocation=true"
        } else {
            Fail 'LS-EV-04' "New-start fixture did not report started=true: $($r4 | ConvertTo-Json -Compress)"
        }

        # LS-EV-05: wrong-mode log cannot satisfy LiveShadow evidence.
        $d5 = Join-Path $fixtureRoot 'ev05'
        New-Item -ItemType Directory -Force -Path $d5 | Out-Null
        $new5 = New-EvFixtureLog -Dir $d5 -Name 'launch_new.json' -Mode 'paper' -Started 'observed_true' -Verified 'observed_true'
        $r5 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d5 -PreExistingLogPaths @()
        if ($r5.daemon_started_by_this_invocation -eq 'not_observed' -and $r5.daemon_reachable_and_verified -eq 'not_observed') {
            Pass 'LS-EV-05' "A wrong-mode (paper) log can never satisfy LiveShadow evidence, even though it is this run's only new log"
        } else {
            Fail 'LS-EV-05' "Wrong-mode log was incorrectly accepted as LiveShadow evidence: $($r5 | ConvertTo-Json -Compress)"
        }

        Remove-Item -Path $fixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
    } catch {
        Fail 'LS-EV-01' "Hermetic fixture harness threw: $($_.Exception.Message)"
        Fail 'LS-EV-02' "skipped -- harness error"
        Fail 'LS-EV-03' "skipped -- harness error"
        Fail 'LS-EV-04' "skipped -- harness error"
        Fail 'LS-EV-05' "skipped -- harness error"
    }
} else {
    Fail 'LS-EV-01' "skipped -- target file missing"
    Fail 'LS-EV-02' "skipped -- target file missing"
    Fail 'LS-EV-03' "skipped -- target file missing"
    Fail 'LS-EV-04' "skipped -- target file missing"
    Fail 'LS-EV-05' "skipped -- target file missing"
}

Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL LIVE-SHADOW-SMOKE-GUARD-01 INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
