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
retention. The manifest records a schema version, a file count, and each
file's relative path, byte count, and SHA-256 — never file contents,
environment variables, or credentials, and never the absolute path the
evidence happened to be created from (so byte-identical evidence produces a
byte-identical manifest regardless of where on disk it is stored).

Reparse points (symlinks/junctions) anywhere under `EvidenceRoot` are
unsupported and cause creation to refuse outright — this tool's exact-
evidence-set contract cannot be honored for content that may not be a plain
on-disk file.

## Verify a run directory (e.g. after a restore)

```powershell
powershell -ExecutionPolicy Bypass -File scripts\windows\Export-ResearchEvidenceManifest.ps1 `
    -EvidenceRoot "<restored run directory>" `
    -ManifestPath "<restored run directory>\manifest.json" `
    -Verify
```

Exit code `0` means every file the manifest declares exists at the exact
declared byte count and hash, **and** no undeclared extra file is present
under the evidence root (verification is strict). This proves the files on
disk match the supplied manifest — it does **not** prove the manifest itself
was never maliciously replaced; the manifest is a checksum sidecar, not a
self-authenticating signed artifact (see "What this does NOT do" below).

Exit code `1` means the supplied manifest should not be trusted against the
evidence set it was compared to. Causes include:

- the manifest is malformed, carries an unsupported `schema_version`, or is
  otherwise internally inconsistent (e.g. `file_count` does not match the
  number of declared file entries);
- evidence relative to the manifest was altered, is missing, or has an
  unexpected extra file;
- a reparse point (symlink/junction) is present under `EvidenceRoot` or
  along a declared file's path.

## What this does NOT do

- Does not invoke B2/restic, and is not a substitute for the offsite proof
  described in `docs/runbooks/recovery_backup_offsite_proof_truth.md`.
- Does not decide retention policy or prune anything.
- Does not infer economic or statistical meaning from the evidence — a
  passing hash proves the bytes are unchanged, not that the research
  conclusions they represent are correct.
- A passing verification proves the files on disk match the *supplied*
  manifest. It does **not** prove the manifest itself is the manifest an
  operator originally generated — a checksum sidecar is not self-
  authenticating. If an attacker can replace both the evidence and its
  manifest together, this tool cannot detect that on its own; it is not a
  signing/PKI mechanism and this patch does not add one.

See `tests/script_guards/test_export_research_evidence_manifest.ps1` for the
disposable-fixture proof (create/verify round trip; negative controls for
mutated content, deleted/added/renamed files, wrong byte count, malformed
manifest, path traversal, unsupported/missing `schema_version`, wrong/missing
`file_count`, and reparse points under `EvidenceRoot`; deterministic re-run;
cross-directory byte-identical manifest content; and no-BOM UTF-8 output).
