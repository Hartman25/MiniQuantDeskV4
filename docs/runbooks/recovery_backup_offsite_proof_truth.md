# Recovery / Offsite Backup — Current Proof Truth

**Recorded by:** `MASTER-LEDGER-CURRENT-TRUTH-CLOSURE-01` (L1), 2026-08-30
**Baseline:** `main` @ `70ed507acfe02ef860b8378b9e5eddb25a36065d`
**Purpose:** Distinguish CODE/CONFIG EXISTS from RESTORE ACTUALLY PROVEN for
disaster-recovery/offsite-backup capability, per CLAUDE.md's operator-truth
discipline. This document does not invoke Backblaze, run restic against
remote storage, print secret-bearing configuration, or modify recovery
storage — it records what current repo evidence already shows.

## Architecture (decided)

Per `scripts/windows/Backup-MiniQuantDeskRecovery.ps1`'s own header:
backup engine = restic, offsite destination = Backblaze B2 (`OPS-OFFSITE-BACKUP-01`).

## What exists (CODE/CONFIG EXISTS)

Three committed scripts implement the full local-stage → offsite → restore
chain:

1. **`scripts/windows/Backup-MiniQuantDeskRecovery.ps1`** — stages a local
   recovery-backup set: exact source/Git identity, a *logical* `pg_dump` of
   the Paper Postgres DB (never a physical data-directory copy), safe
   non-secret config/manifests, and an optional research registry/evidence
   copy — plus a content-addressed (SHA-256 per file) manifest. Never
   uploads anywhere; never reads/writes B2 or restic credentials.
2. **`scripts/windows/Invoke-MiniQuantDeskOffsiteBackup.ps1`** (`D-R2`) — the
   real, encrypted, offsite half: stages a fresh set, `restic init`s the B2
   repo if needed, `restic backup`s it as one encrypted snapshot, confirms via
   `restic snapshots`/`restic check`, runs a non-destructive retention
   dry-run (never `--prune`), then `restic restore`s that exact snapshot into
   a disposable local directory and re-runs the full restore-verification
   path against the **restic-restored** content (not the original staging
   dir) — this is the step that actually proves the round trip through B2.
   Never installs restic, never guesses B2 bucket/endpoint/region, never
   generates/prints a restic repository password.
3. **`scripts/windows/Test-MiniQuantDeskRecoveryBackup.ps1`** — end-to-end
   proof for the local backup/restore pair only: a real `git bundle`, a real
   read-only `pg_dump` against the Paper DB, and a real restore into a
   disposable database on the local `127.0.0.1:5434` disposable-Postgres
   instance, plus static/adversarial proofs of the secret-exclusion and
   unsafe-target-refusal logic. Explicitly never touches B2/restic
   credentials; treats a missing restic install as a truthful `PARTIAL`,
   not a failure.

## What is actually proven (RESTORE ACTUALLY PROVEN — local only)

`docs/audits/2026-08-24_branch_worktree_consolidation_audit.md` records a
direct reproduction of the local restore path: run under Windows PowerShell
5.1 against two independent worktrees, with `restic 0.19.1` and
`docker 29.5.3` present and the disposable test-Postgres container already
running — **"All proofs held. 0 violations." exit 0**, identical on both
worktrees. This is a genuine, reproduced, local functional proof: real
`pg_dump`, real restore, real disposable-DB verification.

## What is NOT proven (outstanding)

The same audit entry states explicitly that "real B2 credentials are a
separate, explicitly-deferred operational proof" — distinct from the local
functional acceptance gate above. No session recorded in this repository's
committed history has run `Invoke-MiniQuantDeskOffsiteBackup.ps1` against a
real Backblaze B2 bucket with real credentials and confirmed a genuine
restic-restored-from-B2 round trip. The historical cause of an earlier
"11 failures" report on this same test chain is recorded as
`UNKNOWN_NEEDS_PROOF` (not reproduced under current prerequisites, and the
original failing transcript was not preserved) — it is not evidence either
way about real-B2 proof status.

**Status: RESTORE-FROM-REAL-OFFSITE-B2 — OUTSTANDING.** This is a real
remaining operational-proof gap, not a code defect: the script that would
prove it is committed and designed correctly (per code review of its
documented invariants above), but has no recorded successful run against
real remote storage.

## What this document does not do

It does not run `Invoke-MiniQuantDeskOffsiteBackup.ps1`, does not request or
handle B2/restic credentials, and does not change the outstanding-proof
status above. Closing the outstanding item requires an operator-authorized
session with real B2 credentials available, explicitly scoped to that one
proof.
