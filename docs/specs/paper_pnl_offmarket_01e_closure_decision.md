# PAPER-PNL-OFFMARKET-01E — Closure Decision

Patch group: `PAPER-PNL-OFFMARKET-COMPLETION-01-COMBINED`, Phase E
(final). Docs-only.

## 1. Is `PAPER-PNL-OFFMARKET-COMPLETION-01-COMBINED` closed?

```text
PAPER-PNL-OFFMARKET-COMPLETION-01-COMBINED: CLOSED_LOCAL
```

Yes. All three mission objectives are met: (1) `timeframe` query-param
support added to `/api/v1/portfolio/positions` and
`/api/v1/portfolio/summary`, (2) proven with DB-backed route tests and
read-only DB readback, (3) a design-only daily-P&L baseline plan produced
without implementing it.

## 2. Is `PAPER-PNL-01F-TIMEFRAME-SELECTION-01` closed?

```text
PAPER-PNL-01F-TIMEFRAME-SELECTION-01: CLOSED_LOCAL
```

Yes — this is the exact "optional" follow-up the prior patch group's Phase
E entry named as `PAPER-PNL-01F-TIMEFRAME-SELECTION-01`. Both routes now
accept `?timeframe=<value>`, default unchanged at `"1D"`.

## 3. Is mark/unrealized-P&L now practically usable for `5m` marks?

Yes. `?timeframe=5m` resolves the real proof-02 `AAPL qty=3
avg_price=314.81` position shape to `mark_price=$314.86`,
`mark_source="md_bars:5m:close"`, `unrealized_pnl≈$0.15` — proven by
DB-backed scenario test `ppv11_timeframe_5m_resolves_mark_and_pnl_when_only_5m_bar_exists`
(Phase B, commit `6c3f976d`), which ran for real against
`mqk-paper-postgres`, and independently re-confirmed against the real
`AAPL` rows via read-only `psql` readback (Phase C, commit `f99dc5f9`).

## 4. Does default `"1D"` remain backward compatible?

Yes. `DEFAULT_POSITIONS_PNL_TIMEFRAME = "1D"` is unchanged;
`selected_positions_pnl_timeframe` falls back to it whenever the query
param is absent or blank. Proven by `ppv10_default_no_query_still_resolves_1d`
(no-query default still resolves `1D`), `ppv12_same_symbol_default_1d_is_mark_unavailable_when_only_5m_bar_exists`
(same symbol, default query, still `mark_unavailable` — the default did
not silently start resolving `5m`), and `ppv14_blank_timeframe_query_param_defaults_to_1d`
(blank `?timeframe=` defaults to `1D`).

## 5. Does daily P&L remain open?

```text
PAPER-PNL-OPERATOR-VISIBILITY-CLOSURE-01-COMBINED: MARK-AND-UNREALIZED-PNL-PRACTICALLY-USABLE-WITH-TIMEFRAME-SELECTION / DAILY-PNL-BASELINE-OPEN
PAPER-TRADE-LIFECYCLE-PROOF-02: PNL-SEAM-CLOSED-FOR-MARK-AND-UNREALIZED-PNL / DAILY-PNL-BASELINE-OPEN
```

Yes, `daily_pnl` remains permanently unavailable on
`/api/v1/portfolio/summary` (`daily_pnl_unavailable_reason =
"no_day_start_equity_baseline_in_schema"`), unchanged by this bundle. No
day-start/previous-close equity baseline was created, and none was
supposed to be per this bundle's explicit scope.

## 6. Was daily-P&L baseline implemented?

No. Phase D (`docs/specs/paper_daily_pnl_baseline_design_only_01.md`) is a
design document only — no code, no route change, no new table.

## 7. Was any DB migration added?

No. Zero migration files were added or modified across all five phases.
The `md_bars` table's existing `timeframe` column already supported this
fix; only the query-argument value passed to
`fetch_recent_completed_bars_for_strategy` changed.

## 8. Were any provider/broker/network calls made in tests?

No. All 13 tests in `scenario_paper_pnl_operator_visibility_01.rs`
(including the 5 new PPV-10..PPV-14) are either fully in-process
(`tower::ServiceExt::oneshot`, no network) or DB-backed against the local
`mqk-paper-postgres` instance via `sqlx::PgPool` — no broker adapter, no
provider client, no external network call anywhere in the test file.

## 9. Were any orders submitted?

No. Zero order submission, zero broker contact, at any phase. Phase B's
existing `ppv09_routes_make_no_outbox_writes` test (unmodified, still
passing) continues to prove both routes make zero writes to `oms_outbox`.

## 10. Were any thresholds/gates/config changed?

No. Zero strategy, risk, OMS, broker, reconcile, gate, `.env.local`, or
config-flag changes across all five phases. The only source-code change
in the entire bundle is the `timeframe` query-param plumbing in
`core-rs/crates/mqk-daemon/src/routes/portfolio.rs` (Phase B) plus its
tests.

## 11. What exact next market-hours proof should be run when the market opens?

```text
PAPER-TRADE-LIFECYCLE-PROOF-03-PNL-VISIBILITY-VERIFY-COMBINED
```

Rebuild and restart the daemon with the Phase B binary, then during market
hours call `GET /api/v1/portfolio/positions?timeframe=5m` and `GET
/api/v1/portfolio/summary?timeframe=5m` against a real live paper position
to confirm the patched binary's live response matches this bundle's
DB-backed test proof (Phase C §3-§5 documents the expected values from
the last-known AAPL state; a fresh live position may differ).

## 12. What exact next off-market patch is recommended?

```text
PAPER-DAILY-PNL-BASELINE-01-COMBINED
```

Implements the design from
`docs/specs/paper_daily_pnl_baseline_design_only_01.md`: a
`sys_account_equity_baseline` table, a previous-session-close capture
mechanism using the existing `market_calendar.rs` trading-day seam, and
route-level `daily_pnl` truth-state vocabulary.

## 13. Full patch-group commit chain

Phase A `a069fff4` (timeframe gap audit) → Phase B `6c3f976d` (query-param
+ tests) → Phase C `f99dc5f9` (DB readback / test proof) → Phase D
`a91e495d` (daily-P&L baseline design-only) → Phase E (this entry,
closure).

## 14. Safety confirmation

No live orders. No forced paper orders. No strategy/threshold changes. No
gate weakened. No fabricated marks/P&L at any phase — every mark still
comes from a real completed `md_bars` row; `mark_unavailable`/
`db_unavailable` still fire whenever no real row exists at the selected
timeframe. No generated evidence staged. No `.env.local` edit. No config
flag change. No DB migration added. No daemon started or restarted at any
phase.
