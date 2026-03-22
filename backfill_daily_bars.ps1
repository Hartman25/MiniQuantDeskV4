param(
    [string]$RepoRoot = (Get-Location).Path,
    [string]$StartDate = "1993-01-01",
    [string]$EndDate = (Get-Date).ToString("yyyy-MM-dd"),
    [int]$ApiCreditsPerMinute = 8,
    [int]$ApiCreditsReservePerMinute = 1,
    [int]$ApiCreditsPerDay = 800,
    [int]$InterRequestDelayMs = 250,
    [int]$MinuteBoundaryBufferSeconds = 2,
    [string]$StartFromSymbol = "",
    [switch]$ContinueOnSymbolError,
    [switch]$WaitForDailyReset,
    [switch]$SkipIngest,
    [switch]$SkipFinalSyncTopOff,
    [switch]$SkipCsvExport
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

if (-not $env:TWELVEDATA_API_KEY) {
    throw "Missing env var: TWELVEDATA_API_KEY"
}

if (-not $env:MQK_DATABASE_URL) {
    throw "Missing env var: MQK_DATABASE_URL"
}

if (-not $env:PGPASSWORD) {
    Write-Warning "PGPASSWORD is not set. CSV export via psql may prompt or fail."
}

if ($ApiCreditsPerMinute -lt 1) {
    throw "ApiCreditsPerMinute must be >= 1"
}

if ($ApiCreditsReservePerMinute -lt 0) {
    throw "ApiCreditsReservePerMinute must be >= 0"
}

if ($ApiCreditsPerDay -lt 0) {
    throw "ApiCreditsPerDay must be >= 0 (set to 0 to disable daily cap)"
}

if ($InterRequestDelayMs -lt 0) {
    throw "InterRequestDelayMs must be >= 0"
}

if ($MinuteBoundaryBufferSeconds -lt 0) {
    throw "MinuteBoundaryBufferSeconds must be >= 0"
}

$coreRs = Join-Path $RepoRoot "core-rs"
if (-not (Test-Path $coreRs)) {
    throw "Could not find core-rs under repo root: $RepoRoot"
}

$exportRoot = Join-Path $RepoRoot "exports\md_backup\daily"
New-Item -ItemType Directory -Force -Path $exportRoot | Out-Null

# ---------- Symbol lists ----------
$repoTop50 = @(
    "SPY","QQQ","IWM","DIA","TLT","IEF","SHY","EEM","EFA","VTI",
    "XLF","XLK","XLV","XLE","XLI","XLY","XLP","XLU","XLB","VNQ",
    "SMH","GLD","SLV",
    "AAPL","MSFT","INTC","CSCO","ORCL","IBM","GE","JPM","BAC","WFC","GS","WMT","HD","KO","PEP","PFE","JNJ","XOM","CVX",
    "NVDA","AMD","AMZN","GOOGL","META","TSLA","NFLX","PLTR"
)

$smallAcctTop50 = @(
    "SOFI","F","INTC","RIVN","LCID","MARA","NIO","DKNG","PLUG","OPEN",
    "HOOD","PFE","BAC","WFC","T","VZ","KGC","NEM","AAL","UAL",
    "CCL","NCLH","JBLU",
    "XLF","XLE","XLP","XLU","XLI","ARKK","TAN","ICLN","KWEB","FXI","GDX","GDXJ","SLV","BITO",
    "RIOT","RKLB","HIMS","IONQ","ACHR","JOBY","AFRM","UPST","RBLX","CHPT","LYFT","PLTR"
)

$allSymbolsFull = [string[]](($repoTop50 + $smallAcctTop50) | Sort-Object -Unique)
$backfillSymbols = $allSymbolsFull

if (-not [string]::IsNullOrWhiteSpace($StartFromSymbol)) {
    $normalizedStartFromSymbol = $StartFromSymbol.Trim().ToUpperInvariant()
    $startIndex = [Array]::IndexOf($allSymbolsFull, $normalizedStartFromSymbol)

    if ($startIndex -lt 0) {
        throw "StartFromSymbol '$normalizedStartFromSymbol' was not found in the deduped symbol list."
    }

    if ($startIndex -lt ($allSymbolsFull.Count - 1)) {
        $backfillSymbols = [string[]]$allSymbolsFull[$startIndex..($allSymbolsFull.Count - 1)]
    }
    else {
        $backfillSymbols = [string[]]@($allSymbolsFull[$startIndex])
    }
}

function Get-DateChunks {
    param(
        [datetime]$Start,
        [datetime]$End,
        [int]$YearsPerChunk = 8
    )

    $chunks = @()
    $cursor = $Start

    while ($cursor -le $End) {
        $chunkEnd = $cursor.AddYears($YearsPerChunk).AddDays(-1)
        if ($chunkEnd -gt $End) {
            $chunkEnd = $End
        }

        $chunks += [pscustomobject]@{
            Start = $cursor.ToString("yyyy-MM-dd")
            End   = $chunkEnd.ToString("yyyy-MM-dd")
        }

        $cursor = $chunkEnd.AddDays(1)
    }

    return $chunks
}

function Get-NextUtcMinuteBoundary {
    $utcNow = [datetime]::UtcNow
    return [datetime]::new(
        $utcNow.Year,
        $utcNow.Month,
        $utcNow.Day,
        $utcNow.Hour,
        $utcNow.Minute,
        0,
        [DateTimeKind]::Utc
    ).AddMinutes(1)
}

function Get-NextUtcMidnightBoundary {
    $utcNow = [datetime]::UtcNow
    return [datetime]::new(
        $utcNow.Year,
        $utcNow.Month,
        $utcNow.Day,
        0,
        0,
        0,
        [DateTimeKind]::Utc
    ).AddDays(1)
}

function Reset-RateWindowsIfNeeded {
    $utcNow = [datetime]::UtcNow

    $minuteKey = $utcNow.ToString("yyyyMMddHHmm")
    if ($script:ThrottleMinuteKey -ne $minuteKey) {
        $script:ThrottleMinuteKey = $minuteKey
        $script:CreditsUsedThisMinute = 0
    }

    $dayKey = $utcNow.ToString("yyyyMMdd")
    if ($script:ThrottleDayKey -ne $dayKey) {
        $script:ThrottleDayKey = $dayKey
        $script:CreditsUsedToday = 0
    }
}

function Reserve-TwelveDataBudget {
    param(
        [int]$CreditsNeeded = 1,
        [string]$Context = "request"
    )

    if ($CreditsNeeded -lt 1) {
        throw "CreditsNeeded must be >= 1"
    }

    if ($CreditsNeeded -gt $script:EffectiveMinuteBudget) {
        throw "CreditsNeeded ($CreditsNeeded) exceeds effective minute budget ($($script:EffectiveMinuteBudget)). Lower batch size or raise ApiCreditsPerMinute."
    }

    if (($ApiCreditsPerDay -gt 0) -and ($CreditsNeeded -gt $ApiCreditsPerDay)) {
        throw "CreditsNeeded ($CreditsNeeded) exceeds configured daily budget ($ApiCreditsPerDay)."
    }

    while ($true) {
        Reset-RateWindowsIfNeeded

        $minuteWouldFit = ($script:CreditsUsedThisMinute + $CreditsNeeded) -le $script:EffectiveMinuteBudget
        $dayWouldFit = ($ApiCreditsPerDay -le 0) -or (($script:CreditsUsedToday + $CreditsNeeded) -le $ApiCreditsPerDay)

        if ($minuteWouldFit -and $dayWouldFit) {
            $script:CreditsUsedThisMinute += $CreditsNeeded
            if ($ApiCreditsPerDay -gt 0) {
                $script:CreditsUsedToday += $CreditsNeeded
            }
            return
        }

        if (-not $dayWouldFit) {
            if (-not $WaitForDailyReset) {
                throw "Configured daily API credit limit ($ApiCreditsPerDay) reached before $Context. Re-run after next UTC midnight or override -ApiCreditsPerDay for your actual plan."
            }

            $wakeAt = Get-NextUtcMidnightBoundary
            $sleepSeconds = [math]::Ceiling(($wakeAt - [datetime]::UtcNow).TotalSeconds)
            if ($sleepSeconds -lt 1) {
                $sleepSeconds = 1
            }

            Write-Host "Daily credit budget exhausted before $Context. Sleeping until next UTC day reset ($sleepSeconds s)..."
            Start-Sleep -Seconds $sleepSeconds
            continue
        }

        $wakeAt = (Get-NextUtcMinuteBoundary).AddSeconds($MinuteBoundaryBufferSeconds)
        $sleepSeconds = [math]::Ceiling(($wakeAt - [datetime]::UtcNow).TotalSeconds)
        if ($sleepSeconds -lt 1) {
            $sleepSeconds = 1
        }

        Write-Host "Minute credit budget exhausted before $Context. Sleeping until next UTC minute window ($sleepSeconds s)..."
        Start-Sleep -Seconds $sleepSeconds
    }
}

function Invoke-CheckedExternal {
    param(
        [string]$FilePath,
        [string[]]$Arguments,
        [string]$FailureMessage
    )

    & $FilePath @Arguments

    if ($LASTEXITCODE -ne 0) {
        throw $FailureMessage
    }
}

function Add-PhaseFailure {
    param(
        [System.Collections.Generic.List[object]]$Failures,
        [string]$Phase,
        [string]$Symbol,
        [string]$Detail
    )

    $Failures.Add([pscustomobject]@{
        Phase  = $Phase
        Symbol = $Symbol
        Error  = $Detail
    }) | Out-Null
}

$startDt = [datetime]::ParseExact($StartDate, "yyyy-MM-dd", $null)
$endDt   = [datetime]::ParseExact($EndDate, "yyyy-MM-dd", $null)

if ($endDt -lt $startDt) {
    throw "EndDate must be >= StartDate"
}

$chunks = Get-DateChunks -Start $startDt -End $endDt -YearsPerChunk 8

$script:EffectiveMinuteBudget = $ApiCreditsPerMinute - $ApiCreditsReservePerMinute
if ($script:EffectiveMinuteBudget -lt 1) {
    throw "Effective minute budget must be >= 1. Current values: ApiCreditsPerMinute=$ApiCreditsPerMinute ApiCreditsReservePerMinute=$ApiCreditsReservePerMinute"
}

$script:ThrottleMinuteKey = ""
$script:ThrottleDayKey = ""
$script:CreditsUsedThisMinute = 0
$script:CreditsUsedToday = 0

$totalBackfillRequests = $backfillSymbols.Count * $chunks.Count
$totalFinalSyncRequests = if ($SkipFinalSyncTopOff) { 0 } else { $allSymbolsFull.Count }
$totalPlannedRequests = $totalBackfillRequests + $totalFinalSyncRequests

$phaseFailures = New-Object System.Collections.Generic.List[object]

Write-Host "Repo root: $RepoRoot"
Write-Host "Universe symbols total (deduped): $($allSymbolsFull.Count)"
Write-Host "Backfill symbols in this run: $($backfillSymbols.Count)"
if (-not [string]::IsNullOrWhiteSpace($StartFromSymbol)) {
    Write-Host "Backfill start symbol: $($StartFromSymbol.Trim().ToUpperInvariant())"
    Write-Host "Final sync top-off still targets FULL universe."
}
Write-Host "Chunk count per symbol: $($chunks.Count)"
Write-Host "Historical backfill range: $StartDate -> $EndDate"
Write-Host "Planned backfill requests: $totalBackfillRequests"
Write-Host "Planned final sync requests: $totalFinalSyncRequests"
Write-Host "Total planned throttled requests: $totalPlannedRequests"
Write-Host "Minute budget: $($script:EffectiveMinuteBudget) usable credits/min (configured $ApiCreditsPerMinute, reserve $ApiCreditsReservePerMinute)"
if ($ApiCreditsPerDay -gt 0) {
    Write-Host "Daily budget: $ApiCreditsPerDay credits/day UTC"
}
else {
    Write-Host "Daily budget: disabled"
}
Write-Host "Inter-request delay: $InterRequestDelayMs ms"
Write-Host "Continue on symbol error: $($ContinueOnSymbolError.IsPresent)"
Write-Host "Skip historical backfill: $($SkipIngest.IsPresent)"
Write-Host "Skip final sync top-off: $($SkipFinalSyncTopOff.IsPresent)"
Write-Host "Skip CSV export: $($SkipCsvExport.IsPresent)"
Write-Host ""

Push-Location $coreRs
try {
    Write-Host "Running DB migrate..."
    Invoke-CheckedExternal `
        -FilePath "cargo" `
        -Arguments @("run","-p","mqk-cli","--bin","mqk-cli","--","db","migrate","--yes") `
        -FailureMessage "DB migrate failed"

    $requestIndex = 0

    if (-not $SkipIngest) {
        Write-Host "==================== HISTORICAL BACKFILL PHASE ===================="
        foreach ($symbol in $backfillSymbols) {
            Write-Host "============================================================"
            Write-Host "BACKFILL SYMBOL: $symbol"
            Write-Host "============================================================"

            try {
                foreach ($chunk in $chunks) {
                    $requestIndex += 1
                    $context = "$symbol 1D $($chunk.Start) -> $($chunk.End)"

                    Reserve-TwelveDataBudget -CreditsNeeded 1 -Context $context

                    Write-Host "[$requestIndex/$totalPlannedRequests] Backfilling $context"
                    Invoke-CheckedExternal `
                        -FilePath "cargo" `
                        -Arguments @(
                            "run","-p","mqk-cli","--bin","mqk-cli","--",
                            "md","ingest-provider",
                            "--source","twelvedata",
                            "--symbols",$symbol,
                            "--timeframe","1D",
                            "--start",$chunk.Start,
                            "--end",$chunk.End
                        ) `
                        -FailureMessage "Historical backfill failed for $context"

                    if ($InterRequestDelayMs -gt 0) {
                        Start-Sleep -Milliseconds $InterRequestDelayMs
                    }
                }

                Write-Host "Backfill complete: $symbol"
                Write-Host ""
            }
            catch {
                $failureMessage = $_.Exception.Message
                Add-PhaseFailure -Failures $phaseFailures -Phase "Backfill" -Symbol $symbol -Detail $failureMessage

                if ($ContinueOnSymbolError) {
                    Write-Warning "Backfill failed and will be skipped: $symbol"
                    Write-Warning $failureMessage
                    Write-Host ""
                    continue
                }

                throw
            }
        }
    }

    if (-not $SkipFinalSyncTopOff) {
        Write-Host "==================== FINAL SYNC TOP-OFF PHASE ===================="
        Write-Host "This phase revisits the FULL universe so earlier symbols can move toward current day."
        foreach ($symbol in $allSymbolsFull) {
            Write-Host "============================================================"
            Write-Host "SYNC SYMBOL: $symbol"
            Write-Host "============================================================"

            try {
                $requestIndex += 1
                $context = "$symbol 1D final sync"

                Reserve-TwelveDataBudget -CreditsNeeded 1 -Context $context

                Write-Host "[$requestIndex/$totalPlannedRequests] Final sync top-off for $symbol"
                Invoke-CheckedExternal `
                    -FilePath "cargo" `
                    -Arguments @(
                        "run","-p","mqk-cli","--bin","mqk-cli","--",
                        "md","sync-provider",
                        "--source","twelvedata",
                        "--symbols",$symbol,
                        "--timeframe","1D",
                        "--full-start",$StartDate
                    ) `
                    -FailureMessage "Final sync top-off failed for $symbol"

                if ($InterRequestDelayMs -gt 0) {
                    Start-Sleep -Milliseconds $InterRequestDelayMs
                }

                Write-Host "Final sync complete: $symbol"
                Write-Host ""
            }
            catch {
                $failureMessage = $_.Exception.Message
                Add-PhaseFailure -Failures $phaseFailures -Phase "FinalSync" -Symbol $symbol -Detail $failureMessage

                if ($ContinueOnSymbolError) {
                    Write-Warning "Final sync failed and will be skipped: $symbol"
                    Write-Warning $failureMessage
                    Write-Host ""
                    continue
                }

                throw
            }
        }
    }

    if (-not $SkipCsvExport) {
        Write-Host "==================== CSV EXPORT PHASE ===================="
        foreach ($symbol in $allSymbolsFull) {
            $csvPath = Join-Path $exportRoot "${symbol}_1D.csv"
            Write-Host "Exporting CSV -> $csvPath"

            try {
                & psql "$env:MQK_DATABASE_URL" -c "\copy (
                    select
                        symbol,
                        timeframe,
                        end_ts,
                        open_micros,
                        high_micros,
                        low_micros,
                        close_micros,
                        volume,
                        is_complete,
                        ingested_at
                    from md_bars
                    where symbol = '$symbol'
                      and timeframe = '1D'
                    order by end_ts
                ) to '$csvPath' with csv header"

                if ($LASTEXITCODE -ne 0) {
                    throw "CSV export failed for $symbol"
                }

                Write-Host "CSV export complete: $symbol"
                Write-Host ""
            }
            catch {
                $failureMessage = $_.Exception.Message
                Add-PhaseFailure -Failures $phaseFailures -Phase "CsvExport" -Symbol $symbol -Detail $failureMessage

                if ($ContinueOnSymbolError) {
                    Write-Warning "CSV export failed and will be skipped: $symbol"
                    Write-Warning $failureMessage
                    Write-Host ""
                    continue
                }

                throw
            }
        }
    }
}
finally {
    Pop-Location
}

Write-Host "All done."
Write-Host "CSV backups: $exportRoot"

if ($phaseFailures.Count -gt 0) {
    Write-Warning "Failures recorded during this run:"
    foreach ($failure in $phaseFailures) {
        Write-Warning (" - Phase={0} Symbol={1} Error={2}" -f $failure.Phase, $failure.Symbol, $failure.Error)
    }
}
else {
    Write-Host "No symbol-level failures recorded."
}