#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
AUTHORITATIVE_DIR="core-rs/crates/mqk-db/migrations"
MANIFEST_PATH="$REPO_ROOT/$AUTHORITATIVE_DIR/manifest.json"

fail() {
  echo "[migration-guard] FAIL: $*" >&2
  exit 1
}

cd "$REPO_ROOT"

echo "[migration-guard] repo root: $REPO_ROOT"

test -f "$MANIFEST_PATH" || fail "missing manifest: $MANIFEST_PATH"

# Guard 1: no tracked SQL migration file may exist outside the authoritative tree.
stray_sql="$(git ls-files '*.sql' | while read -r f; do
  [[ -f "$f" ]] || continue
  if [[ "$f" == */migrations/* ]] && [[ "$f" != core-rs/crates/mqk-db/migrations/* ]]; then
    echo "$f"
  fi
done)"
if [[ -n "$stray_sql" ]]; then
  echo "[migration-guard] unauthorized migration SQL detected outside $AUTHORITATIVE_DIR:" >&2
  echo "$stray_sql" >&2
  fail "single migration authority violated"
fi

echo "[migration-guard] OK: no unauthorized migration SQL directories"

# Guard 2: manifest must exactly match SQL files in authoritative directory.
python3 - "$MANIFEST_PATH" "$REPO_ROOT/$AUTHORITATIVE_DIR" <<'PY'
import json
import pathlib
import sys

manifest_path = pathlib.Path(sys.argv[1])
root = pathlib.Path(sys.argv[2])

manifest = json.loads(manifest_path.read_text())
manifest_paths = sorted(m["path"].replace("\\\\", "/") for m in manifest["migrations"])
sql_paths = sorted(
    str(p.relative_to(root)).replace("\\\\", "/")
    for p in root.rglob("*.sql")
)

if manifest_paths != sql_paths:
    missing_in_manifest = sorted(set(sql_paths) - set(manifest_paths))
    missing_in_fs = sorted(set(manifest_paths) - set(sql_paths))
    print("[migration-guard] FAIL: manifest drift detected", file=sys.stderr)
    if missing_in_manifest:
        print("  SQL files missing from manifest:", file=sys.stderr)
        for item in missing_in_manifest:
            print(f"    - {item}", file=sys.stderr)
    if missing_in_fs:
        print("  Manifest entries missing on disk:", file=sys.stderr)
        for item in missing_in_fs:
            print(f"    - {item}", file=sys.stderr)
    sys.exit(1)

print("[migration-guard] OK: manifest matches authoritative SQL chain")
PY
