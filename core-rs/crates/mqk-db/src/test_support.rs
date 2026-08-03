// core-rs/crates/mqk-db/src/test_support.rs
//
// FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 3: disposable
// per-test Postgres database support, hardened and moved out of the default
// production `mqk-db` public API.
//
// This entire module is gated at its `mod test_support;` declaration in
// lib.rs behind `#[cfg(any(test, feature = "testkit"))]` -- it is compiled
// into mqk-db's own unit-test builds automatically, and into any external
// crate's test builds only when that crate explicitly opts in via
// `mqk-db = { path = "...", features = ["testkit"] }` in its own
// `[dev-dependencies]` (never `[dependencies]` -- see
// scripts/guards/check_disposable_db_not_in_production.sh, which fails CI if
// any crate's production `[dependencies]` section enables this feature).
// No production daemon/runtime binary can reference this module.
//
// Why a disposable database per test at all: some production queries are
// inherently global/singleton/bounded (e.g. "the latest run for this
// engine", a bounded global feed window) and therefore cannot be isolated
// from other concurrently-running tests by per-fixture-ID scoping against
// the shared `MQK_DATABASE_URL` database. Each disposable database is a
// throwaway Postgres database that only one test ever connects to, so there
// is no other test's row to collide with and no global query to race.
// FULL-AUDIT-FAIL-017.
use std::fmt;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use crate::{migrate, ENV_DB_URL};

// ---------------------------------------------------------------------------
// Bounded, typed errors -- replaces the previous `.expect()`-panic-on-
// malformed-input approach in `split_db_url` with a `Result` a caller can
// match on, and gives every disposable-DB failure mode (create/connect/
// migrate/drop) its own variant instead of an opaque anyhow string.
// ---------------------------------------------------------------------------
#[derive(Debug)]
pub enum DisposableDbError {
    MissingEnvVar(String),
    MalformedUrl {
        url_redacted: String,
        reason: &'static str,
    },
    Create(sqlx::Error),
    Connect(sqlx::Error),
    Migrate(anyhow::Error),
    Drop {
        db_name: String,
        source: sqlx::Error,
    },
}

impl fmt::Display for DisposableDbError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingEnvVar(name) => write!(f, "missing env var {name}"),
            Self::MalformedUrl {
                url_redacted,
                reason,
            } => write!(f, "malformed database URL ({reason}): {url_redacted}"),
            Self::Create(e) => write!(f, "failed to create disposable database: {e}"),
            Self::Connect(e) => write!(f, "failed to connect to disposable database: {e}"),
            Self::Migrate(e) => write!(f, "failed to migrate disposable database: {e}"),
            Self::Drop { db_name, source } => {
                write!(f, "failed to drop disposable database {db_name}: {source}")
            }
        }
    }
}

impl std::error::Error for DisposableDbError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Create(e) | Self::Connect(e) => Some(e),
            Self::Drop { source, .. } => Some(source),
            Self::Migrate(e) => e.source(),
            Self::MissingEnvVar(_) | Self::MalformedUrl { .. } => None,
        }
    }
}

/// Redacts credentials out of a Postgres URL for safe inclusion in an error
/// message (never log the raw `MQK_DATABASE_URL`).
fn redact_url(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let rest = &url[scheme_end + 3..];
        if let Some(at) = rest.find('@') {
            let mut s = url.to_string();
            s.replace_range(scheme_end + 3..scheme_end + 3 + at, "****:****");
            return s;
        }
    }
    url.to_string()
}

/// Splits a Postgres URL into `(everything before the final path segment,
/// database name, optional query string)`. Pure string manipulation — every
/// URL used by this repo's test harness (`MQK_DATABASE_URL`) is a plain
/// `postgres://user:pass@host:port/dbname[?params]` with no path segments
/// beyond the database name.
fn split_db_url(url: &str) -> Result<(String, String, Option<String>), DisposableDbError> {
    let (path_part, query) = match url.split_once('?') {
        Some((p, q)) => (p.to_string(), Some(q.to_string())),
        None => (url.to_string(), None),
    };
    let idx = path_part
        .rfind('/')
        .ok_or_else(|| DisposableDbError::MalformedUrl {
            url_redacted: redact_url(&path_part),
            reason: "no '/<dbname>' path segment found",
        })?;
    Ok((
        path_part[..idx].to_string(),
        path_part[idx + 1..].to_string(),
        query,
    ))
}

fn build_db_url(base: &str, db_name: &str, query: &Option<String>) -> String {
    match query {
        Some(q) => format!("{base}/{db_name}?{q}"),
        None => format!("{base}/{db_name}"),
    }
}

/// Best-effort, fire-and-forget drop of a disposable database. Used as the
/// fallback cleanup path (from a synchronous `Drop` impl via
/// `tokio::runtime::Handle::spawn`, and from early-return cleanup inside
/// `create_disposable_test_db`) — errors are intentionally swallowed here
/// because this is *not* the primary, awaited teardown path
/// (`DisposableTestDb::drop_database`); it exists only so a database created
/// just before a connect/migrate failure, panic, or future cancellation does
/// not silently outlive the process.
async fn drop_created_database_best_effort(admin_url: &str, db_name: &str) {
    if let Ok(admin_pool) = PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url)
        .await
    {
        let _ = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
        ))
        .execute(&admin_pool)
        .await;
        admin_pool.close().await;
    }
}

/// A disposable, migrated Postgres database created for exactly one test
/// (or one test's own private scope), on the same server as
/// `MQK_DATABASE_URL`. Never touches any other test's rows because nothing
/// else connects to it.
///
/// Cancellation safety: if this value is dropped without
/// [`DisposableTestDb::drop_database`] ever having been called and
/// completed successfully (e.g. the owning future — `run_isolated` or a
/// caller's own use of this type — is cancelled, or the process is
/// interrupted between construction and explicit teardown), `Drop` schedules
/// a best-effort background cleanup via `tokio::runtime::Handle::spawn` so
/// the disposable database does not outlive the value that owns it. This
/// covers cancellation *after* the database has been created, connected to,
/// and migrated (i.e. after this value exists) — the narrower window of a
/// cancellation landing exactly inside `create_disposable_test_db`'s own
/// CREATE-DATABASE `.await` (before any `DisposableTestDb` exists to carry a
/// guard) is not closable from the client side alone, since Postgres does
/// not roll back completed DDL just because the client connection dropped;
/// a periodic external janitor for stray `mqk_disp_*` databases is the
/// appropriate defense-in-depth for that narrow residual case and is out of
/// scope for this per-test helper.
pub struct DisposableTestDb {
    pub pool: PgPool,
    db_name: String,
    admin_url: String,
    armed: bool,
}

impl Drop for DisposableTestDb {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let db_name = self.db_name.clone();
        let admin_url = self.admin_url.clone();
        if let Ok(handle) = tokio::runtime::Handle::try_current() {
            handle.spawn(async move {
                drop_created_database_best_effort(&admin_url, &db_name).await;
            });
        }
        // If no Tokio runtime is current, cleanup cannot be scheduled here;
        // the unique mqk_disp_* name still prevents any collision with a
        // future test run.
    }
}

impl DisposableTestDb {
    /// The generated `mqk_disp_*` database name. Exposed for test assertions
    /// (e.g. proving concurrent creation yields unique names, or checking
    /// `pg_database` directly after teardown) — never used by production
    /// code, since this whole module is test-only.
    pub fn db_name(&self) -> &str {
        &self.db_name
    }

    /// Drops the disposable database. Must be called after `self.pool` (and
    /// any clone of it) is done being used — this closes `self.pool` first.
    /// On success, disarms the `Drop`-based fallback cleanup (it already
    /// happened here); on failure, leaves it armed so the fallback still
    /// gets a chance to clean up when `self` is finally dropped.
    pub async fn drop_database(mut self) -> Result<(), DisposableDbError> {
        self.pool.close().await;
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(&self.admin_url)
            .await
            .map_err(DisposableDbError::Connect)?;
        // WITH (FORCE) (Postgres 13+) terminates any other backend still
        // holding a connection open against this database, so a teardown
        // that races an in-flight pooled connection still succeeds.
        let result = sqlx::query(&format!(
            "DROP DATABASE IF EXISTS \"{}\" WITH (FORCE)",
            self.db_name
        ))
        .execute(&admin_pool)
        .await;
        admin_pool.close().await;
        result.map_err(|source| DisposableDbError::Drop {
            db_name: self.db_name.clone(),
            source,
        })?;
        self.armed = false;
        Ok(())
    }
}

/// Creates a fresh, migrated, uniquely-named database on the same Postgres
/// server as `MQK_DATABASE_URL` (port 5434 in this repo's test harness), and
/// returns a pool connected to it. `label` is sanitized to `[a-z0-9_]` and
/// truncated so the generated name stays within Postgres' 63-byte identifier
/// limit; the OS process ID plus a per-process monotonic counter guarantee
/// uniqueness across concurrent test runs (deterministic inputs only --
/// `Uuid::new_v4()` is disallowed in this crate's production src/ by
/// scripts/guards/check_unsafe_patterns.ps1/.sh, so this does not use it).
///
/// Every exit path after the `CREATE DATABASE` succeeds either returns a
/// live `DisposableTestDb` (which itself now guards cleanup, see its
/// doc-comment) or drops the just-created database before returning an
/// error — a connect failure or a migration failure never leaks the
/// database it was diagnosing.
static DISPOSABLE_DB_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

pub async fn create_disposable_test_db(label: &str) -> Result<DisposableTestDb, DisposableDbError> {
    let base_url = std::env::var(ENV_DB_URL)
        .map_err(|_| DisposableDbError::MissingEnvVar(ENV_DB_URL.to_string()))?;
    let (base, _orig_db, query) = split_db_url(&base_url)?;

    let sanitized: String = label
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() {
                c.to_ascii_lowercase()
            } else {
                '_'
            }
        })
        .collect();
    let sanitized: String = sanitized.chars().take(24).collect();
    let sequence = DISPOSABLE_DB_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let db_name = format!("mqk_disp_{sanitized}_{}_{sequence}", std::process::id());

    let admin_url = build_db_url(&base, "postgres", &query);
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .map_err(DisposableDbError::Connect)?;
    let create_result = sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin_pool)
        .await;
    admin_pool.close().await;
    create_result.map_err(DisposableDbError::Create)?;
    // From this point on, the database exists on the server. Every
    // remaining exit path below must drop it before returning an error.

    let target_url = build_db_url(&base, &db_name, &query);
    let pool = match PgPoolOptions::new()
        .max_connections(5)
        .connect(&target_url)
        .await
    {
        Ok(pool) => pool,
        Err(e) => {
            drop_created_database_best_effort(&admin_url, &db_name).await;
            return Err(DisposableDbError::Connect(e));
        }
    };

    if let Err(e) = migrate(&pool).await {
        pool.close().await;
        drop_created_database_best_effort(&admin_url, &db_name).await;
        return Err(DisposableDbError::Migrate(e));
    }

    Ok(DisposableTestDb {
        pool,
        db_name,
        admin_url,
        armed: true,
    })
}

/// Runs `test` against a fresh disposable database created just for this
/// call, and guarantees the database is dropped afterward — including when
/// `test` panics (an assertion failure), by running it inside `tokio::spawn`
/// and catching the join error before propagating the original panic, and
/// including when the caller of `run_isolated` itself is cancelled after the
/// disposable database was created (see `DisposableTestDb`'s `Drop` impl).
/// This is the FULL-AUDIT-FAIL-017 replacement for global shared-DB deletion
/// helpers and `Utc::now()`-based "latest row" racing: each caller gets an
/// empty database, so there is no other test's row to collide with and no
/// global query to race.
pub async fn run_isolated<F, Fut>(label: &str, test: F)
where
    F: FnOnce(PgPool) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let disposable = create_disposable_test_db(label)
        .await
        .expect("create_disposable_test_db");
    let pool = disposable.pool.clone();
    let handle = tokio::spawn(async move { test(pool).await });
    let result = handle.await;
    // Teardown runs unconditionally before inspecting the test result, so a
    // panicking test still gets its database dropped. A teardown error is
    // logged rather than propagated via `.expect()`: masking the *test's*
    // panic/result with an unrelated teardown failure would hide the real
    // signal, and the `Drop` guard on `disposable` is still armed to retry
    // best-effort cleanup in the background when it goes out of scope here.
    if let Err(e) = disposable.drop_database().await {
        eprintln!(
            "run_isolated({label}): disposable database teardown reported an error \
             (background fallback cleanup will still attempt to drop it): {e}"
        );
    }
    if let Err(join_err) = result {
        match join_err.try_into_panic() {
            Ok(panic_payload) => std::panic::resume_unwind(panic_payload),
            Err(join_err) => {
                panic!("run_isolated: test task did not panic but failed to join: {join_err}")
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_db_url_rejects_malformed_url_with_a_typed_error_not_a_panic() {
        let err = split_db_url("not-a-url-with-no-slash").expect_err("expected a typed error");
        match err {
            DisposableDbError::MalformedUrl { reason, .. } => {
                assert_eq!(reason, "no '/<dbname>' path segment found");
            }
            other => panic!("expected MalformedUrl, got {other:?}"),
        }
    }

    #[test]
    fn split_db_url_splits_host_dbname_and_query() {
        let (base, db, query) =
            split_db_url("postgres://user:pass@127.0.0.1:5434/mqk_test?sslmode=disable")
                .expect("well-formed URL should split cleanly");
        assert_eq!(base, "postgres://user:pass@127.0.0.1:5434");
        assert_eq!(db, "mqk_test");
        assert_eq!(query.as_deref(), Some("sslmode=disable"));
    }

    #[test]
    fn split_db_url_handles_no_query_string() {
        let (base, db, query) = split_db_url("postgres://user:pass@127.0.0.1:5434/mqk_test")
            .expect("well-formed URL without a query string should split cleanly");
        assert_eq!(base, "postgres://user:pass@127.0.0.1:5434");
        assert_eq!(db, "mqk_test");
        assert_eq!(query, None);
    }

    #[test]
    fn build_db_url_round_trips_with_and_without_query() {
        assert_eq!(
            build_db_url("postgres://user:pass@127.0.0.1:5434", "mqk_disp_x", &None),
            "postgres://user:pass@127.0.0.1:5434/mqk_disp_x"
        );
        assert_eq!(
            build_db_url(
                "postgres://user:pass@127.0.0.1:5434",
                "mqk_disp_x",
                &Some("sslmode=disable".to_string())
            ),
            "postgres://user:pass@127.0.0.1:5434/mqk_disp_x?sslmode=disable"
        );
    }

    #[test]
    fn redact_url_hides_credentials_but_keeps_host_and_db() {
        let redacted = redact_url("postgres://user:supersecret@127.0.0.1:5434/mqk_test");
        assert!(!redacted.contains("supersecret"));
        assert!(!redacted.contains("user:supersecret"));
        assert!(redacted.contains("127.0.0.1:5434/mqk_test"));
    }

    #[test]
    fn redact_url_leaves_a_url_without_credentials_unchanged() {
        let redacted = redact_url("postgres://127.0.0.1:5434/mqk_test");
        assert_eq!(redacted, "postgres://127.0.0.1:5434/mqk_test");
    }

    #[test]
    fn disposable_db_error_display_messages_are_bounded_and_readable() {
        let err = DisposableDbError::MissingEnvVar("MQK_DATABASE_URL".to_string());
        assert_eq!(err.to_string(), "missing env var MQK_DATABASE_URL");

        let err = DisposableDbError::MalformedUrl {
            url_redacted: "postgres://host/nodb".to_string(),
            reason: "no '/<dbname>' path segment found",
        };
        assert!(err
            .to_string()
            .contains("no '/<dbname>' path segment found"));
    }
}
