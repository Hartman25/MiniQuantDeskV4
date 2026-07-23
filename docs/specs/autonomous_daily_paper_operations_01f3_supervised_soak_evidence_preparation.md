# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F3 — Supervised Soak-Evidence Preparation

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F3-SUPERVISED-SOAK-EVIDENCE-PREPARATION`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Phase: Phase F3 — supervised soak-evidence preparation.

Starting HEAD: `8494e1ea` (`docs: correct autonomous paper operations
runbook` — the F2 commit).

Status: **IMPLEMENTATION COMPLETE — AWAITING FINAL COMBINED ACCEPTANCE.**
This document records what F3 built; it is not itself an acceptance record,
and it does not close Phase F, Phase G, or Bundle 3.

## 0. Accepted foundation (recorded, not re-litigated)

```text
D1-D4: ACCEPTED - COMPLETE
PHASE D: ACCEPTED - COMPLETE

E1-E5: ACCEPTED - COMPLETE
PHASE E: ACCEPTED - COMPLETE

F1: ACCEPTED - COMPLETE
F2: IMPLEMENTATION COMPLETE - AWAITING FINAL COMBINED ACCEPTANCE

F3: IMPLEMENTATION COMPLETE - AWAITING FINAL COMBINED ACCEPTANCE
PHASE F: OPEN
PHASE G: NOT STARTED
BUNDLE 3: OPEN
BUNDLE 4: NOT STARTED
```

## 1. Scope

F3 prepares read-only evidence-capture tooling and templates for **future**
supervised Paper + Alpaca sessions. It prepares evidence collection only. It
does not perform, start, count, or claim an unattended soak session. **No
daemon, trading, broker, strategy, risk, or execution behavior change is
made by this patch.**

## 2. Source audit

Reviewed before building new tooling, per the mission's "reuse existing
conventions" instruction:

- `scripts/windows/Capture-PaperSmokeEvidence.ps1` — the closest existing
  PowerShell convention: `[CmdletBinding()]` param block, repo-root
  resolution, a fail-soft `Invoke-DaemonGet`/`Save-DaemonJson` GET-only
  helper pattern (never prints credentials, never mutates, catches errors to
  an `UNAVAILABLE:` marker instead of throwing). F3's capture script reuses
  this exact GET-only/fail-soft helper convention rather than inventing a
  new one.
- `scripts/paper_soak_day.sh` — the existing bash one-day soak harness
  (`soak_manifest.json`, `schema_version="soak-v1"`, pre-open/intraday/
  end-of-day snapshot phases). F3's PowerShell tooling is a **distinct,
  narrower** evidence-manifest tool scoped to the daily-operation lifecycle
  vocabulary (Phases D-F1) that `paper_soak_day.sh` predates; it does not
  replace or modify that script.
- `.gitignore` — confirmed no existing rule covered a soak-evidence output
  path; added a narrow rule for the new default output location only (see
  §6).
- `core-rs/crates/mqk-daemon/src/routes.rs` — confirmed every route the
  capture script calls is a real, registered `GET` route before referencing
  it (`/api/v1/system/status`, `/api/v1/system/preflight`,
  `/api/v1/autonomous/readiness`, `/api/v1/autonomous/paper-status`,
  `/api/v1/autonomous/daily-operation`, `/api/v1/autonomous/daily-operations`,
  `/api/v1/execution/orders` [`GET`, distinct from the same path's `POST`
  order-submit route — never referenced], `/api/v1/portfolio/fills`,
  `/api/v1/reconcile/status`, `/api/v1/risk/summary`, `/api/v1/alerts/active`).
- `core-rs/crates/mqk-daemon/src/api_types.rs` — confirmed `daemon_mode`,
  `adapter_id`, and `live_routing_enabled` field names before referencing
  them in the manifest-building logic.

## 3. Deliverables

- `docs/specs/autonomous_daily_paper_operations_01f3_supervised_soak_evidence_preparation.md`
  (this document).
- `scripts/guards/validate_autonomous_daily_paper_operations_01f3_supervised_soak_evidence_preparation.ps1`
  (see §7).
- `scripts/soak/capture_autonomous_paper_session_evidence.ps1` — read-only,
  GET-only capture tool (see §4).
- `scripts/soak/validate_autonomous_paper_session_evidence.ps1` — manifest
  validator (see §5).
- `scripts/soak/templates/autonomous_paper_session_manifest.template.json`
  — the frozen schema template (`schema_version:
  "autonomous-paper-soak-evidence-v1"`).
- `scripts/soak/supervised_session_evidence_checklist.md` — short
  human-readable operator checklist.
- `.gitignore` — one narrow new rule, `smoke_logs/autonomous_paper_soak/`
  (the tool's default output location).

## 4. Capture script safety

`capture_autonomous_paper_session_evidence.ps1`:

- **GET only.** Every daemon call goes through one `Invoke-DaemonGetOnly`
  helper that always passes `-Method Get`; the script contains no reference
  to `Post`/`Put`/`Patch`/`Delete` anywhere.
- **Local daemon only.** Before doing anything else, the script parses
  `-DaemonBaseUrl` and refuses to proceed (`exit 1`) unless the host is
  `127.0.0.1`, `localhost`, or `::1` — a structural, fail-closed guarantee
  against ever contacting a non-local host, not just a documentation
  promise.
- **Never Alpaca, never Discord, no other external network call.** The
  script contains no Alpaca or Discord URL, and its only network calls are
  the local-daemon-only GETs above.
- **No order submission, no runtime start/stop, no arm/disarm/flatten/
  finalize.** Every route called is an existing read-only `GET` surface
  (§2); the script contains no reference to `/v1/run/start`, `/v1/run/stop`,
  `/v1/run/halt`, `/api/v1/ops/action`, `/v1/integrity/arm`, or any other
  mutating route.
- **Never reads or copies `.env.local`.** Not referenced anywhere in the
  script.
- **Never prints a credential.** The script never reads `ALPACA_API_KEY*`,
  `ALPACA_API_SECRET*`, `MQK_OPERATOR_TOKEN`, or `MQK_DATABASE_URL` — none of
  the routes it calls require operator-token authentication (all are
  existing unauthenticated read-only GET routes), so no credential is ever
  needed or handled.
- **Explicit output directory required, writes only inside it.** `
  -OutputDirectory` is a mandatory parameter; the only `Set-Content`/
  `New-Item` calls in the script target paths built from it.
- **Safe modes:** `-ValidateOnly` performs zero daemon calls and writes zero
  files (verified: §8); `-FixturePath` reads canned local JSON fixture files
  instead of a real daemon, for exercising the manifest-building logic in
  tests without any live daemon or network access.

## 5. Manifest schema and validator

Schema version `autonomous-paper-soak-evidence-v1`
(`scripts/soak/templates/autonomous_paper_session_manifest.template.json`).
Fields exactly match the mission's required list: identity fields
(`schema_version`, `session_evidence_id`, `capture_phase`,
`captured_at_utc`, `market_date`, `repository_commit`, `deployment_mode`,
`adapter_id`, `daemon_base_url`, `operator_supervised`); the eleven
daemon-sourced truth fields (`system_status` through
`completed_bar_task_status`) plus `gui_build_version` (sourced from a local
`package.json` read, not the daemon); and `capture_errors`,
`missing_endpoints`, `operator_notes`, `artifact_hashes`.

`completed_bar_task_status` has no dedicated daemon route today — it is
populated verbatim from `GET /api/v1/alerts/active` (the closest existing
read-only fault-visibility surface; see the runbook's own §19 guidance to
check "system/status / alerts for the specific adapter fault"), never
synthesized.

`capture_phases` supported: `pre_session`, `mid_session`, `post_session`,
`incident`, `restart` — the same five the mission requires. One capture is
one point-in-time snapshot; the tool never represents a single capture as a
completed soak session (the capture script's own closing output line states
this explicitly).

`validate_autonomous_paper_session_evidence.ps1` proves: valid JSON; a known
`schema_version`; every required field key present; `capture_phase` is one
of the five valid values; `deployment_mode` is `paper` when known (fails
closed on any other non-null value); `live_routing_enabled` is `false` when
observable; `repository_commit` is present; `truth_state` values on every
truth-state-bearing surface are members of the closed known vocabulary
(`active`/`not_found`/`backend_unavailable`/`query_failed`/
`invalid_request`/`no_db`) — never collapsed or an ad-hoc string; the three
full-run-lineage count fields on `current_daily_operation.operation` are
either `null` or numeric, never coerced; a manifest-wide text scan rejects
ten secret-shaped patterns (`ALPACA_API_KEY`, `ALPACA_API_SECRET`,
`MQK_OPERATOR_TOKEN`, `MQK_DATABASE_URL`, `DISCORD_WEBHOOK`, `Bearer `,
`password`, `api_secret`, `.env.local`, plus `ALPACA_SECRET`); and each of
the eleven daemon-sourced fields is either present or explicitly listed in
`missing_endpoints` — a silently-absent field with no explanation is a
validation failure, not a pass.

Verified empirically (manual test run, not committed as generated
evidence): a fixture-mode capture against synthetic local JSON fixtures
produces a manifest that the validator accepts; mutating that manifest to
`deployment_mode: "live"` is rejected; injecting an `ALPACA_API_KEY`-shaped
string into `operator_notes` is rejected; nulling a field without also
removing it from `missing_endpoints` is rejected.

## 6. Default output location

`scripts/soak/supervised_session_evidence_checklist.md` recommends
`smoke_logs\autonomous_paper_soak\<date>\<phase>` as the default
`-OutputDirectory`. `.gitignore` gained one narrow rule,
`smoke_logs/autonomous_paper_soak/`, so generated manifests default to an
ignored location. No generated evidence is staged or committed by this
patch — the manual verification in §5 was performed against a temp
directory outside the repository working tree, never written into the repo.

## 7. F3 guard

`validate_autonomous_daily_paper_operations_01f3_supervised_soak_evidence_preparation.ps1`
performs pure text/source validation only — no network call, no DB
connection, no daemon start. It proves, by source-text scan:

1. The capture script contains `-Method Get` calls and no
   `Post`/`Put`/`Patch`/`Delete` method reference.
2. No Alpaca or Discord URL/host string appears anywhere in the capture
   script.
3. No order-submission, start/stop/halt, arm/disarm, flatten, or
   finalization route string appears in the capture script.
4. `.env.local` is never referenced in the capture script.
5. The capture script's fail-closed local-host check exists (refuses any
   host other than `127.0.0.1`/`localhost`/`::1`).
6. The validator script rejects a non-`paper` `deployment_mode` and a
   `live_routing_enabled: true` value (source-level assertion presence).
7. The validator script contains the secret-pattern scan.
8. The validator script's null-count check exists and does not coerce
   `null` to a numeric value.
9. `.gitignore` contains the `smoke_logs/autonomous_paper_soak/` rule.
10. No generated evidence file is staged or present in the committed F3
    range or the current working tree.
11. The spec, guard, capture script, validator script, template, and
    checklist all exist and are nonempty.
12. README/ledger record correct F3 status and never overclaim Phase G,
    Bundle 3 closure, soak-started, or live-capital-ready.
13. No production Rust, migration, or GUI production file is touched.

## 8. Validation performed

```text
F3 guard: PASS
F2 guard: PASS
F1 guard: PASS
Phase E closure guard (transitively re-invokes E1-E4): PASS
check_unsafe_patterns.ps1: PASS

npm test (core-rs/mqk-gui): 850/850 PASS (unchanged — F3 touches no GUI file)
npm run build (core-rs/mqk-gui): PASS (unchanged)

scenario_autonomous_daily_phase_e_closure_01: PASS (unchanged — no daemon change)
scenario_autonomous_daily_operation_api_01: 50/50 PASS (unchanged)
scenario_gui_daemon_contract_gate: 23/23 PASS (unchanged)
scenario_daemon_routes: 84/84 PASS (unchanged)
```

Manual capture/validator round-trip testing (§5) was performed against a
temp directory outside the repository, using `-ValidateOnly` and
`-FixturePath` only — no real daemon was contacted, no external service was
called.

## 9. Documentation status after F3

```text
F1: ACCEPTED - COMPLETE
F2: IMPLEMENTATION COMPLETE - AWAITING FINAL COMBINED ACCEPTANCE
F3: IMPLEMENTATION COMPLETE - AWAITING FINAL COMBINED ACCEPTANCE
PHASE F: IMPLEMENTATION COMPLETE - AWAITING FINAL COMBINED ACCEPTANCE

PHASE G: NOT STARTED
BUNDLE 3: OPEN

UNATTENDED SOAK: NOT STARTED
LIVE CAPITAL: NOT READY
```

## 10. Phase G boundary

Phase G (final Bundle 3 closure audit) is not started by this patch. Bundle
3 remains open.

## 11. Soak / live-capital boundaries

The unattended 10-20-session paper soak has not started and is not
authorized by this patch — this patch prepares evidence tooling only. Live
trading is not ready and is not authorized by this patch.
