# =============================================================================
# DYNAMIC-STRATEGY-SYMBOL-SELECTION-01-PHASE-7A-FINAL-PRIVATE-PRODUCTION-
# EFFECTS-PROOF (R5): no default-build production-reachable Phase 7A
# ProductionRuntimeStartEffects driver or ownership-mutation bypass.
# =============================================================================
#
# Before this patch, `mqk-daemon` exposed several `pub` (non-cfg-gated)
# `_for_test` seams that could drive the real `ProductionRuntimeStartEffects`
# directly, mutate Phase 7A fault seams, or plant active
# metadata/economic-mirror state -- all reachable from any downstream crate
# or external `tests/*.rs` integration test in a normal default-feature
# build, with no compiler or feature boundary enforcing "test-only":
#
#   - drive_production_start_effects_for_test
#   - set_dynamic_selection_fault_seam_for_test
#   - commit_dynamic_selection_runtime_state_for_test
#   - plant_accepted_artifact_for_test
#   - plant_day_signal_count_for_test
#   - accepted_artifact_snapshot_for_test
#   - day_signal_count_snapshot_for_test
#
# Each is now `pub(crate)` + `#[cfg(test)]` -- reachable only from this
# crate's own test build, never from production code or an external
# integration test. This guard fails if any of them ever regress back to a
# plain `pub fn` / `pub async fn` (the exact shape of the bypass), and fails
# if the removed direct-bypass driver name ever reappears as a `pub` method.
#
# This guard is a source-level regression check; `cargo check -p mqk-daemon
# --lib` (default features) succeeding is corroborating evidence, but is NOT
# by itself proof these symbols are `#[cfg(test)]`-gated -- a `pub(crate) fn`
# with no `#[cfg(test)]` attribute compiles cleanly in a default build too,
# while still being reachable from any non-test code path inside this same
# crate (main.rs, routes/*, other non-test modules) -- exactly the kind of
# quiet production-reachable regression this guard exists to catch.
# PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01 therefore adds a second,
# independent check per symbol: its own definition must carry a `#[cfg(test)]`
# attribute on the line(s) immediately preceding it, not merely avoid the
# `pub fn` shape.
#
# Usage:
#   powershell -ExecutionPolicy Bypass -File scripts\guards\check_no_phase7a_production_effects_bypass.ps1
#
# Exit codes: 0 = clean, 1 = violation found.
# =============================================================================

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Definition
$RepoRoot  = (Resolve-Path (Join-Path $ScriptDir "../../")).Path.TrimEnd('\')

Write-Host "============================================================"
Write-Host " MQK Phase 7A Production-Effects Bypass Guard"
Write-Host " Repo root: $RepoRoot"
Write-Host "============================================================"

$SrcFiles = Get-ChildItem -Path "$RepoRoot\core-rs\crates\mqk-daemon\src" -Recurse -Filter "*.rs" |
    Where-Object { $_.FullName -notmatch '\\target\\' }

$BannedSymbols = @(
    'drive_production_start_effects_for_test',
    'set_dynamic_selection_fault_seam_for_test',
    'commit_dynamic_selection_runtime_state_for_test',
    'plant_accepted_artifact_for_test',
    'plant_day_signal_count_for_test',
    'accepted_artifact_snapshot_for_test',
    'day_signal_count_snapshot_for_test',
    # FULL-AUDIT-FAIL-016/018: the hermetic broker-override setter. Must stay
    # `pub(crate)` + `#[cfg(test)]` -- the sole seam that can ever flip
    # `hermetic_test_broker_override` to true, and unreachable outside this
    # crate's own test build.
    'set_hermetic_test_broker_override_for_test'
)

# A plain `pub fn NAME` / `pub async fn NAME` is the bypass shape. Note this
# deliberately does NOT match `pub(crate) fn NAME` / `pub(crate) async fn
# NAME` -- there is no whitespace between `pub` and `(crate)`, so the regex
# below only matches genuinely crate-external-reachable `pub`.
$Violations = 0
$MatchLines = @()

foreach ($Symbol in $BannedSymbols) {
    $Pattern = "pub\s+(async\s+)?fn\s+$Symbol\b"
    foreach ($File in $SrcFiles) {
        $Found = Select-String -Path $File.FullName -Pattern $Pattern
        if ($Found) {
            $Violations++
            $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
            foreach ($Hit in $Found) {
                $MatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim())"
            }
        }
    }
}

# ---------------------------------------------------------------------------
# PHASE-7A-R6-EXHAUSTIVE-MATRIX-CLOSURE-REPAIR-01: each symbol's own
# definition must actually carry `#[cfg(test)]` on the line(s) immediately
# preceding its `fn` -- not merely avoid the plain-`pub` shape checked above.
# `cargo check --lib` succeeding proves the symbol *compiles*, not that it is
# gated out of a default (non-test) build.
# ---------------------------------------------------------------------------
$UngatedSymbols = @()

foreach ($Symbol in $BannedSymbols) {
    $FoundDefinition = $false
    foreach ($File in $SrcFiles) {
        $Lines = Get-Content -Path $File.FullName
        for ($i = 0; $i -lt $Lines.Count; $i++) {
            if ($Lines[$i] -match "(?:pub(?:\(crate\))?\s+)?(?:async\s+)?fn\s+$Symbol\b") {
                $FoundDefinition = $true
                # Walk upward over blank lines, doc comments, and other
                # attributes to find whether #[cfg(test)] is among the
                # attributes immediately attached to this fn -- OR, if an
                # enclosing `impl ... { ... }` block wraps it directly (no
                # intervening fn/impl/mod between this fn and that impl),
                # whether #[cfg(test)] is attached to THAT impl block
                # instead (a whole-impl-block gate covers every fn inside
                # it just as effectively as a per-fn gate).
                $j = $i - 1
                $HasCfgTest = $false
                while ($j -ge 0) {
                    $Line = $Lines[$j].Trim()
                    if ($Line -eq "" -or $Line.StartsWith("///") -or $Line.StartsWith("//!")) {
                        $j--
                        continue
                    }
                    if ($Line.StartsWith("#[cfg(test)]")) {
                        $HasCfgTest = $true
                        $j--
                        continue
                    }
                    if ($Line.StartsWith("#[") -or $Line.StartsWith("//")) {
                        $j--
                        continue
                    }
                    if ($Line -match "^impl\b") {
                        # Check the attribute(s) immediately above this impl.
                        $k = $j - 1
                        while ($k -ge 0) {
                            $ImplAboveLine = $Lines[$k].Trim()
                            if ($ImplAboveLine -eq "" -or $ImplAboveLine.StartsWith("///") -or $ImplAboveLine.StartsWith("//!") -or $ImplAboveLine.StartsWith("//")) {
                                $k--
                                continue
                            }
                            if ($ImplAboveLine.StartsWith("#[cfg(test)]")) {
                                $HasCfgTest = $true
                            }
                            break
                        }
                        break
                    }
                    break
                }
                if (-not $HasCfgTest) {
                    $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
                    $UngatedSymbols += "  ${RelPath}:$($i + 1): fn $Symbol is not #[cfg(test)]-gated (neither directly nor via an enclosing #[cfg(test)] impl block)"
                }
            }
        }
    }
    if (-not $FoundDefinition) {
        $UngatedSymbols += "  (symbol '$Symbol' definition not found anywhere in src/ -- update this guard's BannedSymbols list if it was intentionally removed)"
    }
}

if ($UngatedSymbols.Count -gt 0) {
    $Violations += $UngatedSymbols.Count
    $MatchLines += $UngatedSymbols
}

# ---------------------------------------------------------------------------
# FULL-AUDIT-FAIL-016/018: hermetic broker-override no-production-bypass
# proof. `hermetic_test_broker_override_enabled` (the reader) is deliberately
# NOT `#[cfg(test)]`-gated -- production code reads it unconditionally and it
# is always `false` there -- so it is intentionally excluded from
# $BannedSymbols above (which would wrongly flag it as "ungated"). It must
# instead stay `pub(crate)`, never plain `pub`, so no crate outside
# mqk-daemon (and no route module reaching across the module tree) can read
# or be handed a way to flip it.
# ---------------------------------------------------------------------------
$ReaderSymbol = 'hermetic_test_broker_override_enabled'
$ReaderPlainPubPattern = "pub\s+(async\s+)?fn\s+$ReaderSymbol\b"
foreach ($File in $SrcFiles) {
    $Found = Select-String -Path $File.FullName -Pattern $ReaderPlainPubPattern
    if ($Found) {
        $Violations++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) {
            $MatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim()) [reader must be pub(crate), not pub]"
        }
    }
}

# No env var, route, or config key may exist for this override -- the private
# `hermetic_test_broker_override: Arc<RwLock<bool>>` field on AppState is not
# `pub`, so nothing outside `state.rs` itself (routes/*, main.rs, other
# crates) can write it directly; the only writer is the `#[cfg(test)]`
# setter checked above. This is a tripwire in case a future change adds an
# env-var-driven path that calls the setter from non-test code. Matches only
# an actual `std::env::var("...HERMETIC..." )`-shaped string-literal read, not
# the legitimate Rust identifier `hermetic_test_broker_override` itself.
$EnvBypassPattern = 'env::var\(\s*"[^"]*(HERMETIC|BROKER_OVERRIDE)[^"]*"'
foreach ($File in $SrcFiles) {
    $Found = Select-String -Path $File.FullName -Pattern $EnvBypassPattern
    if ($Found) {
        $Violations++
        $RelPath = $File.FullName.Substring($RepoRoot.Length + 1)
        foreach ($Hit in $Found) {
            $MatchLines += "  ${RelPath}:$($Hit.LineNumber):$($Hit.Line.Trim()) [no env var may gate the hermetic broker override]"
        }
    }
}

# Tripwire: the default-build credential-required proofs this guard relies on
# (informally) must still exist -- if either is ever deleted, this guard's
# "default build still requires real credentials" claim in its own header
# would silently stop being proven anywhere.
$RequiredCredentialProofs = @(
    'build_daemon_broker_paper_is_not_execution_path',
    'env_truth_02_alpaca_live_base_url_env_var_is_not_authoritative'
)
foreach ($ProofSymbol in $RequiredCredentialProofs) {
    $ProofFound = $false
    foreach ($File in $SrcFiles) {
        if (Select-String -Path $File.FullName -Pattern "fn\s+$ProofSymbol\b" -Quiet) {
            $ProofFound = $true
            break
        }
    }
    if (-not $ProofFound) {
        $Violations++
        $MatchLines += "  required credential-gate proof test '$ProofSymbol' not found anywhere in src/ (was it deleted or renamed?)"
    }
}

Write-Host ""
if ($Violations -eq 0) {
    Write-Host " OK -- no default-build production-reachable Phase 7A effects-driver or ownership-mutation bypass," -ForegroundColor Green
    Write-Host "       and every named seam is genuinely #[cfg(test)]-gated" -ForegroundColor Green
    exit 0
} else {
    Write-Host " FAIL -- production-reachable (non-cfg, plain `pub`) Phase 7A bypass symbol(s) found:" -ForegroundColor Red
    $MatchLines | ForEach-Object { Write-Host $_ }
    Write-Host ""
    Write-Host " Remediation: these seams must be `pub(crate)` and `#[cfg(test)]` -- reachable" -ForegroundColor Red
    Write-Host " only from this crate's own test build. Move any external-test consumer of" -ForegroundColor Red
    Write-Host " them into a private in-crate `#[cfg(test)]` module instead of widening" -ForegroundColor Red
    Write-Host " visibility back to `pub`." -ForegroundColor Red
    exit 1
}
