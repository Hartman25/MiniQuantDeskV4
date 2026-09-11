# =============================================================================
# M1-13C-MIGRATION-0069-HISTORICAL-UPGRADE-FENCE-01
#
# Static guard for the production migration authority:
#   - official Paper startup paths must route through mqk_db_migrate;
#   - raw sqlx migration invocation is forbidden in those entrypoints;
#   - the runner must delegate to mqk_db::migrate;
#   - the Rust migration helper must contain the historical 0069 fence.
#
# No daemon, no DB, no network, no secrets.
# =============================================================================

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path

$Launcher = Join-Path $RepoRoot 'scripts\windows\Start-MiniQuantDesk.ps1'
$Smoke = Join-Path $RepoRoot 'scripts\windows\Start-PaperTradingSmoke.ps1'
$Runner = Join-Path $RepoRoot 'core-rs\crates\mqk-db\src\bin\mqk_db_migrate.rs'
$Lib = Join-Path $RepoRoot 'core-rs\crates\mqk-db\src\lib.rs'

$Failures = 0

function Pass {
    param([string]$Id, [string]$Message)
    Write-Host "PASS [$Id] $Message" -ForegroundColor Green
}

function Fail {
    param([string]$Id, [string]$Message)
    Write-Host "FAIL [$Id] $Message" -ForegroundColor Red
    $script:Failures++
}

foreach ($Target in @($Launcher, $Smoke, $Runner, $Lib)) {
    if (-not (Test-Path -LiteralPath $Target -PathType Leaf)) {
        Fail 'M13C-01' "required file missing: $Target"
    }
}

if ($Failures -ne 0) {
    exit 1
}

$LauncherText = Get-Content -LiteralPath $Launcher -Raw
$SmokeText = Get-Content -LiteralPath $Smoke -Raw
$RunnerText = Get-Content -LiteralPath $Runner -Raw
$LibText = Get-Content -LiteralPath $Lib -Raw

foreach ($Item in @(
    @{ Name = 'official launcher'; Text = $LauncherText },
    @{ Name = 'Paper smoke'; Text = $SmokeText }
)) {
    if ($Item.Text -match 'mqk_db_migrate') {
        Pass 'M13C-02' "$($Item.Name) routes through mqk_db_migrate"
    } else {
        Fail 'M13C-02' "$($Item.Name) does not route through mqk_db_migrate"
    }

    if ($Item.Text -match '(?im)^\s*(?:&\s*)?\$?sqlxCmd\s+migrate\s+run\b' -or
        $Item.Text -match '(?im)--bin\s+sqlx\s+--\s+migrate\s+run\b') {
        Fail 'M13C-03' "$($Item.Name) still contains a raw SQLx migration execution path"
    } else {
        Pass 'M13C-03' "$($Item.Name) contains no raw SQLx migration execution path"
    }
}

if ($RunnerText -match 'mqk_db::migrate\(&pool\)\.await') {
    Pass 'M13C-04' 'dedicated runner delegates to mqk_db::migrate'
} else {
    Fail 'M13C-04' 'dedicated runner does not delegate to mqk_db::migrate'
}

if ($RunnerText -match 'database-url|println!.*MQK_DATABASE_URL') {
    Fail 'M13C-05' 'runner appears to accept/print DB URL instead of env-only connection'
} else {
    Pass 'M13C-05' 'runner keeps DB URL env-only and does not print it'
}

$RequiredLibPatterns = @(
    'M1-13C-MIGRATION-0069-HISTORICAL-UPGRADE-FENCE-01',
    'SqlxMigrate::lock',
    'LOCK TABLE runs IN SHARE ROW EXCLUSIVE MODE',
    'LOCK TABLE runtime_leader_lease IN SHARE ROW EXCLUSIVE MODE',
    "status IN \('ARMED', 'RUNNING'\)",
    'migration_69_applied',
    'set_locking\(false\)'
)

foreach ($Pattern in $RequiredLibPatterns) {
    if ($LibText -match $Pattern) {
        Pass 'M13C-06' "mqk-db migration helper contains: $Pattern"
    } else {
        Fail 'M13C-06' "mqk-db migration helper missing: $Pattern"
    }
}

foreach ($Target in @($Launcher, $Smoke)) {
    $Tokens = $null
    $Errors = $null
    [System.Management.Automation.Language.Parser]::ParseFile(
        $Target,
        [ref]$Tokens,
        [ref]$Errors
    ) | Out-Null

    if ($Errors.Count -eq 0) {
        Pass 'M13C-07' "PowerShell parser accepted $Target"
    } else {
        foreach ($Err in $Errors) {
            Fail 'M13C-07' "parse error in $Target line $($Err.Extent.StartLineNumber): $($Err.Message)"
        }
    }
}

if ($Failures -eq 0) {
    Write-Host ''
    Write-Host 'M1-13C migration upgrade fence static guard: PASS' -ForegroundColor Green
    exit 0
}

Write-Host ''
Write-Host "M1-13C migration upgrade fence static guard: FAIL ($Failures)" -ForegroundColor Red
exit 1
