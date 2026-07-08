# BACKTEST-MULTIPLIER-MARGIN-01 — Completion Audit

Patch ID: `BACKTEST-MULTIPLIER-MARGIN-01-COMPLETION-AUDIT-01`

Audit-only. No production code, config, DB, or trading-path change. No
network call, no provider/broker call, no DB connection, no daemon start.

---

## 1. HEAD and relevant commits

Audited at `HEAD = 6e6f69df` ("docs: define registry v2 consumption
boundary"), branch `main`, tracked working tree clean at audit start
(only the pre-existing allowed untracked `MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`
and `smoke_logs/` present).

The `BACKTEST-MULTIPLIER-MARGIN-01` lineage itself was built and committed
before this audit; individual sub-slice commit hashes are intentionally not
re-derived here (per each sub-slice's own ledger entry, which declines to
self-reference its own commit hash). `git log --oneline` on the current
branch is authoritative for the exact hashes.

---

## 2. Sub-slice table

| Sub-slice | Ledger status | What it proves |
|---|---|---|
| `BACKTEST-MULTIPLIER-MARGIN-01-COMBINED` | `CLOSED_LOCAL / PARTIAL` | Foundation: `BacktestInstrumentEconomics`, `BacktestEconomicsReport`, pure `notional_micros`/`mark_to_market_value_micros`/`realized_pnl_micros` helpers, all in new `mqk-backtest/src/economics.rs`. Not wired into the engine. |
| `BACKTEST-MULTIPLIER-RUN-WIRE-01-COMBINED` | `CLOSED_LOCAL / PARTIAL` | Wires a parallel `BacktestEconomicsLedger` into `BacktestEngine` via opt-in `.with_economics(...)`; proves full-run multiplier-aware notional/P&L/equity. `BacktestReport`/`metrics.json`/`manifest.json` still multiplier-unaware; no CLI/daemon/GUI entry point yet. |
| `BACKTEST-ECONOMICS-CONFIG-READY-01` | `CLOSED_LOCAL / PARTIAL` | Adds `BacktestReport::test_fixture()` / confirms `BacktestConfig::test_defaults()` so a new `BacktestReport` field becomes safe to add without breaking exhaustive struct literals in `mqk-backtest`/`mqk-artifacts` tests. |
| `BACKTEST-REPORT-FIXTURE-READY-01-COMBINED` | `CLOSED_LOCAL / PARTIAL` | Closes the same construction-safety blocker for the nine exhaustive `BacktestReport { .. }` literals found in `mqk-promotion/tests/` (test-only, mechanical). |
| `BACKTEST-REPORT-ECONOMICS-ARTIFACT-01-COMBINED` | `CLOSED_LOCAL / PARTIAL` | Adds `BacktestReport.economics: BacktestEconomicsReport`; `report.equity_curve` becomes genuinely multiplier-aware; `run_id` folds in economics (`derive_run_id_with_economics`) so two runs with different economics no longer collide; `metrics.json`/`report.md` surface economics. `manifest.json` and a registry-derived multiplier source still missing; no daemon/CLI/GUI entry point. |
| `BACKTEST-ECONOMICS-CLI-ENTRY-01-COMBINED` | `CLOSED_LOCAL / PARTIAL` | First operator entry point: `--contract-multiplier`/`--initial-margin-micros`/`--maintenance-margin-micros` on `mqk backtest csv`. |
| `BACKTEST-ECONOMICS-DB-CLI-ENTRY-01-COMBINED` | `CLOSED_LOCAL / PARTIAL` | Same flags extended to `mqk backtest db`. |
| `BACKTEST-ECONOMICS-DAEMON-JOB-REQUEST-01-COMBINED` | `CLOSED_LOCAL / PARTIAL` | `BacktestJobRequest.economics: Option<BacktestEconomicsRequest>` on `POST /api/v1/backtests/jobs`, wired for both the CSV-backed and `md_bars`-backed daemon worker paths. |
| `BACKTEST-ECONOMICS-GUI-REGISTRY-01-COMBINED` | `CLOSED_LOCAL / PARTIAL` | GUI submit-form economics fields (Backtest Results screen) wired to the daemon route above; adds read-only `GET /api/v1/backtests/economics-suggestion?symbol=` (equity/ETF-only against the converted v1 registry at this point). |
| `BACKTEST-ECONOMICS-REGISTRY-MANIFEST-01-COMBINED` | `CLOSED_LOCAL / PARTIAL` | Adds `manifest.json` economics (`ManifestEconomics`, merged into every `write_backtest_report` call site generically — not per-call-site special-cased); adds `InstrumentDefinitionV2.economics: Option<InstrumentEconomicsMetadataV2>` registry-v2 schema field + `backtest_economics_suggestion_for_instrument` pure helper. |
| `INSTRUMENT-REGISTRY-V2-SOURCE-01-COMBINED` | `CLOSED_LOCAL / PARTIAL` | Adds a separate `MQK_INSTRUMENT_REGISTRY_V2_PATH`-sourced registry the suggestion route reads first (fail-closed, no silent v1 fallback on a bad config); ships a disabled, non-tradable committed example fixture (`config/instruments/instruments_v2.backtest_suggestions.example.json`) proving the explicit-economics branch end-to-end. No production (non-example) v2 file exists. |
| `BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01` | `CLOSED_LOCAL` | Extends the same opt-in `--contract-multiplier`/`--initial-margin-micros`/`--maintenance-margin-micros` CLI flags already proven on `csv`/`db` to `mqk backtest csv-sweep`, applied identically to every sweep combination's engine via `.with_economics(...)`. `write_backtest_report`'s existing generic manifest-merge means every sweep point's `manifest.json`/`metrics.json` picks up truthful economics with zero `mqk-artifacts` change. Closes the one gap named in §3/§6 below. |

All twelve sub-slices are `CLOSED_LOCAL`; the parent `BACKTEST-MULTIPLIER-MARGIN-01`
label's final status is recorded separately in
[`backtest_multiplier_margin_01_closure_decision.md`](backtest_multiplier_margin_01_closure_decision.md).

---

## 3. Current entry-point matrix

| Entry point | Supports explicit economics? | Evidence |
|---|---|---|
| CLI `mqk backtest csv` | **Yes** | `--contract-multiplier`/`--initial-margin-micros`/`--maintenance-margin-micros` flags in `mqk-cli/src/main.rs` (`BacktestCmd::Csv`), wired via `build_backtest_economics_from_cli_flags` → `engine.with_economics(...)` in `mqk-cli/src/commands/bkt.rs::run_backtest_csv`. |
| CLI `mqk backtest db` | **Yes** | Same three flags on `BacktestCmd::Db`, same helper, wired in `run_backtest_db`. |
| CLI `mqk backtest csv-sweep` | **Yes (closed by `BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01`)** | `BacktestCmd::CsvSweep` (`mqk-cli/src/main.rs`) now carries the same three flags as `csv`/`db`; `run_sweep_csv` validates them once via `build_backtest_economics_from_cli_flags` and applies `.with_economics(economics.clone())` to every per-point `BacktestEngine`, so every sweep combination shares one caller-supplied economics value. `sweep_economics_contract_multiplier=<n>` is printed to stdout; per-point `manifest.json`/`metrics.json` carry the same economics as a single `csv`/`db` run. Proven by `mqk-cli/tests/scenario_cli_backtest_csv_sweep_economics.rs`. |
| Daemon `POST /api/v1/backtests/jobs` | **Yes** | `BacktestJobRequest.economics: Option<BacktestEconomicsRequest>` (`mqk-daemon/src/api_types.rs`), consumed in both the CSV-backed and `md_bars`-backed worker branches of `mqk-daemon/src/routes/backtests.rs`. |
| Daemon backtest-sweep jobs | **N/A — does not exist** | No sweep route or job kind exists in `mqk-daemon` (confirmed: zero `Sweep`/`sweep` matches in `mqk-daemon/src/routes/backtests.rs`). Sweep is CLI-only in this repo; there is no daemon-sweep gap to close. |
| GUI Backtest Results submit form | **Yes** | Optional `contract_multiplier`/`initial_margin_micros`/`maintenance_margin_micros` inputs in `BacktestResultsScreen.tsx`, blank = omitted (byte-identical to pre-flag request shape). |
| GUI sweep | **N/A — does not exist** | No sweep UI exists anywhere under `core-rs/mqk-gui/src/features/backtests` (confirmed: zero `Sweep`/`sweep` matches). |
| Artifacts (`mqk-artifacts::write_backtest_report`) | **Yes, generically** | Every call site (CLI csv/db/sweep-per-point, daemon csv/md_bars workers) already calls `write_backtest_report(&report, ...)`, and that function derives `metrics.json`/`report.md`/`manifest.json` economics from `report.economics` — the field is populated per-caller, not per-artifact-writer, so any caller that starts populating `report.economics` (e.g. sweep, once wired) gets truthful artifacts for free with no `mqk-artifacts` change required. |
| Registry-v2 suggestion route (`GET /api/v1/backtests/economics-suggestion`) | **Yes, as metadata suggestion only** | Reads `MQK_INSTRUMENT_REGISTRY_V2_PATH` if configured (fail-closed on bad config, no v1 fallback); otherwise falls back to the v1→v2 in-memory conversion (equity/ETF only, no `economics` field). Never auto-applies to a form; GUI only populates on explicit operator click. |

---

## 4. Current artifact matrix

| Artifact | Carries economics? | Notes |
|---|---|---|
| `BacktestReport.economics` (in-process) | Yes | `BacktestEconomicsReport { contract_multiplier, initial_margin_micros, maintenance_margin_micros, realized_pnl_micros, margin_enforced }`. |
| `BacktestReport.run_id` | Yes (identity-sensitive) | `derive_run_id_with_economics` folds economics into the run identity; two runs over identical bars/config/strategy but different economics now produce different `run_id`s. `config_id` itself is not economics-sensitive (economics is not a `BacktestConfig` field). |
| `metrics.json` | Yes | `economics` object mirrors `BacktestReport.economics`. |
| `report.md` | Yes | Renders "Contract Multiplier" / margin fields / "Margin Enforced" rows. |
| `manifest.json` | Yes, for any caller whose report carries real economics | `RunManifest.economics: ManifestEconomics` (`contract_multiplier`, margins, `margin_enforced`, `source: "default_equity" | "explicit_request"`), written by a read-parse-merge-write inside `write_backtest_report` — applies uniformly to every caller. The live/paper `run` CLI path (`init_run_artifacts` without a later `write_backtest_report` call) always shows `default_equity` and is unaffected by this metadata by construction. |

---

## 5. Current source matrix

| Source | Status |
|---|---|
| Explicit CLI flags (`csv`, `db`, `csv-sweep`) | Live, tested, fail-closed on non-positive multiplier; identical opt-in shape and defaults across all three CLI entry points. |
| Daemon request economics | Live, tested, fail-closed, both worker paths. |
| GUI form economics | Live, tested; blank fields omit the request field entirely. |
| Registry-v2 economics metadata (`InstrumentEconomicsMetadataV2`) | Schema + validation + pure suggestion helper exist and are unit-tested; reachable end-to-end through the daemon route only when `MQK_INSTRUMENT_REGISTRY_V2_PATH` is explicitly configured to a file containing `economics` — no production file exists, only a disabled/non-tradable committed example. |
| Default equity economics | `BacktestEngine::new(cfg)` without `.with_economics(...)` defaults to `BacktestInstrumentEconomics::equity()` (multiplier=1, no margin) everywhere — byte-identical to pre-economics behavior; this remains the default for every CLI/daemon entry point when no economics flags/fields are supplied. |

---

## 6. Current limitations

- **Margin metadata vs. margin enforcement.** `initial_margin_micros`/`maintenance_margin_micros` are carried faithfully end-to-end (CLI → engine → report → `metrics.json`/`manifest.json`/`report.md`), but `margin_enforced` is hardcoded `false` everywhere (`BacktestEconomicsReport::equity()` and `::from_run()` both set it). **No code path in this repo reads a margin field to gate, block, or alter any backtest behavior.** This is scaffolding, not enforcement, and is not claimed to be enforcement anywhere in the ledger.
- **Registry-derived vs. explicit economics.** The registry-v2 economics-suggestion route is read-only and suggestion-only: the GUI's "Load registry economics" button only populates local display state, never auto-submits into the multiplier/margin form inputs. In production today (no `MQK_INSTRUMENT_REGISTRY_V2_PATH` configured), the route can only ever answer from the v1→v2-converted equity/ETF registry, which carries no `economics` field — so it always returns the equity-default branch. The explicit-registry-economics and non-equity-no-contract-economics branches are proven end-to-end only against the committed, disabled, non-production example fixture.
- **Sweep support.** Closed by `BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01` (§2) — `mqk backtest csv-sweep` now supports the same opt-in economics flags as `csv`/`db`, applied identically to every sweep combination.
- **DB-backed proof status.** `mqk backtest db`'s economics flags are proven by `mqk-cli/tests/scenario_cli_backtest_db_economics.rs` against a real `MQK_DATABASE_URL`-backed `md_bars` table (isolated rows, cleaned up), not merely a fixture.
- **Production/live portfolio accounting boundary.** `mqk-portfolio::accounting.rs`/`metrics.rs` — the same accounting engine `mqk-runtime::orchestrator` uses for live/paper fills — was never modified by any sub-slice in this lineage. `mqk-backtest` is not a dependency of `mqk-runtime`, so there is no code path by which this economics seam can reach live/paper P&L. This boundary is deliberate and is the explicit prerequisite framing in `ASSET-CORE-01H` (`docs/specs/asset_core_01h_instrument_registry_v2_consumption_boundary_decision.md`) — closing `BACKTEST-MULTIPLIER-MARGIN-01` does **not** mean live/paper accounting becomes multiplier-aware.

---

## 7. Closure decision

At this audit's start (before Phase B), status was `PARTIAL / SAFE-GAPS-REMAIN`,
with the exact remaining safe gap being `mqk backtest csv-sweep`'s missing
economics flags (§3, §6). `BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01`
(§2) has since closed that gap: `csv-sweep` now carries the same opt-in
flags as `csv`/`db`, tested end-to-end (`scenario_cli_backtest_csv_sweep_economics.rs`,
4/4 passing) with zero regressions across `mqk-cli`/`mqk-backtest`/`mqk-artifacts`.
No further backtest-only entry-point gap is known. The final parent-label
status (final because margin enforcement and a real non-equity registry-v2
data source remain explicitly out of this session's scope, not because
anything here is incomplete) is recorded in
[`backtest_multiplier_margin_01_closure_decision.md`](backtest_multiplier_margin_01_closure_decision.md).

---

## 8. Recommended next patch

See [`backtest_multiplier_margin_01_closure_decision.md`](backtest_multiplier_margin_01_closure_decision.md)
for the final recommendation. In outline: closing the backtest-side gap
satisfies prerequisite #1 of `ASSET-CORE-01H`'s production-cutover boundary
(`docs/specs/asset_core_01h_instrument_registry_v2_consumption_boundary_decision.md`),
but does not by itself authorize `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01`
— the other prerequisites in that decision doc's §5 remain open.
