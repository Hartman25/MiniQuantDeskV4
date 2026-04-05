# SCRIPT-TRUTH-01: DEPRECATED
#
# This script targets a single legacy scenario test and does NOT reproduce the canonical
# DB-backed proof lane. It does not run migrations, does not cover the full DB proof
# matrix, and does not produce a structured proof transcript.
#
# For the canonical local DB-backed proof, use:
#
#   .\full_repo_proof.ps1 -ProofProfile full
#
# That runs the full db_proof_bootstrap.sh matrix (same as CI db-proof job) plus all
# non-DB proof lanes in sequence, with a structured JSON summary.
#
# This file is retained for historical reference only. Do not use for operator validation.

Write-Warning "DEPRECATED: test-db.ps1 is not the canonical DB proof path."
Write-Warning "Use: .\full_repo_proof.ps1 -ProofProfile full"

& "$PSScriptRoot\dev-shell.ps1"
cargo test -p mqk-db --test scenario_inbox_apply_atomic_recovery -- --nocapture
