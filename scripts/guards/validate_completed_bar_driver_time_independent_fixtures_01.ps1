# =============================================================================
# COMPLETED-BAR-DRIVER-TIME-INDEPENDENT-FIXTURES-01 -- Source-aware static validator
# validate_completed_bar_driver_time_independent_fixtures_01.ps1
#
# Scope: core-rs/crates/mqk-daemon/tests/scenario_autonomous_completed_bar_driver_01.rs
# and docs/specs/completed_bar_driver_time_independent_fixtures_01.md only.
# Pure text/source validation -- no cargo build/test/compile, no DB
# connection, no network call. Running the actual scenario binary (both
# --test-threads=1 and default parallelism, against the isolated port-5434
# test DB) is a separate, required step of the B4-0 mission, not performed
# by this guard.
#
# Checks:
#   [1]  The scenario test file and this patch's spec doc both exist.
#   [2]  `standard_timing()` derives its anchor from `Utc::now()` at call
#        time (no fixed calendar-date anchor for the "current" session).
#   [3]  `standard_timing()` floors its anchor to a 5-minute boundary (the
#        provider-poll seam floors `now_utc` the same way for "5m" targets;
#        an unaligned anchor produces spurious future-skew rejections).
#   [4]  `past_timing()` remains fixed/historical (2020-01-06) -- it exists
#        specifically to prove the staleness gate still fires against a
#        genuinely old bar, and must never be time-independent.
#   [5]  No absolute Unix-epoch-second literal (9-10 digit integer) appears
#        anywhere in the file -- every expected bar timestamp must be
#        derived from the `Timing` fixture, never hardcoded.
#   [6]  No test in the file is `#[ignore]`d.
#   [7]  No sleep-based timing (`std::thread::sleep`, `tokio::time::sleep`)
#        is used to paper over the staleness gate.
#   [8]  The one-time fixture-sweep guard (`OnceCell`) is present in
#        `maybe_db()`, preventing the cross-test DB race where one test's
#        wildcard cleanup could delete another concurrently-running test's
#        fixture rows (or permanently orphan its event rows).
#   [9]  `git diff --stat` (working tree only) touches only the scenario
#        test file for this patch scope -- no production `src/` file, no
#        migration, no GUI file.
#
# Usage:
#   powershell -NoProfile -ExecutionPolicy Bypass `
#       -File scripts\guards\validate_completed_bar_driver_time_independent_fixtures_01.ps1
#
# Exit codes: 0 = all checks pass, 1 = at least one violation.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '../../')).Path.TrimEnd('\')
$TestFile  = Join-Path $RepoRoot 'core-rs\crates\mqk-daemon\tests\scenario_autonomous_completed_bar_driver_01.rs'
$SpecDoc   = Join-Path $RepoRoot 'docs\specs\completed_bar_driver_time_independent_fixtures_01.md'

$Violations = 0

function Show-Fail  { param([string]$M) Write-Host "  FAIL -- $M" -ForegroundColor Red   ; $script:Violations++ }
function Show-Green { param([string]$M) Write-Host "  OK -- $M"   -ForegroundColor Green }
function Show-Info  { param([string]$M) Write-Host $M             -ForegroundColor Cyan  }

Write-Host '============================================================'
Write-Host ' COMPLETED-BAR-DRIVER-TIME-INDEPENDENT-FIXTURES-01 validator'
Write-Host " Test file : $TestFile"
Write-Host " Spec doc  : $SpecDoc"
Write-Host '============================================================'

# [1] Files exist
if (-not (Test-Path -LiteralPath $TestFile)) {
    Show-Fail "Scenario test file not found: $TestFile"
} else {
    Show-Green "Scenario test file found"
}
if (-not (Test-Path -LiteralPath $SpecDoc)) {
    Show-Fail "Spec doc not found: $SpecDoc"
} else {
    Show-Green "Spec doc found"
}

if ($Violations -gt 0) {
    Write-Host ''
    Write-Host " VALIDATION FAILED -- $Violations violation(s) found" -ForegroundColor Red
    exit 1
}

$text = [System.IO.File]::ReadAllText($TestFile)

# [2] standard_timing() anchors to Utc::now()
if ($text -match 'fn standard_timing\(\)[\s\S]{0,400}?let now = Utc::now\(\)') {
    Show-Green "standard_timing() anchors to Utc::now() at call time"
} else {
    Show-Fail "standard_timing() does not appear to anchor to Utc::now() -- check for a reintroduced fixed calendar date"
}

if ($text -match 'fn standard_timing\(\)[\s\S]{0,2000}?NaiveDate::from_ymd_opt\(2026') {
    Show-Fail "standard_timing() still contains a hardcoded 2026 calendar date literal"
} else {
    Show-Green "standard_timing() contains no hardcoded 2026 calendar date literal"
}

# [3] 5-minute boundary alignment
if ($text -match 'div_euclid\(300\)') {
    Show-Green "standard_timing() floors its anchor to a 5-minute boundary"
} else {
    Show-Fail "standard_timing() does not floor its anchor to a 5-minute boundary -- provider-poll future-skew check may spuriously reject"
}

# [4] past_timing() remains fixed/historical
if ($text -match 'fn past_timing\(\)[\s\S]{0,400}?NaiveDate::from_ymd_opt\(2020,\s*1,\s*6\)') {
    Show-Green "past_timing() remains fixed at 2020-01-06 (deliberately historical)"
} else {
    Show-Fail "past_timing() no longer anchors to the fixed 2020-01-06 date -- the staleness-gate-fires proof requires a genuinely old, non-time-independent bar"
}

# [5] No absolute epoch literal anywhere in the file
$epochLiteralMatches = [regex]::Matches($text, '(?<![\w.])\d{9,10}(?![\w.])')
if ($epochLiteralMatches.Count -gt 0) {
    Show-Fail "Found $($epochLiteralMatches.Count) absolute 9-10 digit numeric literal(s) -- expected bar timestamps must be derived from the Timing fixture, never hardcoded"
} else {
    Show-Green "No absolute epoch-second literal found -- all expected timestamps are fixture-derived"
}

# [6] No #[ignore]
if ($text -match '#\[ignore\]') {
    Show-Fail "Found #[ignore] -- no scenario may be ignored"
} else {
    Show-Green "No ignored test found"
}

# [7] No sleep-based timing
if ($text -match 'thread::sleep|tokio::time::sleep') {
    Show-Fail "Found sleep-based timing -- staleness must be proven deterministically, not by waiting"
} else {
    Show-Green "No sleep-based timing found"
}

# [8] One-time fixture-sweep guard present
if ($text -match 'ZZDRV_FIXTURES_SWEPT' -and $text -match 'OnceCell') {
    Show-Green "One-time fixture-sweep guard (OnceCell) present in maybe_db()"
} else {
    Show-Fail "maybe_db() does not appear to guard its wildcard cleanup with a one-time OnceCell -- cross-test DB race risk under parallel execution"
}

# [9] Working-tree diff scope (best effort -- only meaningful when run inside
# a git checkout with this patch's changes still uncommitted or freshly
# committed as the tip commit).
Push-Location $RepoRoot
try {
    $diffFiles = git diff --stat HEAD 2>$null
    if ($LASTEXITCODE -eq 0 -and $diffFiles) {
        $touched = git diff --name-only HEAD 2>$null
        $unexpected = $touched | Where-Object {
            $_ -and ($_ -ne 'core-rs/crates/mqk-daemon/tests/scenario_autonomous_completed_bar_driver_01.rs')
        }
        if ($unexpected) {
            Show-Fail "Unexpected file(s) in working-tree diff for this patch scope: $($unexpected -join ', ')"
        } else {
            Show-Green "Working-tree diff (if any) is scoped to the scenario test file only"
        }
    } else {
        Show-Info "No working-tree diff against HEAD -- scope check skipped (patch already committed)"
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
