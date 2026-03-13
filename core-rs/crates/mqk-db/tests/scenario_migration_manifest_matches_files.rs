use serde::Deserialize;
use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Deserialize)]
struct MigrationManifest {
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

#[test]
fn migration_manifest_matches_sql_files() {
    let migrations_root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("migrations");
    let manifest_path = migrations_root.join("manifest.json");

    let manifest_raw = fs::read_to_string(&manifest_path)
        .unwrap_or_else(|e| panic!("failed to read {}: {e}", manifest_path.display()));
    let manifest: MigrationManifest = serde_json::from_str(&manifest_raw)
        .unwrap_or_else(|e| panic!("failed to parse {}: {e}", manifest_path.display()));

    let manifest_paths: BTreeSet<String> = manifest.migrations.into_iter().map(|m| m.path).collect();

    let mut sql_paths = BTreeSet::new();
    collect_sql_relative_paths(&migrations_root, &migrations_root, &mut sql_paths);

    assert_eq!(
        manifest_paths, sql_paths,
        "manifest.json must enumerate every SQL migration exactly once"
    );
}
