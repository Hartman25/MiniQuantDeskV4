// core-rs/crates/mqk-db/src/test_support.rs
//
// FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 3 /
// FULL-AUDIT-CHECKPOINT-HARDENING-REPAIR-01 Part 2: disposable per-test
// Postgres database support, hardened and moved out of the default
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
//
// FULL-AUDIT-CHECKPOINT-HARDENING-REPAIR-01 Part 2 hardening: the previous
// version's cancellation safety only began the moment a `DisposableTestDb`
// value existed (i.e. after CREATE DATABASE, target connect, *and* migrate
// had all already succeeded) and relied solely on that value's `Drop` impl
// firing a single detached, unobservable `tokio::runtime::Handle::spawn`
// call. That left every earlier await point -- the admin connect, the
// CREATE DATABASE statement itself, the target connect, and migrate --
// completely unowned: a cancellation landing in any of them (the caller's
// future dropped, e.g. by a timeout or an aborted task) could leave a
// durable `mqk_disp_*` database with nothing left alive to clean it up.
//
// The fix is `CleanupAuthority`: an eagerly-spawned, independent background
// task (not work done inside a `Drop` impl) that owns the eventual
// best-effort `DROP DATABASE IF EXISTS ... WITH (FORCE)` for one specific
// `mqk_disp_*` name, created *before* the admin connection is even opened.
// The authority is carried as a plain local value through every later
// await; if the enclosing future is cancelled at any point before an
// explicit, awaited teardown calls `commit()`, the value's implicit drop
// (no custom `Drop` impl needed) drops its `oneshot::Sender`, which the
// already-running background task observes as "please clean up" and acts
// on with its own retry loop. `DROP DATABASE IF EXISTS` makes both possible
// outcomes of a cancellation racing the CREATE statement itself safe: if
// the statement never actually completed server-side, the drop is a
// harmless no-op; if it did complete, the drop removes it. The one
// residual case this cannot fully close -- the exact instant a client-side
// cancellation reaches the network layer while Postgres is mid-execution of
// the CREATE statement -- is a Postgres protocol-level race, not a gap in
// this module's ownership tracking, and is covered by retrying the drop.
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::Result;
use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::sync::{oneshot, Notify};
use tokio::task::JoinHandle;

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

/// Every name this module ever generates or ever tears down carries this
/// prefix. Asserted before every cleanup DROP so that no future refactor of
/// this file, however careless, can be made to target the shared `mqk_test`
/// database — the check is structural, not just an emergent property of
/// today's call sites.
const DISPOSABLE_DB_PREFIX: &str = "mqk_disp_";

fn assert_is_disposable_db_name(db_name: &str) {
    assert!(
        db_name.starts_with(DISPOSABLE_DB_PREFIX),
        "refusing to run disposable-database cleanup against a name outside the {DISPOSABLE_DB_PREFIX} \
         namespace (this would risk the shared mqk_test database): {db_name}"
    );
}

/// Best-effort DROP DATABASE, retried up to `attempts` times (minimum one)
/// with a short backoff between tries so a transient admin-connect hiccup
/// does not masquerade as a permanent leak. Returns whether the final
/// attempt succeeded. `IF EXISTS` makes a database that was never actually
/// created (a cancellation that raced ahead of the server completing
/// CREATE DATABASE) or one some other caller already dropped a trivial
/// success rather than a failure. `WITH (FORCE)` (Postgres 13+) terminates
/// any other backend still holding a connection open, so this still
/// succeeds against a pool with live connections.
async fn drop_with_retries(admin_url: &str, db_name: &str, attempts: u32) -> bool {
    assert_is_disposable_db_name(db_name);
    for attempt in 0..attempts.max(1) {
        if attempt > 0 {
            tokio::time::sleep(Duration::from_millis(100 * u64::from(attempt))).await;
        }
        if let Ok(admin_pool) = PgPoolOptions::new()
            .max_connections(1)
            .connect(admin_url)
            .await
        {
            let result = sqlx::query(&format!(
                "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
            ))
            .execute(&admin_pool)
            .await;
            admin_pool.close().await;
            if result.is_ok() {
                return true;
            }
        }
    }
    false
}

// ---------------------------------------------------------------------------
// Test-only synchronization hooks: let an external integration test
// deterministically cancel `create_disposable_test_db` at an exact point in
// its lifecycle, or intervene from a second connection while the call is
// parked, instead of racing a sleep against real Postgres round-trip
// latency (a `Notify`-barrier rendezvous, not a sleep poll).
// ---------------------------------------------------------------------------

/// A two-way rendezvous: `create_disposable_test_db_with_hooks` notifies
/// `reached` the instant it arrives at the named point, then blocks on
/// `hold` until the test either releases it (`hold.notify_one()`) or aborts
/// the awaiting task outright. This gives a test full, deterministic
/// control over whether/when execution proceeds past that exact point.
#[derive(Clone, Default)]
pub struct TestBarrier {
    pub reached: Arc<Notify>,
    pub hold: Arc<Notify>,
}

impl TestBarrier {
    pub fn new() -> Self {
        Self {
            reached: Arc::new(Notify::new()),
            hold: Arc::new(Notify::new()),
        }
    }
}

/// Populated by `create_disposable_test_db_with_hooks` as soon as the
/// corresponding internal state exists, so a test that aborts the enclosing
/// task before it can return normally can still retrieve the generated
/// `db_name`/`admin_url` and the background cleanup task's `JoinHandle` for
/// fully deterministic (non-sleep) assertions about the eventual state.
#[derive(Default)]
pub struct TestObservations {
    pub db_name: Option<String>,
    pub admin_url: Option<String>,
    pub cleanup_join: Option<JoinHandle<CleanupOutcome>>,
}

/// Outcome reported by a `CleanupAuthority`'s background task, exposed for
/// test assertions. Production callers never inspect this; they only call
/// `commit()` or let the value drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// An explicit, awaited teardown already ran (or is running); this task
    /// took no action.
    Committed,
    /// The authority was dropped without a commit (cancellation, or an
    /// early-return path that intentionally leaves it armed); the task ran
    /// its own retry-capable best-effort DROP DATABASE.
    CleanedUp { succeeded: bool },
}

/// Test-only knobs for `create_disposable_test_db_with_hooks`. The plain
/// `create_disposable_test_db` used by every ordinary caller passes
/// `Default::default()`, so production behavior is completely unaffected.
#[derive(Clone, Default)]
pub struct DisposableDbTestHooks {
    /// Rendezvous immediately before the CREATE DATABASE statement is
    /// issued (used to race a cancellation against that statement itself).
    pub before_create: Option<TestBarrier>,
    /// Rendezvous immediately after CREATE DATABASE succeeds, before the
    /// target-database connect is attempted.
    pub before_target_connect: Option<TestBarrier>,
    /// Rendezvous immediately after the target-database connect succeeds,
    /// before `migrate()` is called.
    pub before_migrate: Option<TestBarrier>,
    pub observer: Option<Arc<Mutex<TestObservations>>>,
}

/// Owns the eventual DROP DATABASE cleanup for exactly one `mqk_disp_*`
/// database, from the moment it is constructed until an explicit, awaited
/// teardown calls `commit()`. Spawned eagerly as an independent Tokio task
/// (never inside a `Drop` impl), so that dropping this guard -- whether
/// from an explicit early return, a panic unwinding through it, or the
/// enclosing future being cancelled and never polled again -- reliably
/// triggers cleanup: the task keeps running under its own poll loop,
/// entirely independent of whatever caused this guard to be dropped. No
/// custom `Drop` impl is needed: letting `commit_tx` (an
/// `Option<oneshot::Sender<()>>`) fall out of scope while still `Some` is
/// itself the signal the background task is waiting on.
struct CleanupAuthority {
    commit_tx: Option<oneshot::Sender<()>>,
}

impl CleanupAuthority {
    fn spawn(
        admin_url: String,
        db_name: String,
        observer: Option<&Arc<Mutex<TestObservations>>>,
    ) -> Self {
        let (commit_tx, commit_rx) = oneshot::channel::<()>();
        let join = tokio::spawn(async move {
            match commit_rx.await {
                Ok(()) => CleanupOutcome::Committed,
                Err(_recv_error) => {
                    let succeeded = drop_with_retries(&admin_url, &db_name, 3).await;
                    CleanupOutcome::CleanedUp { succeeded }
                }
            }
        });
        if let Some(observer) = observer {
            let mut observations = observer
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            observations.cleanup_join = Some(join);
        }
        // If no observer wants it, `join` is simply dropped here, which
        // detaches the task -- it still runs to completion on the runtime;
        // only the ability to `.await` its result is lost, and production
        // code never needs that.
        Self {
            commit_tx: Some(commit_tx),
        }
    }

    /// Suppresses the background cleanup: an explicit, awaited teardown
    /// already ran (or is running) elsewhere. Consumes `self` so it cannot
    /// be called twice and cannot race with an implicit drop.
    fn commit(mut self) {
        if let Some(tx) = self.commit_tx.take() {
            let _ = tx.send(());
        }
    }
}

/// A disposable, migrated Postgres database created for exactly one test
/// (or one test's own private scope), on the same server as
/// `MQK_DATABASE_URL`. Never touches any other test's rows because nothing
/// else connects to it.
///
/// Cancellation safety: `cleanup` (a `CleanupAuthority`) is armed from
/// before this value's `create_disposable_test_db` call ever opened an
/// admin connection, through CREATE DATABASE, the target connect, and
/// migrate. If this value is dropped without [`DisposableTestDb::drop_database`]
/// ever having been called and completing successfully, the still-armed
/// `cleanup` authority's own drop triggers its background task's
/// retry-capable teardown — see the module-level doc-comment for the full
/// design and the one residual, protocol-level race this cannot fully
/// close.
pub struct DisposableTestDb {
    pub pool: PgPool,
    db_name: String,
    admin_url: String,
    cleanup: Option<CleanupAuthority>,
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
    /// On success, suppresses the `CleanupAuthority` background fallback
    /// (it already happened here, synchronously and awaited); on failure,
    /// leaves it armed so the fallback still gets a chance to clean up when
    /// `self` is finally dropped.
    pub async fn drop_database(mut self) -> Result<(), DisposableDbError> {
        assert_is_disposable_db_name(&self.db_name);
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
        if let Some(cleanup) = self.cleanup.take() {
            cleanup.commit();
        }
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
static DISPOSABLE_DB_COUNTER: AtomicU64 = AtomicU64::new(0);

pub async fn create_disposable_test_db(label: &str) -> Result<DisposableTestDb, DisposableDbError> {
    create_disposable_test_db_with_hooks(label, DisposableDbTestHooks::default()).await
}

/// Full implementation behind `create_disposable_test_db`, parameterized by
/// test-only synchronization hooks (see `DisposableDbTestHooks`). Every
/// exit path after `db_name` is generated is covered by `cleanup`
/// (a `CleanupAuthority`, spawned before the admin connection is even
/// opened): a normal error return explicitly awaits a cleanup attempt and
/// then commits the authority; a cancellation at any await point instead
/// leaves the authority armed, and its own drop hands off to the
/// already-running background task.
pub async fn create_disposable_test_db_with_hooks(
    label: &str,
    hooks: DisposableDbTestHooks,
) -> Result<DisposableTestDb, DisposableDbError> {
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
    let sequence = DISPOSABLE_DB_COUNTER.fetch_add(1, Ordering::Relaxed);
    let db_name = format!(
        "{DISPOSABLE_DB_PREFIX}{sanitized}_{}_{sequence}",
        std::process::id()
    );
    let admin_url = build_db_url(&base, "postgres", &query);

    if let Some(observer) = &hooks.observer {
        let mut observations = observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        observations.db_name = Some(db_name.clone());
        observations.admin_url = Some(admin_url.clone());
    }

    // Armed before any I/O for this database happens: db_name is already
    // fixed, so a cancellation at *any* later await -- including mid-flight
    // inside the CREATE DATABASE statement itself -- still leaves a
    // background task that knows exactly which database to retry-drop.
    let cleanup =
        CleanupAuthority::spawn(admin_url.clone(), db_name.clone(), hooks.observer.as_ref());

    if let Some(barrier) = &hooks.before_create {
        barrier.reached.notify_one();
        barrier.hold.notified().await;
    }

    let result = create_disposable_test_db_inner(
        &base,
        &admin_url,
        &db_name,
        &query,
        hooks.before_target_connect.as_ref(),
        hooks.before_migrate.as_ref(),
    )
    .await;

    match result {
        Ok(pool) => Ok(DisposableTestDb {
            pool,
            db_name,
            admin_url,
            cleanup: Some(cleanup),
        }),
        Err(e) => {
            // Ordinary (non-cancelled) failure: await cleanup synchronously
            // so the returned Err already implies the database is gone,
            // then commit the background authority so it does not also
            // attempt a redundant (harmless but wasteful) drop.
            drop_with_retries(&admin_url, &db_name, 1).await;
            cleanup.commit();
            Err(e)
        }
    }
}

/// CREATE DATABASE, then connect to it, then migrate it. Isolated from
/// `create_disposable_test_db_with_hooks` purely so that function's
/// `cleanup` authority and hook-observer wiring stay in one place while
/// this one stays a straight-line happy/error path.
async fn create_disposable_test_db_inner(
    base: &str,
    admin_url: &str,
    db_name: &str,
    query: &Option<String>,
    before_target_connect: Option<&TestBarrier>,
    before_migrate: Option<&TestBarrier>,
) -> Result<PgPool, DisposableDbError> {
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url)
        .await
        .map_err(DisposableDbError::Connect)?;
    let create_result = sqlx::query(&format!("CREATE DATABASE \"{db_name}\""))
        .execute(&admin_pool)
        .await;
    admin_pool.close().await;
    create_result.map_err(DisposableDbError::Create)?;

    if let Some(barrier) = before_target_connect {
        barrier.reached.notify_one();
        barrier.hold.notified().await;
    }

    let target_url = build_db_url(base, db_name, query);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&target_url)
        .await
        .map_err(DisposableDbError::Connect)?;

    if let Some(barrier) = before_migrate {
        barrier.reached.notify_one();
        barrier.hold.notified().await;
    }

    if let Err(e) = migrate(&pool).await {
        pool.close().await;
        return Err(DisposableDbError::Migrate(e));
    }
    Ok(pool)
}

/// Runs `test` against a fresh disposable database created just for this
/// call, and guarantees the database is dropped afterward — including when
/// `test` panics (an assertion failure), by running it inside `tokio::spawn`
/// and catching the join error before propagating the original panic, and
/// including when the caller of `run_isolated` itself is cancelled after the
/// disposable database was created (see `DisposableTestDb`'s cancellation
/// safety doc-comment). This is the FULL-AUDIT-FAIL-017 replacement for
/// global shared-DB deletion helpers and `Utc::now()`-based "latest row"
/// racing: each caller gets an empty database, so there is no other test's
/// row to collide with and no global query to race.
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
    let join_result = handle.await;
    // Teardown runs unconditionally before inspecting the test result, so a
    // panicking test still gets its database dropped.
    let teardown_result = disposable.drop_database().await;

    let test_result: std::thread::Result<()> = match join_result {
        Ok(()) => Ok(()),
        Err(join_err) => {
            match join_err.try_into_panic() {
                Ok(panic_payload) => Err(panic_payload),
                Err(join_err) => {
                    panic!("run_isolated({label}): test task did not panic but failed to join: {join_err}")
                }
            }
        }
    };
    finish_run_isolated(label, test_result, teardown_result);
}

/// Pure decision logic for what `run_isolated` must do once the test task
/// has been joined (already reduced to a plain `std::thread::Result<()>` —
/// `Ok` for a clean run, `Err(panic_payload)` for a propagated panic) and
/// the disposable database's teardown has been attempted. Kept as a
/// synchronous, I/O-free function so every required combination — including
/// the two where the test closure itself ran cleanly but the call must
/// still be reported as failed — can be proven by a fast unit test that
/// never needs a live Postgres instance to force a teardown failure; only a
/// `DisposableDbError` needs to be constructed synthetically.
fn finish_run_isolated(
    label: &str,
    test_result: std::thread::Result<()>,
    teardown_result: Result<(), DisposableDbError>,
) {
    match (test_result, teardown_result) {
        (Ok(()), Ok(())) => {}
        (Ok(()), Err(teardown_err)) => {
            panic!(
                "run_isolated({label}): test succeeded but disposable database teardown failed: {teardown_err}"
            );
        }
        (Err(panic_payload), teardown_result) => {
            if let Err(teardown_err) = &teardown_result {
                eprintln!(
                    "run_isolated({label}): test panicked AND disposable database teardown failed: {teardown_err}"
                );
            }
            std::panic::resume_unwind(panic_payload);
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

    #[test]
    #[should_panic(expected = "refusing to run disposable-database cleanup")]
    fn assert_is_disposable_db_name_rejects_the_shared_test_database() {
        assert_is_disposable_db_name("mqk_test");
    }

    #[test]
    fn assert_is_disposable_db_name_accepts_a_generated_name() {
        assert_is_disposable_db_name("mqk_disp_label_1234_0");
    }

    // -- finish_run_isolated: pure decision-tree coverage, no DB needed ----

    #[test]
    fn finish_run_isolated_passes_when_test_and_teardown_both_succeed() {
        finish_run_isolated("t", Ok(()), Ok(()));
    }

    #[test]
    #[should_panic(expected = "test succeeded but disposable database teardown failed")]
    fn finish_run_isolated_fails_when_teardown_fails_after_a_successful_test() {
        let teardown_err = DisposableDbError::Drop {
            db_name: "mqk_disp_synthetic".to_string(),
            source: sqlx::Error::RowNotFound,
        };
        finish_run_isolated("t", Ok(()), Err(teardown_err));
    }

    #[test]
    #[should_panic(expected = "original panic message")]
    fn finish_run_isolated_resumes_the_original_panic_when_teardown_succeeds() {
        let payload: Box<dyn std::any::Any + Send> = Box::new("original panic message".to_string());
        finish_run_isolated("t", Err(payload), Ok(()));
    }

    #[test]
    #[should_panic(expected = "original panic message")]
    fn finish_run_isolated_still_resumes_the_original_panic_when_teardown_also_fails() {
        // Both failures occurred: the panic payload is what the test
        // harness ultimately reports (proven by #[should_panic] below), and
        // the teardown error is additionally reported to stderr by
        // finish_run_isolated before the panic is resumed -- run this test
        // with `cargo test -- --nocapture` to see both lines.
        let payload: Box<dyn std::any::Any + Send> = Box::new("original panic message".to_string());
        let teardown_err = DisposableDbError::Drop {
            db_name: "mqk_disp_synthetic".to_string(),
            source: sqlx::Error::RowNotFound,
        };
        finish_run_isolated("t", Err(payload), Err(teardown_err));
    }

    // -- CleanupAuthority: commit vs. drop-without-commit, no real Postgres
    // server needed -- 127.0.0.1:1 refuses the connection immediately, so
    // these are fast and fully deterministic without any network fixture.

    const UNREACHABLE_ADMIN_URL: &str = "postgres://user:pass@127.0.0.1:1/postgres";

    #[tokio::test]
    async fn cleanup_authority_commit_prevents_the_background_cleanup_attempt() {
        let observer = Arc::new(Mutex::new(TestObservations::default()));
        let cleanup = CleanupAuthority::spawn(
            UNREACHABLE_ADMIN_URL.to_string(),
            "mqk_disp_unittest_committed".to_string(),
            Some(&observer),
        );
        cleanup.commit();
        let join = observer
            .lock()
            .unwrap()
            .cleanup_join
            .take()
            .expect("join handle observed");
        let outcome = join.await.expect("cleanup task must not panic");
        assert_eq!(outcome, CleanupOutcome::Committed);
    }

    #[tokio::test]
    async fn cleanup_authority_dropped_without_commit_runs_the_cleanup_attempt() {
        let observer = Arc::new(Mutex::new(TestObservations::default()));
        let cleanup = CleanupAuthority::spawn(
            UNREACHABLE_ADMIN_URL.to_string(),
            "mqk_disp_unittest_dropped".to_string(),
            Some(&observer),
        );
        drop(cleanup); // no commit -> sender drops -> background task must act
        let join = observer
            .lock()
            .unwrap()
            .cleanup_join
            .take()
            .expect("join handle observed");
        let outcome = join.await.expect("cleanup task must not panic");
        assert_eq!(
            outcome,
            CleanupOutcome::CleanedUp { succeeded: false },
            "connecting to 127.0.0.1:1 must fail every retry attempt, proving the background task really tried"
        );
    }
}
