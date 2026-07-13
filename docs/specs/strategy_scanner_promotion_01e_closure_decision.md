# Strategy Scanner Promotion 01E — Closure Decision

Patch group: `STRATEGY-SCANNER-PROMOTION-GATES-AND-RESEARCH-QUEUE-01-COMBINED`
Patch: `STRATEGY-SCANNER-PROMOTION-01E-REAL-ARTIFACT-PROOF-AND-CLOSURE-01`

## 1. Is `STRATEGY-SCANNER-PROMOTION-GATES-AND-RESEARCH-QUEUE-01-COMBINED` closed?

**Yes — `CLOSED_LOCAL`.** Scanner candidates can now be classified into a
deterministic, file-artifact-based research-review queue
(`blocked`/`needs_review`/`watchlist_candidate`/`paper_candidate`/`rejected`),
the review artifact is written and independently readable (CLI writer,
daemon read-only route, GUI display panel), and the negative-absolute-
return safety rule was proven against **real** local 1D bar data, not
just synthetic fixtures: every one of 88 real candidates in this
session's scan run had a negative absolute `total_return_pct`, and all
88 were classified `rejected` — zero became `paper_candidate`. No
scanner or review output is wired into admission, the strategy router,
order submission, outbox, OMS, or broker state at any phase.

## 2. What pure review model was added?

`core-rs/crates/mqk-backtest/src/strategy_scan_review.rs`:
`StrategyScanReviewState` (`Blocked` / `NeedsReview` /
`WatchlistCandidate` / `PaperCandidate` / `Rejected`),
`StrategyScanReviewPolicy` (`min_bars_used=252`, `min_trade_count=5`,
`min_total_return_pct=0.0`, `min_alpha_pct=0.0`, `max_drawdown_pct=25.0`,
`min_profit_factor=1.05`), `evaluate_scan_review_decision` /
`build_review_decisions`. A candidate whose scanner `truth_state` never
reached `candidate_ranked` is always `Blocked`; missing required
evidence or too-few-bars is `Blocked`; halted, excess-drawdown, or
negative absolute `total_return_pct` (even with positive `alpha_pct`) is
`Rejected`; too-few-trades is `NeedsReview`; a present-but-weak or
missing profit factor is `WatchlistCandidate`; everything else is
`PaperCandidate`. Pure, no IO. 13 scenario tests (commit `fd34afb3`).

## 3. What CLI command was added?

`mqk backtest review-scan --artifact-dir <scanner artifact dir> --out-dir
exports/strategy_reviews --top 50`
(`core-rs/crates/mqk-cli/src/commands/bkt.rs::run_review_scan`, wired
into `main.rs`'s `BacktestCmd::ReviewScan`). Thin wrapper over
`mqk_backtest::{execute_strategy_scan_review, write_review_artifacts}`.
No provider/broker/network call, no live/paper order, no DB connection.
8 CLI scenario tests via `assert_cmd` subprocess invocation (commit
`a6c45196` + follow-up `5eeb82dc`).

## 4. What review artifact schema was added?

Under `{out_dir}/{review_id}/` (`review_id` a deterministic UUIDv5 of
the scanner `scan_id` + every policy threshold value — never
`Uuid::new_v4()`):

- `manifest.json` — schema version, `review_id`, `scanner_scan_id`,
  `source_artifact_dir`, timestamps, every policy threshold, per-state
  candidate counts, fixed warning set.
- `review_decisions.json` — full `Vec<StrategyScanReviewDecision>`.
- `review_decisions.csv` — same rows, stable CSV header.
- `summary.json` — counts by state, `top_paper_candidates`,
  `top_watchlist_candidates`, blockers, warnings.

## 5. What daemon review-artifact route was added?

`GET /api/v1/strategy-scans/review-artifact?review_dir=<path>` (public,
no auth) in `routes/strategy_scans.rs`. Same canonicalize+root-prefix
path-validation pattern as the sibling
`GET /api/v1/strategy-scans/artifact` route: reads only
`manifest.json`/`summary.json`/`review_decisions.json` from a directory
resolving inside `state.strategy_review_artifact_root` (default
`exports/strategy_reviews`, override
`MQK_STRATEGY_REVIEW_ARTIFACT_ROOT`). `truth_state`: `active` /
`missing_artifact` / `invalid_artifact` / `path_rejected` /
`read_failed`. Decision rows capped at 200. 7 daemon scenario tests
(commit `a8de5121`).

## 6. What GUI review behavior was added?

Extended the existing `StrategyScannerScreen.tsx` (no new screen key) —
no new screen registration was required. A "Research-review queue"
panel: a `review_dir` text input, a load button, and a display-only
`ReviewArtifactPanel` (counts by review state, a `paper_candidate`
table, a `watchlist_candidate` table, blockers). Every result carries
the fixed warning set ("promotion-ready is not trading-approved.",
"paper_candidate is not autonomous trading approval.", "A separate
paper-promotion patch is required before any paper trading."). No
button submits, promotes, or approves anything — extended
`screenSource.test.ts` asserts the new warning strings and route are
present and re-asserts the existing no-trade/promote/approve /
forbidden-route checks still pass unchanged.

## 7. Did real local scanner/review proof run?

**Yes.** Ran both CLI commands against the real (non-fixture) local 1D
bars tree already in this repo (`exports/md_backup/1D/`, 88 symbols),
at HEAD `a8de5121`:

```powershell
mqk-cli backtest scan-strategies --registry config/instruments/equities.json `
  --bars-root exports/md_backup --timeframe 1D --strategy swing_momentum `
  --top 20 --out-dir exports/strategy_scans

mqk-cli backtest review-scan --artifact-dir exports/strategy_scans/<scan_id> `
  --out-dir exports/strategy_reviews --top 50
```

## 8. Artifact paths generated

- `exports/strategy_scans/83b56e5d-f01c-566f-a200-2e682db1c708/`
- `exports/strategy_reviews/a1b3bd4c-c01a-5255-be7a-f5031051dbab/`

## 9. Were generated artifacts staged? Expected no.

**No.** `git status --short exports/` was empty after both runs;
`git check-ignore -v` confirms both directories match the existing
blanket `exports/` rule in `.gitignore` (line 29).

## 10. How many candidates reviewed?

**88** — the full enabled-equity universe scanned by `scan-strategies`
(`universe_count=88`, `ranked_count=88`, `skipped_count=0`; every
candidate reached `candidate_ranked`).

## 11. Counts by review state

```text
candidate_count=88
blocked_count=0
needs_review_count=0
watchlist_candidate_count=0
paper_candidate_count=0
rejected_count=88
```

## 12. Were negative absolute return candidates blocked from `paper_candidate`?

**Yes — all of them.** Every one of the 88 real candidates (including
the top-ranked `LCID` at `score=95.9469`) had a negative absolute
`total_return_pct` (e.g. `LCID: -1.1609`, `PLUG: -3.5698`,
`CHPT: -2.4299`). `review_decisions.csv` shows all 88 rows as
`rejected` with `reason_codes=negative_total_return` and the blocker
text *"a candidate cannot be promoted on rank/alpha alone while losing
money in absolute terms"*. Zero candidates reached `paper_candidate` in
this real run — the rank-vs-absolute-return safety rule (the core
requirement of this whole patch group) holds on real data, not just
synthetic test fixtures.

## 13. Were any provider/broker/network calls made? Expected no.

**No.** Both CLI commands only read local files
(`config/instruments/equities.json`, `exports/md_backup/1D/*.csv`, the
scanner artifact's own JSON files) and wrote local files. No daemon
process was started at any phase of this patch (all daemon-facing proof
came from in-process Axum-router scenario tests, per the mission's own
hard "do not start the daemon runtime" rule).

## 14. Were any orders submitted? Expected no.

**No.** No broker adapter, OMS write path, outbox/inbox table, or order
type is imported by any file this patch added or modified.

## 15. Were strategy thresholds changed? Expected no.

**No.** No existing strategy engine, sizing default, or scanner
threshold was touched. `run_review_scan`/`execute_strategy_scan_review`
consume already-computed `StrategyScanCandidate` rows read from disk;
they do not re-run any strategy or backtest.

## 16. Were risk/session/OMS gates changed? Expected no.

**No.** No file outside this patch's stated scope (the new
`strategy_scan_review` module, the CLI command, the daemon route/
api_types/state field, and the GUI `strategyScanner` feature's additive
panel) was touched.

## 17. Was any DB migration added? Expected no.

**No.** The review model and artifact are entirely file-based
(`exports/strategy_reviews/{review_id}/`), matching the mission's
expected design. No migration file was added; no new DB table or
column exists.

## 18. What remains open before scanner output can feed autonomous trading?

- The review model only classifies research evidence — nothing consumes
  a `paper_candidate` decision to submit, route, or admit an order. That
  remains a distinct, explicitly out-of-scope future decision (the
  mission's own next off-market bundle, §20, is itself still
  research-only).
- On this run's real data, **zero** candidates reached `paper_candidate`
  — the policy defaults (`min_total_return_pct=0.0` in particular) are
  strict by design; a future session may want to review whether that
  default is appropriately calibrated once more real timeframes/symbols
  are available, but changing it is a policy decision, not a code gap.
- No real 1H or 5m local bar data exists in this repo yet (unchanged
  from prior scanner bundles) — this session's real proof, like all
  prior ones, covers only 1D `swing_momentum`.
- Job history for the daemon's scan-job API remains process-lifetime
  only (in-memory); the new review-artifact route is a pure
  file-directory reader with no daemon-side job registry of its own.
- No live daemon-process HTTP replay of the new review-artifact route
  has run yet — same unresolved item carried forward from the prior
  scanner-jobs-gui bundle's own closure doc.

## 19. Recommended next market-hours proof

`PAPER-TRADE-LIFECYCLE-PROOF-03-PNL-VISIBILITY-VERIFY-COMBINED`
(unchanged — this bundle is off-market research-governance tooling and
does not touch the paper P&L visibility proof still pending from prior
bundles).

## 20. Recommended next off-market bundle

`STRATEGY-ROUTER-RESEARCH-ONLY-SELECTION-01-COMBINED` — must remain
research-only: it should select candidates for analysis, not trading.

---

## Final status

```text
STRATEGY-SCANNER-PROMOTION-GATES-AND-RESEARCH-QUEUE-01-COMBINED: CLOSED_LOCAL
```

**Full patch-group commit chain:** Phase A `1b297b8d` (audit + guardrail
design) -> Phase B `fd34afb3` (pure review model, 13 tests) -> Phase C
`a6c45196` + `5eeb82dc` (CLI `review-scan` command + review-artifact IO,
8 CLI tests) -> Phase D `a8de5121` (daemon read-only review-artifact
route, GUI review panel, 7 daemon tests) -> Phase E (this entry,
closure, real-data proof).

**Safety confirmation (whole bundle):** no live orders; no forced or
manually submitted paper orders; no autonomous smoke script run; no
execution armed; no daemon runtime started at any phase; no strategy/
threshold/gate change to any existing code path; no fabricated
candidate, score, bar, decision, or promotion evidence at any phase; no
DB migration; no `.env.local` edit; no provider/broker/network call in
any test or the real-data proof run; no generated scan/review artifact,
smoke log, or untracked ledger draft staged at any phase.
