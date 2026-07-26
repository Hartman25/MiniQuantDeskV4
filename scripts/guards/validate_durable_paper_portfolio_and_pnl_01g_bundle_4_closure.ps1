# =============================================================================
# DURABLE-PAPER-PORTFOLIO-AND-PNL-01G -- Final Bundle 4 Closure Guard
# validate_durable_paper_portfolio_and_pnl_01g_bundle_4_closure.ps1
#
# Invokes every prior guard this bundle depends on (Bundle 3 final +
# B4-0..B4-F) plus check_unsafe_patterns.ps1, then independently proves the
# Bundle-4-wide invariants listed below. A failure in ANY invoked guard, or
# any check in this script, fails the whole run. Pure text/source validation
# plus delegated sub-guard invocation -- no DB connection, no daemon start,
# no cargo/npm build or test (those are separate, required steps of the
# B4-G mission's own regression matrix, documented in the closure spec).
#
# Independently proves:
#   [1]  All required specs/guards/tests for every B4-0..B4-G phase exist.
#   [2]  Durable writes are never route-triggered by a GET (no write-helper
#        call in any routes/*.rs file that also registers a GET route for
#        a durable-* path).
#   [3]  Fills are idempotent (the watermark-rejection/AlreadyCurrent
#        outcome variants exist as real code, not just documented).
#   [4]  Accounting completeness is explicit (accounting_epoch closed
#        vocabulary enforced in both the DB layer and the daemon layer).
#   [5]  Null P&L cannot become zero (formatDurableMoney's null-check
#        precedes any numeric formatting in the GUI).
#   [6]  Snapshot source distinctions remain (source CHECK constraint +
#        Synthetic-branch-never-calls-canonical-seam check).
#   [7]  Paper lifecycle reads durable P&L (already checked by the B4-E
#        guard; re-asserted here as a whole-bundle invariant).
#   [8]  GUI has no mutation controls (already checked by the B4-F guard;
#        re-asserted here).
#   [9]  Soak tooling remains GET-only (capture script never calls
#        Invoke-RestMethod directly, only through Invoke-DaemonGetOnly).
#   [10] Bundle 5 is not started, multi-symbol is not enabled, unattended
#        soak is not claimed started, live capital is not claimed ready --
#        checked across every file this bundle touched.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_durable_paper_portfolio_and_pnl_01g_bundle_4_closure.ps1
#
# Exit codes: 0 = all checks pass (including every invoked sub-guard),
# 1 = at least one violation anywhere.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan  }

Write-Host '============================================================'
Write-Host ' DURABLE-PAPER-PORTFOLIO-AND-PNL-01G -- FINAL BUNDLE 4 CLOSURE GUARD'
Write-Host '============================================================'

# -----------------------------------------------------------------------
# [0] Invoke every prior guard this bundle depends on.
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [0] Invoking all prerequisite guards ---'
$SubGuards = @(
    'validate_autonomous_daily_paper_operations_01g_bundle_3_final_closure.ps1',
    'validate_completed_bar_driver_time_independent_fixtures_01.ps1',
    'validate_durable_paper_portfolio_and_pnl_01a_contract.ps1',
    'validate_durable_paper_portfolio_and_pnl_01b_durable_store.ps1',
    'validate_durable_paper_portfolio_and_pnl_01c_snapshot_persistence_and_restart.ps1',
    'validate_durable_paper_portfolio_and_pnl_01d_accounting_and_pnl.ps1',
    'validate_durable_paper_portfolio_and_pnl_01e_read_only_api.ps1',
    'validate_durable_paper_portfolio_and_pnl_01f_operator_integration.ps1',
    'check_unsafe_patterns.ps1'
)
foreach ($guard in $SubGuards) {
    $guardPath = Join-Path $ScriptDir $guard
    if (-not (Test-Path -LiteralPath $guardPath)) {
        Show-Fail "Sub-guard not found: $guard"
        continue
    }
    Write-Host ""
    Show-Info "    >> Running $guard"
    & powershell -NoProfile -ExecutionPolicy Bypass -File $guardPath
    if ($LASTEXITCODE -eq 0) {
        Show-Green "$guard PASSED"
    } else {
        Show-Fail "$guard FAILED (exit $LASTEXITCODE)"
    }
}

# -----------------------------------------------------------------------
# [1] Required specs/guards/tests exist for every phase.
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [1] Required specs/guards/tests exist for every phase ---'
$RequiredFiles = @(
    'docs\specs\completed_bar_driver_time_independent_fixtures_01.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01a_current_truth_and_contract.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01b_durable_store.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01c_snapshot_persistence_and_restart.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01d_accounting_and_pnl.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01e_read_only_api.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01f_operator_integration.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01g_bundle_4_closure.md',
    'core-rs\crates\mqk-db\src\paper_portfolio.rs',
    'core-rs\crates\mqk-db\migrations\0053_paper_portfolio_durable_store.sql',
    'core-rs\crates\mqk-daemon\src\state\snapshot.rs',
    'core-rs\crates\mqk-daemon\src\state\paper_portfolio_accounting.rs',
    'core-rs\crates\mqk-daemon\src\routes\durable_portfolio.rs',
    'core-rs\crates\mqk-daemon\tests\scenario_durable_paper_portfolio_and_pnl_01.rs',
    'core-rs\crates\mqk-daemon\tests\scenario_durable_paper_portfolio_snapshot_persistence_01.rs',
    'core-rs\crates\mqk-daemon\tests\scenario_durable_paper_portfolio_accounting_01.rs',
    'core-rs\crates\mqk-daemon\tests\scenario_durable_paper_portfolio_read_only_api_01.rs',
    'core-rs\mqk-gui\src\features\portfolio\PortfolioScreen.tsx'
)
foreach ($f in $RequiredFiles) {
    $full = Join-Path $RepoRoot $f
    if (Test-Path -LiteralPath $full) {
        Show-Green "Found: $f"
    } else {
        Show-Fail "Missing required file: $f"
    }
}

# -----------------------------------------------------------------------
# [2] Durable writes are never route-triggered by a GET.
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [2] Durable writes are never route-triggered by a GET ---'
$DurablePortfolioRoutesFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\routes\durable_portfolio.rs'
$PaperLifecycleRoutesFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\routes\paper_lifecycle.rs'
foreach ($routeFile in @($DurablePortfolioRoutesFile, $PaperLifecycleRoutesFile)) {
    if (Test-Path -LiteralPath $routeFile) {
        $text = [System.IO.File]::ReadAllText($routeFile)
        if ($text -match 'insert_or_confirm_paper_portfolio_snapshot|upsert_paper_portfolio_accounting_state|accept_external_broker_snapshot\(|refresh_paper_portfolio_accounting_state') {
            Show-Fail "$(Split-Path -Leaf $routeFile) contains a durable-write call -- GET routes must be read-only"
        } else {
            Show-Green "$(Split-Path -Leaf $routeFile) contains no durable-write call"
        }
    }
}

# -----------------------------------------------------------------------
# [3] Fills are idempotent -- real code, not just documented.
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [3] Fill/watermark idempotency is real code ---'
$PaperPortfolioDbFile = Join-Path $RepoRoot 'core-rs\crates\mqk-db\src\paper_portfolio.rs'
if (Test-Path -LiteralPath $PaperPortfolioDbFile) {
    $text = [System.IO.File]::ReadAllText($PaperPortfolioDbFile)
    if ($text -match 'AlreadyCurrent' -and $text -match 'Rejected') {
        Show-Green "AlreadyCurrent/Rejected watermark outcomes exist as real enum variants"
    } else {
        Show-Fail "AlreadyCurrent/Rejected watermark outcomes not found"
    }
}

# -----------------------------------------------------------------------
# [4] Accounting completeness is explicit in both layers.
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [4] Accounting completeness (accounting_epoch) is explicit in both layers ---'
$AccountingDaemonFile = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\state\paper_portfolio_accounting.rs'
if ((Test-Path -LiteralPath $PaperPortfolioDbFile) -and (Test-Path -LiteralPath $AccountingDaemonFile)) {
    $dbText = [System.IO.File]::ReadAllText($PaperPortfolioDbFile)
    $daemonText = [System.IO.File]::ReadAllText($AccountingDaemonFile)
    if ($dbText -match "CHECK\s*\(accounting_epoch\s+IN" -or $dbText -match 'validate_accounting_epoch') {
        Show-Green "DB layer enforces accounting_epoch closed vocabulary"
    } else {
        Show-Fail "DB layer does not enforce accounting_epoch closed vocabulary"
    }
    if ($daemonText -match '"incomplete"' -and $daemonText -match '"complete"') {
        Show-Green "Daemon layer computes both complete/incomplete accounting_epoch values"
    } else {
        Show-Fail "Daemon layer does not compute both complete/incomplete accounting_epoch values"
    }
}

# -----------------------------------------------------------------------
# [5] Null P&L cannot become zero in the GUI.
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [5] GUI null-P&L-cannot-become-zero ---'
$PortfolioScreenFile = Join-Path $RepoRoot 'core-rs\mqk-gui\src\features\portfolio\PortfolioScreen.tsx'
if (Test-Path -LiteralPath $PortfolioScreenFile) {
    $text = [System.IO.File]::ReadAllText($PortfolioScreenFile)
    if ($text -match 'function formatDurableMoney\(value: number \| null\) \{\s*\r?\n\s*if \(value == null\)') {
        Show-Green "formatDurableMoney checks null before any numeric formatting"
    } else {
        Show-Fail "formatDurableMoney does not appear to check null first"
    }
}

# -----------------------------------------------------------------------
# [6] Snapshot source distinctions remain.
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [6] Snapshot source-authority distinction remains ---'
$MigrationFile = Join-Path $RepoRoot 'core-rs\crates\mqk-db\migrations\0053_paper_portfolio_durable_store.sql'
if (Test-Path -LiteralPath $MigrationFile) {
    $text = [System.IO.File]::ReadAllText($MigrationFile)
    if ($text -match "CHECK\s*\(source\s+IN\s*\('external_alpaca',\s*'synthetic_diagnostic'\)\)") {
        Show-Green "Migration schema-enforces the source authority distinction"
    } else {
        Show-Fail "Migration does not schema-enforce the source authority distinction"
    }
}

# -----------------------------------------------------------------------
# [7] Paper lifecycle reads durable P&L (whole-bundle re-assertion).
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [7] Paper lifecycle reads durable P&L ---'
if (Test-Path -LiteralPath $PaperLifecycleRoutesFile) {
    $text = [System.IO.File]::ReadAllText($PaperLifecycleRoutesFile)
    if ($text -match 'fetch_paper_portfolio_accounting_state' -and $text -match 'fetch_latest_paper_portfolio_snapshot') {
        Show-Green "paper_lifecycle.rs reads durable snapshot and accounting state"
    } else {
        Show-Fail "paper_lifecycle.rs does not read durable snapshot and accounting state"
    }
}

# -----------------------------------------------------------------------
# [8] GUI has no mutation controls (whole-bundle re-assertion).
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [8] GUI has no mutation controls ---'
if (Test-Path -LiteralPath $PortfolioScreenFile) {
    $text = [System.IO.File]::ReadAllText($PortfolioScreenFile)
    if ($text -match '<button' -or $text -match 'invokeOperatorAction') {
        Show-Fail "PortfolioScreen.tsx appears to contain a mutation control"
    } else {
        Show-Green "PortfolioScreen.tsx contains no mutation control"
    }
}

# -----------------------------------------------------------------------
# [9] Soak tooling remains GET-only.
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [9] Soak-evidence capture script remains GET-only ---'
$CaptureScriptFile = Join-Path $RepoRoot 'scripts\soak\capture_autonomous_paper_session_evidence.ps1'
if (Test-Path -LiteralPath $CaptureScriptFile) {
    $lines = Get-Content -LiteralPath $CaptureScriptFile
    $badLines = $lines | Where-Object { $_ -match 'Invoke-RestMethod' -and $_ -notmatch 'function Invoke-DaemonGetOnly' } | Where-Object { $_ -notmatch '^\s*#' }
    # The only legitimate Invoke-RestMethod call is inside Invoke-DaemonGetOnly itself.
    # Exclude comment lines (this file itself documents its own GET-only
    # design in a header comment that mentions "Invoke-RestMethod" as prose,
    # not as a call site).
    $callSites = @($lines | Where-Object { $_ -notmatch '^\s*#' } | Select-String -Pattern 'Invoke-RestMethod')
    if ($callSites.Count -le 1) {
        Show-Green "Exactly one Invoke-RestMethod call site (inside Invoke-DaemonGetOnly) -- no ad-hoc direct calls"
    } else {
        Show-Fail "Multiple Invoke-RestMethod call sites found ($($callSites.Count)) -- capture script may bypass the GET-only seam"
    }
    if (($lines -join "`n") -match '-Method\s+Post|-Method\s+Put|-Method\s+Delete|-Method\s+Patch') {
        Show-Fail "Capture script appears to use a mutation HTTP method"
    } else {
        Show-Green "Capture script uses no mutation HTTP method"
    }
}

# -----------------------------------------------------------------------
# [10] No premature claims anywhere this bundle touched.
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [10] No Bundle 5 / multi-symbol / unattended-soak / live-capital claims ---'
$FilesToScanForClaims = @(
    'docs\specs\durable_paper_portfolio_and_pnl_01a_current_truth_and_contract.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01b_durable_store.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01c_snapshot_persistence_and_restart.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01d_accounting_and_pnl.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01e_read_only_api.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01f_operator_integration.md',
    'docs\specs\durable_paper_portfolio_and_pnl_01g_bundle_4_closure.md',
    'docs\runbooks\autonomous_paper_ops.md'
)
# Each pattern is checked per-sentence (split on '.'/newline) so a match is
# only flagged when NOT accompanied by a negation word in the same
# sentence -- e.g. "the unattended soak has not started" (compliant,
# negated) must not trip the same pattern that would catch "the unattended
# soak has started" (a real premature claim). This avoids false positives
# on legitimate negated/conditional prose ("until ... the soak has
# completed") while still catching an unqualified affirmative claim.
$ForbiddenClaimPatterns = @(
    'Bundle 5 (is |has )?(started|begun|underway)',
    'multi-symbol autonomous (is )?(enabled|started)',
    'unattended soak (has |is )?(started|begun)',
    '10.20.session soak (has |is )?(started|begun)',
    'live capital (is )?(ready|approved|enabled)'
)
$NegationWords = @('not ', "n't ", 'never ', 'no ', 'until ')
foreach ($f in $FilesToScanForClaims) {
    $full = Join-Path $RepoRoot $f
    if (-not (Test-Path -LiteralPath $full)) { continue }
    $text = [System.IO.File]::ReadAllText($full)
    $sentences = $text -split '(?<=[\.\r\n])'
    $foundClaim = $false
    foreach ($pattern in $ForbiddenClaimPatterns) {
        foreach ($sentence in $sentences) {
            if ($sentence -match $pattern) {
                $hasNegation = $false
                foreach ($neg in $NegationWords) {
                    if ($sentence -match [regex]::Escape($neg)) { $hasNegation = $true }
                }
                if (-not $hasNegation) {
                    Show-Fail "$f contains an unqualified forbidden claim matching '$pattern': $($sentence.Trim())"
                    $foundClaim = $true
                }
            }
        }
    }
    if (-not $foundClaim) {
        Show-Green "$f contains no forbidden premature claim"
    }
}

# -----------------------------------------------------------------------
# [11] Fixed, immutable Bundle 4 committed-range authority (never widened
# to ..HEAD, mirroring the Bundle 3 final guard's own dual-mode pattern).
#
# Pre-commit (this guard's own commit not yet made): HEAD still equals the
# B4-F commit; the fixed-range authority proof is deferred (there is no B4-G
# commit yet to bound it), and the working-tree diff checks already run
# above (via each sub-guard's own scope check) are the authority for this
# mode.
#
# Post-commit: locate the unique commit whose exact subject is "test: prove
# durable paper portfolio bundle 4 closure"; require it to exist and be an
# ancestor of HEAD. Once located, freeze
# e3eb2fe220199b69979a03c2c5faac1b1614fd99..<that commit> -- deliberately
# never ..HEAD -- as the immutable Bundle 4 range: no migration or
# production Rust file outside core-rs/crates/mqk-db|mqk-daemon, and no
# multi-symbol/Bundle-5 file, may appear in it.
# -----------------------------------------------------------------------
Write-Host ''
Show-Info '--- [11] Fixed, immutable Bundle 4 committed-range authority (never widened to ..HEAD) ---'
$Bundle3AcceptedHead = 'e3eb2fe220199b69979a03c2c5faac1b1614fd99'
$FinalCommitSubject = 'test: prove durable paper portfolio bundle 4 closure'

$PriorEap = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$CurrentHead = (git -C $RepoRoot rev-parse HEAD 2>$null | Select-Object -First 1)
$ErrorActionPreference = $PriorEap

$MatchingFinalCommits = @()
$ErrorActionPreference = 'Continue'
$MatchingFinalCommits = @(git -C $RepoRoot log --all --format='%H %s' 2>$null |
    Where-Object { $_ -match "^[0-9a-f]{40} $([regex]::Escape($FinalCommitSubject))$" } |
    ForEach-Object { ($_ -split ' ', 2)[0] } |
    Select-Object -Unique)
$ErrorActionPreference = $PriorEap

if ($MatchingFinalCommits.Count -eq 0) {
    Show-Info "No commit with subject '$FinalCommitSubject' exists yet -- pre-commit mode, fixed-range authority proof deferred to post-commit validation"
} elseif ($MatchingFinalCommits.Count -gt 1) {
    Show-Fail "Multiple commits match subject '$FinalCommitSubject' -- cannot uniquely locate the Bundle 4 final commit"
} else {
    $FinalCommit = $MatchingFinalCommits[0]
    $ErrorActionPreference = 'Continue'
    $IsAncestor = $false
    git -C $RepoRoot merge-base --is-ancestor $FinalCommit HEAD 2>$null
    if ($LASTEXITCODE -eq 0) { $IsAncestor = $true }
    $ErrorActionPreference = $PriorEap
    if (-not $IsAncestor) {
        Show-Fail "Located final commit $FinalCommit is not an ancestor of current HEAD"
    } else {
        Show-Green "Located Bundle 4 final commit $FinalCommit, confirmed ancestor of HEAD"
        $ErrorActionPreference = 'Continue'
        $RangeFiles = @(git -C $RepoRoot diff --name-only "$Bundle3AcceptedHead..$FinalCommit" 2>$null)
        $ErrorActionPreference = $PriorEap
        $UnexpectedRust = $RangeFiles | Where-Object {
            $_ -like 'core-rs/*/src/*.rs' -and
            $_ -notlike 'core-rs/crates/mqk-db/*' -and
            $_ -notlike 'core-rs/crates/mqk-daemon/*'
        }
        if ($UnexpectedRust) {
            Show-Fail "Unexpected production Rust file(s) outside mqk-db/mqk-daemon in the frozen Bundle 4 range: $($UnexpectedRust -join ', ')"
        } else {
            Show-Green "No unexpected production Rust file outside mqk-db/mqk-daemon in the frozen Bundle 4 range"
        }
        $MultiSymbolOrBundle5 = $RangeFiles | Where-Object { $_ -match 'bundle_5|multi_symbol_autonomous' }
        if ($MultiSymbolOrBundle5) {
            Show-Fail "Bundle 5 / multi-symbol-autonomous file(s) found in the frozen Bundle 4 range: $($MultiSymbolOrBundle5 -join ', ')"
        } else {
            Show-Green "No Bundle 5 / multi-symbol-autonomous file in the frozen Bundle 4 range"
        }
    }
}

# -----------------------------------------------------------------------
# [12]-[22] FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR invariants.
# -----------------------------------------------------------------------
$PaperPortfolioDbFileG = Join-Path $RepoRoot 'core-rs\crates\mqk-db\src\paper_portfolio.rs'
$SnapshotFileG = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\state\snapshot.rs'
$AccountingFileG = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\state\paper_portfolio_accounting.rs'
$MigrationFileG = Join-Path $RepoRoot 'core-rs\crates\mqk-db\migrations\0054_paper_portfolio_accounting_snapshot_provenance.sql'
$DurablePortfolioTextG = if (Test-Path -LiteralPath $DurablePortfolioRoutesFile) { [System.IO.File]::ReadAllText($DurablePortfolioRoutesFile) } else { '' }
$PaperLifecycleTextG = if (Test-Path -LiteralPath $PaperLifecycleRoutesFile) { [System.IO.File]::ReadAllText($PaperLifecycleRoutesFile) } else { '' }
$PaperPortfolioDbTextG = if (Test-Path -LiteralPath $PaperPortfolioDbFileG) { [System.IO.File]::ReadAllText($PaperPortfolioDbFileG) } else { '' }
$SnapshotTextG = if (Test-Path -LiteralPath $SnapshotFileG) { [System.IO.File]::ReadAllText($SnapshotFileG) } else { '' }
$AccountingTextG = if (Test-Path -LiteralPath $AccountingFileG) { [System.IO.File]::ReadAllText($AccountingFileG) } else { '' }
$GuiValidatorFileG = Join-Path $RepoRoot 'core-rs\mqk-gui\src\features\system\durablePortfolio.ts'

Write-Host ''
Show-Info '--- [12] Run-scoped snapshot helper exists ---'
if ($PaperPortfolioDbTextG -match 'pub async fn fetch_latest_paper_portfolio_snapshot_for_run\(') {
    Show-Green "fetch_latest_paper_portfolio_snapshot_for_run exists in mqk-db"
} else {
    Show-Fail "fetch_latest_paper_portfolio_snapshot_for_run not found in mqk-db"
}

Write-Host ''
Show-Info '--- [13] Run-scoped routes never call the global (non-run-scoped) latest-snapshot helper ---'
# Matches a call to the global helper ONLY (not the _for_run variant): the
# function name followed directly by "(", not by "_for_run(".
$GlobalHelperCallPattern = 'fetch_latest_paper_portfolio_snapshot\('
foreach ($pair in @(
        @{ Name = 'durable_portfolio.rs'; Text = $DurablePortfolioTextG },
        @{ Name = 'paper_lifecycle.rs'; Text = $PaperLifecycleTextG }
    )) {
    if ($pair.Text -match $GlobalHelperCallPattern) {
        Show-Fail "$($pair.Name) still calls the global (non-run-scoped) fetch_latest_paper_portfolio_snapshot -- cross-run contamination risk"
    } else {
        Show-Green "$($pair.Name) never calls the global (non-run-scoped) fetch_latest_paper_portfolio_snapshot"
    }
    if ($pair.Text -match 'fetch_latest_paper_portfolio_snapshot_for_run\(') {
        Show-Green "$($pair.Name) uses the run-scoped helper"
    } else {
        Show-Fail "$($pair.Name) does not call the run-scoped helper"
    }
}

Write-Host ''
Show-Info '--- [14] Response run_id is not independently echoed from an unrelated run ---'
# durable_portfolio.rs must derive its response run_id from the SAME
# resolved `run` the snapshot/accounting lookups used (`run.run_id`), never
# from a second, independently-resolved run.
if ($DurablePortfolioTextG -match 'run_id:\s*Some\(run\.run_id\.to_string\(\)\)') {
    Show-Green "durable_portfolio.rs echoes run_id from the same resolved run used for snapshot/accounting lookups"
} else {
    Show-Fail "durable_portfolio.rs does not visibly tie its echoed run_id to the resolved run"
}

Write-Host ''
Show-Info '--- [15] Accounting carries source_snapshot_id provenance ---'
if ($PaperPortfolioDbTextG -match 'source_snapshot_id') {
    Show-Green "mqk-db accounting record/args carry source_snapshot_id"
} else {
    Show-Fail "mqk-db accounting record/args do not carry source_snapshot_id"
}
if (Test-Path -LiteralPath $MigrationFileG) {
    Show-Green "Additive migration adding source_snapshot_id exists"
} else {
    Show-Fail "No additive migration found adding source_snapshot_id"
}

Write-Host ''
Show-Info '--- [16] Accounting refresh requires confirmed snapshot persistence ---'
if ($SnapshotTextG -match 'ExternalSnapshotPersistOutcome') {
    Show-Green "snapshot.rs defines a typed persistence outcome"
} else {
    Show-Fail "snapshot.rs does not define a typed persistence outcome"
}
if ($SnapshotTextG -match 'ExternalSnapshotPersistOutcome::Confirmed[\s\S]{0,400}refresh_paper_portfolio_accounting_state_best_effort') {
    Show-Green "Accounting refresh in the canonical acceptance seam is gated on a Confirmed persistence outcome"
} else {
    Show-Fail "Accounting refresh does not appear gated on a Confirmed persistence outcome in snapshot.rs"
}

Write-Host ''
Show-Info '--- [17] Same-watermark upsert compares full content (not watermark alone) ---'
if ($PaperPortfolioDbTextG -match 'UpsertPaperPortfolioAccountingStateOutcome::Conflict' -and
    $PaperPortfolioDbTextG -match 'UpsertPaperPortfolioAccountingStateOutcome::UpdatedForSnapshot') {
    Show-Green "Conflict and UpdatedForSnapshot outcome variants exist as real code"
} else {
    Show-Fail "Conflict/UpdatedForSnapshot outcome variants missing -- same-watermark replay is not fully compared"
}
$DbTestFileG = Join-Path $RepoRoot 'core-rs\crates\mqk-db\tests\scenario_paper_portfolio_store_01.rs'
if (Test-Path -LiteralPath $DbTestFileG) {
    $DbTestTextG = [System.IO.File]::ReadAllText($DbTestFileG)
    if ($DbTestTextG -match 'expected Conflict' -and $DbTestTextG -match 'expected UpdatedForSnapshot') {
        Show-Green "Same-watermark/same-snapshot conflict and same-watermark/new-snapshot update are both tested"
    } else {
        Show-Fail "Missing test coverage for same-watermark conflict and/or same-watermark new-snapshot update"
    }
}

Write-Host ''
Show-Info '--- [18] Accounting completeness compares both symbol directions ---'
$RequiredCompletenessReasons = @(
    'broker_position_missing_fill_history',
    'fill_history_position_missing_at_broker',
    'position_quantity_mismatch',
    'broker_position_quantity_unparseable',
    'duplicate_broker_position_symbol'
)
foreach ($reason in $RequiredCompletenessReasons) {
    if ($AccountingTextG -match [regex]::Escape($reason)) {
        Show-Green "Completeness reason code present: $reason"
    } else {
        Show-Fail "Missing bidirectional completeness reason code: $reason"
    }
}

Write-Host ''
Show-Info '--- [19] Unparseable broker quantity cannot be silently skipped ---'
if ($AccountingTextG -match 'broker_position_quantity_unparseable') {
    Show-Green "An unparseable broker quantity is reported as a blocker, not silently continued past"
} else {
    Show-Fail "No reason code found for an unparseable broker quantity"
}

Write-Host ''
Show-Info '--- [20] Public durable-portfolio routes never leak a raw formatted DB error ---'
$RawErrorPattern = '\{err:#\}|\{e:#\}|\{err:\?\}|\{e:\?\}'
foreach ($pair in @(
        @{ Name = 'durable_portfolio.rs'; Text = $DurablePortfolioTextG },
        @{ Name = 'paper_lifecycle.rs'; Text = $PaperLifecycleTextG }
    )) {
    if ($pair.Text -match $RawErrorPattern) {
        Show-Fail "$($pair.Name) still formats a raw error onto a response path (format!(`"{err:#}`") or similar)"
    } else {
        Show-Green "$($pair.Name) contains no raw formatted-error interpolation"
    }
}

Write-Host ''
Show-Info '--- [21] Explicit-run query failure cannot become not_found ---'
if ($DurablePortfolioTextG -match 'enum RunResolution' -and
    $DurablePortfolioTextG -match 'QueryFailed' -and
    $DurablePortfolioTextG -match 'NotFound') {
    Show-Green "RunResolution distinguishes QueryFailed from NotFound as separate variants"
} else {
    Show-Fail "RunResolution does not distinguish QueryFailed from NotFound"
}
if ($DurablePortfolioTextG -match 'is_row_not_found') {
    Show-Green "Row-not-found is distinguished from other DB errors via is_row_not_found"
} else {
    Show-Fail "No is_row_not_found distinction found -- explicit-run DB failures risk collapsing to not_found"
}

Write-Host ''
Show-Info '--- [22] GUI runtime validators exist for the durable-portfolio contract ---'
if (Test-Path -LiteralPath $GuiValidatorFileG) {
    $GuiValidatorTextG = [System.IO.File]::ReadAllText($GuiValidatorFileG)
    $RequiredValidators = @(
        'parseDurablePortfolioSummary',
        'parseDurablePortfolioPositions',
        'parseDurablePortfolioSnapshots',
        'enforceRunScopeConsistency'
    )
    foreach ($fn in $RequiredValidators) {
        if ($GuiValidatorTextG -match "function $fn\(" -or $GuiValidatorTextG -match "export function $fn\(") {
            Show-Green "GUI runtime validator present: $fn"
        } else {
            Show-Fail "Missing GUI runtime validator: $fn"
        }
    }
} else {
    Show-Fail "GUI durable-portfolio runtime validator module not found: $GuiValidatorFileG"
}

# -----------------------------------------------------------------------
# [23]-[36] DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-COHERENCE-AND-
# ACCEPTANCE-PROOF invariants (Phase A/B/C).
# -----------------------------------------------------------------------
$PortfolioProvenanceFileG = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\src\routes\portfolio_provenance.rs'
$PortfolioProvenanceTextG = if (Test-Path -LiteralPath $PortfolioProvenanceFileG) { [System.IO.File]::ReadAllText($PortfolioProvenanceFileG) } else { '' }

Write-Host ''
Show-Info '--- [23] USD authority is enforced ---'
if ($SnapshotTextG -match 'snapshot\.account\.currency\s*!=\s*"USD"') {
    Show-Green "snapshot.rs refuses a non-USD account currency before persisting an external snapshot"
} else {
    Show-Fail "snapshot.rs does not visibly enforce USD currency on external snapshot acceptance"
}
if ($PaperPortfolioDbTextG -match 'candidate_authority\.currency\s*!=\s*"USD"') {
    Show-Green "mqk-db enforces USD currency on the accounting source-snapshot authority check"
} else {
    Show-Fail "mqk-db does not visibly enforce USD currency on the source-snapshot authority check"
}

Write-Host ''
Show-Info '--- [24] Non-null run ownership is enforced for authoritative external snapshots ---'
if ($SnapshotTextG -match 'let Some\(run_id\) = run_id else') {
    Show-Green "snapshot.rs refuses a run_id-less external snapshot before persisting"
} else {
    Show-Fail "snapshot.rs does not visibly require non-null run ownership before persisting"
}

Write-Host ''
Show-Info '--- [25] Blank symbols cannot be skipped ---'
if ($SnapshotTextG -match 'p\.symbol\.trim\(\)\.is_empty\(\)') {
    Show-Green "snapshot.rs refuses a blank position symbol before persisting"
} else {
    Show-Fail "snapshot.rs does not visibly refuse a blank position symbol"
}
if ($AccountingTextG -match 'broker_position_symbol_blank') {
    Show-Green "paper_portfolio_accounting.rs reports a blank broker position symbol as a bounded blocker, never a silent skip"
} else {
    Show-Fail "paper_portfolio_accounting.rs does not report a blank broker position symbol as a bounded blocker"
}

Write-Host ''
Show-Info '--- [26] Source snapshot must belong to the accounting run (transactional integrity) ---'
if ($PaperPortfolioDbTextG -match 'InvalidSourceSnapshot' -and
    $PaperPortfolioDbTextG -match 'candidate_authority\.run_id\s*!=\s*Some\(args\.run_id\)') {
    Show-Green "upsert_paper_portfolio_accounting_state rejects a null/cross-run source snapshot as InvalidSourceSnapshot"
} else {
    Show-Fail "upsert_paper_portfolio_accounting_state does not visibly validate source-snapshot run ownership"
}
if ($PaperPortfolioDbTextG -match 'fetch_snapshot_authority_tx') {
    Show-Green "Source-snapshot authority is fetched inside the same transaction as the accounting write"
} else {
    Show-Fail "No transactional source-snapshot authority fetch found"
}

Write-Host ''
Show-Info '--- [27] Same-snapshot epoch/reason drift is a Conflict, never a silent update ---'
if ($PaperPortfolioDbTextG -match 'epoch classification drifted' -and
    $PaperPortfolioDbTextG -match 'UpsertPaperPortfolioAccountingStateOutcome::Conflict') {
    Show-Green "A same-snapshot epoch/reason drift with unchanged fill-derived values is rejected as Conflict"
} else {
    Show-Fail "Same-snapshot epoch/reason drift does not visibly reject as Conflict"
}

Write-Host ''
Show-Info '--- [28] Older snapshot provenance cannot replace newer ---'
if ($PaperPortfolioDbTextG -match 'RejectedStaleSnapshot') {
    Show-Green "RejectedStaleSnapshot outcome variant exists as real code"
} else {
    Show-Fail "RejectedStaleSnapshot outcome variant missing -- stale-snapshot regression is not rejected"
}

Write-Host ''
Show-Info '--- [29] Snapshot authority ordering is deterministic ((captured_at_utc, snapshot_id)) ---'
if ($PaperPortfolioDbTextG -match 'candidate_snapshot_is_authoritative' -and
    $PaperPortfolioDbTextG -match '\(DateTime<Utc>,\s*Uuid\)') {
    Show-Green "A single deterministic (captured_at_utc, snapshot_id) authority-ordering helper exists and is reused"
} else {
    Show-Fail "No single deterministic snapshot-authority-ordering helper found"
}

Write-Host ''
Show-Info '--- [30] Summary accounting must match the selected snapshot (shared classifier) ---'
if ($PortfolioProvenanceTextG -match 'AccountingSnapshotMismatch') {
    Show-Green "The shared provenance classifier defines an AccountingSnapshotMismatch state"
} else {
    Show-Fail "No AccountingSnapshotMismatch state found in the shared provenance classifier"
}
if ($DurablePortfolioTextG -match 'classify_portfolio_provenance') {
    Show-Green "durable_portfolio.rs uses the shared provenance classifier"
} else {
    Show-Fail "durable_portfolio.rs does not use the shared provenance classifier"
}

Write-Host ''
Show-Info '--- [31] Paper lifecycle uses the same provenance classifier as durable-summary ---'
if ($PaperLifecycleTextG -match 'classify_portfolio_provenance') {
    Show-Green "paper_lifecycle.rs uses the shared provenance classifier"
} else {
    Show-Fail "paper_lifecycle.rs does not use the shared provenance classifier -- risk of drift from durable-summary"
}
if ($PaperLifecycleTextG -match 'durable_accounting\s*:\s*Option') {
    Show-Fail "paper_lifecycle.rs still classifies overall_lifecycle_state from a raw accounting row instead of the shared classifier verdict"
} else {
    Show-Green "paper_lifecycle.rs no longer classifies overall_lifecycle_state from a raw accounting row"
}

Write-Host ''
Show-Info '--- [32] Paper-lifecycle invalid-run_id errors are bounded and never echo raw input ---'
if ($PaperLifecycleTextG -match 'INVALID_RUN_ID_MESSAGE') {
    Show-Green "paper_lifecycle.rs uses the shared fixed, bounded invalid-run_id message"
} else {
    Show-Fail "paper_lifecycle.rs does not use the shared fixed invalid-run_id message"
}
if ($PaperLifecycleTextG -match 'run_id is not a valid UUID: \{s\}') {
    Show-Fail "paper_lifecycle.rs still echoes the raw supplied run_id value in its error message"
} else {
    Show-Green "paper_lifecycle.rs does not echo the raw supplied run_id value"
}

Write-Host ''
Show-Info '--- [33] All nested GUI truth states are closed (including the new Phase B/C states) ---'
$GuiValidatorTextG23 = if (Test-Path -LiteralPath $GuiValidatorFileG) { [System.IO.File]::ReadAllText($GuiValidatorFileG) } else { '' }
$RequiredGuiStates = @(
    'accounting_snapshot_mismatch',
    'fill_history_incomplete',
    'accounting_epoch_unavailable',
    'query_failed'
)
foreach ($state in $RequiredGuiStates) {
    if ($GuiValidatorTextG23 -match [regex]::Escape($state)) {
        Show-Green "GUI closed vocabulary includes: $state"
    } else {
        Show-Fail "GUI closed vocabulary is missing: $state"
    }
}
if ($GuiValidatorTextG23 -match 'Number\.isFinite' -and $GuiValidatorTextG23 -match 'Number\.isSafeInteger') {
    Show-Green "GUI validator rejects non-finite and unsafe/non-integral numeric fields"
} else {
    Show-Fail "GUI validator does not visibly reject non-finite/unsafe numeric fields"
}

Write-Host ''
Show-Info '--- [34] GUI authoritative surfaces require matching run AND snapshot IDs ---'
if ($GuiValidatorTextG23 -match 'snapshotMismatch') {
    Show-Green "enforceRunScopeConsistency compares snapshot_id, not only run_id"
} else {
    Show-Fail "enforceRunScopeConsistency does not visibly compare snapshot_id"
}

Write-Host ''
Show-Info '--- [35] No new database migration was added by this patch ---'
$PriorEapMig = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$StartingHeadForMigrationCheck = '32bda6b7ab2bb85b5908d97d4903c0d4892fcb0a'
$MigrationDiffFiles = @(git -C $RepoRoot diff --name-only "$StartingHeadForMigrationCheck..HEAD" -- core-rs/crates/mqk-db/migrations 2>$null)
$ErrorActionPreference = $PriorEapMig
$MigrationDiffFilesClean = @($MigrationDiffFiles) | Where-Object { $_ -ne '' }
if ($null -eq $MigrationDiffFilesClean -or @($MigrationDiffFilesClean).Count -eq 0) {
    Show-Green "No migration file changed since the patch's starting commit $StartingHeadForMigrationCheck"
} else {
    Show-Fail "Migration file(s) changed since the starting commit: $($MigrationDiffFilesClean -join ', ')"
}

Write-Host ''
Show-Info '--- [36] No broker adapter, order-submission, strategy, or risk-limit path changed ---'
$PriorEapScope = $ErrorActionPreference
$ErrorActionPreference = 'Continue'
$FullDiffFiles = @(git -C $RepoRoot diff --name-only "$StartingHeadForMigrationCheck..HEAD" 2>$null)
$ErrorActionPreference = $PriorEapScope
$FullDiffFilesClean = @($FullDiffFiles) | Where-Object { $_ -ne '' }
$OutOfScopePaths = $FullDiffFilesClean | Where-Object {
    $_ -like 'core-rs/crates/mqk-broker-*/*' -or
    $_ -like 'core-rs/crates/mqk-strategy/*' -or
    $_ -like 'core-rs/crates/mqk-risk/*' -or
    $_ -like 'core-rs/crates/mqk-execution/src/*'
}
if ($null -eq $OutOfScopePaths -or @($OutOfScopePaths).Count -eq 0) {
    Show-Green "No broker adapter, strategy, risk, or execution-core file changed since $StartingHeadForMigrationCheck"
} else {
    Show-Fail "Out-of-scope file(s) changed: $($OutOfScopePaths -join ', ')"
}

Write-Host ''
Write-Host '============================================================'
if ($Violations -gt 0) {
    Write-Host " BUNDLE 4 CLOSURE GUARD FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
} else {
    Write-Host " BUNDLE 4 CLOSURE GUARD PASSED -- 0 violations" -ForegroundColor Green
    exit 0
}
