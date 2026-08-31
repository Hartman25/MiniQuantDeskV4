# =============================================================================
# Script guard: test_dev_shell_dsn_mask.ps1
# DEV-SHELL-DSN-MASK-01
#
# Negative control: a fake credential-bearing DSN must never appear, whole
# or in its password component, in dev-shell.ps1's masked summary output.
# No daemon, no DB, no live calls, no .env.local, no real secrets.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = Split-Path -Parent (Split-Path -Parent $ScriptDir)
$DevShellPath = Join-Path $RepoRoot 'scripts\dev-shell.ps1'
$DsnMaskLibPath = Join-Path $RepoRoot 'scripts\lib\dsn-mask.ps1'

$Failures = 0

function Assert-True {
    param([bool]$Condition, [string]$Message)
    if ($Condition) {
        Write-Host "  PASS: $Message" -ForegroundColor Green
    } else {
        Write-Host "  FAIL: $Message" -ForegroundColor Red
        $script:Failures++
    }
}

function Assert-False {
    param([bool]$Condition, [string]$Message)
    Assert-True (-not $Condition) $Message
}

Write-Host ''
Write-Host '--- test_dev_shell_dsn_mask.ps1 ---'

Assert-True (Test-Path $DevShellPath) 'dev-shell.ps1 exists'
Assert-True (Test-Path $DsnMaskLibPath) 'scripts/lib/dsn-mask.ps1 exists'

$DevShellContent = Get-Content -Path $DevShellPath -Raw

# dev-shell.ps1 must route MQK_DATABASE_URL through the masking helper, not
# print the raw environment value directly.
Assert-True ($DevShellContent -match 'Get-SafeDsnSummary') 'dev-shell.ps1 calls the DSN masking helper'
Assert-False ($DevShellContent -match 'MQK_DATABASE_URL=\$env:MQK_DATABASE_URL') 'dev-shell.ps1 does not print the raw MQK_DATABASE_URL value'

. $DsnMaskLibPath

$FakeUser = 'fake_user'
$FakeSecret = 'SUPER_SECRET_TEST_VALUE'
$FakeDsn = "postgres://${FakeUser}:${FakeSecret}@localhost:5440/fake_db"

$Summary = Get-SafeDsnSummary $FakeDsn

Assert-False ($Summary -match [regex]::Escape($FakeSecret)) 'masked summary does not contain the fake password'
Assert-False ($Summary -eq $FakeDsn) 'masked summary is not the raw DSN'
Assert-True ($Summary -match [regex]::Escape($FakeUser)) 'masked summary still identifies the user'
Assert-True ($Summary -match 'localhost') 'masked summary still identifies the host'
Assert-True ($Summary -match '5440') 'masked summary still identifies the port'
Assert-True ($Summary -match 'fake_db') 'masked summary still identifies the database name'

# Malformed DSN must fail closed to a safe placeholder, never fall back to
# printing the raw unparsable value.
$MalformedDsn = "not-a-valid-dsn-$FakeSecret"
$MalformedSummary = Get-SafeDsnSummary $MalformedDsn
Assert-False ($MalformedSummary -match [regex]::Escape($FakeSecret)) 'malformed DSN summary does not leak the raw value'
Assert-True ($MalformedSummary -match 'cannot be safely summarized') 'malformed DSN produces an explicit safe-fallback message'

# Empty/unset must not error and must not fabricate a value.
$EmptySummary = Get-SafeDsnSummary ''
Assert-True ($EmptySummary -eq '(not set)') 'empty DSN reports (not set)'

if ($Failures -eq 0) {
    Write-Host '  ALL ASSERTIONS PASSED (test_dev_shell_dsn_mask)' -ForegroundColor Green
    exit 0
}

Write-Host "  $Failures ASSERTION(S) FAILED (test_dev_shell_dsn_mask)" -ForegroundColor Red
exit 1
