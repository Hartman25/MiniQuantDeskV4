"""Census-02 Result #1 packager invariants: the preserved Strategy / factor ledgers are exact copies of the settled evidence, the
packager refuses any count / duplicate / missing / extra / order / status-mix deviation, is strictly offline (no acquisition, no
credential, no network) and keeps the immutable Result #1 execution head. Synthetic fixtures; the real-run tests need the local run
directory and are skipped (with an explicit reason) only where it does not exist, e.g. a fresh CI checkout."""

from __future__ import annotations

import ast
import json
import socket
import subprocess
import sys
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
                   "register_factor_population", "begin_attempts_bulk"}
NETWORK_MODULES = {"requests", "urllib", "http", "socket", "aiohttp", "httpx", "ssl"}


def test_the_packager_source_cannot_reach_acquisition_credentials_network_or_recomputation():
    tree = ast.parse((ct.EXP2 / "results" / "build_result_package.py").read_text(encoding="utf-8"))
    for n in ast.walk(tree):
        mods = [a.name for a in n.names] if isinstance(n, ast.Import) else [n.module or ""] if isinstance(n, ast.ImportFrom) else []
        for m in mods:
            assert not set(m.split(".")) & NETWORK_MODULES, f"imports network module {m}"
        if isinstance(n, ast.Call):
            name = n.func.attr if isinstance(n.func, ast.Attribute) else getattr(n.func, "id", "")
            assert name not in FORBIDDEN_CALLS, f"calls {name}"
        if isinstance(n, ast.Attribute):
            assert n.attr not in ("environ", "getenv"), "reads the environment"
    called = {n.func.attr if isinstance(n.func, ast.Attribute) else getattr(n.func, "id", "")
              for n in ast.walk(tree) if isinstance(n, ast.Call)}
    assert "load_settled_universe_offline" in called


@needs_real_run
def test_real_settled_run_packages_offline_byte_identically_and_leaves_the_run_dir_untouched(tmp_path, provider_traps):
    """End to end on the real settled run with every acquisition / network entrance booby-trapped: the package is byte-identical to
    the committed one (manifest, ledgers, FDR report, summaries) and main() itself proves the run directory unchanged."""
    out = tmp_path / "pkg"
    before = ct.tree_snapshot(ct.REAL_RUN_DIR)
    bp.main(["--run-dir", str(ct.REAL_RUN_DIR), "--out", str(out), "--result-run-head", RESULT01_HEAD])
    assert ct.tree_snapshot(ct.REAL_RUN_DIR) == before
    committed = sorted(p.name for p in PACKAGE.iterdir())
    assert sorted(p.name for p in out.iterdir()) == committed and len(committed) == 12
    for name in committed:
        assert (out / name).read_bytes() == (PACKAGE / name).read_bytes(), name


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


def test_main_refuses_a_wrong_head_and_a_manifest_that_disagrees_before_any_io(tmp_path, provider_traps):
    out = tmp_path / "pkg"
    out.mkdir()
    with pytest.raises(bp.PackageRefusal, match="not the Result #1 execution head"):
        bp.main(["--run-dir", str(tmp_path / "nope"), "--out", str(out), "--result-run-head", "f" * 40])
    (out / "RUN_MANIFEST.json").write_text(json.dumps({"result_run_head": "e" * 40}), encoding="utf-8")
    with pytest.raises(bp.PackageRefusal, match="different result-run head"):
        bp.main(["--run-dir", str(tmp_path / "nope"), "--out", str(out), "--result-run-head", RESULT01_HEAD])
    assert json.loads((out / "RUN_MANIFEST.json").read_text())["result_run_head"] == "e" * 40      # never rewritten


def test_main_never_creates_a_missing_registry(tmp_path, provider_traps):
    run = tmp_path / "run"
    run.mkdir()
    with pytest.raises(bp.PackageRefusal, match="registry_strategy.sqlite is absent"):
        bp.main(["--run-dir", str(run), "--out", str(tmp_path / "pkg"), "--result-run-head", RESULT01_HEAD])
    assert list(run.iterdir()) == []


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


def test_main_refuses_if_the_run_directory_changes_during_packaging(tmp_path, monkeypatch, provider_traps):
    run = tmp_path / "run"
    run.mkdir()
    for n in ("registry_strategy.sqlite", "registry_factor.sqlite"):
        (run / n).write_bytes(b"x")
    (run / "chunks").mkdir()
    (run / "chunks" / "chunk_00000.jsonl").write_bytes(b"aaaa\n")

    def tamper(run_dir, out, regs):
        (run_dir / "chunks" / "chunk_00000.jsonl").write_bytes(b"aaab\n")     # same size
    monkeypatch.setattr(bp, "package", tamper)
    with pytest.raises(bp.PackageRefusal, match="changed during packaging"):
        bp.main(["--run-dir", str(run), "--out", str(tmp_path / "pkg"), "--result-run-head", RESULT01_HEAD])
    monkeypatch.setattr(bp, "package", lambda *a: None)                         # an untouched run dir passes
    bp.main(["--run-dir", str(run), "--out", str(tmp_path / "pkg"), "--result-run-head", RESULT01_HEAD])


# ------------------------------------------------------- R5: the output path must be outside the settled run directory
def _stub_run(tmp_path: Path) -> Path:
    """A synthetic 'settled run' that satisfies every precondition except the output-path one."""
    run = tmp_path / "run"
    (run / "chunks").mkdir(parents=True)
    for n in ("registry_strategy.sqlite", "registry_factor.sqlite"):
        (run / n).write_bytes(b"x")
    (run / "chunks" / "chunk_00000.jsonl").write_bytes(b"aaaa\n")
    return run


def _snap(root: Path):
    return ct.tree_snapshot(root)


def _main(run, out):
    bp.main(["--run-dir", str(run), "--out", str(out), "--result-run-head", RESULT01_HEAD])


def test_output_equal_to_the_run_dir_is_refused_before_any_write(tmp_path, provider_traps):
    run = _stub_run(tmp_path)
    before = _snap(run)
    with pytest.raises(bp.PackageRefusal, match="outside the settled run directory"):
        _main(run, run)
    assert _snap(run) == before


def test_output_below_the_run_dir_is_refused_before_the_child_is_created(tmp_path, provider_traps):
    run = _stub_run(tmp_path)
    before = _snap(run)
    for child in (run / "package", run / "chunks", run / "a" / "b" / "c"):
        with pytest.raises(bp.PackageRefusal, match="outside the settled run directory"):
            _main(run, child)
    assert _snap(run) == before and not (run / "package").exists() and not (run / "a").exists()


def test_relative_and_dotdot_aliases_of_the_run_dir_are_refused(tmp_path, monkeypatch, provider_traps):
    run = _stub_run(tmp_path)
    before = _snap(run)
    monkeypatch.chdir(tmp_path)
    for run_arg, out_arg in (("run", "run"), ("run", "run/pkg"), ("run", "./run/../run/pkg"), ("./run/", "run/chunks/../pkg"),
                             ("run/../run", "run/x/..")):
        with pytest.raises(bp.PackageRefusal, match="outside the settled run directory"):
            _main(Path(run_arg), Path(out_arg))
    assert _snap(run) == before and not (run / "pkg").exists()


def test_symlink_aliases_of_the_run_dir_are_refused(tmp_path, provider_traps):
    run = _stub_run(tmp_path)
    before = _snap(run)
    alias = tmp_path / "alias"
    alias.symlink_to(run, target_is_directory=True)
    (run / "inner").mkdir()
    inner_alias = tmp_path / "inner_alias"
    inner_alias.symlink_to(run / "inner", target_is_directory=True)
    for run_arg, out_arg in ((run, alias), (run, alias / "pkg"), (alias, run / "pkg"), (alias, alias / "pkg"),
                             (run, inner_alias), (run, inner_alias / "pkg")):
        with pytest.raises(bp.PackageRefusal, match="outside the settled run directory"):
            _main(run_arg, out_arg)
    assert _snap(run) == before and not (run / "pkg").exists() and not (run / "inner" / "pkg").exists()


def test_an_ordinary_external_output_path_is_accepted_by_the_preflight(tmp_path, monkeypatch, provider_traps):
    run = _stub_run(tmp_path)
    before = _snap(run)
    seen = []
    monkeypatch.setattr(bp, "package", lambda r, o, regs: seen.append(o))
    for out in (tmp_path / "pkg", tmp_path / "run_sibling" / "pkg", tmp_path, tmp_path / "runner"):   # incl. the run's parent / a name prefix
        _main(run, out)
    assert [p.resolve() for p in seen] == [(tmp_path / "pkg").resolve(), (tmp_path / "run_sibling" / "pkg").resolve(),
                                           tmp_path.resolve(), (tmp_path / "runner").resolve()]
    assert _snap(run) == before


def test_a_temporary_registry_location_inside_the_run_dir_is_refused(tmp_path, monkeypatch, provider_traps):
    run = _stub_run(tmp_path)
    before = _snap(run)

    class InsideRun:                                   # a TMPDIR that resolves beneath the settled run
        def __init__(self, *a, **k): ...
        def __enter__(self): return str(run / "tmpregs")
        def __exit__(self, *a): return False
    monkeypatch.setattr(bp.tempfile, "TemporaryDirectory", InsideRun)
    monkeypatch.setattr(bp, "package", lambda *a: pytest.fail("packaging must not start"))
    with pytest.raises(bp.PackageRefusal, match="temporary registry copy location"):
        _main(run, tmp_path / "pkg")
    assert _snap(run) == before and not (run / "tmpregs").exists()
