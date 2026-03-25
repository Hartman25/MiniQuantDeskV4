-- LO-02D-F1: Extend runs.mode check constraint to include LIVE-SHADOW and
-- LIVE-CAPITAL.
--
-- Background: migration 0009 added `runs_mode_check` with the set
-- ('PAPER','LIVE','BACKTEST'), reflecting the modes known at that time.
-- The daemon codebase subsequently added DeploymentMode::LiveShadow and
-- DeploymentMode::LiveCapital, whose as_db_mode() values are 'LIVE-SHADOW'
-- and 'LIVE-CAPITAL' respectively.  Any successful start_execution_runtime
-- call in LiveShadow or LiveCapital mode would fail the constraint when
-- inserting the run row — a latent production bug.
--
-- 'LIVE' is retained for backward compatibility with any existing rows that
-- may have been written before the LiveShadow/LiveCapital split.

alter table runs
    drop constraint if exists runs_mode_check;

alter table runs
    add constraint runs_mode_check
    check (mode in ('PAPER','LIVE','BACKTEST','LIVE-SHADOW','LIVE-CAPITAL'));
