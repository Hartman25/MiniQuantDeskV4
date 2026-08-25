# =============================================================================
# test_offsite_restic_capture.ps1
# D-R2-R3 -- reliable bounded restic stdout/stderr output capture
#
# Proof for the REAL, unshadowed Invoke-ResticCommand function inside
# Invoke-MiniQuantDeskOffsiteBackup.ps1 -- dot-sources that file (its own
# MAIN WORKFLOW guard, `if ($MyInvocation.InvocationName -ne '.')`, keeps
# the real B2/restic workflow from running) so this test calls the exact
# production function, never a copy or a shadowed re-implementation.
#
# Because ResticPath/ResticArgs are plain parameters, the "child process"
# exercised here is a small harmless PowerShell fixture script standing in
# for restic -- this proves the capture mechanism itself (early/delayed/
# final stdout lines, stderr, exit code, hung-child bound), independent of
# whether restic itself is installed on this box.
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

if (-not (Test-Path -LiteralPath $OffsiteScript)) {
    Show-Red "FATAL -- required script not found: $OffsiteScript"
    exit 1
}

# ---------------------------------------------------------------------------
# Load the REAL production Invoke-ResticCommand (and its sibling functions)
# by dot-sourcing the actual offsite script -- its MAIN WORKFLOW guard
# prevents any real restic/B2 invocation from happening as a side effect.
# ---------------------------------------------------------------------------
. $OffsiteScript

$PowershellExe = (Get-Command 'powershell.exe').Source
$ScratchRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("mqk_restic_capture_test_" + [guid]::NewGuid().ToString('N'))
New-Item -ItemType Directory -Force -Path $ScratchRoot | Out-Null

function New-FixtureScript {
    param([Parameter(Mandatory = $true)][string]$Name, [Parameter(Mandatory = $true)][string]$Body)
    $path = Join-Path $ScratchRoot $Name
    Set-Content -Path $path -Value $Body -Encoding UTF8
    return $path
}

# ---------------------------------------------------------------------------
# R3-1 / R3-2 / R3-3: early stdout, a DELAYED final stdout sentinel line,
# stderr, and an exact (non-zero, to prove fidelity rather than a
# coincidental 0) exit code are all captured. This is the exact shape of
# the real defect: restic's final --json summary line (containing
# snapshot_id) arriving after a short delay was reproducibly dropped 2/2
# times against real B2 by the prior Register-ObjectEvent mechanism.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== R3-1/R3-2/R3-3: early + delayed-final stdout, stderr, and exact exit code ==='

$delayedScript = New-FixtureScript -Name 'delayed_output.ps1' -Body @'
Write-Output "EARLY_LINE_ONE"
Start-Sleep -Milliseconds 400
[Console]::Error.WriteLine("STDERR_LINE")
Start-Sleep -Milliseconds 400
Write-Output "FINAL_SENTINEL_LINE_42"
exit 7
'@

$delayedResult = Invoke-ResticCommand -ResticPath $PowershellExe `
    -ResticArgs @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $delayedScript) `
    -ResticEnv @{} -TimeoutSeconds 30

Assert-True 'R3-1: an early stdout line is captured' ($delayedResult.Stdout -match 'EARLY_LINE_ONE')
Assert-True 'R3-1: the DELAYED final stdout sentinel line is captured (the exact defect shape -- a late line arriving after a sleep)' `
    ($delayedResult.Stdout -match 'FINAL_SENTINEL_LINE_42')
Assert-True 'R3-2: stderr is captured' ($delayedResult.Stderr -match 'STDERR_LINE')
Assert-True 'R3-3: the exact (non-zero) process exit code is preserved' ($delayedResult.ExitCode -eq 7)
Assert-True 'sanity: TimedOut is false on a normal, timely exit' ($delayedResult.TimedOut -eq $false)
Assert-True 'sanity: CaptureFailed is false when the drain completes normally' ($delayedResult.CaptureFailed -eq $false)

# ---------------------------------------------------------------------------
# R3-4: a harmless hung child is still bounded -- Invoke-ResticCommand must
# return within roughly the configured timeout, never wait for the child's
# full (much longer) sleep.
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== R3-4: harmless hung child remains bounded ==='

$hungScript = New-FixtureScript -Name 'hung.ps1' -Body @'
Write-Output "STARTED"
Start-Sleep -Seconds 120
Write-Output "NEVER_REACHED"
'@

$boundedTimeoutSeconds = 2
$sw = [System.Diagnostics.Stopwatch]::StartNew()
$hungResult = Invoke-ResticCommand -ResticPath $PowershellExe `
    -ResticArgs @('-NoProfile', '-NonInteractive', '-ExecutionPolicy', 'Bypass', '-File', $hungScript) `
    -ResticEnv @{} -TimeoutSeconds $boundedTimeoutSeconds
$sw.Stop()

Assert-True 'R3-4: a hung child reports TimedOut=true' ($hungResult.TimedOut -eq $true)
Assert-True 'R3-4: a hung child reports a nonzero ExitCode' ($hungResult.ExitCode -ne 0)
Assert-True 'R3-4: the call returns within a small bounded interval, not anywhere near the child''s 120s sleep (proves R3-5: no blocking ReadToEnd before WaitForExit -- a blocking read here would hang for the full 120s)' `
    ($sw.Elapsed.TotalSeconds -lt ($boundedTimeoutSeconds + 15))
Assert-True 'R3-4: NEVER_REACHED does not appear in captured stdout (the child was actually killed before writing it)' `
    (-not ($hungResult.Stdout -match 'NEVER_REACHED'))

# ---------------------------------------------------------------------------
# R3-5 / R3-6: static proof against the real production source -- no
# blocking ReadToEnd before WaitForExit, and the Register-ObjectEvent /
# BeginOutputReadLine event-subscriber race is completely gone (the same
# static assertions also live in test_offsite_b2_workflow.ps1; repeated
# here so this file is a self-contained D-R2-R3 proof on its own).
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== R3-5/R3-6: static proof -- no blocking ReadToEnd, no event-subscriber race ==='

$OffsiteText = Get-Content -Path $OffsiteScript -Raw
Assert-True 'R3-5: no synchronous/blocking .ReadToEnd() call anywhere in the offsite script' `
    (-not ($OffsiteText -match [regex]::Escape('.ReadToEnd()')))
Assert-True 'R3-5: ReadToEndAsync is started before WaitForExit is called' `
    ($OffsiteText.IndexOf('$process.StandardOutput.ReadToEndAsync()') -lt $OffsiteText.IndexOf('$finished = $process.WaitForExit($TimeoutSeconds * 1000)'))
Assert-True 'R3-6: Register-ObjectEvent is no longer used anywhere in the offsite script' `
    (-not ($OffsiteText -match [regex]::Escape('Register-ObjectEvent')))
Assert-True 'R3-6: BeginOutputReadLine/BeginErrorReadLine are no longer used anywhere in the offsite script' `
    (-not ($OffsiteText -match [regex]::Escape('BeginOutputReadLine')) -and -not ($OffsiteText -match [regex]::Escape('BeginErrorReadLine')))
Assert-True 'R3-6: no stray Get-EventSubscriber/Unregister-Event cleanup remains (nothing left to unregister)' `
    (-not ($OffsiteText -match [regex]::Escape('Get-EventSubscriber')))

# ---------------------------------------------------------------------------
# R3-7 / R3-8: static proof that restic backup JSON-summary parsing (the
# code that actually USES Invoke-ResticCommand's captured Stdout) is
# unchanged by this patch -- it still requires message_type=summary AND a
# non-blank snapshot_id, and still fails closed on exit 0 with no
# authoritative snapshot_id. This logic is exercised for real end-to-end in
# test_offsite_b2_workflow.ps1 Section 3 (real snapshot_id extraction
# against a real local restic repository).
# ---------------------------------------------------------------------------
Show-Info ''
Show-Info '=== R3-7/R3-8: static proof -- snapshot_id summary parsing unchanged ==='

Assert-True 'R3-7: the backup JSON parser still requires message_type -eq ''summary''' `
    ($OffsiteText -match [regex]::Escape("`$obj.message_type -eq 'summary'"))
Assert-True 'R3-7: the backup JSON parser still requires a present snapshot_id property before trusting it' `
    ($OffsiteText -match [regex]::Escape("PSObject.Properties['snapshot_id']"))
Assert-True 'R3-8: exit 0 with no snapshot_id found still fails closed (refuses to trust an unproven snapshot)' `
    ($OffsiteText -match [regex]::Escape('restic backup exited 0 but no snapshot_id was found'))

Remove-Item -Path $ScratchRoot -Recurse -Force -ErrorAction SilentlyContinue

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
