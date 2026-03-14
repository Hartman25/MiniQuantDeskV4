$env:PGHOST = [Environment]::GetEnvironmentVariable("PGHOST", "User")
$env:PGPORT = [Environment]::GetEnvironmentVariable("PGPORT", "User")
$env:PGUSER = [Environment]::GetEnvironmentVariable("PGUSER", "User")
$env:MQK_DATABASE_URL = [Environment]::GetEnvironmentVariable("MQK_DATABASE_URL", "User")
$env:DATABASE_URL = [Environment]::GetEnvironmentVariable("DATABASE_URL", "User")

$pgBin = "C:\Program Files\PostgreSQL\18\bin"
if ($env:Path -notlike "*$pgBin*") {
    $env:Path += ";$pgBin"
}

Set-Location "$PSScriptRoot\..\core-rs"

Write-Host "MQK shell ready"
Write-Host "PGHOST=$env:PGHOST"
Write-Host "PGPORT=$env:PGPORT"
Write-Host "PGUSER=$env:PGUSER"
Write-Host "MQK_DATABASE_URL=$env:MQK_DATABASE_URL"
