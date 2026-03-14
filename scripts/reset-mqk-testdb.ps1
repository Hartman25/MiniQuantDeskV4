$ErrorActionPreference = "Stop"

$env:PGHOST = [Environment]::GetEnvironmentVariable("PGHOST", "User")
$env:PGPORT = [Environment]::GetEnvironmentVariable("PGPORT", "User")
$env:PGUSER = [Environment]::GetEnvironmentVariable("PGUSER", "User")

$pgBin = "C:\Program Files\PostgreSQL\18\bin"
if ($env:Path -notlike "*$pgBin*") {
    $env:Path += ";$pgBin"
}

psql -h $env:PGHOST -p $env:PGPORT -U $env:PGUSER -d postgres -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname = 'mqk_test' AND pid <> pg_backend_pid();"
psql -h $env:PGHOST -p $env:PGPORT -U $env:PGUSER -d postgres -c "DROP DATABASE IF EXISTS mqk_test;"
psql -h $env:PGHOST -p $env:PGPORT -U $env:PGUSER -d postgres -c "CREATE DATABASE mqk_test;"

Write-Host "mqk_test recreated on $env:PGHOST`:$env:PGPORT"
