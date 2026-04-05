# SCRIPT-TRUTH-01: DEPRECATED
#
# This script is a stale ad-hoc wrapper that does NOT reproduce the canonical local
# proof posture. It omits --test-threads=1, skips clippy, skips guards, and does not
# produce a structured proof transcript.
#
# Use the canonical proof harness instead:
#
#   .\full_repo_proof.ps1 -ProofProfile local
#   .\full_repo_proof.ps1 -ProofProfile local -LowMemory   # memory-sensitive Windows
#   .\full_repo_proof.ps1 -ProofProfile full                # full DB-backed institutional proof
#
# This file is retained for historical reference only. Do not use for operator validation.

Write-Warning "DEPRECATED: test-all.ps1 is not the canonical proof harness."
Write-Warning "Use: .\full_repo_proof.ps1 -ProofProfile local"
Write-Warning "     .\full_repo_proof.ps1 -ProofProfile local -LowMemory  (memory-sensitive Windows)"

& "$PSScriptRoot\dev-shell.ps1"
cargo test
