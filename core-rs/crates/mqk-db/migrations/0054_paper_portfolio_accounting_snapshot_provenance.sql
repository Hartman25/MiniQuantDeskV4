-- 0054: Durable paper portfolio accounting snapshot provenance
-- (DURABLE-PAPER-PORTFOLIO-AND-PNL-01-FINAL-RUN-SCOPING-ACCOUNTING-AND-CLOSURE-REPAIR).
--
-- Additive only -- does not modify 0053 or any other existing table.
--
-- sys_paper_portfolio_accounting_state.source_snapshot_id: ties a durable
-- accounting row to the exact broker snapshot whose positions were used to
-- compute its accounting_epoch/accounting_epoch_reason (bidirectional
-- completeness, B4-E). Nullable: legacy rows written before this repair
-- never had a snapshot association recorded and must round-trip as null
-- (never fabricated) rather than being backfilled with a guessed value.
-- A null source_snapshot_id means the row's completeness classification
-- cannot be traced to a specific snapshot and must not be reported as fully
-- proven active accounting truth by any reader.
--
-- No DEFAULT now()/gen_random_uuid() -- this migration adds no new
-- timestamp or identity column with an implicit default.

ALTER TABLE sys_paper_portfolio_accounting_state
    ADD COLUMN IF NOT EXISTS source_snapshot_id uuid NULL
        REFERENCES sys_paper_portfolio_snapshots(snapshot_id);
