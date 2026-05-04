param(
    [string]$RepoRoot = (Get-Location).Path,
    [string]$EnvFilePath = "",
    [ValidateSet("1D","1m","5m")]
    [string]$Timeframe = "1D",
    [string]$StartDate = "1993-01-01",
    [string]$EndDate = (Get-Date).ToString("yyyy-MM-dd"),
    [int]$ApiCreditsPerMinute = 8,
    [int]$ApiCreditsReservePerMinute = 1,
    [int]$ApiCreditsPerDay = 800,
    [int]$InterRequestDelayMs = 250,
    [int]$MinuteBoundaryBufferSeconds = 2,
    [string]$StartFromSymbol = "",
    [string]$EndAtSymbol = "",
    [string]$SymbolsRegistryPath = "",
    [switch]$UseLegacyHardcodedSymbols,
    [switch]$ListSymbolsOnly,
    [switch]$AppendOnly,
    [switch]$ContinueOnSymbolError,
    [switch]$WaitForDailyReset,
    [switch]$SkipIngest,
    [switch]$SkipFinalSyncTopOff,
    [switch]$SkipCsvExport
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

function Import-DotEnvFile {
    param(
        [Parameter(Mandatory)]
        [string]$Path
    )

    if (-not (Test-Path $Path)) {
        return $false
    }

    $parsed = [ordered]@{}

    foreach ($rawLine in [System.IO.File]::ReadAllLines($Path)) {
        $line = $rawLine.Trim()

        if ([string]::IsNullOrWhiteSpace($line)) {
            continue
        }

        if ($line.StartsWith("#")) {
            continue
        }

        $hashIndex = $line.IndexOf("#")
        if ($hashIndex -ge 0) {
            $line = $line.Substring(0, $hashIndex).Trim()
        }

        if ([string]::IsNullOrWhiteSpace($line) -or (-not $line.Contains("="))) {
            continue
        }

        $parts = $line.Split("=", 2)
        $name = $parts[0].Trim()
        $value = $parts[1].Trim()

        if ([string]::IsNullOrWhiteSpace($name)) {
            continue
        }

        if (
            ($value.Length -ge 2) -and
            (
                ($value.StartsWith('"') -and $value.EndsWith('"')) -or
                ($value.StartsWith("'") -and $value.EndsWith("'"))
            )
        ) {
            $value = $value.Substring(1, $value.Length - 2)
        }

        # Within the dotenv file, the last occurrence wins.
        $parsed[$name] = $value
    }

    foreach ($entry in $parsed.GetEnumerator()) {
        if ([string]::IsNullOrEmpty([Environment]::GetEnvironmentVariable($entry.Key, "Process"))) {
            [Environment]::SetEnvironmentVariable($entry.Key, [string]$entry.Value, "Process")
        }
    }

    return $true
}

function Set-PgPasswordFromDatabaseUrlIfMissing {
    param(
        [Parameter(Mandatory)]
        [string]$DatabaseUrl
    )

    if (-not [string]::IsNullOrWhiteSpace($env:PGPASSWORD)) {
        return
    }

    try {
        $uri = [System.Uri]$DatabaseUrl
        $userInfo = $uri.UserInfo
        if ([string]::IsNullOrWhiteSpace($userInfo)) {
            return
        }

        $parts = $userInfo.Split(":", 2)
        if ($parts.Count -lt 2) {
            return
        }

        $password = [System.Uri]::UnescapeDataString($parts[1])
        if (-not [string]::IsNullOrWhiteSpace($password)) {
            [Environment]::SetEnvironmentVariable("PGPASSWORD", $password, "Process")
        }
    }
    catch {
        Write-Warning "Could not derive PGPASSWORD from MQK_DATABASE_URL. CSV export may prompt or fail."
    }
}

function Get-TimeframeChunkConfig {
    param(
        [Parameter(Mandatory)]
        [string]$Tf
    )

    switch ($Tf) {
        "1D" {
            return [pscustomobject]@{
                Mode = "Years"
                Size = 8
            }
        }
        "5m" {
            return [pscustomobject]@{
                Mode = "Days"
                Size = 63
            }
        }
        "1m" {
            return [pscustomobject]@{
                Mode = "Days"
                Size = 14
            }
        }
        default {
            throw "Unsupported timeframe: $Tf"
        }
    }
}

function Get-DateChunks {
    param(
        [datetime]$Start,
        [datetime]$End,
        [Parameter(Mandatory)]
        [ValidateSet("Years","Days")]
        [string]$ChunkMode,
        [Parameter(Mandatory)]
        [int]$ChunkSize
    )

    if ($ChunkSize -lt 1) {
        throw "ChunkSize must be >= 1"
    }

    $chunks = @()
    $cursor = $Start

    while ($cursor -le $End) {
        switch ($ChunkMode) {
            "Years" {
                $chunkEnd = $cursor.AddYears($ChunkSize).AddDays(-1)
            }
            "Days" {
                $chunkEnd = $cursor.AddDays($ChunkSize - 1)
            }
            default {
                throw "Unsupported ChunkMode: $ChunkMode"
            }
        }

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

function Resolve-SymbolsFromRegistry {
    param(
        [Parameter(Mandatory)]
        [string]$RegistryPath
    )

    if (-not (Test-Path $RegistryPath)) {
        throw "Registry file not found: $RegistryPath"
    }

    $raw = Get-Content -Raw -Path $RegistryPath | ConvertFrom-Json

    $symbols = @()
    foreach ($entry in $raw) {
        if ($entry.asset_class -ne "equity") { continue }
        if ($entry.enabled -ne $true) { continue }

        $sym = if (
            ($entry.PSObject.Properties.Name -contains "provider_symbol") -and
            (-not [string]::IsNullOrWhiteSpace($entry.provider_symbol))
        ) {
            $entry.provider_symbol
        } else {
            $entry.symbol
        }

        $symbols += $sym.Trim().ToUpperInvariant()
    }

    $deduped = [string[]]($symbols | Sort-Object -Unique)

    if ($deduped.Count -eq 0) {
        throw "Registry at '$RegistryPath' produced zero enabled equity symbols."
    }

    return $deduped
}

if ([string]::IsNullOrWhiteSpace($EnvFilePath)) {
    $candidatePaths = @(
        (Join-Path $RepoRoot ".env.local"),
        (Join-Path (Join-Path $RepoRoot "core-rs") ".env.local")
    )

    foreach ($candidate in $candidatePaths) {
        if (Import-DotEnvFile -Path $candidate) {
            $EnvFilePath = $candidate
            break
        }
    }
}
else {
    if (-not (Import-DotEnvFile -Path $EnvFilePath)) {
        throw "Specified EnvFilePath does not exist: $EnvFilePath"
    }
}

if (-not [string]::IsNullOrWhiteSpace($EnvFilePath)) {
    Write-Host "Loaded environment file: $EnvFilePath"
}
else {
    Write-Warning "No .env.local file was auto-loaded. Using current process environment only."
}

if (-not $ListSymbolsOnly.IsPresent) {
    if (-not $env:TWELVEDATA_API_KEY) {
        throw "Missing env var: TWELVEDATA_API_KEY"
    }

    if (-not $env:MQK_DATABASE_URL) {
        throw "Missing env var: MQK_DATABASE_URL"
    }

    Set-PgPasswordFromDatabaseUrlIfMissing -DatabaseUrl $env:MQK_DATABASE_URL

    if (-not $env:PGPASSWORD) {
        Write-Warning "PGPASSWORD is not set and could not be derived. CSV export via psql may prompt or fail."
    }
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

if (-not $ListSymbolsOnly.IsPresent) {
    $coreRs = Join-Path $RepoRoot "core-rs"
    if (-not (Test-Path $coreRs)) {
        throw "Could not find core-rs under repo root: $RepoRoot"
    }

    $exportRoot = Join-Path $RepoRoot ("exports\md_backup\" + $Timeframe)
    New-Item -ItemType Directory -Force -Path $exportRoot | Out-Null
}

# ---------- Symbol universe resolution ----------
if ([string]::IsNullOrWhiteSpace($SymbolsRegistryPath)) {
    $SymbolsRegistryPath = Join-Path $RepoRoot "config\instruments\equities.json"
}

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

if ($UseLegacyHardcodedSymbols.IsPresent) {
    $allSymbolsFull = [string[]](($repoTop50 + $smallAcctTop50) | Sort-Object -Unique)
    $symbolSource = "legacy-hardcoded"
    $symbolSourcePath = "(inline arrays)"
} elseif (Test-Path $SymbolsRegistryPath) {
    $allSymbolsFull = Resolve-SymbolsFromRegistry -RegistryPath $SymbolsRegistryPath
    $symbolSource = "registry"
    $symbolSourcePath = $SymbolsRegistryPath
} else {
    throw "Registry file not found: $SymbolsRegistryPath`nUse -UseLegacyHardcodedSymbols to fall back to inline arrays."
}

$backfillSymbols = $allSymbolsFull
$appendOnlyMode = $AppendOnly.IsPresent

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

if (-not [string]::IsNullOrWhiteSpace($EndAtSymbol)) {
    $normalizedEndAtSymbol = $EndAtSymbol.Trim().ToUpperInvariant()
    $endIndex = [Array]::IndexOf($allSymbolsFull, $normalizedEndAtSymbol)

    if ($endIndex -lt 0) {
        throw "EndAtSymbol '$normalizedEndAtSymbol' was not found in the deduped symbol list."
    }

    $filtered = @()
    foreach ($symbol in $backfillSymbols) {
        $symbolIndex = [Array]::IndexOf($allSymbolsFull, $symbol)
        if ($symbolIndex -le $endIndex) {
            $filtered += $symbol
        }
    }

    if ($filtered.Count -eq 0) {
        throw "EndAtSymbol '$normalizedEndAtSymbol' produces an empty symbol slice with the current StartFromSymbol."
    }

    $backfillSymbols = [string[]]$filtered
}

$finalSyncSymbols = if ((-not [string]::IsNullOrWhiteSpace($StartFromSymbol)) -or (-not [string]::IsNullOrWhiteSpace($EndAtSymbol))) {
    $backfillSymbols
}
else {
    $allSymbolsFull
}

$exportSymbols = $finalSyncSymbols

# ---------- List-symbols-only exit (no provider call, no DB/CSV write) ----------
if ($ListSymbolsOnly.IsPresent) {
    Write-Host ""
    Write-Host "=== LIST-SYMBOLS-ONLY MODE (no provider calls, no DB/CSV writes) ==="
    Write-Host "Symbol source    : $symbolSource"
    Write-Host "Registry path    : $symbolSourcePath"
    Write-Host "Total universe   : $($allSymbolsFull.Count)"
    Write-Host "Backfill symbols : $($backfillSymbols.Count)"

    if ($allSymbolsFull.Count -gt 0) {
        $showFirst = [math]::Min(5, $allSymbolsFull.Count)
        $showLast  = [math]::Min(5, $allSymbolsFull.Count)
        $firstFew  = $allSymbolsFull[0..($showFirst - 1)]
        $lastStart = [math]::Max(0, $allSymbolsFull.Count - $showLast)
        $lastFew   = $allSymbolsFull[$lastStart..($allSymbolsFull.Count - 1)]
        Write-Host "First symbols    : $($firstFew -join ', ')"
        Write-Host "Last symbols     : $($lastFew -join ', ')"
    }

    if ($backfillSymbols.Count -ne $allSymbolsFull.Count) {
        Write-Host "Sliced symbols   : $($backfillSymbols -join ', ')"
    }

    Write-Host ""
    exit 0
}

$startDt = [datetime]::ParseExact($StartDate, "yyyy-MM-dd", $null)
$endDt   = [datetime]::ParseExact($EndDate, "yyyy-MM-dd", $null)

if ($endDt -lt $startDt) {
    throw "EndDate must be >= StartDate"
}

$chunkConfig = Get-TimeframeChunkConfig -Tf $Timeframe
$chunks = Get-DateChunks -Start $startDt -End $endDt -ChunkMode $chunkConfig.Mode -ChunkSize $chunkConfig.Size

$script:EffectiveMinuteBudget = $ApiCreditsPerMinute - $ApiCreditsReservePerMinute
if ($script:EffectiveMinuteBudget -lt 1) {
    throw "Effective minute budget must be >= 1. Current values: ApiCreditsPerMinute=$ApiCreditsPerMinute ApiCreditsReservePerMinute=$ApiCreditsReservePerMinute"
}

$script:ThrottleMinuteKey = ""
$script:ThrottleDayKey = ""
$script:CreditsUsedThisMinute = 0
$script:CreditsUsedToday = 0

$totalBackfillRequests = $backfillSymbols.Count * $chunks.Count
$totalFinalSyncRequests = if ($SkipFinalSyncTopOff) { 0 } else { $finalSyncSymbols.Count }
$totalPlannedRequests = $totalBackfillRequests + $totalFinalSyncRequests

$phaseFailures = New-Object System.Collections.Generic.List[object]

Write-Host "Repo root: $RepoRoot"
Write-Host "Symbol source: $symbolSource ($symbolSourcePath)"
Write-Host "Timeframe: $Timeframe"
Write-Host "Universe symbols total (deduped): $($allSymbolsFull.Count)"
Write-Host "Backfill symbols in this run: $($backfillSymbols.Count)"
if (-not [string]::IsNullOrWhiteSpace($EndAtSymbol)) {
    Write-Host "Backfill end symbol: $($EndAtSymbol.Trim().ToUpperInvariant())"
}
if (-not [string]::IsNullOrWhiteSpace($StartFromSymbol)) {
    Write-Host "Backfill start symbol: $($StartFromSymbol.Trim().ToUpperInvariant())"
    Write-Host "Final sync top-off targets the selected symbol slice when StartFromSymbol or EndAtSymbol is used."
}
Write-Host "Chunk mode: $($chunkConfig.Mode)"
Write-Host "Chunk size: $($chunkConfig.Size)"
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
Write-Host "Append-only mode: $appendOnlyMode"
Write-Host "Skip historical backfill: $($SkipIngest.IsPresent -or $appendOnlyMode)"
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

    if (-not ($SkipIngest -or $appendOnlyMode)) {
        Write-Host "==================== HISTORICAL BACKFILL PHASE ===================="
        foreach ($symbol in $backfillSymbols) {
            Write-Host "============================================================"
            Write-Host "BACKFILL SYMBOL: $symbol"
            Write-Host "============================================================"

            try {
                foreach ($chunk in $chunks) {
                    $requestIndex += 1
                    $context = "$symbol $Timeframe $($chunk.Start) -> $($chunk.End)"

                    Reserve-TwelveDataBudget -CreditsNeeded 1 -Context $context

                    Write-Host "[$requestIndex/$totalPlannedRequests] Backfilling $context"
                    Invoke-CheckedExternal `
                        -FilePath "cargo" `
                        -Arguments @(
                            "run","-p","mqk-cli","--bin","mqk-cli","--",
                            "md","ingest-provider",
                            "--source","twelvedata",
                            "--symbols",$symbol,
                            "--timeframe",$Timeframe,
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
        Write-Host "This phase runs incremental sync for the selected symbol slice."
        foreach ($symbol in $finalSyncSymbols) {
            Write-Host "============================================================"
            Write-Host "SYNC SYMBOL: $symbol"
            Write-Host "============================================================"

            try {
                $requestIndex += 1
                $context = "$symbol $Timeframe final sync"

                Reserve-TwelveDataBudget -CreditsNeeded 1 -Context $context

                Write-Host "[$requestIndex/$totalPlannedRequests] Final sync top-off for $symbol"
                Invoke-CheckedExternal `
                    -FilePath "cargo" `
                    -Arguments @(
                        "run","-p","mqk-cli","--bin","mqk-cli","--",
                        "md","sync-provider",
                        "--source","twelvedata",
                        "--symbols",$symbol,
                        "--timeframe",$Timeframe,
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
        foreach ($symbol in $exportSymbols) {
            $safeTimeframe = $Timeframe.Replace("/", "-")
            $csvPath = Join-Path $exportRoot ("{0}_{1}.csv" -f $symbol, $safeTimeframe)
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
                      and timeframe = '$Timeframe'
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
