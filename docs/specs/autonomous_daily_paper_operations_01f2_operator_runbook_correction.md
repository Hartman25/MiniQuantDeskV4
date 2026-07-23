# AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F2 — Operator Runbook Correction

Patch ID: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01F2-OPERATOR-RUNBOOK-CORRECTION`
Bundle: `AUTONOMOUS-DAILY-PAPER-OPERATIONS-01-COMBINED`
Phase: Phase F2 — operator runbook correction.

Starting HEAD: `bd7336d4dd14dbb1943638b152886eb40b646b7d` (`fix: harden
autonomous daily operation GUI truth` — the accepted F1 head).

Status: **IMPLEMENTATION COMPLETE — AWAITING FINAL COMBINED ACCEPTANCE.**
This document records what F2 built; it is not itself an acceptance record,
and it does not close Phase F, Phase G, or Bundle 3.

## 0. Accepted foundation (recorded, not re-litigated)

```text
D1-D4: ACCEPTED - COMPLETE
PHASE D: ACCEPTED - COMPLETE

E1-E5: ACCEPTED - COMPLETE
PHASE E: ACCEPTED - COMPLETE

F1: ACCEPTED - COMPLETE

F2: IMPLEMENTATION COMPLETE - AWAITING FINAL COMBINED ACCEPTANCE
F3: NOT STARTED
PHASE F: OPEN
PHASE G: NOT STARTED
BUNDLE 3: OPEN
BUNDLE 4: NOT STARTED
```

## 1. Scope

F2 is documentation and validation only. It creates or corrects the
canonical operator runbook for the supported supervised autonomous lane
(Paper + Alpaca, single-symbol, long-only US equity/ETF, active operator
supervision), and adds a source-aware guard that proves the runbook's
content. **No production Rust, GUI, or migration file is touched by this
patch.**

## 2. Source audit

The existing canonical autonomous-paper runbook was located at
`docs/runbooks/autonomous_paper_ops.md` (AUTON-OPS-01). It already covered
the **session-controller** layer (arm/start/stop of the execution runtime,
session-window behavior, WS gap/recovery, halt recovery) but predated
`AUTONOMOUS-DAILY-PAPER-OPERATIONS-01` (Phases D-F1) and contained no
coverage of the durable **daily-operation lifecycle** record: its
finalization/outcome/evidence vocabulary, the five read-only routes that
project it (`autonomous/readiness`, `autonomous/paper-status`,
`system/preflight`, `autonomous/daily-operation`,
`autonomous/daily-operations`), or operator recovery/evidence procedures
specific to it.

Per the mission's "do not create a competing duplicate runbook" instruction,
this existing file was **updated in place** rather than replaced. Source
audit inputs, cross-checked against currently committed code and docs (never
invented):

- `docs/specs/autonomous_daily_paper_operations_01e4_read_only_daily_operation_api.md`
  (E4) — the five-route contract, truth-state vocabulary, finalization-status
  vocabulary, evidence-state/blocker semantics.
- `docs/specs/autonomous_daily_paper_operations_01f1_gui_daily_operation_projection.md`
  (F1) — the GUI's own consumption of the same vocabulary (used to keep the
  runbook's language consistent with what the operator actually sees).
- `docs/runbooks/operator_control_surface.md` — flatten procedure and
  blocker table, kill-switch/emergency-abort procedure, existing PowerShell
  preflight tooling (`Invoke-PaperPremarketValidation.ps1`,
  `Launch-VeritasLedger.ps1 -CheckOnly`).
- `docs/runbooks/operator_workflows.md` — `/v1/run/start`, `/v1/run/stop`,
  `/v1/run/halt`, `kill_switch_active` vocabulary.
- `README.md` / `README_TECHNICAL.md` — daemon start command (`cargo run
  --manifest-path .\core-rs\Cargo.toml -p mqk-daemon`, default bind
  `127.0.0.1:8899`), GUI start command (`npm run dev` in `core-rs\mqk-gui`),
  Docker/Postgres port map (operating dev DB `5432`, manual proof DB
  `55432`, isolated `cargo test` DB `5434`, `autonomous_reality_test_paper.ps1.ps1`'s
  own reality-test DB `5440` — explicitly a different, non-operating lane),
  `mqk-cli db status` / `db migrate` commands.
- `core-rs/crates/mqk-daemon/src/routes.rs` — confirmed the exact route path
  strings referenced by the runbook (`/api/v1/portfolio/positions`,
  `/api/v1/reconcile/status`, `/api/v1/reconcile/mismatches`,
  `/api/v1/oms/overview`, `/api/v1/risk/denials`) actually exist as
  registered routes, rather than inventing plausible-sounding paths.

No stale command, port, or environment variable was preserved without this
cross-check; none was invented.

## 3. Deliverables

- `docs/runbooks/autonomous_paper_ops.md` — updated in place. New content:
  - `## 0. Safety boundary` (and `0a`/`0b`): the four safety-boundary
    bullets, prerequisites table, and the operating-vs-test-vs-reality-test
    database port table.
  - `# Part 2 - Daily-Operation Lifecycle Truth` (`## 15` through `## 23`):
    start-of-day sequence, the five authoritative read-only routes and their
    full truth-state/finalization-status/evidence vocabulary (explicitly
    stating `not_found` is not a backend failure, null counts are
    unavailable not zero, and generic `completed` is not automatic
    no-trade/activity proof), before-session checklist, during-session
    supervision, recovery procedures (bounded, never inventing a manual
    finalization command or DB rewrite), stop/emergency posture,
    end-of-day evidence capture list, restart distinctions (before
    finalization / after terminal commit / after evidence blocker), and
    explicit prohibitions.
- `docs/specs/autonomous_daily_paper_operations_01f2_operator_runbook_correction.md`
  (this document).
- `scripts/guards/validate_autonomous_daily_paper_operations_01f2_operator_runbook_correction.ps1`
  — source-aware static validator (see §4).

## 4. F2 guard

`validate_autonomous_daily_paper_operations_01f2_operator_runbook_correction.ps1`
performs pure text/source validation only — no network call, no DB
connection, no daemon start, no cargo/npm build or test. It fails when the
runbook:

1. Omits the Paper + Alpaca scope statement.
2. Omits the single-symbol/supervised scope statement.
3. Omits a live-routing-disabled verification step.
4. Omits any of the five authoritative routes.
5. Collapses `not_found` / `backend_unavailable` / `query_failed` into a
   single treatment (each must appear as a distinct value).
6. Fails to state that null counts are unavailable, not zero.
7. Omits `evidence_degraded` handling.
8. Omits restart-before-finalization / restart-after-finalization behavior.
9. Contains a manual finalization or DB-rewrite instruction (forbidden
   phrase scan).
10. Claims the unattended soak has started.
11. Claims live-capital readiness.
12. References the forbidden operating-DB port `5440` as an operating
    database, or otherwise contradicts the port table in §0b.
13. The F2 spec doc is missing or empty.
14. The runbook file itself is missing.

It also re-invokes the F1 guard and the Phase E closure guard (which
transitively re-invokes E1-E4) and asserts both exit 0, and asserts the
committed/working-tree patch-scope boundary: no production Rust file
(`core-rs/**/src/**.rs`, excluding `tests/`), no migration file, no GUI
production file under `core-rs/mqk-gui/src/` is touched by this patch.

## 5. Validation performed

```text
F2 guard: PASS
F1 guard: PASS
Phase E closure guard (transitively re-invokes E1-E4): PASS
check_unsafe_patterns.ps1: PASS

npm test (core-rs/mqk-gui): 850/850 PASS (unchanged from F1 entry gate)
npm run build (core-rs/mqk-gui): PASS (unchanged from F1 entry gate)

scenario_autonomous_daily_operation_api_01: 50/50 PASS (unchanged)
scenario_gui_daemon_contract_gate: 23/23 PASS (unchanged)
scenario_daemon_routes: 84/84 PASS (unchanged)
```

F2 makes no Rust or GUI source change, so the GUI/Rust regression results
above are identical to the F1 entry-gate run recorded in the combined
session's own evidence — re-run is not required to change; F2's guard
itself is the new artifact under test.

## 6. Documentation status after F2

```text
F1: ACCEPTED - COMPLETE
F2: IMPLEMENTATION COMPLETE - AWAITING FINAL COMBINED ACCEPTANCE
F3: NOT STARTED
PHASE F: OPEN
PHASE G: NOT STARTED
BUNDLE 3: OPEN
```

## 7. F3 boundary

F3 (supervised soak-evidence preparation) is not started by this patch. No
capture script, validator, or template is created by F2.

## 8. Phase G boundary

Phase G (final Bundle 3 closure audit) is not started by this patch. Bundle
3 remains open.

## 9. Soak / live-capital boundaries

The unattended 10-20-session paper soak has not started and is not
authorized by this patch. Live trading is not ready and is not authorized by
this patch.
