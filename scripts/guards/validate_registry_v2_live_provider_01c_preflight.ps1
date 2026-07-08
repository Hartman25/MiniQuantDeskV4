# =============================================================================
# REGISTRY-V2-LIVE-PROVIDER-01C -- No-Network Preflight Guard
# =============================================================================
# Pure local file inspection of the 01A audit doc and 01B boundary decision
# doc. Validates that the boundary decision is complete and safe before any
# future, separately-authorized session attempts a live Kraken proof.
#
# This script performs NO network call, NO DB access, NO credential read,
# NO .env.local read, NO cargo/npm build or test, and NO Docker call. It
# only reads committed text files under docs/specs/ and inspects `git diff`
# output for staged/tracked-diff safety.
#
# Checks:
#   [1]  01A audit doc exists and names the selected provider (Kraken).
#   [2]  01B boundary decision doc exists.
#   [3]  01B names the selected provider (Kraken).
#   [4]  01B names the selected symbol(s) (BTC/USD and/or ETH/USD).
#   [5]  01B contains the exact required operator authorization phrase.
#   [6]  01B requires an isolated proof DB by default.
#   [7]  01B forbids paper/live DB by default.
#   [8]  01B forbids credentials.
#   [9]  01B forbids trading enablement.
#   [10] 01B forbids scheduled sync / task registration.
#   [11] 01B forbids production registry-v2 cutover.
#   [12] 01B says prerequisite #4 is not closed until a future live proof runs.
#   [13] 01B says prerequisite #5 remains open.
#   [14] No generated evidence files present in tracked git diff.
#   [15] No forbidden source/config files changed in this phase's diff.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\validate_registry_v2_live_provider_01c_preflight.ps1
#
# Exit codes: 0 = valid, 1 = contract violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$PathAuditSpec    = Join-Path $RepoRoot "docs\specs\registry_v2_live_provider_01a_current_non_equity_provider_audit.md"
$PathBoundarySpec = Join-Path $RepoRoot "docs\specs\registry_v2_live_provider_01b_first_proof_boundary_decision.md"

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

function Test-FileExists {
    param([string]$Label, [string]$Path)
    if (Test-Path $Path) {
        Show-Green "  OK -- $Label found: $Path"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label not found: $Path"
        return $false
    }
}

function Test-ContentContains {
    param([string]$Label, [string]$Content, [string]$Needle)
    if ($null -ne $Content -and $Content.IndexOf($Needle, [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        Show-Green "  OK -- $Label"
        return $true
    } else {
        $script:Violations++
        Show-Red "  FAIL -- $Label (needle not found: '$Needle')"
        return $false
    }
}

Write-Host "============================================================"
Write-Host " REGISTRY-V2-LIVE-PROVIDER-01C No-Network Preflight Guard"
Write-Host "============================================================"

Write-Host ""
Show-Info "--- [1] 01A audit doc exists and names Kraken ---"
$AuditContent = $null
if (Test-FileExists "01A audit doc" $PathAuditSpec) {
    $AuditContent = Get-Content -Raw -Path $PathAuditSpec
    Test-ContentContains "01A names Kraken as selected candidate" $AuditContent "Kraken" | Out-Null
}

Write-Host ""
Show-Info "--- [2] 01B boundary decision doc exists ---"
$BoundaryContent = $null
if (Test-FileExists "01B boundary decision doc" $PathBoundarySpec) {
    $BoundaryContent = Get-Content -Raw -Path $PathBoundarySpec
}

Write-Host ""
Show-Info "--- [3] 01B names the selected provider (Kraken) ---"
Test-ContentContains "01B names Kraken" $BoundaryContent "Kraken" | Out-Null

Write-Host ""
Show-Info "--- [4] 01B names the selected symbol(s) ---"
$Names = $false
if ($null -ne $BoundaryContent) {
    if ($BoundaryContent.IndexOf("BTC/USD", [System.StringComparison]::OrdinalIgnoreCase) -ge 0 -or
        $BoundaryContent.IndexOf("ETH/USD", [System.StringComparison]::OrdinalIgnoreCase) -ge 0) {
        $Names = $true
    }
}
if ($Names) {
    Show-Green "  OK -- 01B names BTC/USD and/or ETH/USD"
} else {
    $Violations++
    Show-Red "  FAIL -- 01B does not name BTC/USD or ETH/USD"
}

Write-Host ""
Show-Info "--- [5] 01B contains the exact required operator authorization phrase ---"
$RequiredPhrase = "I explicitly authorize ONE bounded Kraken public OHLC live-network proof for BTC/USD and ETH/USD into an isolated test/proof database only. Do not enable trading, do not use credentials, do not touch paper/live broker routing, and stop after evidence."
if ($null -ne $BoundaryContent -and $BoundaryContent.Contains($RequiredPhrase)) {
    Show-Green "  OK -- 01B contains the exact required authorization phrase"
} else {
    $Violations++
    Show-Red "  FAIL -- 01B does not contain the exact required authorization phrase"
}

Write-Host ""
Show-Info "--- [6] 01B requires an isolated proof DB by default ---"
Test-ContentContains "01B requires isolated proof/test DB" $BoundaryContent "isolated proof" | Out-Null

Write-Host ""
Show-Info "--- [7] 01B forbids paper/live DB by default ---"
Test-ContentContains "01B forbids paper/live DB" $BoundaryContent "never the paper or live database" | Out-Null

Write-Host ""
Show-Info "--- [8] 01B forbids credentials ---"
Test-ContentContains "01B forbids credentials" $BoundaryContent "do not use credentials" | Out-Null

Write-Host ""
Show-Info "--- [9] 01B forbids trading enablement ---"
Test-ContentContains "01B forbids trading enablement" $BoundaryContent "do not enable trading" | Out-Null
Test-ContentContains "01B requires kraken.enabled to stay false" $BoundaryContent "kraken.enabled must stay false" | Out-Null

Write-Host ""
Show-Info "--- [10] 01B forbids scheduled sync / task registration ---"
Test-ContentContains "01B forbids scheduled sync" $BoundaryContent "No Windows Scheduled Task was registered" | Out-Null
Test-ContentContains "01B forbids recurring sync" $BoundaryContent "No recurring" | Out-Null

Write-Host ""
Show-Info "--- [11] 01B forbids production registry-v2 cutover ---"
Test-ContentContains "01B forbids production cutover" $BoundaryContent "production registry-v2 cutover" | Out-Null

Write-Host ""
Show-Info "--- [12] 01B says prerequisite #4 is not closed until a future live proof runs ---"
Test-ContentContains "01B ties prerequisite #4 closure to a future live proof" $BoundaryContent "does not run the proof" | Out-Null

Write-Host ""
Show-Info "--- [13] 01B says prerequisite #5 remains open ---"
Test-ContentContains "01B leaves prerequisite #5 open" $BoundaryContent "Prerequisite #5" | Out-Null
Test-ContentContains "01B states prerequisite #5 untouched" $BoundaryContent "remains entirely untouched" | Out-Null

Write-Host ""
Show-Info "--- [14] No generated evidence files present in tracked git diff ---"
Push-Location $RepoRoot
try {
    $DiffNames = git diff --name-only HEAD 2>$null
    $StagedNames = git diff --cached --name-only 2>$null
    $AllChanged = @()
    if ($DiffNames) { $AllChanged += $DiffNames }
    if ($StagedNames) { $AllChanged += $StagedNames }
    $AllChanged = $AllChanged | Select-Object -Unique

    $EvidencePatterns = @(
        'exports/',
        'smoke_logs/',
        'kraken_ohlc_ingest_',
        'kraken_ohlc_sync_',
        'kraken_ohlc_dry_run_'
    )
    $EvidenceFound = $false
    foreach ($File in $AllChanged) {
        foreach ($Pattern in $EvidencePatterns) {
            if ($File -like "*$Pattern*") {
                $EvidenceFound = $true
                $script:Violations++
                Show-Red "  FAIL -- generated-evidence-looking file in diff: $File"
            }
        }
    }
    if (-not $EvidenceFound) {
        Show-Green "  OK -- no generated evidence files found in tracked diff"
    }
}
finally {
    Pop-Location
}

Write-Host ""
Show-Info "--- [15] No forbidden source/config files changed in this phase's diff ---"
Push-Location $RepoRoot
try {
    $DiffNames = git diff --name-only HEAD 2>$null
    $StagedNames = git diff --cached --name-only 2>$null
    $AllChanged = @()
    if ($DiffNames) { $AllChanged += $DiffNames }
    if ($StagedNames) { $AllChanged += $StagedNames }
    $AllChanged = $AllChanged | Select-Object -Unique

    $ForbiddenPrefixes = @(
        'core-rs/crates/mqk-runtime/',
        'core-rs/crates/mqk-execution/',
        'core-rs/crates/mqk-risk/',
        'core-rs/crates/mqk-broker-alpaca/',
        'core-rs/crates/mqk-db/migrations/',
        'core-rs/crates/mqk-portfolio/src/accounting.rs',
        'config/instruments/equities.json',
        'config/providers/providers.json',
        'config/instruments/instruments_v2.crypto_local_marks.example.json',
        '.env.local'
    )
    $ForbiddenFound = $false
    foreach ($File in $AllChanged) {
        $Normalized = $File -replace '\\', '/'
        foreach ($Prefix in $ForbiddenPrefixes) {
            if ($Normalized -eq $Prefix -or $Normalized.StartsWith($Prefix)) {
                $ForbiddenFound = $true
                $script:Violations++
                Show-Red "  FAIL -- forbidden file changed in this phase's diff: $File"
            }
        }
    }
    if (-not $ForbiddenFound) {
        Show-Green "  OK -- no forbidden source/config file changed in this phase's diff"
    }
}
finally {
    Pop-Location
}

# =============================================================================
# Summary
# =============================================================================
Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL CHECKS PASSED -- REGISTRY-V2-LIVE-PROVIDER-01C preflight is safe."
    exit 0
} else {
    Show-Red " VALIDATION FAILED -- $Violations violation(s) found."
    exit 1
}
