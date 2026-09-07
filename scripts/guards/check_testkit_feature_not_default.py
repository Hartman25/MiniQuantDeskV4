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
# REPAIR-02 (CI-TESTKIT-FEATURE-GUARD-VERIFY-01-REPAIR-02): REPAIR-01's
# dependency-table checker only compared a dependency edge's *literal*
# requested feature strings against "testkit" -- it did not expand a
# cross-crate feature name into the TARGET crate's own [features] table. A
# dependency edge such as `b = { features = ["full"] }` where b declares
# `full = ["testkit"]` silently bypassed the guard even though Cargo itself
# would activate b's testkit feature. This version builds a real workspace-
# wide feature graph (`feature_reaches_testkit` / `dependency_activation_
# reaches_testkit`) and expands every edge -- local aliases, cross-crate
# "<crate>/<feature>" and "<crate>?/<feature>" edges (including through
# `package = "..."` dependency renames), and `dep:<name>` edges -- inside the
# ACTUAL target crate's feature table, recursively, with a visited-set cycle
# guard. Only crates local to this workspace (core-rs/crates/*) can be
# expanded; an edge into an external crate is still flagged if the literal
# feature name is "testkit" (fail-closed), but cannot be expanded further.
#
# Two independent, workspace-wide checks:
#   1. Default-feature closure: for each crate, does activating its own
#      `[features] default` (a plain `cargo build`/`--release` with no
#      explicit --features) reach "testkit", transitively, through local
#      aliases and cross-crate edges (including a dependency's own default
#      features when a default-array entry activates that dependency)?
#   2. Production-dependency closure: for every non-dev dependency table
#      (top-level [dependencies]/[build-dependencies], and their
#      [target.'cfg(...)'.*] equivalents, plus [workspace.dependencies]
#      itself), does any explicitly-requested feature on that edge --
#      including features inherited from the workspace root via
#      `workspace = true` -- reach "testkit" in the target crate's feature
#      graph? Only [dev-dependencies] (and target-specific dev-dependencies)
#      may enable testkit; that scopes the feature to `cargo test`, never to
#      `cargo build`/`cargo build --release` of a binary target.
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


def package_name(doc: dict) -> str | None:
    pkg = doc.get("package")
    if isinstance(pkg, dict):
        return pkg.get("name")
    return None


def dep_feature_set(dep_value, workspace_deps: dict, dep_name: str) -> set:
    """Effective extra features a dependency table entry explicitly requests
    on its target crate, resolving `workspace = true` inheritance."""
    feats: set = set()
    if not isinstance(dep_value, dict):
        return feats
    if dep_value.get("workspace") is True:
        ws_entry = workspace_deps.get(dep_name, {})
        if isinstance(ws_entry, dict):
            feats.update(ws_entry.get("features", []) or [])
    feats.update(dep_value.get("features", []) or [])
    return feats


def dep_default_features_enabled(dep_value) -> bool:
    if isinstance(dep_value, dict):
        return dep_value.get("default-features", dep_value.get("default_features", True)) is not False
    return True


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


def resolve_dep_package(doc: dict, local_name: str, workspace_deps: dict) -> str:
    """Resolve a dependency table entry's real package name, honoring
    `package = "..."` renames (directly, or inherited through
    `workspace = true`). Falls back to the local dependency-table key name
    when no rename is present, which is the common case."""
    for _section_path, _is_dev, table in walk_dependency_tables(doc):
        if local_name in table:
            dep_value = table[local_name]
            if isinstance(dep_value, dict):
                if "package" in dep_value:
                    return dep_value["package"]
                if dep_value.get("workspace") is True:
                    ws_entry = workspace_deps.get(local_name, {})
                    if isinstance(ws_entry, dict) and "package" in ws_entry:
                        return ws_entry["package"]
            return local_name
    return local_name


def find_dep_value(doc: dict, local_name: str):
    for _section_path, _is_dev, table in walk_dependency_tables(doc):
        if local_name in table:
            return table[local_name]
    return None


def feature_reaches_testkit(pkg_name: str, feature_entry: str, crate_docs: dict, workspace_deps: dict, visited: set) -> bool:
    """Does activating `feature_entry` inside crate `pkg_name` reach
    "testkit", directly or transitively through local feature aliases,
    cross-crate "<crate>/<feature>" / "<crate>?/<feature>" / "dep:<crate>"
    edges (resolved through `package = "..."` renames), and a dependency's
    own default features when that dependency edge is what activates it?"""
    if feature_entry == "testkit":
        return True

    key = (pkg_name, feature_entry)
    if key in visited:
        return False
    visited.add(key)

    if feature_entry.startswith("dep:"):
        dep_local = feature_entry[len("dep:") :].rstrip("?")
        return dependency_activation_reaches_testkit(pkg_name, dep_local, crate_docs, workspace_deps, visited)

    if "/" in feature_entry:
        dep_local, _, target_feat = feature_entry.partition("/")
        dep_local = dep_local.rstrip("?")
        return dependency_activation_reaches_testkit(
            pkg_name, dep_local, crate_docs, workspace_deps, visited, extra_feature=target_feat
        )

    doc = crate_docs.get(pkg_name)
    if doc is None:
        return False
    features_table = doc.get("features", {})
    if isinstance(features_table, dict) and feature_entry in features_table:
        for entry in features_table[feature_entry] or []:
            if feature_reaches_testkit(pkg_name, entry, crate_docs, workspace_deps, visited):
                return True
        return False

    # Not a declared [features] key -- may be an implicit feature naming an
    # optional dependency directly. Enabling it only activates that
    # dependency (no further feature recursion beyond its own defaults).
    return dependency_activation_reaches_testkit(pkg_name, feature_entry, crate_docs, workspace_deps, visited)


def dependency_activation_reaches_testkit(
    pkg_name: str, dep_local_name: str, crate_docs: dict, workspace_deps: dict, visited: set, extra_feature: str | None = None
) -> bool:
    doc = crate_docs.get(pkg_name)
    if doc is None:
        return False
    target_pkg = resolve_dep_package(doc, dep_local_name, workspace_deps)
    if target_pkg == "testkit":
        return True
    if target_pkg not in crate_docs:
        return False  # external crate, outside this workspace's control

    dep_value = find_dep_value(doc, dep_local_name)
    explicit_feats = set(dep_feature_set(dep_value, workspace_deps, dep_local_name))
    if extra_feature:
        explicit_feats.add(extra_feature)

    for feat in explicit_feats:
        if feature_reaches_testkit(target_pkg, feat, crate_docs, workspace_deps, visited):
            return True

    if dep_default_features_enabled(dep_value) and feature_reaches_testkit(
        target_pkg, "default", crate_docs, workspace_deps, visited
    ):
        return True

    return False


def check_crate(cargo_path: Path, doc: dict, crate_docs: dict, workspace_deps: dict) -> None:
    rel = cargo_path.relative_to(REPO_ROOT)
    pkg_name = package_name(doc) or cargo_path.parent.name

    features_table = doc.get("features", {})
    if isinstance(features_table, dict) and "default" in features_table:
        if feature_reaches_testkit(pkg_name, "default", crate_docs, workspace_deps, set()):
            fail(
                f'{rel}: [features] default transitively activates "testkit" -- '
                f"a plain cargo build/--release would enable it"
            )

    for section_path, is_dev, table in walk_dependency_tables(doc):
        if is_dev:
            continue
        for dep_name, dep_value in table.items():
            target_pkg = resolve_dep_package(doc, dep_name, workspace_deps)
            feats = dep_feature_set(dep_value, workspace_deps, dep_name)
            for feat in sorted(feats):
                if feature_reaches_testkit(target_pkg, feat, crate_docs, workspace_deps, set()):
                    fail(
                        f'{rel}: [{section_path}] enables "{dep_name}"\'s testkit feature '
                        f'in a production dependency table (via "{feat}") -- move to '
                        f"[dev-dependencies] only"
                    )
                    break


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

    cargo_paths = sorted(CRATES_DIR.glob("*/Cargo.toml"))
    docs: dict[Path, dict] = {p: load_toml(p) for p in cargo_paths}
    crate_docs: dict[str, dict] = {}
    for p, doc in docs.items():
        crate_docs[package_name(doc) or p.parent.name] = doc

    for p, doc in docs.items():
        check_crate(p, doc, crate_docs, workspace_deps)

    print("")
    if not violations:
        print(" OK -- the testkit feature cannot reach a release-profile build.")
        return 0
    print(f" FAIL -- {len(violations)} violation(s) found above.")
    return 1


if __name__ == "__main__":
    sys.exit(main())
