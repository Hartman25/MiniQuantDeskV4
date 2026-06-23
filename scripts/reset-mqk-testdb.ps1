# reset-mqk-testdb.ps1 — PROOF-DB-RESET-01
#
# Drops and recreates a LOCAL PROOF-ONLY database so that DB-backed tests can
# run against a clean migration state.  Resolves stale sqlx migration checksum
# errors ("migration N was previously applied but has been modified").
#
# SAFE BY DEFAULT:
#   - Requires -DatabaseUrl explicitly (no ambient env guessing).
#   - Extracts and validates the database name from the URL.
#   - Hard-blocks dangerous/runtime DB names; see $BLOCKED_DB_NAMES below.
#   - Prints the full planned action and exits 0 WITHOUT touching anything
#     unless -ConfirmReset is supplied.
#   - Never prints passwords.
#
# USAGE
#   .\scripts\reset-mqk-testdb.ps1 -Help
#   .\scripts\reset-mqk-testdb.ps1 -DatabaseUrl "postgres://mqk:mqk@127.0.0.1:5432/mqk_test"
#   .\scripts\reset-mqk-testdb.ps1 -DatabaseUrl "postgres://mqk:mqk@127.0.0.1:5432/mqk_test" -ConfirmReset
#
# SEE ALSO
#   docs/runbooks/db_migration_safety.md   — full proof DB reset workflow
#   scripts/db_proof_bootstrap.sh          — starts the proof Postgres container

[CmdletBinding()]
param(
    # Full Postgres URL of the proof database to reset.
    # The database name is extracted and validated before any action is taken.
    # Example: postgres://mqk:mqk@127.0.0.1:5432/mqk_test
    [string]$DatabaseUrl = '',

    # Required safety flag.  Without it the script prints the planned action
    # and exits 0 without touching anything (dry-run mode).
    [switch]$ConfirmReset,

    # Print usage and exit.
    [switch]$Help
)

$ErrorActionPreference = 'Stop'

# ---------------------------------------------------------------------------
# Safety: DB names that this script must never drop or recreate.
# Append to this list before adding new environment names.
# ---------------------------------------------------------------------------
$BLOCKED_DB_NAMES = @(
    'postgres',
    'template0',
    'template1',
    'mqk_dev',
    'mqk_live',
    'mqk_prod',
    'production',
    'live',
    'prod',
    'defaultdb'
)

function Show-Help {
    Write-Host @'
reset-mqk-testdb.ps1 — Local proof DB reset  (PROOF-DB-RESET-01)

PURPOSE
  Drops and recreates a local proof database so that DB-backed Cargo tests
  can run against a clean migration state.

  Resolves the sqlx error:
    "migration N was previously applied but has been modified"

  The correct fix is always to recreate the proof DB — never to weaken
  sqlx migration integrity or edit historical migration files.

USAGE
  .\scripts\reset-mqk-testdb.ps1 -DatabaseUrl <url>              (dry-run)
  .\scripts\reset-mqk-testdb.ps1 -DatabaseUrl <url> -ConfirmReset (execute)
  .\scripts\reset-mqk-testdb.ps1 -Help

PARAMETERS
  -DatabaseUrl <url>   Required.  Full Postgres connection URL.
                       The database name is extracted and validated.

  -ConfirmReset        Required to execute the drop/recreate.
                       Without it the script is a safe dry-run.

  -Help                Show this message and exit.

CANONICAL PROOF DB
  URL:       postgres://mqk:mqk@127.0.0.1:5432/mqk_test
  Container: mqk-postgres-proof  (started by scripts/db_proof_bootstrap.sh)
  Port:      5432 (proof container only — do NOT share with the dev DB)
  DB name:   mqk_test
  User:      mqk / mqk

  NOTE: 5432 is only free if nothing else on this machine already owns it.
  Some local setups run a persistent runtime/live/paper Postgres container
  on 5432 (or 5440 for a reality-test lane) that this script must never touch.
  Run `docker ps` and check the PORTS column before assuming 5432 is free;
  if it is not, use a different -DatabaseUrl port (e.g. 55432, per
  README_TECHNICAL.md's "isolated manual proof DB" example) instead of
  colliding with it. Also: if you just recreated a container on a host port
  it previously used, verify the password actually works end-to-end
  (`docker exec <container> psql -U <user> -c "select 1"`) before trusting
  it — a stale host-side port forward has been observed to make a correct
  password look like an authentication failure from outside the container.

BLOCKED DB NAMES (will never be dropped by this script)
  postgres, template0, template1, mqk_dev, mqk_live, mqk_prod,
  production, live, prod, defaultdb

EXAMPLES
  # Dry-run: show planned action, exit without touching DB
  .\scripts\reset-mqk-testdb.ps1 -DatabaseUrl "postgres://mqk:mqk@127.0.0.1:5432/mqk_test"

  # Execute reset:
  .\scripts\reset-mqk-testdb.ps1 -DatabaseUrl "postgres://mqk:mqk@127.0.0.1:5432/mqk_test" -ConfirmReset

  # Set MQK_DATABASE_URL after reset then run parked DB-backed tests:
  $env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:5432/mqk_test"
  cargo test --manifest-path .\core-rs\Cargo.toml -p mqk-daemon --test scenario_ctrl_arm_preflight_01 -- --test-threads=1

SEE ALSO
  docs/runbooks/db_migration_safety.md — full stale-checksum recovery workflow
'@
}

if ($Help) {
    Show-Help
    exit 0
}

if ([string]::IsNullOrWhiteSpace($DatabaseUrl)) {
    Write-Host 'ERROR: -DatabaseUrl is required.' -ForegroundColor Red
    Write-Host '  Run with -Help for usage and the canonical proof DB URL.' -ForegroundColor Yellow
    exit 1
}

# ---------------------------------------------------------------------------
# Parse the URL into components.
# Accepted formats:
#   postgres://user:pass@host:port/dbname
#   postgres://user:pass@host/dbname
#   postgresql://user:pass@host:port/dbname
# ---------------------------------------------------------------------------
$urlPattern = '^postgres(?:ql)?://([^:@/]+)(?::([^@/]*))?@([^:/]+)(?::(\d+))?/([^?/]+)'
if ($DatabaseUrl -notmatch $urlPattern) {
    Write-Host 'ERROR: Cannot parse -DatabaseUrl.' -ForegroundColor Red
    Write-Host "  Value:    $DatabaseUrl" -ForegroundColor Red
    Write-Host '  Expected: postgres://user:pass@host:port/dbname' -ForegroundColor Yellow
    Write-Host '  Example:  postgres://mqk:mqk@127.0.0.1:5432/mqk_test' -ForegroundColor Yellow
    exit 1
}
$pgUser   = $Matches[1]
$pgPass   = $Matches[2]   # never printed
$pgHost   = $Matches[3]
$pgPort   = if ($Matches[4]) { $Matches[4] } else { '5432' }
$dbName   = $Matches[5].Trim()

# ---------------------------------------------------------------------------
# Safety: block dangerous DB names.
# ---------------------------------------------------------------------------
foreach ($blocked in $BLOCKED_DB_NAMES) {
    if ($dbName.ToLowerInvariant() -eq $blocked.ToLowerInvariant()) {
        Write-Host ''
        Write-Host "SAFETY BLOCK: '$dbName' is on the protected list and must not be reset by this script." -ForegroundColor Red
        Write-Host ''
        Write-Host "  Blocked names: $($BLOCKED_DB_NAMES -join ', ')" -ForegroundColor Yellow
        Write-Host '  This script is for proof-only DBs only (e.g. mqk_test).' -ForegroundColor Yellow
        Write-Host '  See docs/runbooks/db_migration_safety.md.' -ForegroundColor Yellow
        Write-Host ''
        exit 1
    }
}

# Warn if the name does not look like a proof DB (does not block execution).
$looksLikeProofDb = (
    $dbName -like '*_test*' -or
    $dbName -like '*test_*' -or
    $dbName -like '*_proof*' -or
    $dbName -like '*proof_*'
)
if (-not $looksLikeProofDb) {
    Write-Host ''
    Write-Host "WARNING: DB name '$dbName' does not contain '_test' or '_proof'." -ForegroundColor Yellow
    Write-Host '  This script is designed for local proof DBs.  Proceed only if you' -ForegroundColor Yellow
    Write-Host '  are certain this is a disposable proof database.' -ForegroundColor Yellow
}

# ---------------------------------------------------------------------------
# Locate psql on PATH.
# ---------------------------------------------------------------------------
$pgBin = 'C:\Program Files\PostgreSQL\18\bin'
if ($env:Path -notlike "*$pgBin*") {
    $env:Path += ";$pgBin"
}

$psqlExe = (Get-Command psql -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -First 1)
if (-not $psqlExe) {
    Write-Host 'ERROR: psql not found on PATH.' -ForegroundColor Red
    Write-Host "  Searched: $pgBin and system PATH." -ForegroundColor Yellow
    Write-Host '  Ensure PostgreSQL client tools are installed.' -ForegroundColor Yellow
    exit 1
}

# ---------------------------------------------------------------------------
# Print planned action (always).
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host '============================================================' -ForegroundColor Cyan
Write-Host 'PROOF-DB-RESET-01 — Local proof DB reset' -ForegroundColor Cyan
Write-Host '============================================================' -ForegroundColor Cyan
Write-Host "  DB name:   $dbName" -ForegroundColor Yellow
Write-Host "  Host:      $pgHost" -ForegroundColor Yellow
Write-Host "  Port:      $pgPort" -ForegroundColor Yellow
Write-Host "  User:      $pgUser" -ForegroundColor Yellow
Write-Host '  Password:  <redacted>' -ForegroundColor Yellow
Write-Host "  psql:      $psqlExe" -ForegroundColor Yellow
Write-Host ''
Write-Host 'PLANNED ACTIONS:' -ForegroundColor Yellow
Write-Host "  1. Terminate all connections to '$dbName'." -ForegroundColor Yellow
Write-Host "  2. DROP DATABASE IF EXISTS $dbName;" -ForegroundColor Yellow
Write-Host "  3. CREATE DATABASE $dbName;" -ForegroundColor Yellow
Write-Host ''

if (-not $ConfirmReset) {
    Write-Host 'DRY-RUN: No changes made.' -ForegroundColor Green
    Write-Host '  Supply -ConfirmReset to execute the reset.' -ForegroundColor Green
    Write-Host ''
    exit 0
}

# ---------------------------------------------------------------------------
# Execute.  PGPASSWORD is set only for the duration of each psql call and
# removed immediately afterward.  It is never echoed.
# ---------------------------------------------------------------------------
function Invoke-ProofPsql {
    param([string]$Sql, [string]$Description)
    Write-Host "  Executing: $Description" -ForegroundColor Cyan
    $env:PGPASSWORD = $pgPass
    try {
        & $psqlExe -h $pgHost -p $pgPort -U $pgUser -d postgres -c $Sql
        $code = $LASTEXITCODE
    }
    finally {
        Remove-Item Env:PGPASSWORD -ErrorAction SilentlyContinue
    }
    if ($code -ne 0) {
        throw "psql failed (exit $code) for: $Description"
    }
}

Invoke-ProofPsql `
    -Sql "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = '$dbName' AND pid <> pg_backend_pid();" `
    -Description "Terminate connections to '$dbName'"

Invoke-ProofPsql `
    -Sql "DROP DATABASE IF EXISTS $dbName;" `
    -Description "DROP DATABASE IF EXISTS $dbName"

Invoke-ProofPsql `
    -Sql "CREATE DATABASE $dbName;" `
    -Description "CREATE DATABASE $dbName"

# ---------------------------------------------------------------------------
# Success — print next steps.
# ---------------------------------------------------------------------------
Write-Host ''
Write-Host "SUCCESS: '$dbName' recreated at ${pgHost}:${pgPort}." -ForegroundColor Green
Write-Host ''
Write-Host 'Next steps:' -ForegroundColor Cyan
Write-Host "  1. Set MQK_DATABASE_URL:" -ForegroundColor Cyan
Write-Host "       `$env:MQK_DATABASE_URL = '$DatabaseUrl'" -ForegroundColor White
Write-Host '  2. Run DB-backed proof lane:' -ForegroundColor Cyan
Write-Host '       bash scripts/db_proof_bootstrap.sh' -ForegroundColor White
Write-Host '     or full proof:' -ForegroundColor Cyan
Write-Host '       .\full_repo_proof.ps1 -ProofProfile full' -ForegroundColor White
Write-Host ''
Write-Host 'See docs/runbooks/db_migration_safety.md for the full recovery workflow.' -ForegroundColor Cyan
Write-Host ''
