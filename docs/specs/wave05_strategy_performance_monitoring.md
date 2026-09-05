# Wave05 — Strategy Performance Monitoring

Contract reference for the Wave05 strategy-lineage / closed-trade / performance
/ decay / regime / risk-visibility read-model chain. This document describes
frozen invariants and current behavior; it does not track patch status or
commit history — see Git log and the canonical patch ledger for that.

All of this chain is READ-ONLY analytics, monitoring, and visibility over
Paper trading. None of it submits orders, changes broker routing, enables
Live, mutates promotion state, or writes/clears a strategy suppression.

## P1 — strategy lineage authority

Exact fill → strategy lineage is:

```
fill.internal_order_id -> oms_outbox.idempotency_key -> order_json
```

with exact run coherence (`oms_outbox.run_id == fill.run_id`). Historical
strategy identity comes ONLY from the exact durable originating order — never
inferred from symbol, current registry/promotion state, or timestamp
proximity.

Exact semantic strategy identity is `(strategy_id, strategy_semantic_fingerprint)`.
`strategy_id` alone is not sufficient identity when fingerprint authority
exists. A **legacy** strategy order (persisted before fingerprint capture)
still carries a real, non-`None` `strategy_id` — only its
`strategy_semantic_fingerprint` is `None`. Manual orders, malformed lineage,
and missing lineage remain distinct, non-fabricated truths
(`mqk_db::FillStrategyLineage`).

## P2 — canonical closed-trade authority

`oms_inbox` is the durable broker-event/fill ledger. The canonical economic
fill replay is `recover_oms_and_portfolio_traced` — the exact same applied
inbox ordering, OMS state transitions, duplicate-fill protection, cross-lane
dedup, effective fill quantity, and terminal overfill correction as ordinary
durable Paper accounting. There is no second raw `oms_inbox` economic replay.

`build_closed_trade_projection` mirrors `mqk_portfolio::accounting`'s FIFO
arithmetic bit-for-bit to attach durable strategy lineage to each FIFO lot
closure fragment. `mqk_portfolio::Lot` remains account-level and never
carries strategy identity.

Closed-trade attribution states (frozen): `attributed`, `cross_strategy`,
`semantic_identity_changed`, `manual_or_mixed`, `lineage_incomplete`,
`lineage_invalid`, `lineage_missing`. Only `attributed` is eligible for exact
semantic-strategy performance analytics.

Gross realized P&L from this chain is **gross trading P&L**, never net —
fees are not currently allocated deterministically across FIFO closure
fragments. See "P&L basis" below.

The single shared authority resolver is
`state::closed_trade_attribution::resolve_authoritative_closed_trade_view`,
consumed by both:
- `routes/paper_journal.rs`'s `closed_trades_lane`
- `routes/strategy_performance.rs` (P3/P4/P5, below)

Neither caller hand-rolls a second provenance classifier, snapshot-authority
validator, canonical replay, or durable-accounting comparison — both map the
same resolved view into their own response shape. Authority is `"active"`
only when: the canonical projection's summed gross realized P&L equals
`mqk_portfolio`'s canonical realized P&L; the shared
`classify_portfolio_provenance` verdict is `Active`; and the durable
accounting row's `last_applied_inbox_id` exactly matches the canonical
replay watermark.

## P3 — exact semantic-strategy performance analytics

`GET /api/v1/strategy/performance?run_id=<uuid>` (public, no auth).

Rows are keyed by `(strategy_id, strategy_semantic_fingerprint)` — two
fingerprints under one `strategy_id` are always two rows, never collapsed.
Only `attributed` FIFO closure fragments contribute to a row. Fragments
sharing the same closing economic fill (`close_inbox_id` +
`close_internal_order_id`) collapse into one `AttributedCloseEvent` — metrics
are computed over close EVENTS, not raw fragments.

Rows are populated only when the upstream closed-trade authority's
`truth_state` is exactly `"active"` (stricter than the Paper Journal, which
also shows `"incomplete"` fill history) — never a fabricated zero when the
authority is not active. Zero attributed closures with `truth_state ==
"active"` is a valid authoritative zero, distinguishable by `truth_state`
alone, never by an empty `rows` with no other signal.

`attribution_coverage` always exposes exactly seven frozen buckets —
`attributed`, `cross_strategy`, `semantic_identity_changed`,
`manual_or_mixed`, `lineage_incomplete`, `lineage_invalid`,
`lineage_missing` — a closed public vocabulary, never a dynamic or
encountered-only set. A bucket with no observed fragments is an explicit
`fragment_count = 0` / `gross_realized_pnl_micros = 0`, not an absent entry.
`sum(bucket.gross_realized_pnl_micros)` always equals the same total the
closed-trade authority reports — no economic P&L silently disappears from
the response, even when it can't be attributed to one exact strategy.
Coverage-derived risk flags (P5) key off `fragment_count > 0`, never bucket
presence or `gross_realized_pnl_micros != 0` — a bucket can hold real
fragments that net to zero P&L.

## P4 — decay monitor and observational regime context

Additive per-row fields: `decay_monitor`, `regime_context`.

**Decay monitor**: fixed windows over the row's ordered attributed
close-event series — `recent` = newest 5 events, `baseline` = the 10
immediately preceding (never overlapping). Fewer than 15 events →
`decay_state = "insufficient_data"`. Classifier detects ONLY a strong
gross-expectancy sign reversal: `decay_observed` (baseline > 0, recent < 0),
`improvement_observed` (baseline <= 0, recent > 0), or
`no_expectancy_sign_flip` otherwise. This is a deterministic monitoring flag,
not proof the strategy's true alpha has disappeared — no significance test,
no p-value, no arbitrary percentage threshold.

**Regime context**: resolves the row's most recent attributed close event's
EXACT durable `(symbol, timeframe_secs)` via
`mqk_db::fetch_order_symbol_timeframe_context` (same
`internal_order_id -> idempotency_key` join P1 uses) — never from current
config or registry state. New internal native strategy decisions persist
their exact positive `timeframe_secs` into `oms_outbox.order_json` at
construction time (`mqk_daemon::decision::build_order_json`); P4 reads that
durable value back, and current config/promotion drift can never rewrite a
historical order's recorded context. Legacy rows persisted before this
provenance existed may still lack the field and remain `context_unavailable`
— there is no current-config reconstruction or backfill for them. Conflicting
exact timeframes for the same
strategy+symbol fail closed to `context_ambiguous`; missing/malformed
context fails closed to `context_unavailable`. Loads only completed
`md_bars` (no provider/network call) and classifies via
`mqk_backtest::regime::detect_market_regime`. `regime_authority` is always
`"research_only_observational"` — this can NEVER gate execution, risk,
promotion, or suppression (`REGIME_CAN_AFFECT_EXECUTION` /
`REGIME_CAN_AFFECT_RISK_GATE` are always `NO`).

## P5 — visibility-only strategy risk visibility

Additive per-row field: `risk_visibility`. No new endpoint, no automated
suppression, no automated clearing.

Consumes only P3 performance truth, P4 decay state and regime context, and
the existing durable suppression READ seam
(`mqk_db::fetch_active_suppression_for_strategy`), keyed by `strategy_id`
alone — matching the real admission-gate semantics: an active suppression
for a `strategy_id` applies to EVERY semantic fingerprint of that strategy,
never fingerprint-specific.

`suppression_truth_state` is a closed vocabulary of exactly `active`,
`not_active`, or `query_failed`. `active_strategy_suppression` is
`Option<bool>`: `Some(true)` / `Some(false)` / `None` respectively — a query
failure is never collapsed into `Some(false)` ("not_active"), which would be
fail-open. `query_failed` forces `risk_visibility_state = "unavailable"` and
`recommended_operator_action = "insufficient_evidence"`.

`risk_visibility_state` closed-vocabulary precedence: `unavailable` >
`suppressed` > `insufficient_data` > `watch` > `normal`. Observational
`high_volatility` regime context is informational only
(`observational_high_volatility_context` flag) and never by itself changes
`risk_visibility_state`. `recommended_operator_action` is text/visibility
only — this route never calls `insert_strategy_suppression` or
`clear_strategy_suppression`; no automated suppression or automated clearing
was added by this chain.

## P&L basis (frozen, all of P3/P4/P5)

Every P&L field in `GET /api/v1/strategy/performance` is
`pnl_basis = "gross_realized_before_fees"` with
`fee_allocation_state = "not_allocated_to_strategy_close_events"`. No
`net_pnl`, `net_expectancy`, or `after_cost_*` field exists anywhere in this
response — fee allocation across FIFO closure fragments is not a
deterministically solved problem today.

## Safety boundary (frozen)

- No Paper or Live order submission, cancel, or replace from this chain.
- No broker routing change; Live is never enabled by any of this.
- No automated strategy suppression or automated suppression clearing.
- No promotion-state change.
- No persistent strategy-trade ledger or new analytics table — every value
  in this chain is computed on read from `oms_inbox`/`oms_outbox`/`md_bars`/
  `sys_strategy_suppressions`, never cached durably.
- `mqk_portfolio::Lot` and ordinary Paper FIFO accounting are never touched
  or reinterpreted by any part of this chain.
