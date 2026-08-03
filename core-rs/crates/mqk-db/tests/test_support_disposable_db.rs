// core-rs/crates/mqk-db/tests/test_support_disposable_db.rs
//
// FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 3 /
// FULL-AUDIT-CHECKPOINT-HARDENING-REPAIR-01 Part 2: DB-backed proofs for the
// hardened disposable-per-test-database helper (mqk_db::test_support).
// Pure-logic cases (URL splitting, error typing, the run_isolated decision
// tree, CleanupAuthority commit/no-commit) live as unit tests inside
// src/test_support.rs and need no database; these integration tests need a
// real Postgres server to create/drop real databases against, so they
// require MQK_DATABASE_URL and the `testkit` feature (registered with
// `required-features = ["testkit"]` in mqk-db/Cargo.toml), matching every
// other DB-backed test in this crate.
//
// The cancellation and failure-injection tests below use `TestBarrier` (a
// two-way rendezvous, not a sleep) to land a `task.abort()` or an external
// admin-connection sabotage at an exact, named point in
// `create_disposable_test_db_with_hooks`'s lifecycle, and `TestObservations`
// to retrieve the generated db_name/admin_url and the background
// `CleanupAuthority` task's JoinHandle even when the outer task never
// returns normally (because it was aborted) -- so "zero residue" is proven
// by awaiting that JoinHandle deterministically, never by a sleep-poll loop.
//
// Run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//      cargo test -p mqk-db --features testkit --test test_support_disposable_db \
//      -- --include-ignored --test-threads=1
#![cfg(feature = "testkit")]

use std::sync::{Arc, Mutex};

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use mqk_db::{
    create_disposable_test_db, create_disposable_test_db_with_hooks, CleanupOutcome,
    DisposableDbTestHooks, DisposableTestDb, TestBarrier, TestObservations,
};

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

    // No explicit drop_database() call: exercise the Drop-guard fallback
    // path that a cancelled/aborted caller would otherwise rely on.
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
/// deposited into `observer` -- no sleep-poll loop needed even though the
/// enclosing call was aborted and never returned normally.
async fn take_cleanup_outcome(observer: &Arc<Mutex<TestObservations>>) -> CleanupOutcome {
    let join = observer
        .lock()
        .expect("observer mutex poisoned")
        .cleanup_join
        .take()
        .expect("cleanup task JoinHandle should have been observed before the call was cancelled");
    join.await.expect("cleanup task itself must not panic")
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
async fn cancellation_racing_the_create_database_statement_leaves_zero_residue() {
    let barrier = TestBarrier::new();
    let observer = Arc::new(Mutex::new(TestObservations::default()));
    let hooks = DisposableDbTestHooks {
        before_create: Some(barrier.clone()),
        observer: Some(observer.clone()),
        ..Default::default()
    };

    let task = tokio::spawn(create_disposable_test_db_with_hooks(
        "ts_cancel_create",
        hooks,
    ));
    // Deterministic: the call is now parked immediately before issuing the
    // CREATE DATABASE statement. Releasing the hold and aborting with no
    // intervening await races the cancellation against that statement
    // itself -- exercising the one window this module cannot fully close
    // client-side (see the module doc-comment), and proving the retry-
    // capable background cleanup makes it safe regardless of which side of
    // the race actually wins on the server.
    barrier.reached.notified().await;
    barrier.hold.notify_one();
    task.abort();
    let _ = task.await;

    let db_name = observed_db_name(&observer);
    let outcome = take_cleanup_outcome(&observer).await;
    assert!(
        matches!(outcome, CleanupOutcome::CleanedUp { succeeded: true }),
        "expected the background cleanup task to run its retry-capable drop and succeed, got {outcome:?}"
    );
    assert!(
        !database_exists(&db_name).await,
        "database {db_name} survived a cancellation racing the CREATE DATABASE statement"
    );
}

#[tokio::test]
#[ignore = "requires MQK_DATABASE_URL; run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test cargo test -p mqk-db --features testkit --test test_support_disposable_db -- --include-ignored --test-threads=1"]
async fn cancellation_after_create_before_target_connect_leaves_zero_residue() {
    let barrier = TestBarrier::new();
    let observer = Arc::new(Mutex::new(TestObservations::default()));
    let hooks = DisposableDbTestHooks {
        before_target_connect: Some(barrier.clone()),
        observer: Some(observer.clone()),
        ..Default::default()
    };

    let task = tokio::spawn(create_disposable_test_db_with_hooks(
        "ts_cancel_connect",
        hooks,
    ));
    barrier.reached.notified().await;
    let db_name = observed_db_name(&observer);
    assert!(
        database_exists(&db_name).await,
        "sanity: database {db_name} should exist -- CREATE DATABASE already succeeded before this barrier"
    );

    task.abort(); // cancel while parked here: CREATE succeeded, target connect never started
    match task.await {
        Err(join_err) => assert!(
            join_err.is_cancelled(),
            "expected the aborted task's JoinError to report cancelled, got {join_err:?}"
        ),
        Ok(_) => panic!("expected the task to be aborted before it could return, but it completed"),
    }

    let outcome = take_cleanup_outcome(&observer).await;
    assert!(
        matches!(outcome, CleanupOutcome::CleanedUp { succeeded: true }),
        "expected the background cleanup task to run its retry-capable drop and succeed, got {outcome:?}"
    );
    assert!(
        !database_exists(&db_name).await,
        "database {db_name} survived cancellation parked between CREATE and target connect"
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

    let task = tokio::spawn(create_disposable_test_db_with_hooks(
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

    let result = task
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

    let task = tokio::spawn(create_disposable_test_db_with_hooks(
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

    let result = task
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
