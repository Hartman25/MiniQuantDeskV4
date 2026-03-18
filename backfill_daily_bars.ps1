param(
    [string]$RepoRoot = (Get-Location).Path,
    [string]$StartDate = "1993-01-01",
    [string]$EndDate = "2026-03-17",
    [switch]$SkipIngest,
    [switch]$SkipCsvExport
)

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

# TEMP: controlled first run with 10 repo symbols only
$allSymbols = @(
    "SPY","QQQ","IWM","DIA","TLT","IEF","XLF","XLK","XLV","SMH"
)

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

$startDt = [datetime]::ParseExact($StartDate, "yyyy-MM-dd", $null)
$endDt   = [datetime]::ParseExact($EndDate, "yyyy-MM-dd", $null)
$chunks  = Get-DateChunks -Start $startDt -End $endDt -YearsPerChunk 8

Write-Host "Repo root: $RepoRoot"
Write-Host "Symbols total (deduped): $($allSymbols.Count)"
Write-Host "Chunk count: $($chunks.Count)"
Write-Host "Date range: $StartDate -> $EndDate"
Write-Host ""

Push-Location $coreRs
try {
    # Optional: migrate first
    Write-Host "Running DB migrate..."
    cargo run -p mqk-cli --bin mqk-cli -- db migrate --yes

    foreach ($symbol in $allSymbols) {
        Write-Host "============================================================"
        Write-Host "SYMBOL: $symbol"
        Write-Host "============================================================"

        if (-not $SkipIngest) {
            foreach ($chunk in $chunks) {
                Write-Host "Ingesting $symbol 1D $($chunk.Start) -> $($chunk.End)"
                cargo run -p mqk-cli --bin mqk-cli -- md ingest-provider `
                    --source "twelvedata" `
                    --symbols $symbol `
                    --timeframe "1D" `
                    --start $chunk.Start `
                    --end $chunk.End

                if ($LASTEXITCODE -ne 0) {
                    throw "Ingest failed for $symbol $($chunk.Start) -> $($chunk.End)"
                }
            }
        }

        if (-not $SkipCsvExport) {
            $csvPath = Join-Path $exportRoot "${symbol}_1D.csv"
            Write-Host "Exporting CSV -> $csvPath"

            psql "$env:MQK_DATABASE_URL" -c "\copy (
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
        }

        Write-Host "Done: $symbol"
        Write-Host ""
    }
}
finally {
    Pop-Location
}

Write-Host "All done."
Write-Host "CSV backups: $exportRoot"