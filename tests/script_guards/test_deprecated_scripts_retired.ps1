# =============================================================================
# CI-DEPRECATED-SCRIPTS-RETIREMENT-01 -- deprecated script retirement guard
#
# Proves DSR01-DSR08: ci_gate.ps1, test-all.ps1, and test-db.ps1 are retired
# hard-fail wrappers that cannot be mistaken for valid proof harnesses.
#
# No daemon, no DB, no live calls. Pure static content and exit-code checks.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_deprecated_scripts_retired.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

$DeprecatedScripts = @(
    @{ Id = 'A'; Name = 'ci_gate.ps1';  RelPath = 'scripts\ci_gate.ps1' },
    @{ Id = 'B'; Name = 'test-all.ps1'; RelPath = 'scripts\test-all.ps1' },
    @{ Id = 'C'; Name = 'test-db.ps1';  RelPath = 'scripts\test-db.ps1' }
)

Write-Host ""
Write-Host "=== Deprecated script retirement guard (CI-DEPRECATED-SCRIPTS-RETIREMENT-01) ==="
Write-Host ""

foreach ($s in $DeprecatedScripts) {
    $id   = $s.Id
    $name = $s.Name
    $path = Join-Path $RepoRoot $s.RelPath

    Write-Host "  -- $name --"

    # -------------------------------------------------------------------------
    # DSR01: Script exists as a retained hard-fail wrapper.
    # -------------------------------------------------------------------------
    if (-not (Test-Path $path)) {
        Fail "DSR01.$id" "$name not found at $path -- must exist as hard-fail wrapper or all references must be removed"
        Write-Host ""
        continue
    }
    Pass "DSR01.$id" "$name exists at $($s.RelPath)"

    $text  = Get-Content -Path $path -Raw
    $lines = Get-Content -Path $path

    # -------------------------------------------------------------------------
    # DSR02: Script contains "DEPRECATED" label and "full_repo_proof" hint.
    # -------------------------------------------------------------------------
    if ($text -match 'DEPRECATED') {
        Pass "DSR02.$id.label" "$name contains DEPRECATED label"
    } else {
        Fail "DSR02.$id.label" "$name missing DEPRECATED label"
    }

    if ($text -match 'full_repo_proof') {
        Pass "DSR02.$id.hint" "$name contains full_repo_proof replacement hint"
    } else {
        Fail "DSR02.$id.hint" "$name missing full_repo_proof replacement hint"
    }

    # -------------------------------------------------------------------------
    # DSR03: Script exits nonzero by default.
    # Invoke the script in a child process and verify exit code != 0.
    # -------------------------------------------------------------------------
    $exitCode = $null
    try {
        & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $path *>$null
        $exitCode = $LASTEXITCODE
    } catch {
        $exitCode = $LASTEXITCODE
    }

    if ($exitCode -ne 0) {
        Pass "DSR03.$id" "$name exits nonzero (exit=$exitCode) -- hard-fail confirmed"
    } else {
        Fail "DSR03.$id" "$name exits 0 -- must exit nonzero to prevent accidental use"
    }

    # -------------------------------------------------------------------------
    # DSR04: Script does not call cargo test directly.
    # Filter out comment lines (lines starting with optional whitespace + #).
    # -------------------------------------------------------------------------
    $cargoTestLines = $lines | Where-Object {
        $_ -match 'cargo\s+test' -and $_ -notmatch '^\s*#'
    }
    if (-not $cargoTestLines) {
        Pass "DSR04.$id" "$name does not call cargo test directly"
    } else {
        Fail "DSR04.$id" "$name still calls cargo test on line(s): $($cargoTestLines -join '; ')"
    }

    # -------------------------------------------------------------------------
    # DSR05: Script does not load .env.local.
    # -------------------------------------------------------------------------
    $envLocalLines = $lines | Where-Object {
        $_ -match '\.env\.local' -and $_ -notmatch '^\s*#'
    }
    if (-not $envLocalLines) {
        Pass "DSR05.$id" "$name does not load .env.local"
    } else {
        Fail "DSR05.$id" "$name references .env.local on line(s): $($envLocalLines -join '; ')"
    }

    # -------------------------------------------------------------------------
    # DSR06: Script does not embed hardcoded DB URLs as defaults.
    # Checks for postgres:// connection strings outside comments.
    # -------------------------------------------------------------------------
    $dbUrlLines = $lines | Where-Object {
        $_ -match 'postgres://' -and $_ -notmatch '^\s*#'
    }
    if (-not $dbUrlLines) {
        Pass "DSR06.$id" "$name does not embed hardcoded postgres:// URL defaults"
    } else {
        Fail "DSR06.$id" "$name embeds postgres:// URL on line(s): $($dbUrlLines -join '; ')"
    }

    # -------------------------------------------------------------------------
    # DSR07: Script is PowerShell 5.1 parse-safe.
    # Uses the PS language parser -- zero parse errors required.
    # -------------------------------------------------------------------------
    $parseErrors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile($path, [ref]$null, [ref]$parseErrors)
    if ($parseErrors.Count -eq 0) {
        Pass "DSR07.$id" "$name parses without errors (PS5.1 safe)"
    } else {
        $errMsgs = ($parseErrors | ForEach-Object { $_.Message }) -join '; '
        Fail "DSR07.$id" "$name has PS parse error(s): $errMsgs"
    }

    # -------------------------------------------------------------------------
    # DSR08: Script contains no non-ASCII bytes (no mojibake or smart quotes).
    # -------------------------------------------------------------------------
    $bytes = [System.IO.File]::ReadAllBytes($path)
    $nonAscii = $bytes | Where-Object { $_ -gt 127 }
    if (-not $nonAscii) {
        Pass "DSR08.$id" "$name is pure ASCII (no mojibake or non-ASCII characters)"
    } else {
        Fail "DSR08.$id" "$name contains $($nonAscii.Count) non-ASCII byte(s) -- encoding issue"
    }

    Write-Host ""
}

# =============================================================================
# Summary
# =============================================================================
if ($Failures -eq 0) {
    Write-Host "=== ALL DSR INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
