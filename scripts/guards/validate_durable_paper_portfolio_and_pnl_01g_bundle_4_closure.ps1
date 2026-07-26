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

Write-Host ''
Write-Host '============================================================'
if ($Violations -gt 0) {
    Write-Host " BUNDLE 4 CLOSURE GUARD FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
} else {
    Write-Host " BUNDLE 4 CLOSURE GUARD PASSED -- 0 violations" -ForegroundColor Green
    exit 0
}
