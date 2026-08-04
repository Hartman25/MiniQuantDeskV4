// core-rs/crates/mqk-db/tests/test_support_disposable_db.rs
//
// FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 3 /
// FULL-AUDIT-CHECKPOINT-HARDENING-REPAIR-01 Part 2 /
// FULL-AUDIT-TESTKIT-AND-IGNORED-AUTHORITY-FINAL-REPAIR-01 Part 1: DB-backed
// proofs for the hardened disposable-per-test-database helper
// (mqk_db::test_support). Pure-logic cases (URL splitting, error typing, the
// run_isolated decision tree, CleanupAuthority commit/no-commit, the
// ordinary-error-path cleanup-inspection proof) live as unit tests inside
// src/test_support.rs and need no database; these integration tests need a
// real Postgres server to create/drop real databases against, so they
// require MQK_DATABASE_URL and the `testkit` feature (registered with
// `required-features = ["testkit"]` in mqk-db/Cargo.toml), matching every
// other DB-backed test in this crate.
//
// Ownership model under test: `create_disposable_test_db_with_hooks` spawns
// an independent `run_setup_owner` Tokio task that performs admin connect /
// CREATE DATABASE / target connect / migrate to completion regardless of
// whether the *caller* awaiting that function is itself cancelled. Every
// test below that exercises caller cancellation therefore aborts the OUTER
// task (the one polling the result channel) and separately awaits the owner
// task's own `JoinHandle<SetupCompletion>` (retrieved via `TestObservations`)
// to deterministically observe what the owner task did -- never a sleep-poll
// loop, and never a race between which side "wins": each cancellation test
// first proves the caller is fully, provably gone (via `task.abort()` +
// `task.await` reporting cancelled) before ever releasing the barrier that
// lets the owner task proceed past that exact point.
//
// Run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//      cargo test -p mqk-db --features testkit --test test_support_disposable_db \
//      -- --include-ignored --test-threads=1
#![cfg(feature = "testkit")]

use std::sync::{Arc, Mutex};

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;
use tokio::task::JoinHandle;

use mqk_db::{
    create_disposable_test_db, create_disposable_test_db_with_hooks, CleanupOutcome,
    DisposableDbTestHooks, DisposableTestDb, SetupCompletion, TestBarrier, TestObservations,
};

/// An address nothing listens on: connection attempts fail deterministically
/// (no real Postgres round-trip needed), used to force an exact cleanup
/// attempt to fail without racing real timing against a real failure
/// injection.
const UNREACHABLE_ADMIN_URL: &str = "postgres://user:pass@127.0.0.1:1/postgres";

/// Connects to the admin `postgres` database on the same server as
/// `MQK_DATABASE_URL` and reports whether a database with the given exact
/// name currently exists. Used to make teardown assertions concrete instead
/// of trusting the helper's own return value.
async fn database_exists(db_name: &str) -> bool {
    let base_url = std::env::var(mqk_db::ENV_DB_URL).expect("MQK_DATABASE_URL must be set");
    let idx = base_url
        .rfind('/')
        .expect("MQK_DATABASE_URL must have a /<dbname> segment");
    let (path_part, query) = match base_url.split_once('?') {
        Some((p, q)) => (p, Some(q)),
        None => (base_url.as_str(), None),
    };
    let idx = idx.min(path_part.len());
    let base = &path_part[..idx];
    let admin_url = match query {
        Some(q) => format!("{base}/postgres?{q}"),
        None => format!("{base}/postgres"),
    };
    let admin_pool: PgPool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connect to admin db for existence check");
    let (exists,): (bool,) =
        sqlx::query_as("select exists(select 1 from pg_database where datname = $1)")
            .bind(db_name)
            .fetch_one(&admin_pool)
            .await
            .expect("pg_database existence query");
    admin_pool.close().await;
    exists
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn explicit_drop_database_removes_the_database_immediately() {
    let disposable = create_disposable_test_db("ts_explicit_01")
        .await
        .expect("create_disposable_test_db");
    let db_name = disposable.db_name().to_string();
    assert!(
        database_exists(&db_name).await,
        "sanity: database {db_name} should exist right after creation"
    );

    disposable
        .drop_database()
        .await
        .expect("drop_database should succeed");

    assert!(
        !database_exists(&db_name).await,
        "database {db_name} should be gone immediately after explicit drop_database()"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn dropping_without_explicit_teardown_still_cleans_up_via_the_drop_guard() {
    let disposable = create_disposable_test_db("ts_drop_guard_01")
        .await
        .expect("create_disposable_test_db");
    let db_name = disposable.db_name().to_string();
    assert!(
        database_exists(&db_name).await,
        "sanity: database {db_name} should exist right after creation"
    );

    // No explicit drop_database() call: exercise the CleanupAuthority
    // fallback path that a caller who forgets teardown would otherwise rely
    // on. By the time a caller ever holds a `DisposableTestDb`, create/
    // connect/migrate has already fully succeeded (see the module doc-
    // comment in src/test_support.rs), so this is an ordinary synchronous
    // drop, not an async-cancellation hazard.
    drop(disposable);

    let mut cleaned = false;
    for _ in 0..50 {
        if !database_exists(&db_name).await {
            cleaned = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        cleaned,
        "Drop-guard fallback did not clean up database {db_name} within 5s of being dropped without explicit teardown"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn panic_inside_run_isolated_still_drops_the_database() {
    let (tx, rx) = tokio::sync::oneshot::channel::<String>();

    let outer = tokio::spawn(async move {
        mqk_db::run_isolated("ts_panic_01", move |pool| async move {
            let (name,): (String,) = sqlx::query_as("select current_database()")
                .fetch_one(&pool)
                .await
                .expect("query current_database");
            let _ = tx.send(name);
            panic!("intentional test panic to prove teardown still runs on a panicking closure");
        })
        .await;
    });

    let db_name = rx
        .await
        .expect("closure should report its database name before panicking");

    let join_result = outer.await;
    assert!(
        join_result.is_err(),
        "run_isolated should propagate the inner closure's panic to the outer join"
    );

    assert!(
        !database_exists(&db_name).await,
        "disposable database {db_name} survived a panicking test closure"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn concurrent_disposable_db_creation_is_unique_and_cleans_up() {
    const N: usize = 8;
    let mut handles = Vec::with_capacity(N);
    for i in 0..N {
        handles.push(tokio::spawn(async move {
            create_disposable_test_db(&format!("ts_concur_{i}"))
                .await
                .expect("create_disposable_test_db")
        }));
    }

    let mut dbs: Vec<DisposableTestDb> = Vec::with_capacity(N);
    for h in handles {
        dbs.push(h.await.expect("join concurrent create task"));
    }

    let mut names: Vec<String> = dbs.iter().map(|d| d.db_name().to_string()).collect();
    let mut sorted_unique = names.clone();
    sorted_unique.sort();
    sorted_unique.dedup();
    assert_eq!(
        sorted_unique.len(),
        names.len(),
        "concurrent disposable DB creation produced duplicate names: {names:?}"
    );

    for name in &names {
        assert!(
            database_exists(name).await,
            "database {name} should exist before teardown"
        );
    }

    for db in dbs {
        db.drop_database().await.expect("drop_database");
    }

    for name in &names {
        assert!(
            !database_exists(name).await,
            "database {name} should be gone after teardown"
        );
    }
    names.clear();
}

/// Deterministically waits for the `CleanupAuthority` background task's
/// outcome via the JoinHandle `create_disposable_test_db_with_hooks`
/// deposited into `observer` -- no sleep-poll loop needed.
async fn take_cleanup_outcome(observer: &Arc<Mutex<TestObservations>>) -> CleanupOutcome {
    let join = observer
        .lock()
        .expect("observer mutex poisoned")
        .cleanup_join
        .take()
        .expect("cleanup task JoinHandle should have been observed");
    join.await.expect("cleanup task itself must not panic")
}

/// Deterministically waits for the setup OWNER task's own outcome via the
/// JoinHandle deposited into `observer` -- this resolves independently of
/// whether the outer call (the one awaiting the result channel) was ever
/// polled to completion, aborted, or cancelled.
fn observed_setup_join(observer: &Arc<Mutex<TestObservations>>) -> JoinHandle<SetupCompletion> {
    observer
        .lock()
        .expect("observer mutex poisoned")
        .setup_join
        .take()
        .expect("setup_join should have been observed before the outer call was cancelled")
}

fn observed_db_name(observer: &Arc<Mutex<TestObservations>>) -> String {
    observer
        .lock()
        .expect("observer mutex poisoned")
        .db_name
        .clone()
        .expect("db_name should have been observed before the call was cancelled")
}

fn observed_admin_url(observer: &Arc<Mutex<TestObservations>>) -> String {
    observer
        .lock()
        .expect("observer mutex poisoned")
        .admin_url
        .clone()
        .expect("admin_url should have been observed before the call was cancelled")
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn caller_cancelled_before_admin_connect_owner_still_completes_and_cleans_up() {
    let observer = Arc::new(Mutex::new(TestObservations::default()));
    let hooks = DisposableDbTestHooks {
        observer: Some(observer.clone()),
        ..Default::default()
    };

    let outer = tokio::spawn(create_disposable_test_db_with_hooks(
        "ts_cancel_before_admin",
        hooks,
    ));
    // Let the outer task run its synchronous prefix (env lookup, name
    // generation, spawning the owner task, recording db_name/admin_url/
    // setup_join into `observer`) up to its first real await point
    // (awaiting the result channel), then cancel it there -- before the
    // owner task has necessarily even opened its admin connection.
    tokio::task::yield_now().await;
    outer.abort();
    let _ = outer.await;

    let db_name = observed_db_name(&observer);
    let setup_join = observed_setup_join(&observer);
    let completion = setup_join
        .await
        .expect("setup owner task itself must not panic");
    assert_eq!(
        completion,
        SetupCompletion::OrphanedAfterSuccess {
            teardown_succeeded: true
        },
        "expected the owner task to finish create/connect/migrate on its own and clean up the \
         orphaned database after discovering the caller was already gone"
    );
    assert!(
        !database_exists(&db_name).await,
        "database {db_name} survived a caller cancellation that landed before admin connect"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn caller_cancelled_immediately_before_create_then_released_owner_completes_and_cleans_up() {
    let barrier = TestBarrier::new();
    let observer = Arc::new(Mutex::new(TestObservations::default()));
    let hooks = DisposableDbTestHooks {
        before_create: Some(barrier.clone()),
        observer: Some(observer.clone()),
        ..Default::default()
    };

    let outer = tokio::spawn(create_disposable_test_db_with_hooks(
        "ts_cancel_create",
        hooks,
    ));
    // Deterministic: the owner task has already opened its admin connection
    // and is now parked immediately before issuing the CREATE DATABASE
    // statement itself.
    barrier.reached.notified().await;

    // Cancel the caller *first*, and confirm via the aborted JoinHandle that
    // it is fully, provably gone, before ever releasing the owner task. This
    // proves the owner completes CREATE (and cleans up afterward) with the
    // caller definitely already gone -- unlike a prior version of this test,
    // which released the barrier and aborted with no intervening await,
    // leaving it ambiguous which side of the race actually won.
    outer.abort();
    match outer.await {
        Err(join_err) => assert!(
            join_err.is_cancelled(),
            "expected the aborted outer task's JoinError to report cancelled, got {join_err:?}"
        ),
        Ok(_) => {
            panic!("expected the outer task to be aborted before it could return, but it completed")
        }
    }

    let db_name = observed_db_name(&observer);
    let setup_join = observed_setup_join(&observer);

    // Only now release the owner task -- it issues CREATE DATABASE with
    // nobody left to ever receive the result.
    barrier.hold.notify_one();

    let completion = setup_join
        .await
        .expect("setup owner task itself must not panic");
    assert_eq!(
        completion,
        SetupCompletion::OrphanedAfterSuccess {
            teardown_succeeded: true
        },
        "expected the owner task to finish CREATE/connect/migrate and clean up the orphaned \
         database after discovering the caller was already (and provably) gone"
    );
    assert!(
        !database_exists(&db_name).await,
        "database {db_name} survived a cancellation that was fully complete before the owner \
         task's CREATE DATABASE statement ran"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn caller_cancelled_after_create_before_target_connect_owner_completes_and_cleans_up() {
    let barrier = TestBarrier::new();
    let observer = Arc::new(Mutex::new(TestObservations::default()));
    let hooks = DisposableDbTestHooks {
        before_target_connect: Some(barrier.clone()),
        observer: Some(observer.clone()),
        ..Default::default()
    };

    let outer = tokio::spawn(create_disposable_test_db_with_hooks(
        "ts_cancel_connect",
        hooks,
    ));
    barrier.reached.notified().await;
    let db_name = observed_db_name(&observer);
    assert!(
        database_exists(&db_name).await,
        "sanity: database {db_name} should exist -- CREATE DATABASE already succeeded before this barrier"
    );

    outer.abort(); // cancel while parked here: CREATE succeeded, target connect never started
    match outer.await {
        Err(join_err) => assert!(
            join_err.is_cancelled(),
            "expected the aborted outer task's JoinError to report cancelled, got {join_err:?}"
        ),
        Ok(_) => {
            panic!("expected the outer task to be aborted before it could return, but it completed")
        }
    }

    let setup_join = observed_setup_join(&observer);
    barrier.hold.notify_one();

    let completion = setup_join
        .await
        .expect("setup owner task itself must not panic");
    assert_eq!(
        completion,
        SetupCompletion::OrphanedAfterSuccess {
            teardown_succeeded: true
        },
        "expected the owner task to finish target connect/migrate and clean up the orphaned \
         database after discovering the caller was already gone"
    );
    assert!(
        !database_exists(&db_name).await,
        "database {db_name} survived cancellation parked between CREATE and target connect"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn caller_cancelled_before_migration_owner_completes_and_cleans_up() {
    let barrier = TestBarrier::new();
    let observer = Arc::new(Mutex::new(TestObservations::default()));
    let hooks = DisposableDbTestHooks {
        before_migrate: Some(barrier.clone()),
        observer: Some(observer.clone()),
        ..Default::default()
    };

    let outer = tokio::spawn(create_disposable_test_db_with_hooks(
        "ts_cancel_migrate",
        hooks,
    ));
    barrier.reached.notified().await; // parked right before migrate(); target connect already succeeded
    let db_name = observed_db_name(&observer);
    assert!(
        database_exists(&db_name).await,
        "sanity: database {db_name} should exist -- CREATE DATABASE and target connect already succeeded"
    );

    outer.abort();
    match outer.await {
        Err(join_err) => assert!(
            join_err.is_cancelled(),
            "expected the aborted outer task's JoinError to report cancelled, got {join_err:?}"
        ),
        Ok(_) => {
            panic!("expected the outer task to be aborted before it could return, but it completed")
        }
    }

    let setup_join = observed_setup_join(&observer);
    barrier.hold.notify_one(); // release; migrate() now runs with nobody left to receive the result

    let completion = setup_join
        .await
        .expect("setup owner task itself must not panic");
    assert_eq!(
        completion,
        SetupCompletion::OrphanedAfterSuccess {
            teardown_succeeded: true
        },
        "expected the owner task to finish migrate() and clean up the orphaned database after \
         discovering the caller was already gone"
    );
    assert!(
        !database_exists(&db_name).await,
        "database {db_name} survived cancellation parked immediately before migrate()"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn connect_failure_after_create_is_reported_as_connect_error_and_cleans_up() {
    let barrier = TestBarrier::new();
    let observer = Arc::new(Mutex::new(TestObservations::default()));
    let hooks = DisposableDbTestHooks {
        before_target_connect: Some(barrier.clone()),
        observer: Some(observer.clone()),
        ..Default::default()
    };

    let outer = tokio::spawn(create_disposable_test_db_with_hooks(
        "ts_connect_fail",
        hooks,
    ));
    barrier.reached.notified().await; // parked right before target connect; CREATE already succeeded
    let db_name = observed_db_name(&observer);
    let admin_url = observed_admin_url(&observer);

    // Simulate an external actor removing the database out from under the
    // pending call, forcing the subsequent target-connect to fail cleanly
    // -- deterministic because we know exactly when the call is parked.
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connect as admin to sabotage the pending call");
    sqlx::query(&format!(
        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
    ))
    .execute(&admin_pool)
    .await
    .expect("drop database out from under the pending connect");
    admin_pool.close().await;

    barrier.hold.notify_one(); // release; the code's own target connect now fails

    let result = outer
        .await
        .expect("outer task should not panic; it should return an Err");
    match result {
        Err(mqk_db::DisposableDbError::Connect(_)) => {}
        Err(other) => panic!("expected DisposableDbError::Connect, got {other}"),
        Ok(_) => panic!("expected an Err(Connect(_)) after sabotaging the pending connect, got Ok"),
    }

    assert!(
        !database_exists(&db_name).await,
        "database {db_name} should not exist after a connect-failure path"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn migration_failure_after_target_connect_is_reported_as_migrate_error_and_cleans_up() {
    let barrier = TestBarrier::new();
    let observer = Arc::new(Mutex::new(TestObservations::default()));
    let hooks = DisposableDbTestHooks {
        before_migrate: Some(barrier.clone()),
        observer: Some(observer.clone()),
        ..Default::default()
    };

    let outer = tokio::spawn(create_disposable_test_db_with_hooks(
        "ts_migrate_fail",
        hooks,
    ));
    barrier.reached.notified().await; // parked right before migrate(); target connect already succeeded
    let db_name = observed_db_name(&observer);
    let admin_url = observed_admin_url(&observer);

    // Drop the target database out from under the already-connected pool
    // (WITH FORCE terminates its live connections too, and the database no
    // longer existing at all makes any reconnect attempt fail as well) --
    // deterministic, content-agnostic, and needs no knowledge of what the
    // actual migration SQL does.
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&admin_url)
        .await
        .expect("connect as admin to sabotage the pending call");
    sqlx::query(&format!(
        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
    ))
    .execute(&admin_pool)
    .await
    .expect("drop database out from under the already-connected pool");
    admin_pool.close().await;

    barrier.hold.notify_one(); // release; migrate() now runs against a pool with no live target

    let result = outer
        .await
        .expect("outer task should not panic; it should return an Err");
    match result {
        Err(mqk_db::DisposableDbError::Migrate(_)) => {}
        Err(other) => panic!("expected DisposableDbError::Migrate, got {other}"),
        Ok(_) => panic!("expected an Err(Migrate(_)) after sabotaging the connected pool, got Ok"),
    }

    assert!(
        !database_exists(&db_name).await,
        "database {db_name} should not exist after a migration-failure path"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn forced_orphan_cleanup_failure_is_observable_and_never_reported_as_success() {
    let barrier = TestBarrier::new();
    let observer = Arc::new(Mutex::new(TestObservations::default()));
    let hooks = DisposableDbTestHooks {
        before_create: Some(barrier.clone()),
        observer: Some(observer.clone()),
        force_cleanup_admin_url: Some(UNREACHABLE_ADMIN_URL.to_string()),
        ..Default::default()
    };

    let outer = tokio::spawn(create_disposable_test_db_with_hooks(
        "ts_forced_orphan_fail",
        hooks,
    ));
    barrier.reached.notified().await;
    outer.abort();
    match outer.await {
        Err(join_err) => assert!(
            join_err.is_cancelled(),
            "expected the aborted outer task's JoinError to report cancelled, got {join_err:?}"
        ),
        Ok(_) => {
            panic!("expected the outer task to be aborted before it could return, but it completed")
        }
    }

    let db_name = observed_db_name(&observer);
    let setup_join = observed_setup_join(&observer);

    barrier.hold.notify_one();

    let completion = setup_join
        .await
        .expect("setup owner task itself must not panic");
    assert_eq!(
        completion,
        SetupCompletion::OrphanedAfterSuccess {
            teardown_succeeded: false
        },
        "the exact orphan teardown attempt was deliberately forced through an unreachable admin \
         URL and must be reported as failed, never silently treated as success"
    );

    // The exact/forced teardown attempt above was made to fail on purpose.
    // The redundant CleanupAuthority armed with the REAL admin URL (see the
    // module doc-comment in src/test_support.rs) is still independently
    // retrying against the real server -- await it explicitly, not a sleep-
    // poll, to know for certain when the real cleanup has finished before
    // checking for residue.
    let cleanup_outcome = take_cleanup_outcome(&observer).await;
    assert!(
        matches!(
            cleanup_outcome,
            CleanupOutcome::CleanedUp { succeeded: true }
        ),
        "expected the redundant real-admin-URL CleanupAuthority to still succeed even though the \
         exact forced teardown attempt failed, got {cleanup_outcome:?}"
    );
    assert!(
        !database_exists(&db_name).await,
        "database {db_name} survived even the redundant real-admin-URL cleanup fallback"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn forced_ordinary_error_cleanup_failure_is_reported_and_not_silently_treated_as_success() {
    let barrier = TestBarrier::new();
    let observer = Arc::new(Mutex::new(TestObservations::default()));
    let hooks = DisposableDbTestHooks {
        before_migrate: Some(barrier.clone()),
        observer: Some(observer.clone()),
        force_cleanup_admin_url: Some(UNREACHABLE_ADMIN_URL.to_string()),
        ..Default::default()
    };

    let outer = tokio::spawn(create_disposable_test_db_with_hooks(
        "ts_forced_ordinary_fail",
        hooks,
    ));
    barrier.reached.notified().await; // parked right before migrate(); target connect already succeeded
    let db_name = observed_db_name(&observer);
    let real_admin_url = observed_admin_url(&observer);
    let setup_join = observed_setup_join(&observer);

    // Sabotage: drop the target database out from under the already-
    // connected pool so migrate() itself fails -- an ordinary create/
    // connect/migrate error, not a cancellation. This also means the
    // database is already gone by construction, independent of whatever the
    // (deliberately forced-to-fail) cleanup attempt below reports.
    let admin_pool = PgPoolOptions::new()
        .max_connections(1)
        .connect(&real_admin_url)
        .await
        .expect("connect as admin to sabotage the pending call");
    sqlx::query(&format!(
        "DROP DATABASE IF EXISTS \"{db_name}\" WITH (FORCE)"
    ))
    .execute(&admin_pool)
    .await
    .expect("drop database out from under the already-connected pool");
    admin_pool.close().await;

    barrier.hold.notify_one(); // release; migrate() now fails, and the subsequent forced cleanup attempt also fails

    let result = outer
        .await
        .expect("outer task should not panic; it should return an Err");
    match result {
        Err(mqk_db::DisposableDbError::Migrate(_)) => {}
        Err(other) => panic!("expected DisposableDbError::Migrate, got {other}"),
        Ok(_) => panic!("expected an Err(Migrate(_)) after sabotaging the connected pool, got Ok"),
    }

    let completion = setup_join
        .await
        .expect("setup owner task itself must not panic");
    assert_eq!(
        completion,
        SetupCompletion::SetupFailed {
            cleanup_succeeded: false
        },
        "the post-failure cleanup attempt was deliberately forced through an unreachable admin \
         URL and must be reported as failed -- there is no CleanupAuthority in this code path to \
         silently disarm, and the observable SetupCompletion must reflect the real result"
    );

    assert!(
        !database_exists(&db_name).await,
        "database {db_name} unexpectedly exists after a migration-failure path"
    );
}
