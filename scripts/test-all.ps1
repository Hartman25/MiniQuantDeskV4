# DEPRECATED: test-all.ps1 -- hard-fail wrapper
#
# This script is retained for reference only. It does NOT run.
# It is not the canonical proof harness and must not be used for operator validation.
#
# Problems with the old script:
#   - Did not use --test-threads=1 (required for determinism)
#   - Skipped clippy (required gate)
#   - Skipped all guard scripts
#   - Did not produce a structured proof transcript
#
# REPLACEMENT COMMANDS:
#
#   powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile local
#   powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile local -LowMemory
#
# Running this script is an error. Use the canonical proof harness above.

Write-Host ""
Write-Host "ERROR: test-all.ps1 is DEPRECATED and must not be used for operator validation." -ForegroundColor Red
Write-Host ""
Write-Host "  This script omits clippy, guards, and --test-threads=1." -ForegroundColor Yellow
Write-Host "  It cannot establish repo truth." -ForegroundColor Yellow
Write-Host ""
Write-Host "  Use the canonical proof harness instead:" -ForegroundColor Yellow
Write-Host ""
Write-Host "    powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile local" -ForegroundColor Cyan
Write-Host "    powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile local -LowMemory" -ForegroundColor Cyan
Write-Host ""
exit 1
