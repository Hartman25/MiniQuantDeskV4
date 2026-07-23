# Supervised Autonomous Paper Session — Evidence Checklist

Short operator checklist for capturing evidence during a **supervised**
Paper + Alpaca autonomous session. This checklist governs evidence capture
only — it does not authorize or describe an unattended soak. The unattended
10–20-session soak has not started; this tooling only prepares for it.

See `docs/runbooks/autonomous_paper_ops.md` Part 2 (§15–§23) for the full
operator runbook this checklist is a companion to.

## Before you start

- [ ] `-OutputDirectory` points inside an ignored location (default:
      `smoke_logs\autonomous_paper_soak\<date>\<phase>`) — never a tracked
      repository path.
- [ ] Daemon is reachable at the local base URL you will pass
      (`-DaemonBaseUrl`, default `http://127.0.0.1:8899`).
- [ ] You are running this against the **operating paper database** (port
      `5432`), not the isolated test/reality-test databases.

## Capture points

Run one capture per point, each with the matching `-CapturePhase`:

- [ ] **`pre_session`** — before the session window opens, after the
      before-session checklist (runbook §17) has been completed.
- [ ] **`mid_session`** — at least once during the session, more often for a
      longer supervised session.
- [ ] **`post_session`** — after the session controller's clean stop, once
      the daily-operation record has reached `finalized` (or its current
      terminal-eligible state).
- [ ] **`incident`** — immediately whenever an unusual condition is
      observed (halt, evidence-degraded, reconcile mismatch, WS gap) —
      capture before taking any recovery action, per runbook §20's
      "evidence preservation before restart" guidance.
- [ ] **`restart`** — immediately after any daemon or GUI restart.

## Command

```powershell
powershell -ExecutionPolicy Bypass -File scripts\soak\capture_autonomous_paper_session_evidence.ps1 `
  -OutputDirectory "smoke_logs\autonomous_paper_soak\$(Get-Date -Format yyyy-MM-dd)\pre_session" `
  -CapturePhase pre_session
```

## After each capture

- [ ] Validate the manifest:
      ```powershell
      powershell -ExecutionPolicy Bypass -File scripts\soak\validate_autonomous_paper_session_evidence.ps1 `
        -ManifestPath <path-to-manifest.json>
      ```
- [ ] Confirm `deployment_mode` is `paper` (or null/unavailable — never
      anything else).
- [ ] Confirm `capture_errors` / `missing_endpoints` are reviewed — an
      unavailable surface is expected occasionally; a growing or unexplained
      list across captures is worth investigating.
- [ ] Do **not** stage or commit the generated manifest or its output
      directory. Generated evidence stays local and ignored.

## Never

- Never point `-DaemonBaseUrl` at anything other than a local daemon —
  the tool refuses non-local hosts by design, but do not attempt to
  override or route around that refusal.
- Never treat a single capture as proof of a completed soak session.
- Never manually edit a written manifest to change its truth values.
- Never commit generated evidence.
