// core-rs/crates/mqk-db/src/test_support.rs
//
// FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 3 /
// FULL-AUDIT-CHECKPOINT-HARDENING-REPAIR-01 Part 2 /
// FULL-AUDIT-TESTKIT-AND-IGNORED-AUTHORITY-FINAL-REPAIR-01 Part 1:
// disposable per-test Postgres database support, hardened and moved out of
// the default production `mqk-db` public API.
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
// Ownership architecture (FULL-AUDIT-TESTKIT-AND-IGNORED-AUTHORITY-FINAL-
// REPAIR-01 Part 1): a prior revision ran admin-connect / CREATE DATABASE /
// target-connect / migrate directly inside the async fn the caller itself
// awaits, guarded only by a `CleanupAuthority` value carried through that
// same call stack. That left the entire setup lifecycle at the mercy of the
// caller's own cancellation: if the caller's future was dropped (a timeout,
// an aborted task) while a query was in flight, the client-side future
// vanished but the server could still be mid-execution of that statement --
// racing a background `DROP DATABASE IF EXISTS` against a still-running
// `CREATE DATABASE` is unsafe, because `IF EXISTS` can observe "not created
// yet", succeed trivially, and never retry, after which the `CREATE`
// completes server-side and leaks a `mqk_disp_*` database with nothing left
// alive to ever clean it up.
//
// The fix is to make the entire setup lifecycle run inside its own eagerly-
// spawned Tokio task (`run_setup_owner`, launched via `tokio::spawn`, never
// inside a `Drop` impl and never inside the future the caller directly
// awaits) so that the caller dropping its own future cannot cancel it. The
// result -- the finished `DisposableTestDb` or a `DisposableDbError` -- is
// delivered back through a `oneshot` channel. Two disjoint cases follow from
// that channel's own cancellation semantics:
//
//   * The receiver is still alive: the result is handed off normally, and
//     from that point on cancellation safety is the caller's own affair
//     (dropping an already-fully-constructed `DisposableTestDb` without
//     calling `drop_database()` is an ordinary synchronous `Drop`, not an
//     async-cancellation hazard, and is still covered by the
//     `CleanupAuthority` background fallback armed on that value).
//   * The receiver was dropped (the caller was cancelled) before the owner
//     task finished: the owner task discovers this only when its `send`
//     fails, at which point create/connect/migrate has already fully
//     resolved one way or the other (there is no earlier point at which the
//     owner task polls the receiver, precisely so that no DROP can ever be
//     issued while its own CREATE is still in flight). If setup succeeded,
//     the owner task is now the database's only owner and awaits its own
//     exact teardown before finishing. If setup failed, the owner task
//     already awaits and inspects a best-effort cleanup unconditionally
//     (see `run_setup_owner`'s `Err` arm) -- whether or not the caller was
//     still around to receive the error.
//
// Every terminal state `run_setup_owner` can reach is reported through its
// `JoinHandle<SetupCompletion>` (exposed to tests via
// `TestObservations::setup_join`), so a test can deterministically await the
// owner task's own outcome even when the outer call was aborted and never
// returned normally -- no sleep-poll loop required.
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
// Bounded, typed errors -- gives every disposable-DB failure mode (create/
// connect/migrate/drop) its own variant instead of an opaque anyhow string,
// and a caller a `Result` to match on instead of an `.expect()` panic.
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
/// created, or one some other caller already dropped, a trivial success
/// rather than a failure. `WITH (FORCE)` (Postgres 13+) terminates any other
/// backend still holding a connection open, so this still succeeds against a
/// pool with live connections.
///
/// Callers of this function only ever invoke it *after* the corresponding
/// create/connect/migrate attempt has already fully resolved (see
/// `run_setup_owner`) -- never concurrently with a still-running `CREATE
/// DATABASE`, which is exactly the race this module must not reintroduce.
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
// deterministically cancel the caller of `create_disposable_test_db_with_hooks`
// (or sabotage a connection) at an exact point in the owner task's lifecycle,
// instead of racing a sleep against real Postgres round-trip latency (a
// `Notify`-barrier rendezvous, not a sleep poll).
// ---------------------------------------------------------------------------

/// A two-way rendezvous: the owner task notifies `reached` the instant it
/// arrives at the named point, then blocks on `hold` until the test either
/// releases it (`hold.notify_one()`) or the test proceeds to cancel the
/// *caller* (never this task -- see the module doc-comment) while it waits.
/// This gives a test full, deterministic control over whether/when execution
/// proceeds past that exact point.
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
/// corresponding internal state exists, so a test that cancels the caller
/// before it can return normally can still retrieve the generated
/// `db_name`/`admin_url`, the owner task's own `JoinHandle<SetupCompletion>`
/// (which keeps running and reports its outcome regardless of caller
/// cancellation), and the post-handoff `CleanupAuthority` background task's
/// `JoinHandle` for fully deterministic (non-sleep) assertions about the
/// eventual state.
#[derive(Default)]
pub struct TestObservations {
    pub db_name: Option<String>,
    pub admin_url: Option<String>,
    /// JoinHandle for the `CleanupAuthority` spawned once create/connect/
    /// migrate succeeds (armed regardless of whether the value is then
    /// successfully handed off to the caller or found to be orphaned).
    pub cleanup_join: Option<JoinHandle<CleanupOutcome>>,
    /// JoinHandle for the setup owner task itself (`run_setup_owner`) -- the
    /// task that performs admin connect / CREATE DATABASE / target connect /
    /// migrate, and which keeps running to completion even if the caller
    /// awaiting `create_disposable_test_db_with_hooks` is cancelled. Await
    /// this to deterministically observe the owner task's own outcome
    /// (`SetupCompletion`) without a sleep-poll loop, even when the outer
    /// call was aborted and never returned normally.
    pub setup_join: Option<JoinHandle<SetupCompletion>>,
}

/// Outcome reported by a `CleanupAuthority`'s background task, exposed for
/// test assertions. Production callers never inspect this; they only call
/// `commit()` or let the value drop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CleanupOutcome {
    /// An explicit, awaited teardown already ran (or is running); this task
    /// took no action.
    Committed,
    /// The authority was dropped without a commit; the task ran its own
    /// retry-capable best-effort DROP DATABASE.
    CleanedUp { succeeded: bool },
}

/// The setup owner task's own final, observable outcome -- see the module
/// doc-comment for the ownership architecture this reports on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetupCompletion {
    /// create/connect/migrate succeeded and the resulting `DisposableTestDb`
    /// was successfully handed off to the caller through the result
    /// channel. From this point on, cleanup is the caller's (and the
    /// handed-off value's `CleanupAuthority`'s) responsibility.
    HandedOff,
    /// create/connect/migrate succeeded, but by the time the owner task
    /// tried to hand the result off, the caller (the channel's receiver) was
    /// already gone -- so the owner task is the database's only remaining
    /// owner. It awaited an exact teardown of that now-orphaned database
    /// itself before finishing; `teardown_succeeded` reports whether that
    /// awaited DROP actually succeeded.
    OrphanedAfterSuccess { teardown_succeeded: bool },
    /// create/connect/migrate itself failed. The owner task always awaits a
    /// best-effort cleanup of whatever partial state exists and inspects
    /// its result -- regardless of whether the caller was still around to
    /// receive the error -- and `cleanup_succeeded` reports that result.
    SetupFailed { cleanup_succeeded: bool },
}

/// Test-only knobs for `create_disposable_test_db_with_hooks`. The plain
/// `create_disposable_test_db` used by every ordinary caller passes
/// `Default::default()`, so production behavior is completely unaffected.
#[derive(Clone, Default)]
pub struct DisposableDbTestHooks {
    /// Rendezvous immediately before the CREATE DATABASE statement is
    /// issued (used to prove the owner task completes/cleans up correctly
    /// even when the caller is cancelled while parked here).
    pub before_create: Option<TestBarrier>,
    /// Rendezvous immediately after CREATE DATABASE succeeds, before the
    /// target-database connect is attempted.
    pub before_target_connect: Option<TestBarrier>,
    /// Rendezvous immediately after the target-database connect succeeds,
    /// before `migrate()` is called.
    pub before_migrate: Option<TestBarrier>,
    pub observer: Option<Arc<Mutex<TestObservations>>>,
    /// If set, every cleanup DROP attempt the owner task makes (both the
    /// ordinary create/connect/migrate error path and the orphaned-after-
    /// success path) connects through this URL instead of the real admin
    /// URL, while CREATE/connect themselves are unaffected. Lets a test
    /// force a deterministic, observable cleanup failure (e.g. pointing
    /// this at an unreachable address) without racing real timing against a
    /// real failure injection, and without ever touching the shared
    /// `mqk_test` database. The real `CleanupAuthority` armed on a
    /// successfully-created database still uses the real admin URL, so a
    /// forced failure here does not itself leak a database long-term.
    pub force_cleanup_admin_url: Option<String>,
}

/// Owns the eventual DROP DATABASE cleanup for exactly one already-created
/// `mqk_disp_*` database, from the moment it is constructed (always *after*
/// create/connect/migrate has already succeeded -- see `run_setup_owner`)
/// until an explicit, awaited teardown calls `commit()`. Spawned eagerly as
/// an independent Tokio task (never inside a `Drop` impl), so that dropping
/// this guard -- whether from an explicit early return, a panic unwinding
/// through it, or the value being dropped without an awaited teardown --
/// reliably triggers cleanup: the task keeps running under its own poll
/// loop, entirely independent of whatever caused this guard to be dropped.
/// No custom `Drop` impl is needed: letting `commit_tx` (an
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
/// By the time a caller ever observes a value of this type, create/connect/
/// migrate has already fully succeeded (see the module doc-comment for how
/// the setup owner task guarantees this regardless of the caller's own
/// cancellation). From this point on, `cleanup` (a `CleanupAuthority`)
/// protects only the ordinary, synchronous case of this value being dropped
/// without [`DisposableTestDb::drop_database`] ever having been called.
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
    /// any clone of it) is done being used. On success, suppresses the
    /// `CleanupAuthority` background fallback (it already happened here,
    /// synchronously and awaited); on failure, leaves it armed so the
    /// fallback still gets a chance to clean up when `self` is finally
    /// dropped.
    pub async fn drop_database(self) -> Result<(), DisposableDbError> {
        let admin_url = self.admin_url.clone();
        self.drop_database_via(&admin_url).await
    }

    /// Shared teardown logic parameterized by which admin URL to connect
    /// through for the DROP itself. `drop_database` always uses the real
    /// admin URL; the setup owner task's orphan-teardown path (see
    /// `run_setup_owner`) may instead route through
    /// `DisposableDbTestHooks::force_cleanup_admin_url` so a test can
    /// deterministically prove cleanup-failure reporting without a real
    /// timing race. Identity (`self.db_name`, asserted against the
    /// `mqk_disp_*` namespace) is always the real generated name either way.
    async fn drop_database_via(mut self, drop_admin_url: &str) -> Result<(), DisposableDbError> {
        assert_is_disposable_db_name(&self.db_name);
        self.pool.close().await;
        let admin_pool = PgPoolOptions::new()
            .max_connections(1)
            .connect(drop_admin_url)
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

/// The message delivered from the setup owner task back to whatever called
/// `create_disposable_test_db_with_hooks`, through a `oneshot` channel. Not
/// exported: external callers only ever see the `Result` that function
/// itself returns.
enum SetupHandoff {
    Ready(DisposableTestDb),
    Failed(DisposableDbError),
}

/// Full implementation behind `create_disposable_test_db`, parameterized by
/// test-only synchronization hooks (see `DisposableDbTestHooks`). The actual
/// create/connect/migrate work happens in `run_setup_owner`, spawned as an
/// independent Tokio task before this function ever awaits anything else --
/// see the module doc-comment for why that ownership boundary is what makes
/// this cancellation-safe.
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

    let DisposableDbTestHooks {
        before_create,
        before_target_connect,
        before_migrate,
        observer,
        force_cleanup_admin_url,
    } = hooks;

    if let Some(observer) = &observer {
        let mut observations = observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        observations.db_name = Some(db_name.clone());
        observations.admin_url = Some(admin_url.clone());
    }

    let (result_tx, result_rx) = oneshot::channel::<SetupHandoff>();
    let observer_for_task = observer.clone();

    // Spawned *before* this function awaits anything else: from this point
    // on, the caller of `create_disposable_test_db_with_hooks` dropping its
    // own future cannot cancel `run_setup_owner` -- it is an independent
    // task on the runtime, not a sub-future of the one being awaited below.
    let setup_join = tokio::spawn(run_setup_owner(
        base,
        admin_url,
        db_name,
        query,
        before_create,
        before_target_connect,
        before_migrate,
        observer_for_task,
        force_cleanup_admin_url,
        result_tx,
    ));

    if let Some(observer) = &observer {
        let mut observations = observer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        observations.setup_join = Some(setup_join);
    }
    // If nobody is observing, `setup_join` is simply dropped here, which
    // detaches the task -- it still runs to completion on the runtime
    // regardless; only the ability to `.await` its result is lost, and
    // production code never needs that.

    match result_rx
        .await
        .expect("run_setup_owner always sends exactly one SetupHandoff before returning")
    {
        SetupHandoff::Ready(db) => Ok(db),
        SetupHandoff::Failed(e) => Err(e),
    }
}

/// The setup owner task: performs admin connect / CREATE DATABASE / target
/// connect / migrate to completion, then either hands the result off to
/// whoever is still listening on `result_tx`, or -- if that receiver is
/// already gone -- takes full responsibility for cleaning up whatever it
/// just did. This task is never cancelled by the caller of
/// `create_disposable_test_db_with_hooks` dropping its own future (see the
/// module doc-comment); the only way it stops early is if the process
/// itself is torn down.
#[allow(clippy::too_many_arguments)]
async fn run_setup_owner(
    base: String,
    admin_url: String,
    db_name: String,
    query: Option<String>,
    before_create: Option<TestBarrier>,
    before_target_connect: Option<TestBarrier>,
    before_migrate: Option<TestBarrier>,
    observer: Option<Arc<Mutex<TestObservations>>>,
    force_cleanup_admin_url: Option<String>,
    result_tx: oneshot::Sender<SetupHandoff>,
) -> SetupCompletion {
    let inner_result = create_disposable_test_db_inner(
        &base,
        &admin_url,
        &db_name,
        &query,
        before_create.as_ref(),
        before_target_connect.as_ref(),
        before_migrate.as_ref(),
    )
    .await;

    let cleanup_admin_url: &str = force_cleanup_admin_url.as_deref().unwrap_or(&admin_url);

    match inner_result {
        Ok(pool) => {
            let db = DisposableTestDb {
                pool,
                db_name: db_name.clone(),
                admin_url: admin_url.clone(),
                cleanup: Some(CleanupAuthority::spawn(
                    admin_url.clone(),
                    db_name.clone(),
                    observer.as_ref(),
                )),
            };
            match result_tx.send(SetupHandoff::Ready(db)) {
                Ok(()) => SetupCompletion::HandedOff,
                Err(SetupHandoff::Ready(orphan)) => {
                    // The caller was gone by the time setup finished. This
                    // task is the database's only owner now, so it must
                    // await its own exact teardown rather than leaving that
                    // to chance -- this is the fix for the defect where a
                    // cancellation left nothing alive to ever clean up.
                    // `cleanup_admin_url` lets a test deterministically
                    // force this exact attempt to fail (via
                    // `force_cleanup_admin_url`) without touching the real
                    // server; the redundant `CleanupAuthority` armed above
                    // (using the real `admin_url`) still fires its own
                    // retry-capable drop -- using the real admin URL -- when
                    // `orphan` is finally dropped here if this exact attempt
                    // did not itself succeed.
                    let teardown_result = orphan.drop_database_via(cleanup_admin_url).await;
                    SetupCompletion::OrphanedAfterSuccess {
                        teardown_succeeded: teardown_result.is_ok(),
                    }
                }
                Err(SetupHandoff::Failed(_)) => {
                    unreachable!("send() only ever returns the exact value it was given")
                }
            }
        }
        Err(e) => {
            // Ordinary (non-cancelled) failure: always await cleanup and
            // inspect its result before reporting -- never blindly commit or
            // disarm a fallback the way the prior version did. There is no
            // separately-armed `CleanupAuthority` in this branch at all, so
            // there is nothing that could be wrongly disarmed.
            let cleanup_succeeded = drop_with_retries(cleanup_admin_url, &db_name, 3).await;
            // Deliver the error regardless of whether the caller is still
            // listening; if it is not, `SetupCompletion` is the only
            // remaining signal, and it is still reported below.
            let _ = result_tx.send(SetupHandoff::Failed(e));
            SetupCompletion::SetupFailed { cleanup_succeeded }
        }
    }
}

/// CREATE DATABASE, then connect to it, then migrate it. Isolated from
/// `run_setup_owner` purely so that function's handoff/orphan/error-cleanup
/// bookkeeping stays in one place while this one stays a straight-line
/// happy/error path.
async fn create_disposable_test_db_inner(
    base: &str,
    admin_url: &str,
    db_name: &str,
    query: &Option<String>,
    before_create: Option<&TestBarrier>,
    before_target_connect: Option<&TestBarrier>,
    before_migrate: Option<&TestBarrier>,
) -> Result<PgPool, DisposableDbError> {
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(admin_url)
        .await
        .map_err(DisposableDbError::Connect)?;

    if let Some(barrier) = before_create {
        barrier.reached.notify_one();
        barrier.hold.notified().await;
    }

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

    // -- run_setup_owner: ordinary (non-cancelled) admin-connect failure --
    // no real Postgres needed: 127.0.0.1:1 refuses immediately, both for the
    // create/connect/migrate attempt itself and for the cleanup attempt that
    // must follow it. Proves defect B is closed structurally: there is no
    // `CleanupAuthority` in this code path at all to wrongly disarm, and
    // `SetupCompletion::SetupFailed.cleanup_succeeded` directly exposes
    // whether the awaited cleanup actually succeeded rather than assuming it
    // did.

    #[tokio::test]
    async fn run_setup_owner_ordinary_failure_awaits_and_reports_cleanup_result_without_disarming_anything(
    ) {
        let (result_tx, result_rx) = oneshot::channel::<SetupHandoff>();
        let completion = run_setup_owner(
            UNREACHABLE_ADMIN_URL
                .trim_end_matches("/postgres")
                .to_string(),
            UNREACHABLE_ADMIN_URL.to_string(),
            "mqk_disp_unittest_ordinary_fail".to_string(),
            None,
            None,
            None,
            None,
            None,
            None,
            result_tx,
        )
        .await;

        match completion {
            SetupCompletion::SetupFailed { cleanup_succeeded } => {
                assert!(
                    !cleanup_succeeded,
                    "connecting to 127.0.0.1:1 must fail every retry attempt, proving the \
                     ordinary-error cleanup path really ran and its failure was observed \
                     rather than silently treated as success"
                );
            }
            other => panic!("expected SetupFailed, got {other:?}"),
        }

        match result_rx
            .await
            .expect("receiver should still get a Failed handoff")
        {
            SetupHandoff::Failed(DisposableDbError::Connect(_)) => {}
            SetupHandoff::Failed(other) => panic!("expected Connect(_), got {other}"),
            SetupHandoff::Ready(_) => {
                panic!("expected Failed, got Ready against an unreachable admin URL")
            }
        }
    }
}
