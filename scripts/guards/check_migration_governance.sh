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
# Prefer python3, but fall back to python -- on some Windows checkouts
# `python3` resolves to the Microsoft Store app-execution-alias stub rather
# than a real interpreter, while `python` resolves to the actual install.
PYTHON_BIN="python3"
if ! command -v python3 >/dev/null 2>&1 || ! python3 --version >/dev/null 2>&1; then
  PYTHON_BIN="python"
fi
"$PYTHON_BIN" - "$MANIFEST_PATH" "$REPO_ROOT/$AUTHORITATIVE_DIR" <<'PY'
import json
import pathlib
import sys

manifest_path = pathlib.Path(sys.argv[1])
root = pathlib.Path(sys.argv[2])

manifest = json.loads(manifest_path.read_text())
manifest_paths = sorted(m["path"].replace("\\", "/") for m in manifest["migrations"])
sql_paths = sorted(
    p.relative_to(root).as_posix()
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
# Guard 3: migration versions 0068+ must resolve to LF checkout bytes.
#
# Versions 0001-0067 predate this policy and are intentionally grandfathered
# because deployed SQLx checksum identity may reflect their historical Windows
# checkout bytes. Every migration from 0068 onward must be explicitly pinned
# to LF before it enters the authoritative chain.
mapfile -t sql_files < <(git ls-files "$AUTHORITATIVE_DIR" | awk '/\.sql$/')
[[ ${#sql_files[@]} -gt 0 ]] || fail "no tracked SQL migration files found"

for f in "${sql_files[@]}"; do
  base="${f##*/}"
  version="${base%%_*}"

  if [[ "$version" =~ ^[0-9]+$ ]] && (( 10#$version >= 68 )); then
    eol_attr="$(git check-attr eol -- "$f")"
    [[ "$eol_attr" == "$f: eol: lf" ]] || \
      fail "migration version >=0068 is not pinned to LF checkout bytes: $eol_attr"
  fi
done

echo "[migration-guard] OK: migration versions 0068+ resolve to eol=lf"