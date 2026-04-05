# SCRIPT-TRUTH-01: DEPRECATED
#
# This gate script predates full_repo_proof.ps1 and does not cover the full required
# lane set. Specifically, it omits:
#   - the daemon proof lanes (scenario_daemon_routes, scenario_gui_daemon_contract_gate, etc.)
#   - the runtime proof lane
#   - the Alpaca broker proof lane
#   - the market data proof lane
#   - GUI typecheck + truth tests + build
#   - the ignored-proof guard (scripts/guards/check_ignored_load_bearing_proofs.sh)
#   - structured JSON summary and VERDICT output
#
# Use the canonical proof harness instead:
#
#   .\full_repo_proof.ps1 -ProofProfile local              # non-DB local proof
#   .\full_repo_proof.ps1 -ProofProfile full               # full DB-backed institutional proof
#   .\full_repo_proof.ps1 -ProofProfile local -LowMemory   # memory-sensitive Windows
#
# This file is retained for historical reference only. Do not use for operator validation.

Write-Warning "DEPRECATED: ci_gate.ps1 does not cover the full canonical proof lane set."
Write-Warning "Use: .\full_repo_proof.ps1 -ProofProfile local"
Write-Warning "     .\full_repo_proof.ps1 -ProofProfile full  (DB-backed institutional proof)"

Write-Host "== Format check =="
cargo fmt --all -- --check
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== Clippy strict =="
cargo clippy --workspace --all-targets -- -D warnings
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== Workspace tests =="
cargo test --workspace --all-targets --no-fail-fast
if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== DB integration tests =="

if (-not $env:MQK_DATABASE_URL) {
    $env:MQK_DATABASE_URL = "postgres://postgres:postgres@localhost/mqk_test"
}

cargo test -p mqk-db `
  --features testkit `
  -- `
  --include-ignored `
  --test-threads=1 `
  --nocapture

if ($LASTEXITCODE -ne 0) { exit $LASTEXITCODE }

Write-Host "== CI GATE PASSED =="
