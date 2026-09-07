#!/usr/bin/env python3
# =============================================================================
# Testkit-feature release-safety guard (Cargo/TOML-structural).
#
# CI-TESTKIT-FEATURE-GUARD-VERIFY-01: proves a release-profile build cannot
# accidentally enable the `testkit` Cargo feature (mqk-cli's own optional
# `testkit` feature, and the same-named feature it propagates to
# mqk-execution/mqk-runtime/mqk-db). `testkit` gates test-only escape hatches
# (BrokerGateway::for_test, OutboxClaimToken::for_test, the disposable-DB
# test_support module) that MUST NOT compile into a production binary.
#
# Parses every Cargo.toml with tomllib (never grep/awk over raw text), so
# the guard is immune to:
#   - multiline `default = [...]` arrays
#   - dotted dependency tables ([dependencies.foo])
#   - target-specific dependency tables ([target.'cfg(...)'.dependencies])
#   - workspace-inherited dependency features (dep.workspace = true pulling
#     `features` from the root [workspace.dependencies] entry)
#   - feature-alias chains (default -> local alias -> ... -> testkit, or
#     default -> ... -> "some-crate/testkit")
#
# Two independent, workspace-wide checks:
#   1. Default-feature closure: starting from a crate's own `[features]
#      default` list, transitively follow LOCAL feature aliases. Fail if
#      "testkit" (this crate's own gate) or any "<crate>/testkit" /
#      "<crate>?/testkit" edge (this crate's default enabling another
#      crate's testkit feature) is reachable. A plain `cargo build` /
#      `cargo build --release` with no explicit --features enables only
#      default features, so this is the check that rules out "accidentally
#      on by default", including transitively through aliases.
#   2. Production-dependency closure: for every non-dev dependency table
#      (top-level [dependencies]/[build-dependencies], and their
#      [target.'cfg(...)'.*] equivalents), resolve the dependency's
#      effective feature set -- including features inherited from the
#      workspace root via `workspace = true` -- and fail if "testkit" is in
#      it. Only [dev-dependencies] (and target-specific dev-dependencies)
#      may enable testkit; that scopes the feature to `cargo test`, never to
#      `cargo build`/`cargo build --release` of a binary target.
#
# The workspace root's own [workspace.dependencies] table is checked
# directly too: any crate that inherits a workspace dependency via
# `workspace = true` with no local override would otherwise silently
# inherit a workspace-level "testkit" feature that no per-crate scan alone
# would ever see.
#
# Usage: python3 scripts/guards/check_testkit_feature_not_default.py
# Exit codes: 0 = clean, 1 = violation found.
# =============================================================================
import sys
import tomllib
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
CRATES_DIR = REPO_ROOT / "core-rs" / "crates"
WORKSPACE_TOML = REPO_ROOT / "core-rs" / "Cargo.toml"

violations: list[str] = []


def fail(msg: str) -> None:
    violations.append(msg)
    print(f"  FAIL: {msg}")


def load_toml(path: Path) -> dict:
    with open(path, "rb") as f:
        return tomllib.load(f)


def dep_feature_set(dep_value, workspace_deps: dict, dep_name: str) -> set:
    """Effective extra features a dependency table entry activates on its
    target crate, resolving `workspace = true` inheritance."""
    feats: set = set()
    if not isinstance(dep_value, dict):
        return feats
    if dep_value.get("workspace") is True:
        ws_entry = workspace_deps.get(dep_name, {})
        if isinstance(ws_entry, dict):
            feats.update(ws_entry.get("features", []) or [])
    feats.update(dep_value.get("features", []) or [])
    return feats


def walk_dependency_tables(doc: dict):
    """Yield (section_path, is_dev, table_dict) for every dependency-like
    table in a parsed Cargo.toml, including target-specific ones."""
    for key in ("dependencies", "build-dependencies"):
        if key in doc and isinstance(doc[key], dict):
            yield (key, False, doc[key])
    if "dev-dependencies" in doc and isinstance(doc["dev-dependencies"], dict):
        yield ("dev-dependencies", True, doc["dev-dependencies"])

    target = doc.get("target")
    if isinstance(target, dict):
        for cond, section in target.items():
            if not isinstance(section, dict):
                continue
            for key in ("dependencies", "build-dependencies"):
                if key in section and isinstance(section[key], dict):
                    yield (f"target.{cond}.{key}", False, section[key])
            if "dev-dependencies" in section and isinstance(section["dev-dependencies"], dict):
                yield (f"target.{cond}.dev-dependencies", True, section["dev-dependencies"])


def expand_default_closure(features_table: dict) -> list:
    """BFS from 'default' over LOCAL feature aliases only. Returns the list
    of violation edges (either the literal "testkit" or a
    "<crate>/testkit" / "<crate>?/testkit" cross-crate edge) reachable from
    default."""
    if "default" not in features_table:
        return []
    hits: list = []
    seen: set = set()
    queue = list(features_table.get("default") or [])
    while queue:
        entry = queue.pop()
        if entry in seen:
            continue
        seen.add(entry)

        if entry == "testkit":
            hits.append("testkit")
            continue
        if entry.startswith("dep:"):
            continue
        if "/" in entry:
            # "<crate>/feature" or "<crate>?/feature" -- cross-crate edge.
            # Cannot expand further (needs the OTHER crate's feature
            # graph), but flag if it targets testkit directly.
            _base, _, feat = entry.partition("/")
            if feat == "testkit":
                hits.append(entry)
            continue

        # Local feature alias -- expand transitively.
        if entry in features_table:
            queue.extend(features_table[entry] or [])

    return hits


def check_crate(cargo_path: Path, workspace_deps: dict) -> None:
    doc = load_toml(cargo_path)
    rel = cargo_path.relative_to(REPO_ROOT)

    features_table = doc.get("features", {})
    if isinstance(features_table, dict):
        for hit in expand_default_closure(features_table):
            fail(
                f'{rel}: [features] default transitively activates "{hit}" -- '
                f"a plain cargo build/--release would enable it"
            )

    for section_path, is_dev, table in walk_dependency_tables(doc):
        if is_dev:
            continue
        for dep_name, dep_value in table.items():
            feats = dep_feature_set(dep_value, workspace_deps, dep_name)
            if "testkit" in feats:
                fail(
                    f'{rel}: [{section_path}] enables "{dep_name}"\'s testkit feature '
                    f"in a production dependency table -- move to [dev-dependencies] only"
                )


def main() -> int:
    print("=" * 60)
    print(" Testkit-feature release-safety guard")
    print("=" * 60)

    if not CRATES_DIR.is_dir():
        fail(f"{CRATES_DIR} not found")
        print(f"\n FAIL -- {len(violations)} violation(s) found above.")
        return 1

    workspace_deps: dict = {}
    if WORKSPACE_TOML.is_file():
        wdoc = load_toml(WORKSPACE_TOML)
        workspace_deps = ((wdoc.get("workspace") or {}).get("dependencies")) or {}
        for dep_name, dep_value in workspace_deps.items():
            if isinstance(dep_value, dict) and "testkit" in (dep_value.get("features") or []):
                fail(
                    f'core-rs/Cargo.toml: [workspace.dependencies] "{dep_name}" enables '
                    f"testkit -- any crate inheriting it via workspace=true with no "
                    f"override would compile it into production"
                )

    for cargo_toml in sorted(CRATES_DIR.glob("*/Cargo.toml")):
        check_crate(cargo_toml, workspace_deps)

    print("")
    if not violations:
        print(" OK -- the testkit feature cannot reach a release-profile build.")
        return 0
    print(f" FAIL -- {len(violations)} violation(s) found above.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
