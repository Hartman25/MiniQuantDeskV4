# =============================================================================
# Script guard: test_paper_smoke_evidence_review.ps1
# PAPER-SMOKE-EVIDENCE-REVIEW-02
#
# Static assertions for Review-PaperSmokeEvidence.ps1
# No daemon, no DB, no live calls, no .env.local, no secrets printed.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Continue'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = Split-Path -Parent (Split-Path -Parent $ScriptDir)
$Target    = Join-Path $RepoRoot 'scripts\windows\Review-PaperSmokeEvidence.ps1'

$Failures  = 0

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
Write-Host '--- test_paper_smoke_evidence_review.ps1 ---'

# 1. Review script exists
Assert-True (Test-Path $Target) "Review script exists at $Target"

if (-not (Test-Path $Target)) {
    Write-Host '  FATAL: Script missing -- skipping remaining assertions' -ForegroundColor Red
    exit 1
}

$Content = Get-Content $Target -Raw

# 2. Supports -EvidencePath parameter
Assert-True ($Content -match '\[string\]\$EvidencePath') 'Supports -EvidencePath parameter'

# 3. Supports -Latest switch
Assert-True ($Content -match '\[switch\]\$Latest') 'Supports -Latest switch'

# 4. Supports -WriteSummary switch
Assert-True ($Content -match '\[switch\]\$WriteSummary') 'Supports -WriteSummary switch'

# 5. Does not call broker order endpoints (/v2/orders)
Assert-False ($Content -match '/v2/orders') 'Does not reference /v2/orders broker endpoint'

# 6. Does not invoke Start-PaperTradingSmoke
Assert-False ($Content -match 'Start-PaperTradingSmoke') 'Does not call Start-PaperTradingSmoke'

# 7. Does not mutate DB (no SQL mutation patterns — uppercase keywords in code context)
Assert-False ($Content -match '\bINSERT INTO\b|\bUPDATE .* SET\b|\bDELETE FROM\b|\bDROP TABLE\b') 'Does not contain DB mutation SQL (INSERT INTO/UPDATE SET/DELETE FROM/DROP TABLE)'
Assert-False ($Content -match '(?i)psql.*-c.*(INSERT|DELETE FROM|DROP TABLE)') 'Does not invoke psql with mutating SQL'

# 8. Does not print secret values
Assert-False ($Content -match 'Write-Host.*ALPACA_API_SECRET') 'Does not print ALPACA_API_SECRET value'
Assert-False ($Content -match 'Write-Host.*MQK_OPERATOR_TOKEN') 'Does not print MQK_OPERATOR_TOKEN value'

# 9. Contains all five classification strings
Assert-True ($Content -match 'TRADE-LIFECYCLE-CLOSED')    'Contains classification TRADE-LIFECYCLE-CLOSED'
Assert-True ($Content -match 'READINESS-CLOSED-NO-TRADE') 'Contains classification READINESS-CLOSED-NO-TRADE'
Assert-True ($Content -match 'PARTIAL')                   'Contains classification PARTIAL'
Assert-True ($Content -match "'OPEN'")                    "Contains classification OPEN"
Assert-True ($Content -match 'FALSE-CLOSED')              'Contains classification FALSE-CLOSED'

# 10. Contains secret scan logic
Assert-True ($Content -match 'SecretWarnings|SecretScan|secret.*scan') 'Contains secret scan logic'

# 11. Does not call Invoke-WebRequest or Invoke-RestMethod (no live HTTP calls)
Assert-False ($Content -match 'Invoke-WebRequest|Invoke-RestMethod') 'Does not make live HTTP calls'

# 12. WriteSummary writes review_summary.md
Assert-True ($Content -match 'review_summary\.md') 'WriteSummary writes review_summary.md'

# 13. Schema version present (review-v1)
Assert-True ($Content -match 'review-v1') 'JSON output includes schema_version review-v1'

Write-Host ''
if ($Failures -eq 0) {
    Write-Host '  ALL ASSERTIONS PASSED (test_paper_smoke_evidence_review)' -ForegroundColor Green
    exit 0
} else {
    Write-Host "  $Failures ASSERTION(S) FAILED (test_paper_smoke_evidence_review)" -ForegroundColor Red
    exit 1
}
