# =============================================================================
# P0-1: MiniQuantDesk V4 — Deterministic Unsafe-Pattern Guard (PowerShell)
# =============================================================================
# Windows companion to check_unsafe_patterns.sh.
# Identical logic, identical exit codes, identical patterns.
# The .sh version runs in GitHub Actions CI (ubuntu-latest).
# This .ps1 version runs locally on Windows for pre-commit verification.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_unsafe_patterns.ps1
#   # or from PowerShell directly:
#   & scripts\guards\check_unsafe_patterns.ps1
#
# Exit codes: 0 = clean, 1 = violations found.
# =============================================================================

$ErrorActionPreference = "Stop"

# Resolve repo root (two levels up from this script's directory).
$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot   = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$Violations = 0

function Write-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Write-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Write-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

Write-Host "============================================================"
Write-Host " MQK P0-1 Safety Guard (PowerShell)"
Write-Host " Repo root: $RepoRoot"
Write-Host "============================================================"

# =============================================================================
# [U] Uuid::new_v4 in production src/ files
# Pattern catches both: Uuid::new_v4() and unwrap_or_else(Uuid::new_v4)
# =============================================================================

Write-Host ""
Write-Info "--- [U] Uuid::new_v4 in production src/ ---"

$UuidFileCount = 0
$UuidMatchLines = @()

# Find all .rs files under crates/*/src/ (not tests/, not target/).
$SrcFiles = Get-ChildItem -Path "$RepoRoot\core-rs\crates" -Recurse -Filter "*.rs" |
    Where-Object {
        $_.FullName -match '\\src\\' -and
        $_.FullName -notmatch '\\tests\\' -and
        $_.FullName -notmatch '\\target\\'
    }

foreach ($File in $SrcFiles) {
    $Matches = Select-String -Path $File.FullName -Pattern "Uuid::new_v4" -SimpleMatch
    if ($Matches) {
        $UuidFileCount++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Match in $Matches) {
            $UuidMatchLines += "  ${RelPath}:$($Match.LineNumber):$($Match.Line.Trim())"
        }
    }
}

if ($UuidFileCount -eq 0) {
    Write-Green "  OK — no Uuid::new_v4 in production src/"
} else {
    $Violations += $UuidFileCount
    Write-Red "  FAIL — Uuid::new_v4 found in $UuidFileCount production file(s):"
    $UuidMatchLines | ForEach-Object { Write-Host $_ }
    Write-Red "  Remediation: D1-1 (run IDs: daemon routes + cli), D1-2 (audit event IDs)."
    Write-Red "  Note: mqk-db/src/md.rs ingest_id fallback also flagged — address in D1-1 or separately."
}

# =============================================================================
# [T] Utc::now() in mqk-db/src/ (enforcement scope)
# =============================================================================

Write-Host ""
Write-Info "--- [T] Utc::now() in mqk-db/src/ (enforcement scope) ---"

$UtcFileCount  = 0
$UtcMatchLines = @()

$MqkDbSrc = "$RepoRoot\core-rs\crates\mqk-db\src"

if (Test-Path $MqkDbSrc) {
    $DbSrcFiles = Get-ChildItem -Path $MqkDbSrc -Recurse -Filter "*.rs" |
        Where-Object { $_.FullName -notmatch '\\target\\' }

    foreach ($File in $DbSrcFiles) {
        $Matches = Select-String -Path $File.FullName -Pattern "Utc::now()" -SimpleMatch
        if ($Matches) {
            $UtcFileCount++
            $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
            foreach ($Match in $Matches) {
                $UtcMatchLines += "  ${RelPath}:$($Match.LineNumber):$($Match.Line.Trim())"
            }
        }
    }
}

if ($UtcFileCount -eq 0) {
    Write-Green "  OK — no Utc::now() in mqk-db/src/"
} else {
    $Violations += $UtcFileCount
    Write-Red "  FAIL — Utc::now() found in $UtcFileCount file(s) in mqk-db/src/:"
    $UtcMatchLines | ForEach-Object { Write-Host $_ }
    Write-Red "  Remediation: D1-3 (inject TimeSource abstraction into deadman)."
}

# =============================================================================
# TODO(D1-4): DEFAULT now() in SQL migrations — disabled (see .sh for rationale)
# =============================================================================

Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Write-Green " ALL GUARDS PASSED — no forbidden patterns detected."
    exit 0
} else {
    Write-Red " GUARD FAILED — $Violations violation(s) found."
    Write-Host ""
    Write-Red " These are known tracked violations. Remediation patches:"
    Write-Red "   D1-1: Uuid::new_v4 in daemon routes + cli run command"
    Write-Red "   D1-2: Uuid::new_v4 in audit event IDs"
    Write-Red "   D1-3: Utc::now() in mqk-db deadman enforcement path"
    Write-Red "   D1-4: DEFAULT now() in SQL migrations (guard disabled until then)"
    exit 1
}
