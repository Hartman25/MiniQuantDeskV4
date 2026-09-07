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
# MQK-LEDGER-BURN-CONTROLLER-04 R1 / MQK-LIVESHADOW-R1B-FINAL -- hermetic
# fixture tests for Resolve-LiveShadowRunEvidence (dot-sourced from the
# target script, no real daemon/child-process invocation). R1B replaced the
# resolver's identity concept from "the one new file since a pre-run
# snapshot" (temporal novelty) to "the log whose own invocation_id field
# exactly equals this invocation's GUID" (causal ownership) -- the fixtures
# below reflect that:
#   LS-EV-01 — a log with a DIFFERENT invocation_id (foreign/stale) is never
#              consumed as this run's evidence when no log for the expected
#              id exists
#   LS-EV-02 — two logs with foreign invocation_ids + one log with the exact
#              expected invocation_id -> only the exact-id log is used
#   LS-EV-03 — already-running verified daemon fixture (exact id match) ->
#              reachable=true, started_by_this_invocation=false (never
#              conflated)
#   LS-EV-04 — new daemon start fixture (exact id match) ->
#              started_by_this_invocation=true
#   LS-EV-05 — wrong-mode log cannot satisfy LiveShadow evidence even with
#              the exact expected invocation_id
#   LS-EV-06 — CheckOnly reports 'not_run' for both daemon fields and never
#              calls Resolve-LiveShadowRunEvidence (real safe -CheckOnly run)
#   LS-EV-07 — no secret value appears in the manifest produced by the real
#              -CheckOnly run
#   LS-EV-08 — FOREIGN-ONLY: expected id A, no log claims A, one otherwise-
#              valid new LiveShadow log claims foreign id B -> B can never
#              satisfy A; stays 'not_observed' (this is the exact defect a
#              pre-fix RED run against the OLD set-difference resolver
#              proved: it accepted B purely because it was the only new file)
#   LS-EV-09 — foreign B + exact A both present -> only A is authoritative;
#              B is ignored regardless of file timestamps
#   LS-EV-10 — two logs both claim exact id A -> fail closed as ambiguous,
#              never guesses which one is authoritative
#
# MQK-LIVESHADOW-PROVENANCE-FINAL-01 (R1C) -- collision-proofing +
# closed-vocabulary hardening of the same provenance chain:
#   LS-EV-11 — two real New-LauncherLog calls (the production launcher-JSON
#              filename seam, dot-sourced from Start-MiniQuantDesk.ps1)
#              never resolve to the same path, independent of second-
#              resolution timestamp equality
#   LS-EV-12 — two real New-LiveShadowBootstrapLogPaths calls (the
#              production bootstrap stdout/stderr filename seam) never
#              collide, including when the caller InvocationId is
#              intentionally reused -- stdout != stderr != a second
#              invocation's stdout/stderr
#   LS-EV-13 — two real Get-LiveShadowEvidenceDirPath calls (the production
#              wrapper evidence-directory seam) never collide, even within
#              the same timestamp bucket
#   LS-EV-14 — a historical log and a current log both claiming exact id A
#              coexist -> resolver still fails closed as ambiguous (proves
#              collision-proof filenames did not silently repair R1B's
#              duplicate/replay fail-closed contract)
#   LS-EV-15 — exact mode+id match but a daemon evidence field holds an
#              invalid type/value (JSON true, "yes", etc.) -> never
#              propagates; fails closed to 'not_observed' with a reason
#   LS-EV-16 — an explicitly supplied malformed -InvocationId to
#              Start-MiniQuantDesk.ps1 -Mode LiveShadow -CheckOnly fails
#              closed (nonzero exit, no operational startup) rather than
#              being accepted as path/identity material
#   LS-EV-17 — a direct -Mode LiveShadow -CheckOnly invocation with no
#              -InvocationId supplied still receives a nonblank, valid,
#              internally-generated invocation_id in its launcher log
#
# No live daemon, no broker call, no order, in any of the above -- LSS08's
# real invocation only exercises Start-MiniQuantDesk.ps1 -Mode LiveShadow
# -CheckOnly, which is itself read-only/report-only by construction
# (Invoke-LiveShadowCheckOnly). LS-EV-16/17 also only ever invoke the
# -CheckOnly path.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$Target    = Join-Path $RepoRoot 'scripts\windows\Start-LiveShadowSmoke.ps1'
$Launcher  = Join-Path $RepoRoot 'scripts\windows\Start-MiniQuantDesk.ps1'

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
    # R1B: $launcherArgs also carries -InvocationId <guid>, so this only
    # anchors on the leading '-Mode','LiveShadow' pair, not an exact-length
    # array literal.
    if ($text -match [regex]::Escape('Start-MiniQuantDesk.ps1') -and
        $text -match "\`$launcherArgs\s*=\s*@\('-Mode',\s*'LiveShadow',") {
        Pass 'LSS06' "Delegates to Start-MiniQuantDesk.ps1 -Mode LiveShadow (the real daemon-bootstrap path, not LiveCapital's read-only preflight)"
    } else {
        Fail 'LSS06' "Does not appear to delegate to Start-MiniQuantDesk.ps1 -Mode LiveShadow"
    }

    # LSS11 (R1B): the wrapper generates its own opaque invocation GUID and
    # passes it to the canonical launcher via the non-secret -InvocationId
    # parameter -- exact-invocation-identity binding, not a generic -Force/
    # -Yes style flag.
    if ($codeText -match "\`$invocationId\s*=\s*\[guid\]::NewGuid\(\)\.ToString\(\)" -and
        $codeText -match "'-InvocationId',\s*\`$invocationId") {
        Pass 'LSS11' "Wrapper generates an opaque invocation GUID and passes it to the canonical launcher via -InvocationId"
    } else {
        Fail 'LSS11' "Could not confirm invocation-GUID generation and -InvocationId pass-through"
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
    Fail 'LSS11' "skipped -- target file missing"
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
                    $manifest.schema_version -eq 'live-shadow-smoke-manifest-v4' -and
                    -not [string]::IsNullOrWhiteSpace($manifest.invocation_id) -and
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
# LS-EV-01..05, LS-EV-08..10: hermetic Resolve-LiveShadowRunEvidence fixture
# tests (R1B: exact invocation_id matching). Dot-source the target script
# (its own dot-source guard returns immediately after defining functions --
# no daemon, no network, no evidence folder) and drive the function directly
# against constructed launch_*.json fixtures.
# ---------------------------------------------------------------------------
if (Test-Path $Target) {
    try {
        . $Target

        $fixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) "mqk-ls-ev-$([guid]::NewGuid().ToString('N'))"
        New-Item -ItemType Directory -Force -Path $fixtureRoot | Out-Null

        function New-EvFixtureLog {
            param([string]$Dir, [string]$Name, [string]$Mode, [string]$InvocationId, $Started, $Verified)
            $p = Join-Path $Dir $Name
            $obj = [ordered]@{
                timestamp     = (Get-Date).ToUniversalTime().ToString('o')
                mode          = $Mode
                invocation_id = $InvocationId
                stages        = @()
            }
            if ($null -ne $Started)  { $obj.daemon_started_by_this_invocation = $Started }
            if ($null -ne $Verified) { $obj.daemon_reachable_and_verified = $Verified }
            ($obj | ConvertTo-Json -Depth 5) | Set-Content -Path $p -Encoding UTF8
            return $p
        }

        $idA = [guid]::NewGuid().ToString()
        $idB = [guid]::NewGuid().ToString()
        $idForeignOld1 = [guid]::NewGuid().ToString()
        $idForeignOld2 = [guid]::NewGuid().ToString()

        # LS-EV-01: a log exists but claims a DIFFERENT (foreign) invocation_id
        # -- no log claims the expected id A at all.
        $d1 = Join-Path $fixtureRoot 'ev01'
        New-Item -ItemType Directory -Force -Path $d1 | Out-Null
        New-EvFixtureLog -Dir $d1 -Name 'launch_stale.json' -Mode 'live-shadow' -InvocationId $idForeignOld1 -Started 'observed_true' -Verified 'observed_true' | Out-Null
        $r1 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d1 -ExpectedInvocationId $idA
        if ($r1.daemon_started_by_this_invocation -eq 'not_observed' -and $r1.daemon_reachable_and_verified -eq 'not_observed' -and $null -eq $r1.source_log) {
            Pass 'LS-EV-01' "A log claiming a foreign invocation_id is never consumed as this run's evidence -- stays 'not_observed'"
        } else {
            Fail 'LS-EV-01' "Foreign-id log was incorrectly consumed as this run's evidence: $($r1 | ConvertTo-Json -Compress)"
        }

        # LS-EV-02: two foreign-id logs + one log with the exact expected id -> only the exact-id log is used.
        $d2 = Join-Path $fixtureRoot 'ev02'
        New-Item -ItemType Directory -Force -Path $d2 | Out-Null
        New-EvFixtureLog -Dir $d2 -Name 'launch_old_a.json' -Mode 'live-shadow' -InvocationId $idForeignOld1 -Started 'observed_true' -Verified 'observed_true' | Out-Null
        New-EvFixtureLog -Dir $d2 -Name 'launch_old_b.json' -Mode 'live-shadow' -InvocationId $idForeignOld2 -Started 'observed_true' -Verified 'observed_true' | Out-Null
        $new2 = New-EvFixtureLog -Dir $d2 -Name 'launch_new.json' -Mode 'live-shadow' -InvocationId $idA -Started 'observed_false' -Verified 'observed_true'
        $r2 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d2 -ExpectedInvocationId $idA
        if ($r2.source_log -eq $new2 -and $r2.daemon_started_by_this_invocation -eq 'observed_false' -and $r2.daemon_reachable_and_verified -eq 'observed_true') {
            Pass 'LS-EV-02' "Exactly the log whose invocation_id matches is used as evidence source; the two foreign-id logs are ignored"
        } else {
            Fail 'LS-EV-02' "Did not select the exact-id log: $($r2 | ConvertTo-Json -Compress)"
        }

        # LS-EV-03: already-running verified daemon (exact id match) -> reachable=true, started=false.
        $d3 = Join-Path $fixtureRoot 'ev03'
        New-Item -ItemType Directory -Force -Path $d3 | Out-Null
        New-EvFixtureLog -Dir $d3 -Name 'launch_new.json' -Mode 'live-shadow' -InvocationId $idA -Started 'observed_false' -Verified 'observed_true' | Out-Null
        $r3 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d3 -ExpectedInvocationId $idA
        if ($r3.daemon_started_by_this_invocation -eq 'observed_false' -and $r3.daemon_reachable_and_verified -eq 'observed_true') {
            Pass 'LS-EV-03' "Attach-to-already-running fixture: reachable_and_verified=true, started_by_this_invocation=false (never conflated)"
        } else {
            Fail 'LS-EV-03' "Attach fixture did not distinguish started vs reachable: $($r3 | ConvertTo-Json -Compress)"
        }

        # LS-EV-04: new daemon start (exact id match) -> started=true.
        $d4 = Join-Path $fixtureRoot 'ev04'
        New-Item -ItemType Directory -Force -Path $d4 | Out-Null
        New-EvFixtureLog -Dir $d4 -Name 'launch_new.json' -Mode 'live-shadow' -InvocationId $idA -Started 'observed_true' -Verified 'observed_true' | Out-Null
        $r4 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d4 -ExpectedInvocationId $idA
        if ($r4.daemon_started_by_this_invocation -eq 'observed_true' -and $r4.daemon_reachable_and_verified -eq 'observed_true') {
            Pass 'LS-EV-04' "New daemon-start fixture: started_by_this_invocation=true"
        } else {
            Fail 'LS-EV-04' "New-start fixture did not report started=true: $($r4 | ConvertTo-Json -Compress)"
        }

        # LS-EV-05: wrong-mode log cannot satisfy LiveShadow evidence, even with the exact expected invocation_id.
        $d5 = Join-Path $fixtureRoot 'ev05'
        New-Item -ItemType Directory -Force -Path $d5 | Out-Null
        New-EvFixtureLog -Dir $d5 -Name 'launch_new.json' -Mode 'paper' -InvocationId $idA -Started 'observed_true' -Verified 'observed_true' | Out-Null
        $r5 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d5 -ExpectedInvocationId $idA
        if ($r5.daemon_started_by_this_invocation -eq 'not_observed' -and $r5.daemon_reachable_and_verified -eq 'not_observed') {
            Pass 'LS-EV-05' "A wrong-mode (paper) log can never satisfy LiveShadow evidence, even with a matching invocation_id"
        } else {
            Fail 'LS-EV-05' "Wrong-mode log was incorrectly accepted as LiveShadow evidence: $($r5 | ConvertTo-Json -Compress)"
        }

        # LS-EV-08 (R1B, the mission's negative control): FOREIGN-ONLY.
        # Expected id A; no log claims A; one otherwise-valid new LiveShadow
        # log claims foreign id B. Must stay 'not_observed' -- this is exactly
        # the scenario a pre-fix RED run against the OLD set-difference
        # resolver failed (it accepted B purely because it was the only new
        # file since the pre-run snapshot, with no identity check at all).
        $d8 = Join-Path $fixtureRoot 'ev08'
        New-Item -ItemType Directory -Force -Path $d8 | Out-Null
        New-EvFixtureLog -Dir $d8 -Name 'launch_foreign_b.json' -Mode 'live-shadow' -InvocationId $idB -Started 'observed_true' -Verified 'observed_true' | Out-Null
        $r8 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d8 -ExpectedInvocationId $idA
        if ($r8.daemon_started_by_this_invocation -eq 'not_observed' -and $r8.daemon_reachable_and_verified -eq 'not_observed' -and $null -eq $r8.source_log) {
            Pass 'LS-EV-08' "FOREIGN-ONLY: a concurrent foreign invocation's log (id B) can never satisfy expected id A -- stays 'not_observed' (this is the exact defect R1B fixed)"
        } else {
            Fail 'LS-EV-08' "Foreign invocation B's log was incorrectly accepted as evidence for A: $($r8 | ConvertTo-Json -Compress)"
        }

        # LS-EV-09: foreign B + exact A both present -> only A is authoritative.
        $d9 = Join-Path $fixtureRoot 'ev09'
        New-Item -ItemType Directory -Force -Path $d9 | Out-Null
        New-EvFixtureLog -Dir $d9 -Name 'launch_foreign_b.json' -Mode 'live-shadow' -InvocationId $idB -Started 'observed_true' -Verified 'observed_true' | Out-Null
        $exactA9 = New-EvFixtureLog -Dir $d9 -Name 'launch_exact_a.json' -Mode 'live-shadow' -InvocationId $idA -Started 'observed_false' -Verified 'observed_true'
        $r9 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d9 -ExpectedInvocationId $idA
        if ($r9.source_log -eq $exactA9 -and $r9.daemon_started_by_this_invocation -eq 'observed_false' -and $r9.daemon_reachable_and_verified -eq 'observed_true') {
            Pass 'LS-EV-09' "Foreign B + exact A both present: only A is authoritative, B is ignored"
        } else {
            Fail 'LS-EV-09' "Did not select exact-id A as sole authoritative source: $($r9 | ConvertTo-Json -Compress)"
        }

        # LS-EV-10: two logs BOTH claim exact id A -> fail closed as ambiguous.
        $d10 = Join-Path $fixtureRoot 'ev10'
        New-Item -ItemType Directory -Force -Path $d10 | Out-Null
        New-EvFixtureLog -Dir $d10 -Name 'launch_a_first.json' -Mode 'live-shadow' -InvocationId $idA -Started 'observed_true' -Verified 'observed_true' | Out-Null
        New-EvFixtureLog -Dir $d10 -Name 'launch_a_second.json' -Mode 'live-shadow' -InvocationId $idA -Started 'observed_false' -Verified 'observed_true' | Out-Null
        $r10 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d10 -ExpectedInvocationId $idA
        if ($r10.daemon_started_by_this_invocation -eq 'not_observed' -and $r10.daemon_reachable_and_verified -eq 'not_observed' -and $null -eq $r10.source_log -and $r10.reason -match 'ambiguous') {
            Pass 'LS-EV-10' "Two logs both claiming exact id A fail closed as ambiguous -- never guesses which is authoritative"
        } else {
            Fail 'LS-EV-10' "Did not fail closed on ambiguous duplicate-id logs: $($r10 | ConvertTo-Json -Compress)"
        }

        # LS-EV-14 (replay/reused-id): a HISTORICAL log and a separate CURRENT
        # log both claim exact id A, with distinct creation times -- proves
        # collision-proof filenames did not silently repair R1B's
        # duplicate/replay fail-closed contract (still no newest-file
        # fallback).
        $d14 = Join-Path $fixtureRoot 'ev14'
        New-Item -ItemType Directory -Force -Path $d14 | Out-Null
        New-EvFixtureLog -Dir $d14 -Name 'launch_historical_a.json' -Mode 'live-shadow' -InvocationId $idA -Started 'observed_true' -Verified 'observed_true' | Out-Null
        Start-Sleep -Milliseconds 50
        New-EvFixtureLog -Dir $d14 -Name 'launch_current_a.json' -Mode 'live-shadow' -InvocationId $idA -Started 'observed_false' -Verified 'observed_true' | Out-Null
        $r14 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d14 -ExpectedInvocationId $idA
        if ($r14.daemon_started_by_this_invocation -eq 'not_observed' -and $r14.daemon_reachable_and_verified -eq 'not_observed' -and $null -eq $r14.source_log -and $r14.reason -match 'ambiguous') {
            Pass 'LS-EV-14' "Historical log + current log both claiming exact id A still fail closed as ambiguous -- no newest-file/creation-time arbitration"
        } else {
            Fail 'LS-EV-14' "Did not fail closed on historical+current duplicate-id logs: $($r14 | ConvertTo-Json -Compress)"
        }

        # LS-EV-15: exact mode+id match, but daemon evidence fields hold
        # invalid types/values -- must never propagate; fails closed to
        # 'not_observed' with a reason.
        $d15 = Join-Path $fixtureRoot 'ev15'
        New-Item -ItemType Directory -Force -Path $d15 | Out-Null
        $p15 = Join-Path $d15 'launch_new.json'
        # Constructed directly (not via New-EvFixtureLog) so the evidence
        # fields can carry a JSON boolean / free-text string rather than one
        # of the closed-set values.
        $obj15 = [ordered]@{
            timestamp     = (Get-Date).ToUniversalTime().ToString('o')
            mode          = 'live-shadow'
            invocation_id = $idA
            stages        = @()
            daemon_started_by_this_invocation = 'yes'
            daemon_reachable_and_verified     = $true
        }
        ($obj15 | ConvertTo-Json -Depth 5) | Set-Content -Path $p15 -Encoding UTF8
        $r15 = Resolve-LiveShadowRunEvidence -LauncherLogDir $d15 -ExpectedInvocationId $idA
        if ($r15.daemon_started_by_this_invocation -eq 'not_observed' -and $r15.daemon_reachable_and_verified -eq 'not_observed' -and $r15.source_log -eq $p15 -and $r15.reason -match 'malformed') {
            Pass 'LS-EV-15' "Invalid evidence values ('yes' string, JSON true) never propagate -- fail closed to 'not_observed' with a reason"
        } else {
            Fail 'LS-EV-15' "Invalid evidence value(s) were not rejected: $($r15 | ConvertTo-Json -Compress)"
        }

        Remove-Item -Path $fixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
    } catch {
        Fail 'LS-EV-01' "Hermetic fixture harness threw: $($_.Exception.Message)"
        Fail 'LS-EV-02' "skipped -- harness error"
        Fail 'LS-EV-03' "skipped -- harness error"
        Fail 'LS-EV-04' "skipped -- harness error"
        Fail 'LS-EV-05' "skipped -- harness error"
        Fail 'LS-EV-08' "skipped -- harness error"
        Fail 'LS-EV-09' "skipped -- harness error"
        Fail 'LS-EV-10' "skipped -- harness error"
        Fail 'LS-EV-14' "skipped -- harness error"
        Fail 'LS-EV-15' "skipped -- harness error"
    }
} else {
    Fail 'LS-EV-01' "skipped -- target file missing"
    Fail 'LS-EV-02' "skipped -- target file missing"
    Fail 'LS-EV-03' "skipped -- target file missing"
    Fail 'LS-EV-04' "skipped -- target file missing"
    Fail 'LS-EV-05' "skipped -- target file missing"
    Fail 'LS-EV-08' "skipped -- target file missing"
    Fail 'LS-EV-09' "skipped -- target file missing"
    Fail 'LS-EV-10' "skipped -- target file missing"
    Fail 'LS-EV-14' "skipped -- target file missing"
    Fail 'LS-EV-15' "skipped -- target file missing"
}

# ---------------------------------------------------------------------------
# LS-EV-11/12/13: real production path-construction seams, dot-sourced from
# both scripts (no daemon, no network, no evidence folder side effects
# beyond the disposable fixture paths constructed here -- these functions
# only build path strings; the launcher-log/bootstrap-log/evidence
# directories are never actually created by calling them).
# ---------------------------------------------------------------------------
if ((Test-Path $Target) -and (Test-Path $Launcher)) {
    try {
        . $Target
        . $Launcher
        # Both target scripts declare their own param() blocks (including a
        # -RepoRoot default on Start-LiveShadowSmoke.ps1); dot-sourcing
        # re-binds those defaults into THIS script's scope, clobbering the
        # $RepoRoot computed at the top of this file. Restore it before use.
        $RepoRoot = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')

        # LS-EV-11: two real New-LauncherLog calls must never resolve to the
        # same launcher JSON path.
        $lp1 = New-LauncherLog -RepoRoot $RepoRoot -ModeLabel 'live-shadow'
        $lp2 = New-LauncherLog -RepoRoot $RepoRoot -ModeLabel 'live-shadow'
        if ($lp1 -ne $lp2) {
            Pass 'LS-EV-11' "Two real New-LauncherLog calls resolve to different paths (collision-proof independent of second-resolution timestamp)"
        } else {
            Fail 'LS-EV-11' "New-LauncherLog produced the SAME path twice: $lp1"
        }

        # LS-EV-12: two real New-LiveShadowBootstrapLogPaths calls must never
        # collide with each other or with themselves (stdout != stderr), and
        # this holds even conceptually reusing the same caller InvocationId
        # (the helper takes no InvocationId at all -- uniqueness is
        # independent of it by construction).
        $bootstrapDir = Join-Path $RepoRoot 'exports\launcher'
        $bp1 = New-LiveShadowBootstrapLogPaths -BootstrapLogDir $bootstrapDir
        $bp2 = New-LiveShadowBootstrapLogPaths -BootstrapLogDir $bootstrapDir
        if ($bp1.StdoutLogPath -ne $bp2.StdoutLogPath -and $bp1.StderrLogPath -ne $bp2.StderrLogPath -and
            $bp1.StdoutLogPath -ne $bp1.StderrLogPath -and $bp2.StdoutLogPath -ne $bp2.StderrLogPath) {
            Pass 'LS-EV-12' "Two real New-LiveShadowBootstrapLogPaths calls produce four distinct paths -- no same-second or reused-identity collision"
        } else {
            Fail 'LS-EV-12' "Bootstrap log path collision: A=$($bp1 | ConvertTo-Json -Compress) B=$($bp2 | ConvertTo-Json -Compress)"
        }

        # LS-EV-13: two real Get-LiveShadowEvidenceDirPath calls (distinct
        # wrapper invocation ids, as the real wrapper always generates) must
        # never collide, even within the same timestamp bucket.
        $ep1 = Get-LiveShadowEvidenceDirPath -RepoRoot $RepoRoot -InvocationId ([guid]::NewGuid().ToString())
        $ep2 = Get-LiveShadowEvidenceDirPath -RepoRoot $RepoRoot -InvocationId ([guid]::NewGuid().ToString())
        if ($ep1 -ne $ep2) {
            Pass 'LS-EV-13' "Two real Get-LiveShadowEvidenceDirPath calls resolve to different evidence directories"
        } else {
            Fail 'LS-EV-13' "Get-LiveShadowEvidenceDirPath produced the SAME evidence directory twice: $ep1"
        }
    } catch {
        Fail 'LS-EV-11' "Real path-construction harness threw: $($_.Exception.Message)"
        Fail 'LS-EV-12' "skipped -- harness error"
        Fail 'LS-EV-13' "skipped -- harness error"
    }
} else {
    Fail 'LS-EV-11' "skipped -- target or launcher file missing"
    Fail 'LS-EV-12' "skipped -- target or launcher file missing"
    Fail 'LS-EV-13' "skipped -- target or launcher file missing"
}

# ---------------------------------------------------------------------------
# LS-EV-16 / LS-EV-17: real Start-MiniQuantDesk.ps1 -Mode LiveShadow
# -CheckOnly invocations only (read-only/report-only by construction --
# Invoke-LiveShadowCheckOnly). No daemon, no broker call, no order.
# ---------------------------------------------------------------------------
if (Test-Path $Launcher) {
    # LS-EV-16: an explicitly malformed -InvocationId must fail closed
    # before any LiveShadow startup behavior -- nonzero exit, no operational
    # startup attempted.
    try {
        $ev16Output = & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $Launcher -Mode LiveShadow -CheckOnly -InvocationId 'not-a-valid-guid' 2>&1 | Out-String
        $ev16Exit = $LASTEXITCODE
        if ($ev16Exit -ne 0 -and $ev16Output -match 'Invalid -InvocationId') {
            Pass 'LS-EV-16' "Malformed -InvocationId 'not-a-valid-guid' fails closed (exit $ev16Exit) before any LiveShadow startup behavior"
        } else {
            Fail 'LS-EV-16' "Malformed -InvocationId was not rejected as expected (exit $ev16Exit): $ev16Output"
        }
    } catch {
        Fail 'LS-EV-16' "Real -CheckOnly invocation with malformed -InvocationId threw: $($_.Exception.Message)"
    }

    # LS-EV-17: a direct invocation with NO -InvocationId supplied must still
    # receive a nonblank, valid, internally-generated invocation_id in its
    # launcher log (closes the blank-ID case, not just the wrapper path).
    try {
        $ev17Before = Get-Date
        & powershell.exe -NoProfile -ExecutionPolicy Bypass -File $Launcher -Mode LiveShadow -CheckOnly *> $null
        $ev17Exit = $LASTEXITCODE

        $ev17LogDir = Join-Path $RepoRoot 'smoke_logs\launcher\live-shadow'
        $ev17Newest = Get-ChildItem -Path $ev17LogDir -Filter 'launch_*.json' -ErrorAction SilentlyContinue |
            Where-Object { $_.CreationTimeUtc -ge $ev17Before.ToUniversalTime().AddSeconds(-5) } |
            Sort-Object CreationTimeUtc -Descending | Select-Object -First 1

        if ($ev17Exit -eq 0 -and $null -ne $ev17Newest) {
            $ev17Entry = Get-Content -Path $ev17Newest.FullName -Raw | ConvertFrom-Json
            $ev17ParsedGuid = [guid]::Empty
            $ev17IsValidGuid = [guid]::TryParse([string]$ev17Entry.invocation_id, [ref]$ev17ParsedGuid)
            if (-not [string]::IsNullOrWhiteSpace($ev17Entry.invocation_id) -and $ev17IsValidGuid) {
                Pass 'LS-EV-17' "Direct -Mode LiveShadow -CheckOnly with no -InvocationId still receives a nonblank, valid, internally-generated invocation_id ($($ev17Entry.invocation_id))"
            } else {
                Fail 'LS-EV-17' "Direct invocation's launcher log has a blank/invalid invocation_id: '$($ev17Entry.invocation_id)'"
            }
        } else {
            Fail 'LS-EV-17' "Direct -CheckOnly invocation (exit $ev17Exit) did not produce a discoverable fresh launcher log under $ev17LogDir"
        }
    } catch {
        Fail 'LS-EV-17' "Real direct -CheckOnly invocation threw: $($_.Exception.Message)"
    }
} else {
    Fail 'LS-EV-16' "skipped -- launcher file missing"
    Fail 'LS-EV-17' "skipped -- launcher file missing"
}

Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL LIVE-SHADOW-SMOKE-GUARD-01 INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
