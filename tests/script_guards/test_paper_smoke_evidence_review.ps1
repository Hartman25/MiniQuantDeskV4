# =============================================================================
# Script guard: test_paper_smoke_evidence_review.ps1
# PAPER-SMOKE-EVIDENCE-REVIEW-02
#
# Static assertions for Review-PaperSmokeEvidence.ps1
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = Split-Path -Parent (Split-Path -Parent $ScriptDir)
$Target    = Join-Path $RepoRoot 'scripts\windows\Review-PaperSmokeEvidence.ps1'

$Failures  = 0

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if ($Condition) {
        Write-Host "  PASS: $Message" -ForegroundColor Green
    } else {
        Write-Host "  FAIL: $Message" -ForegroundColor Red
        $script:Failures++
    }
}

function Assert-False {
    param([bool]$Condition, [string]$Message)
    Assert-True (-not $Condition) $Message
}

Write-Host ''
Write-Host '--- test_paper_smoke_evidence_review.ps1 ---'

# 1. Review script exists
Assert-True (Test-Path $Target) "Review script exists at $Target"

if (-not (Test-Path $Target)) {
    Write-Host '  FATAL: Script missing -- skipping remaining assertions' -ForegroundColor Red
    exit 1
}

$Content = Get-Content $Target -Raw

# 2. Supports -EvidencePath parameter
Assert-True ($Content -match '\[string\]\$EvidencePath') 'Supports -EvidencePath parameter'

# 3. Supports -Latest switch
Assert-True ($Content -match '\[switch\]\$Latest') 'Supports -Latest switch'

# 4. Supports -WriteSummary switch
Assert-True ($Content -match '\[switch\]\$WriteSummary') 'Supports -WriteSummary switch'

# 5. Does not call broker order endpoints (/v2/orders)
Assert-False ($Content -match '(?im)^\s*(Invoke-WebRequest|Invoke-RestMethod|curl|iwr|irm)\b.*\/v2\/orders') 'Does not call /v2/orders broker endpoint'

# 6. Does not invoke Start-PaperTradingSmoke
Assert-False ($Content -match 'Start-PaperTradingSmoke') 'Does not call Start-PaperTradingSmoke'

# 7. Does not mutate DB (no SQL mutation patterns — uppercase keywords in code context)
Assert-False ($Content -match '\bINSERT INTO\b|\bUPDATE .* SET\b|\bDELETE FROM\b|\bDROP TABLE\b') 'Does not contain DB mutation SQL (INSERT INTO/UPDATE SET/DELETE FROM/DROP TABLE)'
Assert-False ($Content -match '(?im)^\s*&?\s*psql\b.*-c.*(INSERT|DELETE FROM|DROP TABLE)') 'Does not invoke psql with mutating SQL'

# 8. Does not print secret values
Assert-False ($Content -match 'Write-Host.*ALPACA_API_SECRET') 'Does not print ALPACA_API_SECRET value'
Assert-False ($Content -match 'Write-Host.*MQK_OPERATOR_TOKEN') 'Does not print MQK_OPERATOR_TOKEN value'

# 9. Contains all five classification strings
Assert-True ($Content -match 'NATURAL-TRADE-LIFECYCLE-CLOSED') 'Contains classification NATURAL-TRADE-LIFECYCLE-CLOSED'
Assert-True ($Content -match 'READINESS-CLOSED-NO-TRADE')      'Contains classification READINESS-CLOSED-NO-TRADE'
Assert-True ($Content -match 'PARTIAL')                        'Contains classification PARTIAL'
Assert-True ($Content -match "'OPEN'")                         "Contains classification OPEN"
Assert-True ($Content -match 'FALSE-CLOSED')                   'Contains classification FALSE-CLOSED'

# 10. Contains secret scan logic
Assert-True ($Content -match 'SecretWarnings|SecretScan|secret.*scan') 'Contains secret scan logic'

# 11. Does not call Invoke-WebRequest or Invoke-RestMethod (no live HTTP calls)
Assert-False ($Content -match 'Invoke-WebRequest|Invoke-RestMethod') 'Does not make live HTTP calls'

# 12. WriteSummary writes review_summary.md
Assert-True ($Content -match 'review_summary\.md') 'WriteSummary writes review_summary.md'

# 13. Schema version present (review-v2)
Assert-True ($Content -match 'review-v2') 'JSON output includes schema_version review-v2'

# 14. Secret scan uses Test-SecretLeakLine helper (targeted patterns, not bare words)
Assert-True ($Content -match 'Test-SecretLeakLine') 'Contains Test-SecretLeakLine helper function'

# 15. Standalone word DISCORD is NOT a bare scan pattern (avoids false-positives on template headers)
#     The old offending line was: @('ALPACA_API_SECRET', 'MQK_OPERATOR_TOKEN', 'DISCORD', 'Bearer ')
#     Verify the bare-word match against 'DISCORD' is gone.
Assert-False ($Content -match "'DISCORD'") "Does not use bare 'DISCORD' as a secret pattern"

# 16. Script contains DISCORD_WEBHOOK_URL as a targeted pattern (real secret detection preserved)
Assert-True ($Content -match 'DISCORD_WEBHOOK_URL') 'Contains DISCORD_WEBHOOK_URL targeted pattern'

# 17. Script contains DISCORD_BOT_TOKEN as a targeted pattern
Assert-True ($Content -match 'DISCORD_BOT_TOKEN') 'Contains DISCORD_BOT_TOKEN targeted pattern'

# 18. Secret scan requires non-empty/non-redacted value before flagging (value filtering present)
Assert-True ($Content -match 'RedactedPlaceholders|notin.*RedactedPlaceholders|val.*-ne.*""') 'Secret scan filters out redacted/empty values'

# 19. Bearer detection requires a minimum token length (not just the word "Bearer")
Assert-True ($Content -match 'token\.Length -gt') 'Bearer detection checks token.Length before flagging'

# 20-22. Runtime: load Test-SecretLeakLine from production source and exercise it inline.
#   Extract the helper block (RedactedPlaceholders + Test-SecretLeakLine + Invoke-SecretScan)
#   then Invoke-Expression it into this scope.  No subprocess needed.
$_rtSrc   = Get-Content $Target -Raw
$_rtLines = $_rtSrc -split '\r?\n'
$_rtStart = ($_rtLines | Select-String -Pattern '^\$RedactedPlaceholders\s*=' |
    Select-Object -First 1).LineNumber - 1
$_rtEnd   = ($_rtLines | Select-String -Pattern '^\}' |
    Where-Object { $_.LineNumber -gt ($_rtStart + 30) } |
    Select-Object -First 1).LineNumber - 1
$_rtBlock = ($_rtLines[$_rtStart..$_rtEnd]) -join "`n"

try {
    Invoke-Expression $_rtBlock

    $rt1 = Test-SecretLeakLine -Line 'Discord observation: smoke passed' -FilePath 'note.txt' -LineNo 1
    $rt2 = Test-SecretLeakLine -Line 'DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/123/realtoken' -FilePath 'note.txt' -LineNo 2
    $rt3 = Test-SecretLeakLine -Line 'Authorization: Bearer eyJhbGciOiJSUzI1NiJ9.longtoken123456' -FilePath 'note.txt' -LineNo 3

    Assert-True  ($null -eq $rt1)                                       'Test-SecretLeakLine does not flag standalone Discord text'
    Assert-True  ($null -ne $rt2 -and $rt2.PatternName -eq 'DISCORD_WEBHOOK_URL') 'Test-SecretLeakLine detects DISCORD_WEBHOOK_URL=<url>'
    Assert-True  ($null -ne $rt3 -and $rt3.PatternName -eq 'Authorization-Bearer') 'Test-SecretLeakLine detects Authorization: Bearer <token>'
} catch {
    Write-Host "  FAIL: Runtime Test-SecretLeakLine test threw: $_" -ForegroundColor Red
    $script:Failures += 3
}

# 23-25. Final-verdict hygiene (PAPER-SMOKE-EVIDENCE-VERDICT-HYGIENE-01)
#   - manual_verdict detection requires an explicitly-CHECKED checkbox, not a
#     bare substring match against the unedited template's option list.
#   - manual_verdict_conflict is computed and surfaced when an operator
#     checks "[x] SMOKE PASSED" but VERDICT did not reach a closed state.
#   - review_summary documents itself (not notes/final_verdict.txt) as the
#     source of truth.
Assert-True ($Content -match [regex]::Escape('\[\s*[xX]\s*\]\s*SMOKE PASSED')) 'manual_verdict detection requires an explicitly-checked [x]/[X] checkbox, not a bare SMOKE PASSED match'
Assert-True ($Content -match 'manual_verdict_conflict') 'Computes and exposes manual_verdict_conflict when an operator note conflicts with VERDICT'
Assert-True ($Content -match '(?i)## Source of Truth') 'review_summary documents a Source of Truth section'
Assert-True ($Content -match '(?i)NOT authoritative') 'review_summary states notes/final_verdict.txt is NOT authoritative'

# 26-30. Multi-symbol dispatch summary (WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01)
#   Static checks: review script reads the captured snapshot, renders a
#   dedicated section, and surfaces the per-symbol fields in its JSON output.
Assert-True ($Content -match [regex]::Escape('multi_symbol_dispatch_summary.json')) 'MS-EV-04: reads multi_symbol_dispatch_summary.json'
Assert-True ($Content -match '(?i)## Multi-symbol dispatch summary') 'MS-EV-05: renders a "Multi-symbol dispatch summary" section'
Assert-True ($Content -match 'multi_symbol_dispatch_truth_state') 'MS-EV-06: JSON output includes multi_symbol_dispatch_truth_state'
Assert-True ($Content -match 'multi_symbol_dispatch_row_count' -and $Content -match 'multi_symbol_dispatch_symbols_seen') 'MS-EV-07: JSON output includes multi_symbol_dispatch_row_count and multi_symbol_dispatch_symbols_seen'
Assert-True (
    $Content -match 'last_decision_disposition' -and
    $Content -match 'day_order_count' -and
    $Content -match 'day_order_limit' -and
    $Content -match 'bar_staleness_secs'
) 'MS-EV-08: per-symbol fields (last_decision_disposition, day_order_count/limit, bar_staleness_secs) are surfaced'

function Write-FixtureJson {
    param([string]$Path, $Value)
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    $Value | ConvertTo-Json -Depth 10 | Set-Content -Path $Path -Encoding UTF8
}

function Write-FixtureText {
    param([string]$Path, [string]$Value)
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path $parent)) {
        New-Item -ItemType Directory -Path $parent -Force | Out-Null
    }
    $Value | Set-Content -Path $Path -Encoding UTF8
}

function Get-UntouchedFinalVerdictTemplateText {
    param([string]$Label = 'guard_test')
    return @"
# OPERATOR FINAL VERDICT -- TEMPLATE ONLY
# Status: PENDING OPERATOR REVIEW
#
# This file is NOT the source of truth until an operator completes it.
# Use review_summary.json and review_summary.md (written by
# Review-PaperSmokeEvidence.ps1 -WriteSummary) as the generated
# evidence-review source of truth.
#
# Do not mark SMOKE PASSED unless the review_summary classification is
# NATURAL-TRADE-LIFECYCLE-CLOSED (or READINESS-CLOSED-NO-TRADE for a
# no-trade session) AND the full lifecycle/reconcile requirements listed
# there are satisfied. If the classification is OPEN, PARTIAL, or
# FALSE-CLOSED, do not select SMOKE PASSED below.
#
# Date (UTC):
# Smoke label:    $Label
# Git commit:
# Proof state:
# Generated review classification (from review_summary.json):
#
# Allowed operator verdict values -- check exactly ONE box after review:
#   [ ] SMOKE PASSED   -- full lifecycle proven, reconcile clean, evidence durable
#   [ ] SMOKE PARTIAL  -- partial lifecycle (describe)
#   [ ] SMOKE FAILED   -- failed (describe)
#   [ ] SMOKE NOT RUN  -- daemon/session not reached
#
# Blockers for next smoke (if any):
#
# Next action:
"@
}

function New-StrictLifecycleFixture {
    param(
        [string]$Name,
        [bool]$IncludeAck = $true,
        [bool]$IncludeApply = $true,
        [bool]$FinalFlat = $true,
        [bool]$ReconcileClean = $true,
        [bool]$LiveRoutingEnabled = $false,
        [string[]]$UnsafeMarkers = @()
    )

    $casePath = Join-Path $script:ReviewFixtureRoot $Name
    New-Item -ItemType Directory -Path (Join-Path $casePath 'api') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $casePath 'db') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $casePath 'notes') -Force | Out-Null

    $reconcileStatus = if ($ReconcileClean) { 'ok' } else { 'dirty' }
    $mismatchRows = [System.Collections.ArrayList]::new()
    if (-not $ReconcileClean) {
        [void]$mismatchRows.Add([pscustomobject]@{ kind = 'position'; symbol = 'AAPL' })
    }
    $positionRows = [System.Collections.ArrayList]::new()
    if (-not $FinalFlat) {
        [void]$positionRows.Add([pscustomobject]@{ symbol = 'AAPL'; qty = 1 })
    }
    $positionCount = if ($FinalFlat) { 0 } else { 1 }

    $flowRows = @(
        [ordered]@{ stage = 'outbox_enqueued'; source_table = 'oms_outbox'; symbol = 'AAPL'; side = 'sell'; message = 'Sell order enqueued through normal path' },
        [ordered]@{ stage = 'broker_sent'; source_table = 'oms_outbox'; symbol = 'AAPL'; side = 'sell'; message = 'Sell order submitted to broker' },
        [ordered]@{ stage = 'broker_final_fill'; source_table = 'fill_quality_telemetry'; symbol = 'AAPL'; message = 'Final fill received for AAPL' }
    )
    if ($IncludeAck) {
        $flowRows += [ordered]@{ stage = 'broker_ack'; source_table = 'oms_inbox'; symbol = 'AAPL'; side = 'sell'; message = 'Broker ACK accepted' }
    }
    if ($IncludeApply) {
        $flowRows += [ordered]@{ stage = 'fill_applied'; source_table = 'oms_inbox'; symbol = 'AAPL'; message = 'Fill applied to durable execution state' }
    }

    Write-FixtureJson (Join-Path $casePath 'api\system_status.json') ([ordered]@{
        daemon_mode = 'paper'
        runtime_status = 'running'
        strategy_armed = $true
        execution_armed = $true
        kill_switch_active = $false
        live_routing_enabled = $LiveRoutingEnabled
        integrity_halt_active = $false
        risk_halt_active = $false
        reconcile_status = $reconcileStatus
        fault_signals = @()
    })
    Write-FixtureJson (Join-Path $casePath 'api\system_preflight.json') ([ordered]@{
        live_routing_disabled = (-not $LiveRoutingEnabled)
    })
    Write-FixtureJson (Join-Path $casePath 'api\autonomous_readiness.json') ([ordered]@{
        arm_state = 'armed'
        reconcile_ready = $ReconcileClean
        signal_ingestion_configured = $true
        last_bar_signal_qty = -1
        market_data_freshness = [ordered]@{
            completed_rows = 5
            freshness_state = 'ok'
        }
    })
    Write-FixtureJson (Join-Path $casePath 'api\events_feed.json') ([ordered]@{
        rows = @([ordered]@{ kind = 'runtime_transition'; detail = 'RUNNING'; run_id = 'strict-fixture-run' })
    })
    Write-FixtureJson (Join-Path $casePath 'api\oms_overview.json') ([ordered]@{
        run_id = 'strict-fixture-run'
        position_count = $positionCount
        open_order_count = 0
        fill_count = 1
        execution_active_orders = 0
        execution_pending_orders = 0
        reconcile_total_mismatches = $(if ($ReconcileClean) { 0 } else { 1 })
    })
    Write-FixtureJson (Join-Path $casePath 'api\reconcile_status.json') ([ordered]@{
        status = $reconcileStatus
        mismatched_positions = $(if ($ReconcileClean) { 0 } else { 1 })
        mismatched_orders = 0
        mismatched_fills = 0
        unmatched_broker_events = 0
    })
    Write-FixtureJson (Join-Path $casePath 'api\reconcile_mismatches.json') ([ordered]@{ rows = @($mismatchRows.ToArray()) })
    Write-FixtureJson (Join-Path $casePath 'api\execution_flow.json') ([ordered]@{ rows = $flowRows })
    Write-FixtureJson (Join-Path $casePath 'api\execution_orders.json') ([ordered]@{
        rows = @([ordered]@{ symbol = 'AAPL'; side = 'sell'; status = 'submitted' })
    })
    Write-FixtureJson (Join-Path $casePath 'api\execution_outbox.json') ([ordered]@{
        rows = @([ordered]@{ symbol = 'AAPL'; side = 'sell'; status = 'sent' })
    })
    Write-FixtureJson (Join-Path $casePath 'api\fill_quality.json') ([ordered]@{
        rows = @([ordered]@{ symbol = 'AAPL'; fill_qty = 1 })
    })
    Write-FixtureJson (Join-Path $casePath 'api\portfolio_fills.json') ([ordered]@{
        rows = @([ordered]@{ symbol = 'AAPL'; qty = 1 })
    })
    Write-FixtureJson (Join-Path $casePath 'api\portfolio_positions.json') ([ordered]@{ rows = @($positionRows.ToArray()) })

    $ackLine = if ($IncludeAck) { '1 | order-1 | broker_ack | t | 2026-06-07T00:00:01Z' } else { '' }
    $applyLine = if ($IncludeApply) { '2 | order-1 | final_fill | t | 2026-06-07T00:00:02Z' } else { '2 | order-1 | final_fill | f | 2026-06-07T00:00:02Z' }
    Write-FixtureText (Join-Path $casePath 'db\oms_inbox_recent.txt') @"
 id | order_id | event_type | applied | created_at
$ackLine
$applyLine
"@
    Write-FixtureText (Join-Path $casePath 'db\broker_order_map_recent.txt') @"
 order_id | broker_order_id | created_at
 order-1 | broker-1 | 2026-06-07T00:00:00Z
(1 row)
"@
    Write-FixtureText (Join-Path $casePath 'notes\smoke_lifecycle_checklist.txt') '# [YES] Broker position baseline adopted (positions=1, orders=0, reconcile_after=ok)'
    if ($UnsafeMarkers.Count -gt 0) {
        Write-FixtureText (Join-Path $casePath 'notes\unsafe_markers.txt') (($UnsafeMarkers | ForEach-Object { "$_=true" }) -join "`n")
    }

    return $casePath
}

function New-MultiSymbolDispatchFixture {
    param(
        [string]$Name,
        [string]$TruthState = 'active',
        [string]$RuntimeExecutionMode = 'multi_symbol',
        $ConfiguredSymbolCount = 0,
        [object[]]$PerSymbol = @(),
        [bool]$WriteFile = $true
    )

    $casePath = New-StrictLifecycleFixture -Name $Name

    if ($WriteFile) {
        Write-FixtureJson (Join-Path $casePath 'api\multi_symbol_dispatch_summary.json') ([ordered]@{
            canonical_route         = '/api/v1/strategy/multi-symbol-dispatch-summary'
            backend                 = 'daemon.runtime_state'
            truth_state             = $TruthState
            runtime_execution_mode  = $RuntimeExecutionMode
            configured_symbol_count = $ConfiguredSymbolCount
            per_symbol              = @($PerSymbol)
        })
    }

    return $casePath
}

function Invoke-ReviewFixture {
    param([string]$Path)
    $output = & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $Target -EvidencePath $Path -WriteSummary 2>&1
    if ($LASTEXITCODE -ne 0) {
        Write-Host "  FAIL: Review fixture exited $LASTEXITCODE for $Path" -ForegroundColor Red
        Write-Host ($output | Out-String) -ForegroundColor Red
        $script:Failures++
        return $null
    }
    $summaryPath = Join-Path $Path 'review_summary.json'
    if (-not (Test-Path $summaryPath)) {
        Write-Host "  FAIL: Review fixture did not write $summaryPath" -ForegroundColor Red
        $script:Failures++
        return $null
    }
    return (Get-Content $summaryPath -Raw | ConvertFrom-Json)
}

function Assert-NotLifecycleClosed {
    param($Review, [string]$Message, [string]$ExpectedMissing)
    Assert-True ($null -ne $Review) "$Message produced review output"
    if ($null -eq $Review) { return }
    Assert-False ($Review.classification -eq 'NATURAL-TRADE-LIFECYCLE-CLOSED') $Message
    if ($ExpectedMissing) {
        Assert-True (@($Review.lifecycle_missing_requirements) -contains $ExpectedMissing) "$Message reports missing $ExpectedMissing"
    }
}

$script:ReviewFixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_review_guard_" + [guid]::NewGuid().ToString('N'))
try {
    Write-Host ''
    Write-Host '  -- strict lifecycle false-close fixture tests --'

    $positive = Invoke-ReviewFixture (New-StrictLifecycleFixture -Name 'positive_complete')
    Assert-True ($null -ne $positive -and $positive.classification -eq 'NATURAL-TRADE-LIFECYCLE-CLOSED') 'Complete strict lifecycle fixture closes'
    Assert-True ($null -ne $positive -and @($positive.lifecycle_missing_requirements).Count -eq 0) 'Complete strict lifecycle fixture has no missing requirements'
    Assert-False ($null -ne $positive -and $positive.classification -match 'GUI|DISCORD') 'Reviewer does not emit GUI/Discord lifecycle closure without GUI/Discord evidence'

    Assert-NotLifecycleClosed (Invoke-ReviewFixture (New-StrictLifecycleFixture -Name 'missing_ack' -IncludeAck:$false)) `
        'Fill exists but ACK missing does not close lifecycle' 'has_order_ack'

    Assert-NotLifecycleClosed (Invoke-ReviewFixture (New-StrictLifecycleFixture -Name 'missing_apply' -IncludeApply:$false)) `
        'ACK/fill exist but inbox apply missing does not close lifecycle' 'has_inbox_apply_or_durable_applied_state'

    $notFlat = Invoke-ReviewFixture (New-StrictLifecycleFixture -Name 'not_flat' -FinalFlat:$false)
    Assert-NotLifecycleClosed $notFlat `
        'ACK/fill/apply exist but final position not flat does not close lifecycle' `
        'final_position_flat'

    Assert-NotLifecycleClosed (Invoke-ReviewFixture (New-StrictLifecycleFixture -Name 'reconcile_dirty' -ReconcileClean:$false)) `
        'Final flat but reconcile dirty does not close lifecycle' 'reconcile_clean'

    Assert-NotLifecycleClosed (Invoke-ReviewFixture (New-StrictLifecycleFixture -Name 'live_routing_enabled' -LiveRoutingEnabled:$true)) `
        'live_routing_enabled=true does not close lifecycle' 'live_routing_enabled=false'

    $unsafe = Invoke-ReviewFixture (New-StrictLifecycleFixture -Name 'unsafe_markers' `
        -UnsafeMarkers @('direct_broker_shortcut', 'fake_fill', 'fake_bar', 'signal_injection', 'manual_db_mutation'))
    Assert-NotLifecycleClosed $unsafe 'Unsafe evidence markers do not close lifecycle' 'no_direct_broker_shortcut_marker'
    Assert-True ($null -ne $unsafe -and $unsafe.classification -eq 'FALSE-CLOSED') 'Unsafe evidence markers classify as FALSE-CLOSED'
    Assert-True ($null -ne $unsafe -and @($unsafe.unsafe_marker_hits).Count -ge 5) 'Unsafe evidence marker details are reported'

    Write-Host ''
    Write-Host '  -- final_verdict.txt verdict-hygiene fixture tests (PAPER-SMOKE-EVIDENCE-VERDICT-HYGIENE-01) --'

    # (a) OPEN classification + untouched (unchecked) template -> no manual
    #     verdict, no conflict. The bare "[ ] SMOKE PASSED" option must not
    #     be misread as an operator verdict.
    $openUntouchedPath = New-StrictLifecycleFixture -Name 'verdict_open_untouched_template' -ReconcileClean:$false
    Write-FixtureText (Join-Path $openUntouchedPath 'notes\final_verdict.txt') (Get-UntouchedFinalVerdictTemplateText -Label 'verdict_open_untouched_template')
    $openUntouched = Invoke-ReviewFixture $openUntouchedPath
    Assert-True ($null -ne $openUntouched -and $openUntouched.classification -eq 'OPEN') 'Reconcile-dirty fixture classifies as OPEN'
    Assert-True ($null -ne $openUntouched -and $null -eq $openUntouched.manual_verdict_note) 'Untouched final_verdict.txt template produces no manual_verdict_note (no false SMOKE PASSED)'
    Assert-False ($null -ne $openUntouched -and $openUntouched.manual_verdict_conflict -eq $true) 'Untouched template does not trigger manual_verdict_conflict'

    # (b) OPEN classification + operator checked "[x] SMOKE PASSED" -> read
    #     as manual_verdict_note=SMOKE PASSED AND flagged as a conflict.
    $openCheckedPath = New-StrictLifecycleFixture -Name 'verdict_open_checked_passed' -ReconcileClean:$false
    $checkedPassedText = (Get-UntouchedFinalVerdictTemplateText -Label 'verdict_open_checked_passed') -replace '\[ \] SMOKE PASSED', '[x] SMOKE PASSED'
    Write-FixtureText (Join-Path $openCheckedPath 'notes\final_verdict.txt') $checkedPassedText
    $openChecked = Invoke-ReviewFixture $openCheckedPath
    Assert-True ($null -ne $openChecked -and $openChecked.classification -eq 'OPEN') 'Reconcile-dirty fixture with [x] SMOKE PASSED still classifies as OPEN'
    Assert-True ($null -ne $openChecked -and $openChecked.manual_verdict_note -eq 'SMOKE PASSED') 'Checked [x] SMOKE PASSED is read as manual_verdict_note=SMOKE PASSED'
    Assert-True ($null -ne $openChecked -and $openChecked.manual_verdict_conflict -eq $true) 'Operator [x] SMOKE PASSED on a non-closed VERDICT sets manual_verdict_conflict=true'

    if ($null -ne $openChecked) {
        $openCheckedMd = Get-Content (Join-Path $openCheckedPath 'review_summary.md') -Raw
        Assert-True ($openCheckedMd -match '(?i)WARNING') 'review_summary.md contains a WARNING when manual_verdict_conflict is true'
        Assert-True ($openCheckedMd -match '(?i)SMOKE PASSED') 'review_summary.md WARNING references SMOKE PASSED'
        Assert-True ($openCheckedMd -match '(?i)source of truth') 'review_summary.md states it is the source of truth, not final_verdict.txt'
    }

    # (c) NATURAL-TRADE-LIFECYCLE-CLOSED + operator checked "[x] SMOKE PASSED"
    #     -> no conflict (operator note agrees with VERDICT).
    $closedCheckedPath = New-StrictLifecycleFixture -Name 'verdict_closed_checked_passed'
    $closedCheckedText = (Get-UntouchedFinalVerdictTemplateText -Label 'verdict_closed_checked_passed') -replace '\[ \] SMOKE PASSED', '[x] SMOKE PASSED'
    Write-FixtureText (Join-Path $closedCheckedPath 'notes\final_verdict.txt') $closedCheckedText
    $closedChecked = Invoke-ReviewFixture $closedCheckedPath
    Assert-True ($null -ne $closedChecked -and $closedChecked.classification -eq 'NATURAL-TRADE-LIFECYCLE-CLOSED') 'Complete strict lifecycle fixture with [x] SMOKE PASSED still classifies as NATURAL-TRADE-LIFECYCLE-CLOSED'
    Assert-True ($null -ne $closedChecked -and $closedChecked.manual_verdict_note -eq 'SMOKE PASSED') 'Checked [x] SMOKE PASSED is read as manual_verdict_note=SMOKE PASSED on a closed verdict'
    Assert-False ($null -ne $closedChecked -and $closedChecked.manual_verdict_conflict -eq $true) 'Operator [x] SMOKE PASSED on a NATURAL-TRADE-LIFECYCLE-CLOSED VERDICT does not trigger manual_verdict_conflict'

    Write-Host ''
    Write-Host '  -- multi-symbol dispatch summary fixture tests (WATCHLIST-PROMO-V2-MULTI-SYMBOL-SMOKE-EVIDENCE-01) --'

    # Six rows covering all six canonical no_order_reason strings, including
    # the three classified as "blocked or skipped" (MSFT/AMZN/TSLA) and a
    # null bar_staleness_secs on MSFT to prove the "n/a" rendering path.
    $msdRows = @(
        [ordered]@{
            symbol = 'AAPL'; strategy_id = 'core_long_v1'
            current_qty = 10; target_qty = 10; delta = 0
            no_order_reason = 'already_at_target'
            last_decision_id = 'dec-aapl-1'; last_decision_disposition = 'no_action'
            day_order_count = 0; day_order_limit = 5
            bar_staleness_secs = 12
        },
        [ordered]@{
            symbol = 'MSFT'; strategy_id = 'core_long_v1'
            current_qty = 0; target_qty = -5; delta = -5
            no_order_reason = 'b5_short_sale_guard'
            last_decision_id = 'dec-msft-1'; last_decision_disposition = 'blocked'
            day_order_count = 1; day_order_limit = 5
            bar_staleness_secs = $null
        },
        [ordered]@{
            symbol = 'GOOG'; strategy_id = 'core_long_v1'
            current_qty = 0; target_qty = 8; delta = 8
            no_order_reason = 'order_will_be_submitted'
            last_decision_id = 'dec-goog-1'; last_decision_disposition = 'order_submitted'
            day_order_count = 2; day_order_limit = 5
            bar_staleness_secs = 30
        },
        [ordered]@{
            symbol = 'AMZN'; strategy_id = 'core_long_v1'
            current_qty = 0; target_qty = 4; delta = 4
            no_order_reason = 'max_new_orders_per_tick_reached'
            last_decision_id = 'dec-amzn-1'; last_decision_disposition = 'blocked'
            day_order_count = 5; day_order_limit = 5
            bar_staleness_secs = 45
        },
        [ordered]@{
            symbol = 'TSLA'; strategy_id = 'core_long_v1'
            current_qty = 2; target_qty = 2; delta = 0
            no_order_reason = 'symbol_mismatch_skipped'
            last_decision_id = 'dec-tsla-1'; last_decision_disposition = 'skipped'
            day_order_count = 0; day_order_limit = 5
            bar_staleness_secs = 60
        },
        [ordered]@{
            symbol = 'NVDA'; strategy_id = 'core_long_v1'
            current_qty = 0; target_qty = 0; delta = 0
            no_order_reason = 'no_decisions'
            last_decision_id = $null; last_decision_disposition = $null
            day_order_count = 0; day_order_limit = 5
            bar_staleness_secs = 5
        }
    )

    $msdActivePath = New-MultiSymbolDispatchFixture -Name 'msd_active_rows' -TruthState 'active' -RuntimeExecutionMode 'multi_symbol' -ConfiguredSymbolCount 6 -PerSymbol $msdRows
    $msdActive = Invoke-ReviewFixture $msdActivePath

    Assert-True ($null -ne $msdActive -and $msdActive.multi_symbol_dispatch_truth_state -eq 'active') 'MS-EV-09: multi_symbol_dispatch_truth_state=active for populated fixture'
    Assert-True ($null -ne $msdActive -and $msdActive.multi_symbol_dispatch_runtime_execution_mode -eq 'multi_symbol') 'MS-EV-09: multi_symbol_dispatch_runtime_execution_mode=multi_symbol'
    Assert-True ($null -ne $msdActive -and $msdActive.multi_symbol_dispatch_configured_symbol_count -eq 6) 'MS-EV-09: multi_symbol_dispatch_configured_symbol_count=6'
    Assert-True ($null -ne $msdActive -and $msdActive.multi_symbol_dispatch_row_count -eq 6) 'MS-EV-09: multi_symbol_dispatch_row_count=6'
    Assert-True ($null -ne $msdActive -and (@($msdActive.multi_symbol_dispatch_symbols_seen) -join ',') -eq 'AAPL,MSFT,GOOG,AMZN,TSLA,NVDA') 'MS-EV-09: multi_symbol_dispatch_symbols_seen preserves per_symbol order'

    $msdActiveMd = $null
    if ($null -ne $msdActive) {
        $msdActiveMd = Get-Content (Join-Path $msdActivePath 'review_summary.md') -Raw
        foreach ($reason in @('already_at_target', 'b5_short_sale_guard', 'order_will_be_submitted', 'max_new_orders_per_tick_reached', 'symbol_mismatch_skipped', 'no_decisions')) {
            $jsonHasReason = @($msdActive.multi_symbol_dispatch_per_symbol | Where-Object { $_.no_order_reason -eq $reason }).Count -gt 0
            Assert-True $jsonHasReason "MS-EV-10: review_summary.json preserves no_order_reason=$reason verbatim"
            Assert-True ($msdActiveMd -match [regex]::Escape($reason)) "MS-EV-10: review_summary.md preserves no_order_reason=$reason verbatim"
        }
    } else {
        Assert-True $false 'MS-EV-10: review fixture produced output to check no_order_reason values'
    }

    Assert-True ($null -ne $msdActive -and (@($msdActive.multi_symbol_dispatch_blocked_or_skipped_symbols) -join ',') -eq 'MSFT,AMZN,TSLA') 'MS-EV-11: blocked_or_skipped_symbols = MSFT,AMZN,TSLA'

    $msdNoSnapshotPath = New-MultiSymbolDispatchFixture -Name 'msd_no_snapshot' -TruthState 'no_snapshot' -ConfiguredSymbolCount 0 -PerSymbol @()
    $msdNoSnapshot = Invoke-ReviewFixture $msdNoSnapshotPath
    Assert-True ($null -ne $msdNoSnapshot -and $msdNoSnapshot.multi_symbol_dispatch_captured -eq $true) 'MS-EV-12: no_snapshot fixture is reported as captured'
    Assert-True ($null -ne $msdNoSnapshot -and $msdNoSnapshot.multi_symbol_dispatch_truth_state -eq 'no_snapshot') 'MS-EV-12: no_snapshot fixture truth_state=no_snapshot'
    Assert-True ($null -ne $msdNoSnapshot -and $msdNoSnapshot.multi_symbol_dispatch_row_count -eq 0) 'MS-EV-12: no_snapshot fixture row_count=0'
    Assert-True ($null -ne $msdNoSnapshot -and @($msdNoSnapshot.multi_symbol_dispatch_warnings).Count -eq 0) 'MS-EV-12: no_snapshot fixture produces no warnings'

    $msdMissingPath = New-MultiSymbolDispatchFixture -Name 'msd_not_captured' -WriteFile:$false
    $msdMissing = Invoke-ReviewFixture $msdMissingPath
    Assert-True ($null -ne $msdMissing -and $msdMissing.multi_symbol_dispatch_captured -eq $false) 'MS-EV-13: missing snapshot is reported as not captured'
    Assert-True ($null -ne $msdMissing -and @($msdMissing.multi_symbol_dispatch_warnings).Count -ge 1) 'MS-EV-13: missing snapshot produces at least one warning'
    if ($null -ne $msdMissing) {
        $msdMissingMd = Get-Content (Join-Path $msdMissingPath 'review_summary.md') -Raw
        Assert-True ($msdMissingMd -match '(?i)not captured') 'MS-EV-13: review_summary.md reports multi-symbol dispatch summary as not captured'
    }

    if ($null -ne $msdActiveMd) {
        Assert-True ($msdActiveMd -match 'MSFT:.*bar_staleness_secs=n/a') 'MS-EV-14: null bar_staleness_secs renders as n/a for MSFT row'
    } else {
        Assert-True $false 'MS-EV-14: review fixture produced markdown to check bar_staleness_secs rendering'
    }
} catch {
    Write-Host "  FAIL: Strict lifecycle fixture tests threw: $_" -ForegroundColor Red
    $script:Failures++
} finally {
    if (Test-Path $script:ReviewFixtureRoot) {
        Remove-Item -LiteralPath $script:ReviewFixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host ''
if ($Failures -eq 0) {
    Write-Host '  ALL ASSERTIONS PASSED (test_paper_smoke_evidence_review)' -ForegroundColor Green
    exit 0
} else {
    Write-Host "  $Failures ASSERTION(S) FAILED (test_paper_smoke_evidence_review)" -ForegroundColor Red
    exit 1
}
