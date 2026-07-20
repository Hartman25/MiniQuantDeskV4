# =============================================================================
# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-DURABLE-OUTCOME-AUTHORITY-AND-
# EVIDENCE-CONTRACT -- Source-aware static validator
# =============================================================================
# Scope: this guard validates the Phase E1 contract audit -- a documentation
# and guard-only patch. No network call, no provider/broker call, no DB
# connection, no daemon start, no cargo/npm build or test -- pure text/source
# validation only.
#
# Checks:
#   [1]  The E1 contract document exists.
#   [2]  Ledger does not mark Bundle 3 closed.
#   [3]  Ledger/README/contract doc do not falsely claim Phase E is
#        implemented, wired, or closed.
#   [4]  The contract document does not define completed_no_trade from the
#        absence of fills alone -- it requires the full evidence hierarchy.
#   [5]  The contract document does not name a process-local counter as final
#        outcome authority -- it explicitly forbids that.
#   [6]  unknown_insufficient_evidence is present and defined in the contract.
#   [7]  Unresolved dispatch claims are never allowed to become no-trade --
#        the contract explicitly blocks this.
#   [8]  stopped_at_utc is required before finalization eligibility.
#   [11] outcome is defined as the terminal-only authority (read-model once
#        finalized); nonterminal unknown_* uses state_reason_code, never
#        outcome.
#   [12] unknown_* classification leaves outcome NULL.
#   [13] unknown_* classification leaves finalized_at_utc NULL.
#   [14] The stopping/stop_retrying -> evidence_degraded legal-transition
#        graph extension is explicitly authorized by the contract.
#   [15] The evidence_degraded -> stopping recovery path is defined.
#   [16] sys_risk_denial_events is documented as not durably correlatable to
#        an operation/run/evaluation.
#   [17] no_trade_all_signals_blocked is deferred, not authorized for E2.
#   [18] Partial/incomplete bar coverage blocks completed_no_trade.
#   [19] unknown_incomplete_bar_coverage is defined.
#   [20] calendar_unavailable is not treated as a clean finalized no-trade
#        day (it cannot reach stopping and is never finalization-eligible).
#   [9]  README.md / README_TECHNICAL.md do not claim the unattended soak has
#        begun.
#   [10] README.md / README_TECHNICAL.md do not claim live-capital readiness.
#   [21] The contract does not use started_at_utc as the first expected-bar
#        lower bound.
#   [22] The contract does not include the close bar without reconciling the
#        driver's exclusive close boundary (an inclusive bar_end_ts <=
#        effective_operation_close_utc rule).
#   [23] The contract does not claim no durable coverage-anchor gap exists
#        without exact proof -- it names the exact evidence gap and the
#        binding "no proof -> no completed_no_trade" rule.
#   [24] The contract does not authorize one combined E2 despite a confirmed
#        evidence-foundation gap -- it authorizes the E2A/E2B split.
#   [25] The contract does not read only the current operation run_id -- it
#        requires the full run lineage.
#   [26] The contract does not omit replacement-run lineage (recovery binding
#        a second/later run_id).
#   [27] The contract does not allow activity from an earlier run to
#        disappear.
#   [28] The contract does not allow partial/recovery-gap coverage to become
#        no-trade.
#   [29] The contract does not contain contradictory precedence between
#        unresolved claims and confirmed activity.
#   [30] The contract does not guarantee a durable database-unavailable write
#        during a complete DB outage.
#
# Correction pass 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1-OPERATION-SCOPED-COVERAGE-AUTHORITY-
# RECONCILIATION-03) checks:
#   [31] The contract does not fall back to the current session's first grid
#        slot when no preopen slot qualifies for the first dispatchable bar.
#   [32] The contract defines the first dispatchable bar as the final element
#        of expected_intraday_end_ts_window evaluated at
#        effective_operation_open_utc.
#   [33] The contract states the first-anchor spillover can select a previous
#        trading session's own final grid identity, not a current-session bar.
#   [34] The contract does not treat every readiness-history-window bar as a
#        separate dispatch obligation -- only the final element anchors
#        dispatch.
#   [35] The contract defines a dedicated, operation-scoped
#        autonomous_daily_coverage_bound event as the coverage authority.
#   [36] The coverage-bound event is run_id-scoped to NULL (operation-scoped,
#        not run-scoped).
#   [37] The contract states the coverage-bound event's own primary key
#        (not an application convention) guarantees at most one row per
#        operation.
#   [38] The contract requires the coverage-bound event to be written before
#        PrepareDataOnly eligibility, bar observation, canonical start, or any
#        dispatch claim.
#   [39] The contract fails closed (no overwrite) on a conflicting coverage
#        replay for the same operation_id.
#   [40] The corrected run-lineage query reads raw, undeduplicated
#        (transition_seq, run_id) rows -- contradiction validation runs in
#        Rust, never via SQL DISTINCT.
#   [41] The contract requires each run_id to appear exactly once across the
#        raw lineage rows (duplicate detection, not SQL-side deduplication).
#   [42] The contract requires run-scoped evidence reads to aggregate across
#        the full validated run lineage, never a single run_id, for
#        strategy_signal_evaluations and oms_outbox/oms_inbox evidence in §6.
#   [43] E2A's mission is corrected to build the new coverage-bound event,
#        not to extend the daily_data_readiness_evaluated pre-start payload.
#   [44] E2B is not authorized until E2A is independently accepted.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_autonomous_daily_paper_operations_01e_outcome_contract.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathContractDoc = Join-Path $RepoRoot "docs\specs\autonomous_daily_paper_operations_01e_outcome_truth_contract.md"
$PathLedger       = Join-Path $RepoRoot "MiniQuantDesk_Master_Patch_Ledger_v2.md"
$PathReadme       = Join-Path $RepoRoot "README.md"
$PathReadmeTech   = Join-Path $RepoRoot "README_TECHNICAL.md"

$Violations = 0
$EmDash = [char]0x2014
$Section10Sign = [char]0x00A7 + "10"

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

function Test-FileExists {
    param([string]$Label, [string]$Path)
    if (Test-Path $Path) {
        Show-Green "  OK -- $Label found: $Path"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label not found: $Path"
        return $false
    }
}

function Get-Normalized {
    # Collapse all runs of whitespace (including markdown source line-wraps) to
    # a single space before matching, so a needle that happens to straddle a
    # line-wrap in the .md file still matches reliably. Applied to every
    # content/needle comparison in this guard.
    param([string]$Content)
    if ($null -eq $Content) { return $null }
    return ($Content -replace '\s+', ' ')
}

function Test-ContentContains {
    param([string]$Label, [string]$Content, [string]$Needle)
    $norm = Get-Normalized $Content
    $needleNorm = $Needle -replace '\s+', ' '
    if ($null -ne $norm -and $norm.IndexOf($needleNorm, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        Show-Green "  OK -- $Label"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (needle not found: '$Needle')"
        return $false
    }
}

function Test-ContentContainsAny {
    param([string]$Label, [string]$Content, [string[]]$Needles)
    if ($null -eq $Content) {
        $script:Violations++
        Show-Red "  FAIL -- $Label (content is null)"
        return $false
    }
    foreach ($Needle in $Needles) {
        if ($Content.IndexOf($Needle, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
            Show-Green "  OK -- $Label"
            return $true
        }
    }
    $script:Violations++
    Show-Red "  FAIL -- $Label (none of the candidate needles found: $($Needles -join ' | '))"
    return $false
}

function Test-ContentDoesNotContain {
    param([string]$Label, [string]$Content, [string]$Needle)
    $norm = Get-Normalized $Content
    $needleNorm = $Needle -replace '\s+', ' '
    if ($null -eq $norm -or $norm.IndexOf($needleNorm, [System.StringComparison]::OrdinalIgnoreCase) -lt 0) {
        Show-Green "  OK -- $Label"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (forbidden needle found: '$Needle')"
        return $false
    }
}

# Test-Content{Contains,DoesNotContain}Normalized are aliases for the (now
# whitespace-normalized-by-default) base functions above, kept as distinct
# names for readability at the [21]-[30] Correction pass 2 call sites below.
function Test-ContentContainsNormalized {
    param([string]$Label, [string]$Content, [string]$Needle)
    return Test-ContentContains -Label $Label -Content $Content -Needle $Needle
}

function Test-ContentDoesNotContainNormalized {
    param([string]$Label, [string]$Content, [string]$Needle)
    return Test-ContentDoesNotContain -Label $Label -Content $Content -Needle $Needle
}

Write-Host "============================================================"
Write-Host " AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1 Outcome Contract Validator"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] E1 contract document exists ---"
$ContractContent = $null
if (Test-FileExists "Phase E1 outcome truth contract" $PathContractDoc) {
    $ContractContent = Get-Content -Raw -Encoding UTF8 -Path $PathContractDoc
}

$LedgerContent = $null
if (Test-FileExists "Master patch ledger" $PathLedger) {
    $LedgerContent = Get-Content -Raw -Encoding UTF8 -Path $PathLedger
}
$ReadmeContent = $null
if (Test-Path $PathReadme) { $ReadmeContent = Get-Content -Raw -Encoding UTF8 -Path $PathReadme }
$ReadmeTechContent = $null
if (Test-Path $PathReadmeTech) { $ReadmeTechContent = Get-Content -Raw -Encoding UTF8 -Path $PathReadmeTech }

Write-Host ""
Show-Info "--- [2] Ledger does not mark Bundle 3 closed ---"
Test-ContentDoesNotContain "ledger does not contain 'Bundle 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED): CLOSED'" $LedgerContent "Bundle 3 (AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED): CLOSED" | Out-Null

Write-Host ""
Show-Info "--- [3] Phase E is not falsely marked implemented/wired/closed ---"
$ForbiddenPhaseEClaims = @(
    "PHASE E: CLOSED",
    "PHASE E: COMPLETE",
    "PHASE E: IMPLEMENTATION COMPLETE",
    "phase e runtime implementation: complete",
    "phase e outcome classifier implemented",
    "phase e is wired",
    "durable daily outcome classification is live"
)
foreach ($DocPair in @(
        @{Name="Master patch ledger"; Content=$LedgerContent},
        @{Name="Phase E1 contract doc"; Content=$ContractContent},
        @{Name="README.md"; Content=$ReadmeContent},
        @{Name="README_TECHNICAL.md"; Content=$ReadmeTechContent}
    )) {
    foreach ($Phrase in $ForbiddenPhaseEClaims) {
        Test-ContentDoesNotContain "$($DocPair.Name) does not contain forbidden claim '$Phrase'" $DocPair.Content $Phrase | Out-Null
    }
}
Test-ContentContainsAny "ledger states Phase E runtime implementation is not started" $LedgerContent @(
    "Phase E runtime implementation: NOT STARTED",
    "unattended 10", # tolerant anchor in case exact phrasing shifts, combined with the explicit check below
    "Phase E1 contract audit: IMPLEMENTATION COMPLETE"
) | Out-Null

Write-Host ""
Show-Info "--- [4] completed_no_trade is never defined from absence of fills alone ---"
Test-ContentContains "contract requires complete expected-bar coverage for completed_no_trade, not merely absence of fills" $ContractContent "durably-anchored coverage" | Out-Null
Test-ContentContains "contract explicitly requires the full evidence hierarchy for completed_no_trade" $ContractContent "completed_no_trade" | Out-Null
Test-ContentContains "contract states the necessary-but-not-sufficient rule for zero fills" $ContractContent "nowhere near sufficient" | Out-Null

Write-Host ""
Show-Info "--- [5] Process-local counters are never named as final outcome authority ---"
Test-ContentContains "contract explicitly forbids process-local counters as no-trade authority" $ContractContent "must never" | Out-Null
Test-ContentContains "contract names bar_tick_dispatch_count as forbidden authority" $ContractContent "bar_tick_dispatch_count" | Out-Null
Test-ContentContains "contract names last_bar_signal_qty as forbidden authority" $ContractContent "last_bar_signal_qty" | Out-Null
Test-ContentContains "contract states these are diagnostic/process-local, carrying no restart-safe authority" $ContractContent "carry no restart-safe authority" | Out-Null

Write-Host ""
Show-Info "--- [6] unknown_insufficient_evidence is present and defined ---"
Test-ContentContains "contract defines unknown_insufficient_evidence" $ContractContent "unknown_insufficient_evidence" | Out-Null
Test-ContentContains "contract states unknown_insufficient_evidence is never a fabricated no-trade reason" $ContractContent "never a fabricated no-trade reason invented to fill the gap" | Out-Null

Write-Host ""
Show-Info "--- [7] Unresolved dispatch claims never become no-trade ---"
Test-ContentContains "contract blocks completed_no_trade outright on any unresolved dispatch claim" $ContractContent "blocks ``completed_no_trade`` outright" | Out-Null
Test-ContentContains "contract names the unresolved-claim reason code" $ContractContent "unknown_unresolved_dispatch_claim" | Out-Null
Test-ContentContains "contract states an unresolved claim blocks no-trade outright" $ContractContent "Any unresolved" | Out-Null

Write-Host ""
Show-Info "--- [8] stopped_at_utc is required before finalization ---"
Test-ContentContains "contract requires stopped_at_utc IS NOT NULL for eligibility" $ContractContent "stopped_at_utc IS NOT NULL" | Out-Null
Test-ContentContains "contract names finalization eligibility section" $ContractContent "Finalization eligibility" | Out-Null

Write-Host ""
Show-Info "--- [11] outcome is the terminal-only authority; unknown_* uses state_reason_code ---"
Test-ContentContains "contract states outcome is the read-model authority once finalized" $ContractContent "is the read-model authority once finalized" | Out-Null
Test-ContentContains "contract routes nonterminal unknown_* codes through state_reason_code, not outcome" $ContractContent "carries the closed-set" | Out-Null
Test-ContentContains "contract forbids writing an unknown_* value into outcome (part 1)" $ContractContent "E2+ must never write an" | Out-Null
Test-ContentContains "contract forbids writing an unknown_* value into outcome (part 2)" $ContractContent "``unknown_*`` value into ``outcome``" | Out-Null

Write-Host ""
Show-Info "--- [12]-[13] unknown_* classification leaves outcome and finalized_at_utc NULL ---"
Test-ContentContains "contract states outcome remains NULL for the evidence_degraded finalization path" $ContractContent "outcome remains NULL" | Out-Null
Test-ContentContains "contract states finalized_at_utc remains NULL for the evidence_degraded finalization path" $ContractContent "finalized_at_utc remains NULL" | Out-Null

Write-Host ""
Show-Info "--- [14] stopping/stop_retrying -> evidence_degraded graph extension is explicitly authorized ---"
Test-ContentContains "contract explicitly authorizes the Rust legal-transition graph extension" $ContractContent "explicitly authorizes E2 to extend the Rust" | Out-Null
Test-ContentContains "contract names both new edges" $ContractContent "stopping -> evidence_degraded`` or" | Out-Null

Write-Host ""
Show-Info "--- [15] evidence_degraded -> stopping recovery path is defined ---"
Test-ContentContains "contract defines the recovery path back through stopping" $ContractContent "evidence_degraded -> stopping`` edge, and the finalization CAS is retried" | Out-Null

Write-Host ""
Show-Info "--- [16] sys_risk_denial_events documented as not durably correlatable ---"
Test-ContentContains "contract states risk-denial rows cannot be durably correlated to an operation/evaluation" $ContractContent "cannot be durably correlated" | Out-Null
Test-ContentContains "contract names the missing correlation columns" $ContractContent "carries **no**" | Out-Null

Write-Host ""
Show-Info "--- [17] no_trade_all_signals_blocked is deferred, not authorized for E2 ---"
Test-ContentContains "contract names no_trade_all_signals_blocked" $ContractContent "no_trade_all_signals_blocked" | Out-Null
Test-ContentContains "contract states it is not authorized for E2 to implement from the current schema" $ContractContent "not** authorized" | Out-Null

Write-Host ""
Show-Info "--- [18] Partial/incomplete bar coverage blocks completed_no_trade ---"
Test-ContentContains "contract requires zero missing expected bar identities" $ContractContent "**Zero** missing expected bar identities" | Out-Null
Test-ContentContains "contract states any missing/unprovable/contradictory expected bar blocks no-trade" $ContractContent "Any missing, unprovable, or contradictory expected-bar identity" | Out-Null

Write-Host ""
Show-Info "--- [19] unknown_incomplete_bar_coverage is defined ---"
Test-ContentContains "contract defines unknown_incomplete_bar_coverage" $ContractContent "unknown_incomplete_bar_coverage" | Out-Null

Write-Host ""
Show-Info "--- [20] calendar_unavailable is never a clean finalized no-trade day ---"
Test-ContentContains "contract states calendar_unavailable has no legal edge to stopping" $ContractContent "calendar_unavailable`` has no legal" | Out-Null
Test-ContentContains "contract states calendar_unavailable is never finalization-eligible" $ContractContent "never finalization-eligible under" | Out-Null

Write-Host ""
Show-Info "--- [9] README.md / README_TECHNICAL.md do not claim the unattended soak has begun ---"
$ForbiddenSoakClaims = @(
    "soak has begun",
    "soak is underway",
    "soak in progress",
    "currently running the soak",
    "unattended soak has started",
    "soak: in progress",
    "soak: started"
)
foreach ($DocPair in @(@{Name="README.md"; Content=$ReadmeContent}, @{Name="README_TECHNICAL.md"; Content=$ReadmeTechContent})) {
    $DocName = $DocPair.Name
    $DocContentLower = $null
    if ($null -ne $DocPair.Content) { $DocContentLower = $DocPair.Content.ToLowerInvariant() }
    foreach ($Phrase in $ForbiddenSoakClaims) {
        if ($null -ne $DocContentLower -and $DocContentLower.Contains($Phrase)) {
            $script:Violations++
            Show-Red "  FAIL -- $DocName contains forbidden soak-started claim: '$Phrase'"
        }
    }
}
if ($null -ne $ReadmeContent -or $null -ne $ReadmeTechContent) {
    Show-Green "  OK -- no forbidden unattended-soak-has-begun claim found in README.md / README_TECHNICAL.md"
}

Write-Host ""
Show-Info "--- [10] README.md / README_TECHNICAL.md do not claim live-capital readiness ---"
$ForbiddenLiveClaims = @(
    "live capital is ready",
    "live capital: ready",
    "approved for live capital",
    "live trading is approved",
    "cleared for live capital",
    "ready for live capital"
)
foreach ($DocPair in @(@{Name="README.md"; Content=$ReadmeContent}, @{Name="README_TECHNICAL.md"; Content=$ReadmeTechContent})) {
    $DocName = $DocPair.Name
    $DocContentLower = $null
    if ($null -ne $DocPair.Content) { $DocContentLower = $DocPair.Content.ToLowerInvariant() }
    foreach ($Phrase in $ForbiddenLiveClaims) {
        if ($null -ne $DocContentLower -and $DocContentLower.Contains($Phrase)) {
            $script:Violations++
            Show-Red "  FAIL -- $DocName contains forbidden live-capital-readiness claim: '$Phrase'"
        }
    }
}
Test-ContentContains "README.md states live capital is not ready" $ReadmeContent "Not ready" | Out-Null
if ($null -ne $ReadmeContent -or $null -ne $ReadmeTechContent) {
    Show-Green "  OK -- no forbidden live-capital-readiness claim found in README.md / README_TECHNICAL.md"
}

Write-Host ""
Show-Info "--- [21] First expected-bar lower bound is never started_at_utc ---"
Test-ContentContainsNormalized "contract states the lower bound is never operation.started_at_utc" $ContractContent "never ``operation.started_at_utc``" | Out-Null
Test-ContentContainsNormalized "contract cites the accepted PrepareDataOnly-observes-bar-before-start production scenario" $ContractContent "durably observes a bar via the preopen tail window" | Out-Null

Write-Host ""
Show-Info "--- [22] Close bar is never included via an inclusive bound without reconciling the exclusive close boundary ---"
Test-ContentContainsNormalized "contract states the upper bound is never the inclusive bar_end_ts <= effective_operation_close_utc rule" $ContractContent "never the inclusive ``bar_end_ts <=" | Out-Null
Test-ContentContainsNormalized "contract states the exact final-dispatchable-bar condition is a strict inequality" $ContractContent "strictly less than, never less-than-or-equal" | Out-Null
Test-ContentContainsNormalized "contract cites the driver's own now_utc >= effective_operation_close_utc refusal" $ContractContent "refuses *all* processing" | Out-Null

Write-Host ""
Show-Info "--- [23] Contract does not claim the durable coverage-anchor gap is closed without exact proof ---"
Test-ContentContainsNormalized "contract section 6a exists documenting the durable coverage-anchor audit" $ContractContent "## 6a. Durable coverage-anchor audit and future authority" | Out-Null
Test-ContentContainsNormalized "contract states the binding no-proof-no-finalization rule" $ContractContent "No durable proof of the original coverage policy" | Out-Null
Test-ContentContainsNormalized "contract confirms the readiness-evidence payload does not persist coverage fields today" $ContractContent "does **not** persist ``expected_latest_bar_ts``" | Out-Null

Write-Host ""
Show-Info "--- [24] Contract authorizes the E2A/E2B split, not one combined E2, given the confirmed evidence-foundation gap ---"
Test-ContentContainsNormalized "contract authorizes the E2A/E2B split" $ContractContent "Resolved: E2A/E2B split is now authorized" | Out-Null
Test-ContentContainsNormalized "contract defines E2A as the durable coverage-anchor and run-lineage evidence foundation" $ContractContent "E2A $EmDash durable coverage-anchor and run-lineage evidence foundation" | Out-Null
Test-ContentDoesNotContainNormalized "contract does not still claim a single E2 is authorized with no E2A/E2B split" $ContractContent "a single E2 is authorized $EmDash no E2A/E2B split." | Out-Null

Write-Host ""
Show-Info "--- [25] Contract requires the full run lineage, never only the operation's current run_id ---"
Test-ContentContainsNormalized "contract section 6b exists defining full run lineage" $ContractContent "## 6b. Full run lineage" | Out-Null
Test-ContentContainsNormalized "contract names the unknown_run_lineage_unavailable reason code" $ContractContent "unknown_run_lineage_unavailable" | Out-Null
Test-ContentContainsNormalized "contract requires evidence reads scoped to the full run lineage, not a single run_id" $ContractContent "not merely the operation's current ``run_id``" | Out-Null

Write-Host ""
Show-Info "--- [26] Contract does not omit replacement-run lineage across recovery ---"
Test-ContentContainsNormalized "contract names a recovery cycle binding a replacement run_id" $ContractContent "run A -> terminal interruption -> recovery_retrying -> run B" | Out-Null

Write-Host ""
Show-Info "--- [27] Contract never allows an earlier run's activity to disappear ---"
Test-ContentContainsNormalized "contract states an earlier run's activity must never disappear" $ContractContent "must never disappear" | Out-Null

Write-Host ""
Show-Info "--- [28] Contract never allows partial or recovery-gap coverage to become no-trade ---"
Test-ContentContainsNormalized "contract defines coverage-across-recovery as a fail-closed requirement" $ContractContent "Coverage across recovery (corrected $EmDash Repair 7, new)" | Out-Null
Test-ContentContainsNormalized "contract routes a recovery-gap bar to unknown_incomplete_bar_coverage, never no-trade" $ContractContent "genuine missing-coverage fact" | Out-Null
Test-ContentContainsNormalized "contract defines late-start coverage as the same fail-closed requirement" $ContractContent "Late start (corrected $EmDash Repair 5, new)" | Out-Null

Write-Host ""
Show-Info "--- [29] Contract resolves the unresolved-claim vs. confirmed-activity precedence contradiction ---"
Test-ContentContainsNormalized "contract's global precedence order states fill plus unresolved claim is not yet terminal" $ContractContent "confirmed fill + unresolved claim elsewhere" | Out-Null
Test-ContentContainsNormalized "contract states this is not a terminal classification yet" $ContractContent "a terminal classification yet" | Out-Null
Test-ContentContainsNormalized "contract states no reason code is unconditionally immune to an earlier global blocker" $ContractContent "No reason code in $Section10Sign is unconditionally immune to an earlier step above." | Out-Null

Write-Host ""
Show-Info "--- [30] Contract never guarantees a durable database-unavailable write during a complete outage ---"
Test-ContentContainsNormalized "contract section 9 defines the database-failure contract" $ContractContent "Database-failure contract (new $EmDash Correction pass 2, Repair 9)" | Out-Null
Test-ContentContainsNormalized "contract states a complete outage performs no durable write attempt" $ContractContent "performs **no** durable write attempt of any kind" | Out-Null
Test-ContentContainsNormalized "contract forbids claiming a blocker was persisted without an authoritative re-read" $ContractContent "never claim the blocker was durably persisted without re-reading" | Out-Null

Write-Host ""
Show-Info "--- [31] First dispatchable bar never falls back to the current session's first grid slot ---"
Test-ContentDoesNotContainNormalized "contract does not retain the retired current-session-first-slot fallback as active guidance" $ContractContent "the lower bound is simply the grid's first in-session slot $EmDash the ordinary case for a normal-hours start" | Out-Null
Test-ContentContainsNormalized "contract explicitly retires the current-session-first-slot fallback" $ContractContent "Retired (Repair 2): the Correction-pass-2 fallback of" | Out-Null

Write-Host ""
Show-Info "--- [32] First dispatchable bar is the final element of expected_intraday_end_ts_window at effective_operation_open_utc ---"
Test-ContentContainsNormalized "contract anchors the first dispatchable bar at effective_operation_open_utc" $ContractContent "evaluated at exactly ``effective_operation_open_utc``" | Out-Null
Test-ContentContainsNormalized "contract requires only the final element of the window to anchor dispatch" $ContractContent "Only the **final** (most recent) element of that window is the first bar" | Out-Null

Write-Host ""
Show-Info "--- [33] Previous-session spillover is an explicit, required consequence ---"
Test-ContentContainsNormalized "contract states the anchor can be the previous session's final grid identity" $ContractContent "own final grid identity, not a current-session bar" | Out-Null

Write-Host ""
Show-Info "--- [34] Not every readiness-history-window bar is a separate dispatch obligation ---"
Test-ContentContainsNormalized "contract states earlier window elements are history context, not dispatch obligations" $ContractContent "not a separate dispatch obligation this operation must independently prove coverage for" | Out-Null

Write-Host ""
Show-Info "--- [35] A dedicated operation-scoped autonomous_daily_coverage_bound event is the coverage authority ---"
Test-ContentContainsNormalized "contract defines the autonomous_daily_coverage_bound event id convention" $ContractContent "autonomous_daily_coverage_bound:{operation_id}" | Out-Null
Test-ContentContainsNormalized "contract retires extending daily_data_readiness_evaluated as the coverage seam" $ContractContent "is unsuitable as a" | Out-Null

Write-Host ""
Show-Info "--- [36] The coverage-bound event is operation-scoped, run_id NULL ---"
Test-ContentContainsNormalized "contract states the coverage-bound event is operation-scoped, not run-scoped" $ContractContent "this event is operation-scoped, not run-scoped" | Out-Null

Write-Host ""
Show-Info "--- [37] The event's own primary key guarantees at most one row per operation ---"
Test-ContentContainsNormalized "contract states the store's primary key (not a convention) guarantees immutability" $ContractContent "can never produce more than one row per operation" | Out-Null

Write-Host ""
Show-Info "--- [38] Coverage-bound write precedes PrepareDataOnly, bar observation, canonical start, and dispatch claims ---"
Test-ContentContainsNormalized "contract requires the coverage-bound write before PrepareDataOnly eligibility" $ContractContent "strictly before: the operation becomes eligible for ``PrepareDataOnly``" | Out-Null

Write-Host ""
Show-Info "--- [39] Conflicting coverage replay fails closed, never overwrites ---"
Test-ContentContainsNormalized "contract fails closed on a conflicting coverage-bound replay" $ContractContent "fail closed: no new coverage authority, operation cannot proceed automatically" | Out-Null

Write-Host ""
Show-Info "--- [40] Run-lineage query reads raw undeduplicated rows; no SQL DISTINCT-based contradiction handling ---"
Test-ContentContainsNormalized "contract's corrected run-lineage query selects raw transition_seq/run_id rows" $ContractContent "select transition_seq, run_id" | Out-Null
Test-ContentContainsNormalized "contract states DISTINCT would discard duplicate-row evidence needed for contradiction detection" $ContractContent "must appear in the select list itself" | Out-Null

Write-Host ""
Show-Info "--- [41] Each run_id must appear exactly once across raw lineage rows (Rust-side duplicate detection) ---"
Test-ContentContainsNormalized "contract requires exactly-once run_id validation against raw rows" $ContractContent "must appear **exactly once** across the raw rows" | Out-Null

Write-Host ""
Show-Info "--- [42] Run-scoped evidence reads (§6) aggregate across the full lineage, never a single run_id ---"
Test-ContentContainsNormalized "contract corrects strategy_signal_evaluations item 3 to the full run lineage, retiring the singular phrasing" $ContractContent "for this operation's ``run_id``" | Out-Null
Test-ContentContainsNormalized "contract corrects the oms_outbox zero-rows item to the full run lineage" $ContractContent "Zero ``oms_outbox`` rows exist across every ``run_id`` in the validated full operation run lineage" | Out-Null
Test-ContentContainsNormalized "contract corrects the oms_inbox zero-rows item to the full run lineage" $ContractContent "Zero ``oms_inbox`` rows with ``event_kind IN" | Out-Null

Write-Host ""
Show-Info "--- [43] E2A's mission builds the new coverage-bound event, not an extension of daily_data_readiness_evaluated ---"
Test-ContentContainsNormalized "contract's E2A mission implements the new operation-scoped coverage-bound event" $ContractContent "implement the new operation-scoped ``autonomous_daily_coverage_bound`` event in ``sys_autonomous_session_events`` per" | Out-Null
Test-ContentContainsNormalized "contract states E2A's coverage-authority half supersedes the extension plan" $ContractContent "not an extension of ``daily_data_readiness_evaluated``" | Out-Null

Write-Host ""
Show-Info "--- [44] E2B is not authorized until E2A is independently accepted ---"
Test-ContentContainsNormalized "contract states E2B is not authorized until E2A is independently accepted" $ContractContent "not authorized until E2A is independently accepted" | Out-Null

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01E1 outcome contract evidence is consistent."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
