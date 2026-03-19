$ErrorActionPreference = "Stop"

# ============================================================
# MiniQuantDesk V4 — Full Repo Proof Script
# Forced DB URL version
# ============================================================

$env:MQK_DATABASE_URL = "postgres://postgres:YourNewStrongPassword123!@127.0.0.1:5433/mqk_test"

# ============================================================
# Repo + environment bootstrap
# ============================================================

$RepoRoot = "C:\Users\Zacha\Desktop\MiniQuantDeskV4"
$CoreRs   = Join-Path $RepoRoot "core-rs"
$GuiDir   = Join-Path $CoreRs "mqk-gui"
$GitBash  = "C:\Program Files\Git\bin\bash.exe"

function Get-RedactedDbUrl {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Url
    )

    if ([string]::IsNullOrWhiteSpace($Url)) {
        return "<not-set>"
    }

    # Redacts: postgres://user:password@host:port/db
    if ($Url -match '^(postgres(?:ql)?://[^:/?#]+:)([^@/?#]+)(@.+)$') {
        return ($matches[1] + '****' + $matches[3])
    }

    # Fallback if format is unusual; never print the raw value.
    return "<redacted-unparseable-db-url>"
}

$DbUrl = $env:MQK_DATABASE_URL
if ([string]::IsNullOrWhiteSpace($DbUrl)) {
    throw "MQK_DATABASE_URL is not set"
}

Set-Location $RepoRoot

function Run-Step {
    param(
        [string]$Name,
        [scriptblock]$Command
    )

    Write-Host ""
    Write-Host "============================================================" -ForegroundColor Cyan
    Write-Host $Name -ForegroundColor Cyan
    Write-Host "============================================================" -ForegroundColor Cyan

    & $Command

    if ($LASTEXITCODE -ne 0) {
        throw ("Step failed with exit code {0}: {1}" -f $LASTEXITCODE, $Name)
    }
}

function Require-Path {
    param([string]$PathToCheck, [string]$Label)

    if (-not (Test-Path $PathToCheck)) {
        throw ("Missing required path for {0}: {1}" -f $Label, $PathToCheck)
    }
}

Require-Path $RepoRoot "repo root"
Require-Path $CoreRs "core-rs"
Require-Path $GuiDir "GUI dir"
Require-Path $GitBash "Git Bash"

Write-Host ""
Write-Host "Repo root: $RepoRoot" -ForegroundColor Yellow
Write-Host "core-rs:   $CoreRs" -ForegroundColor Yellow
Write-Host "GUI dir:   $GuiDir" -ForegroundColor Yellow
Write-Host ("Using MQK_DATABASE_URL={0}" -f (Get-RedactedDbUrl -Url $DbUrl)) -ForegroundColor Green

Run-Step "Repo identity" {
    git status
    git rev-parse HEAD
}

Run-Step "Rust fmt" {
    cargo fmt --manifest-path .\core-rs\Cargo.toml --all
    cargo fmt --manifest-path .\core-rs\Cargo.toml --all --check
}

Run-Step "Workspace clippy" {
    cargo clippy --manifest-path .\core-rs\Cargo.toml --workspace --all-targets -- -D warnings
}

Run-Step "Workspace tests" {
    cargo test --manifest-path .\core-rs\Cargo.toml --workspace -- --test-threads=1
}

Run-Step "Daemon proof lanes" {
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_daemon_routes -- --test-threads=1
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_gui_daemon_contract_gate -- --test-threads=1
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_snapshot_inject_release_gate -- --test-threads=1
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_token_auth_middleware -- --test-threads=1
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_daemon_boot_is_fail_closed -- --test-threads=1
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_daemon_deadman_blocks_dispatch -- --test-threads=1
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_reconcile_tick_disarms_on_drift -- --test-threads=1
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_daemon_runtime_lifecycle -- --test-threads=1
}

Run-Step "Runtime proof lane" {
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-runtime -- --test-threads=1
    cargo clippy --manifest-path .\core-rs\Cargo.toml -p mqk-runtime --all-targets -- -D warnings
}

Run-Step "Broker Alpaca proof lane" {
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-broker-alpaca -- --test-threads=1
    cargo clippy --manifest-path .\core-rs\Cargo.toml -p mqk-broker-alpaca --all-targets -- -D warnings
}

Run-Step "Market data proof lane" {
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-md -- --test-threads=1
    cargo clippy --manifest-path .\core-rs\Cargo.toml -p mqk-md --all-targets -- -D warnings
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-db --test scenario_md_ingest_provider -- --test-threads=1
}

Run-Step "GUI typecheck + build" {
    Push-Location $GuiDir
    try {
        npx tsc --noEmit
        npm run build
    }
    finally {
        Pop-Location
    }
}

Run-Step "Ignored-proof guard" {
    & $GitBash ./scripts/guards/check_ignored_load_bearing_proofs.sh
}

Run-Step "Unsafe-pattern guard" {
    & $GitBash ./scripts/guards/check_unsafe_patterns.sh
}

Run-Step "DB proof bootstrap" {
    & $GitBash ./scripts/db_proof_bootstrap.sh
}

Run-Step "DB-backed mqk-db ignored lanes" {
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-db -- --include-ignored --test-threads=1
}

Run-Step "DB-backed daemon lifecycle ignored lane" {
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_daemon_runtime_lifecycle -- --include-ignored --test-threads=1
}

Run-Step "DB-backed daemon routes ignored lane" {
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_daemon_routes -- --include-ignored --test-threads=1
}

Run-Step "DB-backed market data ingest-provider ignored lane" {
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-db --test scenario_md_ingest_provider -- --include-ignored --test-threads=1
}

Run-Step "Broker map FK proof" {
    cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-db --test scenario_broker_map_fk_enforced -- --test-threads=1
}

Write-Host ""
Write-Host "============================================================" -ForegroundColor Green
Write-Host "FULL REPO PROOF COMPLETED SUCCESSFULLY" -ForegroundColor Green
Write-Host "============================================================" -ForegroundColor Green