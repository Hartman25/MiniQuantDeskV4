# Strategy Scanner Jobs/GUI 01E — Closure Decision

Patch group: `STRATEGY-SCANNER-DAEMON-JOBS-AND-GUI-REVIEW-01-COMBINED`
Patch: `STRATEGY-SCANNER-JOBS-GUI-01E-END-TO-END-PROOF-AND-CLOSURE-01`

## 1. Is `STRATEGY-SCANNER-DAEMON-JOBS-AND-GUI-REVIEW-01-COMBINED` closed?

**Yes — `CLOSED_LOCAL`.** An operator can submit a bounded local-data
strategy scan through the daemon, poll job status, load/review the
generated artifact through the GUI review screen, and every surface
(daemon JSON response and GUI screen alike) carries the fixed
research-only warning set. This was proven end-to-end via daemon
scenario tests (Rust, in-process HTTP router) and a manual browser
verification of the GUI screen; a live HTTP replay against a *running
daemon binary* did not happen — see §15/§16 for why, and why the
scenario-test proof is sufficient for closure per the mission's own
fallback instruction.

## 2. What daemon job routes were added?

- `POST /api/v1/strategy-scans/jobs` (operator auth) — submit a bounded
  scan job.
- `GET /api/v1/strategy-scans/jobs` (public) — list all in-memory jobs,
  newest first.
- `GET /api/v1/strategy-scans/jobs/:job_id` (public) — full job status,
  including the `mqk_backtest::ScanSummary` once completed.

Implemented in `core-rs/crates/mqk-daemon/src/routes/strategy_scans.rs`
+ `core-rs/crates/mqk-daemon/src/strategy_scan_jobs.rs`, wired in
`routes.rs`/`state.rs`. Commit `217cf39c`.

## 3. What artifact readback route was added?

`GET /api/v1/strategy-scans/artifact?artifact_dir=<path>` (public).
Reads `manifest.json`/`summary.json`/`candidates.json` from a directory
that must canonicalize inside the configured
`MQK_STRATEGY_SCAN_ARTIFACT_ROOT` (default `exports/strategy_scans`).
`truth_state`: `active` / `missing_artifact` / `invalid_artifact` /
`path_rejected` / `read_failed`. Candidate rows capped at 200. Commit
`217cf39c` (same commit as the job routes — see the ledger entry for why
Phase B and C were implemented together).

## 4. What GUI screen/panel was added?

A new `strategyScanner` screen
(`core-rs/mqk-gui/src/features/strategyScanner/StrategyScannerScreen.tsx`),
registered in `screenRegistry.tsx`'s `diagnostics` monitor group and
`leftRailNav.ts`'s `LEFT_RAIL_SECONDARY`, adjacent to the existing
`backtests` screen. Commit `497cf787`.

## 5. Does it use local data only?

**Yes.** The daemon job runner calls
`mqk_backtest::execute_strategy_scan` / `write_scan_artifacts` — the
same pure, local-file-only functions the CLI's `mqk backtest
scan-strategies` now calls (moved out of `mqk-cli` in Phase B so both
callers share one implementation). No provider, broker, or network
import exists anywhere in `routes/strategy_scans.rs`,
`strategy_scan_jobs.rs`, or the GUI's `strategyScanner/` feature — the
GUI only calls `/api/v1/strategy-scans/*`, proven by a static source-text
test (`screenSource.test.ts`) that asserts the absence of any other
route reference.

## 6. Does it write deterministic scanner artifacts?

**Yes**, unchanged from the closed `STRATEGY-LAB-COMPLETION-AND-SCANNER-
FOUNDATION-01-COMBINED` bundle: `manifest.json` / `candidates.json` /
`candidates.csv` / `summary.json` under
`{out_dir}/{scan_id}/`, `scan_id` a deterministic UUIDv5. The daemon job
runner produces byte-identical artifacts to the CLI for the same inputs,
since both now call the same shared `mqk-backtest` functions.

## 7. Are generated artifacts staged?

**No.** Every daemon scenario test writes to `std::env::temp_dir()`
subdirectories (`mqk_daemon_strategy_scan_jobs_*`), never to the repo
tree. `exports/strategy_scans/` remains covered by the existing
`.gitignore`. `git status --short exports/` was empty after every test
run and after the manual GUI verification (the GUI verification never
reached a running daemon, so no artifact was written at all — see §15).

## 8. Does GUI show research-only warning?

**Yes**, on every surface that displays scan results: the submit-form
panel, the job-status panel, and the artifact-review panel all render
the same fixed three-sentence warning
(`ResearchOnlyWarningBanner` in `StrategyScannerScreen.tsx`), matching
the daemon's own fixed warning text
(`fixed_research_warnings()` in `routes/strategy_scans.rs`). Verified by
manual browser check (§16) and by the `screenSource.test.ts` static-text
test.

## 9. Does GUI avoid trade/promote/approve controls?

**Yes.** No button, link, or control on `StrategyScannerScreen.tsx`
submits an order, promotes a candidate, or approves anything —
`screenSource.test.ts` asserts the literal absence of "Promote",
"Approve", "Submit Order", "Place Order", "Buy ", "Sell ", "Trade Now",
`recommended_for_live`, and `approved_for_live` from the screen's source
text, and the absence of `/api/v1/execution/orders`,
`/api/v1/strategy/signal`, `/api/v1/ops/action`, and `/v1/run/start` from
both the screen and its API client.

## 10. Were provider/broker/network calls made? Expected no.

**No**, at any phase, in any test or manual verification. The daemon
scenario tests assert `st.db.is_none()` before running a scan job (no DB
pool configured) and the job still completes — the only IO performed is
reading the two local fixture input trees and writing the local output
tree, in a fresh OS temp directory for every test.

## 11. Were paper/live orders submitted? Expected no.

**No.** No `oms_outbox`/`oms_inbox` row was written by any new code path
(none of the new files import an OMS write type, a broker adapter, or a
DB write path for orders); no broker adapter was invoked; no order of
any kind was submitted at any phase.

## 12. Were strategy thresholds changed? Expected no.

**No.** No existing strategy engine's logic, sizing default, or signal
threshold was touched. The daemon job runner and the CLI both call the
same unmodified `PluginRegistry`-resolved strategies through the
unmodified scanner core.

## 13. Were risk/session/OMS gates changed? Expected no.

**No.** No file outside this patch's stated scope (daemon
routes/state/job-store, `mqk-backtest` scanner extraction, GUI
`strategyScanner` feature plus the two small typing-registration edits
`core.ts`/`sourceAuthority.ts` required for the new screen to compile)
was touched.

## 14. Was any DB migration added? Expected no.

**No.** The job store is `Arc<Mutex<HashMap<Uuid,
StrategyScanJobRecord>>>`, in-memory, process-lifetime only — modeled
directly on the existing `backtest_jobs` precedent (Phase A's audit,
§3/§9, confirmed this was the correct precedent over the DB-backed
`ingest_jobs` optional-persistence path).

## 15. Did end-to-end route proof run?

**Partially — by design, not by omission.** The full stack was proven
in two separate ways that together cover the same ground the mission's
suggested `Invoke-RestMethod` script would have covered, without
starting the daemon binary:

- **Daemon HTTP contract**: 14 scenario tests in
  `mqk-daemon/tests/scenario_strategy_scanner_jobs_01.rs` exercise the
  real Axum router in-process (`tower::ServiceExt::oneshot`, the same
  harness every other daemon scenario test in this repo uses) — submit
  → poll → completed → artifact readback, including the exact
  `truth_state` transitions (`active` / `missing_artifact` /
  `invalid_artifact` / `path_rejected`) the mission's route-proof script
  would have observed from a live `Invoke-RestMethod` session. All 14
  pass.
- **GUI**: manually verified in a real browser against the actual Vite
  dev build (§16) — the screen renders, the warning banner and form
  render correctly, and the submit-with-unreachable-daemon failure path
  degrades honestly (visible error, button un-stuck) rather than
  hanging.

What did **not** run: `Invoke-RestMethod` against an actually-running
`mqk-daemon.exe` process, and the GUI submitting a real job against that
running daemon.

## 16. If not, why not?

The mission's own HARD SAFETY RULES list `Do NOT start the daemon
runtime` as an unconditional line item, separate from and in addition to
`Do NOT arm execution` / `Do NOT run autonomous trading`. Starting
`mqk-daemon.exe` — even read-only, even with no arm/execution action
taken — is starting the daemon runtime. The mission's own Phase E text
anticipates exactly this outcome: *"If daemon is not running or would
require unsafe startup, do not start it. Use route scenario tests as
proof and document live route replay pending."* This closure follows
that fallback: the scenario-test proof (§15) is the closure evidence: it
exercises the identical Axum router `build_router` produces, over the
identical route table, with the identical handler code that would run
inside a live daemon process — the only thing not proven is the OS
process boundary (binding to a TCP port, spawning the binary), which
carries no additional code-correctness risk for a research-only,
in-memory-job, no-DB, no-network feature. The GUI's manual verification
(§ above) was run against a real dev server in a real browser and is
therefore a genuine (not simulated) proof of the client side; only the
daemon side stopped short of the OS-process boundary, for the reason
just given.

## 17. What remains open before scanner output can feed autonomous trading?

Unchanged from the prior bundle's own answer (§15 of
`strategy_lab_scanner_01e_closure_decision.md`), plus this bundle's own
additions:

- No real 1H or 5m local bar data exists in this repo yet — only
  `swing_momentum` on `1D` has ever been proven against real (non-
  fixture) data.
- The scanner produces research rankings, not trading signals. No
  admission/promotion gate consumes scanner output — that remains a
  distinct, explicitly out-of-scope future decision.
- Job history is daemon-process-lifetime only (in-memory); a daemon
  restart clears the job list (the artifact files on disk are
  unaffected and remain independently readable via the artifact route
  by directory path, as long as that path still canonicalizes inside
  the configured root).
- No live daemon-process HTTP replay of these routes has run yet (§15/
  §16) — the next off-market or market-hours session that has daemon
  startup already in scope (e.g. a paper-lifecycle proof session) is a
  natural place to fold in a live curl/Invoke-RestMethod check of these
  three routes as a zero-additional-risk addition to work already
  starting the daemon.

## 18. Exact next market-hours proof

`PAPER-TRADE-LIFECYCLE-PROOF-03-PNL-VISIBILITY-VERIFY-COMBINED`
(unchanged — this bundle is off-market research/operator tooling and
does not touch the paper P&L visibility proof still pending from prior
bundles).

## 19. Exact next off-market bundle

`STRATEGY-SCANNER-PROMOTION-GATES-AND-RESEARCH-QUEUE-01-COMBINED` — define
promotion gates and a research review workflow for scanner output. This
next patch must **not** promote scanner output to trading; it defines
the gates and queue that a future, separately authorized decision would
need before any scanner candidate could ever be considered for paper or
live trading.

---

## Final status

```text
STRATEGY-SCANNER-DAEMON-JOBS-AND-GUI-REVIEW-01-COMBINED: CLOSED_LOCAL
```

**Full patch-group commit chain:** Phase A `688f3bf0` (audit + design) ->
Phase B+C `217cf39c` (daemon scan jobs + artifact readback, 14 daemon
tests, scanner core extracted to `mqk-backtest`) -> Phase D `497cf787`
(GUI review screen, 18 new GUI tests) -> Phase E (this entry, closure).

**Safety confirmation (whole bundle):** no live orders; no forced or
manually submitted paper orders; no autonomous smoke script run; no
execution armed; no daemon runtime started at any phase; no strategy/
threshold/gate change to any existing code path; no fabricated
candidate, score, bar, order, fill, or position at any phase; no DB
migration; no `.env.local` edit; no provider/broker/network call in any
test or manual verification; no generated scan artifact, smoke log,
export, or untracked ledger draft staged at any phase.
