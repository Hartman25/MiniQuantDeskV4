# =============================================================================
# CI-WINDOWS-GUARD-PARITY-01 -- check_unsafe_patterns.ps1 static guard tests
#
# Proves WGP01-WGP08: the PowerShell guard covers all Bash guard categories
# and correctly filters comment/allow-list exemptions.
#
# No daemon, no DB, no live calls. Pure pattern and text invariant checks.
# Temp fixture files are written to $env:TEMP and cleaned up on exit.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File tests\script_guards\test_check_unsafe_patterns_ps1.ps1
#
# Exit codes: 0 = all pass, 1 = one or more failures.
# =============================================================================

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$ScriptDir  = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot   = (Resolve-Path (Join-Path $ScriptDir '..\..')).Path.TrimEnd('\')
$GuardScript = Join-Path $RepoRoot 'scripts\guards\check_unsafe_patterns.ps1'

$Failures = 0

function Pass { param([string]$Id, [string]$Msg) Write-Host "  PASS  [$Id] $Msg" -ForegroundColor Green }
function Fail { param([string]$Id, [string]$Msg) Write-Host "  FAIL  [$Id] $Msg" -ForegroundColor Red ; $script:Failures++ }

Write-Host ""
Write-Host "=== check_unsafe_patterns.ps1 parity guard tests (CI-WINDOWS-GUARD-PARITY-01) ==="
Write-Host "    Guard: $GuardScript"
Write-Host ""

# ---------------------------------------------------------------------------
# Load guard script text for static content checks.
# ---------------------------------------------------------------------------
$guardText = ''
if (Test-Path $GuardScript) {
    $guardText = Get-Content -Path $GuardScript -Raw
    Pass 'WGP00' "Guard script exists at scripts\guards\check_unsafe_patterns.ps1"
} else {
    Fail 'WGP00' "Guard script NOT found at: $GuardScript"
}

# ---------------------------------------------------------------------------
# WGP01: [S] SystemTime::now is covered by the guard.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- WGP01: [S] SystemTime::now detection --"

if ($guardText -match [regex]::Escape('SystemTime::now')) {
    Pass 'WGP01a' "Guard script contains 'SystemTime::now' pattern"
} else {
    Fail 'WGP01a' "Guard script does NOT contain 'SystemTime::now' pattern"
}

if ($guardText -match '\[S\]') {
    Pass 'WGP01b' "Guard script labels [S] check in output"
} else {
    Fail 'WGP01b' "Guard script missing [S] label"
}

# ---------------------------------------------------------------------------
# WGP02: [M] timestamp_millis() is covered by the guard.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- WGP02: [M] timestamp_millis() detection --"

if ($guardText -match [regex]::Escape('timestamp_millis')) {
    Pass 'WGP02a' "Guard script contains 'timestamp_millis' pattern"
} else {
    Fail 'WGP02a' "Guard script does NOT contain 'timestamp_millis' pattern"
}

if ($guardText -match '\[M\]') {
    Pass 'WGP02b' "Guard script labels [M] check in output"
} else {
    Fail 'WGP02b' "Guard script missing [M] label"
}

# ---------------------------------------------------------------------------
# WGP03: [R] rand:: is covered by the guard.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- WGP03: [R] rand:: detection --"

if ($guardText -match [regex]::Escape('rand::')) {
    Pass 'WGP03a' "Guard script contains 'rand::' pattern"
} else {
    Fail 'WGP03a' "Guard script does NOT contain 'rand::' pattern"
}

if ($guardText -match '\[R\]') {
    Pass 'WGP03b' "Guard script labels [R] check in output"
} else {
    Fail 'WGP03b' "Guard script missing [R] label"
}

# ---------------------------------------------------------------------------
# WGP04: [Q] SQL now() in mqk-db/src/ is covered by the guard.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- WGP04: [Q] SQL now() detection --"

if ($guardText -match '\[Q\]') {
    Pass 'WGP04a' "Guard script labels [Q] check in output"
} else {
    Fail 'WGP04a' "Guard script missing [Q] label"
}

if ($guardText -match 'mqk-db') {
    Pass 'WGP04b' "Guard [Q] is scoped to mqk-db"
} else {
    Fail 'WGP04b' "Guard [Q] does not reference mqk-db scope"
}

if ($guardText -match [regex]::Escape("'now()'") -or $guardText -match [regex]::Escape('"now()"')) {
    Pass 'WGP04c' "Guard script contains literal now() pattern for [Q]"
} else {
    Fail 'WGP04c' "Guard script missing literal now() pattern for [Q]"
}

# ---------------------------------------------------------------------------
# WGP05: Existing [U/T/N] checks are still present and labeled.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- WGP05: Existing [U/T/N] checks still present --"

if ($guardText -match [regex]::Escape('Uuid::new_v4') -and $guardText -match '\[U\]') {
    Pass 'WGP05a' "[U] Uuid::new_v4 check present"
} else {
    Fail 'WGP05a' "[U] Uuid::new_v4 check missing or unlabeled"
}

if ($guardText -match [regex]::Escape('Utc::now()') -and $guardText -match '\[T\]') {
    Pass 'WGP05b' "[T] Utc::now() check present"
} else {
    Fail 'WGP05b' "[T] Utc::now() check missing or unlabeled"
}

if ($guardText -match [regex]::Escape('0012_') -and $guardText -match '\[N\]') {
    Pass 'WGP05c' "[N] DEFAULT now() migration guard (>= 0012) present"
} else {
    Fail 'WGP05c' "[N] migration guard missing or unlabeled"
}

if ($guardText -match 'CURRENT_TIMESTAMP') {
    Pass 'WGP05d' "[N] guard also checks CURRENT_TIMESTAMP (Bash parity)"
} else {
    Fail 'WGP05d' "[N] guard missing CURRENT_TIMESTAMP check (Bash parity gap)"
}

# ---------------------------------------------------------------------------
# WGP06: Allow-list / comment exclusion logic is present.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- WGP06: Exemption / allow-list filtering present --"

if ($guardText -match [regex]::Escape('// allow:')) {
    Pass 'WGP06a' "Guard filters '// allow:' annotated lines"
} else {
    Fail 'WGP06a' "Guard missing '// allow:' filter"
}

if ($guardText -match [regex]::Escape("'^\s*//'") -or $guardText -match [regex]::Escape('"^\s*//"')) {
    Pass 'WGP06b' "Guard filters pure Rust comment lines (^\s*//)"
} else {
    Fail 'WGP06b' "Guard missing pure-comment-line filter (^\s*//)"
}

if ($guardText -match [regex]::Escape('-- allow:')) {
    Pass 'WGP06c' "Guard filters '-- allow:' SQL annotations (used in [Q])"
} else {
    Fail 'WGP06c' "Guard missing '-- allow:' SQL annotation filter"
}

if ($guardText -match [regex]::Escape("'^\s*--'") -or $guardText -match [regex]::Escape('"^\s*--"')) {
    Pass 'WGP06d' "Guard filters SQL comment lines (^\s*--) in [N]"
} else {
    Fail 'WGP06d' "Guard missing SQL comment-line filter (^\s*--) in [N]"
}

# ---------------------------------------------------------------------------
# WGP07: Pattern detection works on fixture strings.
# Creates temp fixtures in $env:TEMP (outside the repo) and verifies that
# the exact same patterns used by the guard detect violations.
# Fixtures are cleaned up in the finally block.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- WGP07: Pattern detection on temp fixtures --"

$TempDir = Join-Path $env:TEMP "wgp_test_$([System.IO.Path]::GetRandomFileName().Replace('.',''))"
New-Item -ItemType Directory -Force -Path $TempDir | Out-Null

try {
    # --- WGP07a: SystemTime::now detected ---
    $f1 = Join-Path $TempDir 'fixture_sys.rs'
    Set-Content -Path $f1 -Value 'let t = SystemTime::now();'
    $r1 = Select-String -Path $f1 -Pattern 'SystemTime::now' -SimpleMatch |
        Where-Object { $_.Line -notmatch '^\s*//' -and $_.Line -notmatch '// allow:' }
    if ($r1) {
        Pass 'WGP07a' "SystemTime::now detected in fixture (no allow)"
    } else {
        Fail 'WGP07a' "SystemTime::now NOT detected in fixture"
    }

    # --- WGP07b: timestamp_millis() detected ---
    $f2 = Join-Path $TempDir 'fixture_ms.rs'
    Set-Content -Path $f2 -Value 'let ms = now.timestamp_millis();'
    $r2 = Select-String -Path $f2 -Pattern 'timestamp_millis' -SimpleMatch |
        Where-Object { $_.Line -notmatch '^\s*//' -and $_.Line -notmatch '// allow:' }
    if ($r2) {
        Pass 'WGP07b' "timestamp_millis() detected in fixture (no allow)"
    } else {
        Fail 'WGP07b' "timestamp_millis() NOT detected in fixture"
    }

    # --- WGP07c: rand:: detected ---
    $f3 = Join-Path $TempDir 'fixture_rand.rs'
    Set-Content -Path $f3 -Value 'use rand::Rng;'
    $r3 = Select-String -Path $f3 -Pattern 'rand::' -SimpleMatch |
        Where-Object { $_.Line -notmatch '^\s*//' -and $_.Line -notmatch '// allow:' }
    if ($r3) {
        Pass 'WGP07c' "rand:: detected in fixture (no allow)"
    } else {
        Fail 'WGP07c' "rand:: NOT detected in fixture"
    }

    # --- WGP07d: SQL now() detected ---
    $f4 = Join-Path $TempDir 'fixture_sqlnow.rs'
    Set-Content -Path $f4 -Value 'let q = "UPDATE t SET ts = now()";'
    $r4 = Select-String -Path $f4 -Pattern 'now()' -SimpleMatch |
        Where-Object {
            $_.Line -notmatch '^\s*//' -and
            $_.Line -notmatch '// allow:' -and
            $_.Line -notmatch '-- allow:'
        }
    if ($r4) {
        Pass 'WGP07d' "SQL now() detected in fixture (no allow)"
    } else {
        Fail 'WGP07d' "SQL now() NOT detected in fixture"
    }

    # --- WGP07e: Uuid::new_v4 detected ---
    $f5 = Join-Path $TempDir 'fixture_uuid.rs'
    Set-Content -Path $f5 -Value 'let id = Uuid::new_v4();'
    $r5 = Select-String -Path $f5 -Pattern 'Uuid::new_v4' -SimpleMatch |
        Where-Object { $_.Line -notmatch '^\s*//' -and $_.Line -notmatch '// allow:' }
    if ($r5) {
        Pass 'WGP07e' "Uuid::new_v4 detected in fixture (no allow)"
    } else {
        Fail 'WGP07e' "Uuid::new_v4 NOT detected in fixture"
    }

    # --- WGP07f: Pure comment line excluded (SystemTime::now in //) ---
    $f6 = Join-Path $TempDir 'fixture_comment.rs'
    Set-Content -Path $f6 -Value '// let t = SystemTime::now(); this is commented out'
    $r6 = Select-String -Path $f6 -Pattern 'SystemTime::now' -SimpleMatch |
        Where-Object { $_.Line -notmatch '^\s*//' -and $_.Line -notmatch '// allow:' }
    if (-not $r6) {
        Pass 'WGP07f' "SystemTime::now in pure // comment is correctly excluded"
    } else {
        Fail 'WGP07f' "SystemTime::now in pure // comment was NOT excluded (false positive)"
    }

    # --- WGP07g: // allow: annotation excluded ---
    $f7 = Join-Path $TempDir 'fixture_allow.rs'
    Set-Content -Path $f7 -Value 'let id = Uuid::new_v4(); // allow: ops-metadata'
    $r7 = Select-String -Path $f7 -Pattern 'Uuid::new_v4' -SimpleMatch |
        Where-Object { $_.Line -notmatch '^\s*//' -and $_.Line -notmatch '// allow:' }
    if (-not $r7) {
        Pass 'WGP07g' "Uuid::new_v4 with '// allow:' annotation is correctly excluded"
    } else {
        Fail 'WGP07g' "Uuid::new_v4 with '// allow:' annotation was NOT excluded (false positive)"
    }

    # --- WGP07h: SQL -- comment line excluded from [N] ---
    $f8 = Join-Path $TempDir 'fixture_sqlcomment.sql'
    Set-Content -Path $f8 -Value @"
-- created_at has no DEFAULT now() per determinism rules.
-- See: DEFAULT now() is forbidden in migrations >= 0012.
created_at TIMESTAMPTZ NOT NULL
"@
    $r8 = Select-String -Path $f8 -Pattern 'default now\(\)|CURRENT_TIMESTAMP' -AllMatches |
        Where-Object { $_.Line -notmatch '^\s*--' }
    if (-not $r8) {
        Pass 'WGP07h' "DEFAULT now() in SQL -- comment is correctly excluded from [N]"
    } else {
        Fail 'WGP07h' "DEFAULT now() in SQL -- comment was NOT excluded from [N] (false positive)"
    }

    # --- WGP07i: CURRENT_TIMESTAMP in SQL detected by [N] ---
    $f9 = Join-Path $TempDir 'fixture_curts.sql'
    Set-Content -Path $f9 -Value 'created_at TIMESTAMPTZ NOT NULL DEFAULT CURRENT_TIMESTAMP,'
    $r9 = Select-String -Path $f9 -Pattern 'default now\(\)|CURRENT_TIMESTAMP' -AllMatches |
        Where-Object { $_.Line -notmatch '^\s*--' }
    if ($r9) {
        Pass 'WGP07i' "CURRENT_TIMESTAMP detected in SQL fixture for [N]"
    } else {
        Fail 'WGP07i' "CURRENT_TIMESTAMP NOT detected in SQL fixture for [N]"
    }

    # --- WGP07j: -- allow: excluded from [Q] ---
    $f10 = Join-Path $TempDir 'fixture_sqlallow.rs'
    Set-Content -Path $f10 -Value '    "UPDATE t SET ts = now()", -- allow: ops-metadata'
    $r10 = Select-String -Path $f10 -Pattern 'now()' -SimpleMatch |
        Where-Object {
            $_.Line -notmatch '^\s*//' -and
            $_.Line -notmatch '// allow:' -and
            $_.Line -notmatch '-- allow:'
        }
    if (-not $r10) {
        Pass 'WGP07j' "SQL now() with '-- allow:' annotation is correctly excluded from [Q]"
    } else {
        Fail 'WGP07j' "SQL now() with '-- allow:' annotation was NOT excluded from [Q] (false positive)"
    }

} finally {
    Remove-Item -Recurse -Force -Path $TempDir -ErrorAction SilentlyContinue
}

# ---------------------------------------------------------------------------
# WGP08: Guard exits zero on the current clean repo.
# ---------------------------------------------------------------------------
Write-Host ""
Write-Host "  -- WGP08: Guard exits 0 on current clean repo --"

if (-not (Test-Path $GuardScript)) {
    Fail 'WGP08' "Cannot run guard -- script not found (see WGP00)"
} else {
    # Suppress guard output so test results are readable; only exit code matters.
    & powershell.exe -ExecutionPolicy Bypass -NonInteractive -File $GuardScript *>$null
    $guardExit = $LASTEXITCODE
    if ($guardExit -eq 0) {
        Pass 'WGP08' "Guard exits 0 on current repo (all patterns clean)"
    } else {
        Fail 'WGP08' "Guard exits $guardExit on current repo (expected 0 -- violations or false positives present)"
    }
}

# ---------------------------------------------------------------------------
# Summary
# ---------------------------------------------------------------------------
Write-Host ""
if ($Failures -eq 0) {
    Write-Host "=== ALL WGP INVARIANTS PASSED ===" -ForegroundColor Green
    exit 0
} else {
    Write-Host "=== $Failures INVARIANT(S) FAILED ===" -ForegroundColor Red
    exit 1
}
