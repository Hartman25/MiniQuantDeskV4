# =============================================================================
# Windows wrapper for tests/script_guards/test_ci_local_toolchain_convergence.sh
#
# FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 4: the script
# guard aggregator (run_all_script_guards.ps1) only invokes .ps1 guards
# directly; this thin wrapper lets the bash mutation-negative test suite for
# the CI/local toolchain convergence guard run under that same aggregator on
# Windows, using a resolved Git/MSYS bash (never the WSL bash.exe shim).
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_ci_local_toolchain_convergence.ps1
#
# Exit codes: passthrough from the underlying bash test suite.
# =============================================================================

$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$ShTest = Join-Path $ScriptDir 'test_ci_local_toolchain_convergence.sh'

function Resolve-Bash {
    $candidates = [System.Collections.Generic.List[string]]::new()

    $gitCmd = Get-Command git -ErrorAction SilentlyContinue
    if ($gitCmd -and $gitCmd.Source) {
        $gitDir = Split-Path -Parent $gitCmd.Source
        $gitRoot = Split-Path -Parent $gitDir
        $candidates.Add((Join-Path $gitRoot 'bin\bash.exe'))
        $candidates.Add((Join-Path $gitRoot 'usr\bin\bash.exe'))
    }

    foreach ($root in @($env:ProgramFiles, $env:ProgramW6432, ${env:ProgramFiles(x86)})) {
        if (-not [string]::IsNullOrWhiteSpace($root)) {
            $candidates.Add((Join-Path $root 'Git\bin\bash.exe'))
            $candidates.Add((Join-Path $root 'Git\usr\bin\bash.exe'))
        }
    }

    $pathBash = Get-Command bash -ErrorAction SilentlyContinue
    if ($pathBash -and $pathBash.Source) {
        $candidates.Add($pathBash.Source)
    }

    foreach ($candidate in $candidates) {
        if ([string]::IsNullOrWhiteSpace($candidate)) { continue }
        if (-not (Test-Path -LiteralPath $candidate)) { continue }
        $normalized = [System.IO.Path]::GetFullPath($candidate)
        $lower = $normalized.ToLowerInvariant()
        if ($lower -like '*\windows\system32\bash.exe') { continue }
        if ($lower -like '*\git\bin\bash.exe' -or $lower -like '*\git\usr\bin\bash.exe' -or
            $lower -like '*\mingw*\bash.exe' -or $lower -like '*\msys*\bash.exe') {
            return $normalized
        }
    }

    throw 'Unable to resolve a Windows-compatible Git/MSYS bash to run test_ci_local_toolchain_convergence.sh.'
}

if (-not (Test-Path -LiteralPath $ShTest)) {
    Write-Host "MISSING: $ShTest" -ForegroundColor Red
    exit 1
}

$bashExe = Resolve-Bash
$relative = './' + ($ShTest.Substring($RepoRoot.Length).TrimStart('\', '/') -replace '\\', '/')

Push-Location $RepoRoot
$exitCode = 1
try {
    & $bashExe --noprofile --norc $relative
    $exitCode = $LASTEXITCODE
}
finally {
    Pop-Location
}
exit $exitCode
