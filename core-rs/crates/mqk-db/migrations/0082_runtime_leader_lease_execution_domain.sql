-- 0082 (B2.2, V4-M5-M8-APPROVED-DECISIONS-IMPLEMENTATION-01): domain-key the
-- durable runtime leadership lease.
--
-- Before this migration `runtime_leader_lease` was a single global singleton
-- row (`id SMALLINT PRIMARY KEY DEFAULT 1`, `CHECK (id = 1)`, migration
-- 0018). Migration 0081 (B2 part 1) gave `runs` an `execution_domain` so
-- equity_nyse and crypto_24_7 can each hold an independent, coexisting
-- durable run, but a single global lease row would still force those two
-- domains' runtimes to contend for the exact same leadership authority --
-- whichever domain's runtime acquired it first would durably lock the other
-- domain's runtime out of ever claiming/dispatching. Domain-keying the lease
-- itself is the second half of that structural prerequisite.
--
-- Historical-row classification: identical justification to migrations 0080
-- and 0081 -- the Alpaca adapter's `supports_asset_class(Crypto)` has never
-- returned `true` in any shipped commit, so any pre-existing single lease
-- row can only ever have been acquired by an equity_nyse runtime. This table
-- is pure transient coordination state (re-acquired every lease TTL, never
-- historical/audit truth -- see migration 0068's rationale), so this
-- backfill carries no audit-integrity weight the way 0080/0081's did; it is
-- included anyway so an in-flight lease survives this migration bound to
-- the correct domain rather than being silently dropped.
--
-- Row identity moves from the meaningless singleton `id = 1` to the natural
-- `execution_domain` key: exactly one lease row may exist per domain, and
-- `id` (never referenced by any other table's foreign key) is dropped
-- entirely rather than kept as a second, now-redundant identity column.

ALTER TABLE runtime_leader_lease
    ADD COLUMN IF NOT EXISTS execution_domain text;

UPDATE runtime_leader_lease
   SET execution_domain = 'equity_nyse'
 WHERE execution_domain IS NULL;

ALTER TABLE runtime_leader_lease
    ALTER COLUMN execution_domain SET NOT NULL;

DO $$
BEGIN
    IF NOT EXISTS (
        SELECT 1
        FROM pg_constraint
        WHERE conname = 'runtime_leader_lease_execution_domain_known'
          AND conrelid = 'runtime_leader_lease'::regclass
    ) THEN
        ALTER TABLE runtime_leader_lease
            ADD CONSTRAINT runtime_leader_lease_execution_domain_known
            CHECK (execution_domain in ('equity_nyse', 'crypto_24_7'));
    END IF;
END
$$;

-- Drop the old singleton CHECK(id = 1) by definition text, not by a guessed
-- auto-generated name -- Postgres's default naming for an unnamed table-level
-- CHECK is deployment-order-dependent, so matching on the constraint's own
-- definition is the only safe way to identify it deterministically.
DO $$
DECLARE
    c record;
BEGIN
    FOR c IN
        SELECT conname
        FROM pg_constraint
        WHERE conrelid = 'runtime_leader_lease'::regclass
          AND contype = 'c'
          AND pg_get_constraintdef(oid) ILIKE '%id = 1%'
    LOOP
        EXECUTE format('ALTER TABLE runtime_leader_lease DROP CONSTRAINT %I', c.conname);
    END LOOP;
END
$$;

ALTER TABLE runtime_leader_lease
    DROP CONSTRAINT runtime_leader_lease_pkey;

ALTER TABLE runtime_leader_lease
    ADD CONSTRAINT runtime_leader_lease_pkey PRIMARY KEY (execution_domain);

ALTER TABLE runtime_leader_lease
    DROP COLUMN id;
