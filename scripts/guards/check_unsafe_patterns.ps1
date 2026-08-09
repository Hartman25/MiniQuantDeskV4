# =============================================================================
# P0-1: MiniQuantDesk V4 -- Deterministic Unsafe-Pattern Guard (PowerShell)
# =============================================================================
# Windows companion to check_unsafe_patterns.sh.
# Identical logic, identical exit codes, identical patterns.
# The .sh version runs in GitHub Actions CI (ubuntu-latest).
# This .ps1 version runs locally on Windows for pre-commit verification.
#
# Patterns ENFORCED (all under core-rs/crates/*/src/):
#
#   [U] Uuid::new_v4       -- RNG run/event identity (breaks determinism)
#   [T] Utc::now()         -- wall-clock in mqk-db/src/ (enforcement scope)
#   [S] SystemTime::now    -- system clock anywhere in production src/
#   [M] timestamp_millis() -- usually paired with Utc::now; flags temporal coupling
#   [R] rand::             -- any rand crate usage in production src/
#   [N] DEFAULT now()      -- semantics-bearing DB columns in migrations >= 0012
#   [Q] SQL now()          -- inline SQL now() strings within mqk-db/src/
#
# Exemption mechanism:
#   Lines containing "// allow:" are excluded from all [U/T/S/M/R/Q] checks.
#   Pure Rust comment lines (leading //) are excluded from [U/T/S/M/R/Q] checks.
#   SQL comment lines (starting with --) are excluded from [N] checks.
#   SQL "-- allow:" annotations are additionally excluded from [Q] checks.
#
#   Current allow-listed items (Rust // allow:):
#     mqk-daemon/src/routes/backtests.rs job_id    -- "// allow: process-local transient job identifier"
#     mqk-daemon/src/routes/ingest.rs    job_id(s) -- "// allow: process-local transient job identifier"
#     mqk-daemon/src/state.rs            heartbeat  -- "// allow: ops-metadata"
#     mqk-daemon/src/routes/repair.rs, mqk-daemon/src/state/ws_gap_recovery.rs
#       event_ts_ms (parsed from broker REST activity timestamp, used as an
#       economic-match-window discriminator, not a wall-clock read) --
#       "// allow: broker-sourced timestamp, not wall-clock"
#
#   Current allow-listed items (SQL -- allow:, used in [Q] guard):
#     mqk-db/src/lib.rs arm_run armed_at_utc       -- "-- allow: ops-metadata"
#     mqk-db/src/lib.rs begin_run running_at_utc   -- "-- allow: ops-metadata"
#     mqk-db/src/lib.rs stop_run stopped_at_utc    -- "-- allow: ops-metadata"
#     mqk-db/src/lib.rs persist_arm_state upd_at   -- "-- allow: ops-metadata"
#
# Parity note (CI-WINDOWS-GUARD-PARITY-01):
#   This script covers the same pattern set as check_unsafe_patterns.sh.
#   One intentional divergence from the Bash guard: this script additionally
#   excludes files under \tests\ subdirectories of src/ (more conservative).
#   With proper // allow: annotations in place, the violation count is identical.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_unsafe_patterns.ps1
#   # or from PowerShell directly:
#   & .\scripts\guards\check_unsafe_patterns.ps1
#
# Exit codes: 0 = clean, 1 = violations found.
# =============================================================================

$ErrorActionPreference = "Stop"

# Resolve repo root (two levels up from this script's directory).
$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

$Violations = 0

function Show-Red   { param([string]$Msg) Write-Host $Msg -ForegroundColor Red    }
function Show-Green { param([string]$Msg) Write-Host $Msg -ForegroundColor Green  }
function Show-Info  { param([string]$Msg) Write-Host $Msg -ForegroundColor Cyan   }

# =============================================================================
# Helper: Find pattern in a Rust source file, excluding:
#   - Pure Rust comment lines (lines beginning with optional whitespace + "//")
#   - Allow-listed lines (lines containing "// allow:")
# Mirrors the check_rs_pattern() function in check_unsafe_patterns.sh.
# =============================================================================
function Find-RsPattern {
    param([string]$Pattern, [string]$FilePath)
    Select-String -Path $FilePath -Pattern $Pattern -SimpleMatch |
        Where-Object { $_.Line -notmatch '^\s*//' -and $_.Line -notmatch '// allow:' }
}

Write-Host "============================================================"
Write-Host " MQK P0-1 Safety Guard (PowerShell)"
Write-Host " Repo root: $RepoRoot"
Write-Host "============================================================"

# =============================================================================
# Pre-compute file sets (reused across multiple checks).
# =============================================================================

# All .rs files under crates/*/src/ (not tests/ subdirs, not target/).
$SrcFiles = Get-ChildItem -Path "$RepoRoot\core-rs\crates" -Recurse -Filter "*.rs" |
    Where-Object {
        $_.FullName -match '\\src\\' -and
        $_.FullName -notmatch '\\tests\\' -and
        $_.FullName -notmatch '\\target\\'
    }

# .rs files in mqk-db/src/ only (enforcement scope for [T] and [Q]).
$MqkDbSrc = "$RepoRoot\core-rs\crates\mqk-db\src"
$DbSrcFiles = @()
if (Test-Path $MqkDbSrc) {
    $DbSrcFiles = Get-ChildItem -Path $MqkDbSrc -Recurse -Filter "*.rs" |
        Where-Object { $_.FullName -notmatch '\\target\\' }
}

# SQL migration files >= 0012 (enforcement scope for [N]).
$MigDir = "$RepoRoot\core-rs\crates\mqk-db\migrations"
$SqlMigFiles = @()
if (Test-Path $MigDir) {
    $SqlMigFiles = Get-ChildItem -Path $MigDir -Filter "*.sql" |
        Where-Object { $_.Name -ge "0012_" }
}

# =============================================================================
# [U] Uuid::new_v4 in production src/ (all crates)
# Pattern catches both: Uuid::new_v4() direct call and
#                       unwrap_or_else(Uuid::new_v4) function-pointer form.
# =============================================================================

$UuidPattern = 'Uuid::new_v4'

Write-Host ""
Show-Info "--- [U] $UuidPattern in production src/ ---"

$UuidFileCount  = 0
$UuidMatchLines = @()

foreach ($File in $SrcFiles) {
    $Found = Find-RsPattern -Pattern $UuidPattern -FilePath $File.FullName
    if ($Found) {
        $UuidFileCount++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) {
            $UuidMatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim())"
        }
    }
}

if ($UuidFileCount -eq 0) {
    Show-Green "  OK -- no $UuidPattern in production src/"
} else {
    $Violations += $UuidFileCount
    Show-Red "  FAIL -- $UuidPattern found in $UuidFileCount production file(s):"
    $UuidMatchLines | ForEach-Object { Write-Host $_ }
    Show-Red "  Remediation: D1-1 (run IDs), D1-2 (audit event IDs)."
}

# =============================================================================
# [T] Utc::now() in mqk-db/src/ (enforcement scope)
#
# mqk-db/src/ contains deadman_expired() and enforce_deadman_or_halt() which
# gate capital execution. Wall-clock time here breaks determinism.
# The sole permitted call is WallClock::now_utc() marked "// allow: wall-clock-canonical".
# =============================================================================

$UtcPattern = 'Utc::now()'

Write-Host ""
Show-Info "--- [T] $UtcPattern in mqk-db/src/ (enforcement scope) ---"

$UtcFileCount  = 0
$UtcMatchLines = @()

foreach ($File in $DbSrcFiles) {
    $Found = Find-RsPattern -Pattern $UtcPattern -FilePath $File.FullName
    if ($Found) {
        $UtcFileCount++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) {
            $UtcMatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim())"
        }
    }
}

if ($UtcFileCount -eq 0) {
    Show-Green "  OK -- no $UtcPattern in mqk-db/src/"
} else {
    $Violations += $UtcFileCount
    Show-Red "  FAIL -- $UtcPattern found in $UtcFileCount file(s) in mqk-db/src/:"
    $UtcMatchLines | ForEach-Object { Write-Host $_ }
    Show-Red "  Remediation: D1-3 (inject TimeSource into enforcement path)."
}

# =============================================================================
# [S] SystemTime::now in production src/ (all crates)
#
# std::time::SystemTime::now() is a wall-clock read with platform-specific
# behavior (monotonicity not guaranteed, affected by NTP). Use injected
# TimeSource instead for any path that affects gating or determinism.
# =============================================================================

Write-Host ""
Show-Info "--- [S] SystemTime::now in production src/ ---"

$SysFileCount  = 0
$SysMatchLines = @()

foreach ($File in $SrcFiles) {
    $Found = Find-RsPattern -Pattern 'SystemTime::now' -FilePath $File.FullName
    if ($Found) {
        $SysFileCount++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) {
            $SysMatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim())"
        }
    }
}

if ($SysFileCount -eq 0) {
    Show-Green "  OK -- no SystemTime::now in production src/"
} else {
    $Violations += $SysFileCount
    Show-Red "  FAIL -- SystemTime::now found in $SysFileCount file(s):"
    $SysMatchLines | ForEach-Object { Write-Host $_ }
    Show-Red "  Remediation: replace with injected TimeSource."
}

# =============================================================================
# [M] timestamp_millis() in production src/ (all crates)
#
# .timestamp_millis() is typically called on Utc::now() or similar, creating
# a wall-clock dependency. Legitimate ops-metadata uses should be annotated
# "// allow: ops-metadata" to make the intent explicit and suppress this check.
# =============================================================================

Write-Host ""
Show-Info "--- [M] timestamp_millis() in production src/ ---"

$MsFileCount  = 0
$MsMatchLines = @()

foreach ($File in $SrcFiles) {
    $Found = Find-RsPattern -Pattern 'timestamp_millis' -FilePath $File.FullName
    if ($Found) {
        $MsFileCount++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) {
            $MsMatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim())"
        }
    }
}

if ($MsFileCount -eq 0) {
    Show-Green "  OK -- no ungated timestamp_millis() in production src/"
} else {
    $Violations += $MsFileCount
    Show-Red "  FAIL -- timestamp_millis() found in $MsFileCount file(s):"
    $MsMatchLines | ForEach-Object { Write-Host $_ }
    Show-Red "  Remediation: remove wall-clock coupling or annotate '// allow: ops-metadata'."
}

# =============================================================================
# [R] rand:: in production src/ (all crates)
#
# The rand crate must not be used in production execution paths. All IDs and
# ordering must be deterministic and derived from inputs.
# =============================================================================

Write-Host ""
Show-Info "--- [R] rand:: in production src/ ---"

$RandFileCount  = 0
$RandMatchLines = @()

foreach ($File in $SrcFiles) {
    $Found = Find-RsPattern -Pattern 'rand::' -FilePath $File.FullName
    if ($Found) {
        $RandFileCount++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) {
            $RandMatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim())"
        }
    }
}

if ($RandFileCount -eq 0) {
    Show-Green "  OK -- no rand:: in production src/"
} else {
    $Violations += $RandFileCount
    Show-Red "  FAIL -- rand:: found in $RandFileCount file(s):"
    $RandMatchLines | ForEach-Object { Write-Host $_ }
    Show-Red "  Remediation: replace with deterministic derivation."
}

# =============================================================================
# [N] DEFAULT now() / CURRENT_TIMESTAMP in SQL migrations >= 0012
#
# Migrations 0001-0011 use DEFAULT now() for bookkeeping columns that are NOT
# in any enforcement or capital-decision path. These are the D1-4 legacy
# whitelist -- SQLx checksum immutability forbids retroactive changes.
#
# All migrations numbered >= 0012 must NOT use DEFAULT now() on any column.
# Semantics-bearing timestamps must be injected by the caller.
#
# SQL comment lines (starting with --) are excluded from this check.
# =============================================================================

Write-Host ""
Show-Info "--- [N] DEFAULT now() / CURRENT_TIMESTAMP in new migration files (>= 0012) ---"

$SqlMigFileCount = 0

foreach ($File in $SqlMigFiles) {
    $Found = Select-String -Path $File.FullName `
        -Pattern 'default now\(\)|CURRENT_TIMESTAMP' -AllMatches |
        Where-Object { $_.Line -notmatch '^\s*--' }
    if ($Found) {
        $SqlMigFileCount++
        $Violations++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        Show-Red "  FAIL: $RelPath"
        $Found | ForEach-Object { Write-Host "  $($_.Line.Trim())" }
    }
}

if ($SqlMigFileCount -eq 0) {
    Show-Green "  OK -- no DEFAULT now() or CURRENT_TIMESTAMP in post-D1-4 migrations (>= 0012)"
} else {
    Show-Red "  Remediation: Remove DEFAULT now() from new migration; inject timestamp via now: DateTime<Utc> parameter."
}

# =============================================================================
# [Q] SQL now() in inline SQL strings within mqk-db/src/
#
# sqlx::query strings containing now() are equivalent to DEFAULT now() in a
# migration: the DB server supplies a non-deterministic wall-clock timestamp.
# Any column written by the enforcement or capital-decision path must receive
# an injected caller timestamp, not a DB-side now().
#
# Exemptions:
#   Annotate the SQL line with a trailing SQL comment "-- allow: ops-metadata"
#   for columns that are pure bookkeeping (lifecycle timestamps, UI metadata)
#   and are NOT read by any enforcement or capital-decision path.
#   A trailing "// allow:" Rust annotation also suppresses the check.
#   Pure Rust comment lines (starting with //) are also excluded.
# =============================================================================

Write-Host ""
Show-Info "--- [Q] SQL now() in inline SQL strings within mqk-db/src/ ---"

$SqlNowFileCount  = 0
$SqlNowMatchLines = @()

foreach ($File in $DbSrcFiles) {
    $Found = Select-String -Path $File.FullName -Pattern 'now()' -SimpleMatch |
        Where-Object {
            $_.Line -notmatch '^\s*//' -and
            $_.Line -notmatch '// allow:' -and
            $_.Line -notmatch '-- allow:'
        }
    if ($Found) {
        $SqlNowFileCount++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) {
            $SqlNowMatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim())"
        }
    }
}

if ($SqlNowFileCount -eq 0) {
    Show-Green "  OK -- no unannotated SQL now() in mqk-db/src/"
} else {
    $Violations += $SqlNowFileCount
    Show-Red "  FAIL -- SQL now() found in $SqlNowFileCount file(s) in mqk-db/src/:"
    $SqlNowMatchLines | ForEach-Object { Write-Host $_ }
    Show-Red "  Remediation: inject timestamp as caller parameter, or annotate with"
    Show-Red "  '-- allow: ops-metadata' if the column is pure bookkeeping metadata."
}

# =============================================================================
# Summary
# =============================================================================

Write-Host ""
Write-Host "============================================================"
Write-Host " Summary"
Write-Host "============================================================"

if ($Violations -eq 0) {
    Show-Green " ALL GUARDS PASSED -- no forbidden patterns detected."
    exit 0
} else {
    Show-Red " GUARD FAILED -- $Violations violation(s) found."
    Write-Host ""
    Show-Red " Fix each flagged location or annotate with '// allow: <reason>'."
    Show-Red " Allowed exemptions:"
    Show-Red "   '// allow: wall-clock-canonical'  -- WallClock::now_utc() in mqk-db"
    Show-Red "   '// allow: ops-metadata'          -- non-enforcement UI/heartbeat paths"
    Show-Red "   '// allow: process-local transient job identifier'  -- ephemeral job IDs"
    exit 1
}
