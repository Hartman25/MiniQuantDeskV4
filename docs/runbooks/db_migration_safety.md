# DB Migration Safety (Allocator-Grade)

This repo uses SQLx embedded migrations (`sqlx::migrate!`). SQLx records applied migrations + checksums in the database. If a migration file is edited after it was applied, SQLx will refuse to run migrations and you will get a checksum mismatch error.

## Non-negotiable rules

1. **Never edit an applied migration file.**
   - If a change is needed, create a NEW migration.
2. **Dev/Test DBs must be disposable.**
   - Prefer a fresh database for clean runs and CI.
3. **Treat LIVE databases as immutable infrastructure.**
   - Only apply forward-only migrations with explicit operator acknowledgment.

## What happens if you edit a migration?
- SQLx stores checksums in `_sqlx_migrations`.
- If a file changes, `mqk db migrate` will fail with a checksum mismatch.

## Correct remediation if you see a checksum mismatch

### If this is a dev/test DB:
- Blow it away and recreate it (recommended).
- Then run `mqk db migrate`.

### If this is a prod/live DB:
- Stop. Do NOT edit old migrations to “make it match.”
- Create a new migration to apply forward-only changes.
- Confirm what the database currently has in `_sqlx_migrations`.

## Guardrail in this repo
`mqk db migrate` refuses to run if there are any LIVE runs in ARMED or RUNNING state unless you pass `--yes`.

Example:
- Safe: `mqk db migrate`
- Override: `mqk db migrate --yes`

The override is meant for controlled maintenance windows only.

## Operator checklist before migrating
- Verify no LIVE runs are ARMED or RUNNING.
- Snapshot the database.
- Confirm you are pointing at the intended DB URL.
- Run `mqk db status` first.
- Apply migrations with `mqk db migrate` (or `--yes` only when appropriate).
