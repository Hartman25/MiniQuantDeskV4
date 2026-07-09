-- AUTON-NO-TRADE-OFFHOURS-01B: Durable autonomous no-trade diagnostic snapshot.
--
-- One row per distinct dominant no-trade reason observed by
-- GET /api/v1/autonomous/readiness, journaled so the explanation survives a
-- daemon restart even if no operator was polling the route at the moment the
-- reason was true. This is a snapshot of the readiness verdict itself -- it
-- does not replace or duplicate strategy_signal_evaluations (0043), which
-- covers exactly one reason (post-tick no-signal) at a finer
-- symbol/timeframe grain. Writing this table never implies an
-- oms_outbox/order/fill row exists, and this table is never read by any
-- gate, decision, or dispatch path.
--
-- `diagnostic_id` is UUIDv5(NAMESPACE_DNS,
--   "mqk.no-trade-diagnostic.v1|{reason_code}|{stage}|{minute_bucket}")
-- -- deterministic and bucketed to the observing minute, so repeated polls of
-- the readiness route while the same reason holds cannot produce unbounded
-- duplicate rows (ON CONFLICT DO NOTHING caps writes to at most one row per
-- distinct reason per minute).
--
-- `run_id` is intentionally nullable with no FK to `runs`: a diagnostic can
-- legitimately fire with no active run (that is the common off-hours case),
-- and this table is observability only -- it is not part of the
-- outbox/inbox/run lifecycle chain.
--
-- `paper_order_attempted` / `live_order_attempted` are always `false` for
-- every row this patch writes -- this table only ever records why an order
-- was NOT attempted.

create table if not exists autonomous_no_trade_diagnostics (
    diagnostic_id           uuid         primary key,
    observed_at_utc         timestamptz  not null,
    run_id                  uuid,
    mode                    text         not null,
    session_window_state    text         not null,
    runtime_start_allowed   boolean      not null,
    arm_state               text         not null,
    overall_ready           boolean      not null,
    reason_code             text         not null,
    reason                  text         not null,
    stage                   text         not null,
    paper_order_attempted   boolean      not null,
    live_order_attempted    boolean      not null,
    source                  text         not null
);

-- Most-recent-N query pattern used by GET /api/v1/autonomous/no-trade-diagnostics.
create index if not exists autonomous_no_trade_diagnostics_observed_idx
    on autonomous_no_trade_diagnostics (observed_at_utc desc);
