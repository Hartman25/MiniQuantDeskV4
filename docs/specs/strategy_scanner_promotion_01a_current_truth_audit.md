# Strategy Scanner Promotion 01A — Current Truth Audit & Guardrail Design

Patch group: `STRATEGY-SCANNER-PROMOTION-GATES-AND-RESEARCH-QUEUE-01-COMBINED`
Patch: `STRATEGY-SCANNER-PROMOTION-01A-CURRENT-TRUTH-AUDIT-AND-GUARDRAIL-DESIGN-01`

## 1. Current HEAD

`f6f21392` (`docs: close strategy scanner jobs gui`). Branch `main`, no
dirty tracked files, no staged files at audit time. Only allowed
untracked paths present (`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`,
`smoke_logs/`).

## 2. Current scanner foundation status

`STRATEGY-LAB-COMPLETION-AND-SCANNER-FOUNDATION-01-COMBINED` is
`CLOSED_LOCAL`. Confirmed present in the current tree,
`core-rs/crates/mqk-backtest/src/strategy_scanner.rs`: pure
`evaluate_scan_candidate` / `rank_scan_candidates`, `StrategyScanPolicy`,
`StrategyScanCandidate`, `StrategyScanMetrics`, `StrategyScanTruthState`
(`candidate_ranked` / `insufficient_data` / `backtest_failed` /
`unsupported_strategy` / `unsupported_timeframe` / `data_missing` /
`metrics_unavailable`), `StrategyScanReasonCode`. No file/network/DB IO
in this module. `mqk backtest scan-strategies` CLI
(`core-rs/crates/mqk-cli/src/commands/bkt.rs::run_strategy_scan`) is a
thin wrapper over the shared `mqk_backtest::execute_strategy_scan` /
`write_scan_artifacts` functions.

## 3. Current scanner daemon/GUI status

`STRATEGY-SCANNER-DAEMON-JOBS-AND-GUI-REVIEW-01-COMBINED` is
`CLOSED_LOCAL`. Confirmed present:

- `POST /api/v1/strategy-scans/jobs` (operator auth) — submit a bounded
  scan job.
- `GET /api/v1/strategy-scans/jobs` (public) — list in-memory jobs.
- `GET /api/v1/strategy-scans/jobs/:job_id` (public) — job status +
  `mqk_backtest::ScanSummary`.
- `GET /api/v1/strategy-scans/artifact?artifact_dir=...` (public) —
  read-only artifact readback; path must canonicalize inside
  `MQK_STRATEGY_SCAN_ARTIFACT_ROOT` (default `exports/strategy_scans`);
  `truth_state`: `active` / `missing_artifact` / `invalid_artifact` /
  `path_rejected` / `read_failed`; candidate rows capped at 200.
- GUI screen `core-rs/mqk-gui/src/features/strategyScanner/StrategyScannerScreen.tsx`
  (+ `api.ts` / `types.ts`), reachable via `screenRegistry.tsx`'s
  `diagnostics` monitor group. Submit form, 2s-poll job-status panel, and
  an artifact-review panel. Every panel renders the fixed
  `ResearchOnlyWarningBanner` three-sentence warning set. No
  trade/promote/approve control anywhere on the screen (enforced by a
  static `screenSource.test.ts` negative-text assertion).

Jobs are in-memory only (`Arc<Mutex<HashMap<Uuid, StrategyScanJobRecord>>>`),
process-lifetime, no DB table. No provider, broker, or OMS-write import
exists anywhere in `routes/strategy_scans.rs`, `strategy_scan_jobs.rs`,
or the GUI `strategyScanner/` feature.

## 4. Current Strategy Lab evaluator status

`core-rs/crates/mqk-backtest/src/strategy_lab.rs` is a separate,
already-closed, pure research-only evaluator
(`evaluate_strategy_lab_with_policy`) that classifies **already-computed
backtest sweep rows** (not scanner candidates) into
`StrategyLabDecision` (`ResearchPass` / `ResearchWatch` / `ResearchFail`
/ `InsufficientData`) plus an `A`–`F`/`Insufficient` `StrategyLabGrade`
and a 0–100 `score`. It consumes `StrategyLabMetrics` (return, drawdown,
trade count, win rate, profit factor, expectancy, exposure, sharpe,
buy/hold return, alpha) via `strategy_lab_input_from_sweep_row`. This
evaluator is **not** the scanner-candidate promotion/review queue this
patch adds — it operates on a different input shape (`SweepRowResult`,
not `StrategyScanCandidate`) and has no notion of `paper_candidate`/
`watchlist_candidate` states, `blocked`/`rejected` review states, or a
review artifact. It is a useful precedent for scoring/classification
style (deterministic, reason-coded, pure) but is not reused directly —
see §8/§9.

## 5. Existing candidate metrics and their limitations

`StrategyScanMetrics` (per `StrategyScanCandidate`): `total_return_pct`,
`benchmark_return_pct`, `alpha_pct`, `max_drawdown_pct`, `trade_count`,
`win_rate_pct`, `profit_factor`, `fill_count`, `bars_used`,
`data_start_ts`, `data_end_ts`, `halted` — all `Option<T>` except
`bars_used`/`halted`, `None`/absent for any skipped (non-`candidate_ranked`)
candidate. `score` (used for ranking) is `alpha_pct.or(total_return_pct)`
— i.e. rank is driven by alpha-vs-benchmark when available, not by
absolute profitability. No `exposure` metric exists (deliberately
omitted in the scanner foundation patch — `BacktestReport` does not
expose per-bar position size).

## 6. Existing scanner caveat: rank can be positive while absolute return is negative

Confirmed unchanged from the scanner foundation and jobs/GUI closure
docs: because `score = alpha_pct.or(total_return_pct)`, a candidate can
have a strongly positive `alpha_pct` (beat its benchmark) while its own
`total_return_pct` is negative (lost money in absolute terms). The prior
real local-data proof run found every top-ranked 1D `swing_momentum`
candidate had a **negative** absolute `total_return_pct`. Any promotion
review model must gate on **absolute** total return, not on rank or
alpha alone — this is the load-bearing safety requirement for this
patch (see §11 rule 11 in the mission, and Phase B's negative-return
test).

## 7. First proven gap: no promotion/research queue exists

Grepped `core-rs/crates/mqk-backtest`, `core-rs/crates/mqk-daemon`,
`core-rs/crates/mqk-cli`, `core-rs/mqk-gui/src`, `docs` for
`promotion|promote|approved|research queue|review queue|quarantine` in
scanner-adjacent code: no `promotion`/`review` module, state enum, or
route exists anywhere in the scanner or Strategy Lab surfaces today.
`StrategyLabDecision` (§4) is the closest existing concept but is scoped
to sweep rows, not scanner candidates, and has no file-artifact or
daemon/GUI surface of its own beyond a CLI rank command
(`mqk backtest strategy-lab-rank`, confirmed via
`scenario_cli_strategy_lab_evaluate.rs`). No code path anywhere consumes
scanner or Strategy Lab output for order submission, admission, or
strategy routing — confirmed by grepping for `execute_strategy_scan`,
`StrategyScanCandidate`, and `evaluate_strategy_lab` outside
`mqk-backtest`/`mqk-cli`/`mqk-daemon::routes::strategy_scans`/
`mqk-gui::strategyScanner`: no hits in `mqk-execution`, `mqk-daemon`
order/admission/routing modules, or the OMS/outbox/inbox/portfolio
paths.

## 8. Selected design

- File-based research review artifact under
  `exports/strategy_reviews/{review_id}/` — no DB migration.
- Pure classifier module `mqk-backtest::strategy_scan_review` (models
  `mqk-backtest::strategy_scanner`'s existing pure/no-IO style), taking
  already-computed `StrategyScanCandidate` rows and a
  `StrategyScanReviewPolicy` and producing `StrategyScanReviewDecision`
  rows with a `StrategyScanReviewState`
  (`Blocked`/`NeedsReview`/`WatchlistCandidate`/`PaperCandidate`/`Rejected`).
- CLI command `mqk backtest review-scan` (models
  `run_strategy_scan`'s existing thin-wrapper style) reads an existing
  scanner artifact directory and writes a review artifact directory.
- Daemon read-only GET route (models
  `GET /api/v1/strategy-scans/artifact`'s existing path-validation
  pattern: canonicalize + `starts_with` a configured root, `truth_state`
  enum) exposing review artifacts.
- GUI display only — no promotion/trading action added anywhere.
- No trading/admission wiring: this patch does not import or call any
  broker, provider, OMS, outbox, inbox, admission, or strategy-router
  type from any new file.
- No automatic promotion: `paper_candidate` is a review-artifact label
  only, not an executable state; nothing consumes it to submit or route
  an order.

## 9. Exact promotion/review state model

```text
Blocked             — missing required evidence or hard safety issue
                       (never promotable; also the default when data is
                       insufficient).
NeedsReview         — enough evidence exists to inspect, not enough to
                       call it a candidate.
WatchlistCandidate  — can be watched/retested, but not traded.
PaperCandidate      — eligible for a later, SEPARATELY AUTHORIZED
                       paper-promotion patch review only. NOT trading
                       approval, NOT automatically tradable.
Rejected            — explicitly fails evidence requirements (e.g.
                       excess drawdown, halted run).
```

Minimum safe gates (see Phase B for exact policy defaults and
implementation): `candidate_ranked` truth state required; score present;
**absolute total return positive** (this is the rule that stops a
negative-absolute-return candidate from ever reaching
`PaperCandidate`, regardless of alpha); alpha positive; max drawdown
present and within policy; trade count above minimum; profit factor
above minimum if present; bars used above minimum; not halted. Missing
metrics can only produce `Blocked` or `NeedsReview`, never a promotable
state.

## 10. Exact review artifact schema

Under `{out_dir}/{review_id}/`:

- `manifest.json` — `schema_version`, `review_id` (deterministic
  UUIDv5), `scanner_scan_id`, `created_at_utc`, `git_hash`,
  `source_artifact_dir`, policy field values, candidate/state counts,
  fixed warning set.
- `review_decisions.json` — full `Vec<StrategyScanReviewDecision>`.
- `review_decisions.csv` — same rows, CSV.
- `summary.json` — counts by state, top paper/watchlist candidates,
  blockers, warnings.

## 11. Exact CLI/daemon/GUI behavior

- CLI: `mqk backtest review-scan --artifact-dir <scanner artifact dir>
  --out-dir exports/strategy_reviews --top 50`. Reads
  `manifest.json`/`summary.json`/`candidates.json` from the scanner
  artifact dir, classifies every candidate, writes the four review
  artifact files. No provider/broker/network/DB call.
- Daemon: `GET /api/v1/strategy-scans/review-artifact?review_dir=<path>`
  (public, read-only), same canonicalize+root-prefix validation pattern
  as the existing scanner artifact route, `truth_state`: `active` /
  `missing_artifact` / `invalid_artifact` / `path_rejected` /
  `read_failed`. Decisions capped at 200 rows in the response.
- GUI: Strategy Scanner screen gains a review-artifact load/display
  panel — counts by state, paper/watchlist candidate tables, blockers,
  and the fixed research-only + promotion-is-not-approval warning set.
  No new button submits, promotes, or approves anything.

## 12. Exact tests

Phase B (pure, `mqk-backtest`): ranked positive candidate can become
`PaperCandidate`; negative absolute return blocks `PaperCandidate` even
with positive alpha; missing score blocks; missing drawdown blocks; too
few trades blocks/needs-review; too few bars blocks; excess drawdown
rejects; halted candidate rejects; `data_missing` scanner candidate
cannot promote; marginal evidence can reach `WatchlistCandidate`;
deterministic ordering; stable serialization; source-level proof of no
broker/order/OMS/admission import.

Phase C (CLI): reads a tiny scanner fixture artifact; writes all four
review files; negative-return positive-alpha candidate blocked, not
`paper_candidate`; positive candidate becomes `paper_candidate`; missing
scanner files fail truthfully; invalid scanner JSON fails truthfully;
stable CSV headers; deterministic review id; no DB/provider/broker env
required.

Phase D (daemon + GUI): valid review artifact → `active`; missing →
`missing_artifact`; invalid JSON → `invalid_artifact`; path escape →
`path_rejected`; decision list capped; GUI types/API handle the
response; GUI source contains required warnings and no
trade/promote/approve/order route reference; existing scanner GUI/
daemon tests still pass.

## 13. Non-goals

- No trade approval of any kind.
- No live or paper order submission.
- No strategy threshold or logic change.
- No admission/strategy-router integration.
- No broker/provider/network call anywhere in this patch.
- No DB migration.

## Reference: patch group and required terms

`STRATEGY-SCANNER-PROMOTION-GATES-AND-RESEARCH-QUEUE-01` — adds a
file-artifact-based promotion/research-review queue over existing
scanner output. `research evidence only`. `not autonomous trading approval`.
`promotion-ready is not trading-approved`. Candidates can
rank well while still carrying `negative absolute returns` — the review
model gates on absolute return, not rank/alpha alone. Adds a `review
queue` and durable `review artifact` per scan. Hard invariants
unchanged: `no live orders`, `no paper orders`, `no provider calls`,
`no broker calls`, `no strategy threshold changes`, `no admission wiring`,
`no DB migration`.
