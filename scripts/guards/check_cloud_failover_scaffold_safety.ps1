# =============================================================================
# OPS-CLOUD-FAILOVER-PAPER-SCAFFOLD-01: scaffold-only safety guard
# =============================================================================
#
# This is a SCAFFOLD (future connection contract), not an implementation of
# cloud failover. This guard fails the build if any of that scaffold's
# committed artifacts (tools/ops/cloud_failover_scaffold/*) ever drift away
# from the safety invariants the scaffold was defined with:
#   - the example config ships with enabled=false;
#   - the schema fixes deployment_mode to "paper" and live_capability to
#     false (no value that could ever mean Live);
#   - the validator module never imports a networking/subprocess/DB
#     capability, and never references a daemon operator route, Alpaca, or
#     the official launcher.
#
# This guard is static-only: it never runs validate_scaffold.py, never
# parses the example JSON beyond a literal substring check, and makes no
# network/process/DB call itself.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_cloud_failover_scaffold_safety.ps1
#
# Exit codes: 0 = clean, 1 = violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')
$ScaffoldDir = Join-Path $RepoRoot 'tools\ops\cloud_failover_scaffold'

Write-Host "============================================================"
Write-Host " MQK Cloud-Failover-Scaffold Safety Guard"
Write-Host " Repo root: $RepoRoot"
Write-Host "============================================================"

$Violations = 0
function Fail { param([string]$M) Write-Host "FAIL: $M" -ForegroundColor Red; $script:Violations++ }
function Ok   { param([string]$M) Write-Host "OK: $M"   -ForegroundColor Green }

$ConfigExample = Join-Path $ScaffoldDir 'config.example.json'
$SchemaFile    = Join-Path $ScaffoldDir 'schema.json'
$ValidatorFile = Join-Path $ScaffoldDir 'validate_scaffold.py'

foreach ($p in @($ConfigExample, $SchemaFile, $ValidatorFile)) {
    if (-not (Test-Path -LiteralPath $p)) {
        Fail "Required scaffold file missing: $p"
    }
}
if ($Violations -gt 0) {
    Write-Host ""
    Write-Host "$Violations violation(s) found." -ForegroundColor Red
    exit 1
}

$configText = Get-Content -Path $ConfigExample -Raw
if ($configText -match '"enabled"\s*:\s*false') {
    Ok 'config.example.json ships with enabled=false'
} else {
    Fail 'config.example.json does not ship with a literal "enabled": false'
}
if ($configText -match '"live_capability_requested"\s*:\s*false') {
    Ok 'config.example.json ships with live_capability_requested=false'
} else {
    Fail 'config.example.json does not ship with live_capability_requested=false'
}

$schemaText = Get-Content -Path $SchemaFile -Raw
if ($schemaText -match '"deployment_mode"[\s\S]{0,120}?"const"\s*:\s*"paper"') {
    Ok 'schema.json fixes deployment_mode to the const "paper"'
} else {
    Fail 'schema.json does not fix deployment_mode to a const "paper"'
}
if ($schemaText -match '"live_capability"[\s\S]{0,120}?"const"\s*:\s*false') {
    Ok 'schema.json fixes live_capability to the const false'
} else {
    Fail 'schema.json does not fix live_capability to a const false'
}

$validatorText = Get-Content -Path $ValidatorFile -Raw
$forbiddenTokens = @(
    'import socket', 'import requests', 'import urllib', 'import subprocess',
    'os.system(', 'os.popen(', 'psycopg2', 'sqlalchemy',
    '/api/v1/ops/action', 'Start-MiniQuantDesk', 'alpaca'
)
$hitAny = $false
foreach ($tok in $forbiddenTokens) {
    if ($validatorText -match [regex]::Escape($tok)) {
        Fail "validate_scaffold.py contains forbidden token: $tok"
        $hitAny = $true
    }
}
if (-not $hitAny) {
    Ok 'validate_scaffold.py contains no networking/subprocess/DB/broker/daemon-route token'
}

if ($validatorText -match 'MUST NEVER') {
    Ok 'validate_scaffold.py documents its own MUST NEVER fence'
} else {
    Fail 'validate_scaffold.py is missing its documented MUST NEVER fence'
}

Write-Host ""
if ($Violations -eq 0) {
    Write-Host "All proofs held. 0 violations." -ForegroundColor Green
    exit 0
} else {
    Write-Host "$Violations violation(s) found." -ForegroundColor Red
    exit 1
}
