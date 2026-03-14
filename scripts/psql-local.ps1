$env:PGHOST = [Environment]::GetEnvironmentVariable("PGHOST", "User")
$env:PGPORT = [Environment]::GetEnvironmentVariable("PGPORT", "User")
$env:PGUSER = [Environment]::GetEnvironmentVariable("PGUSER", "User")

$pgBin = "C:\Program Files\PostgreSQL\18\bin"
if ($env:Path -notlike "*$pgBin*") {
    $env:Path += ";$pgBin"
}

psql -h $env:PGHOST -p $env:PGPORT -U $env:PGUSER -d postgres
