# =============================================================================
# CI-616 -- Windows canonical formatter command-length guard
#
# Proves scripts/windows/Invoke-CanonicalFmtCheck.ps1's Windows branch no
# longer relies on a single oversized `cargo fmt -p <package>` invocation
# (mqk-daemon exceeds the CreateProcess limit: OS error 206), and stays
# fail-closed. Pure static content checks; no cargo, no DB, no secrets.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_canonical_fmt_check_command_length.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

param([string]$FmtScriptPath = '')

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
if ([string]::IsNullOrWhiteSpace($FmtScriptPath)) {
    $FmtScriptPath = Join-Path $RepoRoot 'scripts\windows\Invoke-CanonicalFmtCheck.ps1'
}

$Failures = 0
function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ''
Write-Host '=== Canonical fmt command-length guard (CI-616) ==='

if (-not (Test-Path -LiteralPath $FmtScriptPath)) {
    Fail 'FC01' "formatter script not found: $FmtScriptPath"
    exit 1
}
Pass 'FC01' 'formatter script exists'

# Code lines only: comments may legitimately describe the old workaround.
$code = (Get-Content -LiteralPath $FmtScriptPath | Where-Object { $_ -notmatch '^\s*#' }) -join "`n"

if ($code -match '(?m)fmt\b[^\n]*\s-p\s') {
    Fail 'FC02' 'script still invokes `cargo fmt ... -p <package>` (unbounded per-package command line)'
} else {
    Pass 'FC02' 'no per-package `cargo fmt -p` invocation'
}

$required = @(
    @{ Id = 'FC03.metadata'; Pattern = 'metadata\s+--format-version\s+1\s+--no-deps'; Msg = 'enumerates targets via cargo metadata --no-deps' },
    @{ Id = 'FC03.rustfmt';  Pattern = '\$RustfmtExe\s+--edition\s+\$batch\.Edition\s+--check'; Msg = 'runs rustfmt --check with the package edition' },
    @{ Id = 'FC03.bound';    Pattern = '\$MaxBatchChars\s*=\s*\d+'; Msg = 'declares a bounded batch size' },
    @{ Id = 'FC04.zero';     Pattern = 'no formattable targets'; Msg = 'zero targets is an error' },
    @{ Id = 'FC04.missing';  Pattern = 'refusing to skip it'; Msg = 'unreadable target root fails closed' },
    @{ Id = 'FC05.linux';    Pattern = 'fmt --manifest-path \$CargoManifest --all -- --check'; Msg = 'non-Windows lane stays canonical cargo fmt --all --check' }
)
foreach ($r in $required) {
    if ($code -match $r.Pattern) { Pass $r.Id $r.Msg } else { Fail $r.Id "missing: $($r.Msg)" }
}

if ($Failures -gt 0) {
    Write-Host "FAILED: $Failures check(s)" -ForegroundColor Red
    exit 1
}
Write-Host 'ALL CHECKS PASSED' -ForegroundColor Green
exit 0
