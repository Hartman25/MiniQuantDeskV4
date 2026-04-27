-- EXEC-RETRY-01: bounded dispatch retry state
--
-- Adds durable backoff + attempt tracking to oms_outbox so that retryable
-- broker failures (Transport/RateLimit with non_delivery_proven=true) are:
--   1. counted (dispatch_attempt_count) — enabling a hard attempt ceiling
--   2. rate-limited (next_dispatch_after_utc) — preventing tight retry loops
--   3. diagnosable (last_dispatch_error) — operator can see why a row is parked
--
-- outbox_claim_batch now filters on next_dispatch_after_utc so a backed-off
-- row is invisible to the dispatcher until its backoff window expires.
--
-- Migration is idempotent: ADD COLUMN IF NOT EXISTS is safe to re-run.

ALTER TABLE oms_outbox
    ADD COLUMN IF NOT EXISTS dispatch_attempt_count  INTEGER     NOT NULL DEFAULT 0,
    ADD COLUMN IF NOT EXISTS next_dispatch_after_utc TIMESTAMPTZ,
    ADD COLUMN IF NOT EXISTS last_dispatch_error     TEXT;

-- Partial index: only PENDING rows with an active backoff need fast lookup.
CREATE INDEX IF NOT EXISTS idx_outbox_retry_backoff
    ON oms_outbox (next_dispatch_after_utc)
    WHERE status = 'PENDING' AND next_dispatch_after_utc IS NOT NULL;
