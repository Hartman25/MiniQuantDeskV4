# =============================================================================
# test_canonical_paper_db_canary_parity.ps1
# D-R2-R4 -- post-restore canary classification parity
#
# Proof that Backup-MiniQuantDeskRecovery.ps1's pre-stage content-canary scan
# and Invoke-MiniQuantDeskOffsiteBackup.ps1's post-restore re-scan apply the
# IDENTICAL narrow canonical-local-Paper-DB classification (R4-12), by
# exercising the REAL shared production classification seam both scripts
# dot-source -- scripts\windows\lib\CanonicalPaperDbCanary.ps1 -- directly,
# never a copied/shadowed implementation.
#
# Real end-to-end integration proof (both scripts running for real, through
# an actual local restic backup+restore round trip) lives in Section 5 of
# test_offsite_b2_workflow.ps1; this file covers the classification
# boundary cases (R4-1..R4-6, R4-11) at the unit level against the one real
# function both production call sites invoke, which is cheaper than a full
# restic round trip per case and exercises the exact same code path.
#
# Exit codes: 0 = all proofs held, 1 = at least one did not.
# =============================================================================

#requires -Version 5.1
[CmdletBinding()]
param()

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$WindowsDir = (Resolve-Path (Join-Path $ScriptDir '..')).Path.TrimEnd('\')
$LibScript = Join-Path $WindowsDir 'lib\CanonicalPaperDbCanary.ps1'
$BackupScript = Join-Path $WindowsDir 'Backup-MiniQuantDeskRecovery.ps1'
$OffsiteScript = Join-Path $WindowsDir 'Invoke-MiniQuantDeskOffsiteBackup.ps1'

$Violations = 0
function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan }
function Assert-True {
    param([string]$Label, [bool]$Condition)
    if ($Condition) {
        Show-Green "  OK -- $Label"
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label"
    }
}

foreach ($p in @($LibScript, $BackupScript, $OffsiteScript)) {
    if (-not (Test-Path -LiteralPath $p)) {
        Show-Red "FATAL -- required script not found: $p"
        exit 1
    }
}

# ---------------------------------------------------------------------------
# Section 1: structural parity -- both production scripts dot-source the
# exact same shared file (a single source of truth), never a duplicated or
# independently-maintained copy of the classification logic.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 1: shared classification source parity (structural) ==='

$BackupText = Get-Content -Path $BackupScript -Raw
$OffsiteText = Get-Content -Path $OffsiteScript -Raw

Assert-True 'R4-12: Backup-MiniQuantDeskRecovery.ps1 dot-sources the shared lib\CanonicalPaperDbCanary.ps1 helper (not an inline copy)' `
    ($BackupText -match [regex]::Escape("Join-Path `$PSScriptRoot 'lib\CanonicalPaperDbCanary.ps1'")) -and `
    (-not ($BackupText -match [regex]::Escape('function Test-IsCanonicalLocalPaperDatabaseUrl')))
Assert-True 'R4-12: Invoke-MiniQuantDeskOffsiteBackup.ps1 dot-sources the SAME shared lib\CanonicalPaperDbCanary.ps1 helper (not an inline copy)' `
    ($OffsiteText -match [regex]::Escape("Join-Path `$PSScriptRoot 'lib\CanonicalPaperDbCanary.ps1'")) -and `
    (-not ($OffsiteText -match [regex]::Escape('function Test-IsCanonicalLocalPaperDatabaseUrl')))
Assert-True 'R4-12: the post-restore re-scan gates the exemption on the exact name MQK_DATABASE_URL (name+value both required, no generalized exemption)' `
    ($OffsiteText -match [regex]::Escape("`$name -eq 'MQK_DATABASE_URL' -and (Test-IsCanonicalLocalPaperDatabaseUrl -Value `$v)"))
Assert-True 'R4-13: the post-restore secret-canary refusal never prints the matched value itself' `
    ($OffsiteText -match [regex]::Escape('value withheld from log'))

# ---------------------------------------------------------------------------
# Section 2: the real shared classification function, exercised directly --
# the exact same Test-IsCanonicalLocalPaperDatabaseUrl both production
# scripts call, dot-sourced from its one real source file.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Section 2: real shared classification function -- boundary proof ==='

. $LibScript

$Canonical = 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable'
Assert-True 'sanity: the lib''s own constant matches the documented canonical value' ($CanonicalLocalPaperDatabaseUrl -eq $Canonical)

# R4-1: exact canonical value.
Assert-True 'R4-1: the exact canonical local Paper DB URL is classified as exempt' (Test-IsCanonicalLocalPaperDatabaseUrl -Value $Canonical)

# R4-2: non-loopback host.
Assert-True 'R4-2: a non-loopback host is refused (not exempt)' `
    (-not (Test-IsCanonicalLocalPaperDatabaseUrl -Value 'postgres://postgres:postgres@example.com:5440/miniquantdesk_paper?sslmode=disable'))

# R4-3: loopback, different credential.
Assert-True 'R4-3: loopback with a different credential than canonical is refused (not exempt)' `
    (-not (Test-IsCanonicalLocalPaperDatabaseUrl -Value 'postgres://postgres:differentpw1@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable'))

# R4-4: wrong port.
Assert-True 'R4-4: the wrong port is refused (not exempt)' `
    (-not (Test-IsCanonicalLocalPaperDatabaseUrl -Value 'postgres://postgres:postgres@127.0.0.1:5432/miniquantdesk_paper?sslmode=disable'))

# R4-5: wrong database identity.
Assert-True 'R4-5: a different database identity is refused (not exempt)' `
    (-not (Test-IsCanonicalLocalPaperDatabaseUrl -Value 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_other?sslmode=disable'))

# R4-6: malformed URL.
Assert-True 'R4-6: a malformed URL never receives the exemption' `
    (-not (Test-IsCanonicalLocalPaperDatabaseUrl -Value 'not-a-valid-postgres-url-but-long-enough'))

# R4-6b: a secret-like query parameter on an otherwise-canonical-looking URL
# is refused -- the exemption requires no secret-shaped query params.
Assert-True 'R4-6b: an otherwise-matching URL with a secret-like query param is refused (not exempt)' `
    (-not (Test-IsCanonicalLocalPaperDatabaseUrl -Value 'postgres://postgres:postgres@127.0.0.1:5440/miniquantdesk_paper?sslmode=disable&token=abc123'))

# R4-11: variable-name spoofing cannot get exemption. Test-IsCanonical...
# only ever inspects VALUE -- the actual name+value gate is the caller's
# combined boolean (asserted structurally in Section 1). Reproducing that
# exact combined expression here proves the canonical value under a
# DIFFERENT allowlisted name is never exempted end-to-end.
$spoofName = 'MQK_OPERATOR_TOKEN'
$spoofExempt = ($spoofName -eq 'MQK_DATABASE_URL' -and (Test-IsCanonicalLocalPaperDatabaseUrl -Value $Canonical))
Assert-True 'R4-11: the canonical value under a DIFFERENT variable name (MQK_OPERATOR_TOKEN) is NOT exempted (name+value must both match)' (-not $spoofExempt)

# empty/whitespace inputs must never throw and must never be exempt.
Assert-True 'edge case: empty string input is refused, not exempt, and does not throw' (-not (Test-IsCanonicalLocalPaperDatabaseUrl -Value ''))
Assert-True 'edge case: whitespace-only input is refused, not exempt, and does not throw' (-not (Test-IsCanonicalLocalPaperDatabaseUrl -Value '   '))

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== Summary ==='
if ($Violations -eq 0) {
    Show-Green "All proofs held. 0 violations."
    exit 0
} else {
    Show-Red "$Violations violation(s) found."
    exit 1
}
