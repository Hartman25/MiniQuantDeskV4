# Dependency Governance — DEP-GOV-01

## sqlx-postgres 0.7.4 — never-type-fallback future-incompatibility

**Patch:** DEP-GOV-01
**Status:** Known open — upgrade deferred (see rationale below)

### What the warning says

```
warning: the following packages contain code that will be rejected by a future version of Rust:
  sqlx-postgres v0.7.4

warning: this function depends on never type fallback being `()`
  --> sqlx-postgres-0.7.4/src/connection/executor.rs:23:1
  = warning: this was previously accepted by the compiler but is being phased out;
    it will become a hard error in Rust 2024 and in a future release in all editions!
  = note: for more information, see
    <https://doc.rust-lang.org/edition-guide/rust-2024/never-type-fallback.html>
```

Cargo surfaces this during `cargo check`/`build`/`test` as a future-incompatibility
lint. It does **not** fail compilation today. It will become a hard error in Rust 2024
edition (and in all editions in a future stable release).

### Root cause

`sqlx-postgres 0.7.4` uses `never type fallback` inference in two internal functions
(`connection/executor.rs` and `copy.rs`). This is dependency code — it cannot be
suppressed with `#[allow]` in our source.

### The fix

Upgrade `sqlx` from `0.7` to `0.8`. The workspace pin is in `core-rs/Cargo.toml`:

```toml
sqlx = { version = "0.7", features = [...] }
```

sqlx 0.8 ships a fixed version of `sqlx-postgres` that does not trigger this warning.

### Why upgrade is deferred

sqlx 0.7 → 0.8 is a semver-major bump. Known breaking changes include:

- `FromRow` derive attribute changes
- `query_as!` / `query!` macro output type changes in some edge cases
- Connection pool configuration API changes
- `runtime-tokio` feature restructuring

The workspace has extensive `sqlx::query!` macro usage across `mqk-db`, `mqk-testkit`,
and other crates. An upgrade requires verifying all macro expansions, migration behavior,
and DB type mappings still hold. This is a non-trivial audit scope.

**Doing it wrong would be worse than deferring** — a silent semantic change in DB
type handling or query execution is a real-money risk. The upgrade belongs in its own
dedicated patch under the standard one-patch-per-turn discipline.

### Proof output impact

The warning appears in cargo build output during `full_repo_proof.ps1` and in CI.
It does not fail any lane. It is visible noise in the transcript.

The `-LowMemory` profile (HARNESS-01) and the Windows CI lane (CI-PLATFORM-01)
both exhibit this warning identically. This is expected and documented.

### Remaining limitation

Until sqlx 0.8 is adopted, this warning will appear in all build output. The repo
is on a known deprecation clock. The Rust 2024 edition adoption date will determine
when this becomes a hard error in practice.

### Tracking

- Governance captured here: `docs/ci/dependency_governance.md`
- Cargo.toml workspace comment references this doc (see `[workspace.dependencies]`)
- Future patch: `DEP-GOV-01-UPGRADE` (sqlx 0.7 → 0.8 full upgrade)
