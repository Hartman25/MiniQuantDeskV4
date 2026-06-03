# EXP-PENNY-01 — Experimental Breakout/Accumulation Engine

**Status: EXPERIMENTAL / NOT LIVE-READY**
**Lane: EXP (research only; not operational truth)**
**Promotion gate: NONE YET — scanner-only phase required before paper execution**
**Foundation: EXP-ENGINE-CORE-01 (DONE) — scanner base, candidate journal, and config gate exist**

---

## Purpose

EXP-PENNY-01 defines a future paper-trading-only engine that scans small-cap and
penny stocks for breakout setups following a period of measured accumulation.

This document is a **specification and planning artifact only**.
No execution code exists. No routing is wired. No live or paper orders will be
placed by this lane until it passes all promotion gates defined below.

---

## Hard Boundaries (enforced before any future code lands)

- **Paper-only.** No live deployment until explicit separate live readiness review.
- **No shorts by default.** Long-side breakouts only unless explicitly re-scoped.
- **Completely separate from the main engine.** No shared execution paths with
  the canonical `intraday_scalper` or any MAIN-lane strategy.
- **No auto-activation.** Must be explicitly enabled by a dedicated config flag
  that is absent (or `false`) in all default config files.
- **Scanner-first.** The engine must log candidate signals for review before any
  paper order is placed. Stage 1 is candidate journaling, not execution.
- **No low-liquidity entries.** Hard stop on any symbol that fails the minimum
  ADV/spread gates.
- **No trading-halted symbols.** Symbol halt flags must be checked before entry.
- **No market orders when spread is too wide.** Limit orders only above the
  max-spread threshold.

---

## Four-Stage Validation Model

| Stage | Name | Action | Gate to advance |
|-------|------|---------|-----------------|
| 1 | Scanner-only candidate journal | Scan + log candidates; no orders | 30-day journal with 50+ unique candidates reviewed |
| 2 | Historical replay/backtest | Run signal logic against history | Gate from `docs/specs/backtest_policy.md`; min 200 trades |
| 3 | Paper-only sandbox execution | Paper orders through broker adapter | 60-day paper run; reconcile clean; no anomalies |
| 4 | Evidence review | Human review of full journal + backtest + paper results | Operator sign-off required |

Stage 5 would be **live shadow** and Stage 6 **live** — both require a separate
future promotion review outside this document's scope.

---

## Candidate Criteria (Scanner Logic)

### Price and Liquidity Filters

| Filter | Parameter | Default | Notes |
|--------|-----------|---------|-------|
| Price range | `min_price` / `max_price` | $0.50 – $20.00 | Configurable per run; excludes sub-penny |
| Average dollar volume (20d) | `min_adv_usd` | $500,000 | Minimum 20-day average dollar volume |
| Minimum daily volume (shares) | `min_daily_vol_shares` | 200,000 | Guard against illiquid thinly-traded days |
| Bid/ask spread | `max_spread_pct` | 1.0% | Measured at scan time; reject if wider |
| Float size | `min_float_shares` | 5,000,000 | Exclude nano-float stocks prone to manipulation |
| Sector exclusion | `excluded_sectors` | OTC bulletin board, shell cos | Configurable list |

### Trend Filters

| Filter | Parameter | Default | Notes |
|--------|-----------|---------|-------|
| MA200 direction | `ma200_slope_min` | > 0.0 (rising) | 200-day simple moving average slope over 20 sessions |
| Price vs MA200 | `price_vs_ma200_min_pct` | price >= MA200 * 0.85 | Must be near or above long-term average |
| MA50 direction | `ma50_slope_min` | > 0.0 (rising) | Intermediate trend must also be up |

### Accumulation/Consolidation Filters

| Filter | Parameter | Default | Notes |
|--------|-----------|---------|-------|
| Consolidation range width | `max_consolidation_range_pct` | 15% | High – Low over consolidation window / midpoint |
| Consolidation window | `consolidation_days` | 10 – 30 | Days in tight range preceding breakout |
| Relative volume increase (pre-breakout) | `rvol_base_threshold` | >= 1.2x | Volume trend rising within consolidation period |

### Breakout Trigger Filters

| Filter | Parameter | Default | Notes |
|--------|-----------|---------|-------|
| Breakout volume | `breakout_rvol_min` | >= 2.0x | Breakout day volume vs 20d average |
| Breakout price confirmation | `breakout_close_above_range` | true | Must close above consolidation high |
| Gap risk flag | `reject_gap_breakouts` | true (default: reject) | Avoid chasing gap-up breakouts; prefer intraday |
| News-risk flag | `reject_news_catalyst` | false (not checked by default; future data dependency) | Requires news API integration not yet present |
| Halt history flag | `reject_recent_halt` | true | Reject symbols with a trading halt in last 30 days |

---

## Hard Risk Gates (apply to any future paper execution stage)

| Gate | Value | Notes |
|------|-------|-------|
| Paper-only enforcement | required | `MQK_PENNY_ENGINE_LIVE_ALLOWED=false` must be absent to prevent live activation |
| Max position notional | `max_position_notional_usd` | $2,000 per position (configurable; default conservative) |
| Max daily loss cap | `max_daily_loss_usd` | $500 (configurable; hard stop for paper session) |
| Max open positions | `max_open_positions` | 3 simultaneous |
| Max trades per day | `max_trades_per_day` | 5 |
| No shorts | `allow_shorts=false` | Default; explicit opt-in to enable |
| Spread gate at entry | `entry_max_spread_pct` | 0.5% at order time (tighter than scan gate) |
| Market order guard | `order_type=limit_only` | No market orders unless explicitly enabled |
| Halt check at entry | required | Live halt status re-checked at order time, not just at scan |

---

## Trade Candidate Journal Schema

Each candidate generated by the scanner (Stage 1) must log the following fields.
This is the authoritative journal record even before any paper order exists.

```json
{
  "schema_version": "exp-penny-candidate-v1",
  "scanned_at_utc": "<ISO8601>",
  "symbol": "<ticker>",
  "exchange": "<exchange>",
  "price_at_scan": "<decimal>",
  "bid_at_scan": "<decimal>",
  "ask_at_scan": "<decimal>",
  "spread_pct_at_scan": "<decimal>",
  "volume_at_scan": "<int>",
  "adv_20d_shares": "<int>",
  "adv_20d_usd": "<decimal>",
  "rvol_at_scan": "<decimal>",
  "ma200_value": "<decimal>",
  "ma200_slope_20d": "<decimal>",
  "ma50_value": "<decimal>",
  "ma50_slope_20d": "<decimal>",
  "consolidation_high": "<decimal>",
  "consolidation_low": "<decimal>",
  "consolidation_range_pct": "<decimal>",
  "consolidation_days": "<int>",
  "breakout_level": "<decimal>",
  "breakout_rvol": "<decimal>",
  "entry_thesis": "<string>",
  "invalidation_level": "<decimal>",
  "gap_flag": "<bool>",
  "halt_flag": "<bool>",
  "news_flag": "<bool | null>",
  "risk_cap_usd": "<decimal>",
  "stage": "candidate | rejected | paper_order_placed | outcome_recorded",
  "rejection_reason": "<string | null>",
  "paper_order_id": "<uuid | null>",
  "outcome": "<null | win | loss | scratch>",
  "outcome_pnl_usd": "<decimal | null>",
  "mae_pct": "<decimal | null>",
  "mfe_pct": "<decimal | null>",
  "holding_period_bars": "<int | null>",
  "notes": "<string>"
}
```

---

## Architecture Separation Requirements

- EXP-PENNY-01 must live in its own module/crate (e.g. `mqk-exp-penny` or
  `research-py/exp/penny_scanner/`).
- It must NOT import or depend on the live execution orchestrator directly.
- It must NOT share the `oms_outbox`/`oms_inbox` tables with the main engine
  unless a new isolated table set is added via migration with explicit scoping.
- Any paper order generated by EXP-PENNY-01 must be tagged with
  `source=exp_penny_01` to prevent reconcile cross-contamination.
- The scanner output path must be a file or separate DB table with no read path
  from the main execution loop.

---

## Future Patch Lane IDs

| ID | Description | Status |
|----|-------------|--------|
| EXP-ENGINE-CORE-01 | Experimental engine foundation (scanner base, config gate, candidate journal) | DONE |
| EXP-PENNY-01A | Scanner-only implementation (Stage 1 journal, no orders) | OPEN |
| EXP-PENNY-01B | Backtest replay harness (Stage 2) | OPEN |
| EXP-PENNY-01C | Paper execution wiring (Stage 3; requires Stage 2 gate pass) | OPEN |
| EXP-PENNY-01D | Evidence review tooling and promotion checklist | OPEN |
| EXP-PENNY-01-LIVE | Live promotion (blocked until Stage 4 operator sign-off) | OPEN |

---

## Explicit Non-Goals

- Does not affect AAPL/5m intraday_scalper.
- Does not change the broker adapter or WS inbound path.
- Does not change OMS, reconcile, or risk logic.
- Does not require new API keys (news feed key would be optional future dependency).
- Does not add any new default config that enables penny trading.
- Does not generate GUI readiness claims.
