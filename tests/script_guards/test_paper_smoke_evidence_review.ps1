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
Assert-True ($Content -match 'NATURAL-TRADE-LIFECYCLE-CLOSED') 'Contains classification NATURAL-TRADE-LIFECYCLE-CLOSED'
Assert-True ($Content -match 'READINESS-CLOSED-NO-TRADE')      'Contains classification READINESS-CLOSED-NO-TRADE'
Assert-True ($Content -match 'PARTIAL')                        'Contains classification PARTIAL'
Assert-True ($Content -match "'OPEN'")                         "Contains classification OPEN"
Assert-True ($Content -match 'FALSE-CLOSED')                   'Contains classification FALSE-CLOSED'

# 10. Contains secret scan logic
Assert-True ($Content -match 'SecretWarnings|SecretScan|secret.*scan') 'Contains secret scan logic'

# 11. Does not call Invoke-WebRequest or Invoke-RestMethod (no live HTTP calls)
Assert-False ($Content -match 'Invoke-WebRequest|Invoke-RestMethod') 'Does not make live HTTP calls'

# 12. WriteSummary writes review_summary.md
Assert-True ($Content -match 'review_summary\.md') 'WriteSummary writes review_summary.md'

# 13. Schema version present (review-v2)
Assert-True ($Content -match 'review-v2') 'JSON output includes schema_version review-v2'

# 14. Secret scan uses Test-SecretLeakLine helper (targeted patterns, not bare words)
Assert-True ($Content -match 'Test-SecretLeakLine') 'Contains Test-SecretLeakLine helper function'

# 15. Standalone word DISCORD is NOT a bare scan pattern (avoids false-positives on template headers)
#     The old offending line was: @('ALPACA_API_SECRET', 'MQK_OPERATOR_TOKEN', 'DISCORD', 'Bearer ')
#     Verify the bare-word match against 'DISCORD' is gone.
Assert-False ($Content -match "'DISCORD'") "Does not use bare 'DISCORD' as a secret pattern"

# 16. Script contains DISCORD_WEBHOOK_URL as a targeted pattern (real secret detection preserved)
Assert-True ($Content -match 'DISCORD_WEBHOOK_URL') 'Contains DISCORD_WEBHOOK_URL targeted pattern'

# 17. Script contains DISCORD_BOT_TOKEN as a targeted pattern
Assert-True ($Content -match 'DISCORD_BOT_TOKEN') 'Contains DISCORD_BOT_TOKEN targeted pattern'

# 18. Secret scan requires non-empty/non-redacted value before flagging (value filtering present)
Assert-True ($Content -match 'RedactedPlaceholders|notin.*RedactedPlaceholders|val.*-ne.*""') 'Secret scan filters out redacted/empty values'

# 19. Bearer detection requires a minimum token length (not just the word "Bearer")
Assert-True ($Content -match 'token\.Length -gt') 'Bearer detection checks token.Length before flagging'

# 20-22. Runtime: load Test-SecretLeakLine from production source and exercise it inline.
#   Extract the helper block (RedactedPlaceholders + Test-SecretLeakLine + Invoke-SecretScan)
#   then Invoke-Expression it into this scope.  No subprocess needed.
$_rtSrc   = Get-Content $Target -Raw
$_rtLines = $_rtSrc -split '\r?\n'
$_rtStart = ($_rtLines | Select-String -Pattern '^\$RedactedPlaceholders\s*=' |
    Select-Object -First 1).LineNumber - 1
$_rtEnd   = ($_rtLines | Select-String -Pattern '^\}' |
    Where-Object { $_.LineNumber -gt ($_rtStart + 30) } |
    Select-Object -First 1).LineNumber - 1
$_rtBlock = ($_rtLines[$_rtStart..$_rtEnd]) -join "`n"

try {
    Invoke-Expression $_rtBlock

    $rt1 = Test-SecretLeakLine -Line 'Discord observation: smoke passed' -FilePath 'note.txt' -LineNo 1
    $rt2 = Test-SecretLeakLine -Line 'DISCORD_WEBHOOK_URL=https://discord.com/api/webhooks/123/realtoken' -FilePath 'note.txt' -LineNo 2
    $rt3 = Test-SecretLeakLine -Line 'Authorization: Bearer eyJhbGciOiJSUzI1NiJ9.longtoken123456' -FilePath 'note.txt' -LineNo 3

    Assert-True  ($null -eq $rt1)                                       'Test-SecretLeakLine does not flag standalone Discord text'
    Assert-True  ($null -ne $rt2 -and $rt2.PatternName -eq 'DISCORD_WEBHOOK_URL') 'Test-SecretLeakLine detects DISCORD_WEBHOOK_URL=<url>'
    Assert-True  ($null -ne $rt3 -and $rt3.PatternName -eq 'Authorization-Bearer') 'Test-SecretLeakLine detects Authorization: Bearer <token>'
} catch {
    Write-Host "  FAIL: Runtime Test-SecretLeakLine test threw: $_" -ForegroundColor Red
    $script:Failures += 3
}

Write-Host ''
if ($Failures -eq 0) {
    Write-Host '  ALL ASSERTIONS PASSED (test_paper_smoke_evidence_review)' -ForegroundColor Green
    exit 0
} else {
    Write-Host "  $Failures ASSERTION(S) FAILED (test_paper_smoke_evidence_review)" -ForegroundColor Red
    exit 1
}
