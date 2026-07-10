# Paper Trading Shortest Path — 01B — Current Lifecycle Truth Audit

Patch ID: `PAPER-TRADING-SHORTEST-PATH-01B-CURRENT-LIFECYCLE-TRUTH-AUDIT-01`
Parent bundle: `PAPER-TRADING-SHORTEST-PATH-AUDIT-01-COMBINED`

Docs-only. No trading behavior changed. No live orders. No paper orders.
No network calls. No DB mutation. No broker/execution/risk/runtime/
strategy/live accounting code touched. Grounded against current HEAD
(`1a42e9af`, descendant of `0162beed`) by direct source/route/table read,
not by re-trusting prior closure docs.

## 1. Lifecycle matrix

| # | Stage | Classification | Evidence | Durable or live-only | Market hours required | Blocks one paper trade? |
|---|---|---|---|---|---|---|
| 1 | Authoritative current market data | **PARTIAL** | `md_bars` schema + `Refresh-IntradayMarketData.ps1` + `GET /api/v1/market-data/intraday-refresh/status` (`core-rs/crates/mqk-daemon/src/routes/transport_quality.rs`) all exist and are wired. `DATA-FRESHNESS-READINESS-GATE-01` correctly fail-closes on stale bars. Live evidence (2026-07-09 17:52–20:00 UTC session) shows `all_passed=false` / `reason_code="provider_returned_stale_intraday_data"` for the full session — a documented sandbox-clock-vs-provider-clock skew, not a code defect. | Live-only (freshness truth is only proven per-session) | Yes | **Yes — this is the primary intermittent blocker** |
| 2 | Feature calculation | **CLOSED** | `intraday_scalper` (`core-rs/crates/mqk-strategy/src/engines/intraday_scalper.rs`) computes `move_bps`/`abs_move_bps`/`gap_to_threshold_bps` deterministically from completed bars; unit-tested (`OBS-D01`–`D08`). | Durable (pure, tested) | No | No |
| 3 | Strategy evaluation | **CLOSED (proven live)** | `AUTON-NO-TRADE-02B` (2026-07-09 15:00–15:12 UTC) recorded a real evaluation: `flat_below_threshold`, `move_bps=-19` vs `threshold_bps=20`, written to `strategy_signal_evaluations`. | Live-proven once; durable code path | Yes (for a fresh proof) | No — code proven; only needs a session where data is fresh |
| 4 | Signal generated and recorded | **CLOSED** | `GET /api/v1/execution/signal-evaluations` (`core-rs/crates/mqk-daemon/src/routes/execution.rs:744`, `execution_signal_evaluations`) reads `strategy_signal_evaluations` and returned rows in the live `02B` observation. | Durable route, live-proven | No (route itself) | No |
| 5 | Risk evaluation | **CLOSED** | Gate 0 / routing guard / asset-risk-policy (`ASSET-CORE-03`) run on every dispatch attempt regardless of outcome; `autonomous_no_trade_diagnostics` rows from `02B` show `paper_order_attempted=false` with explicit reason, proving the gate chain executes even when strategy stays flat. | Durable, tested; live-proven for the no-signal path only | No | No — only the signal→risk transition itself is unproven live (see row 6) |
| 6 | Paper order submitted | **UNPROVEN LIVE / MARKET-HOURS-PROOF-REQUIRED** | Outbox enqueue path (`mqk-execution`/`mqk-runtime` orchestrator, Phase 1 of the tick per `execution_rules.md`) exists and is tested in scenario suites, but no live session to date has produced a nonzero `oms_outbox` row for this repo's single-symbol AAPL config — both `02B` and the `01E` live validation ended with zero orders (flat or gated on stale data). | Durable code, zero live proof | Yes | **Yes — the first unproven live seam** |
| 7 | Broker acknowledgment | **UNPROVEN LIVE (this repo)** | `mqk-broker-alpaca` ack handling is scenario-tested against fixture WS frames; no live paper order has been submitted from this repo to produce a real ack. | Durable code, zero live proof for this repo's current config | Yes (downstream of row 6) | Yes — inherits row 6's gap |
| 8 | Fill received | **UNPROVEN LIVE (this repo)** | Same as row 7 — inbox apply / fill normalization is scenario-tested (deterministic `trade_update_message_id`, `broker_rules.md`-compliant), never exercised against a live fill for this repo's current config. | Durable code, zero live proof | Yes (downstream of row 6) | Yes — inherits row 6's gap |
| 9 | Position/accounting state updated | **CLOSED (code), UNPROVEN LIVE** | `mqk-portfolio::accounting.rs` position/ledger update path is scenario-tested including idempotent double-apply proof (`ASSET-CORE-04B-LIVE-ACCOUNTING-INVARIANT-PROOF-01`). Never exercised end-to-end from a real live fill in this repo. | Durable, tested; zero live proof | Yes (downstream) | Inherits row 6's gap |
| 10 | Realized/unrealized P&L updated | **PARTIAL** | `realized_pnl_micros` is computed and invariant-tested (`state/snapshot.rs:1204`, "INVARIANT_VIOLATED" assertion recomputes from ledger) but is surfaced through the diagnostic `routes/repair.rs` path, not the primary operator P&L view. `GET /api/v1/portfolio/positions` and `/portfolio/summary` (`routes/portfolio.rs`) always return `unrealized_pnl: None`, `mark_price: None`, `daily_pnl: None` — intentionally honest (per code comment in `api_types.rs:3059`, "the data source does not exist yet"), not fabricated, but not wired for operator P&L visibility either. | Durable (realized) / not implemented (unrealized on primary route) | No (structural gap, not session-dependent) | Not a blocker to the order/fill loop itself, but blocks calling the loop "P&L-visible" |
| 11 | Full lifecycle visible to operator | **PARTIAL** | `execution/summary`, `execution/orders`, `execution/flow` (`routes/execution.rs`, `routes/execution_flow.rs`), `autonomous/readiness`, `autonomous/no-trade-diagnostics` (`routes/autonomous_paper_status.rs`, `routes/system.rs`) all exist and are wired; GUI has a `PortfolioScreen.tsx` and execution types consuming several of these. Per-position unrealized/daily P&L is not surfaced (row 10). | Durable routes, live-proven for no-trade path only | No (route existence); Yes (for a full order→fill→P&L proof) | No, but incomplete for the P&L dimension |

## 2. Answers to required audit questions

1. **Is the shortest blocker still "strategy did not generate a signal"?**
   No — that framing is now stale. The strategy *did* generate a real
   evaluation in the most recent proven session and came within 1 bps of
   crossing its threshold. The two live sessions on 2026-07-09 show the
   actual blocker is upstream (data freshness intermittently prevents
   evaluation entirely) or is simply that no live session yet has
   coincided with both fresh data *and* a real move ≥ 20 bps. There is no
   evidence of a code-level signal-generation gap.

2. **Is the strategy no-signal state legitimate under current threshold
   logic?** Yes. `MICRO_MOVE_BPS = 20` (`intraday_scalper.rs:222`) is a
   fixed, tested, non-fabricated constant. The `-19` vs `20` result in
   `02B` is real market data producing a real near-miss, not a gating
   artifact.

3. **Would changing strategy thresholds be a scope/safety violation?**
   Yes, per this bundle's hard rules ("no strategy threshold changes")
   and `CLAUDE.md`'s patch-discipline invariants. Any threshold change
   must be its own separately-scoped, explicitly-authorized patch — not
   inferred here.

4. **Is current market data authoritative enough for one paper-trading
   proof?** Conditionally. The freshness gate is correct and fail-closed;
   the blocker is that this sandbox's real-world data lag makes
   `all_passed=true` freshness intermittent rather than guaranteed. A
   session run when TwelveData's real-world data catches up to the
   sandbox clock (or a session timed so the staleness window doesn't
   straddle the whole run) is authoritative. This is an operational
   timing condition, not a code gap.

5. **Is a paper order/fill lifecycle already proven by tests but not by
   current live smoke?** Yes — exactly. Outbox submit, broker ack/fill
   normalization, inbox apply, and portfolio update are all scenario-
   tested (durable, deterministic) per `execution_rules.md`/
   `broker_rules.md`/`db_rules.md` discipline, but zero live session for
   this repo's current single-symbol AAPL config has produced a real
   `oms_outbox` row, broker ack, or fill. This is the first genuinely
   unproven-live seam (lifecycle matrix rows 6–9).

6. **What is the first unknown or unproven lifecycle seam after the
   latest market-hours proof?** Row 6 — paper order submission. Every
   upstream stage (data → feature → strategy → signal → risk) has now
   been live-observed at least once; every downstream stage (ack → fill →
   position → P&L) is blocked on row 6 happening first.

7. **Which seam should be patched next?** None require a *code* patch —
   the seam is a live-observation gap, not a missing capability. The next
   action is a bounded, market-hours paper-smoke observation window,
   *not* a new code patch, unless that observation surfaces a genuine
   code defect (per this bundle's own gating pattern, matching how
   `AUTON-NO-TRADE-02` was closed). See
   `paper_trading_shortest_path_01c_minimum_blocker_chain.md` for the
   exact next-patch recommendation.

## 3. Safety confirmation

No live orders. No paper orders. No trading behavior changed. No network
calls. No DB mutation. No broker/execution/risk/runtime/strategy/live
accounting code touched. This patch is docs-only.
