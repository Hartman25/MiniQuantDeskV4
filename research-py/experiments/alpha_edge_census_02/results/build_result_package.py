"""Census-02 Discovery result packager. Read-only on the settled run directory; copies the exact output artifacts and derives
settlement statistics, universe dispositions, the bars/provenance manifest and a SHA-256 run manifest. No raw vendor bars are
copied. Result values are never altered. STRICTLY OFFLINE: it reads only settled local artifacts, never acquires data, reads no
credential, opens no network endpoint, and proves the run directory byte-identical before and after.
Publication is collision-safe and all-or-nothing: everything is built and verified in a private staging directory beside the output,
then published into a missing directory by one atomic rename, or into a directory holding only declared package files that are
byte-identical to the new ones (missing files are added, manifest last). Anything else is refused; nothing is ever replaced.
Usage: python build_result_package.py --run-dir <runs/alpha_edge_census_02> --out <dir> --result-run-head <Result #1 head>"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import shutil
import stat
import sys
import tempfile
from collections import Counter, defaultdict
from pathlib import Path

HERE = Path(__file__).resolve().parent
sys.path.insert(0, str(HERE.parent))
import c2_borrow as bw  # noqa: E402
import c2_data  # noqa: E402  (import performs no I/O; only its pure contract check and LoadedUniverse type are used)
import c2_factor_eval as fe  # noqa: E402
import c2_population as pop  # noqa: E402
import c2_protocol as pr  # noqa: E402
import c2_runner as rn  # noqa: E402
import census as ce  # noqa: E402  (Census-01 read-only manifest / bar loading)
import data as dt  # noqa: E402  (Census-01 read-only eligibility / bar validation; its acquisition functions are never called)
import search_space as ss1  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402

OUTPUTS = ("campaign_disclosure.json", "strategy_edges.json", "conditional_edges.json", "factor_fdr_report.json",
           "strategy_neighborhood_report_only.json")
STRATEGY_LEDGER, FACTOR_LEDGER = "strategy_trial_ledger.jsonl", "factor_evidence_ledger.jsonl"
MANIFEST = "RUN_MANIFEST.json"
# The CLOSED package inventory: every artifact hashed into the manifest, plus the manifest itself. Nothing else may exist in a package.
PACKAGE_ARTIFACTS = tuple(sorted((*OUTPUTS, STRATEGY_LEDGER, FACTOR_LEDGER, "strategy_campaign_summary.json",
                                  "factor_campaign_summary.json", "universe_dispositions.json", "bars_provenance_manifest.json")))
EXPECTED_PACKAGE_FILES = tuple(sorted((*PACKAGE_ARTIFACTS, MANIFEST)))
REGISTRIES = ("registry_strategy.sqlite", "registry_factor.sqlite")
REGISTRY_SIDECAR_SUFFIXES = ("-wal", "-shm", "-journal")
TEMP_ENV_KEYS = ("TMPDIR", "TEMP", "TMP", "SQLITE_TMPDIR")
# Result #1 package settlement (a package-integrity check on THIS preserved run, not an economic threshold).
RESULT01_COUNTS = {"strategy_trials": 9400, "factors": 1075, "factor_status": {"succeeded": 1065, "not_evaluable": 10},
                   "dispositions": {"ELIGIBLE": 88}}
# The commit Result #1 actually executed at (== origin/main when the run started). Historical provenance: never inferred from the
# current HEAD and never overridable.
RESULT01_RUN_HEAD = "778099c6e96d9395b6cfb5e0eee27f2b8186a00e"
# Platform of the host that executed Result #1 (recorded in the committed manifest); historical provenance, not the packaging host's.
RESULT01_RUN_PLATFORM = "Linux-6.18.44-fc-v80-x86_64-with-glibc2.39"


class PackageRefusal(RuntimeError):
    pass


def require_result01_run_head(value) -> str:
    """The recorded result-run head of THIS package is the fixed Result #1 execution head; anything else is refused."""
    if value != RESULT01_RUN_HEAD:
        raise PackageRefusal(f"result-run head {value!r} is not the Result #1 execution head {RESULT01_RUN_HEAD}")
    return value


def run_dir_content_digest(root: Path) -> str:
    """SHA-256 over (relative path, file SHA-256) for every file under `root`: a content-strong read-only proof."""
    h = hashlib.sha256()
    for p in sorted(Path(root).rglob("*")):
        if p.is_file():
            h.update(f"{p.relative_to(root).as_posix()}\0{sha_file(p)}\n".encode("utf-8"))
    return h.hexdigest()


def _stat_overlap(run: Path, path: Path) -> bool:
    """True if `path`, or any EXISTING ancestor of it, is the same filesystem object as `run` (device+inode). Catches aliases that
    string comparison cannot: case-insensitive names, bind mounts, junctions."""
    try:
        rs = os.stat(run)
    except OSError:
        return False
    for cand in (path, *path.parents):
        try:
            if os.path.samestat(rs, os.stat(cand)):
                return True
        except OSError:
            continue
    return False


def require_outside_run_dir(run: Path, out: Path, what: str = "result package output") -> None:
    """Refuse `out` equal to, or anywhere beneath, the settled run directory: by resolved path (relative, `..` and symlink aliases)
    and by filesystem identity. Pure check: it creates and writes nothing."""
    run, out = Path(run).resolve(), Path(out).resolve()
    if out == run or run in out.parents or _stat_overlap(run, out):
        raise PackageRefusal(f"{what} {out} must be outside the settled run directory {run}")


def _temp_env_roots() -> list[tuple[str, str]]:
    """The ONLY environment read in this module: the temporary-root configuration that tempfile / SQLite would honour."""
    return [(k, os.environ[k]) for k in TEMP_ENV_KEYS if os.environ.get(k)]


def require_temp_roots_outside_run(run: Path) -> None:
    """Every configured temporary root (and the working directory, the last-resort fallback) must be outside the run, so that no
    library temp file can land in it. Checked BEFORE any directory is created; staging itself never consults these roots."""
    for key, value in _temp_env_roots():
        require_outside_run_dir(run, Path(value), f"temporary root {key}")
    try:
        cwd = Path.cwd()
    except OSError:
        raise PackageRefusal("the working directory is unavailable (deleted?); refusing to run") from None
    require_outside_run_dir(run, cwd, "working directory")


def require_run_dir(run: Path) -> None:
    if not Path(run).is_dir():
        raise PackageRefusal(f"settled run directory {run} does not exist or is not a directory")
    for n in REGISTRIES:
        if not (Path(run) / n).is_file():
            raise PackageRefusal(f"settled {n} is absent; packaging never creates a registry")
        for suffix in REGISTRY_SIDECAR_SUFFIXES:
            if (Path(run) / (n + suffix)).exists():
                raise PackageRefusal(f"settled {n} has a {suffix} sidecar: the registry is not at rest and cannot be copied faithfully")


def _run_inodes(run: Path) -> set:
    found = set()
    for dirpath, _dirs, files in os.walk(run):
        for f in files:
            st = os.lstat(os.path.join(dirpath, f))
            found.add((st.st_dev, st.st_ino))
    return found


def preflight_output(run: Path, out: Path) -> list[str]:
    """Validate the output location without writing anything. Returns the declared package files already present. An existing
    output must be a directory holding ONLY declared package files, each a plain single-link regular file that is not the run's."""
    if not os.path.lexists(out):
        return []
    if not out.is_dir():
        raise PackageRefusal(f"output {out} exists and is not a directory")
    protected = _run_inodes(run)
    existing = []
    for entry in sorted(os.scandir(out), key=lambda e: e.name):
        st = entry.stat(follow_symlinks=False)
        if entry.name not in EXPECTED_PACKAGE_FILES:
            raise PackageRefusal(f"output holds an undeclared entry {entry.name!r}; a package directory must hold only package files")
        if entry.is_symlink() or not stat.S_ISREG(st.st_mode):
            raise PackageRefusal(f"output entry {entry.name!r} is a symlink or not a regular file")
        if st.st_nlink != 1 or (st.st_dev, st.st_ino) in protected:
            raise PackageRefusal(f"output entry {entry.name!r} is hard-linked and may alias settled-run evidence")
        existing.append(entry.name)
    if MANIFEST in existing:
        try:
            head = json.loads((out / MANIFEST).read_text(encoding="utf-8")).get("result_run_head")
        except (ValueError, AttributeError):
            raise PackageRefusal("the existing RUN_MANIFEST is malformed") from None
        if head != RESULT01_RUN_HEAD:
            raise PackageRefusal("the existing RUN_MANIFEST records a different result-run head; historical provenance is immutable")
    return existing


def load_settled_universe_offline(data_dir: Path, request_contract: dict, protocol_id: str) -> "c2_data.LoadedUniverse":
    """Rebuild the eligible universe, bars/provenance manifest and verified bars from SETTLED local artifacts only. A missing,
    malformed or contract-mismatched artifact is a PackageRefusal; nothing is acquired, repaired, written or read from credentials.
    Reuses only read-only Census-01 validation (classify_eligibility / build_bars_manifest / load_bars); never an acquire_* entrance."""
    pr.assert_freeze_gate_passed(protocol_id)
    c2_data.require_request_contract(request_contract)
    data_dir = Path(data_dir)
    seed = json.loads(bw.SEED_UNIVERSE_FILE.read_text(encoding="utf-8"))
    symbols = sorted(seed["symbols"])
    statuses = {}
    for sym in symbols:
        sdir = data_dir / sym
        status_path = sdir / "status.json"
        if not status_path.is_file():
            raise PackageRefusal(f"{sym}: settled status.json is absent; packaging never acquires data")
        try:
            st = json.loads(status_path.read_text(encoding="utf-8"))
        except ValueError:
            raise PackageRefusal(f"{sym}: settled status.json is malformed") from None
        if not isinstance(st, dict) or st.get("request_contract") != dt.REQUEST_CONTRACT:
            raise PackageRefusal(f"{sym}: settled status.json was not acquired under the frozen request contract")
        if st.get("disposition") == "DATA_PRESENT":
            need = {"research_bars.csv", "research_bars_provenance.json", "corporate_actions_provenance.json",
                    *(st.get("artifact_names") or [])}
            missing = sorted(n for n in need if not (sdir / n).is_file())
            if missing:
                raise PackageRefusal(f"{sym}: settled artifact(s) absent: {missing}; packaging never acquires data")
        statuses[sym] = st
    dispositions = {s: dt.classify_eligibility(s, statuses[s], data_dir / s) for s in symbols}
    universe = ss1.build_universe(seed, dispositions)
    manifest = ce.build_bars_manifest(universe, data_dir)
    return c2_data.LoadedUniverse(universe, ce.load_bars(universe, data_dir, manifest), manifest)


def strategy_ledger_bytes(chunk_paths: list[Path], population_ids: list[str]) -> bytes:
    """The settled chunk ledgers, concatenated byte-for-byte in canonical chunk order. Refuses unless the trial ids, in order,
    are exactly the frozen population (count, duplicates, missing, extra and order are all checked)."""
    raw = b"".join(Path(p).read_bytes() for p in chunk_paths)
    lines = raw.split(b"\n")
    if lines[-1] != b"" or any(not ln for ln in lines[:-1]):
        raise PackageRefusal("Strategy chunk ledgers are not newline-terminated non-empty JSONL lines")
    ids = [json.loads(ln)["t"] for ln in lines[:-1]]
    if len(ids) != len(population_ids):
        raise PackageRefusal(f"Strategy ledger has {len(ids)} rows, frozen population has {len(population_ids)}")
    if len(set(ids)) != len(ids):
        raise PackageRefusal("Strategy ledger contains a duplicated trial id")
    if set(ids) != set(population_ids):
        raise PackageRefusal(f"Strategy ledger ids differ from the frozen population: missing={len(set(population_ids) - set(ids))} "
                             f"extra={len(set(ids) - set(population_ids))}")
    if ids != list(population_ids):
        raise PackageRefusal("Strategy ledger is not in frozen population order")
    return raw


def factor_ledger_bytes(records: list[dict], expected_ids: list[str], expected_status: dict | None) -> bytes:
    """One canonical-JSON line per settled factor record, in frozen population order (the registry-bound records are copied, never
    re-evaluated). Refuses on count / duplicate / missing / extra / order, and on a status mix other than `expected_status`."""
    ids = [r["factor_id"] for r in records]
    if len(ids) != len(expected_ids):
        raise PackageRefusal(f"factor ledger has {len(ids)} records, frozen population has {len(expected_ids)}")
    if len(set(ids)) != len(ids):
        raise PackageRefusal("factor ledger contains a duplicated factor id")
    if set(ids) != set(expected_ids):
        raise PackageRefusal(f"factor ledger ids differ from the frozen population: missing={len(set(expected_ids) - set(ids))} "
                             f"extra={len(set(ids) - set(expected_ids))}")
    if ids != list(expected_ids):
        raise PackageRefusal("factor ledger is not in frozen population order")
    status = dict(Counter(r["status"] for r in records))
    if expected_status is not None and status != expected_status:
        raise PackageRefusal(f"factor statuses {status} differ from the expected settlement {expected_status}")
    return b"".join(json.dumps(r, sort_keys=True, separators=(",", ":")).encode("utf-8") + b"\n" for r in records)


def sha_file(p: Path) -> str:
    return hashlib.sha256(Path(p).read_bytes()).hexdigest()


def dump(path: Path, obj) -> None:
    Path(path).write_text(json.dumps(obj, sort_keys=True, indent=1) + "\n", encoding="utf-8", newline="\n")


def strategy_summary(rows: list[dict]) -> dict:
    def by(key):
        out = defaultdict(Counter)
        for r in rows:
            out[r[key]][r["outcome"]] += 1
        return {k: dict(sorted(v.items())) for k, v in sorted(out.items())}
    bands = Counter((r["band"] or "NONE", r["outcome"]) for r in rows)
    qual = [r for r in rows if r["outcome"] == "QUALIFIED"]
    return {"trials": len(rows), "outcomes": dict(sorted(Counter(r["outcome"] for r in rows).items())),
            "dispositions": dict(sorted(Counter(r["d"] for r in rows).items())),
            "by_side": by("side"), "by_family": by("family"), "by_symbol": by("s"),
            "trade_count_bands_all": dict(sorted(Counter(r["band"] or "NONE" for r in rows).items())),
            "trade_count_bands_qualified": dict(sorted(Counter(r["band"] for r in qual).items())),
            "band_by_outcome": {f"{b}|{o}": n for (b, o), n in sorted(bands.items())},
            "complement_tagged_trials": sum(1 for r in rows if r["tags"]["complement_of_census01"]),
            "complement_tagged_qualified": sum(1 for r in qual if r["tags"]["complement_of_census01"]),
            "qualified_executable_pnl_true": sum(1 for r in qual if r["executable_pnl"] is True),
            "executable_pnl_true_all": sum(1 for r in rows if r["executable_pnl"] is True)}


def factor_summary(items, records, fdr, cedges, factor_db: Path) -> dict:
    meta = {s.compute_factor_id(): (c["family"], h) for c, h, s in items}
    ev = [r for r in records if r["status"] == "succeeded"]
    pv = {r["factor_id"]: r["pvalue"]["p_value"] for r in records if r.get("pvalue")}
    q = fdr["q_values"]
    cls = Counter(e["class"] for e in cedges)
    def by(idx, ids):
        return dict(sorted(Counter(meta[i][idx] for i in ids).items()))
    edge_ids = [e["factor_id"] for e in cedges]
    store = ResearchResultStore(factor_db)
    atts = [a for _c, _h, s in items for a in store.list_factor_evaluation_attempts(s.compute_factor_id())]
    return {"registered": len(items), "records": len(records), "evaluable": len(ev), "non_evaluable": len(records) - len(ev),
            "non_evaluable_failure_reasons": dict(sorted(Counter(a.get("failure_reason") for a in atts if a["status"] == "not_evaluable").items())),
            "non_evaluable_by_family_horizon": dict(sorted(Counter(f"{meta[a['factor_id']][0]}|h{meta[a['factor_id']][1]}" for a in atts if a["status"] == "not_evaluable").items())),
            "non_evaluable_factor_ids": sorted(fdr["excluded_factor_ids_with_reasons"]),
            "fdr_status": fdr["status"], "fdr_declared_population": fdr["declared_population_count"],
            "fdr_tested_hypotheses": fdr["hypothesis_count"], "fdr_alpha": fdr["alpha"],
            "classes_highest": dict(sorted(cls.items())), "min_raw_p": min(pv.values()), "min_bh_q": min(q.values()),
            "bh_q_le_alpha_count_any_direction": len(fdr["rejected_factor_ids"]),
            "bh_q_le_alpha_and_favorable_direction_count": cls.get("DISCOVERED_STRONG", 0),
            "classes_by_family": {k: dict(sorted(Counter(e["class"] for e in cedges if meta[e["factor_id"]][0] == k).items()))
                                  for k in sorted({m[0] for m in meta.values()})},
            "edges_by_family": by(0, edge_ids), "edges_by_horizon": {str(k): v for k, v in by(1, edge_ids).items()},
            "factors_by_family": by(0, list(meta)), "factors_by_horizon": {str(k): v for k, v in by(1, list(meta)).items()},
            "complement_related_factors": sum(1 for c, _h, _s in items if c["family"] in pop.gr.COMPLEMENT_FAMILIES),
            "attempts_total": len(atts), "attempts_by_status": dict(sorted(Counter(a["status"] for a in atts).items())),
            "attempts_per_factor_max": max(Counter(a["factor_id"] for a in atts).values())}


def artifact_hashes(pkg: Path) -> dict:
    """SHA-256 of exactly the declared package artifacts (never a directory glob); every one must exist."""
    missing = [n for n in PACKAGE_ARTIFACTS if not (Path(pkg) / n).is_file()]
    if missing:
        raise PackageRefusal(f"package artifact(s) missing before the manifest: {missing}")
    return {n: sha_file(Path(pkg) / n) for n in PACKAGE_ARTIFACTS}


def verify_package_dir(pkg: Path) -> None:
    """A complete package holds EXACTLY the declared files, and its manifest hashes exactly the declared artifacts."""
    pkg = Path(pkg)
    names = sorted(p.name for p in pkg.iterdir())
    if names != list(EXPECTED_PACKAGE_FILES):
        raise PackageRefusal(f"package inventory {names} differs from the declared {len(EXPECTED_PACKAGE_FILES)} files")
    manifest = json.loads((pkg / MANIFEST).read_text(encoding="utf-8"))
    if manifest.get("preserved_artifact_sha256") != artifact_hashes(pkg) or manifest.get("result_run_head") != RESULT01_RUN_HEAD:
        raise PackageRefusal("package manifest does not match the package files / Result #1 execution head")


def _write_new(dest: Path, data: bytes) -> None:
    """Create `dest` exclusively through a private temporary name beside it: never opens, truncates or follows an existing entry."""
    tmp = dest.with_name(f".{dest.name}.partial")
    flags = os.O_WRONLY | os.O_CREAT | os.O_EXCL | getattr(os, "O_NOFOLLOW", 0) | getattr(os, "O_BINARY", 0)
    fd = os.open(tmp, flags, 0o644)
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
        os.replace(tmp, dest)
    except BaseException:
        tmp.unlink(missing_ok=True)
        raise


def publish(pkg: Path, out: Path, existing: list[str]) -> dict:
    """Publish a verified staged package. Never replaces anything: a declared file already present must be byte-identical (and is then
    left untouched); a missing output directory is created by one atomic rename; otherwise only missing files are added, manifest
    last. Returns what was done; on failure everything this call created is removed."""
    staged = {p.name: p.read_bytes() for p in Path(pkg).iterdir()}
    for name in existing:
        if (out / name).read_bytes() != staged[name]:
            raise PackageRefusal(f"existing {name} differs from the freshly built package; published evidence is never replaced")
    if not os.path.lexists(out):
        os.rename(pkg, out)                                        # atomic: same parent directory, hence same filesystem
        return {"created_dir": True, "published": list(EXPECTED_PACKAGE_FILES), "already_identical": []}
    added = []
    try:
        for name in [n for n in EXPECTED_PACKAGE_FILES if n != MANIFEST] + [MANIFEST]:   # manifest last: no manifest => incomplete
            if name in existing:
                continue
            _write_new(out / name, staged[name])
            added.append(name)
    except BaseException:
        for name in added:
            (out / name).unlink(missing_ok=True)
        raise
    return {"created_dir": False, "published": added, "already_identical": sorted(existing)}


def parse_args(argv=None):
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--run-dir", required=True, type=Path)
    ap.add_argument("--out", required=True, type=Path)
    ap.add_argument("--result-run-head", required=True, help="must equal the fixed Result #1 execution head")
    return ap.parse_args(argv)


def main(argv=None) -> None:
    a = parse_args(argv)
    run, out = a.run_dir.resolve(), a.out.resolve()
    require_result01_run_head(a.result_run_head)
    require_run_dir(run)                                 # every check from here to staging is pure: it creates and writes nothing
    require_outside_run_dir(run, out)
    require_temp_roots_outside_run(run)
    require_outside_run_dir(run, out.parent, "output parent directory")
    existing = preflight_output(run, out)
    before = run_dir_content_digest(run)
    created_parents, stage, published, ok = [], None, None, False
    try:
        for d in reversed([p for p in (out.parent, *out.parent.parents) if not p.exists()]):
            d.mkdir()
            created_parents.append(d)
        # Staging lives beside the output through an EXPLICIT dir: tempfile never consults TMPDIR/TEMP/TMP here, and the result is
        # re-validated. The registry stores run DDL on open, so they only ever see private copies in the staging directory.
        candidate = Path(tempfile.mkdtemp(prefix=".census02-stage-", dir=out.parent))
        require_outside_run_dir(run, candidate, "staging directory")      # never adopted (so never deleted) if it is inside the run
        stage = candidate
        pkg, regdir = stage / "package", stage / "registries"
        pkg.mkdir()
        regdir.mkdir()
        regs = {n: Path(shutil.copyfile(run / n, regdir / n)) for n in REGISTRIES}
        package(run, pkg, regs)
        verify_package_dir(pkg)
        if run_dir_content_digest(run) != before:
            raise PackageRefusal("the settled run directory changed during packaging (read-only invariant violated)")
        published = publish(pkg, out, existing)
        verify_package_dir(out)
        ok = True
    except BaseException:
        if published is not None:                        # undo exactly what this call published
            if published["created_dir"]:
                shutil.rmtree(out, ignore_errors=True)
            else:
                for name in published["published"]:
                    (out / name).unlink(missing_ok=True)
        raise
    finally:
        if stage is not None:
            shutil.rmtree(stage, ignore_errors=True)
        if not ok:
            for d in reversed(created_parents):          # leave no residue from a failed run
                try:
                    d.rmdir()
                except OSError:
                    pass
    print(json.dumps({"out": str(out), "files": len(EXPECTED_PACKAGE_FILES), "run_dir_unchanged": True, **published}))


def package(run: Path, out: Path, regs: dict) -> None:
    frozen = pr.require_freeze()
    doc = frozen["predeclaration"]
    protocol_id, decisions = doc["protocol_id"], doc["decisions"]
    loaded = load_settled_universe_offline(run / "data", doc["data_request_contract"], protocol_id)
    got = dict(Counter(d["disposition"] for d in loaded.universe["dispositions"].values()))
    if got != RESULT01_COUNTS["dispositions"]:
        raise PackageRefusal(f"universe dispositions {got} differ from the settled Result #1 universe")
    cells = pop.strategy_cells(decisions, protocol_id)
    rows = rn.load_ledger(run, cells)
    pr.assert_complete_ledger([c[3] for c in cells], [r["t"] for r in rows])
    ctx = fe.population_context(loaded.universe, loaded.bars_manifest, protocol_id)
    items = fe.materialize_specs(decisions, ctx, doc["factor_population"])
    records = fe.load_factor_records(regs["registry_factor.sqlite"], run / "factor_records", items)
    fdr = json.loads((run / "factor_fdr_report.json").read_text(encoding="utf-8"))
    cedges = json.loads((run / "conditional_edges.json").read_text(encoding="utf-8"))
    out.mkdir(exist_ok=True)
    for name in OUTPUTS:
        shutil.copyfile(run / name, out / name)
    chunk_paths = [rn.chunk_path(run, k) for k in range((len(cells) + rn.CHUNK_SIZE - 1) // rn.CHUNK_SIZE)]
    if len(cells) != RESULT01_COUNTS["strategy_trials"] or len(items) != RESULT01_COUNTS["factors"]:
        raise PackageRefusal("frozen populations differ from the Result #1 counts")
    s_bytes = strategy_ledger_bytes(chunk_paths, [c[3] for c in cells])
    f_bytes = factor_ledger_bytes(records, [s.compute_factor_id() for _c, _h, s in items], RESULT01_COUNTS["factor_status"])
    (out / STRATEGY_LEDGER).write_bytes(s_bytes)
    (out / FACTOR_LEDGER).write_bytes(f_bytes)
    # Written bytes must equal the settled run authority: chunk bytes, and the registry-bound factor records re-read from disk.
    if (out / STRATEGY_LEDGER).read_bytes() != b"".join(p.read_bytes() for p in chunk_paths):
        raise PackageRefusal("preserved Strategy ledger differs from the settled chunk ledgers")
    on_disk = [json.loads(ln) for ln in (out / FACTOR_LEDGER).read_bytes().splitlines()]
    if on_disk != records or on_disk != [fe.cd1.read_factor_record(run / "factor_records", r["factor_id"]) for r in records]:
        raise PackageRefusal("preserved factor ledger differs from the settled factor records")
    store = ResearchResultStore(regs["registry_strategy.sqlite"])
    digest = store.trial_attempt_digest(pr.EXPERIMENT_ID)
    ledger_ids = sorted(r["t"] for r in rows)
    attempts = {"registered_trials": len(digest), "attempts": sum(d["attempts"] for d in digest.values()),
                "succeeded": sum(d["succeeded"] for d in digest.values()), "failed": sum(d["failed"] for d in digest.values()),
                "started_unfinalized": sum(d["started"] for d in digest.values()), "blocked": sum(d["blocked"] for d in digest.values()),
                "max_attempts_per_trial": max(d["attempts"] for d in digest.values()),
                "interrupted_or_retried_trials": sum(1 for d in digest.values() if d["attempts"] != 1),
                "registered_equals_frozen_population": sorted(digest) == sorted(c[3] for c in cells),
                "ledger_trial_ids_sha256": hashlib.sha256("\n".join(ledger_ids).encode()).hexdigest(),
                "trial_attempt_digest_sha256": pr.sha256_canonical(digest)}
    dump(out / "strategy_campaign_summary.json", {"attempts": attempts, **strategy_summary(rows)})
    dump(out / "factor_campaign_summary.json", factor_summary(items, records, fdr, cedges, regs["registry_factor.sqlite"]))
    dump(out / "universe_dispositions.json", {"universe_id": loaded.bars_manifest["universe_id"],
         "seed_symbols": len(loaded.universe["dispositions"]), "eligible": len(loaded.universe["symbols"]),
         "disposition_counts": dict(sorted(Counter(d["disposition"] for d in loaded.universe["dispositions"].values()).items())),
         "dispositions": loaded.universe["dispositions"]})
    mf = loaded.bars_manifest
    dump(out / "bars_provenance_manifest.json", mf)
    last = max(r["last_end_ts"] for r in mf["symbols"].values() if "last_end_ts" in r)
    first = min(r["first_end_ts"] for r in mf["symbols"].values() if "first_end_ts" in r)
    if not last < "2024-01-01":
        raise PackageRefusal(f"a settled bar ends at {last}: Discovery bars must end strictly before 2024-01-01")
    chunks = {p.name: sha_file(p) for p in chunk_paths}
    produced = artifact_hashes(out)
    dump(out / MANIFEST, {
        "schema_version": "census02_result_package_v1", "status": "DISCOVERY_ONLY", "VALIDATION_STATUS": "NOT_VALIDATED",
        "PROMOTION_AUTHORITY": "NONE", "protocol_id": protocol_id, "behavior_head": doc["behavior_head"],
        "result_run_head": RESULT01_RUN_HEAD, "environment_identity": doc["environment_identity"],
        "runtime_python_platform": RESULT01_RUN_PLATFORM, "data_request_contract": doc["data_request_contract"],
        "discovery_window": "[2016-01-01, 2024-01-01)", "bars_first_end_ts": first, "bars_last_end_ts": last,
        "bars_manifest_sha256": mf["manifest_sha256"], "universe_id": mf["universe_id"],
        "strategy_population_root": doc["strategy_population"]["population_root"],
        "factor_coordinate_root": doc["factor_population"]["coordinate_root"],
        "reads_2024_confirmation_holdout": {"bars_with_end_ts_ge_2024_01_01": 0,
                                            "fence": "c2_data.load_discovery_universe + fence_bars; request end exclusive 2024-01-01"},
        "attempt_registry": attempts, "strategy_ledger_chunk_sha256": chunks,
        "registry_sqlite_not_committed": {"registry_strategy.sqlite": sha_file(run / "registry_strategy.sqlite"),
                                          "registry_factor.sqlite": sha_file(run / "registry_factor.sqlite")},
        "evidence_ledgers": {
            STRATEGY_LEDGER: {"records": len(cells), "order": "frozen population order (chunk_00000..chunk_00018, line order)",
                              "source": "byte concatenation of the 19 settled chunk ledgers; not recomputed"},
            FACTOR_LEDGER: {"records": len(items), "order": "frozen factor population order",
                            "source": "registry-bound settled factor records (canonical JSON, one per line); not re-evaluated",
                            "status_counts": dict(Counter(r["status"] for r in records))},
            "note": "compact committed copies of the settled evidence; they are NOT the registry SQLite files"},
        "raw_vendor_data": "NOT COMMITTED; per-symbol artifact hashes are in bars_provenance_manifest.json",
        "preserved_artifact_sha256": produced})


if __name__ == "__main__":
    main()
