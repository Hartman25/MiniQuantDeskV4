-- CC-03B: Durable restart intent / restart provenance.
--
-- sys_restart_intent records explicit restart/mode-change intents from the
-- control plane so restart behavior is not inferred only from transient
-- in-memory state or scattered route-local assumptions.
--
-- Key design decisions:
-- - transition_verdict constrained to the four CC-03A canonical verdict strings.
--   This ties every durable record to the canonical mode-transition seam
--   established in mode_transition::evaluate_mode_transition.
-- - from_mode / to_mode use DeploymentMode::as_api_label() values (lowercase).
-- - initiated_at_utc is caller-injected; no DEFAULT now() per determinism rules.
-- - Multi-row log table (not a singleton sentinel); multiple intents can exist.
-- - status lifecycle: pending → completed | cancelled | superseded.
-- - completed_at_utc is caller-injected; nullable until finalised.

create table if not exists sys_restart_intent (
    intent_id           uuid         primary key,
    engine_id           text         not null,
    from_mode           text         not null
                            check (from_mode in (
                                'paper', 'live-shadow', 'live-capital', 'backtest'
                            )),
    to_mode             text         not null
                            check (to_mode in (
                                'paper', 'live-shadow', 'live-capital', 'backtest'
                            )),
    transition_verdict  text         not null
                            check (transition_verdict in (
                                'same_mode',
                                'admissible_with_restart',
                                'refused',
                                'fail_closed'
                            )),
    initiated_by        text         not null
                            check (initiated_by in ('operator', 'system', 'recovery')),
    initiated_at_utc    timestamptz  not null,
    status              text         not null default 'pending'
                            check (status in (
                                'pending', 'completed', 'cancelled', 'superseded'
                            )),
    completed_at_utc    timestamptz,
    note                text         not null default ''
);

-- Supports efficient lookup of the latest pending intent for a given engine.
create index if not exists sys_restart_intent_engine_status_idx
    on sys_restart_intent (engine_id, status, initiated_at_utc desc);
