"""Confirmation invariants (synthetic only; no Confirmation data is read): protocol pins, cohort rule, result-independent
identity, runtime freeze guard, fenced loader/transport, label fence, estimator, decision/BH rules, ranking isolation and
durable attempt semantics."""

from __future__ import annotations

import copy
import json
import math
import os
import shutil
import subprocess
import sys
from pathlib import Path

import numpy as np
import pandas as pd
import pytest

TESTS = Path(__file__).resolve().parent
EXPC = TESTS.parent / "experiments" / "alpha_edge_confirmation_01"
EXP1 = TESTS.parent / "experiments" / "alpha_edge_census_01"
for _p in (str(TESTS), str(EXPC), str(EXP1)):
    sys.path.insert(0, _p)

import c1_cohort as cc  # noqa: E402
import c1_data as cd  # noqa: E402
import c1_eval as ev  # noqa: E402
import c1_protocol as cp  # noqa: E402
import c1_runner as cr  # noqa: E402
import conditional as cn  # noqa: E402
import signals as sg  # noqa: E402
from mqk_research.exp_distributed.storage import ResearchResultStore  # noqa: E402
from mqk_research.factors.fdr import benjamini_hochberg  # noqa: E402
from partitions import PartitionBreach  # noqa: E402

PINNED_PROTOCOL_ID = "50d18700c997c477ec1e60de8185c997f7fedfeee6a42dfe5d73e4a627937557"


@pytest.fixture(scope="module")
def entries():
    return cc.derive_cohort()


@pytest.fixture(scope="module")
def manifest(entries):
    return cc.cohort_manifest(entries)


@pytest.fixture(scope="module")
def cands(manifest):
    return manifest["candidates"]


def synth(symbol: str, seed: int, *, phi=-0.15, mu=0.0003, sig=0.012, price0=50.0, sessions=None) -> pd.DataFrame:
    sessions = sessions if sessions is not None else ev.SESSIONS
    rng = np.random.default_rng(seed)
    n = len(sessions)
    r = np.zeros(n)
    eps = rng.standard_normal(n)
    for i in range(1, n):
        r[i] = mu + phi * r[i - 1] + sig * eps[i]
    close = np.round(price0 * np.exp(np.cumsum(r)), 2)
    open_ = np.round(np.concatenate([[close[0]], close[:-1]]) * (1 + 0.002 * rng.standard_normal(n)), 2)
    hi = np.maximum(np.round(np.maximum(open_, close) * (1 + np.abs(rng.standard_normal(n)) * 0.004), 2), np.maximum(open_, close))
    lo = np.minimum(np.round(np.minimum(open_, close) * (1 - np.abs(rng.standard_normal(n)) * 0.004), 2), np.minimum(open_, close))
    ts = [pd.Timestamp(d, tz="UTC") + pd.Timedelta(hours=5) for d in sessions]
    return pd.DataFrame({"symbol": symbol, "end_ts": ts, "open": open_, "high": hi, "low": lo, "close": close,
                         "volume": np.round(1e6 * (1 + rng.random(n)), 0)})


def synth_universe(n_sym=14):
    bars = {"SPY": synth("SPY", 1, phi=0.0, mu=0.0004, sig=0.009, price0=200.0)}
    for i in range(n_sym - 1):
        bars[f"X{i:02d}"] = synth(f"X{i:02d}", 100 + i, phi=-0.25 if i % 2 else -0.1, sig=0.02)
    return ev.ConfUniverse(bars)


@pytest.fixture(scope="module")
def U():
    return synth_universe()


# ------------------------------------------------------------------------------------------------ protocol / cohort

def test_protocol_thresholds_are_the_predeclared_numbers():
    assert (cp.MIN_EVENTS, cp.N_PERMUTATIONS, cp.BASE_SEED, cp.FDR_ALPHA, cp.P_STRONG_MAX) == (30, 200, 0, 0.10, 0.10)
    assert (cp.WARMUP_START, cp.SCORE_START, cp.SCORE_END_EXCLUSIVE) == ("2024-01-01", "2025-01-01", "2026-03-01")
    assert cp.EXPECTED_COHORT == 18 and cp.EXPECTED_FAMILY_COUNTS == {"S05": 4, "S06": 12, "S13": 2}
    assert cp.LABELS["VALIDATION_STATUS"] == "NOT_VALIDATED" and cp.LABELS["PROMOTION_AUTHORITY"] == "NONE"
    assert cp.LABELS["EXECUTABLE_PNL"] is False
    assert cp.REQUEST_CONTRACT["feed"] == "sip" and cp.REQUEST_CONTRACT["adjustment"] == "all"
    assert cp.REQUEST_CONTRACT["end_utc_exclusive"] == "2026-03-01T00:00:00+00:00"


@pytest.mark.parametrize("path,val", [(("estimator", "min_events"), 29), (("null", "n_permutations"), 199),
                                      (("null", "base_seed"), 1), (("fdr", "alpha"), 0.05),
                                      (("fdr", "winner_only_fdr"), True), (("cohort", "expected_count"), 19)])
def test_every_threshold_is_bound_into_the_protocol_id(path, val):
    doc = cp.build_protocol()
    node = doc
    for k in path[:-1]:
        node = node[k]
    node[path[-1]] = val
    assert cp.protocol_id(doc) != cp.CONFIRMATION_PROTOCOL_ID


def test_protocol_id_is_pinned():
    assert cp.CONFIRMATION_PROTOCOL_ID == PINNED_PROTOCOL_ID


def test_cohort_is_exactly_the_18_committed_pass2_survivors(entries):
    assert len(entries) == 18 and [e["factor_id"] for e in entries] == sorted(e["factor_id"] for e in entries)
    fams = {}
    for e in entries:
        fams[e["family"]] = fams.get(e["family"], 0) + 1
        assert e["direction"] == "higher_is_better" and e["horizon"] in cp.HORIZONS
    assert dict(sorted(fams.items())) == cp.EXPECTED_FAMILY_COUNTS
    assert all(e["discovery_diagnostics"]["effect"] > 0 for e in entries)


def _tampered_pass2(tmp_path, mutate):
    d = tmp_path / "p2"
    shutil.copytree(cc.PASS2_RESULTS, d)
    ledger = d / "conditional_robustness_ledger.jsonl"
    rows = list(cc.iter_jsonl(ledger))
    mutate(rows)
    ledger.write_text("".join(json.dumps(r, sort_keys=True) + "\n" for r in rows), encoding="utf-8", newline="\n")
    disp = json.loads((d / "PASS2_PROCESS_DISPOSITION.json").read_text(encoding="utf-8"))
    disp["bound_artifacts_sha256_lf"]["conditional_robustness_ledger.jsonl"] = cc.sha_lf(ledger)
    (d / "PASS2_PROCESS_DISPOSITION.json").write_text(json.dumps(disp), encoding="utf-8")
    for name in disp["bound_artifacts_sha256_lf"]:
        disp["bound_artifacts_sha256_lf"][name] = cc.sha_lf(d / name)
    (d / "PASS2_PROCESS_DISPOSITION.json").write_text(json.dumps(disp), encoding="utf-8")
    return d


def test_cohort_not_18_is_refused_dropping_or_adding_a_rejected_factor(tmp_path):
    def drop(rows):
        next(r for r in rows if r["verdict"] == cp.PASS2_SURVIVOR_VERDICT)["verdict"] = "PASS2_CONDITIONAL_REJECTED"

    def add(rows):
        next(r for r in rows if r["verdict"] != cp.PASS2_SURVIVOR_VERDICT)["verdict"] = cp.PASS2_SURVIVOR_VERDICT

    for i, mut in enumerate((drop, add)):
        with pytest.raises(cc.FreezeRefusal):
            cc.derive_cohort(_tampered_pass2(tmp_path / str(i), mut))


def test_pass2_artifact_tamper_without_rebinding_is_refused(tmp_path):
    d = tmp_path / "p2"
    shutil.copytree(cc.PASS2_RESULTS, d)
    with open(d / "conditional_robustness_ledger.jsonl", "a", encoding="utf-8") as f:
        f.write("\n")
        f.write(json.dumps({"x": 1}) + "\n")
    with pytest.raises(cc.FreezeRefusal):
        cc.derive_cohort(d)


def test_evaluation_identity_is_result_independent_and_bound(entries, manifest):
    fam, prov = manifest["fdr_family_identity"], manifest["data_provenance_identity"]
    ids = [c["evaluation_id"] for c in manifest["candidates"]]
    assert len(set(ids)) == 18 and all(len(i) == 32 for i in ids)
    fid = entries[0]["factor_id"]
    base = cc.evaluation_id(fid, fam, prov)
    assert base == ids[0] == cc.evaluation_id(fid, fam, prov)
    assert cc.evaluation_id(entries[1]["factor_id"], fam, prov) != base
    assert cc.evaluation_id(fid, "x" * 64, prov) != base and cc.evaluation_id(fid, fam, "y" * 64) != base
    assert cc.evaluation_id(fid, fam, prov, protocol_id="z" * 64) != base
    assert cc.fdr_family_identity([e["factor_id"] for e in entries][:17]) != fam
    # evaluation outputs never enter the cohort manifest
    stripped = copy.deepcopy(entries)
    stripped[0]["discovery_diagnostics"]["effect"] = 123.0
    assert cc.cohort_manifest(stripped, prov)["candidates"][0]["evaluation_id"] == base


# --------------------------------------------------------------------------------------------------- freeze guard

def _git(repo, *args):
    env = {**os.environ, "GIT_AUTHOR_NAME": "t", "GIT_AUTHOR_EMAIL": "t@t", "GIT_COMMITTER_NAME": "t", "GIT_COMMITTER_EMAIL": "t@t"}
    subprocess.run(["git", *args], cwd=repo, check=True, capture_output=True, env=env)


def _freeze_repo(tmp_path, entries, *, commit=True, write_files=True):
    repo = tmp_path / "repo"
    exp = repo / "exp"
    exp.mkdir(parents=True)
    _git(repo, "init", "-q")
    src = exp / "c1_dummy.py"
    src.write_text("VALUE = 1\n", encoding="utf-8", newline="\n")
    sources = ["exp/c1_dummy.py"]
    kw = {"repo": repo, "exp_dir": exp, "derive": lambda: entries, "sources": sources}
    if write_files:
        m = cc.cohort_manifest(entries)
        (exp / "CONFIRMATION_COHORT_MANIFEST.json").write_text(json.dumps(m, sort_keys=True), encoding="utf-8", newline="\n")
        (exp / "CONFIRMATION_PREDECLARATION.json").write_text(
            json.dumps(cc.predeclaration(m, starting_head="a" * 40, freeze_base_head="b" * 40), sort_keys=True), encoding="utf-8", newline="\n")
        proof = {"predeclaration_sha256_lf": cc.sha_lf(exp / "CONFIRMATION_PREDECLARATION.json"),
                 "cohort_manifest_sha256_lf": cc.sha_lf(exp / "CONFIRMATION_COHORT_MANIFEST.json"),
                 "source_sha256_lf": cc.source_sha256_lf(repo, sources)}
        (exp / "CONFIRMATION_FREEZE_PROOF.json").write_text(json.dumps(proof, sort_keys=True), encoding="utf-8", newline="\n")
    if commit:
        _git(repo, "add", "-A")
        _git(repo, "commit", "-q", "-m", "freeze")
    return kw, exp, src


def test_C01_guard_refuses_before_freeze_files_exist(tmp_path, entries):
    kw, *_ = _freeze_repo(tmp_path, entries, commit=False, write_files=False)
    with pytest.raises(cc.FreezeRefusal, match="does not exist"):
        cc.require_freeze(**kw)


def test_C01_guard_refuses_freeze_files_present_but_uncommitted(tmp_path, entries):
    kw, *_ = _freeze_repo(tmp_path, entries, commit=False)
    with pytest.raises(cc.FreezeRefusal, match="not committed"):
        cc.require_freeze(**kw)


def test_guard_passes_only_when_everything_is_committed_and_matches(tmp_path, entries):
    kw, *_ = _freeze_repo(tmp_path, entries)
    assert len(cc.require_freeze(**kw)["manifest"]["candidates"]) == 18


def test_guard_refuses_dirty_or_changed_bound_source(tmp_path, entries):
    kw, exp, src = _freeze_repo(tmp_path, entries)
    src.write_text("VALUE = 2\n", encoding="utf-8", newline="\n")
    with pytest.raises(cc.FreezeRefusal, match="differs from HEAD"):
        cc.require_freeze(**kw)
    _git(kw["repo"], "add", "-A")
    _git(kw["repo"], "commit", "-q", "-m", "edit after freeze")
    with pytest.raises(cc.FreezeRefusal, match="hash bound by the freeze proof"):
        cc.require_freeze(**kw)


def test_guard_refuses_manifest_or_cohort_tamper(tmp_path, entries):
    kw, exp, _ = _freeze_repo(tmp_path, entries)
    m = json.loads((exp / "CONFIRMATION_COHORT_MANIFEST.json").read_text(encoding="utf-8"))
    m["candidates"][0]["direction"] = "lower_is_better"
    (exp / "CONFIRMATION_COHORT_MANIFEST.json").write_text(json.dumps(m, sort_keys=True), encoding="utf-8", newline="\n")
    _git(kw["repo"], "add", "-A")
    _git(kw["repo"], "commit", "-q", "-m", "tamper")
    with pytest.raises(cc.FreezeRefusal):
        cc.require_freeze(**kw)


def test_guard_refuses_a_cohort_that_rederives_differently(tmp_path, entries):
    kw, *_ = _freeze_repo(tmp_path, entries)
    shifted = copy.deepcopy(entries)
    shifted[0]["horizon"] = 20 if shifted[0]["horizon"] != 20 else 10
    with pytest.raises(cc.FreezeRefusal, match="re-derived"):
        cc.require_freeze(**{**kw, "derive": lambda: shifted})


def test_guard_refuses_a_cohort_that_is_not_18(tmp_path, entries):
    kw, *_ = _freeze_repo(tmp_path, entries[:17])
    with pytest.raises(cc.FreezeRefusal, match="18"):
        cc.require_freeze(**kw)


def test_acquire_and_load_refuse_before_any_provider_call_pre_freeze(tmp_path, entries, monkeypatch):
    from mqk_research.data import alpaca_historical as ah
    calls = []
    monkeypatch.setattr(ah, "_default_http_get", lambda *a, **k: calls.append(a) or (200, b"{}"))
    kw, *_ = _freeze_repo(tmp_path, entries, commit=False, write_files=False)
    with pytest.raises(cc.FreezeRefusal):
        cd.acquire_symbol("SPY", tmp_path / "bars" / "SPY", freeze=kw)
    with pytest.raises(cc.FreezeRefusal):
        cd.load_symbol_bars(tmp_path / "bars" / "SPY", freeze=kw)
    with pytest.raises(cc.FreezeRefusal):
        cd.classify_symbol("SPY", {"disposition": "DATA_PRESENT"}, tmp_path / "bars" / "SPY", freeze=kw)
    assert calls == [] and not (tmp_path / "bars").exists()


# ---------------------------------------------------------------------------------------- fenced transport / loader

def _body(*ts):
    return json.dumps({"bars": {"SPY": [{"t": t, "o": 1, "h": 1, "l": 1, "c": 1, "v": 1} for t in ts]}}).encode()


def _transport(body):
    from mqk_research.data import alpaca_historical as ah
    inner = lambda url, params, headers: (200, body)  # noqa: E731
    return cd.GuardedTransport(inner), ah.BARS_PATH


def _params(start="2024-01-01T00:00:00+00:00", end="2026-03-01T00:00:00+00:00"):
    return {"start": start, "end": end}


def test_C10_transport_refuses_a_provider_row_at_or_after_the_fence():
    for t in ("2026-03-01T05:00:00Z", "2026-03-02T05:00:00Z"):
        g, path = _transport(_body("2026-02-27T05:00:00Z", t))
        with pytest.raises(PartitionBreach):
            g("https://x" + path, _params(), {})


def test_transport_passes_the_frozen_window_and_records_raw_max():
    g, path = _transport(_body("2024-01-02T05:00:00Z", "2026-02-27T05:00:00Z"))
    assert g("https://x" + path, _params(), {})[0] == 200
    assert g.raw_rows == 2 and g.raw_max_t == pd.Timestamp("2026-02-27T05:00:00Z")


@pytest.mark.parametrize("params", [_params(end="2026-03-02T00:00:00+00:00"), _params(end="2026-03-01T00:00:01+00:00"),
                                    _params(start="2023-12-31T00:00:00+00:00")])
def test_transport_refuses_any_request_window_other_than_the_frozen_one(params):
    g, path = _transport(_body("2024-01-02T05:00:00Z"))
    with pytest.raises(PartitionBreach):
        g("https://x" + path, params, {})
    assert g.bar_requests == 0


@pytest.mark.parametrize("ts", [["2026-03-01T05:00:00Z"], ["2023-12-29T05:00:00Z"], []])
def test_window_checker_refuses_fence_pre_window_and_empty(ts):
    with pytest.raises(PartitionBreach):
        cd.require_confirmation_window(pd.Series(ts, dtype="object"), what="t")


def test_window_checker_accepts_the_reserve_and_reports_bounds():
    b = cd.require_confirmation_window(["2024-01-02T05:00:00Z", "2026-02-27T05:00:00Z"], what="t")
    assert b["rows"] == 2 and b["max"].startswith("2026-02-27")


# ------------------------------------------------------------------------------------------------ label fence / eval

def test_C10_symbol_data_refuses_a_bar_at_or_after_the_fence():
    extra = synth("SPY", 1, sessions=ev.SESSIONS)
    bad = pd.concat([extra, extra.tail(1).assign(end_ts=pd.Timestamp("2026-03-02", tz="UTC") + pd.Timedelta(hours=5))], ignore_index=True)
    with pytest.raises(ev.LabelFenceBreach):
        ev.ConfSymbolData("SPY", bad)


def test_off_calendar_bar_fails_closed():
    b = synth("SPY", 1)
    b.loc[10, "end_ts"] = b.loc[10, "end_ts"] + pd.Timedelta(days=1) if False else pd.Timestamp("2024-01-06", tz="UTC") + pd.Timedelta(hours=5)
    with pytest.raises(RuntimeError):
        ev.ConfSymbolData("SPY", b)


def _always(horizon):
    return {"factor_id": "f", "condition_id": "c", "family": "S06", "params": {}, "horizon": horizon}


def test_C11_scored_frame_never_reaches_the_fence_for_every_horizon(U, entries):
    for h in cp.HORIZONS:
        e = next(x for x in entries if x["horizon"] == h) if any(x["horizon"] == h for x in entries) else None
        if e is None:
            continue
        frame, _aux = ev.build_conf_frame(U, e, h)
        if frame.empty:
            continue
        b = ev.assert_scored(frame)
        assert pd.Timestamp(b["max_label_end"]) < ev.FENCE_TS and pd.Timestamp(b["min_period"]) >= ev.SCORE_START_TS
        last_sessions = ev.SESSIONS[-h - 1:]
        assert pd.Timestamp(b["max_period"]).date() <= last_sessions[0]


def test_h20_last_scored_observation_is_20_sessions_before_the_last_bar(U):
    import datetime as dt
    cond = next(e for e in cc.derive_cohort() if e["horizon"] == 20) if any(e["horizon"] == 20 for e in cc.derive_cohort()) else None
    if cond is None:
        pytest.skip("no h=20 factor in cohort")
    frame, _ = ev.build_conf_frame(U, cond, 20)
    assert not frame.empty
    assert pd.to_datetime(frame["period_ts_utc"], utc=True).max().date() <= ev.SESSIONS[-21]
    assert pd.to_datetime(frame["label_end_ts_utc"], utc=True).max().date() <= ev.SESSIONS[-1] < dt.date(2026, 3, 1)


def test_C08_C11_assert_scored_refuses_warmup_rows_and_fence_endpoints():
    def mk(per, end):
        return pd.DataFrame({"period_ts_utc": [per], "label_end_ts_utc": [end]})
    ev.assert_scored(mk("2025-01-02T05:00:00+00:00", "2025-01-03T05:00:00+00:00"))
    for per, end in (("2024-12-31T05:00:00+00:00", "2025-01-03T05:00:00+00:00"),
                     ("2026-02-27T05:00:00+00:00", "2026-03-02T05:00:00+00:00"),
                     ("2026-03-01T05:00:00+00:00", "2026-03-02T05:00:00+00:00"),
                     ("2025-01-02T05:00:00+00:00", "2025-01-02T05:00:00+00:00")):
        with pytest.raises(ev.LabelFenceBreach):
            ev.assert_scored(mk(per, end))


def test_scored_frame_contains_no_warmup_rows_and_baseline_is_over_scored_rows(U, entries):
    e = entries[0]
    frame, aux = ev.build_conf_frame(U, e, e["horizon"])
    per = pd.to_datetime(frame["period_ts_utc"], utc=True)
    assert per.min() >= ev.SCORE_START_TS
    assert (frame.groupby("symbol")["label_fwd_ret"].mean().abs() < 1e-12).all()


def test_effect_and_events_match_an_independent_recomputation(U, cands):
    e = next(x for x in cands if cn.condition_sig(U, "X00", x) is not None)
    rec = ev.evaluate_factor(U, e)
    h, ev_n, num = e["horizon"], 0, 0.0
    per_sym = {}
    for s in U.symbols:
        sig = cn.condition_sig(U, s, e)
        if sig is None:
            continue
        sd = U.sd[s]
        first = int(np.searchsorted(sd.dates, np.datetime64("2025-01-01"), side="left"))
        idx = np.arange(max(sig.s, first), sd.n - h)
        idx = idx[sd.dates[idx + h] < np.datetime64("2026-03-01")]
        if not idx.size:
            continue
        ret = sd.c[idx + h] / sd.c[idx] - 1.0
        lab = ret - ret.mean()
        m = sig.cond[idx] == 1
        per_sym[s] = (int(m.sum()), float(lab[m].sum()))
    ev_n = sum(a for a, _ in per_sym.values())
    assert rec["events"] == ev_n
    if ev_n:
        assert rec["effect"] == pytest.approx(sum(b for _, b in per_sym.values()) / ev_n, rel=1e-9, abs=1e-12)


def test_evaluation_is_deterministic_and_p_uses_the_frozen_null(U, cands):
    e = next(x for x in cands if ev.evaluate_factor(U, x)["evaluable"])
    a, b = ev.evaluate_factor(U, e), ev.evaluate_factor(U, e)
    assert a == b and a["null"]["n_permutations"] == 200 and a["null"]["base_seed"] == 0
    assert 0.0 < a["p_value"] <= 1.0


def test_C13_below_the_event_floor_is_not_evaluable_not_not_confirmed():
    tiny = ev.ConfUniverse({"SPY": synth("SPY", 1, phi=0.0, mu=0.0004, sig=0.009, price0=200.0),
                            "A": synth("A", 5, sig=0.02), "B": synth("B", 6, sig=0.02)})
    for e in cc.derive_cohort():
        rec = ev.evaluate_factor(tiny, {**e, "evaluation_id": "e" * 32})
        if rec["events"] < cp.MIN_EVENTS:
            assert rec["evaluable"] is False and rec["reason"] == cp.REASON_INSUFFICIENT_EVENTS and rec["p_value"] is None
            break
    else:
        pytest.fail("no factor fell under the event floor on a 3-symbol universe")


def test_unsupported_family_refused_by_the_universe(U):
    with pytest.raises(ValueError):
        U.build("X00", "S01", {})


# ---------------------------------------------------------------------------------------------- decisions / FDR

@pytest.mark.parametrize("events,effect,p,q,want", [
    (30, 0.002, 0.10, 0.10, cp.CONFIRMED_STRONG),
    (30, 0.002, 0.0101, 0.0999, cp.CONFIRMED_STRONG),
    (29, 0.002, 0.001, 0.001, cp.NOT_EVALUABLE),
    (30, 0.002, 0.1001, 0.05, cp.CONFIRMED_DIRECTIONAL_ONLY),
    (30, 0.002, 0.05, 0.1001, cp.CONFIRMED_DIRECTIONAL_ONLY),
    (500, 0.0, 0.001, 0.001, cp.NOT_CONFIRMED),
    (500, -0.002, 0.001, 0.001, cp.NOT_CONFIRMED),
    (500, float("nan"), 0.001, 0.001, cp.NOT_EVALUABLE),
    (500, None, 0.001, 0.001, cp.NOT_EVALUABLE)])
def test_decision_rule_table(events, effect, p, q, want):
    assert ev.decide(events, effect, p, q)[0] == want


def test_decision_rule_refuses_an_evaluable_row_without_p_or_q():
    for p, q in ((None, 0.1), (0.1, None), (float("nan"), 0.1)):
        with pytest.raises(ValueError):
            ev.decide(100, 0.01, p, q)


def _fake_evidence(cands, *, p_of, effect_of=lambda i: 0.002, events_of=lambda i: 100, evaluable_of=lambda i: True):
    out = {}
    for i, c in enumerate(cands):
        okv = evaluable_of(i)
        out[c["factor_id"]] = {"factor_id": c["factor_id"], "evaluation_id": c["evaluation_id"], "family": c["family"],
                               "horizon": c["horizon"], "events": events_of(i), "effect": effect_of(i) if okv else None,
                               "p_value": p_of(i) if okv else None, "evaluable": okv,
                               "reason": None if okv else cp.REASON_INSUFFICIENT_EVENTS}
    return out


def test_C04_C16_bh_denominator_is_all_18_including_non_evaluables(entries, cands):
    evid = _fake_evidence(cands, p_of=lambda i: 0.001 + 0.002 * i, evaluable_of=lambda i: i < 6)
    fin = ev.finalize(entries, evid)
    assert fin["fdr"]["hypothesis_count"] == 18 and len(fin["rows"]) == 18
    pmap = {c["factor_id"]: (0.001 + 0.002 * i if i < 6 else 1.0) for i, c in enumerate(cands)}
    assert fin["fdr"]["q_values"] == benjamini_hochberg(pmap, alpha=0.10)["q_values"]
    winners = benjamini_hochberg({k: v for k, v in pmap.items() if v < 1.0}, alpha=0.10)["q_values"]
    assert any(fin["fdr"]["q_values"][k] != winners[k] for k in winners)  # winner-only BH would give different q
    assert sum(r["status"] == cp.NOT_EVALUABLE for r in fin["rows"]) == 12


def test_finalize_refuses_incomplete_or_extra_evidence(entries, cands):
    evid = _fake_evidence(cands, p_of=lambda i: 0.5)
    short = {k: v for k, v in list(evid.items())[:17]}
    with pytest.raises(ValueError):
        ev.finalize(entries, short)
    with pytest.raises(ValueError):
        ev.finalize(entries, {**evid, "extra": evid[cands[0]["factor_id"]]})


def test_C17_C18_C19_strong_requires_p_q_and_positive_effect(entries, cands):
    base = dict(p_of=lambda i: 0.0005 * (i + 1))
    fin = ev.finalize(entries, _fake_evidence(cands, **base))
    assert all(r["status"] == cp.CONFIRMED_STRONG for r in fin["rows"])
    fin = ev.finalize(entries, _fake_evidence(cands, p_of=lambda i: 0.5))
    assert all(r["status"] == cp.CONFIRMED_DIRECTIONAL_ONLY for r in fin["rows"])
    fin = ev.finalize(entries, _fake_evidence(cands, **base, effect_of=lambda i: -0.001 if i == 0 else 0.002))
    assert [r["status"] for r in fin["rows"]].count(cp.NOT_CONFIRMED) == 1
    # p just above the strong threshold but q small cannot be STRONG
    fin = ev.finalize(entries, _fake_evidence(cands, p_of=lambda i: 0.11 if i == 0 else 0.0001 * (i + 1)))
    assert fin["rows"][0]["status"] == cp.CONFIRMED_DIRECTIONAL_ONLY


def test_C25_ranking_is_read_only_and_never_changes_status(entries, cands):
    fin = ev.finalize(entries, _fake_evidence(cands, p_of=lambda i: 0.002 * (i + 1) if i < 9 else 0.4))
    rows = fin["rows"]
    snap = copy.deepcopy(rows)
    rk = ev.rankings(rows)
    assert rows == snap and rk["read_only"] is True and rk["alters_status_or_identity"] is False
    strong = rk["confirmed_strong"]
    assert all(r["status"] == cp.CONFIRMED_STRONG for r in strong)
    keys = [(r["q_value"], r["p_value"], -r["effect"], -r["events"], r["factor_id"]) for r in strong]
    assert keys == sorted(keys)
    assert all(r["status"] == cp.CONFIRMED_DIRECTIONAL_ONLY for r in rk["confirmed_directional_only_separate_table"])
    assert ev.rankings(list(reversed(rows))) == rk


def test_C23_C24_label_contract_refuses_promotion_validation_pnl_and_holdout_claims():
    ev.assert_labels({**cp.LABELS})
    for bad in ({"VALIDATION_STATUS": "VALIDATED"}, {"PROMOTION_AUTHORITY": "PAPER"}, {"EXECUTABLE_PNL": True},
                {"promotion": "CLAIMED"}, {"final_holdout": "CONSUMED"}):
        with pytest.raises(ValueError):
            ev.assert_labels(bad)


def test_C12_record_without_the_no_pnl_label_is_refused(cands):
    rec = {"factor_id": cands[0]["factor_id"], **cp.LABELS}
    cr.assert_record_labels(rec)
    for k, v in (("EXECUTABLE_PNL", True), ("PROMOTION_AUTHORITY", "PAPER"), ("VALIDATION_STATUS", "VALIDATED")):
        with pytest.raises(cr.ConfirmationRefusal):
            cr.assert_record_labels({**rec, k: v})
    with pytest.raises(cr.ConfirmationRefusal):
        cr.assert_record_labels({"factor_id": "x"})


def test_summary_counts_every_row_once(entries, cands):
    fin = ev.finalize(entries, _fake_evidence(cands, p_of=lambda i: 0.002 * (i + 1) if i < 9 else 0.4))
    for r in fin["rows"]:
        r["discovery"]["regime_class"] = r["discovery"].get("regime_class")
    s = ev.summarize(fin["rows"])
    assert s["denominator"] == 18 and sum(s["by_status"].values()) == 18
    assert sum(sum(v.values()) for v in s["by_family"].values()) == 18
    assert sum(sum(v.values()) for v in s["by_horizon"].values()) == 18


# ------------------------------------------------------------------------------------------- durable attempts

@pytest.fixture()
def store(tmp_path, manifest):
    s = ResearchResultStore(tmp_path / "reg.sqlite")
    cr.register(s, manifest)
    return s


def _good(c):
    return {"factor_id": c["factor_id"], "evaluation_id": c["evaluation_id"], "events": 40, **cp.LABELS}


def test_freeze_marker_gate_and_registration(store, manifest):
    st = cr.require_frozen(store, manifest, allow_attempts=False)
    assert (st["registered"], st["attempts"]) == (18, 0)


def test_require_frozen_refuses_missing_marker_and_extra_trials(tmp_path, manifest):
    s = ResearchResultStore(tmp_path / "x.sqlite")
    with pytest.raises(cr.ConfirmationRefusal):
        cr.require_frozen(s, manifest, allow_attempts=True)
    cr.register(s, manifest)
    s.register_hypothesis(hypothesis_id="other", experiment_id=cp.EXPERIMENT_ID)
    s.register_trials_bulk([{"trial_id": "c1-extra", "experiment_id": cp.EXPERIMENT_ID, "hypothesis_id": "other",
                             "strategy_id": "s", "protocol_id": "p", "identity": {"k": 1}}])
    with pytest.raises(cr.ConfirmationRefusal, match="extra=1"):
        cr.require_frozen(s, manifest, allow_attempts=True)


def test_attempt_is_opened_before_evaluation_and_second_run_creates_nothing(store, cands):
    seen = []

    def fn(c):
        att = store.list_attempts(cr.cc.trial_id_of(c["evaluation_id"]))
        seen.append([a["status"] for a in att])
        return _good(c)

    a = cr.execute(store, cands, fn, log=lambda *_: None)
    assert a["executed"] == 18 and all(s == ["started"] for s in seen)
    d1 = store.trial_attempt_digest(cp.EXPERIMENT_ID)
    b = cr.execute(store, cands, lambda c: pytest.fail("terminal evaluation re-executed"), log=lambda *_: None)
    assert b["executed"] == 0 and b["terminal_before"] == 18
    assert store.trial_attempt_digest(cp.EXPERIMENT_ID) == d1
    assert all(v["attempts"] == 1 and v["succeeded"] == 1 for v in d1.values())
    assert [r["factor_id"] for r in cr.collect(store, cands)] == [c["factor_id"] for c in cands]


def test_C20_statistical_failure_is_terminal_and_never_retried(store, cands):
    def boom(c):
        raise ValueError("statistical failure")
    with pytest.raises(cr.ConfirmationDefect):
        cr.execute(store, cands, boom, batch=6, log=lambda *_: None)
    d = store.trial_attempt_digest(cp.EXPERIMENT_ID)
    assert sum(v["failed"] for v in d.values()) == 6 and sum(v["succeeded"] for v in d.values()) == 0
    failed = [a for c in cands[:6] for a in store.list_attempts(cr.cc.trial_id_of(c["evaluation_id"]))]
    assert sum(a["failure_reason"] == cp.INTERRUPTED_REASON for a in failed) == 5
    with pytest.raises(cr.ConfirmationRefusal, match="never retried"):
        cr.execute(store, cands, _good, log=lambda *_: None)
    assert store.trial_attempt_digest(cp.EXPERIMENT_ID) == d


def test_only_infrastructure_interruption_is_retried(store, cands):
    tid = cr.cc.trial_id_of(cands[0]["evaluation_id"])
    store.begin_attempts_bulk([tid], origin=cp.ORIGIN)  # a stale 'started' attempt from a dead controller
    out = cr.execute(store, cands, _good, log=lambda *_: None)
    atts = store.list_attempts(tid)
    assert out["executed"] == 18 and [a["status"] for a in atts] == ["failed", "succeeded"]
    assert atts[0]["failure_reason"] == cp.INTERRUPTED_REASON


def test_C21_foreign_evaluation_id_fails_closed_before_mutation(store, cands):
    foreign = [{**cands[0], "evaluation_id": "f" * 32}] + cands[1:]
    d0 = store.trial_attempt_digest(cp.EXPERIMENT_ID)
    with pytest.raises(cr.ConfirmationRefusal, match="not registered"):
        cr.execute(store, foreign, _good, log=lambda *_: None)
    assert store.trial_attempt_digest(cp.EXPERIMENT_ID) == d0


def test_C21_persisted_record_with_a_foreign_evaluation_id_refuses(store, cands):
    cr.execute(store, cands[:1], lambda c: {**_good(c), "evaluation_id": "f" * 32}, log=lambda *_: None)
    with pytest.raises(cr.ConfirmationRefusal, match="foreign evaluation_id"):
        cr.execute(store, cands, _good, log=lambda *_: None)


def test_C22_terminal_succeeded_attempt_is_not_rerun_even_if_the_function_would_differ(store, cands):
    cr.execute(store, cands, _good, log=lambda *_: None)
    before = cr.collect(store, cands)
    cr.execute(store, cands, lambda c: {**_good(c), "events": 9999}, log=lambda *_: None)
    assert cr.collect(store, cands) == before


def test_record_failing_the_label_contract_is_a_durable_defect(store, cands):
    with pytest.raises(cr.ConfirmationDefect):
        cr.execute(store, cands[:2], lambda c: {**_good(c), "EXECUTABLE_PNL": True}, log=lambda *_: None)
    assert store.trial_attempt_digest(cp.EXPERIMENT_ID)[cr.cc.trial_id_of(cands[0]["evaluation_id"])]["failed"] == 1


def test_one_controller_owns_the_registry(tmp_path, monkeypatch):
    monkeypatch.setattr(cr, "RUN", tmp_path / "run")
    with cr.exclusive_owner("evaluate"):
        with pytest.raises(cr.ConfirmationRefusal, match="another"):
            with cr.exclusive_owner("evaluate"):
                pass
    with cr.exclusive_owner("evaluate"):
        pass


def test_confirmation_store_is_separate_from_pass1_and_pass2_registries():
    assert cr.STORE_DB.name == "registry_confirmation.sqlite" and cr.RUN.name == cp.EXPERIMENT_ID
    for other in ("alpha_edge_pass2_01", "alpha_edge_census_01"):
        assert other not in str(cr.STORE_DB)


def test_write_immutable_refuses_a_differing_regeneration(tmp_path):
    p = tmp_path / "d.json"
    cr.write_immutable(p, {"a": 1}, "doc")
    cr.write_immutable(p, {"a": 1}, "doc")
    with pytest.raises(cr.ConfirmationRefusal):
        cr.write_immutable(p, {"a": 2}, "doc")
    cr.write_immutable(p, {"a": 1, "b": 2}, "doc", ignore=("b",))
