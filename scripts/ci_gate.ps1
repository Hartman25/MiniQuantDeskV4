# DEPRECATED: ci_gate.ps1 -- hard-fail wrapper
#
# This script is retained for reference only. It does NOT run.
# It is not the canonical proof harness and must not be used for operator validation.
#
# Lanes omitted by the old script (relative to the canonical harness):
#   - daemon proof lanes (scenario_daemon_routes, scenario_gui_daemon_contract_gate, etc.)
#   - runtime proof lane
#   - Alpaca broker proof lane
#   - market data proof lane
#   - GUI typecheck + truth tests + build
#   - ignored-proof guard (scripts/guards/check_ignored_load_bearing_proofs.sh)
#   - structured JSON summary and VERDICT output
#
# REPLACEMENT COMMANDS:
#
#   powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile local
#   powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile full
#   powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile local -LowMemory
#
# Running this script is an error. Use the canonical proof harness above.

Write-Host ""
Write-Host "ERROR: ci_gate.ps1 is DEPRECATED and must not be used for operator validation." -ForegroundColor Red
Write-Host ""
Write-Host "  This script omits critical proof lanes and cannot establish repo truth." -ForegroundColor Yellow
Write-Host ""
Write-Host "  Use the canonical proof harness instead:" -ForegroundColor Yellow
Write-Host ""
Write-Host "    powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile local" -ForegroundColor Cyan
Write-Host "    powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile full" -ForegroundColor Cyan
Write-Host "    powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile local -LowMemory" -ForegroundColor Cyan
Write-Host ""
exit 1
