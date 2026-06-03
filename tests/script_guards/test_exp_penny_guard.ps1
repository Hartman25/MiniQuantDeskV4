# =============================================================================
# EXP-PENNY-01A static guard
#
# Proves:
#   G1. exp_penny source files do not reference oms_outbox
#   G2. exp_penny source files do not reference oms_inbox
#   G3. exp_penny source files do not reference BrokerGateway
#   G4. exp_penny source files do not reference broker_adapter
#   G5. exp_penny source files do not reference alpaca
#   G6. exp_penny source files do not reference Start-PaperTradingSmoke
#   G7. scanner.py sets paper_order_id via build_candidate_record (null enforced upstream)
#   G8. scanner.py does not assign non-null paper_order_id or live_order_id
#   G9. run_scanner.py requires --dry-run (not optional)
#   G10. universe_loader.py exists and does not reference forbidden strings
#
# Exit codes: 0 = all guards pass, 1 = one or more guards failed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$RepoRoot = Split-Path -Parent (Split-Path -Parent $PSScriptRoot)
$ExpPennyDir = Join-Path $RepoRoot 'research-py\experiments\exp_penny'

$Failures = [System.Collections.Generic.List[string]]::new()

function Fail([string]$msg) {
    $Failures.Add("  FAIL: $msg")
    Write-Host "  FAIL: $msg" -ForegroundColor Red
}

function Pass([string]$msg) {
    Write-Host "  PASS: $msg" -ForegroundColor Green
}

Write-Host ''
Write-Host '============================================================'
Write-Host 'EXP-PENNY-01A static guard'
Write-Host '============================================================'

# Guard dir exists
if (-not (Test-Path $ExpPennyDir)) {
    Fail "exp_penny directory not found: $ExpPennyDir"
    Write-Host ''
    Write-Host "EXP-PENNY guard: 1 FAILURE(S)" -ForegroundColor Red
    exit 1
}

$SourceFiles = Get-ChildItem -Path $ExpPennyDir -Recurse -Include '*.py' |
    Where-Object { $_.Name -notmatch '^test_' }

# G1-G6: forbidden patterns in source files (excludes test files)
$ForbiddenPatterns = [ordered]@{
    'G1' = 'oms_outbox'
    'G2' = 'oms_inbox'
    'G3' = 'BrokerGateway'
    'G4' = 'broker_adapter'
    'G5' = 'alpaca'
    'G6' = 'Start-PaperTradingSmoke'
}

foreach ($key in $ForbiddenPatterns.Keys) {
    $pattern = $ForbiddenPatterns[$key]
    Write-Host ''
    Write-Host "--- $key`: exp_penny source must not reference '$pattern' ---"
    $hits = $SourceFiles | Where-Object {
        (Get-Content $_.FullName -Raw) -match [regex]::Escape($pattern)
    }
    if ($hits) {
        foreach ($hit in $hits) {
            Fail "$($hit.Name) references '$pattern'"
        }
    } else {
        Pass "No exp_penny source file references '$pattern'"
    }
}

# G7: scanner.py must use build_candidate_record (null order IDs enforced upstream)
Write-Host ''
Write-Host '--- G7: scanner.py must call build_candidate_record ---'
$scannerPath = Join-Path $ExpPennyDir 'scanner.py'
if (Test-Path $scannerPath) {
    $scannerContent = Get-Content $scannerPath -Raw
    if ($scannerContent -match 'build_candidate_record') {
        Pass "scanner.py calls build_candidate_record"
    } else {
        Fail "scanner.py does not call build_candidate_record"
    }
} else {
    Fail "scanner.py not found at $scannerPath"
}

# G8: scanner.py must not set paper_order_id or live_order_id to a non-null value
Write-Host ''
Write-Host '--- G8: scanner.py must not assign non-null paper_order_id or live_order_id ---'
if (Test-Path $scannerPath) {
    $scannerContent = Get-Content $scannerPath -Raw
    # Disallow any assignment like paper_order_id = <something other than None>
    if ($scannerContent -match '"paper_order_id"\s*:\s*(?!None)[^,\n]') {
        Fail "scanner.py assigns non-null paper_order_id"
    } else {
        Pass "scanner.py does not assign non-null paper_order_id"
    }
    if ($scannerContent -match '"live_order_id"\s*:\s*(?!None)[^,\n]') {
        Fail "scanner.py assigns non-null live_order_id"
    } else {
        Pass "scanner.py does not assign non-null live_order_id"
    }
}

# G9: run_scanner.py must declare --dry-run as required
Write-Host ''
Write-Host '--- G9: run_scanner.py must require --dry-run ---'
$runnerPath = Join-Path $ExpPennyDir 'run_scanner.py'
if (Test-Path $runnerPath) {
    $runnerContent = Get-Content $runnerPath -Raw
    if ($runnerContent -match 'required\s*=\s*True' -and $runnerContent -match 'dry.run') {
        Pass "run_scanner.py declares --dry-run as required"
    } else {
        Fail "run_scanner.py does not declare --dry-run as required"
    }
} else {
    Fail "run_scanner.py not found at $runnerPath"
}

# G10: universe_loader.py must exist and must not reference forbidden patterns
Write-Host ''
Write-Host '--- G10: universe_loader.py must exist and be clean ---'
$loaderPath = Join-Path $ExpPennyDir 'universe_loader.py'
if (Test-Path $loaderPath) {
    Pass "universe_loader.py exists"
    $loaderContent = Get-Content $loaderPath -Raw
    foreach ($key in $ForbiddenPatterns.Keys) {
        $pattern = $ForbiddenPatterns[$key]
        if ($loaderContent -match [regex]::Escape($pattern)) {
            Fail "universe_loader.py references '$pattern'"
        } else {
            Pass "universe_loader.py does not reference '$pattern'"
        }
    }
} else {
    Fail "universe_loader.py not found at $loaderPath"
}

# G11: screener_profiles.py must exist and must not reference forbidden patterns
Write-Host ''
Write-Host '--- G11: screener_profiles.py must exist and be clean ---'
$profilerPath = Join-Path $ExpPennyDir 'screener_profiles.py'
if (Test-Path $profilerPath) {
    Pass "screener_profiles.py exists"
    $profilerContent = Get-Content $profilerPath -Raw
    foreach ($key in $ForbiddenPatterns.Keys) {
        $pattern = $ForbiddenPatterns[$key]
        if ($profilerContent -match [regex]::Escape($pattern)) {
            Fail "screener_profiles.py references '$pattern'"
        } else {
            Pass "screener_profiles.py does not reference '$pattern'"
        }
    }
} else {
    Fail "screener_profiles.py not found at $profilerPath"
}

# G12: no network import modules in exp_penny source files
Write-Host ''
Write-Host '--- G12: exp_penny source must not import network modules ---'
$NetworkModules = @('import requests', 'import urllib', 'import http.client', 'import aiohttp')
foreach ($netImport in $NetworkModules) {
    $hits = $SourceFiles | Where-Object {
        (Get-Content $_.FullName -Raw) -match [regex]::Escape($netImport)
    }
    if ($hits) {
        foreach ($hit in $hits) {
            Fail "$($hit.Name) contains forbidden network import '$netImport'"
        }
    } else {
        Pass "No exp_penny source file contains '$netImport'"
    }
}

# G13: indicator_enrichment.py must exist and must not reference forbidden patterns
Write-Host ''
Write-Host '--- G13: indicator_enrichment.py must exist and be clean ---'
$enrichmentPath = Join-Path $ExpPennyDir 'indicator_enrichment.py'
if (Test-Path $enrichmentPath) {
    Pass "indicator_enrichment.py exists"
    $enrichmentContent = Get-Content $enrichmentPath -Raw
    foreach ($key in $ForbiddenPatterns.Keys) {
        $pattern = $ForbiddenPatterns[$key]
        if ($enrichmentContent -match [regex]::Escape($pattern)) {
            Fail "indicator_enrichment.py references '$pattern'"
        } else {
            Pass "indicator_enrichment.py does not reference '$pattern'"
        }
    }
    foreach ($netImport in $NetworkModules) {
        if ($enrichmentContent -match [regex]::Escape($netImport)) {
            Fail "indicator_enrichment.py contains forbidden network import '$netImport'"
        } else {
            Pass "indicator_enrichment.py does not contain '$netImport'"
        }
    }
} else {
    Fail "indicator_enrichment.py not found at $enrichmentPath"
}

# G14: .env.local.example must have required safety values
Write-Host ''
Write-Host '--- G14: .env.local.example must lock EXP_PENNY safety vars ---'
$EnvExamplePath = Join-Path $RepoRoot '.env.local.example'
if (Test-Path $EnvExamplePath) {
    $envContent = Get-Content $EnvExamplePath -Raw
    $SafetyChecks = [ordered]@{
        'MQK_EXP_PENNY_LIVE_ALLOWED=false'   = 'MQK_EXP_PENNY_LIVE_ALLOWED=false'
        'MQK_EXP_PENNY_ORDER_MODE=scanner_only' = 'MQK_EXP_PENNY_ORDER_MODE=scanner_only'
        'MQK_EXP_PENNY_ALLOW_SHORTS=false'   = 'MQK_EXP_PENNY_ALLOW_SHORTS=false'
    }
    foreach ($check in $SafetyChecks.Keys) {
        $expected = $SafetyChecks[$check]
        if ($envContent -match [regex]::Escape($expected)) {
            Pass ".env.local.example contains '$expected'"
        } else {
            Fail ".env.local.example must contain '$expected'"
        }
    }
} else {
    Fail ".env.local.example not found at $EnvExamplePath"
}

# G15: penny_config.py must exist and be clean
Write-Host ''
Write-Host '--- G15: penny_config.py must exist and be clean ---'
$pennyConfigPath = Join-Path $ExpPennyDir 'penny_config.py'
if (Test-Path $pennyConfigPath) {
    Pass "penny_config.py exists"
    $configContent = Get-Content $pennyConfigPath -Raw
    foreach ($key in $ForbiddenPatterns.Keys) {
        $pattern = $ForbiddenPatterns[$key]
        if ($configContent -match [regex]::Escape($pattern)) {
            Fail "penny_config.py references '$pattern'"
        } else {
            Pass "penny_config.py does not reference '$pattern'"
        }
    }
    foreach ($netImport in $NetworkModules) {
        if ($configContent -match [regex]::Escape($netImport)) {
            Fail "penny_config.py contains forbidden network import '$netImport'"
        } else {
            Pass "penny_config.py does not contain '$netImport'"
        }
    }
} else {
    Fail "penny_config.py not found at $pennyConfigPath"
}

# Summary
Write-Host ''
Write-Host '============================================================'
$fileCount = $SourceFiles.Count
$failCount = $Failures.Count
if ($failCount -eq 0) {
    Write-Host "EXP-PENNY guard: ALL PASS ($fileCount source files checked)" -ForegroundColor Green
    exit 0
} else {
    Write-Host "EXP-PENNY guard: $failCount FAILURE(S)" -ForegroundColor Red
    foreach ($f in $Failures) { Write-Host $f }
    exit 1
}
