-- 0090 (D2, V4-M5-M8-FINAL-INDEPENDENT-REVIEW-CORRECTION-02): the canonical
-- options-lifecycle adjustment journal.
--
-- Authority relationship (explicit): the accepted mutable economic ledger is
-- mqk_portfolio's append-only `PortfolioState`/`LedgerEntry` stream, rebuilt
-- from durable evidence on every run start. Fills reach it from the inbox;
-- an exercise/assignment/expiration is NOT a fill and has no inbox event, so
-- this journal is the durable evidence stream for the new
-- `LedgerEntry::LifecycleAdjustment` variant: one immutable row per applied
-- lifecycle event carrying the SIGNED option-position delta, the SIGNED
-- underlying-share delta (with its strike basis) and the provider's own SIGNED
-- cash amount. `sys_option_lifecycle_applied` (0084/0087) recorded only a
-- marker and is superseded: nothing reads it as economic state.
--
-- One apply transaction (mqk_db::apply_lifecycle_adjustment_tx) inserts the
-- journal row and moves the event state READY_TO_APPLY -> APPLIED_AWAITING_BROKER
-- atomically. economic_apply_id is a deterministic UUIDv5 of
-- (account, domain, lifecycle event) -- a retry or a crashed-then-retried apply
-- can only ever find the same row, never add a second economic effect.
--
-- Rows are immutable except for `baseline_subsumed_at_utc`, set once when an
-- operator baseline adoption (broker truth) has absorbed the effect so the
-- journal stops being replayed on top of it. A DELETE/other UPDATE is refused
-- by trigger.
--
-- Idempotent: safe to re-run. Does not modify 0083-0089.

CREATE TABLE IF NOT EXISTS sys_option_lifecycle_adjustment_journal (
    journal_seq                 bigint GENERATED ALWAYS AS IDENTITY,
    economic_apply_id           text        NOT NULL,
    broker_account_id           text        NOT NULL,
    execution_domain            text        NOT NULL,
    lifecycle_activity_id       text        NOT NULL,
    lifecycle_activity_type     text        NOT NULL,
    option_symbol               text        NOT NULL,
    underlying_symbol           text        NOT NULL,
    option_qty_delta_micros     bigint      NOT NULL,
    underlying_qty_delta_micros bigint      NULL,
    strike_micros               bigint      NULL,
    cash_delta_micros           bigint      NULL,
    optrd_activity_id           text        NULL,
    correlation_basis           text        NOT NULL,
    applied_at_utc              timestamptz NOT NULL,
    baseline_subsumed_at_utc    timestamptz NULL,
    CONSTRAINT sys_option_lifecycle_adjustment_journal_pkey PRIMARY KEY (economic_apply_id),
    CONSTRAINT sys_option_lifecycle_adjustment_journal_seq_unique UNIQUE (journal_seq),
    CONSTRAINT sys_option_lifecycle_adjustment_journal_event_unique
        UNIQUE (broker_account_id, lifecycle_activity_id, lifecycle_activity_type),
    CONSTRAINT sys_option_lifecycle_adjustment_journal_account_fk
        FOREIGN KEY (broker_account_id)
        REFERENCES sys_broker_account_authority (authority_key),
    CONSTRAINT sys_option_lifecycle_adjustment_journal_event_fk
        FOREIGN KEY (broker_account_id, lifecycle_activity_id, lifecycle_activity_type)
        REFERENCES sys_option_lifecycle_event_state
            (broker_account_id, lifecycle_activity_id, lifecycle_activity_type),
    CONSTRAINT sys_option_lifecycle_adjustment_journal_type_check
        CHECK (lifecycle_activity_type IN ('OPEXC', 'OPASN', 'OPEXP')),
    CONSTRAINT sys_option_lifecycle_adjustment_journal_option_delta_check
        CHECK (option_qty_delta_micros <> 0),
    CONSTRAINT sys_option_lifecycle_adjustment_journal_basis_check
        CHECK (correlation_basis IN ('not_required', 'explicit_group_id',
                                     'same_activity_id', 'unique_evidence')),
    -- An expiration carries NO underlying delivery and NO cash; an
    -- exercise/assignment carries both, from provider evidence.
    CONSTRAINT sys_option_lifecycle_adjustment_journal_shape_check
        CHECK ((lifecycle_activity_type = 'OPEXP'
                AND underlying_qty_delta_micros IS NULL
                AND strike_micros IS NULL
                AND cash_delta_micros IS NULL
                AND optrd_activity_id IS NULL
                AND correlation_basis = 'not_required')
            OR (lifecycle_activity_type <> 'OPEXP'
                AND underlying_qty_delta_micros IS NOT NULL
                AND underlying_qty_delta_micros <> 0
                AND strike_micros IS NOT NULL
                AND strike_micros > 0
                AND cash_delta_micros IS NOT NULL
                AND optrd_activity_id IS NOT NULL
                AND correlation_basis <> 'not_required'))
);

-- A settlement trade is consumed by at most one lifecycle event per account.
CREATE UNIQUE INDEX IF NOT EXISTS uq_option_lifecycle_adjustment_journal_optrd
    ON sys_option_lifecycle_adjustment_journal (broker_account_id, optrd_activity_id)
    WHERE optrd_activity_id IS NOT NULL;

CREATE INDEX IF NOT EXISTS idx_option_lifecycle_adjustment_journal_replay
    ON sys_option_lifecycle_adjustment_journal
        (execution_domain, journal_seq)
    WHERE baseline_subsumed_at_utc IS NULL;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'sys_option_lifecycle_event_state_apply_journal_fk'
    ) THEN
        ALTER TABLE sys_option_lifecycle_event_state
            ADD CONSTRAINT sys_option_lifecycle_event_state_apply_journal_fk
            FOREIGN KEY (economic_apply_id)
            REFERENCES sys_option_lifecycle_adjustment_journal (economic_apply_id);
    END IF;
END $$;

CREATE OR REPLACE FUNCTION sys_option_lifecycle_adjustment_journal_guard() RETURNS trigger AS $$
BEGIN
    IF TG_OP = 'DELETE' THEN
        RAISE EXCEPTION 'sys_option_lifecycle_adjustment_journal rows are immutable';
    END IF;
    IF (NEW.journal_seq, NEW.economic_apply_id, NEW.broker_account_id, NEW.execution_domain,
        NEW.lifecycle_activity_id, NEW.lifecycle_activity_type, NEW.option_symbol,
        NEW.underlying_symbol, NEW.option_qty_delta_micros, NEW.underlying_qty_delta_micros,
        NEW.strike_micros, NEW.cash_delta_micros, NEW.optrd_activity_id, NEW.correlation_basis,
        NEW.applied_at_utc)
       IS DISTINCT FROM
       (OLD.journal_seq, OLD.economic_apply_id, OLD.broker_account_id, OLD.execution_domain,
        OLD.lifecycle_activity_id, OLD.lifecycle_activity_type, OLD.option_symbol,
        OLD.underlying_symbol, OLD.option_qty_delta_micros, OLD.underlying_qty_delta_micros,
        OLD.strike_micros, OLD.cash_delta_micros, OLD.optrd_activity_id, OLD.correlation_basis,
        OLD.applied_at_utc) THEN
        RAISE EXCEPTION 'sys_option_lifecycle_adjustment_journal rows are immutable';
    END IF;
    IF OLD.baseline_subsumed_at_utc IS NOT NULL
       AND NEW.baseline_subsumed_at_utc IS DISTINCT FROM OLD.baseline_subsumed_at_utc THEN
        RAISE EXCEPTION 'baseline_subsumed_at_utc is set once';
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS sys_option_lifecycle_adjustment_journal_guard_trg
    ON sys_option_lifecycle_adjustment_journal;
CREATE TRIGGER sys_option_lifecycle_adjustment_journal_guard_trg
    BEFORE UPDATE OR DELETE ON sys_option_lifecycle_adjustment_journal
    FOR EACH ROW EXECUTE FUNCTION sys_option_lifecycle_adjustment_journal_guard();
