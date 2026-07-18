-- AUTONOMOUS-DAILY-PAPER-OPERATIONS-01D2-LIFECYCLE-CLOSURE-REPAIR-01: add a
-- durable blocker-signature column to sys_autonomous_daily_operations.
-- Additive only; does not modify migrations 0048, 0049, 0050, or 0051.
--
-- Dedup authority for a manual/blocked transition was previously
-- state_reason_code alone, which cannot distinguish two distinct blocker
-- occurrences that happen to share the same reason code (e.g. two different
-- operation_identity_conflict causes, or a changed assignment_identity that
-- still classifies as the same reason). state_blocker_signature carries the
-- D1 typed blocker signature (mqk-daemon's
-- autonomous_retry_policy::blocker_signature, a bounded
-- "sha256:<64 lowercase hex chars>" digest over the reason plus its stable
-- identity context) so dedup can require (state, state_reason_code,
-- state_blocker_signature) to all match before treating a blocker as
-- unchanged.
--
-- Nullable for legacy-row compatibility only: a row created before this
-- migration has no recorded blocker signature (never fabricated). New rows
-- created through the current
-- mqk_db::autonomous_daily_operation::create_or_recover_autonomous_daily_operation
-- API supply an explicit null at insert time -- no SQL DEFAULT.

alter table sys_autonomous_daily_operations
    add column if not exists state_blocker_signature text;

comment on column sys_autonomous_daily_operations.state_blocker_signature is
    'Bounded D1 typed blocker signature ("sha256:<64 hex>") for the current state_reason_code, when the current state was reached via a blocked/manual transition. Null for a legacy row created before this migration, for a row that has never been blocked, and cleared by any non-blocked forward transition. Dedup authority for manual-intervention/preflight-blocked truth is (state, state_reason_code, state_blocker_signature) together -- never state_reason_code alone.';

alter table sys_autonomous_daily_operations
    add constraint sys_autonomous_daily_operations_state_blocker_signature_len
        check (state_blocker_signature is null or length(state_blocker_signature) <= 128);
