# Paper Trading Shortest Path — 01A — Turnover / Ledger Reconcile

Patch ID: `PAPER-TRADING-SHORTEST-PATH-01A-TURNOVER-LEDGER-RECONCILE-01`
Parent bundle: `PAPER-TRADING-SHORTEST-PATH-AUDIT-01-COMBINED`

Docs-only. No trading behavior changed. No live orders. No paper orders.
No network calls. No DB mutation. No broker/execution/risk/runtime/
strategy/live accounting code touched.

## 1. Current HEAD

`0162beed` (`docs: decide asset core cutover go-no-go`), tip of the
`ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01-COMBINED` bundle
(`eca81772` → `77129dd2` → `68294bd4` → `0162beed`). Working tree clean
against tracked files; only allowed untracked files present
(`MiniQuantDesk_Master_Patch_Ledger_v2_updated.md`, `smoke_logs/`).

## 2. Turnover directives (summary)

The operator's strategic turnover, reconciled against this repo, states:

1. Finish visible paper-trading machinery first — one trustworthy,
   operator-visible paper trade end to end (data → feature → strategy →
   signal → risk → order → ack/fill → position → P&L → operator view)
   before any further scope expansion.
2. Do **not** integrate Vertus now.
3. Do **not** add an AI research analyst now.
4. Do **not** add new strategies merely to feel closer to a trade — the
   repo already has five registered strategy engines
   (`swing_momentum`, `mean_reversion`, `volatility_breakout`,
   `intraday_scalper` long, `intraday_scalper` short —
   `core-rs/crates/mqk-strategy/src/engines/mod.rs`); the blocker is not
   an absence of strategy code.
5. Preserve — but do not build now — the following research concepts for
   a later, explicitly-authorized phase: expectancy framework, random
   baseline gate, MAE/MFE trade stats, exit-policy lab, and a no-edge
   cost gate.

## 3. Current ledger status (verified against `MiniQuantDesk_Master_Patch_Ledger_v2.md` at HEAD)

| Item | Ledger status | Verified location |
|---|---|---|
| `AUTON-NO-TRADE-01` | `CLOSED_LOCAL` (both off-hours and market-hours halves closed) | ledger §ledger line ~1047, ~1189 |
| `AUTON-NO-TRADE-02` | `CLOSED_LOCAL` (Phase A audit → Phase B live market-hours observation → Phase C closure decision) | ledger lines ~1153–1190 |
| `MARKET-HOURS-PROOF-SWEEP-01` | `CLOSED_LOCAL` | ledger line ~1170 |
| `PAPER-SMOKE-FOLLOWUP-01A`–`01E` | `CLOSED_LOCAL`, `01E` also live-validated on 2026-07-09 | ledger lines ~6188–6440 |
| `ASSET-CORE-04-PRODUCTION-CUTOVER-DESIGN-ONLY-01-COMBINED` | `CLOSED_LOCAL / DESIGN-ONLY`; parent `ASSET-CORE-04: PARTIAL / PRODUCTION-CUTOVER-DESIGNED-NOT-AUTHORIZED` | ledger lines ~6735–6754 |
| Strategy/research roadmap (`docs/specs/roadmap_completion_reconcile_01.md`) | Multi-asset items (`CRYPTO-*`, `REGISTRY-V2-PRODUCTION-CUTOVER-DECISION-01`) remain `PARTIAL`/`MISSING`/not-started; equities remain sole production trading truth | `docs/specs/roadmap_completion_reconcile_01.md` §2–§3 |

The single most load-bearing fact for this mission: the most recent live
market-hours evidence (`AUTON-NO-TRADE-02B`, 2026-07-09 15:00–15:12 UTC)
shows a **real** strategy evaluation
(`intraday_scalper`/AAPL/5m, `flat_below_threshold`, `move_bps=-19` vs
`threshold_bps=20`) that stayed flat by a 1-bps margin — not a code gap,
not a fabricated result. A second live session the same day
(`PAPER-SMOKE-FOLLOWUP-01E`'s live validation, 17:52–20:00 UTC) instead
never reached strategy evaluation at all: `DATA-FRESHNESS-READINESS-GATE-01`
correctly fired `bar_data_stale`/`intraday_bar_stale` on every dispatch
tick, tracing to `all_passed=false` /
`reason_code="provider_returned_stale_intraday_data"` on the intraday
refresh evidence — an already-documented sandbox artifact (this sandbox's
system clock runs materially ahead of TwelveData's real-world data), not a
code defect.

## 4. Classification of turnover concepts

**Required for one visible paper trade (active path):**
- Nothing new. The full durable chain (outbox → broker submit → ack/fill →
  inbox → portfolio) and every operator-visible route named in the mission
  (`execution/signal-evaluations`, `execution/summary`, `execution/flow`,
  `execution/orders`, `autonomous/no-trade-diagnostics`,
  `autonomous/readiness`) already exist and are already wired (see
  `paper_trading_shortest_path_01b_current_lifecycle_truth_audit.md`).
  What remains is a live-observation proof during a market-hours window
  where data freshness holds and the strategy's real move happens to
  cross its threshold — not new code.

**Research backlog (explicitly parked, not started by this patch):**
- Random baseline gate
- MAE/MFE trade stats
- Exit-policy lab
- No-edge cost gate
- Position-sizing stress
- AI research analyst boundary design

**Feature backlog (explicitly parked, not started by this patch):**
- Vertus integration (any form)
- New strategy engines beyond the five already registered
- Multi-asset (crypto/futures/options/forex) trading enablement —
  `ASSET-CORE-04` production cutover remains
  `PRODUCTION-CUTOVER-DESIGNED-NOT-AUTHORIZED`; `CRYPTO-REGISTRY-01`,
  `CRYPTO-DATA-01`, `CRYPTO-RISK-01`, `CRYPTO-EXEC-01`, `CRYPTO-STRAT-01`
  remain `PARTIAL`/`MISSING` per `roadmap_completion_reconcile_01.md`

**Watchlist only (no ledger action needed beyond acknowledgment):**
- Vertus — explicitly watchlist-only per the turnover; no ledger item
  exists or is created for it by this patch.

## 5. Explicit non-authorization statement

This patch does not authorize, start, or schedule: AI/LLM/Vertus
integration work, any new strategy engine, or any multi-asset
(crypto/futures/options/forex/rates) trading enablement. Per the
turnover's own directive, no new strategy should be added now — the
existing five registered engines are sufficient to prove the loop. No
trading behavior changed. No live orders. No paper orders.

## 6. Backlog items — duplication check before any addition

Per this bundle's own scope, backlog items are added **only if missing**
from the current ledger/roadmap docs. Checked each candidate against
`MiniQuantDesk_Master_Patch_Ledger_v2.md` (full-file grep) and
`docs/specs/roadmap_completion_reconcile_01.md`:

| Candidate backlog ID | Already exists under a different name? | Action |
|---|---|---|
| `AI-RESEARCH-ANALYST-BOUNDARY-DESIGN-ONLY-01` | No — no AI/LLM/analyst-boundary item found anywhere in the ledger. | Add as `QUEUED / PARKED`, future-only, no scope until explicitly authorized. |
| `RANDOM-BASELINE-GATE-01` | No — no random-baseline/null-hypothesis gate item found. | Add as `QUEUED / PARKED`. |
| `MAE-MFE-TRADE-STATS-01` | No — no MAE/MFE trade-stats item found. (Backtest reporting exists via `BACKTEST-COMPLETION-BUNDLE-01`, but it does not compute MAE/MFE.) | Add as `QUEUED / PARKED`. |
| `EXIT-POLICY-LAB-01` | No — exit logic exists only inside individual strategy engines; no dedicated exit-policy research lab item found. | Add as `QUEUED / PARKED`. |
| `POSITION-SIZING-STRESS-01` | No — `STRATEGY-POSITION-SIZING-01` (commit `f83ca51`) added static sizing caps, and `STRATEGY-EQUITY-PERCENT-SIZING-01` (`OPEN`, ledger line ~1200) is queued for percent-of-equity sizing, but neither is a stress-test item. Not a duplicate. | Add as `QUEUED / PARKED`. |
| `NO-EDGE-COST-GATE-01` | No — no cost/slippage/no-edge gate item found. | Add as `QUEUED / PARKED`. |

All six are added to the ledger's backlog section (§6 below, ledger
update) as `QUEUED / PARKED` with a one-line pointer to this reconcile
doc — no implementation scope, no design work, no code.

## 7. Answers to pre-flight inspection questions

1. **Already proven closed:** off-hours and market-hours durable no-trade
   explanation (`AUTON-NO-TRADE-01`/`02`); intraday freshness gating
   (`INTRADAY-MD-FRESHNESS-AUTONOMOUS-01`, `INTRADAY-MD-REFRESHER-01`,
   `INTRADAY-MD-PROVIDER-FRESHNESS-TRUTH-01-COMBINED`); operator
   visibility routes for signal evaluations, execution summary/flow/
   orders, autonomous readiness/diagnostics, and intraday refresh status;
   single-symbol smoke path (`PAPER-SMOKE-FOLLOWUP-01C`); a full live
   single-symbol session running end-to-end without crashing or
   mis-gating (`PAPER-SMOKE-FOLLOWUP-01E` live validation).
2. **Proven by market-hours evidence but no order produced:** the full
   dispatch path through strategy evaluation — `AUTON-NO-TRADE-02B`'s
   `move_bps=-19` vs `threshold_bps=20` run reached real strategy
   evaluation and stayed flat by design.
3. **Exact reason the last real market-hours run did not trade:** two
   distinct sessions the same day showed two distinct reasons — (a) the
   15:00–15:12 UTC session reached strategy evaluation and the market
   move (-19 bps) did not cross the 20-bps threshold; (b) the 17:52–20:00
   UTC session never reached strategy evaluation because
   `DATA-FRESHNESS-READINESS-GATE-01` correctly fail-closed on stale
   intraday bars every tick (sandbox-clock-vs-provider-clock skew).
4. **Current blocker category:** a combination of (i) data freshness
   reliability in this sandbox (intermittent, not a code defect) and
   (ii) ordinary market variance — the strategy's fixed 20-bps threshold
   is tight enough that a real intraday move can miss it by 1 bps. Not a
   strategy-absence, risk-denial, OMS/outbox, broker-submit, broker-ack/
   fill, position-accounting, or operator-visibility gap — see
   `paper_trading_shortest_path_01b_current_lifecycle_truth_audit.md` for
   the full per-row classification.
5. **Safe way to force a paper order:** none found in production code.
   Grep of `mqk-daemon/src` for force/debug/manual order or fill injection
   patterns returns no matches. No test-only backdoor exists in daemon
   state. This is correct fail-closed behavior per `CLAUDE.md` and is not
   something this patch proposes adding.
6. **Known strategy likely to generate a signal under realistic current
   data:** `intraday_scalper` (armed via `MQK_STRATEGY_IDS` at runtime,
   env-configured, not read by this audit — `.env.local` is out of
   scope). It already came within 1 bps of firing on real market data.
   No threshold or strategy-logic change is proposed by this patch —
   changing it would be a scope/safety violation per this bundle's rules.
7. **Signal/order/fill/position/P&L visibility routes:** yes, all exist
   — see §1 lifecycle matrix in `01B`.
8. **GUI lifecycle exposure:** partial — GUI panels exist for several
   lifecycle stages (intraday refresh status, watchlist status, etc.);
   full end-to-end order/fill/P&L GUI coverage is assessed in `01B`.
9. **Exact minimum next patch or market-hours proof:** determined in
   `paper_trading_shortest_path_01c_minimum_blocker_chain.md`.
10. **Turnover ideas already implemented, partial, missing, or parked:**
    see §4 above.

## 8. Safety confirmation

No live orders. No paper orders. No trading behavior changed. No network
calls. No DB mutation. No broker/execution/risk/runtime/strategy/live
accounting code touched. This patch is docs/ledger-reconcile only.
