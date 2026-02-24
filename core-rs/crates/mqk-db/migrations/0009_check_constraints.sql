-- Patch D1: Add CHECK constraints for enumerated text state columns.
--
-- Problem (from CLAUDE_PATCH_RUNBOOK_AUDIT.md):
--   Critical state columns are plain `text` with comment-based documentation.
--   A typo or out-of-range value (e.g. "PENIDNG", "DSIARMEDD") is silently
--   accepted by the DB, corrupting state machine logic that relies on exact
--   string matching.
--
-- Fix: add DB-level CHECK constraints for every closed-enum text column.
--   These constraints reject writes at the storage layer, independent of any
--   application-layer validation, ensuring state machine invariants hold even
--   if a caller bypasses the Rust helpers.
--
-- Columns constrained here (runs.status already constrained in 0002):
--   oms_outbox.status               — PENDING | CLAIMED | SENT | ACKED | FAILED
--   runs.mode                       — PAPER | LIVE | BACKTEST
--   sys_arm_state.state             — ARMED | DISARMED
--   sys_arm_state.reason            — DisarmReason variants (nullable)
--   sys_reconcile_checkpoint.verdict — CLEAN | DIRTY

-- ---------------------------------------------------------------------------
-- 1. oms_outbox.status
-- ---------------------------------------------------------------------------
-- Migration 0001 defined the initial set (PENDING|SENT|ACKED|FAILED) via comment.
-- Migration 0005 (Patch L3) added CLAIMED to the protocol, also via comment only.
-- This constraint formalises the complete five-value allowed set.

alter table oms_outbox
    drop constraint if exists oms_outbox_status_check;

alter table oms_outbox
    add constraint oms_outbox_status_check
    check (status in ('PENDING','CLAIMED','SENT','ACKED','FAILED'));

-- ---------------------------------------------------------------------------
-- 2. runs.mode
-- ---------------------------------------------------------------------------
-- Migration 0001 comment: PAPER | LIVE.
-- mqk-cli commands/mod.rs and commands/run.rs also use BACKTEST.

alter table runs
    drop constraint if exists runs_mode_check;

alter table runs
    add constraint runs_mode_check
    check (mode in ('PAPER','LIVE','BACKTEST'));

-- ---------------------------------------------------------------------------
-- 3. sys_arm_state.state
-- ---------------------------------------------------------------------------
-- Migration 0006 comment: 'ARMED' | 'DISARMED' — no constraint existed.

alter table sys_arm_state
    drop constraint if exists sys_arm_state_state_check;

alter table sys_arm_state
    add constraint sys_arm_state_state_check
    check (state in ('ARMED','DISARMED'));

-- ---------------------------------------------------------------------------
-- 4. sys_arm_state.reason
-- ---------------------------------------------------------------------------
-- Migration 0006 comment lists the five DisarmReason variants.
-- reason is NULL when state = 'ARMED'.

alter table sys_arm_state
    drop constraint if exists sys_arm_state_reason_check;

alter table sys_arm_state
    add constraint sys_arm_state_reason_check
    check (
        reason is null
        or reason in (
            'BootDefault',
            'ManualDisarm',
            'DeadmanHalt',
            'IntegrityViolation',
            'ReconcileDrift'
        )
    );

-- ---------------------------------------------------------------------------
-- 5. sys_reconcile_checkpoint.verdict
-- ---------------------------------------------------------------------------
-- Migration 0008 comment: 'CLEAN' | 'DIRTY' — no constraint existed.

alter table sys_reconcile_checkpoint
    drop constraint if exists sys_reconcile_checkpoint_verdict_check;

alter table sys_reconcile_checkpoint
    add constraint sys_reconcile_checkpoint_verdict_check
    check (verdict in ('CLEAN','DIRTY'));
