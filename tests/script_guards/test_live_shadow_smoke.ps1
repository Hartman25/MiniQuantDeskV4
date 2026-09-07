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
                # A3B: CheckOnly's observed-evidence fields are the string
                # 'not_run' (this action category was never attempted) --
                # not a boolean $false -- per the truth-repair rationale in
                # Start-LiveShadowSmoke.ps1's own header. wrapper_direct_*
                # are the separate, provable-by-construction static-contract
                # booleans (always $false for this file, both CheckOnly and
                # full run).
                if ($manifest.check_only -eq $true -and
                    $manifest.schema_version -eq 'live-shadow-smoke-manifest-v2' -and
                    $manifest.deployment_mode_forced -eq 'live-shadow' -and
                    $manifest.canonical_launcher_mode -eq 'LiveShadow' -and
                    $manifest.wrapper_direct_broker_call -eq $false -and
                    $manifest.wrapper_direct_order_submission -eq $false -and
                    $manifest.real_daemon_start_performed -eq 'not_run' -and
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

Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL LIVE-SHADOW-SMOKE-GUARD-01 INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
