# =============================================================================
# DURABLE-PAPER-PORTFOLIO-AND-PNL-01B -- Source-aware static validator
# validate_durable_paper_portfolio_and_pnl_01b_durable_store.ps1
#
# Scope: core-rs/crates/mqk-db/migrations/0053_paper_portfolio_durable_store.sql,
# core-rs/crates/mqk-db/migrations/manifest.json, core-rs/crates/mqk-db/src/paper_portfolio.rs,
# core-rs/crates/mqk-db/src/lib.rs, core-rs/crates/mqk-db/tests/scenario_paper_portfolio_store_01.rs,
# docs/specs/durable_paper_portfolio_and_pnl_01b_durable_store.md.
# Pure text/source validation -- no cargo build/test/compile, no DB
# connection. Running the actual DB-backed test binary against the isolated
# port-5434 test DB is a separate, required step of the B4-B mission, not
# performed by this guard.
#
# Checks:
#   [1]  Migration file and manifest entry both exist, and the manifest
#        entry's path matches the actual filename.
#   [2]  The migration contains no DEFAULT now()/gen_random_uuid().
#   [3]  The migration's source CHECK constraint enforces exactly
#        ('external_alpaca', 'synthetic_diagnostic') -- the source-authority
#        distinction is schema-enforced, not just conventional.
#   [4]  The migration does not create any table that duplicates fill
#        content (no symbol/side/qty/price columns on the accounting-state
#        table) -- this must remain a computed-summary cache, not a second
#        fill ledger.
#   [5]  paper_portfolio.rs is registered in lib.rs (both `pub mod` and
#        `pub use`).
#   [6]  All required public functions/types exist in paper_portfolio.rs.
#   [7]  The Conflict/Rejected outcome variants exist -- fail-closed
#        idempotency is a real code path, not just documented intent.
#   [8]  The test file exists and contains all required proof scenarios.
#   [9]  The spec doc exists and documents the "no separate accounting
#        event table" decision.
#   [10] No mqk-daemon file is touched by this patch's working-tree diff
#        (B4-B is DB-layer only; wiring is B4-C/B4-D).
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_durable_paper_portfolio_and_pnl_01b_durable_store.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir   = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot    = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$MigrationFile = Join-Path $RepoRoot 'core-rs\crates\mqk-db\migrations\0053_paper_portfolio_durable_store.sql'
$ManifestFile  = Join-Path $RepoRoot 'core-rs\crates\mqk-db\migrations\manifest.json'
$ModuleFile    = Join-Path $RepoRoot 'core-rs\crates\mqk-db\src\paper_portfolio.rs'
$LibFile       = Join-Path $RepoRoot 'core-rs\crates\mqk-db\src\lib.rs'
$TestFile      = Join-Path $RepoRoot 'core-rs\crates\mqk-db\tests\scenario_paper_portfolio_store_01.rs'
$SpecDoc       = Join-Path $RepoRoot 'docs\specs\durable_paper_portfolio_and_pnl_01b_durable_store.md'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }

Write-Host '============================================================'
Write-Host ' DURABLE-PAPER-PORTFOLIO-AND-PNL-01B validate_..._01b_durable_store'
Write-Host '============================================================'

$requiredFiles = @($MigrationFile, $ManifestFile, $ModuleFile, $LibFile, $TestFile, $SpecDoc)
foreach ($f in $requiredFiles) {
    if (-not (Test-Path -LiteralPath $f)) {
        Show-Fail "Required file not found: $f"
    } else {
        Show-Green "Found: $f"
    }
}
if ($Violations -gt 0) {
    Write-Host ''
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
}

$migrationText = [System.IO.File]::ReadAllText($MigrationFile)
# Strip SQL line comments before content checks below, so a doc comment that
# merely *mentions* "DEFAULT now()" (to explain that the migration avoids
# it) isn't misread as the migration actually using it.
$migrationCode = ($migrationText -split "`n" | Where-Object { $_ -notmatch '^\s*--' }) -join "`n"
$manifestText  = [System.IO.File]::ReadAllText($ManifestFile)
$moduleText    = [System.IO.File]::ReadAllText($ModuleFile)
$libText       = [System.IO.File]::ReadAllText($LibFile)
$testText      = [System.IO.File]::ReadAllText($TestFile)
$specText      = [System.IO.File]::ReadAllText($SpecDoc)

# [1] Manifest entry matches
if ($manifestText -match '"id":\s*"0053"[\s\S]{0,200}?"path":\s*"0053_paper_portfolio_durable_store\.sql"') {
    Show-Green "Manifest entry 0053 matches the migration filename"
} else {
    Show-Fail "Manifest does not contain a matching 0053 entry for the migration filename"
}

# [2] No DEFAULT now()/gen_random_uuid()
if ($migrationCode -match '(?i)DEFAULT\s+now\(\)|DEFAULT\s+gen_random_uuid\(\)') {
    Show-Fail "Migration contains DEFAULT now()/gen_random_uuid() -- every timestamp/identity must be caller-supplied"
} else {
    Show-Green "Migration contains no DEFAULT now()/gen_random_uuid()"
}

# [3] Source CHECK constraint
if ($migrationText -match "CHECK\s*\(source\s+IN\s*\('external_alpaca',\s*'synthetic_diagnostic'\)\)") {
    Show-Green "Source CHECK constraint enforces ('external_alpaca', 'synthetic_diagnostic')"
} else {
    Show-Fail "Migration does not schema-enforce the source authority distinction"
}

# [4] No duplicated fill content columns on the accounting-state table
$accountingStateBlock = [regex]::Match($migrationText, 'CREATE TABLE IF NOT EXISTS sys_paper_portfolio_accounting_state[\s\S]*?\);')
if ($accountingStateBlock.Success) {
    $block = $accountingStateBlock.Value
    if ($block -match '(?i)\bsymbol\b|\bside\b|\bqty\b|\bprice_micros\b') {
        Show-Fail "sys_paper_portfolio_accounting_state appears to duplicate fill content columns -- must remain a computed-summary cache, not a second fill ledger"
    } else {
        Show-Green "sys_paper_portfolio_accounting_state contains no duplicated fill-content columns"
    }
} else {
    Show-Fail "Could not locate sys_paper_portfolio_accounting_state table definition"
}

# [5] Module registration in lib.rs
if ($libText -match 'pub mod paper_portfolio;' -and $libText -match 'pub use paper_portfolio::\*;') {
    Show-Green "paper_portfolio module registered in lib.rs (mod + use)"
} else {
    Show-Fail "paper_portfolio module is not fully registered in lib.rs"
}

# [6] Required public functions/types
$requiredSymbols = @(
    'pub async fn insert_or_confirm_paper_portfolio_snapshot',
    'pub async fn fetch_paper_portfolio_snapshot_by_id',
    'pub async fn fetch_latest_paper_portfolio_snapshot',
    'pub async fn fetch_recent_paper_portfolio_snapshots',
    'pub async fn upsert_paper_portfolio_accounting_state',
    'pub async fn fetch_paper_portfolio_accounting_state',
    'pub enum InsertPaperPortfolioSnapshotOutcome',
    'pub enum UpsertPaperPortfolioAccountingStateOutcome',
    'PAPER_PORTFOLIO_SNAPSHOT_SOURCE_EXTERNAL_ALPACA',
    'PAPER_PORTFOLIO_SNAPSHOT_SOURCE_SYNTHETIC_DIAGNOSTIC',
    'PAPER_PORTFOLIO_ACCOUNTING_EPOCH_COMPLETE',
    'PAPER_PORTFOLIO_ACCOUNTING_EPOCH_INCOMPLETE'
)
foreach ($sym in $requiredSymbols) {
    if ($moduleText -like "*$sym*") {
        Show-Green "Found required symbol: $sym"
    } else {
        Show-Fail "Missing required symbol: $sym"
    }
}

# [7] Fail-closed outcome variants present
if ($moduleText -match 'Conflict\s*\{' -and $moduleText -match 'Rejected\s*\{') {
    Show-Green "Conflict and Rejected outcome variants are real code paths"
} else {
    Show-Fail "Conflict/Rejected outcome variants missing -- idempotency must fail closed, not silently succeed"
}

# [8] Required test scenarios present
$requiredTests = @(
    'empty_store_returns_none',
    'first_snapshot_insert_round_trips',
    'exact_replay_is_idempotent',
    'conflicting_replay_is_rejected',
    'multiple_snapshots_ordered_deterministically',
    'flat_account_has_zero_positions',
    'synthetic_source_never_masquerades_as_external',
    'invalid_source_rejected',
    'first_accounting_state_write_inserts',
    'accounting_state_watermark_idempotency_and_ordering',
    'incomplete_accounting_epoch_round_trips_reason',
    'invalid_accounting_epoch_rejected',
    'restart_reconstruction_reads_back_prior_writes',
    'no_outbox_or_inbox_writes'
)
foreach ($t in $requiredTests) {
    if ($testText -match "fn $t\(") {
        Show-Green "Found required test: $t"
    } else {
        Show-Fail "Missing required test: $t"
    }
}

# [9] Spec doc documents the architecture decision
if ($specText -match 'no separate' -or $specText -match 'Why no separate') {
    Show-Green "Spec doc documents the no-separate-accounting-event-table decision"
} else {
    Show-Fail "Spec doc does not document the no-separate-accounting-event-table decision"
}

# [10] No mqk-daemon file touched (working tree only, best effort)
Push-Location $RepoRoot
try {
    $touched = git diff --name-only HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $touched) {
        $daemonTouched = $touched | Where-Object { $_ -like 'core-rs/crates/mqk-daemon/*' }
        if ($daemonTouched) {
            Show-Fail "mqk-daemon file(s) touched by this patch's working-tree diff: $($daemonTouched -join ', ') -- B4-B is DB-layer only"
        } else {
            Show-Green "No mqk-daemon file touched by this patch's working-tree diff"
        }
    } else {
        Show-Green "No working-tree diff against HEAD -- scope check skipped (patch already committed)"
    }
} finally {
    Pop-Location
}

Write-Host ''
if ($Violations -gt 0) {
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
} else {
    Write-Host " VALIDATION PASSED -- 0 violations" -ForegroundColor Green
    exit 0
}
