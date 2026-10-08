"""Census-02 protocol scaffold: borrow capability, data fences, denominator guards, operator-decision schema and the
freeze guard. Synthetic fixtures only; no real Census-02 result is read or produced."""

from __future__ import annotations

import ast
import copy
import json
import shutil
import subprocess
import sys
from pathlib import Path

import pandas as pd
import pytest

EXP2 = Path(__file__).resolve().parents[1] / "experiments" / "alpha_edge_census_02"
sys.path.insert(0, str(EXP2))

import c2_borrow as bw  # noqa: E402
import c2_environment as env  # noqa: E402
import c2_policy as pol  # noqa: E402
import c2_protocol as pr  # noqa: E402
import simulate as sm1  # noqa: E402



# ============================================================================================ borrow capability
ASSUME = {"etf_short_scope": ["IWM", "SPY"], "annual_borrow_fee_bps": 50, "availability": bw.ASSUMPTION_FIXED["availability"],
          "recall": "NONE_ASSUMED", "short_rebate": "ZERO"}




@pytest.mark.parametrize("mut", [
    lambda a: a.pop("annual_borrow_fee_bps"), lambda a: a.update(annual_borrow_fee_bps=None),
    lambda a: a.update(annual_borrow_fee_bps=-1), lambda a: a.update(annual_borrow_fee_bps=float("inf")),
    lambda a: a.update(annual_borrow_fee_bps=True), lambda a: a.update(etf_short_scope=[]),
    lambda a: a.update(etf_short_scope=["SPY", "IWM"]), lambda a: a.update(etf_short_scope=["SPY", "SPY"]),
    lambda a: a.update(recall="MODELED"), lambda a: a.update(short_rebate="0.01"),
    lambda a: a.update(availability="EASY_TO_BORROW"), lambda a: a.update(extra=1)])
def test_etf_assumption_is_fail_closed(mut):
    a = copy.deepcopy(ASSUME)
    mut(a)
    with pytest.raises(bw.BorrowRefusal):
        bw.classify_evidence("SPY", a)



@pytest.mark.parametrize("scope", [["ZZZZ"], ["SPY", "NOTASYMBOL"], ["SPYX"], ["spy"], ["SPY "], ["BTC"], ["SPY", "ZZZ"]])
def test_etf_scope_outside_the_frozen_seed_universe_fails_closed_without_ticker_inference(scope):
    a = {**copy.deepcopy(ASSUME), "etf_short_scope": sorted(set(scope))}
    with pytest.raises(bw.BorrowRefusal):
        bw.validate_etf_assumption(a)
    with pytest.raises(bw.BorrowRefusal):
        bw.classify_evidence("SPY", a)
    # the explicit list is the authority: an in-universe symbol is accepted whatever its ticker text looks like
    assert bw.validate_etf_assumption({**copy.deepcopy(ASSUME), "etf_short_scope": ["AAPL"]})["etf_short_scope"] == ["AAPL"]
    assert "SPY" in bw.seed_universe_symbols() and len(bw.seed_universe_symbols()) == 88


def test_etf_scope_validation_fails_closed_when_the_seed_universe_is_unavailable(monkeypatch, tmp_path):
    monkeypatch.setattr(bw, "SEED_UNIVERSE_FILE", tmp_path / "missing.json")
    with pytest.raises(bw.BorrowRefusal):
        bw.validate_etf_assumption(copy.deepcopy(ASSUME))
    (tmp_path / "bad.json").write_text("{}")
    monkeypatch.setattr(bw, "SEED_UNIVERSE_FILE", tmp_path / "bad.json")
    with pytest.raises(bw.BorrowRefusal):
        bw.validate_etf_assumption(copy.deepcopy(ASSUME))


# ============================================================================================ cost protocol (D1)
def test_protocol_cost_truth_equals_the_accepted_constants_and_the_executed_economics():
    ex = pr.build_structural_protocol()["execution_contract"]
    accepted = {"commission_bps_per_side": 10.0, "fill_slippage_bps_per_side": 5, "volatility_mult_bps": 0,
                "additional_slippage_bps": 0.0, "pricing_model_id": "rust_conservative_bar_range_v1",
                "annualization_days": 252, "entry_budget_usd": 10000.0, "initial_capital_usd": 100000.0}
    assert ex["cost_model"] == accepted                               # literals: drift in either direction fails
    assert (sm1.COMMISSION_BPS, sm1.SLIPPAGE_BPS, sm1.VOL_MULT_BPS, sm1.ANNUALIZATION, sm1.BUDGET_USD, sm1.CAPITAL_USD) == (
        10.0, 5, 0, 252, 10000.0, 100000.0)
    assert "slippage 5 bps" in ex["fill"] and "floor(10000 USD" in ex["sizing"] and "/252" in ex["borrow_cost"]
    assert "strictly after" in ex["fill"] and "same-bar fill forbidden" in ex["fill"]


def test_executed_fill_economics_equal_the_protocol_slippage_and_commission():
    import numpy as np
    import c2_simulate as sim2
    hm, lm, cm = (np.array([int(round(v * 1e6)) for v in x], np.int64) for x in (
        [10.3, 10.4, 10.3, 10.0, 9.9, 9.6, 9.9], [9.8, 9.9, 9.7, 9.4, 9.2, 8.9, 9.3], [10.0, 10.2, 10.0, 9.8, 9.5, 9.2, 9.6]))
    d = np.zeros(7, np.int8)
    d[2:4] = -1
    so = sim2.simulate_signed(hm, lm, cm, d, 2, borrow_fee_bps_annual=0.0)
    cost = pr.build_structural_protocol()["execution_contract"]["cost_model"]
    qty = int(cost["entry_budget_usd"] * 1e6) // int(cm[2])
    slip = cost["fill_slippage_bps_per_side"]
    sell = int(lm[3]) - int(lm[3]) * slip // 10_000                                  # short entry = SELL on the next bar
    expected = (qty * (int(cm[3]) - sell) + qty * sell * cost["commission_bps_per_side"] / 10_000) / 1e6
    assert so.cost[3] == pytest.approx(expected, abs=1e-9)
    zero = sim2.simulate_signed(hm, lm, cm, d, 2, borrow_fee_bps_annual=0.0, commission_bps=0.0, slippage_bps=0)
    assert zero.cost[3] == pytest.approx(qty * (int(cm[3]) - int(lm[3])) / 1e6, abs=1e-9)     # overrides: no slippage, no commission


# ============================================================================================ fences
def _bars_at(dates):
    return pd.DataFrame({"end_ts": [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in dates],
                         "open": 1.0, "high": 1.1, "low": 0.9, "close": 1.0, "volume": 1.0, "symbol": "T"})


@pytest.mark.parametrize("day", ["2024-01-02", "2024-06-03", "2025-01-02", "2025-12-01", "2026-03-02", "2026-09-01"])
def test_confirmation_contaminated_and_holdout_rows_are_fenced(day):
    with pytest.raises(pr.pt.PartitionBreach):
        pr.fence_bars(_bars_at(["2023-12-28", day]), what="t")


def test_fence_accepts_discovery_and_refuses_empty():
    assert len(pr.fence_bars(_bars_at(["2023-12-27", "2023-12-28"]), what="t")) == 2
    with pytest.raises(pr.pt.PartitionBreach):
        pr.fence_bars(_bars_at([]), what="t")


# ============================================================================================ denominators
def test_denominator_cannot_shrink_and_winner_only_registration_is_refused():
    pop = [f"ac02-{i:03d}" for i in range(10)]
    pr.assert_complete_ledger(pop, list(reversed(pop)))
    for bad in (pop[:-1], [pop[0]], pop + ["ac02-zzz"], pop + [pop[0]]):
        with pytest.raises(pr.DenominatorShrink):
            pr.assert_complete_ledger(pop, bad)
    winners = [{"trial_id": pop[0]}, {"trial_id": pop[3]}]
    with pytest.raises(pr.DenominatorShrink):
        pr.register_edges(winners, pop, [pop[0], pop[3]])                      # winners only
    assert pr.register_edges(winners, pop, pop) == winners
    with pytest.raises(pr.DenominatorShrink):
        pr.register_edges([{"trial_id": "ac02-stray"}], pop, pop)


# ============================================================================================ freeze guard
GOOD_DECISIONS = pol.approved_decisions()


def _git(repo, *a):
    subprocess.run(["git", *a], cwd=repo, check=True, capture_output=True)


def _frozen_doc(repo, **over):
    """The production builder over the throwaway repo's committed sources (so every refusal below is a real-path refusal)."""
    doc = pr.build_predeclaration(repo)
    doc.update(over)
    return doc


@pytest.fixture
def repo(tmp_path):
    """A throwaway git repo holding byte copies of every behavior-bearing source at its repo-relative path."""
    for rel in (*pr.BEHAVIOR_SOURCES, *pr.AUTHORITY_DATA):
        (tmp_path / rel).parent.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(pr.REPO / rel, tmp_path / rel)
    _git(tmp_path, "init", "-q")
    _git(tmp_path, "config", "user.email", "t@t")
    _git(tmp_path, "config", "user.name", "t")
    _git(tmp_path, "add", "-A")
    _git(tmp_path, "commit", "-qm", "seed")
    return tmp_path


def _write_commit(repo, doc, commit=True):
    f = repo / "CENSUS02_PREDECLARATION.json"
    f.write_text(json.dumps(doc, sort_keys=True), encoding="utf-8")
    if commit:
        _git(repo, "add", "-A")
        _git(repo, "commit", "-qm", "freeze")
    return f


def _forged(good, change):
    """A document whose decisions differ from the approved policy but whose protocol id AND populations are recomputed for
    them: every neighbouring check is satisfied, so only the approved-policy equality check can refuse it."""
    import c2_population
    d = {**good["decisions"], **change}
    pid = pr.frozen_protocol_id(good["structural_protocol"], d, good["behavior_source_manifest"], good["environment_identity"])
    return {**good, "decisions": d, "protocol_id": pid, **c2_population.population_authority(d, pid)}


def test_freeze_guard_positive_control_then_every_refusal(repo):
    f = _write_commit(repo, _frozen_doc(repo))
    got = pr.require_freeze(repo, f)
    assert got["predeclaration"]["status"] == pr.STATUS_FROZEN and got["predeclaration"]["attempts_at_freeze"] == 0   # positive control
    assert got["predeclaration"]["behavior_head"] != got["head"]                         # freeze commit is AFTER the behavior commit

    for fn in (lambda: pr.require_freeze(repo, repo / "nope.json"),
               lambda: pr.require_freeze(pr.REPO, pr.HERE / "DOES_NOT_EXIST.json")):
        with pytest.raises(pr.FreezeRefusal):
            fn()
    good = _frozen_doc(repo)
    bad_env = {**good["environment_identity"], "numpy": "0.0.0"}
    variants = {
        "proposed": _frozen_doc(repo, status=pr.STATUS_PROPOSED), "attempts": _frozen_doc(repo, attempts_at_freeze=1),
        "bad_id": _frozen_doc(repo, protocol_id="0" * 32),
        "no_manifest": {k: v for k, v in good.items() if k != "behavior_source_manifest"},
        "changed_structure": {**good, "structural_protocol": {"x": 1}},
        "env_mismatch_even_with_a_self_consistent_id": {**good, "environment_identity": bad_env, "protocol_id": pr.frozen_protocol_id(
            good["structural_protocol"], good["decisions"], good["behavior_source_manifest"], bad_env)},
        "env_missing": {k: v for k, v in good.items() if k != "environment_identity"},
        "decisions_not_the_approved_policy": {**good, "decisions": {**good["decisions"], "ssr_handling": "FLAG_ONLY",
                                                                   "multiple_testing_denominator": "LOCAL_CENSUS02"}},
        # self-consistent forgeries: ONLY the intended check can refuse these (every neighbouring check is satisfied)
        "decisions_changed_with_a_self_consistent_protocol_id": _forged(good, {"multiple_testing_denominator": "LOCAL_CENSUS02"}),
        "wrong_protocol_id_with_self_consistent_populations": {
            **good, "protocol_id": "f" * 32,
            **__import__("c2_population").population_authority(good["decisions"], "f" * 32)},
        "behavior_head_missing": {k: v for k, v in good.items() if k != "behavior_head"},
        "behavior_head_not_an_ancestor": {**good, "behavior_head": "0" * 40},
        "population_tampered": {**good, "strategy_population": {**good["strategy_population"], "trial_count": 9399}},
        "factor_population_tampered": {**good, "factor_population": {**good["factor_population"], "factor_count": 1074}},
        "request_contract_tampered": {**good, "data_request_contract": {**good["data_request_contract"], "feed": "iex"}},
        "disclosure_tampered": {**good, "global_disclosure": {**good["global_disclosure"], "census01_strategy_trials": 0}},
    }
    for name, doc in variants.items():
        f = _write_commit(repo, doc)
        with pytest.raises(pr.FreezeRefusal):
            pr.require_freeze(repo, f)
    f = _write_commit(repo, _frozen_doc(repo), commit=False)
    f.write_text(f.read_text() + " ")                                                        # dirty vs HEAD
    with pytest.raises(pr.FreezeRefusal):
        pr.require_freeze(repo, f)
    f.write_text(json.dumps(_frozen_doc(repo), sort_keys=True))
    _git(repo, "rm", "-q", "--cached", f.name)                                               # present but untracked
    with pytest.raises(pr.FreezeRefusal):
        pr.require_freeze(repo, f)


def test_freeze_document_is_result_free_and_malformed_documents_are_typed_refusals(repo):
    doc = _frozen_doc(repo)
    bad = {"net_pnl_usd", "net_alpha_usd", "p_value", "q_values", "mean_ic", "event_count", "direction_adjusted_effect", "trade_count"}

    def keys(o, acc):
        if isinstance(o, dict):
            for k, v in o.items():
                if k in bad and isinstance(v, (int, float, list, dict)) and not isinstance(v, bool):
                    acc.add(k)
                keys(v, acc)
        elif isinstance(o, list):
            for v in o:
                keys(v, acc)
        return acc
    assert not keys(doc, set()) and doc["attempts_at_freeze"] == 0 and doc["results_present_at_freeze"] is False
    f = repo / "CENSUS02_PREDECLARATION.json"
    for garbage in ("not json", "[]", "null", '"s"'):
        f.write_text(garbage)
        _git(repo, "add", "-A")
        _git(repo, "commit", "-qm", "garbage", "--allow-empty")
        with pytest.raises(pr.FreezeRefusal):
            pr.require_freeze(repo, f)


def test_numerical_environment_mismatch_is_refused_before_anything_else(repo, monkeypatch):
    f = _write_commit(repo, _frozen_doc(repo))
    assert pr.require_freeze(repo, f)
    now = env.environment_identity()
    assert sorted(now) == ["numpy", "pandas", "python"] and all(now.values())
    for key in ("python", "numpy", "pandas"):
        monkeypatch.setattr(env, "environment_identity", lambda k=key: {**now, k: now[k] + ".1"})
        with pytest.raises(pr.FreezeRefusal, match="environment"):
            pr.require_freeze(repo, f)
    monkeypatch.undo()
    for frozen in (None, {}, {**now, "extra": "1"}, {"python": now["python"], "numpy": now["numpy"]}, {**now, "pandas": ""}):
        with pytest.raises(env.EnvironmentMismatch):
            env.require_environment(frozen)
    # the identity is part of the protocol id: a different runtime cannot reproduce the same campaign identity
    d = _frozen_doc(repo)
    assert pr.frozen_protocol_id(d["structural_protocol"], d["decisions"], d["behavior_source_manifest"], now) != pr.frozen_protocol_id(
        d["structural_protocol"], d["decisions"], d["behavior_source_manifest"], {**now, "numpy": "0.0.0"})


def test_behavior_head_chronology_is_enforced_independently_of_the_manifest(repo):
    """An attacker who edits a bound source AND regenerates the manifest after the behavior head is still refused."""
    f = _write_commit(repo, _frozen_doc(repo))
    assert pr.require_freeze(repo, f)
    target = repo / pr.BEHAVIOR_SOURCES[0]
    target.write_bytes(target.read_bytes() + b"\n# changed after the behavior head\n")
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "behavior change after the behavior head")
    doc = json.loads(f.read_text())
    doc["behavior_source_manifest"] = pr.behavior_source_manifest(repo)                    # manifest now self-consistent
    doc["protocol_id"] = pr.frozen_protocol_id(doc["structural_protocol"], doc["decisions"], doc["behavior_source_manifest"],
                                               doc["environment_identity"])
    f = _write_commit(repo, doc)
    with pytest.raises(pr.FreezeRefusal, match="between the frozen behavior head and HEAD"):
        pr.require_freeze(repo, f)


@pytest.mark.parametrize("mut", [
    lambda d: d.pop("benchmark_rule"), lambda d: d.update(benchmark_rule="LONG_HOLD"), lambda d: d.update(extra=1),
    lambda d: d.update(benchmark_rule="NET_POSITIVE_AND_SAME_DIRECTION_HOLD_ALPHA_POSITIVE"),       # undefined for long/short
    lambda d: d.update(benchmark_rule="SAME_DIRECTION_HOLD_ALPHA_ONLY"),
    lambda d: d.update(conditional_scope="EQUITY_SYMBOLS_ONLY"),            # no complete instrument-class authority exists
    lambda d: d.update(etf_borrow_assumption=None),
    lambda d: d.update(borrow_policy="NO_EXECUTABLE_SHORTS_HYPOTHESIS_ONLY"),         # assumption present but disallowed
    lambda d: d.update(funnel_thresholds={"year_stability": "x"}),
    lambda d: d.update(etf_borrow_assumption={**ASSUME, "annual_borrow_fee_bps": None})])
def test_operator_decisions_must_be_complete_and_consistent(mut):
    d = copy.deepcopy(GOOD_DECISIONS)
    mut(d)
    with pytest.raises(pr.FreezeRefusal):
        pr.validate_decisions(d)
    assert pr.validate_decisions(copy.deepcopy(GOOD_DECISIONS))
    hypo = {**copy.deepcopy(GOOD_DECISIONS), "borrow_policy": "NO_EXECUTABLE_SHORTS_HYPOTHESIS_ONLY", "etf_borrow_assumption": None}
    assert pr.validate_decisions(hypo)


ACQUISITION_MODULES = {"data", "census", "alpaca_historical", "bars_provenance"}
NETWORK_MODULES = {"requests", "urllib", "http", "socket", "aiohttp", "httpx"}
ACQUISITION_CALLS = {"load_symbol_bars", "acquire_symbol", "acquire_universe", "classify_eligibility", "build_bars_manifest",
                     "load_bars", "extract_research_bars_with_provenance", "read_csv", "read_parquet", "urlopen",
                     "load_discovery_universe", "load_alpaca_env"}


def _census02_sources():
    return [f for f in EXP2.glob("*.py") if f.name not in ("c2_mutation_proof.py", "c2_proposal.py")]


def test_only_the_single_guarded_entrance_can_reach_data_acquisition():
    """Acquisition modules/calls exist ONLY in c2_data.py; the runner reaches c2_data lazily, after the freeze gate."""
    for f in _census02_sources():
        tree = ast.parse(f.read_text(encoding="utf-8"))
        for n in ast.walk(tree):
            mods = [a.name for a in n.names] if isinstance(n, ast.Import) else [n.module or ""] if isinstance(n, ast.ImportFrom) else []
            for m in mods:
                parts = set(m.split("."))
                assert not parts & NETWORK_MODULES, f"{f.name} imports {m}"
                if f.name != "c2_data.py":
                    assert not parts & ACQUISITION_MODULES, f"{f.name} imports {m}: only c2_data.py is the acquisition entrance"
            if isinstance(n, ast.Call) and f.name != "c2_data.py":
                name = n.func.attr if isinstance(n.func, ast.Attribute) else getattr(n.func, "id", "")
                allowed = {"load_discovery_universe"} if f.name == "c2_runner.py" else set()   # the one guarded, lazy entrance
                assert name not in ACQUISITION_CALLS - allowed, f"{f.name} calls {name}"
    # c2_data is imported by exactly one place, inside run_campaign, and never at module level
    importers = set()
    for f in _census02_sources():
        for n in ast.walk(ast.parse(f.read_text(encoding="utf-8"))):
            if isinstance(n, ast.Import) and any(a.name == "c2_data" for a in n.names) or \
                    isinstance(n, ast.ImportFrom) and n.module == "c2_data":
                importers.add(f.name)
    assert importers == {"c2_runner.py"}
    tree = ast.parse((EXP2 / "c2_runner.py").read_text(encoding="utf-8"))
    for node in tree.body:
        if isinstance(node, (ast.Import, ast.ImportFrom)):
            assert "c2_data" not in ast.dump(node), "c2_runner must not import c2_data at module level"
    run = next(n for n in tree.body if isinstance(n, ast.FunctionDef) and n.name == "run_campaign")
    first_gate = min(n.lineno for n in ast.walk(run) if isinstance(n, ast.Call) and getattr(n.func, "attr", "") == "require_freeze")
    lazy_import = min(n.lineno for n in ast.walk(run) if isinstance(n, ast.Import) and any(a.name == "c2_data" for a in n.names))
    loader_call = min(n.lineno for n in ast.walk(run) if isinstance(n, ast.Call) and getattr(n.func, "id", "") == "loader")
    assert first_gate < lazy_import < loader_call


def test_no_dynamic_import_in_any_bound_source_beyond_the_reviewed_stdlib_one():
    for rel in pr.BEHAVIOR_SOURCES:
        if not rel.endswith(".py"):
            continue
        for n in ast.walk(ast.parse((pr.REPO / rel).read_text(encoding="utf-8"))):
            if isinstance(n, ast.Call):
                is_attr = isinstance(n.func, ast.Attribute)
                name = n.func.attr if is_attr else getattr(n.func, "id", "")
                # re.compile / pattern methods are attributes; the builtin exec/eval/compile/__import__ are bare names
                if (is_attr and name == "import_module") or (not is_attr and name in ("__import__", "exec", "eval", "compile")):
                    ok = name == "__import__" and n.args and isinstance(n.args[0], ast.Constant) and n.args[0].value == "contextlib"
                    assert ok, f"{rel}:{n.lineno} dynamic import / code execution via {name}"
            if isinstance(n, (ast.Import, ast.ImportFrom)):
                mods = [a.name for a in n.names] if isinstance(n, ast.Import) else [n.module or ""]
                assert not any(m.split(".")[0] == "importlib" for m in mods), f"{rel} imports importlib"


# ============================================================================== behavior-source manifest (D5)
def _reachable(roots):
    """In-repo import closure including relative imports, lazy in-function imports and package __init__ chains."""
    rp = pr.REPO / "research-py"
    search = [rp / "experiments" / "alpha_edge_census_01", rp / "experiments" / "alpha_edge_census_02", rp / "src"]

    def mod_files(mod):
        parts = mod.split(".")
        for base in search:
            p = base.joinpath(*parts)
            f = p.with_suffix(".py") if p.with_suffix(".py").exists() else (p / "__init__.py" if (p / "__init__.py").exists() else None)
            if f:
                return [f] + [base.joinpath(*parts[:i]) / "__init__.py" for i in range(1, len(parts))
                              if (base.joinpath(*parts[:i]) / "__init__.py").exists()]
        return []

    def pkg(f):
        for base in search:
            try:
                return list(f.relative_to(base).parts[:-1])
            except ValueError:
                continue
        return []

    def imports(f):
        for n in ast.walk(ast.parse(f.read_text(encoding="utf-8"))):
            if isinstance(n, ast.Import):
                yield from (a.name for a in n.names)
            elif isinstance(n, ast.ImportFrom):
                if n.level == 0 and n.module:
                    yield n.module
                    yield from (f"{n.module}.{a.name}" for a in n.names)
                elif n.level > 0:
                    base = pkg(f)
                    base = base[:len(base) - (n.level - 1)] if n.level > 1 else base
                    pre = ".".join(base)
                    mod = ".".join(x for x in (pre, n.module) if x)
                    if n.module:
                        yield mod
                    yield from (".".join(x for x in (mod, a.name) if x) for a in n.names)

    seen, stack = set(), list(roots)
    while stack:
        f = stack.pop()
        if f in seen:
            continue
        seen.add(f)
        for m in imports(f):
            stack.extend(mod_files(m))
    return {str(p.relative_to(pr.REPO)).replace("\\", "/") for p in seen}


def test_manifest_covers_every_reachable_behavior_source_or_documents_why_not():
    """Every in-repo module reachable by import (static, relative, lazy, package-init) from the Census-02 modules is bound or
    carries a reviewed reason. Supporting proof only: dynamic imports are separately forbidden."""
    roots = [pr.REPO / r for r in pr.BEHAVIOR_SOURCES if "alpha_edge_census_02/" in r]
    reachable = _reachable(roots)
    bound = set(pr.BEHAVIOR_SOURCES)
    unaccounted = sorted(reachable - bound - set(pr.REVIEWED_NON_BEHAVIOR))
    assert not unaccounted, f"behavior source omitted from the manifest (or never reviewed): {unaccounted}"
    assert all(pr.REVIEWED_NON_BEHAVIOR.values()) and not bound & set(pr.REVIEWED_NON_BEHAVIOR)
    assert all((pr.REPO / r).is_file() for r in bound | set(pr.AUTHORITY_DATA))
    own = {f"research-py/experiments/alpha_edge_census_02/{f.name}" for f in EXP2.glob("*.py")}
    assert own - {"research-py/experiments/alpha_edge_census_02/c2_proposal.py",
                  "research-py/experiments/alpha_edge_census_02/c2_mutation_proof.py"} <= bound, "a Census-02 module is unbound"
    assert "research-py/pyproject.toml" in bound                                       # dependency declaration is behavior authority
    assert not any("results" in r or "PROPOSAL" in r or "runs/" in r or "PREDECLARATION" in r for r in bound | set(pr.AUTHORITY_DATA))


@pytest.mark.parametrize("rel", pr.BEHAVIOR_SOURCES + pr.AUTHORITY_DATA)
def test_freeze_guard_reds_on_one_byte_of_drift_in_every_bound_source(repo, rel, monkeypatch):
    """Grammar, signal, simulator, borrow, factor, partition, calendar and indicator sources: one flipped byte after the
    freeze commit must refuse real-data access, whether committed or merely dirty."""
    import c2_population
    real = c2_population.population_authority
    memo: dict = {}                                                                   # speed only: same inputs -> same output
    monkeypatch.setattr(c2_population, "population_authority",
                        lambda d, pid: memo.setdefault((json.dumps(d, sort_keys=True), pid), real(d, pid)))
    f = _write_commit(repo, _frozen_doc(repo))
    assert pr.require_freeze(repo, f)                                                # positive control, unmutated
    target = repo / rel
    original = target.read_bytes()
    target.write_bytes(original + b"\n#")
    with pytest.raises(pr.FreezeRefusal):                                           # dirty vs HEAD
        pr.require_freeze(repo, f)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "drift")
    with pytest.raises(pr.FreezeRefusal, match="differs from the frozen manifest"):  # committed but unbound
        pr.require_freeze(repo, f)
    target.write_bytes(original)
    _git(repo, "add", "-A")
    _git(repo, "commit", "-qm", "restore")
    assert pr.require_freeze(repo, f)                                                # restored byte-for-byte -> GREEN


def test_manifest_hash_is_line_ending_normalised_and_binds_the_protocol_id(repo):
    m = pr.behavior_source_manifest(repo)
    p = repo / pr.BEHAVIOR_SOURCES[0]
    p.write_bytes(p.read_bytes().replace(b"\n", b"\r\n"))
    assert pr.behavior_source_manifest(repo) == m
    e = env.environment_identity()
    assert pr.frozen_protocol_id(pr.build_structural_protocol(), GOOD_DECISIONS, m, e) != pr.frozen_protocol_id(
        pr.build_structural_protocol(), GOOD_DECISIONS, {**m, "sources": {**m["sources"], pr.BEHAVIOR_SOURCES[0]: "0" * 64}}, e)
    with pytest.raises(pr.FreezeRefusal):
        pr.behavior_source_manifest(repo / "nowhere")
