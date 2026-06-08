# =============================================================================
# Script guard: test_paper_smoke_ledger_and_shutdown.ps1
# AUTONOMOUS-PAPER-NORMAL-OPS-AND-SMOKE-SEPARATION-01
#
# Static + fixture-based assertions proving:
#   (a) Stop-PaperTradingClean.ps1 (PAPER-SMOKE-CLEAN-SHUTDOWN-CAPTURE-01)
#       captures evidence before stopping/disarming and again after, uses
#       ONLY the pre-existing authorized stop-system/disarm-execution
#       action_key values, never touches broker/order routes or the DB,
#       and never executes the daemon-kill / container-stop steps it prints.
#   (b) operator_control_surface.md documents the normal-ops vs. smoke/
#       proof-harness separation and the clean-shutdown sequence.
#   (c) Review-PaperSmokeEvidence.ps1's ledger-specific classifier
#       (PAPER-SMOKE-LEDGER-SPECIFIC-CLASSIFIER-01) emits each of the six
#       ledger-specific CLOSED labels ONLY when its own strict, narrowed
#       evidence gate is satisfied -- never on partial/placeholder evidence,
#       and never merely because the generic NATURAL-TRADE-LIFECYCLE-CLOSED
#       gate happened to close.
#
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = Split-Path -Parent (Split-Path -Parent $ScriptDir)

$StopScript   = Join-Path $RepoRoot 'scripts\windows\Stop-PaperTradingClean.ps1'
$ReviewScript = Join-Path $RepoRoot 'scripts\windows\Review-PaperSmokeEvidence.ps1'
$ControlDoc   = Join-Path $RepoRoot 'docs\runbooks\operator_control_surface.md'

$Failures = 0

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
Write-Host '--- test_paper_smoke_ledger_and_shutdown.ps1 ---'

# ===========================================================================
# Section A: Stop-PaperTradingClean.ps1 (PAPER-SMOKE-CLEAN-SHUTDOWN-CAPTURE-01)
# ===========================================================================
Write-Host ''
Write-Host '  -- Section A: Stop-PaperTradingClean.ps1 static assertions --'

Assert-True (Test-Path $StopScript) "Stop-PaperTradingClean.ps1 exists at $StopScript"

if (-not (Test-Path $StopScript)) {
    Write-Host '  FATAL: Stop-PaperTradingClean.ps1 missing -- skipping Section A/D(shutdown) assertions' -ForegroundColor Red
} else {
    $StopContent = Get-Content $StopScript -Raw
    $StopLines   = $StopContent -split '\r?\n'

    # A1. Declares exactly the two pre-existing authorized action_key constants.
    Assert-True ($StopContent -match "\`$ACTION_KEY_STOP\s*=\s*'stop-system'")        "Declares ACTION_KEY_STOP = 'stop-system'"
    Assert-True ($StopContent -match "\`$ACTION_KEY_DISARM\s*=\s*'disarm-execution'") "Declares ACTION_KEY_DISARM = 'disarm-execution'"

    # A2. Invoke-DaemonOpsAction structurally refuses any other action_key (throws).
    Assert-True ($StopContent -match '(?s)function Invoke-DaemonOpsAction.*?throw\s+"BUG:.*only issues.*Refusing') `
        'Invoke-DaemonOpsAction throws on any action_key other than stop-system/disarm-execution'

    # A3. The only -ActionKey arguments passed anywhere are the two authorized constants.
    $actionKeyCalls = [regex]::Matches($StopContent, '-ActionKey\s+(\$\w+)')
    $unauthorizedActionKeyCalls = @($actionKeyCalls | Where-Object {
        $_.Groups[1].Value -ne '$ACTION_KEY_STOP' -and $_.Groups[1].Value -ne '$ACTION_KEY_DISARM'
    })
    Assert-True ($actionKeyCalls.Count -ge 2) 'Calls Invoke-DaemonOpsAction with -ActionKey at least twice (stop + disarm)'
    Assert-True ($unauthorizedActionKeyCalls.Count -eq 0) 'Every -ActionKey argument is one of the two authorized constants'

    # A4. Never calls a broker order endpoint or any other mutating route.
    Assert-False ($StopContent -match '(?im)^\s*(Invoke-WebRequest|Invoke-RestMethod|curl|iwr|irm)\b.*\/v2\/orders') 'Does not call /v2/orders broker endpoint'
    Assert-False ($StopContent -match '\/api\/v1\/(orders|broker)\b') 'Does not reference order/broker daemon routes'

    # A5. Never issues SQL / never mutates the DB directly.
    Assert-False ($StopContent -match '\bINSERT INTO\b|\bUPDATE .* SET\b|\bDELETE FROM\b|\bDROP TABLE\b') 'Does not contain DB mutation SQL'
    Assert-False ($StopContent -match '(?im)^\s*&?\s*psql\b') 'Does not invoke psql directly'

    # A6. Secret guard present and never prints secret values.
    Assert-True  ($StopContent -match 'SECRET_NAMES')           'Declares $SECRET_NAMES secret guard list'
    Assert-True  ($StopContent -match 'function Assert-NotSecret') 'Declares Assert-NotSecret helper'
    Assert-False ($StopContent -match 'Write-Host.*\$operatorToken\b') 'Does not print $operatorToken value'
    Assert-False ($StopContent -match 'Write-Host.*ALPACA_API_SECRET') 'Does not print ALPACA_API_SECRET value'

    # A7. Phase ordering: pre-stop capture is textually defined/invoked before
    #     stop-system, and post-stop capture is textually defined/invoked after
    #     disarm-execution. This mirrors the script's own "hard invariant" comment
    #     that phase 1 always runs before any mutating action_key, and phase 5
    #     always runs after.
    $idxPreStopCapture  = $StopContent.IndexOf('Phase 1/6: Pre-stop evidence capture')
    $idxStopAction      = $StopContent.IndexOf('Phase 2/6: Stop execution runtime')
    $idxDisarmAction    = $StopContent.IndexOf('Phase 4/6: Disarm execution')
    $idxPostStopCapture = $StopContent.IndexOf('Phase 5/6: Post-stop evidence capture')
    Assert-True ($idxPreStopCapture  -ge 0 -and $idxStopAction -ge 0 -and $idxPreStopCapture -lt $idxStopAction) `
        'Pre-stop evidence capture (phase 1) is sequenced before stop-system (phase 2)'
    Assert-True ($idxDisarmAction -ge 0 -and $idxPostStopCapture -ge 0 -and $idxDisarmAction -lt $idxPostStopCapture) `
        'Disarm-execution (phase 4) is sequenced before post-stop evidence capture (phase 5)'

    # A8. Both pre-stop and post-stop capture phases delegate to the existing
    #     read-only Capture-PaperSmokeEvidence.ps1 with distinct labels.
    Assert-True ($StopContent -match '\$PreStopLabel\s*=\s*"\$\{Label\}_pre_stop"')   'Builds <Label>_pre_stop evidence label'
    Assert-True ($StopContent -match '\$PostStopLabel\s*=\s*"\$\{Label\}_post_stop"') 'Builds <Label>_post_stop evidence label'
    Assert-True ($StopContent -match 'Invoke-EvidenceCapture -CaptureLabel \$PreStopLabel')  'Phase 1 invokes capture with the pre-stop label'
    Assert-True ($StopContent -match 'Invoke-EvidenceCapture -CaptureLabel \$PostStopLabel') 'Phase 5 invokes capture with the post-stop label'
    Assert-True ($StopContent -match 'Capture-PaperSmokeEvidence\.ps1') 'Delegates evidence capture to Capture-PaperSmokeEvidence.ps1'

    # A9. Never kills the daemon process or stops the DB container -- those are
    #     printed (operator decision) only, never executed.
    Assert-False ($StopContent -match "(?im)^\s*&?\s*Stop-Process\s+-Name\s+'mqk-daemon'") 'Does not directly invoke Stop-Process against mqk-daemon'
    Assert-False ($StopContent -match '(?im)^\s*&?\s*docker\s+stop\s+mqk-paper-postgres')  'Does not directly invoke docker stop against the Postgres container'
    Assert-True  ($StopContent -match "Write-Host\s+`"\s*Stop-Process -Name 'mqk-daemon'") 'Prints (does not execute) the daemon-kill step as operator guidance'
    Assert-True  ($StopContent -match 'Write-Host\s+"\s*docker stop mqk-paper-postgres')   'Prints (does not execute) the container-stop step as operator guidance'

    # A10. Fails soft -- never aborts the whole sequence just because the daemon
    #      is unreachable (UNAVAILABLE convention, matches Capture script).
    Assert-True ($StopContent -match 'Write-Unavail') 'Uses the UNAVAILABLE convention on soft failure (matches Capture-PaperSmokeEvidence.ps1)'
}

# ===========================================================================
# Section B: operator_control_surface.md documents the separation + shutdown
# ===========================================================================
Write-Host ''
Write-Host '  -- Section B: operator_control_surface.md separation + shutdown doc assertions --'

Assert-True (Test-Path $ControlDoc) "operator_control_surface.md exists at $ControlDoc"

if (Test-Path $ControlDoc) {
    $DocContent = Get-Content $ControlDoc -Raw

    Assert-True ($DocContent -match '(?i)Normal operation vs\.\s*smoke\s*/\s*proof harness') `
        'Documents the "normal operation vs. smoke/proof harness" framing'
    Assert-True ($DocContent -match 'autonomous_paper_ops\.md')   'References autonomous_paper_ops.md as the canonical normal-ops runbook'
    Assert-True ($DocContent -match 'AUTON-OPS-01')               'References AUTON-OPS-01'
    Assert-True ($DocContent -match 'PAPER-SMOKE-CLEAN-SHUTDOWN-CAPTURE-01') 'References PAPER-SMOKE-CLEAN-SHUTDOWN-CAPTURE-01'
    Assert-True ($DocContent -match 'Stop-PaperTradingClean\.ps1') 'References Stop-PaperTradingClean.ps1'
    Assert-True ($DocContent -match '(?im)^###?\s*9\.1')           'Documents section 9.1 (orchestrated script)'
    Assert-True ($DocContent -match '(?im)^###?\s*9\.2')           'Documents section 9.2 (manual fallback sequence)'
    Assert-True ($DocContent -match '(?im)^###?\s*9\.3')           'Documents section 9.3 (Postgres container)'
    Assert-True ($DocContent -match 'eod_shutdown_pre_stop')       'References the eod_shutdown_pre_stop evidence label'
    Assert-True ($DocContent -match 'eod_shutdown_post_stop')      'References the eod_shutdown_post_stop evidence label'
    Assert-True ($DocContent -match '(?is)capture useful evidence before\s+you stop anything.*?capture it again after') `
        'States the "capture before stopping, capture again after" rule in prose'
    Assert-True ($DocContent -match '\|\s*`?Stop-PaperTradingClean\.ps1`?\s*\|') 'Lists Stop-PaperTradingClean.ps1 in the script reference table'
}

# ===========================================================================
# Section C: Review-PaperSmokeEvidence.ps1 ledger classifier static assertions
# ===========================================================================
Write-Host ''
Write-Host '  -- Section C: Review-PaperSmokeEvidence.ps1 ledger classifier static assertions --'

Assert-True (Test-Path $ReviewScript) "Review-PaperSmokeEvidence.ps1 exists at $ReviewScript"

$ReviewContent = $null
if (Test-Path $ReviewScript) {
    $ReviewContent = Get-Content $ReviewScript -Raw

    foreach ($label in @(
        'AAPL-NATURAL-SELL-LIFECYCLE-CLOSED',
        'PAPER-SAFE-FLATTEN-ROUTE-MARKET-PROOF-CLOSED',
        'PAPER-FULL-BUY-SELL-LIFECYCLE-CLOSED',
        'GUI-TRADE-LIFECYCLE-VISIBILITY-CLOSED',
        'DISCORD-TRADE-LIFECYCLE-REAL-CLOSED',
        'REPEATED-AUTONOMOUS-TRADE-CYCLE-CLOSED'
    )) {
        Assert-True ($ReviewContent -match [regex]::Escape($label)) "Contains ledger-specific label $label"
    }

    Assert-True ($ReviewContent -match 'function New-LedgerVerdict')        'Contains New-LedgerVerdict helper function'
    Assert-True ($ReviewContent -match '\$ledger_verdicts\s*=\s*\[ordered\]@\{') 'Composes $ledger_verdicts as an ordered table'
    Assert-True ($ReviewContent -match 'ledger_classifications\s*=\s*\$ledger_verdicts') 'JSON output includes ledger_classifications'
    Assert-True ($ReviewContent -match 'ledger_closed_labels\s*=') 'JSON output includes ledger_closed_labels'
    Assert-True ($ReviewContent -match '\$ledger_withheld_status') 'Computes a withheld-status value shared by open ledger items'
    Assert-True ($ReviewContent -match "'OPEN'" -and $ReviewContent -match "'MARKET-PROOF-PENDING'" -and $ReviewContent -match "'PARTIAL'") `
        'Withheld-status distinguishes OPEN / MARKET-PROOF-PENDING / PARTIAL'

    # AAPL-specific narrowing: must not just reuse the generic lifecycle gate.
    Assert-True ($ReviewContent -match '\$aapl_symbol_confirmed\s*=') 'AAPL-sell gate computes an explicit aapl_symbol_confirmed flag'
    Assert-True ($ReviewContent -match "aapl_sell_missing\.AddRange\(\[string\[\]\]@\(\`$lifecycle_missing_requirements\)\)") `
        'AAPL-sell gate starts from the strict lifecycle missing-requirements list (never weaker than the generic gate)'
    Assert-True ($ReviewContent -match "aapl_sell_missing\.Add\('aapl_symbol_confirmed_in_trade_evidence'\)") `
        'AAPL-sell gate adds its own symbol-confirmation requirement on top of the generic gate'

    # Full buy->sell cycle: must require an OBSERVED buy leg, not a pre-existing baseline.
    Assert-True ($ReviewContent -match '\$buy_order_detected\s*=\s*\$false') 'Full-cycle gate scans for an observed buy/long order'
    Assert-True ($ReviewContent -match "full_cycle_missing_requirements\.Add\('buy_or_long_order_detected_in_evidence'\)") `
        'Full-cycle gate requires buy_or_long_order_detected_in_evidence explicitly'
    Assert-False ($ReviewContent -match '\$full_buy_sell_cycle_proven\s*=\s*\$trade_lifecycle_detected\b') `
        'Full-cycle gate is NOT a simple alias of the generic strict lifecycle gate'

    # Safe-flatten route: must require positively-matched flatten.requested evidence.
    Assert-True ($ReviewContent -match '\$flatten_requested_evidence_detected\s*=\s*\$false') 'Safe-flatten gate computes an explicit flatten_requested_evidence_detected flag'
    Assert-True ($ReviewContent -match [regex]::Escape('(?i)flatten[\.\-_ ]?requested')) `
        'Safe-flatten gate positively matches a flatten.requested pattern in events/alerts evidence'

    # GUI / Discord / repeated-cycle: require the strict lifecycle gate PLUS an
    # explicit, affirmative, non-placeholder operator note -- not file presence alone.
    Assert-True ($ReviewContent -match [regex]::Escape("'notes\gui_observation.txt'"))               'GUI gate reads notes\gui_observation.txt'
    Assert-True ($ReviewContent -match [regex]::Escape("'notes\discord_observation.txt'"))           'Discord gate reads notes\discord_observation.txt'
    Assert-True ($ReviewContent -match [regex]::Escape("'notes\repeated_cycle_confirmation.txt'"))   'Repeated-cycle gate reads notes\repeated_cycle_confirmation.txt'
    Assert-True ($ReviewContent -match '\$DISCORD_STAGE_KEYS_REQUIRED\s*=\s*@\(') 'Declares DISCORD_STAGE_KEYS_REQUIRED stage-key list'
    foreach ($stageKey in @('autonomous.run.start', 'signal.admitted', 'order.submitted', 'order.acked', 'flatten.requested', 'reconcile.clean')) {
        Assert-True ($ReviewContent -match [regex]::Escape("'$stageKey'")) "DISCORD_STAGE_KEYS_REQUIRED includes '$stageKey'"
    }
    Assert-True ($ReviewContent -match [regex]::Escape('not\s+(yet\s+)?(observed|confirmed')) `
        'GUI/Discord observation parsing rejects "not (yet) observed/confirmed" placeholder language'

    # Repeated-cycle label: explicitly designed to remain withheld absent cross-session proof.
    Assert-True ($ReviewContent -match '(?i)cross-session proof') 'Repeated-cycle gate documents that this is inherently cross-session proof'
    Assert-True ($ReviewContent -match [regex]::Escape('only\s+(one|1)\s+session')) `
        'Repeated-cycle gate treats "only one session" language as a disqualifying negative marker'
    Assert-True ($ReviewContent -match 'gui_visibility_missing\.Add\(.*strict_trade_lifecycle_proven') 'GUI gate requires the strict trade lifecycle gate as a precondition'
    Assert-True ($ReviewContent -match 'discord_lifecycle_missing\.Add\(.*strict_trade_lifecycle_proven') 'Discord gate requires the strict trade lifecycle gate as a precondition'
    Assert-True ($ReviewContent -match 'repeated_cycle_missing\.Add\(.*strict_trade_lifecycle_proven') 'Repeated-cycle gate requires the strict trade lifecycle gate as a precondition'
}

# ===========================================================================
# Section D: Fixture-based runtime proofs of ledger-classifier strictness
# ===========================================================================
Write-Host ''
Write-Host '  -- Section D: ledger-classifier fixture proofs (positive + negative) --'

function Write-FixtureJson {
    param([string]$Path, $Value)
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    $Value | ConvertTo-Json -Depth 10 | Set-Content -Path $Path -Encoding UTF8
}

function Write-FixtureText {
    param([string]$Path, [string]$Value)
    $parent = Split-Path -Parent $Path
    if (-not (Test-Path $parent)) { New-Item -ItemType Directory -Path $parent -Force | Out-Null }
    $Value | Set-Content -Path $Path -Encoding UTF8
}

function Invoke-LedgerReviewFixture {
    param([string]$Path)
    $output = & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $ReviewScript -EvidencePath $Path -WriteSummary 2>&1
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

# A single strict, complete trade-lifecycle fixture builder, parameterized so
# the same proven "everything strict passes" base can be narrowly varied along
# exactly the axes each ledger label gates on (symbol identity, an observed
# buy leg, flatten.requested evidence, and the three affirmative operator
# notes) -- proving both the withhold (negative) and the close (positive)
# side of each label's gate from one consistent foundation.
function New-LedgerProofFixture {
    param(
        [string]$Name,
        [string]$Symbol           = 'AAPL',
        [bool]  $IncludeBuyLeg    = $false,
        [bool]  $FlattenEvidence  = $false,
        [string]$GuiNote              = $null,
        [string]$DiscordNote          = $null,
        [string]$RepeatedCycleNote    = $null
    )

    $casePath = Join-Path $script:LedgerFixtureRoot $Name
    New-Item -ItemType Directory -Path (Join-Path $casePath 'api') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $casePath 'db') -Force | Out-Null
    New-Item -ItemType Directory -Path (Join-Path $casePath 'notes') -Force | Out-Null

    $flowRows = [System.Collections.ArrayList]::new()
    if ($IncludeBuyLeg) {
        [void]$flowRows.Add([ordered]@{ stage = 'outbox_enqueued';   source_table = 'oms_outbox';           symbol = $Symbol; side = 'buy'; message = "Buy order enqueued through normal path for $Symbol" })
        [void]$flowRows.Add([ordered]@{ stage = 'broker_ack';        source_table = 'oms_inbox';            symbol = $Symbol; side = 'buy'; message = 'Broker ACK accepted for buy leg' })
        [void]$flowRows.Add([ordered]@{ stage = 'broker_final_fill'; source_table = 'fill_quality_telemetry'; symbol = $Symbol; side = 'buy'; message = "Buy leg fill received for $Symbol" })
    }
    [void]$flowRows.Add([ordered]@{ stage = 'outbox_enqueued';   source_table = 'oms_outbox';             symbol = $Symbol; side = 'sell'; message = "Sell order enqueued through normal path for $Symbol" })
    [void]$flowRows.Add([ordered]@{ stage = 'broker_sent';       source_table = 'oms_outbox';             symbol = $Symbol; side = 'sell'; message = "Sell order submitted to broker for $Symbol" })
    [void]$flowRows.Add([ordered]@{ stage = 'broker_ack';        source_table = 'oms_inbox';              symbol = $Symbol; side = 'sell'; message = 'Broker ACK accepted' })
    [void]$flowRows.Add([ordered]@{ stage = 'broker_final_fill'; source_table = 'fill_quality_telemetry'; symbol = $Symbol;               message = "Final fill received for $Symbol" })
    [void]$flowRows.Add([ordered]@{ stage = 'fill_applied';      source_table = 'oms_inbox';              symbol = $Symbol;               message = 'Fill applied to durable execution state' })

    $orderRows = [System.Collections.ArrayList]::new()
    if ($IncludeBuyLeg) {
        [void]$orderRows.Add([ordered]@{ symbol = $Symbol; side = 'buy'; status = 'filled' })
    }
    [void]$orderRows.Add([ordered]@{ symbol = $Symbol; side = 'sell'; status = 'submitted' })

    $eventsFeedRows = [System.Collections.ArrayList]::new()
    [void]$eventsFeedRows.Add([ordered]@{ kind = 'runtime_transition'; detail = 'RUNNING'; run_id = 'ledger-fixture-run' })
    if ($FlattenEvidence) {
        [void]$eventsFeedRows.Add([ordered]@{ kind = 'alert'; detail = 'flatten.requested issued by autonomous session guard'; run_id = 'ledger-fixture-run' })
    }

    Write-FixtureJson (Join-Path $casePath 'api\system_status.json') ([ordered]@{
        daemon_mode = 'paper'
        runtime_status = 'running'
        strategy_armed = $true
        execution_armed = $true
        kill_switch_active = $false
        live_routing_enabled = $false
        integrity_halt_active = $false
        risk_halt_active = $false
        reconcile_status = 'ok'
        fault_signals = @()
    })
    Write-FixtureJson (Join-Path $casePath 'api\system_preflight.json') ([ordered]@{
        live_routing_disabled = $true
    })
    Write-FixtureJson (Join-Path $casePath 'api\autonomous_readiness.json') ([ordered]@{
        arm_state = 'armed'
        reconcile_ready = $true
        signal_ingestion_configured = $true
        last_bar_signal_qty = -1
        market_data_freshness = [ordered]@{
            completed_rows = 5
            freshness_state = 'ok'
        }
    })
    Write-FixtureJson (Join-Path $casePath 'api\events_feed.json') ([ordered]@{ rows = @($eventsFeedRows.ToArray()) })
    Write-FixtureJson (Join-Path $casePath 'api\oms_overview.json') ([ordered]@{
        run_id = 'ledger-fixture-run'
        position_count = 0
        open_order_count = 0
        fill_count = 1
        execution_active_orders = 0
        execution_pending_orders = 0
        reconcile_total_mismatches = 0
    })
    Write-FixtureJson (Join-Path $casePath 'api\reconcile_status.json') ([ordered]@{
        status = 'ok'
        mismatched_positions = 0
        mismatched_orders = 0
        mismatched_fills = 0
        unmatched_broker_events = 0
    })
    Write-FixtureJson (Join-Path $casePath 'api\reconcile_mismatches.json') ([ordered]@{ rows = @() })
    Write-FixtureJson (Join-Path $casePath 'api\execution_flow.json') ([ordered]@{ rows = @($flowRows.ToArray()) })
    Write-FixtureJson (Join-Path $casePath 'api\execution_orders.json') ([ordered]@{ rows = @($orderRows.ToArray()) })
    Write-FixtureJson (Join-Path $casePath 'api\execution_outbox.json') ([ordered]@{
        rows = @([ordered]@{ symbol = $Symbol; side = 'sell'; status = 'sent' })
    })
    Write-FixtureJson (Join-Path $casePath 'api\fill_quality.json') ([ordered]@{
        rows = @([ordered]@{ symbol = $Symbol; fill_qty = 1 })
    })
    Write-FixtureJson (Join-Path $casePath 'api\portfolio_fills.json') ([ordered]@{
        rows = @([ordered]@{ symbol = $Symbol; qty = 1 })
    })
    Write-FixtureJson (Join-Path $casePath 'api\portfolio_positions.json') ([ordered]@{ rows = @() })

    Write-FixtureText (Join-Path $casePath 'db\oms_inbox_recent.txt') @"
 id | order_id | event_type | applied | created_at
 1 | order-1 | broker_ack | t | 2026-06-07T00:00:01Z
 2 | order-1 | final_fill | t | 2026-06-07T00:00:02Z
"@
    Write-FixtureText (Join-Path $casePath 'db\broker_order_map_recent.txt') @"
 order_id | broker_order_id | created_at
 order-1 | broker-1 | 2026-06-07T00:00:00Z
(1 row)
"@
    Write-FixtureText (Join-Path $casePath 'notes\smoke_lifecycle_checklist.txt') '# [YES] Broker position baseline adopted (positions=1, orders=0, reconcile_after=ok)'

    if ($null -ne $GuiNote)           { Write-FixtureText (Join-Path $casePath 'notes\gui_observation.txt') $GuiNote }
    if ($null -ne $DiscordNote)       { Write-FixtureText (Join-Path $casePath 'notes\discord_observation.txt') $DiscordNote }
    if ($null -ne $RepeatedCycleNote) { Write-FixtureText (Join-Path $casePath 'notes\repeated_cycle_confirmation.txt') $RepeatedCycleNote }

    return $casePath
}

function Get-LedgerVerdict {
    param($Review, [string]$Key)
    if ($null -eq $Review) { return $null }
    $lc = $Review.ledger_classifications
    if ($null -eq $lc) { return $null }
    return (Get-Field $lc $Key)
}

function Get-Field {
    param($Obj, [string]$Field)
    if ($null -eq $Obj) { return $null }
    $p = $Obj.PSObject.Properties[$Field]
    if ($null -eq $p) { return $null }
    return $p.Value
}

$discordAffirmativeNote = @'
Discord observation: all 7 stage messages confirmed in #trading-alerts.
Observed live: autonomous.run.start, signal.admitted, order.submitted,
order.acked, fill.partial, fill.terminal, flatten.requested, reconcile.clean.
All 7 stage messages were received and verified in the correct order.
'@

$discordPlaceholderNote = @'
Discord observation: TODO -- paste a screenshot of each stage message here.
autonomous.run.start: [pending]. signal.admitted: [pending].
order.submitted: [pending]. order.acked: [pending].
flatten.requested: [pending]. reconcile.clean: [pending]. fill.partial: [pending].
Not yet confirmed -- placeholder template, fill in after the session.
'@

$guiAffirmativeNote = @'
GUI observation: captured a screenshot of the filled order in the operator
console trade-flow panel. The fill was clearly visible and confirmed on screen.
'@

$guiPlaceholderNote = @'
GUI observation: TODO -- take a screenshot of a filled order in the console
and confirm it is visible here. Not yet observed; placeholder template.
'@

$repeatedCycleAffirmativeNote = @'
Repeated autonomous cycle confirmation: two consecutive sessions were confirmed.
Session #1 started cleanly and stopped cleanly, completing a full natural
sell-side lifecycle. Session #2 started cleanly and stopped cleanly the
following trading day, completing another full natural sell-side lifecycle.
Both sessions are independently verified end to end.
'@

$repeatedCycleOnlyOneNote = @'
Repeated cycle note: only one session has been completed so far.
Session #1 started cleanly and stopped cleanly. A second session has not yet
been observed -- pending confirmation of two consecutive sessions.
'@

$script:LedgerFixtureRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_ledger_guard_" + [guid]::NewGuid().ToString('N'))
try {
    if (-not (Test-Path $ReviewScript)) {
        Write-Host '  FATAL: Review-PaperSmokeEvidence.ps1 missing -- skipping Section D fixture proofs' -ForegroundColor Red
    } else {
        # --- D1: base AAPL strict-lifecycle fixture, no extra evidence -------
        # Proves: the ledger-specific gates do NOT close just because the
        # generic NATURAL-TRADE-LIFECYCLE-CLOSED gate closed. Only the label
        # whose narrowed evidence is genuinely present (AAPL symbol) closes.
        $base = Invoke-LedgerReviewFixture (New-LedgerProofFixture -Name 'ledger_base')
        Assert-True ($null -ne $base -and $base.classification -eq 'NATURAL-TRADE-LIFECYCLE-CLOSED') `
            'D1: base fixture proves the generic strict lifecycle gate (sanity precondition)'

        $d1Aapl     = Get-LedgerVerdict $base 'aapl_natural_sell_lifecycle'
        $d1Flatten  = Get-LedgerVerdict $base 'paper_safe_flatten_route'
        $d1FullCyc  = Get-LedgerVerdict $base 'paper_full_buy_sell_lifecycle'
        $d1Gui      = Get-LedgerVerdict $base 'gui_trade_lifecycle_visibility'
        $d1Discord  = Get-LedgerVerdict $base 'discord_trade_lifecycle_real'
        $d1Repeat   = Get-LedgerVerdict $base 'repeated_autonomous_trade_cycle'

        Assert-True  ($null -ne $d1Aapl -and $d1Aapl.closed -eq $true) `
            'D1: AAPL-NATURAL-SELL-LIFECYCLE-CLOSED closes when the traded symbol is confirmed AAPL'
        Assert-False ($null -ne $d1Flatten -and $d1Flatten.closed)  'D1: PAPER-SAFE-FLATTEN-ROUTE stays withheld without flatten.requested evidence'
        Assert-False ($null -ne $d1FullCyc -and $d1FullCyc.closed)  'D1: PAPER-FULL-BUY-SELL-LIFECYCLE stays withheld without an observed buy leg'
        Assert-False ($null -ne $d1Gui     -and $d1Gui.closed)      'D1: GUI-TRADE-LIFECYCLE-VISIBILITY stays withheld without notes/gui_observation.txt'
        Assert-False ($null -ne $d1Discord -and $d1Discord.closed)  'D1: DISCORD-TRADE-LIFECYCLE-REAL stays withheld without notes/discord_observation.txt'
        Assert-False ($null -ne $d1Repeat  -and $d1Repeat.closed)   'D1: REPEATED-AUTONOMOUS-TRADE-CYCLE stays withheld without notes/repeated_cycle_confirmation.txt'

        Assert-True ($null -ne $d1FullCyc -and (@($d1FullCyc.missing_requirements) -contains 'buy_or_long_order_detected_in_evidence')) `
            'D1: full-cycle gate names the missing observed-buy requirement explicitly'
        Assert-True ($null -ne $d1Flatten -and (@($d1Flatten.missing_requirements) -match 'flatten_requested_evidence_detected').Count -gt 0) `
            'D1: safe-flatten gate names the missing flatten.requested evidence requirement explicitly'

        if ($null -ne $base -and $null -ne $base.PSObject.Properties['ledger_closed_labels']) {
            $closedLabels = @($base.ledger_closed_labels)
            Assert-True  ($closedLabels -contains 'AAPL-NATURAL-SELL-LIFECYCLE-CLOSED') 'D1: ledger_closed_labels includes AAPL-NATURAL-SELL-LIFECYCLE-CLOSED'
            Assert-False ($closedLabels -contains 'PAPER-FULL-BUY-SELL-LIFECYCLE-CLOSED')      'D1: ledger_closed_labels excludes PAPER-FULL-BUY-SELL-LIFECYCLE-CLOSED'
            Assert-False ($closedLabels -contains 'GUI-TRADE-LIFECYCLE-VISIBILITY-CLOSED')     'D1: ledger_closed_labels excludes GUI-TRADE-LIFECYCLE-VISIBILITY-CLOSED'
            Assert-False ($closedLabels -contains 'DISCORD-TRADE-LIFECYCLE-REAL-CLOSED')       'D1: ledger_closed_labels excludes DISCORD-TRADE-LIFECYCLE-REAL-CLOSED'
            Assert-False ($closedLabels -contains 'REPEATED-AUTONOMOUS-TRADE-CYCLE-CLOSED')    'D1: ledger_closed_labels excludes REPEATED-AUTONOMOUS-TRADE-CYCLE-CLOSED'
            Assert-False ($closedLabels -contains 'PAPER-SAFE-FLATTEN-ROUTE-MARKET-PROOF-CLOSED') 'D1: ledger_closed_labels excludes PAPER-SAFE-FLATTEN-ROUTE-MARKET-PROOF-CLOSED'
        } else {
            Assert-True $false 'D1: review_summary.json carries ledger_closed_labels'
        }

        # --- D2: identical strict lifecycle, but traded symbol is NOT AAPL ---
        # Proves the AAPL-specific label is narrower than (not an alias of) the
        # generic lifecycle gate: a fully-proven non-AAPL lifecycle still closes
        # NATURAL-TRADE-LIFECYCLE-CLOSED, but must NOT close the AAPL-specific label.
        $nonAapl = Invoke-LedgerReviewFixture (New-LedgerProofFixture -Name 'ledger_non_aapl' -Symbol 'MSFT')
        Assert-True ($null -ne $nonAapl -and $nonAapl.classification -eq 'NATURAL-TRADE-LIFECYCLE-CLOSED') `
            'D2: non-AAPL fixture still proves the generic strict lifecycle gate (sanity precondition)'
        $d2Aapl = Get-LedgerVerdict $nonAapl 'aapl_natural_sell_lifecycle'
        Assert-False ($null -ne $d2Aapl -and $d2Aapl.closed) `
            'D2: AAPL-NATURAL-SELL-LIFECYCLE-CLOSED stays withheld when the proven lifecycle traded a different symbol (MSFT)'
        Assert-True ($null -ne $d2Aapl -and (@($d2Aapl.missing_requirements) -contains 'aapl_symbol_confirmed_in_trade_evidence')) `
            'D2: AAPL gate names the missing aapl_symbol_confirmed_in_trade_evidence requirement explicitly'

        # --- D3: observed buy leg + flatten.requested evidence on top of base -
        # Proves these two labels DO close once their specific narrow evidence
        # is genuinely present (not just file-presence -- real positive content).
        $fullProof = Invoke-LedgerReviewFixture (New-LedgerProofFixture -Name 'ledger_full_and_flatten' -IncludeBuyLeg $true -FlattenEvidence $true)
        $d3FullCyc = Get-LedgerVerdict $fullProof 'paper_full_buy_sell_lifecycle'
        $d3Flatten = Get-LedgerVerdict $fullProof 'paper_safe_flatten_route'
        Assert-True ($null -ne $d3FullCyc -and $d3FullCyc.closed -eq $true) `
            'D3: PAPER-FULL-BUY-SELL-LIFECYCLE-CLOSED closes once an observed buy leg is genuinely present in evidence'
        Assert-True ($null -ne $d3Flatten -and $d3Flatten.closed -eq $true) `
            'D3: PAPER-SAFE-FLATTEN-ROUTE-MARKET-PROOF-CLOSED closes once positively-matched flatten.requested evidence is present'

        # --- D4: placeholder/incomplete operator notes do NOT close GUI/Discord/repeated-cycle
        # Proves file presence alone (or template/placeholder language) is rejected.
        $negNotes = Invoke-LedgerReviewFixture (New-LedgerProofFixture -Name 'ledger_negative_notes' `
            -GuiNote $guiPlaceholderNote -DiscordNote $discordPlaceholderNote -RepeatedCycleNote $repeatedCycleOnlyOneNote)
        $d4Gui     = Get-LedgerVerdict $negNotes 'gui_trade_lifecycle_visibility'
        $d4Discord = Get-LedgerVerdict $negNotes 'discord_trade_lifecycle_real'
        $d4Repeat  = Get-LedgerVerdict $negNotes 'repeated_autonomous_trade_cycle'
        Assert-False ($null -ne $d4Gui     -and $d4Gui.closed)     'D4: GUI label stays withheld given a TODO/placeholder gui_observation.txt (file presence is not proof)'
        Assert-False ($null -ne $d4Discord -and $d4Discord.closed) 'D4: Discord label stays withheld given a TODO/placeholder discord_observation.txt with negative markers'
        Assert-False ($null -ne $d4Repeat  -and $d4Repeat.closed)  'D4: Repeated-cycle label stays withheld given an explicit "only one session" note'

        # --- D5: explicit, affirmative, fully-compliant operator notes -------
        # Proves the labels CAN close given genuine, structured, affirmative
        # operator-authored confirmation -- the strictness has a real path to
        # closure, it is not permanently unsatisfiable.
        $posNotes = Invoke-LedgerReviewFixture (New-LedgerProofFixture -Name 'ledger_affirmative_notes' `
            -GuiNote $guiAffirmativeNote -DiscordNote $discordAffirmativeNote -RepeatedCycleNote $repeatedCycleAffirmativeNote)
        $d5Gui     = Get-LedgerVerdict $posNotes 'gui_trade_lifecycle_visibility'
        $d5Discord = Get-LedgerVerdict $posNotes 'discord_trade_lifecycle_real'
        $d5Repeat  = Get-LedgerVerdict $posNotes 'repeated_autonomous_trade_cycle'
        Assert-True ($null -ne $d5Gui     -and $d5Gui.closed -eq $true)     'D5: GUI-TRADE-LIFECYCLE-VISIBILITY-CLOSED closes given an explicit affirmative gui_observation.txt'
        Assert-True ($null -ne $d5Discord -and $d5Discord.closed -eq $true) 'D5: DISCORD-TRADE-LIFECYCLE-REAL-CLOSED closes given all 7 stage keys explicitly confirmed'
        Assert-True ($null -ne $d5Repeat  -and $d5Repeat.closed -eq $true)  'D5: REPEATED-AUTONOMOUS-TRADE-CYCLE-CLOSED closes given an explicit two-session confirmation note'
    }
} catch {
    Write-Host "  FAIL: Ledger classifier fixture tests threw: $_" -ForegroundColor Red
    $script:Failures++
} finally {
    if (Test-Path $script:LedgerFixtureRoot) {
        Remove-Item -LiteralPath $script:LedgerFixtureRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
}

Write-Host ''
if ($Failures -eq 0) {
    Write-Host '  ALL ASSERTIONS PASSED (test_paper_smoke_ledger_and_shutdown)' -ForegroundColor Green
    exit 0
} else {
    Write-Host "  $Failures ASSERTION(S) FAILED (test_paper_smoke_ledger_and_shutdown)" -ForegroundColor Red
    exit 1
}
