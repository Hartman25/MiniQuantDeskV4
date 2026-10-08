"""Census-02 Result #1 packager invariants: the preserved Strategy / factor ledgers are exact copies of the settled evidence, the
packager refuses any count / duplicate / missing / extra / order / status-mix deviation, is strictly offline (no acquisition, no
credential, no network) and keeps the immutable Result #1 execution head. Synthetic fixtures; the real-run tests need the local run
directory and are skipped (with an explicit reason) only where it does not exist, e.g. a fresh CI checkout."""

from __future__ import annotations

import ast
import json
import os
import socket
import subprocess
import sys
import tempfile
import urllib.request
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))
import c2_testing as ct  # noqa: E402,F401  (puts the Census-02 modules on sys.path)

sys.path.insert(0, str(ct.EXP2 / "results"))
import build_result_package as bp  # noqa: E402
import c2_borrow as bw  # noqa: E402
import c2_data  # noqa: E402
import c2_protocol as pr  # noqa: E402
import data as dt  # noqa: E402
from mqk_research.data import alpaca_historical as ah  # noqa: E402

PACKAGE = ct.EXP2 / "results" / "discovery_result_01"
RESULT01_HEAD = "778099c6e96d9395b6cfb5e0eee27f2b8186a00e"
needs_real_run = pytest.mark.skipif(not ct.REAL_RUN_DIR.exists(), reason="the settled local Result #1 run directory is absent")


def _chunks(tmp_path: Path, ids: list[str], size: int) -> list[Path]:
    paths = []
    for k in range(0, len(ids), size):
        p = tmp_path / f"chunk_{k // size:05d}.jsonl"
        p.write_text("".join(json.dumps({"t": t, "outcome": "NOT_QUALIFIED"}, sort_keys=True, separators=(",", ":")) + "\n"
                             for t in ids[k:k + size]), encoding="utf-8", newline="\n")
        paths.append(p)
    return paths


IDS = [f"ac02-{i:04d}" for i in range(7)]


def test_strategy_ledger_is_the_exact_chunk_concatenation_in_population_order(tmp_path):
    paths = _chunks(tmp_path, IDS, 3)
    out = bp.strategy_ledger_bytes(paths, IDS)
    assert out == b"".join(p.read_bytes() for p in paths) and out.count(b"\n") == len(IDS)


@pytest.mark.parametrize("mutate,match", [
    (lambda ids: ids[:-1], "rows"),                                  # dropped line
    (lambda ids: ids + ["ac02-extra"], "rows"),                      # extra line
    (lambda ids: ids[:-1] + [ids[0]], "duplicated"),                 # duplicate id (count preserved)
    (lambda ids: ids[:-1] + ["ac02-other"], "differ"),               # same count, different id set
    (lambda ids: [ids[1], ids[0], *ids[2:]], "order"),               # same set, different order
])
def test_strategy_ledger_refuses_any_deviation_from_the_frozen_population(tmp_path, mutate, match):
    with pytest.raises(bp.PackageRefusal, match=match):
        bp.strategy_ledger_bytes(_chunks(tmp_path, mutate(list(IDS)), 3), IDS)


def test_strategy_ledger_refuses_a_blank_or_unterminated_line(tmp_path):
    p = tmp_path / "chunk_00000.jsonl"
    p.write_text('{"t":"a"}\n\n{"t":"b"}\n', encoding="utf-8")
    with pytest.raises(bp.PackageRefusal, match="newline-terminated"):
        bp.strategy_ledger_bytes([p], ["a", "b"])
    p.write_text('{"t":"a"}\n{"t":"b"}', encoding="utf-8")
    with pytest.raises(bp.PackageRefusal, match="newline-terminated"):
        bp.strategy_ledger_bytes([p], ["a", "b"])


FIDS = [f"f{i:02d}" for i in range(5)]


def _records(ids=FIDS, bad=1):
    return [{"factor_id": f, "evaluation_id": "e" + f, "status": "not_evaluable" if i < bad else "succeeded",
             **({"reason": "zero_variance_factor"} if i < bad else {"pvalue": {"p_value": 0.5}})} for i, f in enumerate(ids)]


def test_factor_ledger_is_canonical_one_line_per_record_and_round_trips():
    recs = _records()
    out = bp.factor_ledger_bytes(recs, FIDS, {"succeeded": 4, "not_evaluable": 1})
    assert [json.loads(ln) for ln in out.splitlines()] == recs and out.endswith(b"\n")
    assert out.splitlines()[0] == json.dumps(recs[0], sort_keys=True, separators=(",", ":")).encode("utf-8")


@pytest.mark.parametrize("make,match", [
    (lambda: _records()[:-1], "records"),                                              # dropped record
    (lambda: _records() + [{"factor_id": "fx", "status": "succeeded"}], "records"),    # extra record
    (lambda: [*_records()[:-1], _records()[0]], "duplicated"),                         # duplicate id
    (lambda: [*_records()[:-1], {"factor_id": "other", "status": "succeeded"}], "differ"),
    (lambda: list(reversed(_records())), "order"),
])
def test_factor_ledger_refuses_any_deviation_from_the_frozen_population(make, match):
    with pytest.raises(bp.PackageRefusal, match=match):
        bp.factor_ledger_bytes(make(), FIDS, {"succeeded": 4, "not_evaluable": 1})


def test_factor_ledger_refuses_an_unexpected_status_mix():
    with pytest.raises(bp.PackageRefusal, match="statuses"):
        bp.factor_ledger_bytes(_records(bad=2), FIDS, {"succeeded": 4, "not_evaluable": 1})
    assert bp.factor_ledger_bytes(_records(bad=2), FIDS, None)          # no expectation supplied: only identity checks apply


def test_result01_counts_are_the_frozen_population_sizes():
    assert bp.RESULT01_COUNTS == {"strategy_trials": 9400, "factors": 1075, "factor_status": {"succeeded": 1065, "not_evaluable": 10},
                                  "dispositions": {"ELIGIBLE": 88}}


# ----------------------------------------------------------------------------------------------- D1: strictly offline packaging
@pytest.fixture
def provider_traps(monkeypatch):
    """Every acquisition / credential / network entrance fails the test if it is reached."""
    hits = []

    def boom(name):
        def _f(*a, **k):
            hits.append(name)
            raise AssertionError(f"offline packager reached {name}")
        return _f
    for mod, names in ((dt, ("acquire_universe", "acquire_symbol", "load_alpaca_env")), (c2_data, ("load_discovery_universe",)),
                       (ah, ("extract_research_bars_with_provenance", "write_research_extraction_artifacts")),
                       (urllib.request, ("urlopen",))):
        for n in names:
            monkeypatch.setattr(mod, n, boom(f"{mod.__name__}.{n}"))
    monkeypatch.setattr(socket.socket, "connect", boom("socket.connect"))
    yield hits
    assert hits == []


def _settled(tmp_path, monkeypatch, statuses: dict[str, dict | None]) -> Path:
    """A synthetic settled data dir + seed for the symbols in `statuses` (None => no status.json at all)."""
    seed = tmp_path / "seed.json"
    seed.write_text(json.dumps({"symbols": sorted(statuses), "survivorship_classification": "TEST"}), encoding="utf-8")
    monkeypatch.setattr(bw, "SEED_UNIVERSE_FILE", seed)
    monkeypatch.setitem(pr._GATE, "protocol_id", "pid")
    data = tmp_path / "data"
    for sym, st in statuses.items():
        (data / sym).mkdir(parents=True)
        if st is not None:
            (data / sym / "status.json").write_text(json.dumps({"request_contract": dt.REQUEST_CONTRACT, **st}), encoding="utf-8")
    return data


EXCLUDED_CA = {"disposition": "NON_EVALUABLE_UNSUPPORTED_CORPORATE_ACTION", "detail": "synthetic"}


def _load(data):
    return bp.load_settled_universe_offline(data, pr.DATA_REQUEST_CONTRACT, "pid")


def test_offline_loader_success_path_never_reaches_acquisition(tmp_path, monkeypatch, provider_traps):
    data = _settled(tmp_path, monkeypatch, {"AAA": EXCLUDED_CA, "BBB": EXCLUDED_CA})
    loaded = _load(data)
    assert loaded.universe["symbols"] == [] and sorted(loaded.universe["dispositions"]) == ["AAA", "BBB"]
    assert {d["disposition"] for d in loaded.universe["dispositions"].values()} == {"EXCLUDED_UNSUPPORTED_CORPORATE_ACTION"}
    assert loaded.bars == {} and loaded.bars_manifest["symbols"] == {}


def test_a_missing_status_is_refused_not_fetched(tmp_path, monkeypatch, provider_traps):
    data = _settled(tmp_path, monkeypatch, {"AAA": EXCLUDED_CA, "BBB": None})
    with pytest.raises(bp.PackageRefusal, match="BBB: settled status.json is absent"):
        _load(data)
    assert not (data / "BBB" / "status.json").exists()                       # nothing was repaired or written


@pytest.mark.parametrize("artifact", ["research_bars.csv", "research_bars_provenance.json", "corporate_actions_provenance.json",
                                      "corporate_actions.json"])
def test_a_missing_bar_or_provenance_artifact_is_refused_before_provider_access(tmp_path, monkeypatch, provider_traps, artifact):
    names = ["research_bars.csv", "research_bars_provenance.json", "corporate_actions_provenance.json", "corporate_actions.json"]
    data = _settled(tmp_path, monkeypatch, {"AAA": {"disposition": "DATA_PRESENT", "artifact_names": names}})
    for n in names:
        if n != artifact:
            (data / "AAA" / n).write_text("{}", encoding="utf-8")
    with pytest.raises(bp.PackageRefusal, match=f"AAA: settled artifact.*{artifact}"):
        _load(data)


def test_a_malformed_or_foreign_contract_status_is_refused(tmp_path, monkeypatch, provider_traps):
    data = _settled(tmp_path, monkeypatch, {"AAA": EXCLUDED_CA})
    (data / "AAA" / "status.json").write_text("{not json", encoding="utf-8")
    with pytest.raises(bp.PackageRefusal, match="malformed"):
        _load(data)
    (data / "AAA" / "status.json").write_text(json.dumps({"request_contract": {**dt.REQUEST_CONTRACT, "feed": "iex"}, **EXCLUDED_CA}))
    with pytest.raises(bp.PackageRefusal, match="frozen request contract"):
        _load(data)
    (data / "AAA" / "status.json").write_text(json.dumps(EXCLUDED_CA))      # no recorded contract at all
    with pytest.raises(bp.PackageRefusal, match="frozen request contract"):
        _load(data)


def test_the_offline_loader_still_requires_the_freeze_gate_and_the_frozen_contract(tmp_path, monkeypatch, provider_traps):
    data = _settled(tmp_path, monkeypatch, {"AAA": EXCLUDED_CA})
    monkeypatch.setitem(pr._GATE, "protocol_id", "other")
    with pytest.raises(pr.FreezeRefusal):
        _load(data)
    monkeypatch.setitem(pr._GATE, "protocol_id", "pid")
    with pytest.raises(c2_data.DataContractRefusal):
        bp.load_settled_universe_offline(data, {**pr.DATA_REQUEST_CONTRACT, "feed": "iex"}, "pid")


FORBIDDEN_CALLS = {"acquire_universe", "acquire_symbol", "load_alpaca_env", "load_discovery_universe", "urlopen",
                   "extract_research_bars_with_provenance", "write_research_extraction_artifacts", "getenv", "run_campaign",
                   "run_strategy_chunks", "run_factors", "evaluate_factor", "family_fdr_report", "register_strategy_population",
                   "register_factor_population", "begin_attempts_bulk", "system", "Popen", "popen", "check_output"}
NETWORK_MODULES = {"requests", "urllib", "http", "socket", "aiohttp", "httpx", "ssl", "subprocess", "platform"}


def _packager_tree():
    return ast.parse((ct.EXP2 / "results" / "build_result_package.py").read_text(encoding="utf-8"))


def test_the_packager_source_cannot_reach_acquisition_credentials_network_or_recomputation():
    tree = _packager_tree()
    for n in ast.walk(tree):
        mods = [a.name for a in n.names] if isinstance(n, ast.Import) else [n.module or ""] if isinstance(n, ast.ImportFrom) else []
        for m in mods:
            assert not set(m.split(".")) & NETWORK_MODULES, f"imports forbidden module {m}"
        if isinstance(n, ast.Call):
            name = n.func.attr if isinstance(n.func, ast.Attribute) else getattr(n.func, "id", "")
            assert name not in FORBIDDEN_CALLS, f"calls {name}"
    called = {n.func.attr if isinstance(n.func, ast.Attribute) else getattr(n.func, "id", "")
              for n in ast.walk(tree) if isinstance(n, ast.Call)}
    assert "load_settled_universe_offline" in called


def test_the_only_environment_read_is_the_temp_root_helper_with_literal_temp_keys():
    tree = _packager_tree()
    funcs = {f.name: f for f in ast.walk(tree) if isinstance(f, ast.FunctionDef)}
    inside = {id(n) for n in ast.walk(funcs["_temp_env_roots"])}
    reads = [n for n in ast.walk(tree) if isinstance(n, ast.Attribute) and n.attr in ("environ", "getenv")]
    assert reads and all(id(n) in inside for n in reads), "an environment read exists outside _temp_env_roots"
    assert set(bp.TEMP_ENV_KEYS) == {"TMPDIR", "TEMP", "TMP", "SQLITE_TMPDIR"}
    assert not [n for n in ast.walk(tree) if isinstance(n, ast.Name) and n.id == "environ"]


def test_package_inventory_is_derived_from_the_declared_names_never_a_directory_glob():
    tree = _packager_tree()
    for fn in (f for f in ast.walk(tree) if isinstance(f, ast.FunctionDef) and f.name in ("package", "artifact_hashes", "main")):
        for n in ast.walk(fn):
            if isinstance(n, ast.Call) and isinstance(n.func, ast.Attribute):
                assert n.func.attr not in ("glob", "rglob"), f"{fn.name} derives an inventory from a glob"


@needs_real_run
def test_real_offline_universe_equals_the_committed_bars_manifest(provider_traps):
    pr.require_freeze()
    doc = json.loads(pr.PREDECLARATION_FILE.read_text(encoding="utf-8"))
    loaded = bp.load_settled_universe_offline(ct.REAL_RUN_DIR / "data", doc["data_request_contract"], doc["protocol_id"])
    committed = json.loads((PACKAGE / "bars_provenance_manifest.json").read_text(encoding="utf-8"))
    assert loaded.bars_manifest == committed and len(loaded.universe["symbols"]) == 88 and len(loaded.bars) == 88


# ----------------------------------------------------------------------------------- D3: immutable Result #1 execution head
def test_the_committed_manifest_records_the_fixed_result01_head():
    assert json.loads((PACKAGE / "RUN_MANIFEST.json").read_text(encoding="utf-8"))["result_run_head"] == RESULT01_HEAD
    assert bp.RESULT01_RUN_HEAD == RESULT01_HEAD


def test_only_the_exact_result01_head_is_accepted():
    assert bp.require_result01_run_head(RESULT01_HEAD) == RESULT01_HEAD
    rev = subprocess.run(["git", "rev-parse", "HEAD"], cwd=pr.REPO, capture_output=True, text=True)
    current = rev.stdout.strip() if rev.returncode == 0 else ""          # a commit-less throwaway checkout has no HEAD to try
    heads = [current] if len(current) == 40 and current != RESULT01_HEAD else []
    for bad in (*heads, "1" * 40, "0123456789abcdef0123456789abcdef01234567", RESULT01_HEAD[:39], RESULT01_HEAD.upper(), "", None):
        with pytest.raises(bp.PackageRefusal, match="not the Result #1 execution head"):
            bp.require_result01_run_head(bad)


def test_an_omitted_head_is_refused_by_the_cli(tmp_path):
    with pytest.raises(SystemExit):
        bp.parse_args(["--run-dir", str(tmp_path), "--out", str(tmp_path)])
    assert bp.parse_args(["--run-dir", "r", "--out", "o", "--result-run-head", RESULT01_HEAD]).result_run_head == RESULT01_HEAD


def test_run_dir_content_digest_sees_a_same_size_same_mtime_rewrite(tmp_path):
    import os
    f = tmp_path / "x.bin"
    f.write_bytes(b"abcd")
    st = f.stat()
    d0 = bp.run_dir_content_digest(tmp_path)
    f.write_bytes(b"abce")
    os.utime(f, ns=(st.st_atime_ns, st.st_mtime_ns))
    assert f.stat().st_size == st.st_size and f.stat().st_mtime_ns == st.st_mtime_ns
    assert bp.run_dir_content_digest(tmp_path) != d0


def test_the_manifest_platform_is_the_historical_run_platform_not_the_packaging_host(monkeypatch):
    import platform
    assert json.loads((PACKAGE / "RUN_MANIFEST.json").read_text(encoding="utf-8"))["runtime_python_platform"] == bp.RESULT01_RUN_PLATFORM
    monkeypatch.setattr(platform, "platform", lambda *a, **k: "SomeOtherHost-1.0")
    assert "platform" not in {a.name for n in ast.walk(_packager_tree()) if isinstance(n, ast.Import) for a in n.names}


# ============================================================================================ R6: stage, validate, publish
NAMES = ("RUN_MANIFEST.json", "bars_provenance_manifest.json", "campaign_disclosure.json", "conditional_edges.json",
         "factor_campaign_summary.json", "factor_evidence_ledger.jsonl", "factor_fdr_report.json", "strategy_campaign_summary.json",
         "strategy_edges.json", "strategy_neighborhood_report_only.json", "strategy_trial_ledger.jsonl", "universe_dispositions.json")


def _stub_run(tmp_path: Path) -> Path:
    run = tmp_path / "run"
    (run / "chunks").mkdir(parents=True)
    (run / "data").mkdir()
    for n in ("registry_strategy.sqlite", "registry_factor.sqlite"):
        (run / n).write_bytes(b"x")
    (run / "chunks" / "chunk_00000.jsonl").write_bytes(b"aaaa\n")
    (run / "data" / "status.json").write_bytes(b"{}")
    return run


def _snap(root: Path):
    return ct.tree_snapshot(root)


def _lstats(root: Path) -> dict:
    """Everything under `root` without following links: name -> (mode, size, inode, nlink, mtime_ns, link target)."""
    out = {}
    for p in sorted(root.rglob("*")):
        st = p.lstat()
        out[str(p.relative_to(root))] = (st.st_mode, st.st_size, st.st_ino, st.st_nlink, st.st_mtime_ns,
                                         str(p.readlink()) if p.is_symlink() else None)
    return out


def _fake_builder(run, pkg, regs):
    """Stands in ONLY for the number-producing builder: writes the declared 12 files with a self-consistent manifest."""
    for n in bp.PACKAGE_ARTIFACTS:
        (pkg / n).write_bytes(f"{n}\n".encode())
    (pkg / "RUN_MANIFEST.json").write_text(json.dumps({"result_run_head": bp.RESULT01_RUN_HEAD,
                                                       "preserved_artifact_sha256": bp.artifact_hashes(pkg)}), encoding="utf-8")


def _must_not_build(*a):
    pytest.fail("the builder must not run")


@pytest.fixture
def fake_builder(monkeypatch):
    monkeypatch.setattr(bp, "package", _fake_builder)


def _main(run, out):
    bp.main(["--run-dir", str(run), "--out", str(out), "--result-run-head", RESULT01_HEAD])


def _stages(parent: Path) -> list:
    return sorted(p.name for p in parent.iterdir() if p.name.startswith(".census02-stage-"))


# ---- D3 / registries / run directory preconditions
def test_main_refuses_a_wrong_head_before_any_io(tmp_path, provider_traps):
    with pytest.raises(bp.PackageRefusal, match="not the Result #1 execution head"):
        bp.main(["--run-dir", str(tmp_path / "nope"), "--out", str(tmp_path / "pkg"), "--result-run-head", "f" * 40])
    assert not (tmp_path / "pkg").exists()


def test_an_existing_manifest_with_a_different_or_malformed_head_is_refused_and_never_rewritten(tmp_path, monkeypatch, provider_traps):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    out = tmp_path / "pkg"
    out.mkdir()
    for content in (json.dumps({"result_run_head": "e" * 40}), "{not json", "[]"):
        (out / "RUN_MANIFEST.json").write_text(content, encoding="utf-8")
        with pytest.raises(bp.PackageRefusal, match="different result-run head|malformed"):
            _main(run, out)
        assert (out / "RUN_MANIFEST.json").read_text(encoding="utf-8") == content


def test_missing_run_dir_registries_and_sidecars_are_refused_without_creating_anything(tmp_path, monkeypatch, provider_traps):
    monkeypatch.setattr(bp, "package", _must_not_build)
    with pytest.raises(bp.PackageRefusal, match="does not exist or is not a directory"):
        _main(tmp_path / "nope", tmp_path / "pkg")
    run = tmp_path / "run"
    run.mkdir()
    with pytest.raises(bp.PackageRefusal, match="registry_strategy.sqlite is absent"):
        _main(run, tmp_path / "pkg")
    run = _stub_run(tmp_path / "second")
    (run / "registry_factor.sqlite-wal").write_bytes(b"w")
    with pytest.raises(bp.PackageRefusal, match="-wal sidecar"):
        _main(run, tmp_path / "pkg")
    assert not (tmp_path / "pkg").exists() and _stages(tmp_path) == [] and list((tmp_path / "run").iterdir()) == []


def test_main_refuses_and_publishes_nothing_if_the_run_directory_changes_during_packaging(tmp_path, monkeypatch, provider_traps):
    run = _stub_run(tmp_path)

    def build_and_tamper(run_dir, pkg, regs):
        _fake_builder(run_dir, pkg, regs)
        (run_dir / "chunks" / "chunk_00000.jsonl").write_bytes(b"aaab\n")      # same size
    monkeypatch.setattr(bp, "package", build_and_tamper)
    with pytest.raises(bp.PackageRefusal, match="changed during packaging"):
        _main(run, tmp_path / "pkg")
    assert not (tmp_path / "pkg").exists() and _stages(tmp_path) == []


# ---- R5: the output path itself
def test_output_equal_to_the_run_dir_is_refused_before_any_write(tmp_path, monkeypatch, provider_traps):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    before = _snap(run)
    with pytest.raises(bp.PackageRefusal, match="outside the settled run directory"):
        _main(run, run)
    assert _snap(run) == before and _stages(tmp_path) == []


def test_output_below_the_run_dir_is_refused_before_the_child_is_created(tmp_path, monkeypatch, provider_traps):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    before = _snap(run)
    for child in (run / "package", run / "chunks", run / "a" / "b" / "c"):
        with pytest.raises(bp.PackageRefusal, match="outside the settled run directory"):
            _main(run, child)
    assert _snap(run) == before and not (run / "package").exists() and not (run / "a").exists() and _stages(tmp_path) == []


def test_relative_dotdot_and_symlink_aliases_of_the_run_dir_are_refused(tmp_path, monkeypatch, provider_traps):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    (run / "inner").mkdir()
    before = _snap(run)
    alias, inner_alias = tmp_path / "alias", tmp_path / "inner_alias"
    alias.symlink_to(run, target_is_directory=True)
    inner_alias.symlink_to(run / "inner", target_is_directory=True)
    monkeypatch.chdir(tmp_path)
    cases = [(Path("run"), Path("run")), (Path("run"), Path("run/pkg")), (Path("run"), Path("./run/../run/pkg")),
             (Path("./run/"), Path("run/chunks/../pkg")), (Path("run/../run"), Path("run/x/..")),
             (run, alias), (run, alias / "pkg"), (alias, run / "pkg"), (alias, alias / "pkg"), (run, inner_alias),
             (run, inner_alias / "pkg")]
    for run_arg, out_arg in cases:
        with pytest.raises(bp.PackageRefusal, match="outside the settled run directory"):
            _main(run_arg, out_arg)
    assert _snap(run) == before and not (run / "pkg").exists() and not (run / "inner" / "pkg").exists() and _stages(tmp_path) == []


def test_filesystem_identity_catches_an_alias_that_string_resolution_does_not(tmp_path):
    """`_stat_overlap` compares device+inode of the path and its existing ancestors: the mechanism that also covers case-insensitive
    names, bind mounts and junctions (not creatable here; a symlink is used as the portable stand-in with resolution bypassed)."""
    run = _stub_run(tmp_path)
    alias = tmp_path / "alias"
    alias.symlink_to(run, target_is_directory=True)
    assert bp._stat_overlap(run, alias) and bp._stat_overlap(run, alias / "not" / "yet" / "created")
    assert not bp._stat_overlap(run, tmp_path / "elsewhere") and not bp._stat_overlap(run, tmp_path)


@pytest.mark.skipif(sys.platform != "win32", reason="Windows junction/reparse points cannot be created on this platform")
def test_a_windows_junction_to_the_run_dir_is_refused(tmp_path, monkeypatch, provider_traps):    # pragma: no cover - not exercised here
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    junction = tmp_path / "junction"
    subprocess.run(["cmd", "/c", "mklink", "/J", str(junction), str(run)], check=True, capture_output=True)
    with pytest.raises(bp.PackageRefusal, match="outside the settled run directory"):
        _main(run, junction / "pkg")


def test_ordinary_external_destinations_publish_exactly_the_declared_package(tmp_path, fake_builder, provider_traps):
    run = _stub_run(tmp_path)
    before = _snap(run)
    outs = [tmp_path / "pkg", tmp_path / "run_sibling" / "pkg", tmp_path / "runner", tmp_path / "deep" / "er" / "pkg",
            tmp_path / "a" / ".." / "rel_alias"]
    for out in outs:
        _main(run, out)
        target = out.resolve()
        assert sorted(p.name for p in target.iterdir()) == list(NAMES) and len(NAMES) == 12
        assert sorted(json.loads((target / "RUN_MANIFEST.json").read_text())["preserved_artifact_sha256"]) == [
            n for n in NAMES if n != "RUN_MANIFEST.json"]
    assert _snap(run) == before and _stages(tmp_path) == [] and _stages(tmp_path / "run_sibling") == []


def test_an_output_directory_that_contains_the_run_is_refused_as_ambiguous(tmp_path, monkeypatch, provider_traps):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    before = _snap(run)
    with pytest.raises(bp.PackageRefusal, match="undeclared entry 'run'"):
        _main(run, tmp_path)
    assert _snap(run) == before and _stages(tmp_path) == []


# ---- A: temporary roots are validated before anything is created
def _temp_locations(tmp_path: Path, run: Path) -> dict:
    link = tmp_path / "tmplink"
    link.symlink_to(run, target_is_directory=True)
    return {"equal": run, "nonexistent_below": run / "t" / "mp", "existing_below": run / "chunks",
            "dotdot": run / "chunks" / ".." / "x", "symlink": link, "symlink_below": link / "t"}


@pytest.mark.parametrize("key", ["TMPDIR", "TEMP", "TMP", "SQLITE_TMPDIR"])
@pytest.mark.parametrize("where", ["equal", "nonexistent_below", "existing_below", "dotdot", "symlink", "symlink_below"])
def test_a_temporary_root_inside_the_run_is_refused_before_any_directory_is_created(tmp_path, monkeypatch, provider_traps, key, where):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    before = _snap(run)
    monkeypatch.setattr(tempfile, "tempdir", None)                      # real tempfile semantics: the environment decides
    monkeypatch.setenv(key, str(_temp_locations(tmp_path, run)[where]))
    with pytest.raises(bp.PackageRefusal, match=f"temporary root {key}"):
        _main(run, tmp_path / "pkg")
    assert _snap(run) == before and not (run / "t").exists() and not (tmp_path / "pkg").exists() and _stages(tmp_path) == []


def test_a_relative_temporary_root_resolving_into_the_run_is_refused(tmp_path, monkeypatch, provider_traps):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    monkeypatch.chdir(tmp_path)
    monkeypatch.setenv("TMPDIR", "run/../run/chunks")
    with pytest.raises(bp.PackageRefusal, match="temporary root TMPDIR"):
        _main(run, tmp_path / "pkg")
    monkeypatch.delenv("TMPDIR")
    monkeypatch.chdir(run / "chunks")
    with pytest.raises(bp.PackageRefusal, match="working directory"):
        _main(run, tmp_path / "pkg")
    assert _stages(tmp_path) == [] and not (tmp_path / "pkg").exists()


def test_an_unavailable_working_directory_is_refused_not_a_traceback(tmp_path, monkeypatch, provider_traps):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)

    def gone():
        raise FileNotFoundError("cwd deleted")
    monkeypatch.setattr(bp.Path, "cwd", staticmethod(gone))
    with pytest.raises(bp.PackageRefusal, match="working directory is unavailable"):
        _main(run, tmp_path / "pkg")
    assert _stages(tmp_path) == [] and not (tmp_path / "pkg").exists()


def test_the_premise_a_default_tempfile_would_land_inside_the_run(tmp_path, monkeypatch):
    """Negative control for the guard: with TMPDIR=run the REAL tempfile machinery really does create inside the run."""
    run = _stub_run(tmp_path)
    monkeypatch.setattr(tempfile, "tempdir", None)
    monkeypatch.setenv("TMPDIR", str(run))
    made = Path(tempfile.mkdtemp())
    assert run in made.parents
    made.rmdir()


def test_staging_is_beside_the_output_and_never_uses_the_environment_temp_root(tmp_path, monkeypatch, provider_traps):
    run = _stub_run(tmp_path)
    ext_tmp = tmp_path / "ext_tmp"
    ext_tmp.mkdir()
    monkeypatch.setattr(tempfile, "tempdir", None)
    monkeypatch.setenv("TMPDIR", str(ext_tmp))
    seen = []

    def spy(run_dir, pkg, regs):
        seen.append((pkg.parent.parent, sorted(p.name for p in regs.values()), all(run not in p.parents for p in regs.values())))
        _fake_builder(run_dir, pkg, regs)
    monkeypatch.setattr(bp, "package", spy)
    out = tmp_path / "out" / "pkg"
    out.parent.mkdir()
    _main(run, out)
    assert seen == [(out.parent, sorted(bp.REGISTRIES), True)]
    assert list(ext_tmp.iterdir()) == [] and _stages(out.parent) == []


def test_a_staging_directory_that_resolves_inside_the_run_is_refused_before_any_registry_copy(tmp_path, monkeypatch, provider_traps):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    bad = run / "staging"

    def mkdtemp_inside_run(*a, **k):
        bad.mkdir()
        return str(bad)
    monkeypatch.setattr(tempfile, "mkdtemp", mkdtemp_inside_run)
    with pytest.raises(bp.PackageRefusal, match="staging directory"):
        _main(run, tmp_path / "pkg")
    assert list(bad.iterdir()) == [] and not (tmp_path / "pkg").exists()           # nothing was copied into it
    assert (run / "registry_strategy.sqlite").read_bytes() == b"x"


# ---- B: file-level aliases of protected evidence inside an otherwise external destination
def _alias_cases(tmp_path: Path, run: Path) -> dict:
    ext = tmp_path / "external_file.json"
    ext.write_bytes(b"external")
    (tmp_path / "ext_dir").mkdir()
    return {
        "symlink_to_chunk": lambda out: (out / "strategy_trial_ledger.jsonl").symlink_to(run / "chunks" / "chunk_00000.jsonl"),
        "symlink_to_registry": lambda out: (out / "campaign_disclosure.json").symlink_to(run / "registry_strategy.sqlite"),
        "symlink_to_other_run_file": lambda out: (out / "factor_fdr_report.json").symlink_to(run / "data" / "status.json"),
        "symlink_to_external_file": lambda out: (out / "strategy_edges.json").symlink_to(ext),
        "dangling_symlink": lambda out: (out / "strategy_edges.json").symlink_to(tmp_path / "does_not_exist"),
        "hardlink_to_chunk": lambda out: os.link(run / "chunks" / "chunk_00000.jsonl", out / "strategy_trial_ledger.jsonl"),
        "hardlink_to_registry": lambda out: os.link(run / "registry_factor.sqlite", out / "factor_evidence_ledger.jsonl"),
        "hardlink_to_external_file": lambda out: os.link(ext, out / "universe_dispositions.json"),
        "symlinked_directory": lambda out: (out / "bars_provenance_manifest.json").symlink_to(run / "chunks", target_is_directory=True),
        "directory_alias_in_place_of_file": lambda out: (out / "conditional_edges.json").mkdir(),
        "symlink_to_external_dir": lambda out: (out / "conditional_edges.json").symlink_to(tmp_path / "ext_dir", target_is_directory=True),
    }


ALIAS_CASES = ("symlink_to_chunk", "symlink_to_registry", "symlink_to_other_run_file", "symlink_to_external_file", "dangling_symlink",
               "hardlink_to_chunk", "hardlink_to_registry", "hardlink_to_external_file", "symlinked_directory",
               "directory_alias_in_place_of_file", "symlink_to_external_dir")


@pytest.mark.parametrize("case", ALIAS_CASES)
def test_an_existing_destination_entry_that_may_alias_protected_evidence_is_refused_before_any_write(tmp_path, monkeypatch, provider_traps, case):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    out = tmp_path / "pkg"
    out.mkdir()
    cases = _alias_cases(tmp_path, run)
    assert tuple(cases) == ALIAS_CASES
    cases[case](out)
    run_before, run_files = _snap(run), {p: p.read_bytes() for p in run.rglob("*") if p.is_file()}
    out_before = _lstats(out)
    with pytest.raises(bp.PackageRefusal, match="symlink|hard-linked|undeclared|not a regular"):
        _main(run, out)
    assert _snap(run) == run_before and {p: p.read_bytes() for p in run.rglob("*") if p.is_file()} == run_files
    assert _lstats(out) == out_before and _stages(tmp_path) == [] and (tmp_path / "external_file.json").read_bytes() == b"external"


def test_an_output_that_is_a_regular_file_is_refused(tmp_path, monkeypatch, provider_traps):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    (tmp_path / "pkg").write_bytes(b"i am a file")
    with pytest.raises(bp.PackageRefusal, match="not a directory"):
        _main(run, tmp_path / "pkg")
    assert (tmp_path / "pkg").read_bytes() == b"i am a file"


def test_publication_never_writes_through_an_existing_destination_alias(tmp_path, monkeypatch, fake_builder, provider_traps):
    """Faithful filesystem-level proof: a symlink/hardlink planted AFTER preflight (a race the preflight cannot see) is replaced, never
    written through - the protected chunk keeps its bytes and its inode."""
    run = _stub_run(tmp_path)
    protected = run / "chunks" / "chunk_00000.jsonl"
    ino = protected.stat().st_ino
    real_publish = bp.publish
    for kind in ("symlink", "hardlink"):
        out = tmp_path / f"pkg_{kind}"
        out.mkdir()

        def plant_then_publish(pkg, out_dir, existing, kind=kind):
            target = out_dir / "strategy_trial_ledger.jsonl"
            target.symlink_to(protected) if kind == "symlink" else os.link(protected, target)
            return real_publish(pkg, out_dir, existing)
        monkeypatch.setattr(bp, "publish", plant_then_publish)
        _main(run, out)
        assert protected.read_bytes() == b"aaaa\n" and protected.stat().st_ino == ino and protected.stat().st_nlink == 1
        written = out / "strategy_trial_ledger.jsonl"
        assert written.is_file() and not written.is_symlink() and written.stat().st_ino != ino and written.read_bytes() == b"strategy_trial_ledger.jsonl\n"


# ---- C: the closed inventory
@pytest.mark.parametrize("extra", ["extra.json", "extra.jsonl", ".hidden", "notes.txt", "subdir/"])
def test_unrelated_output_entries_are_refused_not_swept_into_the_package(tmp_path, monkeypatch, provider_traps, extra):
    monkeypatch.setattr(bp, "package", _must_not_build)
    run = _stub_run(tmp_path)
    out = tmp_path / "pkg"
    out.mkdir()
    if extra.endswith("/"):
        (out / extra.rstrip("/")).mkdir()
    else:
        (out / extra).write_bytes(b"{}")
    before = _lstats(out)
    with pytest.raises(bp.PackageRefusal, match="undeclared entry"):
        _main(run, out)
    assert _lstats(out) == before and _stages(tmp_path) == []


def test_the_manifest_inventory_is_exactly_the_declared_artifacts(tmp_path):
    pkg = tmp_path / "p"
    pkg.mkdir()
    for n in bp.PACKAGE_ARTIFACTS:
        (pkg / n).write_bytes(n.encode())
    (pkg / "extra.json").write_bytes(b"x")
    (pkg / "extra.jsonl").write_bytes(b"x")
    hashes = bp.artifact_hashes(pkg)
    assert sorted(hashes) == [n for n in NAMES if n != "RUN_MANIFEST.json"] and len(hashes) == 11 and len(bp.EXPECTED_PACKAGE_FILES) == 12
    assert tuple(sorted(NAMES)) == bp.EXPECTED_PACKAGE_FILES
    (pkg / "universe_dispositions.json").unlink()
    with pytest.raises(bp.PackageRefusal, match="missing before the manifest"):
        bp.artifact_hashes(pkg)


def test_verify_package_dir_refuses_missing_extra_and_mismatched_content(tmp_path, monkeypatch, fake_builder):
    pkg = tmp_path / "p"
    pkg.mkdir()
    _fake_builder(None, pkg, None)
    bp.verify_package_dir(pkg)
    (pkg / "extra.json").write_bytes(b"x")
    with pytest.raises(bp.PackageRefusal, match="inventory"):
        bp.verify_package_dir(pkg)
    (pkg / "extra.json").unlink()
    (pkg / "campaign_disclosure.json").write_bytes(b"changed")
    with pytest.raises(bp.PackageRefusal, match="manifest does not match"):
        bp.verify_package_dir(pkg)
    _fake_builder(None, pkg, None)
    m = json.loads((pkg / "RUN_MANIFEST.json").read_text())
    m["result_run_head"] = "d" * 40
    (pkg / "RUN_MANIFEST.json").write_text(json.dumps(m))
    with pytest.raises(bp.PackageRefusal, match="manifest does not match"):
        bp.verify_package_dir(pkg)
    (pkg / "RUN_MANIFEST.json").unlink()
    with pytest.raises(bp.PackageRefusal, match="inventory"):
        bp.verify_package_dir(pkg)


def test_an_existing_complete_identical_package_is_a_verified_no_op(tmp_path, fake_builder, provider_traps):
    run = _stub_run(tmp_path)
    out = tmp_path / "pkg"
    _main(run, out)
    before = _lstats(out)
    _main(run, out)
    assert _lstats(out) == before and _stages(tmp_path) == []


def test_an_existing_differing_file_is_never_replaced(tmp_path, fake_builder, provider_traps):
    run = _stub_run(tmp_path)
    out = tmp_path / "pkg"
    _main(run, out)
    (out / "campaign_disclosure.json").write_bytes(b"accepted earlier evidence")
    before = _lstats(out)
    with pytest.raises(bp.PackageRefusal, match="never replaced"):
        _main(run, out)
    assert _lstats(out) == before and (out / "campaign_disclosure.json").read_bytes() == b"accepted earlier evidence"


def test_a_partial_existing_package_is_completed_without_touching_present_files(tmp_path, fake_builder, provider_traps):
    run = _stub_run(tmp_path)
    out = tmp_path / "pkg"
    _main(run, out)
    for gone in ("universe_dispositions.json", "RUN_MANIFEST.json"):
        (out / gone).unlink()
    before = _lstats(out)
    _main(run, out)
    after = _lstats(out)
    assert sorted(p.name for p in out.iterdir()) == list(NAMES)
    assert all(after[n] == before[n] for n in before)                                  # untouched files keep inode and mtime


# ---- D: no partial publication
def test_a_builder_failure_leaves_no_output_no_stage_and_no_created_parents(tmp_path, monkeypatch, provider_traps):
    run = _stub_run(tmp_path)
    before = _snap(run)

    def partial_then_fail(run_dir, pkg, regs):
        (pkg / "campaign_disclosure.json").write_bytes(b"half")
        raise RuntimeError("injected")
    monkeypatch.setattr(bp, "package", partial_then_fail)
    out = tmp_path / "new" / "parents" / "pkg"
    with pytest.raises(RuntimeError, match="injected"):
        _main(run, out)
    assert not (tmp_path / "new").exists() and _stages(tmp_path) == [] and _snap(run) == before


def test_an_inconsistent_built_package_is_never_published(tmp_path, monkeypatch, provider_traps):
    run = _stub_run(tmp_path)

    def bad_manifest(run_dir, pkg, regs):
        _fake_builder(run_dir, pkg, regs)
        (pkg / "RUN_MANIFEST.json").write_text(json.dumps({"result_run_head": bp.RESULT01_RUN_HEAD,
                                                           "preserved_artifact_sha256": {"x": "y"}}))
    monkeypatch.setattr(bp, "package", bad_manifest)
    with pytest.raises(bp.PackageRefusal, match="manifest does not match"):
        _main(run, tmp_path / "pkg")
    assert not (tmp_path / "pkg").exists() and _stages(tmp_path) == []


def test_a_mid_publication_failure_removes_only_what_this_call_added(tmp_path, monkeypatch, fake_builder, provider_traps):
    run = _stub_run(tmp_path)
    out = tmp_path / "pkg"
    _main(run, out)
    for gone in ("conditional_edges.json", "strategy_edges.json", "RUN_MANIFEST.json"):
        (out / gone).unlink()
    before = _lstats(out)
    real_replace, calls = os.replace, []

    def flaky(src, dst):
        calls.append(dst)
        if len(calls) == 2:
            raise OSError("injected disk failure")
        return real_replace(src, dst)
    monkeypatch.setattr(bp.os, "replace", flaky)
    with pytest.raises(OSError, match="injected"):
        _main(run, out)
    assert _lstats(out) == before and _stages(tmp_path) == []                           # no half-added files, no .partial left


def test_a_fresh_publication_rename_failure_leaves_nothing(tmp_path, monkeypatch, fake_builder, provider_traps):
    run = _stub_run(tmp_path)

    def no_rename(src, dst):
        raise OSError("injected rename failure")
    monkeypatch.setattr(bp.os, "rename", no_rename)
    with pytest.raises(OSError, match="injected"):
        _main(run, tmp_path / "pkg")
    assert not (tmp_path / "pkg").exists() and _stages(tmp_path) == []


def test_a_post_publication_verification_failure_rolls_the_publication_back(tmp_path, monkeypatch, fake_builder, provider_traps):
    run = _stub_run(tmp_path)
    real, calls = bp.verify_package_dir, []

    def second_fails(pkg):
        calls.append(pkg)
        if len(calls) == 2:
            raise bp.PackageRefusal("injected post-publish failure")
        return real(pkg)
    monkeypatch.setattr(bp, "verify_package_dir", second_fails)
    with pytest.raises(bp.PackageRefusal, match="post-publish"):
        _main(run, tmp_path / "pkg")
    assert not (tmp_path / "pkg").exists() and _stages(tmp_path) == []
    out = tmp_path / "pkg2"
    _main(run, out)                                                                      # (the 3rd verify call passes) a clean package
    calls.clear()
    for gone in ("universe_dispositions.json", "RUN_MANIFEST.json"):
        (out / gone).unlink()
    before = _lstats(out)
    calls.append(None)                                                                   # next call is the 2nd => post-publish fails
    # existing-dir variant: the files THIS call added are removed again
    with pytest.raises(bp.PackageRefusal, match="post-publish"):
        _main(run, out)
    assert sorted(p.name for p in out.iterdir()) == sorted(n for n in NAMES if n not in ("universe_dispositions.json", "RUN_MANIFEST.json"))


@pytest.mark.skipif(os.geteuid() == 0 if hasattr(os, "geteuid") else True, reason="permission bits do not bind root / are not POSIX here")
def test_a_read_only_output_directory_fails_cleanly(tmp_path, fake_builder, provider_traps):   # pragma: no cover - needs non-root
    run = _stub_run(tmp_path)
    out = tmp_path / "pkg"
    out.mkdir()
    out.chmod(0o555)
    try:
        with pytest.raises(OSError):
            _main(run, out)
        assert list(out.iterdir()) == [] and _stages(tmp_path) == []
    finally:
        out.chmod(0o755)


# ---- real settled run: positive controls, injected failures and the dynamic call-graph proof
@needs_real_run
@pytest.mark.parametrize("name", ["load_settled_universe_offline", "strategy_ledger_bytes", "artifact_hashes"])
def test_real_run_failures_at_each_stage_publish_nothing_and_leave_the_run_untouched(tmp_path, monkeypatch, provider_traps, name):
    """offline data loading / Strategy-ledger validation / manifest construction, each failing inside the REAL builder."""
    before = ct.tree_snapshot(ct.REAL_RUN_DIR)
    monkeypatch.setattr(bp, name, lambda *a, **k: (_ for _ in ()).throw(RuntimeError(f"injected {name} failure")))
    out = tmp_path / "pkg"
    with pytest.raises(RuntimeError, match="injected"):
        bp.main(["--run-dir", str(ct.REAL_RUN_DIR), "--out", str(out), "--result-run-head", RESULT01_HEAD])
    assert not out.exists() and _stages(tmp_path) == [] and ct.tree_snapshot(ct.REAL_RUN_DIR) == before


@needs_real_run
def test_real_run_factor_record_loading_failure_publishes_nothing(tmp_path, monkeypatch, provider_traps):
    before = ct.tree_snapshot(ct.REAL_RUN_DIR)
    monkeypatch.setattr(bp.fe, "load_factor_records", lambda *a, **k: (_ for _ in ()).throw(RuntimeError("injected factor failure")))
    with pytest.raises(RuntimeError, match="injected factor"):
        bp.main(["--run-dir", str(ct.REAL_RUN_DIR), "--out", str(tmp_path / "pkg"), "--result-run-head", RESULT01_HEAD])
    assert not (tmp_path / "pkg").exists() and _stages(tmp_path) == [] and ct.tree_snapshot(ct.REAL_RUN_DIR) == before


@needs_real_run
def test_real_run_existing_committed_package_collision_is_a_verified_no_op_and_extras_are_refused(tmp_path, provider_traps):
    import shutil
    out = tmp_path / "pkg"
    shutil.copytree(PACKAGE, out)
    before = _lstats(out)
    bp.main(["--run-dir", str(ct.REAL_RUN_DIR), "--out", str(out), "--result-run-head", RESULT01_HEAD])
    assert _lstats(out) == before
    (out / "extra.json").write_bytes(b"{}")
    with pytest.raises(bp.PackageRefusal, match="undeclared entry"):
        bp.main(["--run-dir", str(ct.REAL_RUN_DIR), "--out", str(out), "--result-run-head", RESULT01_HEAD])


_WATCH = {"open", "sqlite3.connect", "os.mkdir", "os.rename", "os.remove", "os.rmdir", "os.truncate", "os.chmod", "os.utime", "os.symlink",
          "os.link", "shutil.copyfile", "shutil.rmtree", "socket.connect", "socket.getaddrinfo", "socket.gethostbyname", "subprocess.Popen",
          "os.putenv", "os.unsetenv"}
_AUDIT = {"on": False, "events": []}
_AUDIT_INSTALLED = []


def _audit_hook(event, args):
    if _AUDIT["on"] and event in _WATCH:
        _AUDIT["events"].append((event, args))


def _as_path(x):
    if isinstance(x, int):
        return None
    try:
        return Path(os.fsdecode(os.fspath(x))).resolve()
    except (TypeError, ValueError):
        return None


@needs_real_run
def test_dynamic_call_graph_the_real_packaging_never_writes_to_the_run_touches_its_registries_or_leaves_the_machine(tmp_path):
    """Process-wide audit-hook proof over the REAL production path (loader -> classify_eligibility -> load_symbol_bars -> provenance
    validators, build_bars_manifest, load_bars, factor-record loading, every ResearchResultStore, ledger loading)."""
    if not _AUDIT_INSTALLED:
        sys.addaudithook(_audit_hook)
        _AUDIT_INSTALLED.append(True)
    run = ct.REAL_RUN_DIR.resolve()
    before = ct.tree_snapshot(ct.REAL_RUN_DIR)
    _AUDIT["events"].clear()
    _AUDIT["on"] = True
    try:
        bp.main(["--run-dir", str(run), "--out", str(tmp_path / "pkg"), "--result-run-head", RESULT01_HEAD])
    finally:
        _AUDIT["on"] = False
    events = list(_AUDIT["events"])
    under = lambda p: p is not None and (p == run or run in p.parents)          # noqa: E731
    write_flags = os.O_WRONLY | os.O_RDWR | os.O_CREAT | os.O_TRUNC | os.O_APPEND
    run_writes, sqlite_paths, reads_under_run, net, procs, env_files = [], [], 0, [], [], []
    for event, args in events:
        if event == "open":
            p, mode, flags = _as_path(args[0]), args[1], args[2]
            writing = (isinstance(mode, str) and any(c in mode for c in "wax+")) or (isinstance(flags, int) and flags & write_flags)
            if under(p) and writing:
                run_writes.append((event, str(p)))
            reads_under_run += bool(under(p) and not writing)
            if p is not None and any(part.startswith(".env") for part in p.parts):
                env_files.append(str(p))
        elif event == "sqlite3.connect":
            sqlite_paths.append(_as_path(args[0]))
        elif event in ("socket.connect", "socket.getaddrinfo", "socket.gethostbyname"):
            net.append(event)
        elif event == "subprocess.Popen":
            procs.append(os.path.basename(str(args[0])))
        elif event in ("shutil.copyfile",):
            if under(_as_path(args[1])):
                run_writes.append((event, str(args[1])))
        elif any(under(_as_path(a)) for a in args[:2] if isinstance(a, (str, bytes, os.PathLike))):
            run_writes.append((event, str(args[:2])))
    assert run_writes == [], run_writes[:5]
    assert not any(under(p) for p in sqlite_paths) and len(sqlite_paths) >= 3                  # positive signal: private copies were opened
    assert reads_under_run > 1000                                                               # positive signal: the run was really read
    assert net == [] and env_files == [] and set(procs) <= {"git"}
    assert ct.tree_snapshot(ct.REAL_RUN_DIR) == before


@needs_real_run
def test_real_settled_run_packages_byte_identically_and_publishes_exactly_the_declared_files(tmp_path, provider_traps):
    out = tmp_path / "pkg"
    before = ct.tree_snapshot(ct.REAL_RUN_DIR)
    bp.main(["--run-dir", str(ct.REAL_RUN_DIR), "--out", str(out), "--result-run-head", RESULT01_HEAD])
    assert ct.tree_snapshot(ct.REAL_RUN_DIR) == before
    assert sorted(p.name for p in out.iterdir()) == list(NAMES) and len(NAMES) == 12 and _stages(tmp_path) == []
    for name in NAMES:
        assert (out / name).read_bytes() == (PACKAGE / name).read_bytes(), name
