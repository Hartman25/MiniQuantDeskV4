# BACKTEST-MULTIPLIER-MARGIN-01 — Closure Decision

Patch ID: `BACKTEST-MULTIPLIER-MARGIN-01-CLOSURE-OR-BOUNDARY-DECISION-01`

Decision-only. Builds on
[`backtest_multiplier_margin_01_completion_audit.md`](backtest_multiplier_margin_01_completion_audit.md)
(Phase A) and the `BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01` sweep-
economics wiring (Phase B). No code changed by this document.

---

## 1. Is `BACKTEST-MULTIPLIER-MARGIN-01` closed for backtesting?

**Yes.** Every backtest entry point that exists in this repo — CLI `csv`,
CLI `db`, CLI `csv-sweep`, and the daemon `POST /api/v1/backtests/jobs`
route (both CSV-backed and `md_bars`-backed workers) — now supports
explicit, opt-in, fail-closed multiplier/margin economics that flow through
to `BacktestReport`, `metrics.json`, `report.md`, and `manifest.json`. The
GUI Backtest Results screen submits the same daemon request shape. No
backtest-only entry point or artifact surface is known to lack economics
support as of this decision.

## 2. Which entry points support economics?

| Entry point | Supports economics? |
|---|---|
| CLI `mqk backtest csv` | Yes |
| CLI `mqk backtest db` | Yes |
| CLI `mqk backtest csv-sweep` | Yes (closed this session by `BACKTEST-MULTIPLIER-MARGIN-01-SAFE-GAP-CLOSURE-01`) |
| Daemon `POST /api/v1/backtests/jobs` (CSV-backed worker) | Yes |
| Daemon `POST /api/v1/backtests/jobs` (`md_bars`-backed worker) | Yes |
| Daemon backtest-sweep jobs | N/A — no such route exists in this repo |
| GUI Backtest Results submit form | Yes |
| GUI sweep | N/A — no sweep feature exists in the GUI |
| Registry-v2 economics-suggestion route (`GET /api/v1/backtests/economics-suggestion`) | Yes, as a read-only suggestion (never auto-applied); reachable end-to-end only when `MQK_INSTRUMENT_REGISTRY_V2_PATH` is explicitly configured — no production (non-example) file exists today |

## 3. Which artifact/report surfaces carry economics?

- `BacktestReport.economics` (in-process) — always populated (`BacktestEconomicsReport::equity()` by default).
- `BacktestReport.run_id` — economics-sensitive (`derive_run_id_with_economics`); two runs with different economics no longer collide on identity.
- `metrics.json` — `economics` object.
- `report.md` — "Contract Multiplier" / margin / "Margin Enforced" rows.
- `manifest.json` — `economics: ManifestEconomics` (`contract_multiplier`, margins, `margin_enforced`, `source`), merged generically inside `write_backtest_report` for every caller including every `csv-sweep` point.

All four surfaces are proven for `csv`, `db`, and `csv-sweep` by
`mqk-cli/tests/scenario_cli_backtest_economics.rs`,
`scenario_cli_backtest_db_economics.rs`, and
`scenario_cli_backtest_csv_sweep_economics.rs`; the daemon job path is
proven by `mqk-daemon/tests/scenario_backtest_jobs_01.rs` (`bj15`/`bj16`).

## 4. Is margin enforced or metadata-only?

**Metadata-only.** `initial_margin_micros`/`maintenance_margin_micros` carry
through every surface above faithfully, but `margin_enforced` is hardcoded
`false` in both `BacktestEconomicsReport::equity()` and `::from_run()`.
**No code path anywhere in this repo reads a margin field to gate, block,
clamp, or otherwise alter backtest behavior.** This has been true and
truthfully documented since the first sub-slice
(`BACKTEST-MULTIPLIER-MARGIN-01-COMBINED`) and remains true after this
session's closure work — margin enforcement was never in scope for this
patch lineage and is not silently claimed here. A future
`BACKTEST-MARGIN-ENFORCEMENT-01`-style patch would need its own explicit
mission and its own scenario-test proof standard before `margin_enforced`
could ever legitimately become `true`.

## 5. Does live/shared portfolio accounting use this?

**No.** `mqk-portfolio/src/accounting.rs` and `mqk-portfolio/src/metrics.rs`
— the same functions `mqk-runtime::orchestrator` calls for live/paper fills
— were not modified by any sub-slice in this lineage (confirmed at Phase A
by direct code read, and unaffected by Phase B's CLI-only change).
`mqk-backtest` is not a `Cargo.toml` dependency of `mqk-runtime`, so there
is no code path — direct or transitive — by which this economics seam can
reach live/paper P&L, NAV, or risk calculations. This boundary is
deliberate and matches the explicit framing in `ASSET-CORE-01H`
(`docs/specs/asset_core_01h_instrument_registry_v2_consumption_boundary_decision.md`):
closing backtest economics does not make live/paper accounting
multiplier-aware.

## 6. What remains open after this closure?

1. **Margin enforcement** — explicitly deferred; metadata-only today, and no patch in this session's scope claims otherwise.
2. **A real, non-equity, production `InstrumentRegistryV2` data source** — the registry-v2 economics-suggestion route's explicit-economics branch is proven only against a disabled, non-tradable, committed example fixture. No production (non-test) non-equity registry file exists, and creating one requires a live-verified non-equity market-data provider (out of this session's hard safety rules — zero network calls).
3. **Live/shared portfolio (`mqk-portfolio`) multiplier awareness** — deliberately out of scope; see §5.
4. **Aggregate sweep-summary files** (`sweep_summary.csv`/`.json`/`.md`) do not carry an economics column — each individual sweep point's `manifest.json`/`metrics.json` does, which was judged sufficient for this closure (the aggregate summary's own schema was left untouched to avoid scope creep beyond the named safe gap).

## 7. Are remaining gaps part of backtest completion, or production/live accounting boundaries?

- Item 1 (margin enforcement) and item 4 (sweep-summary aggregate columns) are backtest-lane items, but neither blocks calling backtest multiplier/margin **economics** support complete — margin enforcement was never claimed as part of this patch's scope, and the aggregate summary gap is a minor reporting nicety with the underlying truthful data already present per-point.
- Items 2 and 3 are **production/consumption boundaries**, not backtest gaps — they belong to `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` and its own named prerequisites (`docs/specs/asset_core_01h_instrument_registry_v2_consumption_boundary_decision.md` §5), not to this patch.

## 8. What next patch does this unblock?

Closing `BACKTEST-MULTIPLIER-MARGIN-01` satisfies **prerequisite #1** of
`ASSET-CORE-01H`'s five-item production-cutover checklist
(`docs/specs/asset_core_01h_instrument_registry_v2_consumption_boundary_decision.md`
§5). It does **not** by itself authorize
`REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01` — prerequisites #2-#5
(symbol/instrument-id translation layer, Gate 0 / routing-guard parity
re-verification, a live-network-verified non-equity market-data provider,
and an explicit operator enablement decision) remain entirely open and
untouched by this session.

---

## 9. Closure verdict

```text
BACKTEST-MULTIPLIER-MARGIN-01 is CLOSED_LOCAL / BACKTEST-COMPLETE for
multiplier-aware backtest economics.

Margin remains metadata-only unless a separate enforcement patch is
created.

Live/shared portfolio accounting remains outside this lane.

This unblocks REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01 from
prerequisite #1, but does not itself authorize production cutover.
```

No config flag was changed, no trading was enabled, no network or DB call
was made, and no broker/execution/risk/OMS/runtime/strategy/portfolio file
was touched by this decision or by the Phase A/B work it summarizes.
