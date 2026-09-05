# Research Evidence Manifest — Runbook

## Purpose

`scripts/windows/Export-ResearchEvidenceManifest.ps1` hashes every file under
a retained research-run evidence directory (for example a
`research-py/experiments/<experiment>/runs/<run_id>` tree) into a small,
deterministic, content-addressed manifest — so a later restore (from local
disk, an archive, or an offsite copy) can be verified byte-for-byte instead
of trusted on sight. It does not upload anything, does not read credentials,
and does not touch `smoke_logs/` or any live/Paper runtime state.

This tool is independent of the whole-repo recovery lane
(`Backup-MiniQuantDeskRecovery.ps1` / `Restore-MiniQuantDeskRecovery.ps1` /
`Invoke-MiniQuantDeskOffsiteBackup.ps1`, see
`docs/runbooks/recovery_backup_offsite_proof_truth.md`) — it may be mentioned
alongside that restic/B2 chain in operator notes, but it never invokes
restic, never asks for or prints `RESTIC_PASSWORD`, and makes no offsite
claim on its own.

## Generate a manifest for a retained run directory

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Export-ResearchEvidenceManifest.ps1 `
    -EvidenceRoot "research-py\experiments\discovery_01_low_volatility_anomaly\runs\<run_id>" `
    -ManifestPath "research-py\experiments\discovery_01_low_volatility_anomaly\runs\<run_id>\manifest.json"
```

Store `manifest.json` alongside the run directory (or wherever your archive
process keeps sidecar metadata) before copying the run elsewhere for
retention. The manifest records each file's relative path, byte count, and
SHA-256 — never file contents, environment variables, or credentials.

## Verify a run directory (e.g. after a restore)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Export-ResearchEvidenceManifest.ps1 `
    -EvidenceRoot "<restored run directory>" `
    -ManifestPath "<restored run directory>\manifest.json" `
    -Verify
```

Exit code `0` means every file the manifest declares exists at the exact
declared byte count and hash, **and** no undeclared extra file is present
under the evidence root (verification is strict). Exit code `1` means the
evidence set should not be trusted — missing file, altered content, wrong
size, an unexpected extra file, or a malformed/tampered manifest.

## What this does NOT do

- Does not invoke B2/restic, and is not a substitute for the offsite proof
  described in `docs/runbooks/recovery_backup_offsite_proof_truth.md`.
- Does not decide retention policy or prune anything.
- Does not infer economic or statistical meaning from the evidence — a
  passing hash proves the bytes are unchanged, not that the research
  conclusions they represent are correct.

See `tests/script_guards/test_export_research_evidence_manifest.ps1` for the
disposable-fixture proof (create/verify round trip plus the negative
controls: mutated content, deleted/added/renamed files, wrong byte count,
malformed manifest, path traversal, and deterministic re-run).
