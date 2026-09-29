-- 0089 (D1, V4-M5-M8-FINAL-INDEPENDENT-REVIEW-CORRECTION-02): dedicated
-- options-lifecycle provider model + persisted lifecycle event state.
--
-- 1. sys_option_lifecycle_activity_ledger keeps every provider field the
--    dedicated raw type carries: optional provider correlation identifiers
--    (group_id / ref_id), provider status, description, and the exact parsed
--    provider record as jsonb. All nullable: the REST activities surface does
--    not document group_id/ref_id, so absence is stored as NULL, never
--    invented. Legacy rows keep NULLs (retained, not rewritten).
--
-- 2. sys_option_lifecycle_event_state: one row per OPEXC/OPASN/OPEXP raw
--    lifecycle row, created in the SAME transaction as the raw insert so an
--    ingested lifecycle event is fenced from its first committed instant.
--    States: PENDING_EVIDENCE, PENDING_AMBIGUOUS, READY_TO_APPLY,
--    APPLIED_AWAITING_BROKER, RECONCILED. Only RECONCILED clears the gate.
--    A BEFORE UPDATE trigger enforces the monotonic machine and identity
--    immutability at the database level, not merely caller discipline.
--    (economic_apply_id gains its foreign key to the journal in 0090.)
--
-- Idempotent: safe to re-run. Does not modify 0083-0088.

ALTER TABLE sys_option_lifecycle_activity_ledger
    ADD COLUMN IF NOT EXISTS provider_group_id text NULL,
    ADD COLUMN IF NOT EXISTS provider_ref_id   text NULL,
    ADD COLUMN IF NOT EXISTS provider_status   text NULL,
    ADD COLUMN IF NOT EXISTS description_raw   text NULL,
    ADD COLUMN IF NOT EXISTS raw_json          jsonb NULL;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1 FROM pg_constraint
         WHERE conname = 'sys_option_lifecycle_activity_ledger_provider_ids_nonblank_check'
    ) THEN
        ALTER TABLE sys_option_lifecycle_activity_ledger
            ADD CONSTRAINT sys_option_lifecycle_activity_ledger_provider_ids_nonblank_check
            CHECK ((provider_group_id IS NULL OR btrim(provider_group_id) <> '')
               AND (provider_ref_id IS NULL OR btrim(provider_ref_id) <> ''));
    END IF;
END $$;

CREATE INDEX IF NOT EXISTS idx_option_lifecycle_activity_ledger_account_group
    ON sys_option_lifecycle_activity_ledger (broker_account_id, provider_group_id)
    WHERE provider_group_id IS NOT NULL;

CREATE TABLE IF NOT EXISTS sys_option_lifecycle_event_state (
    broker_account_id            text        NOT NULL,
    lifecycle_activity_id        text        NOT NULL,
    lifecycle_activity_type      text        NOT NULL,
    execution_domain             text        NOT NULL,
    option_symbol                text        NOT NULL,
    underlying_symbol            text        NULL,
    state                        text        NOT NULL,
    state_reason                 text        NOT NULL,
    correlated_optrd_activity_id text        NULL,
    correlation_basis            text        NULL,
    economic_apply_id            text        NULL,
    created_at_utc               timestamptz NOT NULL,
    updated_at_utc               timestamptz NOT NULL,
    CONSTRAINT sys_option_lifecycle_event_state_pkey
        PRIMARY KEY (broker_account_id, lifecycle_activity_id, lifecycle_activity_type),
    CONSTRAINT sys_option_lifecycle_event_state_account_fk
        FOREIGN KEY (broker_account_id)
        REFERENCES sys_broker_account_authority (authority_key),
    CONSTRAINT sys_option_lifecycle_event_state_raw_fk
        FOREIGN KEY (broker_account_id, lifecycle_activity_id, lifecycle_activity_type)
        REFERENCES sys_option_lifecycle_activity_ledger
            (broker_account_id, activity_id, activity_type),
    CONSTRAINT sys_option_lifecycle_event_state_type_check
        CHECK (lifecycle_activity_type IN ('OPEXC', 'OPASN', 'OPEXP')),
    CONSTRAINT sys_option_lifecycle_event_state_state_check
        CHECK (state IN ('PENDING_EVIDENCE', 'PENDING_AMBIGUOUS', 'READY_TO_APPLY',
                         'APPLIED_AWAITING_BROKER', 'RECONCILED')),
    CONSTRAINT sys_option_lifecycle_event_state_basis_check
        CHECK (correlation_basis IS NULL
            OR correlation_basis IN ('not_required', 'explicit_group_id',
                                     'same_activity_id', 'unique_evidence')),
    -- READY / APPLIED / RECONCILED require a proven underlying and a proven
    -- correlation: an expiration needs no paired trade; an exercise/assignment
    -- requires exactly one.
    CONSTRAINT sys_option_lifecycle_event_state_resolved_shape_check
        CHECK (state IN ('PENDING_EVIDENCE', 'PENDING_AMBIGUOUS')
            OR (underlying_symbol IS NOT NULL
                AND ((lifecycle_activity_type = 'OPEXP'
                      AND correlated_optrd_activity_id IS NULL
                      AND correlation_basis = 'not_required')
                  OR (lifecycle_activity_type <> 'OPEXP'
                      AND correlated_optrd_activity_id IS NOT NULL
                      AND correlation_basis IN ('explicit_group_id', 'same_activity_id',
                                                'unique_evidence'))))),
    CONSTRAINT sys_option_lifecycle_event_state_apply_id_check
        CHECK ((state IN ('APPLIED_AWAITING_BROKER', 'RECONCILED')
                    AND economic_apply_id IS NOT NULL)
            OR (state NOT IN ('APPLIED_AWAITING_BROKER', 'RECONCILED')
                    AND economic_apply_id IS NULL))
);

CREATE INDEX IF NOT EXISTS idx_option_lifecycle_event_state_account_symbols
    ON sys_option_lifecycle_event_state (broker_account_id, execution_domain, state);
CREATE INDEX IF NOT EXISTS idx_option_lifecycle_event_state_option_symbol
    ON sys_option_lifecycle_event_state (broker_account_id, option_symbol);
CREATE INDEX IF NOT EXISTS idx_option_lifecycle_event_state_underlying
    ON sys_option_lifecycle_event_state (broker_account_id, underlying_symbol)
    WHERE underlying_symbol IS NOT NULL;

CREATE OR REPLACE FUNCTION sys_option_lifecycle_event_state_guard() RETURNS trigger AS $$
BEGIN
    IF NEW.broker_account_id <> OLD.broker_account_id
       OR NEW.lifecycle_activity_id <> OLD.lifecycle_activity_id
       OR NEW.lifecycle_activity_type <> OLD.lifecycle_activity_type
       OR NEW.execution_domain <> OLD.execution_domain
       OR NEW.option_symbol <> OLD.option_symbol
       OR NEW.created_at_utc <> OLD.created_at_utc THEN
        RAISE EXCEPTION 'sys_option_lifecycle_event_state identity columns are immutable';
    END IF;
    IF OLD.state = 'RECONCILED' AND NEW.state <> 'RECONCILED' THEN
        RAISE EXCEPTION 'lifecycle event state RECONCILED is terminal';
    END IF;
    IF OLD.state = 'APPLIED_AWAITING_BROKER' AND NEW.state NOT IN ('APPLIED_AWAITING_BROKER', 'RECONCILED') THEN
        RAISE EXCEPTION 'lifecycle event state APPLIED_AWAITING_BROKER can only advance to RECONCILED';
    END IF;
    IF NEW.state IN ('APPLIED_AWAITING_BROKER', 'RECONCILED') THEN
        IF OLD.state NOT IN ('READY_TO_APPLY', 'APPLIED_AWAITING_BROKER', 'RECONCILED') THEN
            RAISE EXCEPTION 'lifecycle event must be READY_TO_APPLY before it can be applied';
        END IF;
        IF OLD.state <> 'READY_TO_APPLY' AND NEW.economic_apply_id IS DISTINCT FROM OLD.economic_apply_id THEN
            RAISE EXCEPTION 'lifecycle economic_apply_id is immutable once applied';
        END IF;
        IF OLD.state <> 'READY_TO_APPLY' AND (
               NEW.underlying_symbol IS DISTINCT FROM OLD.underlying_symbol
            OR NEW.correlated_optrd_activity_id IS DISTINCT FROM OLD.correlated_optrd_activity_id
            OR NEW.correlation_basis IS DISTINCT FROM OLD.correlation_basis) THEN
            RAISE EXCEPTION 'applied lifecycle correlation is immutable';
        END IF;
    END IF;
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

DROP TRIGGER IF EXISTS sys_option_lifecycle_event_state_guard_trg
    ON sys_option_lifecycle_event_state;
CREATE TRIGGER sys_option_lifecycle_event_state_guard_trg
    BEFORE UPDATE ON sys_option_lifecycle_event_state
    FOR EACH ROW EXECUTE FUNCTION sys_option_lifecycle_event_state_guard();
