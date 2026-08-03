// core-rs/crates/mqk-db/tests/test_support_disposable_db.rs
//
// FULL-AUDIT-SAFE-IGNORED-AND-SHARED-DB-FINAL-CLOSURE-01 Part 3: DB-backed
// proofs for the hardened disposable-per-test-database helper
// (mqk_db::test_support). Pure-logic cases (URL splitting, error typing)
// live as unit tests inside src/test_support.rs and need no database; these
// integration tests need a real Postgres server to create/drop real
// databases against, so they require MQK_DATABASE_URL and the `testkit`
// feature (registered with `required-features = ["testkit"]` in
// mqk-db/Cargo.toml), matching every other DB-backed test in this crate.
//
// Run: MQK_DATABASE_URL=postgres://postgres:postgres@127.0.0.1:5434/mqk_test \
//      cargo test -p mqk-db --features testkit --test test_support_disposable_db \
//      -- --include-ignored --test-threads=1
#![cfg(feature = "testkit")]

use sqlx::postgres::PgPoolOptions;
use sqlx::PgPool;

use mqk_db::{create_disposable_test_db, DisposableTestDb};

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
