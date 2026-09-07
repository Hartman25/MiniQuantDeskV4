# Deployment Decision

**DEPLOYMENT-DECISION-DOC-01.** This document records a decision, not a
runbook — see `scripts/windows/Launch-VeritasLedger.ps1`,
`scripts/windows/Start-MiniQuantDesk.ps1`, and `scripts/windows/Start-
PaperTradingSmoke.ps1` for actual operational launch procedures.

## Decision

MiniQuantDesk deploys as **local, directly-run native processes on a
single operator's Windows machine** — the `mqk-daemon` Rust binary, the
`mqk-cli` Rust binary, and the Tauri-shell GUI (`mqk-gui`) — launched and
supervised by the PowerShell scripts in `scripts/windows/`. There is no
Dockerfile, docker-compose file, Kubernetes manifest, or any other
container-packaging artifact for the application itself anywhere in this
repository, and none is planned. This is intentional, not an oversight.

## Rationale

- **Single-operator desktop application, not a multi-tenant service.**
  The system is designed around one operator running one daemon instance
  against one broker account, supervised interactively or via Windows Task
  Scheduler — not a fleet of horizontally-scaled containers behind a load
  balancer.
- **Windows-first, by design.** The canonical operator platform is
  Windows (`CI-PLATFORM-01`'s dedicated `windows` CI lane, the pinned
  `core-rs/rust-toolchain.toml`, and the entire `scripts/windows/*.ps1`
  launcher family). Container packaging would add an abstraction layer
  the actual deployment target doesn't need.
- **Direct process control is load-bearing for safety.** The deadman-file
  mechanism (`runtime/ARMED.flag`), local log capture (`smoke_logs/`), and
  the halt/kill-switch operational procedures all assume direct filesystem
  and process access on the host the daemon runs on. Containerizing the
  daemon would not change any of these mechanisms' correctness, but it
  would add an unnecessary indirection layer between the operator and the
  safety-critical file/process state during an incident.
- **No current multi-host or cloud-deployment requirement.** Nothing in
  the accepted architecture (see `MiniQuantDesk_Master_Patch_Ledger_v2_
  updated.md`) calls for running the daemon across multiple hosts,
  autoscaling, or cloud-native orchestration.

## The one exception: local Postgres runs via Docker

The Paper database (`mqk-paper-postgres`) and the local test database
(`mqk-test-postgres`) both run as local Docker containers — see the
`docker` references throughout `scripts/windows/*.ps1` (e.g.
`Launch-VeritasLedger.ps1`'s `Invoke-PaperDbPrerequisites`-style container
inspect/start logic). This is a **data-layer dependency**, not application
containerization: Docker here plays the same role a locally-installed
Postgres service would, chosen for reproducible version pinning and easy
teardown/reset during development — it does not run any MiniQuantDesk
Rust/TypeScript code. The application binaries themselves are never built
into, or run from, a container image.

## Boundaries

- This document records the current decision and its rationale. It does
  **not** authorize building a Dockerfile, docker-compose file, or any
  other container-packaging artifact for the application.
- If this decision is ever reversed (e.g. a future multi-host or
  cloud-deployment requirement), that is a separate, explicitly-requested,
  properly-scoped patch — not an incremental extension of this one.
- Nothing here changes how the local Postgres dependency containers
  (`mqk-paper-postgres`, `mqk-test-postgres`) are used; see the relevant
  `scripts/windows/*.ps1` scripts for that operational detail.
