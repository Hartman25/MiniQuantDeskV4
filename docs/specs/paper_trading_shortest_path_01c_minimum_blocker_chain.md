# Paper Trading Shortest Path — 01C — Minimum Blocker Chain

Patch ID: `PAPER-TRADING-SHORTEST-PATH-01C-MINIMUM-BLOCKER-CHAIN-01`
Parent bundle: `PAPER-TRADING-SHORTEST-PATH-AUDIT-01-COMBINED`

Docs-only. No trading behavior changed. No live orders. No paper orders.
No network calls. No DB mutation. Built directly from
`paper_trading_shortest_path_01b_current_lifecycle_truth_audit.md`'s
11-row matrix — no new evidence gathered, no re-derivation.

## 1. Minimum blocker chain

```text
CURRENT HEAD (19f93a66)
    ↓
BLOCKER 1: Data-freshness reliability window
    Not a code gap — DATA-FRESHNESS-READINESS-GATE-01 (mqk-runtime
    orchestrator, per-tick) is correct and fail-closed. The blocker is
    operational: this sandbox's system clock runs materially ahead of
    TwelveData's real-world provider data, so a full session can see
    all_passed=false / reason_code=provider_returned_stale_intraday_data
    for its entire duration (proven: PAPER-SMOKE-FOLLOWUP-01E live
    validation, 2026-07-09 17:52-20:00 UTC). A session timed so real-world
    provider data has caught up avoids this. No patch required — this is
    a scheduling/observation condition.
    Files/routes: scripts/windows/Refresh-IntradayMarketData.ps1,
    GET /api/v1/market-data/intraday-refresh/status
    (core-rs/crates/mqk-daemon/src/routes/transport_quality.rs)
    ↓
BLOCKER 2: Market-move-vs-threshold coincidence
    Not a code gap — intraday_scalper's MICRO_MOVE_BPS=20 fixed threshold
    (core-rs/crates/mqk-strategy/src/engines/intraday_scalper.rs:222) is
    tested and correct. The blocker is that no live session to date has
    coincided with both fresh data AND a real ≥20bps intraday move for
    AAPL/5m. Closest proven miss: move_bps=-19 (AUTON-NO-TRADE-02B,
    2026-07-09 15:00-15:12 UTC). Changing the threshold is explicitly
    out of scope (scope/safety violation per this bundle's rules) — the
    only remaining action is another live observation window.
    Files/routes: GET /api/v1/execution/signal-evaluations
    (core-rs/crates/mqk-daemon/src/routes/execution.rs:744)
    ↓
BLOCKER 3: Live proof of paper order → ack → fill → position → P&L chain
    Not a code gap — outbox/broker-submit/ack/fill/inbox/portfolio-apply
    are all scenario-tested per execution_rules.md/broker_rules.md/
    db_rules.md discipline (including an idempotent double-apply proof,
    ASSET-CORE-04B-LIVE-ACCOUNTING-INVARIANT-PROOF-01). Zero live
    exercise exists for this repo's current single-symbol AAPL config,
    because Blockers 1-2 have so far prevented a signal from ever
    reaching risk-approved order submission in a live session. This
    blocker resolves automatically once Blockers 1-2 clear in the same
    session — no independent code work is needed here either.
    Files/routes: oms_outbox / oms_inbox tables, mqk-broker-alpaca,
    mqk-portfolio::accounting.rs
    ↓
MARKET DATA RECEIVED
    ↓
STRATEGY EVALUATED
    ↓
SIGNAL GENERATED
    ↓
RISK APPROVED
    ↓
PAPER ORDER SUBMITTED
    ↓
ACK/FILL RECEIVED
    ↓
POSITION + P&L UPDATED
    ↓
FULL LIFECYCLE VISIBLE
    (with one known structural gap: per-position unrealized/daily P&L is
    not yet surfaced on the primary portfolio/summary or
    portfolio/positions routes — realized P&L is computed and
    invariant-proven at the ledger level but only exposed through the
    routes/repair.rs diagnostic surface, per 01B row 10. This does not
    block a first visible trade — order/fill/position state and realized
    P&L truth are both provable — but it is the next gap after that
    proof lands.)
```

## 2. Required answers

1. **Shortest path that does not require new asset classes:** Confirmed
   above. Every blocker and every route in the chain is equity-only
   (AAPL/5m, already-registered `intraday_scalper`). No `CRYPTO-*` or
   `ASSET-CORE-04` production-cutover item appears anywhere in the chain.

2. **Shortest path that does not require AI:** Confirmed. No AI/LLM/
   Vertus component appears in the chain; nothing in Blockers 1-3 needs
   one.

3. **Shortest path that does not require strategy-library expansion:**
   Confirmed. The existing `intraday_scalper` strategy is sufficient —
   it already produced a real, near-miss evaluation on live data. No new
   strategy engine is named or needed anywhere in the chain.

4. **Is a strategy-research patch required before a paper trade can occur
   naturally?** No. All three blockers are observation/timing conditions,
   not missing capability. A strategy-research patch (threshold tuning,
   new signal logic, etc.) is explicitly out of scope for reaching one
   paper trade and would be a scope violation if attempted here.

5. **Is a market-hours proof required?** Yes — for all three blockers.
   None of them can be resolved by further docs/audit work; each requires
   a live NYSE regular-session observation window where fresh intraday
   data is available for the session's duration.

6. **Should a bounded paper-smoke proof be the next market-hours action?**
   Yes. The exact mechanism already exists and is proven safe:
   `Start-PaperTradingSmoke.ps1 -StartIntradayRefreshLoop
   -IntradayRefreshIntervalSeconds 300 -RequireIntradayRefresh
   -WatchSeconds <bounded>` (the same invocation pattern validated by
   `PAPER-SMOKE-FOLLOWUP-01D`/`01E`), run during an active session with
   `-RequireIntradayRefresh` so the run fails closed with an actionable
   reason code if Blocker 1 recurs, rather than running blind.

7. **Exact next patch ID recommendation:**
   `PAPER-TRADE-LIFECYCLE-PROOF-01-COMBINED` — a market-hours-gated,
   bounded live-observation patch (audit/proof scope, same shape as
   `AUTON-NO-TRADE-02`/`MARKET-HOURS-PROOF-SWEEP-01`) whose sole mission
   is to run the existing smoke path during a session where
   `-RequireIntradayRefresh` passes, and record whatever the daemon
   naturally does — including a real paper order if the strategy signals,
   or a durable no-trade explanation if it does not. This is justified
   because every lifecycle stage already has durable, tested code (per
   `01B`'s matrix) and the sole remaining gap in all three blockers is
   live observation, exactly matching the precedent already set by
   `AUTON-NO-TRADE-02B`/`02C` and `PAPER-SMOKE-FOLLOWUP-01E`'s live
   validation. `STRATEGY-SIGNAL-GENERATION-ADMISSION-AUDIT-01-COMBINED`
   and `INTRADAY-DATA-TO-SIGNAL-PROOF-01-COMBINED` are not recommended
   because both stages they would audit (rows 2-5 of `01B`'s matrix) are
   already `CLOSED`/live-proven — re-auditing them would not remove a
   blocker. `PAPER-ORDER-FILL-LIFECYCLE-SMOKE-01-COMBINED` is not
   recommended as a *separate* patch because it cannot be exercised
   independently of Blockers 1-2 clearing first — it would just be
   `PAPER-TRADE-LIFECYCLE-PROOF-01`'s downstream outcome, not a
   prerequisite to it.

8. **Exact non-goals:** No strategy threshold change. No new strategy
   engine. No new asset class. No AI/Vertus integration. No forced or
   simulated order, ack, or fill. No change to
   `DATA-FRESHNESS-READINESS-GATE-01`, Gate 0, the routing guard, or any
   OMS/outbox/inbox semantics. No config flag change. No live routing
   enablement.

## 3. Safety confirmation

No live orders. No paper orders. No trading behavior changed. No network
calls. No DB mutation. This patch is docs-only, derived entirely from
`01B`'s already-grounded evidence.
