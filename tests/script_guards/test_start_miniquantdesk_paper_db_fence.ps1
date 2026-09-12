# =============================================================================
# M1-PAPER-READINESS-WAVE-01
# Script guard: test_start_miniquantdesk_paper_db_fence.ps1
#
# Static assertions against scripts\windows\Start-MiniQuantDesk.ps1's
# "PAPER DB HARD FENCE" -- the unconditional reassert of MQK_DATABASE_URL to
# the accepted paper DB literal immediately before any DB-dependent step.
# Before this guard, Start-MiniQuantDesk.ps1 had zero script-guard coverage
# proving that a contaminated shell (e.g. MQK_DATABASE_URL pointing at the
# test DB on 5434, or the live DB on 5432) cannot become effective Paper DB
# authority.
#
# No daemon, no DB, no live calls, no .env.local required. All checks are
# read-only source inspections.
#
# Guard assertions:
#   PDBF01  Script exists at scripts\windows\Start-MiniQuantDesk.ps1
#   PDBF02  Paper DB literal uses port 5440
#   PDBF03  Paper DB literal never mentions port 5432 or 5434
#   PDBF04  MQK_DATABASE_URL reassert to the paper literal is unconditional
#           (not gated behind an "if not already set" check)
#   PDBF05  Reassert happens after Postgres-readiness confirmation (ordering)
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path
$Target    = Join-Path $RepoRoot 'scripts\windows\Start-MiniQuantDesk.ps1'

$Passed = 0
$Failed = 0

function Assert-True {
    param([string]$Id, [string]$Description, [bool]$Condition)
    if ($Condition) {
        Write-Host "  PASS [$Id] $Description" -ForegroundColor Green
        $script:Passed++
    } else {
        Write-Host "  FAIL [$Id] $Description" -ForegroundColor Red
        $script:Failed++
    }
}

Write-Host ""
Write-Host "=== test_start_miniquantdesk_paper_db_fence.ps1 (M1-PAPER-READINESS-WAVE-01) ===" -ForegroundColor Cyan
Write-Host "    Target: $Target"
Write-Host ""

if (-not (Test-Path $Target)) {
    Write-Host "  FAIL [PDBF01] Script not found: $Target" -ForegroundColor Red
    exit 1
}
Assert-True 'PDBF01' 'Script exists at scripts\windows\Start-MiniQuantDesk.ps1' $true

$Content = Get-Content $Target -Raw

# PDBF02: paper DB literal uses port 5440
$paperDbLiteralMatch = [regex]::Match($Content, "\`$paperDbUrl\s*=\s*'([^']+)'")
Assert-True 'PDBF02' 'Paper DB literal ($paperDbUrl) uses port 5440' `
    ($paperDbLiteralMatch.Success -and $paperDbLiteralMatch.Groups[1].Value -match ':5440/')

# PDBF03: paper DB literal never mentions port 5432 or 5434
Assert-True 'PDBF03' 'Paper DB literal never mentions port 5432 or 5434 (test/live DB ports)' `
    ($paperDbLiteralMatch.Success -and
     $paperDbLiteralMatch.Groups[1].Value -notmatch ':5432' -and
     $paperDbLiteralMatch.Groups[1].Value -notmatch ':5434')

# PDBF04: the reassert is unconditional -- not gated behind an
# "if not already set" style check on MQK_DATABASE_URL. We look at the
# statement immediately assigning $env:MQK_DATABASE_URL = $paperDbUrl and
# confirm it is a bare assignment, not the body of a conditional that tests
# whether MQK_DATABASE_URL is empty/unset first.
$reassertMatch = [regex]::Match($Content, '(?s)(.{0,200})\$env:MQK_DATABASE_URL\s*=\s*\$paperDbUrl')
Assert-True 'PDBF04' 'MQK_DATABASE_URL reassert to the paper literal is unconditional (not gated on "if not set")' `
    ($reassertMatch.Success -and
     $reassertMatch.Groups[1].Value -notmatch 'if\s*\(\s*-not\s*\$env:MQK_DATABASE_URL' -and
     $reassertMatch.Groups[1].Value -notmatch 'if\s*\(\s*\[string\]::IsNullOrWhiteSpace\(\s*\$env:MQK_DATABASE_URL')

# PDBF05: reassert happens after Postgres-readiness confirmation (ordering) --
# mirrors the existing OPR03 pattern for Start-PaperTradingSmoke.ps1.
$pgReadyIdx   = $Content.IndexOf('Postgres is ready inside')
$reassertIdx  = $Content.IndexOf('$env:MQK_DATABASE_URL = $paperDbUrl')
Assert-True 'PDBF05' 'Reassert happens after Postgres-readiness confirmation, not before' `
    ($pgReadyIdx -ge 0 -and $reassertIdx -gt $pgReadyIdx)

# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "=== Results ===" -ForegroundColor Cyan
Write-Host "  Passed: $Passed" -ForegroundColor Green
Write-Host "  Failed: $Failed" -ForegroundColor $(if ($Failed -gt 0) { 'Red' } else { 'Green' })
Write-Host ""

if ($Failed -gt 0) {
    Write-Host "GUARD FAILED: $Failed assertion(s) failed." -ForegroundColor Red
    exit 1
} else {
    Write-Host "GUARD PASSED: all $Passed assertions passed." -ForegroundColor Green
    exit 0
}
