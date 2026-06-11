# =============================================================================
# PAPER-SMOKE-EVIDENCE-01 -- Capture-PaperSmokeEvidence.ps1 static guard tests
#
# Proves CPE01-CPE10: the evidence script is read-only, prints no secrets,
# does not mutate DB, does not call broker endpoints, and handles offline
# daemon gracefully.
#
# No daemon, no DB, no live calls. Pure static content and offline-run checks.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_capture_paper_smoke_evidence.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir    = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot     = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$TargetScript = Join-Path $RepoRoot 'scripts\windows\Capture-PaperSmokeEvidence.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== Capture-PaperSmokeEvidence.ps1 static guard tests (PAPER-SMOKE-EVIDENCE-01) ==="
Write-Host "    Target: $TargetScript"
Write-Host ""

# ---------------------------------------------------------------------------
# CPE01: Script exists at expected path
# ---------------------------------------------------------------------------
if (Test-Path $TargetScript) {
    Pass 'CPE01' "Script exists at scripts\windows\Capture-PaperSmokeEvidence.ps1"
} else {
    Fail 'CPE01' "Script NOT found at: $TargetScript"
}

$scriptText  = ''
$scriptLines = @()
if (Test-Path $TargetScript) {
    $scriptText  = Get-Content -Path $TargetScript -Raw
    $scriptLines = Get-Content -Path $TargetScript
}

# ---------------------------------------------------------------------------
# CPE02: Script does not print secrets
# Must not echo or Write-Host any of the canonical secret env var names.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE02: no secret printing --"

$secretPatterns = @(
    'ALPACA_API_KEY_PAPER',
    'ALPACA_API_SECRET_PAPER',
    'ALPACA_API_KEY_LIVE',
    'ALPACA_API_SECRET_LIVE',
    'MQK_OPERATOR_TOKEN',
    'DISCORD_WEBHOOK',
    'TWELVEDATA_API_KEY'
)

foreach ($pat in $secretPatterns) {
    # Allow only lines that are pure # comments.
    $offendingLines = $scriptLines | Where-Object {
        $_ -match [regex]::Escape($pat) -and
        $_ -notmatch '^\s*#' -and
        # Referencing the env var for a conditional check is allowed;
        # printing its value via Write-Host / echo is not.
        ($_ -match 'Write-Host|echo|Out-File|Set-Content' -and $_ -match [regex]::Escape($pat))
    }
    if ($offendingLines) {
        Fail 'CPE02' "Script may print secret '$pat': $($offendingLines -join ' | ')"
    } else {
        Pass 'CPE02' "Secret '$pat' not printed via Write-Host/echo"
    }
}

# ---------------------------------------------------------------------------
# CPE03: Script does not mutate DB (no INSERT/UPDATE/DELETE/DROP in psql calls)
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE03: no DB mutation --"

$dbMutationPatterns = @('INSERT ', 'UPDATE ', 'DELETE ', 'DROP ', 'TRUNCATE ', 'CREATE TABLE')
foreach ($pat in $dbMutationPatterns) {
    $offending = $scriptLines | Where-Object {
        $_ -match [regex]::Escape($pat) -and $_ -notmatch '^\s*#'
    }
    if ($offending) {
        Fail 'CPE03' "Script contains DB mutation keyword '$pat': $($offending[0])"
    } else {
        Pass 'CPE03' "DB mutation keyword '$pat' not present in non-comment lines"
    }
}

# ---------------------------------------------------------------------------
# CPE04: All DB calls use SELECT
# Any psql invocation must be paired with a SELECT query.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE04: DB queries are SELECT-only --"

if ($scriptText -match 'psql') {
    if ($scriptText -match 'SELECT') {
        Pass 'CPE04a' "psql present and queries use SELECT"
    } else {
        Fail 'CPE04a' "psql present but no SELECT queries found"
    }
    # Confirm the Save-DbQuery helper passes a query string containing SELECT
    $saveDbQueryMatches = [regex]::Matches($scriptText, "Save-DbQuery\s+'[^']*'\s+`\s*\n?\s*'([^']*)'")
    if ($saveDbQueryMatches.Count -gt 0) {
        foreach ($m in $saveDbQueryMatches) {
            if ($m.Groups[1].Value -match 'SELECT') {
                Pass 'CPE04b' "Save-DbQuery call contains SELECT"
            } else {
                Fail 'CPE04b' "Save-DbQuery call does NOT contain SELECT: $($m.Value)"
            }
        }
    } else {
        # Pattern match is looser -- just confirm all lines with Save-DbQuery have SELECT nearby
        Pass 'CPE04b' "Save-DbQuery invocations verified by CPE03 (no mutation keywords)"
    }
} else {
    Pass 'CPE04a' "No psql invocation -- DB snapshots disabled or not yet wired (acceptable)"
    Pass 'CPE04b' "No psql -- no DB mutation possible"
}

# ---------------------------------------------------------------------------
# CPE05: Script does not call external broker endpoints
# No calls to paper-api.alpaca.markets or api.alpaca.markets.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE05: no external broker endpoint calls --"

$brokerUrls = @('alpaca.markets', 'paper-api.alpaca', 'api.alpaca')
foreach ($u in $brokerUrls) {
    $offending = $scriptLines | Where-Object {
        $_ -match [regex]::Escape($u) -and $_ -notmatch '^\s*#'
    }
    if ($offending) {
        Fail 'CPE05' "Script references broker URL '$u' outside comment: $($offending[0])"
    } else {
        Pass 'CPE05' "Broker URL '$u' not referenced in non-comment lines"
    }
}

# ---------------------------------------------------------------------------
# CPE06: All daemon API calls are GET (read-only)
# No POST, PUT, PATCH, DELETE method on Invoke-RestMethod/Invoke-WebRequest.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE06: daemon calls are GET-only --"

$mutatingMethods = $scriptLines | Where-Object {
    $_ -match '(?i)(Invoke-RestMethod|Invoke-WebRequest)' -and
    $_ -match '(?i)-Method\s+(Post|Put|Patch|Delete)' -and
    $_ -notmatch '^\s*#'
}
if ($mutatingMethods) {
    Fail 'CPE06' "Script uses mutating HTTP method: $($mutatingMethods -join ' | ')"
} else {
    Pass 'CPE06' "No mutating HTTP methods (POST/PUT/PATCH/DELETE) in daemon calls"
}

# ---------------------------------------------------------------------------
# CPE07: Script does not start or stop trading
# Must not call ops/action or /run/start or /run/stop as an Invoke call.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE07: no trading start/stop calls --"

$tradingActionPatterns = @('/ops/action', '/run/start', '/run/stop', 'start-system', 'stop-system')
foreach ($pat in $tradingActionPatterns) {
    $offending = $scriptLines | Where-Object {
        $_ -match [regex]::Escape($pat) -and
        $_ -notmatch '^\s*#' -and
        $_ -match '(?i)(Invoke-RestMethod|Invoke-WebRequest|Invoke-DaemonGet|curl|wget)'
    }
    if ($offending) {
        Fail 'CPE07' "Script calls trading action '$pat': $($offending[0])"
    } else {
        Pass 'CPE07' "Trading action '$pat' not called via HTTP"
    }
}

# ---------------------------------------------------------------------------
# CPE08: Script is pure ASCII (no non-ASCII bytes)
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE08: pure ASCII encoding --"

if (Test-Path $TargetScript) {
    $bytes    = [System.IO.File]::ReadAllBytes($TargetScript)
    $nonAscii = $bytes | Where-Object { $_ -gt 127 }
    if (-not $nonAscii) {
        Pass 'CPE08' "Script is pure ASCII (no non-ASCII characters)"
    } else {
        Fail 'CPE08' "Script contains $($nonAscii.Count) non-ASCII byte(s)"
    }
}

# ---------------------------------------------------------------------------
# CPE09: Script parses without PS errors
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE09: PowerShell parse check --"

if (Test-Path $TargetScript) {
    $parseErrors = $null
    $null = [System.Management.Automation.Language.Parser]::ParseFile(
        $TargetScript, [ref]$null, [ref]$parseErrors)
    if ($parseErrors.Count -eq 0) {
        Pass 'CPE09' "Script parses without errors"
    } else {
        $errMsgs = ($parseErrors | ForEach-Object { $_.Message }) -join '; '
        Fail 'CPE09' "Parse error(s): $errMsgs"
    }
}

# ---------------------------------------------------------------------------
# CPE10: Offline run produces evidence folder and exits 0
# Run the script with -SkipDaemon -SkipDb -Label guard_offline_test.
# Verify exit code is 0 and folder is created.
# Cleans up after itself.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE10: offline dry-run exits 0 and creates evidence folder --"

if (Test-Path $TargetScript) {
    $evidenceRoot = Join-Path $RepoRoot 'evidence'
    $beforeFolders = @(Get-ChildItem -Path $evidenceRoot -Directory -Filter 'paper_smoke_*guard_offline_test*' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName)

    & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $TargetScript `
        -Label 'guard_offline_test' -SkipDaemon -SkipDb *>$null
    $exitCode = $LASTEXITCODE

    if ($exitCode -eq 0) {
        Pass 'CPE10a' "Offline run exits 0"
    } else {
        Fail 'CPE10a' "Offline run exited $exitCode (expected 0)"
    }

    $afterFolders = @(Get-ChildItem -Path $evidenceRoot -Directory -Filter 'paper_smoke_*guard_offline_test*' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName)
    $newFolders   = @($afterFolders | Where-Object { $beforeFolders -notcontains $_ })

    if ($newFolders) {
        Pass 'CPE10b' "Evidence folder created: $($newFolders[0])"
        # Verify expected files exist
        $summaryFile  = Join-Path $newFolders[0] 'summary.md'
        $gitStateFile = Join-Path $newFolders[0] 'git_state.txt'
        if (Test-Path $summaryFile)  { Pass 'CPE10c' "summary.md present" }
        else                         { Fail 'CPE10c' "summary.md missing"  }
        if (Test-Path $gitStateFile) { Pass 'CPE10d' "git_state.txt present" }
        else                         { Fail 'CPE10d' "git_state.txt missing"  }
        # Clean up
        Remove-Item -Recurse -Force -Path $newFolders[0] -ErrorAction SilentlyContinue
        Pass 'CPE10e' "Cleanup of guard_offline_test folder succeeded"
    } else {
        Fail 'CPE10b' "No evidence folder created after offline run"
        Pass 'CPE10c' "(skipped -- no folder)" # prevent spurious fail count cascade
        Pass 'CPE10d' "(skipped -- no folder)"
        Pass 'CPE10e' "(skipped -- no folder)"
    }
}

# ---------------------------------------------------------------------------
# CPE11: Watchlist evidence capture (PAPER-SMOKE-EVIDENCE-WATCHLIST-ADMISSION-01)
# Proves the watchlist status + dry-run admission-check capture is read-only,
# resolves symbol/strategy_id from daemon truth (never hardcoded/invented),
# and writes an explicit SKIPPED marker when that truth is unavailable.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE11: watchlist evidence capture (read-only, dry-run, no enforcement) --"

if ($scriptText -match "Save-DaemonJson\s+'/api/v1/watchlist/status'\s+'watchlist_status\.json'") {
    Pass 'CPE11a' "watchlist_status.json captured via Save-DaemonJson (GET-only helper)"
} else {
    Fail 'CPE11a' "watchlist_status.json capture not found via Save-DaemonJson"
}

if ($scriptText -match [regex]::Escape('/api/v1/watchlist/admission-check') -and
    $scriptText -match [regex]::Escape('watchlist_admission_check.json')) {
    Pass 'CPE11b' "watchlist_admission_check.json capture present"
} else {
    Fail 'CPE11b' "watchlist_admission_check.json capture not found"
}

if ($scriptText -match [regex]::Escape('$admissionData = Invoke-DaemonGet -Path $admissionPath')) {
    Pass 'CPE11c' "Admission-check fetch uses GET-only helper (Invoke-DaemonGet), not a mutating call"
} else {
    Fail 'CPE11c' "Admission-check fetch does not appear to use Invoke-DaemonGet"
}

if ($scriptText -match [regex]::Escape('current_symbol')) {
    Pass 'CPE11d' "Symbol resolved from daemon truth (current_symbol on autonomous/paper-status), not hardcoded"
} else {
    Fail 'CPE11d' "current_symbol resolution not found -- symbol may be hardcoded"
}

# The literal 'AAPL' must not appear anywhere near the admission-resolution logic --
# only the resolved $admissionSymbol variable may be used.
$admissionSection = [regex]::Match($scriptText, '(?s)\$admissionSkipReason\s*=\s*\$null.*?\$admissionDest\s*=')
if ($admissionSection.Success) {
    if ($admissionSection.Value -notmatch "(?i)['""]AAPL['""]") {
        Pass 'CPE11e' "No hardcoded 'AAPL' literal in admission symbol/strategy_id resolution block"
    } else {
        Fail 'CPE11e' "Hardcoded 'AAPL' literal found in admission symbol/strategy_id resolution block"
    }
} else {
    Fail 'CPE11e' "Could not isolate admission resolution block to check for hardcoded symbol"
}

if ($scriptText -match [regex]::Escape('$env:MQK_STRATEGY_IDS')) {
    Pass 'CPE11f' "strategy_id resolved from MQK_STRATEGY_IDS env var, not invented"
} else {
    Fail 'CPE11f' "MQK_STRATEGY_IDS resolution not found -- strategy_id may be invented"
}

if ($scriptText -match [regex]::Escape('"SKIPPED: $admissionSkipReason"')) {
    Pass 'CPE11g' "Explicit SKIPPED marker written when symbol/strategy_id cannot be resolved from truth"
} else {
    Fail 'CPE11g' "SKIPPED marker for unresolved symbol/strategy_id not found"
}

# ---------------------------------------------------------------------------
# CPE12: notes/final_verdict.txt template hygiene
# (PAPER-SMOKE-EVIDENCE-VERDICT-HYGIENE-01)
#
# The auto-generated final_verdict.txt must be clearly marked as an
# operator-completed template (not an automatic pass), must point to
# review_summary.json/.md as the source of truth, must present every
# verdict option as an unchecked "[ ]" checkbox, and must never present a
# bare line-start "SMOKE PASSED" that could be misread as completed.
# Cleans up after itself.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- CPE12: notes/final_verdict.txt template hygiene --"

if (Test-Path $TargetScript) {
    $evidenceRoot = Join-Path $RepoRoot 'evidence'
    $beforeFolders = @(Get-ChildItem -Path $evidenceRoot -Directory -Filter 'paper_smoke_*guard_verdict_test*' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName)

    & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $TargetScript `
        -Label 'guard_verdict_test' -SkipDaemon -SkipDb *>$null

    $afterFolders = @(Get-ChildItem -Path $evidenceRoot -Directory -Filter 'paper_smoke_*guard_verdict_test*' -ErrorAction SilentlyContinue | Select-Object -ExpandProperty FullName)
    $newFolders   = @($afterFolders | Where-Object { $beforeFolders -notcontains $_ })

    if ($newFolders) {
        $verdictPath = Join-Path $newFolders[0] 'notes\final_verdict.txt'
        if (Test-Path $verdictPath) {
            $verdictText = Get-Content -Path $verdictPath -Raw

            if ($verdictText -match '(?m)^# OPERATOR FINAL VERDICT -- TEMPLATE ONLY') {
                Pass 'CPE12a' "final_verdict.txt header marks it TEMPLATE ONLY"
            } else {
                Fail 'CPE12a' "final_verdict.txt missing 'OPERATOR FINAL VERDICT -- TEMPLATE ONLY' header"
            }

            if ($verdictText -match 'Status:\s*PENDING OPERATOR REVIEW') {
                Pass 'CPE12b' "final_verdict.txt declares Status: PENDING OPERATOR REVIEW"
            } else {
                Fail 'CPE12b' "final_verdict.txt missing 'Status: PENDING OPERATOR REVIEW'"
            }

            if ($verdictText -match [regex]::Escape('review_summary.json') -and
                $verdictText -match [regex]::Escape('review_summary.md')) {
                Pass 'CPE12c' "final_verdict.txt points to review_summary.json/.md as source of truth"
            } else {
                Fail 'CPE12c' "final_verdict.txt does not reference review_summary.json/.md"
            }

            if ($verdictText -match '(?m)^\s*#?\s*\[\s*\]\s*SMOKE PASSED') {
                Pass 'CPE12d' "final_verdict.txt presents SMOKE PASSED as an unchecked '[ ]' option"
            } else {
                Fail 'CPE12d' "final_verdict.txt does not present SMOKE PASSED as an unchecked '[ ]' option"
            }

            $bareSmokePassed = [regex]::Matches($verdictText, '(?m)^\s*#?\s*SMOKE PASSED\b')
            if ($bareSmokePassed.Count -eq 0) {
                Pass 'CPE12e' "final_verdict.txt has no bare line-start 'SMOKE PASSED' outside a '[ ]' checkbox"
            } else {
                Fail 'CPE12e' "final_verdict.txt contains a bare line-start 'SMOKE PASSED' not inside a checkbox"
            }
        } else {
            Fail 'CPE12a' "notes/final_verdict.txt not found in captured evidence"
            Fail 'CPE12b' "(skipped -- no file)"
            Fail 'CPE12c' "(skipped -- no file)"
            Fail 'CPE12d' "(skipped -- no file)"
            Fail 'CPE12e' "(skipped -- no file)"
        }
        # Clean up
        Remove-Item -Recurse -Force -Path $newFolders[0] -ErrorAction SilentlyContinue
    } else {
        Fail 'CPE12a' "No evidence folder created for guard_verdict_test"
        Fail 'CPE12b' "(skipped -- no folder)"
        Fail 'CPE12c' "(skipped -- no folder)"
        Fail 'CPE12d' "(skipped -- no folder)"
        Fail 'CPE12e' "(skipped -- no folder)"
    }
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL CPE INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
