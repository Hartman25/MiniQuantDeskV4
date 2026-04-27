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
- Stop. Do NOT edit old migrations to "make it match."
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

---

## Local proof DB reset — stale migration checksum (PROOF-DB-RESET-01)

### Symptom

DB-backed Cargo tests fail with an error like:

```
error: migration 6 was previously applied but has been modified
```

or more generally:

```
error: migration N was previously applied but has been modified
```

This appears in tests that call `sqlx::migrate!()` against the local proof DB
(any test requiring `MQK_DATABASE_URL`).  It means the local proof database
was created against an earlier revision of the migrations, and one or more
migration files have since changed in the repository.

**This is always a local environment hygiene problem, not a code defect.**
The correct fix is to recreate the proof DB from scratch.

Do NOT:
- Edit historical migration files to "make the checksum match."
- Weaken sqlx migration integrity.
- Skip migrations or bypass `_sqlx_migrations`.

Historical migrations are immutable.  The proof DB is disposable.

---

### Proof DB identity

| Property  | Value                                         |
|-----------|-----------------------------------------------|
| URL       | `postgres://mqk:mqk@127.0.0.1:5432/mqk_test` |
| DB name   | `mqk_test`                                    |
| User      | `mqk`                                         |
| Password  | `mqk`                                         |
| Host/port | `127.0.0.1:5432`                              |
| Container | `mqk-postgres-proof`                          |

The dev DB (`mqk_dev` at `localhost:5432`) and the proof DB (`mqk_test`) are
separate databases.  The reset script enforces this — it will never touch `mqk_dev`.

---

### Step-by-step: proof DB reset on Windows (PowerShell)

#### Step 1 — Verify the proof container is running

```powershell
docker ps --filter name=mqk-postgres-proof
```

If the container is absent, start or create it:

```powershell
# Start an existing stopped container:
docker start mqk-postgres-proof

# Or create a fresh one (first time):
docker run --name mqk-postgres-proof `
  -e POSTGRES_USER=mqk `
  -e POSTGRES_PASSWORD=mqk `
  -e POSTGRES_DB=mqk_test `
  -p 5432:5432 `
  -d postgres:16
```

Wait a few seconds for Postgres to initialize before proceeding.

#### Step 2 — Inspect the reset script (optional)

```powershell
powershell -NoProfile -ExecutionPolicy Bypass `
  -File .\scripts\reset-mqk-testdb.ps1 -Help
```

#### Step 3 — Dry-run (no changes)

```powershell
powershell -NoProfile -ExecutionPolicy Bypass `
  -File .\scripts\reset-mqk-testdb.ps1 `
  -DatabaseUrl "postgres://mqk:mqk@127.0.0.1:5432/mqk_test"
```

The script prints what it will do and exits without touching anything.
Verify the DB name shown is `mqk_test`.

#### Step 4 — Execute the reset

```powershell
powershell -NoProfile -ExecutionPolicy Bypass `
  -File .\scripts\reset-mqk-testdb.ps1 `
  -DatabaseUrl "postgres://mqk:mqk@127.0.0.1:5432/mqk_test" `
  -ConfirmReset
```

The script:
1. Terminates active connections to `mqk_test`.
2. Drops `mqk_test`.
3. Recreates `mqk_test` as an empty database.

sqlx will apply all migrations from scratch the first time a test runs.

#### Step 5 — Set MQK_DATABASE_URL

```powershell
$env:MQK_DATABASE_URL = "postgres://mqk:mqk@127.0.0.1:5432/mqk_test"
```

#### Step 6 — Validate with targeted test commands

Run DB-backed tests that were blocked by the stale checksum.  All of the
following skip gracefully if `MQK_DATABASE_URL` is unset; they fail (not skip)
if the DB is reachable but migrations are stale.

```powershell
# CTRL-ARM-01: arm preflight DB-backed proof
cargo test --manifest-path .\core-rs\Cargo.toml `
  -p mqk-daemon --test scenario_ctrl_arm_preflight_01 `
  -- --test-threads=1

# OPS-REPAIR-01: ops repair route
cargo test --manifest-path .\core-rs\Cargo.toml `
  -p mqk-daemon --test scenario_ops_repair_01 `
  -- --test-threads=1

# FLOW Phase 1: execution flow DB-positive
cargo test --manifest-path .\core-rs\Cargo.toml `
  -p mqk-daemon --test scenario_execution_flow_flow01 `
  -- --test-threads=1

# EXEC-RETRY-01: retryable dispatch
cargo test --manifest-path .\core-rs\Cargo.toml `
  -p mqk-testkit --test scenario_exec_retry_01 `
  -- --test-threads=1

# Cancel target missing
cargo test --manifest-path .\core-rs\Cargo.toml `
  -p mqk-testkit --test scenario_cancel_target_missing_c1 `
  -- --test-threads=1

# mqk-db full suite (includes all ignored DB-backed proofs)
cargo test --manifest-path .\core-rs\Cargo.toml `
  -p mqk-db -- --include-ignored --test-threads=1
```

#### Step 7 — Full proof (optional, for institutional closure)

```powershell
.\full_repo_proof.ps1 -ProofProfile full
```

Requires `MQK_DATABASE_URL` to be set.  Runs the full CI-10 mandatory matrix
plus all non-DB proof lanes and produces a structured JSON summary.

---

### Safety properties of the reset script

- `scripts/reset-mqk-testdb.ps1` requires `-DatabaseUrl` explicitly — it will
  not guess from ambient environment variables.
- The database name is extracted from the URL and checked against a blocklist
  before any action is taken.
- Blocked names: `postgres`, `template0`, `template1`, `mqk_dev`, `mqk_live`,
  `mqk_prod`, `production`, `live`, `prod`, `defaultdb`.
- Without `-ConfirmReset` the script is a safe dry-run that prints the planned
  action and exits 0.
- Passwords are never printed.
- Runtime and dev databases are never touched.

---

### Bash alternative (Linux/macOS or Git Bash on Windows)

```bash
# Start or reuse the proof container and reset the DB in one step:
bash scripts/db_proof_bootstrap.sh --start-postgres

# Or reset manually with psql:
PGPASSWORD=mqk psql -h 127.0.0.1 -p 5432 -U mqk -d postgres \
  -c "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE datname='mqk_test' AND pid <> pg_backend_pid();"
PGPASSWORD=mqk psql -h 127.0.0.1 -p 5432 -U mqk -d postgres \
  -c "DROP DATABASE IF EXISTS mqk_test;"
PGPASSWORD=mqk psql -h 127.0.0.1 -p 5432 -U mqk -d postgres \
  -c "CREATE DATABASE mqk_test;"

export MQK_DATABASE_URL="postgres://mqk:mqk@127.0.0.1:5432/mqk_test"
bash scripts/db_proof_bootstrap.sh
```
