use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct MigrationManifest {
    authoritative_for: String,
    migrations: Vec<MigrationEntry>,
}

#[derive(Debug, Deserialize)]
struct MigrationEntry {
    path: String,
}

fn collect_sql_relative_paths(root: &Path, dir: &Path, out: &mut BTreeSet<String>) {
    let entries = fs::read_dir(dir).expect("failed to read migrations directory");
    for entry in entries {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        if path.is_dir() {
            collect_sql_relative_paths(root, &path, out);
            continue;
        }
        if path.extension().and_then(|s| s.to_str()) != Some("sql") {
            continue;
        }
        let rel = path
            .strip_prefix(root)
            .expect("sql path must be under migrations root")
            .to_string_lossy()
            .replace('\\', "/");
        out.insert(rel);
    }
}

fn collect_named_directory_relative_paths(
    root: &Path,
    dir: &Path,
    needle: &str,
    out: &mut BTreeSet<String>,
) {
    let entries = fs::read_dir(dir).expect("failed to read directory");
    for entry in entries {
        let entry = entry.expect("failed to read directory entry");
        let path = entry.path();
        if !path.is_dir() {
            continue;
        }

        let name = path.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name == "target" {
            continue;
        }

        if name == needle {
            let rel = path
                .strip_prefix(root)
                .expect("directory path must be under root")
                .to_string_lossy()
                .replace('\\', "/");
            out.insert(rel);
        }

        collect_named_directory_relative_paths(root, &path, needle, out);
    }
}

#[test]
fn migration_manifest_matches_sql_files() {
    let migrations_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let manifest_path = migrations_root.join("manifest.json");

    let manifest_raw = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", manifest_path.display()));
    let manifest: MigrationManifest = serde_json::from_str(&manifest_raw)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", manifest_path.display()));

    assert_eq!(
        manifest.authoritative_for,
        "core-rs/crates/mqk-db/migrations/*.sql",
        "manifest.json must declare the single authoritative migration chain"
    );

    let manifest_paths: BTreeSet<String> =
        manifest.migrations.into_iter().map(|m| m.path).collect();

    let mut sql_paths = BTreeSet::new();
    collect_sql_relative_paths(&migrations_root, &migrations_root, &mut sql_paths);

    assert_eq!(
        manifest_paths, sql_paths,
        "manifest.json must enumerate every SQL migration exactly once"
    );
}

#[test]
fn migration_authority_is_single_under_core_rs() {
    let crate_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let core_rs_root = crate_root
        .parent()
        .and_then(Path::parent)
        .expect("mqk-db crate must live under core-rs/crates");

    let mut migration_dirs = BTreeSet::new();
    collect_named_directory_relative_paths(
        &core_rs_root,
        &core_rs_root,
        "migrations",
        &mut migration_dirs,
    );

    let expected = BTreeSet::from([String::from("crates/mqk-db/migrations")]);
    assert_eq!(
        migration_dirs, expected,
        "core-rs must contain exactly one authoritative migrations directory"
    );
}
