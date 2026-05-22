# DEPRECATED: test-db.ps1 -- hard-fail wrapper
#
# This script is retained for reference only. It does NOT run.
# It is not the canonical DB proof path and must not be used for operator validation.
#
# Problems with the old script:
#   - Targeted a single legacy scenario test only
#   - Did not run migrations from scratch
#   - Did not cover the full DB proof matrix
#   - Did not produce a structured proof transcript
#
# REPLACEMENT COMMANDS:
#
#   Full DB-backed institutional proof:
#     powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile full
#
#   DB bootstrap lane only:
#     bash scripts/db_proof_bootstrap.sh
#
# Running this script is an error. Use the canonical proof harness above.

Write-Host ""
Write-Host "ERROR: test-db.ps1 is DEPRECATED and must not be used for operator validation." -ForegroundColor Red
Write-Host ""
Write-Host "  This script targets a single legacy test and cannot establish DB proof truth." -ForegroundColor Yellow
Write-Host ""
Write-Host "  Use the canonical DB proof harness instead:" -ForegroundColor Yellow
Write-Host ""
Write-Host "    powershell -ExecutionPolicy Bypass -File .\full_repo_proof.ps1 -ProofProfile full" -ForegroundColor Cyan
Write-Host "    bash scripts/db_proof_bootstrap.sh" -ForegroundColor Cyan
Write-Host ""
exit 1
